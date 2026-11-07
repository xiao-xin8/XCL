//! Java 探测。
//!
//! 只做「本机已装了什么」，不负责下载 —— 自举下载是 `xcl-service::java_runtime`
//! 的事（装到 `data/java/`，由调用方通过 `detect_all_with` 的 extra_roots 扫进来）。
//!
//! 扫描范围（Windows 的安装根清单）：
//! 1. `JDK_HOME` / `JAVA_HOME` —— 安装器显式指定的，应当尊重
//! 2. `PATH` 里能找到的每个 java
//! 3. 各启动器的运行时目录：官方 MC 启动器和常见第三方启动器 ——
//!    机器上「有一打 Java」多半是这么来的，这个清单漏一个就是「检测做不明白」
//! 4. 各发行版的默认安装目录（Adoptium / Corretto / Zulu / BellSoft / Microsoft / IDEA 的 .jdks）
//!
//! 判定版本靠跑 `java -version`：它输出到 stderr，不是 stdout（很多人踩过）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;
use tracing::{debug, warn};
use xcl_core::AppError;

/// 探测单个 Java 的超时。正常情况下 `java -version` 是毫秒级；
/// 卡住的多半是损坏的安装或网络盘上的 java，等下去没意义。
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// 递归翻一个根目录时的最大深度。启动器运行时的真实布局是
/// `<根>/<组件>/bin/java`（深度 1），macOS 的 `Contents/Home/bin` 是深度 3，
/// 留余量给意外嵌套，再深的一律不翻。
const MAX_SCAN_DEPTH: u32 = 4;

/// 单个根最多翻多少层目录。防的是网络盘/损坏树把扫描拖死。
const MAX_DIRS_PER_ROOT: usize = 4096;

/// 要排掉的「特殊路径」片段：自家临时目录 + system32 里的 java 垫片。
const SPECIAL_PATH_MARKERS: [&str; 4] = ["java8path_target_", "javapath_target_", "javatmp", "system32"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaInstall {
    /// java 可执行文件（Windows 上是 `java.exe`，不是 `javaw.exe`）
    pub path: PathBuf,
    /// 完整版本串，如 `17.0.2` / `1.8.0_202`
    pub version: String,
    /// 主版本号：`8` / `17` / `21`
    pub major: u32,
}

#[cfg(windows)]
const JAVA_BIN: &str = "java.exe";
#[cfg(not(windows))]
const JAVA_BIN: &str = "java";

/// 扫描本机所有可用的 Java，按主版本降序（新版本在前，通常更好用）。
pub async fn detect_all() -> Vec<JavaInstall> {
    detect_all_with(&[]).await
}

/// 同上，但额外扫一遍指定的根目录（每个子目录下找 `bin/java`）。
///
/// 用来把我们自己下载的运行时（`data/java/<组件>/`）算进来 ——
/// 否则刚下完的 Java 探测不到，会以为白下了。
pub async fn detect_all_with(extra_roots: &[PathBuf]) -> Vec<JavaInstall> {
    // 目录扫描是纯同步 IO（几十个根、每根上千个目录）。它在启动链路上，
    // 不能占着 tokio 的 worker：慢盘/网络盘会把其他 IPC 一起拖住。
    let roots = extra_roots.to_vec();
    let candidates = match tokio::task::spawn_blocking(move || candidate_paths(&roots)).await {
        Ok(candidates) => candidates,
        Err(err) => {
            // 阻塞池异常（极罕见）：退回同步扫描，保证探测结果不变
            warn!(error = %err, "扫描 Java 目录的阻塞任务异常，回退同步扫描");
            candidate_paths(extra_roots)
        }
    };

    let mut seen_paths: HashSet<PathBuf> = HashSet::new();
    let mut handles = Vec::with_capacity(candidates.len());
    for path in candidates {
        if !path.is_file() || !seen_paths.insert(path.clone()) {
            continue;
        }
        handles.push(tokio::spawn(async move { probe(&path).await }));
    }

    let mut found = Vec::new();
    for handle in handles {
        let Ok(Some(install)) = handle.await else {
            continue;
        };
        found.push(install);
    }

    let mut found = dedup_by_path(found);
    found.sort_by(|a, b| b.major.cmp(&a.major).then_with(|| a.path.cmp(&b.path)));
    found
}

/// 按物理路径去重。
///
/// 同一份 JDK 常被多处引用（JAVA_HOME + PATH + 目录扫描），canonicalize 之后
/// 自然归一。不能按 (主版本, 版本串) 去重 —— 机器上装着一打 Java，
/// 两个不同路径的同版本安装是独立的两份，都得展示（按文件夹去重）：
/// `.minecraft\runtime` 里的 Java 8 和 `data/java` 里的 Java 8 都该出现。
fn dedup_by_path(installs: Vec<JavaInstall>) -> Vec<JavaInstall> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out: Vec<JavaInstall> = Vec::with_capacity(installs.len());
    for install in installs {
        let key = std::fs::canonicalize(&install.path).unwrap_or_else(|_| install.path.clone());
        if seen.insert(key) {
            out.push(install);
        }
    }
    out
}

