//! 版本清单。
//!
//! 这里体现「后端胖」：拉取、解析、按大版本分组、排序、标记最新都在 Rust 完成，
//! 前端拿到的结构可以直接渲染成折叠面板，不需要自己做任何分组计算。
//!
//! 另外这里也是「自动选源」唯一做探测的地方：拉清单时顺带测出官方源够不够快，
//! 结论会回灌给 `AppState`，影响之后的下载排序与并发数。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use xcl_core::{AppError, VersionKind, VersionManifest};
use xcl_net::RaceWinner;
use xcl_source::{Strategy, UrlKind, OFFICIAL_FAST_MS};

use crate::{mirror_label, AppState, ProbeResult};

/// 官方版本清单地址。镜像会由 `xcl-source` 自动改写。
pub const VERSION_MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

/// 「自动」档下先给官方源的独占窗口。
///
/// 超时 5 秒：既给官方一次机会，
/// 选 5 秒的理由：国内直连官方 manifest 通常在 1~3s，5 秒足够判定「能连上」，
/// 又不至于让连不上的人白等太久。
const OFFICIAL_HEDGE: Duration = Duration::from_secs(5);

/// 非探测场景下「镜像优先」时给镜像的独占窗口。
///
/// 镜像正常时 200ms 内就回来了，官方根本不会被惊动；镜像挂住时官方在 700ms
/// 后并发加入，比「顺序等满一遍重试退避」快两个数量级。
const META_HEDGE: Duration = Duration::from_millis(700);

/// 元数据请求的整体上限。清单约 4MB，30 秒还下不来就该换路了。
const META_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionItem {
    pub id: String,
    /// ISO 8601 时间串
    pub release_time: Option<String>,
    /// 该版本的详情 JSON 地址（下载层会用）
    pub url: String,
    /// 是否当前最新
    pub latest: bool,
    /// 图标分类。清单里只有原版，所以只会是原版 / 快照 / 远古三类
    pub kind: VersionKind,
    /// 本地是否已经装好。每次查询实时算，不进缓存 ——
    /// 清单可以缓存十分钟，但「装没装」是磁盘事实，缓存了就会显示过期状态
    pub installed: bool,
    /// 这个原版上已经装过哪些加载器（展示名，如 `["Fabric"]`、`["Forge","NeoForge"]`）。
    ///
    /// 装加载器是「一条自包含版本」，原版目录可能压根不存在，所以它
    /// 独立于 `installed`：列表里要能看到「这个版本其实已经有 Forge 了」。
    /// 判据是实例内容里的加载器库 + 原版号（`local::loader_instances`），
    /// 所以实例被改名也照样认得出。
    pub installed_loaders: Vec<String>,
}

/// 一组同大版本下的版本，例如 `1.21` → [1.21.1, 1.21, 1.21-pre1...]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionGroup {
    /// 分组标识，用作 React key
    pub key: String,
    /// 展示名，例如 `1.21`、`24w`
    pub label: String,
    /// 组内是否含最新版本
    pub latest: bool,
    pub items: Vec<VersionItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionListView {
    pub releases: Vec<VersionGroup>,
    pub snapshots: Vec<VersionGroup>,
    pub old: Vec<VersionGroup>,
    pub fools: Vec<VersionGroup>,
    pub release_count: usize,
    pub snapshot_count: usize,
    pub old_count: usize,
    pub fool_count: usize,
    /// 本次使用的下载源名称
    pub source: String,
    /// 抓取时间（毫秒时间戳），展示格式交给前端
    pub fetched_at_ms: u64,
    /// 数据是否来自缓存（用于前端友好提示）
    pub from_cache: bool,
}

/// 缓存键。和其它接口共存于同一个 `ApiCache` 里。
const CACHE_KEY: &str = "versions:manifest";

/// 下载源测试结果。`source` 是实际命中的源（官方 / BMCLAPI…）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceTestView {
    pub source: String,
    pub ms: u64,
}

impl AppState {
    /// 拉取版本清单。`force` 为 true 时跳过缓存（"刷新"按钮用）。
    pub async fn version_list(&self, force: bool) -> Result<VersionListView, AppError> {
        let (mut view, from_cache) = self
            .cache()
            .get_or_fetch(CACHE_KEY, force, || async { self.fetch_version_list().await })
            .await?;
        view.from_cache = from_cache;
        self.mark_installed(&mut view);
        Ok(view)
    }

    /// 下载源连通性测试：真拉一次版本清单（跳过缓存），返回耗时与命中的源。
    ///
    /// 只「连上」不够 —— 镜像返回 502 页面同样是 200。所以解析一遍清单才算通过：
    /// 这样测的是「这个源现在能不能用」，而不是「TCP 通不通」。
    pub async fn test_source(&self) -> Result<SourceTestView, AppError> {
        let started = std::time::Instant::now();
        let (text, source) = self.fetch_manifest_text().await?;
        VersionManifest::parse(&text)?;
        Ok(SourceTestView {
            source,
            ms: started.elapsed().as_millis() as u64,
        })
    }

