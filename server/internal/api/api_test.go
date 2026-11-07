package api

import (
	"bytes"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/xin-craft/xcl-server/internal/auth"
	"github.com/xin-craft/xcl-server/internal/captcha"
	"github.com/xin-craft/xcl-server/internal/store"
)

type harness struct {
	handler http.Handler
	db      *store.Store
	priv    ed25519.PrivateKey
}

func newHarness(t *testing.T) *harness {
	t.Helper()
	_, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatalf("生成密钥：%v", err)
	}
	seed := make([]byte, 32)
	if _, err := rand.Read(seed); err != nil {
		t.Fatalf("生成 seed：%v", err)
	}
	db, err := store.Open(t.TempDir() + "/xcl.db")
	if err != nil {
		t.Fatalf("打开数据库：%v", err)
	}
	t.Cleanup(func() { _ = db.Close() })

	srv := New(Config{
		SignKey:               priv,
		PowSeed:               seed,
		RegisterPerHour:       100,
		LoginPerMinute:        100,
		LoginPerUserPerMinute: 100,
	}, db)
	return &harness{handler: srv.Handler(), db: db, priv: priv}
}

func (h *harness) do(t *testing.T, method, path string, body any, token string) (*httptest.ResponseRecorder, map[string]any) {
	t.Helper()
	var reader *bytes.Reader
	if body == nil {
		reader = bytes.NewReader(nil)
	} else {
		raw, err := json.Marshal(body)
		if err != nil {
			t.Fatalf("序列化：%v", err)
		}
		reader = bytes.NewReader(raw)
	}
	req := httptest.NewRequest(method, path, reader)
	req.Header.Set("Content-Type", "application/json")
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	rec := httptest.NewRecorder()
	h.handler.ServeHTTP(rec, req)

	out := map[string]any{}
	_ = json.Unmarshal(rec.Body.Bytes(), &out)
	return rec, out
}

// captchaProof 走一遍真实流程：取题 → 解题 → 交答案。
func (h *harness) captchaProof(t *testing.T) map[string]any {
	t.Helper()
	rec, body := h.do(t, http.MethodGet, "/v1/captcha", nil, "")
	if rec.Code != http.StatusOK {
		t.Fatalf("取题失败 %d：%s", rec.Code, rec.Body.String())
	}
	challenge, _ := body["challenge"].(string)
	difficulty, _ := body["difficulty"].(float64)
	nonce, ok := captcha.Solve(challenge, int(difficulty), 1<<24)
	if !ok {
		t.Fatal("解不出题")
	}
	return map[string]any{"provider": "pow", "challenge": challenge, "nonce": nonce}
}

func (h *harness) register(t *testing.T, username, password string) (*httptest.ResponseRecorder, map[string]any) {
	t.Helper()
	return h.do(t, http.MethodPost, "/v1/auth/register", map[string]any{
		"username": username,
		"password": password,
		"captcha":  h.captchaProof(t),
	}, "")
}

func TestRegisterAndLogin(t *testing.T) {
	h := newHarness(t)

	rec, body := h.register(t, "xiaoxin", "password123")
	if rec.Code != http.StatusOK {
		t.Fatalf("注册应当成功，实际 %d：%s", rec.Code, rec.Body.String())
	}
	token, _ := body["token"].(string)
	if token == "" {
		t.Fatal("注册应当直接返回登录令牌")
	}
	// 令牌必须能被公钥验过，且带正确的账号 id
	raw, err := auth.DecodeToken(token)
	if err != nil {
		t.Fatalf("令牌解不开：%v", err)
	}
	tok, err := auth.ParseToken(h.priv.Public().(ed25519.PublicKey), raw, time.Now())
	if err != nil {
		t.Fatalf("令牌验签失败：%v", err)
	}
	user, _ := body["user"].(map[string]any)
	if int64(user["id"].(float64)) != tok.UserID {
		t.Fatal("令牌里的账号 id 和返回的不一致")
	}

	// 用同一个密码登录
	rec, login := h.do(t, http.MethodPost, "/v1/auth/login", map[string]any{
		"username": "xiaoxin", "password": "password123",
	}, "")
	if rec.Code != http.StatusOK {
		t.Fatalf("登录应当成功，实际 %d：%s", rec.Code, rec.Body.String())
	}
	if login["token"] == "" || login["token"] == token {
		t.Fatal("每次登录应当发新令牌（不同会话）")
	}
}

