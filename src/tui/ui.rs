//! Pure render functions. No state mutation, no I/O.

use crate::tui::app::{App, Mode, Sort, Toast, VmRow};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Row, Table, Wrap},
    Frame,
};

const TITLE: &str = concat!(" qvm ", env!("CARGO_PKG_VERSION"), " ");
const HOTKEYS_TABLE: &str =
    "[c]reate  [s]tart  [t]op  [r]estart  [d]elete  [v]nc  [e]console  [i]nspect  \
     [/]filter  [o]sort  [?]help  [q]uit";

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                 // header
            Constraint::Min(5),                    // table
            Constraint::Length(if app.current_toast().is_some() { 1 } else { 0 }),
            Constraint::Length(2),                 // key bar
        ])
        .split(area);

    draw_header(f, layout[0], app);
    draw_body(f, layout[1], app);
    if app.current_toast().is_some() {
        draw_toast(f, layout[2], app);
    }
    draw_keybar(f, layout[3], app);

    // Modal overlays.
    match &app.mode {
        Mode::CreateForm    => draw_create_modal(f, area, app),
        Mode::DeleteConfirm => draw_confirm(f, area, app),
        Mode::Inspect { content, scroll } => draw_popup(f, area, "Inspect", content, Some(*scroll)),
        Mode::Vnc { content } => draw_popup(f, area, "VNC", content, None),
        Mode::Help          => draw_help(f, area),
        Mode::Filter        => draw_filter(f, area, app),
        Mode::Table         => {}
    }
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let visible = app.visible_count();
    let total   = app.rows.len();
    let running = app.rows.iter().filter(|r| r.state == "running").count();
    let summary = if app.filter.is_empty() {
        format!(" {total} VMs ({running} running) ")
    } else {
        format!(" {visible}/{total} VMs (filter: {}) ", app.filter)
    };

    let line = Line::from(vec![
        Span::styled(TITLE,    Style::default().bg(Color::DarkGray).fg(Color::White).bold()),
        Span::raw("  "),
        Span::styled(summary, Style::default().fg(Color::Gray)),
        Span::raw("  "),
        Span::styled(format!("sort: {}", app.sort.label()),
                     Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_body(f: &mut Frame, area: Rect, app: &App) {
    let rows: Vec<&VmRow> = app.visible();
    if rows.is_empty() {
        draw_empty(f, area, app);
        return;
    }

    let widths = [
        Constraint::Percentage(28),
        Constraint::Length(12),
        Constraint::Length(18),
    ];

    let header = Row::new(vec!["NAME", "STATE", "IP"])
        .style(Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD));

    let body_rows: Vec<Row> = rows.iter().enumerate().map(|(i, r)| {
        let selected = i == app.selected;
        let prefix = if selected { "› " } else { "  " };
        let name_style = if selected {
            Style::default().fg(Color::White).bg(Color::Blue).bold()
        } else {
            Style::default().fg(Color::White)
        };
        let state_style = match r.state.as_str() {
            "running"  => Style::default().fg(Color::Green),
            "paused"   => Style::default().fg(Color::Yellow),
            "crashed"  => Style::default().fg(Color::Red),
            _          => Style::default().fg(Color::DarkGray),
        };
        let ip_text = r.ip.as_deref().unwrap_or("—");
        let ip_style = if r.ip.is_some() { Style::default() }
                       else { Style::default().fg(Color::DarkGray) };
        Row::new(vec![
            Span::styled(format!("{prefix}{}", r.name), name_style),
            Span::styled(r.state.clone(), state_style),
            Span::styled(ip_text.to_string(), ip_style),
        ])
    }).collect();

    let table = Table::new(body_rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Plain));
    f.render_widget(table, area);
}

fn draw_empty(f: &mut Frame, area: Rect, app: &App) {
    let lines = if app.rows.is_empty() {
        vec![
            Line::from(""),
            Line::from("No VMs yet.").alignment(Alignment::Center),
            Line::from(""),
            Line::from(vec![
                Span::styled("Press ", Style::default().fg(Color::DarkGray)),
                Span::styled("c", Style::default().fg(Color::Yellow).bold()),
                Span::styled(" to create your first.", Style::default().fg(Color::DarkGray)),
            ]).alignment(Alignment::Center),
            Line::from(vec![
                Span::styled("Press ", Style::default().fg(Color::DarkGray)),
                Span::styled("q", Style::default().fg(Color::Yellow).bold()),
                Span::styled(" to quit.", Style::default().fg(Color::DarkGray)),
            ]).alignment(Alignment::Center),
        ]
    } else {
        vec![
            Line::from(""),
            Line::from(format!("No VMs match filter '{}'.", app.filter))
                .alignment(Alignment::Center),
            Line::from("Press Esc or clear with /  ").alignment(Alignment::Center),
        ]
    };
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

fn draw_keybar(f: &mut Frame, area: Rect, app: &App) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);
    f.render_widget(
        Paragraph::new(HOTKEYS_TABLE).style(Style::default().fg(Color::Gray)),
        layout[0],
    );
    let refreshed = app.last_refresh
        .map(|t| format!("refreshed {}s ago", t.elapsed().as_secs()))
        .unwrap_or_else(|| "—".into());
    f.render_widget(
        Paragraph::new(refreshed)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Right),
        layout[1],
    );
}

