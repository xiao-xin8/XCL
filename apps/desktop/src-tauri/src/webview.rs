//! 启动前确保 WebView2 运行时存在（仅 Windows 有实际动作）。
//!
//! 为什么必须自己写：`tauri.conf.json` 的 `bundle.windows.webviewInstallMode`
//! 只在安装包（NSIS / MSI）里生效。XCL 发的是单个 exe，Tauri 在建窗口失败时
//! 只会抛一句英文错误，普通玩家看到的就是「双击没反应 / 弹个看不懂的框」。
//!
//! 这里在 Tauri 初始化之前跑：查注册表 → 缺了就下载官方 bootstrapper
//! 静默安装 → 装完继续启动。全程用系统原生对话框，不依赖任何界面框架
//! （WebView2 没装，Tauri 窗口根本创建不出来，没法用自己画 UI）。

#[cfg(windows)]
mod imp {
    use std::path::PathBuf;
    use std::process::Command;

    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MB_YESNO,
        MessageBoxW,
    };
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    /// WebView2 Evergreen 运行时在注册表里的产品 ID。
    const PRODUCT: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

    /// 官方 Evergreen Bootstrapper 的固定短链（302 到当前最新版，约 1.8MB）。
    /// 装的时候它自己再下载真正的运行时，所以必须联网。
    const BOOTSTRAPPER_URL: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";

    /// 已安装的运行时版本号；`None` = 没装。
    ///
    /// 三个位置都要看：64 位系统上 32 位安装器写在 WOW6432Node，新版本可能写
    /// 在原生节点，也有 per-user 安装（HKCU，不需要管理员权限）。
    /// `pv` 为 `0.0.0.0` 表示「登记了但没装好」，按缺失处理。
    ///
    /// 微软官方文档给的就是这个检测方式（`EdgeUpdate\Clients\{产品ID}` 的 `pv`），
    /// 不是我们自创的偏方。
    fn runtime_version() -> Option<String> {
        const SUBKEYS: [&str; 2] = [
            r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\",
            r"SOFTWARE\Microsoft\EdgeUpdate\Clients\",
        ];
        for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
            for sub in SUBKEYS {
                let path = format!("{sub}{PRODUCT}");
                let Ok(key) = RegKey::predef(root).open_subkey(&path) else {
                    continue;
                };
                let Ok(version) = key.get_value::<String, _>("pv") else {
                    continue;
                };
                let version = version.trim().to_string();
                if !version.is_empty() && version != "0.0.0.0" {
                    return Some(version);
                }
            }
        }
        None
    }

    /// 演练模式：跳过「已装」判定，把整条链路走一遍。
    ///
    /// 开发机上都装着 WebView2，真实缺失分支根本走不到，等于没法验证。
    /// 设环境变量 `XCL_WEBVIEW_DRILL=1` 启动，就能看到
    /// 「询问 → 下载 → 安装 → 继续启动」的完整流程（本机已装时安装器会
    /// 立刻返回「已是最新」，不会真的重装）。
    fn drill() -> bool {
        std::env::var("XCL_WEBVIEW_DRILL").is_ok_and(|value| value.trim() == "1")
    }

    /// 原生消息框。返回按钮 ID（`IDYES` = 6，`IDOK` = 1）。
    fn message(text: &str, title: &str, flags: u32) -> i32 {
        let wide = |value: &str| -> Vec<u16> {
            value.encode_utf16().chain(std::iter::once(0)).collect()
        };
        let text = wide(text);
        let title = wide(title);
        // SAFETY: 两个指针都指向以 NUL 结尾的、在调用期间存活的 UTF-16 缓冲。
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                flags | MB_SETFOREGROUND,
            ) as i32
        }
    }

    /// 下载 bootstrapper 到临时目录。
    fn download() -> Result<PathBuf, String> {
        let path = std::env::temp_dir().join("XCL-WebView2-Setup.exe");
        let response = reqwest::blocking::get(BOOTSTRAPPER_URL)
            .map_err(|err| format!("连接下载服务器失败：{err}"))?;
        if !response.status().is_success() {
            return Err(format!("下载失败：HTTP {}", response.status()));
        }
        let bytes = response
            .bytes()
            .map_err(|err| format!("读取下载内容失败：{err}"))?;
        // 正常有 1.7MB 左右；太小基本是运营商劫持页或错误页
        if bytes.len() < 512 * 1024 {
            return Err(format!(
                "下载到的文件不完整（{} 字节），可能被网络拦截了",
                bytes.len()
            ));
        }
        std::fs::write(&path, &bytes).map_err(|err| format!("写入临时文件失败：{err}"))?;
        Ok(path)
    }

    /// 运行安装程序 —— 带界面，不加 `/silent`。
    ///
    /// 为什么不静默：真正的运行时有一百多兆，静默模式下面对的是一个
    /// 没有任何反馈的窗口，多半会以为卡死、然后强杀进程。微软自己的安装
    /// 向导有进度条和「安装」按钮，看得见它在干什么 —— 宁可多一次点击，
    /// 也不要「安静但可疑」。
    ///
    /// 退出码不可信（取消 / 已是最新 / 成功都可能是 0），调用方以注册表为准。
    fn run_installer(path: &PathBuf) -> Result<(), String> {
        Command::new(path)
            .status()
            .map(|_| ())
            .map_err(|err| format!("启动安装程序失败：{err}"))
    }

    /// 打开官方下载页，作为最后一道兜底。
    fn open_help() {
        let _ = Command::new("cmd")
            .args(["/c", "start", "", BOOTSTRAPPER_URL])
            .spawn();
    }

    /// `true` = 运行时可用，可以继续启动；`false` = 应该退出。
    pub fn ensure() -> bool {
        let drill = drill();
        // 演练模式：假装没装，把流程走一遍（安装器在已装机器上是幂等的）
        if !drill {
            if let Some(version) = runtime_version() {
                // 只有 debug 构建才打印，release 没有控制台也不影响
                if cfg!(debug_assertions) {
                    eprintln!("[webview] WebView2 运行时已安装：{version}");
                }
                return true;
            }
        }

        let note = if drill { "（演练模式）" } else { "" };
        let proceed = message(
            &format!(
                "XCL 需要「Microsoft Edge WebView2 运行时」才能显示界面，\
                 这台电脑还没有安装。{note}\n\n\
                 是否现在自动安装？\n\n\
                 · 会先下载一个 1.8 MB 的安装引导程序\n\
                 · 然后弹出微软的安装窗口，点「安装」等它跑完即可\n\
                 · 整个过程通常 1~3 分钟，需要联网\n\
                 · 不需要管理员权限；如果弹出权限确认，请点「是」"
            ),
            &format!("缺少运行组件{note}"),
            MB_YESNO | MB_ICONWARNING,
        ) == IDYES;

        if !proceed {
            open_help();
            return false;
        }

        message(
            "接下来会弹出微软的安装窗口。\n\n\
             在窗口里点「安装」，等进度条走完，XCL 会自动打开。\n\
             如果中途关掉了那个窗口，重新打开 XCL 就会再来一次。",
            "准备安装",
            MB_OK | MB_ICONINFORMATION,
        );

        // 下载：网络抖一下就失败太脆，重试一次
        let mut error: Option<String> = None;
        let mut installer = None;
        for attempt in 0..2 {
            match download() {
                Ok(path) => {
                    installer = Some(path);
                    break;
                }
                Err(err) => {
                    if attempt == 0 {
                        std::thread::sleep(std::time::Duration::from_millis(800));
                    }
                    error = Some(err);
                }
            }
        }

        let install_result = match installer {
            Some(path) => {
                let outcome = run_installer(&path);
                // 装完就把安装包删掉，别在临时目录留垃圾
                let _ = std::fs::remove_file(&path);
                outcome
            }
            None => Err(error.unwrap_or_else(|| "下载失败".to_string())),
        };

        // 以注册表为准，不看安装器退出码。
        // WebView2 bootstrapper 的退出码语义不统一（「已是最新」「已安装」
        // 都可能给非 0），拿它判断会对已经装好的机器误报失败。
        if runtime_version().is_some() {
            return true;
        }

        let reason = install_result.err().unwrap_or_else(|| {
            "安装程序已关闭，但没有检测到运行时".to_string()
        });
        message(
            &format!(
                "没有检测到安装完成（{reason}）。\n\n\
                 可能是刚才关掉了安装窗口，或者安装被中途打断。\n\n\
                 已经为你打开官方下载页面：下载后双击运行、点「安装」，\n\
                 装好之后再打开 XCL 就行。\n\n\
                 如果还是不行，把这句话发给帮你装电脑的人：\n\
                 需要安装 Microsoft Edge WebView2 Runtime。"
            ),
            "安装未完成",
            MB_OK | MB_ICONERROR,
        );
        open_help();
        false
    }
}

#[cfg(not(windows))]
mod imp {
    /// 其它平台不需要（Linux 用系统的 WebKitGTK，macOS 用 WKWebView）。
    pub fn ensure() -> bool {
        true
    }
}

/// 启动前检查。返回 `false` 表示运行组件不可用，主程序应当直接退出。
pub fn ensure() -> bool {
    imp::ensure()
}
