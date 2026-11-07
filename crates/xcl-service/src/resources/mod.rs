//! 资源平台（Mod 下载）：搜索、项目、版本。
//!
//! 两个源是互斥的：界面上用胶囊切换 `Modrinth` / `CurseForge`，一次只查一个。
//! 所以这里没有"合并分页""跨源排序"那套东西 —— 每个源各管各的 offset/total。
//!
//! 网络层走 `UrlKind::Mod` 的独立域名表（`xcl-source::MOD_HOST_MAP`）：
//! `api.modrinth.com → mod.mcimirror.top/modrinth`、`api.curseforge.com →
//! mod.mcimirror.top/curseforge`。CurseForge 的 API Key 由镜像代持，
//! 官方地址无 key 必然 403，所以 CF 只留镜像候选。

pub mod curseforge;
pub mod deps;
pub mod modrinth;
pub mod targets;

mod types;

pub use types::*;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use xcl_core::AppError;
use xcl_source::UrlKind;

use crate::download::{ProgressSink, TaskView};
use crate::{isolation, local, AppState};

/// 镜像的独占窗口：正常 200ms 内就回来了，官方根本不会被惊动。
const MOD_HEDGE: Duration = Duration::from_millis(700);
/// 元数据整体超时：Mod 搜索是小请求，卡 20 秒已经很不正常。
const MOD_TIMEOUT: Duration = Duration::from_secs(20);

/// 单个文件的完整信息 + 它声明的依赖。
#[derive(Debug, Clone)]
pub(crate) struct FileFacts {
    pub file: ResourceFileView,
    pub dependencies: Vec<deps::FileDependency>,
}

impl AppState {
    /// 搜索社区资源。两个源各走各的实现，返回统一视图。
    ///
    /// `kind` 是资源类型（Mod / 资源包），只影响平台侧的枚举值与项目页路径，
    /// 分页、排序、视图结构全部共用。
    #[allow(clippy::too_many_arguments)]
    pub async fn mod_search(
        &self,
        source: &str,
        query: &str,
        offset: usize,
        sort: &str,
        game_version: Option<&str>,
        loader: Option<&str>,
        kind: Option<&str>,
    ) -> Result<ModSearchView, AppError> {
        let source = ModSource::parse(source).ok_or_else(|| {
            AppError::unsupported("未知的下载源", format!("不认识的 Mod 源：{source}"))
        })?;
        let kind = ResourceKind::parse(kind).ok_or_else(|| {
            AppError::unsupported("未知的资源类型", format!("不认识的类型：{}", kind.unwrap_or("")))
        })?;
        let sort = ModSort::parse(sort);
        let query = query.trim();

        match source {
            ModSource::Modrinth => {
                self.search_modrinth(query, offset, sort, game_version, loader, kind)
                    .await
            }
            ModSource::CurseForge => {
                self.search_curseforge(query, offset, sort, game_version, loader, kind)
                    .await
            }
        }
    }

    /// 图标这类静态资源：`<img>` 没法自己套源策略，所以在后端就改写到镜像。
    ///
    /// 返回候选里的第一个（配了镜像就是镜像地址，纯官方模式就是原地址）。
    pub(crate) fn mod_asset_url(&self, url: &str) -> String {
        self.http_policy()
            .candidates(UrlKind::Mod, url)
            .into_iter()
            .next()
            .unwrap_or_else(|| url.to_string())
    }

    /// 版本筛选的选项：所有正式版的大版本线，新的在前。
    ///
    /// 从版本清单算，不写死 —— MC 的命名换过（`1.20.1` → `26.3`），
    /// 前端写死一份迟早过期。
    pub async fn mod_version_lines(&self) -> Result<Vec<String>, AppError> {
        let view = self.version_list(false).await?;
        let mut out: Vec<String> = Vec::new();
        for group in view.releases {
            for item in group.items {
                let line = version_line(&item.id);
                if !out.contains(&line) {
                    out.push(line);
                }
            }
        }
        Ok(out)
    }

    /// 把「大版本线」（`1.20`）展开成这条线上的所有正式版（`1.20`、`1.20.1` …）。
    ///
    /// 下载页的版本筛选只给粗粒度选项，但两个平台都是按精确版本号匹配的
    /// （`versions:1.20` 一个都搜不到），所以展开放在后端。版本清单走的还是
    /// 那份缓存，拿不到（离线 / 源全挂）就只退回原值，不影响搜索本身。
    pub(super) async fn expand_game_versions(&self, major: &str) -> Vec<String> {
        // 只有「1.20」这种一段小数的才算大版本线；快照 / 远古版本原样传
        if major.matches('.').count() != 1 {
            return vec![major.to_string()];
        }
        let Ok(view) = self.version_list(false).await else {
            return vec![major.to_string()];
        };

        let prefix = format!("{major}.");
        let mut out: Vec<String> = Vec::new();
        for group in view.releases {
            for item in group.items {
                if item.id == major || item.id.starts_with(&prefix) {
                    out.push(item.id);
                }
            }
        }
        if out.is_empty() {
            vec![major.to_string()]
        } else {
            out
        }
    }