/// 在已探测结果里挑一个满足要求的。
///
/// 策略：满足条件的里面挑最低的 —— 要求 Java 8 的版本塞个 Java 21 进去，
/// 老 Forge / 老 mod 反而容易崩；能用低版本就用低版本。
pub fn pick(installs: &[JavaInstall], min: u32, max: Option<u32>) -> Option<&JavaInstall> {
    installs
        .iter()
        .filter(|java| java.major >= min && max.map_or(true, |m| java.major <= m))
        .min_by_key(|java| java.major)
}

/// 跑一次 `java -version` 拿到版本号。
pub async fn probe(path: &Path) -> Option<JavaInstall> {
    let mut command = Command::new(path);
    command.arg("-version");
    command.stdin(std::process::Stdio::null());
    // 不加这个，从 GUI 启动时会闪一个黑色控制台窗口
    #[cfg(windows)]
    {
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let output = match tokio::time::timeout(PROBE_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => {
            debug!(path = %path.display(), error = %err, "Java 探测失败");
            return None;
        }
        Err(_) => {
            warn!(path = %path.display(), "Java 探测超时");
            return None;
        }
    };

    // java -version 把版本信息写到 stderr
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );

    let (version, major) = parse_version(&text)?;
    Some(JavaInstall {
        path: path.to_path_buf(),
        version,
        major,
    })
}

/// 从 `java -version` 的输出里抠出版本号。
///
/// 两种形态：
/// - `openjdk version "17.0.2" 2022-01-18`  → 17
/// - `java version "1.8.0_202"`            → 8（`1.8` 是 Java 8 的老写法）
pub fn parse_version(output: &str) -> Option<(String, u32)> {
    let quoted = output.split('"').nth(1)?.trim();
    if quoted.is_empty() {
        return None;
    }

    let major = if let Some(rest) = quoted.strip_prefix("1.") {
        rest.split(['.', '_', '-', '+']).next()?.parse::<u32>().ok()?
    } else {
        quoted
            .split(['.', '_', '-', '+'])
            .next()?
            .parse::<u32>()
            .ok()?
    };

    Some((quoted.to_string(), major))
}

/// 收集所有可能藏着 java 的路径（存在性检查在 `detect_all_with` 里做）。
fn candidate_paths(extra_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();

    // 1. JDK_HOME / JAVA_HOME：安装器显式指定的，最高优先级。
    //    可能是 JDK 根，也可能直接指向 bin 目录本身，两种形态都试；
    //    多个路径用 `;` 分隔，值可能带引号（安装器就爱写 `"C:\...\"`）。
    for var in ["JDK_HOME", "JAVA_HOME"] {
        let Some(raw) = std::env::var_os(var) else {
            continue;
        };
        for part in std::env::split_paths(&raw) {
            let home = clean_env_path(&part);
            if home.as_os_str().is_empty() {
                continue;
            }
            out.push(home.join("bin").join(JAVA_BIN));
            out.push(home.join(JAVA_BIN));
        }
    }

    // 2. PATH：自己放上去的每个目录
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            out.push(dir.join(JAVA_BIN));
        }
    }

    // 3. Microsoft Build of OpenJDK：只认直接的 `jdk-*` 子目录。
    //    `Program Files\Microsoft` 底下还有 Edge / EdgeWebView 之类的深树，
    //    整个递归翻又慢又没意义（只用 `jdk-*` 模式匹配）。
    #[cfg(windows)]
    for root in jdk_prefix_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if !entry
                .file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .starts_with("jdk-")
            {
                continue;
            }
            out.push(path.join("bin").join(JAVA_BIN));
        }
    }

    // 4. 启动器运行时目录 + 各发行版安装目录（有界递归）
    for root in install_roots() {
        collect_java_under(&root, &mut out);
    }

    // 5. 我们自己下载的运行时（data/java/<组件>/bin/java）
    for root in extra_roots {
        collect_java_under(root, &mut out);
    }

    out
}

