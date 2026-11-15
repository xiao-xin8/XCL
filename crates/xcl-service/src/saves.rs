//! 存档列表：只读扫描 `saves/`，从 `level.dat` 读世界名、游戏版本和最后游玩时间。
//!
//! 和 `content.rs` 的区别：存档是目录不是文件，也没有启用 / 禁用的概念
//! （把世界改名成 `.disabled` 对游戏毫无意义），所以这里只有列出、打开目录、删除三件事。
//!
//! `level.dat` 是 gzip 压缩的 NBT。这里带一个够用的只读解析器：存档列表只关心几个标量，
//! 方块、实体、生物群系这些大块内容整段跳过，不建 AST、不分配 —— 一个存档几十 MB，
//! 为了几行字去完整反序列化不值得。将来地图预览走专门的渲染库，不共用这段代码。

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use flate2::read::GzDecoder;
use serde::Serialize;
use xcl_core::AppError;

use crate::storage::dir_size;
use crate::AppState;

/// `level.dat` 上限。正常存档只有几十 KB，超过这个数说明文件已经坏了，不去啃。
const MAX_LEVEL_BYTES: u64 = 8 * 1024 * 1024;
/// 世界封面 `icon.png` 上限。Minecraft 自己写的是 64×64，远小于这个值。
const MAX_ICON_BYTES: u64 = 256 * 1024;

const TAG_END: u8 = 0;
const TAG_BYTE: u8 = 1;
const TAG_SHORT: u8 = 2;
const TAG_INT: u8 = 3;
const TAG_LONG: u8 = 4;
const TAG_FLOAT: u8 = 5;
const TAG_DOUBLE: u8 = 6;
const TAG_BYTE_ARRAY: u8 = 7;
const TAG_STRING: u8 = 8;
const TAG_LIST: u8 = 9;
const TAG_COMPOUND: u8 = 10;
const TAG_INT_ARRAY: u8 = 11;
const TAG_LONG_ARRAY: u8 = 12;

/// 列表里的一条存档。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveView {
    /// 存档目录名。定位删除 / 打开目录都靠它，世界名可以被玩家改，目录名不会。
    pub folder_name: String,
    /// `level.dat` 里的世界名；读不到时退回目录名。
    pub name: String,
    /// 生成这个世界的游戏版本，如 `1.20.1`。
    pub game_version: Option<String>,
    /// 是否由快照版本生成。
    pub snapshot: bool,
    pub data_version: Option<i32>,
    /// 最后一次游玩时间（毫秒时间戳）。
    pub last_played_ms: Option<u64>,
    /// 目录总大小，含 `region/`、`DIM-1/` 这些。
    pub size_bytes: u64,
    /// `icon.png` 的 data URL；世界没有封面时为 `None`。
    pub icon: Option<String>,
    /// 世界种子。i64 超出 JS 安全整数范围，按字符串传，前端要用时自己转。
    pub seed: Option<String>,
    /// `survival` / `creative` / `adventure` / `spectator`。
    pub game_mode: Option<&'static str>,
    /// `peaceful` / `easy` / `normal` / `hard`。
    pub difficulty: Option<&'static str>,
    pub hardcore: bool,
    pub cheats: bool,
    pub spawn_x: Option<i32>,
    pub spawn_z: Option<i32>,
    /// `level.dat` 缺失或解析失败。界面照常列出来，只是没有元数据 ——
    /// 一个读不出来的世界也仍然要能被删除或者打开目录。
    pub broken: bool,
}

/// 一个版本的存档列表快照。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveListView {
    pub version_id: String,
    /// 实际扫描的目录。非隔离版本会指向共享根目录。
    pub game_dir: String,
    pub isolated: bool,
    /// 这个版本正在准备 / 运行。运行时不允许删文件。
    pub running: bool,
    pub items: Vec<SaveView>,
    pub total_bytes: u64,
}

impl AppState {
    /// 列出某个版本的全部存档。只读，不改任何文件。
    pub fn list_saves(&self, version_id: &str) -> Result<SaveListView, AppError> {
        self.require_version_dir(version_id)?;
        let config = self.config();
        let resolution = crate::isolation::resolution_for(&self.layout, &config, version_id)?;
        let saves_dir = resolution.game_dir.join("saves");
        let running = self.launches.active_for(version_id);

        let mut items = Vec::new();
        if let Ok(entries) = fs::read_dir(&saves_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(folder_name) = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
                else {
                    continue;
                };
                items.push(read_save(&path, folder_name));
            }
        }

