//! 依赖关系解析的纯逻辑部分。
//!
//! 两个平台的依赖字段完全不一样：
//! - Modrinth 挂在 version 上，`dependency_type` 是字符串
//!   （`required` / `optional` / `incompatible`，其余如 `embedded` 不参与安装）；
//! - CurseForge 挂在 file 上，`relationType` 是数字
//!   （1 内嵌库 / 2 可选 / 3 必需 / 4 工具 / 5 不兼容 / 6 Include），
//!   而且只给 `modId`，具体文件要再查一次。
//!
//! 这里只做「关系映射」和「按本地版本挑文件」，不碰网络、不碰磁盘。

use serde::Serialize;

use super::{targets, ResourceFileView};

/// 界面上需要区分的关系。其余类型（内嵌库 / 工具 / Include）直接不展示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DepRelation {
    Required,
    Optional,
    Incompatible,
}

impl DepRelation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Optional => "optional",
            Self::Incompatible => "incompatible",
        }
    }
}

/// 一条依赖（从文件里解析出来的原始信息）。
///
/// 会随文件列表一起序列化给前端：手动模式不用选安装位置就能显示
/// 「这个文件依赖谁」，具体依赖文件等选了目标版本再解析。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileDependency {
    /// 依赖的项目 id：Modrinth 是 base62，CurseForge 是数字串
    pub project_id: String,
    /// Modrinth 可能钉住具体文件版本；CurseForge 永远为 `None`
    pub version_id: Option<String>,
    pub relation: DepRelation,
}

/// Modrinth 的 `dependency_type` → 我们认的关系。
pub(crate) fn modrinth_relation(raw: &str) -> Option<DepRelation> {
    match raw {
        "required" => Some(DepRelation::Required),
        "optional" => Some(DepRelation::Optional),
        "incompatible" => Some(DepRelation::Incompatible),
        // embedded / unsupported / 其它未知值：不参与安装，也不在界面里列
        _ => None,
    }
}

/// CurseForge 的 `relationType` → 我们认的关系。
pub(crate) fn curseforge_relation(raw: u32) -> Option<DepRelation> {
    match raw {
        3 => Some(DepRelation::Required),
        2 => Some(DepRelation::Optional),
        5 => Some(DepRelation::Incompatible),
        // 1 内嵌库 / 4 工具 / 6 Include：不单独安装
        _ => None,
    }
}

/// 从候选文件里挑出「这个本地版本能用」的那一个。
///
/// 和自动模式同一套口径：MC 版本精确相等 + 加载器名对得上，
/// 优先 release → beta → alpha。挑不到返回 `None`（界面上显示「无兼容文件」）。
pub(crate) fn pick_dependency_file(
    files: &[ResourceFileView],
    game_version: &str,
    loader: Option<&str>,
) -> Option<ResourceFileView> {
    let matching = targets::matching_files(files, game_version, loader);
    for wanted in ["release", "beta", "alpha"] {
        if let Some(file) = matching
            .iter()
            .find(|file| file.release_type == wanted)
        {
            return Some((*file).clone());
        }
    }
    matching.first().map(|file| (*file).clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: &str, release_type: &str, versions: &[&str], loaders: &[&str]) -> ResourceFileView {
        ResourceFileView {
            dependency_count: 0,
            dependencies: Vec::new(),
            file_id: id.into(),
            version_number: id.into(),
            filename: format!("{id}.jar"),
            size: 1,
            sha1: None,
            url: "https://example.invalid/a.jar".into(),
            published: None,
            release_type: release_type.into(),
            game_versions: versions.iter().map(|v| v.to_string()).collect(),
            loaders: loaders.iter().map(|l| l.to_string()).collect(),
        }
    }

    #[test]
    fn modrinth_relations_are_mapped_and_noise_is_dropped() {
        assert_eq!(modrinth_relation("required"), Some(DepRelation::Required));
        assert_eq!(modrinth_relation("optional"), Some(DepRelation::Optional));
        assert_eq!(
            modrinth_relation("incompatible"),
            Some(DepRelation::Incompatible)
        );
        // 内嵌库不单独安装
        assert_eq!(modrinth_relation("embedded"), None);
        assert_eq!(modrinth_relation("whatever"), None);
    }

    #[test]
    fn curseforge_relations_are_mapped_and_noise_is_dropped() {
        assert_eq!(curseforge_relation(3), Some(DepRelation::Required));
        assert_eq!(curseforge_relation(2), Some(DepRelation::Optional));
        assert_eq!(curseforge_relation(5), Some(DepRelation::Incompatible));
        // 1 内嵌库 / 4 工具 / 6 Include 都不单独装
        assert_eq!(curseforge_relation(1), None);
        assert_eq!(curseforge_relation(4), None);
        assert_eq!(curseforge_relation(6), None);
    }

    #[test]
    fn pick_prefers_release_and_requires_exact_target() {
        let files = vec![
            file("beta", "beta", &["1.20.1"], &["fabric"]),
            file("release", "release", &["1.20.1"], &["fabric"]),
            file("other-loader", "release", &["1.20.1"], &["forge"]),
            file("other-game", "release", &["1.20.2"], &["fabric"]),
        ];
        let picked = pick_dependency_file(&files, "1.20.1", Some("fabric")).unwrap();
        assert_eq!(picked.file_id, "release");

        // 只有 beta / alpha 时退而求其次
        let only_beta = vec![file("b1", "beta", &["1.20.1"], &["fabric"])];
        assert_eq!(
            pick_dependency_file(&only_beta, "1.20.1", Some("fabric"))
                .unwrap()
                .file_id,
            "b1"
        );

        // 一个都对不上 → None
        assert!(pick_dependency_file(&files, "1.21.1", Some("fabric")).is_none());
    }
}
