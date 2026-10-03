//! 设备档案。
//!
//! `device.json` 位于 EXE 当前目录，EXE 是唯一写者，DLL 从不读写。
//! 缺失/损坏时重生成；`keepLoginKey` 缺席按无 key 处理（不报错）。

use serde::{Deserialize, Serialize};

use crate::consts::FILE_DEVICE;
use crate::enc;
use crate::paths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// `XX-XX-XX-XX-XX-XX`，大写连字符，虚拟单播。
    pub mac_id: String,
    /// `{SEG0}:{SEG1}:`（第三段空，保留末尾冒号原文发送）。
    pub device_id: String,
    /// `DESKTOP-` + 7 位大写字母数字。
    pub ep_name: String,
    /// 首个私网 IPv4。
    pub ep_ip: String,
    /// `ULSKLK-…` 或 None（无 key 走 QR）。
    pub keep_login_key: Option<String>,
    /// 上次成功启动游戏所用的大区 id；`--area` 未指定时作为默认值（无则进菜单）。
    pub last_area_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DeviceFile {
    #[serde(rename = "macId", default)]
    mac_id: String,
    #[serde(rename = "deviceId", default)]
    device_id: String,
    #[serde(rename = "epName", default)]
    ep_name: String,
    #[serde(rename = "epIp", default)]
    ep_ip: String,
    #[serde(rename = "keepLoginKey", default)]
    keep_login_key: String,
    /// 本实现的扩展字段：记住上次成功登录的大区。
    #[serde(rename = "lastAreaId", default)]
    last_area_id: String,
}

impl Device {
    pub fn generate() -> Result<Device, String> {
        let ep_ip =
            local_private_ipv4().ok_or_else(|| "本机未找到私网 IPv4 地址，中止登录".to_string())?;
        let mac_id = generate_mac_id();
        Ok(Device {
            device_id: make_device_id(&mac_id),
            mac_id,
            ep_name: generate_ep_name(),
            ep_ip,
            keep_login_key: None,
            last_area_id: None,
        })
    }

    pub(crate) fn segments(&self) -> (String, String) {
        let mut it = self.device_id.split(':');
        let s0 = it.next().unwrap_or_default().to_string();
        let s1 = it.next().unwrap_or_default().to_string();
        (s0, s1)
    }

    /// `deviceId` 的 `SEG0` 是否等于 `MD5(macId)`。
    pub fn seg0_consistent(&self) -> bool {
        self.segments().0 == enc::md5_hex_upper(&self.mac_id)
    }

    /// 读取当前目录 `device.json`；缺失/损坏返回 None。
    pub(crate) fn load_from(path: &std::path::Path) -> Option<Device> {
        let bytes = std::fs::read(path).ok()?;
        let f: DeviceFile = serde_json::from_slice(&bytes).ok()?;
        Some(Device {
            mac_id: f.mac_id,
            device_id: f.device_id,
            ep_name: f.ep_name,
            ep_ip: f.ep_ip,
            keep_login_key: normalize_key(&f.keep_login_key),
            last_area_id: normalize_area_id(&f.last_area_id),
        })
    }

    /// 载入或生成并立即落盘（原子写）。返回 `(device, 是否新建)`。
    pub fn load_or_create() -> Result<(Device, bool), String> {
        let path = paths::cwd_file(FILE_DEVICE);
        let existing = Device::load_from(&path);
        let created = existing.is_none();
        let mut dev = existing.unwrap_or(Device::generate()?);
        let before = dev.clone();
        dev.repair()?;
        if created || dev != before {
            dev.save_atomic(&path)
                .map_err(|e| format!("写 {} 失败: {e}", path.display()))?;
        }
        Ok((dev, created))
    }

