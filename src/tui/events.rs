//! Map crossterm key events → `Action`s. Pure function; no state mutation.
//!
//! The right pane's mode (`Detail` vs `CreateForm` vs `Filter` vs
//! `ConfirmDelete` vs `Help` vs `EmptyState`) decides which key map runs.
//! Sidebar navigation (↑/↓) and quit (q/Ctrl-C) are global across modes.

use crate::tui::app::{App, Mode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Noop,
    Quit,

    // Sidebar navigation (always available except in text-entry modes).
    Up,
    Down,

    // Detail-pane scrolling.
    ScrollDetailUp,
    ScrollDetailDown,

    // Focus / view toggles.
    CycleFocus,
    ToggleRaw, // switch between structured detail and raw `virsh dominfo`

    // Mode transitions.
    OpenCreate,
    OpenResize,
    OpenDelete,
    OpenHelp,
    OpenFilter,
    CloseToDetail,

    // Sort cycle.
    CycleSort,

    // Lifecycle.
    Start,
    Stop,
    Restart,

    // Console (handled in tui/mod.rs — suspend + exec virsh console).
    Console,
    // Browser VNC (handled in tui/mod.rs — suspend + websockify).
    Browser,

    // VNC info (shown as a toast).
    ShowVnc,

    // Delete confirm.
    ConfirmDelete,

    // Create form.
    CreateNext,
    CreatePrev,
    CreateLeft,
    CreateRight,
    CreateHome,
    CreateEnd,
    CreateInsert(char),
    CreateBackspace,
    CreateDelete,
    /// Handled in tui/mod.rs (suspend + run create).
    SubmitCreate,

    // Resize form.
    ResizeNext,
    ResizePrev,
    ResizeLeft,
    ResizeRight,
    ResizeHome,
    ResizeEnd,
    ResizeInsert(char),
    ResizeBackspace,
    ResizeDelete,
    /// Handled in tui/mod.rs (suspend if disk grow, else inline).
    SubmitResize,

    // Snapshots view.
    OpenSnapshots,
    SnapshotsUp,
    SnapshotsDown,
    /// Create a new snapshot (timestamped name). Handled in tui/mod.rs.
    SnapshotsNew,
    /// Enter confirm-revert state for the selected snapshot.
    SnapshotsRevertConfirm,
    /// Enter confirm-delete state for the selected snapshot.
    SnapshotsDeleteConfirm,
    /// Execute the pending revert or delete. Handled in tui/mod.rs.
    SnapshotsConfirm,
    /// Cancel the pending revert/delete confirm (back to plain Snapshots).
    SnapshotsCancel,

    // Filter.
    FilterInsert(char),
    FilterBackspace,
    FilterDelete,
    FilterLeft,
    FilterRight,
    FilterCommit,
    FilterCancel,
}

pub fn map_key(app: &App, k: KeyEvent) -> Action {
    // Ctrl-C always quits.
    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
        return Action::Quit;
    }
    match &app.mode {
        Mode::Detail        => key_in_detail(k),
        Mode::EmptyState    => key_in_empty(k),
        Mode::CreateForm    => key_in_create(k),
        Mode::ResizeForm    => key_in_resize(k),
        Mode::ConfirmDelete => key_in_confirm(k),
        Mode::Help          => key_in_help(k),
        Mode::Filter        => key_in_filter(k),
        Mode::Snapshots     => key_in_snapshots(app, k),
    }
}

/// Default — VM list focus, no text entry. All the main hotkeys live here.
fn key_in_detail(k: KeyEvent) -> Action {
    match k.code {
        KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
        KeyCode::Tab                       => Action::CycleFocus,
        KeyCode::Char('j') | KeyCode::Down => Action::Down,
        KeyCode::Char('k') | KeyCode::Up   => Action::Up,
        KeyCode::PageDown                  => Action::ScrollDetailDown,
        KeyCode::PageUp                    => Action::ScrollDetailUp,
        KeyCode::Char('c') => Action::OpenCreate,
        KeyCode::Char('m') => Action::OpenResize,
        KeyCode::Char('d') => Action::OpenDelete,
        KeyCode::Char('v') => Action::ShowVnc,
        KeyCode::Char('e') => Action::Console,
        KeyCode::Char('b') => Action::Browser,
        KeyCode::Char('s') => Action::Start,
        KeyCode::Char('t') => Action::Stop,
        KeyCode::Char('r') => Action::Restart,
        KeyCode::Char('/') => Action::OpenFilter,
        KeyCode::Char('o') => Action::CycleSort,
        KeyCode::Char('?') => Action::OpenHelp,
        KeyCode::Char('R') => Action::ToggleRaw,
        KeyCode::Char('p') => Action::OpenSnapshots,
        _ => Action::Noop,
    }
}

