//! Fabric 元数据：版本列表、profile JSON 地址、实例命名。
//!
//! 只做纯逻辑，不做 IO。真实的请求由 `xcl-service` 用 `xcl-net` 发出去，
//! 镜像改写由 `xcl-source` 负责（`meta.fabricmc.net` → `{镜像}/fabric-meta`）。
//!
//! Fabric 是三个加载器里最轻的一个：官方直接给一份完整的 profile JSON，
//! 拿回来落盘就是版本文件，不需要安装器、不需要跑 JVM。

use serde::{Deserialize, Serialize};
use xcl_core::AppError;

/// Fabric 官方元数据根地址。
pub const META_BASE: &str = "https://meta.fabricmc.net";

/// 版本列表接口。
///
/// 刻意用不带 MC 版本的那个：带上 MC 版本会为每条记录附带一份
/// `launcherMeta.libraries`，响应从 39KB 涨到 600KB，而版本号集合是同一批。
pub fn loader_list_url() -> String {
    format!("{META_BASE}/v2/versions/loader")
}

/// 某个版本组合的 profile JSON。
pub fn profile_url(game: &str, loader: &str) -> String {
    format!("{META_BASE}/v2/versions/loader/{game}/{loader}/profile/json")
}

/// 加载器实例的版本名（同时是目录名和 JSON 文件名）。
///
/// 与 Fabric 自己在 profile JSON 里给的 `id` 保持一致，
/// 也和常见启动器的 `fabric-loader-<loader>-<mc>` 一致 —— 换启动器不会变成两个版本。
pub fn instance_id(loader: &str, game: &str) -> String {
    format!("fabric-loader-{loader}-{game}")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoaderVersion {
    pub version: String,
    #[serde(default)]
    pub stable: bool,
}

/// 解析版本列表。未知字段一律忽略（官方随时会加 `separator` / `build` 之类）。
pub fn parse_loader_versions(json: &str) -> Result<Vec<LoaderVersion>, AppError> {
    serde_json::from_str(json)
        .map_err(|e| AppError::parse("Fabric 版本列表解析失败", e.to_string()))
}

/// 一组同一 minor 的版本（`0.16.x`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderGroup {
    /// 分组键，同时是展示名：`0.16.x`
    pub key: String,
    /// 这一组是否包含全局最新版
    pub latest: bool,
    pub items: Vec<LoaderVersion>,
}

/// 把 `0.16.5` 归到 `0.16.x`。段数不够就原样保留。
fn minor_key(version: &str) -> String {
    let mut parts = version.split('.');
    match (parts.next(), parts.next()) {
        (Some(major), Some(minor)) if !major.is_empty() && !minor.is_empty() => {
            format!("{major}.{minor}.x")
        }
        _ => version.to_string(),
    }
}

/// 按 minor 分组，保持官方返回的顺序（最新在前）。
///
/// 分组在 Rust 侧算完再交给前端：前端只渲染，不做数据变换。
pub fn group_by_minor(versions: &[LoaderVersion]) -> Vec<LoaderGroup> {
    let mut groups: Vec<LoaderGroup> = Vec::new();

    for item in versions {
        let key = minor_key(&item.version);
        match groups.last_mut() {
            // 同组且相邻：直接追加（官方列表是倒序的，同 minor 一定连续）
            Some(last) if last.key == key => last.items.push(item.clone()),
            _ => groups.push(LoaderGroup {
                key,
                latest: false,
                items: vec![item.clone()],
            }),
        }
    }

    if let Some(first) = groups.first_mut() {
        first.latest = true;
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实响应片段（截取前几条，含一条 `stable: false` 的）。
    const LIST: &str = r#"[
        { "separator": ".", "build": 1, "maven": "net.fabricmc:fabric-loader:0.20.1", "version": "0.20.1", "stable": true },
        { "separator": ".", "build": 0, "maven": "net.fabricmc:fabric-loader:0.20.0", "version": "0.20.0", "stable": false },
        { "separator": ".", "build": 5, "maven": "net.fabricmc:fabric-loader:0.19.5", "version": "0.19.5", "stable": true },
        { "separator": ".", "build": 12, "maven": "net.fabricmc:fabric-loader:0.15.11", "version": "0.15.11", "stable": true }
    ]"#;

    #[test]
    fn parses_version_list() {
        let list = parse_loader_versions(LIST).unwrap();
        assert_eq!(list.len(), 4);
        assert_eq!(list[0].version, "0.20.1");
        assert!(list[0].stable);
        assert!(!list[1].stable, "0.20.0 官方标的是非稳定版");
    }

    #[test]
    fn groups_by_minor_and_keeps_order() {
        let list = parse_loader_versions(LIST).unwrap();
        let groups = group_by_minor(&list);

        assert_eq!(
            groups.iter().map(|g| g.key.as_str()).collect::<Vec<_>>(),
            ["0.20.x", "0.19.x", "0.15.x"]
        );
        // 0.20.1 和 0.20.0 归到同一组，且顺序不变
        assert_eq!(groups[0].items.len(), 2);
        assert_eq!(groups[0].items[1].version, "0.20.0");
        // 只有第一组带「最新」标记
        assert!(groups[0].latest);
        assert!(!groups[1].latest);
    }

    #[test]
    fn minor_key_handles_short_versions() {
        assert_eq!(minor_key("0.16.5"), "0.16.x");
        assert_eq!(minor_key("1.0"), "1.0.x");
        assert_eq!(minor_key("nightly"), "nightly");
    }

    #[test]
    fn instance_id_matches_fabric_naming() {
        // 必须和 Fabric 自己 profile JSON 里的 id 一致，否则同一个版本会装成两份
        assert_eq!(
            instance_id("0.16.5", "1.20.1"),
            "fabric-loader-0.16.5-1.20.1"
        );
    }

    #[test]
    fn urls_point_at_official_hosts() {
        // 官方地址；镜像改写交给 xcl-source
        assert_eq!(
            profile_url("1.20.1", "0.16.5"),
            "https://meta.fabricmc.net/v2/versions/loader/1.20.1/0.16.5/profile/json"
        );
        assert_eq!(
            loader_list_url(),
            "https://meta.fabricmc.net/v2/versions/loader"
        );
    }

    #[test]
    fn broken_json_is_reported_not_panicked() {
        let err = parse_loader_versions("[{").unwrap_err();
        assert!(!err.title.is_empty());
    }
}
