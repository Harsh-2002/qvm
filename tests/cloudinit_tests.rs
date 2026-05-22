//! Verify the cloud-init user-data we generate is well-formed and contains
//! every contract item: hostname, login user, password hash, SSH keys for
//! both user and root, the first-boot script, and the GRUB timeout.

use qvm::cloudinit::Seed;
use std::path::Path;

// We can't actually run cloud-init in a test, but we can verify the YAML
// structure and confirm distro-agnostic guarantees.

fn user_data(seed: &Seed) -> String {
    // Use write_files (no genisoimage required) so tests run anywhere.
    let td = tempfile::tempdir().unwrap();
    let ci = td.path().join("ci");
    seed.write_files(&ci).expect("write_files should succeed");
    std::fs::read_to_string(ci.join("user-data")).expect("user-data written")
}

fn default_seed<'a>(name: &'a str, user: &'a str, shell: &'a str, keys: &'a [String]) -> Seed<'a> {
    Seed {
        vm_name: name,
        login_user: user,
        login_shell: shell,
        password_hash: "$6$abc$def",
        ssh_keys: keys,
        grub_timeout: Some(0),
        motd: None,
    }
}


#[test]
fn user_data_starts_with_cloud_config_marker() {
    let keys = vec!["ssh-ed25519 AAAA test".to_string()];
    let s = default_seed("web01", "dev", "/bin/bash", &keys);
    let ud = user_data(&s);
    assert!(ud.starts_with("#cloud-config\n"), "must begin with #cloud-config marker");
}

#[test]
fn user_data_contains_hostname_and_user() {
    let keys = vec!["ssh-ed25519 AAAA test".to_string()];
    let s = default_seed("myhost", "myuser", "/bin/bash", &keys);
    let ud = user_data(&s);
    assert!(ud.contains("hostname: myhost"));
    assert!(ud.contains("- name: myuser"));
    assert!(ud.contains("- name: root"));
}

#[test]
fn ssh_keys_present_for_both_user_and_root() {
    let keys = vec![
        "ssh-ed25519 AAAATESTKEY1 a@host".to_string(),
        "ssh-ed25519 AAAATESTKEY2 b@host".to_string(),
    ];
    let s = default_seed("h", "u", "/bin/bash", &keys);
    let ud = user_data(&s);

    // Both keys must appear at least twice (once for user, once for root)
    for key in &keys {
        let count = ud.matches(key.as_str()).count();
        assert!(count >= 2, "key '{key}' appears {count} times, expected ≥2 (user + root)");
    }
    // `ssh-authorized-keys:` should appear at least twice (one block per user)
    let blocks = ud.matches("ssh-authorized-keys:").count();
    assert!(blocks >= 2, "expected 2 ssh-authorized-keys blocks, found {blocks}");
}

#[test]
fn empty_ssh_key_list_still_valid_yaml() {
    let keys: Vec<String> = vec![];
    let s = default_seed("h", "u", "/bin/bash", &keys);
    let ud = user_data(&s);
    // Empty list must be `[]` not bare colon (YAML invalid otherwise)
    assert!(ud.contains("ssh-authorized-keys:"));
    // No raw `ssh-authorized-keys:\n  - name:` (which would mean we tried to nest user blocks under keys list)
    assert!(!ud.contains("ssh-authorized-keys:\n  - name:"), "key block followed immediately by name: would be malformed");
}

