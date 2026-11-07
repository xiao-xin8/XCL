package store

import (
	"errors"
	"path/filepath"
	"testing"
	"time"
)

func openTemp(t *testing.T) *Store {
	t.Helper()
	s, err := Open(filepath.Join(t.TempDir(), "xcl.db"))
	if err != nil {
		t.Fatalf("打开数据库：%v", err)
	}
	t.Cleanup(func() { _ = s.Close() })
	return s
}

func TestCreateUserAndLookup(t *testing.T) {
	s := openTemp(t)
	now := time.Unix(1_800_000_000, 0)

	user, err := s.CreateUser("xiaoxin", "hash-1", now)
	if err != nil {
		t.Fatalf("建账号：%v", err)
	}
	if user.ID == 0 {
		t.Fatal("应当拿到自增 id")
	}

	got, hash, err := s.UserByName("xiaoxin")
	if err != nil {
		t.Fatalf("查账号：%v", err)
	}
	if got.ID != user.ID || hash != "hash-1" {
		t.Fatalf("查回来的不一致：%+v / %s", got, hash)
	}
	if got.Banned {
		t.Fatal("新账号不该是封禁状态")
	}
}

func TestUsernameIsCaseInsensitive(t *testing.T) {
	s := openTemp(t)
	now := time.Unix(1_800_000_000, 0)

	if _, err := s.CreateUser("XiaoXin", "hash", now); err != nil {
		t.Fatalf("建账号：%v", err)
	}
	// 大小写不同也算重名，否则「XiaoXin / xiaoxin」能互相冒充
	if _, err := s.CreateUser("xiaoxin", "hash", now); !errors.Is(err, ErrUsernameTaken) {
		t.Fatalf("重名应当报已占用，实际 %v", err)
	}

	// 但登录时大小写不敏感，按自己喜欢的方式输
	if _, _, err := s.UserByName("XIAOXIN"); err != nil {
		t.Fatalf("大小写应当能查到：%v", err)
	}
}

func TestSessionLifecycle(t *testing.T) {
	s := openTemp(t)
	now := time.Unix(1_800_000_000, 0)

	user, err := s.CreateUser("player1", "hash", now)
	if err != nil {
		t.Fatalf("建账号：%v", err)
	}
	sess := Session{
		ID: "sess-1", UserID: user.ID,
		CreatedAt: now.Unix(), ExpiresAt: now.Add(time.Hour).Unix(), LastSeenAt: now.Unix(),
	}
	if err := s.CreateSession(sess, "token-hash"); err != nil {
		t.Fatalf("写会话：%v", err)
	}

	state, err := s.CheckSession("sess-1", now)
	if err != nil {
		t.Fatalf("检查会话：%v", err)
	}
	if state.Revoked || state.Expired || state.Banned {
		t.Fatalf("新会话应当可用：%+v", state)
	}
	if state.Username != "player1" {
		t.Fatalf("会话应当带出用户名：%q", state.Username)
	}

	// 过期
	if err := s.CreateSession(Session{
		ID: "sess-old", UserID: user.ID,
		CreatedAt: now.Add(-2 * time.Hour).Unix(),
		ExpiresAt: now.Add(-time.Hour).Unix(),
		LastSeenAt: now.Add(-2 * time.Hour).Unix(),
	}, "h"); err != nil {
		t.Fatalf("写过期会话：%v", err)
	}
	if state, _ := s.CheckSession("sess-old", now); !state.Expired {
		t.Fatal("过期会话应当被标出来")
	}

	// 退出登录
	if err := s.RevokeSession("sess-1"); err != nil {
		t.Fatalf("作废会话：%v", err)
	}
	if state, _ := s.CheckSession("sess-1", now); !state.Revoked {
		t.Fatal("作废后应当是 revoked")
	}

	if _, err := s.CheckSession("不存在的会话", now); !errors.Is(err, ErrNotFound) {
		t.Fatalf("不存在的会话应当报 ErrNotFound，实际 %v", err)
	}
}

