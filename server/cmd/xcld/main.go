// Command xcld 是 XCL 的账号服务：一个二进制同时提供
//
//   - HTTP：注册 / 登录 / 续期 / 退出（玩家账号）
//   - TCP 桥接：节点主入口，没有登录态直接断
//
// 两者放同一个进程的原因很实在：桥接每条连接都要查「这个账号还在不在、
// 被封了没有」，分进程就得反向调 HTTP 或定时拉黑名单，既有延迟又有陈旧窗口。
// 一个进程一份 SQLite，验签 + 查库都在本地完成。
package main

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/xin-craft/xcl-server/internal/api"
	"github.com/xin-craft/xcl-server/internal/auth"
	"github.com/xin-craft/xcl-server/internal/bridge"
	"github.com/xin-craft/xcl-server/internal/guard"
	"github.com/xin-craft/xcl-server/internal/store"
)

const usage = `xcld —— XCL 账号服务

用法：
  xcld keygen                       生成签名密钥与 PoW 种子（打印成环境变量写法）
  xcld serve [选项]                 启动服务（HTTP + TCP 桥接）
  xcld user list                    列出账号
  xcld user ban <用户名> [--reason]  封号
  xcld user unban <用户名>          解封
  xcld user passwd <用户名>         重置密码（不给 --password 就随机生成并打印）

serve 选项：
  --dir       数据目录（默认 ./data，数据库文件是 <dir>/xcl.db）
  --addr      HTTP 监听（默认 127.0.0.1:8080）
  --bridge    TCP 桥接监听（默认 0.0.0.0:11010，给 "" 可关掉）
  --upstream  easytier core 地址（默认 127.0.0.1:11011）
  --ttl       登录态有效期（默认 720h，即 30 天）
  --max-per-ip          同一 IP 的桥接并发连接数（默认 64）
  --max-per-device      同一账号的桥接并发连接数（默认 8，easytier 一个节点开 3~6 条）
  --device-rate         同一账号在 rate-window 内的建连次数上限（默认 2000）
  --ip-rate             同一 IP 在 rate-window 内的建连次数上限（默认 20000）
  --rate-window         建连速率窗口（默认 10m）
  --bad-token-limit     同一 IP 每分钟坏令牌次数，超过即临时拉黑（默认 20）
  --bad-token-ban       拉黑时长（默认 10m）
  --token-cache         令牌校验结果缓存时长，0 表示不缓存（默认 60s）
  --register-per-day    全站每天注册上限（默认 300）
  --max-sessions        单账号活跃登录态上限（默认 3）
  --accept-rate         每秒建连数上限，持续超时熔断（默认 500）
  --mem-limit-mb        进程内存上限，超过直接停服（默认 1536）
  --limit-off           关闭全部风控限制（排障用，默认 false）

环境变量：
  XCL_SIGN_KEY          64 位 hex（32 字节 ed25519 seed）或 128 位 hex（完整私钥）
  XCL_POW_SEED          64 位 hex，签发人机验证挑战用
  XCL_TURNSTILE_SECRET  Cloudflare Turnstile 密钥；留空则用自算工作量证明
  XCL_TURNSTILE_SITEKEY 上面那个 secret 对应的 sitekey（下发给客户端）
  XCL_BRIDGE_ADVERTISE  下发给客户端的桥接地址，如 bridge.example.com:11010
  XCL_LIMIT_DISABLE     设为 1 等价于 --limit-off

上面每个 --xxx 选项都可以用 XCL_XXX 环境变量给默认值（如 XCL_MAX_PER_IP=128）。
`

func main() {
	if len(os.Args) < 2 {
		fmt.Fprint(os.Stderr, usage)
		os.Exit(2)
	}

	log := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelInfo}))

	var err error
	switch os.Args[1] {
	case "keygen":
		err = runKeygen()
	case "serve":
		err = runServe(log, os.Args[2:])
	case "user":
		err = runUser(log, os.Args[2:])
	case "help", "-h", "--help":
		fmt.Print(usage)
	default:
		fmt.Fprintf(os.Stderr, "不认识的子命令 %q\n\n%s", os.Args[1], usage)
		os.Exit(2)
	}
	if err != nil {
		fmt.Fprintf(os.Stderr, "出错：%v\n", err)
		os.Exit(1)
	}
}

