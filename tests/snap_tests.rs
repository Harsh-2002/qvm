use qvm::commands::snap::parse_snapshot_list;

#[test]
fn parses_virsh_snapshot_list_output() {
    let raw = "\
 Name             Creation Time               State
-----------------------------------------------------
 before-upgrade   2026-05-21 10:00:00 +0000   shutoff
 after-upgrade    2026-05-21 10:30:00 +0000   running
";
    let v = parse_snapshot_list(raw);
    assert_eq!(v, vec!["before-upgrade", "after-upgrade"]);
}

#[test]
fn returns_empty_when_no_snapshots() {
    let raw = "\
 Name   Creation Time   State
----------------------------
";
    let v = parse_snapshot_list(raw);
    assert!(v.is_empty());
}

#[test]
fn handles_leading_blank_lines() {
    let raw = "\n\n Name   Creation Time   State\n-----\n one  2026-05-21  running\n";
    let v = parse_snapshot_list(raw);
    assert_eq!(v, vec!["one"]);
}
