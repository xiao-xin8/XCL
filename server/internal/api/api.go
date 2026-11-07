// Package api 是账号服务的 HTTP 面。
//
// 只有一组接口：注册、登录、续期、退出、查自己。没有「账号列表」之类的对外接口 ——
// 管理动作全部走命令行（`xcld user ...`），服务器上不需要再开一套管理后台。
package api

import (
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"log/slog"
	"net"
	"net/http"
	"strings"
	"time"

	"github.com/xin-craft/xcl-server/internal/auth"
	"github.com/xin-craft/xcl-server/internal/captcha"
	"github.com/xin-craft/xcl-server/internal/limit"
	"github.com/xin-craft/xcl-server/internal/store"
)

// Config 是服务配置。
type Config struct {
	// SignKey 是签发登录令牌的 Ed25519 私钥，节点桥接持对应公钥离线验签。
	SignKey ed25519.PrivateKey
	// TokenTTL 是登录态有效期。默认 30 天。
	TokenTTL time.Duration
	// TrustForwarded 为 true 时用 CF-Connecting-IP / X-Forwarded-For 判断来源 IP。
	// 只有前面确实挂了可信反代才开，否则来源 IP 可被随意伪造，限流即失效。
	TrustForwarded bool

	// PowSeed 是签发 PoW 挑战的对称密钥（可以和别处复用，但建议独立）。
	PowSeed []byte
	// TurnstileSecret 非空表示启用 Cloudflare Turnstile。
	TurnstileSecret string
	// TurnstileSiteKey 只是转发给客户端渲染用，不是秘密。
	TurnstileSiteKey string
	// AllowPowFallback：Turnstile 服务不可达时是否接受 PoW 兜底。
	// 关掉它的效果是「Cloudflare 一挂，注册就全停」。
	AllowPowFallback bool

	// RegisterPerHour 每 IP 每小时的注册次数。
	RegisterPerHour int
	// RegisterPerDay 是全站每天的注册上限（按 UTC+8 的自然日算）。
	//
	// 它挡的不是洪水（那是 Cloudflare 的事），而是「慢慢把用户名占满」：
	// 一天三百个，涨得比真人玩家快得多，又不会一次性暴露成攻击。
	// 计数落在数据库里，进程重启不清零。
	RegisterPerDay int
	// LoginPerMinute 每 IP 每分钟的登录尝试次数。
	LoginPerMinute int
	// LoginPerUserPerMinute 每个用户名每分钟的登录尝试次数（防针对单个账号爆破）。
	LoginPerUserPerMinute int
	// MaxSessionsPerUser 单账号活跃登录态上限。超出时作废最旧的。
	//
	// 令牌是 bearer 凭据，发出去收不回来：不管的话一个账号登录一百次就有一百个
	// 能用的令牌，转手发给一百个人，桥接那边看到的全是「合法令牌」。
	MaxSessionsPerUser int

	// BridgeAdvertise 是下发给客户端的桥接地址，形如 `bridge.example.com:11010`。
	//
	// 必须由服务端下发，不能让客户端自己猜：账号接口挂在 Cloudflare 后面
	// （域名指向 CF 的 IP），而桥接是裸 TCP，必须直连源站。客户端要是拿账号域名
	// 加个端口去连，连到的是 CF 的边缘节点，永远连不上。
	//
	// 留空时不下发，客户端回退到「账号服务主机名:11010」（仅本地调试可用）。
	BridgeAdvertise string

	Logger *slog.Logger
}

// Server 是接口集合。
type Server struct {
	cfg       Config
	db        *store.Store
	pow       *captcha.Pow
	turnstile *captcha.Turnstile

	regLimiter   *limit.Limiter
	loginLimiter *limit.Limiter
	userLimiter  *limit.Limiter

	started time.Time
	log     *slog.Logger
}

