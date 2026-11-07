package bridge

import (
	"crypto/ed25519"
	"crypto/rand"
	"encoding/binary"
	"io"
	"net"
	"testing"
	"time"

	"github.com/xin-craft/xcl-server/internal/auth"
	"github.com/xin-craft/xcl-server/internal/store"
)

type fixture struct {
	bridge    *Server
	db        *store.Store
	priv      ed25519.PrivateKey
	addr      string
	user      store.User
	sessionID string
}

// startEcho 起一个「本机 easytier core」的替身：收到什么原样吐回来。
func startEcho(t *testing.T) string {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("起回显服务：%v", err)
	}
	t.Cleanup(func() { _ = ln.Close() })
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			go func(c net.Conn) {
				defer c.Close()
				_, _ = io.Copy(c, c)
			}(conn)
		}
	}()
	return ln.Addr().String()
}

func newFixture(t *testing.T) *fixture {
	t.Helper()
	return newFixtureWith(t, nil)
}

// newFixtureWith 允许在起服务之前改配置，用来测各种上限。
func newFixtureWith(t *testing.T, tune func(*Config)) *fixture {
	t.Helper()
	_, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatalf("生成密钥：%v", err)
	}
	db, err := store.Open(t.TempDir() + "/xcl.db")
	if err != nil {
		t.Fatalf("打开数据库：%v", err)
	}
	t.Cleanup(func() { _ = db.Close() })

	now := time.Now()
	user, err := db.CreateUser("player", "hash", now)
	if err != nil {
		t.Fatalf("建账号：%v", err)
	}
	sid, err := auth.NewSessionID()
	if err != nil {
		t.Fatalf("生成会话：%v", err)
	}
	if err := db.CreateSession(store.Session{
		ID: auth.SessionHex(sid), UserID: user.ID,
		CreatedAt: now.Unix(), ExpiresAt: now.Add(time.Hour).Unix(), LastSeenAt: now.Unix(),
	}, "hash"); err != nil {
		t.Fatalf("写会话：%v", err)
	}

	cfg := Config{
		Listen:    "127.0.0.1:0",
		Upstream:  startEcho(t),
		PublicKey: priv.Public().(ed25519.PublicKey),
		// 认证超时压到 300ms，否则「连上不说话」的用例要等满 10 秒
		AuthTimeout: 300 * time.Millisecond,
	}
	if tune != nil {
		tune(&cfg)
	}
	srv := New(cfg, db)
	if err := srv.Listen(); err != nil {
		t.Fatalf("监听：%v", err)
	}
	t.Cleanup(func() { _ = srv.Close() })

	return &fixture{
		bridge:    srv,
		db:        db,
		priv:      priv,
		addr:      srv.Addr().String(),
		user:      user,
		sessionID: auth.SessionHex(sid),
	}
}

func (f *fixture) token(t *testing.T, exp time.Time) string {
	t.Helper()
	sid, err := auth.SessionFromHex(f.sessionID)
	if err != nil {
		t.Fatalf("会话 id：%v", err)
	}
	return auth.EncodeToken(auth.SignToken(f.priv, f.user.ID, sid, exp.Unix()))
}

// send 连上去发一帧，返回连接和读到的首字节数（0 表示被直接拒绝）。
func (f *fixture) send(t *testing.T, token string) (net.Conn, []byte) {
	t.Helper()
	conn, err := net.DialTimeout("tcp", f.addr, 3*time.Second)
	if err != nil {
		t.Fatalf("连桥接：%v", err)
	}
	if err := conn.SetDeadline(time.Now().Add(3 * time.Second)); err != nil {
		t.Fatalf("设超时：%v", err)
	}

	if token != "" {
		var header [6]byte
		copy(header[:4], magic[:])
		binary.BigEndian.PutUint16(header[4:], uint16(len(token)))
		if _, err := conn.Write(append(header[:], token...)); err != nil {
			t.Fatalf("发帧：%v", err)
		}
	}

	buf := make([]byte, 16)
	n, err := conn.Read(buf)
	if err != nil {
		return conn, nil
	}
	return conn, buf[:n]
}

