//! 终端二维码渲染：还原模块网格后输出。
use proto::paths;

use crate::ui;

/// 暗模块背景（黑）。
const BG_DARK: &str = "\x1b[40m";
/// 亮模块/静默区背景（白）。
const BG_LIGHT: &str = "\x1b[47m";
const RESET: &str = "\x1b[0m";

/// 渲染模块网格并输出到终端，返回模块数。
pub fn render_terminal_with(png_bytes: &[u8], color: bool) -> Result<usize, String> {
    let grid = module_grid(png_bytes)?;
    let n = grid.len();
    if n == 0 {
        return Err("二维码模块数为 0".to_string());
    }
    if !color {
        print!("{}", ascii_text(&grid));
        return Ok(n);
    }
    let cols = ui::console_width().unwrap_or(100);
    // 每模块 2 列（字符单元约 1:2，视觉上接近正方形）；放不下时退化为每模块 1 列。
    let two_cols = n * 2 + 8 <= cols;
    let quiet = 2usize; // 左右各 2 模块白边；上下各 1 行 ≈ 2 模块高度
    print!("{}", render_grid(&grid, two_cols, quiet));
    Ok(n)
}

/// 模块网格 → `##`/空格纯文本。
fn ascii_text(grid: &[Vec<bool>]) -> String {
    let n = grid.len();
    let quiet = 2usize;
    let mut out = String::with_capacity((n + quiet * 2 + 2) * (n + quiet * 2) * 2);
    let blank = " ".repeat((n + quiet * 2) * 2);
    out.push_str(&blank);
    out.push('\n');
    for row in grid {
        for _ in 0..quiet {
            out.push_str("  ");
        }
        for &dark in row {
            out.push_str(if dark { "##" } else { "  " });
        }
        for _ in 0..quiet {
            out.push_str("  ");
        }
        out.push('\n');
    }
    out.push_str(&blank);
    out.push('\n');
    out
}

/// 还原 QR 模块网格（`true` = 暗模块）。
pub fn module_grid(png_bytes: &[u8]) -> Result<Vec<Vec<bool>>, String> {
    let (w, h, dark) = binarize(png_bytes)?;
    let grid = Binarized::new(w, h, &dark).ok_or_else(|| "二维码图像全白，无法识别".to_string())?;
    Ok(grid.to_modules())
}

/// 二值位图 + 包围盒 + 模块数。
struct Binarized<'a> {
    dark: &'a [bool],
    w: usize,
    x0: usize,
    y0: usize,
    bw: usize,
    bh: usize,
    modules: usize,
}

