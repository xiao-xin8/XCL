//! 邀请码。
//!
//! 一条码承载「进哪个房间、房主的游戏开在哪个端口」——好友粘一次就能进，
//! 不用再去问「端口多少」。
//!
//! # 为什么端口要进来
//!
//! 端口是房主那边游戏每次开服都可能变的数字，但它必须随码走：
//! 让加入方自己去填，等于把「端口问一下」这件事甩出去，一次能粘完的事变成两步。
//! 所以端口留在码里，房间名 / 密码另存配置 —— 房主下次开房不用重填，码由我们重新生成。
//!
//! # 编码
//!
//! ```text
//! 载荷 = 版本(1) | MC端口(2, LE) | 房主IP(len+bytes) | 房名(len+bytes) | 密码(len+bytes) | 节点(len+bytes)
//! 校验 = 两个字节：sum % 251 与 加权和 % 251
//! 文本 = "XCL" + base32(载荷 + 校验)，每 5 个字符插一个 '-'
//! ```

use std::net::Ipv4Addr;
use xcl_core::AppError;

/// 文本前缀，同时用来识别「这看起来是不是一条邀请码」。
const PREFIX: &str = "XCL";
/// 载荷格式版本。字段布局一改就必须加一，旧客户端据此给出「升级启动器」的提示。
///
/// - v1：初版，字段与 v2 相同但布局有出入；
/// - v2：加上 MC 端口；
/// - v3：`host_ip` 从「房主虚拟 IP」改成「房主局域网映射进隧道后的地址」。
///   免 TUN 模式下虚拟 IP 上没有游戏在听，v2 的码会静默连不上，必须拒掉。
const FORMAT_VERSION: u8 = 3;
/// 每多少个字符插一个分隔符。纯为了肉眼分段，解码时会被忽略。
const GROUP: usize = 5;
/// 房名上限。EasyTier 的网络名没有硬限制，这里限长只是防止有人粘错东西进来。
const MAX_ROOM: usize = 24;
/// 密码上限。
const MAX_SECRET: usize = 64;
/// base32 字母表（RFC 4648，去掉末尾的 padding）。
const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
/// 校验取模的底数，取质数让「少打一个字符」更容易被察觉。
const CHECK_MOD: u32 = 251;

/// 一条邀请码的全部内容 —— 也就是「房间身份」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    /// 房间名，对应 `--network-name`。
    pub room: String,
    /// 房间密码，对应 `--network-secret`。
    pub secret: String,
    /// 房主的转发目标地址：房主局域网映射进隧道后的地址，形如 `10.99.0.7`。
    ///
    /// 不是房主的虚拟 IP（`10.144.144.1`）—— 免 TUN 模式没有虚拟网卡，
    /// 游戏只监听真实网卡，虚拟 IP 上的连接没人接。
    pub host_ip: Ipv4Addr,
    /// 房主那边 Minecraft 开在哪个端口。`0` 表示房主还没扫到游戏。
    pub mc_port: u16,
    /// 共享节点地址，形如 `tcp://host:port`。
    pub node: String,
}

impl Invite {
    /// 编码成可直接粘贴的一行文本。
    ///
    /// 内容不合法时返回 [`AppError`]（`Validation`），文案已经是可以直接展示的中文。
    pub fn encode(&self) -> Result<String, AppError> {
        self.validate()?;

        let mut body = Vec::with_capacity(96);
        body.push(FORMAT_VERSION);
        body.extend_from_slice(&self.mc_port.to_le_bytes());
        for field in [
            self.host_ip.to_string().into_bytes(),
            self.room.trim().as_bytes().to_vec(),
            self.secret.as_bytes().to_vec(),
            self.node.trim().as_bytes().to_vec(),
        ] {
            if field.len() > u8::MAX as usize {
                return Err(AppError::validation(
                    "邀请码内容太长",
                    format!("其中有一段是 {} 字节，最多只能 255", field.len()),
                ));
            }
            body.push(field.len() as u8);
            body.extend_from_slice(&field);
        }

        let (a, b) = checksum(&body);
        body.push(a);
        body.push(b);

        let encoded = base32_encode(&body);
        let mut text = String::with_capacity(PREFIX.len() + encoded.len() + encoded.len() / GROUP);
        text.push_str(PREFIX);
        for (i, ch) in encoded.chars().enumerate() {
            if i > 0 && i % GROUP == 0 {
                text.push('-');
            }
            text.push(ch);
        }
        Ok(text)
    }

