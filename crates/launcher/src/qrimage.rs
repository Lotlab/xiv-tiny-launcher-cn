//! 二维码 PNG 解码：终端渲染与窗口显示共用同一份像素。

/// 解码结果：`0x00RRGGBB` 像素，行优先。
pub struct PngImage {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u32>,
}

impl PngImage {
    pub fn is_dark(&self, x: usize, y: usize) -> bool {
        let c = self.rgb[y * self.width + x];
        let (r, g, b) = ((c >> 16) & 0xFF, (c >> 8) & 0xFF, c & 0xFF);
        (r + g + b) / 3 < 128
    }
}

pub fn decode(png_bytes: &[u8]) -> Result<PngImage, String> {
    use proto::log;
    let mut dec = png::Decoder::new(png_bytes);
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().map_err(|e| {
        log::debug(&format!("PNG 解码细节：{e}"));
        "二维码图片损坏，无法解码，请重新获取".to_string()
    })?;
    let mut raw = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut raw).map_err(|e| {
        log::debug(&format!("PNG 取帧细节：{e}"));
        "二维码图片损坏，无法解码，请重新获取".to_string()
    })?;

    let (width, height) = (info.width as usize, info.height as usize);
    let line = info.line_size;
    let comps = match info.color_type {
        png::ColorType::Grayscale => 1usize,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        other => {
            log::debug(&format!("不支持的 PNG 颜色类型细节：{other:?}"));
            return Err("不支持的 PNG 颜色类型，请使用常见的 RGB/灰度 PNG".to_string());
        }
    };
    if info.bit_depth != png::BitDepth::Eight {
        log::debug(&format!("不支持的 PNG 位深细节：{:?}", info.bit_depth));
        return Err("不支持的 PNG 位深".to_string());
    }

    let mut rgb = vec![0u32; width * height];
    for y in 0..height {
        for x in 0..width {
            let p = y * line + x * comps;
            let v = |i: usize| raw.get(p + i).copied().unwrap_or(255) as u32;
            rgb[y * width + x] = match comps {
                1 => {
                    let g = v(0);
                    (g << 16) | (g << 8) | g
                }
                2 => {
                    let g = on_white(v(0), v(1));
                    (g << 16) | (g << 8) | g
                }
                3 => (v(0) << 16) | (v(1) << 8) | v(2),
                _ => {
                    let a = v(3);
                    (on_white(v(0), a) << 16) | (on_white(v(1), a) << 8) | on_white(v(2), a)
                }
            };
        }
    }
    Ok(PngImage { width, height, rgb })
}

/// 把带 alpha 的分量合成到白底上。
fn on_white(c: u32, a: u32) -> u32 {
    (c * a + 255 * (255 - a)) / 255
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 合成 1 位灰度 PNG：左半亮、右半暗（`invert` 时整张反相）。
    fn gray_one_bit_png(invert: bool) -> Vec<u8> {
        let (w, h) = (8u32, 8u32);
        let ls = (w as usize).div_ceil(8);
        let mut data = vec![0u8; ls * h as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let left = x < 4;
                let bit = if invert { !left } else { left };
                if bit {
                    data[y * ls + x / 8] |= 0x80 >> (x % 8);
                }
            }
        }
        encode(
            png::ColorType::Grayscale,
            png::BitDepth::One,
            data,
            w,
            h,
            None,
        )
    }

    /// 1 位调色板 PNG：索引 0 = 黑、1 = 白，位图与上面一致。
    fn indexed_one_bit_png() -> Vec<u8> {
        let (w, h) = (8u32, 8u32);
        let ls = (w as usize).div_ceil(8);
        let mut data = vec![0u8; ls * h as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                if x < 4 {
                    data[y * ls + x / 8] |= 0x80 >> (x % 8);
                }
            }
        }
        encode(
            png::ColorType::Indexed,
            png::BitDepth::One,
            data,
            w,
            h,
            Some(vec![0, 0, 0, 255, 255, 255]),
        )
    }

    fn encode(
        color: png::ColorType,
        depth: png::BitDepth,
        data: Vec<u8>,
        w: u32,
        h: u32,
        palette: Option<Vec<u8>>,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(color);
            enc.set_depth(depth);
            if let Some(p) = palette {
                enc.set_palette(p);
            }
            enc.write_header().unwrap().write_image_data(&data).unwrap();
        }
        out
    }

    /// 1 位灰度必须还原成 8 位采样：1 = 白、0 = 黑。
    ///
    /// `normalize_to_color8()` 会把不足 8 位的深度升到 8 位，所以这里断言的是"展开后的字节
    /// 被正确读出来"，而不是"自己拆打包位"。
    #[test]
    fn one_bit_grayscale_expands_to_bytes() {
        let img = decode(&gray_one_bit_png(false)).unwrap();
        assert_eq!((img.width, img.height), (8, 8));
        assert_eq!(img.rgb[0], 0xFFFFFF, "置位的左半应为白");
        assert_eq!(img.rgb[7], 0x000000, "未置位的右半应为黑");

        let inv = decode(&gray_one_bit_png(true)).unwrap();
        assert_eq!(inv.rgb[0], 0x000000, "反相后左半应为黑");
        assert_eq!(inv.rgb[7], 0xFFFFFF, "反相后右半应为白");
    }

    /// 调色板/低位深由 `normalize_to_color8()` 展开，解码侧只读展开后字节。
    #[test]
    fn indexed_palette_expands_to_rgb() {
        let img = decode(&indexed_one_bit_png()).unwrap();
        assert_eq!((img.width, img.height), (8, 8));
        assert_eq!(img.rgb[0], 0xFFFFFF, "索引 1 = 白");
        assert_eq!(img.rgb[7], 0x000000, "索引 0 = 黑");
    }

    #[test]
    fn dark_is_luminance_based() {
        let img = PngImage {
            width: 3,
            height: 1,
            rgb: vec![0x000000, 0x7F7F7F, 0x808080],
        };
        assert!(img.is_dark(0, 0), "纯黑必须算暗");
        assert!(img.is_dark(1, 0), "127 必须算暗");
        assert!(!img.is_dark(2, 0), "128 必须算亮");
    }

    /// 透明像素按白底合成：透明必须算亮，否则二维码会多出黑块。
    #[test]
    fn transparent_composites_on_white() {
        assert_eq!(on_white(0, 0), 255, "全透明 → 白");
        assert_eq!(on_white(0, 255), 0, "全不透明 → 原色");
        assert_eq!(on_white(255, 128), 255, "白色半透明 → 白");
    }
}
