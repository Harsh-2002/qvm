//! Config-loading tests.

use qvm::config::{builtin_distros, Config};
use std::io::Write;
use std::path::Path;

fn write_tmp(contents: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().expect("tempfile");
    f.write_all(contents.as_bytes()).unwrap();
    f
}

#[test]
fn defaults_apply_when_no_file() {
    let cfg = Config::load(Some(std::path::Path::new("/nonexistent/qvm.yml"))).unwrap();
    assert_eq!(cfg.network.bridge, "br0");
    assert_eq!(cfg.defaults.cpus, 2);
    assert_eq!(cfg.defaults.memory_gb, 4);
    assert_eq!(cfg.defaults.distro, "debian:13");
    assert_eq!(cfg.defaults.grub_timeout, Some(0));
    assert!(!cfg.distros.is_empty(), "baked-in distros must populate");
}

#[test]
fn empty_file_uses_defaults_and_baked_distros() {
    // An empty YAML doc deserialises as the default Config (all #[serde(default)]).
    // serde_yaml accepts an empty string as a valid null document.
    let f = write_tmp("");
    let cfg = Config::load(Some(f.path())).unwrap();
    assert_eq!(cfg.network.bridge, "br0");
    for key in [
        "ubuntu:24.04", "ubuntu:26.04", "debian:13", "alpine:3.20", "fedora:42",
        "rocky:9", "almalinux:9", "opensuse:15.6", "centos-stream:10", "arch",
    ] {
        assert!(cfg.distros.contains_key(key), "missing baked-in distro: {key}");
    }
}

