//! Host architecture detection. qvm runs on amd64 and arm64 Linux hosts;
//! per-arch differences (qemu binary name, virt-install `--machine`, UEFI
//! requirement) consult the helpers here.

use std::sync::OnceLock;

/// Cached `uname -m`, normalized to a Linux-style arch name. Falls back
/// to `std::env::consts::ARCH` when uname isn't on PATH (useful in tests).
///
/// Normalization rules:
///   - `arm64` (macOS, FreeBSD) → `aarch64` (Linux)
///   - everything else returned verbatim.
pub fn host() -> &'static str {
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| {
        let raw = crate::cmd::run("uname", ["-m"])
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| std::env::consts::ARCH.to_string());
        match raw.as_str() {
            "arm64" => "aarch64".to_string(),
            _       => raw,
        }
    }).as_str()
}

/// The qemu-system-<X> binary expected on this host.
pub fn qemu_system_bin() -> &'static str {
    match host() {
        "aarch64" | "arm64" => "qemu-system-aarch64",
        _                   => "qemu-system-x86_64",
    }
}

/// `true` when ARM, where UEFI is mandatory (no SeaBIOS on aarch64).
pub fn is_arm() -> bool {
    matches!(host(), "aarch64" | "arm64")
}

/// `--machine` value for virt-install on this arch. ARM gets `virt`;
/// x86_64 leaves machine type alone unless a distro requests q35 for UEFI.
pub fn virt_install_machine() -> Option<&'static str> {
    if is_arm() { Some("virt") } else { None }
}