func TestValidTokenGetsTunnel(t *testing.T) {
	f := newFixture(t)
	conn, first := f.send(t, f.token(t, time.Now().Add(time.Hour)))
	defer conn.Close()

	if len(first) != 1 || first[0] != 0x00 {
		t.Fatalf("应当回一个 0x00 表示通过，实际 %v", first)
	}

	// 通过之后就是透明管道
	if _, err := conn.Write([]byte("hello easytier")); err != nil {
		t.Fatalf("写：%v", err)
	}
	buf := make([]byte, 32)
	n, err := conn.Read(buf)
	if err != nil {
		t.Fatalf("读回显：%v", err)
	}
	if string(buf[:n]) != "hello easytier" {
		t.Fatalf("回显不一致：%q", buf[:n])
	}

	accepted, rejected := f.bridge.Stats()
	if accepted != 1 || rejected != 0 {
		t.Fatalf("统计不对：accepted=%d rejected=%d", accepted, rejected)
	}
}

func TestNoTokenIsDroppedSilently(t *testing.T) {
	f := newFixture(t)

	// 连上但一个字节都不发（扫描器的典型行为）
	conn, err := net.DialTimeout("tcp", f.addr, 3*time.Second)
	if err != nil {
		t.Fatalf("连桥接：%v", err)
	}
	defer conn.Close()
	if err := conn.SetDeadline(time.Now().Add(3 * time.Second)); err != nil {
		t.Fatalf("设超时：%v", err)
	}

	// 等认证超时把它踢掉
	time.Sleep(600 * time.Millisecond)

	// 关键：一个字节都不回。让对方连「这个端口后面是什么」都问不出来。
	buf := make([]byte, 16)
	if n, err := conn.Read(buf); err == nil && n > 0 {
		t.Fatalf("无令牌不该有任何响应，实际收到 %v", buf[:n])
	}
	_, rejected := f.bridge.Stats()
	if rejected != 1 {
		t.Fatalf("应当记一次拒绝，实际 %d", rejected)
	}
}

func TestExpiredTokenIsDropped(t *testing.T) {
	f := newFixture(t)
	// 签名合法但已过期
	conn, first := f.send(t, f.token(t, time.Now().Add(-time.Minute)))
	defer conn.Close()
	if len(first) != 0 {
		t.Fatalf("过期令牌不该有任何响应，实际 %v", first)
	}
}

func TestForgedTokenIsDropped(t *testing.T) {
	f := newFixture(t)
	// 换一把私钥签出来的令牌：验签必须失败
	_, otherPriv, _ := ed25519.GenerateKey(rand.Reader)
	sid, _ := auth.SessionFromHex(f.sessionID)
	forged := auth.EncodeToken(auth.SignToken(otherPriv, f.user.ID, sid, time.Now().Add(time.Hour).Unix()))

	conn, first := f.send(t, forged)
	defer conn.Close()
	if len(first) != 0 {
		t.Fatalf("伪造令牌不该有任何响应，实际 %v", first)
	}
}

func TestRevokedSessionIsDropped(t *testing.T) {
	f := newFixture(t)
	token := f.token(t, time.Now().Add(time.Hour))

	// 退出登录（或封号）之后，同一个令牌必须立刻失效
	if err := f.db.RevokeSession(f.sessionID); err != nil {
		t.Fatalf("作废会话：%v", err)
	}

	conn, first := f.send(t, token)
	defer conn.Close()
	if len(first) != 0 {
		t.Fatalf("已作废的会话不该被放行，实际 %v", first)
	}
}

func TestBannedUserIsDropped(t *testing.T) {
	f := newFixture(t)
	token := f.token(t, time.Now().Add(time.Hour))

	if err := f.db.SetBanned(f.user.ID, true, "测试"); err != nil {
		t.Fatalf("封号：%v", err)
	}

	conn, first := f.send(t, token)
	defer conn.Close()
	if len(first) != 0 {
		t.Fatalf("被封账号不该被放行，实际 %v", first)
	}
}

func TestGarbageFirstFrameIsDropped(t *testing.T) {
	f := newFixture(t)
	conn, err := net.DialTimeout("tcp", f.addr, 3*time.Second)
	if err != nil {
		t.Fatalf("连桥接：%v", err)
	}
	defer conn.Close()
	if err := conn.SetDeadline(time.Now().Add(3 * time.Second)); err != nil {
		t.Fatalf("设超时：%v", err)
	}

	// 比如有人拿 HTTP 请求打这个端口
	if _, err := conn.Write([]byte("GET / HTTP/1.1\r\n\r\n")); err != nil {
		t.Fatalf("写：%v", err)
	}
	buf := make([]byte, 16)
	if n, err := conn.Read(buf); err == nil && n > 0 {
		t.Fatalf("不该有任何响应，实际 %v", buf[:n])
	}
}

// ---------- 门卫 ----------

