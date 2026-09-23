//! The host editor: a list of rows you move through, rather than a fixed set of
//! text boxes. Forwards get a purpose-built form, every other ssh_config
//! keyword comes from the generated catalogue with a value editor that matches
//! its type, and anything we do not recognise stays editable as raw text.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{Block, split_directive};
use crate::forward::{Forward, ForwardKind, is_port};
use crate::schema::{self, Keyword, Kind};
use crate::textinput::TextInput;

/// Directives pinned to the top of the form, always visible even when unset.
pub const ESSENTIALS: [&str; 4] = ["HostName", "User", "Port", "IdentityFile"];

/// One line of a host body that is neither an essential nor a forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionRow {
    Directive { key: String, value: String },
    Comment(String),
}

/// A position in the form. Rebuilt each keystroke from the editor's contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Alias,
    Essential(usize),
    Forward(usize),
    AddForward,
    Option(usize),
    AddOption,
}

pub enum Overlay {
    /// Searchable list of every ssh_config keyword.
    Picker(Picker),
    /// One-of-N value list for flags and enums.
    Choice(Choice),
    /// The port-forward form.
    Forward(ForwardForm),
}

pub struct Editor {
    /// Index into `config.blocks`, or `None` when adding a host.
    pub target: Option<usize>,
    pub alias: String,
    pub essentials: [String; ESSENTIALS.len()],
    pub forwards: Vec<Forward>,
    pub options: Vec<OptionRow>,
    pub cursor: usize,
    /// Set while typing into the focused row.
    pub inline: Option<TextInput>,
    pub overlay: Option<Overlay>,
    pub error: Option<String>,
}

impl Editor {
    pub fn new(target: Option<usize>, block: &Block) -> Self {
        let mut editor = Editor {
            target,
            alias: block.patterns().join(" "),
            essentials: Default::default(),
            forwards: Vec::new(),
            options: Vec::new(),
            cursor: 0,
            inline: None,
            overlay: None,
            error: None,
        };

        for line in &block.body {
            let Some((key, value)) = split_directive(line) else {
                if !line.trim().is_empty() {
                    editor
                        .options
                        .push(OptionRow::Comment(line.trim().to_string()));
                }
                continue;
            };

            // Forwards we understand become structured rows; the rest stay text.
            if let Some(forward) = Forward::parse(key, value) {
                editor.forwards.push(forward);
                continue;
            }
            match ESSENTIALS.iter().position(|e| e.eq_ignore_ascii_case(key)) {
                Some(index) if editor.essentials[index].is_empty() => {
                    editor.essentials[index] = value.to_string()
                }
                _ => editor.options.push(OptionRow::Directive {
                    key: schema::canonical(key),
                    value: value.to_string(),
                }),
            }
        }
        editor
    }

    pub fn is_new(&self) -> bool {
        self.target.is_none()
    }

