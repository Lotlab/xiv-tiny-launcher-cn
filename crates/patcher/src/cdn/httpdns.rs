//! HTTPDNS：用公共 DoH 解析 CDN host 的全量边缘 IP，配合
//! `reqwest::ClientBuilder::resolve_to_addrs` 把连接 pin 到指定 IP
//! （等价官方 libcurl 的 `CURLOPT_RESOLVE`）。
//!
//! 为什么需要：系统 DNS（GSLB）一次往往只返回单个 IP，撞到坏节点就只能干等；
//! DoH 返回候选 IP 列表，重试时轮换首选 IP 来换节点。
//!
//! 只在重试路径调用，健康时零额外开销；DoH 查不到就回退系统 DNS。
//! pin 只决定连哪个 IP，SNI/证书校验保持域名不变，文件另有 MD5 校验，安全无损。

use std::net::IpAddr;
use std::time::Duration;

/// DoH JSON 接口（阿里公共 DNS，无需鉴权）。
const DOH_URL: &str = "https://dns.alidns.com/resolve";

/// 解析 `host` 的全部 A 记录；失败/无记录返回 Err（调用方回退系统 DNS）。
pub fn resolve_host(host: &str) -> Result<Vec<IpAddr>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
        .map_err(|e| format!("DoH 客户端初始化失败：{e}"))?;
    let resp = client
        .get(DOH_URL)
        .query(&[("name", host), ("type", "A")])
        .send()
        .map_err(|e| format!("DoH 请求失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("DoH 返回 HTTP {}", resp.status().as_u16()));
    }
    let text = resp.text().map_err(|e| format!("DoH 响应读取失败：{e}"))?;
    let body: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("DoH 响应解析失败：{e}"))?;
    let ips = parse_answers(&body);
    if ips.is_empty() {
        return Err(format!("DoH 无 {host} 的 A 记录"));
    }
    Ok(ips)
}

/// 从 alidns JSON 的 `Answer` 段提取 `type=1`（A 记录）的 IP（去重保序）。
fn parse_answers(body: &serde_json::Value) -> Vec<IpAddr> {
    let mut out = Vec::new();
    let empty = Vec::new();
    let answers = body.get("Answer").and_then(|a| a.as_array()).unwrap_or(&empty);
    for a in answers {
        if a.get("type").and_then(|t| t.as_u64()) != Some(1) {
            continue;
        }
        if let Some(data) = a.get("data").and_then(|d| d.as_str()) {
            if let Ok(ip) = data.trim().parse::<IpAddr>() {
                if !out.contains(&ip) {
                    out.push(ip);
                }
            }
        }
    }
    out
}

/// 轮换：把第 `n` 个放到首位。hyper 按顺序试 IP，换首位即换节点。
pub(crate) fn rotated(ips: &[IpAddr], n: usize) -> Vec<IpAddr> {
    if ips.is_empty() {
        return Vec::new();
    }
    let k = n % ips.len();
    let mut out = ips[k..].to_vec();
    out.extend_from_slice(&ips[..k]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    const SAMPLE: &str = r#"{
        "Status": 0, "TC": false, "RD": true, "RA": true, "AD": false, "CD": false,
        "Question": [{"name": "ff14.jijiagames.com.", "type": 1}],
        "Answer": [
            {"name": "ff14.jijiagames.com.", "type": 5, "TTL": 60, "data": "x.cdn.example."},
            {"name": "x.cdn.example.", "type": 1, "TTL": 60, "data": "113.56.145.137"},
            {"name": "x.cdn.example.", "type": 1, "TTL": 60, "data": "113.56.145.164"},
            {"name": "x.cdn.example.", "type": 1, "TTL": 60, "data": "113.56.145.137"},
            {"name": "x.cdn.example.", "type": 28, "TTL": 60, "data": "::1"},
            {"name": "x.cdn.example.", "type": 1, "TTL": 60, "data": "not-an-ip"}
        ]
    }"#;

    #[test]
    fn parses_only_a_records_deduped() {
        let body: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(
            parse_answers(&body),
            vec![
                IpAddr::V4(Ipv4Addr::new(113, 56, 145, 137)),
                IpAddr::V4(Ipv4Addr::new(113, 56, 145, 164)),
            ]
        );
        // 无 Answer → 空。
        assert!(parse_answers(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn rotation_moves_head() {
        let ips = vec![
            IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
            IpAddr::V4(Ipv4Addr::new(2, 2, 2, 2)),
            IpAddr::V4(Ipv4Addr::new(3, 3, 3, 3)),
        ];
        assert_eq!(rotated(&ips, 0), ips);
        assert_eq!(rotated(&ips, 1)[0].to_string(), "2.2.2.2");
        assert_eq!(rotated(&ips, 3), ips);
        assert!(rotated(&[], 5).is_empty());
    }
}
