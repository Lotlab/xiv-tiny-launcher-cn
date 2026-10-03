//! 响应解析与 Cookie 提取（纯逻辑，便于单测）。

use serde_json::Value;

/// 顶层 `return_code`（QR/push/fast/SSO 系）。
pub fn return_code(v: &Value) -> Option<i64> {
    match v.get("return_code") {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.parse().ok(),
        _ => None,
    }
}

/// `data` 节点。
pub(crate) fn data(v: &Value) -> Option<&Value> {
    v.get("data")
}

/// `data.<key>` 取字符串（数字同样接受，按字符串比较）。
pub fn data_str(v: &Value, key: &str) -> Option<String> {
    let d = data(v)?;
    match d.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    }
}

/// 顶层/`data` 任意一层取字符串（`failReason` 有时在顶层有时在 data）。
pub(crate) fn any_str(v: &Value, key: &str) -> Option<String> {
    let pick = |node: &Value| match node.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    };
    pick(v).or_else(|| data(v).and_then(pick))
}

/// 展示用：`failReason` 缺席时退回错误码数字，不暴露协议字段名。
pub fn fail_reason_text(v: &Value) -> String {
    if let Some(f) = any_str(v, "failReason") {
        if !f.is_empty() {
            return f;
        }
    }
    match return_code(v) {
        Some(rc) => format!("错误码 {rc}"),
        None => "未知错误".to_string(),
    }
}

/// 成功谓词：`return_code==0` 且 `required` 键均非空（HTTP 200 由调用方先判）。
pub fn is_success(v: &Value, required: &[&str]) -> bool {
    if return_code(v) != Some(0) {
        return false;
    }
    required
        .iter()
        .all(|k| data_str(v, k).map(|s| !s.is_empty()).unwrap_or(false))
}

/// 从 `Set-Cookie` 值列表里取 `CODEKEY=`（大小写不敏感，取第一个分号之前的值）。
/// `CODEKEY_COUNT` / `SECURE_CODEKEY` 不得误命中。
pub fn extract_codekey<'a>(set_cookies: impl IntoIterator<Item = &'a str>) -> Option<String> {
    for raw in set_cookies {
        let pair = raw.split(';').next().unwrap_or("");
        let mut it = pair.splitn(2, '=');
        let name = it.next().unwrap_or("").trim();
        let value = it.next().unwrap_or("").trim();
        if name.eq_ignore_ascii_case("CODEKEY") && !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

pub fn is_png(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[0] == 0x89 && bytes[1] == 0x50 && bytes[2] == 0x4E && bytes[3] == 0x47
}

/// `openFace` 判定；注意人脸接口顶层字段是 `resultCode` 而非 `return_code`。
pub fn open_face(v: &Value) -> Option<String> {
    any_str(v, "openFace")
}

pub fn result_code(v: &Value) -> Option<i64> {
    match v.get("resultCode") {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn success_predicate() {
        let ok = json!({"return_code":0,"data":{"ticket":"ULS21-x","sndaId":"1234567890","tgt":"ULSTGT-y"}});
        assert!(is_success(&ok, &["ticket", "sndaId", "tgt"]));
        // sndaId 为数字同样接受
        let num = json!({"return_code":0,"data":{"ticket":"t","sndaId":1234567890i64,"tgt":"g"}});
        assert!(is_success(&num, &["ticket", "sndaId", "tgt"]));
        // 缺 tgt → 失败
        let miss = json!({"return_code":0,"data":{"ticket":"t","sndaId":"1"}});
        assert!(!is_success(&miss, &["ticket", "sndaId", "tgt"]));
        // return_code 非零 → 失败
        let rc = json!({"return_code":-10515805,"data":{"ticket":"t","sndaId":"1","tgt":"g"}});
        assert!(!is_success(&rc, &["ticket", "sndaId", "tgt"]));
    }

    #[test]
    fn fail_reason_original_text() {
        let v = json!({"return_code":-10515805,"data":{"failReason":"二维码未通过验证，请重试"}});
        assert_eq!(fail_reason_text(&v), "二维码未通过验证，请重试");
        let v2 = json!({"return_code":-14001710,"data":{}});
        assert_eq!(fail_reason_text(&v2), "错误码 -14001710");
    }

    #[test]
    fn codekey_extraction() {
        let cookies = vec![
            "CODEKEY=0123456789abcdef0123456789abcdef;Path=/;Domain=.cas.sdo.com",
            "CODEKEY_COUNT=1;Path=/;Domain=.cas.sdo.com",
            "SECURE_CODEKEY=deadbeef;Path=/;Domain=.cas.sdo.com;SameSite=None;Secure",
        ];
        assert_eq!(
            extract_codekey(cookies).unwrap(),
            "0123456789abcdef0123456789abcdef"
        );
        // 只有 SECURE_CODEKEY / CODEKEY_COUNT 时不得误命中
        let only_other = vec!["CODEKEY_COUNT=1;Path=/", "SECURE_CODEKEY=deadbeef;Path=/"];
        assert_eq!(extract_codekey(only_other), None);
        // 大小写不敏感
        assert_eq!(extract_codekey(vec!["codekey=abc;Path=/"]).unwrap(), "abc");
    }

    #[test]
    fn png_magic() {
        assert!(is_png(&[0x89, 0x50, 0x4E, 0x47, 0x0D]));
        assert!(!is_png(b"<html>"));
        assert!(!is_png(&[0x89]));
    }

    #[test]
    fn face_verify_predicate() {
        let v = json!({"resultCode":0,"data":{"openFace":"0"}});
        assert_eq!(result_code(&v), Some(0));
        assert_eq!(open_face(&v).as_deref(), Some("0"));
        let bad = json!({"resultCode":0,"data":{"openFace":"1"}});
        assert_eq!(open_face(&bad).as_deref(), Some("1"));
    }
}
