//! 存档地图瓦片：把一个世界切成 256×256 的瓦片，按需渲染 + 磁盘缓存。
//!
//! 渲染交给 `conic-worldmap`，这里负责
//! 瓦片坐标换算、降采样、编码 PNG、缓存。前端拿瓦片自己拼成可拖动缩放的地图。
//!
//! 三个必须记住的约束：
//!
//! 1. 库是 1 方块 = 1 像素，不做降采样。所以 zoom > 0 时我们先按原始分辨率
//!    渲染 `256 << zoom` 见方，再由 [`downsample`] 用 box filter 缩到 256。
//!    这也意味着 zoom 每加一级，渲染像素数是 4 倍 —— 所以 zoom 上限压在 2。
//! 2. `render` 是同步的，必须走 `spawn_blocking`；`WorldMap` 用完即丢
//!    （它内部缓存 2048 个 chunk，常驻会吃掉几百 MB）。
//! 3. 瓦片坐标用「方块坐标右移」，不依赖世界中心：
//!    `tile = floor(block / (256 * scale))`，zoom 之间没有重叠，拼接不会错位。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use base64::Engine as _;
use conic_worldmap::{RenderOptions, RenderRequest, WorldMap};
use serde::Serialize;
use sha1::{Digest, Sha1};
use xcl_core::{AppError, ErrorCode};

use crate::saves::safe_save_dir;
use crate::AppState;

/// 一个瓦片的边长（像素）。
pub const TILE_PIXELS: u32 = 256;

/// 最大缩放级别（1 像素 = 4 方块）。
///
/// 每加一级，一个瓦片要读的区块数翻 4 倍：zoom 0 是 256 个，zoom 1 是 1024，
/// zoom 2 是 4096。再往上就得上分钟了，界面也用不到那么远。
pub const MAX_ZOOM: u32 = 2;

/// 一张瓦片。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapTile {
    /// `data:image/png;base64,...`；`empty` 时是空串。
    pub data_url: String,
    /// 这块一个已生成的区块都没有。
    pub empty: bool,
}

impl AppState {
    /// 渲染一张瓦片。只读，不碰存档里的任何文件。
    ///
    /// `zoom = 0` 时 1 像素 = 1 方块；`zoom = n` 时 1 像素 = `1 << n` 方块。
    pub fn render_save_tile(
        &self,
        version_id: &str,
        folder_name: &str,
        zoom: u32,
        tile_x: i32,
        tile_z: i32,
    ) -> Result<MapTile, AppError> {
        let zoom = zoom.min(MAX_ZOOM);
        let scale = 1i32 << zoom;

        let config = self.config();
        let resolution = crate::isolation::resolution_for(&self.layout, &config, version_id)?;
        let saves_dir = resolution.game_dir.join("saves");
        let world_dir = safe_save_dir(&saves_dir, folder_name)?;
        if !world_dir.is_dir() {
            return Err(AppError::not_found(
                "存档不存在",
                format!("找不到存档目录 {folder_name}"),
            ));
        }

        let stamp = region_stamp(&region_dir(&world_dir));
        let cache_path = self
            .layout
            .cache_root()
            .join("maps")
            .join(cache_key(version_id, folder_name, stamp))
            .join(format!("z{zoom}"))
            .join(format!("{tile_x}_{tile_z}.png"));

        if let Some(bytes) = fs::read(&cache_path).ok().filter(|data| !data.is_empty()) {
            return Ok(MapTile {
                data_url: data_url(&bytes),
                empty: false,
            });
        }

        let world = WorldMap::open(&world_dir).map_err(|err| {
            AppError::new(ErrorCode::Unknown, "这个存档读不出地图", format!("{err}"))
                .with_hint("世界可能来自 1.9 之前的版本，或者 level.dat 已经损坏")
        })?;

        // 瓦片左上角的方块坐标 = tx * 256 * scale；中心在瓦片正中。
        let origin_x = tile_x.saturating_mul(TILE_PIXELS as i32).saturating_mul(scale);
        let origin_z = tile_z.saturating_mul(TILE_PIXELS as i32).saturating_mul(scale);
        let center_x = origin_x + (TILE_PIXELS as i32 * scale) / 2;
        let center_z = origin_z + (TILE_PIXELS as i32 * scale) / 2;

        // 先按 1:1 渲出整块，再缩到瓦片大小。
        let raw_side = TILE_PIXELS << zoom;
        let preview = render_tile(&world, center_x, center_z, raw_side)?;
        let Some(pixels) = preview else {
            return Ok(MapTile {
                data_url: String::new(),
                empty: true,
            });
        };

        let scaled = downsample(&pixels, raw_side, TILE_PIXELS);
        let png = encode_png(&scaled, TILE_PIXELS, TILE_PIXELS)?;
        write_cache(&cache_path, &png);

        Ok(MapTile {
            data_url: data_url(&png),
            empty: false,
        })
    }
}

