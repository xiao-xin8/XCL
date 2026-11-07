package auth

import (
	"crypto/ed25519"
	"crypto/rand"
	"errors"
	"strings"
	"testing"
	"time"
)

func newKey(t *testing.T) (ed25519.PrivateKey, ed25519.PublicKey) {
	t.Helper()
	pub, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatalf("生成密钥：%v", err)
	}
	return priv, pub
}

func TestTokenRoundTrip(t *testing.T) {
	priv, pub := newKey(t)
	now := time.Unix(1_800_000_000, 0)

	var session [SessionIDLen]byte
	copy(session[:], "0123456789abcdef")
	raw := SignToken(priv, 42, session, now.Add(24*time.Hour).Unix())

	tok, err := ParseToken(pub, raw, now)
	if err != nil {
		t.Fatalf("应当解析成功：%v", err)
	}
	if tok.UserID != 42 {
		t.Fatalf("账号 id 不对：%d", tok.UserID)
	}
	if tok.SessionID != session {
		t.Fatalf("会话 id 不对：%x", tok.SessionID)
	}
	if tok.Exp != now.Add(24*time.Hour).Unix() {
		t.Fatalf("过期时间不对：%d", tok.Exp)
	}
}

func TestTokenTamperIsRejected(t *testing.T) {
	priv, pub := newKey(t)
	now := time.Unix(1_800_000_000, 0)
	raw := SignToken(priv, 7, [SessionIDLen]byte{1, 2, 3}, now.Add(time.Hour).Unix())

	// 换账号 id：等于把自己变成别人
	bad := append([]byte(nil), raw...)
	bad[8] ^= 0xff
	if _, err := ParseToken(pub, bad, now); !errors.Is(err, ErrTokenSig) {
		t.Fatalf("改过账号 id 的令牌应当验签失败，实际 %v", err)
	}

	// 延长有效期
	bad = append([]byte(nil), raw...)
	bad[1+8+SessionIDLen] ^= 0x01
	if _, err := ParseToken(pub, bad, now); !errors.Is(err, ErrTokenSig) {
		t.Fatalf("改过过期时间的令牌应当验签失败，实际 %v", err)
	}

	// 换会话 id：等于绕开「退出登录」
	bad = append([]byte(nil), raw...)
	bad[9] ^= 0xff
	if _, err := ParseToken(pub, bad, now); !errors.Is(err, ErrTokenSig) {
		t.Fatalf("改过会话 id 的令牌应当验签失败，实际 %v", err)
	}
}

func TestTokenExpired(t *testing.T) {
	priv, pub := newKey(t)
	now := time.Unix(1_800_000_000, 0)
	raw := SignToken(priv, 1, [SessionIDLen]byte{}, now.Add(-time.Second).Unix())

	if _, err := ParseToken(pub, raw, now); !errors.Is(err, ErrTokenExpired) {
		t.Fatalf("过期令牌应当被拒，实际 %v", err)
	}
}

func TestTokenVersionMismatch(t *testing.T) {
	priv, pub := newKey(t)
	now := time.Unix(1_800_000_000, 0)
	raw := SignToken(priv, 1, [SessionIDLen]byte{}, now.Add(time.Hour).Unix())
	raw[0] = 99

	if _, err := ParseToken(pub, raw, now); !errors.Is(err, ErrTokenVersion) {
		t.Fatalf("版本不符应当被拒，实际 %v", err)
	}
}

func TestTokenTextRoundTrip(t *testing.T) {
	priv, _ := newKey(t)
	raw := SignToken(priv, 3, [SessionIDLen]byte{}, time.Now().Add(time.Hour).Unix())

	text := EncodeToken(raw)
	back, err := DecodeToken(text)
	if err != nil {
		t.Fatalf("解码失败：%v", err)
	}
	if string(back) != string(raw) {
		t.Fatal("编解码后内容不一致")
	}
	if _, err := DecodeToken("!!!"); !errors.Is(err, ErrTokenFormat) {
		t.Fatalf("非法文本应当报格式错误，实际 %v", err)
	}
	if _, err := DecodeToken(EncodeToken(raw[:10])); !errors.Is(err, ErrTokenFormat) {
		t.Fatalf("长度不足应当报格式错误，实际 %v", err)
	}
}

