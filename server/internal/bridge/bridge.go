// Package bridge 是节点对外的唯一入口。
//
// 它做的事很少，但每一条都不能少：
//
//  1. 读走连接上的第一帧令牌，验签、查登录态、判封禁；
//  2. 通过之后才把这条 TCP 接到本机的 easytier core 上；
//  3. 不通过就直接关，一个字节都不回 —— 让对方连「这里有什么」都问不出来。
//
// 强制登录的依据就在这里：没有登录态，连接根本到不了 core。
// 而 Cloudflare 那类免费 CDN 只代理 HTTP，TCP 这一层帮不上忙，只能自己守着。
//
// # 门卫的顺序（顺序就是成本）
//
// 每一条检查都是要花时间的，所以贵的必须排在便宜后面：
//
//	黑名单 → IP 速率 → IP 并发 → 校验令牌（可能要查库）→ 账号速率 → 账号并发 → 转发
//
// 第一版把「校验令牌」排在账号并发之前，等于一万人扑上来就查一万次库。
// 现在能挡住的最前面就挡住了，查库只发生在「IP 维度看着正常」的连接上。
//
// # 令牌缓存
//
// 每条连接都查一次库（一次 join）在正常量级下完全没问题，被刷的时候就是瓶颈。
// 所以校验通过的结果缓存 60 秒。代价是「封号最多延迟一分钟生效」—— 这个取舍是
// 刻意的：不缓存的话，一次洪水就能把 SQLite 打满，那时候不是「封得快」，是全都连不上。
// 需要立刻生效的场合（改密码、退出登录）走 HTTP 侧，那条路径不经过这里。
package bridge

import (
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/binary"
	"errors"
	"io"
	"log/slog"
	"net"
	"strconv"
	"sync"
	"time"

	"github.com/xin-craft/xcl-server/internal/auth"
	"github.com/xin-craft/xcl-server/internal/limit"
	"github.com/xin-craft/xcl-server/internal/store"
)

// 首帧：magic(4) + 长度(2, 大端) + 令牌文本。
//
// 用文本而不是裸字节，是为了升级协议时还能塞别的东西（比如版本协商）；
// 令牌本身是 base64url，118 个字符，一帧就够。
var magic = [4]byte{'X', 'C', 'L', '1'}

const (
	maxTokenLen = 512
	// authTimeout 限制「连上来但不说话」的时长。没有它，扫描器可以拿几千个
	// 空闲连接把文件描述符耗光。
	authTimeout = 10 * time.Second
	// upstreamTimeout 是连本机 core 的超时。core 就在同一台机器上，
	// 连不上说明它挂了，不该让客户端干等。
	upstreamTimeout = 5 * time.Second
	// cacheKeyLen 是令牌缓存键的长度（sha256 前 16 字节）。
	// 存摘要而不是令牌原文：万一内存被 dump，拿不到能直接用的令牌。
	cacheKeyLen = 16
	// banSweepEvery 是黑名单清扫间隔。不清的话被扫过的 IP 会永久留在表里。
	banSweepEvery = time.Minute
)

