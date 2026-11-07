//! Java 运行时管理：清单、安装、删除、按需推荐。
//!
//! 数据源是 Mojang 官方的 java-runtime（见 `xcl_core::java_runtime` 的说明），
//! 下载复用现有的并发引擎——它跟装游戏文件是同一件事：一堆 URL + sha1 + 落地路径。
//!
//! 安装在 `data/java/<组件名>/`，跟 `versions/`、`libraries/` 平级：
//! 整个 `data/` 拷到别的机器就能直接玩，这是便携分发的前提。

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tokio_util::sync::CancellationToken;
use xcl_core::java_runtime::{
    choose_component, component_java_major, component_label, RuntimeIndex, RuntimeManifest,
};
use xcl_core::{java_requirement, AppError};
use xcl_download::{DownloadItem, DownloadOptions, Progress};
use xcl_source::UrlKind;

use crate::download::{new_task_id, run_task, ProgressSink, TaskPhase, TaskView};
use crate::AppState;

/// Mojang 的 Java 运行时索引。约 13KB，覆盖 8/16/17/21/25。
const INDEX_URL: &str =
    "https://piston-meta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";

const INDEX_CACHE_KEY: &str = "java:runtime:index";

/// 任务表里 Java 任务的前缀。
///
/// 复用同一张任务表的好处是：全局进度、取消、`install_tasks` 恢复都不用写第二套。
/// 加前缀是为了跟游戏版本 id 区分开（版本 id 不会有冒号）。
pub const TASK_PREFIX: &str = "java:";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaComponent {
    /// 组件名，如 `jre-legacy`
    pub name: String,
    /// 展示名，如 `Java 8（jre-legacy）`
    pub label: String,
    /// 它提供的 Java 主版本（0 表示未知）
    pub major: u32,
    /// 官方发布的版本号，如 `8u51-cacert462b08`
    pub version: String,
    pub installed: bool,
    /// 已安装的可执行文件路径
    pub path: Option<String>,
    /// 已占用的磁盘空间（字节）
    pub size_on_disk: Option<u64>,
    /// 本机已经有一个同主版本的 Java（不是我们下的），那就没必要再下 143MB。
    ///
    /// 探测走的是跟启动选 Java 同一套逻辑（`xcl_launch::detect_all_with`），
    /// 所以这里说有，启动时就能真的用上 —— 不会出现「页面说已有、启动却找不到」。
    pub system_java: Option<SystemJava>,
}

/// 本机探测到的一个 Java。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemJava {
    pub path: String,
    pub version: String,
    pub major: u32,
    /// 是不是在 `data/java/` 下（由我们下载管理的）
    pub managed: bool,
}

impl AppState {
    /// 扫描本机所有 Java：JDK_HOME/JAVA_HOME / PATH / 各启动器的运行时目录 /
    /// 各发行版的安装目录 / `data/java/`。
    ///
    /// 候选清单见 `xcl_launch::java` 的文档，每个候选都会真的跑一次
    /// `java -version` 读主版本，所以结果是可信的，不是靠目录名猜。
    pub async fn java_detected(&self) -> Vec<SystemJava> {
        let roots = self.java_runtime_roots();
        let managed_root = self.layout.java_dir();
        xcl_launch::detect_all_with(&roots)
            .await
            .into_iter()
            .map(|java| SystemJava {
                managed: java.path.starts_with(&managed_root),
                path: java.path.display().to_string(),
                version: java.version,
                major: java.major,
            })
            .collect()
    }

    /// 当前平台可用的 Java 运行时列表（含是否已装、本机是否已有同版本）。
    pub async fn java_components(&self) -> Result<Vec<JavaComponent>, AppError> {
        let index = self.java_runtime_index().await?;
        // 扫一次就够了，下面每个组件都在这个结果里找
        let detected = self.java_detected().await;

        let mut out: Vec<JavaComponent> = index
            .components()
            .into_iter()
            .map(|(name, entry)| {
                let exe = self.layout.java_executable(&name);
                let installed = exe.is_file();
                let major = component_java_major(&name).unwrap_or(0);
                // 只有「我们自己没装」时，本机已装的才有意义；
                // 优先挑非 managed 的（自己装的那份更有可能是想要的）
                let system_java = if installed || major == 0 {
                    None
                } else {
                    detected
                        .iter()
                        .find(|java| !java.managed && java.major == major)
                        .or_else(|| detected.iter().find(|java| java.major == major))
                        .cloned()
                };
                JavaComponent {
                    major,
                    label: component_label(&name),
                    size_on_disk: if installed {
                        Some(dir_size(&self.layout.java_component_dir(&name)))
                    } else {
                        None
                    },
                    path: installed.then(|| exe.display().to_string()),
                    version: entry.version.name,
                    installed,
                    name,
                    system_java,
                }
            })
            .collect();

        // 按 Java 版本升序，看着顺
        out.sort_by_key(|component| component.major);
        Ok(out)
    }