        // 最近玩过的排前面；没读过 `level.dat` 的（`broken`）垫底，同类按名字。
        items.sort_by(|a, b| {
            b.last_played_ms
                .cmp(&a.last_played_ms)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.folder_name.cmp(&b.folder_name))
        });

        let total_bytes = items.iter().map(|item| item.size_bytes).sum();
        Ok(SaveListView {
            version_id: version_id.to_string(),
            game_dir: resolution.game_dir.display().to_string(),
            isolated: resolution.enabled,
            running,
            items,
            total_bytes,
        })
    }

    /// 用系统文件管理器打开某个存档目录。
    pub fn open_save_folder(&self, version_id: &str, folder_name: &str) -> Result<(), AppError> {
        let config = self.config();
        let resolution = crate::isolation::resolution_for(&self.layout, &config, version_id)?;
        let path = safe_save_dir(&resolution.game_dir.join("saves"), folder_name)?;
        if !path.is_dir() {
            return Err(AppError::not_found(
                "存档不存在",
                format!("找不到存档目录 {folder_name}"),
            ));
        }
        crate::folders::open_in_file_manager(&path)
    }

    /// 删除一个存档：移到 `versions/<id>/XCL/trash/saves/`，不直接永久删除。
    pub fn delete_save(
        &self,
        version_id: &str,
        folder_name: &str,
    ) -> Result<SaveListView, AppError> {
        if self.launches.active_for(version_id) {
            return Err(AppError::validation(
                "游戏正在运行或准备中",
                "先等启动结束并关闭游戏，再删除存档",
            )
            .with_hint("运行中的游戏会锁住 region 文件"));
        }

        let config = self.config();
        let resolution = crate::isolation::resolution_for(&self.layout, &config, version_id)?;
        let saves_dir = resolution.game_dir.join("saves");
        let path = safe_save_dir(&saves_dir, folder_name)?;
        if !path.is_dir() {
            return Err(AppError::not_found(
                "存档不存在",
                format!("找不到存档目录 {folder_name}"),
            ));
        }

        let trash = self
            .layout
            .version_dir(version_id)
            .join("XCL")
            .join("trash")
            .join("saves");
        fs::create_dir_all(&trash).map_err(|err| AppError::io(&err, "创建存档回收站"))?;
        let target = unique_path(&trash, format!("{}-{folder_name}", now_ms()));
        fs::rename(&path, &target).map_err(|err| AppError::io(&err, "移动存档到回收站"))?;

        self.list_saves(version_id)
    }
}

/// 读一个存档目录。任何一步失败都退化成「只有目录名」的条目，不整体报错。
fn read_save(dir: &Path, folder_name: String) -> SaveView {
    let level = read_level_dat(&dir.join("level.dat"));
    let broken = level.is_none();
    let level = level.unwrap_or_default();

    SaveView {
        name: level
            .name
            .clone()
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| folder_name.clone()),
        game_version: level.game_version,
        snapshot: level.snapshot,
        data_version: level.data_version,
        last_played_ms: level.last_played.and_then(|ms| u64::try_from(ms).ok()),
        size_bytes: dir_size(dir),
        icon: read_icon(dir),
        seed: level.seed.map(|seed| seed.to_string()),
        game_mode: level.game_type.and_then(game_mode_name),
        difficulty: level.difficulty.and_then(difficulty_name),
        hardcore: level.hardcore,
        cheats: level.cheats,
        spawn_x: level.spawn_x.or(level.spawn_pos.map(|pos| pos.0)),
        spawn_z: level.spawn_z.or(level.spawn_pos.map(|pos| pos.1)),
        broken,
        folder_name,
    }
}

fn game_mode_name(value: i32) -> Option<&'static str> {
    match value {
        0 => Some("survival"),
        1 => Some("creative"),
        2 => Some("adventure"),
        3 => Some("spectator"),
        _ => None,
    }
}

fn difficulty_name(value: i8) -> Option<&'static str> {
    match value {
        0 => Some("peaceful"),
        1 => Some("easy"),
        2 => Some("normal"),
        3 => Some("hard"),
        _ => None,
    }
}

