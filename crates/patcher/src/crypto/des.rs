//! 盛趣本地版本元数据（`LocalVersion3.xml`）所用的分组密码。
//!
//! 逆向自 `Launcher.dat`（地址为镜像基址 `0x400000` 下的 VA）：
//!
//! - `0x4997a0`：解密文件内容。校验头 4 字节 == `0xFFFFFFFF`，取 `len-8` 字节
//!   （必须是 8 的倍数）从偏移 4 起逐块调用 `0x49a430`，尾部 4 字节是小端明文长度。
//! - `0x49a430`：`memcpy(key, 0x79bc3c, 16)` 取 16 字节密钥，`0x49a020` 做两次 DES
//!   密钥编排（K1→ctx1、K2→ctx2），再三次 `0x499ca0`：解密 = `D_K1 · E_K2 · D_K1`。
//! - `0x49a020`：标准 DES 密钥编排（PC1/PC2/移位表）。
//! - `0x499ca0`：`IP` → 16 轮 `0x49a230` → `FP`。
//! - `0x49a230`：`E` 扩展 → 异或子密钥 → 8 个 S 盒 → `P`。
//!
//! **这不是标准 DES。** 表是标准 DES 表，但位序是「每字节 LSB-first」，
//! 且 S 盒输出取低位在前；Feistel 两条分支也按反汇编原样实现。实测
//! `binary DES ≠ 标准 DES + 每字节位反转`，所以必须照抄位序。

use super::tables::{E, FP, IP, P, PC1, PC2, SBOX, SHIFTS};

/// 每字节 LSB-first 展开成位数组：`bits[i] = (bytes[i/8] >> (i%8)) & 1`。
fn to_bits(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() * 8);
    for b in bytes {
        for i in 0..8 {
            out.push((b >> i) & 1);
        }
    }
    out
}

/// `to_bits` 的逆：`bytes[i/8] |= bits[i] << (i%8)`。
fn from_bits(bits: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; bits.len() / 8];
    for (i, &v) in bits.iter().enumerate() {
        out[i / 8] |= v << (i % 8);
    }
    out
}

/// 表项是 1-based，索引到位数组。
fn permute(bits: &[u8], table: &[u8]) -> Vec<u8> {
    table.iter().map(|&t| bits[t as usize - 1]).collect()
}

/// 16 个 48 位子密钥（每个存成 48 字节的位数组，与二进制一致）。
pub(crate) type SubKeys = [[u8; 48]; 16];

/// DES 密钥编排（`0x49a020`）。
pub(crate) fn key_schedule(key: &[u8; 8]) -> SubKeys {
    let a = to_bits(key);
    let k = permute(&a, &PC1); // 56 位
    let (mut c, mut d) = (k[..28].to_vec(), k[28..].to_vec());
    let mut subs = [[0u8; 48]; 16];
    for (round, &shift) in SHIFTS.iter().enumerate() {
        c.rotate_left(shift as usize);
        d.rotate_left(shift as usize);
        let mut cd = c.clone();
        cd.extend_from_slice(&d);
        let sk = permute(&cd, &PC2);
        subs[round].copy_from_slice(&sk);
    }
    subs
}

/// 轮函数（`0x49a230`）：`E` → 异或子密钥 → S 盒（低位在前）→ `P`。
fn feistel(r: &[u8], subkey: &[u8; 48]) -> Vec<u8> {
    let expanded = permute(r, &E); // 48 位
    let mut x = [0u8; 48];
    for i in 0..48 {
        x[i] = expanded[i] ^ subkey[i];
    }
    let mut s_out = Vec::with_capacity(32);
    for g in 0..8 {
        let c = &x[g * 6..g * 6 + 6];
        let row = (c[0] * 2 + c[5]) as usize;
        let col = (c[1] * 8 + c[2] * 4 + c[3] * 2 + c[4]) as usize;
        let v = SBOX[(row + g * 4) * 16 + col];
        for j in 0..4 {
            s_out.push((v >> j) & 1);
        }
    }
    permute(&s_out, &P)
}

