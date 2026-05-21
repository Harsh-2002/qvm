//! Pure render functions. No state mutation, no I/O.
//!
//! Layout (≥80 cols):
//!
//!   Header               (1 line)
//!   ┌──────────┬──────────────────┐
//!   │ Sidebar  │  Content pane    │   ← body, fills remaining height
//!   └──────────┴──────────────────┘
//!   Toast line           (1 line, only when a toast is active)
//!   ╭──────────────────────────────╮
//!   │ Action bar  (2 rows of keys) │   ← bordered panel, 4 lines incl. borders
//!   ╰──────────────────────────────╯
//!
//! Every render fn takes `&App`; styles come from `app.theme` exclusively.

use crate::tui::app::{App, CreateForm, FocusPane, Mode, Sort, Toast, VmRow};
use crate::tui::theme::Theme;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn draw(f: &mut Frame, app: &mut App) {
    // Reset hit-test tables — repopulated by sub-renderers.
    app.sidebar_hits.clear();
    app.action_hits.clear();

    let area = f.area();
    let theme = app.theme.clone();

    // Build the action list once and decide the action-bar layout up front
    // so we know its exact height (and the body gets the leftover space).
    let actions = build_actions(app);
    let rows_of_actions = fit_action_rows(&actions, area.width.saturating_sub(4));
    let action_bar_h: u16 = (rows_of_actions.len() as u16).max(1) + 2; // rows + 2 border lines

    let toast_h: u16 = if app.current_toast().is_some() { 1 } else { 0 };

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),               // header
            Constraint::Min(3),                  // body
            Constraint::Length(toast_h),
            Constraint::Length(action_bar_h),
        ])
        .split(area);

    draw_header(f, layout[0], app);

    if layout[1].width < 80 {
        draw_narrow_body(f, layout[1], app);
    } else {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(28), Constraint::Min(40)])
            .split(layout[1]);
        draw_sidebar(f, body[0], app);
        draw_content(f, body[1], app);
    }

    if toast_h > 0 { draw_toast_line(f, layout[2], app); }
    draw_action_bar(f, layout[3], app, &rows_of_actions);

    // Modal overlay — confirm delete renders centered on top of everything.
    if let Mode::ConfirmDelete = app.mode {
        draw_confirm_dialog(f, area, app);
    }

    // Position the text cursor for active text-entry modes.
    match &app.mode {
        Mode::Filter => place_filter_cursor(f, app, layout[1], &theme),
        Mode::CreateForm => place_create_cursor(f, app, layout[1]),
        _ => {}
    }
}

// ── header ────────────────────────────────────────────────────────────────────

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let running = app.rows.iter().filter(|r| r.state == "running").count();
    let total   = app.rows.len();
    let summary = if app.filter.is_empty() {
        format!("{total} VMs · {running} running")
    } else {
        format!("{}/{total} VMs · filter “{}”", app.visible_count(), app.filter)
    };

    let mut spans: Vec<Span<'_>> = vec![
        Span::raw(" "),
        Span::styled(format!("qvm {VERSION}"), t.accent()),
        Span::styled("  •  ", Style::default().fg(t.text_faint)),
        Span::styled(app.host_label.clone(), t.text()),
        Span::styled("  •  ", Style::default().fg(t.text_faint)),
        Span::styled(summary, t.dim()),
    ];

    // Right-aligned context hint or spinner.
    let right_hint = if app.is_refreshing {
        format!("{} refreshing… ", t.spinner(app.tick))
    } else {
        match app.mode {
            Mode::Help => "any key to dismiss ".into(),
            _          => "[?] help  [Tab] focus  [q] quit ".into(),
        }
    };

    // Pad between left and right.
    let used_left: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let pad = (area.width as usize).saturating_sub(used_left + right_hint.chars().count());
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(right_hint, t.faint()));

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ── sidebar ───────────────────────────────────────────────────────────────────