#[test]
fn user_overrides_individual_fields() {
    let f = write_tmp(r#"
network:
  bridge: vmbr0
defaults:
  cpus: 8
  memory_gb: 16
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
    // Legacy flat (single-arch) form — implicitly x86_64.
    let f = write_tmp(r#"
distros:
  "ubuntu:22.04":
    image:  ubuntu-22.04.qcow2
    osinfo: ubuntu22.04
    shell:  /bin/bash
    uefi:   false
    url:    https://example.com/ubuntu-22.04.img
"#);
    let cfg = Config::load(Some(f.path())).unwrap();
    assert!(cfg.distros.contains_key("ubuntu:22.04"));
    let d = cfg.distros.get("ubuntu:22.04").unwrap();
    let (img, url) = d.variant_for("x86_64").unwrap();
    assert_eq!(img, "ubuntu-22.04.qcow2");
    assert_eq!(url, "https://example.com/ubuntu-22.04.img");
    assert_eq!(d.osinfo, "ubuntu22.04");
    assert!(!d.uefi);
}

#[test]
fn user_can_override_a_baked_in_distro() {
    // If the user redefines debian:13, that wins.
    let f = write_tmp(r#"
distros:
  "debian:13":
    image:  my-debian.qcow2
    osinfo: debian12
    url:    https://my.mirror/debian.qcow2
"#);
    let cfg = Config::load(Some(f.path())).unwrap();
    let d = cfg.distros.get("debian:13").unwrap();
    let (img, _) = d.variant_for("x86_64").unwrap();
    assert_eq!(img, "my-debian.qcow2");
}

#[test]
fn user_can_define_per_arch_variants() {
    let f = write_tmp(r#"
distros:
  "ubuntu:22.04":
    osinfo: ubuntu22.04
    arch:
      x86_64:
        image: u22-amd64.qcow2
        url:   https://example.com/u22-amd64.img
      aarch64:
        image: u22-arm64.qcow2
        url:   https://example.com/u22-arm64.img
"#);
    let cfg = Config::load(Some(f.path())).unwrap();
    let d = cfg.distros.get("ubuntu:22.04").unwrap();
    let (img64, _) = d.variant_for("x86_64").unwrap();
    let (imgaa, _) = d.variant_for("aarch64").unwrap();
    assert_eq!(img64, "u22-amd64.qcow2");
    assert_eq!(imgaa, "u22-arm64.qcow2");
}

#[test]
fn image_path_joins_correctly() {
    let f = write_tmp(r#"
paths:
  images: /data/imgs
"#);
    let cfg = Config::load(Some(f.path())).unwrap();
    let p = cfg.image_path("debian:13").unwrap();
    assert!(p.to_str().unwrap().starts_with("/data/imgs/debian-13-"));
    assert!(p.to_str().unwrap().ends_with(".qcow2"));
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
        for (arch, v) in &d.arch {
            assert!(
                v.url.starts_with("https://"),
                "{key}/{arch} URL must be https (got: {})", v.url
            );
        }
    }
}

#[test]
fn builtin_distros_carry_both_amd64_and_arm64_variants() {
    for (key, d) in builtin_distros() {
        assert!(d.arch.contains_key("x86_64"),
            "{key} missing x86_64 variant");
        // Arch upstream ships only an x86_64 cloud image.
        if key == "arch" { continue; }
        assert!(d.arch.contains_key("aarch64"),
            "{key} missing aarch64 variant");
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
    for key in [
        "ubuntu:24.04", "ubuntu:26.04", "debian:13", "fedora:42", "rocky:9",
        "almalinux:9", "opensuse:15.6", "centos-stream:10", "arch",
    ] {
        let d = m.get(key).unwrap();
        assert!(!d.uefi, "{key} should be BIOS, not UEFI");
        assert_eq!(d.shell, "/bin/bash", "{key} should use /bin/bash");
    }
}

#[test]
fn vm_path_accessors_join_under_configured_dirs() {
    let cfg = Config::load(Some(Path::new("/nonexistent/qvm.yml"))).unwrap();
    assert_eq!(cfg.vm_disk("web01"),     Path::new("/var/lib/qvm/vms/web01.qcow2"));
    assert_eq!(cfg.vm_seed_iso("web01"), Path::new("/var/lib/qvm/cloudinit/web01.iso"));
    assert_eq!(cfg.vm_ci_dir("web01"),   Path::new("/var/lib/qvm/cloudinit/web01"));
}

#[test]
fn motd_defaults_are_enabled_with_auto_color() {
    let cfg = Config::default();
    assert!(cfg.motd.enable, "MOTD must default to enabled");
    assert_eq!(cfg.motd.color, "auto");
    // Default palette must match the 16-colour ANSI we ship.
    assert_eq!(cfg.motd.colors.label, "[0;36m");
    assert_eq!(cfg.motd.colors.ok,    "[0;32m");
}

#[test]
fn motd_toggle_off_via_yaml() {
    let f = write_tmp(r#"
motd:
  enable: false
"#);
    let cfg = Config::load(Some(f.path())).unwrap();
    assert!(!cfg.motd.enable);
}

#[test]
fn motd_palette_override_via_yaml() {
    let f = write_tmp(r#"
motd:
  colors:
    ok: "[0;34m"
"#);
    let cfg = Config::load(Some(f.path())).unwrap();
    assert_eq!(cfg.motd.colors.ok, "[0;34m");
    // Untouched fields keep defaults.
    assert_eq!(cfg.motd.colors.label, "[0;36m");
}

#[test]
fn ssh_keys_top_level_array_parses() {
    let f = write_tmp(r#"
ssh_keys:
  - "ssh-ed25519 AAAA top@host"
  - "ssh-rsa BBBB other@host"

paths:
  images: /tmp/imgs
"#);
    let cfg = Config::load(Some(f.path())).unwrap();
    assert_eq!(cfg.ssh_keys.len(), 2);
    assert_eq!(cfg.ssh_keys[0], "ssh-ed25519 AAAA top@host");
}

#[test]
fn sample_yaml_parses_without_overriding_defaults_unexpectedly() {
    // The sample shipped with `qvm init` should be valid YAML and
    // not silently change behaviour from baked-in defaults
    // (other than what's literally in the file).
    let sample = qvm::config::sample_yaml();
    let f = write_tmp(sample);
    let cfg = Config::load(Some(f.path())).expect("sample must parse");
    // Sample has bridge "br0" same as default
    assert_eq!(cfg.network.bridge, "br0");
    // Sample doesn't list distros at all, so the builtin set fills in.
    assert!(cfg.distros.len() >= 10);
    // Sample explicitly sets motd defaults — verify they round-trip.
    assert!(cfg.motd.enable);
    assert_eq!(cfg.motd.color, "auto");
}

#[test]
fn render_config_round_trips_back_through_load() {
    // The TUI/onboarding-generated YAML must be parseable by the same
    // loader, otherwise users will hit a confusing "qvm init wrote
    // something I can't read" loop. This test guards that contract.
    use qvm::commands::init::{render_config, WizardAnswers};
    let keys = vec!["ssh-ed25519 AAAA me@laptop".to_string()];
    let rendered = render_config(WizardAnswers {
        bridge: "br0",
        distro: "debian:13",
        cpus: 2, memory_gb: 4, disk_gb: 50,
        autostart: true, grub_timeout: 0,
        vnc_bind: "127.0.0.1",
        ssh_keys: &keys,
        images_path:    "/var/lib/qvm/images",
        vms_path:       "/var/lib/qvm/vms",
        cloudinit_path: "/var/lib/qvm/cloudinit",
    });
    let f = write_tmp(&rendered);
    let cfg = Config::load(Some(f.path())).expect("render_config output must parse");
    assert_eq!(cfg.ssh_keys, vec!["ssh-ed25519 AAAA me@laptop".to_string()]);
    assert_eq!(cfg.network.bridge, "br0");
    assert_eq!(cfg.defaults.distro, "debian:13");
    assert_eq!(cfg.motd.color, "auto");
    assert!(cfg.motd.enable);
}
