//! 子进程与日志管道。
//!
//! 几个不显然但必须做对的地方：
//! - 用 `java.exe` 而不是 `javaw.exe`：
//!   `javaw` 会把输出丢掉，日志管道就成空的了
//! - `appdata` 环境变量指向游戏目录：不少 mod 会去读它，不设就读到系统的 `%APPDATA%`
//! - 工作目录是实例的游戏目录，不是版本目录 —— 存档和 config 都在那里
//! - Windows 上要加 `CREATE_NO_WINDOW`，否则从 GUI 启动会闪一个黑框
//! - 输出可能不是合法 UTF-8（老版本用系统编码），按 UTF-8 硬读会断流，
//!   所以按字节读行再 lossy 解码

use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tracing::debug;
use xcl_core::{AppError, CommandLine};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub stream: LogStream,
    pub text: String,
}

pub struct SpawnOptions<'a> {
    /// java 可执行文件
    pub java: &'a Path,
    pub command: &'a CommandLine,
    /// 工作目录（游戏目录）
    pub working_dir: &'a Path,
    /// 额外环境变量，例如 `appdata`
    pub envs: &'a [(String, String)],
}

/// 跑起来的游戏进程。
pub struct GameProcess {
    pub child: Child,
    /// 合并后的 stdout + stderr。两个流都关闭时 `recv()` 返回 `None`。
    pub logs: mpsc::UnboundedReceiver<LogLine>,
}

/// 拉起游戏进程。不会等待，调用方负责 `wait()` 与读日志。
pub fn spawn(options: &SpawnOptions<'_>) -> Result<GameProcess, AppError> {
    let mut command = Command::new(options.java);
    command.args(&options.command.jvm);
    command.arg(&options.command.main_class);
    command.args(&options.command.game);

    command.current_dir(options.working_dir);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    // 不少 mod 靠这个变量找「.minecraft」，不设就会跑到系统的 AppData 里
    command.env("appdata", options.working_dir);
    for (key, value) in options.envs {
        command.env(key, value);
    }

    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW = 0x08000000
        command.creation_flags(0x0800_0000);
    }

    let mut child = command.spawn().map_err(|err| {
        AppError::io(&err, "启动 Java 进程").with_hint(
            "确认 Java 路径有效、且该版本需要的 Java 版本已安装；若提示内存不足请调小最大内存",
        )
    })?;

    let (tx, rx) = mpsc::unbounded_channel();
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(pump(stdout, LogStream::Stdout, tx.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(pump(stderr, LogStream::Stderr, tx.clone()));
    }
    // 丢掉自己这一份，两个 pump 都结束后 channel 才会关闭
    drop(tx);

    Ok(GameProcess { child, logs: rx })
}

/// 把一路输出读成行，逐条送进 channel。
async fn pump<R>(reader: R, stream: LogStream, tx: mpsc::UnboundedSender<LogLine>)
where
    R: AsyncRead + Unpin,
{
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::with_capacity(256);

    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) => break,
            Ok(_) => {
                // 去掉行尾的 \r\n
                while matches!(buf.last(), Some(b'\n') | Some(b'\r')) {
                    buf.pop();
                }
                // 容错解码：老版本控制台输出是 GBK，硬按 UTF-8 会得到一片问号，
                // 但至少不会因为一行脏数据把整条流掐断
                let text = String::from_utf8_lossy(&buf).into_owned();
                if tx.send(LogLine { stream, text }).is_err() {
                    // 接收端没了（关掉了界面），停止读
                    break;
                }
            }
            Err(err) => {
                debug!(?stream, error = %err, "读取游戏输出失败，流已结束");
                break;
            }
        }
    }
}

/// 强制结束进程。已经自然退出时调用也不会报错。
pub fn kill(child: &mut Child) {
    // start_kill 只发信号不等待，避免卡在僵尸态
    let _ = child.start_kill();
}

/// 把退出状态翻成给看的结论。
pub fn describe_exit(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(0) => "游戏已正常退出".to_string(),
        Some(code) => format!("游戏异常退出（退出码 {code}）"),
        None => "游戏进程被强制结束".to_string(),
    }
}
