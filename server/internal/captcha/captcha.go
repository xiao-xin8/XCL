// Package captcha 是注册接口的「你真的是人吗」这一关。
//
// 两档实现：
//
//   - Turnstile（Cloudflare）：业界最省事，但要在客户端 WebView 里加载
//     challenges.cloudflare.com 的脚本 —— WebView2 有可能被判成异常环境，
//     国内网络访问也不一定通。所以它不能是唯一的路。
//   - 工作量证明（PoW）：服务端发一道难题，客户端算一会儿再提交。
//     无外部依赖、无网络、WebView 完全无感；挡脚本够用。
//
// 服务端的策略是「配了 Turnstile 就用它，Turnstile 本身不可达时才退回 PoW」，
// 而不是「两个都收」—— 都收等于谁都能挑简单的那条走，等于没有。
package captcha

import (
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"time"
)

var (
	ErrChallengeFormat = errors.New("挑战格式不对")
	ErrChallengeExpired = errors.New("挑战已过期，请刷新重试")
	ErrChallengeUsed   = errors.New("挑战已使用过，请刷新重试")
	ErrProofOfWork     = errors.New("工作量证明不满足要求")
	ErrTurnstile       = errors.New("人机验证未通过")
	ErrTurnstileDown   = errors.New("人机验证服务暂时不可用")
)

const (
	powDomain = "xcl-pow-v1"
	// challengeTTL 是挑战的有效期。够慢慢算，也不至于让一份挑战
	// 被囤起来批量用。
	challengeTTL = 5 * time.Minute
	// Difficulty 是要求的前导零位数（bit）。
	// 20 bit ≈ 一百万次 sha256，普通机器 0.3~1 秒 —— 正常人无感，脚本要付 5 倍代价。
	Difficulty = 20
)

// Pow 是自算难题。
type Pow struct {
	seed []byte
	// used 记住已经兑过的挑战，防止同一份挑战被复用无数次。
	used map[string]int64
}

// NewPow 创建发题器。
func NewPow(seed []byte) *Pow {
	return &Pow{seed: seed, used: make(map[string]int64)}
}

// Issue 发一道新题。
//
// 挑战自包含（随机数 + 时间戳 + 截断的 HMAC），服务端验证时不需要查表 ——
// 进程重启也不会把正在算题的全废掉。
func (p *Pow) Issue(now time.Time) string {
	random := make([]byte, 16)
	if _, err := rand.Read(random); err != nil {
		// 随机数取不到就没法发题了；这种时候宁可让注册失败，也别发一个可预测的挑战
		return ""
	}
	return p.pack(random, now)
}

func (p *Pow) pack(random []byte, now time.Time) string {
	var stamp [8]byte
	binary.BigEndian.PutUint64(stamp[:], uint64(now.Unix()))

	payload := make([]byte, 0, 16+8+8)
	payload = append(payload, random...)
	payload = append(payload, stamp[:]...)
	payload = append(payload, p.mac(random, stamp[:])...)
	return base64.RawURLEncoding.EncodeToString(payload)
}

func (p *Pow) mac(random, stamp []byte) []byte {
	m := hmac.New(sha256.New, p.seed)
	m.Write([]byte(powDomain))
	m.Write([]byte{0})
	m.Write(random)
	m.Write(stamp)
	sum := m.Sum(nil)
	return sum[:8]
}

// Verify 校验「挑战 + 解答」。
func (p *Pow) Verify(challenge, nonce string, now time.Time) error {
	raw, err := base64.RawURLEncoding.DecodeString(challenge)
	if err != nil || len(raw) != 32 {
		return ErrChallengeFormat
	}
	random, stamp, mac := raw[:16], raw[16:24], raw[24:32]
	if !hmac.Equal(mac, p.mac(random, stamp)) {
		return ErrChallengeFormat
	}
	issued := int64(binary.BigEndian.Uint64(stamp))
	if now.Sub(time.Unix(issued, 0)) > challengeTTL {
		return ErrChallengeExpired
	}

	// 同一份挑战只能兑一次：否则算出一道题就能无限注册
	p.gc(now)
	if _, seen := p.used[challenge]; seen {
		return ErrChallengeUsed
	}

	if !meetsDifficulty(challenge, nonce, Difficulty) {
		return ErrProofOfWork
	}
	p.used[challenge] = issued
	return nil
}

