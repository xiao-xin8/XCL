package captcha

import (
	"crypto/rand"
	"errors"
	"testing"
	"time"
)

func newPow(t *testing.T) *Pow {
	t.Helper()
	seed := make([]byte, 32)
	if _, err := rand.Read(seed); err != nil {
		t.Fatalf("生成 seed：%v", err)
	}
	return NewPow(seed)
}

func TestPowHappyPath(t *testing.T) {
	p := newPow(t)
	now := time.Unix(1_800_000_000, 0)

	challenge := p.Issue(now)
	if challenge == "" {
		t.Fatal("应当能发题")
	}
	nonce, ok := Solve(challenge, Difficulty, 1<<24)
	if !ok {
		t.Fatal("参考实现应当能解出题")
	}
	if err := p.Verify(challenge, nonce, now); err != nil {
		t.Fatalf("合法解答应当通过：%v", err)
	}
	// 同一份挑战只能兑一次
	if err := p.Verify(challenge, nonce, now); !errors.Is(err, ErrChallengeUsed) {
		t.Fatalf("重复使用应当被拒，实际 %v", err)
	}
}

func TestPowRejectsUnsolved(t *testing.T) {
	p := newPow(t)
	now := time.Unix(1_800_000_000, 0)
	challenge := p.Issue(now)

	// 随便一个 nonce 几乎必然不满足 20 bit 前导零
	if err := p.Verify(challenge, "deadbeef", now); !errors.Is(err, ErrProofOfWork) {
		t.Fatalf("未解出的题应当被拒，实际 %v", err)
	}
}

func TestPowRejectsForgedChallenge(t *testing.T) {
	p := newPow(t)
	other := newPow(t)
	now := time.Unix(1_800_000_000, 0)

	// 换一把 seed 签出来的挑战，本机不认
	challenge := other.Issue(now)
	nonce, ok := Solve(challenge, Difficulty, 1<<24)
	if !ok {
		t.Fatal("应当能解出")
	}
	if err := p.Verify(challenge, nonce, now); !errors.Is(err, ErrChallengeFormat) {
		t.Fatalf("伪造的挑战应当被拒，实际 %v", err)
	}

	// 乱码同理
	if err := p.Verify("不是 base64", "0", now); !errors.Is(err, ErrChallengeFormat) {
		t.Fatalf("乱码应当被拒，实际 %v", err)
	}
}

func TestPowRejectsExpired(t *testing.T) {
	p := newPow(t)
	now := time.Unix(1_800_000_000, 0)
	challenge := p.Issue(now)

	later := now.Add(challengeTTL + time.Minute)
	if err := p.Verify(challenge, "00", later); !errors.Is(err, ErrChallengeExpired) {
		t.Fatalf("过期挑战应当被拒，实际 %v", err)
	}
}

func TestPowChallengesAreUnique(t *testing.T) {
	p := newPow(t)
	now := time.Unix(1_800_000_000, 0)
	seen := map[string]bool{}
	for i := 0; i < 100; i++ {
		c := p.Issue(now)
		if seen[c] {
			t.Fatal("挑战必须每次不同（随机数要真随机）")
		}
		seen[c] = true
	}
}

func TestSolveFailsWhenTooHard(t *testing.T) {
	// 56 bit 前导零在 1000 次内不可能解出来，验证 maxIter 会退出
	if _, ok := Solve("challenge", 56, 1000); ok {
		t.Fatal("不该在 1000 次内解出 56 bit")
	}
}

func TestTurnstileDisabledWhenNoSecret(t *testing.T) {
	if NewTurnstile("").Enabled() {
		t.Fatal("空 secret 表示没启用")
	}
	if !NewTurnstile("abc").Enabled() {
		t.Fatal("有 secret 就是启用")
	}
}
