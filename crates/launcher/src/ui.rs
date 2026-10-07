//! 终端交互 + `sdo_client::Ui` 的实现：按键、状态行、选区菜单、更新确认、二维码窗口。
//!
//! 除 `Keys` 外全部逻辑跨平台（`IsTerminal` + ANSI）。
//! Windows 上用 `ReadConsoleInputW` 取单键，可以免回车按 k/q/Ctrl+C；
//! 非 Windows 平台不做原始模式（那需要 termios），`Keys::poll` 恒返回 `None`：
//! 扫码期间无法按 k 勾选保持登录、也无法用 Ctrl+C 换码
//! （Ctrl+C 恢复系统默认行为：直接结束进程）。

use std::io::{IsTerminal, Write};

use proto::log;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// 非 Windows 平台没有按键来源，三个变体不会被构造（但仍会被 login 匹配）。
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Key {
    KeepLogin,
    CancelCode,
    Quit,
}

#[cfg(windows)]
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
#[cfg(windows)]
use windows_sys::Win32::System::Console::{
    GetConsoleMode, GetConsoleScreenBufferInfo, GetNumberOfConsoleInputEvents, GetStdHandle,
    ReadConsoleInputW, SetConsoleMode, CONSOLE_SCREEN_BUFFER_INFO, ENABLE_ECHO_INPUT,
    ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, INPUT_RECORD,
    KEY_EVENT, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};

#[cfg(windows)]
pub struct Keys {
    handle: HANDLE,
    orig_mode: u32,
    console: bool,
}

#[cfg(windows)]
impl Keys {
    pub fn new() -> Keys {
        unsafe {
            let handle = GetStdHandle(STD_INPUT_HANDLE);
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                return Keys {
                    handle,
                    orig_mode: 0,
                    console: false,
                };
            }
            let mut mode: u32 = 0;
            if GetConsoleMode(handle, &mut mode) == 0 {
                return Keys {
                    handle,
                    orig_mode: 0,
                    console: false,
                };
            }
            let new_mode = mode & !(ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT);
            SetConsoleMode(handle, new_mode);
            Keys {
                handle,
                orig_mode: mode,
                console: true,
            }
        }
    }

    /// 非阻塞取一个已识别的按键（读到的事件全部消费）。
    pub fn poll(&self) -> Option<Key> {
        if !self.console {
            return None;
        }
        unsafe {
            let mut n: u32 = 0;
            if GetNumberOfConsoleInputEvents(self.handle, &mut n) == 0 || n == 0 {
                return None;
            }
            let mut buf: [INPUT_RECORD; 32] = std::mem::zeroed();
            let mut read: u32 = 0;
            if ReadConsoleInputW(self.handle, buf.as_mut_ptr(), buf.len() as u32, &mut read) == 0 {
                return None;
            }
            let mut found = None;
            for rec in buf.iter().take(read as usize) {
                if rec.EventType != KEY_EVENT as u16 {
                    continue;
                }
                let ke = rec.Event.KeyEvent;
                if ke.bKeyDown == 0 {
                    continue;
                }
                let ch = ke.uChar.UnicodeChar;
                found = match ch {
                    3 => Some(Key::CancelCode),
                    c if c == u16::from(b'k') || c == u16::from(b'K') => Some(Key::KeepLogin),
                    c if c == u16::from(b'q') || c == u16::from(b'Q') => Some(Key::Quit),
                    _ => found,
                };
            }
            found
        }
    }
}

#[cfg(windows)]
impl Drop for Keys {
    fn drop(&mut self) {
        if self.console {
            unsafe {
                SetConsoleMode(self.handle, self.orig_mode);
            }
        }
    }
}

/// 非 Windows：不做原始模式，因此没有免回车的单键交互。
#[cfg(not(windows))]
#[derive(Default)]
pub struct Keys;

#[cfg(not(windows))]
impl Keys {
    pub fn new() -> Keys {
        Keys
    }

    /// 恒为 `None`：没有按键来源，登录流程只按倒计时 / 轮询次数推进。
    pub fn poll(&self) -> Option<Key> {
        None
    }
}

/// stdout 是否连接到终端（重定向时为 false）。
pub fn has_console_out() -> bool {
    std::io::stdout().is_terminal()
}

/// 打开 stdout 的 VT 序列处理。
///
/// Windows 需要显式打开 `ENABLE_VIRTUAL_TERMINAL_PROCESSING`；Unix 终端原生支持。
#[cfg(windows)]
pub fn enable_vt() {
    unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return;
        }
        let mut mode: u32 = 0;
        if GetConsoleMode(h, &mut mode) == 0 {
            return;
        }
        if mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING == 0 {
            let _ = SetConsoleMode(h, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        }
    }
}