func TestRegisterValidatesInput(t *testing.T) {
	h := newHarness(t)

	// 用户名太短
	rec, _ := h.register(t, "ab", "password123")
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("短用户名应当 400，实际 %d", rec.Code)
	}
	// 用户名带中文
	rec, _ = h.register(t, "小鑫", "password123")
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("中文用户名应当 400，实际 %d", rec.Code)
	}
	// 密码太短
	rec, _ = h.register(t, "goodname", "1234567")
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("短密码应当 400，实际 %d", rec.Code)
	}
}

func TestRegisterRejectsDuplicate(t *testing.T) {
	h := newHarness(t)
	if rec, _ := h.register(t, "player", "password123"); rec.Code != http.StatusOK {
		t.Fatalf("第一次注册失败 %d", rec.Code)
	}
	rec, out := h.register(t, "PLAYER", "password456")
	if rec.Code != http.StatusConflict {
		t.Fatalf("重名应当 409，实际 %d", rec.Code)
	}
	if out["code"] != "username_taken" {
		t.Fatalf("错误码不对：%v", out["code"])
	}
}

func TestRegisterRequiresCaptcha(t *testing.T) {
	h := newHarness(t)

	// 不交验证
	rec, out := h.do(t, http.MethodPost, "/v1/auth/register", map[string]any{
		"username": "nocaptcha", "password": "password123",
	}, "")
	if rec.Code != http.StatusBadRequest || out["code"] != "captcha_failed" {
		t.Fatalf("没有验证应当被拒，实际 %d %v", rec.Code, out["code"])
	}

	// 交一份没解出来的
	rec, out = h.do(t, http.MethodPost, "/v1/auth/register", map[string]any{
		"username": "badcaptcha",
		"password": "password123",
		"captcha":  map[string]any{"provider": "pow", "challenge": "bad", "nonce": "0"},
	}, "")
	if rec.Code != http.StatusBadRequest || out["code"] != "captcha_failed" {
		t.Fatalf("伪造的挑战应当被拒，实际 %d %v", rec.Code, out["code"])
	}
}

func TestLoginRejectsWrongPassword(t *testing.T) {
	h := newHarness(t)
	if rec, _ := h.register(t, "alice", "password123"); rec.Code != http.StatusOK {
		t.Fatalf("注册失败 %d", rec.Code)
	}

	rec, out := h.do(t, http.MethodPost, "/v1/auth/login", map[string]any{
		"username": "alice", "password": "wrong-password",
	}, "")
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("错密码应当 401，实际 %d", rec.Code)
	}
	if out["code"] != "bad_credentials" {
		t.Fatalf("错误码不对：%v", out["code"])
	}

	// 不存在的用户名：必须给同样的错，不能暴露账号是否存在
	rec, out2 := h.do(t, http.MethodPost, "/v1/auth/login", map[string]any{
		"username": "nobody", "password": "password123",
	}, "")
	if rec.Code != http.StatusUnauthorized || out2["code"] != out["code"] {
		t.Fatalf("不存在的账号应当给同样的错误，实际 %d %v", rec.Code, out2["code"])
	}
}

func TestBannedUserCannotLogin(t *testing.T) {
	h := newHarness(t)
	rec, body := h.register(t, "cheater", "password123")
	if rec.Code != http.StatusOK {
		t.Fatalf("注册失败 %d", rec.Code)
	}
	token := body["token"].(string)

	user, _, _ := h.db.UserByName("cheater")
	if err := h.db.SetBanned(user.ID, true, "开挂"); err != nil {
		t.Fatalf("封号：%v", err)
	}

	rec, out := h.do(t, http.MethodPost, "/v1/auth/login", map[string]any{
		"username": "cheater", "password": "password123",
	}, "")
	if rec.Code != http.StatusForbidden || out["code"] != "banned" {
		t.Fatalf("封号后登录应当 403 banned，实际 %d %v", rec.Code, out["code"])
	}

	// 封号前拿到的令牌也必须立刻失效
	rec, _ = h.do(t, http.MethodGet, "/v1/auth/me", nil, token)
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("封号后旧令牌应当失效，实际 %d", rec.Code)
	}
}

