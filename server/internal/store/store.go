// Package store 是账号台账（SQLite）。
//
// 为什么是数据库而不是一个 JSON 文件：账号系统要做的是「多个人同时注册、
// 同时登录、随时封号」，这些都是事务性写入。单文件读写要靠自己加锁、
// 自己保证原子性、自己处理「写到一半断电」，等于重写一遍数据库。
// SQLite 是单文件、零运维、有事务，正好卡在两者中间。
//
// 驱动用 modernc.org/sqlite（纯 Go，无 CGO）：交叉编译到 Linux 只要
// `GOOS=linux GOARCH=amd64 go build`，不需要目标机器装 gcc。
package store

import (
	"database/sql"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	_ "modernc.org/sqlite"
)

// ErrUsernameTaken 表示用户名已被占用。
var ErrUsernameTaken = errors.New("用户名已被占用")

// ErrNotFound 表示记录不存在。
var ErrNotFound = errors.New("记录不存在")

// User 是账号。
type User struct {
	ID           int64
	Username     string
	Email        string
	CreatedAt    int64
	LastLoginAt  int64
	Banned       bool
	BannedReason string
}

// Session 是一条登录态。
//
// 令牌本身是签名自包含的，这张表存在的唯一意义是能作废：
// 退出登录、改密码、封号时把对应行标记掉，桥接下一笔连接就拒。
type Session struct {
	ID         string
	UserID     int64
	CreatedAt  int64
	ExpiresAt  int64
	LastSeenAt int64
	DeviceName string
	Revoked    bool
}

// Store 包住一个 SQLite 连接池。
type Store struct {
	db   *sql.DB
	path string
}

const schema = `
CREATE TABLE IF NOT EXISTS users (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  username      TEXT    NOT NULL COLLATE NOCASE,
  email         TEXT    NOT NULL DEFAULT '',
  password_hash TEXT    NOT NULL,
  created_at    INTEGER NOT NULL,
  last_login_at INTEGER NOT NULL DEFAULT 0,
  banned        INTEGER NOT NULL DEFAULT 0,
  banned_reason TEXT    NOT NULL DEFAULT ''
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_username ON users(username COLLATE NOCASE);
-- 注册日上限要按 created_at 数今天注册了多少个，没索引就是每次注册全表扫一遍
CREATE INDEX IF NOT EXISTS idx_users_created ON users(created_at);

CREATE TABLE IF NOT EXISTS sessions (
  id           TEXT    PRIMARY KEY,
  user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  token_hash   TEXT    NOT NULL,
  created_at   INTEGER NOT NULL,
  expires_at   INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL DEFAULT 0,
  device_name  TEXT    NOT NULL DEFAULT '',
  revoked      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_exp  ON sessions(expires_at);
`

// Open 打开（必要时创建）数据库。
func Open(path string) (*Store, error) {
	if dir := filepath.Dir(path); dir != "" && dir != "." {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return nil, fmt.Errorf("创建数据目录：%w", err)
		}
	}

	// WAL 让读不阻塞写；busy_timeout 兜住「两个请求同时写」的那一小段；
	// synchronous=NORMAL 在 WAL 下已经足够安全（断电最多丢最后几条事务）。
	dsn := "file:" + filepath.ToSlash(path) +
		"?_pragma=busy_timeout(5000)&_pragma=journal_mode(WAL)" +
		"&_pragma=foreign_keys(1)&_pragma=synchronous(NORMAL)"

	db, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, fmt.Errorf("打开数据库：%w", err)
	}
	// SQLite 的写是串行的，连接数开太多只会互相等锁
	db.SetMaxOpenConns(8)
	db.SetMaxIdleConns(4)
	db.SetConnMaxLifetime(time.Hour)

	if _, err := db.Exec(schema); err != nil {
		_ = db.Close()
		return nil, fmt.Errorf("建表：%w", err)
	}
	return &Store{db: db, path: path}, nil
}

// Close 关闭数据库。
func (s *Store) Close() error { return s.db.Close() }