fn draw_sidebar(f: &mut Frame, area: Rect, app: &mut App) {
    let t = &app.theme;
    let focused = app.focused == FocusPane::Sidebar;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(t.border_style(focused))
        .title(Span::styled("  VIRTUAL MACHINES  ",
            Style::default().fg(t.text_faint).add_modifier(Modifier::BOLD)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut cursor_y = inner.y;

    // Filter row (only when filtering or filter is set).
    if matches!(app.mode, Mode::Filter) || !app.filter.is_empty() {
        let active = matches!(app.mode, Mode::Filter);
        let value = if active { &app.filter_input.value } else { &app.filter };
        let icon_style = if active { t.accent() } else { t.faint() };
        let line = Line::from(vec![
            Span::raw(" "),
            Span::styled("/", icon_style),
            Span::raw(" "),
            Span::styled(value.clone(), t.text()),
        ]);
        f.render_widget(Paragraph::new(line), Rect { x: inner.x, y: cursor_y, width: inner.width, height: 1 });
        cursor_y = cursor_y.saturating_add(2); // blank line after filter
    }

    // Reserve 3 lines at the bottom for separator + "+ create new VM" row + blank.
    let reserved_bottom: u16 = 3;
    let avail_h = inner.height.saturating_sub(cursor_y - inner.y).saturating_sub(reserved_bottom);

    let rows = app.visible();
    // Each item is 2 lines (name + state line) + 1 blank => 3 lines/item.
    let per_item: u16 = 3;
    let max_items = (avail_h / per_item) as usize;
    let items: Vec<ListItem<'_>> = rows.iter().enumerate().take(max_items).map(|(i, r)| {
        let selected = i == app.selected;
        let state_for_display = app.displayed_state(r);
        let sel_marker = if selected { "▶ " } else { "  " };
        let name_style = if selected {
            Style::default().fg(t.text).bg(t.surface).add_modifier(Modifier::BOLD)
        } else {
            t.text()
        };
        let line1 = Line::from(vec![
            Span::styled(sel_marker, t.accent()),
            Span::styled(t.state_glyph(&state_for_display).to_string(),
                Style::default().fg(t.state_color(&state_for_display))),
            Span::raw(" "),
            Span::styled(r.name.clone(), name_style),
        ]);
        let detail = match &r.ip {
            Some(ip) => format!("    {} · {}", state_for_display, ip),
            None     => format!("    {}", state_for_display),
        };
        let line2 = Line::from(Span::styled(detail, t.faint()));
        ListItem::new(vec![line1, line2, Line::raw("")])
    }).collect();

    let list_h = (items.len() as u16) * per_item;
    let list_area = Rect { x: inner.x, y: cursor_y, width: inner.width, height: list_h };
    f.render_widget(List::new(items), list_area);

    // Record hit areas for mouse clicks.
    for i in 0..items_count(app, max_items) {
        let row_rect = Rect {
            x: inner.x,
            y: cursor_y + (i as u16) * per_item,
            width: inner.width,
            height: 2, // name + state line; ignore the blank
        };
        app.sidebar_hits.push((row_rect, i));
    }

    // [+] create new VM at the bottom of the sidebar.
    let bottom_y = inner.y + inner.height - 2;
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ─ ", t.faint()),
            Span::styled("─".repeat(inner.width.saturating_sub(4) as usize), t.faint()),
            Span::raw(" "),
        ])),
        Rect { x: inner.x, y: bottom_y, width: inner.width, height: 1 },
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled("+", t.accent()),
            Span::raw(" "),
            Span::styled("create new VM", t.text()),
            Span::styled("    (c)", t.faint()),
        ])),
        Rect { x: inner.x, y: bottom_y + 1, width: inner.width, height: 1 },
    );
}

// ── content pane ──────────────────────────────────────────────────────────────

fn items_count(app: &App, max_items: usize) -> usize {
    app.visible().len().min(max_items)
}

