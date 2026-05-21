//! Pure render functions. No state mutation, no I/O.
//!
//! Layout (≥80 cols): header (1 line) · split-pane (sidebar + content) ·
//! status bar (2 lines). On narrower terminals we collapse to a single
//! pane and just show the sidebar list with a status bar below.

use crate::tui::app::{App, CreateForm, Mode, Sort, Toast, VmRow};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph, Row, Table, Wrap},
    Frame,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),   // header
            Constraint::Min(3),      // body (split-pane)
            Constraint::Length(2),   // status bar (hints + toast)
        ])
        .split(area);

    draw_header(f, layout[0], app);

    if layout[1].width < 80 {
        // Very narrow terminals — single-pane fallback.
        draw_narrow_body(f, layout[1], app);
    } else {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(26), Constraint::Min(40)])
            .split(layout[1]);
        draw_sidebar(f, body[0], app);
        draw_content(f, body[1], app);
    }

    draw_status_bar(f, layout[2], app);
}

// ── header ────────────────────────────────────────────────────────────────────

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let running = app.rows.iter().filter(|r| r.state == "running").count();
    let total   = app.rows.len();
    let summary = if app.filter.is_empty() {
        format!(" {total} VMs · {running} running ")
    } else {
        let v = app.visible_count();
        format!(" {v}/{total} VMs · filter: {} ", app.filter)
    };

    let cols = area.width as usize;
    let left  = format!(" qvm {VERSION} ");
    let mid   = format!(" {} ", app.host_label);
    let right = summary;
    // pad middle so total fits cols
    let pad = cols.saturating_sub(left.len() + mid.len() + right.len());
    let bar = format!("{left}{mid}{:pad$}{right}", "", pad = pad);

    let style = Style::default().bg(Color::DarkGray).fg(Color::White)
        .add_modifier(Modifier::BOLD);
    f.render_widget(Paragraph::new(bar).style(style), area);
}

// ── sidebar ───────────────────────────────────────────────────────────────────