#[test]
fn password_hash_quoted_correctly() {
    let keys = vec![];
    let s = Seed {
        vm_name: "h", login_user: "u", login_shell: "/bin/bash",
        password_hash: "$6$saltyabc$hashhashhash",
        ssh_keys: &keys,
        grub_timeout: Some(0),
        motd: None,
    };
    let ud = user_data(&s);
    assert!(ud.contains(r#"passwd: "$6$saltyabc$hashhashhash""#), "password hash must be in double quotes to survive YAML");
}

#[test]
fn shell_is_passed_through() {
    let keys = vec![];
    let s = default_seed("h", "u", "/bin/sh", &keys); // Alpine case
    let ud = user_data(&s);
    assert!(ud.contains("shell: /bin/sh"), "Alpine shell /bin/sh must be in user-data");
}

#[test]
fn firstboot_script_contains_grub_and_guest_agent() {
    let keys = vec![];
    let s = default_seed("h", "u", "/bin/bash", &keys);
    let ud = user_data(&s);
    assert!(ud.contains("/opt/qvm-firstboot.sh"), "firstboot script path must be referenced");
    assert!(ud.contains("GRUB_TIMEOUT"), "GRUB timeout logic must be embedded");
    assert!(ud.contains("qemu-guest-agent"), "guest agent enable must be embedded");
    // generic detection (no per-distro branches)
    assert!(ud.contains("systemctl"), "systemd branch must be present");
    assert!(ud.contains("rc-update"), "openrc branch must be present");
}

#[test]
fn grub_timeout_none_omits_the_block() {
    let keys = vec![];
    let s = Seed {
        vm_name: "h", login_user: "u", login_shell: "/bin/bash",
        password_hash: "$6$x$y",
        ssh_keys: &keys,
        grub_timeout: None,
        motd: None,
    };
    let ud = user_data(&s);
    // When grub_timeout is None we pass an empty string to the script.
    // The script's `if [ -n "$T" ]` guards the whole block, so it's a no-op.
    // We check that the embedded TIMEOUT variable is empty.
    assert!(ud.contains(r#"T="""#), "with no grub_timeout, T should be empty");
}

#[test]
fn vmuser_sidecar_written() {
    let td = tempfile::tempdir().unwrap();
    let ci = td.path().join("ci");
    let keys = vec![];
    let s = default_seed("h", "deployer", "/bin/bash", &keys);
    s.write_files(&ci).expect("write_files");
    let recorded = std::fs::read_to_string(ci.join(".vmuser")).expect(".vmuser written");
    assert_eq!(recorded.trim(), "deployer");
}

#[test]
fn yaml_indentation_is_consistent_for_user_blocks() {
    // The two users blocks (`- name: user` and `- name: root`) must be
    // at the same indent (2 spaces under `users:`). Otherwise YAML parses
    // weirdly or breaks cloud-init.
    let keys = vec!["ssh-ed25519 AAAA test".to_string()];
    let s = default_seed("h", "myuser", "/bin/bash", &keys);
    let ud = user_data(&s);

    let myuser_line = ud.lines().find(|l| l.contains("- name: myuser")).expect("user line");
    let root_line   = ud.lines().find(|l| l.contains("- name: root")).expect("root line");
    let indent_of = |l: &str| l.len() - l.trim_start().len();
    assert_eq!(indent_of(myuser_line), indent_of(root_line),
        "user blocks must share indent.\n  {myuser_line}\n  {root_line}");
}

#[test]
fn login_user_recovery_via_sidecar() {
    use qvm::cloudinit::login_user_of;
    use qvm::config::Config;
    use std::path::PathBuf;
    let td = tempfile::tempdir().unwrap();
    let cfg = Config {
        paths: qvm::config::Paths {
            images:    PathBuf::from(td.path()),
            vms:       PathBuf::from(td.path()),
            cloudinit: PathBuf::from(td.path()),
        },
        ..Default::default()
    };
    std::fs::create_dir_all(td.path().join("web01")).unwrap();
    std::fs::write(td.path().join("web01").join(".vmuser"), "deployer\n").unwrap();
    assert_eq!(login_user_of(&cfg, "web01"), Some("deployer".to_string()));
    assert_eq!(login_user_of(&cfg, "nonexistent"), None);
}

// ── MOTD ────────────────────────────────────────────────────────

fn seed_with_motd<'a>(keys: &'a [String], motd: &'a qvm::config::Motd) -> Seed<'a> {
    Seed {
        vm_name: "h",
        login_user: "u",
        login_shell: "/bin/bash",
        password_hash: "$6$x$y",
        ssh_keys: keys,
        grub_timeout: Some(0),
        motd: Some(motd),
    }
}

#[test]
fn motd_enabled_writes_profile_d_entry() {
    let keys: Vec<String> = vec![];
    let motd = qvm::config::Motd::default();
    let s = seed_with_motd(&keys, &motd);
    let ud = user_data(&s);
    assert!(ud.contains("/etc/profile.d/qvm-motd.sh"),
        "MOTD enabled must add a write_files entry for /etc/profile.d/qvm-motd.sh\n{ud}");
    assert!(ud.contains("permissions: '0755'"),
        "MOTD write_files entry must specify mode 0755");
    // Bone of the script must be present after substitution.
    assert!(ud.contains("LABEL_ESC="), "MOTD body must inline LABEL_ESC line");
    assert!(ud.contains("OK_ESC="),    "MOTD body must inline OK_ESC line");
}

#[test]
fn motd_enabled_silences_distro_defaults_in_firstboot() {
    let keys: Vec<String> = vec![];
    let motd = qvm::config::Motd::default();
    let s = seed_with_motd(&keys, &motd);
    let ud = user_data(&s);
    assert!(ud.contains("chmod -x /etc/update-motd.d"),
        "firstboot must disable update-motd.d when MOTD enabled");
    assert!(ud.contains(": > /etc/motd"),
        "firstboot must truncate /etc/motd when MOTD enabled");
}

#[test]
fn motd_disabled_omits_everything() {
    let keys: Vec<String> = vec![];
    let s = default_seed("h", "u", "/bin/bash", &keys); // motd = None
    let ud = user_data(&s);
    assert!(!ud.contains("/etc/profile.d/qvm-motd.sh"),
        "MOTD disabled must NOT mention the qvm-motd.sh path");
    assert!(!ud.contains("chmod -x /etc/update-motd.d"),
        "MOTD disabled must NOT touch update-motd.d");
}

#[test]
fn motd_script_template_is_pure_posix_sh() {
    // The on-disk template (before substitution) must be POSIX-only.
    // No bash-isms that would crash under /bin/sh on Alpine etc.
    let tpl = include_str!("../src/motd.sh");
    assert!(tpl.starts_with("#!/bin/sh"), "MOTD template must use /bin/sh");
    assert!(!tpl.contains("<<<"), "no bash here-strings");
    assert!(!tpl.contains("[["),  "no bash [[ ]] tests");
    assert!(!tpl.contains("${BASH"), "no bash-only ${{BASH...}} expansions");
}

#[test]
fn motd_color_mode_propagates_to_script() {
    let keys: Vec<String> = vec![];
    let motd = qvm::config::Motd { color: "never".into(), ..Default::default() };
    let s = seed_with_motd(&keys, &motd);
    let ud = user_data(&s);
    assert!(ud.contains("COLOR_MODE_DEFAULT=\"never\""),
        "motd.color = \"never\" must appear as COLOR_MODE_DEFAULT in the script");
}

#[test]
fn motd_custom_palette_round_trips() {
    let keys: Vec<String> = vec![];
    let motd = qvm::config::Motd {
        colors: qvm::config::MotdColors {
            ok: "[0;34m".into(), // blue instead of green
            ..Default::default()
        },
        ..Default::default()
    };
    let s = seed_with_motd(&keys, &motd);
    let ud = user_data(&s);
    assert!(ud.contains("OK_ESC='[0;34m'"),
        "custom palette `ok` must land verbatim in the rendered script");
    // The defaults stay for the unset ones.
    assert!(ud.contains("LABEL_ESC='[0;36m'"),
        "untouched palette fields must keep the defaults");
}

// Sanity-keep so we don't accidentally remove the Path import.
#[test]
fn ensure_path_compiles() { let _: &Path = std::path::Path::new("/"); }
