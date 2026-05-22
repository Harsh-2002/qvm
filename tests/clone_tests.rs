//! Parser helpers used by `qvm clone` to recover source metadata.

use qvm::commands::clone::{extract_osinfo, extract_passwd_hash, extract_shell};

#[test]
fn passwd_hash_finds_quoted_hash() {
    let ud = "users:\n  - name: foo\n    passwd: \"$6$xx$abc\"\n";
    assert_eq!(extract_passwd_hash(ud).as_deref(), Some("$6$xx$abc"));
}

#[test]
fn passwd_hash_unquoted_works_too() {
    let ud = "    passwd: $6$xx$abc\n";
    assert_eq!(extract_passwd_hash(ud).as_deref(), Some("$6$xx$abc"));
}

#[test]
fn passwd_hash_returns_none_when_missing() {
    let ud = "users: []\n";
    assert!(extract_passwd_hash(ud).is_none());
}

#[test]
fn shell_picks_first_match() {
    let ud = "    shell: /bin/sh\n    shell: /bin/bash\n";
    assert_eq!(extract_shell(ud).as_deref(), Some("/bin/sh"));
}

#[test]
fn shell_none_when_absent() {
    assert!(extract_shell("users: []\n").is_none());
}

#[test]
fn osinfo_parses_libosinfo_id() {
    let xml = r#"<metadata>
  <libosinfo:libosinfo xmlns:libosinfo="...">
    <libosinfo:os id="http://debian.org/debian/13"/>
  </libosinfo:libosinfo>
</metadata>"#;
    assert_eq!(extract_osinfo(xml).as_deref(), Some("debian13"));
}

#[test]
fn osinfo_handles_fedora_uri() {
    let xml = r#"<libosinfo:os id="http://fedoraproject.org/fedora/41"/>"#;
    assert_eq!(extract_osinfo(xml).as_deref(), Some("fedora41"));
}

#[test]
fn osinfo_none_when_missing() {
    assert!(extract_osinfo("<domain/>").is_none());
}