    /// 拉一个资源平台的 JSON 文本。
    ///
    /// `mirror_only`：CurseForge 官方地址必须带 key（否则 403），所以只留镜像候选；
    /// Modrinth 官方能直连，两个候选就错峰竞速。
    pub(crate) async fn fetch_mod_text(
        &self,
        url: &str,
        mirror_only: bool,
    ) -> Result<String, AppError> {
        let candidates = self.mod_candidates(url, mirror_only)?;

        let bytes = if candidates.len() >= 2 {
            // 主源先跑，另一个晚 MOD_HEDGE 加入，谁先回用谁
            let outcome = self
                .http()
                .race_text(
                    &candidates[0],
                    &candidates[1],
                    MOD_HEDGE,
                    MOD_TIMEOUT,
                )
                .await?;
            outcome.body
        } else {
            self.http().get_from_candidates(&candidates).await?
        };

        String::from_utf8(bytes).map_err(|e| {
            AppError::parse("资源平台返回的不是有效文本", e.to_string())
                .with_hint("可能是镜像返回了错误页面，稍后重试")
        })
    }

    /// 资源平台的候选地址。
    ///
    /// `mirror_only` 时把官方地址剔掉：CF 官方必然 403，发出去只会把
    /// 「没配镜像」伪装成「网络问题」。一个镜像都没有就明确报错，不退回官方。
    fn mod_candidates(&self, url: &str, mirror_only: bool) -> Result<Vec<String>, AppError> {
        let all = self.http_policy().candidates(UrlKind::Mod, url);
        let candidates: Vec<String> = if mirror_only {
            all.into_iter()
                .filter(|candidate| {
                    !candidate.contains("api.curseforge.com")
                        && !candidate.contains("forgecdn.net")
                })
                .collect()
        } else {
            all
        };

        if candidates.is_empty() {
            return Err(if mirror_only {
                AppError::network("CurseForge 暂时不可用", "当前没有可用的 CurseForge 镜像")
                    .with_hint("检查网络后重试，或改用 Modrinth")
            } else {
                AppError::network("没有可用的下载源", "所有候选地址都被过滤掉了")
            });
        }
        Ok(candidates)
    }

    /// POST 一个资源平台接口（只用于 CF 的「批量查文件」）。
    ///
    /// 镜像不一定实现 POST：`classify_status` 会把 405/501 当致命错误，
    /// 直接换下一个候选，不会在同一个地址上重试。
    pub(crate) async fn post_mod_json(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<String, AppError> {
        let candidates = self.mod_candidates(url, true)?;
        let bytes = self
            .http()
            .post_json_from_candidates(&candidates, body.to_string().into_bytes())
            .await?;
        String::from_utf8(bytes).map_err(|e| {
            AppError::parse("资源平台返回的不是有效文本", e.to_string())
                .with_hint("可能是镜像返回了错误页面，稍后重试")
        })
    }
}

// ---------- 详情页 / 安装 ----------

/// 解析源名，报错文案统一。
fn parse_source(source: &str) -> Result<ModSource, AppError> {
    ModSource::parse(source).ok_or_else(|| {
        AppError::unsupported("未知的下载源", format!("不认识的 Mod 源：{source}"))
    })
}

/// 解析资源类型，报错文案统一。不传按 Mod 处理（老调用方的行为不变）。
fn parse_kind(kind: Option<&str>) -> Result<ResourceKind, AppError> {
    ResourceKind::parse(kind).ok_or_else(|| {
        AppError::unsupported(
            "未知的资源类型",
            format!("不认识的类型：{}", kind.unwrap_or("")),
        )
    })
}

/// 「类型级前置」那一条依赖（光影 → Iris），构造好可直接塞进依赖列表。
fn prerequisite_dependency(kind: ResourceKind) -> Option<deps::FileDependency> {
    kind.prerequisite_project()
        .map(|project_id| deps::FileDependency {
            project_id: project_id.to_string(),
            // 不钉版本：让 `resolve_dependencies` 按本地版本自己的加载器挑最新
            version_id: None,
            relation: deps::DepRelation::Required,
        })
}

/// 把「类型级前置」补进文件的依赖里。
///
/// 为什么要造这条数据：依赖区、`dependencyCount`、「前往安装」整套东西都是按
/// 「文件声明的依赖」设计的，而上游偏偏不给光影声明 Iris。与其在前端另画一张
/// 假的卡，不如在返回给前端之前补进去 —— 后面的解析、已安装判定、跳转全都白捡，
/// 前端一行都不用改。
///
/// 只在 Modrinth 源注入：`IRIS_PROJECT_ID` 是 Modrinth 的项目 id，
/// 拿它去 CurseForge 查不到东西，硬塞只会多一条"项目不存在"的错误。
fn attach_prerequisites(kind: ResourceKind, source: ModSource, files: &mut [ResourceFileView]) {
    let Some(prereq) = prerequisite_dependency(kind) else {
        return;
    };
    if source != ModSource::Modrinth {
        return;
    }
    for file in files {
        if !file
            .dependencies
            .iter()
            .any(|dep| dep.project_id == prereq.project_id)
        {
            file.dependencies.push(prereq.clone());
        }
        // 首屏靠它决定出不出依赖骨架，补完要跟着刷新
        file.dependency_count = file.dependencies.len();
    }
}

impl AppState {
    /// 项目详情（左侧详情卡用）。两个源的字段差异见 [`ResourceProjectView`]。
    pub async fn resource_project(
        &self,
        source: &str,
        id: &str,
        kind: Option<&str>,
    ) -> Result<ResourceProjectView, AppError> {
        let kind = parse_kind(kind)?;
        match parse_source(source)? {
            ModSource::Modrinth => self.modrinth_project_detail(id, kind).await,
            ModSource::CurseForge => self.curseforge_project_detail(id).await,
        }
    }