fn draw_sidebar(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_type(BorderType::Plain)
        .title(Span::styled(" VIRTUAL MACHINES ",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut y = inner.y;

    // Filter input line (only visible when filtering or filter is set).
    if matches!(app.mode, Mode::Filter) || !app.filter.is_empty() {
        let active = matches!(app.mode, Mode::Filter);
        let prefix = if active { "/" } else { " filter:" };
        let value = if active { &app.filter_input.value } else { &app.filter };
        let style = if active {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let r = Rect { x: inner.x, y, width: inner.width.saturating_sub(1), height: 1 };
        f.render_widget(Paragraph::new(format!("{prefix} {value}")).style(style), r);
        if active {
            // Cursor position inside the filter input.
            let cx = inner.x + (prefix.len() + 1) as u16 + app.filter_input.cursor as u16;
            f.set_cursor_position((cx, y));
        }
        y = y.saturating_add(1);
    }

    let avail_h = inner.height.saturating_sub(y - inner.y + 1) as usize; // reserve 1 for [+] new
    let rows = app.visible();
    let items: Vec<ListItem> = rows.iter().enumerate().take(avail_h).map(|(i, r)| {
        let selected = i == app.selected;
        let dot = state_dot(&r.state);
        let prefix = if selected { "› " } else { "  " };
        let name_style = if selected {
            Style::default().fg(Color::White).bg(Color::Blue)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        ListItem::new(Line::from(vec![
            Span::styled(prefix, name_style),
            Span::raw(dot),
            Span::raw(" "),
            Span::styled(r.name.clone(), name_style),
        ]))
    }).collect();

    let list_area = Rect {
        x: inner.x,
        y,
        width: inner.width.saturating_sub(1),
        height: avail_h as u16,
    };
    f.render_widget(List::new(items), list_area);

    // [+] new at the bottom of the sidebar.
    let new_y = inner.y + inner.height.saturating_sub(1);
    let new_style = if matches!(app.mode, Mode::CreateForm) {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    f.render_widget(
        Paragraph::new(" [+] create  (c) ").style(new_style),
        Rect { x: inner.x, y: new_y, width: inner.width.saturating_sub(1), height: 1 },
    );
}

fn state_dot(state: &str) -> &'static str {
    match state {
        "running" => "●",
        "paused"  => "◐",
        "crashed" => "✗",
        _         => "○",
    }
}

fn state_color(state: &str) -> Color {
    match state {
        "running" => Color::Green,
        "paused"  => Color::Yellow,
        "crashed" => Color::Red,
        _         => Color::DarkGray,
    }
}

// ── content pane ──────────────────────────────────────────────────────────────

fn draw_content(f: &mut Frame, area: Rect, app: &App) {
    match &app.mode {
        Mode::Detail | Mode::ConfirmDelete | Mode::Filter => draw_detail(f, area, app),
        Mode::CreateForm => draw_create(f, area, app),
        Mode::Help       => draw_help(f, area),
        Mode::EmptyState => draw_empty(f, area),
    }
}

fn draw_detail(f: &mut Frame, area: Rect, app: &App) {
    let Some(row) = app.selected_row() else {
        draw_empty(f, area);
        return;
    };
    let inner_area = area.inner(ratatui::layout::Margin { horizontal: 2, vertical: 1 });

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // title
            Constraint::Length(1),  // rule
            Constraint::Length(11), // metadata table
            Constraint::Min(2),     // dominfo scroll
        ])
        .split(inner_area);

    // Title
    f.render_widget(
        Paragraph::new(Span::styled(row.name.clone(),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD))),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(Span::styled("─".repeat(area.width as usize / 2),
            Style::default().fg(Color::DarkGray))),
        chunks[1],
    );

    // Metadata KV table.
    let metadata = build_metadata(row);
    let kv_rows: Vec<Row> = metadata.into_iter().map(|(k, v, vc)| {
        Row::new(vec![
            Span::styled(format!("  {k:<10}"), Style::default().fg(Color::DarkGray)),
            v,
            Span::raw(""),
        ]).style(Style::default().fg(vc))
    }).collect();
    let kv = Table::new(kv_rows, [Constraint::Length(14), Constraint::Min(20), Constraint::Length(0)]);
    f.render_widget(kv, chunks[2]);

    // dominfo scrollable region.
    let raw = row.dominfo.clone().unwrap_or_else(|| "  (loading dominfo …)".into());
    f.render_widget(
        Paragraph::new(raw)
            .style(Style::default().fg(Color::DarkGray))
            .scroll((app.detail_scroll, 0))
            .wrap(Wrap { trim: false }),
        chunks[3],
    );
}

fn build_metadata(row: &VmRow) -> Vec<(&'static str, Span<'static>, Color)> {
    vec![
        ("Status", Span::styled(
            format!("{} {}", state_dot(&row.state), row.state),
            Style::default().fg(state_color(&row.state)).add_modifier(Modifier::BOLD)),
            Color::Reset),
        ("IP", Span::styled(
            row.ip.clone().unwrap_or_else(|| "—".into()),
            if row.ip.is_some() { Style::default().fg(Color::White) }
            else { Style::default().fg(Color::DarkGray) }), Color::Reset),
        ("Name",      Span::raw(row.name.clone()), Color::Gray),
    ]
}

fn draw_create(f: &mut Frame, area: Rect, app: &App) {
    let inner = area.inner(ratatui::layout::Margin { horizontal: 2, vertical: 1 });

    f.render_widget(
        Paragraph::new(Span::styled("Create VM",
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD))),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );
    f.render_widget(
        Paragraph::new(Span::styled("Tab/Shift-Tab navigate · ←/→ cycle distro · Enter create · Esc cancel",
            Style::default().fg(Color::DarkGray))),
        Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: 1 },
    );

    let labels = ["Name", "Distro", "vCPUs", "RAM (GB)", "Disk (GB)", "User"];
    let items: Vec<ListItem> = labels.iter().enumerate().map(|(i, label)| {
        let focused = i == app.create.field;
        let bullet = if focused { "▸ " } else { "  " };
        let value = field_value(&app.create, i);
        let label_style = if focused {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        ListItem::new(Line::from(vec![
            Span::styled(format!("{bullet}{label:<10}"), label_style),
            Span::raw(value),
        ]))
    }).collect();

    f.render_widget(List::new(items),
        Rect { x: inner.x, y: inner.y + 3, width: inner.width, height: 6 });

    // Cursor placement on the focused text field.
    if let Some(off) = focused_cursor_offset(&app.create) {
        // 2 ("▸ ") + 10 (label width) = 12 chars before value
        let cx = inner.x + 12 + off as u16;
        let cy = inner.y + 3 + app.create.field as u16;
        f.set_cursor_position((cx, cy));
    }
}

fn field_value(c: &CreateForm, i: usize) -> String {
    match i {
        0 => c.name.value.clone(),
        1 => match c.distros.get(c.distro_idx) {
            Some(name) => {
                let pulled = c.distro_pulled.get(c.distro_idx).copied().unwrap_or(false);
                let badge = if pulled { "● pulled" } else { "○ not pulled (auto-download)" };
                format!("{name}  ({}/{})   {badge}",
                    c.distro_idx + 1, c.distros.len())
            }
            None => String::new(),
        },
        2 => c.cpus.value.clone(),
        3 => c.memory_gb.value.clone(),
        4 => c.disk_gb.value.clone(),
        5 => {
            let v = &c.user.value;
            if v.trim().is_empty() { "(auto: vmXXXXXX)".into() } else { v.clone() }
        }
        _ => String::new(),
    }
}