    /// 逐字段校验并就地修复（只重生成非法字段）。
    pub fn repair(&mut self) -> Result<(), String> {
        if !is_valid_mac(&self.mac_id) {
            self.mac_id = generate_mac_id();
        }
        let seg1 = self.segments().1;
        if !self.seg0_consistent() || !is_hex32(&seg1) {
            self.device_id = make_device_id(&self.mac_id);
        }
        if !is_valid_ep_name(&self.ep_name) {
            self.ep_name = generate_ep_name();
        }
        if self.ep_ip.parse::<std::net::Ipv4Addr>().is_err() || !is_private_v4(&self.ep_ip) {
            let ip = local_private_ipv4()
                .ok_or_else(|| "本机未找到私网 IPv4 地址，中止登录".to_string())?;
            self.ep_ip = ip;
        }
        self.keep_login_key = self.keep_login_key.as_deref().and_then(normalize_key);
        self.last_area_id = self.last_area_id.as_deref().and_then(normalize_area_id);
        Ok(())
    }

    /// 原子写 `device.json`（写 tmp 后改名）。
    pub fn save_atomic(&self, path: &std::path::Path) -> std::io::Result<()> {
        let f = DeviceFile {
            mac_id: self.mac_id.clone(),
            device_id: self.device_id.clone(),
            ep_name: self.ep_name.clone(),
            ep_ip: self.ep_ip.clone(),
            keep_login_key: self.keep_login_key.clone().unwrap_or_default(),
            last_area_id: self.last_area_id.clone().unwrap_or_default(),
        };
        let text = serde_json::to_string_pretty(&f).unwrap_or_else(|_| "{}".to_string());
        paths::write_atomic(path, text.as_bytes())
    }

    /// 更新 `keepLoginKey` 并立即原子写盘。
    pub fn set_keep_login_key(
        &mut self,
        key: Option<String>,
        path: &std::path::Path,
    ) -> std::io::Result<()> {
        self.keep_login_key = key.and_then(|k| normalize_key(&k));
        self.save_atomic(path)
    }

    /// 记住"上次成功登录的大区"并立即原子写盘。
    pub fn set_last_area_id(
        &mut self,
        area_id: &str,
        path: &std::path::Path,
    ) -> std::io::Result<()> {
        self.last_area_id = normalize_area_id(area_id);
        self.save_atomic(path)
    }
}

/// 大区 id 只接受十进制数字串（避免把脏数据写进 device.json）。
fn normalize_area_id(raw: &str) -> Option<String> {
    let t = raw.trim();
    if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) {
        Some(t.to_string())
    } else {
        None
    }
}

fn normalize_key(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

pub(crate) fn make_device_id(mac_id: &str) -> String {
    format!(
        "{}:{}:",
        enc::md5_hex_upper(mac_id),
        enc::random_hex_upper(16)
    )
}

pub(crate) fn generate_ep_name() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rnd = [0u8; 7];
    if getrandom::fill(&mut rnd).is_err() {
        enc::fill_from_clock(&mut rnd);
    }
    let mut s = String::from("DESKTOP-");
    for b in rnd {
        s.push(ALPHABET[(b as usize) % ALPHABET.len()] as char);
    }
    s
}

/// 虚拟单播 MAC：优先取本机网卡 OUI 号段 + 3 字节随机；首字节最低位清零。
pub(crate) fn generate_mac_id() -> String {
    let mut b = [0u8; 6];
    match adapters::first_oui() {
        Some(oui) => b[..3].copy_from_slice(&oui),
        None => {
            let mut oui = [0u8; 3];
            let _ = getrandom::fill(&mut oui);
            oui[0] &= 0xFE;
            b[..3].copy_from_slice(&oui);
        }
    }
    let mut rest = [0u8; 3];
    let _ = getrandom::fill(&mut rest);
    b[3..].copy_from_slice(&rest);
    b[0] &= 0xFE; // 单播（首字节最低位 0）
    b.iter()
        .map(|x| format!("{:02X}", x))
        .collect::<Vec<_>>()
        .join("-")
}

pub fn is_valid_mac(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 6
        && parts.iter().all(|p| {
            p.len() == 2
                && p.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase())
        })
        && !s.chars().all(|c| c == '0' || c == '-')
}

pub(crate) fn is_valid_ep_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.is_ascii()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_hex32(s: &str) -> bool {
    s.len() == 32
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase())
}

