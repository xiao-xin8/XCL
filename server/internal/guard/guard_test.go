package guard

import (
	"testing"
	"time"
)

var t0 = time.Unix(1_800_000_000, 0)

func feed(m *Monitor, now time.Time, n int) {
	for i := 0; i < n; i++ {
		m.OnAccept(now)
	}
}

func TestNoTripOnNormalLoad(t *testing.T) {
	m := New(Config{MaxAcceptsPerSec: 100, Sustain: 2 * time.Second})
	// 每秒 50 条，远低于阈值
	for i := 0; i < 20; i++ {
		now := t0.Add(time.Duration(i) * time.Second)
		feed(m, now, 50)
		if trip, why := m.Evaluate(now); trip != TripNone {
			t.Fatalf("正常流量不该触发：%v %s", trip, why)
		}
	}
}

func TestBurstDoesNotTripImmediately(t *testing.T) {
	m := New(Config{MaxAcceptsPerSec: 10, Sustain: 5 * time.Second})
	// 一秒钟冲 1000 条，但只持续 1 秒
	feed(m, t0, 1000)
	if trip, _ := m.Evaluate(t0); trip != TripNone {
		t.Fatalf("瞬时毛刺不该立刻熔断，得到 %v", trip)
	}
}

func TestSustainedOverloadTripsBridge(t *testing.T) {
	m := New(Config{MaxAcceptsPerSec: 10, Sustain: 5 * time.Second})
	var trip Trip
	var why string
	for i := 0; i < 20; i++ {
		now := t0.Add(time.Duration(i) * time.Second)
		feed(m, now, 1000)
		trip, why = m.Evaluate(now)
		if trip == TripBridge {
			if i < 5 {
				t.Fatalf("才 %d 秒就熔断，Sustain 没生效", i)
			}
			break
		}
	}
	if trip != TripBridge {
		t.Fatalf("持续超载应当熔断，得到 %v %s", trip, why)
	}
}

func TestRecoverAfterLoadDrops(t *testing.T) {
	m := New(Config{MaxAcceptsPerSec: 10, Sustain: 2 * time.Second})
	for i := 0; i < 10; i++ {
		now := t0.Add(time.Duration(i) * time.Second)
		feed(m, now, 1000)
	}
	// 洪水停了，空转 20 秒让滑动窗口清空
	quiet := t0.Add(20 * time.Second)
	for i := 0; i < 20; i++ {
		m.Evaluate(quiet.Add(time.Duration(i) * time.Second))
	}
	if trip, _ := m.Evaluate(quiet.Add(21 * time.Second)); trip != TripNone {
		t.Fatalf("流量下来后应当恢复，得到 %v", trip)
	}
}

func TestMemoryLimitTripsExit(t *testing.T) {
	// 阈值设成 1 字节，一定超
	m := New(Config{MemLimit: 1, MaxAcceptsPerSec: 1000, FDRatio: 0.99})
	if trip, _ := m.Evaluate(t0); trip != TripExit {
		t.Fatalf("内存超限应当到 TripExit，得到 %v", trip)
	}
}