/// Empty state: only useful keys are create / help / quit.
fn key_in_empty(k: KeyEvent) -> Action {
    match k.code {
        KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
        KeyCode::Char('c') => Action::OpenCreate,
        KeyCode::Char('?') => Action::OpenHelp,
        KeyCode::Tab       => Action::CycleFocus,
        _ => Action::Noop,
    }
}

fn key_in_create(k: KeyEvent) -> Action {
    match k.code {
        KeyCode::Esc       => Action::CloseToDetail,
        KeyCode::Enter     => Action::SubmitCreate,
        KeyCode::Tab       => Action::CreateNext,
        KeyCode::BackTab   => Action::CreatePrev,
        KeyCode::Left      => Action::CreateLeft,
        KeyCode::Right     => Action::CreateRight,
        KeyCode::Home      => Action::CreateHome,
        KeyCode::End       => Action::CreateEnd,
        KeyCode::Backspace => Action::CreateBackspace,
        KeyCode::Delete    => Action::CreateDelete,
        KeyCode::Char(c)   => Action::CreateInsert(c),
        _ => Action::Noop,
    }
}

fn key_in_resize(k: KeyEvent) -> Action {
    match k.code {
        KeyCode::Esc       => Action::CloseToDetail,
        KeyCode::Enter     => Action::SubmitResize,
        KeyCode::Tab       => Action::ResizeNext,
        KeyCode::BackTab   => Action::ResizePrev,
        KeyCode::Left      => Action::ResizeLeft,
        KeyCode::Right     => Action::ResizeRight,
        KeyCode::Home      => Action::ResizeHome,
        KeyCode::End       => Action::ResizeEnd,
        KeyCode::Backspace => Action::ResizeBackspace,
        KeyCode::Delete    => Action::ResizeDelete,
        KeyCode::Char(c)   => Action::ResizeInsert(c),
        _ => Action::Noop,
    }
}

fn key_in_confirm(k: KeyEvent) -> Action {
    match k.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => Action::ConfirmDelete,
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Action::CloseToDetail,
        _ => Action::Noop,
    }
}

fn key_in_help(k: KeyEvent) -> Action {
    match k.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') | KeyCode::Enter => Action::CloseToDetail,
        _ => Action::Noop,
    }
}

/// Snapshot list keymap. When a confirm is pending (revert/delete), y/n
/// drive the confirm; otherwise the letter keys trigger new actions.
fn key_in_snapshots(app: &App, k: KeyEvent) -> Action {
    let in_confirm = app.snapshots.confirm.is_some();
    if in_confirm {
        return match k.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Action::SnapshotsConfirm,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Action::SnapshotsCancel,
            _ => Action::Noop,
        };
    }
    match k.code {
        KeyCode::Esc | KeyCode::Char('q')   => Action::CloseToDetail,
        KeyCode::Tab                        => Action::CycleFocus,
        KeyCode::Down | KeyCode::Char('j')  => Action::SnapshotsDown,
        KeyCode::Up   | KeyCode::Char('k')  => Action::SnapshotsUp,
        KeyCode::Char('n')                  => Action::SnapshotsNew,
        KeyCode::Char('r')                  => Action::SnapshotsRevertConfirm,
        KeyCode::Char('d')                  => Action::SnapshotsDeleteConfirm,
        _ => Action::Noop,
    }
}

fn key_in_filter(k: KeyEvent) -> Action {
    match k.code {
        KeyCode::Esc       => Action::FilterCancel,
        KeyCode::Enter     => Action::FilterCommit,
        KeyCode::Left      => Action::FilterLeft,
        KeyCode::Right     => Action::FilterRight,
        KeyCode::Backspace => Action::FilterBackspace,
        KeyCode::Delete    => Action::FilterDelete,
        // While typing in filter, ↑/↓ still navigate the list (matches the
        // user's expectation when they're refining a search).
        KeyCode::Down      => Action::Down,
        KeyCode::Up        => Action::Up,
        KeyCode::Char(c)   => Action::FilterInsert(c),
        _ => Action::Noop,
    }
}