    /// 给列表里的每个版本打上「已安装」标记。
    ///
    /// 刻意放在缓存之后：清单本身能缓存，安装状态不能 —— 刚装完一个版本
    /// 回头刷新列表，如果读到的是缓存里的旧标记，会以为没装上。
    fn mark_installed(&self, view: &mut VersionListView) {
        // 已装实例整份扫一次：判定只看内容（加载器库 + 原版号），
        // 不看目录名 —— 把 `fabric-loader-0.20.1-26.2` 改成「我的整合」，
        // 1.20.1 这一行照样要显示「已安装」。
        let versions = crate::local::scan(&self.layout);
        if versions.is_empty() {
            return;
        }

        for group in view
            .releases
            .iter_mut()
            .chain(view.snapshots.iter_mut())
            .chain(view.old.iter_mut())
            .chain(view.fools.iter_mut())
        {
            for item in &mut group.items {
                item.installed = crate::local::has_vanilla(&versions, &item.id)
                    // 名字恰好等于原版号的也算（历史数据 / 手动整理的目录）
                    || versions
                        .iter()
                        .any(|version| version.valid && version.id == item.id);
                item.installed_loaders = crate::local::loader_instances(&versions, &item.id)
                    .into_iter()
                    .map(|(label, _)| label.to_string())
                    .collect();
            }
        }
    }

    /// 真正的网络 + 解析逻辑，不含缓存。
    async fn fetch_version_list(&self) -> Result<VersionListView, AppError> {
        let (text, source) = self.fetch_manifest_text().await?;
        let manifest = VersionManifest::parse(&text)?;

        let latest_release = manifest.latest.release.clone();
        let latest_snapshot = manifest.latest.snapshot.clone();

        let mut releases: Vec<VersionGroup> = Vec::new();
        let mut snapshots: Vec<VersionGroup> = Vec::new();
        let mut old: Vec<VersionGroup> = Vec::new();
        let mut fools: Vec<VersionGroup> = Vec::new();
        let mut release_count = 0usize;
        let mut snapshot_count = 0usize;
        let mut old_count = 0usize;
        let mut fool_count = 0usize;

        for version in manifest.versions {
            let latest = version.id == latest_release || version.id == latest_snapshot;
            let item = VersionItem {
                id: version.id.clone(),
                release_time: version.release_time.clone(),
                url: version.url,
                latest,
                kind: VersionKind::from_manifest_type(&version.version_type),
                // 真实值在 version_list() 里统一回填
                installed: false,
                installed_loaders: Vec::new(),
            };

            match version.version_type.as_str() {
                "release" => {
                    release_count += 1;
                    let (key, label) = release_group(&version.id);
                    push_grouped(&mut releases, key, label, item);
                }
                "snapshot" => {
                    // 愚人节版本在清单里仍是 snapshot，按固定 ID / 4 月 1 日挑出来
                    if is_fool_version(&version.id, version.release_time.as_deref()) {
                        fool_count += 1;
                        let (key, label) = fool_group(version.release_time.as_deref());
                        push_grouped(&mut fools, key, label, item);
                    } else {
                        snapshot_count += 1;
                        let (key, label) = snapshot_group(&version.id);
                        push_grouped(&mut snapshots, key, label, item);
                    }
                }
                "old_alpha" | "old_beta" => {
                    old_count += 1;
                    let (key, label) = if version.version_type == "old_alpha" {
                        ("alpha".to_string(), "远古 Alpha".to_string())
                    } else {
                        ("beta".to_string(), "远古 Beta".to_string())
                    };
                    push_grouped(&mut old, key, label, item);
                }
                _ => {}
            }
        }

        Ok(VersionListView {
            releases,
            snapshots,
            old,
            fools,
            release_count,
            snapshot_count,
            old_count,
            fool_count,
            source,
            fetched_at_ms: now_ms(),
            from_cache: false,
        })
    }

    /// 拉版本清单正文，返回 (正文, 实际使用的源名称)。
    ///
    /// 只是 [`Self::fetch_meta_text`] 的一个具体地址。
    async fn fetch_manifest_text(&self) -> Result<(String, String), AppError> {
        self.fetch_meta_text(VERSION_MANIFEST_URL).await
    }

