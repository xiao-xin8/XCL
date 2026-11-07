//! 安装清单。
//!
//! 目标：给出"装这个版本需要哪些文件、多大"，不涉及任何下载行为。
//!
//! 安装分层：
//! 版本 JSON → 支持库 → 资源文件 → 版本 JAR，其中资源文件是最大的一块。

use std::path::PathBuf;

use serde::Serialize;
use xcl_core::path::maven_to_relative_path;
use xcl_core::{merge_versions, AppError, AssetIndexFile, Env, VersionDetail};

use crate::loader::LoaderRef;
use crate::{local, AppState};

/// 资源文件的官方根地址（镜像改写由 `xcl-source` 在下载时处理）。
const ASSET_BASE: &str = "https://resources.download.minecraft.net";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    VersionJson,
    ClientJar,
    Library,
    Native,
    /// `assets/indexes/<id>.json` —— 资源索引本身。
    ///
    /// 必须单独落盘：启动时游戏要靠它找每个资源对象的哈希
    /// （`launch.rs` 读 `assets/indexes/<name>.json`）。只把索引读进内存
    /// 用来展开对象列表是不够的 —— 之前就是漏了这一步，
    /// 装完之后启动直接报「找不到 indexes/1.12.json」。
    AssetIndex,
    Asset,
    /// 社区资源（Mod）。只走单文件计划：它不属于版本 JSON，
    /// 所以「校验并修复」永远不该碰它。
    Mod,
    /// 社区资源（资源包）。同样只走单文件计划，区别只在落点（`resourcepacks/`）
    /// 和任务卡片上的名字。
    ResourcePack,
    /// 社区资源（光影）。落点 `shaderpacks/`；要不要 Iris 才生效不归这里管。
    Shader,
    /// 整合包自带的一个文件。落点由整合包自己的清单决定（`mods/`、`config/`…），
    /// 不属于版本 JSON，所以「校验并修复」也不该碰它。
    ///
    /// 单独一个变体是因为任务卡要按它分组：一个 300 个文件的整合包，
    /// 全标成「Mod」会让抽屉里出现一堆莫名其妙的 Mod 计数。
    ModpackFile,
}

impl FileKind {
    pub fn key(self) -> &'static str {
        match self {
            FileKind::VersionJson => "json",
            FileKind::ClientJar => "jar",
            FileKind::Library => "library",
            FileKind::Native => "native",
            FileKind::AssetIndex => "assetIndex",
            FileKind::Asset => "asset",
            FileKind::Mod => "mod",
            FileKind::ResourcePack => "resourcepack",
            FileKind::Shader => "shader",
            FileKind::ModpackFile => "modpackFile",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FileKind::VersionJson => "版本描述",
            FileKind::ClientJar => "主文件",
            FileKind::Library => "支持库",
            FileKind::Native => "本地库",
            FileKind::AssetIndex => "资源索引",
            FileKind::Asset => "资源文件",
            FileKind::Mod => "Mod",
            FileKind::ResourcePack => "资源包",
            FileKind::Shader => "光影",
            FileKind::ModpackFile => "整合包文件",
        }
    }

    fn order(self) -> u8 {
        match self {
            FileKind::VersionJson => 0,
            FileKind::ClientJar => 1,
            FileKind::Library => 2,
            FileKind::Native => 3,
            FileKind::AssetIndex => 4,
            FileKind::Asset => 5,
            FileKind::Mod => 6,
            FileKind::ResourcePack => 7,
            FileKind::Shader => 8,
            FileKind::ModpackFile => 9,
        }
    }
}

/// 一个待下载文件。`url` 存官方地址，下载时再按源策略展开候选。
#[derive(Debug, Clone)]
pub struct PendingFile {
    pub kind: FileKind,
    pub local: PathBuf,
    pub url: String,
    pub sha1: Option<String>,
    pub size: u64,
}

/// 完整安装计划，供将来的下载引擎消费。
#[derive(Debug)]
pub struct InstallPlan {
    pub version_id: String,
    pub resolved: VersionDetail,
    pub files: Vec<PendingFile>,
}

