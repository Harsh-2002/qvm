//! Pure-logic tests for the TUI app state machine. No TTY, no virsh.

use qvm::tui::{App, Mode};

#[test]
fn fresh_app_starts_in_empty_state() {
    // With no rows yet, the app's default Mode is EmptyState — the friendly
    // "no VMs, press c to create" pane. Once refresh() loads rows, Detail
    // takes over.
    let app = App::new();
    assert_eq!(app.mode, Mode::EmptyState);
    assert_eq!(app.selected, 0);
    assert!(app.rows.is_empty());
    assert!(!app.should_quit);
    assert!(app.filter.is_empty());
}

#[test]
fn tick_due_initially_true_then_false_after_refresh_stamp() {
    let mut app = App::new();
    assert!(app.tick_due(), "first tick should always be due");
    // Mark "just refreshed" — we can't call refresh() without virsh, so emulate.
    app.last_refresh = Some(std::time::Instant::now());
    assert!(!app.tick_due(), "tick should not be due immediately after refresh");
}

#[test]
fn toast_ok_and_err_distinguish() {
    use qvm::tui::app::Toast;
    let mut app = App::new();
    app.toast_ok("ok message".into());
    assert!(matches!(app.current_toast(), Some(Toast::Ok(_))));
    app.toast_err("err message".into());
    assert!(matches!(app.current_toast(), Some(Toast::Err(_))));
}

#[test]
fn snapshots_view_starts_empty() {
    let app = App::new();
    assert!(app.snapshots.vm_name.is_empty());
    assert!(app.snapshots.snaps.is_empty());
    assert_eq!(app.snapshots.selected, 0);
    assert!(app.snapshots.confirm.is_none());
}

#[test]
fn snapshots_selected_snap_returns_none_when_list_empty() {
    let app = App::new();
    assert!(app.snapshots.selected_snap().is_none());
}