func runKeygen() error {
	seed := make([]byte, 32)
	if _, err := rand.Read(seed); err != nil {
		return err
	}
	_, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		return err
	}
	fmt.Printf("# 写进 systemd 的 EnvironmentFile，不要提交到仓库\n")
	fmt.Printf("XCL_SIGN_KEY=%s\n", hex.EncodeToString(priv.Seed()))
	fmt.Printf("XCL_SIGN_PUBKEY=%s\n", hex.EncodeToString(priv.Public().(ed25519.PublicKey)))
	fmt.Printf("XCL_POW_SEED=%s\n", hex.EncodeToString(seed))
	return nil
}

func runServe(log *slog.Logger, args []string) error {
	fs := flag.NewFlagSet("serve", flag.ExitOnError)
	dir := fs.String("dir", "./data", "数据目录")
	addr := fs.String("addr", "127.0.0.1:8080", "HTTP 监听地址")
	bridgeAddr := fs.String("bridge", "0.0.0.0:11010", "TCP 桥接监听地址，空字符串表示不启")
	upstream := fs.String("upstream", "127.0.0.1:11011", "easytier core 地址")
	ttl := fs.Duration("ttl", 30*24*time.Hour, "登录态有效期")
	trustForwarded := fs.Bool("trust-forwarded", false,
		"信任 CF-Connecting-IP / X-Forwarded-For（只有前面挂了可信反代才开）")

	// 风控阈值。全部可配的理由很实在：这些数字第一次上线一定是拍脑袋定的，
	// 上线后用日志校正，改配置重启就行，不用重新发版。
	maxPerIP := fs.Int("max-per-ip", envInt("XCL_MAX_PER_IP", 64),
		"同一 IP 的桥接并发连接数")
	maxPerDevice := fs.Int("max-per-device", envInt("XCL_MAX_PER_DEVICE", 8),
		"同一账号的桥接并发连接数")
	rateWindow := fs.Duration("rate-window", envDuration("XCL_RATE_WINDOW", 10*time.Minute),
		"建连速率窗口")
	deviceRate := fs.Int("device-rate", envInt("XCL_DEVICE_RATE", 2000),
		"同一账号在窗口内的建连次数上限")
	ipRate := fs.Int("ip-rate", envInt("XCL_IP_RATE", 20000),
		"同一 IP 在窗口内的建连次数上限")
	badTokenLimit := fs.Int("bad-token-limit", envInt("XCL_BAD_TOKEN_LIMIT", 20),
		"同一 IP 每分钟坏令牌次数，超过即临时拉黑")
	badTokenBan := fs.Duration("bad-token-ban", envDuration("XCL_BAD_TOKEN_BAN", 10*time.Minute),
		"坏令牌拉黑时长")
	tokenCache := fs.Duration("token-cache", envDuration("XCL_TOKEN_CACHE", 60*time.Second),
		"令牌校验结果缓存时长，0 表示每次都查库")
	registerPerDay := fs.Int("register-per-day", envInt("XCL_REGISTER_PER_DAY", 300),
		"全站每天注册上限")
	maxSessions := fs.Int("max-sessions", envInt("XCL_MAX_SESSIONS", 3),
		"单账号活跃登录态上限")
	acceptRate := fs.Float64("accept-rate", envFloat("XCL_ACCEPT_RATE", 500),
		"每秒建连数上限，持续超过即熔断")
	memLimitMB := fs.Int("mem-limit-mb", envInt("XCL_MEM_LIMIT_MB", 1536),
		"进程内存上限（MB），超过即停服")
	fdRatio := fs.Float64("fd-ratio", envFloat("XCL_FD_RATIO", 0.8),
		"文件句柄使用比例上限，超过即停服")
	limitOff := fs.Bool("limit-off", envBool("XCL_LIMIT_DISABLE", false),
		"关闭全部风控限制（排障用）")

	if err := fs.Parse(args); err != nil {
		return err
	}

	signKey, err := loadSignKey()
	if err != nil {
		return err
	}
	powSeed, err := envHex("XCL_POW_SEED", 32)
	if err != nil {
		return err
	}

	db, err := store.Open(filepath.Join(*dir, "xcl.db"))
	if err != nil {
		return err
	}
	defer db.Close()
	log.Info("数据库已就绪", "path", db.Path(), "users", db.CountUsers())

	accountSrv := api.New(api.Config{
		SignKey:        signKey,
		TokenTTL:       *ttl,
		TrustForwarded: *trustForwarded,
		// 账号接口走 Cloudflare，桥接走裸 TCP，地址必须分开下发
		BridgeAdvertise:  strings.TrimSpace(os.Getenv("XCL_BRIDGE_ADVERTISE")),
		PowSeed:          powSeed,
		TurnstileSecret:  strings.TrimSpace(os.Getenv("XCL_TURNSTILE_SECRET")),
		TurnstileSiteKey: strings.TrimSpace(os.Getenv("XCL_TURNSTILE_SITEKEY")),
		// 默认允许降级：Cloudflare 一挂，注册就全停的代价太大
		AllowPowFallback:   true,
		RegisterPerDay:     *registerPerDay,
		MaxSessionsPerUser: *maxSessions,
		Logger:             log,
	}, db)
	handler := accountSrv.Handler()

	// 熔断器：只采样、只判定，动作交给下面那个 goroutine。
	// 这样「判定」可以单测，不会因为在测试里触发了熔断就把测试进程杀掉。
	monitor := guard.New(guard.Config{
		MaxAcceptsPerSec: *acceptRate,
		MemLimit:         uint64(*memLimitMB) << 20,
		FDRatio:          *fdRatio,
	})
	if *limitOff {
		accountSrv.SetLimitDisabled(true)
		log.Warn("风控限制已关闭（--limit-off），仅用于排障")
	}

	var tcp *bridge.Server
	if *bridgeAddr != "" {
		tcp = bridge.New(bridge.Config{
			Listen:            *bridgeAddr,
			Upstream:          *upstream,
			PublicKey:         signKey.Public().(ed25519.PublicKey),
			MaxPerDevice:      *maxPerDevice,
			MaxPerIP:          *maxPerIP,
			RateWindow:        *rateWindow,
			DeviceRate:        *deviceRate,
			IPRate:            *ipRate,
			BadTokenPerMinute: *badTokenLimit,
			BadTokenBan:       *badTokenBan,
			TokenCacheTTL:     *tokenCache,
			OnAccept:          func() { monitor.OnAccept(time.Now()) },
			Logger:            log,
		}, db)
		if *limitOff {
			tcp.SetLimitDisabled(true)
		}
		if err := tcp.Listen(); err != nil {
			return fmt.Errorf("桥接无法监听 %s：%w", *bridgeAddr, err)
		}
		defer tcp.Close()
	} else {
		log.Warn("没有启用 TCP 桥接，节点主端口将不可用")
	}

	srv := &http.Server{
		Addr:              *addr,
		Handler:           handler,
		ReadHeaderTimeout: 5 * time.Second,
		ReadTimeout:       15 * time.Second,
		WriteTimeout:      15 * time.Second,
		IdleTimeout:       60 * time.Second,
	}

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	go func() {
		log.Info("HTTP 已监听", "addr", *addr, "trustForwarded", *trustForwarded)
		if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			log.Error("HTTP 服务退出", "err", err)
			stop()
		}
	}()

	// 熔断看门狗：每秒看一眼，命中就动手。
	//
	// 分两档是因为「被刷」和「要崩了」是两件不同的事：
	//   - 建连速率失控：只停联机入口，玩家还能登进来看到「联机维护中」；
	//   - 内存/句柄见底：留在场上只会更糟，整个进程退出。
	//
	// 注：熔断只拒绝新连接，已经在玩的人不会被踢 —— 停服是为了自保，
	// 不是为了惩罚正在玩的人。
	go runWatchdog(ctx, monitor, tcp, *limitOff, *acceptRate, filepath.Join(*dir, "cooling.json"), log)

	// 稳定跑满 10 分钟就把熔断计数清零，否则一次偶发会把冷却时间永久拉长
	go func() {
		select {
		case <-ctx.Done():
		case <-time.After(10 * time.Minute):
			_ = os.Remove(filepath.Join(*dir, "cooling.json"))
		}
	}()

	// 每天清一次过期会话：量很小，不值得单独的服务，跟着进程走就行
	go func() {
		ticker := time.NewTicker(24 * time.Hour)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case now := <-ticker.C:
				if n, err := db.PurgeExpired(now); err != nil {
					log.Warn("清理过期会话失败", "err", err)
				} else if n > 0 {
					log.Info("清理过期会话", "count", n)
				}
			}
		}
	}()

	<-ctx.Done()
	log.Info("收到退出信号，正在收尾")
	shutdownCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := srv.Shutdown(shutdownCtx); err != nil {
		log.Warn("HTTP 关闭超时", "err", err)
	}
	return nil
}