    /// 拉任意元数据正文，返回 (正文, 实际使用的源名称)。
    ///
    /// 所有元数据都必须走这里（版本清单、Fabric 版本列表、profile JSON、
    /// 版本 JSON 原文、资源索引、Java 运行时…）。之前这些地方各自
    /// `http().get_text()`，等于绕过了竞速与测速记录：首选源挂住时要一个源一个源
    /// 地等重试退避，看起来就是「加载器列表一直转圈」，和版本清单的速度完全是
    /// 两回事。
    ///
    /// 没配镜像时只有官方，无从竞速；配了镜像一律错峰竞速：
    /// - 「自动」档第一次：官方独占 5 秒，超时再并发镜像。这 5 秒同时就是测速，
    ///   不额外花流量
    /// - 之后按测速结果决定谁先起跑，另一个晚 [`META_HEDGE`] 加入，谁先回用谁
    pub(crate) async fn fetch_meta_text(
        &self,
        url: &str,
    ) -> Result<(String, String), AppError> {
        let policy = self.http_policy();
        let probing = policy.strategy == Strategy::Auto && self.probe_result().is_none();

        // 没有配镜像（或该地址不参与镜像）：只有官方，无从竞速
        let Some(mirror_url) = policy.mirror_only(UrlKind::Meta, url) else {
            let text = self.http().get_text(UrlKind::Meta, url).await?;
            return Ok((text, "官方源".to_string()));
        };

        // 已知官方够快：官方先起跑，镜像晚 `META_HEDGE` 再加入。
        //
        // 注意这里不退化成「只用官方顺序拉」：三档全是错峰竞速，
        // 官方优先那一档也只让官方先跑
        // 30ms 而已。顺序拉在「首选源挂住」时要等满 `单次 60 秒 × 3 次重试`
        // 才轮到另一个源，对「点开就要出」的列表来说就是卡死。
        let (primary, secondary, hedge) = if probing {
            (url.to_string(), mirror_url.clone(), OFFICIAL_HEDGE)
        } else if policy.official_first() {
            (url.to_string(), mirror_url.clone(), META_HEDGE)
        } else {
            (mirror_url.clone(), url.to_string(), META_HEDGE)
        };

        let outcome = self
            .http()
            .race_text(&primary, &secondary, hedge, META_TIMEOUT)
            .await?;

        // 只有探测那次才写测速结果，其它情况不要污染它。
        // 「官方自己赢，且耗时低于阈值」才算官方够快；官方没赢说明它至少慢了整个窗口
        if probing {
            let official_fast = outcome.winner == RaceWinner::Primary
                && outcome
                    .primary_ms
                    .is_some_and(|ms| ms < OFFICIAL_FAST_MS);
            self.record_probe(ProbeResult {
                official_fast,
                official_ms: outcome.primary_ms,
            });
        }

        let winner_url = match outcome.winner {
            RaceWinner::Primary => primary.as_str(),
            RaceWinner::Secondary => secondary.as_str(),
        };
        let label = source_label(&policy, winner_url);
        let text = String::from_utf8(outcome.body).map_err(|e| {
            AppError::parse("元数据不是有效的 UTF-8", e.to_string())
                .with_hint("可能是下载源返回了错误页面")
        })?;
        Ok((text, label))
    }
}

/// 按实际使用的地址给出展示名。
fn source_label(policy: &xcl_source::SourcePolicy, url: &str) -> String {
    match policy.mirror.base() {
        Some(base) if url.starts_with(base) => mirror_label(policy),
        _ => "官方源".to_string(),
    }
}

/// 把条目塞进对应分组；分组不存在则按出现顺序创建（清单本身已按时间倒序）。
fn push_grouped(groups: &mut Vec<VersionGroup>, key: String, label: String, item: VersionItem) {
    let is_latest = item.latest;

    if let Some(group) = groups.iter_mut().find(|g| g.key == key) {
        if is_latest {
            group.latest = true;
        }
        group.items.push(item);
        return;
    }

    groups.push(VersionGroup {
        key,
        label,
        latest: is_latest,
        items: vec![item],
    });
}

/// 正式版按 `主.次` 分组：`1.21.1` → `1.21`，`1.21` → `1.21`。
fn release_group(id: &str) -> (String, String) {
    let mut parts = id.split('.');
    match (parts.next(), parts.next()) {
        (Some(major), Some(minor)) if !major.is_empty() && !minor.is_empty() => {
            // minor 可能带后缀（如 `21-rc1`），只取开头的数字
            let minor_digits: String = minor.chars().take_while(|c| c.is_ascii_digit()).collect();
            if minor_digits.is_empty() {
                (id.to_string(), id.to_string())
            } else {
                let key = format!("{major}.{minor_digits}");
                (key.clone(), key)
            }
        }
        _ => (id.to_string(), id.to_string()),
    }
}

/// 快照按年份周分组：`24w33a` → `24w`。识别不出就归到「其他」。
fn snapshot_group(id: &str) -> (String, String) {
    match id.find('w') {
        Some(idx) if idx == 2 && id[..idx].chars().all(|c| c.is_ascii_digit()) => {
            let key = id[..=idx].to_string();
            (key.clone(), key)
        }
        _ => ("other".to_string(), "其他".to_string()),
    }
}