    /// 从粘贴的文本解出一条邀请码。
    ///
    /// 容错：忽略空格、换行、分隔符和大小写 —— 从聊天软件里复制经常带上这些。
    /// 但仍然无法容忍「少字符」「多字符」「被改动」，这三种都会在校验位被拦下。
    pub fn decode(input: &str) -> Result<Self, AppError> {
        let cleaned: String = input
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .flat_map(|c| c.to_uppercase())
            .collect();

        if cleaned.is_empty() {
            return Err(AppError::validation("邀请码是空的", "还没粘贴内容")
                .with_hint("把房主发你的整条邀请码复制过来"));
        }
        if !cleaned.starts_with(PREFIX) {
            return Err(AppError::validation(
                "这不是一条邀请码",
                "邀请码以 XCL 开头",
            )
            .with_hint("确认复制的是「邀请码」，而不是房间号或密码"));
        }

        let payload_text = &cleaned[PREFIX.len()..];
        let bytes = base32_decode(payload_text).ok_or_else(|| {
            AppError::validation("邀请码不完整", "有一部分字符无法识别")
                .with_hint("多半是复制时少了一截，请重新复制整条邀请码")
        })?;

        if bytes.len() < 3 {
            return Err(AppError::validation("邀请码不完整", "内容太短")
                .with_hint("请重新复制整条邀请码"));
        }

        let (body, tail) = bytes.split_at(bytes.len() - 2);
        let (a, b) = checksum(body);
        if tail != [a, b] {
            return Err(AppError::validation(
                "邀请码校验没通过",
                "内容与校验位对不上",
            )
            .with_hint("可能复制时少了几个字符，或者中间被改动过"));
        }

        let mut cursor = 0usize;
        let version = take_u8(body, &mut cursor)?;
        if version != FORMAT_VERSION {
            let (title, detail) = if version < FORMAT_VERSION {
                (
                    "这条邀请码是旧格式",
                    format!("旧格式是第 {version} 版，当前程序只认第 {FORMAT_VERSION} 版"),
                )
            } else {
                (
                    "邀请码来自更新版本的启动器",
                    format!("这条码是第 {version} 版格式，当前程序只认第 {FORMAT_VERSION} 版"),
                )
            };
            return Err(AppError::validation(title, detail)
                .with_hint("让房主用新版启动器重新发一条邀请码"));
        }

        let port_lo = take_u8(body, &mut cursor)?;
        let port_hi = take_u8(body, &mut cursor)?;
        let mc_port = u16::from_le_bytes([port_lo, port_hi]);

        let host_ip = take_str(body, &mut cursor, "房主地址")?;
        let room = take_str(body, &mut cursor, "房间名")?;
        let secret = take_str(body, &mut cursor, "房间密码")?;
        let node = take_str(body, &mut cursor, "节点地址")?;

        let host_ip: Ipv4Addr = host_ip.parse().map_err(|_| {
            AppError::validation("邀请码里的地址不合法", format!("读出来是「{host_ip}」"))
        })?;

        let invite = Self {
            room,
            secret,
            host_ip,
            mc_port,
            node,
        };
        invite.validate()?;
        Ok(invite)
    }

    /// 校验字段是否合法。`encode` 与 `decode` 共用，保证两边规则一致。
    fn validate(&self) -> Result<(), AppError> {
        let room = self.room.trim();
        if room.is_empty() {
            return Err(AppError::validation("房间名不能为空", "房间名是给对端认的标识"));
        }
        if room.chars().count() > MAX_ROOM {
            return Err(AppError::validation(
                "房间名太长",
                format!("最多 {MAX_ROOM} 个字符"),
            ));
        }
        if room.chars().any(|c| c.is_control()) {
            return Err(AppError::validation("房间名含有不可见字符", "请换一个名字"));
        }
        if self.secret.is_empty() {
            return Err(AppError::validation("房间密码不能为空", "没有密码，任何人都能进"));
        }
        if self.secret.chars().count() > MAX_SECRET {
            return Err(AppError::validation(
                "房间密码太长",
                format!("最多 {MAX_SECRET} 个字符"),
            ));
        }
        if self.node.trim().is_empty() {
            return Err(AppError::validation("没有指定共享节点", "不知道要连哪台服务器"));
        }
        // 转发目标必须是隧道里能走到的真实地址。回环或 `0.0.0.0` 只可能是码被改坏了，
        // 或者是旧版本生成的码 —— 后者在版本号那一关就该被拦下。
        if self.host_ip.is_loopback() || self.host_ip.is_unspecified() {
            return Err(AppError::validation(
                "邀请码里的房主地址不可用",
                format!("读出来是 {}", self.host_ip),
            )
            .with_hint("让房主用新版启动器重新发一条邀请码"));
        }
        Ok(())
    }
}

