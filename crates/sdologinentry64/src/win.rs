//! Win32 FFI 辅助（DLL 侧）：指针可读性校验、宽字符串、GUID、env、BSTR。

use std::ffi::c_void;

use windows_sys::Win32::Foundation::BOOL;
use windows_sys::Win32::System::Environment::{GetEnvironmentVariableW, SetEnvironmentVariableW};
use windows_sys::Win32::System::Memory::{
    VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
    PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE,
    PAGE_WRITECOPY,
};

#[link(name = "oleaut32")]
extern "system" {
    fn SysAllocString(psz: *const u16) -> *mut u16;
}

/// `&str` → 以 NUL 结尾的 UTF-16 序列（`W` 系列 API 的入参形态）。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 分配 BSTR（调用方负责 `SysFreeString`）。
pub fn bstr(s: &str) -> *mut u16 {
    let w = wide(s);
    unsafe { SysAllocString(w.as_ptr()) }
}

/// 指针可读性检查：非空、已提交、无 `PAGE_GUARD`，且 `len` 字节落在同一区域内。
pub fn readable(ptr: *const u8, len: usize) -> bool {
    if ptr.is_null() {
        return false;
    }
    unsafe {
        let mut mbi: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
        let r = VirtualQuery(
            ptr as *const c_void,
            &mut mbi,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        );
        if r == 0 || mbi.State != MEM_COMMIT {
            return false;
        }
        let p = mbi.Protect;
        if p == PAGE_NOACCESS || p & PAGE_GUARD != 0 {
            return false;
        }
        let readable_prot = p
            & (PAGE_READONLY
                | PAGE_READWRITE
                | PAGE_WRITECOPY
                | PAGE_EXECUTE_READ
                | PAGE_EXECUTE_READWRITE
                | PAGE_EXECUTE_WRITECOPY)
            != 0;
        if !readable_prot {
            return false;
        }
        let base = mbi.BaseAddress as usize;
        let end = ptr as usize + len.max(1);
        end <= base + mbi.RegionSize
    }
}

/// 读取宽字符串（上限 `max` 个字符），失败按空串。
/// 仅测试使用：生产代码走 BSTR/环境变量路径，不直接读宽串。
#[cfg(test)]
pub fn read_wstr(ptr: *const u16, max: usize) -> String {
    if ptr.is_null() {
        return String::new();
    }
    if !readable(ptr as *const u8, max * 2) {
        return String::new();
    }
    let mut out: Vec<u16> = Vec::with_capacity(max);
    for i in 0..max {
        let ch = unsafe { *ptr.add(i) };
        if ch == 0 {
            break;
        }
        out.push(ch);
    }
    String::from_utf16_lossy(&out)
}

/// IID/CLSID 结构
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

/// GUID → 大写连字符字符串；指针不可读返回 `<null>`。
pub fn guid_to_string(g: *const Guid) -> String {
    if g.is_null() || !readable(g as *const u8, std::mem::size_of::<Guid>()) {
        return "<null>".to_string();
    }
    let g = unsafe { *g };
    format!(
        "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        g.data1,
        g.data2,
        g.data3,
        g.data4[0],
        g.data4[1],
        g.data4[2],
        g.data4[3],
        g.data4[4],
        g.data4[5],
        g.data4[6],
        g.data4[7]
    )
}

/// `GetEnvironmentVariableW`；不存在/为空返回 None。
pub fn get_env(name: &str) -> Option<String> {
    let key = wide(name);
    let mut buf = vec![0u16; 8192];
    let n: u32 =
        unsafe { GetEnvironmentVariableW(key.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if n == 0 || n as usize > buf.len() {
        return None;
    }
    buf.truncate(n as usize);
    String::from_utf16(&buf).ok().filter(|s| !s.is_empty())
}

/// `SetEnvironmentVariableW(name, NULL)`：仅清除本进程内的变量。
pub fn clear_env(name: &str) {
    let key = wide(name);
    unsafe {
        let _: BOOL = SetEnvironmentVariableW(key.as_ptr(), std::ptr::null());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_formatting() {
        let g = Guid {
            data1: 0xAD887932,
            data2: 0x2D1C,
            data3: 0x48EC,
            data4: [0xB3, 0xE0, 0x53, 0x5B, 0x60, 0x9C, 0x12, 0xD6],
        };
        assert_eq!(
            guid_to_string(&g as *const Guid),
            "AD887932-2D1C-48EC-B3E0-535B609C12D6"
        );
        assert_eq!(guid_to_string(std::ptr::null()), "<null>");
    }

    #[test]
    fn readable_checks() {
        let v = [1u8, 2, 3, 4];
        assert!(readable(v.as_ptr(), 4));
        assert!(!readable(std::ptr::null(), 4));
        // 1 字节缓冲区不能声称可读 1MB
        assert!(!readable(v.as_ptr(), 1 << 20));
    }

    #[test]
    fn wstr_roundtrip() {
        let s: Vec<u16> = "DESKTOP-ABC1234"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        assert_eq!(read_wstr(s.as_ptr(), 32), "DESKTOP-ABC1234");
        // 超过上限即截断
        assert_eq!(read_wstr(s.as_ptr(), 5), "DESKT");
        assert_eq!(read_wstr(std::ptr::null(), 32), "");
    }
}