func TestSessionHexRoundTrip(t *testing.T) {
	id, err := NewSessionID()
	if err != nil {
		t.Fatalf("生成会话 id：%v", err)
	}
	back, err := SessionFromHex(SessionHex(id))
	if err != nil {
		t.Fatalf("解析会话 id：%v", err)
	}
	if back != id {
		t.Fatal("会话 id 往返不一致")
	}
	if _, err := SessionFromHex("zz"); err == nil {
		t.Fatal("非法 hex 应当报错")
	}
}

// ---------- 密码 ----------

func TestHashAndVerify(t *testing.T) {
	hash, err := HashPassword("correct horse battery")
	if err != nil {
		t.Fatalf("哈希：%v", err)
	}
	if !strings.HasPrefix(hash, "$argon2id$") {
		t.Fatalf("应当是 PHC 格式：%s", hash)
	}
	if strings.Contains(hash, "correct horse") {
		t.Fatal("哈希串里不该出现明文")
	}

	if err := VerifyPassword(hash, "correct horse battery"); err != nil {
		t.Fatalf("正确密码应当通过：%v", err)
	}
	if err := VerifyPassword(hash, "wrong password"); !errors.Is(err, ErrBadCredentials) {
		t.Fatalf("错误密码应当被拒，实际 %v", err)
	}
}

func TestHashIsSalted(t *testing.T) {
	a, _ := HashPassword("same-password")
	b, _ := HashPassword("same-password")
	if a == b {
		t.Fatal("同一个密码两次哈希必须不同（盐要随机）")
	}
	if err := VerifyPassword(a, "same-password"); err != nil {
		t.Fatalf("a 应当能验过：%v", err)
	}
	if err := VerifyPassword(b, "same-password"); err != nil {
		t.Fatalf("b 应当能验过：%v", err)
	}
}

func TestVerifyRejectsBrokenHash(t *testing.T) {
	for _, broken := range []string{
		"",
		"plaintext",
		"$bcrypt$v=19$m=1,t=1,p=1$aaa$bbb",
		"$argon2id$v=19$m=65536,t=3,p=2$不是base64$xxx",
		"$argon2id$v=99$m=65536,t=3,p=2$YWJj$ZGVm",
	} {
		if err := VerifyPassword(broken, "whatever"); !errors.Is(err, ErrBadHashFormat) {
			t.Fatalf("%q 应当报格式错误，实际 %v", broken, err)
		}
	}
}

// ---------- 输入规则 ----------

func TestValidateUsername(t *testing.T) {
	valid := []string{"abc", "Xiao_Xin", "player-1", strings.Repeat("a", UsernameMaxLen)}
	for _, name := range valid {
		if err := ValidateUsername(name); err != nil {
			t.Fatalf("%q 应当合法：%v", name, err)
		}
	}
	invalid := []string{"", "ab", "小鑫", "has space", "emoji😀", "semi;colon", strings.Repeat("a", UsernameMaxLen+1)}
	for _, name := range invalid {
		if err := ValidateUsername(name); err == nil {
			t.Fatalf("%q 应当被拒", name)
		}
	}
}

func TestValidatePassword(t *testing.T) {
	if err := ValidatePassword("12345678"); err != nil {
		t.Fatalf("8 位应当合法：%v", err)
	}
	if err := ValidatePassword("1234567"); err == nil {
		t.Fatal("7 位应当被拒")
	}
	if err := ValidatePassword(strings.Repeat("a", PasswordMaxLen+1)); err == nil {
		t.Fatal("超长密码应当被拒（argon2 的开销会被放大）")
	}
}