    /// The form's rows, in display order.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Alias];
        rows.extend((0..ESSENTIALS.len()).map(Row::Essential));
        rows.extend((0..self.forwards.len()).map(Row::Forward));
        rows.push(Row::AddForward);
        rows.extend((0..self.options.len()).map(Row::Option));
        rows.push(Row::AddOption);
        rows
    }

    pub fn current_row(&self) -> Row {
        let rows = self.rows();
        rows[self.cursor.min(rows.len() - 1)]
    }

    /// The value behind a row, for display and for seeding an inline edit.
    pub fn row_value(&self, row: Row) -> String {
        match row {
            Row::Alias => self.alias.clone(),
            Row::Essential(i) => self.essentials[i].clone(),
            Row::Option(i) => match &self.options[i] {
                OptionRow::Directive { value, .. } => value.clone(),
                OptionRow::Comment(text) => text.clone(),
            },
            _ => String::new(),
        }
    }

    pub fn row_label(&self, row: Row) -> String {
        match row {
            Row::Alias => "Host".to_string(),
            Row::Essential(i) => ESSENTIALS[i].to_string(),
            Row::Option(i) => match &self.options[i] {
                OptionRow::Directive { key, .. } => key.clone(),
                OptionRow::Comment(_) => "#".to_string(),
            },
            Row::Forward(i) => self.forwards[i].kind.label().to_string(),
            Row::AddForward => "+ add a port forward".to_string(),
            Row::AddOption => "+ add an option".to_string(),
        }
    }

    /// The catalogue entry for a row, when it maps to a known keyword.
    fn keyword_of(&self, row: Row) -> Option<&'static Keyword> {
        match row {
            Row::Essential(i) => schema::lookup(ESSENTIALS[i]),
            Row::Option(i) => match &self.options[i] {
                OptionRow::Directive { key, .. } => schema::lookup(key),
                OptionRow::Comment(_) => None,
            },
            _ => None,
        }
    }

    pub fn help_for_cursor(&self) -> Option<&'static Keyword> {
        self.keyword_of(self.current_row())
    }

    fn move_cursor(&mut self, delta: isize) {
        let len = self.rows().len() as isize;
        self.cursor = (self.cursor as isize + delta).rem_euclid(len) as usize;
    }

    fn focus(&mut self, row: Row) {
        if let Some(index) = self.rows().iter().position(|r| *r == row) {
            self.cursor = index;
        }
    }

    /// Handles a key. Returns `Some(true)` to save, `Some(false)` to cancel.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<bool> {
        self.error = None;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            return Some(true);
        }
        if self.overlay.is_some() {
            self.handle_overlay(key);
            return None;
        }
        if self.inline.is_some() {
            self.handle_inline(key);
            return None;
        }

        match key.code {
            KeyCode::Esc => return Some(false),
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::Enter => self.activate(),
            KeyCode::Char(' ') => self.toggle_or_activate(),
            KeyCode::Char('d') | KeyCode::Delete => self.delete_row(),
            _ => {}
        }
        None
    }

    /// Enter on the focused row: edit it, or open the right dialog.
    fn activate(&mut self) {
        let row = self.current_row();
        match row {
            Row::AddOption => {
                self.overlay = Some(Overlay::Picker(Picker::new()));
            }
            Row::AddForward => {
                self.overlay = Some(Overlay::Forward(ForwardForm::new(
                    None,
                    Forward::new(ForwardKind::Local),
                )));
            }
            Row::Forward(i) => {
                self.overlay = Some(Overlay::Forward(ForwardForm::new(
                    Some(i),
                    self.forwards[i].clone(),
                )));
            }
            _ => match self.keyword_of(row).map(|k| k.kind) {
                // Fixed-value options get a list instead of a text box, so you
                // never have to remember whether it is "ask" or "confirm".
                Some(Kind::Flag) | Some(Kind::Choice) => {
                    let keyword = self.keyword_of(row).unwrap();
                    self.overlay = Some(Overlay::Choice(Choice::new(keyword, row)));
                }
                _ => self.inline = Some(TextInput::single(&self.row_value(row))),
            },
        }
    }

    /// Space flips a yes/no straight away rather than opening a list.
    fn toggle_or_activate(&mut self) {
        let row = self.current_row();
        // Some yes/no options are spelled as two-value enums in ssh's parser;
        // both should toggle.
        if self
            .keyword_of(row)
            .is_some_and(|k| k.values == ["yes", "no"])
        {
            let next = if self.row_value(row).trim().eq_ignore_ascii_case("yes") {
                "no"
            } else {
                "yes"
            };
            self.set_row_value(row, next.to_string());
            return;
        }
        self.activate();
    }

    fn set_row_value(&mut self, row: Row, value: String) {
        match row {
            Row::Alias => self.alias = value,
            Row::Essential(i) => self.essentials[i] = value,
            Row::Option(i) => match &mut self.options[i] {
                OptionRow::Directive { value: slot, .. } => *slot = value,
                OptionRow::Comment(text) => *text = value,
            },
            _ => {}
        }
    }

    fn delete_row(&mut self) {
        match self.current_row() {
            Row::Essential(i) => self.essentials[i].clear(),
            Row::Forward(i) => {
                self.forwards.remove(i);
                self.cursor = self
                    .cursor
                    .saturating_sub(usize::from(i == self.forwards.len()));
            }
            Row::Option(i) => {
                self.options.remove(i);
                self.cursor = self
                    .cursor
                    .saturating_sub(usize::from(i == self.options.len()));
            }
            _ => {}
        }
    }

    fn handle_inline(&mut self, key: KeyEvent) {
        let Some(input) = self.inline.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Enter => {
                let value = input.value().trim().to_string();
                self.inline = None;
                let row = self.current_row();
                // An option left empty was never really added.
                if let (true, Row::Option(i)) = (value.is_empty(), row)
                    && matches!(&self.options[i], OptionRow::Directive { .. })
                {
                    self.options.remove(i);
                    self.cursor = self.cursor.saturating_sub(1);
                    return;
                }
                self.set_row_value(row, value);
            }
            KeyCode::Esc => {
                self.inline = None;
                // Cancelling a brand-new option removes the placeholder row.
                let row = self.current_row();
                let placeholder = OptionRow::Directive {
                    key: self.row_label(row),
                    value: String::new(),
                };
                if let Row::Option(i) = row
                    && self.options[i] == placeholder
                {
                    self.options.remove(i);
                    self.cursor = self.cursor.saturating_sub(1);
                }
            }
            _ => {
                input.handle_key(key);
            }
        }
    }

    fn handle_overlay(&mut self, key: KeyEvent) {
        match self.overlay.as_mut() {
            Some(Overlay::Picker(picker)) => match picker.handle_key(key) {
                PickerResult::Cancel => self.overlay = None,
                PickerResult::Pick(keyword) => {
                    self.overlay = None;
                    self.add_keyword(keyword);
                }
                PickerResult::Continue => {}
            },
            Some(Overlay::Choice(choice)) => match key.code {
                KeyCode::Esc => self.overlay = None,
                KeyCode::Down | KeyCode::Char('j') => choice.move_by(1),
                KeyCode::Up | KeyCode::Char('k') => choice.move_by(-1),
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let Some(Overlay::Choice(choice)) = self.overlay.take() else {
                        return;
                    };
                    let value = choice.values[choice.index].to_string();
                    match choice.row {
                        Some(row) => self.set_row_value(row, value),
                        None => self.push_option(choice.keyword, value),
                    }
                }
                _ => {}
            },
            Some(Overlay::Forward(form)) => match form.handle_key(key) {
                ForwardResult::Cancel => self.overlay = None,
                ForwardResult::Save => {
                    let Some(Overlay::Forward(form)) = self.overlay.take() else {
                        return;
                    };
                    match form.index {
                        Some(index) => self.forwards[index] = form.forward,
                        None => {
                            self.forwards.push(form.forward);
                            self.focus(Row::Forward(self.forwards.len() - 1));
                        }
                    }
                }
                ForwardResult::Continue => {}
            },
            None => {}
        }
    }

    /// Adds a keyword picked from the catalogue, routing it to whichever
    /// editor suits it.
    fn add_keyword(&mut self, keyword: &'static Keyword) {
        if let Some(kind) = ForwardKind::from_keyword(keyword.name) {
            self.overlay = Some(Overlay::Forward(ForwardForm::new(None, Forward::new(kind))));
            return;
        }
        if let Some(index) = ESSENTIALS.iter().position(|e| *e == keyword.name) {
            self.focus(Row::Essential(index));
            self.activate();
            return;
        }
        // Setting a non-repeatable option twice would be ignored by ssh, so
        // jump to the existing one instead of adding a second.
        if !keyword.repeatable {
            let existing = self.options.iter().position(|row| {
                matches!(row, OptionRow::Directive { key, .. } if key.eq_ignore_ascii_case(keyword.name))
            });
            if let Some(index) = existing {
                self.focus(Row::Option(index));
                self.activate();
                return;
            }
        }
        match keyword.kind {
            Kind::Flag | Kind::Choice => {
                self.overlay = Some(Overlay::Choice(Choice::for_new(keyword)));
            }
            _ => {
                self.push_option(keyword.name, String::new());
                self.inline = Some(TextInput::single(""));
            }
        }
    }

    fn push_option(&mut self, key: &str, value: String) {
        self.options.push(OptionRow::Directive {
            key: key.to_string(),
            value,
        });
        self.focus(Row::Option(self.options.len() - 1));
    }

    /// Validates and renders the editor back into a config block.
    /// The body is written essentials first, then forwards, then everything
    /// else in the order it was already in.
    pub fn to_block(&self, existing: Option<&Block>) -> Result<Block, String> {
        let alias = self.alias.trim();
        if alias.is_empty() {
            return Err("Host alias can't be empty".into());
        }
        let port = self.essentials[2].trim();
        if !port.is_empty() && !is_port(port) {
            return Err(format!(
                "Port must be a number between 1 and 65535, not '{port}'"
            ));
        }
        if let Some(bad) = self.forwards.iter().find(|f| !f.is_complete()) {
            return Err(format!("Forward '{}' is incomplete", bad.describe()));
        }

        let mut block = match existing {
            Some(block) => block.clone(),
            None => Block::new_host(""),
        };
        block.set_patterns(alias);

        let mut lines: Vec<String> = Vec::new();
        for (index, key) in ESSENTIALS.iter().enumerate() {
            let value = self.essentials[index].trim();
            if !value.is_empty() {
                lines.push(format!("{key} {value}"));
            }
        }
        lines.extend(self.forwards.iter().map(Forward::line));
        for option in &self.options {
            match option {
                OptionRow::Directive { key, value } if !value.trim().is_empty() => {
                    lines.push(format!("{key} {}", value.trim()))
                }
                OptionRow::Directive { .. } => {}
                OptionRow::Comment(text) => lines.push(text.clone()),
            }
        }
        block.set_body(&lines);
        Ok(block)
    }
}

