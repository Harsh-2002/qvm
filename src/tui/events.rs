//! Map crossterm key events → `Action`s. Pure function; no state mutation.

use crate::tui::app::{App, Mode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Every user-initiated thing the TUI can do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Noop,
    Quit,

    // Table mode.
    Up,
    Down,
    OpenCreate,
    OpenDelete,
    OpenVnc,
    OpenInspect,
    OpenHelp,
    OpenFilter,
    CycleSort,
    Start,
    Stop,
    Restart,
    Console, // handled in tui/mod.rs (suspend-exec)

    // Modal close / generic Esc.
    CloseModal,

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
    SubmitCreate, // handled in tui/mod.rs (suspend-exec)

    // Filter mode.
    FilterInsert(char),
    FilterBackspace,
    FilterDelete,
    FilterLeft,
    FilterRight,
    FilterCommit,
    FilterCancel,

    // Inspect popup scroll.
    InspectScroll(i32),
}

pub fn map_key(app: &App, k: KeyEvent) -> Action {
    // Global: Ctrl-C always quits, regardless of mode.
    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
        return Action::Quit;
    }

    match &app.mode {
        Mode::Table         => table_key(k),
        Mode::CreateForm    => create_key(k),
        Mode::DeleteConfirm => confirm_key(k),
        Mode::Inspect { .. } => inspect_key(k),
        Mode::Vnc { .. }    => any_to_close(k),
        Mode::Help          => any_to_close(k),
        Mode::Filter        => filter_key(k),
    }
}

fn table_key(k: KeyEvent) -> Action {
    match k.code {
        KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
        KeyCode::Char('j') | KeyCode::Down => Action::Down,
        KeyCode::Char('k') | KeyCode::Up   => Action::Up,
        KeyCode::Char('c') => Action::OpenCreate,
        KeyCode::Char('d') => Action::OpenDelete,
        KeyCode::Char('v') => Action::OpenVnc,
        KeyCode::Char('e') => Action::Console,
        KeyCode::Char('i') => Action::OpenInspect,
        KeyCode::Char('?') => Action::OpenHelp,
        KeyCode::Char('/') => Action::OpenFilter,
        KeyCode::Char('o') => Action::CycleSort,
        KeyCode::Char('s') => Action::Start,
        KeyCode::Char('t') => Action::Stop,
        KeyCode::Char('r') => Action::Restart,
        _ => Action::Noop,
    }
}

fn create_key(k: KeyEvent) -> Action {
    match k.code {
        KeyCode::Esc       => Action::CloseModal,
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

fn confirm_key(k: KeyEvent) -> Action {
    match k.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => Action::ConfirmDelete,
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Action::CloseModal,
        _ => Action::Noop,
    }
}

fn inspect_key(k: KeyEvent) -> Action {
    match k.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('i') => Action::CloseModal,
        KeyCode::Up        => Action::InspectScroll(-1),
        KeyCode::Down      => Action::InspectScroll(1),
        KeyCode::PageUp    => Action::InspectScroll(-10),
        KeyCode::PageDown  => Action::InspectScroll(10),
        KeyCode::Home      => Action::InspectScroll(i32::MIN),
        KeyCode::End       => Action::InspectScroll(i32::MAX),
        _ => Action::Noop,
    }
}

fn any_to_close(k: KeyEvent) -> Action {
    match k.code { KeyCode::Char(_) | KeyCode::Esc | KeyCode::Enter => Action::CloseModal, _ => Action::Noop }
}

fn filter_key(k: KeyEvent) -> Action {
    match k.code {
        KeyCode::Esc       => Action::FilterCancel,
        KeyCode::Enter     => Action::FilterCommit,
        KeyCode::Left      => Action::FilterLeft,
        KeyCode::Right     => Action::FilterRight,
        KeyCode::Backspace => Action::FilterBackspace,
        KeyCode::Delete    => Action::FilterDelete,
        KeyCode::Char(c)   => Action::FilterInsert(c),
        _ => Action::Noop,
    }
}