// Path 返回数据库文件路径（启动日志里打出来）。
func (s *Store) Path() string { return s.path }

// ---------- 账号 ----------

// CreateUser 建账号。用户名重复返回 ErrUsernameTaken。
func (s *Store) CreateUser(username, passwordHash string, now time.Time) (User, error) {
	res, err := s.db.Exec(
		`INSERT INTO users (username, password_hash, created_at, last_login_at) VALUES (?, ?, ?, ?)`,
		username, passwordHash, now.Unix(), now.Unix(),
	)
	if err != nil {
		// 唯一索引冲突的判定不靠错误串匹配不行（驱动的错误码各家不同），
		// 这里退一步：冲突后回查一次，能查到就说明是重名。
		if _, _, lookupErr := s.UserByName(username); lookupErr == nil {
			return User{}, ErrUsernameTaken
		}
		return User{}, fmt.Errorf("写入账号：%w", err)
	}
	id, err := res.LastInsertId()
	if err != nil {
		return User{}, fmt.Errorf("读取新账号 id：%w", err)
	}
	return User{ID: id, Username: username, CreatedAt: now.Unix(), LastLoginAt: now.Unix()}, nil
}

// UserByName 按用户名取账号，同时返回密码哈希（列表接口不会带出来）。
func (s *Store) UserByName(username string) (User, string, error) {
	var (
		user User
		hash string
	)
	err := s.db.QueryRow(
		`SELECT id, username, email, password_hash, created_at, last_login_at, banned, banned_reason
		   FROM users WHERE username = ? COLLATE NOCASE`, username,
	).Scan(&user.ID, &user.Username, &user.Email, &hash, &user.CreatedAt,
		&user.LastLoginAt, &user.Banned, &user.BannedReason)
	if errors.Is(err, sql.ErrNoRows) {
		return User{}, "", ErrNotFound
	}
	if err != nil {
		return User{}, "", fmt.Errorf("读取账号：%w", err)
	}
	return user, hash, nil
}

// UserByID 按 id 取账号。
func (s *Store) UserByID(id int64) (User, error) {
	var user User
	err := s.db.QueryRow(
		`SELECT id, username, email, created_at, last_login_at, banned, banned_reason
		   FROM users WHERE id = ?`, id,
	).Scan(&user.ID, &user.Username, &user.Email, &user.CreatedAt,
		&user.LastLoginAt, &user.Banned, &user.BannedReason)
	if errors.Is(err, sql.ErrNoRows) {
		return User{}, ErrNotFound
	}
	if err != nil {
		return User{}, fmt.Errorf("读取账号：%w", err)
	}
	return user, nil
}

// TouchLogin 记录一次成功登录。
func (s *Store) TouchLogin(userID int64, now time.Time) error {
	_, err := s.db.Exec(`UPDATE users SET last_login_at = ? WHERE id = ?`, now.Unix(), userID)
	return err
}

// SetPassword 改密码。调用方应当同时吊销该账号的全部会话。
func (s *Store) SetPassword(userID int64, passwordHash string) error {
	_, err := s.db.Exec(`UPDATE users SET password_hash = ? WHERE id = ?`, passwordHash, userID)
	if err != nil {
		return fmt.Errorf("更新密码：%w", err)
	}
	return nil
}

// SetBanned 封号 / 解封。封号会顺手吊销所有会话，
// 这样「封了但还能玩到令牌过期」的漏洞就不存在。
func (s *Store) SetBanned(userID int64, banned bool, reason string) error {
	flag := 0
	if banned {
		flag = 1
	}
	if _, err := s.db.Exec(
		`UPDATE users SET banned = ?, banned_reason = ? WHERE id = ?`, flag, reason, userID,
	); err != nil {
		return fmt.Errorf("更新封禁状态：%w", err)
	}
	if banned {
		return s.RevokeUserSessions(userID)
	}
	return nil
}

