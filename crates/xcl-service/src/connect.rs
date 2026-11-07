//! 联机组件（EasyTier）的下载与安装。
//!
//! 这一层只做「把组件装到盘上」：拉清单 → 下载 → 校验 → 解压 → 改名 → 落标记。
//! 起进程、管连接在后续模块里，不在这里。
//!
//! # 为什么没就绪时整个功能都不露出来
//!
//! 组件是本机唯一能跟服务器说话的东西，没它就什么都干不了。所以界面上只给「下载」一个动作，
//! 装好之后才出现创建 / 加入房间 —— 避免点了半天才发现缺东西。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tokio_util::sync::CancellationToken;
use xcl_connect::manifest::{CLI_INSTALLED, CORE_INSTALLED, ConnectManifest, installed_name};
use xcl_core::AppError;
use xcl_download::{DownloadItem, DownloadOptions, Progress};

use crate::AppState;
use crate::download::{ProgressSink, TaskPhase, TaskView, new_task_id, run_task};

/// 任务表里的键前缀。同一个键只会有一个任务在跑。
const TASK_KEY: &str = "connect-module";

/// 装好之后落的标记文件，记版本与逐文件校验值。
const MARKER: &str = "installed.json";

/// 组件当前状态。前端只认 `ready`，其余用于显示。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectModuleStatus {
    /// 两个可执行文件都在盘上，可以用了。
    pub ready: bool,
    /// 正在下载后处理（校验 + 解压）。界面据此显示「正在准备」，而不是又弹回下载按钮。
    pub installing: bool,
    pub version: Option<String>,
    /// 主程序路径，后端算好给前端显示（前端不拼路径）。
    pub core_path: Option<String>,
    pub cli_path: Option<String>,
    /// 整个组件目录占多少字节。
    pub bytes: u64,
    /// 失败原因等补充说明。
    pub message: Option<String>,
}

impl ConnectModuleStatus {
    fn missing() -> Self {
        Self {
            ready: false,
            installing: false,
            version: None,
            core_path: None,
            cli_path: None,
            bytes: 0,
            message: None,
        }
    }

    /// 下载已完成，正在校验 / 解压。这一步没有进度可报，界面只显示「正在准备」。
    fn unpacking() -> Self {
        Self {
            ready: false,
            installing: true,
            version: None,
            core_path: None,
            cli_path: None,
            bytes: 0,
            message: None,
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self {
            ready: false,
            installing: false,
            version: None,
            core_path: None,
            cli_path: None,
            bytes: 0,
            message: Some(message.into()),
        }
    }
}

impl AppState {
    /// 组件是否就绪。
    ///
    /// 判定标准是两个可执行文件都在，而不是「标记文件存在」——标记可能因为中途断电
    /// 变成孤儿，而 exe 在不在是硬事实。
    pub fn connect_module_status(&self) -> ConnectModuleStatus {
        let root = self.layout.connect_tools_dir();
        let mut versions: Vec<PathBuf> = match std::fs::read_dir(&root) {
            Ok(iter) => iter
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect(),
            Err(_) => return ConnectModuleStatus::missing(),
        };
        // 目录名就是版本号，从新到旧找第一个完整的
        versions.sort();

        for dir in versions.iter().rev() {
            let core = dir.join(CORE_INSTALLED);
            let cli = dir.join(CLI_INSTALLED);
            if !core.is_file() || !cli.is_file() {
                continue;
            }
            let bytes = dir_bytes(dir);
            return ConnectModuleStatus {
                ready: true,
                installing: false,
                version: dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.to_string()),
                core_path: Some(core.to_string_lossy().into_owned()),
                cli_path: Some(cli.to_string_lossy().into_owned()),
                bytes,
                message: None,
            };
        }
        ConnectModuleStatus::missing()
    }