/// 两字节校验：一字节求和、一字节加权求和。
///
/// 不是为了防篡改（邀请码本来就不是密文），而是为了拦住「复制时少打了几个字符」
/// 这种最常见的手滑 —— 少一个字符会让字节流整体错位，校验几乎必然对不上。
fn checksum(body: &[u8]) -> (u8, u8) {
    let mut sum: u32 = 0;
    let mut weighted: u32 = 0;
    for (i, b) in body.iter().enumerate() {
        let v = *b as u32;
        sum = sum.wrapping_add(v);
        weighted = weighted.wrapping_add(v.wrapping_mul(i as u32 + 1));
    }
    ((sum % CHECK_MOD) as u8, (weighted % CHECK_MOD) as u8)
}

fn take_u8(body: &[u8], cursor: &mut usize) -> Result<u8, AppError> {
    let byte = *body.get(*cursor).ok_or_else(|| {
        AppError::validation("邀请码不完整", "读到一半就没内容了").with_hint("请重新复制整条邀请码")
    })?;
    *cursor += 1;
    Ok(byte)
}

fn take_str(body: &[u8], cursor: &mut usize, label: &str) -> Result<String, AppError> {
    let len = take_u8(body, cursor)? as usize;
    let end = *cursor + len;
    let slice = body.get(*cursor..end).ok_or_else(|| {
        AppError::validation("邀请码不完整", format!("{label}那一段被截断了"))
            .with_hint("请重新复制整条邀请码")
    })?;
    *cursor = end;
    String::from_utf8(slice.to_vec()).map_err(|_| {
        AppError::validation("邀请码内容损坏", format!("{label}不是合法的文本"))
            .with_hint("请让房主重新生成一条")
    })
}

fn base32_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 8 / 5 + 1);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for &byte in data {
        buf = (buf << 8) | byte as u32;
        bits += 8;
        while bits >= 5 {
            let index = ((buf >> (bits - 5)) & 0x1f) as usize;
            out.push(ALPHABET[index] as char);
            bits -= 5;
        }
        buf &= (1u32 << bits.max(1)) - 1;
    }
    if bits > 0 {
        let index = ((buf << (5 - bits)) & 0x1f) as usize;
        out.push(ALPHABET[index] as char);
    }
    out
}

