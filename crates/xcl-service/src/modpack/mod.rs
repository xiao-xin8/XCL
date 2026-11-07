//! 整合包安装。
//!
//! 一条独立链路，不复用 Mod 那套「装进某个本地版本」的流程：
//! 整合包装出来的是一个新实例（`versions/<包名>/`），装完直接能启动。
//!
//! 流程（六份实现里五个都是这个顺序）：
//!
//! ```text
//! 下包本体 → 解析清单 → 定实例名 → 建目录 → 解 overrides
//!   → 装原版 + 加载器（并把包里的文件并进同一份安装清单）
//!   → 一张任务卡跑完
//! ```
//!
//! 两个刻意的决定：
//! 1. overrides 在下之前解压。整合包声明的文件后落盘，
//!    冲突时以清单为准 —— 作者显式声明的优先级更高。
//! 2. 全程一张任务卡。前端 `taskPhase.ts` 禁止 `done → running`，
//!    所以不能「先开一张卡装游戏，完成后再开一张下文件」。

mod curseforge;
mod extract;
mod mrpack;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use xcl_core::{AppError, PathLayout};
use xcl_download::{FileLedger, Progress};
use xcl_loader::LoaderKind;
use xcl_source::UrlKind;

use crate::download::{new_task_id, ProgressSink, TaskPhase, TaskView};
use crate::install::{FileKind, PendingFile};
use crate::loader::{LoaderInstall, LoaderRef};
use crate::resources::ModSource;
use crate::AppState;

/// 一个还没落地的整合包文件。
#[derive(Debug, Clone)]
pub(crate) struct PackFile {
    /// 包内相对路径（CF 那边由我们按 modules 拼出来）。
    pub path: String,
    /// 候选地址（已过镜像策略）。
    pub urls: Vec<String>,
    pub sha1: Option<String>,
    /// 0 表示未知（mrpack 官方明确不保证这个值）。
    pub size: u64,
}

/// 拿不到的文件。不阻断安装，装完在结果里说明。
#[derive(Debug, Clone)]
pub(crate) struct MissingFile {
    pub name: String,
    pub reason: String,
    /// 整合包声明为必需。缺了它游戏大概率进不去，提示要更重。
    pub required: bool,
}

/// 两种格式解析后的统一产物。
#[derive(Debug)]
pub(crate) struct PackManifest {
    pub name: String,
    pub game: String,
    pub loader: Option<(LoaderKind, String)>,
    pub files: Vec<PackFile>,
    pub missing: Vec<MissingFile>,
    /// zip 内的目录前缀（`overrides/`、`client-overrides/`）。
    pub overrides: Vec<String>,
    pub icon: Option<Vec<u8>>,
}

/// 包本体在平台上的下载信息。
struct SourceFile {
    url: String,
    filename: String,
    size: u64,
}

impl AppState {
    /// 装一个整合包。立刻返回初始任务，进度和游戏安装共用同一张卡。
    pub async fn modpack_install(
        self: &Arc<Self>,
        source: &str,
        project_id: &str,
        version_id: &str,
        emit: ProgressSink,
    ) -> Result<TaskView, AppError> {
        let source = ModSource::parse(source).ok_or_else(|| {
            AppError::unsupported("未知的下载源", format!("不认识的源：{source}"))
        })?;

        // 同一份包正在装就直接复用那张卡
        let dedupe_key = format!("modpack:{project_id}:{version_id}");
        if let Some(running) = self.tasks().running_for(&dedupe_key) {
            return Ok(running);
        }

        // 拿到包本体的地址（这一步失败还不算开工，能如实报错）
        let pack = self.pack_source_file(source, project_id, version_id).await?;

        let task_id = new_task_id();
        let cancel = CancellationToken::new();
        let initial = TaskView {
            title: Some(pack.filename.clone()),
            message: Some("正在下载整合包".to_string()),
            ..TaskView::new(&task_id, &dedupe_key, 1, pack.size)
        };
        self.tasks()
            .insert(task_id.clone(), cancel.clone(), initial.clone());
        emit(initial.clone());

        // 后面全在后台：失败只能靠任务卡自己收尾
        let state = Arc::clone(self);
        let task_for_run = task_id.clone();
        let emit_for_run = Arc::clone(&emit);
        let project = project_id.to_string();
        tokio::spawn(async move {
            if let Err(error) = state
                .run_modpack_install(&task_for_run, source, &project, &pack, cancel, &emit_for_run)
                .await
            {
                crate::forgelike_install::fail_task(
                    &state,
                    &task_for_run,
                    &error,
                    &emit_for_run,
                );
            }
        });

        Ok(initial)
    }