    /// 详情页自动模式的左侧列表：哪些本地版本能装这个 Mod。
    ///
    /// 一行 = 一个本地版本。只保留「这个文件支持的 MC 版本 + 加载器」都对得上的，
    /// 一行都没有就返回空 —— 界面据此显示空态并引导去手动安装。
    ///
    /// 每个候选要单独查一次文件列表（`game_versions` + `loaders` 都是服务端筛选），
    /// 好处是不用把几百个版本全拉回来自己比对；本地版本通常只有几个，代价可以接受。
    pub async fn resource_targets(
        &self,
        source: &str,
        id: &str,
        kind: Option<&str>,
    ) -> Result<Vec<LocalTargetView>, AppError> {
        let source = parse_source(source)?;
        let kind = parse_kind(kind)?;
        let candidates = targets::candidates(kind, &local::scan(&self.layout));

        // 资源包 / 光影都不按 MC 版本筛文件（见 `targets` 模块头），所以只要项目
        // 有文件，每个本地版本都算候选 —— 一次全量请求就够，不用按版本各查一遍。
        if kind != ResourceKind::Mod {
            let files = self.fetch_files(source, id, None, None, None).await?;
            if files.is_empty() {
                return Ok(Vec::new());
            }
            let names: Vec<String> = files
                .iter()
                .map(|file| safe_filename(&file.filename))
                .collect();
            let mut out: Vec<LocalTargetView> = Vec::with_capacity(candidates.len());
            for candidate in candidates {
                let dir = self.resource_dir(&candidate.version_id, kind)?;
                let installed = names.iter().any(|name| dir.join(name).exists());
                out.push(LocalTargetView {
                    version_id: candidate.version_id,
                    game_version: candidate.game_version,
                    // 展示用：本地版本自身的类型（原版 / Fabric / Forge）
                    loader: candidate.display_loader.to_string(),
                    last_played_ms: candidate.last_played_ms,
                    installed,
                });
            }
            return Ok(out);
        }

        let mut out: Vec<LocalTargetView> = Vec::new();
        for candidate in candidates {
            let files = self
                .fetch_files(
                    source,
                    id,
                    Some(candidate.game_version.as_str()),
                    candidate.file_loader(),
                    None,
                )
                .await?;
            if files.is_empty() {
                continue;
            }
            // 已经装过：目标目录（`mods/` 或 `resourcepacks/`）里存在这个项目的任意一个文件。
            // 文件名要过一遍 `safe_filename` —— 落盘时是它写的名字，这里用原名会对不上。
            let dir = self.resource_dir(&candidate.version_id, kind)?;
            let installed = files
                .iter()
                .any(|file| dir.join(safe_filename(&file.filename)).exists());

            out.push(LocalTargetView {
                version_id: candidate.version_id,
                game_version: candidate.game_version,
                // 展示用：资源包这里放的是本地版本自身的类型（原版 / Fabric / Forge）
                loader: candidate.display_loader.to_string(),
                last_played_ms: candidate.last_played_ms,
                installed,
            });
        }
        Ok(out)
    }

    /// 侧栏要的文件列表。
    ///
    /// - `manual = false`：给 `version_id`，返回那个本地版本能用的文件，新到旧；
    /// - `manual = true`：忽略 `version_id`，返回全量（按 [`MANUAL_LIMIT`] 截断）。
    pub async fn resource_files(
        &self,
        source: &str,
        id: &str,
        version_id: Option<&str>,
        manual: bool,
        kind: Option<&str>,
    ) -> Result<ResourceFilesView, AppError> {
        let source = parse_source(source)?;
        let kind = parse_kind(kind)?;

        if manual {
            let mut files = self.fetch_files(source, id, None, None, Some(MANUAL_LIMIT)).await?;
            // 光影的 Iris 前置也要挂在手动模式的文件上，否则换了文件依赖区就空了
            attach_prerequisites(kind, source, &mut files);
            let truncated = files.len() >= MANUAL_LIMIT;
            return Ok(ResourceFilesView {
                items: files,
                // 手动模式不替选：文件是自己点的
                default_file_id: None,
                // 没有默认文件，依赖等选完文件 + 落点再查
                dependencies: Vec::new(),
                truncated,
            });
        }

        let version_id = version_id.ok_or_else(|| {
            AppError::validation(
                "还没选目标版本",
                "自动模式要先在左边点一个本地版本，才谈得上装哪个文件",
            )
        })?;
        let candidate = targets::candidates(kind, &local::scan(&self.layout))
            .into_iter()
            .find(|item| item.version_id == version_id)
            .ok_or_else(|| {
                AppError::not_found(
                    format!("这个版本装不了{}", kind.label()),
                    format!("{version_id} 不是能装{}的本地版本，或者缺 MC 版本号", kind.label()),
                )
            })?;

        // 资源包 / 光影都不按 MC 版本筛文件：它们的 `game_versions` 只标作者测过
        // 的版本，服务端一过滤就把大部分筛没了。拿全量，默认项再优先挑对得上的那个。
        let mut files = if kind != ResourceKind::Mod {
            self.fetch_files(source, id, None, None, None).await?
        } else {
            self.fetch_files(
                source,
                id,
                Some(candidate.game_version.as_str()),
                candidate.file_loader(),
                None,
            )
            .await?
        };
        // 光影必须靠 Iris 才能加载，而文件本身不声明 —— 在这里补进去
        attach_prerequisites(kind, source, &mut files);
        let default_file_id = if kind != ResourceKind::Mod {
            targets::default_pack_file_id(&files, &candidate.game_version)
        } else {
            targets::default_file_id(&files)
        };
        // 依赖跟着文件列表一起回来：还在读列表时依赖就已经在路上了，
        // 不会再出现"人家都开始下了，依赖才慢吞吞甩过来"。
        //
        // 资源包也有依赖（Fresh Animations 要 EMF，没有它包是死的），
        // 所以这里不再按类型短路。依赖本身永远是 Mod，走 `dep_loader()`。
        // 光影的 Iris 前置已经由 `attach_prerequisites` 补在文件上了。
        let dependencies = match default_file_id.as_deref() {
            Some(file_id) => match files.iter().find(|file| file.file_id == file_id) {
                Some(file) => {
                    self.resolve_dependencies(source, id, Some(&candidate), &file.dependencies)
                        .await?
                }
                None => Vec::new(),
            },
            None => Vec::new(),
        };

        Ok(ResourceFilesView {
            items: files,
            default_file_id,
            dependencies,
            truncated: false,
        })
    }