// Config 是桥接配置。
type Config struct {
	// Listen 是对外监听地址，例如 0.0.0.0:11010。
	Listen string
	// Upstream 是 easytier core 的地址，形如 127.0.0.1:11011。
	Upstream string
	// PublicKey 是签发令牌那把私钥对应的公钥。
	PublicKey ed25519.PublicKey

	// MaxPerDevice / MaxPerIP 是并发连接上限。
	// easytier 一个节点会开 3~6 条连接，账号维度留到 8 足够重连时抖一下。
	//
	// IP 维度必须开得比账号维度大得多：网吧、学校、公司往往几十个人共用一个
	// 出口 IP，按「每人 3~6 条」算，20 个人就是上百条。定成 16 会把真人挡在门外，
	// 而且症状是「有的人能进有的人进不去」，极难排查。
	MaxPerDevice int
	MaxPerIP     int

	// RateWindow / DeviceRate / IPRate 是建连速率闸。
	//
	// 注意：这不是资源配额（资源由并发上限管），它挡的是「一个账号在疯狂重连」
	// 和「一个 IP 在建连洪水」。所以阈值要远高于正常值，绝不能误伤 easytier
	// 断网后的自动重连 —— 那时候它会短时间内反复建连，这是正常行为。
	RateWindow time.Duration
	DeviceRate int
	IPRate     int

	// BadTokenPerMinute / BadTokenBan：同一个 IP 在一分钟内坏令牌超过这个次数，
	// 直接封一段时间。没有这条，扫描器可以无限次重试 —— 每次都只是被断连，
	// 对它来说零成本。
	BadTokenPerMinute int
	BadTokenBan       time.Duration

	// TokenCacheTTL 是令牌校验结果缓存时长。0 表示不缓存（每次都查库）。
	TokenCacheTTL time.Duration

	// OnAccept 在每次接受连接时调用（熔断器采样用）。
	OnAccept func()

	AuthTimeout time.Duration
	Logger      *slog.Logger
}

// Server 是监听器 + 计数器的组合。
type Server struct {
	cfg Config
	db  *store.Store
	log *slog.Logger

	mu        sync.Mutex
	byUser    map[int64]int
	byIP      map[string]int
	bannedIP  map[string]time.Time
	accepted  uint64
	rejected  uint64
	lastSweep time.Time

	cacheMu sync.Mutex
	cache   map[[cacheKeyLen]byte]cacheEntry

	ipRate   *limit.Limiter
	userRate *limit.Limiter
	badRate  *limit.Limiter

	listener net.Listener
}

type cacheEntry struct {
	userID int64
	exp    time.Time
}

// New 组装桥接。
func New(cfg Config, db *store.Store) *Server {
	if cfg.MaxPerDevice <= 0 {
		cfg.MaxPerDevice = 8
	}
	if cfg.MaxPerIP <= 0 {
		cfg.MaxPerIP = 64
	}
	if cfg.AuthTimeout <= 0 {
		cfg.AuthTimeout = authTimeout
	}
	if cfg.RateWindow <= 0 {
		cfg.RateWindow = 10 * time.Minute
	}
	if cfg.DeviceRate <= 0 {
		cfg.DeviceRate = 2000
	}
	if cfg.IPRate <= 0 {
		cfg.IPRate = 20000
	}
	if cfg.BadTokenPerMinute <= 0 {
		cfg.BadTokenPerMinute = 20
	}
	if cfg.BadTokenBan <= 0 {
		cfg.BadTokenBan = 10 * time.Minute
	}
	if cfg.TokenCacheTTL == 0 {
		cfg.TokenCacheTTL = 60 * time.Second
	}
	log := cfg.Logger
	if log == nil {
		log = slog.Default()
	}
	if cfg.PublicKey == nil {
		panic("bridge: 必须提供 PublicKey")
	}
	s := &Server{
		cfg:      cfg,
		db:       db,
		log:      log,
		byUser:   make(map[int64]int),
		byIP:     make(map[string]int),
		bannedIP: make(map[string]time.Time),
		cache:    make(map[[cacheKeyLen]byte]cacheEntry),
		ipRate:   limit.New(cfg.RateWindow, cfg.IPRate).WithMaxKeys(50000),
		userRate: limit.New(cfg.RateWindow, cfg.DeviceRate),
		badRate:  limit.New(time.Minute, cfg.BadTokenPerMinute).WithMaxKeys(50000),
	}
	return s
}

// SetLimitDisabled 一键关掉所有新加的限制（排障用）。
func (s *Server) SetLimitDisabled(v bool) {
	s.ipRate.SetDisabled(v)
	s.userRate.SetDisabled(v)
	s.badRate.SetDisabled(v)
}