    /// 读取远端清单。
    ///
    /// 地址来自 `config.toml` 的 `connect.manifestUrl`：部署信息不进代码。
    /// 空着说明这个构建没有联机服务，直接说清楚，别发一个注定失败的请求。
    async fn connect_manifest(&self) -> Result<ConnectManifest, AppError> {
        let url = self.config().connect.manifest_url.trim().to_string();
        if url.is_empty() {
            return Err(AppError::unsupported(
                "没有配置联机服务",
                "这个构建没有内置联机组件地址",
            )
            .with_hint("在config.toml的connect.manifestUrl里填上清单地址"));
        }
        let (text, _) = self.fetch_meta_text(&url).await?;
        ConnectManifest::parse(&text)
    }

    /// 下载并安装联机组件。
    ///
    /// 立刻返回初始任务（进度进右下角的下载球），装完通过 `on_status` 通知调用方。
    pub async fn connect_module_install(
        self: &Arc<Self>,
        emit: ProgressSink,
        on_status: Arc<dyn Fn(ConnectModuleStatus) + Send + Sync>,
    ) -> Result<TaskView, AppError> {
        // 已经装好就不重复下载
        let current = self.connect_module_status();
        if current.ready {
            on_status(current);
            return Err(AppError::unsupported("联机组件已经就绪", "不需要重复下载"));
        }

        let task_key = format!("{TASK_KEY}:install");
        if let Some(running) = self.tasks().running_for(&task_key) {
            return Ok(running);
        }

        let manifest = self.connect_manifest().await?;
        let dir = self.layout.connect_module_dir(&manifest.version);
        let pkg_dir = dir.join(".pkg");
        tokio::fs::create_dir_all(&pkg_dir)
            .await
            .map_err(|err| AppError::io(&err, "创建联机组件目录"))?;
        let package = pkg_dir.join(manifest.archive_name());

        let urls = manifest.urls();
        // 组件包不分片：单线程实测 2.3 MB/s，开多连接反而互相抢带宽。
        let split_urls = if manifest.archive.allow_split {
            crate::download::split_urls_for(&urls)
        } else {
            Vec::new()
        };

        let items = vec![DownloadItem {
            name: "XCL联机模块".to_string(),
            urls,
            split_urls,
            dest: package.clone(),
            sha1: None,
            size: manifest.archive.size,
        }];

        let task_id = new_task_id();
        let cancel = CancellationToken::new();
        let progress = Progress::new(manifest.archive.size, 1);
        let mut initial = TaskView::new(&task_id, &task_key, 1, manifest.archive.size);
        initial.title = Some("XCL联机模块".to_string());
        self.tasks()
            .insert(task_id.clone(), cancel.clone(), initial.clone());
        emit(initial.clone());

        let state = Arc::clone(self);
        let options = DownloadOptions {
            concurrency: 1,
            attempts_per_url: 2,
            split: state.split_config(),
        };
        let fetcher = Arc::clone(state.fetcher());

        tokio::spawn(async move {
            run_task(
                Arc::clone(&state),
                task_id.clone(),
                items,
                fetcher,
                progress,
                options,
                cancel,
                Arc::clone(&emit),
            )
            .await;

            // 下载结束后才开始校验与解压。任务表是唯一真相，所以从表里读最终阶段。
            let phase = state
                .tasks()
                .list()
                .into_iter()
                .find(|view| view.task_id == task_id)
                .map(|view| view.phase);
            if phase != Some(TaskPhase::Done) {
                on_status(ConnectModuleStatus::failed("组件下载没有完成"));
                return;
            }

            let package_for_task = package.clone();
            let dir_for_task = dir.clone();
            let manifest_for_task = manifest.clone();
            // 下载结束到装好之间还有校验 + 解压，会有肉眼可见的一两秒。
            // 不先发这个态的话，界面会以为任务结束＝没装成，又把「下载组件」摆回来。
            on_status(ConnectModuleStatus::unpacking());
            let unpacked = tokio::task::spawn_blocking(move || {
                unpack_and_install(&package_for_task, &dir_for_task, &manifest_for_task)
            })
            .await;

            match unpacked {
                Ok(Ok(())) => on_status(state.connect_module_status()),
                Ok(Err(err)) => on_status(ConnectModuleStatus::failed(err.message)),
                Err(err) => on_status(ConnectModuleStatus::failed(format!("解压过程意外中断：{err}"))),
            }
        });

        Ok(initial)
    }
}

