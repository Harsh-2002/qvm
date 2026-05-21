use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// User-facing error - print and exit 1, no stack trace.
    User(String),
    /// External command (virsh, qemu-img, ...) returned non-zero.
    Command {
        cmd: String,
        status: i32,
        stderr: String,
    },
    /// I/O failure (config read, qcow2 path, etc.).
    Io(std::io::Error),
    /// TOML parsing.
    Toml(toml::de::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::User(m) => write!(f, "{m}"),
            Error::Command { cmd, status, stderr } => {
                if stderr.is_empty() {
                    write!(f, "command `{cmd}` failed (exit {status})")
                } else {
                    write!(f, "command `{cmd}` failed (exit {status}):\n{}", stderr.trim())
                }
            }
            Error::Io(e) => write!(f, "{e}"),
            Error::Toml(e) => write!(f, "config parse error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self { Error::Io(e) }
}
impl From<toml::de::Error> for Error {
    fn from(e: toml::de::Error) -> Self { Error::Toml(e) }
}

#[macro_export]
macro_rules! bail {
    ($($arg:tt)*) => { return Err($crate::error::Error::User(format!($($arg)*))) };
}
