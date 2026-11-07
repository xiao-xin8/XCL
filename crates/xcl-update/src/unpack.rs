//! 从压缩包里取出启动器。
//!
//! 两条硬规则：
//!
//! 1. 只认清单里写的那个文件名，不按压缩包内的路径落盘 ——
//!    否则一个 `../../AppData/Roaming/Microsoft/Windows/Start Menu/…` 就能
//!    在别处丢文件（zip slip）。
//! 2. 解压后再校验一次大小和 SHA-256。压缩包的哈希对只能证明「包没坏」，
//!    证明不了「解出来的字节对」—— 磁盘写满、写入中断都可能产出长度正确但
//!    内容错误的东西。

use std::fs::File;
use std::io::Read;
use std::path::Path;

use xcl_core::AppError;

use crate::verify;

/// 解压大小的宽容度。比清单声明的大一点点也算异常 —— 这是 zip bomb 的闸门。
const SIZE_SLACK: u64 = 4096;

/// 把 `archive` 里名为 `expect_name` 的那个文件解到 `dest`，然后校验。
///
/// 失败时 `dest` 要么不存在、要么已被删除 —— 不会留下半截文件让上层误用。
pub fn extract_exe(
    archive: &Path,
    dest: &Path,
    expect_name: &str,
    expect_size: u64,
    expect_sha256: &str,
) -> Result<(), AppError> {
    let file = File::open(archive).map_err(|err| AppError::io(&err, "打开更新包"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|err| {
        AppError::parse("更新包已损坏", format!("无法读取压缩包：{err}"))
            .with_hint("文件可能在下载时损坏了，重试一次通常就好")
            .with_retryable(true)
    })?;

    // 先只找下标，不长期持有条目（ZipArchive 是 &mut 借出的）
    let total = zip.len();
    let mut wanted: Option<usize> = None;
    for index in 0..total {
        let (is_dir, name) = {
            let entry = zip
                .by_index_raw(index)
                .map_err(|err| AppError::parse("更新包已损坏", err.to_string()))?;
            (entry.is_dir(), entry.name().to_string())
        };
        if is_dir {
            continue;
        }
        // 比的是文件名：压缩包里带不带目录都无所谓，落盘路径由我们自己定
        let base = Path::new(&name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if base.eq_ignore_ascii_case(expect_name) {
            wanted = Some(index);
            break;
        }
    }

    let Some(index) = wanted else {
        return Err(AppError::parse(
            "更新包里找不到启动器",
            format!("压缩包里没有「{expect_name}」"),
        )
        .with_hint("更新包可能不完整，请重试"));
    };

    let entry = zip
        .by_index(index)
        .map_err(|err| AppError::parse("更新包已损坏", err.to_string()))?;

    // 压缩包自己声明的解压大小先卡一道：和清单差太多说明被改过
    if entry.size() > expect_size + SIZE_SLACK {
        return Err(AppError::parse(
            "更新包内容不对",
            format!(
                "压缩包里的文件声明为 {} 字节，清单写的是 {expect_size}",
                entry.size()
            ),
        )
        .with_hint("这个更新包可能被人改过，已中止更新"));
    }

    let mut out = File::create(dest).map_err(|err| AppError::io(&err, "写入更新文件"))?;
    // 再卡一道：最多只读这么多字节，超了就停，绝不让它把磁盘吃满
    let mut limited = entry.take(expect_size + SIZE_SLACK + 1);
    let written = std::io::copy(&mut limited, &mut out)
        .map_err(|err| AppError::io(&err, "解压更新文件"))?;
    let _ = out.sync_all();
    drop(out);

    if written != expect_size {
        let _ = std::fs::remove_file(dest);
        return Err(AppError::parse(
            "更新包内容不对",
            format!("解压出 {written} 字节，清单写的是 {expect_size}"),
        )
        .with_hint("这个更新包可能不完整，已中止更新"));
    }

    // 最后是内容：解压过程本身也可能出问题
    if let Err(err) = verify::verify_artifact(dest, expect_size, expect_sha256) {
        let _ = std::fs::remove_file(dest);
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_zip(dir: &Path, entries: &[(&str, &[u8])]) -> std::path::PathBuf {
        let path = dir.join("pack.zip");
        let file = File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in entries {
            writer.start_file(*name, opts).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
        path
    }

    const NAME: &str = "Xin Craft Launcher.exe";

    #[test]
    fn extracts_named_entry_and_verifies() {
        let dir = crate::testutil::temp_dir("unpack-ok");
        let payload = b"new-build-bytes-here";
        let zip = make_zip(&dir, &[(NAME, payload)]);
        let dest = dir.join("out.exe");

        extract_exe(
            &zip,
            &dest,
            NAME,
            payload.len() as u64,
            &verify::sha256_bytes(payload),
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), payload);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finds_entry_even_inside_a_folder() {
        // 压缩包里带不带目录都行：落盘路径由我们定，只看文件名
        let dir = crate::testutil::temp_dir("unpack-nested");
        let payload = b"payload";
        let nested = format!("build/{NAME}");
        let zip = make_zip(&dir, &[(&nested, payload)]);
        let dest = dir.join("out.exe");

        extract_exe(
            &zip,
            &dest,
            NAME,
            payload.len() as u64,
            &verify::sha256_bytes(payload),
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), payload);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_entry_is_an_error() {
        let dir = crate::testutil::temp_dir("unpack-missing");
        let zip = make_zip(&dir, &[("other.exe", b"x")]);
        let dest = dir.join("out.exe");

        let err = extract_exe(&zip, &dest, NAME, 1, &verify::sha256_bytes(b"x")).unwrap_err();
        assert!(err.title.contains("找不到"));
        assert!(!dest.exists(), "失败时不该留下文件");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn wrong_hash_leaves_no_file_behind() {
        let dir = crate::testutil::temp_dir("unpack-hash");
        let payload = b"tampered-bytes";
        let zip = make_zip(&dir, &[(NAME, payload)]);
        let dest = dir.join("out.exe");

        // 哈希是另一份内容的：长度对、内容不对
        let wrong = verify::sha256_bytes(b"something-else!!!");
        let err = extract_exe(&zip, &dest, NAME, payload.len() as u64, &wrong).unwrap_err();
        assert!(!err.title.is_empty());
        assert!(!dest.exists(), "校验不过的产物必须删掉，否则会被当成下好了");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn refuses_when_unpacked_size_much_bigger_than_declared() {
        // zip bomb：清单说 16 字节，包里塞了几千字节
        let dir = crate::testutil::temp_dir("unpack-bomb");
        let payload = vec![b'A'; 8_000];
        let zip = make_zip(&dir, &[(NAME, &payload)]);
        let dest = dir.join("out.exe");

        let err = extract_exe(&zip, &dest, NAME, 16, &verify::sha256_bytes(&payload)).unwrap_err();
        assert!(err.title.contains("内容不对"), "实际：{}", err.title);
        assert!(!dest.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn broken_zip_is_a_clear_error() {
        let dir = crate::testutil::temp_dir("unpack-broken");
        let path = dir.join("pack.zip");
        std::fs::write(&path, b"this is not a zip").unwrap();
        let dest = dir.join("out.exe");

        let err = extract_exe(&path, &dest, NAME, 10, "0".repeat(64).as_str()).unwrap_err();
        assert!(err.title.contains("损坏"));
        assert!(err.retryable, "坏包重试一次通常就好，应该标成可重试");
        std::fs::remove_dir_all(&dir).ok();
    }
}
