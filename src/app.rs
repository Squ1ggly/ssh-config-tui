//! Application state and key handling. Rendering lives in `ui`.
//!
//! Every change is written the moment it's made. Anything that throws data
//! away — deleting a host, removing a row, overwriting someone else's edit,
//! dropping changes that couldn't be saved — goes through a confirm dialog.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;

use crate::config::{Block, BlockKind, SaveError, SshConfig, display_path};
use crate::editor::{Editor, EditorEvent};

/// How long a message stays in the status bar.
pub const INFO_TTL: Duration = Duration::from_secs(2);
/// Errors stay a little longer: they usually need reading.
pub const ERROR_TTL: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    List,
    Search,
    Edit,
    Confirm,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Error,
}

pub struct Status {
    pub text: String,
    pub kind: StatusKind,
    pub shown_at: Instant,
}

impl Status {
    fn expires_at(&self) -> Instant {
        self.shown_at
            + match self.kind {
                StatusKind::Info => INFO_TTL,
                StatusKind::Error => ERROR_TTL,
            }
    }
}

/// Something that waits on a confirm dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    DeleteHost(usize),
    /// Write our version over a file another program changed.
    Overwrite,
    /// Take the other program's version, dropping our unsaved change.
    ReloadTheirs,
    /// Reload, dropping changes that couldn't be saved.
    Reload,
    /// Quit, dropping changes that couldn't be saved.
    Quit,
}

pub struct Confirm {
    pub title: &'static str,
    pub message: String,
    /// Key, label and what it does. Any other key cancels.
    pub choices: Vec<(char, &'static str, Action)>,
    /// Shown when the dialog is dismissed without choosing.
    pub cancel_note: Option<&'static str>,
    /// The mode to go back to afterwards.
    back_to: Mode,
}

pub struct App {
    pub config: SshConfig,
    pub mode: Mode,
    pub filter: String,
    pub list_state: ListState,
    pub editor: Option<Editor>,
    pub status: Option<Status>,
    pub confirm: Option<Confirm>,
    /// In-memory changes that aren't on disk because a save failed or was
    /// declined. Reloading or quitting would lose them, so both ask first.
    pub unsaved: bool,
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
            confirm: None,
            unsaved: false,
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
        self.set_status(msg.into(), StatusKind::Info);
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.set_status(msg.into(), StatusKind::Error);
    }

    fn set_status(&mut self, text: String, kind: StatusKind) {
        self.status = Some(Status {
            text,
            kind,
            shown_at: Instant::now(),
        });
    }

    /// When the current message should disappear, so the event loop knows how
    /// long it can sleep.
    pub fn status_deadline(&self) -> Option<Instant> {
        self.status.as_ref().map(Status::expires_at)
    }