fn draw_content(f: &mut Frame, area: Rect, app: &mut App) {
    let t = &app.theme;
    let focused = app.focused == FocusPane::Detail;
    let title = match &app.mode {
        Mode::CreateForm => "  CREATE VM  ".to_string(),
        Mode::Help       => "  HELP  ".to_string(),
        Mode::EmptyState => "".to_string(),
        _                => app.selected_name()
            .map(|n| format!("  {n}  "))
            .unwrap_or_default(),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(t.border_style(focused))
        .title(Span::styled(title,
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    match &app.mode {
        Mode::Detail | Mode::ConfirmDelete | Mode::Filter => draw_detail(f, inner, app),
        Mode::CreateForm => draw_create(f, inner, app),
        Mode::Help       => draw_help(f, inner, t),
        Mode::EmptyState => draw_empty(f, inner, t),
    }
}

fn draw_detail(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let Some(row) = app.selected_row() else {
        draw_empty(f, area, t);
        return;
    };

    let inner = area.inner(Margin { horizontal: 2, vertical: 1 });

    // Status banner (1 line, colored).
    let state = app.displayed_state(row);
    f.render_widget(
        Paragraph::new(Line::from(t.status_banner_span(&state))),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );

    // Metadata rows or raw dominfo, depending on toggle.
    if app.show_raw_dominfo {
        let raw = row.dominfo.clone().unwrap_or_else(|| "(loading dominfo…)".into());
        f.render_widget(
            Paragraph::new(raw)
                .style(t.dim())
                .scroll((app.detail_scroll, 0))
                .wrap(Wrap { trim: false }),
            Rect { x: inner.x, y: inner.y + 2, width: inner.width,
                   height: inner.height.saturating_sub(2) },
        );
        // Footer hint
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Press ", t.faint()),
                Span::styled("[Shift+R]", t.accent()),
                Span::styled(" to show structured view", t.faint()),
            ])),
            Rect { x: inner.x, y: inner.y + inner.height - 1, width: inner.width, height: 1 },
        );
        return;
    }

    // Structured metadata table.
    let rows_meta = build_metadata_rows(row, t);
    let mut y = inner.y + 2; // skip status + 1 blank line
    for (label, value_span) in rows_meta {
        if y >= inner.y + inner.height - 2 { break; }
        let label_padded = format!("{:<12}", label);
        let line = Line::from(vec![
            Span::raw("  "),
            Span::styled(label_padded, t.dim()),
            value_span,
        ]);
        f.render_widget(Paragraph::new(line), Rect { x: inner.x, y, width: inner.width, height: 1 });
        y += 1;
    }

    // Section separator + heading for the secondary metadata.
    if y < inner.y + inner.height - 4 {
        y += 1;
        f.render_widget(
            Paragraph::new(Line::from(t.section_heading_span("details"))),
            Rect { x: inner.x + 2, y, width: inner.width.saturating_sub(4), height: 1 },
        );
        y += 1;
        for (label, value) in extract_secondary_meta(row) {
            if y >= inner.y + inner.height - 2 { break; }
            let label_padded = format!("{:<12}", label);
            let line = Line::from(vec![
                Span::raw("  "),
                Span::styled(label_padded, t.dim()),
                Span::styled(value, t.text()),
            ]);
            f.render_widget(Paragraph::new(line), Rect { x: inner.x, y, width: inner.width, height: 1 });
            y += 1;
        }
    }

    // Bottom hint
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Press ", t.faint()),
            Span::styled("[Shift+R]", t.accent()),
            Span::styled(" for raw  ", t.faint()),
            Span::styled("[Tab]", t.accent()),
            Span::styled(" focus", t.faint()),
        ])),
        Rect { x: inner.x, y: inner.y + inner.height - 1, width: inner.width, height: 1 },
    );
}

fn build_metadata_rows<'a>(row: &'a VmRow, t: &'a Theme) -> Vec<(&'a str, Span<'a>)> {
    vec![
        ("IP",         Span::styled(row.ip.clone().unwrap_or_else(|| "—".into()),
            if row.ip.is_some() { t.text() } else { t.faint() })),
        ("Name",       Span::styled(row.name.clone(), t.text())),
        ("State",      Span::styled(row.state.clone(),
            Style::default().fg(t.state_color(&row.state)))),
    ]
}

fn extract_secondary_meta(row: &VmRow) -> Vec<(String, String)> {
    let Some(raw) = &row.dominfo else { return vec![]; };
    let mut keep: Vec<(String, String)> = Vec::new();
    let interesting: &[&str] = &[
        "UUID", "OS Type", "CPU(s)", "CPU time", "Max memory", "Used memory",
        "Persistent", "Autostart",
    ];
    for line in raw.lines() {
        let Some(colon) = line.find(':') else { continue };
        let key = line[..colon].trim();
        if !interesting.contains(&key) { continue; }
        let val = line[colon+1..].trim();
        let pretty = match key {
            "Max memory" | "Used memory" => humanize_kib(val),
            "CPU(s)"                     => format!("{val} CPU"),
            _                            => val.to_string(),
        };
        keep.push((key.to_string(), pretty));
    }
    keep
}