// New 组装服务。
func New(cfg Config, db *store.Store) *Server {
	if cfg.TokenTTL <= 0 {
		cfg.TokenTTL = 30 * 24 * time.Hour
	}
	if cfg.RegisterPerHour <= 0 {
		cfg.RegisterPerHour = 10
	}
	if cfg.RegisterPerDay <= 0 {
		cfg.RegisterPerDay = 300
	}
	if cfg.LoginPerMinute <= 0 {
		cfg.LoginPerMinute = 10
	}
	if cfg.LoginPerUserPerMinute <= 0 {
		cfg.LoginPerUserPerMinute = 5
	}
	if cfg.MaxSessionsPerUser <= 0 {
		cfg.MaxSessionsPerUser = 3
	}
	if len(cfg.PowSeed) == 0 {
		panic("api: 必须提供 PowSeed")
	}
	if cfg.SignKey == nil {
		panic("api: 必须提供 SignKey")
	}
	log := cfg.Logger
	if log == nil {
		log = slog.Default()
	}

	s := &Server{
		cfg:          cfg,
		db:           db,
		pow:          captcha.NewPow(cfg.PowSeed),
		turnstile:    captcha.NewTurnstile(cfg.TurnstileSecret),
		regLimiter:   limit.New(time.Hour, cfg.RegisterPerHour).WithMaxKeys(50000),
		loginLimiter: limit.New(time.Minute, cfg.LoginPerMinute).WithMaxKeys(50000),
		userLimiter:  limit.New(time.Minute, cfg.LoginPerUserPerMinute).WithMaxKeys(20000),
		started:      time.Now(),
		log:          log,
	}
	if s.turnstile.Enabled() {
		log.Info("注册人机验证：Cloudflare Turnstile", "powFallback", cfg.AllowPowFallback)
	} else {
		log.Info("注册人机验证：自算工作量证明（未配置 Turnstile）")
	}
	return s
}

// SetLimitDisabled 一键关掉 HTTP 侧的全部频率限制（排障用）。
func (s *Server) SetLimitDisabled(v bool) {
	s.regLimiter.SetDisabled(v)
	s.loginLimiter.SetDisabled(v)
	s.userLimiter.SetDisabled(v)
}

// Handler 返回路由。
func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /v1/health", s.handleHealth)
	mux.HandleFunc("GET /v1/captcha", s.handleCaptcha)
	mux.HandleFunc("POST /v1/auth/register", s.handleRegister)
	mux.HandleFunc("POST /v1/auth/login", s.handleLogin)
	mux.HandleFunc("POST /v1/auth/refresh", s.handleRefresh)
	mux.HandleFunc("POST /v1/auth/logout", s.handleLogout)
	mux.HandleFunc("GET /v1/auth/me", s.handleMe)
	return mux
}

// ---------- 视图 ----------

type userView struct {
	ID        int64  `json:"id"`
	Username  string `json:"username"`
	CreatedAt int64  `json:"createdAt"`
	LastLogin int64  `json:"lastLoginAt"`
}

func viewOf(u store.User) userView {
	return userView{ID: u.ID, Username: u.Username, CreatedAt: u.CreatedAt, LastLogin: u.LastLoginAt}
}

type sessionResponse struct {
	Token     string   `json:"token"`
	ExpiresAt int64    `json:"expiresAt"`
	User      userView `json:"user"`
	// Bridge 是客户端应该去连的桥接地址（`host:port`）。
	// 账号接口走 Cloudflare，桥接走裸 TCP，两者地址不同，必须分开告诉客户端。
	Bridge string `json:"bridge,omitempty"`
}

type errorResponse struct {
	Code    string `json:"code"`
	Message string `json:"message"`
}

type captchaProof struct {
	// Provider 为 "turnstile" 或 "pow"；留空表示「按服务端配置来」。
	Provider  string `json:"provider"`
	Token     string `json:"token"`
	Challenge string `json:"challenge"`
	Nonce     string `json:"nonce"`
}

type registerRequest struct {
	Username string `json:"username"`
	Password string `json:"password"`
	// DeviceName 只是让账号在「登录设备」里认出自己，不参与鉴权。
	DeviceName string       `json:"deviceName"`
	Captcha    captchaProof `json:"captcha"`
}

type loginRequest struct {
	Username   string `json:"username"`
	Password   string `json:"password"`
	DeviceName string `json:"deviceName"`
}

// ---------- 注册 ----------

