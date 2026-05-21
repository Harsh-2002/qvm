use crate::error::{Error, Result};
use rand::Rng;

/// libvirt domain name rules (kept permissive but safe).
pub fn valid_vm_name(s: &str) -> bool {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    cs.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

/// Linux username: lowercase, start [a-z_], <=32 chars.
pub fn valid_username(s: &str) -> bool {
    if s.is_empty() || s.len() > 32 { return false; }
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if c.is_ascii_lowercase() || c == '_' => {}
        _ => return false,
    }
    cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Generate "vm" + 6 lowercase alphanumerics. Always valid as a Linux username.
pub fn random_username() -> String {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    let s: String = (0..6)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect();
    format!("vm{s}")
}

pub fn require_name(name: &str) -> Result<()> {
    if !valid_vm_name(name) {
        return Err(Error::User(format!(
            "invalid VM name '{name}'. Use letters, digits, . - _ and start alnum."
        )));
    }
    Ok(())
}

pub fn require_username(u: &str) -> Result<()> {
    if !valid_username(u) {
        return Err(Error::User(format!(
            "invalid username '{u}'. Lowercase, start [a-z_], max 32 chars."
        )));
    }
    Ok(())
}

/// SHA-512 crypt of a plaintext password (compatible with /etc/shadow and cloud-init).
pub fn hash_password(plain: &str) -> Result<String> {
    use sha_crypt::{sha512_simple, Sha512Params};
    let params = Sha512Params::new(5000)
        .map_err(|e| Error::User(format!("password hash setup failed: {e:?}")))?;
    sha512_simple(plain, &params)
        .map_err(|e| Error::User(format!("password hash failed: {e:?}")))
}
