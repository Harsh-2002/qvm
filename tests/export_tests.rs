use qvm::commands::export::Meta;

#[test]
fn meta_parses_full_file() {
    let raw = r#"
name         = "web01"
arch         = "x86_64"
qvm_version  = "0.1.0"
exported_at  = "2026-05-21T10:00:00Z"
export_mode  = "live"
source_host  = "aether"
distro_hint  = "debian:13"
cpus         = 2
memory_gb    = 4
disk_gb      = 50
disk_sha256  = "deadbeef"
"#;
    let m = Meta::parse(raw);
    assert_eq!(m.name, "web01");
    assert_eq!(m.arch, "x86_64");
    assert_eq!(m.export_mode, "live");
    assert_eq!(m.cpus, Some(2));
    assert_eq!(m.memory_gb, Some(4));
    assert_eq!(m.disk_gb, Some(50));
    assert_eq!(m.disk_sha256.as_deref(), Some("deadbeef"));
}

#[test]
fn meta_tolerates_missing_fields() {
    let raw = r#"
name = "stub"
arch = "x86_64"
"#;
    let m = Meta::parse(raw);
    assert_eq!(m.name, "stub");
    assert_eq!(m.cpus, None);
    assert!(m.disk_sha256.is_none());
}

#[test]
fn meta_ignores_comments_and_blank_lines() {
    let raw = "\
# a comment
\n\
name = \"x\"
# memory_gb = 999
cpus = 4
";
    let m = Meta::parse(raw);
    assert_eq!(m.name, "x");
    assert_eq!(m.cpus, Some(4));
    assert_eq!(m.memory_gb, None);
}