fn humanize_kib(s: &str) -> String {
    let trimmed = s.trim_end_matches(" KiB");
    if let Ok(n) = trimmed.parse::<u64>() {
        let gib = n as f64 / 1024.0 / 1024.0;
        if gib >= 1.0 { return format!("{gib:.2} GiB"); }
        let mib = n as f64 / 1024.0;
        if mib >= 1.0 { return format!("{mib:.0} MiB"); }
    }
    s.to_string()
}

fn draw_create(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let inner = area.inner(Margin { horizontal: 2, vertical: 1 });
    let c = &app.create;

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Tab/Shift-Tab", t.accent()),
            Span::styled(" move · ", t.faint()),
            Span::styled("←/→", t.accent()),
            Span::styled(" cycle distro · ", t.faint()),
            Span::styled("Enter", t.accent()),
            Span::styled(" create · ", t.faint()),
            Span::styled("Esc", t.accent()),
            Span::styled(" cancel", t.faint()),
        ])),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );

    let labels = ["Name", "Distro", "CPUs", "RAM (GB)", "Disk (GB)", "User", "Password"];
    for (i, label) in labels.iter().enumerate() {
        let focused = i == c.field;
        let bullet = if focused { "▸ " } else { "  " };
        let label_pad = format!("{:<11}", label);
        let value = field_value(c, i);
        let label_style = if focused { t.accent() } else { t.dim() };
        let line = Line::from(vec![
            Span::styled(bullet, label_style),
            Span::styled(label_pad, label_style),
            Span::styled(value, t.text()),
        ]);
        f.render_widget(Paragraph::new(line),
            Rect { x: inner.x, y: inner.y + 2 + i as u16, width: inner.width, height: 1 });
    }
}

fn field_value(c: &CreateForm, i: usize) -> String {
    match i {
        0 => c.name.value.clone(),
        1 => match c.distros.get(c.distro_idx) {
            Some(name) => {
                let pulled = c.distro_pulled.get(c.distro_idx).copied().unwrap_or(false);
                let badge = if pulled { "● pulled" } else { "○ not pulled (auto-download)" };
                format!("{name}  ({}/{})   {badge}", c.distro_idx + 1, c.distros.len())
            }
            None => String::new(),
        },
        2 => c.cpus.value.clone(),
        3 => c.memory_gb.value.clone(),
        4 => c.disk_gb.value.clone(),
        5 => {
            let v = &c.user.value;
            if v.trim().is_empty() { "(required — no default)".into() } else { v.clone() }
        }
        6 => {
            // Mask the password as bullets so onlookers can't read it.
            let n = c.password.value.chars().count();
            if n == 0 { "(required — no default)".into() } else { "•".repeat(n) }
        }
        _ => String::new(),
    }
}

fn place_create_cursor(f: &mut Frame, app: &App, body: Rect) {
    // Detail/content pane lives in body[1] after the layout split, but at
    // this level we just compute approximate cursor coords inside the
    // create form. The form is rendered with margin (2,1) inside the
    // content pane block. Field rows start at body.y + 2 (inside block) +
    // 2 (margin) + 0 (form header line) = body.y + 4.
    let c = &app.create;
    let off = match c.field {
        0 => c.name.cursor,
        2 => c.cpus.cursor,
        3 => c.memory_gb.cursor,
        4 => c.disk_gb.cursor,
        5 if !c.user.value.trim().is_empty() => c.user.cursor,
        6 if !c.password.value.is_empty() => c.password.cursor,
        _ => return,
    };
    // Inner content starts after the sidebar (28 cols) + content pane border (1 col)
    // + margin (2 cols) = body.x + 31. Then 2 (bullet) + 11 (label) = +13.
    let cx = body.x + 28 + 1 + 2 + 2 + 11 + off as u16;
    let cy = body.y + 1 + 1 + 2 + c.field as u16; // border(1) + margin(1) + header(2) + field
    f.set_cursor_position((cx, cy));
}

