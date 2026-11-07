// Package limit 是「每 key 每窗口最多 N 次」的计数器。
//
// 为什么要单独一个包：账号接口和桥接都要用它，而两边对它的要求是同一套：
//
//  1. 不能无限长。第一版用 map[string][]int64 存每一次命中的秒级时间戳，
//     看起来没毛病，实际上是两个坑：key 是提交上来的（登录名），攻击者
//     每次换一个名字就在内存里留一格，而清理只发生在「同一个 key 再被访问」
//     的时候 —— 不同 key 永远删不掉，发一百万个假登录名就能把内存撑爆。
//  2. 不能吃太多内存。滑动窗口按「每次命中一个 int64」存，10000 次/10 分钟
//     的一个 key 就是 80 KB；而按秒分桶是常数开销。
//
// 所以这里的实现是：固定 300 个桶的环形数组 + uint16 计数（600 字节/key），
// 外加每 60 秒一次的过期清扫和 maxKeys 硬上限。
//
// 计数精度上，桶宽 = window/300 自适应（10 分钟窗口就是 2 秒一桶）。宁可粗一点：
// 挡脚本不需要精确到秒，而精确的代价是内存。
package limit

import (
	"sync"
	"time"
)

const (
	// slots 是每个 key 的桶数。定死而不是按窗口算，是为了让单个 key 的
	// 内存开销可预测：300 * 2 字节 = 600 字节，和窗口长短无关。
	slots = 300
	// sweepEvery 是清扫间隔。清扫做的是「把整个窗口内都没动静的 key 删掉」，
	// 比逐次修剪省事得多，也顺带解决了「不同 key 永不清理」的问题。
	sweepEvery = 60 * time.Second
	// 计数上限：一桶之内不可能有 65535 次命中（那是 3 万/秒），饱和即可。
	saturate = 65535
)

// Limiter 是固定窗口计数的集合。
//
// 零值不可用，必须经 New 构造。并发安全。
type Limiter struct {
	window   time.Duration
	bucket   int64 // 桶宽（秒），至少 1
	limit    int
	maxKeys  int
	disabled bool

	mu        sync.Mutex
	entries   map[string]*entry
	lastSweep time.Time

	// 统计用：被拒次数，便于日志观察，不做逻辑判断
	rejected uint64
}

type entry struct {
	counts [slots]uint16
	last   int64 // 最后一次命中所在的秒
}

// New 构造一个窗口为 window、上限为 limit 次的计数器。
func New(window time.Duration, limit int) *Limiter {
	if window <= 0 {
		window = time.Minute
	}
	bucket := int64(window / (slots * time.Second))
	if bucket < 1 {
		bucket = 1
	}
	if limit < 1 {
		limit = 1
	}
	return &Limiter{
		window:  window,
		bucket:  bucket,
		limit:   limit,
		maxKeys: 20000,
		entries: make(map[string]*entry, 64),
	}
}

// WithMaxKeys 改单表最多容纳多少个 key。
//
// 超过之后是整体清空而不是拒绝新 key：被拒会让真实账号连不上，
// 清空最多让某个脚本多试几次 —— 两害相权。
func (l *Limiter) WithMaxKeys(n int) *Limiter {
	if n > 0 {
		l.maxKeys = n
	}
	return l
}

// SetDisabled 一键关掉（排障用）。关掉后 Allow 恒为 true。
func (l *Limiter) SetDisabled(v bool) {
	l.mu.Lock()
	l.disabled = v
	l.mu.Unlock()
}

// Limit 返回上限（日志用）。
func (l *Limiter) Limit() int { return l.limit }

// Allow 记一次命中；超限返回 false。
func (l *Limiter) Allow(key string, now time.Time) bool {
	return l.AllowN(key, now, 1)
}

// AllowN 一次记 n 次命中（比如按流量折算）。
func (l *Limiter) AllowN(key string, now time.Time, n int) bool {
	if key == "" || n <= 0 {
		return true
	}
	cur := now.Unix()

	l.mu.Lock()
	defer l.mu.Unlock()
	if l.disabled {
		return true
	}
	if l.lastSweep.IsZero() || now.Sub(l.lastSweep) >= sweepEvery {
		l.sweepLocked(cur)
	}

	e := l.entries[key]
	if e == nil {
		if len(l.entries) >= l.maxKeys {
			// 硬上限：整体重来。宁可让某个脚本重试，也不让内存被撑爆。
			l.entries = make(map[string]*entry, 64)
		}
		e = &entry{last: cur}
		l.entries[key] = e
	}

	l.rollLocked(e, cur)

	total := 0
	for _, c := range e.counts {
		total += int(c)
	}
	if total+n > l.limit {
		l.rejected++
		return false
	}
	idx := int((cur / l.bucket) % slots)
	v := int(e.counts[idx]) + n
	if v > saturate {
		v = saturate
	}
	e.counts[idx] = uint16(v)
	return true
}

// Count 返回当前窗口内的命中数（只读，不计数）。
func (l *Limiter) Count(key string, now time.Time) int {
	l.mu.Lock()
	defer l.mu.Unlock()
	e := l.entries[key]
	if e == nil {
		return 0
	}
	l.rollLocked(e, now.Unix())
	total := 0
	for _, c := range e.counts {
		total += int(c)
	}
	return total
}

// Len 返回当前跟踪了多少个 key（健康检查 / 观察内存用）。
func (l *Limiter) Len() int {
	l.mu.Lock()
	defer l.mu.Unlock()
	return len(l.entries)
}

// Rejected 返回累计拒绝次数。
func (l *Limiter) Rejected() uint64 {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.rejected
}

// rollLocked 把 e 里「已经滑出窗口」的桶清零。
//
// 只在 key 被访问时推进，所以不需要后台把每个 key 都滚一遍 ——
// 没人访问的 key 会在 sweep 里整个丢掉。
func (l *Limiter) rollLocked(e *entry, cur int64) {
	curBucket := cur / l.bucket
	lastBucket := e.last / l.bucket
	if curBucket == lastBucket {
		return
	}
	gap := curBucket - lastBucket
	if gap >= slots {
		for i := range e.counts {
			e.counts[i] = 0
		}
	} else {
		for i := int64(1); i <= gap; i++ {
			idx := int((lastBucket + i) % slots)
			e.counts[idx] = 0
		}
	}
	e.last = cur
}

// sweepLocked 丢掉「整个窗口内都没有动静」的 key。
func (l *Limiter) sweepLocked(cur int64) {
	l.lastSweep = time.Unix(cur, 0)
	cutoff := cur - int64(l.window/time.Second)
	for k, e := range l.entries {
		if e.last < cutoff {
			delete(l.entries, k)
		}
	}
}
