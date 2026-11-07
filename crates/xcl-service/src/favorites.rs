//! 资源收藏（Mod / 资源包 / 光影 / 整合包）。
//!
//! 存 `<root>/favorites.toml`，只在本机，不做账号同步。
//!
//! 两个刻意的决定：
//!
//! 1. 每条都带一份卡片快照（`info`）。收藏页要能搜自己的名字、要能离线打开，
//!    只存 id 的话每次进来都得回平台批量查一遍（实测 1.1 秒），搜名字也无从谈起。
//!    快照只用来画卡片；点进去走的是实时接口，所以它过期了也不影响正确性。
//! 2. 唯一键是 `(source, kind, id)`，重复收藏就是刷新时间并挪到最前，
//!    不会出现两条一模一样的。`saved_ms` 同时是排序键。

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use xcl_core::{AppError, PathLayout};

use crate::resources::ModItemView;

/// 文件结构版本。以后要做跨设备同步时，靠它判断要不要迁移。
pub const SCHEMA: u32 = 1;

fn default_schema() -> u32 {
    SCHEMA
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteEntry {
    /// `modrinth` / `curseforge`
    pub source: String,
    /// `mod` / `resourcepack` / `shader` / `modpack`
    pub kind: String,
    pub id: String,
    /// 收藏时间（毫秒）。列表按它倒序，也就是「最近收藏的在最前」。
    pub saved_ms: u64,
    /// 卡片快照。只用于渲染，字段以收藏那一刻为准。
    pub info: ModItemView,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteStore {
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// 不叫 `items`：TOML 里 `[[item]]` 一行一条，读起来最干净。
    #[serde(default)]
    pub item: Vec<FavoriteEntry>,
}

impl Default for FavoriteStore {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            item: Vec::new(),
        }
    }
}