func runUser(log *slog.Logger, args []string) error {
	if len(args) == 0 {
		return errors.New("缺子命令：list / ban / unban / passwd")
	}
	action := args[0]

	fs := flag.NewFlagSet("user", flag.ExitOnError)
	dir := fs.String("dir", "./data", "数据目录")
	reason := fs.String("reason", "", "封禁原因（ban 时记下，玩家能看到）")
	password := fs.String("password", "", "新密码（passwd 时用；留空则随机生成）")
	if err := fs.Parse(args[1:]); err != nil {
		return err
	}

	db, err := store.Open(filepath.Join(*dir, "xcl.db"))
	if err != nil {
		return err
	}
	defer db.Close()

	if action == "list" {
		users, err := db.ListUsers()
		if err != nil {
			return err
		}
		if len(users) == 0 {
			fmt.Println("（还没有账号）")
			return nil
		}
		fmt.Printf("%-6s %-20s %-8s %-20s %-20s %s\n", "ID", "用户名", "状态", "注册时间", "最近登录", "备注")
		for _, u := range users {
			state := "正常"
			if u.Banned {
				state = "已封"
			}
			fmt.Printf("%-6d %-20s %-8s %-20s %-20s %s\n",
				u.ID, u.Username, state, stamp(u.CreatedAt), stamp(u.LastLoginAt), u.BannedReason)
		}
		fmt.Printf("\n共 %d 个账号\n", len(users))
		return nil
	}

	if fs.NArg() < 1 {
		return fmt.Errorf("%s 需要用户名", action)
	}
	username := store.NormalizeUsername(fs.Arg(0))
	user, _, err := db.UserByName(username)
	if errors.Is(err, store.ErrNotFound) {
		return fmt.Errorf("没有这个账号：%s", username)
	}
	if err != nil {
		return err
	}

	switch action {
	case "ban":
		if err := db.SetBanned(user.ID, true, strings.TrimSpace(*reason)); err != nil {
			return err
		}
		log.Info("已封号", "user", user.Username, "id", user.ID, "reason", *reason)
		fmt.Printf("已封禁 %s，它的全部登录态同时作废\n", user.Username)
		return nil

	case "unban":
		if err := db.SetBanned(user.ID, false, ""); err != nil {
			return err
		}
		fmt.Printf("已解封 %s（旧登录态不会复活，需要重新登录）\n", user.Username)
		return nil

	case "passwd":
		newPassword := *password
		generated := false
		if newPassword == "" {
			newPassword = randomPassword(12)
			generated = true
		}
		if err := auth.ValidatePassword(newPassword); err != nil {
			return err
		}
		hash, err := auth.HashPassword(newPassword)
		if err != nil {
			return err
		}
		if err := db.SetPassword(user.ID, hash); err != nil {
			return err
		}
		// 改密码必须踢掉所有会话，否则「密码已经被改过」之后旧令牌还能继续用
		if err := db.RevokeUserSessions(user.ID); err != nil {
			return err
		}
		if generated {
			fmt.Printf("已重置 %s 的密码：%s\n（全部登录态已作废，请让玩家用这个密码重新登录）\n",
				user.Username, newPassword)
		} else {
			fmt.Printf("已修改 %s 的密码，全部登录态已作废\n", user.Username)
		}
		return nil

	default:
		return fmt.Errorf("不认识的 user 子命令 %q", action)
	}
}