// ListUsers 列出全部账号（新注册的在前）。
func (s *Store) ListUsers() ([]User, error) {
	rows, err := s.db.Query(
		`SELECT id, username, email, created_at, last_login_at, banned, banned_reason
		   FROM users ORDER BY id DESC`)
	if err != nil {
		return nil, fmt.Errorf("列出账号：%w", err)
	}
	defer rows.Close()

	out := make([]User, 0, 16)
	for rows.Next() {
		var user User
		if err := rows.Scan(&user.ID, &user.Username, &user.Email, &user.CreatedAt,
			&user.LastLoginAt, &user.Banned, &user.BannedReason); err != nil {
			return nil, fmt.Errorf("读取账号行：%w", err)
		}
		out = append(out, user)
	}
	return out, rows.Err()
}

// CountUsers 返回账号数（健康检查用）。
func (s *Store) CountUsers() int {
	var n int
	if err := s.db.QueryRow(`SELECT COUNT(*) FROM users`).Scan(&n); err != nil {
		return 0
	}
	return n
}

// CountUsersSince 返回某个时间点之后注册的账号数。
//
// 注册日上限必须落库而不是放在内存里：进程一重启计数归零，
// 「一天最多 N 个」就成了「每次重启后又能再来 N 个」，等于没设。
func (s *Store) CountUsersSince(since int64) (int, error) {
	var n int
	err := s.db.QueryRow(`SELECT COUNT(*) FROM users WHERE created_at >= ?`, since).Scan(&n)
	if err != nil {
		return 0, fmt.Errorf("统计注册数：%w", err)
	}
	return n, nil
}

// ---------- 会话 ----------

// CreateSession 落一条登录态。tokenHash 是令牌的 sha256（库里不存令牌原文）。
func (s *Store) CreateSession(sess Session, tokenHash string) error {
	_, err := s.db.Exec(
		`INSERT INTO sessions (id, user_id, token_hash, created_at, expires_at, last_seen_at, device_name)
		 VALUES (?, ?, ?, ?, ?, ?, ?)`,
		sess.ID, sess.UserID, tokenHash, sess.CreatedAt, sess.ExpiresAt, sess.LastSeenAt, sess.DeviceName,
	)
	if err != nil {
		return fmt.Errorf("写入会话：%w", err)
	}
	return nil
}

// SessionByID 取登录态。
func (s *Store) SessionByID(id string) (Session, error) {
	var sess Session
	err := s.db.QueryRow(
		`SELECT id, user_id, created_at, expires_at, last_seen_at, device_name, revoked
		   FROM sessions WHERE id = ?`, id,
	).Scan(&sess.ID, &sess.UserID, &sess.CreatedAt, &sess.ExpiresAt,
		&sess.LastSeenAt, &sess.DeviceName, &sess.Revoked)
	if errors.Is(err, sql.ErrNoRows) {
		return Session{}, ErrNotFound
	}
	if err != nil {
		return Session{}, fmt.Errorf("读取会话：%w", err)
	}
	return sess, nil
}

// SessionState 是一次「能不能放行」的完整答复。
//
// 一次 join 拿全：会话是否作废、是否过期、账号是否被封。
// 分三次查会在中间留出「刚封号但会话还有效」的窗口。
type SessionState struct {
	UserID   int64
	Username string
	Expired  bool
	Revoked  bool
	Banned   bool
}

// CheckSession 判断一条登录态当下是否可用。
func (s *Store) CheckSession(id string, now time.Time) (SessionState, error) {
	var (
		state     SessionState
		expiresAt int64
		revoked   int
		banned    int
	)
	err := s.db.QueryRow(
		`SELECT s.user_id, u.username, s.expires_at, s.revoked, u.banned
		   FROM sessions s JOIN users u ON u.id = s.user_id
		  WHERE s.id = ?`, id,
	).Scan(&state.UserID, &state.Username, &expiresAt, &revoked, &banned)
	if errors.Is(err, sql.ErrNoRows) {
		return SessionState{}, ErrNotFound
	}
	if err != nil {
		return SessionState{}, fmt.Errorf("检查会话：%w", err)
	}
	state.Expired = expiresAt <= now.Unix()
	state.Revoked = revoked != 0
	state.Banned = banned != 0
	return state, nil
}

