//! 待应用的更新。
//!
//! 下载完不等于更新完。中间隔着「什么时候退出」这件事 —— 所以下载成功后
//! 在磁盘上留一份记录，下次启动时 `main()` 一进来就知道：手上有活没干完。
//!
//! 为什么必须落盘而不是内存：下载完直接关掉程序是最常见的路径，
//! 内存里的状态一关就没，下次启动包还在、记录没了，等于白下。
//!
//! 记录里存版本号，是为了判断「这个包还要不要」：
//! - `version > 当前版本` → 更新还没完成，包必须留着
//! - `version <= 当前版本` → 已经更新过了（或装了更高版本），包和记录都可以清

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::manifest::parse_version;

const FILE_NAME: &str = "pending.json";

/// 一份「已经下好、等着替换」的更新。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingUpdate {
    /// 要更新到的版本
    pub version: String,
    /// 更新包绝对路径
    pub file: PathBuf,
    pub size: u64,
    pub sha256: String,
    /// 界面上展示的版本标题（可选）
    #[serde(default)]
    pub title: String,
}

impl PendingUpdate {
    /// 这份更新是不是还没完成（目标版本比当前高）。
    pub fn is_pending(&self, current: &str) -> bool {
        let Some(target) = parse_version(&self.version) else {
            // 版本号坏了：当它无效，交给调用方清掉
            return false;
        };
        let Some(current) = parse_version(current) else {
            return false;
        };
        target > current
    }
}

/// 记录文件的位置：`<程序目录>/data/update/pending.json`。
pub fn path(work_dir: &Path) -> PathBuf {
    work_dir.join(FILE_NAME)
}

/// 读记录。文件缺失 / 格式坏掉都返回 `None` —— 这只是个加速提示，
/// 读不出来就当没有，正常启动即可。
pub fn load(work_dir: &Path) -> Option<PendingUpdate> {
    let text = std::fs::read_to_string(path(work_dir)).ok()?;
    serde_json::from_str::<PendingUpdate>(&text).ok()
}

/// 写记录。用「临时文件 + rename」：写到一半断电也不会留下半截 JSON。
pub fn save(work_dir: &Path, pending: &PendingUpdate) -> std::io::Result<()> {
    std::fs::create_dir_all(work_dir)?;
    let target = path(work_dir);
    let temp = target.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(pending).unwrap_or_default();
    std::fs::write(&temp, body)?;
    std::fs::rename(&temp, &target)
}

/// 删记录。文件本来就不在也算成功。
pub fn clear(work_dir: &Path) {
    std::fs::remove_file(path(work_dir)).ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    fn sample(file: PathBuf) -> PendingUpdate {
        PendingUpdate {
            version: "0.2.2".into(),
            file,
            size: 1234,
            sha256: "a".repeat(64),
            title: "演示版本".into(),
        }
    }

    #[test]
    fn roundtrip() {
        let dir = temp_dir("pending");
        let pending = sample(dir.join("pkg.exe"));

        save(&dir, &pending).unwrap();
        assert_eq!(load(&dir), Some(pending.clone()));

        clear(&dir);
        assert_eq!(load(&dir), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn broken_file_is_treated_as_absent() {
        let dir = temp_dir("pending-broken");
        std::fs::write(path(&dir), "{ not json").unwrap();
        assert_eq!(load(&dir), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pending_is_only_pending_while_target_is_newer() {
        let pending = sample(PathBuf::from("x.exe"));
        assert!(pending.is_pending("0.2.1"));
        // 已经追上了：不该再拦着
        assert!(!pending.is_pending("0.2.2"));
        assert!(!pending.is_pending("0.3.0"));
        // 版本号坏了当无效
        assert!(!pending.is_pending("dev-build"));
    }

    #[test]
    fn save_creates_directory() {
        let dir = temp_dir("pending-mkdir").join("deep");
        let pending = sample(dir.join("pkg.exe"));
        save(&dir, &pending).unwrap();
        assert!(path(&dir).is_file());
        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }
}