    async fn run_modpack_install(
        self: &Arc<Self>,
        task_id: &str,
        source: ModSource,
        project_id: &str,
        pack: &SourceFile,
        cancel: CancellationToken,
        emit: &ProgressSink,
    ) -> Result<(), AppError> {
        let cache = self.layout.cache_root().join("modpack");
        fs::create_dir_all(&cache).map_err(|err| AppError::io(&err, "创建整合包缓存目录"))?;
        let archive_path = cache.join(safe_file_name(&pack.filename));

        // 1. 下包本体
        self.download_pack(pack, &archive_path, &cancel).await?;
        if cancel.is_cancelled() {
            return Err(AppError::canceled());
        }

        // 2. 解析
        let manifest = self.parse_pack(&archive_path, source).await?;

        // 3. 实例名 + 目录
        let id = unique_instance_id(&self.layout, &manifest.name);
        let dir = self.layout.version_dir(&id);
        fs::create_dir_all(&dir).map_err(|err| AppError::io(&err, "创建实例目录"))?;
        // 先记「这个实例属于哪个原版」，改名后仍能认出它
        let _ = crate::isolation::ensure_game_version(&self.layout, &id, &manifest.game);

        // 卡上的版本 id 换成真实例名
        if let Some(mut view) = self.tasks().get(task_id) {
            view.version_id = id.clone();
            self.tasks().update(view.clone());
            emit(view);
        }

        // 4. 解 overrides（失败就把刚建的目录删掉）
        if let Err(error) = self.unpack_overrides(&archive_path, &manifest, &dir) {
            let _ = fs::remove_dir_all(&dir);
            let _ = fs::remove_file(&archive_path);
            return Err(error);
        }
        // 包本体用完就删，装完不管成不成都不留
        let _ = fs::remove_file(&archive_path);

        // 5. 图标：包里带的直接用，没有就去平台拿项目的图标
        if let Some(icon) = self.pick_icon(&manifest, source, project_id).await {
            let icon_path = self.layout.version_settings_dir(&id).join("icon.png");
            if let Some(parent) = icon_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(&icon_path, icon);
        }

        // 6. 包里的文件 → 安装清单
        let extra = pack_files_to_pending(&manifest, &dir)?;

        // 7. 装原版 + 加载器 + 包文件，一张卡
        let installed = match &manifest.loader {
            Some((kind, version)) => {
                self.install_loader(
                    LoaderInstall {
                        game: manifest.game.clone(),
                        loader: LoaderRef {
                            kind: kind.as_str().to_string(),
                            version: version.clone(),
                        },
                        instance_id: Some(id.clone()),
                        extra_files: extra,
                        task_id: Some(task_id.to_string()),
                    },
                    emit.clone(),
                )
                .await
            }
            None => {
                let resolved = self.resolve_version_any(&manifest.game).await?;
                let mut plan = self.plan_from(&id, &resolved).await?;
                plan.files.extend(extra);
                self.start_install_plan_as(
                    plan,
                    Some(&manifest.game),
                    task_id.to_string(),
                    emit.clone(),
                )
                .await
            }
        };

        if let Err(error) = installed {
            let _ = fs::remove_dir_all(&dir);
            return Err(error);
        }

        // 7. 等真正下完再收尾（下载是 `spawn_task` 里另起的任务）
        if let Some(mut view) = self.wait_task(task_id).await {
            match view.phase {
                TaskPhase::Done => {
                    if !manifest.missing.is_empty() {
                        view.message = Some(missing_summary(&manifest.missing));
                        self.tasks().update(view.clone());
                        emit(view);
                    }
                }
                // 下载阶段失败 = 这个实例是废的（游戏文件缺一半）。
                // 失败就清掉目录，让重新装，而不是留个
                // 「能点进去但启动报错」的空壳。
                TaskPhase::Failed => {
                    let _ = fs::remove_dir_all(&dir);
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// 轮询到任务结束。整合包安装本来就要几分钟，500ms 一次没有成本。
    async fn wait_task(&self, task_id: &str) -> Option<TaskView> {
        for _ in 0..2400 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            match self.tasks().get(task_id) {
                Some(view) if view.phase != TaskPhase::Running => return Some(view),
                None => return None,
                _ => {}
            }
        }
        None
    }

    // ---------- 各步骤 ----------

    /// 实例图标：哪里有就用哪里的。
    ///
    /// 1. 包里带的 `icon.png`（根 / `overrides/` / `client-overrides/`）；
    /// 2. 没有就去资源平台拿项目图标。
    ///
    /// 第二档是必需的：Modrinth 的 mrpack 大多不塞图标，CF 的 zip 更是只有
    /// manifest。在线安装时可以直接把详情页那张图标的缓存路径传进安装器，
    /// 本地导入则干脆不给图标 —— 我们统一成「装的时候自己拉」。
    ///
    /// 格式按文件头认，不按扩展名：Modrinth 的项目图标是 `.webp`，
    /// 存下来照样叫 `icon.png`；读取端（`local::read_version_meta`）会按魔数
    /// 生成对应的 MIME，所以这里只要是个能认出来的图片就收。
    async fn pick_icon(
        &self,
        manifest: &PackManifest,
        source: ModSource,
        project_id: &str,
    ) -> Option<Vec<u8>> {
        const MAX_ICON_BYTES: usize = 512 * 1024;

        let usable = |bytes: &[u8]| {
            bytes.len() <= MAX_ICON_BYTES && crate::local::image_mime(bytes).is_some()
        };

        if let Some(icon) = &manifest.icon {
            if usable(icon) {
                return Some(icon.clone());
            }
        }

        let project = self
            .resource_project(source.as_str(), project_id, Some("modpack"))
            .await
            .ok()?;
        let url = project.icon_url?;
        // 图标地址也要过镜像；候选链里还有原地址兜底（镜像挂了能自己降级）
        let candidates = self.http_policy().candidates(UrlKind::Mod, &url);
        if candidates.is_empty() {
            return None;
        }
        let bytes = self.http().get_from_candidates(&candidates).await.ok()?;
        usable(&bytes).then_some(bytes)
    }

    /// 包本体的下载信息：Modrinth 查版本，CF 查文件。
    async fn pack_source_file(
        &self,
        source: ModSource,
        project_id: &str,
        file_id: &str,
    ) -> Result<SourceFile, AppError> {
        match source {
            ModSource::Modrinth => {
                let url = format!("https://api.modrinth.com/v2/version/{file_id}");
                let text = self.fetch_mod_text(&url, false).await?;
                let value: Value = serde_json::from_str(&text)
                    .map_err(|err| AppError::parse("整合包信息损坏", err.to_string()))?;
                let files = value
                    .get("files")
                    .and_then(Value::as_array)
                    .ok_or_else(|| AppError::parse("整合包信息损坏", "版本里没有文件列表"))?;
                let file = files
                    .iter()
                    .find(|file| file.get("primary").and_then(Value::as_bool) == Some(true))
                    .or_else(|| files.first())
                    .ok_or_else(|| AppError::parse("整合包信息损坏", "版本里没有文件"))?;
                Ok(SourceFile {
                    url: file
                        .get("url")
                        .and_then(Value::as_str)
                        .ok_or_else(|| AppError::parse("整合包信息损坏", "文件没有下载地址"))?
                        .to_string(),
                    filename: file
                        .get("filename")
                        .and_then(Value::as_str)
                        .unwrap_or("modpack.mrpack")
                        .to_string(),
                    size: file.get("size").and_then(Value::as_u64).unwrap_or(0),
                })
            }
            ModSource::CurseForge => {
                let url = format!(
                    "https://api.curseforge.com/v1/mods/{project_id}/files/{file_id}"
                );
                let text = self.fetch_mod_text(&url, true).await?;
                let value: Value = serde_json::from_str(&text)
                    .map_err(|err| AppError::parse("整合包信息损坏", err.to_string()))?;
                let data = value
                    .get("data")
                    .filter(|data| !data.is_null())
                    .ok_or_else(|| AppError::not_found("找不到这个整合包", "CurseForge 没有返回文件信息"))?;
                let raw_url = data
                    .get("downloadUrl")
                    .and_then(Value::as_str)
                    .filter(|url| !url.is_empty())
                    .ok_or_else(|| {
                        AppError::unsupported(
                            "这个整合包不能直接下载",
                            "作者关闭了第三方分发",
                        )
                        .with_hint("去 CurseForge 网页上下载后手动导入（暂未支持）")
                    })?;
                Ok(SourceFile {
                    url: self.mod_asset_url(raw_url),
                    filename: data
                        .get("fileName")
                        .and_then(Value::as_str)
                        .unwrap_or("modpack.zip")
                        .to_string(),
                    size: data.get("fileLength").and_then(Value::as_u64).unwrap_or(0),
                })
            }
        }
    }

    /// 下载包本体。候选按源策略展开，逐个试。
    async fn download_pack(
        &self,
        pack: &SourceFile,
        dest: &Path,
        cancel: &CancellationToken,
    ) -> Result<(), AppError> {
        let candidates = self.download_policy().candidates(UrlKind::Mod, &pack.url);
        if candidates.is_empty() {
            return Err(AppError::network(
                "没有可用的下载源",
                "整合包的地址被过滤掉了",
            ));
        }
        let progress = Progress::new(pack.size, 1);
        let ledger = FileLedger::new(Arc::clone(&progress));
        let mut last: Option<AppError> = None;
        for url in &candidates {
            match self
                .fetcher()
                .fetch_to(url, dest, pack.size, Arc::clone(&ledger), cancel.clone())
                .await
            {
                Ok(_) => {
                    ledger.settle(pack.size);
                    return Ok(());
                }
                Err(error) => {
                    if cancel.is_cancelled() {
                        return Err(AppError::canceled());
                    }
                    last = Some(error);
                }
            }
        }
        Err(last.unwrap_or_else(|| AppError::network("整合包下载失败", "所有候选地址都试过了")))
    }

    /// 打开包并解析。
    async fn parse_pack(&self, path: &Path, source: ModSource) -> Result<PackManifest, AppError> {
        let file = fs::File::open(path).map_err(|err| AppError::io(&err, "打开整合包"))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|err| AppError::parse("整合包文件损坏", err.to_string()))?;

        match source {
            ModSource::Modrinth => mrpack::parse(&mut archive),
            ModSource::CurseForge => {
                let raw = curseforge::parse(&mut archive)?;
                // 解析完就没用了，先把 zip 放掉再发请求
                drop(archive);
                let (files, missing) = self.resolve_pack_files(&raw).await?;
                Ok(raw.into_manifest(files, missing))
            }
        }
    }

    /// 把 overrides / client-overrides 解到实例根。
    fn unpack_overrides(
        &self,
        archive_path: &Path,
        manifest: &PackManifest,
        dir: &Path,
    ) -> Result<(), AppError> {
        if manifest.overrides.is_empty() {
            return Ok(());
        }
        let file = fs::File::open(archive_path).map_err(|err| AppError::io(&err, "打开整合包"))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|err| AppError::parse("整合包文件损坏", err.to_string()))?;
        manifest.unpack_overrides(&mut archive, dir)?;
        Ok(())
    }
}

/// 包内文件 → `PendingFile`。
fn pack_files_to_pending(
    manifest: &PackManifest,
    dir: &Path,
) -> Result<Vec<PendingFile>, AppError> {
    let mut out = Vec::with_capacity(manifest.files.len());
    for file in &manifest.files {
        // 再校验一次：CF 那侧是自己拼的路径，Modrinth 那侧解析时校验过了
        let relative = extract::safe_relative(&file.path)?;
        let Some(url) = file.urls.first() else {
            continue;
        };
        out.push(PendingFile {
            kind: FileKind::ModpackFile,
            local: dir.join(relative),
            url: url.clone(),
            sha1: file.sha1.clone(),
            size: file.size,
        });
    }
    Ok(out)
}

/// 实例名：规范化 + 重名递增。
fn unique_instance_id(layout: &PathLayout, raw: &str) -> String {
    let base = sanitize_instance_name(raw);
    let mut candidate = base.clone();
    let mut index = 2;
    while layout.version_dir(&candidate).exists() {
        candidate = format!("{base} ({index})");
        index += 1;
    }
    candidate
}

/// 包名 → 目录名。
///
/// Windows 不允许的字符一律转全角：直接删掉会让
/// 「我的整合包 1.0」变成「我的整合包 10」，看着像另一回事。
fn sanitize_instance_name(raw: &str) -> String {
    let trimmed = raw.trim();
    let stem = trimmed
        .strip_suffix(".mrpack")
        .or_else(|| trimmed.strip_suffix(".zip"))
        .unwrap_or(trimmed);

    let mut out = String::new();
    for ch in stem.chars() {
        match ch {
            '/' => out.push('／'),
            '\\' => out.push('＼'),
            ':' => out.push('：'),
            '*' => out.push('＊'),
            '?' => out.push('？'),
            '"' => out.push('＂'),
            '<' => out.push('＜'),
            '>' => out.push('＞'),
            '|' => out.push('｜'),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }

    let out: String = out.trim().chars().take(64).collect();
    if out.is_empty() || out == "." || out == ".." {
        "整合包".to_string()
    } else {
        out
    }
}

/// 缓存文件名：只留一个干净的 basename。
fn safe_file_name(raw: &str) -> String {
    let name = Path::new(raw)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "modpack.zip".to_string());
    let name = sanitize_instance_name(&name);
    if name.contains('.') {
        name
    } else {
        format!("{name}.zip")
    }
}

fn missing_summary(missing: &[MissingFile]) -> String {
    if missing.is_empty() {
        return "安装完成".to_string();
    }
    let required = missing.iter().filter(|file| file.required).count();
    let first = missing
        .iter()
        .take(3)
        .map(|file| format!("{}（{}）", file.name, file.reason))
        .collect::<Vec<_>>()
        .join("、");
    let head = if required > 0 {
        format!("安装完成，但缺了 {} 个必需文件，游戏可能进不去", required)
    } else {
        format!("安装完成，有 {} 个可选文件没下到", missing.len())
    };
    format!(
        "{head}：{first}{}",
        if missing.len() > 3 { " 等" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::{sanitize_instance_name, unique_instance_id};
    use xcl_core::PathLayout;

    #[test]
    fn replaces_characters_windows_rejects() {
        assert_eq!(sanitize_instance_name("a/b:c*d?"), "a／b：c＊d？");
        assert_eq!(sanitize_instance_name("  我的整合包  "), "我的整合包");
    }

    #[test]
    fn falls_back_when_the_name_is_empty() {
        assert_eq!(sanitize_instance_name(""), "整合包");
        assert_eq!(sanitize_instance_name(".."), "整合包");
    }

    #[test]
    fn strips_a_pack_extension() {
        assert_eq!(sanitize_instance_name("Cool Pack.mrpack"), "Cool Pack");
        assert_eq!(sanitize_instance_name("Cool Pack.zip"), "Cool Pack");
    }

    #[test]
    fn adds_a_suffix_when_the_name_is_taken() {
        let dir = std::env::temp_dir().join(format!("xcl-mp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let layout = PathLayout::new(dir.clone(), dir.join("cache"));
        std::fs::create_dir_all(layout.version_dir("Pack")).unwrap();
        assert_eq!(unique_instance_id(&layout, "Pack"), "Pack (2)");
        std::fs::create_dir_all(layout.version_dir("Pack (2)")).unwrap();
        assert_eq!(unique_instance_id(&layout, "Pack"), "Pack (3)");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