/// 只翻直接 `jdk-*` 子目录的根（Microsoft 的目录树太深，不能递归）。
#[cfg(windows)]
fn jdk_prefix_roots() -> Vec<PathBuf> {
    jdk_prefix_roots_with(|var| std::env::var_os(var).map(PathBuf::from))
}

#[cfg(windows)]
fn jdk_prefix_roots_with(get: impl Fn(&str) -> Option<PathBuf>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for base in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = env_path_from(&get, base) {
            roots.push(base.join("Microsoft"));
        }
    }
    // 用户级安装的 Microsoft Build of OpenJDK
    if let Some(local) = env_path_from(&get, "LOCALAPPDATA") {
        roots.push(local.join("Programs").join("Microsoft"));
    }
    roots
}

/// 需要递归翻找的根（每个根下翻到 `MAX_SCAN_DEPTH` 层为止）。
#[cfg(windows)]
fn install_roots() -> Vec<PathBuf> {
    windows_install_roots(|var| std::env::var_os(var).map(PathBuf::from))
}

/// Windows 的安装根清单。
/// 环境变量通过 `get` 注入，方便单测。
#[cfg(windows)]
fn windows_install_roots(get: impl Fn(&str) -> Option<PathBuf>) -> Vec<PathBuf> {
    let mut roots = Vec::new();

    // —— 启动器自带的运行时目录 ——
    // 官方 MC 启动器和常见第三方启动器都会把下载的 Java 放在这些位置。
    // 漏掉这个目录，最常见的两个 Java 直接看不见。
    if let Some(appdata) = env_path_from(&get, "APPDATA") {
        roots.push(appdata.join(".minecraft").join("runtime"));
        roots.push(appdata.join(".hmcl").join("java"));
        roots.push(appdata.join("ATLauncher").join("runtimes").join("minecraft"));
        roots.push(appdata.join("ModrinthApp").join("meta").join("java_versions"));
        roots.push(appdata.join("PrismLauncher").join("java"));
    }
    if let Some(user) = env_path_from(&get, "USERPROFILE") {
        roots.push(user.join(".jdks")); // IDEA 自己下载的 JDK
        roots.push(user.join(".sdkman").join("candidates").join("java"));
        roots.push(
            user.join("curseforge")
                .join("minecraft")
                .join("Install")
                .join("runtime"),
        );
        roots.push(
            user.join("Documents")
                .join("Curse")
                .join("Minecraft")
                .join("Install")
                .join("runtime"),
        );
    }
    if let Some(local) = env_path_from(&get, "LOCALAPPDATA") {
        roots.push(local.join(".ftba").join("bin").join("runtime"));
        // Microsoft Store 版 Minecraft 的运行时
        roots.push(
            local
                .join("Packages")
                .join("Microsoft.4297127D64EC6_8wekyb3d8bbwe")
                .join("LocalCache")
                .join("Local")
                .join("runtime"),
        );
    }

    // —— 各发行版的默认安装目录 ——
    // JDK 就放在厂商目录的子目录里：<根>/<厂商>/<版本>/bin/java
    for base in ["ProgramFiles", "ProgramFiles(x86)"] {
        let Some(base) = env_path_from(&get, base) else {
            continue;
        };
        for vendor in [
            "Java",               // 老 Oracle
            "Eclipse Adoptium",   // Temurin
            "Eclipse Foundation",
            "AdoptOpenJDK",       // 老 Temurin
            "Amazon Corretto",
            "Zulu",
            "BellSoft",
            "Semeru",
            "Minecraft Launcher", // 旧版启动器把运行时装在这儿
        ] {
            roots.push(base.join(vendor));
        }
        roots.push(base.join("Minecraft").join("runtime"));
    }
    // 用户级安装（per-user 的 Temurin / Corretto 等）
    if let Some(local) = env_path_from(&get, "LOCALAPPDATA") {
        for vendor in ["Eclipse Adoptium", "Amazon Corretto", "Zulu", "BellSoft", "Java"] {
            roots.push(local.join("Programs").join(vendor));
        }
    }

    roots
}

