#[derive(Debug)]
pub enum Error {
    Msg(String),
    UserQuit,
}

impl Error {
    pub fn msg<S: Into<String>>(s: S) -> Error {
        Error::Msg(s.into())
    }
}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::Msg(s)
    }
}

/// 网络层错误统一转成用户可见文案（分类只用于 sdo-client 内部的重试判断）。
impl From<sdo_client::Error> for Error {
    fn from(e: sdo_client::Error) -> Self {
        Error::Msg(e.to_string())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Msg(m) => write!(f, "{m}"),
            Error::UserQuit => write!(f, "用户退出"),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
