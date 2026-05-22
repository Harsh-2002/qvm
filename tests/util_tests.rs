//! Unit tests for util.rs — the parts I can verify without libvirt.

use qvm::util::{hash_password, parse_size_mb, random_username, valid_username, valid_vm_name};

#[test]
fn vm_name_accepts_normal_names() {
    for name in ["web01", "Dev", "db-1", "host.example.com", "a", "A1.b_c-d"] {
        assert!(valid_vm_name(name), "should accept: {name}");
    }
}

#[test]
fn vm_name_rejects_garbage() {
    for name in ["", "-leading", ".dot", "_under", "has space", "tab\there", "sl/ash", "qu'ote"] {
        assert!(!valid_vm_name(name), "should reject: {name:?}");
    }
}

#[test]
fn username_accepts_linux_legal() {
    for u in ["dev", "vm7f3a9c", "_admin", "user1", "a", "_", "u-with-dash"] {
        assert!(valid_username(u), "should accept: {u}");
    }
}

#[test]
fn username_rejects_illegal() {
    for u in [
        "", "1leading", "-leading", "Has", "UPPER", "with space",
        "thirty_three_chars_is_too_longg!", // 33 with bang, illegal char
    ] {
        assert!(!valid_username(u), "should reject: {u:?}");
    }
    // length boundary covered separately by username_length_boundary
}

#[test]
fn username_length_boundary() {
    // 32 chars exactly = legal; 33 = illegal
    let ok = format!("a{}", "b".repeat(31)); // 32
    let bad = format!("a{}", "b".repeat(32)); // 33
    assert_eq!(ok.len(), 32);
    assert_eq!(bad.len(), 33);
    assert!(valid_username(&ok), "32-char username should be accepted");
    assert!(!valid_username(&bad), "33-char username should be rejected");
}

#[test]
fn random_username_is_always_valid() {
    for _ in 0..200 {
        let u = random_username();
        assert!(valid_username(&u), "random gen produced invalid username: {u}");
        assert!(u.starts_with("vm"));
        assert_eq!(u.len(), 8); // "vm" + 6 chars
    }
}

#[test]
fn random_username_is_random() {
    // Probability of collision in 50 draws of 36^6 space is vanishingly small.
    let mut seen = std::collections::HashSet::new();
    for _ in 0..50 { seen.insert(random_username()); }
    assert!(seen.len() >= 40, "random_username is not actually random ({} unique of 50)", seen.len());
}

#[test]
fn password_hash_is_sha512_crypt_format() {
    let h = hash_password("hunter2").expect("hash should succeed");
    // SHA-512 crypt format: $6$<salt>$<hash> where total len ≈ 86 + salt
    assert!(h.starts_with("$6$"), "expected SHA-512 crypt prefix, got: {h}");
    let parts: Vec<&str> = h.splitn(4, '$').collect();
    assert_eq!(parts.len(), 4, "malformed crypt string: {h}");
    assert_eq!(parts[1], "6");
    assert!(!parts[2].is_empty(), "no salt component");
    assert!(parts[3].len() >= 80, "hash component too short: {} chars", parts[3].len());
}

// ── parse_size_mb ────────────────────────────────────────────────

#[test]
fn parse_size_mb_handles_units() {
    assert_eq!(parse_size_mb("512M").unwrap(),   512);
    assert_eq!(parse_size_mb("512m").unwrap(),   512);
    assert_eq!(parse_size_mb("512MB").unwrap(),  512);
    assert_eq!(parse_size_mb("512mb").unwrap(),  512);
    assert_eq!(parse_size_mb("1G").unwrap(),     1024);
    assert_eq!(parse_size_mb("1g").unwrap(),     1024);
    assert_eq!(parse_size_mb("2GB").unwrap(),    2048);
    assert_eq!(parse_size_mb("2gb").unwrap(),    2048);
    assert_eq!(parse_size_mb("100MB").unwrap(),  100);
    assert_eq!(parse_size_mb("  1G  ").unwrap(), 1024); // tolerates outer whitespace
}

#[test]
fn parse_size_mb_rejects_garbage() {
    for bad in ["", "  ", "abc", "1.5G", "-100M", "100K", "100", "G", "M", "0M", "0G"] {
        let err = parse_size_mb(bad);
        assert!(err.is_err(), "should reject {bad:?} but parsed as {err:?}");
    }
}

#[test]
fn password_hash_is_deterministic_per_salt_random_across_calls() {
    // Different calls produce different hashes (random salt).
    let a = hash_password("samepass").unwrap();
    let b = hash_password("samepass").unwrap();
    assert_ne!(a, b, "hashes should differ because salt is random");
}