func (s *Server) handleRegister(w http.ResponseWriter, r *http.Request) {
	ip := s.clientIP(r)
	now := time.Now()
	if !s.regLimiter.Allow(ip, now) {
		s.writeError(w, http.StatusTooManyRequests, "rate_limited", "注册太频繁，请稍后再试")
		return
	}

	var req registerRequest
	if !s.decode(w, r, &req) {
		return
	}

	username := store.NormalizeUsername(req.Username)
	if err := auth.ValidateUsername(username); err != nil {
		s.writeError(w, http.StatusBadRequest, "bad_request", err.Error())
		return
	}
	if err := auth.ValidatePassword(req.Password); err != nil {
		s.writeError(w, http.StatusBadRequest, "bad_request", err.Error())
		return
	}

	// 全站日上限。放在人机验证之前：名额已经满了还让人算一遍 PoW，纯属浪费。
	// 查库失败时不拦截 —— 宁可放过去一个注册，也别因为统计出错把注册全停掉。
	if n, err := s.db.CountUsersSince(startOfDay(now).Unix()); err != nil {
		s.log.Warn("统计今日注册数失败", "err", err)
	} else if n >= s.cfg.RegisterPerDay {
		s.log.Warn("今日注册名额已满", "count", n, "limit", s.cfg.RegisterPerDay)
		s.writeError(w, http.StatusTooManyRequests, "rate_limited", "今天的注册名额已经用完了，明天再来")
		return
	}

	if !s.verifyCaptcha(w, req.Captcha, ip) {
		return
	}

	hash, err := auth.HashPassword(req.Password)
	if err != nil {
		s.log.Error("密码哈希失败", "err", err)
		s.writeError(w, http.StatusInternalServerError, "server_error", "服务暂时不可用")
		return
	}

	user, err := s.db.CreateUser(username, hash, now)
	if errors.Is(err, store.ErrUsernameTaken) {
		s.writeError(w, http.StatusConflict, "username_taken", "这个用户名已经有人用了")
		return
	}
	if err != nil {
		s.log.Error("建账号失败", "err", err)
		s.writeError(w, http.StatusInternalServerError, "server_error", "服务暂时不可用")
		return
	}

	s.log.Info("注册", "user", user.Username, "id", user.ID, "ip", ip)
	s.issueSession(w, user, req.DeviceName, ip)
}

// ---------- 登录 ----------

func (s *Server) handleLogin(w http.ResponseWriter, r *http.Request) {
	ip := s.clientIP(r)
	now := time.Now()
	if !s.loginLimiter.Allow(ip, now) {
		s.writeError(w, http.StatusTooManyRequests, "rate_limited", "尝试太频繁，请稍后再试")
		return
	}

	var req loginRequest
	if !s.decode(w, r, &req) {
		return
	}
	// 用户名会被当作限流表的 key，所以必须在上表之前卡住长度。
	// 否则一个 1 MB 的用户名就能在表里占一格，几万个就是内存耗尽 ——
	// 这跟有没有 Cloudflare 挡无关，是代码自己的洞。
	if len(req.Username) > maxUsernameInput || len(req.Password) > maxPasswordInput {
		s.writeError(w, http.StatusBadRequest, "bad_request", "用户名或密码格式不对")
		return
	}
	username := store.NormalizeUsername(req.Username)
	// 针对单个账号的爆破也限速：同一用户名每分钟 5 次
	if username != "" && !s.userLimiter.Allow(strings.ToLower(username), now) {
		s.writeError(w, http.StatusTooManyRequests, "rate_limited", "尝试太频繁，请稍后再试")
		return
	}

	user, hash, err := s.db.UserByName(username)
	if errors.Is(err, store.ErrNotFound) {
		// 统一回报「用户名或密码不对」，不告诉对方这个账号存不存在
		s.writeError(w, http.StatusUnauthorized, "bad_credentials", auth.ErrBadCredentials.Error())
		return
	}
	if err != nil {
		s.log.Error("查账号失败", "err", err)
		s.writeError(w, http.StatusInternalServerError, "server_error", "服务暂时不可用")
		return
	}

	if err := auth.VerifyPassword(hash, req.Password); err != nil {
		if errors.Is(err, auth.ErrBadHashFormat) {
			s.log.Error("库里的密码哈希坏了", "user", user.ID, "err", err)
			s.writeError(w, http.StatusInternalServerError, "server_error", "服务暂时不可用")
			return
		}
		s.log.Info("登录失败", "user", user.Username, "ip", ip)
		s.writeError(w, http.StatusUnauthorized, "bad_credentials", auth.ErrBadCredentials.Error())
		return
	}

	if user.Banned {
		s.log.Info("被封账号尝试登录", "user", user.Username, "reason", user.BannedReason)
		s.writeError(w, http.StatusForbidden, "banned", bannedMessage(user.BannedReason))
		return
	}

	_ = s.db.TouchLogin(user.ID, now)
	s.log.Info("登录", "user", user.Username, "id", user.ID, "ip", ip)
	s.issueSession(w, user, req.DeviceName, ip)
}

