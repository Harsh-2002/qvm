//! Run external commands. The whole tool is a thin shell over these.

use crate::error::{Error, Result};
use std::ffi::OsStr;
use std::process::{Command, Stdio};

/// Run a command, return stdout. Fail if the command exits non-zero
/// or if the binary is missing.
pub fn run<I, S>(prog: &str, args: I) -> Result<String>
where I: IntoIterator<Item = S>, S: AsRef<OsStr>
{
    let out = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Error::User(format!("cannot run `{prog}`: {e}")))?;
    if !out.status.success() {
        return Err(Error::Command {
            cmd: prog.into(),
            status: out.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Like `run`, but discard stdout and inherit stderr (so progress shows live).
/// stdin is null'd — appropriate for non-interactive batch programs (wget,
/// qemu-img, virt-install). Use [`run_tty`] for interactive children.
pub fn run_inherit<I, S>(prog: &str, args: I) -> Result<()>
where I: IntoIterator<Item = S>, S: AsRef<OsStr>
{
    let status = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| Error::User(format!("cannot run `{prog}`: {e}")))?;
    if !status.success() {
        return Err(Error::Command {
            cmd: prog.into(),
            status: status.code().unwrap_or(-1),
            stderr: String::new(),
        });
    }
    Ok(())
}

/// Run an interactive child: inherit ALL three stdio. Use this only when
/// the child needs keyboard input (`virsh console`, `virsh edit`). For
/// batch downloads/conversions, prefer [`run_inherit`] (stdin null'd).
pub fn run_tty<I, S>(prog: &str, args: I) -> Result<()>
where I: IntoIterator<Item = S>, S: AsRef<OsStr>
{
    let status = Command::new(prog)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| Error::User(format!("cannot run `{prog}`: {e}")))?;
    if !status.success() {
        return Err(Error::Command {
            cmd: prog.into(),
            status: status.code().unwrap_or(-1),
            stderr: String::new(),
        });
    }
    Ok(())
}

/// Replace this process with another (for `qvm console`, `qvm vnc --open`, etc.).
/// Returns only on error.
#[allow(dead_code)]
pub fn exec<I, S>(prog: &str, args: I) -> Result<()>
where I: IntoIterator<Item = S>, S: AsRef<OsStr>
{
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new(prog);
    cmd.args(args);
    Err(Error::User(format!("cannot exec `{prog}`: {}", cmd.exec())))
}

/// True if the binary is in PATH.
pub fn have(prog: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {prog} >/dev/null 2>&1"))
        .status().map(|s| s.success()).unwrap_or(false)
}

/// Require a binary or fail with a helpful message.
pub fn require(prog: &str) -> Result<()> {
    if !have(prog) {
        return Err(Error::User(format!(
            "`{prog}` not found in PATH. Install it and try again."
        )));
    }
    Ok(())
}