    /// Clears the message once its time is up. Returns whether it did.
    pub fn expire_status(&mut self, now: Instant) -> bool {
        if self.status.as_ref().is_some_and(|s| now >= s.expires_at()) {
            self.status = None;
            return true;
        }
        false
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

    /// Opens a confirm dialog over whatever is on screen.
    fn ask(
        &mut self,
        title: &'static str,
        message: String,
        choices: Vec<(char, &'static str, Action)>,
        cancel_note: Option<&'static str>,
    ) {
        let back_to = if self.mode == Mode::Confirm {
            self.confirm.as_ref().map_or(Mode::List, |c| c.back_to)
        } else {
            self.mode
        };
        self.confirm = Some(Confirm {
            title,
            message,
            choices,
            cancel_note,
            back_to,
        });
        self.mode = Mode::Confirm;
    }

    /// Writes the config. Returns whether it reached the disk.
    fn persist(&mut self, force: bool) -> bool {
        match self.config.save(force) {
            Ok(()) => {
                self.unsaved = false;
                true
            }
            Err(SaveError::Conflict) => {
                self.unsaved = true;
                let path = display_path(&self.config.path);
                self.ask(
                    " File changed on disk ",
                    format!(
                        "{path} was changed by another program since sshconfig read it. \
                         Saving now would overwrite that change."
                    ),
                    vec![
                        ('o', "overwrite it with my version", Action::Overwrite),
                        (
                            'r',
                            "reload theirs, dropping my change",
                            Action::ReloadTheirs,
                        ),
                    ],
                    Some("Not saved — your change is only in sshconfig for now"),
                );
                false
            }
            Err(SaveError::Io(e)) => {
                self.unsaved = true;
                self.error(format!("Could not save: {e}"));
                false
            }
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
            Mode::Confirm => self.on_key_confirm(key),
            Mode::Help => {
                self.mode = Mode::List;
            }
        }
    }

    fn on_key_list(&mut self, key: KeyEvent) {
        self.status = None;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit(),
            KeyCode::Char('c') if ctrl => self.quit(),
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
            KeyCode::Char('d') => self.ask_delete(),
            KeyCode::Char('r') => self.ask_reload(),
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
        let Some(confirm) = self.confirm.take() else {
            self.mode = Mode::List;
            return;
        };
        self.mode = confirm.back_to;

        let chosen = match key.code {
            KeyCode::Char(c) => confirm
                .choices
                .iter()
                .find(|(k, _, _)| k.eq_ignore_ascii_case(&c))
                .map(|(_, _, action)| *action),
            _ => None,
        };
        match chosen {
            Some(action) => self.run(action),
            None => {
                if let Some(note) = confirm.cancel_note {
                    self.info(note);
                }
            }
        }
    }

    fn run(&mut self, action: Action) {
        match action {
            Action::DeleteHost(index) => {
                let label = self.config.blocks[index].label();
                self.config.blocks.remove(index);
                self.clamp_selection();
                if self.persist(false) {
                    self.info(format!("Deleted {label}"));
                }
            }
            Action::Overwrite => {
                if self.persist(true) {
                    self.info("Saved — the other program's change was overwritten");
                }
            }
            Action::ReloadTheirs | Action::Reload => {
                // Block indices may mean something else in the reloaded file.
                self.editor = None;
                self.mode = Mode::List;
                self.reload();
            }
            Action::Quit => self.should_quit = true,
        }
    }

    fn quit(&mut self) {
        if self.unsaved {
            self.ask(
                " Unsaved changes ",
                "Some changes couldn't be saved. Quitting now will lose them.".into(),
                vec![('y', "quit anyway", Action::Quit)],
                None,
            );
        } else {
            self.should_quit = true;
        }
    }

    fn ask_delete(&mut self) {
        let Some(index) = self.selected_block() else {
            return;
        };
        let label = self.config.blocks[index].label();
        self.ask(
            " Delete host ",
            format!("Delete {label}? It is removed from the file straight away."),
            vec![('y', "delete", Action::DeleteHost(index))],
            None,
        );
    }

    fn ask_reload(&mut self) {
        if self.unsaved {
            self.ask(
                " Reload ",
                "Some changes couldn't be saved. Reloading drops them.".into(),
                vec![('y', "reload anyway", Action::Reload)],
                None,
            );
        } else {
            self.reload();
        }
    }

    fn begin_add(&mut self) {
        self.editor = Some(Editor::new_host());
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
        if self.persist(false) {
            self.info(format!("Duplicated as {alias}"));
        }
    }

    fn reload(&mut self) {
        match SshConfig::load(self.config.path.clone()) {
            Ok(mut config) => {
                // The backup still holds the file from before this session.
                config.backed_up = self.config.backed_up;
                self.config = config;
                self.unsaved = false;
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
        let event = editor.handle_key(key);
        let message = editor.error.clone();
        let never_saved = editor.is_new();
        // The editor reports validation problems through the status bar.
        if let Some(message) = message {
            self.error(message);
        }
        match event {
            EditorEvent::Changed => self.apply_editor(),
            EditorEvent::Close => {
                self.editor = None;
                self.mode = Mode::List;
                if never_saved {
                    self.info("Nothing saved — a new host needs an alias first");
                }
            }
            EditorEvent::None => {}
        }
    }

    /// Writes the editor's current state into the config and saves it.
    fn apply_editor(&mut self) {
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
                // The first valid change to a new host is what creates it.
                self.config.blocks.push(block);
                let index = self.config.blocks.len() - 1;
                if let Some(editor) = self.editor.as_mut() {
                    editor.target = Some(index);
                }
                // Reveal the new host even if the current filter would hide it.
                self.filter.clear();
                self.list_state.select(Some(index));
            }
        }

        if self.persist(false) {
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

    /// An app backed by a scratch file, so saving exercises the real path.
    fn app_with(name: &str, text: &str) -> App {
        let dir = std::env::temp_dir().join(format!("sshconfig-app-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
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

    /// Focuses a row, replaces its value and presses enter.
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

    fn is_error(app: &App) -> bool {
        app.status
            .as_ref()
            .is_some_and(|s| s.kind == StatusKind::Error)
    }

    fn status_text(app: &App) -> String {
        app.status
            .as_ref()
            .map(|s| s.text.clone())
            .unwrap_or_default()
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

    // --- saving as you go ------------------------------------------------

    #[test]
    fn each_change_is_written_as_soon_as_it_is_made() {
        let mut app = app_with("autosave", SAMPLE);
        press(&mut app, "je");

        set_row(&mut app, "HostName", "10.0.0.9");
        assert!(
            read_back(&app).contains("    HostName 10.0.0.9"),
            "saved without ctrl-s"
        );
        assert_eq!(app.mode, Mode::Edit, "and the editor stays open");
        assert_eq!(status_text(&app), "Saved nas");

        set_row(&mut app, "User", "root");
        assert!(read_back(&app).contains("    User root"));
    }

    #[test]
    fn ctrl_s_is_no_longer_needed_or_bound() {
        let mut app = app_with("ctrl-s", SAMPLE);
        press(&mut app, "je");
        app.on_key(ctrl(KeyCode::Char('s')));
        assert_eq!(app.mode, Mode::Edit);
        assert_eq!(
            read_back(&app),
            SAMPLE,
            "nothing changed, so nothing written"
        );
    }

    #[test]
    fn esc_leaves_the_editor_with_everything_already_saved() {
        let mut app = app_with("esc", SAMPLE);
        press(&mut app, "je");
        set_row(&mut app, "HostName", "10.0.0.9");
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::List);
        assert!(
            read_back(&app).contains("    HostName 10.0.0.9"),
            "esc doesn't undo"
        );
    }

    #[test]
    fn a_new_host_is_created_by_giving_it_an_alias() {
        let mut app = app_with("add", SAMPLE);
        press(&mut app, "a");
        assert_eq!(app.mode, Mode::Edit);
        assert!(
            app.editor.as_ref().unwrap().inline.is_some(),
            "typing the alias straight away"
        );

        press(&mut app, "db");
        app.on_key(key(KeyCode::Enter));
        assert!(
            read_back(&app).ends_with("Host db\n"),
            "{}",
            read_back(&app)
        );

        set_row(&mut app, "HostName", "db.internal");
        set_row(&mut app, "User", "postgres");
        let text = read_back(&app);
        assert!(
            text.ends_with("Host db\n    HostName db.internal\n    User postgres\n"),
            "{text}"
        );
        assert_eq!(
            text.matches("Host db").count(),
            1,
            "one host, not one per change"
        );

        app.on_key(key(KeyCode::Esc));
        assert_eq!(
            app.selected().unwrap().label(),
            "db",
            "cursor lands on the new host"
        );
    }

    #[test]
    fn abandoning_a_new_host_before_naming_it_writes_nothing() {
        let mut app = app_with("abandon", SAMPLE);
        press(&mut app, "a");
        app.on_key(key(KeyCode::Esc)); // stop typing the alias
        app.on_key(key(KeyCode::Esc)); // leave the editor
        assert_eq!(app.mode, Mode::List);
        assert_eq!(read_back(&app), SAMPLE);
        assert!(status_text(&app).contains("Nothing saved"));
    }

    #[test]
    fn an_empty_alias_is_refused_and_stays_editable() {
        let mut app = app_with("empty", SAMPLE);
        press(&mut app, "a");
        app.on_key(key(KeyCode::Enter));
        assert!(is_error(&app));
        assert!(
            app.editor.as_ref().unwrap().inline.is_some(),
            "still typing"
        );
        assert_eq!(read_back(&app), SAMPLE);
    }

    #[test]
    fn a_bad_value_is_refused_before_it_reaches_the_file() {
        let mut app = app_with("port", SAMPLE);
        press(&mut app, "je");
        set_row(&mut app, "Port", "http");
        assert!(is_error(&app));
        assert!(status_text(&app).contains("Port"), "{}", status_text(&app));
        assert!(
            app.editor.as_ref().unwrap().inline.is_some(),
            "left in the box to fix"
        );
        assert_eq!(read_back(&app), SAMPLE);
    }

    #[test]
    fn editing_preserves_directives_the_form_does_not_show() {
        let mut app = app_with("edit", SAMPLE);
        press(&mut app, "je");
        set_row(&mut app, "HostName", "10.0.0.9");

        let text = read_back(&app);
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
    fn adding_a_port_forward_writes_the_directive_for_you() {
        let mut app = app_with("forward", SAMPLE);
        press(&mut app, "je");
        press(&mut app, "a"); // add a forward from anywhere
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
    fn space_switches_a_forward_off_and_on_by_commenting_it_out() {
        let mut app = app_with("toggle", SAMPLE);
        press(&mut app, "je");
        focus(&mut app, "local");

        press(&mut app, " ");
        let text = read_back(&app);
        assert!(
            text.contains("    # LocalForward 8081 127.0.0.1:8081"),
            "{text}"
        );
        assert!(!text.contains("    LocalForward 8081"), "no live copy left");

        press(&mut app, " ");
        let text = read_back(&app);
        assert!(
            text.contains("    LocalForward 8081 127.0.0.1:8081"),
            "{text}"
        );
        assert!(!text.contains("# LocalForward"), "{text}");
    }

    #[test]
    fn a_forward_commented_out_by_hand_can_be_switched_on() {
        let text = SAMPLE.replace("    LocalForward 8081", "    #LocalForward 8081");
        let mut app = app_with("by-hand", &text);
        press(&mut app, "je");
        let editor = app.editor.as_ref().unwrap();
        assert_eq!(editor.forwards.len(), 1);
        assert!(!editor.forwards[0].enabled);

        focus(&mut app, "local");
        press(&mut app, " ");
        assert!(read_back(&app).contains("    LocalForward 8081 127.0.0.1:8081"));
    }

    #[test]
    fn adding_an_option_from_the_catalogue() {
        let mut app = app_with("option", SAMPLE);
        press(&mut app, "je");
        focus(&mut app, "+ add an option");
        app.on_key(key(KeyCode::Enter));
        press(&mut app, "ServerAliveInterval");
        app.on_key(key(KeyCode::Enter));
        press(&mut app, "60"); // takes a time, so the value is typed
        app.on_key(key(KeyCode::Enter));

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

        assert!(
            read_back(&app).contains("    Compression yes"),
            "{}",
            read_back(&app)
        );
    }

    #[test]
    fn slash_in_the_editor_searches_its_rows_not_the_host_list() {
        let mut app = app_with("editor-search", SAMPLE);
        press(&mut app, "je");
        press(&mut app, "/");
        assert_eq!(app.mode, Mode::Edit, "stays in the editor");
        press(&mut app, "user");
        app.on_key(key(KeyCode::Enter));

        let editor = app.editor.as_ref().unwrap();
        assert_eq!(editor.row_label(editor.current_row()), "User");
        assert!(app.filter.is_empty(), "the host filter is untouched");
        assert_eq!(read_back(&app), SAMPLE, "searching changes nothing");
    }

    // --- dangerous actions ask first -------------------------------------

    #[test]
    fn deleting_a_host_asks_first() {
        let mut app = app_with("delete", SAMPLE);
        press(&mut app, "j");

        press(&mut app, "d");
        assert_eq!(app.mode, Mode::Confirm);
        press(&mut app, "n");
        assert_eq!(app.mode, Mode::List);
        assert_eq!(read_back(&app), SAMPLE, "declining changes nothing");

        press(&mut app, "dy");
        let text = read_back(&app);
        assert!(!text.contains("Host nas"));
        assert!(!text.contains("# the NAS"), "the host's label goes with it");
        assert!(text.contains("Host web"));
        assert!(text.contains("Host *"));
    }

    #[test]
    fn removing_a_row_in_the_editor_asks_first() {
        let mut app = app_with("remove-row", SAMPLE);
        press(&mut app, "je");
        focus(&mut app, "local");

        press(&mut app, "d");
        press(&mut app, "n");
        assert_eq!(read_back(&app), SAMPLE, "declining keeps it");

        press(&mut app, "dy");
        assert!(!read_back(&app).contains("LocalForward"));
    }

    #[test]
    fn an_edit_made_elsewhere_is_never_silently_overwritten() {
        let mut app = app_with("conflict", SAMPLE);
        let theirs = format!("{SAMPLE}\nHost added-in-vim\n");
        std::fs::write(&app.config.path, &theirs).unwrap();

        press(&mut app, "je");
        set_row(&mut app, "HostName", "10.0.0.9");
        assert_eq!(app.mode, Mode::Confirm, "asks what to do");
        assert_eq!(read_back(&app), theirs, "their edit is still there");

        // Dismissing it keeps both: theirs on disk, ours in memory.
        press(&mut app, "x");
        assert_eq!(app.mode, Mode::Edit);
        assert_eq!(read_back(&app), theirs);
        assert!(app.unsaved);
    }

    #[test]
    fn a_conflict_can_be_resolved_by_overwriting() {
        let mut app = app_with("overwrite", SAMPLE);
        std::fs::write(&app.config.path, "Host theirs\n").unwrap();
        press(&mut app, "je");
        set_row(&mut app, "HostName", "10.0.0.9");
        press(&mut app, "o");

        let text = read_back(&app);
        assert!(text.contains("HostName 10.0.0.9"));
        assert!(!text.contains("Host theirs"));
        assert!(!app.unsaved);
    }

    #[test]
    fn a_conflict_can_be_resolved_by_taking_theirs() {
        let mut app = app_with("theirs", SAMPLE);
        std::fs::write(&app.config.path, "Host theirs\n").unwrap();
        press(&mut app, "je");
        set_row(&mut app, "HostName", "10.0.0.9");
        press(&mut app, "r");

        assert_eq!(
            app.mode,
            Mode::List,
            "the editor closes: its host may be gone"
        );
        assert_eq!(read_back(&app), "Host theirs\n");
        assert_eq!(app.config.blocks[0].label(), "theirs");
    }

    #[test]
    fn quitting_with_unsaved_changes_asks_first() {
        let mut app = app_with("quit", SAMPLE);
        press(&mut app, "q");
        assert!(app.should_quit, "nothing to lose, so no question");

        let mut app = app_with("quit-unsaved", SAMPLE);
        std::fs::write(&app.config.path, "Host theirs\n").unwrap();
        press(&mut app, "je");
        set_row(&mut app, "HostName", "10.0.0.9");
        press(&mut app, "x"); // leave the conflict for later
        app.on_key(key(KeyCode::Esc)); // out of the editor
        press(&mut app, "q");
        assert!(!app.should_quit);
        assert_eq!(app.mode, Mode::Confirm);
        press(&mut app, "y");
        assert!(app.should_quit);
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
    fn the_backup_is_the_file_from_before_the_session() {
        let mut app = app_with("backup", SAMPLE);
        press(&mut app, "je");
        set_row(&mut app, "HostName", "one");
        set_row(&mut app, "HostName", "two");
        set_row(&mut app, "HostName", "three");
        assert_eq!(
            std::fs::read_to_string(app.config.backup_path()).unwrap(),
            SAMPLE
        );
    }

    // --- status messages -------------------------------------------------

    #[test]
    fn messages_clear_themselves_after_a_couple_of_seconds() {
        let mut app = app_with("expire", SAMPLE);
        press(&mut app, "r");
        assert_eq!(status_text(&app), "Reloaded from disk");
        let shown = app.status.as_ref().unwrap().shown_at;

        assert!(
            !app.expire_status(shown + Duration::from_millis(1900)),
            "still showing"
        );
        assert!(app.status.is_some());
        assert!(app.expire_status(shown + Duration::from_millis(2000)));
        assert!(app.status.is_none());
    }

    #[test]
    fn errors_stay_up_a_little_longer() {
        let mut app = app_with("expire-error", SAMPLE);
        press(&mut app, "l"); // "*" can't be connected to
        assert!(is_error(&app));
        let shown = app.status.as_ref().unwrap().shown_at;
        assert!(!app.expire_status(shown + Duration::from_secs(3)));
        assert!(app.expire_status(shown + ERROR_TTL));
    }

    #[test]
    fn the_event_loop_is_told_when_to_wake_for_the_next_expiry() {
        let mut app = app_with("deadline", SAMPLE);
        assert!(app.status_deadline().is_none(), "nothing to wait for");
        press(&mut app, "r");
        let shown = app.status.as_ref().unwrap().shown_at;
        assert_eq!(app.status_deadline(), Some(shown + INFO_TTL));
    }

    // --- everything else ------------------------------------------------

    #[test]
    fn connecting_is_refused_for_wildcard_entries() {
        let mut app = app_with("connect", SAMPLE);
        press(&mut app, "l");
        assert!(app.connect_to.is_none());
        assert!(is_error(&app));

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
        assert!(app.connect_to.is_none(), "enter doesn't connect");
    }

    #[test]
    fn match_blocks_are_not_editable_through_the_form() {
        let mut app = app_with("match", "Match host bastion\n    ForwardAgent yes\n");
        press(&mut app, "e");
        assert_eq!(app.mode, Mode::List);
        assert!(is_error(&app));
    }

    #[test]
    fn works_on_an_empty_config() {
        let mut app = app_with("blank", "");
        assert!(app.selected().is_none());
        press(&mut app, "d");
        assert_eq!(app.mode, Mode::List, "nothing to delete, so no question");
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
