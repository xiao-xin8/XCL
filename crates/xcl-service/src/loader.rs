//! 模组加载器的查询与安装。
//!
//! 当前只有 Fabric 接通：官方给一份完整的 profile JSON，拿回来落盘就是版本文件，
//! 不需要安装器、不需要跑 JVM。Forge / NeoForge 的纯逻辑在
//! `xcl_loader::forgelike`，安装流水线（预下载 → 跑官方安装器 → 收编）还没接进来，
//! 所以 `LoaderKind::is_ready` 仍然只放行 Fabric，界面上也就看不到那两个选项。
//!
//! 安装策略：装出来的是一条自包含版本（原版 + 加载器合并成一份 JSON），
//! 原版没装也照样能装加载器，且不会多出一条原版记录；
//! 库、资源、Java 仍在数据根共享，所以重复的部分直接命中缓存。

use std::fs;

use serde::{Deserialize, Serialize};
use tracing::debug;
use xcl_core::{merge_version_json, AppError, VersionDetail, VersionKind};
use xcl_loader::LoaderKind;
use xcl_loader::{fabric, forgelike};

use crate::install::ResolvedVersion;
use crate::local;
use crate::download::ProgressSink;
use crate::AppState;

/// 前端选中的加载器（IPC 入参）。
#[derive(Debug, Clone, Deserialize)]
pub struct LoaderRef {
    pub kind: String,
    pub version: String,
}

/// 一次加载器安装要用到的全部参数。
///
/// 多出来的三个字段是给整合包用的：整合包要装进它自己命名的实例、
/// 要把包里的文件并进同一份安装清单、还要沿用调用方已经开好的任务卡。
/// 普通安装（下载页点「安装」）三个都传空，行为和以前一字不差。
pub struct LoaderInstall {
    pub game: String,
    pub loader: LoaderRef,
    /// 不传就按规范命名生成（`fabric-loader-x-y` / `forge-x-y`）。
    pub instance_id: Option<String>,
    /// 合并进安装清单的额外文件（整合包内容）。空 = 只装游戏。
    pub extra_files: Vec<crate::install::PendingFile>,
    /// 不传就自己开一张任务卡。传了 = 沿用调用方的卡，不插新卡也不去重。
    pub task_id: Option<String>,
}

impl LoaderRef {
    pub fn kind(&self) -> Result<LoaderKind, AppError> {
        LoaderKind::parse(&self.kind).ok_or_else(|| {
            AppError::unsupported("不支持的加载器", format!("未知的加载器类型：{}", self.kind))
        })
    }
}

// ---------- 已安装判定 ----------

/// 加载器 → 版本分类。进度表只有三种，一一对应。
pub(crate) fn version_kind_of(kind: LoaderKind) -> VersionKind {
    match kind {
        LoaderKind::Fabric => VersionKind::Fabric,
        LoaderKind::Forge => VersionKind::Forge,
        LoaderKind::NeoForge => VersionKind::NeoForge,
    }
}

/// 规范实例 id。写盘时用它命名，判定时不靠它（名字会改）。
fn instance_id_of(kind: LoaderKind, loader: &str, game: &str) -> String {
    match kind {
        LoaderKind::Fabric => fabric::instance_id(loader, game),
        other => forgelike::instance_id(other, game, loader),
    }
}

/// 「这个加载器版本 + 这个原版」装过了吗。
///
/// 判据是版本内容：`libraries` 里的加载器库版本 + 落盘的原版号。
/// 留一条按规范实例 id 的兜底，盖住极老、库坐标认不出来的版本 JSON。
fn loader_installed(
    versions: &[local::LocalVersion],
    kind: LoaderKind,
    loader: &str,
    game: &str,
) -> bool {
    local::has_loader(versions, version_kind_of(kind), loader, game)
        || versions
            .iter()
            .any(|version| version.valid && version.id == instance_id_of(kind, loader, game))
}

// ---------- 加载器类型 ----------

/// 界面上的加载器下拉项。
///
/// 三个类型一律给出：界面只负责渲染，不给任何类型开特殊分支。
/// 能力差异（安装流水线还没接通）由 `ensure_installable` 在预览/安装那一步
/// 明确报错，而不是在前端藏一半界面。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderKindView {
    pub kind: String,
    pub label: String,
}

