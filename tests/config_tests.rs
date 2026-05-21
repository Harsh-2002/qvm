//! Config-loading tests.

use qvm::config::{builtin_distros, Config};
use std::io::Write;

fn write_tmp(contents: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().expect("tempfile");
    f.write_all(contents.as_bytes()).unwrap();
    f
}

#[test]
fn defaults_apply_when_no_file() {
    let cfg = Config::load(Some(std::path::Path::new("/nonexistent/qvm.toml"))).unwrap();
    assert_eq!(cfg.network.bridge, "br0");
    assert_eq!(cfg.defaults.cpus, 2);
    assert_eq!(cfg.defaults.memory_gb, 4);
    assert_eq!(cfg.defaults.distro, "debian:13");
    assert_eq!(cfg.defaults.grub_timeout, Some(0));
    assert!(!cfg.distros.is_empty(), "baked-in distros must populate");
}

#[test]
fn empty_file_uses_defaults_and_baked_distros() {
    let f = write_tmp("");
    let cfg = Config::load(Some(f.path())).unwrap();
    assert_eq!(cfg.network.bridge, "br0");
    assert!(cfg.distros.contains_key("ubuntu:24.04"));
    assert!(cfg.distros.contains_key("debian:13"));
    assert!(cfg.distros.contains_key("alpine:3.20"));
    assert!(cfg.distros.contains_key("fedora:42"));
    assert!(cfg.distros.contains_key("rocky:9"));
}

#[test]
fn user_overrides_individual_fields() {
    let f = write_tmp(r#"
[network]
bridge = "vmbr0"

[defaults]
cpus = 8
memory_gb = 16
"#);
    let cfg = Config::load(Some(f.path())).unwrap();
    assert_eq!(cfg.network.bridge, "vmbr0");
    assert_eq!(cfg.defaults.cpus, 8);
    assert_eq!(cfg.defaults.memory_gb, 16);
    // Untouched fields keep defaults
    assert_eq!(cfg.defaults.disk_gb, 50);
    assert_eq!(cfg.defaults.distro, "debian:13");
}

#[test]
fn user_can_add_a_custom_distro() {
    let f = write_tmp(r#"
[distros."ubuntu:22.04"]
image  = "ubuntu-22.04.qcow2"
osinfo = "ubuntu22.04"
shell  = "/bin/bash"
uefi   = false
url    = "https://example.com/ubuntu-22.04.img"
"#);
    let cfg = Config::load(Some(f.path())).unwrap();
    assert!(cfg.distros.contains_key("ubuntu:22.04"));
    let d = cfg.distros.get("ubuntu:22.04").unwrap();
    assert_eq!(d.image, "ubuntu-22.04.qcow2");
    assert_eq!(d.osinfo, "ubuntu22.04");
    assert!(!d.uefi);
}

#[test]
fn user_can_override_a_baked_in_distro() {
    // If the user redefines debian:13, that wins.
    let f = write_tmp(r#"
[distros."debian:13"]
image  = "my-debian.qcow2"
osinfo = "debian12"
url    = "https://my.mirror/debian.qcow2"
"#);
    let cfg = Config::load(Some(f.path())).unwrap();
    let d = cfg.distros.get("debian:13").unwrap();
    assert_eq!(d.image, "my-debian.qcow2");
}

#[test]
fn image_path_joins_correctly() {
    let f = write_tmp(r#"
[paths]
images = "/data/imgs"
"#);
    let cfg = Config::load(Some(f.path())).unwrap();
    let p = cfg.image_path("debian:13").unwrap();
    assert_eq!(p.to_str().unwrap(), "/data/imgs/debian-13.qcow2");
}

#[test]
fn unknown_distro_errors_helpfully() {
    let cfg = Config::default();
    let err = cfg.distro("not-a-distro").unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("unknown distro"), "msg was: {msg}");
    assert!(msg.contains("qvm distros"), "should hint at qvm distros");
}

#[test]
fn builtin_distros_all_have_https_urls() {
    for (key, d) in builtin_distros() {
        assert!(
            d.url.starts_with("https://"),
            "{key} URL must be https (got: {})", d.url
        );
    }
}

#[test]
fn builtin_distros_alpine_uses_uefi_and_sh() {
    let m = builtin_distros();
    let a = m.get("alpine:3.20").expect("alpine present");
    assert!(a.uefi, "alpine cloud image requires UEFI");
    assert_eq!(a.shell, "/bin/sh", "alpine has no bash by default");
}

#[test]
fn builtin_distros_others_are_bios_and_bash() {
    let m = builtin_distros();
    for key in ["ubuntu:24.04", "debian:13", "fedora:42", "rocky:9"] {
        let d = m.get(key).unwrap();
        assert!(!d.uefi, "{key} should be BIOS, not UEFI");
        assert_eq!(d.shell, "/bin/bash", "{key} should use /bin/bash");
    }
}

#[test]
fn sample_toml_parses_without_overriding_defaults_unexpectedly() {
    // The sample shipped with `qvm init` should be valid TOML and
    // not silently change behaviour from baked-in defaults
    // (other than what's literally in the file).
    let sample = qvm::config::sample_toml();
    let f = write_tmp(sample);
    let cfg = Config::load(Some(f.path())).expect("sample must parse");
    // Sample has bridge "br0" same as default
    assert_eq!(cfg.network.bridge, "br0");
    // Sample has all 5 baked distros in the registry
    assert!(cfg.distros.len() >= 5);
}
