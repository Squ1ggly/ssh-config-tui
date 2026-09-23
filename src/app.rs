//! Application state and key handling. Rendering lives in `ui`.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;

use crate::config::{Block, BlockKind, SshConfig};
use crate::editor::Editor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    List,
    Search,
    Edit,
    ConfirmDelete,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Error,
}

pub struct App {
    pub config: SshConfig,
    pub mode: Mode,
    pub filter: String,
    pub list_state: ListState,
    pub editor: Option<Editor>,
    pub status: Option<(String, StatusKind)>,
    pub should_quit: bool,
    /// Set when the user asks to connect; `main` drains this, drops out of the
    /// alternate screen and runs ssh.
    pub connect_to: Option<String>,
}

impl App {
    pub fn new(config: SshConfig) -> Self {
        let mut app = App {
            config,
            mode: Mode::List,
            filter: String::new(),
            list_state: ListState::default(),
            editor: None,
            status: None,
            should_quit: false,
            connect_to: None,
        };
        app.list_state.select(if app.config.blocks.is_empty() {
            None
        } else {
            Some(0)
        });
        app
    }

    /// Indices of blocks matching the current filter, in file order.
    pub fn visible(&self) -> Vec<usize> {
        let needle = self.filter.to_lowercase();
        self.config
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| {
                if needle.is_empty() {
                    return true;
                }
                let haystack = format!(
                    "{} {} {}",
                    block.header,
                    block.get("HostName").unwrap_or_default(),
                    block.get("User").unwrap_or_default()
                );
                haystack.to_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Index into `config.blocks` of the highlighted row.
    pub fn selected_block(&self) -> Option<usize> {
        let visible = self.visible();
        self.list_state
            .selected()
            .and_then(|i| visible.get(i).copied())
    }

    pub fn selected(&self) -> Option<&Block> {
        self.selected_block().map(|i| &self.config.blocks[i])
    }

    fn info(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), StatusKind::Info));
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), StatusKind::Error));
    }

    fn clamp_selection(&mut self) {
        let len = self.visible().len();
        if len == 0 {
            self.list_state.select(None);
        } else {
            let current = self.list_state.selected().unwrap_or(0);
            self.list_state.select(Some(current.min(len - 1)));
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.visible().len();
        if len == 0 {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(len as isize) as usize;
        self.list_state.select(Some(next));
    }

    fn save(&mut self) {
        match self.config.save() {
            Ok(()) => {
                let backup = self.config.backup_path();
                let name = backup
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                self.info(format!(
                    "Saved {} (backup: {name})",
                    self.config.path.display()
                ));
            }
            Err(e) => self.error(format!("Could not save: {e}")),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        // Windows terminals deliver both press and release; ignore the latter.
        if key.kind == KeyEventKind::Release {
            return;
        }
        match self.mode {
            Mode::List => self.on_key_list(key),
            Mode::Search => self.on_key_search(key),
            Mode::Edit => self.on_key_edit(key),
            Mode::ConfirmDelete => self.on_key_confirm(key),
            Mode::Help => {
                self.mode = Mode::List;
            }
        }
    }

    fn on_key_list(&mut self, key: KeyEvent) {
        self.status = None;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('g') | KeyCode::Home => self.list_state.select_first(),
            KeyCode::Char('G') | KeyCode::End => {
                let len = self.visible().len();
                if len > 0 {
                    self.list_state.select(Some(len - 1));
                }
            }
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::Char('/') => {
                self.mode = Mode::Search;
            }
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('a') => self.begin_add(),
            KeyCode::Char('e') | KeyCode::Enter => self.begin_edit(),
            KeyCode::Char('y') => self.duplicate(),
            KeyCode::Char('d') => {
                if self.selected_block().is_some() {
                    self.mode = Mode::ConfirmDelete;
                }
            }
            KeyCode::Char('r') => self.reload(),
            KeyCode::Char('l') => self.begin_connect(),
            _ => {}
        }
    }

    fn on_key_search(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.filter.clear();
                self.mode = Mode::List;
                self.clamp_selection();
            }
            KeyCode::Enter => {
                self.mode = Mode::List;
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.clamp_selection();
            }
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.list_state.select(Some(0));
                self.clamp_selection();
            }
            KeyCode::Down => self.move_selection(1),
            KeyCode::Up => self.move_selection(-1),
            _ => {}
        }
    }

    fn on_key_confirm(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Some(index) = self.selected_block() {
                    let label = self.config.blocks[index].label();
                    self.config.blocks.remove(index);
                    self.clamp_selection();
                    self.save();
                    if !matches!(self.status, Some((_, StatusKind::Error))) {
                        self.info(format!("Deleted {label}"));
                    }
                }
                self.mode = Mode::List;
            }
            _ => self.mode = Mode::List,
        }
    }

    fn begin_add(&mut self) {
        self.editor = Some(Editor::new(None, &Block::new_host("")));
        self.mode = Mode::Edit;
    }

    fn begin_edit(&mut self) {
        let Some(index) = self.selected_block() else {
            return;
        };
        if self.config.blocks[index].kind == BlockKind::Match {
            self.error("Match blocks can't be edited here — edit the file directly");
            return;
        }
        self.editor = Some(Editor::new(Some(index), &self.config.blocks[index]));
        self.mode = Mode::Edit;
    }

    fn duplicate(&mut self) {
        let Some(index) = self.selected_block() else {
            return;
        };
        let mut copy = self.config.blocks[index].clone();
        let alias = format!("{}-copy", copy.label());
        copy.set_patterns(&alias);
        self.config.blocks.insert(index + 1, copy);
        // Land the cursor on the new entry so it can be edited straight away.
        let position = self.visible().iter().position(|i| *i == index + 1);
        if let Some(position) = position {
            self.list_state.select(Some(position));
        }
        self.save();
        if !matches!(self.status, Some((_, StatusKind::Error))) {
            self.info(format!("Duplicated as {alias} — press e to edit"));
        }
    }

    fn reload(&mut self) {
        match SshConfig::load(self.config.path.clone()) {
            Ok(config) => {
                self.config = config;
                self.clamp_selection();
                self.info("Reloaded from disk");
            }
            Err(e) => self.error(format!("Could not reload: {e}")),
        }
    }

    fn begin_connect(&mut self) {
        let Some(block) = self.selected() else { return };
        if block.is_pattern() {
            self.error("That entry is a pattern, not a host you can connect to");
            return;
        }
        self.connect_to = Some(block.label());
    }

    fn on_key_edit(&mut self, key: KeyEvent) {
        let Some(editor) = self.editor.as_mut() else {
            self.mode = Mode::List;
            return;
        };
        match editor.handle_key(key) {
            Some(true) => self.commit_editor(),
            Some(false) => {
                self.editor = None;
                self.mode = Mode::List;
                self.info("Cancelled");
            }
            None => {
                // The editor reports validation problems through the status bar.
                if let Some(error) = editor.error.clone() {
                    self.error(error);
                }
            }
        }
    }

    /// Renders the editor back into the config and writes it out.
    fn commit_editor(&mut self) {
        let Some(editor) = self.editor.as_ref() else {
            return;
        };
        let existing = editor.target.map(|index| &self.config.blocks[index]);

        let block = match editor.to_block(existing) {
            Ok(block) => block,
            Err(message) => {
                self.error(message);
                return;
            }
        };

        let label = block.label();
        match editor.target {
            Some(index) => self.config.blocks[index] = block,
            None => {
                self.config.blocks.push(block);
                // Reveal the new host even if the current filter would hide it.
                self.filter.clear();
                self.list_state.select(Some(self.config.blocks.len() - 1));
            }
        }

        self.editor = None;
        self.mode = Mode::List;
        self.save();
        if !matches!(self.status, Some((_, StatusKind::Error))) {
            self.info(format!("Saved {label}"));
        }
    }
}

