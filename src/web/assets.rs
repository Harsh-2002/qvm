//! Static assets embedded at compile-time.

pub const STYLE_CSS: &str = include_str!("style.css");
pub const APP_JS:    &str = include_str!("app.js");