/// 读世界封面。跳过读不了和过大的文件，绝不因为封面挂掉整条记录。
fn read_icon(dir: &Path) -> Option<String> {
    let path = dir.join("icon.png");
    let meta = fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_ICON_BYTES {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    let mime = crate::local::image_mime(&bytes)?;
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// 校验 `folder_name` 是 `saves/` 的直接子目录名。
///
/// 存档目录名来自磁盘，理论上可能是玩家手打的任意字符串；这里挡掉路径分隔符和 `..`，
/// 保证删除只会作用在 `saves/` 里面。
pub(crate) fn safe_save_dir(saves_dir: &Path, folder_name: &str) -> Result<PathBuf, AppError> {
    let invalid = folder_name.is_empty()
        || folder_name == "."
        || folder_name == ".."
        || folder_name.contains(['/', '\\', ':', '\0'])
        || folder_name.chars().any(|ch| ch.is_control());
    if invalid {
        return Err(AppError::validation(
            "存档目录名不合法",
            format!("`{folder_name}` 不是合法的存档目录"),
        ));
    }
    Ok(saves_dir.join(folder_name))
}

fn unique_path(dir: &Path, name: String) -> PathBuf {
    let mut candidate = dir.join(&name);
    let mut index = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{name} ({index})"));
        index += 1;
    }
    candidate
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|span| span.as_millis())
        .unwrap_or(0)
}

// ---------- level.dat ----------

/// 从 `level.dat` 里取到的那几个字段。全是 `Option`，缺哪个都不影响别的。
#[derive(Debug, Default)]
struct LevelInfo {
    name: Option<String>,
    game_version: Option<String>,
    snapshot: bool,
    data_version: Option<i32>,
    last_played: Option<i64>,
    seed: Option<i64>,
    game_type: Option<i32>,
    difficulty: Option<i8>,
    hardcore: bool,
    cheats: bool,
    spawn_x: Option<i32>,
    spawn_z: Option<i32>,
    /// 26.x 把出生点搬到了 `spawn.pos`（`[x, y, z]` 浮点列表），
    /// 老字段可能不存在。两处都读，老的优先。
    spawn_pos: Option<(i32, i32)>,
}

fn read_level_dat(path: &Path) -> Option<LevelInfo> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_LEVEL_BYTES {
        return None;
    }
    let file = fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    GzDecoder::new(file)
        .take(MAX_LEVEL_BYTES)
        .read_to_end(&mut buf)
        .ok()?;
    parse_level(&buf)
}

/// 解析 `level.dat` 的根 compound，只认 `Data` 这一支。
fn parse_level(bytes: &[u8]) -> Option<LevelInfo> {
    let mut nbt = Nbt::new(bytes);
    if nbt.u8().ok()? != TAG_COMPOUND {
        return None;
    }
    nbt.skip_string().ok()?;

    let mut info = LevelInfo::default();
    loop {
        let tag = nbt.u8().ok()?;
        if tag == TAG_END {
            break;
        }
        let name = nbt.string().ok()?;
        if tag == TAG_COMPOUND && name == "Data" {
            read_data(&mut nbt, &mut info).ok()?;
        } else {
            nbt.skip_payload(tag).ok()?;
        }
    }
    // 连世界名都没有，说明不是一份能认的 level.dat。
    info.name.is_some().then_some(info)
}

fn read_data(nbt: &mut Nbt, info: &mut LevelInfo) -> NbtResult<()> {
    loop {
        let tag = nbt.u8()?;
        if tag == TAG_END {
            return Ok(());
        }
        let name = nbt.string()?;
        // 字段名和类型都对上才读，类型变了（例如某个版本把 byte 改成 int）
        // 就走 `_` 分支整段跳过，不会读错位。
        match (name.as_str(), tag) {
            ("LevelName", TAG_STRING) => info.name = Some(nbt.string()?),
            ("LastPlayed", TAG_LONG) => info.last_played = Some(nbt.i64()?),
            ("RandomSeed", TAG_LONG) => info.seed = Some(nbt.i64()?),
            ("GameType", TAG_INT) => info.game_type = Some(nbt.i32()?),
            ("Difficulty", TAG_BYTE) => info.difficulty = Some(nbt.u8()? as i8),
            ("hardcore", TAG_BYTE) => info.hardcore = nbt.u8()? != 0,
            ("allowCommands", TAG_BYTE) => info.cheats = nbt.u8()? != 0,
            ("SpawnX", TAG_INT) => info.spawn_x = Some(nbt.i32()?),
            ("SpawnZ", TAG_INT) => info.spawn_z = Some(nbt.i32()?),
            ("DataVersion", TAG_INT) => info.data_version = Some(nbt.i32()?),
            ("Version", TAG_COMPOUND) => read_version(nbt, info)?,
            ("spawn", TAG_COMPOUND) => read_spawn(nbt, info)?,
            _ => nbt.skip_payload(tag)?,
        }
    }
}