    /// 装在 `data/java/` 下的运行时目录，供 Java 探测时一并扫描。
    pub fn java_runtime_roots(&self) -> Vec<PathBuf> {
        vec![self.layout.java_dir()]
    }

    /// 下载并安装一个 Java 运行时。立刻返回初始任务，进度走 `emit`。
    pub async fn java_install(
        self: &Arc<Self>,
        component: &str,
        emit: ProgressSink,
    ) -> Result<TaskView, AppError> {
        validate_component(component)?;

        let task_key = format!("{TASK_PREFIX}{component}");
        if let Some(running) = self.tasks().running_for(&task_key) {
            return Ok(running);
        }

        let index = self.java_runtime_index().await?;
        let entry = index.find(component).ok_or_else(|| {
            AppError::not_found(
                "找不到这个 Java 运行时",
                format!("Mojang 的清单里没有 {component}（当前平台：{}）", xcl_core::java_runtime::platform_key()),
            )
            .with_hint("点刷新重新获取清单；持续失败可能是官方调整了清单结构")
        })?;

        let manifest = self
            .java_component_manifest(component, &entry.manifest.url)
            .await?;
        let files = manifest.download_entries();
        if files.is_empty() {
            return Err(AppError::parse(
                "Java 运行时清单是空的",
                format!("{component} 的文件清单里没有可下载内容"),
            ));
        }

        let policy = self.download_policy();
        let base = self.layout.java_component_dir(component);
        let mut items: Vec<DownloadItem> = Vec::with_capacity(files.len());
        for file in &files {
            let mut urls = policy.candidates(UrlKind::Meta, &file.url);
            if urls.is_empty() {
                continue;
            }
            // 官方地址排最前。Java 运行时这几个包动辄几十 MB（`rt.jar` 63MB），
            // 全在 `piston-data` 上且支持 Range；而 BMCLAPI 对 `/v1/objects` 的大文件
            // 会限流（实测 192 个文件 64 并发跑出 0.2MB/s，143MB 花了 25 分钟）。
            // `sort_by_key` 是稳定排序，所以同类之间的原有顺序不变。
            urls.sort_by_key(|url| !url.starts_with("https://piston-"));
            let split_urls = crate::download::split_urls_for(&urls);
            items.push(DownloadItem {
                name: file.path.clone(),
                urls,
                split_urls,
                dest: base.join(&file.path),
                sha1: Some(file.sha1.clone()),
                size: file.size,
            });
        }

        if items.is_empty() {
            return Err(AppError::unsupported(
                "没有可下载的文件",
                format!("{component} 展开后是空的"),
            ));
        }

        let total_files = items.len();
        let total_bytes: u64 = items.iter().map(|item| item.size).sum();
        let task_id = new_task_id();
        let cancel = CancellationToken::new();
        let progress = Progress::new(total_bytes, total_files);

        let initial = TaskView::new(&task_id, &task_key, total_files, total_bytes);
        self.tasks()
            .insert(task_id.clone(), cancel.clone(), initial.clone());
        emit(initial.clone());

        let state = Arc::clone(self);
        // 跟装游戏走同一套并发策略：官方 64、镜像 16。
        // 是否分片由每个文件的 `split_urls` 决定，这里只给上限。
        let options = DownloadOptions {
            concurrency: state.effective_concurrency(),
            attempts_per_url: 2,
            split: state.split_config(),
        };
        let fetcher = Arc::clone(state.fetcher());

        tokio::spawn(async move {
            run_task(
                state, task_id, items, fetcher, progress, options, cancel, emit,
            )
            .await;
        });

        Ok(initial)
    }