/// Searchable list of every ssh_config keyword.
pub struct Picker {
    pub query: TextInput,
    pub index: usize,
}

pub enum PickerResult {
    Continue,
    Cancel,
    Pick(&'static Keyword),
}

impl Picker {
    fn new() -> Self {
        Picker {
            query: TextInput::single(""),
            index: 0,
        }
    }

    /// Matches on name first, then description, so typing "forward" finds
    /// LocalForward but also ExitOnForwardFailure; common options float up.
    pub fn matches(&self) -> Vec<&'static Keyword> {
        let query = self.query.value().trim().to_lowercase();
        let mut matches: Vec<&'static Keyword> = schema::KEYWORDS
            .iter()
            .filter(|k| {
                query.is_empty()
                    || k.name.to_lowercase().contains(&query)
                    || k.desc.to_lowercase().contains(&query)
                    || k.aliases.contains(&query)
            })
            .collect();
        matches.sort_by_key(|k| {
            let name = k.name.to_lowercase();
            (
                !query.is_empty() && !name.starts_with(&query),
                !query.is_empty() && !name.contains(&query),
                !k.common,
                name,
            )
        });
        matches
    }

    pub fn selected(&self) -> Option<&'static Keyword> {
        self.matches().get(self.index).copied()
    }

    fn handle_key(&mut self, key: KeyEvent) -> PickerResult {
        match key.code {
            KeyCode::Esc => return PickerResult::Cancel,
            KeyCode::Enter => {
                if let Some(keyword) = self.selected() {
                    return PickerResult::Pick(keyword);
                }
            }
            KeyCode::Down => self.index += 1,
            KeyCode::Up => self.index = self.index.saturating_sub(1),
            _ => {
                if self.query.handle_key(key) {
                    self.index = 0;
                }
            }
        }
        let len = self.matches().len();
        self.index = self.index.min(len.saturating_sub(1));
        PickerResult::Continue
    }
}