fn place_filter_cursor(f: &mut Frame, app: &App, body: Rect, _t: &Theme) {
    // Filter renders inside sidebar at inner.y (which is body.y + 1 for the border).
    // Sidebar inner x = body.x + 1. Filter "/" + " " takes 3 chars before value.
    let cx = body.x + 1 + 3 + app.filter_input.cursor as u16;
    let cy = body.y + 1;
    f.set_cursor_position((cx, cy));
}

fn draw_help(f: &mut Frame, area: Rect, t: &Theme) {
    let inner = area.inner(Margin { horizontal: 3, vertical: 1 });
    let sections: &[(&str, &[(&str, &str)])] = &[
        ("Navigation", &[
            ("↑ ↓ / k j",        "select VM in the sidebar"),
            ("Tab",              "switch focus between sidebar and detail"),
            ("PgUp / PgDn",      "scroll the detail pane"),
            ("/",                "filter VMs by name"),
            ("o",                "cycle sort (name / state / ip)"),
        ]),
        ("VM lifecycle", &[
            ("s · t · r",        "start · stop · restart selected"),
            ("c",                "create a new VM"),
            ("d",                "delete selected (with confirmation)"),
        ]),
        ("View the console", &[
            ("e",                "attach serial console (Ctrl-] to exit)"),
            ("v",                "show native VNC connect info as a toast"),
            ("b",                "open VNC in a browser (with QR for mobile)"),
        ]),
        ("View toggles", &[
            ("Shift+R",          "structured view ↔ raw virsh dominfo"),
        ]),
        ("General", &[
            ("?",                "this screen"),
            ("q · Esc",          "go back / quit"),
            ("Ctrl-C",           "force quit"),
        ]),
    ];

    let mut y = inner.y;
    for (title, rows) in sections {
        if y >= inner.y + inner.height { break; }
        f.render_widget(
            Paragraph::new(Line::from(t.section_heading_span(title))),
            Rect { x: inner.x, y, width: inner.width, height: 1 },
        );
        y += 1;
        for (key, label) in *rows {
            if y >= inner.y + inner.height { break; }
            let line = Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{key:<14}"), t.accent()),
                Span::styled((*label).to_string(), t.text()),
            ]);
            f.render_widget(Paragraph::new(line),
                Rect { x: inner.x, y, width: inner.width, height: 1 });
            y += 1;
        }
        y += 1;
    }
}

fn draw_empty(f: &mut Frame, area: Rect, t: &Theme) {
    let art = [
        "                                    ",
        "       ╔═══════════════╗            ",
        "       ║  ▁▁▁▁▁▁▁▁▁▁▁  ║            ",
        "       ║  ▍ ▍ ▍ ▍ ▍ ▍  ║            ",
        "       ║  ▔▔▔▔▔▔▔▔▔▔▔  ║            ",
        "       ╚═══════════════╝            ",
        "                                    ",
    ];
    let mut lines: Vec<Line<'_>> = art.iter()
        .map(|s| Line::from(Span::styled(s.to_string(), t.faint())))
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled("No virtual machines yet.", t.bold()))
        .alignment(Alignment::Center));
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("Press ", t.dim()),
        Span::styled("[c]", t.accent()),
        Span::styled(" to create your first VM.", t.dim()),
    ]).alignment(Alignment::Center));
    lines.push(Line::from(vec![
        Span::styled("Or run ", t.faint()),
        Span::styled("qvm doctor", t.accent()),
        Span::styled(" in a shell to check the host setup.", t.faint()),
    ]).alignment(Alignment::Center));

    let total = lines.len() as u16;
    let pad_top = area.height.saturating_sub(total) / 2;
    let rect = Rect {
        x: area.x,
        y: area.y + pad_top,
        width: area.width,
        height: total.min(area.height),
    };
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).alignment(Alignment::Center),
        rect,
    );
}

fn draw_narrow_body(f: &mut Frame, area: Rect, app: &mut App) {
    draw_sidebar(f, area, app);
}

// ── toast + action bar ────────────────────────────────────────────────────────