fn draw_toast(f: &mut Frame, area: Rect, app: &App) {
    if let Some(toast) = app.current_toast() {
        let (msg, style) = match toast {
            Toast::Ok(m)  => (m.clone(), Style::default().fg(Color::Green)),
            Toast::Err(m) => (m.clone(), Style::default().fg(Color::Red).bold()),
        };
        f.render_widget(Paragraph::new(msg).style(style), area);
    }
}

// ── modals ────────────────────────────────────────────────────────────────────

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect { x, y, width: w.min(area.width), height: h.min(area.height) }
}

fn draw_popup(f: &mut Frame, area: Rect, title: &str, content: &str, scroll: Option<u16>) {
    let w = (area.width as i32 - 8).clamp(40, 100) as u16;
    let h = (area.height as i32 - 4).clamp(8, 30) as u16;
    let r = centered(area, w, h);
    f.render_widget(Clear, r);
    let mut p = Paragraph::new(content.to_string())
        .block(Block::default().title(format!(" {title} ")).borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    if let Some(s) = scroll {
        p = p.scroll((s, 0));
    }
    f.render_widget(p, r);
}

fn draw_confirm(f: &mut Frame, area: Rect, app: &App) {
    let name = app.selected_name().unwrap_or_default();
    let content = format!("Delete '{name}' and all its data?\n\nThis cannot be undone.\n\n[y] confirm   [n] cancel");
    let r = centered(area, 56, 9);
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(content)
            .block(Block::default().title(" Confirm delete ")
                  .borders(Borders::ALL)
                  .border_style(Style::default().fg(Color::Red)))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false }),
        r,
    );
}

fn draw_help(f: &mut Frame, area: Rect) {
    let body = "\
NAVIGATION
  ↑ ↓ / k j         move selection
  /                 filter by name
  o                 cycle sort (name / state / ip)

VM ACTIONS
  c                 create a new VM
  s / t / r         start / stop / restart selected
  d                 delete selected (confirm with y)
  v                 show VNC connect info
  e                 open serial console (Ctrl-] to exit)
  i                 inspect (full `virsh dominfo`)

GENERAL
  ?                 this screen
  Ctrl-C, q, Esc    quit
";
    let r = centered(area, 70, 22);
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(body)
            .block(Block::default().title(" Help ").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        r,
    );
}

fn draw_filter(f: &mut Frame, area: Rect, app: &App) {
    let r = centered(area, 50, 3);
    f.render_widget(Clear, r);
    let v = &app.filter_input.value;
    let display = format!("/ {v}");
    f.render_widget(
        Paragraph::new(display)
            .block(Block::default().title(" Filter (Enter=apply  Esc=cancel) ").borders(Borders::ALL)),
        r,
    );
    // Cursor position: 1 (start of border) + 2 ("/ ") + cursor offset.
    let cx = r.x + 1 + 2 + app.filter_input.cursor as u16;
    let cy = r.y + 1;
    f.set_cursor_position((cx, cy));
}