/// 世界目录里的 region 目录：1.21.6 之前是 `<world>/region`，之后搬进了
/// `dimensions/minecraft/overworld/region`。两种都要认。
fn region_dir(world_dir: &Path) -> PathBuf {
    let legacy = world_dir.join("region");
    if legacy.is_dir() {
        return legacy;
    }
    world_dir
        .join("dimensions")
        .join("minecraft")
        .join("overworld")
        .join("region")
}

/// region 目录里最新的 .mca 修改时间（秒）。世界玩过之后这个数会变，
/// 整棵缓存自然失效，不需要手动刷新。
fn region_stamp(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("mca"))
        })
        .filter_map(|entry| entry.metadata().ok()?.modified().ok())
        .filter_map(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|span| span.as_secs())
        .max()
        .unwrap_or(0)
}

/// 缓存目录名：`<世界哈希>-<region mtime>`。同一世界的所有 zoom / 瓦片共用一层，
/// 世界变了整目录换名字，旧目录留着也不会被误用。
fn cache_key(version_id: &str, folder_name: &str, stamp: u64) -> String {
    format!("{}-{stamp}", world_hash(version_id, folder_name))
}

/// `(版本 id + 存档目录)` 的短哈希。缓存目录名和地图会话 id 共用。
fn world_hash(version_id: &str, folder_name: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(version_id.as_bytes());
    hasher.update([0]);
    hasher.update(folder_name.as_bytes());
    hasher
        .finalize()
        .iter()
        .take(12)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 渲染一块正方形区域。全透明时返回 `None`（没有已生成的区块）。
fn render_tile(
    world: &WorldMap,
    center_x: i32,
    center_z: i32,
    side: u32,
) -> Result<Option<Vec<u8>>, AppError> {
    let result = world
        .render(
            &RenderRequest::new(center_x, center_z, side, side),
            &RenderOptions::default(),
        )
        .map_err(|err| AppError::new(ErrorCode::Unknown, "渲染地图失败", format!("{err}")))?;

    if result.pixels.chunks_exact(4).all(|pixel| pixel[3] == 0) {
        return Ok(None);
    }
    Ok(Some(result.pixels))
}

/// RGBA 的 box filter 降采样。`factor = src / dst` 必须是整数（`256 << zoom`
/// 到 256 天然满足）。
///
/// 透明像素不参与 RGB 平均 —— 直接平均会让地形边缘发灰发暗。
fn downsample(pixels: &[u8], src: u32, dst: u32) -> Vec<u8> {
    let factor = (src / dst).max(1);
    if factor <= 1 || src == dst {
        return pixels.to_vec();
    }
    let mut out = vec![0u8; (dst * dst * 4) as usize];
    for y in 0..dst {
        for x in 0..dst {
            let mut alpha = 0u32;
            let mut rgb = [0u32; 3];
            let mut opaque = 0u32;
            for sy in 0..factor {
                for sx in 0..factor {
                    let src_x = x * factor + sx;
                    let src_y = y * factor + sy;
                    let offset = ((src_y * src + src_x) * 4) as usize;
                    let a = pixels[offset + 3] as u32;
                    alpha += a;
                    if a > 0 {
                        rgb[0] += pixels[offset] as u32;
                        rgb[1] += pixels[offset + 1] as u32;
                        rgb[2] += pixels[offset + 2] as u32;
                        opaque += 1;
                    }
                }
            }
            let count = factor * factor;
            let target = ((y * dst + x) * 4) as usize;
            if opaque > 0 {
                out[target] = (rgb[0] / opaque) as u8;
                out[target + 1] = (rgb[1] / opaque) as u8;
                out[target + 2] = (rgb[2] / opaque) as u8;
            }
            out[target + 3] = (alpha / count) as u8;
        }
    }
    out
}

/// 裸 RGBA → PNG。
fn data_url(bytes: &[u8]) -> String {
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

fn encode_png(pixels: &[u8], width: u32, height: u32) -> Result<Vec<u8>, AppError> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|err| AppError::new(ErrorCode::Unknown, "编码地图失败", err.to_string()))?;
        writer
            .write_image_data(pixels)
            .map_err(|err| AppError::new(ErrorCode::Unknown, "编码地图失败", err.to_string()))?;
    }
    Ok(out)
}