    /// 手动模式左侧：全量文件，按 MC 版本折叠分组。
    ///
    /// 平铺几千条没法看（按项目版本平铺，但我们这里文件更多），
    /// 所以按"文件声明的 MC 版本"分组，一个文件属于几个版本就出现在几个组里。
    /// 组按版本号倒序（新的在上），组内保持接口给的新→旧顺序。
    pub async fn resource_file_groups(
        &self,
        source: &str,
        id: &str,
    ) -> Result<ResourceGroupsView, AppError> {
        let source = parse_source(source)?;
        let files = match source {
            ModSource::Modrinth => {
                // Modrinth 一次给全部，没有分页
                self.modrinth_files(id, None, None, None).await?
            }
            // CF 单页 50 条，必须翻完
            ModSource::CurseForge => self.curseforge_all_files(id).await?,
        };
        Ok(group_by_game_version(files))
    }

    /// 手动模式右栏：本地所有能装的版本，最近玩过的在前。
    ///
    /// 「能装」的口径跟着 `kind` 走：Mod 要有加载器，资源包原版也算。
    pub fn resource_install_targets(&self, kind: Option<&str>) -> Result<Vec<InstallTargetView>, AppError> {
        let kind = parse_kind(kind)?;
        Ok(targets::candidates(kind, &local::scan(&self.layout))
            .into_iter()
            .map(|candidate| InstallTargetView {
                version_id: candidate.version_id,
                game_version: candidate.game_version,
                loader: candidate.display_loader.to_string(),
                last_played_ms: candidate.last_played_ms,
            })
            .collect())
    }

    /// 下载一个文件。
    ///
    /// `target_version_id` 与 `target_dir` 二选一：
    /// - 给版本 → 落点到该版本的游戏目录 `/mods/`（资源包是 `/resourcepacks/`，
    ///   隔离与否由 `isolation` 判定）；
    /// - 给目录 → 直接用，不再套一层 `mods/`（自己选的就是目的地）。
    ///
    /// 同名文件直接拒绝：文件名就是模组身份，加 `(2)` 没有任何意义。
    pub async fn resource_install(
        self: &Arc<Self>,
        source: &str,
        id: &str,
        file_id: &str,
        target_version_id: Option<&str>,
        target_dir: Option<&str>,
        kind: Option<&str>,
        emit: ProgressSink,
    ) -> Result<TaskView, AppError> {
        let source = parse_source(source)?;
        let kind = parse_kind(kind)?;
        // 整合包装的是新实例，不是往某个版本里塞文件 —— 走 `modpack_install`
        if kind == ResourceKind::Modpack {
            return Err(AppError::validation(
                "整合包不能这样装",
                "整合包会新建一个版本，不该指定落点",
            ));
        }

        let dest_dir = match (target_version_id, target_dir) {
            (Some(_), Some(_)) => {
                return Err(AppError::validation(
                    "目标重复",
                    "不能同时指定版本和文件夹",
                ))
            }
            (None, None) => {
                return Err(AppError::validation(
                    "还没选装到哪",
                    "选一个本地版本，或者选一个文件夹",
                ))
            }
            (None, Some(dir)) => PathBuf::from(dir),
            (Some(version_id), None) => self.resource_dir(version_id, kind)?,
        };

        let facts = self.file_facts(source, id, file_id).await?;
        let file = facts.file;
        let filename = safe_filename(&file.filename);
        let dest = dest_dir.join(&filename);

        if dest.exists() {
            return Err(AppError::validation(
                "这个文件已经装过了",
                dest.display().to_string(),
            )
            .with_hint("要重新下载，请先把那个文件删掉"));
        }

        std::fs::create_dir_all(&dest_dir)
            .map_err(|e| AppError::io(&e, "创建资源目录"))?;

        // 任务 id 用 `mod:<fileId>`：不会和版本安装任务的 id 撞上，
        // 也不会被版本详情的 `forVersion` 误认成自己发起的安装。
        // 资源包也用这个前缀 —— 前端的 `FILE_TASK_PREFIX` 只认它一份，
        // 换个前缀就等于「进度不显示了」。
        let task_version = format!("mod:{file_id}");
        // 落点标识：给版本就用版本 id，自定义文件夹就用目录本身 ——
        // 和前端「文件 + 目标」这一对键对得上。
        let target = target_version_id
            .map(str::to_string)
            .unwrap_or_else(|| dest_dir.display().to_string());
        self.start_file_install(
            kind.file_kind(),
            dest,
            &file.url,
            file.sha1.clone(),
            file.size,
            &task_version,
            filename,
            target,
            emit,
        )
        .await
    }