fn base32_decode(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() * 5 / 8 + 1);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for ch in text.chars() {
        let value = ALPHABET.iter().position(|a| *a as char == ch)? as u32;
        buf = (buf << 5) | value;
        bits += 5;
        if bits >= 8 {
            out.push(((buf >> (bits - 8)) & 0xff) as u8);
            bits -= 8;
        }
        buf &= (1u32 << bits.max(1)) - 1;
    }
    // 末尾不足 8 位的残留必须全是 0，否则说明内容被改动过。
    if bits > 0 && buf != 0 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Invite {
        Invite {
            room: "xcl-测试房-01".to_string(),
            secret: "s3cret-秘钥".to_string(),
            host_ip: Ipv4Addr::new(10, 99, 0, 7),
            mc_port: 25565,
            node: "tcp://relay.example.com:11010".to_string(),
        }
    }

    #[test]
    fn roundtrip_preserves_fields() {
        let invite = sample();
        let code = invite.encode().expect("编码应当成功");
        assert!(code.starts_with("XCL"));
        let back = Invite::decode(&code).expect("解码应当成功");
        assert_eq!(back, invite);
    }

    /// 端口随码走：加入方粘一条就能拿到房主的游戏端口，不用再问。
    /// 房主还没扫到游戏时端口是 0 —— 码照样能出，加入方那边会说明原因。
    #[test]
    fn port_travels_with_the_code() {
        let mut invite = sample();
        invite.mc_port = 25577;
        let back = Invite::decode(&invite.encode().unwrap()).unwrap();
        assert_eq!(back.mc_port, 25577);

        invite.mc_port = 0;
        let code = invite.encode().expect("端口未知也要能出码");
        assert_eq!(Invite::decode(&code).unwrap().mc_port, 0);
    }

    #[test]
    fn tolerates_spaces_dashes_and_lowercase() {
        let code = sample().encode().unwrap();
        let messy = format!(
            "  {}  ",
            code.to_lowercase().replace('-', " - ").replace("-  -", "-")
        );
        let back = Invite::decode(&messy).expect("应当照样解得出来");
        assert_eq!(back.room, sample().room);
    }

    #[test]
    fn truncated_code_fails_checksum() {
        let code = sample().encode().unwrap();
        // 掐掉尾部 6 个字符（大约一个字节）
        let broken = &code[..code.len() - 6];
        let err = Invite::decode(broken).expect_err("应当失败");
        assert!(err.hint.is_some(), "必须给出下一步建议");
    }

    #[test]
    fn tampered_char_fails_checksum() {
        let code = sample().encode().unwrap();
        let mut chars: Vec<char> = code.chars().collect();
        // 换掉中间靠后的一个非分隔符字符
        let idx = chars.len() / 2;
        let original = chars[idx];
        chars[idx] = if original == 'A' { 'B' } else { 'A' };
        let broken: String = chars.into_iter().collect();
        assert!(Invite::decode(&broken).is_err());
    }

    #[test]
    fn empty_and_wrong_prefix_have_hints() {
        let empty = Invite::decode("   ").unwrap_err();
        assert_eq!(empty.code, xcl_core::ErrorCode::Validation);
        assert!(empty.title.contains("空"));

        let wrong = Invite::decode("hello world").unwrap_err();
        assert!(wrong.title.contains("不是一条邀请码"));
    }

    #[test]
    fn invalid_fields_rejected_on_encode() {
        let mut invite = sample();
        invite.room = "   ".to_string();
        assert!(invite.encode().is_err());

        let mut invite = sample();
        invite.secret.clear();
        assert!(invite.encode().is_err());

        let mut invite = sample();
        invite.node = " ".to_string();
        assert!(invite.encode().is_err());
    }

    /// 旧码必须给出「让房主重发一条」，而不是含糊地说格式不对 ——
    /// 手里那条码本身没坏，是版本变了。
    #[test]
    fn old_format_is_rejected_with_a_hint() {
        // v1（无端口语义）与 v2（端口指向虚拟 IP）都构造一遍：两种都得被拒。
        for version in [1u8, 2] {
            let mut body = vec![version];
            body.extend_from_slice(&25565u16.to_le_bytes());
            for field in [
                b"10.144.144.1".to_vec(),
                b"old-room".to_vec(),
                b"old-secret".to_vec(),
                b"tcp://relay.example.com:11010".to_vec(),
            ] {
                body.push(field.len() as u8);
                body.extend_from_slice(&field);
            }
            let (a, b) = checksum(&body);
            body.push(a);
            body.push(b);
            let code = format!("XCL{}", base32_encode(&body));

            let err = Invite::decode(&code).expect_err("旧版本应当被拒");
            assert!(
                err.title.contains("旧格式"),
                "v{version} 文案要说清是版本问题：{}",
                err.title
            );
            assert!(err.hint.is_some());
        }
    }

    /// 转发目标不能是回环 / `0.0.0.0`：那两种地址永远连不到房主的游戏。
    #[test]
    fn loopback_forward_target_is_rejected() {
        let mut invite = sample();
        invite.host_ip = Ipv4Addr::LOCALHOST;
        assert!(invite.encode().is_err());

        let mut invite = sample();
        invite.host_ip = Ipv4Addr::UNSPECIFIED;
        assert!(invite.encode().is_err());
    }

    #[test]
    fn overlong_room_rejected() {
        let mut invite = sample();
        invite.room = "长".repeat(MAX_ROOM + 1);
        assert!(invite.encode().is_err());
    }

    #[test]
    fn base32_roundtrip() {
        for len in 1..24usize {
            let data: Vec<u8> = (0..len).map(|i| (i * 37 % 251) as u8).collect();
            let text = base32_encode(&data);
            assert_eq!(base32_decode(&text).unwrap(), data, "长度 {len} 往返失败");
        }
    }

    #[test]
    fn invite_stays_short_enough() {
        let code = sample().encode().unwrap();
        // 约 100 字符上下；若哪天字段变多，这条会提醒我们重新评估好不好粘贴
        assert!(code.len() < 160, "邀请码太长了：{}", code.len());
    }
}
