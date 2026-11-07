//go:build !unix

package guard

// fdLimit 在非 Unix 平台上不参与判断（开发机是 Windows，服务端是 Linux）。
func fdLimit() int { return 0 }