    /// 右栏依赖区块：这个文件声明的直接依赖。
    ///
    /// `target_version_id` 可以没有：手动模式不选安装位置也要先显示
    /// 「依赖谁、必装/可选/不兼容」；此时只解析依赖项目，不解析具体文件版本。
    /// 选了本地版本后再调一次，才补 `file` / `installed`。
    ///
    /// 资源包也有依赖：Fresh Animations 依赖 EMF（Entity Model Features），
    /// 没有它包是死的（游戏里能选、画面不动）。所以不再按类型短路。
    /// 依赖本身永远是 Mod，文件配对用 `dep_loader()`（本地版本自身的加载器），
    /// 落点也永远是 `mods/`。
    pub async fn resource_dependencies(
        &self,
        source: &str,
        id: &str,
        file_id: &str,
        target_version_id: Option<&str>,
        kind: Option<&str>,
    ) -> Result<Vec<ResourceDependencyView>, AppError> {
        let source = parse_source(source)?;
        let kind = parse_kind(kind)?;
        let candidate = match target_version_id {
            Some(target_version_id) => Some(
                targets::candidates(kind, &local::scan(&self.layout))
                    .into_iter()
                    .find(|item| item.version_id == target_version_id)
                    .ok_or_else(|| {
                        AppError::not_found(
                            format!("这个版本装不了{}", kind.label()),
                            format!("{target_version_id} 不是可以装{}的本地版本", kind.label()),
                        )
                    })?,
            ),
            None => None,
        };

        let mut facts = self.file_facts(source, id, file_id).await?;
        // 光影的类型级前置（Iris）也要算进来：手动模式换了文件、或前端单独查
        // 依赖时都会走到这里，只在文件列表里补是留不住的 —— 切个文件就没了。
        if source == ModSource::Modrinth {
            if let Some(prereq) = prerequisite_dependency(kind) {
                if !facts
                    .dependencies
                    .iter()
                    .any(|dep| dep.project_id == prereq.project_id)
                {
                    facts.dependencies.push(prereq);
                }
            }
        }
        self.resolve_dependencies(source, id, candidate.as_ref(), &facts.dependencies)
            .await
    }

