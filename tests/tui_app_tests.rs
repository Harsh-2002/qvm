//! Pure-logic tests for the TUI app state machine. No TTY, no virsh.

use qvm::tui::{App, Mode};

#[test]
fn fresh_app_starts_in_table_mode() {
    let app = App::new();
    assert_eq!(app.mode, Mode::Table);
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