fn draw_toast_line(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    if let Some(toast) = app.current_toast() {
        let (text, style) = match toast {
            Toast::Ok(m)  => (m.clone(),
                Style::default().fg(t.ok).add_modifier(Modifier::BOLD)),
            Toast::Err(m) => (m.clone(),
                Style::default().fg(t.err).add_modifier(Modifier::BOLD)),
        };
        let line = Line::from(vec![
            Span::raw(" "),
            Span::styled("●", style),
            Span::raw(" "),
            Span::styled(text, t.text()),
        ]);
        f.render_widget(Paragraph::new(line), area);
    }
}

/// All actions surfaced in the action bar for the current mode, in display
/// order. Returns owned tuples of static strings so the caller can reflow.
fn build_actions(app: &App) -> Vec<(&'static str, &'static str, bool)> {
    // Use the *displayed* state so when the user presses Start, the row
    // immediately shows "starting…" and the Start button dims without a
    // wrong-direction flicker before the real state catches up.
    let state = app.selected_row()
        .map(|r| app.displayed_state(r))
        .unwrap_or_default();
    let running = state == "running";
    let in_transition = state.ends_with('…');
    let has_selection = !app.rows.is_empty();

    if matches!(app.mode, Mode::CreateForm) {
        vec![
            ("Tab",   "Move",   true),
            ("←/→",   "Distro", true),
            ("Enter", "Create", true),
            ("Esc",   "Cancel", true),
        ]
    } else if matches!(app.mode, Mode::ConfirmDelete) {
        vec![
            ("y",   "Confirm Delete", true),
            ("n",   "Cancel",         true),
            ("Esc", "Cancel",         true),
        ]
    } else {
        // Order matters: lifecycle first, then create/view/utility.
        vec![
            ("s", "Start",    has_selection && !running && !in_transition),
            ("t", "Stop",     has_selection &&  running && !in_transition),
            ("r", "Restart",  has_selection &&  running && !in_transition),
            ("e", "Console",  has_selection && !in_transition),
            ("b", "Browser",  has_selection && !in_transition),
            ("d", "Delete",   has_selection),
            ("c", "Create",   true),
            ("v", "VNC info", has_selection &&  running),
            ("/", "Filter",   true),
            ("o", "Sort",     true),
            ("?", "Help",     true),
            ("q", "Quit",     true),
        ]
    }
}

/// `[k] Label` button width in terminal cells.
fn button_width(key: &str, label: &str) -> u16 {
    // `[` `key` `]` ` ` `label`
    (1 + key.chars().count() + 1 + 1 + label.chars().count()) as u16
}

/// Greedily pack action buttons into rows that each fit `max_w` cells.
/// Most terminals show the action bar on a single line; very narrow ones
/// wrap to 2 or 3 rows automatically.
fn fit_action_rows<'a>(items: &[(&'a str, &'a str, bool)], max_w: u16)
    -> Vec<Vec<(&'a str, &'a str, bool)>>
{
    const GAP:  u16 = 3; // separator between buttons
    const LEAD: u16 = 2; // leading indent inside the action bar
    let mut rows: Vec<Vec<(&str, &str, bool)>> = vec![];
    let mut current: Vec<(&str, &str, bool)> = vec![];
    let mut width: u16 = LEAD;
    for item in items {
        let bw = button_width(item.0, item.1);
        let extra = if current.is_empty() { bw } else { GAP + bw };
        if !current.is_empty() && width + extra > max_w {
            rows.push(std::mem::take(&mut current));
            width = LEAD + bw;
            current.push(*item);
        } else {
            current.push(*item);
            width += extra;
        }
    }
    if !current.is_empty() { rows.push(current); }
    rows
}

fn draw_action_bar(f: &mut Frame, area: Rect, app: &mut App,
    rows: &[Vec<(&str, &str, bool)>])
{
    // Owned theme copy — keeps `&app.theme` alive from conflicting with
    // `&mut app.action_hits` we mutate below.
    let t = app.theme.clone();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(t.border_style(false));
    let inner = block.inner(area);
    f.render_widget(block, area);

    for (i, row) in rows.iter().enumerate() {
        let row_area = Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 };
        draw_action_row(f, row_area, &t, row, &mut app.action_hits);
    }
}

