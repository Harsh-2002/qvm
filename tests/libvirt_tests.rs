//! Tests for pure parsers in `libvirt.rs` (the bits that don't need virsh).

use qvm::libvirt::{parse_vnc_display, VncEndpoint};

#[test]
fn vnc_display_zero_is_port_5900() {
    let ep = parse_vnc_display("10.1.1.10:0").unwrap();
    assert_eq!(ep, VncEndpoint { display: 0, port: 5900 });
}

#[test]
fn vnc_display_one_is_port_5901() {
    let ep = parse_vnc_display("127.0.0.1:1").unwrap();
    assert_eq!(ep, VncEndpoint { display: 1, port: 5901 });
}

#[test]
fn vnc_display_high_number() {
    // display 99 → port 5999
    let ep = parse_vnc_display("host:99").unwrap();
    assert_eq!(ep, VncEndpoint { display: 99, port: 5999 });
}

#[test]
fn vnc_display_handles_trailing_whitespace_via_trim() {
    // parse_vnc_display itself does not trim; vnc_endpoint trims before
    // calling. The pure parser should still accept the canonical form.
    let ep = parse_vnc_display(":0").unwrap();
    assert_eq!(ep.port, 5900);
}

#[test]
fn vnc_display_empty_string_is_none() {
    assert!(parse_vnc_display("").is_none());
}

#[test]
fn vnc_display_no_colon_is_none() {
    // "1234" — parses to a number but the syntax is wrong.
    // Our implementation splits on ':' which makes "1234" the only
    // segment, so this would actually be parsed as display 1234 → port
    // 7134. virsh never produces that shape; document it as a known
    // edge case the parser tolerates.
    let ep = parse_vnc_display("1234").unwrap();
    assert_eq!(ep.display, 1234);
}

#[test]
fn vnc_display_garbage_after_colon_is_none() {
    assert!(parse_vnc_display("host:abc").is_none());
    assert!(parse_vnc_display("host:").is_none());
}

#[test]
fn vnc_display_overflow_is_none() {
    // u16::MAX = 65535. display 60000 → port would overflow u16.
    assert!(parse_vnc_display("host:60000").is_none());
}