// TouchSession 更新会话的最近活跃时间。
func (s *Store) TouchSession(id string, now time.Time) {
	_, _ = s.db.Exec(`UPDATE sessions SET last_seen_at = ? WHERE id = ?`, now.Unix(), id)
}

// ExtendSession 把会话的有效期往后推（续期时用）。
func (s *Store) ExtendSession(id string, expiresAt int64, now time.Time) error {
	res, err := s.db.Exec(
		`UPDATE sessions SET expires_at = ?, last_seen_at = ? WHERE id = ? AND revoked = 0`,
		expiresAt, now.Unix(), id,
	)
	if err != nil {
		return fmt.Errorf("续期会话：%w", err)
	}
	// 没改到行说明这期间被作废了 —— 让调用方当「会话已失效」处理
	if n, _ := res.RowsAffected(); n == 0 {
		return ErrNotFound
	}
	return nil
}

// RevokeSession 作废一条登录态（退出登录）。
func (s *Store) RevokeSession(id string) error {
	_, err := s.db.Exec(`UPDATE sessions SET revoked = 1 WHERE id = ?`, id)
	if err != nil {
		return fmt.Errorf("作废会话：%w", err)
	}
	return nil
}

// RevokeUserSessions 作废某个账号的全部登录态（改密码、封号时用）。
func (s *Store) RevokeUserSessions(userID int64) error {
	_, err := s.db.Exec(`UPDATE sessions SET revoked = 1 WHERE user_id = ?`, userID)
	if err != nil {
		return fmt.Errorf("作废账号会话：%w", err)
	}
	return nil
}

// PurgeExpired 清掉过期超过一天的会话。
//
// 保留一天是为了让「令牌刚过期」的看到的是「请重新登录」，
// 而不是「这个会话压根不存在」——两者对排障意义不同。
func (s *Store) PurgeExpired(now time.Time) (int64, error) {
	res, err := s.db.Exec(`DELETE FROM sessions WHERE expires_at < ?`, now.Add(-24*time.Hour).Unix())
	if err != nil {
		return 0, err
	}
	return res.RowsAffected()
}

// SessionCount 返回有效会话数（健康检查 / 运维看）。
func (s *Store) SessionCount(now time.Time) int {
	var n int
	_ = s.db.QueryRow(
		`SELECT COUNT(*) FROM sessions WHERE revoked = 0 AND expires_at > ?`, now.Unix(),
	).Scan(&n)
	return n
}

// TrimSessions 只保留该账号最近 keep 条有效登录态，其余作废。
//
// 为什么要管：令牌是 bearer 凭据，发出去就收不回来。一个账号登录一百次就有
// 一百个能用的令牌，转手发给一百个人，服务端完全分不出来 —— 桥接那边只看到
// 「同一个账号的合法令牌」。把活跃登录数压到一个手数得过来，至少让共享账号有成本。
//
// 返回被作废的条数。
func (s *Store) TrimSessions(userID int64, now time.Time, keep int) (int64, error) {
	if keep < 1 {
		keep = 1
	}
	res, err := s.db.Exec(
		`UPDATE sessions SET revoked = 1
		  WHERE user_id = ? AND revoked = 0 AND expires_at > ?
		    AND id NOT IN (
		      SELECT id FROM sessions
		       WHERE user_id = ? AND revoked = 0 AND expires_at > ?
		       ORDER BY created_at DESC, rowid DESC
		       LIMIT ?
		    )`,
		userID, now.Unix(), userID, now.Unix(), keep,
	)
	if err != nil {
		return 0, fmt.Errorf("清理旧登录态：%w", err)
	}
	n, _ := res.RowsAffected()
	return n, nil
}

// NormalizeUsername 做一次轻量归一：去掉首尾空白。
//
// 不做大小写折叠：库里用 COLLATE NOCASE 保证「Alice 和 alice 不能同时注册」，
// 但展示时保留自己写的大小写。
func NormalizeUsername(name string) string {
	return strings.TrimSpace(name)
}