#[cfg(target_os = "macos")]
fn install_roots() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/Library/Java/JavaVirtualMachines")];
    if let Some(home) = env_path("HOME") {
        // macOS 的 JDK 布局是 <根>/<版本>.jdk/Contents/Home/bin/java，
        // 递归到深度 4 正好覆盖（Contents 在深度 2、Home 在深度 3）
        roots.push(home.join("Library").join("Java").join("JavaVirtualMachines"));
    }
    roots
}

#[cfg(all(unix, not(target_os = "macos")))]
fn install_roots() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/usr/lib/jvm"), PathBuf::from("/usr/java")];
    if let Some(home) = env_path("HOME") {
        roots.push(home.join(".jdks"));
        roots.push(home.join(".sdkman").join("candidates").join("java"));
    }
    roots
}

/// 有界递归：收集 `root` 下所有的 `*/bin/java`（含 `root/bin/java` 本身）。
///
/// 「有界」指：深度不超过 `MAX_SCAN_DEPTH`、每个根翻的目录数不超过
/// `MAX_DIRS_PER_ROOT`、符号链接目录直接跳过。三重保险防的是网络盘、
/// junction 成环、损坏目录树把扫描拖死。
fn collect_java_under(root: &Path, out: &mut Vec<PathBuf>) {
    let mut visited: HashSet<PathBuf> = HashSet::new();
    walk_dir(root, 0, &mut visited, out);
}

fn walk_dir(dir: &Path, depth: u32, visited: &mut HashSet<PathBuf>, out: &mut Vec<PathBuf>) {
    if depth > MAX_SCAN_DEPTH || visited.len() >= MAX_DIRS_PER_ROOT {
        return;
    }
    // canonicalize 拿到物理路径：失败（不存在/无权限）就放弃这个分支；
    // 翻过也跳过 —— 顺带防住 junction 指向已翻过的目录造成重复或成环
    let Ok(real) = std::fs::canonicalize(dir) else {
        return;
    };
    if !visited.insert(real) {
        return;
    }
    // 同款：这些路径里的 java 不认（自家临时目录、system32 垫片）
    let lowered = dir.to_string_lossy().to_ascii_lowercase();
    if SPECIAL_PATH_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
    {
        return;
    }

    out.push(dir.join("bin").join(JAVA_BIN));

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // 符号链接目录不翻：自己链的不归我们管，PATH / JAVA_HOME 会覆盖到
        if path.is_symlink() || !path.is_dir() {
            continue;
        }
        walk_dir(&path, depth + 1, visited, out);
    }
}

/// 取一个环境变量并翻成路径；空值当没有。
#[cfg(not(windows))]
fn env_path(var: &str) -> Option<PathBuf> {
    env_path_from(&|name| std::env::var_os(name).map(PathBuf::from), var)
}

fn env_path_from(get: &impl Fn(&str) -> Option<PathBuf>, var: &str) -> Option<PathBuf> {
    get(var).filter(|path| !path.as_os_str().is_empty())
}

/// 环境变量里的路径可能带引号和空格（`"C:\Program Files\Java"`），洗干净再用。
fn clean_env_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let trimmed = text.trim_matches(|c: char| c == '"' || c.is_whitespace());
    if trimmed.is_empty() {
        PathBuf::new()
    } else {
        PathBuf::from(trimmed)
    }
}

