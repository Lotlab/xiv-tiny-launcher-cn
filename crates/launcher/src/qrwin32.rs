//! 原生 Win32 + GDI 二维码窗口（不可缩放、置顶）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use proto::log;

use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateSolidBrush, DeleteDC,
    DeleteObject, EndPaint, FillRect, SelectObject, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HGDIOBJ, PAINTSTRUCT, SRCCOPY,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetClientRect, GetSystemMetrics, GetWindowLongPtrW, PeekMessageW, PostQuitMessage,
    RegisterClassExW, SetForegroundWindow, SetWindowLongPtrW, ShowWindow, TranslateMessage,
    CREATESTRUCTW, GWLP_USERDATA, MSG, PM_REMOVE, SM_CXSCREEN, SM_CYSCREEN, SW_SHOW, WM_CLOSE,
    WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_NCCREATE, WM_PAINT, WM_QUIT, WNDCLASSEXW, WS_CAPTION,
    WS_EX_TOPMOST, WS_SYSMENU, WS_VISIBLE,
};

/// 带标题栏与关闭按钮，不可缩放。
const WINDOW_STYLE: u32 = WS_CAPTION | WS_SYSMENU;

/// 固定整倍放大（最近邻），避免插值灰边。
const QR_SCALE: usize = 2;

/// 已解包的 32 位 BGRA 图像。
#[derive(Clone)]
pub struct Bgra {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

impl Bgra {
    /// 从 `0x00RRGGBB` 像素生成 1:1 的 DIB 数据。
    pub fn from_rgb(pixels: &[u32], width: usize, height: usize) -> Bgra {
        let mut buf = vec![0u8; width * height * 4];
        for y in 0..height {
            // DIB 自下而上：输出第 0 行是图像最后一行。
            let row = (height - 1 - y) * width * 4;
            for x in 0..width {
                let c = pixels[y * width + x];
                let o = row + x * 4;
                buf[o] = (c & 0xFF) as u8;
                buf[o + 1] = ((c >> 8) & 0xFF) as u8;
                buf[o + 2] = ((c >> 16) & 0xFF) as u8;
                buf[o + 3] = 0xFF;
            }
        }
        Bgra {
            width,
            height,
            pixels: buf,
        }
    }

    /// 整数倍最近邻放大。
    pub fn scaled(&self, scale: usize) -> Bgra {
        let scale = scale.max(1);
        let (w, h) = (self.width * scale, self.height * scale);
        let mut buf = vec![0u8; w * h * 4];
        for y in 0..h {
            let src_row = (y / scale) * self.width * 4;
            let row = y * w * 4;
            for x in 0..w {
                let s = src_row + (x / scale) * 4;
                let o = row + x * 4;
                buf[o..o + 4].copy_from_slice(&self.pixels[s..s + 4]);
            }
        }
        Bgra {
            width: w,
            height: h,
            pixels: buf,
        }
    }
}

/// 窗口线程状态。
struct Shared {
    img: Bgra,
    stop: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
}

/// 裸指针的 `Send` 包装：`Shared` 的归属在任一时刻只属于一个线程（建窗线程），
/// 这里只是把它送进窗口线程，不存在并发访问。
struct SendPtr(*mut Shared);
// SAFETY: 指针在送出后只由窗口线程使用（原线程不再触碰），不存在并发访问。
unsafe impl Send for SendPtr {}
impl SendPtr {
    fn get(&self) -> *mut Shared {
        self.0
    }
}

unsafe fn set_ptr(hwnd: HWND, ptr: *mut Shared) {
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize) };
}

unsafe fn ptr(hwnd: HWND) -> *mut Shared {
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Shared }
}

/// 建窗失败时重试次数。
const SPAWN_TRIES: usize = 2;
const SPAWN_RETRY_WAIT: Duration = Duration::from_millis(200);
const WAIT_RESULT: Duration = Duration::from_secs(2);