func TestBanKillsSessions(t *testing.T) {
	s := openTemp(t)
	now := time.Unix(1_800_000_000, 0)

	user, _ := s.CreateUser("cheater", "hash", now)
	if err := s.CreateSession(Session{
		ID: "s1", UserID: user.ID, CreatedAt: now.Unix(),
		ExpiresAt: now.Add(time.Hour).Unix(), LastSeenAt: now.Unix(),
	}, "h"); err != nil {
		t.Fatalf("写会话：%v", err)
	}

	if err := s.SetBanned(user.ID, true, "开挂"); err != nil {
		t.Fatalf("封号：%v", err)
	}

	// 封号必须同时把已有登录态干掉，否则「封了还能玩到令牌过期」
	state, err := s.CheckSession("s1", now)
	if err != nil {
		t.Fatalf("检查会话：%v", err)
	}
	if !state.Banned || !state.Revoked {
		t.Fatalf("封号应当同时作废会话：%+v", state)
	}

	after, _, _ := s.UserByName("cheater")
	if !after.Banned || after.BannedReason != "开挂" {
		t.Fatalf("封禁信息没落库：%+v", after)
	}

	// 解封后旧会话仍然是作废的（要重新登录），新会话可用
	if err := s.SetBanned(user.ID, false, ""); err != nil {
		t.Fatalf("解封：%v", err)
	}
	if state, _ := s.CheckSession("s1", now); !state.Revoked {
		t.Fatal("解封不该复活旧会话")
	}
}

func TestRevokeAllSessionsOfUser(t *testing.T) {
	s := openTemp(t)
	now := time.Unix(1_800_000_000, 0)

	user, _ := s.CreateUser("multi", "hash", now)
	for _, id := range []string{"a", "b", "c"} {
		if err := s.CreateSession(Session{
			ID: id, UserID: user.ID, CreatedAt: now.Unix(),
			ExpiresAt: now.Add(time.Hour).Unix(), LastSeenAt: now.Unix(),
		}, "h"); err != nil {
			t.Fatalf("写会话 %s：%v", id, err)
		}
	}
	if got := s.SessionCount(now); got != 3 {
		t.Fatalf("应当有 3 条有效会话，实际 %d", got)
	}

	if err := s.RevokeUserSessions(user.ID); err != nil {
		t.Fatalf("全部作废：%v", err)
	}
	if got := s.SessionCount(now); got != 0 {
		t.Fatalf("应当没有有效会话，实际 %d", got)
	}
}

func TestPurgeExpiredKeepsRecent(t *testing.T) {
	s := openTemp(t)
	now := time.Unix(1_800_000_000, 0)

	user, _ := s.CreateUser("p", "hash", now)
	// 过期两天：该清
	if err := s.CreateSession(Session{
		ID: "old", UserID: user.ID, CreatedAt: now.Add(-72 * time.Hour).Unix(),
		ExpiresAt: now.Add(-48 * time.Hour).Unix(), LastSeenAt: now.Unix(),
	}, "h"); err != nil {
		t.Fatalf("写会话：%v", err)
	}
	// 刚过期一小时：留着，看到「请重新登录」而不是「会话不存在」
	if err := s.CreateSession(Session{
		ID: "fresh", UserID: user.ID, CreatedAt: now.Add(-2 * time.Hour).Unix(),
		ExpiresAt: now.Add(-time.Hour).Unix(), LastSeenAt: now.Unix(),
	}, "h"); err != nil {
		t.Fatalf("写会话：%v", err)
	}

	n, err := s.PurgeExpired(now)
	if err != nil {
		t.Fatalf("清理：%v", err)
	}
	if n != 1 {
		t.Fatalf("应当只清 1 条，实际 %d", n)
	}
	if _, err := s.SessionByID("fresh"); err != nil {
		t.Fatalf("刚过期的会话不该被清：%v", err)
	}
}

func TestDataSurvivesReopen(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "xcl.db")
	now := time.Unix(1_800_000_000, 0)

	s, err := Open(path)
	if err != nil {
		t.Fatalf("打开：%v", err)
	}
	if _, err := s.CreateUser("persisted", "hash", now); err != nil {
		t.Fatalf("建账号：%v", err)
	}
	if err := s.Close(); err != nil {
		t.Fatalf("关闭：%v", err)
	}

	// 重启服务端之后账号必须还在 —— 这条就是「为什么不用内存 map」
	again, err := Open(path)
	if err != nil {
		t.Fatalf("重新打开：%v", err)
	}
	defer again.Close()
	if _, _, err := again.UserByName("persisted"); err != nil {
		t.Fatalf("重启后账号丢了：%v", err)
	}
}