pub(crate) fn is_private_v4(s: &str) -> bool {
    match s.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => {
            let o = ip.octets();
            o[0] == 10 || (o[0] == 172 && (16..=31).contains(&o[1])) || (o[0] == 192 && o[1] == 168)
        }
        Err(_) => false,
    }
}

/// 首个私网 IPv4：优先网卡枚举，其次 UDP 路由探测。
pub(crate) fn local_private_ipv4() -> Option<String> {
    if let Some(ip) = adapters::first_private_ipv4() {
        return Some(ip);
    }
    // 兜底：不实际发包，仅取路由选中的本地地址。
    if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("114.114.114.114:53").is_ok() {
            if let Ok(addr) = sock.local_addr() {
                let ip = addr.ip().to_string();
                if is_private_v4(&ip) {
                    return Some(ip);
                }
            }
        }
    }
    None
}

mod adapters {
    use windows_sys::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetAdaptersInfo, IP_ADAPTER_INFO};

    pub struct Adapter {
        pub mac: Vec<u8>,
        pub if_type: u32,
        pub ips: Vec<String>,
    }

    /// `GetAdaptersInfo`（iphlpapi）枚举，输出缓冲区按 8 字节对齐。
    pub fn enumerate() -> Vec<Adapter> {
        let mut size: u32 = 16 * 1024;
        let mut words = vec![0u64; (size as usize).div_ceil(8)];
        let mut ret =
            unsafe { GetAdaptersInfo(words.as_mut_ptr() as *mut IP_ADAPTER_INFO, &mut size) };
        if ret == ERROR_BUFFER_OVERFLOW {
            words = vec![0u64; (size as usize).div_ceil(8)];
            ret = unsafe { GetAdaptersInfo(words.as_mut_ptr() as *mut IP_ADAPTER_INFO, &mut size) };
        }
        if ret != 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut p = words.as_ptr() as *const IP_ADAPTER_INFO;
        let mut guard = 0;
        while !p.is_null() && guard < 256 {
            guard += 1;
            let a = unsafe { &*p };
            let n = (a.AddressLength as usize).min(a.Address.len());
            let mut ips = Vec::new();
            let mut ip_node: *const windows_sys::Win32::NetworkManagement::IpHelper::IP_ADDR_STRING =
                &a.IpAddressList;
            let mut ip_guard = 0;
            while !ip_node.is_null() && ip_guard < 64 {
                ip_guard += 1;
                let node = unsafe { &*ip_node };
                if let Some(s) = cstr_i8(&node.IpAddress.String) {
                    if !s.is_empty() {
                        ips.push(s);
                    }
                }
                ip_node = node.Next;
            }
            out.push(Adapter {
                mac: a.Address[..n].to_vec(),
                if_type: a.Type,
                ips,
            });
            p = a.Next;
        }
        out
    }

    fn cstr_i8(buf: &[i8]) -> Option<String> {
        let bytes: Vec<u8> = buf
            .iter()
            .take_while(|c| **c != 0)
            .map(|c| *c as u8)
            .collect();
        String::from_utf8(bytes).ok()
    }

    const IF_TYPE_ETHERNET: u32 = 6;
    const IF_TYPE_IEEE80211: u32 = 71;

    /// 本机真实网卡的前三字节（OUI 号段）。
    pub fn first_oui() -> Option<[u8; 3]> {
        for a in enumerate() {
            let physical = a.if_type == IF_TYPE_ETHERNET || a.if_type == IF_TYPE_IEEE80211;
            if physical && a.mac.len() == 6 && a.mac.iter().any(|b| *b != 0) && a.mac[0] & 1 == 0 {
                return Some([a.mac[0], a.mac[1], a.mac[2]]);
            }
        }
        None
    }

    pub fn first_private_ipv4() -> Option<String> {
        // 先看有实体网卡的适配器，再看其余。
        let mut candidates: Vec<(bool, String)> = Vec::new();
        for a in enumerate() {
            let physical = a.if_type == IF_TYPE_ETHERNET || a.if_type == IF_TYPE_IEEE80211;
            for ip in a.ips {
                if super::is_private_v4(&ip) {
                    candidates.push((physical, ip));
                }
            }
        }
        candidates.sort_by_key(|(physical, _)| !*physical);
        candidates.into_iter().next().map(|(_, ip)| ip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_and_device_id_rules() {
        let mac = generate_mac_id();
        assert!(is_valid_mac(&mac), "mac={mac}");
        assert_eq!(mac.len(), 17);
        // 单播：首字节最低位 0
        let first = u8::from_str_radix(&mac[0..2], 16).unwrap();
        assert_eq!(first & 1, 0);
        // MD5(macId) 已知向量
        assert_eq!(
            enc::md5_hex_upper("00-11-22-33-44-55"),
            "88440FF9DD5D5C6819D6D4652279A1BC"
        );
        let did = make_device_id(&mac);
        assert!(did.ends_with(':'));
        assert_eq!(did.matches(':').count(), 2);
        let (s0, s1) = {
            let d = Device {
                mac_id: mac.clone(),
                device_id: did.clone(),
                ep_name: generate_ep_name(),
                ep_ip: "192.168.1.2".into(),
                keep_login_key: None,
                last_area_id: None,
            };
            assert!(d.seg0_consistent());
            d.segments()
        };
        assert_eq!(s0, enc::md5_hex_upper(&mac));
        assert!(is_hex32(&s1));
    }

    #[test]
    fn ep_name_is_ascii_virtual() {
        let n = generate_ep_name();
        assert!(is_valid_ep_name(&n));
        assert_eq!(n.len(), 15);
        assert!(n.starts_with("DESKTOP-"));
        assert!(!is_valid_ep_name("中文主机"));
        assert!(!is_valid_ep_name(""));
    }

    #[test]
    fn private_ip_detection() {
        assert!(is_private_v4("192.168.1.100"));
        assert!(is_private_v4("10.1.2.3"));
        assert!(is_private_v4("172.16.0.1"));
        assert!(is_private_v4("172.31.255.254"));
        assert!(!is_private_v4("172.32.0.1"));
        assert!(!is_private_v4("8.8.8.8"));
        assert!(!is_private_v4("nonsense"));
    }

    #[test]
    fn device_file_roundtrip_and_repair() {
        let dir = std::env::temp_dir().join(format!("xivtl-dev-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("device.json");
        // 损坏内容 → 解析为 None（触发重生成）
        std::fs::write(&path, b"{not json").unwrap();
        assert!(Device::load_from(&path).is_none());
        // 合法往返
        let mut d = Device {
            mac_id: "AA-BB-CC-11-22-33".into(),
            device_id: String::new(),
            ep_name: "DESKTOP-ABC1234".into(),
            ep_ip: "192.168.0.9".into(),
            keep_login_key: None,
            last_area_id: None,
        };
        d.repair().unwrap();
        d.save_atomic(&path).unwrap();
        let back = Device::load_from(&path).unwrap();
        assert_eq!(back.mac_id, d.mac_id);
        assert_eq!(back.device_id, d.device_id);
        assert_eq!(back.ep_name, d.ep_name);
        assert_eq!(back.ep_ip, d.ep_ip);
        assert_eq!(back.keep_login_key, None);
        assert_eq!(back.last_area_id, None);
        assert!(back.seg0_consistent());
        // 记住上次大区：写盘后重新读回
        d.set_last_area_id("7", &path).unwrap();
        let back2 = Device::load_from(&path).unwrap();
        assert_eq!(back2.last_area_id.as_deref(), Some("7"));
        // 脏数据不接受
        d.set_last_area_id("abc", &path).unwrap();
        assert_eq!(d.last_area_id, None, "非数字大区 id 必须被拒绝");
        // 老文件没有 lastAreaId 字段也必须能读（向后兼容）
        std::fs::write(&path, br#"{"macId":"AA-BB-CC-11-22-33","deviceId":"","epName":"DESKTOP-ABC1234","epIp":"192.168.0.9","keepLoginKey":""}"#).unwrap();
        let legacy = Device::load_from(&path).unwrap();
        assert_eq!(legacy.last_area_id, None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