/// 校验压缩包 → 解压 → 逐个核对 → 改名 → 落到正式目录。
///
/// 全程在临时目录里做，最后才逐个搬过去：中途失败最多留下 `.staging`，
/// 不会出现「半个组件被当成装好了」。整个过程都是同步 IO，调用方要放进 `spawn_blocking`。
fn unpack_and_install(package: &Path, dir: &Path, manifest: &ConnectManifest) -> Result<(), AppError> {
    let actual = xcl_update::verify::sha256_file(package)?;
    if !actual.eq_ignore_ascii_case(&manifest.archive.sha256) {
        // 压缩包坏了就没有继续的意义，顺手删掉，下次会重新下
        let _ = std::fs::remove_file(package);
        return Err(AppError::parse(
            "联机组件包已损坏",
            "下载到的文件与清单里的校验值对不上",
        )
        .with_hint("已自动删除，重试一次即可"));
    }

    let staging = dir.join(".staging");
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    std::fs::create_dir_all(&staging).map_err(|err| AppError::io(&err, "创建临时目录"))?;

    let file = std::fs::File::open(package).map_err(|err| AppError::io(&err, "打开组件包"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|err| {
        AppError::parse("联机组件包解不开", format!("不是有效的 zip：{err}"))
    })?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|err| {
            AppError::parse("联机组件包内容损坏", format!("第 {index} 项读不出来：{err}"))
        })?;
        if entry.is_dir() {
            continue;
        }
        // 只取文件名：压缩包里可能带目录前缀，直接拼路径会有目录穿越风险
        let raw = entry.name().rsplit('/').next().unwrap_or("").to_string();
        if raw.is_empty() {
            continue;
        }
        let target = staging.join(installed_name(&raw));
        let mut out =
            std::fs::File::create(&target).map_err(|err| AppError::io(&err, "写出组件文件"))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|err| AppError::io(&err, "写出组件文件"))?;
    }

    // 逐个核对清单里的校验值（有就查，没有就跳过）
    for expected in &manifest.files {
        if expected.sha256.len() != 64 {
            continue;
        }
        let path = staging.join(installed_name(&expected.name));
        if !path.is_file() {
            if expected.required {
                return Err(AppError::parse(
                    "联机组件包不完整",
                    format!("包里缺少 {}", expected.name),
                )
                .with_hint("可能是发布时打错了包，请联系维护者"));
            }
            continue;
        }
        let actual = xcl_update::verify::sha256_file(&path)?;
        if !actual.eq_ignore_ascii_case(&expected.sha256) {
            return Err(AppError::parse(
                "联机组件校验失败",
                format!("{} 与清单记录的不一致", expected.name),
            )
            .with_hint("重新下载一次；仍然失败请联系维护者"));
        }
    }

    for required in [CORE_INSTALLED, CLI_INSTALLED] {
        if !staging.join(required).is_file() {
            return Err(AppError::parse(
                "联机组件包不完整",
                format!("缺少 {required}"),
            )
            .with_hint("重新下载一次；仍然失败请联系维护者"));
        }
    }

    // 搬到正式目录
    let entries =
        std::fs::read_dir(&staging).map_err(|err| AppError::io(&err, "读取临时目录"))?;
    for entry in entries {
        let entry = entry.map_err(|err| AppError::io(&err, "读取临时目录"))?;
        let name = entry.file_name();
        let target = dir.join(&name);
        if target.exists() {
            let _ = std::fs::remove_file(&target);
        }
        std::fs::rename(entry.path(), &target)
            .map_err(|err| AppError::io(&err, "移动到组件目录"))?;
    }

    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_dir_all(dir.join(".pkg"));

    write_marker(dir, manifest)?;
    Ok(())
}

