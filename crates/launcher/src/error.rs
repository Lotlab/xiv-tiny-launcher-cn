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

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Msg(m) => write!(f, "{m}"),
            Error::UserQuit => write!(f, "用户退出"),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