// envHex 读一个定长的十六进制环境变量。
//
// 长度不对直接失败 —— 密钥少几个字符不该表现成「签名总是验不过」这种要靠猜的现象。
func envHex(name string, size int) ([]byte, error) {
	text := strings.TrimSpace(os.Getenv(name))
	if text == "" {
		return nil, fmt.Errorf("缺少环境变量 %s（用 xcld keygen 生成）", name)
	}
	raw, err := hex.DecodeString(text)
	if err != nil {
		return nil, fmt.Errorf("%s 不是合法 hex：%w", name, err)
	}
	if len(raw) != size {
		return nil, fmt.Errorf("%s 应当是 %d 字节，实际 %d", name, size, len(raw))
	}
	return raw, nil
}

func loadSignKey() (ed25519.PrivateKey, error) {
	text := strings.TrimSpace(os.Getenv("XCL_SIGN_KEY"))
	if text == "" {
		return nil, errors.New("缺少环境变量 XCL_SIGN_KEY（用 xcld keygen 生成）")
	}
	raw, err := hex.DecodeString(text)
	if err != nil {
		return nil, fmt.Errorf("XCL_SIGN_KEY 不是合法 hex：%w", err)
	}
	switch len(raw) {
	case ed25519.SeedSize:
		return ed25519.NewKeyFromSeed(raw), nil
	case ed25519.PrivateKeySize:
		return ed25519.PrivateKey(raw), nil
	default:
		return nil, fmt.Errorf("XCL_SIGN_KEY 应当是 32 或 64 字节，实际 %d", len(raw))
	}
}

