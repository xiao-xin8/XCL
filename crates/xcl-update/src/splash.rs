//! helper 的提示窗。
//!
//! 为什么不用 Tauri：helper 走 apply 分支，不初始化 Tauri、不建 WebView2。
//! 为了一个 420×190 的提示框把整个前端栈拉起来，既慢又容易和主程序抢资源。
//!
//! 所以这里是纯 Win32 自绘：无边框、圆角、配色跟主界面同一套
//! （米白底 + 金色进度条），而不是系统那种灰扑扑的标题栏窗口。
//!
//! 两件事容易忽略：
//!
//! 1. 必须自己声明 DPI 感知。helper 是独立进程，不声明的话 Windows 会把
//!    整个窗口当位图拉伸，125% / 150% 缩放下糊成一团。
//! 2. 尺寸和字号都要按 DPI 缩放，否则高 DPI 下字小得像蚂蚁。
//!
//! 尽力而为：建不出来就返回 `None`，更新照常进行 —— 不能因为画不出提示
//! 就把更新卡住。

use std::sync::atomic::{AtomicIsize, AtomicUsize};
use std::time::Duration;

/// 主窗口句柄，`0` 表示还没建好。
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
/// 动画计数（跑马灯的相位）。
static TICKS: AtomicUsize = AtomicUsize::new(0);
/// 两个字体句柄，WM_CREATE 建、WM_DESTROY 删。
static TITLE_FONT: AtomicIsize = AtomicIsize::new(0);
static BODY_FONT: AtomicIsize = AtomicIsize::new(0);
/// DPI 缩放系数 × 1000（整数存，避免浮点原子）。创建窗口时算一次。
static DPI_SCALE: AtomicUsize = AtomicUsize::new(1000);

/// 窗口上唯一的一句话。不随阶段变化 —— 换文案除了让界面闪，
/// 对看的人没有任何帮助，只需要知道「在更新、别关」。
const STATUS_TEXT: &str = "正在更新";

/// 提示窗固定显示这么久，到点自己收。
///
/// 这个窗口的存在意义是「看到有个更新程序在干活」，避免一闪而过、
/// 或者干脆没界面 —— 那种才像被病毒顶了。它不是进度指示器（文案是固定的
/// 「正在更新」，本来也不反映进度），所以没必要陪着整个更新过程挂着：
/// 体感上，「更新完窗口还赖着几秒」比「窗口一闪」更像卡死。
const MIN_VISIBLE: Duration = Duration::from_secs(1);

/// 一个已经显示出来的提示窗。
pub struct Splash;

impl Splash {
    /// 创建并显示窗口。失败（或 2 秒内没建出来）返回 `None`。
    pub fn show() -> Option<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        std::thread::spawn(move || {
            let ok = imp::run_window();
            let _ = tx.send(ok);
        });