    /// 把一批直接依赖解析成右栏视图。
    ///
    /// - 并行：每个依赖各开一个 future，不再一条条排队等（CF 镜像一次就 1s 左右）；
    /// - 项目级已安装判定：只要这个依赖项目在当前 MC 版本 + 加载器下
    ///   任意一个候选文件已经在 `mods/` 里，就算已安装 —— 依赖页里装的是
    ///   另一个文件版本（Modrinth 钉的 beta、或手动选了别的版本）也不会漏判；
    /// - 钉住的版本只影响「默认装哪个」，不参与已安装判定。
    async fn resolve_dependencies(
        &self,
        source: ModSource,
        id: &str,
        candidate: Option<&targets::LocalCandidate>,
        dependencies: &[deps::FileDependency],
    ) -> Result<Vec<ResourceDependencyView>, AppError> {
        let mods = match candidate {
            Some(candidate) => Some(self.resource_dir(&candidate.version_id, ResourceKind::Mod)?),
            None => None,
        };
        let mut seen: Vec<String> = Vec::new();
        let mut list: Vec<&deps::FileDependency> = Vec::new();
        for dep in dependencies {
            if dep.project_id == id || seen.contains(&dep.project_id) {
                continue;
            }
            seen.push(dep.project_id.clone());
            list.push(dep);
        }

        let futures = list.into_iter().map(|dep| {
            // PathBuf 克隆很小；不克隆的话 `FnMut` 每次都会试图把 `mods` move 走
            let mods = mods.clone();
            async move {
            let project = match source {
                ModSource::Modrinth => self.modrinth_project(&dep.project_id).await?,
                ModSource::CurseForge => {
                    // 指向资源包等非 Mod 项目时跳过（它们不适用 Mod 的文件接口）
                    let Some(project) = self.curseforge_mod(&dep.project_id).await? else {
                        return Ok::<Option<ResourceDependencyView>, AppError>(None);
                    };
                    project
                }
            };

            // 不兼容项只做警示，不解析文件
            if dep.relation == deps::DepRelation::Incompatible {
                return Ok(Some(ResourceDependencyView {
                    project,
                    relation: dep.relation.as_str(),
                    file: None,
                    installed: false,
                    reason: None,
                }));
            }

            let (file, installed) = match candidate {
                // 原版实例（没有加载器）：依赖是 Mod，装进去也不会被加载。
                // 只报「依赖谁 + 什么关系」，不给文件 —— 否则就是"装了没用"。
                Some(candidate) if candidate.dep_loader().is_none() => (None, false),
                Some(candidate) => {
                    // 有目标版本时 mods 一定算过
                    let mods = mods.as_ref().expect("有目标版本时 mods 一定存在");
                    match source {
                        ModSource::Modrinth => {
                            // 项目候选先拿全：既用于已安装判定，也用作钉版本不可用时的回退
                            let files = self
                                .modrinth_files(
                                    &dep.project_id,
                                    Some(&candidate.game_version),
                                    candidate.dep_loader(),
                                    None,
                                )
                                .await?;
                            let mut installed = files
                                .iter()
                                .any(|file| mods.join(safe_filename(&file.filename)).exists());
                            let pinned = match dep.version_id.as_deref() {
                                Some(version_id) => {
                                    match self.modrinth_file_facts(version_id).await {
                                        // 钉住的版本可能被作者删了，拉不到就退回按项目挑最新
                                        Ok(facts)
                                            if targets::file_matches(
                                                &facts.file,
                                                &candidate.game_version,
                                                candidate.dep_loader(),
                                            ) =>
                                        {
                                            installed |= mods
                                                .join(safe_filename(&facts.file.filename))
                                                .exists();
                                            Some(facts.file)
                                        }
                                        _ => None,
                                    }
                                }
                                None => None,
                            };
                            let file = pinned.or_else(|| {
                                deps::pick_dependency_file(
                                    &files,
                                    &candidate.game_version,
                                    candidate.dep_loader(),
                                )
                            });
                            (file, installed)
                        }
                        ModSource::CurseForge => {
                            let files = self
                                .curseforge_files(
                                    &dep.project_id,
                                    Some(&candidate.game_version),
                                    candidate.dep_loader(),
                                    None,
                                )
                                .await?;
                            let installed = files
                                .iter()
                                .any(|file| mods.join(safe_filename(&file.filename)).exists());
                            (
                                deps::pick_dependency_file(
                                    &files,
                                    &candidate.game_version,
                                    candidate.dep_loader(),
                                ),
                                installed,
                            )
                        }
                    }
                }
                // 还没选安装位置：只给「依赖谁 + 关系」，具体文件等选了再看
                None => (None, false),
            };

            let reason = match candidate {
                // 依赖永远是 Mod，所以按本地版本自身的加载器拼文案。
                // 原版实例没有加载器 —— 它的依赖不是"没有对应文件"，而是根本装不了。
                Some(candidate) if file.is_none() => Some(match candidate.dep_loader() {
                    Some(loader) => {
                        format!("没有支持 {} · {} 的文件", candidate.game_version, loader)
                    }
                    None => "这个版本没有安装加载器，装不了 Mod 依赖".to_string(),
                }),
                _ => None,
            };

            Ok(Some(ResourceDependencyView {
                project,
                relation: dep.relation.as_str(),
                file,
                installed,
                reason,
            }))
            }
        });

        let results = futures::future::join_all(futures).await;
        let mut out: Vec<ResourceDependencyView> = Vec::new();
        for result in results {
            if let Some(view) = result? {
                out.push(view);
            }
        }
        Ok(out)
    }

    /// 这个版本的资源目录（`mods/` 或 `resourcepacks/`）。
    /// 路径只在后端算，前端不拼。
    fn resource_dir(&self, version_id: &str, kind: ResourceKind) -> Result<PathBuf, AppError> {
        let resolution =
            isolation::resolution_for(&self.layout, &self.config(), version_id)?;
        Ok(resolution.game_dir.join(kind.subdir()))
    }

