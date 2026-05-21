//! Visual design system for the TUI.
//!
//! All colours, helper styles and `Span` builders live here. Every render
//! function in `ui.rs` takes a `&Theme` and never instantiates colours
//! inline. The default palette is Catppuccin Mocha — popular, warm, dark,
//! works on any truecolor terminal.
//!
//! We deliberately leave the application background unset so the user's
//! terminal transparency / background image still shows through.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

#[derive(Debug, Clone)]
pub struct Theme {
    pub surface:      Color,
    pub border:       Color,
    pub border_focus: Color,
    pub text:         Color,
    pub text_dim:     Color,
    pub text_faint:   Color,
    pub accent:       Color,
    pub ok:           Color,
    pub warn:         Color,
    pub err:          Color,
    pub overlay:      Color,
}

impl Default for Theme {
    fn default() -> Self {
        // Catppuccin Mocha — https://github.com/catppuccin/catppuccin
        Self {
            surface:      Color::Rgb(0x31, 0x32, 0x44), // surface0
            border:       Color::Rgb(0x45, 0x47, 0x5a), // surface1
            border_focus: Color::Rgb(0xcb, 0xa6, 0xf7), // mauve
            text:         Color::Rgb(0xcd, 0xd6, 0xf4), // text
            text_dim:     Color::Rgb(0xa6, 0xad, 0xc8), // subtext0
            text_faint:   Color::Rgb(0x6c, 0x70, 0x86), // overlay0
            accent:       Color::Rgb(0xcb, 0xa6, 0xf7), // mauve
            ok:           Color::Rgb(0xa6, 0xe3, 0xa1), // green
            warn:         Color::Rgb(0xf9, 0xe2, 0xaf), // yellow
            err:          Color::Rgb(0xf3, 0x8b, 0xa8), // red
            overlay:      Color::Rgb(0x18, 0x18, 0x25), // mantle (modal bg)
        }
    }
}

impl Theme {
    // ── style primitives ───────────────────────────────────────────────────

    pub fn text(&self)        -> Style { Style::default().fg(self.text) }
    pub fn dim(&self)         -> Style { Style::default().fg(self.text_dim) }
    pub fn faint(&self)       -> Style { Style::default().fg(self.text_faint) }
    pub fn bold(&self)        -> Style { Style::default().fg(self.text).add_modifier(Modifier::BOLD) }
    pub fn accent(&self)      -> Style { Style::default().fg(self.accent).add_modifier(Modifier::BOLD) }
    pub fn label(&self)       -> Style { Style::default().fg(self.text_dim) }
    pub fn err_style(&self)   -> Style { Style::default().fg(self.err) }

    pub fn border_style(&self, focused: bool) -> Style {
        Style::default().fg(if focused { self.border_focus } else { self.border })
    }

    // ── state-aware helpers ────────────────────────────────────────────────

    pub fn state_color(&self, state: &str) -> Color {
        match state {
            "running"            => self.ok,
            "paused"             => self.warn,
            "crashed"            => self.err,
            "starting…" | "stopping…" | "restarting…" => self.warn,
            _                    => self.text_faint,
        }
    }

    pub fn state_glyph(&self, state: &str) -> &'static str {
        match state {
            "running"            => "●",
            "paused"             => "◐",
            "crashed"            => "✗",
            "starting…" | "stopping…" | "restarting…" => "◌",
            _                    => "○",
        }
    }

    /// `▶ RUNNING` styled with bold colored text for the detail-pane status banner.
    pub fn status_banner_span<'a>(&self, state: &'a str) -> Span<'a> {
        let color = self.state_color(state);
        let glyph = self.state_glyph(state);
        Span::styled(
            format!("{glyph}  {}", state.to_uppercase()),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )
    }

    /// Small inline state pill: `● running` or `○ stopped`.
    pub fn state_pill_span<'a>(&self, state: &'a str) -> Span<'a> {
        Span::styled(
            format!("{}  {}", self.state_glyph(state), state),
            Style::default().fg(self.state_color(state)),
        )
    }

    /// Render a metadata label, e.g. dim/aligned `Distro`. Caller pads.
    pub fn meta_label_span<'a>(&self, label: &'a str) -> Span<'a> {
        Span::styled(label.to_string(), self.dim())
    }

    pub fn meta_value_span<'a>(&self, value: &'a str) -> Span<'a> {
        Span::styled(value.to_string(), self.text())
    }

    /// Hot-key hint pair: `[s] Start`. `enabled=false` faints the whole pair.
    pub fn keyhint<'a>(&self, key: char, label: &'a str, enabled: bool) -> Vec<Span<'a>> {
        if enabled {
            vec![
                Span::styled("[", Style::default().fg(self.text_faint)),
                Span::styled(key.to_string(),
                    Style::default().fg(self.accent).add_modifier(Modifier::BOLD)),
                Span::styled("]", Style::default().fg(self.text_faint)),
                Span::raw(" "),
                Span::styled(label.to_string(), Style::default().fg(self.text)),
            ]
        } else {
            vec![
                Span::styled("[", Style::default().fg(self.text_faint)),
                Span::styled(key.to_string(),
                    Style::default().fg(self.text_faint)),
                Span::styled("]", Style::default().fg(self.text_faint)),
                Span::raw(" "),
                Span::styled(label.to_string(),
                    Style::default().fg(self.text_faint).add_modifier(Modifier::DIM)),
            ]
        }
    }

    /// Section heading inside the detail pane (small caps, dim, with rule).
    pub fn section_heading_span<'a>(&self, label: &'a str) -> Span<'a> {
        Span::styled(
            format!("─ {}  ─", label.to_uppercase()),
            Style::default().fg(self.text_faint).add_modifier(Modifier::BOLD),
        )
    }

    /// Spinner glyph for the currently visible tick (10 frames @ 100 ms ≈ 1 s loop).
    pub fn spinner(&self, tick: u64) -> &'static str {
        const FRAMES: &[&str] = &["⠋","⠙","⠹","⠸","⠼","⠴","⠦","⠧","⠇","⠏"];
        FRAMES[(tick as usize) % FRAMES.len()]
    }
}
