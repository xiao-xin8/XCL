//go:build unix

package guard

import "syscall"

// fdLimit 返回进程的打开文件数上限。
func fdLimit() int {
	var rl syscall.Rlimit
	if err := syscall.Getrlimit(syscall.RLIMIT_NOFILE, &rl); err != nil {
		return 0
	}
	// 有些系统的硬上限是 ^uint64(0)，取软上限更贴近实际
	return int(rl.Cur)
}