impl App {
    /// Reports how an ssh session ended, once we are back in the TUI.
    pub fn note_disconnect(&mut self, host: &str, code: Option<i32>) {
        match code {
            Some(code) => self.error(format!("ssh {host} exited with status {code}")),
            None => self.info(format!("ssh {host} was interrupted")),
        }
    }

    pub fn note_ssh_error(&mut self, host: &str, error: &std::io::Error) {
        self.error(format!("Could not run ssh {host}: {error}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SshConfig;
    use ratatui::crossterm::event::KeyEventState;

    const SAMPLE: &str = "\
Host *
    AddKeysToAgent yes

# the NAS
Host nas
    HostName 10.0.0.4
    User st
    LocalForward 8081 127.0.0.1:8081

Host web
    HostName web.example.com
    User deploy
";

    /// An app backed by a scratch file, so `save()` exercises the real path.
    fn app_with(name: &str, text: &str) -> App {
        let dir = std::env::temp_dir().join(format!("sshconfig-app-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        std::fs::write(&path, text).unwrap();
        App::new(SshConfig::load(path).unwrap())
    }

    fn read_back(app: &App) -> String {
        std::fs::read_to_string(&app.config.path).unwrap()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent {
            modifiers: KeyModifiers::CONTROL,
            ..key(code)
        }
    }

    /// Puts the editor's cursor on the row with this label.
    fn focus(app: &mut App, label: &str) {
        let editor = app.editor.as_mut().expect("not editing");
        let rows = editor.rows();
        editor.cursor = rows
            .iter()
            .position(|row| editor.row_label(*row) == label)
            .unwrap_or_else(|| panic!("no row labelled {label}"));
    }

    /// Focuses a row, replaces its value and accepts it.
    fn set_row(app: &mut App, label: &str, value: &str) {
        focus(app, label);
        app.on_key(key(KeyCode::Enter));
        let existing = app
            .editor
            .as_ref()
            .unwrap()
            .inline
            .as_ref()
            .unwrap()
            .value();
        for _ in 0..existing.chars().count() {
            app.on_key(key(KeyCode::Backspace));
        }
        press(app, value);
        app.on_key(key(KeyCode::Enter));
    }

    fn press(app: &mut App, keys: &str) {
        for c in keys.chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn navigation_wraps_and_tracks_selection() {
        let mut app = app_with("nav", SAMPLE);
        assert_eq!(app.selected().unwrap().label(), "*");
        press(&mut app, "j");
        assert_eq!(app.selected().unwrap().label(), "nas");
        press(&mut app, "jj");
        assert_eq!(app.selected().unwrap().label(), "*", "should wrap around");
        press(&mut app, "k");
        assert_eq!(app.selected().unwrap().label(), "web");
    }

    #[test]
    fn search_filters_on_alias_hostname_and_user() {
        let mut app = app_with("search", SAMPLE);
        press(&mut app, "/");
        press(&mut app, "deploy");
        assert_eq!(app.visible().len(), 1);
        assert_eq!(app.selected().unwrap().label(), "web");

        app.on_key(key(KeyCode::Backspace));
        press(&mut app, "");
        assert_eq!(app.filter, "deplo");

        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::List);
        assert_eq!(app.visible().len(), 3, "esc clears the filter");
    }

    #[test]
    fn selection_stays_valid_when_the_filter_shrinks_the_list() {
        let mut app = app_with("clamp", SAMPLE);
        press(&mut app, "jj");
        press(&mut app, "/");
        press(&mut app, "nas");
        assert_eq!(app.selected().unwrap().label(), "nas");
        assert!(app.selected_block().is_some());
    }

    #[test]
    fn adding_a_host_writes_it_to_disk() {
        let mut app = app_with("add", SAMPLE);
        press(&mut app, "a");
        assert_eq!(app.mode, Mode::Edit);

        set_row(&mut app, "Host", "db");
        set_row(&mut app, "HostName", "db.internal");
        set_row(&mut app, "User", "postgres");
        app.on_key(ctrl(KeyCode::Char('s')));

        assert_eq!(app.mode, Mode::List);
        let text = read_back(&app);
        assert!(
            text.ends_with("Host db\n    HostName db.internal\n    User postgres\n"),
            "{text}"
        );
        assert_eq!(
            app.selected().unwrap().label(),
            "db",
            "cursor lands on the new host"
        );
    }

    #[test]
    fn adding_a_port_forward_writes_the_directive_for_you() {
        let mut app = app_with("forward", SAMPLE);
        press(&mut app, "j"); // nas
        press(&mut app, "e");

        focus(&mut app, "+ add a port forward");
        app.on_key(key(KeyCode::Enter));
        app.on_key(key(KeyCode::Down)); // bind address
        app.on_key(key(KeyCode::Down)); // listening port
        press(&mut app, "5432");
        app.on_key(key(KeyCode::Down)); // host
        for _ in 0.."localhost".len() {
            app.on_key(key(KeyCode::Backspace));
        }
        press(&mut app, "db.internal");
        app.on_key(key(KeyCode::Down)); // destination port
        press(&mut app, "5432");
        app.on_key(key(KeyCode::Enter));
        app.on_key(ctrl(KeyCode::Char('s')));

        let text = read_back(&app);
        assert!(
            text.contains("    LocalForward 5432 db.internal:5432"),
            "{text}"
        );
        assert!(
            text.contains("    LocalForward 8081 127.0.0.1:8081"),
            "existing forward kept"
        );
    }

    #[test]
    fn adding_an_option_from_the_catalogue() {
        let mut app = app_with("option", SAMPLE);
        press(&mut app, "je");

        focus(&mut app, "+ add an option");
        app.on_key(key(KeyCode::Enter));
        press(&mut app, "ServerAliveInterval");
        app.on_key(key(KeyCode::Enter));

        // ServerAliveInterval takes a time, so the value is typed.
        press(&mut app, "60");
        app.on_key(key(KeyCode::Enter));
        app.on_key(ctrl(KeyCode::Char('s')));

        assert!(
            read_back(&app).contains("    ServerAliveInterval 60"),
            "{}",
            read_back(&app)
        );
    }

    #[test]
    fn a_yes_no_option_is_chosen_from_a_list() {
        let mut app = app_with("choice", SAMPLE);
        press(&mut app, "je");

        focus(&mut app, "+ add an option");
        app.on_key(key(KeyCode::Enter));
        press(&mut app, "Compression");
        app.on_key(key(KeyCode::Enter)); // pick the keyword
        app.on_key(key(KeyCode::Enter)); // accept the highlighted value
        app.on_key(ctrl(KeyCode::Char('s')));

        assert!(
            read_back(&app).contains("    Compression yes"),
            "{}",
            read_back(&app)
        );
    }

    #[test]
    fn editing_preserves_directives_the_form_does_not_show() {
        let mut app = app_with("edit", SAMPLE);
        press(&mut app, "j");
        press(&mut app, "e");
        set_row(&mut app, "HostName", "10.0.0.9");
        app.on_key(ctrl(KeyCode::Char('s')));

        let text = read_back(&app);
        assert!(text.contains("    HostName 10.0.0.9"));
        assert!(
            text.contains("    LocalForward 8081 127.0.0.1:8081"),
            "forward kept: {text}"
        );
        assert!(
            text.contains("# the NAS\nHost nas"),
            "label comment kept: {text}"
        );
        assert!(
            text.contains("Host *\n    AddKeysToAgent yes"),
            "neighbour untouched"
        );
    }

    #[test]
    fn escaping_the_form_discards_changes() {
        let mut app = app_with("cancel", SAMPLE);
        press(&mut app, "je");
        set_row(&mut app, "HostName", "wrong.example.com");
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::List);
        assert_eq!(read_back(&app), SAMPLE, "file untouched");
    }

    #[test]
    fn an_empty_alias_is_rejected_and_keeps_the_form_open() {
        let mut app = app_with("empty", SAMPLE);
        press(&mut app, "a");
        app.on_key(ctrl(KeyCode::Char('s')));
        assert_eq!(app.mode, Mode::Edit);
        assert!(matches!(app.status, Some((_, StatusKind::Error))));
        assert_eq!(read_back(&app), SAMPLE);
    }

    #[test]
    fn a_non_numeric_port_is_rejected() {
        let mut app = app_with("port", SAMPLE);
        press(&mut app, "je");
        set_row(&mut app, "Port", "http");
        app.on_key(ctrl(KeyCode::Char('s')));

        assert_eq!(
            app.mode,
            Mode::Edit,
            "the editor stays open so it can be fixed"
        );
        assert!(matches!(app.status, Some((_, StatusKind::Error))));
        assert_eq!(read_back(&app), SAMPLE);
    }

    #[test]
    fn delete_asks_first_and_only_removes_the_selected_host() {
        let mut app = app_with("delete", SAMPLE);
        press(&mut app, "j");

        press(&mut app, "d");
        assert_eq!(app.mode, Mode::ConfirmDelete);
        press(&mut app, "n");
        assert_eq!(read_back(&app), SAMPLE, "declining changes nothing");

        press(&mut app, "d");
        press(&mut app, "y");
        let text = read_back(&app);
        assert!(!text.contains("Host nas"));
        assert!(!text.contains("# the NAS"), "the host's label goes with it");
        assert!(text.contains("Host web"));
        assert!(text.contains("Host *"));
    }

    #[test]
    fn duplicate_copies_the_body_under_a_new_alias() {
        let mut app = app_with("dup", SAMPLE);
        press(&mut app, "jy");
        let text = read_back(&app);
        assert!(text.contains("Host nas-copy"));
        assert_eq!(text.matches("LocalForward 8081 127.0.0.1:8081").count(), 2);
        assert_eq!(app.selected().unwrap().label(), "nas-copy");
    }

    #[test]
    fn connecting_is_refused_for_wildcard_entries() {
        let mut app = app_with("connect", SAMPLE);
        press(&mut app, "l");
        assert!(app.connect_to.is_none());
        assert!(matches!(app.status, Some((_, StatusKind::Error))));

        press(&mut app, "jl");
        assert_eq!(app.connect_to.as_deref(), Some("nas"));
    }

    #[test]
    fn enter_opens_the_editor_just_like_e() {
        let mut app = app_with("enter", SAMPLE);
        press(&mut app, "j");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Edit);
        assert_eq!(app.editor.as_ref().unwrap().alias, "nas");
        assert!(app.connect_to.is_none(), "enter no longer connects");
    }

    #[test]
    fn match_blocks_are_not_editable_through_the_form() {
        let mut app = app_with("match", "Match host bastion\n    ForwardAgent yes\n");
        press(&mut app, "e");
        assert_eq!(app.mode, Mode::List);
        assert!(matches!(app.status, Some((_, StatusKind::Error))));
    }

    #[test]
    fn works_on_an_empty_config() {
        let mut app = app_with("blank", "");
        assert!(app.selected().is_none());
        press(&mut app, "d");
        assert_eq!(
            app.mode,
            Mode::List,
            "delete with nothing selected is a no-op"
        );
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::List, "nothing to edit");
        press(&mut app, "l");
        assert!(app.connect_to.is_none());
        press(&mut app, "j");
        assert!(app.selected().is_none());
    }

    #[test]
    fn reload_picks_up_external_edits() {
        let mut app = app_with("reload", SAMPLE);
        std::fs::write(&app.config.path, "Host only\n").unwrap();
        press(&mut app, "r");
        assert_eq!(app.config.blocks.len(), 1);
        assert_eq!(app.selected().unwrap().label(), "only");
    }
}
