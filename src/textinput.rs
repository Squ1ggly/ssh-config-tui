//! A single-line text buffer for the editor's inline fields.
//!
//! Columns are character indices, not byte offsets, so values containing
//! non-ASCII (an IdentityFile under a unicode path, say) cannot panic on a
//! slice boundary.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Default)]
pub struct TextInput {
    pub text: String,
    pub col: usize,
}

impl TextInput {
    pub fn single(value: &str) -> Self {
        let mut input = TextInput {
            text: value.to_string(),
            col: 0,
        };
        input.col = input.char_len();
        input
    }

    pub fn value(&self) -> String {
        self.text.clone()
    }

    fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    /// Byte offset of character index `col`.
    fn byte_at(&self, col: usize) -> usize {
        self.text
            .char_indices()
            .nth(col)
            .map_or(self.text.len(), |(i, _)| i)
    }

    /// Handles a key press, returning false if it was not consumed so the
    /// caller can treat it as one of its own bindings.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char(c) if !ctrl => {
                let at = self.byte_at(self.col);
                self.text.insert(at, c);
                self.col += 1;
            }
            KeyCode::Char('a') if ctrl => self.col = 0,
            KeyCode::Char('e') if ctrl => self.col = self.char_len(),
            KeyCode::Char('u') if ctrl => {
                let at = self.byte_at(self.col);
                self.text.replace_range(..at, "");
                self.col = 0;
            }
            KeyCode::Char('k') if ctrl => {
                let at = self.byte_at(self.col);
                self.text.truncate(at);
            }
            KeyCode::Char('w') if ctrl => self.delete_word(),
            KeyCode::Backspace if self.col > 0 => {
                let at = self.byte_at(self.col - 1);
                self.text.remove(at);
                self.col -= 1;
            }
            KeyCode::Delete if self.col < self.char_len() => {
                let at = self.byte_at(self.col);
                self.text.remove(at);
            }
            KeyCode::Left if self.col > 0 => self.col -= 1,
            KeyCode::Right if self.col < self.char_len() => self.col += 1,
            KeyCode::Home => self.col = 0,
            KeyCode::End => self.col = self.char_len(),
            _ => return false,
        }
        true
    }

    fn delete_word(&mut self) {
        let chars: Vec<char> = self.text.chars().collect();
        let mut start = self.col;
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        let (from, to) = (self.byte_at(start), self.byte_at(self.col));
        self.text.replace_range(from..to, "");
        self.col = start;
    }
}
