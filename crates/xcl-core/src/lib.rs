//! XCL 领域模型。
//!
//! 本 crate 的硬约束：不依赖网络、不依赖文件系统、不使用 async。
//! 所有逻辑必须是纯函数或纯数据变换，以便完整单测。
//!
//! 计划容纳：
//! - `error`：统一错误模型（已完成）
//! - `path`：目录布局与 maven 坐标到本地路径的推导（已完成）
//! - `config`：配置数据模型（已完成）
//! - `version`：版本 JSON（含 `inheritsFrom` 合并、`libraries` / `assets` / `arguments`）
//! - `rule`：`rules` 判定（os / features）
//! - `launch`：启动参数拼装（占位符替换、去重、Java 需求推断）
//! - `java_runtime`：Mojang 官方 Java 运行时清单（组件选择 + 文件展开）

pub mod config;
pub mod error;
pub mod java_runtime;
pub mod kind;
pub mod launch;
pub mod path;
pub mod rule;
pub mod version;

pub use config::{CloseAction, Config, UpdateChannel, UpdateConfig};
pub use error::{AppError, ErrorCode};
pub use kind::{detect_kind, kind_from_id, loader_stamp, VersionKind};
pub use java_runtime::{
    choose_component, component_java_major, component_label, components_for_java, RuntimeFileEntry,
    RuntimeIndex, RuntimeManifest,
};
pub use launch::{
    build_command, classpath_separator, dedupe_args, expand_arguments, java_requirement,
    offline_uuid, split_legacy_arguments, CommandLine, JavaRequirement, LaunchPaths, LaunchRequest,
    PlayerProfile, WindowOptions,
};
pub use path::{maven_to_relative_path, PathLayout};
pub use rule::{rules_match, Env, Rule, RuleAction};
pub use version::{
    library_dedupe_key, merge_version_json, merge_versions, AssetIndexFile, AssetObject,
    Library, VersionDetail, VersionManifest, VersionSummary,
};
