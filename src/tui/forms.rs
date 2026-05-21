//! Minimal text-input helper for the create modal.
//!
//! We avoid `tui-input` to keep the dep tree narrow. This struct handles:
//! - Insert / Backspace / Delete
//! - Left / Right cursor movement
//! - Home / End
//!
//! It is *pure* — no rendering, no terminal I/O. `ui.rs` reads `value` and
//! `cursor` to render. `events.rs` calls the mutator methods.

#[derive(Debug, Clone, Default)]
pub struct TextInput {
    pub value:  String,
    pub cursor: usize,
}

impl TextInput {
    pub fn with_value(s: impl Into<String>) -> Self {
        let value = s.into();
        let cursor = value.chars().count();
        Self { value, cursor }
    }

    pub fn insert(&mut self, c: char) {
        // Operate on char boundaries via the cursor index in chars.
        let byte = self
            .value
            .char_indices()
            .nth(self.cursor)
            .map(|(b, _)| b)
            .unwrap_or(self.value.len());
        self.value.insert(byte, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let new_cursor = self.cursor - 1;
        let byte = self
            .value
            .char_indices()
            .nth(new_cursor)
            .map(|(b, _)| b)
            .unwrap_or(0);
        let next_byte = self
            .value
            .char_indices()
            .nth(self.cursor)
            .map(|(b, _)| b)
            .unwrap_or(self.value.len());
        self.value.replace_range(byte..next_byte, "");
        self.cursor = new_cursor;
    }

    pub fn delete(&mut self) {
        let len = self.value.chars().count();
        if self.cursor >= len {
            return;
        }
        let byte = self
            .value
            .char_indices()
            .nth(self.cursor)
            .map(|(b, _)| b)
            .unwrap_or(0);
        let next_byte = self
            .value
            .char_indices()
            .nth(self.cursor + 1)
            .map(|(b, _)| b)
            .unwrap_or(self.value.len());
        self.value.replace_range(byte..next_byte, "");
    }

    pub fn left(&mut self)  { if self.cursor > 0 { self.cursor -= 1; } }
    pub fn right(&mut self) {
        let len = self.value.chars().count();
        if self.cursor < len { self.cursor += 1; }
    }
    pub fn home(&mut self)  { self.cursor = 0; }
    pub fn end(&mut self)   { self.cursor = self.value.chars().count(); }

    pub fn as_str(&self) -> &str { &self.value }
    pub fn is_empty(&self) -> bool { self.value.trim().is_empty() }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }
}