// Listen 开始接受连接。返回后由调用方决定何时 Close。
func (s *Server) Listen() error {
	ln, err := net.Listen("tcp", s.cfg.Listen)
	if err != nil {
		return err
	}
	s.listener = ln
	s.log.Info("桥接已监听", "addr", ln.Addr().String(), "upstream", s.cfg.Upstream,
		"maxPerDevice", s.cfg.MaxPerDevice, "maxPerIP", s.cfg.MaxPerIP,
		"deviceRate", s.cfg.DeviceRate, "ipRate", s.cfg.IPRate)

	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				if errors.Is(err, net.ErrClosed) {
					return
				}
				// 单次 accept 失败（比如瞬时 fd 用尽）不该让整个服务停下来
				s.log.Warn("accept 失败", "err", err)
				time.Sleep(50 * time.Millisecond)
				continue
			}
			if s.cfg.OnAccept != nil {
				s.cfg.OnAccept()
			}
			go s.serve(conn)
		}
	}()
	return nil
}

// Addr 返回实际监听地址（Listen 之前是 nil）。
func (s *Server) Addr() net.Addr {
	if s.listener == nil {
		return nil
	}
	return s.listener.Addr()
}

// Close 停止监听。已在转发中的连接不受影响。
func (s *Server) Close() error {
	if s.listener == nil {
		return nil
	}
	return s.listener.Close()
}

// Restart 关掉监听再重新打开，用于熔断后的试探恢复。
//
// 已经建立的长连接不受影响：熔断只拒绝新连接，正在玩的玩家不能被踢。
func (s *Server) Restart() error {
	_ = s.Close()
	return s.Listen()
}

// Stats 供健康检查 / 运维查看。
func (s *Server) Stats() (accepted, rejected uint64) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.accepted, s.rejected
}

// serve 处理一条连接。
func (s *Server) serve(client net.Conn) {
	defer client.Close()

	now := time.Now()
	ip := hostOf(client.RemoteAddr())

	// ① 黑名单。连首帧都不读 —— 已经被判过的来源，不值得为它调度。
	if s.isBanned(ip, now) {
		s.reject("ip_banned", ip, 0)
		return
	}

	// ② IP 建连速率。挡的是「一个 IP 扑上来开连接」，不是资源配额。
	if !s.ipRate.Allow(ip, now) {
		s.reject("ip_rate", ip, 0)
		return
	}

	// ③ IP 并发。在任何查库之前 —— 洪水大多死在这一步，一次库都不用查。
	if !s.tryAcquireIP(ip) {
		s.reject("ip_conn", ip, 0)
		return
	}
	defer s.releaseIP(ip)

	// ④ 校验令牌（可能查库，命中缓存则不查）
	userID, ok := s.authenticate(client, ip, now)
	if !ok {
		return
	}

	// ⑤ 账号建连速率
	if !s.userRate.Allow(strconv.FormatInt(userID, 10), now) {
		s.reject("user_rate", ip, userID)
		return
	}

	// ⑥ 账号并发
	if !s.tryAcquireUser(userID) {
		s.reject("user_conn", ip, userID)
		return
	}
	defer s.releaseUser(userID)

	upstream, err := net.DialTimeout("tcp", s.cfg.Upstream, upstreamTimeout)
	if err != nil {
		s.bump(false)
		s.log.Error("连不上本机联机模块", "upstream", s.cfg.Upstream, "err", err)
		return
	}
	defer upstream.Close()

	s.bump(true)
	// 通过之后回一个字节：客户端据此确认「令牌被接受了」，而不是靠猜。
	if _, err := client.Write([]byte{0x00}); err != nil {
		return
	}
	// 认证阶段设过读超时，转发前必须清掉，否则长连接会在 10 秒后自己断
	if err := client.SetDeadline(time.Time{}); err != nil {
		return
	}

	pipe(client, upstream)
}