// randomPassword 取无歧义字符集（去掉 0/o/O、1/l/I）——
// 这个密码大概率要被人工念给玩家。
func randomPassword(n int) string {
	const alphabet = "abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789"
	buf := make([]byte, n)
	if _, err := rand.Read(buf); err != nil {
		return ""
	}
	out := make([]byte, n)
	for i := range buf {
		out[i] = alphabet[int(buf[i])%len(alphabet)]
	}
	return string(out)
}

func stamp(unix int64) string {
	if unix == 0 {
		return "-"
	}
	return time.Unix(unix, 0).Local().Format("2006-01-02 15:04")
}

// ---------- 熔断 ----------

// coolingState 是「为什么停的、什么时候可以再试」的记录。
//
// 写在数据目录而不是日志里：看门狗要读它来决定什么时候拉起服务，
// 翻日志去解析时间太脆。
type coolingState struct {
	Reason   string `json:"reason"`
	Since    int64  `json:"since"`
	ResumeAt int64  `json:"resumeAt"`
	Strikes  int    `json:"strikes"`
}

// runWatchdog 每秒评估一次，命中就动手。
//
// 两档动作的区别：
//   - TripBridge：关掉联机入口，HTTP 不动。玩家还能登录、还能看到「联机维护中」；
//     流量下来后每 30 秒试探一次，恢复就重新监听。已经在玩的连接不受影响。
//   - TripExit：写冷却标记后整个进程退出。端口无人监听时内核直接回 RST，
//     用户态零开销；这是「再不停就要崩」时的唯一正确动作。
func runWatchdog(ctx context.Context, m *guard.Monitor, tcp *bridge.Server,
	limitOff bool, acceptRate float64, coolingPath string, log *slog.Logger) {

	ticker := time.NewTicker(time.Second)
	defer ticker.Stop()

	bridgeDown := false
	var nextProbe time.Time

	for {
		select {
		case <-ctx.Done():
			return
		case now := <-ticker.C:
			if limitOff {
				continue
			}

			trip, why := m.Evaluate(now)

			// 资源告急优先：不管联机停没停，内存/句柄见底都该退出。
			// 先判这一条，是因为下面的恢复分支会 `continue` —— 顺序错了就会
			// 出现「联机已经停了，但内存还在涨，却再也没人看」的死角。
			if trip == guard.TripExit {
				enterCooling(coolingPath, why, log)
				return
			}

			// 联机已停：定期试探，流量下来了就恢复
			if bridgeDown {
				if now.Before(nextProbe) {
					continue
				}
				if m.Averages() >= acceptRate {
					nextProbe = now.Add(30 * time.Second)
					continue
				}
				if tcp == nil {
					return
				}
				if err := tcp.Restart(); err != nil {
					log.Warn("联机入口恢复失败", "err", err)
					nextProbe = now.Add(30 * time.Second)
					continue
				}
				log.Info("联机入口已恢复", "avgPerSec", m.Averages())
				bridgeDown = false
				continue
			}

			if trip == guard.TripBridge {
				if tcp != nil {
					_ = tcp.Close()
				}
				log.Error("建连速率异常，暂停联机入口", "why", why, "avgPerSec", m.Averages())
				bridgeDown = true
				nextProbe = now.Add(60 * time.Second)
			}
		}
	}
}