/// A one-of-N value list for flags and enums.
pub struct Choice {
    pub keyword: &'static str,
    pub desc: &'static str,
    pub default: Option<&'static str>,
    pub values: &'static [&'static str],
    pub index: usize,
    /// The row being changed, or `None` when the option is being added.
    pub row: Option<Row>,
}

impl Choice {
    fn new(keyword: &'static Keyword, row: Row) -> Self {
        Choice {
            keyword: keyword.name,
            desc: keyword.desc,
            default: keyword.default,
            values: keyword.values,
            index: 0,
            row: Some(row),
        }
    }

    fn for_new(keyword: &'static Keyword) -> Self {
        Choice {
            row: None,
            ..Choice::new(keyword, Row::Alias)
        }
    }

    fn move_by(&mut self, delta: isize) {
        let len = self.values.len() as isize;
        if len > 0 {
            self.index = (self.index as isize + delta).rem_euclid(len) as usize;
        }
    }
}

/// The port-forward form: pick a direction, then fill in the ports.
pub struct ForwardForm {
    pub index: Option<usize>,
    pub forward: Forward,
    /// 0 = direction, then bind / listen / host / port.
    pub field: usize,
    pub inputs: [TextInput; 4],
    pub error: Option<String>,
}

pub enum ForwardResult {
    Continue,
    Cancel,
    Save,
}