// authenticate 读首帧、验令牌、查登录态。
//
// 任何一步失败都只关连接、不回字节，并记一次「坏令牌」——
// 累计到阈值就把这个 IP 拉黑一段时间。
func (s *Server) authenticate(conn net.Conn, ip string, now time.Time) (int64, bool) {
	if err := conn.SetReadDeadline(now.Add(s.cfg.AuthTimeout)); err != nil {
		s.bump(false)
		return 0, false
	}

	var header [6]byte
	if _, err := io.ReadFull(conn, header[:]); err != nil {
		// 读超时 / 半路断开不算坏令牌：正常玩家网络抖一下也会这样，
		// 按坏令牌累计会把整个 NAT 出口封掉（网吧里 20 个人同时抖一次就够了）。
		// 这类连接由 IP 并发上限 + 认证超时兜住，本来就占不住资源。
		s.failAuth(conn, ip, now, "首帧读不到", err, false)
		return 0, false
	}
	if [4]byte(header[:4]) != magic {
		s.failAuth(conn, ip, now, "首帧 magic 不对", nil, true)
		return 0, false
	}
	size := int(binary.BigEndian.Uint16(header[4:]))
	if size == 0 || size > maxTokenLen {
		s.failAuth(conn, ip, now, "首帧长度非法", nil, true)
		return 0, false
	}

	text := make([]byte, size)
	if _, err := io.ReadFull(conn, text); err != nil {
		s.failAuth(conn, ip, now, "令牌读不全", err, false)
		return 0, false
	}

	raw, err := auth.DecodeToken(string(text))
	if err != nil {
		s.failAuth(conn, ip, now, "令牌格式不对", err, true)
		return 0, false
	}

	// 缓存键用令牌原文的摘要：只有验签通过的令牌才会被写进缓存，
	// 所以命中缓存等价于「这个令牌一分钟内验签过」。
	sum := sha256.Sum256(raw)
	var key [cacheKeyLen]byte
	copy(key[:], sum[:cacheKeyLen])

	if s.cfg.TokenCacheTTL > 0 {
		if uid, hit := s.cacheGet(key, now); hit {
			return uid, true
		}
	}

	tok, err := auth.ParseToken(s.cfg.PublicKey, raw, now)
	if err != nil {
		s.failAuth(conn, ip, now, "令牌不通过", err, true)
		return 0, false
	}

	state, err := s.db.CheckSession(auth.SessionHex(tok.SessionID), now)
	if err != nil || state.Revoked || state.Expired || state.Banned {
		s.bump(false)
		s.log.Info("登录态不可用", "user", tok.UserID, "remote", conn.RemoteAddr(),
			"revoked", state.Revoked, "expired", state.Expired, "banned", state.Banned)
		return 0, false
	}

	if s.cfg.TokenCacheTTL > 0 {
		s.cachePut(key, tok.UserID, now.Add(s.cfg.TokenCacheTTL))
	}
	return tok.UserID, true
}

// failAuth 统一处理认证失败：计数 + 关连接；`suspicious` 为真时才算坏令牌。
//
// 区分这两类很重要：「说得不对」和「没说话」不是一回事。前者是有人在试，
// 该罚；后者多半是网络抖了一下，把整个 NAT 出口封掉会伤到一整家网吧。
func (s *Server) failAuth(conn net.Conn, ip string, now time.Time, what string, err error, suspicious bool) {
	s.bump(false)
	s.log.Debug("连接未通过", "what", what, "remote", conn.RemoteAddr(), "err", err)
	if suspicious {
		s.punishBadToken(ip, now)
	}
}

// punishBadToken 累计坏令牌，超过阈值就把 IP 拉黑。
func (s *Server) punishBadToken(ip string, now time.Time) {
	if ip == "" {
		return
	}
	if s.badRate.Allow(ip, now) {
		return // 还没到阈值
	}

	// 只有「第一次拉黑」才写日志。
	// 否则持续刷的攻击者能让 Warn 打出几十万行 —— 被攻击时日志本身就是第二个
	// 放大点，磁盘写满比被打更难收拾。
	s.mu.Lock()
	_, already := s.bannedIP[ip]
	if !already {
		s.bannedIP[ip] = now.Add(s.cfg.BadTokenBan)
	}
	s.mu.Unlock()
	if already {
		return
	}
	s.log.Warn("坏令牌过多，暂时拒绝该来源",
		"ip", ip, "ban", s.cfg.BadTokenBan.String(), "threshold", s.cfg.BadTokenPerMinute)
}