func TestMeAndLogout(t *testing.T) {
	h := newHarness(t)
	_, body := h.register(t, "bob", "password123")
	token := body["token"].(string)

	rec, me := h.do(t, http.MethodGet, "/v1/auth/me", nil, token)
	if rec.Code != http.StatusOK {
		t.Fatalf("/me 应当 200，实际 %d", rec.Code)
	}
	if me["username"] != "bob" {
		t.Fatalf("用户名不对：%v", me["username"])
	}

	rec, _ = h.do(t, http.MethodPost, "/v1/auth/logout", nil, token)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("退出应当 204，实际 %d", rec.Code)
	}

	// 退出后同一个令牌不能再查
	rec, _ = h.do(t, http.MethodGet, "/v1/auth/me", nil, token)
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("退出后应当 401，实际 %d", rec.Code)
	}
}

func TestRefreshExtendsSession(t *testing.T) {
	h := newHarness(t)
	_, body := h.register(t, "carol", "password123")
	token := body["token"].(string)
	firstExp := int64(body["expiresAt"].(float64))

	time.Sleep(1100 * time.Millisecond) // 让新有效期至少差一秒
	rec, out := h.do(t, http.MethodPost, "/v1/auth/refresh", nil, token)
	if rec.Code != http.StatusOK {
		t.Fatalf("续期应当成功，实际 %d：%s", rec.Code, rec.Body.String())
	}
	if out["token"] == token {
		t.Fatal("续期应当换一个新令牌")
	}
	if int64(out["expiresAt"].(float64)) <= firstExp {
		t.Fatal("续期应当把有效期往后推")
	}

	// 新令牌可用
	rec, _ = h.do(t, http.MethodGet, "/v1/auth/me", nil, out["token"].(string))
	if rec.Code != http.StatusOK {
		t.Fatalf("续期后的令牌应当可用，实际 %d", rec.Code)
	}
}

func TestProtectedEndpointsNeedToken(t *testing.T) {
	h := newHarness(t)
	for _, path := range []string{"/v1/auth/me"} {
		rec, _ := h.do(t, http.MethodGet, path, nil, "")
		if rec.Code != http.StatusUnauthorized {
			t.Fatalf("%s 无令牌应当 401，实际 %d", path, rec.Code)
		}
	}
	rec, _ := h.do(t, http.MethodPost, "/v1/auth/refresh", nil, "不是令牌")
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("乱码令牌应当 401，实际 %d", rec.Code)
	}
}

func TestHealth(t *testing.T) {
	h := newHarness(t)
	rec, body := h.do(t, http.MethodGet, "/v1/health", nil, "")
	if rec.Code != http.StatusOK {
		t.Fatalf("健康检查应当 200，实际 %d", rec.Code)
	}
	if body["status"] != "ok" {
		t.Fatalf("状态不对：%v", body["status"])
	}
}

func TestRateLimitOnLogin(t *testing.T) {
	// 单独造一个低配服务：默认的限流阈值太高，测不出来
	_, priv, _ := ed25519.GenerateKey(rand.Reader)
	seed := make([]byte, 32)
	_, _ = rand.Read(seed)
	db, _ := store.Open(t.TempDir() + "/rl.db")
	defer db.Close()
	limited := New(Config{SignKey: priv, PowSeed: seed, LoginPerMinute: 3}, db)
	h := &harness{handler: limited.Handler(), db: db, priv: priv}

	var last int
	for i := 0; i < 5; i++ {
		rec, _ := h.do(t, http.MethodPost, "/v1/auth/login", map[string]any{
			"username": "whoever", "password": "password123",
		}, "")
		last = rec.Code
	}
	if last != http.StatusTooManyRequests {
		t.Fatalf("超出频率应当 429，实际 %d", last)
	}
}
