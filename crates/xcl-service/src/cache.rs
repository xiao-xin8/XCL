//! 通用 TTL 内存缓存。
//!
//! 为后续所有接口铺路：任何"拉一次、短时间不会变"的数据都可以扔进来。
//!
//! 设计取向：
//! - 只做内存缓存，不落盘（落盘是另一个问题，等真有需求再说）
//! - 过期即删除，不做后台清理线程
//! - 存 `Arc<V>`，取出是克隆句柄而不是深拷贝
//!
//! 注意：这里不做"在途请求合并"，那属于前端层的职责（见 `api.ts`）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::Serialize;
use xcl_core::AppError;

pub struct TtlCache<V> {
    ttl: Duration,
    inner: Mutex<HashMap<String, Entry<V>>>,
}

struct Entry<V> {
    value: Arc<V>,
    stored_at: Instant,
}

impl<V> TtlCache<V> {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// 命中则返回句柄；过期则顺手清掉并返回 `None`。
    pub fn get(&self, key: &str) -> Option<Arc<V>> {
        let mut map = self.inner.lock().ok()?;
        match map.get(key) {
            Some(entry) if entry.stored_at.elapsed() < self.ttl => Some(Arc::clone(&entry.value)),
            Some(_) => {
                map.remove(key);
                None
            }
            None => None,
        }
    }

    /// 已缓存内容的年龄，用于告诉前端"这份数据多久前抓的"。
    pub fn age(&self, key: &str) -> Option<Duration> {
        let map = self.inner.lock().ok()?;
        map.get(key).map(|entry| entry.stored_at.elapsed())
    }

    pub fn put(&self, key: impl Into<String>, value: V) {
        if let Ok(mut map) = self.inner.lock() {
            map.insert(
                key.into(),
                Entry {
                    value: Arc::new(value),
                    stored_at: Instant::now(),
                },
            );
        }
    }

    pub fn invalidate(&self, key: &str) {
        if let Ok(mut map) = self.inner.lock() {
            map.remove(key);
        }
    }

    /// 配置变更（例如换下载源）时整体失效。
    pub fn clear(&self) {
        if let Ok(mut map) = self.inner.lock() {
            map.clear();
        }
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|map| map.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 统一的接口缓存入口。
///
/// 任何需要缓存的命令都可以直接用它，不需要各自维护字段：
///
/// ```ignore
/// let (data, from_cache) = state
///     .cache()
///     .get_or_fetch("some_key", force, || async { fetch_something().await })
///     .await?;
/// ```
///
/// 内部把值序列化成 `serde_json::Value` 保存，因此对值类型没有约束，
/// 只要求能序列化/反序列化 —— 加新接口时不必改缓存层。
pub struct ApiCache {
    inner: TtlCache<serde_json::Value>,
}

impl ApiCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: TtlCache::new(ttl),
        }
    }

    /// 命中就直接返回；否则调用 `fetch` 并写入缓存。
    ///
    /// 返回 `(值, 是否来自缓存)`。
    pub async fn get_or_fetch<T, F, Fut>(
        &self,
        key: &str,
        force: bool,
        fetch: F,
    ) -> Result<(T, bool), AppError>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, AppError>>,
    {
        if !force && let Some(value) = self.inner.get(key) {
            match serde_json::from_value::<T>((*value).clone()) {
                Ok(parsed) => return Ok((parsed, true)),
                // 结构变了（升级后旧缓存不兼容）：丢掉重新拉
                Err(_) => self.inner.invalidate(key),
            }
        }

        let value = fetch().await?;
        if let Ok(json) = serde_json::to_value(&value) {
            self.inner.put(key, json);
        }
        Ok((value, false))
    }

    pub fn invalidate(&self, key: &str) {
        self.inner.invalidate(key);
    }

    /// 配置变更（例如切换下载源）时整体失效。
    pub fn clear(&self) {
        self.inner.clear();
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_hits() {
        let cache: TtlCache<String> = TtlCache::new(Duration::from_secs(60));
        cache.put("k", "v".to_string());
        assert_eq!(cache.get("k").as_deref().map(String::as_str), Some("v"));
    }

    #[test]
    fn expires_after_ttl() {
        let cache: TtlCache<u8> = TtlCache::new(Duration::from_millis(30));
        cache.put("k", 1);
        std::thread::sleep(Duration::from_millis(60));
        assert!(cache.get("k").is_none());
        // 过期项会被顺手删掉
        assert!(cache.is_empty());
    }

    #[test]
    fn invalidate_and_clear() {
        let cache: TtlCache<u8> = TtlCache::new(Duration::from_secs(60));
        cache.put("a", 1);
        cache.put("b", 2);
        cache.invalidate("a");
        assert!(cache.get("a").is_none());
        assert!(cache.get("b").is_some());
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn age_is_reported() {
        let cache: TtlCache<u8> = TtlCache::new(Duration::from_secs(60));
        cache.put("k", 1);
        std::thread::sleep(Duration::from_millis(20));
        assert!(cache.age("k").unwrap() >= Duration::from_millis(15));
        assert!(cache.age("missing").is_none());
    }
}

#[cfg(test)]
mod api_cache_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Payload {
        n: u32,
    }

    #[tokio::test]
    async fn second_call_hits_cache() {
        let cache = ApiCache::new(Duration::from_secs(60));
        let calls = AtomicUsize::new(0);

        let first = cache
            .get_or_fetch("k", false, || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(Payload { n: 1 })
            })
            .await
            .unwrap();
        let second = cache
            .get_or_fetch("k", false, || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(Payload { n: 2 })
            })
            .await
            .unwrap();

        assert_eq!(first, (Payload { n: 1 }, false));
        assert_eq!(second, (Payload { n: 1 }, true), "第二次应命中缓存");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "fetch 只应被调用一次");
    }

    #[tokio::test]
    async fn force_bypasses_cache() {
        let cache = ApiCache::new(Duration::from_secs(60));
        cache
            .get_or_fetch("k", false, || async { Ok(Payload { n: 1 }) })
            .await
            .unwrap();

        let (value, from_cache) = cache
            .get_or_fetch("k", true, || async { Ok(Payload { n: 2 }) })
            .await
            .unwrap();

        assert_eq!(value, Payload { n: 2 });
        assert!(!from_cache);
    }

    #[tokio::test]
    async fn failed_fetch_is_not_cached() {
        let cache = ApiCache::new(Duration::from_secs(60));
        let err = cache
            .get_or_fetch::<Payload, _, _>("k", false, || async {
                Err(AppError::network("失败", "无法连接"))
            })
            .await;
        assert!(err.is_err());
        assert!(cache.is_empty(), "失败结果不该写进缓存");
    }

    #[tokio::test]
    async fn clear_invalidates_everything() {
        let cache = ApiCache::new(Duration::from_secs(60));
        cache
            .get_or_fetch("a", false, || async { Ok(Payload { n: 1 }) })
            .await
            .unwrap();
        assert_eq!(cache.len(), 1);
        cache.clear();
        assert!(cache.is_empty());
    }
}