#[cfg(not(windows))]
pub fn enable_vt() {}

/// 终端可见列数。
#[cfg(windows)]
pub fn console_width() -> Option<usize> {
    unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
        if GetConsoleScreenBufferInfo(h, &mut info) == 0 {
            return None;
        }
        let w = info.srWindow.Right as i32 - info.srWindow.Left as i32 + 1;
        if w > 0 {
            Some(w as usize)
        } else {
            None
        }
    }
}

/// 非 Windows：只能看 `COLUMNS`（交互式 shell 通常设了，但不保证导出），
/// 取不到就返回 `None`，由调用方兜底（二维码排版用 100 列）。
#[cfg(not(windows))]
pub fn console_width() -> Option<usize> {
    std::env::var("COLUMNS")
        .ok()?
        .parse::<usize>()
        .ok()
        .filter(|w| *w > 0)
}

/// 是否用 ANSI 彩色渲染：控制台默认开；重定向时可用 `SDO_FFXIV_FORCE_COLOR=1` 强制
/// （便于 `less -R`）；`NO_COLOR` 一律关闭。
pub fn color_ok() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    has_console_out() || std::env::var_os("SDO_FFXIV_FORCE_COLOR").is_some()
}

/// 是否把每次轮询都打出来（重定向输出时默认静默）。
fn verbose_status() -> bool {
    if let Ok(v) = std::env::var("SDO_FFXIV_VERBOSE_STATUS") {
        return v != "0" && !v.is_empty();
    }
    has_console_out()
}

/// 原地刷新状态行；重定向且未开 verbose 时不输出。
pub fn status(line: &str) {
    if !verbose_status() {
        return;
    }
    let mut out = std::io::stdout();
    if has_console_out() {
        let _ = write!(out, "\r\x1b[K{line}");
    } else {
        let _ = writeln!(out, "{line}");
    }
    let _ = out.flush();
}

pub fn status_end() {
    if verbose_status() {
        println!();
    }
}

/// 选区菜单：只列 id 与名字，输入数字选择。
pub fn pick_area(menu_lines: &[String], allowed: &[String]) -> Option<String> {
    println!("\n可用子区（输入数字后回车；直接回车取消）：");
    for l in menu_lines {
        println!("{l}");
    }
    loop {
        print!("请选择子区 [{}]: ", allowed.join("/"));
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return None;
        }
        let t = line.trim();
        if t.is_empty() {
            return None;
        }
        if allowed.iter().any(|a| a == t) {
            return Some(t.to_string());
        }
        println!("非法选择：{t}（应为 {}）", allowed.join("/"));
    }
}

/// stdin 是否可交互（能问「是否现在更新？[Y/n]」）。
///
/// 调用方用它在**长校验之前**预判能不能问：问不了就别白读一遍整份安装（~118 GB）。
pub fn can_confirm() -> bool {
    std::io::stdin().is_terminal()
}

/// 交互式确认：打印 `body` 后问一句「是否现在更新？[Y/n]」。
///
/// 返回 `Some(true)` 确认、`Some(false)` 取消；`None` 表示**问不了**
/// （stdin 不是终端，或读到了 EOF）——调用方据此中止并要求 `--yes`。
/// 不设超时：没有输入就一直等，避免误触发几十 GB 的下载。
pub fn confirm_update(body: &str) -> Option<bool> {
    if !can_confirm() {
        return None;
    }
    println!("{body}");
    loop {
        print!("是否现在更新？[Y/n]: ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => return None,
            Ok(_) => {}
        }
        match parse_answer(&line) {
            Some(v) => return Some(v),
            None => println!("请输入 y 或 n（直接回车视为 y）：{}", line.trim()),
        }
    }
}

/// 解析确认回答：回车 / `y` / `yes` / `是` → 确认，`n` / `no` / `否` → 取消，
/// 其他 → `None`（继续问）。
fn parse_answer(line: &str) -> Option<bool> {
    match line.trim().to_ascii_lowercase().as_str() {
        "" | "y" | "yes" | "是" => Some(true),
        "n" | "no" | "否" => Some(false),
        _ => None,
    }
}