/// 认定愚人节版本的方式：
/// 先匹配固定 ID 名单，其余按发布时间判断 —— 换算到 UTC+2 后是不是 4 月 1 日。
fn is_fool_version(id: &str, release_time: Option<&str>) -> bool {
    const FOOL_IDS: &[&str] = &[
        "20w14infinite",
        "20w14∞",
        "3d shareware v1.34",
        "1.rv-pre1",
        "15w14a",
        "2.0",
        "22w13oneblockatatime",
        "23w13a_or_b",
        "24w14potato",
        "25w14craftmine",
        "26w14a",
    ];

    if FOOL_IDS.contains(&id.to_ascii_lowercase().as_str()) {
        return true;
    }
    release_time.is_some_and(is_april_first_utc_plus_2)
}

/// 清单时间固定是 UTC（`2015-04-01T13:00:00+00:00`）。
/// 按 UTC+2 看是不是 4/1，所以 UTC 的 3/31 22 点起算 4/1，
/// 4/1 22 点起已经是 4/2。
fn is_april_first_utc_plus_2(release_time: &str) -> bool {
    let Some((date, rest)) = release_time.split_once('T') else {
        return false;
    };
    let hour = rest.get(0..2).and_then(|h| h.parse::<u32>().ok());
    let mut parts = date.split('-');
    let month = parts.nth(1).and_then(|m| m.parse::<u32>().ok());
    let day = parts.next().and_then(|d| d.parse::<u32>().ok());

    match (month, day, hour) {
        (Some(4), Some(1), Some(hour)) => hour < 22,
        (Some(3), Some(31), Some(hour)) => hour >= 22,
        _ => false,
    }
}

/// 愚人节版按发布年份分组：`2024`。缺时间就归到「其他」。
fn fool_group(release_time: Option<&str>) -> (String, String) {
    match release_time.and_then(|t| t.get(0..4)) {
        Some(year) if year.chars().all(|c| c.is_ascii_digit()) => {
            (year.to_string(), year.to_string())
        }
        _ => ("other".to_string(), "其他".to_string()),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_grouping_takes_major_minor() {
        assert_eq!(release_group("1.21.1"), ("1.21".into(), "1.21".into()));
        assert_eq!(release_group("1.21"), ("1.21".into(), "1.21".into()));
        assert_eq!(release_group("1.7.10"), ("1.7".into(), "1.7".into()));
        assert_eq!(release_group("26.3"), ("26.3".into(), "26.3".into()));
    }

    #[test]
    fn snapshot_grouping_takes_year_week_prefix() {
        assert_eq!(snapshot_group("24w33a"), ("24w".into(), "24w".into()));
        assert_eq!(snapshot_group("13w01a"), ("13w".into(), "13w".into()));
        assert_eq!(snapshot_group("1.21-pre1"), ("other".into(), "其他".into()));
    }

    #[test]
    fn fool_versions_match_pcl_rules() {
        // 固定 ID 名单
        assert!(is_fool_version("24w14potato", None));
        assert!(is_fool_version("23w13a_or_b", None));
        assert!(is_fool_version("3d shareware v1.34", None));
        // 普通快照
        assert!(!is_fool_version("24w33a", Some("2024-08-15T12:00:00+00:00")));
        // UTC 3/31 晚间 = UTC+2 的 4/1
        assert!(is_fool_version("xxw00a", Some("2026-03-31T23:00:00+00:00")));
        // UTC 4/1 晚间 = UTC+2 的 4/2，不算
        assert!(!is_fool_version("xxw00a", Some("2026-04-01T23:00:00+00:00")));
    }

    #[test]
    fn fool_grouping_uses_release_year() {
        assert_eq!(
            fool_group(Some("2024-04-01T13:00:00+00:00")),
            ("2024".into(), "2024".into())
        );
        assert_eq!(fool_group(None), ("other".into(), "其他".into()));
    }

    #[test]
    fn grouping_preserves_order_and_marks_latest() {
        let mut groups = Vec::new();
        let mk = |id: &str, latest: bool| VersionItem {
            id: id.to_string(),
            release_time: None,
            url: String::new(),
            latest,
            kind: VersionKind::Unknown,
            installed: false,
            installed_loaders: Vec::new(),
        };
        push_grouped(&mut groups, "1.21".into(), "1.21".into(), mk("1.21.1", false));
        push_grouped(&mut groups, "1.20".into(), "1.20".into(), mk("1.20.6", false));
        push_grouped(&mut groups, "1.21".into(), "1.21".into(), mk("1.21", true));

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].key, "1.21");
        assert_eq!(groups[0].items.len(), 2);
        assert!(groups[0].latest);
        assert!(!groups[1].latest);
    }
}