        match rx.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(true) => {
                // 到点自动收，不等更新流程 —— 更新照常在后台跑完，
                // 后面的 `close()` 也就变成一个幂等的空操作（句柄已被清零）。
                std::thread::spawn(|| {
                    std::thread::sleep(MIN_VISIBLE);
                    imp::close_window();
                });
                Some(Self)
            }
            // 建不出来或者超时：不让它拖住更新
            _ => None,
        }
    }

    /// 关窗并让窗口线程退出。幂等 —— 到点自动关闭的那个定时器可能已经收过了。
    pub fn close(&self) {
        imp::close_window();
    }

    /// 系统弹窗。用在「窗口已经关掉、但新版崩了」这种事后补救。
    ///
    /// 这时候没有自己的窗口可用，只能用系统的 —— 丑归丑，但它一定弹得出来。
    pub fn alert(title: &str, message: &str) {
        imp::alert(title, message);
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::sync::Once;
    use std::sync::atomic::Ordering;

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        BeginPaint, BitBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleBitmap,
        CreateCompatibleDC, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DEFAULT_CHARSET,
        DEFAULT_PITCH, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, FillRgn, FW_NORMAL,
        FW_SEMIBOLD, GetDC, GetDeviceCaps, HGDIOBJ, InvalidateRect, LOGPIXELSX, OUT_DEFAULT_PRECIS,
        PAINTSTRUCT, ReleaseDC, SRCCOPY, SelectObject, SetBkMode, SetTextColor, SetWindowRgn,
        TRANSPARENT, UpdateWindow,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
        GetMessageW, GetSystemMetrics, IsWindow, MB_ICONERROR, MB_OK, MB_TOPMOST, MSG, MessageBoxW,
        PostMessageW, PostQuitMessage, RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SW_SHOW, SetTimer,
        ShowWindow, TranslateMessage, WM_CLOSE, WM_CREATE, WM_DESTROY, WM_ERASEBKGND, WM_PAINT,
        WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };

    use super::{
        BODY_FONT, DPI_SCALE, MAIN_HWND, STATUS_TEXT, TICKS, TITLE_FONT,
    };

    const CLASS_NAME: &str = "XinCraftLauncherUpdate";
    const WINDOW_TITLE: &str = "Xin Craft Launcher";
    // 下面这些是 96 DPI 下的设计尺寸，实际按屏幕 DPI 等比放大
    const WIDTH: i32 = 420;
    const HEIGHT: i32 = 190;
    const RADIUS: i32 = 18;
    const BAR_W: i32 = 260;
    const BAR_H: i32 = 4;
    const THUMB_W: i32 = 88;

    const TIMER_ID: usize = 1;
    /// 25 fps：跑马灯够顺，又不至于把 CPU 吃掉
    const FRAME_MS: u32 = 40;

    // 配色跟主界面一套：米白底、深棕字、金色进度
    const BG: u32 = rgb(0xFB, 0xF8, 0xF1);
    const TITLE_INK: u32 = rgb(0x3A, 0x30, 0x09);
    const SUBTLE_INK: u32 = rgb(0x8A, 0x7A, 0x4A);
    const TRACK: u32 = rgb(0xEF, 0xE7, 0xD2);
    const ACCENT: u32 = rgb(0xC9, 0xA2, 0x27);

    // DrawTextW 的标志位
    const DT_CENTER: u32 = 0x0000_0001;
    const DT_SINGLELINE: u32 = 0x0000_0020;
    const DT_VCENTER: u32 = 0x0000_0004;

    static REGISTER: Once = Once::new();

    const fn rgb(r: u32, g: u32, b: u32) -> u32 {
        (b << 16) | (g << 8) | r
    }

    /// 按 DPI 缩放。96 DPI 时原样返回。
    fn scaled(value: i32) -> i32 {
        let scale = DPI_SCALE.load(Ordering::Relaxed) as f64 / 1000.0;
        (value as f64 * scale).round() as i32
    }

    pub(super) fn run_window() -> bool {
        // SAFETY: 纯 Win32 调用，句柄都来自系统返回值。
        unsafe {
            // 必须在建窗口之前：声明了之后系统不再做位图拉伸，字才是清晰的
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            DPI_SCALE.store(system_dpi() as usize * 1000 / 96, Ordering::Relaxed);

            let hinstance = GetModuleHandleW(std::ptr::null());
            if hinstance.is_null() {
                return false;
            }

            let class = wide(CLASS_NAME);
            REGISTER.call_once(|| {
                let wc = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(wndproc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: hinstance,
                    hIcon: std::ptr::null_mut(),
                    hCursor: std::ptr::null_mut(),
                    hbrBackground: std::ptr::null_mut(),
                    lpszMenuName: std::ptr::null(),
                    lpszClassName: class.as_ptr(),
                };
                RegisterClassW(&wc);
            });

            let width = scaled(WIDTH);
            let height = scaled(HEIGHT);
            let x = (GetSystemMetrics(SM_CXSCREEN) - width) / 2;
            let y = (GetSystemMetrics(SM_CYSCREEN) - height) / 2;
            let title = wide(WINDOW_TITLE);

            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                x,
                y,
                width,
                height,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                hinstance,
                std::ptr::null_mut(),
            );
            if hwnd.is_null() {
                return false;
            }

            let radius = scaled(RADIUS);
            let rgn = CreateRoundRectRgn(0, 0, width + 1, height + 1, radius, radius);
            if !rgn.is_null() {
                SetWindowRgn(hwnd, rgn, 1);
            }
            MAIN_HWND.store(hwnd as isize, Ordering::Relaxed);

            ShowWindow(hwnd, SW_SHOW);
            UpdateWindow(hwnd);
            SetTimer(hwnd, TIMER_ID, FRAME_MS, None);

            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            true
        }
    }

    /// 屏幕 DPI。用 `GetDeviceCaps` 而不是 `GetDpiForSystem`：
    /// 后者要 Win10 1607+，前者从 Win2000 就有，少一个兼容性坑。
    unsafe fn system_dpi() -> u32 {
        unsafe {
            let hdc = GetDC(std::ptr::null_mut());
            if hdc.is_null() {
                return 96;
            }
            let dpi = GetDeviceCaps(hdc, LOGPIXELSX as i32);
            ReleaseDC(std::ptr::null_mut(), hdc);
            if dpi <= 0 {
                96
            } else {
                dpi as u32
            }
        }
    }

    pub(super) fn close_window() {
        let raw = MAIN_HWND.load(Ordering::Relaxed);
        if raw == 0 {
            return;
        }
        // SAFETY: 句柄由本进程创建，先用 IsWindow 确认还在
        unsafe {
            let hwnd = raw as HWND;
            if IsWindow(hwnd) != 0 {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
        }
    }

    pub(super) fn alert(title: &str, message: &str) {
        // SAFETY: 只是弹一个系统消息框，参数都是本进程的宽字符串
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                wide(message).as_ptr(),
                wide(title).as_ptr(),
                MB_OK | MB_ICONERROR | MB_TOPMOST,
            );
        }
    }

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_CREATE => {
                // SAFETY: 建字体，句柄留给 WM_PAINT 用
                unsafe {
                    TITLE_FONT.store(
                        make_font(-22, FW_SEMIBOLD as i32) as isize,
                        Ordering::Relaxed,
                    );
                    BODY_FONT.store(make_font(-14, FW_NORMAL as i32) as isize, Ordering::Relaxed);
                }
                0
            }
            WM_TIMER => {
                // SAFETY: 只推进相位并请求重绘
                unsafe {
                    TICKS.fetch_add(1, Ordering::Relaxed);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                0
            }
            // 自己画，别让系统先擦一遍（会闪）
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                // SAFETY: 标准双缓冲绘制，GDI 对象全部在函数内释放
                unsafe {
                    paint(hwnd);
                }
                0
            }
            WM_CLOSE => {
                // SAFETY: 标准销毁流程
                unsafe {
                    DestroyWindow(hwnd);
                }
                0
            }
            WM_DESTROY => {
                MAIN_HWND.store(0, Ordering::Relaxed);
                // SAFETY: 释放字体并结束消息循环
                unsafe {
                    for slot in [&TITLE_FONT, &BODY_FONT] {
                        let raw = slot.load(Ordering::Relaxed);
                        if raw != 0 {
                            DeleteObject(raw as HGDIOBJ);
                            slot.store(0, Ordering::Relaxed);
                        }
                    }
                    PostQuitMessage(0);
                }
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    /// 双缓冲：先画到内存位图再一次性贴上去，否则 25fps 重绘会明显闪。
    unsafe fn paint(hwnd: HWND) {
        unsafe {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            if hdc.is_null() {
                return;
            }
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rect);
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;

            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, width, height);
            let old = SelectObject(mem, bmp as HGDIOBJ);

            let brush = CreateSolidBrush(BG);
            FillRect(mem, &rect, brush);
            DeleteObject(brush as HGDIOBJ);

            SetBkMode(mem, TRANSPARENT as i32);

            // 标题
            let title = wide(WINDOW_TITLE);
            let mut title_rect = RECT {
                left: 0,
                top: scaled(46),
                right: width,
                bottom: scaled(84),
            };
            let title_font = TITLE_FONT.load(Ordering::Relaxed);
            let previous = if title_font != 0 {
                SelectObject(mem, title_font as HGDIOBJ)
            } else {
                std::ptr::null_mut()
            };
            SetTextColor(mem, TITLE_INK);
            DrawTextW(
                mem,
                title.as_ptr(),
                -1,
                &mut title_rect,
                DT_CENTER | DT_SINGLELINE | DT_VCENTER,
            );

            // 唯一那句状态文字，静态不动
            let status = wide(STATUS_TEXT);
            let mut status_rect = RECT {
                left: 0,
                top: scaled(88),
                right: width,
                bottom: scaled(112),
            };
            let body_font = BODY_FONT.load(Ordering::Relaxed);
            if body_font != 0 {
                SelectObject(mem, body_font as HGDIOBJ);
            }
            SetTextColor(mem, SUBTLE_INK);
            DrawTextW(
                mem,
                status.as_ptr(),
                -1,
                &mut status_rect,
                DT_CENTER | DT_SINGLELINE | DT_VCENTER,
            );

            // 进度条：轨道 + 来回跑的滑块
            let bar_w = scaled(BAR_W);
            let bar_h = scaled(BAR_H);
            let thumb_w = scaled(THUMB_W);
            let bar_x = (width - bar_w) / 2;
            let bar_y = scaled(132);

            let track = CreateSolidBrush(TRACK);
            let track_rgn = CreateRoundRectRgn(
                bar_x,
                bar_y,
                bar_x + bar_w + 1,
                bar_y + bar_h + 1,
                bar_h,
                bar_h,
            );
            FillRgn(mem, track_rgn, track);
            DeleteObject(track_rgn as HGDIOBJ);
            DeleteObject(track as HGDIOBJ);

            // 三角波相位：0 → 1 → 0，看起来是「来回跑」而不是「一圈一圈转」
            let span = bar_w + thumb_w;
            let speed = scaled(14).max(8);
            let phase = (TICKS.load(Ordering::Relaxed) as i32 * speed) % (span * 2);
            let offset = (if phase <= span {
                phase
            } else {
                span * 2 - phase
            }) - thumb_w;
            let thumb_x = bar_x + offset.max(0);
            let thumb_w = thumb_w.min(bar_x + bar_w - thumb_x).max(0);

            let accent = CreateSolidBrush(ACCENT);
            let thumb_rgn = CreateRoundRectRgn(
                thumb_x,
                bar_y,
                thumb_x + thumb_w + 1,
                bar_y + bar_h + 1,
                bar_h,
                bar_h,
            );
            FillRgn(mem, thumb_rgn, accent);
            DeleteObject(thumb_rgn as HGDIOBJ);
            DeleteObject(accent as HGDIOBJ);

            if !previous.is_null() {
                SelectObject(mem, previous);
            }

            BitBlt(hdc, 0, 0, width, height, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            DeleteObject(bmp as HGDIOBJ);
            DeleteDC(mem);
            EndPaint(hwnd, &ps);
        }
    }

    unsafe fn make_font(height: i32, weight: i32) -> windows_sys::Win32::Graphics::Gdi::HFONT {
        unsafe {
            let face = wide("Microsoft YaHei UI");
            CreateFontW(
                scaled(height),
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                CLEARTYPE_QUALITY as u32,
                DEFAULT_PITCH as u32,
                face.as_ptr(),
            )
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
}

#[cfg(not(windows))]
mod imp {
    pub(super) fn run_window() -> bool {
        false
    }

    pub(super) fn close_window() {}

    pub(super) fn alert(title: &str, message: &str) {
        eprintln!("{title}: {message}");
    }
}
