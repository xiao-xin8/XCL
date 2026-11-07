package auth

import (
	"crypto/rand"
	"crypto/subtle"
	"encoding/base64"
	"errors"
	"fmt"
	"strings"

	"golang.org/x/crypto/argon2"
)

// 密码哈希参数。
//
// argon2id + 64MB / 3 轮 / 2 并行：单次约 50~80ms，登录接口感觉不到，
// 但暴力破解的成本被抬得很高。机器只有 2GB 内存，所以内存参数不能更大 ——
// 64MB 是「单次开销可接受、并发几个也不会 OOM」的折中。
const (
	argonTime    uint32 = 3
	argonMemory  uint32 = 64 * 1024 // KiB
	argonThreads uint8  = 2
	argonKeyLen  uint32 = 32
	argonSaltLen        = 16
)

var (
	// ErrBadCredentials 故意不区分「用户名不存在」和「密码错」，
	// 避免把「这个用户名有没有注册过」白送给撞库的人。
	ErrBadCredentials = errors.New("用户名或密码不对")
	ErrBadHashFormat  = errors.New("密码哈希格式不对")
)

// HashPassword 生成 PHC 格式的哈希串：
//
//	$argon2id$v=19$m=65536,t=3,p=2$<salt>$<hash>
//
// 参数全部写进串里，将来调大参数时老密码仍然能验证，下次登录再顺带升级。
func HashPassword(password string) (string, error) {
	salt := make([]byte, argonSaltLen)
	if _, err := rand.Read(salt); err != nil {
		return "", fmt.Errorf("生成盐：%w", err)
	}
	key := argon2.IDKey([]byte(password), salt, argonTime, argonMemory, argonThreads, argonKeyLen)
	return fmt.Sprintf("$argon2id$v=%d$m=%d,t=%d,p=%d$%s$%s",
		argon2.Version, argonMemory, argonTime, argonThreads,
		base64.RawStdEncoding.EncodeToString(salt),
		base64.RawStdEncoding.EncodeToString(key),
	), nil
}

// VerifyPassword 校验密码。哈希串损坏或算法不认识时返回 ErrBadHashFormat，
// 调用方应当把它当服务端故障处理（记日志），而不是当「密码错」。
func VerifyPassword(encoded, password string) error {
	parts := strings.Split(encoded, "$")
	// ["", "argon2id", "v=19", "m=...,t=...,p=...", salt, hash]
	if len(parts) != 6 || parts[1] != "argon2id" {
		return ErrBadHashFormat
	}

	var version int
	if _, err := fmt.Sscanf(parts[2], "v=%d", &version); err != nil || version != argon2.Version {
		return ErrBadHashFormat
	}

	var memory, iterations uint32
	var threads uint8
	if _, err := fmt.Sscanf(parts[3], "m=%d,t=%d,p=%d", &memory, &iterations, &threads); err != nil {
		return ErrBadHashFormat
	}

	salt, err := base64.RawStdEncoding.DecodeString(parts[4])
	if err != nil {
		return ErrBadHashFormat
	}
	want, err := base64.RawStdEncoding.DecodeString(parts[5])
	if err != nil {
		return ErrBadHashFormat
	}

	got := argon2.IDKey([]byte(password), salt, iterations, memory, threads, uint32(len(want)))
	// 常量时间比较：普通的 bytes.Equal 会在第一个不同的字节就返回，
	// 理论上能按耗时一位一位试出哈希。
	if subtle.ConstantTimeCompare(got, want) != 1 {
		return ErrBadCredentials
	}
	return nil
}

// ---------- 输入规则 ----------

const (
	UsernameMinLen = 3
	UsernameMaxLen = 20
	PasswordMinLen = 8
	// PasswordMaxLen 不是安全需要，是 argon2 的开销：超长输入会让单次哈希变慢，
	// 有人拿 1MB 密码打登录接口就能把 CPU 拖死。
	PasswordMaxLen = 128
)

var (
	ErrUsernameShape = fmt.Errorf("用户名要 %d~%d 个字符，只能用字母、数字、下划线、短横线", UsernameMinLen, UsernameMaxLen)
	ErrPasswordShape = fmt.Errorf("密码要 %d~%d 个字符", PasswordMinLen, PasswordMaxLen)
)

// ValidateUsername 检查用户名。
//
// 只允许 ASCII 的字母数字和 _ -：中文用户名在日志、命令行参数、邀请好友时
// 都会引入编码问题，而且极易和别的名字「看起来一样」（同形字），
// 封号时容易封错人。
func ValidateUsername(name string) error {
	if len(name) < UsernameMinLen || len(name) > UsernameMaxLen {
		return ErrUsernameShape
	}
	for _, r := range name {
		switch {
		case r >= 'a' && r <= 'z',
			r >= 'A' && r <= 'Z',
			r >= '0' && r <= '9',
			r == '_', r == '-':
		default:
			return ErrUsernameShape
		}
	}
	return nil
}

// ValidatePassword 只检查长度。
//
// 不强制「大小写+数字+符号」：那类规则的实际效果是人人都用 P@ssw0rd1，
// 反而更好猜。长度是唯一真正有效的约束。
func ValidatePassword(password string) error {
	if len(password) < PasswordMinLen || len(password) > PasswordMaxLen {
		return ErrPasswordShape
	}
	return nil
}