/// 单个 DES 分组（`0x499ca0`）。
///
/// `decrypt=false` 走前向分支：`(A,B) -> (f(A)^B, A)`；
/// `decrypt=true` 走后向分支：`(B,A) -> (f(B)^A, B)`，子密钥倒序。
/// 末置换作用在 `[B|A]`（二进制里 `B` 在低地址）。
pub(crate) fn des_block(block: &[u8; 8], subs: &SubKeys, decrypt: bool) -> [u8; 8] {
    let arr = permute(&to_bits(block), &IP);
    let mut b = arr[..32].to_vec();
    let mut a = arr[32..].to_vec();
    if !decrypt {
        for sk in subs.iter() {
            let f = feistel(&a, sk);
            let new_a: Vec<u8> = f.iter().zip(&b).map(|(x, y)| x ^ y).collect();
            b = a;
            a = new_a;
        }
    } else {
        for sk in subs.iter().rev() {
            let f = feistel(&b, sk);
            let new_b: Vec<u8> = f.iter().zip(&a).map(|(x, y)| x ^ y).collect();
            a = b;
            b = new_b;
        }
    }
    let mut out = b;
    out.extend_from_slice(&a);
    let bytes = from_bits(&permute(&out, &FP));
    let mut r = [0u8; 8];
    r.copy_from_slice(&bytes);
    r
}

/// 3DES-EDE（`0x49a430`）：16 字节密钥，`K1 = key[0..8]`、`K2 = key[8..16]`、第三轮再用 `K1`。
pub fn des3(key: &[u8; 16], block: &[u8; 8], decrypt: bool) -> [u8; 8] {
    let mut k1 = [0u8; 8];
    k1.copy_from_slice(&key[..8]);
    let mut k2 = [0u8; 8];
    k2.copy_from_slice(&key[8..]);
    let s1 = key_schedule(&k1);
    let s2 = key_schedule(&k2);
    if decrypt {
        let a = des_block(block, &s1, true);
        let b = des_block(&a, &s2, false);
        des_block(&b, &s1, true)
    } else {
        let a = des_block(block, &s1, false);
        let b = des_block(&a, &s2, true);
        des_block(&b, &s1, false)
    }
}

/// 对任意长度（8 的倍数）缓冲做 ECB 3DES。
pub fn des3_ecb(key: &[u8; 16], data: &[u8], decrypt: bool) -> Vec<u8> {
    assert!(
        data.len().is_multiple_of(8),
        "des3_ecb 只吃 8 的倍数长度，实际 {}",
        data.len()
    );
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks_exact(8) {
        let mut blk = [0u8; 8];
        blk.copy_from_slice(chunk);
        out.extend_from_slice(&des3(key, &blk, decrypt));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    /// 用测试密钥（非线上密钥）生成的向量，验证位序实现与二进制一致。
    #[test]
    fn des3_matches_binary_vectors() {
        let vectors: &[(&str, &str, &str)] = &[
            ("00000000000000000000000000000000", "0000000000000000", "acec93d9a7c03c98"),
            ("00000000000000000000000000000000", "0123456789abcdef", "556e9c9edc1a6b04"),
            ("00000000000000000000000000000000", "ffffffffffffffff", "bafabe5bf4e9787f"),
            ("00000000000000000000000000000000", "48656c6c6f313233", "e10b171dc40ed6fb"),
            ("0123456789abcdeffedcba9876543210", "0000000000000000", "759cebf78e131030"),
            ("0123456789abcdeffedcba9876543210", "0123456789abcdef", "bfeb234c0f805810"),
            ("0123456789abcdeffedcba9876543210", "ffffffffffffffff", "d2e5bf1225f475f9"),
            ("0123456789abcdeffedcba9876543210", "48656c6c6f313233", "c7db979bcf4da5be"),
            ("ffffffffffffffffffffffffffffffff", "0000000000000000", "450541a40b168780"),
            ("ffffffffffffffffffffffffffffffff", "0123456789abcdef", "564e179714e8cb40"),
            ("ffffffffffffffffffffffffffffffff", "ffffffffffffffff", "53136c26583fc367"),
            ("ffffffffffffffffffffffffffffffff", "48656c6c6f313233", "4ce47b515a5d0911"),
        ];
        for (k, p, c) in vectors {
            let mut key = [0u8; 16];
            key.copy_from_slice(&h(k));
            let mut pt = [0u8; 8];
            pt.copy_from_slice(&h(p));
            let got = des3(&key, &pt, false);
            assert_eq!(got.to_vec(), h(c), "encrypt {k} {p}");
            let mut ct = [0u8; 8];
            ct.copy_from_slice(&h(c));
            assert_eq!(des3(&key, &ct, true).to_vec(), h(p), "decrypt {k} {c}");
        }
    }

    #[test]
    fn ecb_roundtrip() {
        let key = h("0123456789abcdeffedcba9876543210");
        let mut k = [0u8; 16];
        k.copy_from_slice(&key);
        let data: Vec<u8> = (0..40u8).collect();
        let enc = des3_ecb(&k, &data, false);
        assert_eq!(des3_ecb(&k, &enc, true), data);
    }
}