fn draw_create_modal(f: &mut Frame, area: Rect, app: &App) {
    let w = 60u16;
    let h = 18u16;
    let r = centered(area, w, h);
    f.render_widget(Clear, r);
    f.render_widget(
        Block::default().title(" Create VM ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
        r,
    );

    let inner = Rect { x: r.x + 2, y: r.y + 1, width: r.width - 4, height: r.height - 2 };
    let labels = ["Name", "Distro", "vCPUs", "RAM (GB)", "Disk (GB)", "User"];
    let items: Vec<ListItem> = labels.iter().enumerate().map(|(i, label)| {
        let focused = i == app.create.field;
        let bullet = if focused { "▸ " } else { "  " };
        let value = field_value(app, i);
        let label_style = if focused {
            Style::default().fg(Color::Yellow).bold()
        } else {
            Style::default().fg(Color::Gray)
        };
        ListItem::new(Line::from(vec![
            Span::styled(format!("{bullet}{label:<10}"), label_style),
            Span::raw(value),
        ]))
    }).collect();

    let list = List::new(items);
    f.render_widget(list, Rect {
        x: inner.x, y: inner.y, width: inner.width, height: 6,
    });

    // Hint line.
    f.render_widget(
        Paragraph::new("Tab/Shift-Tab move • Enter create • Esc cancel  (←/→ cycles distro)")
            .style(Style::default().fg(Color::DarkGray)),
        Rect { x: inner.x, y: inner.y + 8, width: inner.width, height: 1 },
    );

    // Distro list hint (pulled vs missing).
    let distro_legend = match app.create.distros.get(app.create.distro_idx) {
        Some(d) => format!("({}/{})  ←/→ to cycle", app.create.distro_idx + 1, app.create.distros.len())
            + &format!("   {d}"),
        None => String::new(),
    };
    f.render_widget(
        Paragraph::new(distro_legend)
            .style(Style::default().fg(Color::DarkGray)),
        Rect { x: inner.x, y: inner.y + 10, width: inner.width, height: 1 },
    );

    // Cursor on the focused text field (if applicable).
    if let Some((cx_offset, focused_text)) = focused_cursor_offset(app) {
        let cx = inner.x + 12 + cx_offset as u16; // 2 ("▸ ") + 10 (label width) = 12
        let cy = inner.y + app.create.field as u16;
        // Only set cursor for text fields, not the distro picker.
        if focused_text { f.set_cursor_position((cx, cy)); }
    }
}

fn field_value(app: &App, i: usize) -> String {
    match i {
        0 => app.create.name.value.clone(),
        1 => app.create.distros.get(app.create.distro_idx).cloned().unwrap_or_default(),
        2 => app.create.cpus.value.clone(),
        3 => app.create.memory_gb.value.clone(),
        4 => app.create.disk_gb.value.clone(),
        5 => {
            let v = &app.create.user.value;
            if v.trim().is_empty() { "(auto: vmXXXXXX)".into() } else { v.clone() }
        }
        _ => String::new(),
    }
}

fn focused_cursor_offset(app: &App) -> Option<(usize, bool)> {
    match app.create.field {
        0 => Some((app.create.name.cursor, true)),
        1 => Some((0, false)), // distro picker — no cursor
        2 => Some((app.create.cpus.cursor, true)),
        3 => Some((app.create.memory_gb.cursor, true)),
        4 => Some((app.create.disk_gb.cursor, true)),
        5 => {
            let user_blank = app.create.user.value.trim().is_empty();
            if user_blank { Some((0, false)) }
            else { Some((app.create.user.cursor, true)) }
        }
        _ => None,
    }
}

#[allow(dead_code)]
fn _coerce(_: Sort) {} // keep the enum import used; doesn't ship
