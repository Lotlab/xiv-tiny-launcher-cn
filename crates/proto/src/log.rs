//! 脱敏日志。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

struct Logger {
    file: Mutex<FileState>,
    echo: bool,
}

/// 文件句柄 + 未落盘的缓冲行数。
struct FileState {
    file: Option<std::fs::File>,
    pending: usize,
}

const FLUSH_EVERY: usize = 32;

const MAX_LOG_BYTES: u64 = 1 << 20;

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// 初始化全局日志，返回实际落点。
pub fn init(log_file_name: &str, echo: bool, explicit: Option<&Path>) -> PathBuf {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = explicit {
        candidates.push(p.to_path_buf());
    }
    for c in crate::paths::log_file_candidates(log_file_name) {
        if !candidates.contains(&c) {
            candidates.push(c);
        }
    }
    let primary = candidates
        .first()
        .cloned()
        .unwrap_or_else(|| PathBuf::from(log_file_name));

    let mut chosen: Option<(PathBuf, std::fs::File)> = None;
    for c in &candidates {
        if let Some(parent) = c.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let oversized = std::fs::metadata(c)
            .map(|m| m.len() > MAX_LOG_BYTES)
            .unwrap_or(false);
        let open = OpenOptions::new()
            .create(true)
            .write(true)
            .append(!oversized)
            .truncate(oversized)
            .open(c);
        match open {
            Ok(f) => {
                chosen = Some((c.clone(), f));
                break;
            }
            Err(e) => eprintln!("[WARN] 日志路径不可写：{}（{e}）", c.display()),
        }
    }

    let (path, file) = match chosen {
        Some((p, f)) => (p, Some(f)),
        None => {
            eprintln!("[WARN] 日志无可用落点，本次运行不写日志");
            (primary.clone(), None)
        }
    };
    if file.is_some() && path != primary {
        eprintln!(
            "[WARN] 日志未落在 {}，已改用 {}",
            primary.display(),
            path.display()
        );
    }

    // 进程内只初始化一次。
    let initialized = LOGGER
        .set(Logger {
            file: Mutex::new(FileState { file, pending: 0 }),
            echo,
        })
        .is_ok();
    if !initialized {
        return path;
    }
    info(&format!("日志初始化 path={}", path.display()));
    path
}

/// 把缓冲的行立即落盘。进程退出前调用。
pub fn flush() {
    if let Some(l) = LOGGER.get() {
        if let Ok(mut g) = l.file.lock() {
            if let Some(f) = g.file.as_mut() {
                let _ = f.flush();
            }
            g.pending = 0;
        }
    }
}

pub fn info(msg: &str) {
    write(Level::Info, msg);
}
/// 仅写文件，不回显到终端。轮询等待、模板细节等排查信息用它。
pub fn debug(msg: &str) {
    write(Level::Debug, msg);
}
pub fn warn(msg: &str) {
    write(Level::Warn, msg);
}
pub fn error(msg: &str) {
    write(Level::Error, msg);
}

pub(crate) fn write(level: Level, msg: &str) {
    let line = format!("[{}][{}] {}", now_stamp(), level.tag(), sanitize(msg));
    if let Some(l) = LOGGER.get() {
        if let Ok(mut g) = l.file.lock() {
            let flush = !matches!(level, Level::Info | Level::Debug) || g.pending + 1 >= FLUSH_EVERY;
            if let Some(f) = g.file.as_mut() {
                let _ = writeln!(f, "{}", line);
                if flush {
                    let _ = f.flush();
                }
            }
            if g.file.is_some() {
                g.pending = if flush { 0 } else { g.pending + 1 };
            }
        }
        if l.echo && matches!(level, Level::Warn | Level::Error) {
            eprintln!("{}", line);
        }
    }
}

/// `HH:MM:SS.mmm`。
fn now_stamp() -> String {
    let ms = crate::clock::now_millis() as u64;
    let tod = (ms / 1000) % 86_400;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60,
        ms % 1000
    )
}