/// EXE 侧的 [`sdo_client::Ui`] 实现：终端渲染 + 按键 + 二维码窗口。
///
/// 轮询循环在 `sdo_client::Flow` 里，这里只负责"展示 + 干预"：
/// 二维码落盘与渲染、按键读取、状态行、以及把「保持登录」勾选回报给层 2。
pub struct TerminalUi {
    keys: Keys,
    keep_login: bool,
    render: crate::cli::QrRender,
    qr_out: Option<std::path::PathBuf>,
    /// 显示中的原生二维码窗口；换码 / 结束时 drop 掉即关闭。
    window: Option<crate::qrwindow::NativeWindow>,
}

impl TerminalUi {
    pub fn new(
        render: crate::cli::QrRender,
        qr_out: Option<std::path::PathBuf>,
        keep_login: bool,
    ) -> TerminalUi {
        TerminalUi {
            keys: Keys::new(),
            keep_login,
            render,
            qr_out,
            window: None,
        }
    }
}

impl sdo_client::Ui for TerminalUi {
    fn show_code(&mut self, png: &[u8], round: u32) {
        // 上一张码的窗口先关掉。
        self.window = None;
        let saved = match crate::qr::save_png_at(png, self.qr_out.as_deref()) {
            Ok(p) => {
                println!("\n第 {round} 张二维码（也已保存到 {}）：", p.display());
                Some(p)
            }
            Err(e) => {
                log::debug(&format!("二维码图片保存失败：{e}"));
                println!("\n第 {round} 张二维码：");
                None
            }
        };
        if self.render != crate::cli::QrRender::Ascii {
            match crate::qrwindow::show(png) {
                Ok(w) => {
                    self.window = Some(w);
                    println!("已弹出二维码窗口（本张码结束时自动关闭）。");
                    return;
                }
                Err(reason) => log::debug(&format!(
                    "二维码窗口不可用：{reason}（{}）",
                    crate::qrwindow::environment_facts()
                )),
            }
        }
        let want_color = self.render == crate::cli::QrRender::Auto && color_ok();
        if let Err(e) = crate::qr::render_terminal_with(png, want_color) {
            log::debug(&format!("终端二维码渲染细节：{e}"));
            match saved {
                Some(p) => println!("终端显示失败，请直接扫 {}", p.display()),
                None => println!("终端显示失败"),
            }
        }
    }

    fn tick(&mut self, wait: sdo_client::Wait) -> sdo_client::Action {
        if let Some(k) = self.keys.poll() {
            match k {
                Key::Quit => return sdo_client::Action::Quit,
                Key::CancelCode => return sdo_client::Action::RefreshCode,
                // 按键直接改的是这个字段；层 2 每轮通过 keep_login() 来读。
                Key::KeepLogin => self.keep_login = true,
            }
        }
        match wait.phase {
            sdo_client::Phase::ScanCode => status(&format!(
                "[{:>3}s] 等待扫码　按键：k=勾选保持登录　Ctrl+C=换一张码　q=退出　保持登录：{}",
                wait.left_secs,
                if self.keep_login { "已勾选" } else { "未勾选" }
            )),
            sdo_client::Phase::ConfirmPush => {
                status(&format!("[{:>3}s] 等待手机确认　q=退出", wait.left_secs))
            }
        }
        sdo_client::Action::Wait
    }

    fn keep_login(&self) -> bool {
        self.keep_login
    }

    fn note(&mut self, note: sdo_client::Note) {
        match note {
            sdo_client::Note::Scanned => {
                self.window = None;
                status_end();
                println!("扫码成功");
            }
            sdo_client::Note::CodeRefreshed(reason) => {
                self.window = None;
                status_end();
                println!("\n本张码结束（{reason}），正在获取新码…");
            }
            sdo_client::Note::PushSent => {
                println!("已发送手机确认请求（请在手机 App 上确认），等待确认…");
            }
            sdo_client::Note::ServerError(text) => {
                status_end();
                println!("服务端返回错误：{text}");
            }
            sdo_client::Note::ExchangeFailed(reason) => {
                println!("换票失败（{reason}），重新扫码登录…");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_answer;

    #[test]
    fn confirm_answer_parsing() {
        // 回车视为确认（提示语写的就是 [Y/n]）。
        assert_eq!(parse_answer("\n"), Some(true));
        for s in ["y", "Y", " yes ", "是"] {
            assert_eq!(parse_answer(s), Some(true), "{s:?}");
        }
        for s in ["n", "N", " No ", "否"] {
            assert_eq!(parse_answer(s), Some(false), "{s:?}");
        }
        for s in ["q", "1", "yep"] {
            assert_eq!(parse_answer(s), None, "{s:?}");
        }
    }
}