    /// 按源分发取文件列表。
    async fn fetch_files(
        &self,
        source: ModSource,
        id: &str,
        game_version: Option<&str>,
        loader: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<ResourceFileView>, AppError> {
        match source {
            ModSource::Modrinth => {
                self.modrinth_files(id, game_version, loader, limit).await
            }
            ModSource::CurseForge => {
                self.curseforge_files(id, game_version, loader, limit).await
            }
        }
    }

    /// 按 fileId 取单个文件的完整信息（下载要用文件名 / 大小 / sha1），
    /// 连同它声明的依赖一起返回。
    async fn file_facts(
        &self,
        source: ModSource,
        id: &str,
        file_id: &str,
    ) -> Result<FileFacts, AppError> {
        match source {
            ModSource::Modrinth => self.modrinth_file_facts(file_id).await,
            ModSource::CurseForge => self.curseforge_file_facts(id, file_id).await,
        }
    }
}

/// 按「文件声明的 MC 版本」分组（纯函数）。
pub(crate) fn group_by_game_version(files: Vec<ResourceFileView>) -> ResourceGroupsView {
    let total = files.len();
    let mut versions: std::collections::BTreeMap<String, Vec<ResourceFileView>> =
        std::collections::BTreeMap::new();

    for file in files {
        // 同一个文件在不同组里各放一份（clone）；一个文件声明了 20 个版本
        // 就会出现 20 次，这是"按版本找文件"必须付的代价。
        let mut seen: Vec<&String> = Vec::new();
        for version in &file.game_versions {
            if seen.contains(&version) {
                continue;
            }
            seen.push(version);
            versions.entry(version.clone()).or_default().push(file.clone());
        }
    }

    // 具体版本 → 版本线（`1.21.11` 收进 `1.21`）。层数不能塌成一层：
    // 老 Mod 几千个文件、每个文件挂在好几个版本上，平铺就是上万行的单个列表。
    let mut lines: std::collections::BTreeMap<String, Vec<ResourceFileGroupView>> =
        std::collections::BTreeMap::new();
    for (label, items) in versions {
        // 快照 / 预发行版不各占一条线：`24w14a`、`1.21.5-pre1` 这种一多就
        // 散成几十条线，整列全是碎片。统一收进「预览版」一条线里。
        let line = if is_preview(&label) {
            PREVIEW_LINE.to_string()
        } else {
            version_line(&label)
        };
        lines
            .entry(line)
            .or_default()
            .push(ResourceFileGroupView { label, items });
    }

    let mut out: Vec<ResourceFileLineView> = lines
        .into_iter()
        .map(|(label, mut versions)| {
            // 线内：版本号倒序（1.21.11 > 1.21.9）
            versions.sort_by(|a, b| version_key(&b.label).cmp(&version_key(&a.label)));
            let count = versions.iter().map(|v| v.items.len()).sum();
            ResourceFileLineView {
                label,
                count,
                versions,
            }
        })
        .collect();
    // 线之间同样按版本号倒序（1.21 > 1.20 > 1.12），「预览版」永远垫底
    out.sort_by(|a, b| line_order(&b.label).cmp(&line_order(&a.label)));

    ResourceGroupsView { lines: out, total }
}

/// 快照 / 预发行版归到这一条线里。
const PREVIEW_LINE: &str = "预览版";

/// 是不是快照或预发行版（`24w14a` / `1.21.5-pre1` / `1.21.5-rc1`）。
///
/// 只看字符串形态：Mod 文件里没有"版本类型"这个字段，靠名字判断是唯一
/// 不用额外请求的办法。`26.3` 这种新命名是正式版，别误判。
fn is_preview(version: &str) -> bool {
    let bytes = version.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2] == b'w'
    {
        return true;
    }
    version.contains("-pre")
        || version.contains("-rc")
        || version.contains("-snapshot")
        || version.contains(" Pre-Release")
        || version.contains(" Release Candidate")
}

/// 版本线的排序键：正式版线按版本号，`预览版` 永远在最后。
fn line_order(label: &str) -> (u8, (u8, Vec<u32>, String)) {
    if label == PREVIEW_LINE {
        (0, (0, Vec::new(), String::new()))
    } else {
        (1, version_key(label))
    }
}

/// 版本号排序键：`1.21.11` → `[1,21,11]`；`24w14a` 这种快照排最后。
fn version_key(version: &str) -> (u8, Vec<u32>, String) {
    let numbers: Option<Vec<u32>> = version.split('.').map(|part| part.parse().ok()).collect();
    match numbers {
        Some(numbers) => (2, numbers, version.to_string()),
        None => (1, Vec::new(), version.to_string()),
    }
}

/// 只留文件名本身：接口回来的 `filename` 不能带路径分隔符，
/// 否则一个恶意的 `/` 就能把文件写到目录外面去。
fn safe_filename(raw: &str) -> String {
    std::path::Path::new(raw)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        .unwrap_or("mod.jar")
        .to_string()
}

/// 版本号 → 它所属的大版本线。
///
/// - 新命名（`26.3`）本身就是一条线，原样返回；
/// - 旧命名（`1.20.1`）截到前两段（`1.20`），`1.7.10` → `1.7`。
fn version_line(id: &str) -> String {
    let parts: Vec<&str> = id.split('.').collect();
    if parts.len() <= 2 {
        id.to_string()
    } else {
        format!("{}.{}", parts[0], parts[1])
    }
}

/// 最小的百分号编码：只放行 unreserved 字符（`A-Z a-z 0-9 - _ . ~`）。
///
/// 不引第三方库：这里只需要给搜索词和 facets 用，够用就行。
pub(crate) fn percent_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    // `file_kind()` 现在住在 `types.rs`，`super::*` 拿得到类型但拿不到 `FileKind` 本身
    use crate::install::FileKind;

    #[test]
    fn encode_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(percent_encode("sodium"), "sodium");
        assert_eq!(percent_encode("1.20.1"), "1.20.1");
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(
            percent_encode(r#"[["project_type:mod"]]"#),
            "%5B%5B%22project_type%3Amod%22%5D%5D"
        );
        assert_eq!(percent_encode("钠"), "%E9%92%A0");
    }

    fn file(id: &str, versions: &[&str]) -> ResourceFileView {
        ResourceFileView {
            dependency_count: 0,
            dependencies: Vec::new(),
            file_id: id.into(),
            version_number: id.into(),
            filename: format!("{id}.jar"),
            size: 1,
            sha1: None,
            url: String::new(),
            published: None,
            release_type: "release".into(),
            game_versions: versions.iter().map(|v| v.to_string()).collect(),
            loaders: vec!["fabric".into()],
        }
    }