/// 需要掩码的参数名。
const SECRET_KEYS: [&str; 14] = [
    "ticket",
    "ticket0",
    "ticket1",
    "tgt",
    "tgt0",
    "guid",
    "guid0",
    "keepLoginKey",
    "codeKey",
    "authorization",
    "authenToken",
    "sndaId",
    "sndaId0",
    "sndaId1",
];

/// 掩码单个敏感值。
pub fn mask(v: &str) -> String {
    let n = v.chars().count();
    if v.is_empty() {
        return String::new();
    }
    if n <= 8 {
        return format!("****({n})");
    }
    let head: String = v.chars().take(6).collect();
    format!("{}…({})", head, n)
}

/// 对整行文本做敏感值掩码。
pub fn sanitize(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let mut i = 0usize;
    while i < bytes.len() {
        let boundary = i == 0
            || matches!(
                bytes[i - 1],
                b'&' | b'?' | b' ' | b'\t' | b',' | b':' | b'"' | b'\''
            );
        if boundary {
            if let Some(klen) = match_secret_key(&bytes[i..]) {
                let value_start = i + klen + 1;
                let mut value_end = value_start;
                while value_end < bytes.len()
                    && !matches!(bytes[value_end], b'&' | b' ' | b'\t' | b'\r' | b'\n' | b',')
                {
                    value_end += 1;
                }
                out.extend_from_slice(&bytes[i..value_start]);
                out.extend_from_slice(mask(&s[value_start..value_end]).as_bytes());
                i = value_end;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn match_secret_key(b: &[u8]) -> Option<usize> {
    for k in SECRET_KEYS {
        let kb = k.as_bytes();
        if b.len() > kb.len() && &b[..kb.len()] == kb && b[kb.len()] == b'=' {
            return Some(kb.len());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_masks_secrets_in_url() {
        let url = "https://cas.sdo.com/authen/codeKeyLogin.json?codeKey=0123456789abcdef0123456789abcdef&guid=0123456789ABCDEF0123456789ABCDEF&keepLoginFlag=1";
        let out = sanitize(url);
        assert!(
            !out.contains("0123456789abcdef0123456789abcdef"),
            "codeKey leaked: {out}"
        );
        assert!(
            !out.contains("0123456789ABCDEF0123456789ABCDEF"),
            "guid leaked: {out}"
        );
        assert!(
            out.contains("keepLoginFlag=1"),
            "non-secret param mangled: {out}"
        );
        assert!(out.contains("codeKey=012345"), "{out}");
    }

    #[test]
    fn sanitize_masks_suffixed_names() {
        let line = "SSO 换票成功 ticket1=ULS21-ABCDEFGH sndaId1=1234567890 tgt0=ULSTGT-XYZ";
        let out = sanitize(line);
        assert!(!out.contains("ULS21-ABCDEFGH"), "{out}");
        assert!(!out.contains("1234567890"), "{out}");
        assert!(!out.contains("ULSTGT-XYZ"), "{out}");
    }

    #[test]
    fn sanitize_plain_text_untouched() {
        assert_eq!(sanitize("no secrets here"), "no secrets here");
    }

    #[test]
    fn mask_shape() {
        assert_eq!(mask(""), "");
        assert_eq!(mask("abcd"), "****(4)");
        assert!(mask("ULS21-abcdefghij").starts_with("ULS21-"));
    }

    /// 超限截断、攒批落盘。本测试占用全局 logger，只用显式路径。
    #[test]
    fn oversized_log_is_truncated_and_flush_persists() {
        let dir = std::env::temp_dir().join(format!("xivtl-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("launcher.log");
        std::fs::write(&path, vec![b'x'; (MAX_LOG_BYTES + 1024) as usize]).unwrap();

        init("launcher.log", false, Some(&path));
        assert!(
            std::fs::metadata(&path).unwrap().len() < MAX_LOG_BYTES,
            "超限日志必须被截断"
        );
        info("截断后写入的一行");
        flush();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("截断后写入的一行"), "{text}");
        assert!(!text.contains("xxxx"), "旧内容必须已被截断");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