/// 写缓存失败不影响这次请求，最多下次再渲一遍。
fn write_cache(path: &Path, bytes: &[u8]) {
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let tmp = path.with_extension("png.tmp");
    if fs::write(&tmp, bytes).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_changes_with_world_and_mtime() {
        let a = cache_key("1.20.1", "World", 100);
        assert_eq!(a, cache_key("1.20.1", "World", 100));
        assert_ne!(a, cache_key("1.20.1", "World", 101), "世界变了要换缓存");
        assert_ne!(a, cache_key("1.21.1", "World", 100));
        assert_ne!(a, cache_key("1.20.1", "World2", 100));
        assert!(a.ends_with("-100"));
    }

    #[test]
    fn downsample_averages_opaque_pixels_only() {
        // 2×2 → 1×1：三个不透明纯红 + 一个全透明。
        let src = vec![
            255, 0, 0, 255, 255, 0, 0, 255, // 上排
            255, 0, 0, 255, 0, 0, 0, 0, // 下排
        ];
        let out = downsample(&src, 2, 1);
        assert_eq!(&out[0..3], &[255, 0, 0], "RGB 只由不透明像素决定");
        assert_eq!(out[3], 191, "alpha 是四格平均：255*3/4");
    }

    #[test]
    fn downsample_keeps_size_when_factor_is_one() {
        let src = vec![9u8; 4 * 4 * 4];
        let out = downsample(&src, 4, 4);
        assert_eq!(out, src);
    }

    #[test]
    fn encodes_rgba_to_png() {
        let pixels = vec![255u8; 4 * 4 * 4];
        let png = encode_png(&pixels, 4, 4).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "要有 PNG 文件头");
        assert!(png.len() > 40);
    }

    // ---------- 端到端：合成一个只有一个区块的世界，跑完整瓦片渲染 ----------
    //
    // NBT 写法按 1.18+ 的根级 `sections` + `block_states`（区块必须带 `yPos`，
    // 缺了库会判「格式不认识」，渲染出来是全透明）。

    const T_BYTE: u8 = 1;
    const T_INT: u8 = 3;
    const T_STRING: u8 = 8;
    const T_LIST: u8 = 9;
    const T_COMPOUND: u8 = 10;

    fn push_str(buf: &mut Vec<u8>, text: &str) {
        buf.extend_from_slice(&(text.len() as u16).to_be_bytes());
        buf.extend_from_slice(text.as_bytes());
    }

    fn compound(fields: &[(u8, &str, Vec<u8>)]) -> Vec<u8> {
        let mut buf = Vec::new();
        for (id, name, value) in fields {
            buf.push(*id);
            push_str(&mut buf, name);
            buf.extend_from_slice(value);
        }
        buf.push(0);
        buf
    }

    fn list(items: Vec<Vec<u8>>, element: u8) -> Vec<u8> {
        let mut buf = vec![element];
        buf.extend_from_slice(&(items.len() as u32).to_be_bytes());
        for item in items {
            buf.extend_from_slice(&item);
        }
        buf
    }

    fn int(value: i32) -> Vec<u8> {
        value.to_be_bytes().to_vec()
    }

    fn string(value: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        push_str(&mut buf, value);
        buf
    }

    fn root(fields: &[(u8, &str, Vec<u8>)]) -> Vec<u8> {
        let mut buf = vec![T_COMPOUND];
        push_str(&mut buf, "");
        buf.extend_from_slice(&compound(fields));
        buf
    }

    fn block_name(name: &str) -> Vec<u8> {
        compound(&[(T_STRING, "Name", string(name))])
    }

    fn modern_chunk() -> Vec<u8> {
        let block_states = compound(&[(
            T_LIST,
            "palette",
            list(vec![block_name("minecraft:stone")], T_COMPOUND),
        )]);
        let section = compound(&[
            (T_BYTE, "Y", vec![0]),
            (T_COMPOUND, "block_states", block_states),
        ]);
        root(&[
            (T_INT, "DataVersion", int(3465)),
            (T_INT, "xPos", int(0)),
            (T_INT, "zPos", int(0)),
            (T_INT, "yPos", int(-4)),
            (T_STRING, "Status", string("minecraft:full")),
            (T_LIST, "sections", list(vec![section], T_COMPOUND)),
        ])
    }

    fn write_region(dir: &Path, chunk_nbt: &[u8]) {
        use std::io::Write as _;

        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(chunk_nbt).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut payload = Vec::new();
        payload.extend_from_slice(&((compressed.len() + 1) as u32).to_be_bytes());
        payload.push(2);
        payload.extend_from_slice(&compressed);

        let sectors = payload.len().div_ceil(4096);
        let offset = 2usize;
        let mut file = vec![0u8; 8192 + sectors * 4096];
        file[0] = (offset >> 16) as u8;
        file[1] = (offset >> 8) as u8;
        file[2] = offset as u8;
        file[3] = sectors as u8;
        file[offset * 4096..offset * 4096 + payload.len()].copy_from_slice(&payload);
        fs::write(dir.join("r.0.0.mca"), file).unwrap();
    }

    fn write_world(dir: &Path) {
        use std::io::Write as _;

        fs::create_dir_all(dir.join("region")).unwrap();
        let level = root(&[(
            T_COMPOUND,
            "Data",
            compound(&[
                (T_INT, "DataVersion", int(3465)),
                (T_INT, "SpawnX", int(8)),
                (T_INT, "SpawnZ", int(8)),
                (T_STRING, "LevelName", string("map-test")),
            ]),
        )]);
        let mut encoder =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&level).unwrap();
        fs::write(dir.join("level.dat"), encoder.finish().unwrap()).unwrap();
        write_region(&dir.join("region"), &modern_chunk());
    }

    #[test]
    fn renders_a_tile_covering_the_chunk() {
        let dir = std::env::temp_dir().join(format!("xcl-tile-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        write_world(&dir);

        let world = WorldMap::open(&dir).expect("最小世界要能打开");
        // zoom 0、瓦片 (0,0) 覆盖方块 0..256，正好盖住 chunk (0,0)。
        let pixels = render_tile(&world, 128, 128, TILE_PIXELS).expect("要能渲染");
        let pixels = pixels.expect("整块石头不该是全透明");
        assert_eq!(pixels.len(), (TILE_PIXELS * TILE_PIXELS * 4) as usize);

        let png = encode_png(&pixels, TILE_PIXELS, TILE_PIXELS).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn renders_far_away_tile_as_empty() {
        let dir = std::env::temp_dir().join(format!("xcl-tile-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        write_world(&dir);

        let world = WorldMap::open(&dir).expect("最小世界要能打开");
        assert!(render_tile(&world, 8192, 8192, 64)
            .expect("要能渲染")
            .is_none());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn zoomed_tile_downsamples_to_tile_size() {
        let dir = std::env::temp_dir().join(format!("xcl-tile-zoom-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        write_world(&dir);

        let world = WorldMap::open(&dir).expect("最小世界要能打开");
        // zoom 1：渲染 512×512，再缩到 256×256。
        let source = render_tile(&world, 256, 256, TILE_PIXELS << 1)
            .expect("要能渲染")
            .expect("这块有区块");
        let scaled = downsample(&source, TILE_PIXELS << 1, TILE_PIXELS);
        assert_eq!(scaled.len(), (TILE_PIXELS * TILE_PIXELS * 4) as usize);

        fs::remove_dir_all(&dir).ok();
    }
}
