//! 更新日志 `changelog.json`。
//!
//! 只用于展示，不参与更新校验：更新流程仍然只认 `update.json` 和它的签名。
//! 这里不做签名，但同样限制形状和长度 —— 数据来自网络，坏了不能把界面拖垮。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use xcl_core::AppError;

use crate::manifest::{
    Channel, MAX_CHANGELOG_LINE_CHARS, MAX_CHANGELOG_LINES, MAX_NOTES_CHARS, MAX_TITLE_CHARS,
    parse_version,
};

/// 更新日志格式版本。结构变了才 +1。
pub const SCHEMA: u32 = 1;

/// 最多接受多少个历史版本。超出说明文件被塞了东西，不是正常发布。
pub const MAX_RELEASES: usize = 200;

/// 一条发布记录。字段与 `update.json` 的对应部分一致，方便发布脚本直接搬运。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChangelogRelease {
    pub version: String,
    pub channel: Channel,
    pub published_at: Option<String>,
    pub title: String,
    pub notes: String,
    pub changelog: Vec<String>,
    pub force: bool,
}

/// 整个更新日志文件。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Changelog {
    pub schema: u32,
    pub releases: Vec<ChangelogRelease>,
}

impl Changelog {
    /// 解析并整理：丢掉版本号解析不了或内容超限的条目，按版本从新到旧排序。
    ///
    /// 单条坏了只丢那一条，不让整份历史打不开 —— 一条写错不该连累其它版本。
    pub fn parse(text: &str) -> Result<Self, AppError> {
        let parsed: Changelog = serde_json::from_str(text).map_err(|err| {
            AppError::parse("更新日志格式错误", format!("无法解析更新日志：{err}"))
                .with_hint("稍后重试；若持续失败，可能是服务器上的文件有问题")
                .with_retryable(true)
        })?;

        if parsed.schema != SCHEMA {
            return Err(AppError::parse(
                "更新日志格式不受支持",
                format!(
                    "这个版本只认识 schema {SCHEMA} 的更新日志，服务器给的是 {}",
                    parsed.schema
                ),
            ));
        }
        if parsed.releases.len() > MAX_RELEASES {
            return Err(AppError::parse(
                "更新日志内容异常",
                format!(
                    "更新日志包含 {} 个版本，超过上限 {MAX_RELEASES}",
                    parsed.releases.len()
                ),
            ));
        }

        let mut seen = HashSet::new();
        let mut releases: Vec<ChangelogRelease> = parsed
            .releases
            .into_iter()
            .filter(is_reasonable)
            .filter(|release| seen.insert(release.version.clone()))
            .collect();

        releases.sort_by(|a, b| parse_version(&b.version).cmp(&parse_version(&a.version)));

        Ok(Self {
            schema: SCHEMA,
            releases,
        })
    }
}

fn is_reasonable(release: &ChangelogRelease) -> bool {
    if parse_version(&release.version).is_none() {
        return false;
    }
    if release.notes.chars().count() > MAX_NOTES_CHARS {
        return false;
    }
    if release.title.chars().count() > MAX_TITLE_CHARS {
        return false;
    }
    if release.changelog.len() > MAX_CHANGELOG_LINES {
        return false;
    }
    release
        .changelog
        .iter()
        .all(|line| line.chars().count() <= MAX_CHANGELOG_LINE_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str) -> ChangelogRelease {
        ChangelogRelease {
            version: version.to_string(),
            channel: Channel::Stable,
            published_at: None,
            title: String::new(),
            notes: String::new(),
            changelog: Vec::new(),
            force: false,
        }
    }

    #[test]
    fn sorts_newest_first() {
        let text = r#"{
            "schema": 1,
            "releases": [
                { "version": "0.2.2" },
                { "version": "0.3.0" },
                { "version": "0.1.0" }
            ]
        }"#;
        let parsed = Changelog::parse(text).unwrap();
        let versions: Vec<&str> = parsed.releases.iter().map(|r| r.version.as_str()).collect();
        assert_eq!(versions, ["0.3.0", "0.2.2", "0.1.0"]);
    }

    #[test]
    fn drops_duplicate_and_invalid_versions() {
        let text = r#"{
            "schema": 1,
            "releases": [
                { "version": "0.3.0", "notes": "first" },
                { "version": "0.3.0", "notes": "second" },
                { "version": "not-a-version" }
            ]
        }"#;
        let parsed = Changelog::parse(text).unwrap();
        assert_eq!(parsed.releases.len(), 1);
        assert_eq!(parsed.releases[0].notes, "first");
    }

    #[test]
    fn rejects_unknown_schema() {
        let text = r#"{ "schema": 99, "releases": [] }"#;
        assert!(Changelog::parse(text).is_err());
    }

    #[test]
    fn drops_overlong_entries() {
        let long_line = "x".repeat(MAX_CHANGELOG_LINE_CHARS + 1);
        let text = format!(
            r#"{{ "schema": 1, "releases": [
                {{ "version": "0.3.0", "changelog": ["{long_line}"] }},
                {{ "version": "0.2.2" }}
            ] }}"#
        );
        let parsed = Changelog::parse(&text).unwrap();
        assert_eq!(parsed.releases.len(), 1);
        assert_eq!(parsed.releases[0].version, "0.2.2");
    }

    #[test]
    fn empty_history_is_valid() {
        let parsed = Changelog::parse(r#"{ "schema": 1, "releases": [] }"#).unwrap();
        assert!(parsed.releases.is_empty());
    }

    #[test]
    fn release_struct_round_trips() {
        let mut item = release("0.3.0");
        item.published_at = Some("2026-11-08T02:08:49Z".to_string());
        item.force = true;
        let text = serde_json::to_string(&item).unwrap();
        assert!(text.contains("\"publishedAt\""));
        assert!(text.contains("\"force\":true"));
    }
}
