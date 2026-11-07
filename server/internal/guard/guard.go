// Package guard 是最后一道闸：当请求量或资源占用已经不可能来自正常玩家时，
// 把服务自己掐掉。
//
// 它和 limit 的分工是这样：
//
//	limit  = 掐个别账号 / 个别 IP，阈值可以低，因为只影响那一个人
//	guard  = 大家都别玩，阈值必须极高，只有「这量级不可能是真人」时才触发
//
// 为什么要有它：应用层的限流在洪水面前是零。真到每分钟上亿次那种量级
// 打的时候，Go 的计数器、map、日志本身就会先成为负担。这时候唯一有用的动作是
// 不再接受连接 —— 进程退出后端口无人监听，内核直接回 RST，用户态零开销。
//
// 判定只看三件事，全部是「客观到不需要调参」的信号：
//
//  1. 建连速率：正常全站几百人在线，峰值几十条/秒；阈值按 10 倍余量给。
//  2. 内存占用：Go 进程正常几十 MB，涨到上限说明有东西在攒（map 爆、连接泄漏）。
//  3. 文件句柄：句柄耗尽前一定先到 80%，这是最早、最准的预警。
//
// 三档动作：不触发 / 只停桥接（玩家还能登录看到「联机维护中」）/
// 整个进程退出（systemd 按 RestartPreventExitStatus 不再拉起，交给看门狗）。
package guard

import (
	"os"
	"runtime"
	"sync"
	"time"
)

// Trip 是判定结果。
type Trip int

const (
	// TripNone 一切正常。
	TripNone Trip = 0
	// TripBridge 只停联机入口，HTTP 继续 —— 玩家至少还能登进来看公告。
	TripBridge Trip = 1
	// TripExit 整个进程退出。只有「再不停就要崩」时才到这一档。
	TripExit Trip = 2
)

func (t Trip) String() string {
	switch t {
	case TripBridge:
		return "只停联机"
	case TripExit:
		return "整个停服"
	default:
		return "正常"
	}
}

// ExitCode 是主动熔断退出时用的退出码。
//
// 必须和 systemd 的 RestartPreventExitStatus 对上，否则会被立刻拉起来，
// 变成「退出 → 重启 → 又被打死」的死循环，比扛着还糟。
const ExitCode = 75

// Config 是阈值。全部有保守默认值：宁可触发得晚，也不要误伤正常玩家。
type Config struct {
	// MaxAcceptsPerSec 是每秒新建连接数的上限。0 表示用默认值。
	MaxAcceptsPerSec float64
	// MemLimit 是进程占用上限（字节）。0 表示用默认值。
	MemLimit uint64
	// FDRatio 是「已用句柄 / 上限」的警戒比例，0 表示用默认值。
	FDRatio float64
	// Sustain 是「超阈值要持续多久才动手」，避免瞬时毛刺就停服。
	Sustain time.Duration
}

const (
	defaultAcceptsPerSec = 500
	defaultMemLimit      = 1536 << 20 // 1.5 GB：Go 服务正常几十 MB，到这个数一定是攒住了东西
	defaultFDRatio       = 0.8
	defaultSustain       = 10 * time.Second
	// window 是速率采样的秒数，和 Sustain 配合：内部按滑动窗口算平均，
	// 这样「持续 10 秒都超」和「一瞬间冲高」能区分开。
	window = 10
)

// Monitor 采样并给出判定。并发安全。
type Monitor struct {
	cfg Config

	mu       sync.Mutex
	buckets  [window]uint32
	lastSec  int64
	overFrom time.Time // 第一次连续超阈值的时间，零值表示当前没超
}

// New 构造监控器。
func New(cfg Config) *Monitor {
	if cfg.MaxAcceptsPerSec <= 0 {
		cfg.MaxAcceptsPerSec = defaultAcceptsPerSec
	}
	if cfg.MemLimit == 0 {
		cfg.MemLimit = defaultMemLimit
	}
	if cfg.FDRatio <= 0 {
		cfg.FDRatio = defaultFDRatio
	}
	if cfg.Sustain <= 0 {
		cfg.Sustain = defaultSustain
	}
	return &Monitor{cfg: cfg}
}

