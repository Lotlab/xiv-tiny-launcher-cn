//! 终端交互：按键、状态行、选区菜单。

use std::io::Write;

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Console::{
    GetConsoleMode, GetConsoleScreenBufferInfo, GetNumberOfConsoleInputEvents, GetStdHandle,
    ReadConsoleInputW, SetConsoleMode, CONSOLE_SCREEN_BUFFER_INFO, ENABLE_ECHO_INPUT,
    ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, INPUT_RECORD,
    KEY_EVENT, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    KeepLogin,
    CancelCode,
    Quit,
}

pub struct Keys {
    handle: HANDLE,
    orig_mode: u32,
    console: bool,
}

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

impl Drop for Keys {
    fn drop(&mut self) {
        if self.console {
            unsafe {
                SetConsoleMode(self.handle, self.orig_mode);
            }
        }
    }
}

/// stdout 是否连接到控制台。
pub fn has_console_out() -> bool {
    unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut mode: u32 = 0;
        GetConsoleMode(h, &mut mode) != 0
    }
}

/// 打开 stdout 的 VT 序列处理。
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

/// 控制台可见列数（拿不到返回 None）。
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