impl ForwardForm {
    pub const FIELDS: [&'static str; 4] = ["Bind address (optional)", "Port", "Host", "Port"];

    fn new(index: Option<usize>, forward: Forward) -> Self {
        let inputs = [
            TextInput::single(&forward.bind),
            TextInput::single(&forward.listen),
            TextInput::single(&forward.host),
            TextInput::single(&forward.port),
        ];
        ForwardForm {
            index,
            forward,
            field: 0,
            inputs,
            error: None,
        }
    }

    /// Dynamic forwards have no destination, so those fields are hidden.
    pub fn visible_fields(&self) -> usize {
        if self.forward.kind == ForwardKind::Dynamic {
            2
        } else {
            4
        }
    }

    fn pull(&mut self) {
        self.forward.bind = self.inputs[0].value().trim().to_string();
        self.forward.listen = self.inputs[1].value().trim().to_string();
        self.forward.host = self.inputs[2].value().trim().to_string();
        self.forward.port = self.inputs[3].value().trim().to_string();
    }

    fn handle_key(&mut self, key: KeyEvent) -> ForwardResult {
        self.error = None;
        let last = self.visible_fields();
        match key.code {
            KeyCode::Esc => return ForwardResult::Cancel,
            KeyCode::Tab | KeyCode::Down => self.field = (self.field + 1) % (last + 1),
            KeyCode::BackTab | KeyCode::Up => self.field = (self.field + last) % (last + 1),
            KeyCode::Enter => {
                self.pull();
                if self.forward.is_complete() {
                    return ForwardResult::Save;
                }
                // Enter on an unfinished forward just moves on.
                if self.field < last {
                    self.field += 1;
                } else {
                    self.error = Some(match self.forward.kind {
                        ForwardKind::Dynamic => "A listening port is required".into(),
                        _ => "A listening port, a host and a destination port are required".into(),
                    });
                }
            }
            KeyCode::Left | KeyCode::Right if self.field == 0 => {
                let step = if key.code == KeyCode::Left { 2 } else { 1 };
                let current = ForwardKind::ALL
                    .iter()
                    .position(|k| *k == self.forward.kind)
                    .unwrap_or(0);
                self.forward.kind = ForwardKind::ALL[(current + step) % 3];
                self.field = 0;
            }
            _ if self.field > 0 => {
                self.inputs[self.field - 1].handle_key(key);
                self.pull();
            }
            _ => {}
        }
        ForwardResult::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SshConfig;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};
    use std::path::PathBuf;

    const NAS: &str = "\
Host nas
    HostName 10.0.0.4
    User st
    # a note about this box
    LocalForward 8081 127.0.0.1:8081
    DynamicForward 1080
    ServerAliveInterval 60
    compression yes
    LocalForward /tmp/mysql.sock /var/run/mysqld.sock
";

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn block(text: &str) -> Block {
        SshConfig::parse(PathBuf::from("/tmp/c"), text)
            .blocks
            .remove(0)
    }

    fn open(text: &str) -> Editor {
        Editor::new(Some(0), &block(text))
    }

    fn press(editor: &mut Editor, keys: &[KeyCode]) {
        for code in keys {
            editor.handle_key(key(*code));
        }
    }

    fn type_text(editor: &mut Editor, text: &str) {
        for c in text.chars() {
            editor.handle_key(key(KeyCode::Char(c)));
        }
    }

    /// Moves the cursor onto the first row matching `predicate`.
    fn focus_row(editor: &mut Editor, predicate: impl Fn(&Editor, Row) -> bool) {
        let rows = editor.rows();
        editor.cursor = rows
            .iter()
            .position(|row| predicate(editor, *row))
            .expect("no such row");
    }

    #[test]
    fn splits_a_host_into_essentials_forwards_options_and_comments() {
        let editor = open(NAS);
        assert_eq!(editor.alias, "nas");
        assert_eq!(editor.essentials[0], "10.0.0.4");
        assert_eq!(editor.essentials[1], "st");
        assert_eq!(editor.essentials[2], "", "no port set");

        assert_eq!(editor.forwards.len(), 2);
        assert_eq!(editor.forwards[0].describe(), "8081 → 127.0.0.1:8081");
        assert_eq!(editor.forwards[1].kind, ForwardKind::Dynamic);

        assert_eq!(
            editor.options,
            vec![
                OptionRow::Comment("# a note about this box".into()),
                OptionRow::Directive {
                    key: "ServerAliveInterval".into(),
                    value: "60".into()
                },
                // Casing is normalised to the catalogue's spelling.
                OptionRow::Directive {
                    key: "Compression".into(),
                    value: "yes".into()
                },
                // A forward we can't model stays editable as raw text.
                OptionRow::Directive {
                    key: "LocalForward".into(),
                    value: "/tmp/mysql.sock /var/run/mysqld.sock".into()
                },
            ]
        );
    }

    #[test]
    fn writes_essentials_then_forwards_then_everything_else() {
        let editor = open(NAS);
        let rendered = editor.to_block(Some(&block(NAS))).unwrap();
        assert_eq!(
            rendered.body,
            [
                "    HostName 10.0.0.4",
                "    User st",
                "    LocalForward 8081 127.0.0.1:8081",
                "    DynamicForward 1080",
                "    # a note about this box",
                "    ServerAliveInterval 60",
                "    Compression yes",
                "    LocalForward /tmp/mysql.sock /var/run/mysqld.sock",
            ]
        );
    }

    #[test]
    fn keeps_extra_copies_of_a_repeatable_essential() {
        let editor = open("Host a\n    IdentityFile ~/.ssh/one\n    IdentityFile ~/.ssh/two\n");
        assert_eq!(editor.essentials[3], "~/.ssh/one");
        assert_eq!(
            editor.options,
            vec![OptionRow::Directive {
                key: "IdentityFile".into(),
                value: "~/.ssh/two".into()
            }]
        );
        let rendered = editor.to_block(None).unwrap();
        assert_eq!(
            rendered.body,
            ["    IdentityFile ~/.ssh/one", "    IdentityFile ~/.ssh/two"]
        );
    }

    #[test]
    fn typing_into_a_row_updates_it() {
        let mut editor = open(NAS);
        focus_row(&mut editor, |_, row| row == Row::Essential(0));
        press(&mut editor, &[KeyCode::Enter]);
        press(&mut editor, &[KeyCode::Char('u')]); // plain char, not ctrl
        assert!(editor.inline.is_some());
        press(&mut editor, &[KeyCode::Esc]);
        assert_eq!(editor.essentials[0], "10.0.0.4", "esc discards");

        press(&mut editor, &[KeyCode::Enter]);
        for _ in 0..8 {
            press(&mut editor, &[KeyCode::Backspace]);
        }
        type_text(&mut editor, "10.0.0.9");
        press(&mut editor, &[KeyCode::Enter]);
        assert_eq!(editor.essentials[0], "10.0.0.9");
    }

    #[test]
    fn space_toggles_a_yes_no_option_without_typing() {
        let mut editor = open(NAS);
        focus_row(&mut editor, |e, row| e.row_label(row) == "Compression");
        press(&mut editor, &[KeyCode::Char(' ')]);
        assert_eq!(editor.row_value(editor.current_row()), "no");
        press(&mut editor, &[KeyCode::Char(' ')]);
        assert_eq!(editor.row_value(editor.current_row()), "yes");
    }

    #[test]
    fn a_choice_option_opens_a_value_list_rather_than_a_text_box() {
        let mut editor = open("Host a\n    StrictHostKeyChecking ask\n");
        focus_row(&mut editor, |e, row| {
            e.row_label(row) == "StrictHostKeyChecking"
        });
        press(&mut editor, &[KeyCode::Enter]);

        let Some(Overlay::Choice(choice)) = editor.overlay.as_ref() else {
            panic!("expected a choice list");
        };
        assert_eq!(choice.values, ["yes", "no", "ask", "off", "accept-new"]);
        assert_eq!(choice.default, Some("ask"));

        press(
            &mut editor,
            &[KeyCode::Down, KeyCode::Down, KeyCode::Down, KeyCode::Down],
        );
        press(&mut editor, &[KeyCode::Enter]);
        assert!(editor.overlay.is_none());
        assert_eq!(editor.row_value(editor.current_row()), "accept-new");
    }

    #[test]
    fn the_picker_finds_options_by_name_and_by_description() {
        let mut picker = Picker::new();
        assert!(
            picker.matches().len() > 90,
            "everything is listed by default"
        );
        assert!(picker.matches()[0].common, "common options come first");

        for c in "control".chars() {
            picker.query.handle_key(key(KeyCode::Char(c)));
        }
        let names: Vec<&str> = picker.matches().iter().map(|k| k.name).collect();
        assert_eq!(
            &names[..3],
            ["ControlMaster", "ControlPath", "ControlPersist"]
        );

        // You can also find an option by what it does, not just what it's called.
        let mut picker = Picker::new();
        for c in "socks".chars() {
            picker.query.handle_key(key(KeyCode::Char(c)));
        }
        let names: Vec<&str> = picker.matches().iter().map(|k| k.name).collect();
        assert!(names.contains(&"DynamicForward"), "{names:?}");

        // "my connection keeps dropping when idle" -> ServerAliveInterval
        let mut picker = Picker::new();
        for c in "idle".chars() {
            picker.query.handle_key(key(KeyCode::Char(c)));
        }
        let names: Vec<&str> = picker.matches().iter().map(|k| k.name).collect();
        assert!(names.contains(&"ServerAliveInterval"), "{names:?}");
    }

    #[test]
    fn picking_a_text_option_adds_it_and_starts_typing() {
        let mut editor = open("Host a\n");
        focus_row(&mut editor, |_, row| row == Row::AddOption);
        press(&mut editor, &[KeyCode::Enter]);
        type_text(&mut editor, "RemoteCommand");
        press(&mut editor, &[KeyCode::Enter]);

        assert!(editor.overlay.is_none());
        assert!(editor.inline.is_some(), "focus lands in the value");
        type_text(&mut editor, "tmux attach");
        press(&mut editor, &[KeyCode::Enter]);
        assert_eq!(
            editor.options,
            vec![OptionRow::Directive {
                key: "RemoteCommand".into(),
                value: "tmux attach".into()
            }]
        );
    }

    #[test]
    fn picking_a_flag_goes_straight_to_its_values() {
        let mut editor = open("Host a\n");
        focus_row(&mut editor, |_, row| row == Row::AddOption);
        press(&mut editor, &[KeyCode::Enter]);
        type_text(&mut editor, "ForwardX11");
        press(&mut editor, &[KeyCode::Enter]);
        assert!(matches!(editor.overlay, Some(Overlay::Choice(_))));
        press(&mut editor, &[KeyCode::Enter]);
        assert_eq!(
            editor.options,
            vec![OptionRow::Directive {
                key: "ForwardX11".into(),
                value: "yes".into()
            }]
        );
    }

    #[test]
    fn picking_an_option_that_is_already_set_jumps_to_it() {
        let mut editor = open(NAS);
        focus_row(&mut editor, |_, row| row == Row::AddOption);
        press(&mut editor, &[KeyCode::Enter]);
        type_text(&mut editor, "ServerAliveInterval");
        press(&mut editor, &[KeyCode::Enter]);

        assert_eq!(editor.options.len(), 4, "no duplicate was added");
        assert_eq!(
            editor.row_label(editor.current_row()),
            "ServerAliveInterval"
        );
        assert!(editor.inline.is_some());
    }

    #[test]
    fn picking_an_essential_jumps_to_its_pinned_row() {
        let mut editor = open("Host a\n");
        focus_row(&mut editor, |_, row| row == Row::AddOption);
        press(&mut editor, &[KeyCode::Enter]);
        type_text(&mut editor, "User");
        press(&mut editor, &[KeyCode::Enter]);
        assert_eq!(editor.current_row(), Row::Essential(1));
        assert!(editor.inline.is_some());
    }

    #[test]
    fn picking_a_forward_keyword_opens_the_forward_form() {
        let mut editor = open("Host a\n");
        focus_row(&mut editor, |_, row| row == Row::AddOption);
        press(&mut editor, &[KeyCode::Enter]);
        type_text(&mut editor, "RemoteForward");
        press(&mut editor, &[KeyCode::Enter]);

        let Some(Overlay::Forward(form)) = editor.overlay.as_ref() else {
            panic!("expected the forward form");
        };
        assert_eq!(form.forward.kind, ForwardKind::Remote);
    }

    #[test]
    fn adding_a_forward_through_the_form() {
        let mut editor = open("Host a\n");
        focus_row(&mut editor, |_, row| row == Row::AddForward);
        press(&mut editor, &[KeyCode::Enter]);

        press(&mut editor, &[KeyCode::Down]); // into the bind field
        press(&mut editor, &[KeyCode::Down]); // listen port
        type_text(&mut editor, "5432");
        press(&mut editor, &[KeyCode::Down]);
        for _ in 0.."localhost".len() {
            press(&mut editor, &[KeyCode::Backspace]);
        }
        type_text(&mut editor, "db.internal");
        press(&mut editor, &[KeyCode::Down]);
        type_text(&mut editor, "5432");
        press(&mut editor, &[KeyCode::Enter]);

        assert!(editor.overlay.is_none());
        assert_eq!(editor.forwards.len(), 1);
        assert_eq!(
            editor.forwards[0].line(),
            "LocalForward 5432 db.internal:5432"
        );
        assert_eq!(
            editor.current_row(),
            Row::Forward(0),
            "cursor follows the new forward"
        );
    }

    #[test]
    fn a_dynamic_forward_only_asks_for_a_port() {
        let mut editor = open("Host a\n");
        focus_row(&mut editor, |_, row| row == Row::AddForward);
        press(&mut editor, &[KeyCode::Enter]);
        // local -> remote -> dynamic
        press(&mut editor, &[KeyCode::Right, KeyCode::Right]);

        let Some(Overlay::Forward(form)) = editor.overlay.as_ref() else {
            panic!()
        };
        assert_eq!(form.forward.kind, ForwardKind::Dynamic);
        assert_eq!(form.visible_fields(), 2, "no destination fields");

        press(&mut editor, &[KeyCode::Down, KeyCode::Down]);
        type_text(&mut editor, "1080");
        press(&mut editor, &[KeyCode::Enter]);
        assert_eq!(editor.forwards[0].line(), "DynamicForward 1080");
    }

    #[test]
    fn an_incomplete_forward_is_not_saved_by_enter() {
        let mut editor = open("Host a\n");
        focus_row(&mut editor, |_, row| row == Row::AddForward);
        press(&mut editor, &[KeyCode::Enter]);
        // Enter walks the fields, and complains once there is nowhere left to go.
        for _ in 0..5 {
            press(&mut editor, &[KeyCode::Enter]);
        }

        let Some(Overlay::Forward(form)) = editor.overlay.as_ref() else {
            panic!("form closed")
        };
        assert!(form.error.is_some(), "the form explains what is missing");
        assert!(editor.forwards.is_empty());
    }

    #[test]
    fn editing_an_existing_forward_keeps_its_place() {
        let mut editor = open(NAS);
        focus_row(&mut editor, |_, row| row == Row::Forward(0));
        press(&mut editor, &[KeyCode::Enter]);
        press(&mut editor, &[KeyCode::Down]); // direction -> bind address
        type_text(&mut editor, "127.0.0.1");
        press(&mut editor, &[KeyCode::Enter]);

        assert_eq!(editor.forwards.len(), 2);
        assert_eq!(
            editor.forwards[0].line(),
            "LocalForward 127.0.0.1:8081 127.0.0.1:8081"
        );
    }

    #[test]
    fn removing_rows() {
        let mut editor = open(NAS);
        focus_row(&mut editor, |_, row| row == Row::Essential(1));
        press(&mut editor, &[KeyCode::Char('d')]);
        assert_eq!(editor.essentials[1], "");

        focus_row(&mut editor, |_, row| row == Row::Forward(1));
        press(&mut editor, &[KeyCode::Char('d')]);
        assert_eq!(editor.forwards.len(), 1);

        focus_row(&mut editor, |e, row| e.row_label(row) == "#");
        press(&mut editor, &[KeyCode::Char('d')]);
        assert!(
            !editor
                .options
                .iter()
                .any(|o| matches!(o, OptionRow::Comment(_)))
        );
    }

    #[test]
    fn validation_rejects_bad_input_with_a_specific_message() {
        let mut editor = open("Host a\n");
        editor.alias = String::new();
        assert_eq!(
            editor.to_block(None).unwrap_err(),
            "Host alias can't be empty"
        );

        let mut editor = open("Host a\n");
        editor.essentials[2] = "http".into();
        assert!(
            editor
                .to_block(None)
                .unwrap_err()
                .contains("Port must be a number")
        );

        let mut editor = open("Host a\n");
        editor.forwards.push(Forward::new(ForwardKind::Local));
        assert!(editor.to_block(None).unwrap_err().contains("incomplete"));
    }

    #[test]
    fn ctrl_s_saves_and_esc_cancels_from_anywhere() {
        let mut editor = open(NAS);
        let ctrl_s = KeyEvent {
            code: KeyCode::Char('s'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert_eq!(editor.handle_key(ctrl_s), Some(true));
        assert_eq!(editor.handle_key(key(KeyCode::Esc)), Some(false));

        // ...but esc inside an overlay only closes the overlay.
        focus_row(&mut editor, |_, row| row == Row::AddOption);
        press(&mut editor, &[KeyCode::Enter]);
        assert_eq!(editor.handle_key(key(KeyCode::Esc)), None);
        assert!(editor.overlay.is_none());
    }
}

/// Checked against the caller's own config with
/// `cargo test -- --ignored real_hosts`.
#[cfg(test)]
mod real_hosts {
    use super::*;
    use crate::config::{default_path, BlockKind, SshConfig};

    /// Loading every host into the editor and saving it straight back must not
    /// add, drop or alter a single directive — only regroup them.
    #[test]
    #[ignore = "reads the developer's own ~/.ssh/config"]
    fn survive_a_round_trip_through_the_editor() {
        let path = default_path();
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        let config = SshConfig::parse(path, &text);

        for block in config.blocks.iter().filter(|b| b.kind == BlockKind::Host) {
            let editor = Editor::new(Some(0), block);
            let rendered = editor.to_block(Some(block)).expect("should be valid");

            let normalise = |block: &Block| {
                let mut lines: Vec<String> = block
                    .body
                    .iter()
                    .filter(|l| !l.trim().is_empty())
                    .map(|line| match split_directive(line) {
                        Some((key, value)) => format!("{} {value}", key.to_lowercase()),
                        None => line.trim().to_string(),
                    })
                    .collect();
                lines.sort();
                lines
            };

            assert_eq!(
                normalise(block),
                normalise(&rendered),
                "host '{}' changed when edited",
                block.label()
            );
            assert_eq!(block.patterns(), rendered.patterns());
        }
    }
}