/// 建窗并跑消息循环，失败返回原因。
pub fn start(
    img: Bgra,
    stop: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
) -> Result<std::thread::JoinHandle<()>, String> {
    let mut last = String::new();
    for attempt in 1..=SPAWN_TRIES {
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let img_i = img.clone();
        let stop_i = stop.clone();
        let finished_i = finished.clone();
        let handle = std::thread::spawn(move || run(img_i, stop_i, finished_i, tx));
        match rx.recv_timeout(WAIT_RESULT) {
            Ok(Ok(())) => return Ok(handle),
            Ok(Err(e)) => {
                let _ = handle.join();
                last = e;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                last = "二维码窗口打开失败".to_string();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                last = "二维码窗口打开失败".to_string();
            }
        }
        if attempt < SPAWN_TRIES {
            std::thread::sleep(SPAWN_RETRY_WAIT);
        }
    }
    log::debug(&format!("二维码窗口建窗细节：{last}"));
    Err("二维码窗口打开失败，已改用终端二维码".to_string())
}

/// 建窗并跑消息循环。
fn run(
    img: Bgra,
    stop: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    tx: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let shared = Box::new(Shared {
        img,
        stop,
        finished,
    });
    let param = Box::into_raw(shared);
    let send_param = SendPtr(param);

    // 建窗必须在这个线程里（消息循环独占该线程）。
    // 类名每次唯一，避免重复注册。
    let class: Vec<u16> = {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let mut name = format!("SdoFfxivQrWindow_{}_{}", std::process::id(), n);
        name.push('\u{0}');
        name
    }
    .encode_utf16()
    .collect();
    let title: Vec<u16> = "扫码登录 · FFXIV\u{0}".encode_utf16().collect();
    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let s = &*send_param.get();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        let atom = RegisterClassExW(&wc);
        let reg_err = if atom == 0 {
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
        } else {
            0
        };
        let ex_style = WS_EX_TOPMOST;
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: (s.img.width * QR_SCALE) as i32,
            bottom: (s.img.height * QR_SCALE) as i32,
        };
        AdjustWindowRectEx(&mut rc, WINDOW_STYLE, 0, ex_style);
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        let (x, y) = (
            (GetSystemMetrics(SM_CXSCREEN) - w) / 2,
            (GetSystemMetrics(SM_CYSCREEN) - h) / 2,
        );
        let scaled = s.img.scaled(QR_SCALE);
        (*send_param.get()).img = scaled;

        let hwnd = CreateWindowExW(
            ex_style,
            class.as_ptr(),
            title.as_ptr(),
            WINDOW_STYLE | WS_VISIBLE,
            x,
            y,
            w,
            h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            param as *mut core::ffi::c_void,
        );
        if hwnd.is_null() {
            let code = GetLastError();
            log::debug(&format!(
                "二维码窗口建窗失败细节：code={code} atom={atom} reg_err={reg_err}"
            ));
            let _ = tx.send(Err("窗口创建被系统拒绝".to_string()));
            // 建窗失败：WM_NCCREATE 没跑过，Shared 还归我们管。
            drop(Box::from_raw(param));
            return;
        }
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        let _ = tx.send(Ok(()));

        let mut msg: MSG = std::mem::zeroed();
        loop {
            let p = ptr(hwnd);
            if !p.is_null() && (*(*p).stop).load(Ordering::Relaxed) {
                DestroyWindow(hwnd);
            }
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    return;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// `WM_PAINT`：白底 + 居中贴二维码（`StretchDIBits`，无 GPU 依赖）。
unsafe fn paint(hwnd: HWND) {
    let p = unsafe { ptr(hwnd) };
    if p.is_null() {
        return;
    }
    let img = unsafe { &(*p).img };
    let mut ps: PAINTSTRUCT = unsafe { std::mem::zeroed() };
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
    let mut rc = std::mem::zeroed();
    unsafe { GetClientRect(hwnd, &mut rc) };
    let (cw, ch) = (rc.right - rc.left, rc.bottom - rc.top);

    let mem = unsafe { CreateCompatibleDC(hdc) };
    let bmp = unsafe { CreateCompatibleBitmap(hdc, cw, ch) };
    let old = unsafe { SelectObject(mem, bmp as HGDIOBJ) };
    // 白底：二维码的静默区必须是浅色，否则扫不出来。
    let white = unsafe { CreateSolidBrush(0x00FF_FFFF) };
    unsafe { FillRect(mem, &rc, white) };
    let _ = unsafe { DeleteObject(white) };

    let mut bi: BITMAPINFO = unsafe { std::mem::zeroed() };
    bi.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: img.width as i32,
        biHeight: img.height as i32, // 正数 = 自下而上，与 Bgra 排布一致
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..unsafe { std::mem::zeroed() }
    };
    unsafe {
        StretchDIBits(
            mem,
            0,
            0,
            img.width as i32,
            img.height as i32,
            0,
            0,
            img.width as i32,
            img.height as i32,
            img.pixels.as_ptr() as *const core::ffi::c_void,
            &bi,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
        BitBlt(hdc, 0, 0, cw, ch, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old);
        DeleteObject(bmp as HGDIOBJ);
        DeleteDC(mem);
        EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let cs = lp as *const CREATESTRUCTW;
            if !cs.is_null() {
                set_ptr(hwnd, (*cs).lpCreateParams as *mut Shared);
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_PAINT => {
            unsafe { paint(hwnd) };
            0
        }
        // 自己全量重绘，不让系统擦背景（避免闪烁）。
        WM_ERASEBKGND => 1,
        WM_KEYDOWN => {
            if wp == 0x1B {
                unsafe { DestroyWindow(hwnd) };
            }
            0
        }
        WM_CLOSE => {
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_DESTROY => {
            let p = unsafe { ptr(hwnd) };
            if !p.is_null() {
                unsafe {
                    (*(*p).finished).store(true, Ordering::Relaxed);
                    drop(Box::from_raw(p)); // 释放 Shared
                    set_ptr(hwnd, std::ptr::null_mut());
                }
            }
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2×2 BGRA 测试夹具。
    fn two_by_two() -> Bgra {
        Bgra {
            width: 2,
            height: 2,
            pixels: [1u8, 2, 3, 4]
                .iter()
                .flat_map(|v| [*v, *v, *v, 0xFF])
                .collect(),
        }
    }

    fn px(img: &Bgra, x: usize, y: usize) -> u8 {
        img.pixels[(y * img.width + x) * 4]
    }

    /// 整块复制放大。
    #[test]
    fn scaled_replicates_pixels_and_keeps_row_order() {
        let out = two_by_two().scaled(2);
        assert_eq!((out.width, out.height), (4, 4));
        // 底部两行来自 buffer 第 0 行（图像底行）：1 1 2 2。
        for y in [0, 1] {
            assert_eq!(
                [
                    px(&out, 0, y),
                    px(&out, 1, y),
                    px(&out, 2, y),
                    px(&out, 3, y)
                ],
                [1, 1, 2, 2],
                "第 {y} 行"
            );
        }
        // 顶部两行来自 buffer 第 1 行：3 3 4 4。
        for y in [2, 3] {
            assert_eq!(
                [
                    px(&out, 0, y),
                    px(&out, 1, y),
                    px(&out, 2, y),
                    px(&out, 3, y)
                ],
                [3, 3, 4, 4],
                "第 {y} 行"
            );
        }
    }

    /// 放大 1 倍（含传 0 被夹到 1）必须等价于原图。
    #[test]
    fn scaled_once_is_identity() {
        let src = two_by_two();
        for scale in [0, 1] {
            let out = src.scaled(scale);
            assert_eq!((out.width, out.height), (2, 2), "scale={scale}");
            assert_eq!(out.pixels, src.pixels, "scale={scale}");
        }
    }
}
