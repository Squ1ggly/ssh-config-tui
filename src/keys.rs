//! Telling a letter command apart from the same letter held with a modifier.
//!
//! crossterm reports Ctrl+D as `Char('d')` with CONTROL set, and some
//! terminals report Shift+D as `Char('d')` with SHIFT rather than `Char('D')`,
//! so matching on the key code alone reads both as a plain `d`.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// How many rows PageUp/PageDown and Ctrl+U/Ctrl+D move.
pub const PAGE: isize = 10;

/// The key as a command. A character held with Ctrl, Alt or another
/// modifier becomes `KeyCode::Null`, so it matches no letter command. Shift
/// is allowed because it's how `G`, `N` and `?` are typed, but Shift with a
/// lowercase letter is not that letter.
pub fn command(key: &KeyEvent) -> KeyCode {
    match key.code {
        KeyCode::Char(c) => {
            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
            let others = key.modifiers - KeyModifiers::SHIFT;
            if others.is_empty() && !(shift && c.is_lowercase()) {
                key.code
            } else {
                KeyCode::Null
            }
        }
        code => code,
    }
}

/// Whether this is Ctrl plus `c` and nothing else.
pub fn ctrl(key: &KeyEvent, c: char) -> bool {
    key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char(c)
}

/// The rows to move for a paging key (PageUp/PageDown, Ctrl+U/Ctrl+D).
pub fn page(key: &KeyEvent) -> Option<isize> {
    match key.code {
        KeyCode::PageDown => Some(PAGE),
        KeyCode::PageUp => Some(-PAGE),
        _ if ctrl(key, 'd') => Some(PAGE),
        _ if ctrl(key, 'u') => Some(-PAGE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    fn with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn modifiers_turn_letters_into_something_else() {
        let d = KeyCode::Char('d');
        assert_eq!(command(&with(d, KeyModifiers::NONE)), d);
        assert_eq!(command(&with(d, KeyModifiers::CONTROL)), KeyCode::Null);
        assert_eq!(command(&with(d, KeyModifiers::ALT)), KeyCode::Null);
        assert_eq!(command(&with(d, KeyModifiers::SHIFT)), KeyCode::Null);
        let upper = KeyCode::Char('D');
        assert_eq!(command(&with(upper, KeyModifiers::SHIFT)), upper);
        assert_eq!(command(&with(upper, KeyModifiers::NONE)), upper);
        let question = KeyCode::Char('?');
        assert_eq!(command(&with(question, KeyModifiers::SHIFT)), question);
        // Non-character keys keep their modifiers' meaning to the caller.
        let down = KeyCode::Down;
        assert_eq!(command(&with(down, KeyModifiers::CONTROL)), down);
    }

    #[test]
    fn ctrl_d_and_ctrl_u_page() {
        let ctrl_d = with(KeyCode::Char('d'), KeyModifiers::CONTROL);
        let ctrl_u = with(KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(page(&ctrl_d), Some(PAGE));
        assert_eq!(page(&ctrl_u), Some(-PAGE));
        assert_eq!(page(&with(KeyCode::Char('d'), KeyModifiers::NONE)), None);
        let ctrl_shift_d = with(
            KeyCode::Char('D'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert_eq!(page(&ctrl_shift_d), None);
    }
}