impl<'a> Binarized<'a> {
    fn new(w: usize, h: usize, dark: &'a [bool]) -> Option<Binarized<'a>> {
        let (x0, y0, x1, y1) = bbox(w, h, dark)?;
        let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
        // 包围盒内最小游程 ≈ 单个模块的像素宽，再吸附到 QR 版本序列 21+4n。
        let mp = min_run(w, dark, x0, y0, x1, y1).unwrap_or(1);
        Some(Binarized {
            dark,
            w,
            x0,
            y0,
            bw,
            bh,
            modules: snap_modules(bw.max(bh), mp),
        })
    }

    fn to_modules(&self) -> Vec<Vec<bool>> {
        (0..self.modules)
            .map(|my| (0..self.modules).map(|mx| self.is_dark(mx, my)).collect())
            .collect()
    }

    fn is_dark(&self, mx: usize, my: usize) -> bool {
        let sx = (mx * self.bw) / self.modules;
        let ex = (((mx + 1) * self.bw) / self.modules)
            .max(sx + 1)
            .min(self.bw);
        let sy = (my * self.bh) / self.modules;
        let ey = (((my + 1) * self.bh) / self.modules)
            .max(sy + 1)
            .min(self.bh);
        let (mut dark_count, mut total) = (0usize, 0usize);
        for y in sy..ey {
            for x in sx..ex {
                let px = self.x0 + x;
                let py = self.y0 + y;
                if px < self.w {
                    total += 1;
                    if self.dark[py * self.w + px] {
                        dark_count += 1;
                    }
                }
            }
        }
        total > 0 && dark_count * 2 >= total
    }
}

fn render_grid(grid: &[Vec<bool>], two_cols: bool, quiet: usize) -> String {
    let n = grid.len();
    let unit = if two_cols { 2usize } else { 1usize };
    let total = (n + quiet * 2) * unit;
    let mut out = String::with_capacity(total * (n + 4) * 6);

    fn emit(out: &mut String, dark: bool, count: usize) {
        if count == 0 {
            return;
        }
        out.push_str(if dark { BG_DARK } else { BG_LIGHT });
        for _ in 0..count {
            out.push(' ');
        }
    }

    // 上静默区（1 行字符高度 ≈ 2 模块高度）
    emit(&mut out, false, total);
    out.push_str(RESET);
    out.push('\n');

    for row in grid {
        emit(&mut out, false, quiet * unit);
        let mut cur = false;
        let mut run = 0usize;
        for &d in row {
            if d == cur {
                run += 1;
            } else {
                emit(&mut out, cur, run * unit);
                cur = d;
                run = 1;
            }
        }
        emit(&mut out, cur, run * unit);
        emit(&mut out, false, quiet * unit);
        out.push_str(RESET);
        out.push('\n');
    }

    emit(&mut out, false, total);
    out.push_str(RESET);
    out.push('\n');
    out
}

/// 保存二维码图片。
pub fn save_png_at(
    png_bytes: &[u8],
    dir: Option<&std::path::Path>,
) -> std::io::Result<std::path::PathBuf> {
    let path = match dir {
        Some(d) => {
            std::fs::create_dir_all(d)?;
            d.join(proto::consts::FILE_QRCODE)
        }
        None => paths::cwd_file(proto::consts::FILE_QRCODE),
    };
    paths::write_atomic(&path, png_bytes)?;
    Ok(path)
}

/// 解码 PNG → 二值位图（`true` = 暗像素）。
///
/// 极性不能猜：1 位灰度里 `1` 表示白，但这是编码方的约定。这里先按亮度解出明暗，
/// 再用 QR 定位图案（必然"外圈暗、内圈亮"）反推是否需要取反。
fn binarize(png_bytes: &[u8]) -> Result<(usize, usize, Vec<bool>), String> {
    let img = crate::qrimage::decode(png_bytes)?;
    let mut dark = vec![false; img.width * img.height];
    for y in 0..img.height {
        for x in 0..img.width {
            dark[y * img.width + x] = img.is_dark(x, y);
        }
    }
    invert_if_finder_is_light(&mut dark, img.width, img.height);
    Ok((img.width, img.height, dark))
}

/// 定位图案外圈必须暗、其内侧一圈必须亮。样本落在包围盒左上角的 7×7 定位图案上：
/// 若"外圈"反而不如"内圈"暗，说明整张图的明暗是反的，全部取反。
fn invert_if_finder_is_light(dark: &mut [bool], w: usize, h: usize) {
    let Some((x0, y0, x1, y1)) = bbox(w, h, dark) else {
        return;
    };
    let mp = min_run(w, dark, x0, y0, x1, y1).unwrap_or(1).max(1);
    let ring = |mx: usize, my: usize| -> u32 {
        let px = x0 + mx * mp + mp / 2;
        let py = y0 + my * mp + mp / 2;
        if px > x1 || py > y1 || px >= w || py >= h {
            return 0;
        }
        u32::from(dark[py * w + px])
    };
    // 外圈取四角，内圈取环内与中心。
    let outer = ring(0, 0) + ring(6, 0) + ring(0, 6) + ring(6, 6);
    let inner = ring(1, 1) + ring(3, 3) + ring(5, 5) + ring(1, 5);
    if outer < inner {
        for v in dark.iter_mut() {
            *v = !*v;
        }
    }
}

fn bbox(w: usize, h: usize, dark: &[bool]) -> Option<(usize, usize, usize, usize)> {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
    let mut any = false;
    for y in 0..h {
        for x in 0..w {
            if dark[y * w + x] {
                any = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if any {
        Some((x0, y0, x1, y1))
    } else {
        None
    }
}

/// 包围盒内最小的连续同色游程（长度 ≥2）≈ 单个模块像素宽。
fn min_run(w: usize, dark: &[bool], x0: usize, y0: usize, x1: usize, y1: usize) -> Option<usize> {
    let mut min = usize::MAX;
    for y in y0..=y1 {
        let mut prev = dark[y * w + x0];
        let mut n = 1usize;
        for x in (x0 + 1)..=x1 {
            let v = dark[y * w + x];
            if v == prev {
                n += 1;
            } else {
                if n >= 2 {
                    min = min.min(n);
                }
                prev = v;
                n = 1;
            }
        }
        if n >= 2 {
            min = min.min(n);
        }
    }
    if min == usize::MAX {
        None
    } else {
        Some(min)
    }
}

/// QR 合法模块数：21 + 4n。
fn snap_modules(size: usize, module_px: usize) -> usize {
    if module_px == 0 {
        return size.max(1);
    }
    let raw = ((size as f64) / (module_px as f64)).round() as i64;
    let mut best: Option<(i64, i64)> = None;
    for k in 0..40i64 {
        let candidate = 21 + 4 * k;
        let d = (candidate - raw).abs();
        if best.map(|(bd, _)| d < bd).unwrap_or(true) {
            best = Some((d, candidate));
        }
    }
    match best {
        Some((d, candidate)) if d <= 2 && candidate >= 21 => candidate as usize,
        _ => raw.clamp(1, 200) as usize,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_qr_version_grid() {
        // 包围盒 99px、最小游程 3px → 33 模块（version 4）
        assert_eq!(snap_modules(99, 3), 33);
        // 21 模块
        assert_eq!(snap_modules(63, 3), 21);
        // 非整数缩放：100px / 3px ≈ 33.3 → 33
        assert_eq!(snap_modules(100, 3), 33);
        // 噪声过大时不硬凑版本号
        assert_eq!(snap_modules(100, 10), 10);
    }

    /// 合成 1 位灰度 PNG：`modules` × `modules` 的类 QR 图案，每模块 `px` 像素（1 = 白、0 = 黑）。
    ///
    /// `invert=true` 时整张位图反相（白黑互换），用来验证极性不是靠约定猜的。
    fn synth_png(modules: usize, px: usize, invert: bool) -> Vec<u8> {
        let size = modules * px;
        let line = size.div_ceil(8);
        let mut data = vec![0u8; line * size]; // 1 = 白，0 = 黑
        for y in 0..size {
            for x in 0..size {
                data[y * line + x / 8] |= 0x80 >> (x % 8); // 先全白
            }
        }
        let mut set = |mx: usize, my: usize| {
            for dy in 0..px {
                for dx in 0..px {
                    let x = mx * px + dx;
                    let y = my * px + dy;
                    data[y * line + x / 8] &= !(0x80 >> (x % 8)); // 置黑
                }
            }
        };
        // 三个角上的 finder 图案（7×7 边框 + 3×3 实心）
        for &(ox, oy) in &[(0usize, 0usize), (modules - 7, 0), (0, modules - 7)] {
            for i in 0..7 {
                for j in 0..7 {
                    let border = i == 0 || j == 0 || i == 6 || j == 6;
                    let core = (2..=4).contains(&i) && (2..=4).contains(&j);
                    if border || core {
                        set(ox + i, oy + j);
                    }
                }
            }
        }
        // 一条 1 模块宽的时序图案，保证最小游程 = 1 模块
        for i in 8..(modules - 8) {
            if i % 2 == 0 {
                set(i, 6);
                set(6, i);
            }
        }
        if invert {
            for b in data.iter_mut() {
                *b = !*b;
            }
        }

        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, size as u32, size as u32);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::One);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&data).unwrap();
        }
        out
    }

    /// 反相位图不能把极性翻过来：定位图案必须仍是"外圈暗、内圈亮"。
    #[test]
    fn inverted_bitmap_is_normalized() {
        let normal = synth_png(21, 3, false);
        let inverted = synth_png(21, 3, true);

        let g_normal = module_grid(&normal).unwrap();
        let g_inverted = module_grid(&inverted).unwrap();
        assert_eq!(g_normal, g_inverted, "反相位图必须还原出同一个模块网格");

        // 定位图案：外角暗、内圈亮、中心暗
        for &(ox, oy) in &[(0usize, 0usize), (14, 0), (0, 14)] {
            assert!(g_inverted[oy][ox], "finder 外角应为暗");
            assert!(!g_inverted[oy + 1][ox + 1], "finder 内圈应为亮");
            assert!(g_inverted[oy + 3][ox + 3], "finder 中心应为暗");
        }
    }

    #[test]
    fn binarize_and_module_inference() {
        let png = synth_png(21, 3, false);
        let (w, h, dark) = binarize(&png).unwrap();
        assert_eq!((w, h), (63, 63));
        let (x0, y0, x1, y1) = bbox(w, h, &dark).unwrap();
        assert_eq!((x0, y0, x1, y1), (0, 0, 62, 62));
        let mp = min_run(w, &dark, x0, y0, x1, y1).unwrap();
        assert_eq!(mp, 3);
        assert_eq!(snap_modules(x1 - x0 + 1, mp), 21);
    }

    /// 纯文本渲染：零 ANSI、零非 ASCII 字形，逐行与模块网格逐格一致。
    #[test]
    fn ascii_render_is_pure_text_and_matches_grid() {
        let grid = module_grid(&synth_png(21, 2, false)).unwrap();
        let n = 21usize;
        let width = (n + 2 * 2) * 2; // 含左右各 2 模块静默区，每模块 2 列
        let text = ascii_text(&grid);
        assert!(!text.contains('\x1b'), "纯文本模式不得含 ANSI 转义");
        assert!(
            text.is_ascii(),
            "纯文本模式必须全 ASCII（不依赖任何特殊字形）"
        );
        assert!(!text.contains('█'), "纯文本模式不得使用方块字形");

        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), n + 2, "上下各 1 行静默区");
        for line in &lines {
            assert_eq!(line.chars().count(), width, "行宽不一致：{line:?}");
        }
        // 逐格比对：静默区 2 模块 + 网格行 + 静默区 2 模块
        for (y, row) in grid.iter().enumerate() {
            let mut want = String::from("    ");
            for &dark in row {
                want.push_str(if dark { "##" } else { "  " });
            }
            want.push_str("    ");
            assert_eq!(lines[y + 1], want, "第 {y} 行与网格不一致");
        }
    }

    /// `--qr-out` 指定目录时 PNG 落到该目录。
    #[test]
    fn save_png_at_honours_target_dir() {
        let dir = std::env::temp_dir().join(format!("qrout-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = save_png_at(b"not-really-png", Some(&dir)).unwrap();
        assert_eq!(path, dir.join(proto::consts::FILE_QRCODE));
        assert_eq!(std::fs::read(&path).unwrap(), b"not-really-png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_terminal_reports_module_count() {
        // 只断言模块数。
        let png = synth_png(21, 2, false);
        assert_eq!(render_terminal_with(&png, false).unwrap(), 21);
    }

    #[test]
    fn module_grid_matches_pattern() {
        let png = synth_png(21, 3, false);
        let grid = module_grid(&png).unwrap();
        assert_eq!(grid.len(), 21);
        // 三个 finder 图案：外框暗、第 1 圈亮、中心 3×3 暗
        for &(ox, oy) in &[(0usize, 0usize), (14, 0), (0, 14)] {
            assert!(grid[oy][ox], "finder 外角应为暗");
            assert!(!grid[oy + 1][ox + 1], "finder 内圈应为亮");
            assert!(grid[oy + 3][ox + 3], "finder 中心应为暗");
            assert!(grid[oy + 6][ox + 6], "finder 外角应为暗");
        }
        // 时序图案（1 模块宽 → 校验最小游程推断）
        assert!(grid[6][8]);
        assert!(!grid[6][9]);
    }

    /// 极性回归：暗模块必须走黑底、亮模块与静默区必须走白底，与终端主题无关。
    #[test]
    fn colored_render_is_dark_on_light() {
        let grid = module_grid(&synth_png(21, 2, false)).unwrap();
        let out = render_grid(&grid, true, 2);
        assert!(out.starts_with(BG_LIGHT), "首行静默区必须是白底");
        assert!(out.contains(BG_DARK), "必须存在黑底暗模块");
        assert!(out.contains(BG_LIGHT), "必须存在白底亮模块");
        assert!(out.trim_end().ends_with(RESET), "行尾必须复位颜色");
        // 无色路径必须完全不碰 ANSI，也不依赖方块字形（`█` 在部分终端会变占位方块）。
        let plain = ascii_text(&grid);
        assert!(!plain.contains('\x1b'));
        assert!(!plain.contains('█'));
        assert!(plain.contains("##"));
        // 每行字符数一致（双列：2*(21+4) = 50）
        for line in plain.lines() {
            assert_eq!(line.chars().count(), 50, "行宽不一致：{line:?}");
        }
    }

    /// 几何回归：渲染出的终端矩阵必须与模块网格逐格一致（含首/末格为暗、整行亮等边界）。
    #[test]
    fn rendered_terminal_matches_grid_cell_by_cell() {
        let n = 21usize;
        let quiet = 2usize;
        // 伪随机图案 + 首/末格暗 + 一行全亮，覆盖 run-length 边界
        let mut grid = vec![vec![false; n]; n];
        let mut seed = 0x1234_5678u32;
        for (y, row) in grid.iter_mut().enumerate() {
            for (x, cell) in row.iter_mut().enumerate() {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *cell = (seed >> 16) & 1 == 1;
                if x == 0 && y % 2 == 0 {
                    *cell = true;
                }
                if x == n - 1 && y % 3 == 0 {
                    *cell = true;
                }
            }
        }
        grid[7] = vec![false; n]; // 整行亮
        grid[8] = vec![true; n]; // 整行暗

        for two_cols in [true, false] {
            let out = render_grid(&grid, two_cols, quiet);
            let unit = if two_cols { 2 } else { 1 };
            let decoded = decode_rendered(&out, n, unit, quiet);
            assert_eq!(decoded, grid, "two_cols={two_cols} 渲染矩阵与网格不一致");
        }
    }

    /// 测试用：把着色渲染结果解回模块矩阵（空格无法区分颜色，按背景色序列解析）。
    fn decode_rendered(out: &str, n: usize, unit: usize, quiet: usize) -> Vec<Vec<bool>> {
        let lines: Vec<&str> = out.lines().collect();
        let body = &lines[1..=n];
        body.iter()
            .map(|line| decode_colored_line(line, n, unit, quiet))
            .collect()
    }

    fn decode_colored_line(line: &str, n: usize, unit: usize, quiet: usize) -> Vec<bool> {
        let mut flat = String::new();
        let mut cur = false;
        for part in line.split("\x1b[") {
            if part.is_empty() {
                continue;
            }
            let (code, rest) = match part.split_once('m') {
                Some((c, r)) => (c, r),
                None => continue,
            };
            match code {
                "40" => cur = true,
                "47" => cur = false,
                "0" => cur = false,
                _ => {}
            }
            for _ in rest.chars() {
                flat.push(if cur { '#' } else { '.' });
            }
        }
        let chars: Vec<char> = flat.chars().collect();
        (0..n).map(|mx| chars[(quiet + mx) * unit] == '#').collect()
    }

    /// 真实服务端 `getCodeKey` PNG 的回归夹具（128×128、1 位灰度、`1` 表示白）。
    const SERVER_PNG: &[u8] = include_bytes!("../qrcode-raw.png");

    /// 真实 PNG 必须还原成 33x33、极性与定位图案都正确的模块网格。
    #[test]
    fn server_png_decodes_to_correct_grid() {
        assert_eq!(&SERVER_PNG[..4], b"\x89PNG", "夹具不是 PNG");
        let grid = module_grid(SERVER_PNG).unwrap();
        let n = grid.len();
        assert_eq!(n, 33, "真实 PNG 是 version 4（33 模块）");

        // 三个定位图案：外角暗、内圈亮、中心暗
        for &(ox, oy) in &[(0usize, 0usize), (n - 7, 0), (0, n - 7)] {
            assert!(grid[oy][ox], "finder 外角应为暗");
            assert!(!grid[oy + 1][ox + 1], "finder 内圈应为亮");
            assert!(grid[oy + 3][ox + 3], "finder 中心应为暗");
        }

        // 极性：暗模块占比应落在 QR 常见区间；反相时会明显偏高
        let dark = grid.iter().flatten().filter(|d| **d).count();
        let ratio = dark as f64 / (n * n) as f64;
        assert!(
            (0.25..0.60).contains(&ratio),
            "暗模块占比 {ratio:.3} 不在合理区间"
        );
    }
}