impl AppState {
    /// 可选的加载器类型（界面按这个顺序摆下拉项）。
    pub fn loader_kinds(&self) -> Vec<LoaderKindView> {
        xcl_loader::LoaderKind::all()
            .into_iter()
            .map(|kind| LoaderKindView {
                kind: kind.as_str().to_string(),
                label: kind.label().to_string(),
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderItemView {
    pub version: String,
    pub stable: bool,
    /// 这个加载器版本配上当前这个原版的实例已经装好了。
    ///
    /// 必须在 Rust 侧算：实例 id（`fabric-loader-x-y`）的命名规则只在
    /// `xcl-loader::fabric` 里有一份，前端不该再复制一遍。
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderGroupView {
    /// `0.16.x`
    pub key: String,
    pub latest: bool,
    pub items: Vec<LoaderItemView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderListView {
    pub kind: String,
    /// 默认选中项（最新稳定版）。列表为空时是 `None`。
    pub latest: Option<String>,
    pub groups: Vec<LoaderGroupView>,
}

impl AppState {
    /// 某个加载器的可选版本，已经按 minor 分好组。
    ///
    /// 分组在 Rust 侧算：前端只渲染，不做数据变换。
    ///
    /// `game` 给了就顺带标出「这个加载器版本 + 这个原版」的实例是否已装 ——
    /// 只扫描版本目录，不联网，所以每次调用都能给最新状态（不进缓存）。
    pub async fn loader_versions(
        &self,
        kind: &str,
        game: Option<&str>,
    ) -> Result<LoaderListView, AppError> {
        let kind = LoaderKind::parse(kind).ok_or_else(|| {
            AppError::unsupported("不支持的加载器", format!("未知的加载器类型：{kind}"))
        })?;

        // 已装实例：按「加载器 + 加载器版本 + 原版」认人，不看目录名 ——
        // 把 `fabric-loader-0.20.1-26.2` 改成「我的整合」，这里照样认得出来。
        let local_versions = match game {
            Some(_) => local::scan(&self.layout),
            None => Vec::new(),
        };

        let list = match kind {
            // Fabric 的元数据是「全局 loader 版本列表」，不带原版信息，
            // 所以这里不按 game 筛（历史行为，保持不变）
            LoaderKind::Fabric => {
                let url = fabric::loader_list_url();
                let (list, _) = self
                    .cache()
                    .get_or_fetch("loader:list:fabric", false, || async {
                        self.fetch_loader_text(&url, fabric::parse_loader_versions)
                            .await
                    })
                    .await?;
                list
            }
            // Forge / NeoForge 的元数据是「所有 MC 版本混在一份文件里」（2000 多条），
            // 所以缓存全量原始版本串，切原版只在内存里筛：
            // 按 game 建缓存键会让每点一个原版就打一次 213KB
            LoaderKind::Forge => {
                let game = require_game(kind, game)?;
                let url = forgelike::forge_metadata_url();
                let (raw, _) = self
                    .cache()
                    .get_or_fetch("loader:list:forge", false, || async {
                        self.fetch_loader_text(&url, forgelike::parse_metadata_versions)
                            .await
                    })
                    .await?;
                forgelike::forge_loader_versions(&raw, game)
            }
            LoaderKind::NeoForge => {
                let game = require_game(kind, game)?;
                let modern_url = forgelike::neoforge_metadata_url(false);
                let (mut raw, _) = self
                    .cache()
                    .get_or_fetch("loader:list:neoforge", false, || async {
                        self.fetch_loader_text(&modern_url, forgelike::parse_metadata_versions)
                            .await
                    })
                    .await?;
                // 1.20.1 那一支在另一个 artifact（`net/neoforged/forge`）里，
                // 而且镜像上可能没有这份（实测 302）：拉不到就只当没有，
                // 另一支有货就不该整体报错
                let legacy_url = forgelike::neoforge_metadata_url(true);
                if let Ok((mut legacy, _)) = self
                    .cache()
                    .get_or_fetch("loader:list:neoforge:legacy", false, || async {
                        self.fetch_loader_text(&legacy_url, forgelike::parse_metadata_versions)
                            .await
                    })
                    .await
                {
                    raw.append(&mut legacy);
                }
                forgelike::neoforge_loader_versions(&raw, game)
            }
        };

        // 默认给最新稳定版：不稳定版留给愿意自己挑的人
        let latest = list
            .iter()
            .find(|item| item.stable)
            .or_else(|| list.first())
            .map(|item| item.version.clone());

        let groups = fabric::group_by_minor(&list)
            .into_iter()
            .map(|group| LoaderGroupView {
                key: group.key,
                latest: group.latest,
                items: group
                    .items
                    .into_iter()
                    .map(|item| {
                        let is_installed = match game {
                            Some(g) => loader_installed(&local_versions, kind, &item.version, g),
                            None => false,
                        };
                        LoaderItemView {
                            version: item.version,
                            stable: item.stable,
                            installed: is_installed,
                        }
                    })
                    .collect(),
            })
            .collect();

        Ok(LoaderListView {
            kind: kind.as_str().to_string(),
            latest,
            groups,
        })
    }

    /// 拉一段加载器元数据文本并交给 `parse` 解析（三个加载器共用）。
    ///
    /// 统一走 `fetch_meta_text`：和版本清单同一套双源竞速 + 测速记录。
    /// 直接 `http().get_text()` 会按候选顺序逐个等重试退避，
    /// 镜像慢的时候就是「加载器列表一直转圈」。
    async fn fetch_loader_text<T>(
        &self,
        url: &str,
        parse: impl FnOnce(&str) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let (text, _) = self.fetch_meta_text(url).await?;
        parse(&text)
    }

    /// 安装流水线还没接通的类型，一律在这里挡住（预览与安装共用一条判定）。
    pub(crate) fn ensure_installable(kind: LoaderKind) -> Result<(), AppError> {
        if kind.is_installable() {
            return Ok(());
        }
        Err(AppError::unsupported(
            "这个加载器的安装还没接通",
            format!(
                "{} 的版本列表已经能看，但安装器流水线还没接进来",
                kind.label()
            ),
        )
        .with_hint("先能选能看；安装等这轮做完就放开"))
    }

    /// 拉 profile JSON 原文（带缓存）。返回文本而不解析，
    /// 是为了安装时能原样落盘 —— 解析再序列化会丢字段。
    async fn fetch_profile(&self, game: &str, loader: &str) -> Result<String, AppError> {
        let url = fabric::profile_url(game, loader);
        let cache_key = format!("loader:profile:fabric:{loader}:{game}");
        let (text, _) = self
            .cache()
            .get_or_fetch(&cache_key, false, || async {
                let (text, _) = self.fetch_meta_text(&url).await?;
                Ok::<String, AppError>(text)
            })
            .await?;
        Ok(text)
    }

    /// 解析出一个加载器版本的完整信息。
    ///
    /// 装出来的是自包含版本：原版与加载器合并成一份 JSON，
    /// 不再靠 `inheritsFrom` 指回原版 —— 所以不会在版本列表里多出一条原版记录。
    /// 合并后的 JSON 已经存在时直接读本地，不重新合并。
    ///
    /// 返回 `(解析结果, 需要落盘的 JSON 原文)`：原文为 `None` 说明本地已经有 JSON。
    pub(crate) async fn resolve_loader(
        &self,
        game: &str,
        loader: &LoaderRef,
    ) -> Result<(ResolvedVersion, Option<String>), AppError> {
        // 先按类型分流：下面的 id 与 profile 地址只属于 Fabric，
        // Forge / NeoForge 走安装器那条路（读安装器里的 version.json 再合并）
        let kind = loader.kind()?;
        if kind != LoaderKind::Fabric {
            return self.resolve_forgelike_loader(game, loader).await;
        }

        let id = fabric::instance_id(&loader.version, game);
        let json_path = self.layout.version_json(&id);

        // 之前装过：本地那份已经是合并结果，直接用
        if json_path.is_file() {
            let detail = local::read_detail(&json_path)?;
            let reuse_jar = local::client_jar(&self.layout, game).is_some()
                || local::client_jar(&self.layout, &id).is_some();
            return Ok((
                ResolvedVersion {
                    // 自己那份就是合并后的全部内容（没有父版本要合了）
                    own: detail.clone(),
                    detail,
                    json_url: None,
                    // 语义上仍然「源自这个原版」：清单里可以直接复用原版已下的文件
                    parent: Some(game.to_string()),
                    reuse_jar,
                },
                None,
            ));
        }

        let (text, child, merged_detail, reuse_jar) =
            self.merged_loader_json(game, loader).await?;
        Ok((
            ResolvedVersion {
                // `own` 保持只属于加载器自己的那份：原版已装时清单靠它
                // 判断「只要 7 个库」，把合并结果塞进来会让一千多个资源
                // 文件全部重新校验一遍，「秒装」变成「转半天」。
                own: child,
                detail: merged_detail,
                json_url: None,
                parent: Some(game.to_string()),
                reuse_jar,
            },
            Some(text),
        ))
    }

    /// 把「原版 JSON + 加载器 profile」合并成一份自包含的版本 JSON。
    ///
    /// 返回 `(合并后的文本, 加载器自己的详情, 合并后的详情, 能否复用原版的 jar)`。
    async fn merged_loader_json(
        &self,
        game: &str,
        loader: &LoaderRef,
    ) -> Result<(String, VersionDetail, VersionDetail, bool), AppError> {
        let kind = loader.kind()?;
        Self::ensure_installable(kind)?;

        let id = fabric::instance_id(&loader.version, game);
        let child_text = self.fetch_profile(game, &loader.version).await?;

        // 解析一次做校验：字段坏掉要在这里报错，而不是等落盘后启动才炸
        let child = VersionDetail::parse(&child_text)?;
        if child.inherits_from.as_deref() != Some(game) {
            return Err(AppError::parse(
                "加载器文件不对",
                format!(
                    "Fabric 返回的 inheritsFrom 是 {:?}，期望 {game}",
                    child.inherits_from
                ),
            ));
        }

        // 原版 JSON：装过就读本地文件，没装就从清单拉原文
        let parent_text = self.version_json_text(game).await?;
        let merged = merge_version_json(&child_text, &parent_text, &id)?;
        let detail = VersionDetail::parse(&merged)?;

        // 原版装过的话，它的 jar 可以复制过来，省一次 20MB 下载
        let reuse_jar = local::client_jar(&self.layout, game).is_some();
        debug!(id, "合并原版与加载器完成");
        Ok((merged, child, detail, reuse_jar))
    }

    /// 读某个版本的 JSON 原文（保真，不走类型序列化）。
    ///
    /// 本地有就读文件；没有就从清单拉（走 TTL 缓存）。
    pub(crate) async fn version_json_text(&self, id: &str) -> Result<String, AppError> {
        let path = self.layout.version_json(id);
        if path.is_file() {
            return fs::read_to_string(&path).map_err(|e| AppError::io(&e, "读取版本描述"));
        }

        let (url, _) = self.version_url(id).await?;
        let cache_key = format!("versions:text:{id}");
        let (text, _) = self
            .cache()
            .get_or_fetch(&cache_key, false, || async {
                let (text, _) = self.fetch_meta_text(&url).await?;
                Ok::<String, AppError>(text)
            })
            .await?;
        Ok(text)
    }

    /// 把父版本的客户端 jar 复制到加载器版本目录（已经存在就不动）。
    ///
    /// 自包含版本必须自己有 jar：没有 `inheritsFrom`，启动链路不会去父目录找。
    /// 复制 20MB 是瞬间的事，比重新下一个快得多。
    pub(crate) fn reuse_parent_jar(&self, game: &str, id: &str) -> bool {
        let dest = self.layout.version_jar(id);
        if dest.is_file() {
            return true;
        }
        let Some(source) = local::client_jar(&self.layout, game) else {
            return false;
        };
        if let Some(dir) = dest.parent() {
            let _ = fs::create_dir_all(dir);
        }
        match fs::copy(&source, &dest) {
            Ok(_) => true,
            Err(e) => {
                // 复制失败不是致命错误：清单里会把它当成待下载的文件
                debug!(?e, "复用原版 jar 失败，改为下载");
                false
            }
        }
    }

    /// 加载器安装清单（供 `install_preview` 预览用，不写盘）。
    pub(crate) async fn loader_plan(
        &self,
        game: &str,
        loader: &LoaderRef,
    ) -> Result<crate::install::InstallPlan, AppError> {
        let kind = loader.kind()?;
        let (resolved, _) = self.resolve_loader(game, loader).await?;
        // 预览清单的目标版本 id 必须和真正装出来的一致
        let id = match kind {
            LoaderKind::Fabric => fabric::instance_id(&loader.version, game),
            other => forgelike::instance_id(other, game, &loader.version),
        };
        self.plan_from(&id, &resolved).await
    }

    /// 安装加载器：写合并后的版本 JSON（原版 + 加载器一起装掉）→ 走统一下载引擎。
    ///
    /// 返回的 `TaskView` 与普通安装完全一样，前端不需要区分。
    ///
    /// 整合包靠 `request.instance_id` 装进它自己命名的实例，靠 `extra_files`
    /// 把包里的文件并进同一份清单（否则会多出一张任务卡），靠 `task_id`
    /// 沿用调用方已经开好的卡 —— 三处都留空时行为和以前一字不差。
    pub async fn install_loader(
        self: &std::sync::Arc<Self>,
        request: LoaderInstall,
        emit: ProgressSink,
    ) -> Result<crate::download::TaskView, AppError> {
        let LoaderInstall {
            game,
            loader,
            instance_id,
            extra_files,
            task_id,
        } = request;
        let game = game.as_str();
        let kind = loader.kind()?;
        // 类型分流要早于 id 计算：下面这个 id 用的是 Fabric 的命名
        if kind != LoaderKind::Fabric {
            return self
                .start_forgelike_install(game, &loader, instance_id, extra_files, task_id, emit)
                .await
                .map_err(|e| {
                    e.with_hint(format!(
                        "{} {} 安装失败，可回详情页重试",
                        kind.label(),
                        loader.version
                    ))
                });
        }
        Self::ensure_installable(kind)?;
        let id = instance_id.unwrap_or_else(|| fabric::instance_id(&loader.version, game));
        // 记下「这个实例属于哪个原版」：改名后「下载 Mod」页仍能认出它
        let _ = crate::isolation::ensure_game_version(&self.layout, &id, game);

        // 同一个版本已经在装了就直接复用任务（调用方自己开了卡时不查，
        // 它会查到刚插进去的那张）
        if task_id.is_none() {
            if let Some(running) = self.tasks().running_for(&id) {
                return Ok(running);
            }
        }

        let (resolved, merged) = self.resolve_loader(game, &loader).await?;
        if let Some(text) = merged {
            self.write_version_json(&id, &text)?;
        }
        // 原版装过就把 jar 拿过来，清单里只剩加载器那几个库
        self.reuse_parent_jar(game, &id);

        // 清单在这里算好再交给引擎：装完之后本地 JSON 没有 inheritsFrom，
        // 引擎自己重新 build_plan 会按「原版没装」去校验一千多个资源文件
        let mut plan = self.plan_from(&id, &resolved).await?;
        plan.files.extend(extra_files);
        let started = match task_id {
            Some(task_id) => {
                self.start_install_plan_as(plan, Some(game), task_id, emit)
                    .await
            }
            None => self.start_install_plan(plan, Some(game), emit).await,
        };
        started.map_err(|e| {
            e.with_hint(format!(
                "{} {} 安装失败，可到下载页重试",
                kind.label(),
                loader.version
            ))
        })
    }

    /// 版本 JSON 落盘（临时文件 + rename，避免写一半被启动链路读到）。
    pub(crate) fn write_version_json(&self, id: &str, text: &str) -> Result<(), AppError> {
        let path = self.layout.version_json(id);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| AppError::io(&e, "创建版本目录"))?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, text).map_err(|e| AppError::io(&e, "写入版本描述"))?;
        fs::rename(&tmp, &path).map_err(|e| AppError::io(&e, "替换版本描述"))?;
        Ok(())
    }
}

/// Forge / NeoForge 的版本必须按原版筛（元数据里所有 MC 版本混在一起）。
fn require_game(kind: LoaderKind, game: Option<&str>) -> Result<&str, AppError> {
    game.ok_or_else(|| {
        AppError::validation(
            "缺少原版版本",
            format!("{} 的版本列表要按原版筛选，必须给 game", kind.label()),
        )
    })
}