    #[test]
    fn shader_kind_follows_the_three_platform_mappings() {
        let kind = ResourceKind::Shader;
        // Modrinth 是单数 `shader`，写成 `shaders` 会搜出 0 条（实测 1093 条）
        assert_eq!(kind.modrinth_type(), "shader");
        assert_eq!(kind.modrinth_segment(), "shader");
        // CF 的光影 classId 是 6552（实测能返回真实的 shader 项目）
        assert_eq!(kind.curseforge_class(), 6552);
        assert_eq!(kind.subdir(), "shaderpacks");
        assert_eq!(kind.file_kind(), FileKind::Shader);
        assert_eq!(kind.label(), "光影");
    }

    #[test]
    fn only_shader_declares_a_type_level_prerequisite() {
        assert_eq!(
            ResourceKind::Shader.prerequisite_project(),
            Some(IRIS_PROJECT_ID)
        );
        // Mod 和资源包的依赖完全靠文件自己声明，不该凭空多一条
        assert_eq!(ResourceKind::Mod.prerequisite_project(), None);
        assert_eq!(ResourceKind::ResourcePack.prerequisite_project(), None);
    }

    #[test]
    fn prerequisites_are_attached_only_for_shader_on_modrinth() {
        let mut shader_files = vec![file("a", &["1.21.1"])];
        attach_prerequisites(ResourceKind::Shader, ModSource::Modrinth, &mut shader_files);
        // 依赖被补上，且 `dependency_count` 跟着刷新 —— 前端靠它决定出不出骨架
        assert_eq!(shader_files[0].dependency_count, 1);
        assert_eq!(shader_files[0].dependencies[0].project_id, IRIS_PROJECT_ID);
        assert_eq!(
            shader_files[0].dependencies[0].relation,
            deps::DepRelation::Required
        );

        // CurseForge 上拿这个 Modrinth id 查不到东西，硬塞只会多一条错误
        let mut cf_files = vec![file("a", &["1.21.1"])];
        attach_prerequisites(ResourceKind::Shader, ModSource::CurseForge, &mut cf_files);
        assert_eq!(cf_files[0].dependency_count, 0);

        // 资源包没有类型级前置
        let mut pack_files = vec![file("a", &["1.21.1"])];
        attach_prerequisites(
            ResourceKind::ResourcePack,
            ModSource::Modrinth,
            &mut pack_files,
        );
        assert_eq!(pack_files[0].dependency_count, 0);
    }

    #[test]
    fn prerequisites_do_not_duplicate_an_existing_dependency() {
        // 万一上游哪天自己声明了 Iris，不能出两张卡
        let mut files = vec![file("a", &["1.21.1"])];
        files[0].dependencies.push(deps::FileDependency {
            project_id: IRIS_PROJECT_ID.to_string(),
            version_id: None,
            relation: deps::DepRelation::Required,
        });
        files[0].dependency_count = 1;
        attach_prerequisites(ResourceKind::Shader, ModSource::Modrinth, &mut files);
        assert_eq!(files[0].dependencies.len(), 1);
        assert_eq!(files[0].dependency_count, 1);
    }

    #[test]
    fn files_are_grouped_into_line_then_version() {
        let view = group_by_game_version(vec![
            file("a", &["1.21.11", "1.21.9"]),
            file("b", &["1.21.9"]),
            file("c", &["1.20.1"]),
            file("d", &["24w14a"]),
            file("e", &["1.21.5-pre1"]),
        ]);
        let labels: Vec<&str> = view.lines.iter().map(|l| l.label.as_str()).collect();
        // 正式版线倒序，快照 / 预发行版统一收进「预览版」并垫底
        assert_eq!(labels, vec!["1.21", "1.20", "预览版"]);
        // 线内还是倒序，一个文件声明了多个版本 → 出现在多个版本组里
        let line = &view.lines[0];
        let inner: Vec<&str> = line.versions.iter().map(|v| v.label.as_str()).collect();
        assert_eq!(inner, vec!["1.21.11", "1.21.9"]);
        assert_eq!(line.versions[1].items.len(), 2);
        // count 是这条线下的条目数（含重复），total 是文件数（不含重复）
        assert_eq!(line.count, 3);
        assert_eq!(view.total, 5);
        // 预览版那条线里各自还是一个独立的版本组，只是不再各占一条线
        let preview = &view.lines[2];
        assert_eq!(preview.versions.len(), 2);
        assert_eq!(preview.count, 2);
    }

    #[test]
    fn new_style_release_is_not_mistaken_for_snapshot() {
        // 26.3 是正式版命名，不能因为"短"就归进预览版
        assert!(!is_preview("26.3"));
        assert!(!is_preview("1.21.11"));
        assert!(is_preview("24w14a"));
        assert!(is_preview("1.21.5-pre1"));
        assert!(is_preview("1.21.5-rc1"));
    }

    #[test]
    fn source_parses_both_names() {
        assert_eq!(ModSource::parse("modrinth"), Some(ModSource::Modrinth));
        assert_eq!(ModSource::parse("curseforge"), Some(ModSource::CurseForge));
        assert_eq!(ModSource::parse("curse"), None);
        // 序列化出去的名字要和前端一致
        assert_eq!(ModSource::CurseForge.as_str(), "curseforge");
    }
}