/// 落一个标记，记下装的是哪版、含哪些文件。纯记录用途，判定仍以 exe 是否存在为准。
fn write_marker(dir: &Path, manifest: &ConnectManifest) -> Result<(), AppError> {
    let payload = serde_json::json!({
        "version": manifest.version,
        "build": manifest.build,
        "platform": manifest.platform,
        "archiveSha256": manifest.archive.sha256,
        "files": manifest.files.iter().map(|file| {
            serde_json::json!({
                "name": installed_name(&file.name),
                "size": file.size,
                "sha256": file.sha256,
            })
        }).collect::<Vec<_>>(),
        "installedAt": chrono_like_now(),
    });
    let text = serde_json::to_string_pretty(&payload).unwrap_or_default();
    std::fs::write(dir.join(MARKER), text).map_err(|err| AppError::io(&err, "写入安装标记"))
}

/// 秒级时间戳。这里只要一个「什么时候装的」，不值得为此引入 chrono。
fn chrono_like_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn dir_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.metadata().ok())
        .map(|meta| meta.len())
        .sum()
}

/// 扫本机 Minecraft 正在监听的端口。
///
/// 单机「对局域网开放」的端口是随机高位，猜不出来，只能真去查：
/// 先找出所有 java/javaw 进程，再看它们各自监听了哪个 TCP 端口。
/// `netstat` 比引 winapi 便宜，这里只是取一张端口表，不值得为它加平台代码。
///
/// 返回结果里 25565 排最前（专用服和默认端口），其余按端口号升序。
/// 扫不到就返回空 —— 调用方保留手填的值。
pub fn scan_minecraft_ports() -> Vec<u16> {
    let mut system = sysinfo::System::new_all();
    system.refresh_all();

    let mut pids: Vec<u32> = Vec::new();
    for (pid, process) in system.processes() {
        let name = process.name().to_string_lossy().to_ascii_lowercase();
        // 用包含而不是枚举：JDK 自带的各种 java 变体名字不完全一样。
        if name.contains("java") {
            pids.push(pid.as_u32());
        }
    }
    if pids.is_empty() {
        return Vec::new();
    }

    let mut netstat = std::process::Command::new("netstat");
    netstat.args(["-ano", "-p", "tcp"]);
    #[cfg(windows)]
    {
        // netstat 也是控制台程序。启动器是 GUI 子系统，spawn 它就会闪一个黑框 ——
        // 开房时自动扫端口正好走到这里，看到的就是「一开房窗口弹一下」。
        use std::os::windows::process::CommandExt as _;
        netstat.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let Ok(output) = netstat.output() else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);

    let mut ports: Vec<u16> = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 5 || !cols[0].eq_ignore_ascii_case("TCP") {
            continue;
        }
        if !cols[3].eq_ignore_ascii_case("LISTENING") {
            continue;
        }
        let Ok(pid) = cols[4].parse::<u32>() else {
            continue;
        };
        if !pids.contains(&pid) {
            continue;
        }
        let Some(port) = cols[1].rsplit(':').next().and_then(|s| s.parse::<u16>().ok()) else {
            continue;
        };
        // 跳过 java 自己的调试端口之类；游戏端口不会低于 1024
        if port >= 1024 && !ports.contains(&port) {
            ports.push(port);
        }
    }

    ports.sort_unstable();
    if let Some(index) = ports.iter().position(|port| *port == 25565) {
        ports.swap(0, index);
    }
    ports
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sample_manifest(archive_sha: &str) -> ConnectManifest {
        xcl_connect::manifest::ConnectManifest::parse(&format!(
            r#"{{
              "product":"easytier","version":"9.9.9","platform":"win-x64",
              "archive":{{"primary":"https://example.com/pkg.zip","size":123,
                         "sha256":"{archive_sha}"}},
              "files":[
                {{"name":"easytier-core.exe","size":3,"sha256":"{core}","required":true}},
                {{"name":"easytier-cli.exe","size":2,"sha256":"{cli}","required":true}},
                {{"name":"Packet.dll","size":1,"sha256":"{dll}","required":true}}
              ]
            }}"#,
            core = sha_of(b"core"),
            cli = sha_of(b"cli"),
            dll = sha_of(b"d"),
        ))
        .expect("样例清单应当能解析")
    }

    fn sha_of(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
    }

    fn build_zip(path: &Path) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, data) in [
            ("easytier-core.exe", &b"core"[..]),
            ("easytier-cli.exe", &b"cli"[..]),
            ("Packet.dll", &b"d"[..]),
            ("NOTICE.txt", &b"notice"[..]),
        ] {
            zip.start_file(name, options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn installs_and_renames_executables() {
        let root = std::env::temp_dir().join(format!("xcl-connect-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("9.9.9");
        std::fs::create_dir_all(&dir).unwrap();
        let package = dir.join("pkg.zip");
        build_zip(&package);

        let archive_sha = sha_of(&std::fs::read(&package).unwrap());
        let manifest = sample_manifest(&archive_sha);

        unpack_and_install(&package, &dir, &manifest).expect("应当安装成功");

        assert!(dir.join("XCL联机模块.exe").is_file(), "主程序应当改名");
        assert!(dir.join("XCL联机模块 CLI.exe").is_file(), "CLI 应当改名");
        assert!(dir.join("Packet.dll").is_file(), "动态库必须保持原名");
        assert!(dir.join("NOTICE.txt").is_file());
        assert!(dir.join(MARKER).is_file(), "应当落安装标记");
        assert!(!dir.join(".staging").exists(), "临时目录应当清理掉");
        assert!(!dir.join(".pkg").exists(), "压缩包应当清理掉");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_tampered_package() {
        let root = std::env::temp_dir().join(format!("xcl-connect-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("9.9.9");
        std::fs::create_dir_all(&dir).unwrap();
        let package = dir.join("pkg.zip");
        build_zip(&package);

        // 清单里写一个对不上的校验值
        let manifest = sample_manifest(&"0".repeat(64));
        let err = unpack_and_install(&package, &dir, &manifest).unwrap_err();
        assert!(err.title.contains("损坏"));
        assert!(!package.exists(), "坏包应当被删掉，方便下次重下");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_required_file_fails() {
        let root = std::env::temp_dir().join(format!("xcl-connect-miss-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("9.9.9");
        std::fs::create_dir_all(&dir).unwrap();
        let package = dir.join("pkg.zip");
        build_zip(&package);
        let archive_sha = sha_of(&std::fs::read(&package).unwrap());

        // 清单声称有四个文件，包里却少了 cli
        let manifest = ConnectManifest::parse(&format!(
            r#"{{"version":"9.9.9","platform":"win-x64",
                 "archive":{{"primary":"https://e.com/p.zip","size":1,"sha256":"{archive_sha}"}},
                 "files":[{{"name":"easytier-core.exe","size":3,"sha256":"{core}","required":true}},
                          {{"name":"easytier-cli.exe","size":2,"sha256":"{cli}","required":true}},
                          {{"name":"Packet.dll","size":1,"sha256":"{dll}","required":true}}]}}"#,
            core = sha_of(b"core"),
            cli = "a".repeat(64),
            dll = sha_of(b"d"),
        ))
        .unwrap();

        let err = unpack_and_install(&package, &dir, &manifest).unwrap_err();
        assert!(err.title.contains("校验失败") || err.title.contains("不完整"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