func (p *Pow) gc(now time.Time) {
	cutoff := now.Add(-2 * challengeTTL).Unix()
	for key, issued := range p.used {
		if issued < cutoff {
			delete(p.used, key)
		}
	}
}

// meetsDifficulty 判断 sha256(challenge || nonce) 的前 Difficulty 位是否全为 0。
//
// 客户端从 "0"、"1"、"2"… 往上试 hex 计数器即可；用字符串是为了两端实现简单，
// 序列化也天然一致。
func meetsDifficulty(challenge, nonce string, bits int) bool {
	sum := sha256.Sum256([]byte(challenge + "|" + nonce))
	full := bits / 8
	for i := 0; i < full; i++ {
		if sum[i] != 0 {
			return false
		}
	}
	rem := bits % 8
	if rem == 0 {
		return true
	}
	return sum[full]>>(8-rem) == 0
}

// ---------- Turnstile ----------

// Turnstile 校验 Cloudflare 的验证码结果。
type Turnstile struct {
	secret string
	client *http.Client
}

// NewTurnstile 创建校验器。secret 为空表示没启用。
func NewTurnstile(secret string) *Turnstile {
	return &Turnstile{
		secret: secret,
		// 注册接口不该被 Cloudflare 拖住：5 秒就是上限
		client: &http.Client{Timeout: 5 * time.Second},
	}
}

// Enabled 表示是否配置了 Turnstile。
func (t *Turnstile) Enabled() bool { return strings.TrimSpace(t.secret) != "" }

type turnstileResponse struct {
	Success    bool     `json:"success"`
	ErrorCodes []string `json:"error-codes"`
}

// Verify 调 siteverify。
//
// 返回 ErrTurnstileDown（而不是 ErrTurnstile）表示「服务本身没应答」，
// 调用方据此决定要不要退回 PoW —— 算错了不该得到降级，服务挂了才该。
func (t *Turnstile) Verify(token, remoteIP string) error {
	if token == "" {
		return ErrTurnstile
	}
	form := url.Values{}
	form.Set("secret", t.secret)
	form.Set("response", token)
	if remoteIP != "" {
		form.Set("remoteip", remoteIP)
	}

	resp, err := t.client.PostForm("https://challenges.cloudflare.com/turnstile/v0/siteverify", form)
	if err != nil {
		return fmt.Errorf("%w：%v", ErrTurnstileDown, err)
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 500 {
		return fmt.Errorf("%w：HTTP %d", ErrTurnstileDown, resp.StatusCode)
	}

	var body turnstileResponse
	if err := json.NewDecoder(resp.Body).Decode(&body); err != nil {
		return fmt.Errorf("%w：%v", ErrTurnstileDown, err)
	}
	if !body.Success {
		return fmt.Errorf("%w：%s", ErrTurnstile, strings.Join(body.ErrorCodes, ","))
	}
	return nil
}

// HexNonce 是给客户端用的示例：把计数器变成 nonce。
// 放在服务端只是为了让两端对「nonce 长什么样」有唯一解释。
func HexNonce(counter uint64) string {
	var buf [8]byte
	binary.BigEndian.PutUint64(buf[:], counter)
	return hex.EncodeToString(buf[:])
}

// Solve 是客户端解法的参考实现（也给测试用）。
//
// 客户端可以更聪明（多线程、跳过区间），但算法必须是这一个：
// 从 0 开始递增计数器，直到 sha256(challenge + "|" + hex(nonce)) 满足难度。
// maxIter 防止测试或误用时死循环。
func Solve(challenge string, bits int, maxIter uint64) (string, bool) {
	for i := uint64(0); i < maxIter; i++ {
		nonce := HexNonce(i)
		if meetsDifficulty(challenge, nonce, bits) {
			return nonce, true
		}
	}
	return "", false
}