/// 解析结果：合并后的详情 + 它从哪来 + 它的父版本（原版）。
pub struct ResolvedVersion {
    /// 合并后（自己 + 所有父版本）的完整详情。
    pub detail: VersionDetail,
    /// 自己那份未合并的详情。加载器安装只要它自己声明的库。
    pub own: VersionDetail,
    /// 这个版本自己的 JSON 地址。本地已经有 JSON 时为 `None`。
    pub json_url: Option<String>,
    /// `inheritsFrom` 指向的第一个版本，也就是原版。原版自身为 `None`。
    pub parent: Option<String>,
    /// 客户端 jar 已经有现成的可以复用（原版装过，安装时直接复制过来）。
    ///
    /// 自包含版本（装加载器时合并出来的那种）没有 `inheritsFrom`，
    /// 启动链路只会去自己目录里找 jar，所以那份 jar 必须存在 —— 要么复制，
    /// 要么老老实实下一遍。这个标记只影响清单里算不算这个 jar。
    pub reuse_jar: bool,
}

impl InstallPlan {
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanGroup {
    pub kind: String,
    pub label: String,
    pub count: usize,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPreview {
    pub version_id: String,
    pub version_type: Option<String>,
    pub release_time: Option<String>,
    pub java_major: Option<u32>,
    pub main_class: Option<String>,
    pub groups: Vec<PlanGroup>,
    pub total_count: usize,
    pub total_size: u64,
    /// 这份清单的目标版本已经装好了。
    ///
    /// 加载器场景里 `version_id` 是 `fabric-loader-x-y` 这个实例 id，
    /// 界面据此把主按钮从「安装」换成「校验并修复」。
    pub installed: bool,
}

impl AppState {
    /// 给界面用的摘要：数量 + 体积，不含文件明细（避免几 MB 的 IPC 负载）。
    pub async fn install_preview(
        &self,
        version_id: &str,
        loader: Option<&LoaderRef>,
    ) -> Result<InstallPreview, AppError> {
        let (plan, release_time) = match loader {
            // 加载器预览：清单要现合现算（不写文件）
            Some(loader) => {
                let plan = self.loader_plan(version_id, loader).await?;
                // 合并出来的方案里没有发布时间，但原版清单里有，顺手带上
                let release_time = self
                    .version_url(version_id)
                    .await
                    .ok()
                    .and_then(|(_, time)| time);
                (plan, release_time)
            }
            None => {
                let (_, release_time) = self.version_url(version_id).await?;
                (self.build_plan(version_id).await?, release_time)
            }
        };

        let mut groups: Vec<PlanGroup> = Vec::new();
        let mut files = plan.files.clone();
        files.sort_by_key(|f| f.kind.order());

        for file in &files {
            let key = file.kind.key();
            match groups.iter_mut().find(|g| g.kind == key) {
                Some(group) => {
                    group.count += 1;
                    group.size += file.size;
                }
                None => groups.push(PlanGroup {
                    kind: key.to_string(),
                    label: file.kind.label().to_string(),
                    count: 1,
                    size: file.size,
                }),
            }
        }

        // 「已经装好了」按版本内容判定，不看目录名 —— 实例改成
        // 「我的整合」之后，这里仍然要给出「校验并修复」而不是又装一份。
        let versions = local::scan(&self.layout);
        let installed = match loader {
            Some(loader) => {
                let kind = loader.kind()?;
                local::has_loader(
                    &versions,
                    crate::loader::version_kind_of(kind),
                    &loader.version,
                    version_id,
                ) || local::load(&self.layout, &plan.version_id).is_ok()
            }
            None => {
                local::has_vanilla(&versions, version_id)
                    || local::load(&self.layout, &plan.version_id).is_ok()
            }
        };

        Ok(InstallPreview {
            version_id: plan.version_id.clone(),
            version_type: plan.resolved.version_type.clone(),
            release_time,
            java_major: plan.resolved.java_version.as_ref().map(|v| v.major_version),
            main_class: plan.resolved.main_class.clone(),
            total_count: files.len(),
            total_size: plan.total_size(),
            installed,
            groups,
        })
    }

    /// 生成完整清单。下载引擎会直接吃这个。
    ///
    /// 本地优先：`versions/<id>/<id>.json` 存在时不再去清单里找 ——
    /// 加载器版本（Fabric 等）根本不在 Mojang 清单里。
    pub async fn build_plan(&self, version_id: &str) -> Result<InstallPlan, AppError> {
        let resolved = self.resolve_version_any(version_id).await?;
        self.plan_from(version_id, &resolved).await
    }

    /// 由解析结果生成清单。
    ///
    /// 两种形态：
    /// - 完整安装：父版本没装，客户端 jar / 支持库 / 资源全都要下。
    /// - 增量安装（父版本已装，典型就是加载器版本）：只下自己声明的库。
    ///   客户端 jar 复用父版本那份（启动侧本来就会回落），资源和原生库也都在，
    ///   没必要让引擎去逐个校验一千多个资源文件 —— 那会让「秒装」变成「转半天」。
    pub(crate) async fn plan_from(
        &self,
        version_id: &str,
        resolved: &ResolvedVersion,
    ) -> Result<InstallPlan, AppError> {
        let detail = &resolved.detail;
        let env = Env::current();
        let layout = &self.layout;

        let parent_installed = resolved
            .parent
            .as_deref()
            .is_some_and(|parent| local::load(layout, parent).is_ok());

        let mut files: Vec<PendingFile> = Vec::new();

        // 1. 版本 JSON 自己：本地已有就别再下一遍（会把自己覆盖掉）
        if !layout.version_json(version_id).is_file() {
            if let Some(url) = &resolved.json_url {
                files.push(PendingFile {
                    kind: FileKind::VersionJson,
                    local: layout.version_json(version_id),
                    url: url.clone(),
                    sha1: None,
                    size: 0,
                });
            }
        }

        // 2. 客户端 jar：自己的目录里有 / 父版本那份还在（且可以复用），都不用下
        let own_jar = layout.version_jar(version_id).is_file();
        let parent_jar = resolved.reuse_jar
            || resolved
                .parent
                .as_deref()
                .is_some_and(|parent| local::client_jar(layout, parent).is_some());

        if !own_jar && !parent_jar {
            if let Some(client) = detail.downloads.as_ref().and_then(|d| d.client.as_ref()) {
                files.push(PendingFile {
                    kind: FileKind::ClientJar,
                    local: layout.version_jar(version_id),
                    url: client.url.clone(),
                    sha1: client.sha1.clone(),
                    size: client.size.unwrap_or(0),
                });
            }
        }

        // 3. 支持库 + natives（按当前平台过滤规则）
        //
        // 增量安装只看自己声明的库：继承来的那批属于父版本，父版本已经装好了。
        let libraries = if parent_installed {
            &resolved.own.libraries
        } else {
            &detail.libraries
        };

        for library in libraries.iter().filter(|l| l.is_applicable(&env)) {
            // `resolved_artifact` 而不是 `artifact`：Fabric 的库只有 maven 坐标
            // 没有 `downloads`，只认后者会把它们整批丢掉。
            if let Some(artifact) = library.resolved_artifact() {
                let relative = artifact
                    .path
                    .clone()
                    .or_else(|| maven_to_relative_path(&library.name));
                if let Some(relative) = relative {
                    files.push(PendingFile {
                        kind: FileKind::Library,
                        local: layout.libraries_dir().join(relative),
                        url: artifact.url.clone(),
                        sha1: artifact.sha1.clone(),
                        size: artifact.size.unwrap_or(0),
                    });
                }
            }

            if let Some(native) = library.native_artifact(&env.os_name) {
                let relative = native
                    .path
                    .clone()
                    .or_else(|| library.native_classifier(&env.os_name))
                    .unwrap_or_else(|| "native.jar".to_string());
                files.push(PendingFile {
                    kind: FileKind::Native,
                    local: layout.libraries_dir().join(relative),
                    url: native.url.clone(),
                    sha1: native.sha1.clone(),
                    size: native.size.unwrap_or(0),
                });
            }
        }

        // 4. 资源文件：先拿索引，再展开成逐个对象
        //    增量安装时资源全在（父版本装的），直接用 `assets/indexes/<name>.json` 那个文件就行
        let index = if parent_installed {
            resolved.own.asset_index.as_ref()
        } else {
            detail.asset_index.as_ref()
        };

        if let Some(index) = index {
            // 索引 JSON 本身也要落盘：启动时游戏自己会去读
            // `assets/indexes/<name>.json`，缺了就直接起不来。
            //
            // 文件名必须和启动侧用同一个函数算，否则会出现
            // 「装了 legacy.json，启动却去找 pre-1.6.json」。
            files.push(PendingFile {
                kind: FileKind::AssetIndex,
                local: layout
                    .assets_indexes_dir()
                    .join(format!("{}.json", crate::launch::asset_index_name(detail))),
                url: index.url.clone(),
                sha1: index.sha1.clone(),
                size: index.size.unwrap_or(0),
            });

            let cache_key = format!("assets:index:{}", index.id);
            let (index_file, _) = self
                .cache()
                .get_or_fetch(&cache_key, false, || async {
                    let (text, _) = self.fetch_meta_text(&index.url).await?;
                    AssetIndexFile::parse(&text)
                })
                .await?;

            for object in index_file.objects.values() {
                let Some(prefix) = object.hash.get(0..2) else {
                    continue;
                };
                files.push(PendingFile {
                    kind: FileKind::Asset,
                    local: layout.asset_object(&object.hash),
                    url: format!("{ASSET_BASE}/{prefix}/{}", object.hash),
                    sha1: Some(object.hash.clone()),
                    size: object.size,
                });
            }
        }

        // 同一路径只保留一次（父版本与子版本可能重复声明同一个库）
        files.sort_by_key(|f| f.local.clone());
        files.dedup_by_key(|f| f.local.clone());

        // 去重用的路径排序会留下「assets 排最前」的副作用 —— 那样取消安装时
        // 磁盘上只有一堆资源文件，连版本 JSON 都没有，看起来像什么都没装。
        // 这里重排成「先骨架后血肉」：json → jar → 库 → 资源。
        files.sort_by_key(|f| f.kind.order());

        Ok(InstallPlan {
            version_id: version_id.to_string(),
            resolved: resolved.detail.clone(),
            files,
        })
    }

    /// 沿 `inheritsFrom` 向上解析并合并：本地有就读本地，本地没有才查清单。
    ///
    /// 这样「给已装的原版加装加载器」不需要联网重下父版本，
    /// 而「全新安装」仍然能从清单把父版本拉下来。
    pub(crate) async fn resolve_version_any(
        &self,
        id: &str,
    ) -> Result<ResolvedVersion, AppError> {
        let local_json = self.layout.version_json(id);
        let (own, json_url) = if local_json.is_file() {
            (local::read_detail(&local_json)?, None)
        } else {
            let (url, _) = self.version_url(id).await?;
            let detail = self.fetch_version_json(id, &url).await?;
            (detail, Some(url))
        };

        // 合并会把 inherits_from 清掉，先记下第一个父版本（= 原版）
        let parent = own.inherits_from.clone();
        let mut merged = own.clone();
        let mut next = parent.clone();
        let mut visited: Vec<String> = vec![id.to_string()];

        // 防御：正常最多两三层，超过 8 层视为数据异常
        while let Some(parent_id) = next {
            if visited.contains(&parent_id) || visited.len() >= 8 {
                break;
            }
            visited.push(parent_id.clone());

            let parent_detail = match local::load(&self.layout, &parent_id) {
                Ok(detail) => detail,
                Err(_) => {
                    let (url, _) = self.version_url(&parent_id).await?;
                    self.fetch_version_json(&parent_id, &url).await?
                }
            };
            next = parent_detail.inherits_from.clone();
            merged = merge_versions(merged, parent_detail);
        }

        let reuse_jar = parent
            .as_deref()
            .is_some_and(|parent| local::client_jar(&self.layout, parent).is_some());

        Ok(ResolvedVersion {
            detail: merged,
            own,
            json_url,
            parent,
            reuse_jar,
        })
    }

    async fn fetch_version_json(
        &self,
        id: &str,
        url: &str,
    ) -> Result<VersionDetail, AppError> {
        let cache_key = format!("versions:json:{id}");
        let url = url.to_string();
        let (detail, _) = self
            .cache()
            .get_or_fetch(&cache_key, false, || async {
                let (text, _) = self.fetch_meta_text(&url).await?;
                VersionDetail::parse(&text)
            })
            .await?;
        Ok(detail)
    }

    /// 从（已缓存的）版本清单里查出某个版本的 JSON 地址。
    pub(crate) async fn version_url(&self, id: &str) -> Result<(String, Option<String>), AppError> {
        let view = self.version_list(false).await?;
        for groups in [&view.releases, &view.snapshots, &view.old] {
            for group in groups {
                for item in &group.items {
                    if item.id == id {
                        return Ok((item.url.clone(), item.release_time.clone()));
                    }
                }
            }
        }
        Err(
            AppError::not_found("找不到该版本", format!("版本清单里没有 {id}"))
                .with_hint("点击右上角「刷新」重新获取版本清单后再试"),
        )
    }
}
