#!/usr/bin/env python3
"""发一个自更新版本：构建产物 -> 哈希 -> 清单 -> 签名。

用法（在项目任意位置）：

    python xcl-launcher/scripts/release.py            # 用 Cargo.toml 里的版本号
    python xcl-launcher/scripts/release.py --dry-run  # 只打印，不写文件

它做的事：

1. 从 `xcl-launcher/Cargo.toml` 读 `[workspace.package] version` —— **版本号唯一来源**；
2. 在 `target/release/` 里找构建出来的 exe；
3. 算 SHA-256 和字节数；
4. 把 exe 复制成 `<cloud>/XCL-<版本>.exe`；
5. 用 `<cloud>/release-notes.json`（人工维护）补上标题、更新内容、是否强制；
6. 写出 `<cloud>/update.json`；
7. 给清单签名，生成 `update.json.sig`。默认调 `cargo run --example sign`；
   已经编译过 sign 时用 `--sign-bin` 直接跑它，不触发构建。

为什么要有这个脚本：手抄哈希进 JSON，十次里必错一次，而错了的表现是
「下载完校验失败」，很难查。这里全部算出来，签名也一起做掉。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path
from urllib.parse import quote

# 脚本在 <repo>/scripts/ 下，仓库根是它的父目录；cloud 在**项目根**（仓库外）
SCRIPT = Path(__file__).resolve()
REPO = SCRIPT.parent.parent
PROJECT = REPO.parent
CLOUD = PROJECT / "cloud"
NOTES = CLOUD / "release-notes.json"

SCHEMA = 2


def read_version() -> str:
    """从 workspace Cargo.toml 读版本。tauri.conf.json 和前端都不再各存一份。"""
    text = (REPO / "Cargo.toml").read_text(encoding="utf-8")
    # 只认 [workspace.package] 段里的 version，避免撞上依赖的版本
    match = re.search(r"\[workspace\.package\](.*?)(\n\[|\Z)", text, re.S)
    if not match:
        raise SystemExit("Cargo.toml 里没有 [workspace.package] 段")
    version = re.search(r'^version\s*=\s*"([^"]+)"', match.group(1), re.M)
    if not version:
        raise SystemExit("[workspace.package] 里没有 version")
    return version.group(1)


def find_exe() -> Path:
    """找构建产物。`mainBinaryName` 生效后是全名，兜底认旧名。"""
    release = REPO / "target" / "release"
    for name in ("Xin Craft Launcher.exe", "XCL.exe", "xcl-desktop.exe"):
        candidate = release / name
        if candidate.is_file():
            return candidate
    raise SystemExit(
        f"在 {release} 里找不到构建产物。"
        "先跑 `cd apps/desktop && cargo tauri build --no-bundle`。"
    )


def sync_tauri_version(version: str) -> None:
    """把版本写进 tauri.conf.json。

    PE 里的 FileVersion / ProductVersion 取自这个字段 —— 不写就没有版本信息。
    版本号的**唯一来源**仍然是 Cargo.toml，这里只是把它同步过去。

    注意**必须在构建之前跑**：exe 里的版本信息是编译期写进去的，
    构建完再改配置，产物里还是旧版本号。
    """
    conf = REPO / "apps" / "desktop" / "src-tauri" / "tauri.conf.json"
    text = conf.read_text(encoding="utf-8")
    updated, count = re.subn(
        r'("version"\s*:\s*")[^"]+(")', rf"\g<1>{version}\g<2>", text, count=1
    )
    if count == 0:
        raise SystemExit(f"{conf} 里没有顶层 version 字段")
    if updated != text:
        conf.write_text(updated, encoding="utf-8")
        print(f"已同步    {conf} → {version}")
    else:
        print(f"已是最新  {conf} → {version}")


def read_tauri_version() -> str:
    conf = REPO / "apps" / "desktop" / "src-tauri" / "tauri.conf.json"
    text = conf.read_text(encoding="utf-8")
    match = re.search(r'"version"\s*:\s*"([^"]+)"', text)
    if not match:
        raise SystemExit("tauri.conf.json 里没有顶层 version")
    return match.group(1)


def digest(path: Path) -> tuple[str, int]:
    hasher = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            hasher.update(chunk)
            size += len(chunk)
    return hasher.hexdigest(), size


def build_archive(exe: Path, out: Path) -> None:
    """把 exe 压成 zip，里面只放这一个文件。

    条目名用纯文件名、不带目录：解压端按文件名匹配，它不相信压缩包里的路径。
    `compresslevel=9` 对 20MB 的 exe 只多花一两秒，换来的体积差是实打实的。
    """
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
        zf.write(exe, arcname=exe.name)

    # 打完立刻自检：条目必须恰好一个、名字对得上、内容没坏。
    # 这一步能省掉「发出去之后客户端说包里找不到启动器」那一整轮排查。
    with zipfile.ZipFile(out) as zf:
        names = zf.namelist()
        if names != [exe.name]:
            raise SystemExit(f"压缩包内容不对，条目是 {names}，应该是 {[exe.name]}")
        if zf.testzip() is not None:
            raise SystemExit("压缩包自检失败，文件可能写坏了")


def main() -> int:
    parser = argparse.ArgumentParser(description="生成自更新清单并签名")
    parser.add_argument("--dry-run", action="store_true", help="只打印，不写文件")
    parser.add_argument(
        "--base-url",
        default=None,
        help="更新包所在目录的地址，覆盖 release-notes.json 里的 baseUrl",
    )
    parser.add_argument(
        "--sync-version",
        action="store_true",
        help="只把 Cargo.toml 的版本同步到 tauri.conf.json 然后退出（构建前跑）",
    )
    parser.add_argument(
        "--version",
        default=None,
        help="覆盖清单里的版本号（演练用：拿 0.2.1 的 exe 冒充 0.2.2）",
    )
    parser.add_argument(
        "--sign-bin",
        default=None,
        help="用已编译好的 sign 程序签名，跳过 cargo run",
    )
    args = parser.parse_args()

    version = read_version()
    if args.sync_version:
        sync_tauri_version(version)
        print("\n版本已就绪，现在可以构建：")
        print("  cd apps/desktop && cargo tauri build --no-bundle")
        return 0

    if not NOTES.is_file():
        raise SystemExit(f"缺少 {NOTES}，先写一份更新说明")
    notes = json.loads(NOTES.read_text(encoding="utf-8"))

    exe = find_exe()
    sha256, size = digest(exe)

    # 产物里的版本信息是编译期写死的：配置和 Cargo 不一致，说明这份 exe
    # 是用旧版本号构建的，发出去 PE 里就是错的
    conf_version = read_tauri_version()
    if args.version:
        # 演练模式：清单版本和产物版本故意不一致，这里不拦，只提示
        print(f"⚠ 演练模式：清单写 {args.version}，产物是 {conf_version}（更新完会再次提示）")
        version = args.version
    elif conf_version != version:
        raise SystemExit(
            f"tauri.conf.json 的版本是 {conf_version}，Cargo.toml 是 {version}。\n"
            "先跑 `python scripts/release.py --sync-version`，再重新构建，否则 exe 的版本信息是旧的。"
        )

    base_url = (args.base_url or notes.get("baseUrl") or "").rstrip("/")
    if not base_url:
        raise SystemExit("没有 baseUrl：在 release-notes.json 里填，或用 --base-url 传")
    if not (base_url.startswith("https://") or "127.0.0.1" in base_url or "localhost" in base_url):
        raise SystemExit("baseUrl 必须是 https（本机回环地址除外）")

    # 分发名就是程序名，不带版本号：
    # 自更新替换的是「当前正在运行的那个文件名」，名字里带版本会导致
    # 每更新一次文件名变一次，用户的快捷方式/固定到任务栏的图标全断。
    exe_name = exe.name
    # 压缩包名带版本号：每次发的包内容不同，CDN / 浏览器缓存不会串。
    # 空格一律换成连字符 —— URL 里编码成 %20 虽然能用，但不好看也不好排查。
    archive_name = f"Xin-Craft-Launcher-{version}.zip"

    # 压缩只在临时目录做，dry-run 不留任何文件
    tmp_dir = Path(tempfile.mkdtemp(prefix="xcl-release-"))
    try:
        archive_src = tmp_dir / archive_name
        build_archive(exe, archive_src)
        archive_sha256, archive_size = digest(archive_src)

        manifest = {
            "schema": SCHEMA,
            "version": version,
            "channel": notes.get("channel", "stable"),
            "publishedAt": notes.get("publishedAt") or _now(),
            "severity": notes.get("severity", "normal"),
            "force": bool(notes.get("force", False)),
            "title": notes.get("title", ""),
            "minSupported": notes.get("minSupported", ""),
            "notes": notes.get("notes", ""),
            "changelog": notes.get("changelog", []),
            "notesUrl": notes.get("notesUrl", ""),
            "archive": {
                "url": f"{base_url}/{quote(archive_name)}",
                "size": archive_size,
                "sha256": archive_sha256,
            },
            "exe": {
                "name": exe_name,
                "size": size,
                "sha256": sha256,
            },
        }
        # 空字符串字段会让后端当成合法值（比如 minSupported=""），干脆去掉
        manifest = {key: value for key, value in manifest.items() if value != ""}

        saved = size - archive_size
        ratio = archive_size / size * 100 if size else 0
        print(f"版本      {version}")
        print(f"产物      {exe}")
        print(f"原始大小  {size} 字节")
        print(f"压缩后    {archive_size} 字节（{ratio:.1f}%，省了 {saved} 字节）")
        print(f"包地址    {manifest['archive']['url']}")
        print(f"强制更新  {'是' if manifest['force'] else '否'}")

        if args.dry_run:
            print("\n--- update.json ---")
            print(json.dumps(manifest, ensure_ascii=False, indent=2))
            return 0

        CLOUD.mkdir(parents=True, exist_ok=True)
        target_archive = CLOUD / archive_name
        shutil.copy2(archive_src, target_archive)
        print(f"已放置    {target_archive}")
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)

    manifest_path = CLOUD / "update.json"
    manifest_path.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(f"已写出    {manifest_path}")

    # 签名走 Rust 示例：私钥在 %USERPROFILE%\.xcl\ 下，python 这边没有 ed25519。
    # 机器上已有编译好的 sign 时可以直接指定，避免为了签一次名再起一轮构建。
    if args.sign_bin:
        sign_bin = Path(args.sign_bin)
        if not sign_bin.is_file():
            raise SystemExit(f"找不到签名程序：{sign_bin}")
        sign_cmd = [str(sign_bin.resolve()), str(manifest_path)]
    else:
        sign_cmd = [
            "cargo",
            "run",
            "-q",
            "-p",
            "xcl-update",
            "--example",
            "sign",
            "--",
            str(manifest_path),
        ]
    signed = subprocess.run(
        sign_cmd,
        cwd=REPO,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if signed.returncode != 0:
        print(signed.stdout)
        print(signed.stderr)
        raise SystemExit("签名失败")
    print(f"已签名    {manifest_path}.sig")

    print("\n下一步：")
    print(f"  cd {CLOUD}")
    print("  python -m http.server 8765 --bind 127.0.0.1")
    return 0


def _now() -> str:
    from datetime import datetime, timezone

    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


if __name__ == "__main__":
    sys.exit(main())
