//! 打包时注入的密钥（见 `build.rs`）。源码里不出现明文。

/// CDN `v3ctrl.xml` 鉴权用的 RSA **公钥**（PEM 全文）。
///
/// PEM 多行，`build.rs` 把它写进 `OUT_DIR` 后用 `include_str!` 读回。
pub const CDN_RSA_PUBLIC_KEY_PEM: &str =
    include_str!(env!("SDO_FFXIV_CDN_RSA_PUBLIC_KEY_PEM_FILE"));

/// 本地版本元数据 3DES 密钥（32 位小写 hex）。
pub const META_DES_KEY_HEX: &str = env!("SDO_FFXIV_META_DES_KEY_HEX");

/// 解析成 16 字节密钥；`build.rs` 已保证格式合法。
pub fn meta_des_key() -> [u8; 16] {
    crate::hash::decode_hex_array(META_DES_KEY_HEX).expect("build.rs 已校验密钥为 32 位 hex")
}
