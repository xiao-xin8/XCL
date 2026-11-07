// Package auth 负责账号系统的三件底层事：
//
//  1. 登录令牌：服务端用 Ed25519 私钥签发，节点侧的桥接用公钥离线验签。
//     令牌里带 user_id 与 session_id —— 前者说明「你是谁」，后者让「退出登录 /
//     踢下线」能立刻生效（服务端把那条 session 标记作废就行，不必等令牌过期）。
//  2. 密码哈希：argon2id，不存明文、不存可逆摘要。
//  3. 输入校验：用户名与密码的规则集中在这里，注册和改密码共用一套。
package auth

import (
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"time"
)

const (
	// TokenVersion 是令牌布局的版本。字段一改就要加一，节点据此拒绝旧格式，
	// 而不是把字节错位解析成一个看起来合法的令牌。
	TokenVersion byte = 1

	// SessionIDLen 是会话 id 的字节数（hex 展示时 32 个字符）。
	SessionIDLen = 16

	tokenBodyLen = 1 + 8 + SessionIDLen + 8 // version + user id + session id + exp
	TokenSize    = tokenBodyLen + ed25519.SignatureSize
)

// 签名域的用途分隔符。同一把密钥以后可能还要签别的东西，
// 加域可以保证「一种用途的签名」不会被当成另一种用途。
const domainToken = "xcl-access-v1"

var (
	ErrTokenFormat  = errors.New("令牌格式不对")
	ErrTokenSig     = errors.New("令牌签名校验失败")
	ErrTokenExpired = errors.New("令牌已过期")
	ErrTokenVersion = errors.New("令牌版本不认识")
)

// Token 是解出来的登录令牌。
type Token struct {
	UserID    int64
	SessionID [SessionIDLen]byte
	Exp       int64 // unix 秒
}

// SignToken 签发令牌。
func SignToken(priv ed25519.PrivateKey, userID int64, session [SessionIDLen]byte, exp int64) []byte {
	sig := ed25519.Sign(priv, tokenSignedPayload(userID, session, exp))
	out := make([]byte, 0, TokenSize)
	out = append(out, tokenBody(userID, session, exp)...)
	return append(out, sig...)
}

// EncodeToken 把字节令牌编成文本形式（base64url，可直接放进 HTTP 头或 TCP 帧）。
func EncodeToken(raw []byte) string {
	return base64.RawURLEncoding.EncodeToString(raw)
}

// DecodeToken 解析文本令牌。
func DecodeToken(text string) ([]byte, error) {
	raw, err := base64.RawURLEncoding.DecodeString(text)
	if err != nil {
		return nil, ErrTokenFormat
	}
	if len(raw) != TokenSize {
		return nil, fmt.Errorf("%w：长度 %d，应为 %d", ErrTokenFormat, len(raw), TokenSize)
	}
	return raw, nil
}

// ParseToken 验签并检查有效期。
//
// `now` 显式传入（不内部读时钟），这样过期判定可以单测。
func ParseToken(pub ed25519.PublicKey, raw []byte, now time.Time) (Token, error) {
	if len(raw) != TokenSize {
		return Token{}, fmt.Errorf("%w：长度 %d，应为 %d", ErrTokenFormat, len(raw), TokenSize)
	}
	if raw[0] != TokenVersion {
		return Token{}, fmt.Errorf("%w：%d", ErrTokenVersion, raw[0])
	}

	var tok Token
	tok.UserID = int64(binary.BigEndian.Uint64(raw[1:9]))
	copy(tok.SessionID[:], raw[9:9+SessionIDLen])
	tok.Exp = int64(binary.BigEndian.Uint64(raw[9+SessionIDLen:]))

	if !ed25519.Verify(pub, tokenSignedPayload(tok.UserID, tok.SessionID, tok.Exp), raw[tokenBodyLen:]) {
		return Token{}, ErrTokenSig
	}
	// 先验签再判过期：签名不对的令牌不该因为「恰好过期」而得到一句更友好的错误，
	// 那等于给攻击者一个免费的探测信号。
	if tok.Exp <= now.Unix() {
		return Token{}, ErrTokenExpired
	}
	return tok, nil
}

func tokenBody(userID int64, session [SessionIDLen]byte, exp int64) []byte {
	body := make([]byte, 0, tokenBodyLen)
	body = append(body, TokenVersion)
	var user [8]byte
	binary.BigEndian.PutUint64(user[:], uint64(userID))
	body = append(body, user[:]...)
	body = append(body, session[:]...)
	var stamp [8]byte
	binary.BigEndian.PutUint64(stamp[:], uint64(exp))
	return append(body, stamp[:]...)
}

func tokenSignedPayload(userID int64, session [SessionIDLen]byte, exp int64) []byte {
	out := make([]byte, 0, len(domainToken)+1+tokenBodyLen)
	out = append(out, domainToken...)
	out = append(out, 0)
	return append(out, tokenBody(userID, session, exp)...)
}

// NewSessionID 随机生成一个会话 id。
func NewSessionID() ([SessionIDLen]byte, error) {
	var id [SessionIDLen]byte
	if _, err := rand.Read(id[:]); err != nil {
		return id, fmt.Errorf("生成会话 id：%w", err)
	}
	return id, nil
}

// SessionHex 把会话 id 编成数据库里用的形式。
func SessionHex(id [SessionIDLen]byte) string {
	return hex.EncodeToString(id[:])
}

// SessionFromHex 解析数据库里的会话 id。
func SessionFromHex(text string) ([SessionIDLen]byte, error) {
	var id [SessionIDLen]byte
	raw, err := hex.DecodeString(text)
	if err != nil || len(raw) != SessionIDLen {
		return id, fmt.Errorf("%w：会话 id %q", ErrTokenFormat, text)
	}
	copy(id[:], raw)
	return id, nil
}
