//! 版本 JSON 里的 `rules` 判定。
//!
//! 这套规则同时作用于 `libraries` 和 `arguments` 两处完全不同的逻辑，
//! 写歪了会出现「游戏能启动但缺库」这类最难查的 bug，所以单独成模块并写满测试。
//!
//! 语义（与官方行为一致）：
//! - 没有 `rules` 字段 → 允许
//! - 有 `rules` → 默认不允许，按顺序逐条判定，最后一条匹配的规则决定结果

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Allow,
    Disallow,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OsRule {
    /// `windows` / `osx` / `linux`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 系统版本。原版 JSON 里极少出现，目前按精确匹配处理。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// `x86` / `x86_64` / `arm64`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub action: RuleAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<OsRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<HashMap<String, bool>>,
}

/// 当前运行环境。features 由上层按启动配置填入
/// （如 `has_custom_resolution`、`is_demo_user`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Env {
    /// 统一成 Mojang 的写法：`windows` / `osx` / `linux`
    pub os_name: String,
    pub os_version: String,
    pub os_arch: String,
    pub features: HashMap<String, bool>,
}

impl Env {
    /// 用当前平台构造环境。
    pub fn current() -> Self {
        Self {
            os_name: mojang_os_name(),
            // 系统版本号需要额外依赖才能拿到；版本规则在原版 JSON 中几乎不存在
            os_version: String::new(),
            os_arch: std::env::consts::ARCH.to_string(),
            features: HashMap::new(),
        }
    }

    pub fn with_feature(mut self, key: impl Into<String>, value: bool) -> Self {
        self.features.insert(key.into(), value);
        self
    }
}

/// `std::env::consts::OS` 的 `macos` 要映射成 Mojang 的 `osx`。
pub fn mojang_os_name() -> String {
    match std::env::consts::OS {
        "macos" => "osx".to_string(),
        other => other.to_string(),
    }
}

/// 判定一组规则是否放行。
pub fn rules_match(rules: Option<&[Rule]>, env: &Env) -> bool {
    let Some(rules) = rules else {
        return true;
    };
    if rules.is_empty() {
        return true;
    }

    let mut allowed = false;
    for rule in rules {
        if single_rule_matches(rule, env) {
            allowed = matches!(rule.action, RuleAction::Allow);
        }
    }
    allowed
}

fn single_rule_matches(rule: &Rule, env: &Env) -> bool {
    if let Some(os) = &rule.os {
        if let Some(name) = &os.name
            && !name.eq_ignore_ascii_case(&env.os_name)
        {
            return false;
        }
        if let Some(arch) = &os.arch
            && !arch.eq_ignore_ascii_case(&env.os_arch)
        {
            return false;
        }
        if let Some(version) = &os.version
            && version != &env.os_version
        {
            return false;
        }
    }

    if let Some(features) = &rule.features {
        for (key, expected) in features {
            let actual = env.features.get(key).copied().unwrap_or(false);
            if actual != *expected {
                return false;
            }
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(os: &str) -> Env {
        Env {
            os_name: os.to_string(),
            os_version: String::new(),
            os_arch: "x86_64".to_string(),
            features: HashMap::new(),
        }
    }

    fn allow_on(os: &str) -> Rule {
        Rule {
            action: RuleAction::Allow,
            os: Some(OsRule {
                name: Some(os.to_string()),
                ..Default::default()
            }),
            features: None,
        }
    }

    #[test]
    fn no_rules_means_allowed() {
        let e = env("windows");
        assert!(rules_match(None, &e));
        assert!(rules_match(Some(&[]), &e));
    }

    #[test]
    fn allow_windows_only_matches_windows() {
        let rules = vec![allow_on("windows")];
        assert!(rules_match(Some(&rules), &env("windows")));
        assert!(!rules_match(Some(&rules), &env("linux")));
    }

    #[test]
    fn disallow_osx_only_still_blocks_non_matching() {
        // 只有一条 disallow 规则时，默认是不允许 —— 这是官方语义，
        // 很多实现搞错成「默认允许」，会导致多装/少装库
        let rules = vec![Rule {
            action: RuleAction::Disallow,
            os: Some(OsRule {
                name: Some("osx".to_string()),
                ..Default::default()
            }),
            features: None,
        }];
        assert!(!rules_match(Some(&rules), &env("windows")));
    }

    #[test]
    fn later_rule_overrides_earlier() {
        let rules = vec![
            Rule {
                action: RuleAction::Allow,
                os: None,
                features: None,
            },
            Rule {
                action: RuleAction::Disallow,
                os: Some(OsRule {
                    name: Some("windows".to_string()),
                    ..Default::default()
                }),
                features: None,
            },
        ];
        assert!(!rules_match(Some(&rules), &env("windows")));
        assert!(rules_match(Some(&rules), &env("linux")));
    }

    #[test]
    fn feature_rules_respect_env() {
        let rules = vec![Rule {
            action: RuleAction::Allow,
            os: None,
            features: Some(HashMap::from([("has_custom_resolution".to_string(), true)])),
        }];
        assert!(!rules_match(Some(&rules), &env("windows")));
        assert!(rules_match(
            Some(&rules),
            &env("windows").with_feature("has_custom_resolution", true)
        ));
    }

    #[test]
    fn arch_rule_respected() {
        let rules = vec![Rule {
            action: RuleAction::Allow,
            os: Some(OsRule {
                arch: Some("x86".to_string()),
                ..Default::default()
            }),
            features: None,
        }];
        assert!(!rules_match(Some(&rules), &env("windows")));
    }
}
