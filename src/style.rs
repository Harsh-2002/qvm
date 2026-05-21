//! CLI output styling. Single source of truth for colours.
//!
//! Every `println!` / `eprintln!` in `commands::*` should run user-facing
//! text through one of the helpers below. Direct `owo_colors` imports
//! anywhere else are a smell.
//!
//! Rules:
//! - Honour `NO_COLOR` (env var) and detect non-TTY stdout — colors are
//!   stripped silently when piped or running under a dumb terminal.
//! - Match the TUI's Catppuccin Mocha palette so the CLI and TUI feel
//!   like the same product (ok=green, warn=yellow, err=red, accent=mauve).

use owo_colors::{OwoColorize, Style};
use std::io::IsTerminal;

/// True when stdout is a terminal AND `NO_COLOR` is not set. All public
/// helpers fall back to plain text when this is false, so output piped
/// to a file or another program stays clean.
pub fn color_enabled() -> bool {
    if std::env::var_os("NO_COLOR").is_some() { return false; }
    std::io::stdout().is_terminal()
}

// ── Catppuccin Mocha palette (RGB pinned, matches src/tui/theme.rs) ──

fn s_ok()     -> Style { Style::new().color(c_ok()).bold() }
fn s_err()    -> Style { Style::new().color(c_err()).bold() }
fn s_warn()   -> Style { Style::new().color(c_warn()) }
fn s_dim()    -> Style { Style::new().color(c_dim()) }
fn s_accent() -> Style { Style::new().color(c_accent()).bold() }
fn s_cmd()    -> Style { Style::new().color(c_accent()) }

fn c_ok()     -> owo_colors::Rgb { owo_colors::Rgb(0xa6, 0xe3, 0xa1) }
fn c_err()    -> owo_colors::Rgb { owo_colors::Rgb(0xf3, 0x8b, 0xa8) }
fn c_warn()   -> owo_colors::Rgb { owo_colors::Rgb(0xf9, 0xe2, 0xaf) }
fn c_dim()    -> owo_colors::Rgb { owo_colors::Rgb(0x6c, 0x70, 0x86) }
fn c_accent() -> owo_colors::Rgb { owo_colors::Rgb(0xcb, 0xa6, 0xf7) }

// ── Public helpers — apply a style if colour is enabled, else passthrough ──

macro_rules! styled_fn {
    ($name:ident, $style:expr) => {
        pub fn $name(text: impl AsRef<str>) -> String {
            if !color_enabled() { return text.as_ref().to_string(); }
            text.as_ref().style($style).to_string()
        }
    };
}

// Doc-comments on macro invocations are stripped by rustdoc; describe
// them in the module header instead. ok/err/warn/dim/accent are the
// semantic flavors; cmd is the code-span flavour; label is dim.
styled_fn!(ok,     s_ok());
styled_fn!(err,    s_err());
styled_fn!(warn,   s_warn());
styled_fn!(dim,    s_dim());
styled_fn!(accent, s_accent());
styled_fn!(cmd,    s_cmd());
pub fn label(text: impl AsRef<str>) -> String { dim(text) }

// ── State / inventory helpers ─────────────────────────────────────────

/// Glyph + colour for a libvirt domstate.
pub fn state_glyph(state: &str) -> &'static str {
    match state {
        "running" => "●",
        "paused"  => "◐",
        "crashed" => "✗",
        _         => "○",
    }
}

/// Colored `● running` / `○ shut off` / `✗ crashed` — used by `qvm ls`.
pub fn state_styled(state: &str) -> String {
    let g = state_glyph(state);
    let s = match state {
        "running" => s_ok(),
        "paused"  => Style::new().color(c_warn()),
        "crashed" => s_err(),
        _         => s_dim(),
    };
    if !color_enabled() { return format!("{g} {state}"); }
    format!("{} {}", g.style(s), state.style(s))
}

pub fn yes_no(b: bool) -> String {
    if !color_enabled() { return (if b { "yes" } else { "no" }).into(); }
    if b { "yes".style(s_ok()).to_string() }
    else { "no".style(s_err()).to_string() }
}

pub fn pulled_badge(pulled: bool) -> String {
    if pulled { ok("● pulled") } else { dim("○ not pulled") }
}

/// Convenience: `Error: <msg>` with the prefix red and bold.
pub fn error_prefix(msg: &str) -> String {
    format!("{} {msg}", err("Error:"))
}