fn draw_action_row(f: &mut Frame, area: Rect, t: &Theme, items: &[(&str, &str, bool)], hits: &mut Vec<(Rect, char)>) {
    let mut spans: Vec<Span<'_>> = vec![Span::raw("  ")];
    let mut x_offset: u16 = 2;
    for (i, (key, label, enabled)) in items.iter().enumerate() {
        if i > 0 { spans.push(Span::raw("   ")); x_offset += 3; }
        let key_style = if *enabled {
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.text_faint)
        };
        let lbl_style = if *enabled { t.text() } else { t.faint().add_modifier(Modifier::DIM) };
        spans.push(Span::styled("[", t.faint()));
        spans.push(Span::styled((*key).to_string(), key_style));
        spans.push(Span::styled("]", t.faint()));
        spans.push(Span::raw(" "));
        spans.push(Span::styled((*label).to_string(), lbl_style));

        // Hit-test region for this button. Only register if enabled.
        if *enabled {
            // Use the first char of the key string as the lookup char.
            // For multi-char keys (Tab, Enter, Esc) we don't register mouse hits.
            if let Some(c) = key.chars().next() {
                if key.chars().count() == 1 {
                    let button_w = (key.len() + 2 + 1 + label.len()) as u16; // [k] label
                    let r = Rect { x: area.x + x_offset, y: area.y, width: button_w, height: 1 };
                    hits.push((r, c));
                    x_offset += button_w;
                } else {
                    let button_w = (key.len() + 2 + 1 + label.len()) as u16;
                    x_offset += button_w;
                }
            }
        } else {
            let button_w = (key.len() + 2 + 1 + label.len()) as u16;
            x_offset += button_w;
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ── confirm-delete modal ──────────────────────────────────────────────────────

fn draw_confirm_dialog(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let name = app.selected_name().unwrap_or_default();

    // Wide enough that the explanation doesn't wrap, with room for two
    // visually-distinct yes/no buttons drawn as their own bordered boxes.
    let dialog_w: u16 = 64;
    let dialog_h: u16 = 13;
    let r = centered(area, dialog_w, dialog_h);

    f.render_widget(Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.err))
        .title(Span::styled("  ⚠  Delete VM  ",
            Style::default().fg(t.err).add_modifier(Modifier::BOLD)));
    let inner = block.inner(r);
    f.render_widget(block, r);

    // Top: question + explanation. Interior width = dialog_w - 2 = 62 cols.
    let body = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled("  Delete ", t.text()),
            Span::styled(format!("'{name}'"),
                Style::default().fg(t.err).add_modifier(Modifier::BOLD)),
            Span::styled(" ?", t.text()),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "  This permanently removes the VM's disk, libvirt config",
            t.dim(),
        )),
        Line::from(Span::styled(
            "  and cloud-init seed. It cannot be undone.",
            t.dim(),
        )),
        Line::raw(""),
    ];
    f.render_widget(
        Paragraph::new(body).wrap(Wrap { trim: false }),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 7 },
    );

    // Buttons: two boxed regions side by side near the bottom of the modal.
    // Delete is destructive (err palette); Cancel is neutral (accent).
    let row_y = inner.y + inner.height - 3;
    let btn_h: u16 = 3;
    let btn1_w: u16 = 18;
    let btn2_w: u16 = 14;
    let gap: u16 = 4;
    let total_w = btn1_w + gap + btn2_w;
    let row_x = inner.x + (inner.width.saturating_sub(total_w)) / 2;

    let btn1 = Rect { x: row_x, y: row_y, width: btn1_w, height: btn_h };
    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.err)),
        btn1,
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("[y]", Style::default().fg(t.err).add_modifier(Modifier::BOLD)),
            Span::styled(" Delete", Style::default().fg(t.err).add_modifier(Modifier::BOLD)),
        ])).alignment(Alignment::Center),
        Rect { x: btn1.x, y: btn1.y + 1, width: btn1.width, height: 1 },
    );

    let btn2 = Rect { x: row_x + btn1_w + gap, y: row_y, width: btn2_w, height: btn_h };
    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.border)),
        btn2,
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("[n]", t.accent()),
            Span::styled(" Cancel", t.text()),
        ])).alignment(Alignment::Center),
        Rect { x: btn2.x, y: btn2.y + 1, width: btn2.width, height: 1 },
    );
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect { x, y, width: w.min(area.width), height: h.min(area.height) }
}

// Keep Sort referenced so import isn't unused.
#[allow(dead_code)]
fn _sort_export(_: Sort) {}