// isBanned 判断来源是否在临时黑名单里，顺手清理过期条目。
func (s *Server) isBanned(ip string, now time.Time) bool {
	s.mu.Lock()
	defer s.mu.Unlock()

	if now.Sub(s.lastSweep) >= banSweepEvery {
		s.lastSweep = now
		for k, until := range s.bannedIP {
			if now.After(until) {
				delete(s.bannedIP, k)
			}
		}
	}

	until, ok := s.bannedIP[ip]
	return ok && now.Before(until)
}

func (s *Server) cacheGet(key [cacheKeyLen]byte, now time.Time) (int64, bool) {
	s.cacheMu.Lock()
	defer s.cacheMu.Unlock()
	e, ok := s.cache[key]
	if !ok || now.After(e.exp) {
		if ok {
			delete(s.cache, key)
		}
		return 0, false
	}
	return e.userID, true
}

func (s *Server) cachePut(key [cacheKeyLen]byte, userID int64, exp time.Time) {
	s.cacheMu.Lock()
	defer s.cacheMu.Unlock()
	// 硬上限：缓存不是无限容器，超了整体重来（丢掉缓存只是多查几次库）
	if len(s.cache) >= 20000 {
		s.cache = make(map[[cacheKeyLen]byte]cacheEntry, 1024)
	}
	s.cache[key] = cacheEntry{userID: userID, exp: exp}
}

func (s *Server) tryAcquireIP(ip string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.byIP[ip] >= s.cfg.MaxPerIP {
		return false
	}
	s.byIP[ip]++
	return true
}

func (s *Server) releaseIP(ip string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.byIP[ip] > 1 {
		s.byIP[ip]--
	} else {
		delete(s.byIP, ip)
	}
}

func (s *Server) tryAcquireUser(id int64) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.byUser[id] >= s.cfg.MaxPerDevice {
		return false
	}
	s.byUser[id]++
	return true
}

func (s *Server) releaseUser(id int64) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.byUser[id] > 1 {
		s.byUser[id]--
	} else {
		delete(s.byUser, id)
	}
}

// reject 统一出口：计数 + 记一笔。调用方负责关连接。
//
// 日志一律 Debug：被拒不是错误，是正常运行的一部分；量大时开 Info 会自己把
// 磁盘写满 —— 被攻击的时候，日志反而成了第二个放大器。
func (s *Server) reject(reason, ip string, userID int64) {
	s.bump(false)
	s.log.Debug("拒绝连接", "reason", reason, "ip", ip, "user", userID)
}

func (s *Server) bump(ok bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if ok {
		s.accepted++
	} else {
		s.rejected++
	}
}

// pipe 双向搬运。任意方向结束就把两条连接都关掉 —— easytier 断线时对端要尽快知道，
// 挂着半条连接比直接断更糟。
func pipe(a, b net.Conn) {
	var once sync.Once
	closeBoth := func() {
		once.Do(func() {
			_ = a.Close()
			_ = b.Close()
		})
	}

	done := make(chan struct{}, 2)
	go func() {
		_, _ = io.Copy(a, b)
		done <- struct{}{}
		closeBoth()
	}()
	go func() {
		_, _ = io.Copy(b, a)
		done <- struct{}{}
		closeBoth()
	}()
	<-done
	<-done
}

func hostOf(addr net.Addr) string {
	if addr == nil {
		return ""
	}
	if host, _, err := net.SplitHostPort(addr.String()); err == nil {
		return host
	}
	return addr.String()
}
