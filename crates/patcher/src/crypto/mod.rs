//! 盛趣本地元数据所用的 DES 变体与 3DES-EDE。

pub mod des;
pub(crate) mod tables;

pub use des::{des3, des3_ecb};