// enterCooling 写冷却标记并退出进程。
//
// 冷却时间按连续触发次数翻倍（5 → 10 → 20 → 40 分钟封顶）：
// 一直被打就一直离线，攻击者的成本是持续占用机器，我们的成本是「暂时没人能联机」。
// 一旦稳定跑满 10 分钟，计数会被清零，下次重新从 5 分钟开始。
func enterCooling(path, reason string, log *slog.Logger) {
	strikes := 0
	if raw, err := os.ReadFile(path); err == nil {
		var old coolingState
		if json.Unmarshal(raw, &old) == nil {
			strikes = old.Strikes
		}
	}
	strikes++

	cooldown := 5 * time.Minute
	for i := 1; i < strikes; i++ {
		cooldown *= 2
		if cooldown >= 40*time.Minute {
			cooldown = 40 * time.Minute
			break
		}
	}

	now := time.Now()
	st := coolingState{
		Reason:   reason,
		Since:    now.Unix(),
		ResumeAt: now.Add(cooldown).Unix(),
		Strikes:  strikes,
	}
	if raw, err := json.MarshalIndent(st, "", "  "); err == nil {
		if err := os.WriteFile(path, raw, 0o640); err != nil {
			log.Error("写冷却标记失败", "path", path, "err", err)
		}
	}

	log.Error("触发熔断，服务退出",
		"reason", reason, "cooldown", cooldown.String(),
		"resumeAt", stamp(st.ResumeAt), "strikes", strikes)
	os.Exit(guard.ExitCode)
}

// ---------- 环境变量 ----------

func envInt(name string, def int) int {
	if v, err := strconv.Atoi(strings.TrimSpace(os.Getenv(name))); err == nil {
		return v
	}
	return def
}

func envFloat(name string, def float64) float64 {
	if v, err := strconv.ParseFloat(strings.TrimSpace(os.Getenv(name)), 64); err == nil {
		return v
	}
	return def
}

func envDuration(name string, def time.Duration) time.Duration {
	if v, err := time.ParseDuration(strings.TrimSpace(os.Getenv(name))); err == nil {
		return v
	}
	return def
}

func envBool(name string, def bool) bool {
	switch strings.ToLower(strings.TrimSpace(os.Getenv(name))) {
	case "1", "true", "yes", "on":
		return true
	case "0", "false", "no", "off":
		return false
	}
	return def
}