// OnAccept 在每次接受连接时调用。
func (m *Monitor) OnAccept(now time.Time) {
	sec := now.Unix()
	m.mu.Lock()
	defer m.mu.Unlock()
	m.rollLocked(sec)
	if m.buckets[sec%window] < 1<<31 {
		m.buckets[sec%window]++
	}
}

// rollLocked 按当前秒推进滑动窗口：把 (lastSec, sec] 之间跳过的桶清零。
//
// Evaluate 也要调它 —— 洪水一停，最后那几秒的桶如果没人推进，
// 会一直留在窗口里，导致「流量已经下来了但还在熔断」。
func (m *Monitor) rollLocked(sec int64) {
	if m.lastSec == sec {
		return
	}
	if m.lastSec == 0 || sec <= m.lastSec {
		for i := range m.buckets {
			m.buckets[i] = 0
		}
		m.lastSec = sec
		return
	}
	gap := sec - m.lastSec
	if gap >= window {
		for i := range m.buckets {
			m.buckets[i] = 0
		}
	} else {
		for i := int64(1); i <= gap; i++ {
			m.buckets[(m.lastSec+i)%window] = 0
		}
	}
	m.lastSec = sec
}

// Evaluate 评估当前状态。每秒调一次即可。
func (m *Monitor) Evaluate(now time.Time) (Trip, string) {
	// 资源类信号优先级最高：它们不依赖流量统计，看到了就是真出事了
	if used, limit := fdUsage(); limit > 0 {
		if float64(used)/float64(limit) >= m.cfg.FDRatio {
			return TripExit, "文件句柄快用完了"
		}
	}

	var mem runtime.MemStats
	runtime.ReadMemStats(&mem)
	// 用 HeapInuse 而不是 Sys：Sys 包含 Go 保留但已释放的页，只会单调上涨，
	// 一个长时间运行的服务哪怕没有任何泄漏，Sys 也会慢慢爬 —— 那会误熔断。
	// HeapInuse 是「GC 之后还活着的堆」，真出问题才涨。
	if mem.HeapInuse > m.cfg.MemLimit {
		return TripExit, "内存占用超过上限"
	}

	// 速率类：滑动窗口内的平均值。先推进窗口，否则洪水停了也看不出下降。
	m.mu.Lock()
	m.rollLocked(now.Unix())
	var total uint64
	for _, v := range m.buckets {
		total += uint64(v)
	}
	m.mu.Unlock()

	avg := float64(total) / float64(window)
	if avg < m.cfg.MaxAcceptsPerSec {
		m.mu.Lock()
		m.overFrom = time.Time{}
		m.mu.Unlock()
		return TripNone, ""
	}

	m.mu.Lock()
	if m.overFrom.IsZero() {
		m.overFrom = now
	}
	over := now.Sub(m.overFrom)
	m.mu.Unlock()

	if over < m.cfg.Sustain {
		return TripNone, ""
	}
	return TripBridge, "建连速率持续超过上限"
}

// Averages 返回当前观测到的每秒建连数（日志用）。
func (m *Monitor) Averages() float64 {
	m.mu.Lock()
	defer m.mu.Unlock()
	var total uint64
	for _, v := range m.buckets {
		total += uint64(v)
	}
	return float64(total) / float64(window)
}

// fdUsage 返回「已用句柄 / 上限」。拿不到上限时返回 (0, 0)，调用方跳过判断。
//
// Linux 上通过 /proc/self/fd 数目录项，Windows 上直接放弃 —— 服务端是 Linux，
// 开发机是 Windows，这里不能因为读不到就报错。
func fdUsage() (used int, limit int) {
	entries, err := os.ReadDir("/proc/self/fd")
	if err != nil {
		return 0, 0
	}
	limit = fdLimit()
	if limit <= 0 {
		return 0, 0
	}
	return len(entries), limit
}