    /// 下载并等它装完 —— 启动链路缺 Java 时用，装完立刻接着启动。
    ///
    /// 为什么不复用 `run_task` 的返回值：任务可能本来就在跑（在 Java 页面
    /// 手动点过下载），这种情况下启动应该搭上那趟车，而不是再起一个。
    /// 任务表是唯一真相，所以这里就盯着它的状态。
    pub async fn java_install_and_wait(
        self: &Arc<Self>,
        component: &str,
        emit: ProgressSink,
    ) -> Result<(), AppError> {
        let initial = self.java_install(component, emit).await?;

        loop {
            let Some(view) = self.tasks().get(&initial.task_id) else {
                return Err(AppError::unsupported(
                    "Java 下载任务丢失",
                    "任务表里找不到这个下载任务了".to_string(),
                )
                .with_hint("再点一次启动会重新下载"));
            };
            match view.phase {
                TaskPhase::Done => return Ok(()),
                TaskPhase::Failed => {
                    return Err(AppError::unsupported(
                        "Java 运行时下载失败",
                        view.message.unwrap_or_else(|| "有文件没下下来".to_string()),
                    )
                    .with_hint("再点一次启动会重试；也可以到「下载 → Java 运行时」里手动装"));
                }
                TaskPhase::Cancelled => {
                    return Err(AppError::unsupported(
                        "Java 下载已取消",
                        "缺少 Java 运行时，游戏没有启动".to_string(),
                    )
                    .with_hint("再点一次启动会重新下载"));
                }
                TaskPhase::Running => {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
        }
    }

    /// 删掉一个已安装的运行时（目录整个删）。
    pub fn java_remove(&self, component: &str) -> Result<(), AppError> {
        validate_component(component)?;
        let dir = self.layout.java_component_dir(component);
        if !dir.exists() {
            return Ok(());
        }
        std::fs::remove_dir_all(&dir)
            .map_err(|e| AppError::io(&e, "删除 Java 运行时").with_hint(
                "如果游戏正在运行，先把它关掉再删",
            ))
    }

    /// 某个版本缺 Java 时，推荐一个可下载的组件。
    ///
    /// 返回 `None` 有三种情况：本机已经有合适的 Java、这个版本的 Java 需求
    /// 没有对应的官方运行时（如 Java 11）、或者版本信息读不出来。
    pub async fn java_suggest(&self, version_id: &str) -> Option<JavaComponent> {
        let detail = crate::local::load(&self.layout, version_id).ok()?;
        let requirement = java_requirement(
            version_id,
            detail.java_version.as_ref().map(|v| v.major_version),
        );

        // 已经有能用的就别推荐了
        let available =
            xcl_launch::detect_all_with(&self.java_runtime_roots()).await;
        if xcl_launch::pick(&available, requirement.min, requirement.max).is_some() {
            return None;
        }

        let (component, major) = choose_component(&requirement)?;
        let version = self
            .java_runtime_index()
            .await
            .ok()
            .and_then(|index| index.find(component).map(|e| e.version.name.clone()))
            .unwrap_or_default();

        Some(JavaComponent {
            name: component.to_string(),
            label: component_label(component),
            major,
            version,
            installed: false,
            path: None,
            size_on_disk: None,
            // 走到这里说明启动时已经找不到合适的 Java 了，不存在「本机已有」
            system_java: None,
        })
    }

    // ---- 内部 ----

    async fn java_runtime_index(&self) -> Result<RuntimeIndex, AppError> {
        let (index, _) = self
            .cache()
            .get_or_fetch(INDEX_CACHE_KEY, false, || async {
                let (text, _) = self.fetch_meta_text(INDEX_URL).await?;
                RuntimeIndex::parse(&text)
            })
            .await?;
        Ok(index)
    }

    async fn java_component_manifest(
        &self,
        component: &str,
        url: &str,
    ) -> Result<RuntimeManifest, AppError> {
        let key = format!("java:manifest:{component}");
        let url = url.to_string();
        let (manifest, _) = self
            .cache()
            .get_or_fetch(&key, false, || async {
                let (text, _) = self.fetch_meta_text(&url).await?;
                RuntimeManifest::parse(&text)
            })
            .await?;
        Ok(manifest)
    }
}

/// 组件名会拼进路径，必须挡住 `..` 之类的穿越。
fn validate_component(component: &str) -> Result<(), AppError> {
    let ok = !component.is_empty()
        && component.len() <= 64
        && !component.contains("..")
        && component
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(AppError::unsupported(
            "非法的组件名",
            format!("{component:?} 不是合法的 Java 运行时组件名"),
        ))
    }
}

/// 目录占用大小。算不出来就返回 0（只是展示用，不值得报错）。
fn dir_size(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += dir_size(&path);
        } else if let Ok(meta) = entry.metadata() {
            total += meta.len();
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal_in_component_name() {
        assert!(validate_component("jre-legacy").is_ok());
        assert!(validate_component("java-runtime-gamma").is_ok());
        assert!(validate_component("../etc").is_err());
        assert!(validate_component("a/b").is_err());
        assert!(validate_component("a\\b").is_err());
        assert!(validate_component("").is_err());
        assert!(validate_component("jre legacy").is_err());
    }

    #[test]
    fn task_prefix_is_reserved() {
        // 版本 id 里不会有冒号，所以不会和游戏任务撞
        assert!(TASK_PREFIX.contains(':'));
    }
}