fn focused_cursor_offset(c: &CreateForm) -> Option<usize> {
    match c.field {
        0 => Some(c.name.cursor),
        1 => None,
        2 => Some(c.cpus.cursor),
        3 => Some(c.memory_gb.cursor),
        4 => Some(c.disk_gb.cursor),
        5 => if c.user.value.trim().is_empty() { None } else { Some(c.user.cursor) },
        _ => None,
    }
}

fn draw_help(f: &mut Frame, area: Rect) {
    let inner = area.inner(ratatui::layout::Margin { horizontal: 2, vertical: 1 });
    let body = "\
qvm — keyboard reference

NAVIGATION
  ↑ ↓ / k j        move selection in the sidebar
  PgUp PgDn        scroll the detail pane
  /                filter VMs by name (Enter applies, Esc cancels)
  o                cycle sort (name / state / ip)

VM ACTIONS
  c                create new VM (form inline)
  s · t · r        start · stop · restart selected
  d                delete selected (y/n confirm in status bar)
  v                show VNC connect info as a toast
  e                attach the guest serial console (Ctrl-] to exit)

GENERAL
  ?                this screen
  q · Esc          back · quit
  Ctrl-C           force quit
";
    f.render_widget(
        Paragraph::new(body).wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::Gray)),
        inner,
    );
}

fn draw_empty(f: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled("No virtual machines yet.",
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)))
            .alignment(Alignment::Center),
        Line::from(""),
        Line::from(vec![
            Span::styled("Press ", Style::default().fg(Color::DarkGray)),
            Span::styled("c", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(" to create your first VM.", Style::default().fg(Color::DarkGray)),
        ]).alignment(Alignment::Center),
        Line::from(vec![
            Span::styled("Press ", Style::default().fg(Color::DarkGray)),
            Span::styled("?", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(" for help · ", Style::default().fg(Color::DarkGray)),
            Span::styled("q", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(" to quit.", Style::default().fg(Color::DarkGray)),
        ]).alignment(Alignment::Center),
    ];
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        area.inner(ratatui::layout::Margin { horizontal: 2, vertical: 2 }),
    );
}

fn draw_narrow_body(f: &mut Frame, area: Rect, app: &App) {
    // Just show the sidebar at full width for very narrow terminals.
    draw_sidebar(f, area, app);
}

// ── status bar ────────────────────────────────────────────────────────────────

fn draw_status_bar(f: &mut Frame, area: Rect, app: &App) {
    let lines = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);

    // Line 1: contextual confirm OR key hints.
    if let Mode::ConfirmDelete = app.mode {
        let name = app.selected_name().unwrap_or_default();
        let line = Line::from(vec![
            Span::styled(format!(" Delete '{name}' and all its data? "),
                Style::default().fg(Color::White).bg(Color::Red)
                    .add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled("[y]es", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled("[n]o", Style::default().fg(Color::Gray)),
        ]);
        f.render_widget(Paragraph::new(line), lines[0]);
    } else {
        f.render_widget(
            Paragraph::new(hint_text(&app.mode))
                .style(Style::default().fg(Color::Gray)),
            lines[0],
        );
    }

    // Line 2: refresh ticker + toast.
    let refreshed = app.last_refresh
        .map(|t| format!("refreshed {}s ago · sort {}", t.elapsed().as_secs(), app.sort.label()))
        .unwrap_or_else(|| "—".into());
    let left = Span::styled(refreshed, Style::default().fg(Color::DarkGray));
    let mut spans: Vec<Span> = vec![left, Span::raw("  ")];
    if let Some(t) = app.current_toast() {
        let (text, style) = match t {
            Toast::Ok(m)  => (m.clone(), Style::default().fg(Color::Green)),
            Toast::Err(m) => (m.clone(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        };
        spans.push(Span::styled(text, style));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), lines[1]);
}

fn hint_text(mode: &Mode) -> String {
    match mode {
        Mode::Detail | Mode::EmptyState | Mode::Filter => {
            " ↑↓ select · s start · t stop · r restart · e console · d delete · c create · v vnc · / filter · o sort · ? help · q quit".to_string()
        }
        Mode::CreateForm  => " Tab/Shift-Tab move · ←/→ cycle distro · Enter create · Esc cancel".to_string(),
        Mode::ConfirmDelete => String::new(), // line 1 handled above
        Mode::Help        => " any key to dismiss".to_string(),
    }
}

// Keep public so doc-tests / external callers can import Sort if needed.
#[allow(dead_code)]
fn _sort_export(_: Sort) {}