impl FavoriteStore {
    /// 读盘。文件不存在就是空收藏，不是错误 —— 第一次打开都没有它。
    /// 文件坏了也退回空表：收藏丢了比启动不了好，而且下次写入会重建。
    pub fn load(layout: &PathLayout) -> Self {
        let path = layout.favorites_file();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                if err.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(path = %path.display(), error = %err, "读取收藏失败，按空表处理");
                }
                return Self::default();
            }
        };
        toml::from_str(&text).unwrap_or_else(|err| {
            tracing::warn!(path = %path.display(), error = %err, "收藏文件损坏，按空表处理");
            Self::default()
        })
    }

    /// 落盘：临时文件 + rename，和 `config.toml` 同一套，
    /// 避免写到一半断电留下半截文件。
    pub fn save(&self, layout: &PathLayout) -> Result<(), AppError> {
        let path = layout.favorites_file();
        let text = toml::to_string_pretty(self)
            .map_err(|err| AppError::parse("收藏无法写入", err.to_string()))?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text).map_err(|err| AppError::io(&err, "写入收藏"))?;
        fs::rename(&tmp, &path).map_err(|err| AppError::io(&err, "替换收藏文件"))?;
        Ok(())
    }

    pub fn contains(&self, source: &str, kind: &str, id: &str) -> bool {
        self.item
            .iter()
            .any(|entry| entry.source == source && entry.kind == kind && entry.id == id)
    }

    /// 收藏 / 取消收藏。返回操作后「是否处于收藏状态」。
    ///
    /// 收藏时整个快照都换新：可能在看详情页时改过收藏，
    /// 那里的数据比旧快照新，没理由留着旧的。
    pub fn set(
        &mut self,
        source: &str,
        kind: &str,
        id: &str,
        info: Option<ModItemView>,
        on: bool,
    ) -> bool {
        self.item
            .retain(|entry| !(entry.source == source && entry.kind == kind && entry.id == id));

        if !on {
            return false;
        }
        let Some(mut info) = info else {
            // 没有快照就没法画卡片，宁可不收 —— 这属于调用方 bug，不该静默塞个空壳
            tracing::warn!(source, kind, id, "收藏缺少卡片快照，已忽略");
            return false;
        };
        // 唯一键和快照必须一致，否则收藏页的 key 会和列表页对不上
        info.source = source.to_string();
        self.item.insert(
            0,
            FavoriteEntry {
                source: source.to_string(),
                kind: kind.to_string(),
                id: id.to_string(),
                saved_ms: now_ms(),
                info,
            },
        );
        true
    }

    /// 收藏页的数据源：纯本地过滤，不发任何网络请求。
    ///
    /// - `kind`：`None` 或空串 = 不筛类型；
    /// - `query`：按名字（标题）忽略大小写包含；同时匹配作者，
    ///   因为经常记得「作者叫啥」而记不住完整标题。
    pub fn list(&self, kind: Option<&str>, query: Option<&str>) -> Vec<FavoriteEntry> {
        let kind = kind.map(str::trim).filter(|value| !value.is_empty());
        let needle = query
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_lowercase());

        let mut items: Vec<FavoriteEntry> = self
            .item
            .iter()
            .filter(|entry| kind.is_none_or(|want| entry.kind == want))
            .filter(|entry| {
                let Some(needle) = &needle else {
                    return true;
                };
                entry.info.title.to_lowercase().contains(needle)
                    || entry.info.author.to_lowercase().contains(needle)
            })
            .cloned()
            .collect();

        // 最近收藏在最前；时间相同时按 id 兜底，保证顺序稳定（不会每次刷新都变）
        items.sort_by(|a, b| {
            b.saved_ms
                .cmp(&a.saved_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        items
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0)
}

impl crate::AppState {
    /// 收藏页的数据：纯本地，不发网络请求。
    pub fn favorites_list(&self, kind: Option<&str>, query: Option<&str>) -> Vec<FavoriteEntry> {
        let store = self
            .favorites
            .read()
            .unwrap_or_else(|err| err.into_inner());
        store.list(kind, query)
    }

    /// 收藏 / 取消收藏。返回操作后的状态。
    ///
    /// `kind` 必须是四种之一：收藏页按类型筛，存进一个叫不出名字的类型
    /// 等于点了收藏却在任何筛选项里都找不到。
    pub fn favorite_set(
        &self,
        source: &str,
        kind: &str,
        id: &str,
        info: Option<ModItemView>,
        on: bool,
    ) -> Result<bool, AppError> {
        let kind = crate::resources::ResourceKind::parse(Some(kind)).ok_or_else(|| {
            AppError::unsupported("未知的资源类型", format!("不认识的类型：{kind}"))
        })?;
        if id.trim().is_empty() {
            return Err(AppError::unsupported("收藏失败", "项目 id 是空的"));
        }

        let mut store = self
            .favorites
            .write()
            .unwrap_or_else(|err| err.into_inner());
        let now_favorited = store.set(source, kind.modrinth_type(), id, info, on);
        store.save(&self.layout)?;
        Ok(now_favorited)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str, _kind: &str, title: &str) -> ModItemView {
        ModItemView {
            source: "modrinth".to_string(),
            id: id.to_string(),
            url: format!("https://modrinth.com/mod/{id}"),
            title: title.to_string(),
            description: String::new(),
            author: "someone".to_string(),
            icon_url: None,
            downloads: 0,
            updated: None,
            loaders: Vec::new(),
        }
    }

    fn store() -> FavoriteStore {
        let mut store = FavoriteStore::default();
        store.set("modrinth", "mod", "a", Some(sample("a", "mod", "Sodium")), true);
        store.set(
            "modrinth",
            "modpack",
            "b",
            Some(sample("b", "modpack", "Fabulously Optimized")),
            true,
        );
        store
    }

    #[test]
    fn keeps_only_one_entry_per_project() {
        let mut store = FavoriteStore::default();
        store.set("modrinth", "mod", "a", Some(sample("a", "mod", "Sodium")), true);
        store.set("modrinth", "mod", "a", Some(sample("a", "mod", "Sodium 2")), true);
        assert_eq!(store.item.len(), 1, "重复收藏不该产生第二条");
        assert_eq!(store.item[0].info.title, "Sodium 2", "快照应当换新");
    }

    #[test]
    fn unfavorite_removes_the_entry() {
        let mut store = store();
        assert!(store.contains("modrinth", "mod", "a"));
        assert!(!store.set("modrinth", "mod", "a", None, false));
        assert!(!store.contains("modrinth", "mod", "a"));
    }

    #[test]
    fn refuses_to_favorite_without_a_snapshot() {
        let mut store = FavoriteStore::default();
        assert!(!store.set("modrinth", "mod", "a", None, true));
        assert!(store.item.is_empty(), "没有快照就画不出卡片，不该收");
    }

    #[test]
    fn filters_by_kind_and_name() {
        let store = store();
        assert_eq!(store.list(Some("modpack"), None).len(), 1);
        assert_eq!(store.list(None, Some("sodium")).len(), 1, "名字忽略大小写");
        assert_eq!(store.list(None, Some("fab")).len(), 1);
        assert_eq!(store.list(None, Some("查无此项")).len(), 0);
        // 作者也能搜到
        assert_eq!(store.list(None, Some("SOMEONE")).len(), 2);
    }

    #[test]
    fn newest_first() {
        let store = store();
        let list = store.list(None, None);
        assert_eq!(list.len(), 2);
        assert!(list[0].saved_ms >= list[1].saved_ms);
    }

    fn temp_layout(tag: &str) -> PathLayout {
        let dir = std::env::temp_dir().join(format!("xcl-fav-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        PathLayout::new(dir.clone(), dir.join("cache"))
    }

    #[test]
    fn roundtrips_through_the_file() {
        let layout = temp_layout("roundtrip");
        let mut store = FavoriteStore::default();
        store.set(
            "modrinth",
            "modpack",
            "abc",
            Some(sample("abc", "modpack", "整合包 A")),
            true,
        );
        store.save(&layout).unwrap();
        assert!(layout.favorites_file().exists(), "应当落成 favorites.toml");

        let loaded = FavoriteStore::load(&layout);
        assert_eq!(loaded.item.len(), 1);
        assert_eq!(loaded.item[0].info.title, "整合包 A");
        assert_eq!(loaded.item[0].kind, "modpack");
        let _ = fs::remove_dir_all(layout.root());
    }

    #[test]
    fn missing_file_is_an_empty_list() {
        let layout = temp_layout("missing");
        assert!(FavoriteStore::load(&layout).item.is_empty(), "没有文件不该报错");
        let _ = fs::remove_dir_all(layout.root());
    }

    #[test]
    fn broken_file_falls_back_to_empty() {
        let layout = temp_layout("broken");
        fs::write(layout.favorites_file(), "这不是 toml [[[").unwrap();
        assert!(
            FavoriteStore::load(&layout).item.is_empty(),
            "收藏丢了比启动不了好，坏文件按空表处理"
        );
        let _ = fs::remove_dir_all(layout.root());
    }
}