/// 把 `JavaInstall` 翻成可读文案，用在错误提示里。
pub fn describe_available(installs: &[JavaInstall]) -> String {
    if installs.is_empty() {
        return "本机没有找到任何 Java".to_string();
    }
    let list = installs
        .iter()
        .map(|java| format!("Java {}", java.major))
        .collect::<Vec<_>>()
        .join("、");
    format!("本机找到：{list}")
}

/// 组装「缺 Java」的错误提示 —— 文案在 Rust 侧写完，前端直接展示。
pub fn missing_java_error(requirement: &xcl_core::JavaRequirement, installs: &[JavaInstall]) -> AppError {
    AppError::unsupported(
        "找不到合适的 Java",
        format!(
            "这个版本{}，但{}。",
            requirement.describe(),
            describe_available(installs)
        ),
    )
    .with_hint("安装对应版本的 Java 后重新启动；也可以稍后在设置里手动指定 Java 路径")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modern_version() {
        let out = "openjdk version \"17.0.2\" 2022-01-18\nOpenJDK Runtime Environment Temurin-17.0.2+8 (build 17.0.2+8)";
        assert_eq!(parse_version(out), Some(("17.0.2".into(), 17)));
    }

    #[test]
    fn parses_java_eight_legacy_scheme() {
        // 1.8.0_202 是 Java 8 的老写法，主版本是 8 不是 1
        let out = "java version \"1.8.0_202\"\nJava(TM) SE Runtime Environment";
        assert_eq!(parse_version(out), Some(("1.8.0_202".into(), 8)));
    }

    #[test]
    fn parses_single_digit_version() {
        assert_eq!(parse_version("openjdk version \"21\""), Some(("21".into(), 21)));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_version("command not found"), None);
        assert_eq!(parse_version(""), None);
    }

    fn java(major: u32) -> JavaInstall {
        JavaInstall {
            path: PathBuf::from(format!("/jdk{major}/bin/java")),
            version: format!("{major}.0.1"),
            major,
        }
    }

    #[test]
    fn picks_lowest_satisfying_java() {
        let installs = vec![java(21), java(17), java(8)];
        // 要求 17+：挑 17，不是 21 —— 老 mod 在高版本上更容易出问题
        assert_eq!(pick(&installs, 17, None).unwrap().major, 17);
        assert_eq!(pick(&installs, 8, None).unwrap().major, 8);
        assert_eq!(pick(&installs, 22, None), None);
    }

    #[test]
    fn respects_upper_bound() {
        let installs = vec![java(21), java(8)];
        // 1.5.2 这类老版本要 Java 8，不能给 21
        assert_eq!(pick(&installs, 8, Some(8)).unwrap().major, 8);
    }

    #[test]
    fn available_list_is_readable() {
        assert!(describe_available(&[]).contains("没有找到"));
        assert!(describe_available(&[java(8), java(17)]).contains("Java 8"));
    }

    fn install(major: u32, version: &str, path: &str) -> JavaInstall {
        JavaInstall {
            path: PathBuf::from(path),
            version: version.to_string(),
            major,
        }
    }

    #[test]
    fn dedups_same_install_referenced_from_multiple_places() {
        // JAVA_HOME 和厂商目录扫描指向同一份 JDK，只留一份
        let out = dedup_by_path(vec![
            install(17, "17.0.2", "/fake/jdk17/bin/java"),
            install(17, "17.0.2", "/fake/jdk17/bin/java"),
        ]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn keeps_two_installs_of_same_version_at_different_paths() {
        // .minecraft\runtime 里的 Java 8 和 data/java 里的 Java 8 是两份，都得留 ——
        // 这就是之前按 (主版本, 版本串) 去重导致「检测不全」的 bug
        let out = dedup_by_path(vec![
            install(8, "1.8.0_51", "/fake/minecraft-runtime/bin/java"),
            install(8, "1.8.0_51", "/fake/data-java/bin/java"),
        ]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn cleans_quotes_and_spaces_from_env_paths() {
        assert_eq!(
            clean_env_path(Path::new("\"C:\\Program Files\\Java\"")),
            PathBuf::from("C:\\Program Files\\Java")
        );
        assert_eq!(
            clean_env_path(Path::new("  D:\\jdk17  ")),
            PathBuf::from("D:\\jdk17")
        );
        assert!(clean_env_path(Path::new("\"\"")).as_os_str().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn windows_roots_cover_every_launcher_runtime() {
        let roots = windows_install_roots(|var| {
            let value = match var {
                "APPDATA" => "C:\\Users\\u\\AppData\\Roaming",
                "LOCALAPPDATA" => "C:\\Users\\u\\AppData\\Local",
                "USERPROFILE" => "C:\\Users\\u",
                "ProgramFiles" => "C:\\Program Files",
                "ProgramFiles(x86)" => "C:\\Program Files (x86)",
                _ => return None,
            };
            Some(PathBuf::from(value))
        });
        let joined: Vec<String> = roots
            .iter()
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .collect();
        let has = |needle: &str| joined.iter().any(|r| r.contains(needle));
        // 官方 MC 启动器的运行时 —— 之前就是漏了它，机器上两个 Java 直接看不见
        assert!(has(".minecraft/runtime"), "roots = {joined:?}");
        assert!(has(".hmcl/java"), "roots = {joined:?}");
        assert!(has("PrismLauncher"), "roots = {joined:?}");
        assert!(has("ATLauncher"), "roots = {joined:?}");
        assert!(has("ModrinthApp"), "roots = {joined:?}");
        assert!(has(".ftba"), "roots = {joined:?}");
        assert!(has(".sdkman"), "roots = {joined:?}");
        assert!(has(".jdks"), "roots = {joined:?}");
        assert!(has("curseforge"), "roots = {joined:?}");
        assert!(has("Eclipse Adoptium"), "roots = {joined:?}");
        assert!(has("Amazon Corretto"), "roots = {joined:?}");
        assert!(has("Zulu"), "roots = {joined:?}");
        assert!(has("BellSoft"), "roots = {joined:?}");
        assert!(has("Semeru"), "roots = {joined:?}");
        assert!(has("Minecraft Launcher"), "roots = {joined:?}");
    }

    #[cfg(windows)]
    #[test]
    fn microsoft_roots_only_take_jdk_prefixed_children() {
        let roots = jdk_prefix_roots_with(|var| match var {
            "ProgramFiles" => Some(PathBuf::from("C:\\Program Files")),
            "ProgramFiles(x86)" => Some(PathBuf::from("C:\\Program Files (x86)")),
            "LOCALAPPDATA" => Some(PathBuf::from("C:\\Users\\u\\AppData\\Local")),
            _ => None,
        });
        // 两个 Program Files + LOCALAPPDATA\Programs
        assert_eq!(roots.len(), 3);
        assert!(roots.iter().all(|r| r.ends_with("Microsoft")));
    }

    #[test]
    fn walker_is_bounded_and_skips_special_paths() {
        let base = std::env::temp_dir().join("xcl-java-walk-test");
        let _ = std::fs::remove_dir_all(&base);
        let make = |sub: &str| {
            let bin = base.join(sub).join("bin");
            std::fs::create_dir_all(&bin).expect("创建测试目录");
            std::fs::write(bin.join(JAVA_BIN), b"").expect("创建测试文件");
        };
        make(""); // base/bin/java —— 根自己的 bin 也要收
        make("a/b/c/d"); // 深度 4：收
        make("a/b/c/d/e"); // 深度 5：不收
        make("javatmp"); // 特殊路径：不收

        let mut out = Vec::new();
        collect_java_under(&base, &mut out);
        let names: Vec<String> = out
            .iter()
            .map(|p| {
                p.strip_prefix(&base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert!(names.contains(&"bin/java.exe".to_string()), "{names:?}");
        assert!(
            names.contains(&"a/b/c/d/bin/java.exe".to_string()),
            "{names:?}"
        );
        assert!(
            !names.iter().any(|n| n.contains("e/bin/java.exe")),
            "{names:?}"
        );
        assert!(!names.iter().any(|n| n.contains("javatmp")), "{names:?}");

        let _ = std::fs::remove_dir_all(&base);
    }
}
