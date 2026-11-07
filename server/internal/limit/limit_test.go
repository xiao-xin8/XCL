package limit

import (
	"fmt"
	"testing"
	"time"
)

var t0 = time.Unix(1_800_000_000, 0)

func TestAllowWithinLimit(t *testing.T) {
	l := New(time.Minute, 3)
	for i := 0; i < 3; i++ {
		if !l.Allow("a", t0) {
			t.Fatalf("第 %d 次不该被拒", i+1)
		}
	}
	if l.Allow("a", t0) {
		t.Fatal("第 4 次应当被拒")
	}
	if l.Rejected() != 1 {
		t.Fatalf("拒绝计数 = %d，期望 1", l.Rejected())
	}
}

func TestKeyIsolation(t *testing.T) {
	l := New(time.Minute, 2)
	l.Allow("a", t0)
	l.Allow("a", t0)
	if l.Allow("a", t0) {
		t.Fatal("a 应当已满")
	}
	if !l.Allow("b", t0) {
		t.Fatal("b 不该受 a 影响")
	}
}

func TestWindowRolls(t *testing.T) {
	l := New(time.Minute, 2)
	l.Allow("a", t0)
	l.Allow("a", t0)
	if l.Allow("a", t0) {
		t.Fatal("窗口内应当已满")
	}
	// 窗口滑过去之后应当重新可用
	if !l.Allow("a", t0.Add(61*time.Second)) {
		t.Fatal("过了窗口应当放行")
	}
}

func TestPartialSlideKeepsRecent(t *testing.T) {
	// 窗口 1 分钟、上限 2：第 0 秒用掉 2 次，第 30 秒仍应被拒
	l := New(60*time.Second, 2)
	l.Allow("a", t0)
	l.Allow("a", t0)
	if l.Allow("a", t0.Add(30*time.Second)) {
		t.Fatal("30 秒后仍在窗口内，应当被拒")
	}
	if got := l.Count("a", t0.Add(30*time.Second)); got != 2 {
		t.Fatalf("窗口内计数 = %d，期望 2", got)
	}
}

func TestSweepDropsIdleKeys(t *testing.T) {
	l := New(time.Minute, 5)
	for i := 0; i < 100; i++ {
		l.Allow(fmt.Sprintf("k%d", i), t0)
	}
	if l.Len() != 100 {
		t.Fatalf("key 数 = %d，期望 100", l.Len())
	}
	// 触发清扫（间隔 60 秒），此时所有 key 都已滑出窗口
	l.Allow("fresh", t0.Add(2*time.Minute))
	if got := l.Len(); got != 1 {
		t.Fatalf("清扫后 key 数 = %d，期望只剩 fresh 一个", got)
	}
}

func TestMaxKeysBounds(t *testing.T) {
	l := New(time.Minute, 5).WithMaxKeys(10)
	for i := 0; i < 50; i++ {
		l.Allow(fmt.Sprintf("k%d", i), t0)
	}
	if got := l.Len(); got > 10 {
		t.Fatalf("key 数 = %d，超过硬上限 10", got)
	}
}

func TestDisabledAllowsEverything(t *testing.T) {
	l := New(time.Second, 1)
	l.SetDisabled(true)
	for i := 0; i < 100; i++ {
		if !l.Allow("a", t0) {
			t.Fatal("关掉之后不该再拒绝")
		}
	}
}

func TestEmptyKeyIgnored(t *testing.T) {
	l := New(time.Second, 1)
	if !l.Allow("", t0) {
		t.Fatal("空 key 不该参与计数")
	}
	if l.Len() != 0 {
		t.Fatal("空 key 不该占表")
	}
}