// ---------- 续期 / 退出 / 我 ----------

func (s *Server) handleRefresh(w http.ResponseWriter, r *http.Request) {
	sess, user, ok := s.current(w, r)
	if !ok {
		return
	}

	now := time.Now()
	exp := now.Add(s.cfg.TokenTTL).Unix()
	// 沿用同一个会话 id，只把有效期往后推：并发的几个请求不会因为
	// 「续期换了个 id」而突然变成未登录。
	if err := s.db.ExtendSession(sess.ID, exp, now); err != nil {
		s.writeError(w, http.StatusUnauthorized, "unauthorized", "登录已失效，请重新登录")
		return
	}
	sid, err := auth.SessionFromHex(sess.ID)
	if err != nil {
		s.log.Error("会话 id 损坏", "session", sess.ID, "err", err)
		s.writeError(w, http.StatusInternalServerError, "server_error", "服务暂时不可用")
		return
	}
	token := auth.SignToken(s.cfg.SignKey, user.ID, sid, exp)
	s.writeJSON(w, http.StatusOK, sessionResponse{
		Token:     auth.EncodeToken(token),
		ExpiresAt: exp,
		User:      viewOf(user),
		Bridge:    s.cfg.BridgeAdvertise,
	})
}

func (s *Server) handleLogout(w http.ResponseWriter, r *http.Request) {
	sess, _, ok := s.current(w, r)
	if !ok {
		return
	}
	if err := s.db.RevokeSession(sess.ID); err != nil {
		s.log.Error("作废会话失败", "err", err)
		s.writeError(w, http.StatusInternalServerError, "server_error", "服务暂时不可用")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) handleMe(w http.ResponseWriter, r *http.Request) {
	_, user, ok := s.current(w, r)
	if !ok {
		return
	}
	s.writeJSON(w, http.StatusOK, userView{
		ID:        user.ID,
		Username:  user.Username,
		CreatedAt: user.CreatedAt,
		LastLogin: user.LastLoginAt,
	})
}

// current 从 Authorization 头解析登录态，并确认会话与账号都还能用。
func (s *Server) current(w http.ResponseWriter, r *http.Request) (store.Session, store.User, bool) {
	raw, err := bearer(r)
	if err != nil {
		s.writeError(w, http.StatusUnauthorized, "unauthorized", "请先登录")
		return store.Session{}, store.User{}, false
	}
	tok, err := auth.ParseToken(s.cfg.SignKey.Public().(ed25519.PublicKey), raw, time.Now())
	if err != nil {
		s.writeError(w, http.StatusUnauthorized, "unauthorized", "登录已失效，请重新登录")
		return store.Session{}, store.User{}, false
	}

	sessionID := auth.SessionHex(tok.SessionID)
	state, err := s.db.CheckSession(sessionID, time.Now())
	if err != nil || state.Revoked || state.Expired || state.Banned {
		// 四种情况对外都是「请重新登录」：区分开只会帮到攻击者
		s.writeError(w, http.StatusUnauthorized, "unauthorized", "登录已失效，请重新登录")
		return store.Session{}, store.User{}, false
	}

	user, err := s.db.UserByID(tok.UserID)
	if err != nil {
		s.writeError(w, http.StatusUnauthorized, "unauthorized", "登录已失效，请重新登录")
		return store.Session{}, store.User{}, false
	}
	s.db.TouchSession(sessionID, time.Now())
	return store.Session{ID: sessionID, UserID: user.ID}, user, true
}

// ---------- 人机验证 ----------

func (s *Server) handleCaptcha(w http.ResponseWriter, _ *http.Request) {
	if s.turnstile.Enabled() {
		s.writeJSON(w, http.StatusOK, map[string]any{
			"provider": "turnstile",
			"siteKey":  s.cfg.TurnstileSiteKey,
		})
		return
	}
	challenge := s.pow.Issue(time.Now())
	if challenge == "" {
		s.writeError(w, http.StatusInternalServerError, "server_error", "服务暂时不可用")
		return
	}
	s.writeJSON(w, http.StatusOK, map[string]any{
		"provider":   "pow",
		"challenge":  challenge,
		"difficulty": captcha.Difficulty,
		"ttl":        int(captchaTTL.Seconds()),
	})
}

// verifyCaptcha 决定这次注册放不放行。返回 false 时已经把错误写回去了。
func (s *Server) verifyCaptcha(w http.ResponseWriter, proof captchaProof, ip string) bool {
	now := time.Now()

	wantTurnstile := proof.Provider == "turnstile" || (proof.Provider == "" && s.turnstile.Enabled())
	if wantTurnstile && s.turnstile.Enabled() {
		err := s.turnstile.Verify(proof.Token, ip)
		if err == nil {
			return true
		}
		// 只有「Cloudflare 没应答」才允许降级到 PoW；
		// 「token 不对」是没过验证，不能让他换个方式绕过去。
		if !errors.Is(err, captcha.ErrTurnstileDown) || !s.cfg.AllowPowFallback {
			s.log.Info("人机验证未通过", "provider", "turnstile", "ip", ip, "err", err)
			s.writeError(w, http.StatusBadRequest, "captcha_failed", "人机验证没通过，请重试")
			return false
		}
		s.log.Warn("Turnstile 不可达，改用工作量证明", "err", err)
	}

	if err := s.pow.Verify(proof.Challenge, proof.Nonce, now); err != nil {
		s.log.Info("工作量证明未通过", "ip", ip, "err", err)
		code := "captcha_failed"
		if errors.Is(err, captcha.ErrChallengeExpired) || errors.Is(err, captcha.ErrChallengeUsed) {
			code = "captcha_expired"
		}
		s.writeError(w, http.StatusBadRequest, code, err.Error())
		return false
	}
	return true
}

// ---------- 工具 ----------

// issueSession 建会话并签发令牌。
func (s *Server) issueSession(w http.ResponseWriter, user store.User, deviceName, ip string) {
	now := time.Now()
	exp := now.Add(s.cfg.TokenTTL).Unix()

	sid, err := auth.NewSessionID()
	if err != nil {
		s.log.Error("生成会话 id 失败", "err", err)
		s.writeError(w, http.StatusInternalServerError, "server_error", "服务暂时不可用")
		return
	}
	token := auth.SignToken(s.cfg.SignKey, user.ID, sid, exp)

	// 库里只存令牌的 sha256：即使数据库被拖走，也拿不到能直接用的令牌
	sum := sha256.Sum256(token)
	if err := s.db.CreateSession(store.Session{
		ID:         auth.SessionHex(sid),
		UserID:     user.ID,
		CreatedAt:  now.Unix(),
		ExpiresAt:  exp,
		LastSeenAt: now.Unix(),
		DeviceName: trim(deviceName, 64),
	}, hex.EncodeToString(sum[:])); err != nil {
		s.log.Error("写会话失败", "err", err, "ip", ip)
		s.writeError(w, http.StatusInternalServerError, "server_error", "服务暂时不可用")
		return
	}

	// 顶掉旧的登录态：令牌发出去就收不回来，只能从源头限制数量。
	// 失败不当成登录失败 —— 最坏情况是多留了一条有效登录态，不影响本次登录。
	if n, err := s.db.TrimSessions(user.ID, now, s.cfg.MaxSessionsPerUser); err != nil {
		s.log.Warn("清理旧登录态失败", "user", user.ID, "err", err)
	} else if n > 0 {
		s.log.Info("顶掉旧登录态", "user", user.ID, "count", n, "keep", s.cfg.MaxSessionsPerUser)
	}

	s.writeJSON(w, http.StatusOK, sessionResponse{
		Token:     auth.EncodeToken(token),
		ExpiresAt: exp,
		User:      viewOf(user),
		Bridge:    s.cfg.BridgeAdvertise,
	})
}

func bearer(r *http.Request) ([]byte, error) {
	header := r.Header.Get("Authorization")
	if header == "" {
		return nil, errors.New("缺少 Authorization")
	}
	text, ok := strings.CutPrefix(header, "Bearer ")
	if !ok {
		return nil, errors.New("Authorization 不是 Bearer")
	}
	return auth.DecodeToken(strings.TrimSpace(text))
}

func (s *Server) decode(w http.ResponseWriter, r *http.Request, into any) bool {
	// 请求体只有几百字节，卡死上限免得有人拿大包把内存顶满
	r.Body = http.MaxBytesReader(w, r.Body, 8<<10)
	dec := json.NewDecoder(r.Body)
	dec.DisallowUnknownFields()
	if err := dec.Decode(into); err != nil {
		s.writeError(w, http.StatusBadRequest, "bad_request", "请求体不是合法 JSON")
		return false
	}
	return true
}

func (s *Server) clientIP(r *http.Request) string {
	if s.cfg.TrustForwarded {
		if ip := r.Header.Get("CF-Connecting-IP"); ip != "" {
			return ip
		}
		if fwd := r.Header.Get("X-Forwarded-For"); fwd != "" {
			if first, _, ok := strings.Cut(fwd, ","); ok {
				return strings.TrimSpace(first)
			}
			return strings.TrimSpace(fwd)
		}
	}
	host, _, err := net.SplitHostPort(r.RemoteAddr)
	if err != nil {
		return r.RemoteAddr
	}
	return host
}

func (s *Server) writeJSON(w http.ResponseWriter, status int, body any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(status)
	if err := json.NewEncoder(w).Encode(body); err != nil {
		s.log.Debug("写响应失败", "err", err)
	}
}

func (s *Server) writeError(w http.ResponseWriter, status int, code, message string) {
	s.writeJSON(w, status, errorResponse{Code: code, Message: message})
}

func (s *Server) handleHealth(w http.ResponseWriter, _ *http.Request) {
	s.writeJSON(w, http.StatusOK, map[string]any{
		"status":   "ok",
		"users":    s.db.CountUsers(),
		"sessions": s.db.SessionCount(time.Now()),
		"uptime":   int64(time.Since(s.started).Seconds()),
	})
}

func bannedMessage(reason string) string {
	if strings.TrimSpace(reason) == "" {
		return "这个账号已被停用"
	}
	return "这个账号已被停用：" + reason
}

func trim(text string, max int) string {
	if len(text) <= max {
		return text
	}
	return text[:max]
}

// challengeTTL 与 captcha 包里的保持同步，只用于下发 TTL 给客户端。
const captchaTTL = 5 * time.Minute

// ---------- 限流 ----------

// 输入长度上限。
//
// 登录接口的用户名会直接当限流表的 key，不设上限等于把内存交给对方：
// 一个请求带一个超长用户名，几万个就是几百 MB，而且这些 key 要 60 秒才清。
const (
	maxUsernameInput = 64
	maxPasswordInput = 256
)

// cst 是注册日上限用的时区。
//
// 固定 UTC+8 而不是跟服务器本地时区走：换一台机器、改一次 TZ，
// 「今天」的定义就变了，配额会在跨时区迁移时莫名其妙地重置一次。
var cst = time.FixedZone("CST", 8*3600)

// startOfDay 返回「北京时间今天 0 点」，用作注册日上限的起点。
func startOfDay(now time.Time) time.Time {
	t := now.In(cst)
	return time.Date(t.Year(), t.Month(), t.Day(), 0, 0, 0, 0, cst)
}