// 同一个账号开太多连接要被挡。easytier 一个节点 3~6 条，8 是留了重连余量。
func TestPerDeviceConcurrencyLimit(t *testing.T) {
	f := newFixtureWith(t, func(c *Config) { c.MaxPerDevice = 1 })
	tok := f.token(t, time.Now().Add(time.Hour))

	conn1, first1 := f.send(t, tok)
	defer conn1.Close()
	if len(first1) != 1 {
		t.Fatalf("第一条应当放行，实际 %v", first1)
	}

	conn2, first2 := f.send(t, tok)
	defer conn2.Close()
	if len(first2) != 0 {
		t.Fatalf("超出账号并发上限应当被静默丢弃，实际 %v", first2)
	}
}

// 同一个 IP 开太多连接要被挡。这个值必须远大于账号维度：
// 网吧/学校几十个人共用出口 IP，按账号维度定会把真人挡在门外。
func TestPerIPConcurrencyLimit(t *testing.T) {
	f := newFixtureWith(t, func(c *Config) { c.MaxPerIP = 1 })
	tok := f.token(t, time.Now().Add(time.Hour))

	conn1, first1 := f.send(t, tok)
	defer conn1.Close()
	if len(first1) != 1 {
		t.Fatalf("第一条应当放行，实际 %v", first1)
	}

	conn2, first2 := f.send(t, tok)
	defer conn2.Close()
	if len(first2) != 0 {
		t.Fatalf("超出 IP 并发上限应当被静默丢弃，实际 %v", first2)
	}
}

// 建连速率：不是为了省资源，是为了发现「一个账号在疯狂重连」。
func TestDeviceRateLimit(t *testing.T) {
	f := newFixtureWith(t, func(c *Config) {
		c.DeviceRate = 1
		c.RateWindow = time.Minute
	})
	tok := f.token(t, time.Now().Add(time.Hour))

	conn1, first1 := f.send(t, tok)
	if len(first1) != 1 {
		t.Fatalf("第一次应当放行，实际 %v", first1)
	}
	_ = conn1.Close()

	conn2, first2 := f.send(t, tok)
	defer conn2.Close()
	if len(first2) != 0 {
		t.Fatalf("超出建连速率应当被静默丢弃，实际 %v", first2)
	}
}

// 坏令牌不能白试：同一个 IP 短时间内试太多次，拉黑一段时间。
func TestBadTokenBansIP(t *testing.T) {
	f := newFixtureWith(t, func(c *Config) {
		c.BadTokenPerMinute = 2
		c.BadTokenBan = time.Minute
	})

	for i := 0; i < 3; i++ {
		conn, first := f.send(t, "not-a-real-token")
		_ = conn.Close()
		if len(first) != 0 {
			t.Fatalf("第 %d 次坏令牌不该有响应，实际 %v", i+1, first)
		}
	}

	// 拉黑之后，连合法令牌也进不来 —— 这就是「不理你」的效果
	conn, first := f.send(t, f.token(t, time.Now().Add(time.Hour)))
	defer conn.Close()
	if len(first) != 0 {
		t.Fatalf("被拉黑的 IP 应当被静默丢弃，实际 %v", first)
	}
}

// 令牌缓存是刻意的取舍：一分钟内不重复查库，代价是封号最多延迟一分钟生效。
// 这个用例把行为钉死，避免以后有人「顺手」把缓存去掉让洪水把库打满。
func TestTokenCacheHidesRevokeWithinTTL(t *testing.T) {
	f := newFixtureWith(t, func(c *Config) { c.TokenCacheTTL = time.Minute })
	tok := f.token(t, time.Now().Add(time.Hour))

	conn1, first1 := f.send(t, tok)
	if len(first1) != 1 {
		t.Fatalf("首次应当放行，实际 %v", first1)
	}
	_ = conn1.Close()

	if err := f.db.RevokeSession(f.sessionID); err != nil {
		t.Fatalf("作废会话：%v", err)
	}

	conn2, first2 := f.send(t, tok)
	defer conn2.Close()
	if len(first2) != 1 {
		t.Fatalf("缓存有效期内应当仍然放行（已知取舍），实际 %v", first2)
	}

	// 清掉缓存之后必须立刻生效 —— 保证「严格」这条路还在
	f.bridge.cacheMu.Lock()
	f.bridge.cache = make(map[[cacheKeyLen]byte]cacheEntry)
	f.bridge.cacheMu.Unlock()

	conn3, first3 := f.send(t, tok)
	defer conn3.Close()
	if len(first3) != 0 {
		t.Fatalf("缓存清掉后应当立刻拒绝，实际 %v", first3)
	}
}
