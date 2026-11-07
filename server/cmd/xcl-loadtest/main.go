// Command xcl-loadtest 对着桥接端口敲门，看门卫是不是按预期在挡人。
//
// 为什么单独写这个：限流阈值这种东西没法在线上试 —— 一试就是全站玩家陪你。
// 这个工具只打本机（默认 127.0.0.1:11010），验证三件事：
//
//  1. 正常量级能通过（别把真人挡在门外）
//  2. 超并发 / 超速率被静默丢弃（零字节，不是 RST、不是错误页）
//  3. 坏令牌刷够次数后整个 IP 被拉黑（之后连合法令牌也进不来）
//
// 用法：
//
//	go run ./cmd/xcl-loadtest -addr 127.0.0.1:11010 -n 300 -conc 20
//
// 想验证「合法令牌能过」，把 -token 换成真令牌；默认塞的是垃圾值，
// 正好用来测坏令牌拉黑和 IP 速率闸。
package main

import (
	"encoding/binary"
	"flag"
	"fmt"
	"net"
	"os"
	"sync"
	"sync/atomic"
	"time"
)

var magic = [4]byte{'X', 'C', 'L', '1'}

const (
	resultOK      = "ok"
	resultDropped = "dropped"
	resultFailed  = "failed"
	resultWeird   = "weird"
)

func main() {
	addr := flag.String("addr", "127.0.0.1:11010", "桥接地址")
	n := flag.Int("n", 300, "总共发起多少次连接")
	conc := flag.Int("conc", 20, "并发度")
	token := flag.String("token", "not-a-real-token", "首帧里塞的令牌；默认是垃圾值")
	timeout := flag.Duration("timeout", 3*time.Second, "单次连接超时")
	flag.Parse()

	var ok, dropped, failed, weird int64

	sem := make(chan struct{}, *conc)
	var wg sync.WaitGroup
	start := time.Now()

	for i := 0; i < *n; i++ {
		wg.Add(1)
		sem <- struct{}{}
		go func() {
			defer wg.Done()
			defer func() { <-sem }()

			switch probe(*addr, *token, *timeout) {
			case resultOK:
				atomic.AddInt64(&ok, 1)
			case resultDropped:
				atomic.AddInt64(&dropped, 1)
			case resultWeird:
				atomic.AddInt64(&weird, 1)
			default:
				atomic.AddInt64(&failed, 1)
			}
		}()
	}
	wg.Wait()

	elapsed := time.Since(start)
	rate := float64(*n) / elapsed.Seconds()

	fmt.Printf("目标 %s，共 %d 次连接，并发 %d，耗时 %s（%.0f 次/秒）\n",
		*addr, *n, *conc, elapsed.Round(time.Millisecond), rate)
	fmt.Printf("  通过（收到 0x00）：%d\n", ok)
	fmt.Printf("  静默丢弃（无任何响应）：%d\n", dropped)
	fmt.Printf("  连不上（拒绝/超时）：%d\n", failed)
	if weird > 0 {
		fmt.Printf("  异常响应（既非 0x00 也非空）：%d  ← 不该出现\n", weird)
	}

	if weird > 0 {
		os.Exit(1)
	}
}

// probe 走一次完整的「连上 → 发首帧 → 看回什么」。
func probe(addr, token string, timeout time.Duration) string {
	conn, err := net.DialTimeout("tcp", addr, timeout)
	if err != nil {
		return resultFailed
	}
	defer conn.Close()
	if err := conn.SetDeadline(time.Now().Add(timeout)); err != nil {
		return resultFailed
	}

	var header [6]byte
	copy(header[:4], magic[:])
	binary.BigEndian.PutUint16(header[4:], uint16(len(token)))
	if _, err := conn.Write(append(header[:], token...)); err != nil {
		return resultDropped
	}

	buf := make([]byte, 1)
	n, err := conn.Read(buf)
	if n == 1 {
		if buf[0] == 0x00 {
			return resultOK
		}
		return resultWeird
	}
	// 读不到东西（EOF 或超时）：门卫没说话，这就是预期行为
	_ = err
	return resultDropped
}
