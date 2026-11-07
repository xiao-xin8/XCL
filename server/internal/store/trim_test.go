package store

import (
	"fmt"
	"testing"
	"time"
)

// 令牌发出去就收不回来，所以只能限制「同时有几个」。
// 这里验证两件事：只保留最新的 N 条，以及被动过的会话确实失效。
func TestTrimSessionsKeepsNewest(t *testing.T) {
	s := openTemp(t)
	now := time.Unix(1_800_000_000, 0)

	user, err := s.CreateUser("player", "hash", now)
	if err != nil {
		t.Fatalf("建账号：%v", err)
	}

	ids := make([]string, 0, 5)
	for i := 0; i < 5; i++ {
		id := fmt.Sprintf("sess-%d", i)
		ids = append(ids, id)
		created := now.Add(time.Duration(i) * time.Minute)
		if err := s.CreateSession(Session{
			ID:         id,
			UserID:     user.ID,
			CreatedAt:  created.Unix(),
			ExpiresAt:  created.Add(time.Hour).Unix(),
			LastSeenAt: created.Unix(),
		}, "hash"); err != nil {
			t.Fatalf("写会话 %d：%v", i, err)
		}
	}

	n, err := s.TrimSessions(user.ID, now, 3)
	if err != nil {
		t.Fatalf("清理：%v", err)
	}
	if n != 2 {
		t.Fatalf("应当作废 2 条，实际 %d", n)
	}

	// 最新的三条（sess-2/3/4）必须活着，最早的两条必须死
	for i, id := range ids {
		sess, err := s.SessionByID(id)
		if err != nil {
			t.Fatalf("读会话 %s：%v", id, err)
		}
		wantRevoked := i < 2
		if sess.Revoked != wantRevoked {
			t.Fatalf("%s revoked=%v，期望 %v", id, sess.Revoked, wantRevoked)
		}
	}

	// 有效会话数应当正好等于上限
	if got := s.SessionCount(now); got != 3 {
		t.Fatalf("有效会话数 = %d，期望 3", got)
	}
}

// 上限比现有会话多时不该动任何东西。
func TestTrimSessionsNoopWhenUnderLimit(t *testing.T) {
	s := openTemp(t)
	now := time.Unix(1_800_000_000, 0)

	user, _ := s.CreateUser("player", "hash", now)
	_ = s.CreateSession(Session{
		ID: "only", UserID: user.ID,
		CreatedAt: now.Unix(), ExpiresAt: now.Add(time.Hour).Unix(), LastSeenAt: now.Unix(),
	}, "hash")

	n, err := s.TrimSessions(user.ID, now, 3)
	if err != nil {
		t.Fatalf("清理：%v", err)
	}
	if n != 0 {
		t.Fatalf("没超上限不该作废任何会话，实际 %d", n)
	}
}

// 注册日上限的计数必须来自数据库：进程重启后仍然算数。
func TestCountUsersSince(t *testing.T) {
	s := openTemp(t)
	base := time.Unix(1_800_000_000, 0)

	for i := 0; i < 3; i++ {
		if _, err := s.CreateUser(fmt.Sprintf("u%d", i), "hash", base.Add(time.Duration(i)*time.Second)); err != nil {
			t.Fatalf("建账号 %d：%v", i, err)
		}
	}
	// 一个「昨天」的账号，不该被算进今天的配额
	if _, err := s.CreateUser("old", "hash", base.Add(-24*time.Hour)); err != nil {
		t.Fatalf("建旧账号：%v", err)
	}

	got, err := s.CountUsersSince(base.Unix())
	if err != nil {
		t.Fatalf("统计：%v", err)
	}
	if got != 3 {
		t.Fatalf("今日注册数 = %d，期望 3", got)
	}
}