/// `Data.spawn`（26.x）：出生点从两个 int 字段换成了 `pos` 浮点列表。
fn read_spawn(nbt: &mut Nbt, info: &mut LevelInfo) -> NbtResult<()> {
    loop {
        let tag = nbt.u8()?;
        if tag == TAG_END {
            return Ok(());
        }
        let name = nbt.string()?;
        if name == "pos" && tag == TAG_LIST {
            info.spawn_pos = read_block_pos(nbt)?;
        } else {
            nbt.skip_payload(tag)?;
        }
    }
}

/// 读 `pos` 列表的 x / z。元素可能是 double，也可能是 int，两种都认。
fn read_block_pos(nbt: &mut Nbt) -> NbtResult<Option<(i32, i32)>> {
    let element = nbt.u8()?;
    let len = nbt.i32()?;
    if len < 0 {
        return Err(());
    }
    let mut values = [0f64; 3];
    let mut read = 0usize;
    for index in 0..len {
        let value = match element {
            TAG_DOUBLE => nbt.f64()?,
            TAG_FLOAT => nbt.f32()? as f64,
            TAG_INT => nbt.i32()? as f64,
            TAG_LONG => nbt.i64()? as f64,
            _ => {
                nbt.skip_payload(element)?;
                continue;
            }
        };
        if let Some(slot) = values.get_mut(index as usize) {
            *slot = value;
            read += 1;
        }
    }
    if read < 3 {
        return Ok(None);
    }
    Ok(Some((values[0] as i32, values[2] as i32)))
}

/// `Data.Version`：1.9 之后才有，更老的存档直接没有这一支。
fn read_version(nbt: &mut Nbt, info: &mut LevelInfo) -> NbtResult<()> {
    loop {
        let tag = nbt.u8()?;
        if tag == TAG_END {
            return Ok(());
        }
        let name = nbt.string()?;
        match (name.as_str(), tag) {
            ("Name", TAG_STRING) => info.game_version = Some(nbt.string()?),
            ("Snapshot", TAG_BYTE) => info.snapshot = nbt.u8()? != 0,
            _ => nbt.skip_payload(tag)?,
        }
    }
}

type NbtResult<T> = Result<T, ()>;

/// 顺序读大端 NBT 的最小实现。没有 AST，边读边取，过长的内容按长度跳过。
struct Nbt<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Nbt<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, len: usize) -> NbtResult<&'a [u8]> {
        let end = self.pos.checked_add(len).ok_or(())?;
        if end > self.buf.len() {
            return Err(());
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> NbtResult<u8> {
        Ok(self.take(1)?[0])
    }

    fn i16(&mut self) -> NbtResult<i16> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into().map_err(|_| ())?))
    }

    fn i32(&mut self) -> NbtResult<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().map_err(|_| ())?))
    }

    fn i64(&mut self) -> NbtResult<i64> {
        Ok(i64::from_be_bytes(self.take(8)?.try_into().map_err(|_| ())?))
    }

    fn f32(&mut self) -> NbtResult<f32> {
        Ok(f32::from_be_bytes(self.take(4)?.try_into().map_err(|_| ())?))
    }

    fn f64(&mut self) -> NbtResult<f64> {
        Ok(f64::from_be_bytes(self.take(8)?.try_into().map_err(|_| ())?))
    }

    fn string(&mut self) -> NbtResult<String> {
        let len = self.i16()?;
        if len < 0 {
            return Err(());
        }
        Ok(decode_mutf8(self.take(len as usize)?))
    }

    fn skip_string(&mut self) -> NbtResult<()> {
        let len = self.i16()?;
        if len < 0 {
            return Err(());
        }
        self.take(len as usize).map(|_| ())
    }

    /// 整段跳过一个 tag 的 payload。数组和列表只按声明的长度前进，不读内容。
    fn skip_payload(&mut self, tag: u8) -> NbtResult<()> {
        match tag {
            TAG_BYTE => self.take(1).map(|_| ()),
            TAG_SHORT => self.take(2).map(|_| ()),
            TAG_INT | TAG_FLOAT => self.take(4).map(|_| ()),
            TAG_LONG | TAG_DOUBLE => self.take(8).map(|_| ()),
            TAG_BYTE_ARRAY => {
                let len = array_len(self.i32()?)?;
                self.take(len).map(|_| ())
            }
            TAG_STRING => self.skip_string(),
            TAG_LIST => {
                let element = self.u8()?;
                let len = self.i32()?;
                if len < 0 {
                    return Err(());
                }
                for _ in 0..len {
                    self.skip_payload(element)?;
                }
                Ok(())
            }
            TAG_COMPOUND => loop {
                let inner = self.u8()?;
                if inner == TAG_END {
                    return Ok(());
                }
                self.skip_string()?;
                self.skip_payload(inner)?;
            },
            TAG_INT_ARRAY => {
                let len = array_len(self.i32()?)?;
                self.take(len.checked_mul(4).ok_or(())?).map(|_| ())
            }
            TAG_LONG_ARRAY => {
                let len = array_len(self.i32()?)?;
                self.take(len.checked_mul(8).ok_or(())?).map(|_| ())
            }
            _ => Err(()),
        }
    }
}

fn array_len(len: i32) -> NbtResult<usize> {
    if len < 0 {
        return Err(());
    }
    Ok(len as usize)
}

/// Minecraft 的字符串是「修改过的 UTF-8」：`\0` 写成 `C0 80`，
/// 增补平面字符（emoji）按代理对分别编码，在标准 UTF-8 里属于非法序列。
/// 先按标准 UTF-8 解，失败再按 UTF-16 单元重组 —— 绝大多数世界名走第一条路。
fn decode_mutf8(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    let mut units = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let (unit, step) = match byte {
            0x00..=0x7F => (byte as u16, 1),
            0xC0..=0xDF if index + 1 < bytes.len() => (
                (((byte as u16) & 0x1F) << 6) | (bytes[index + 1] as u16 & 0x3F),
                2,
            ),
            0xE0..=0xEF if index + 2 < bytes.len() => (
                (((byte as u16) & 0x0F) << 12)
                    | ((bytes[index + 1] as u16 & 0x3F) << 6)
                    | (bytes[index + 2] as u16 & 0x3F),
                3,
            ),
            _ => (0xFFFD, 1),
        };
        units.push(unit);
        index += step;
    }
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 拼一份只含 `Data.LevelName` / `Data.GameType` 的最小 level.dat。
    fn minimal_level(name: &str, game_type: i32) -> Vec<u8> {
        let mut buf = vec![TAG_COMPOUND, 0, 0];
        buf.extend([TAG_COMPOUND, 0, 4]);
        buf.extend(b"Data");

        buf.push(TAG_STRING);
        buf.extend([0, 9]);
        buf.extend(b"LevelName");
        buf.extend((name.len() as u16).to_be_bytes());
        buf.extend(name.as_bytes());

        buf.push(TAG_INT);
        buf.extend([0, 8]);
        buf.extend(b"GameType");
        buf.extend(game_type.to_be_bytes());

        buf.push(TAG_END);
        buf.push(TAG_END);
        buf
    }

    #[test]
    fn parses_scalar_fields() {
        let info = parse_level(&minimal_level("TestWorld", 1)).unwrap();
        assert_eq!(info.name.as_deref(), Some("TestWorld"));
        assert_eq!(info.game_type, Some(1));
    }

    #[test]
    fn skips_unknown_and_large_payloads() {
        let mut buf = vec![TAG_COMPOUND, 0, 0];
        buf.extend([TAG_COMPOUND, 0, 4]);
        buf.extend(b"Data");

        // 一个 int array 和一个 compound，解析器应该整段跳过并继续读后面的字段。
        buf.push(TAG_INT_ARRAY);
        buf.extend([0, 3]);
        buf.extend(b"Big");
        buf.extend(3i32.to_be_bytes());
        buf.extend([0u8; 12]);

        buf.push(TAG_COMPOUND);
        buf.extend([0, 3]);
        buf.extend(b"Sub");
        buf.push(TAG_END);

        buf.push(TAG_STRING);
        buf.extend([0, 9]);
        buf.extend(b"LevelName");
        buf.extend(5u16.to_be_bytes());
        buf.extend(b"After");

        buf.push(TAG_END);
        buf.push(TAG_END);

        let info = parse_level(&buf).unwrap();
        assert_eq!(info.name.as_deref(), Some("After"));
    }

    #[test]
    fn missing_level_name_is_not_a_level() {
        let mut buf = vec![TAG_COMPOUND, 0, 0];
        buf.extend([TAG_COMPOUND, 0, 4]);
        buf.extend(b"Data");
        buf.push(TAG_END);
        buf.push(TAG_END);
        assert!(parse_level(&buf).is_none());
    }

    #[test]
    fn truncated_bytes_do_not_panic() {
        let full = minimal_level("TestWorld", 0);
        for cut in 0..full.len() {
            let _ = parse_level(&full[..cut]);
        }
    }

    #[test]
    fn decodes_mutf8_surrogate_pairs() {
        // 每个汉字按三字节 UTF-8 编码，落在标准 UTF-8 那条路上。
        assert_eq!(decode_mutf8("世界".as_bytes()), "世界");
        // emoji 的代理对是 MUTF-8 特有形态：两个三字节单元，标准 UTF-8 解不出来。
        let emoji = "\u{1F600}";
        let mut mutf8 = Vec::new();
        for unit in emoji.encode_utf16() {
            mutf8.push(0xE0 | (unit >> 12) as u8);
            mutf8.push(0x80 | ((unit >> 6) as u8 & 0x3F));
            mutf8.push(0x80 | (unit as u8 & 0x3F));
        }
        assert!(
            std::str::from_utf8(&mutf8).is_err(),
            "代理对不该是合法 UTF-8"
        );
        assert_eq!(decode_mutf8(&mutf8), emoji);
    }

    /// 26.x 把出生点换成了 `spawn.pos` 浮点列表，老字段可能整个不存在。
    #[test]
    fn reads_spawn_from_modern_pos_list() {
        let mut buf = vec![TAG_COMPOUND, 0, 0];
        buf.extend([TAG_COMPOUND, 0, 4]);
        buf.extend(b"Data");
        buf.push(TAG_STRING);
        buf.extend([0, 9]);
        buf.extend(b"LevelName");
        buf.extend(5u16.to_be_bytes());
        buf.extend(b"Spawn");

        // spawn: { pos: [x, 64.0, z] }，元素是 double
        buf.push(TAG_COMPOUND);
        buf.extend([0, 5]);
        buf.extend(b"spawn");
        buf.push(TAG_LIST);
        buf.extend([0, 3]);
        buf.extend(b"pos");
        buf.push(TAG_DOUBLE);
        buf.extend(3i32.to_be_bytes());
        for value in [-112.5f64, 64.0, -66.25] {
            buf.extend(value.to_be_bytes());
        }
        buf.push(TAG_END);
        buf.push(TAG_END);
        buf.push(TAG_END);

        let info = parse_level(&buf).unwrap();
        assert!(info.spawn_x.is_none(), "老字段确实不存在");
        assert_eq!(info.spawn_pos, Some((-112, -66)));
    }

    /// 真实存档里 `level.dat` 是 gzip 的，单独验一遍解压 + 组装。
    #[test]
    fn reads_gzipped_level_dat_from_disk() {
        use std::io::Write as _;

        let dir = std::env::temp_dir().join(format!("xcl-save-test-{}", now_ms()));
        fs::create_dir_all(&dir).unwrap();

        let mut encoder =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&minimal_level("磁盘世界", 0)).unwrap();
        let bytes = encoder.finish().unwrap();
        fs::write(dir.join("level.dat"), bytes).unwrap();

        let save = read_save(&dir, "FolderName".to_string());
        assert_eq!(save.name, "磁盘世界");
        assert!(!save.broken);
        assert_eq!(save.game_mode, Some("survival"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_path_traversal_in_save_folder() {
        let root = Path::new("saves");
        for bad in ["", ".", "..", "../x", "a/b", "a\\b", "c:x"] {
            assert!(safe_save_dir(root, bad).is_err(), "{bad} 应被拒绝");
        }
        assert!(safe_save_dir(root, "我的世界").is_ok());
    }
}
