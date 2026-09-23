//! Parsing and serialising of an OpenSSH client config file.
//!
//! The guiding rule here is that we are a *round-trip* editor: anything we do
//! not explicitly understand is carried through byte-for-byte. Comments, odd
//! indentation, `Match` blocks and exotic directives all survive an edit of an
//! unrelated host.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const INDENT: &str = "    ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Host,
    /// `Match` blocks are shown read-only-ish (raw editing only): their
    /// semantics do not map onto the host form.
    Match,
}

/// One `Host` (or `Match`) stanza plus its indented body.
#[derive(Debug, Clone)]
pub struct Block {
    pub kind: BlockKind,
    /// Comment lines sitting directly above the header with no blank line in
    /// between. They read as a label for this stanza, so they travel with it.
    pub leading: Vec<String>,
    /// The header line exactly as it appeared, e.g. `Host web-01 web-02`.
    pub header: String,
    /// Body lines, verbatim including their original indentation.
    pub body: Vec<String>,
}

impl Block {
    pub fn new_host(patterns: &str) -> Self {
        Block {
            kind: BlockKind::Host,
            leading: Vec::new(),
            header: format!("Host {patterns}"),
            body: Vec::new(),
        }
    }

    /// The patterns on the header line, e.g. `["web-01", "web-02"]`.
    pub fn patterns(&self) -> Vec<String> {
        self.header
            .split_once(char::is_whitespace)
            .map(|(_, rest)| rest.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default()
    }

    pub fn set_patterns(&mut self, patterns: &str) {
        let keyword = if self.kind == BlockKind::Host {
            "Host"
        } else {
            "Match"
        };
        self.header = format!("{keyword} {}", patterns.trim());
    }

    /// Display name: the first pattern, or the whole header for `Match`.
    pub fn label(&self) -> String {
        match self.kind {
            BlockKind::Host => self
                .patterns()
                .first()
                .cloned()
                .unwrap_or_else(|| "?".into()),
            BlockKind::Match => self.header.trim().to_string(),
        }
    }

    /// True when the alias contains glob characters, i.e. it is a defaults
    /// stanza rather than something you can actually `ssh` to.
    pub fn is_pattern(&self) -> bool {
        self.kind == BlockKind::Match || self.patterns().iter().any(|p| p.contains(['*', '?', '!']))
    }

    /// First value for `key`, case-insensitively, ignoring comments.
    pub fn get(&self, key: &str) -> Option<String> {
        self.body
            .iter()
            .find_map(|line| match split_directive(line) {
                Some((k, v)) if k.eq_ignore_ascii_case(key) => Some(v.to_string()),
                _ => None,
            })
    }

    /// Replaces the body with `lines`, applying our indentation. Lines that
    /// already start with whitespace keep whatever indentation they had.
    pub fn set_body(&mut self, lines: &[String]) {
        self.body = lines
            .iter()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                if line.starts_with(char::is_whitespace) {
                    line.trim_end().to_string()
                } else {
                    format!("{INDENT}{}", line.trim())
                }
            })
            .collect();
    }

    fn to_text(&self) -> String {
        let mut out = String::new();
        for line in &self.leading {
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out.push_str(self.header.trim_end());
        out.push('\n');
        for line in &self.body {
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}

/// Splits `    HostName example.com` into `("HostName", "example.com")`.
/// Returns `None` for blank lines and comments. Also accepts the `Key=Value`
/// form that ssh_config permits.
pub fn split_directive(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let (key, value) = match trimmed.find(['=', ' ', '\t']) {
        Some(idx) => (
            &trimmed[..idx],
            trimmed[idx + 1..].trim_start_matches(['=', ' ', '\t']),
        ),
        None => (trimmed, ""),
    };
    Some((key, value.trim()))
}

fn keyword_is(line: &str, keyword: &str) -> bool {
    split_directive(line).is_some_and(|(k, _)| k.eq_ignore_ascii_case(keyword))
}

/// Removes and returns the run of comment lines at the end of `lines`.
fn take_trailing_comments(lines: &mut Vec<String>) -> Vec<String> {
    let start = lines
        .iter()
        .rposition(|line| !line.trim().starts_with('#'))
        .map_or(0, |i| i + 1);
    lines.split_off(start)
}

#[derive(Debug, Clone)]
pub struct SshConfig {
    pub path: PathBuf,
    /// Lines before the first stanza (global defaults, header comments).
    pub preamble: Vec<String>,
    pub blocks: Vec<Block>,
    /// The file's contents as we last read or wrote them (`None`: no file).
    /// Compared against the disk before every write, so edits made by another
    /// program while we're open are never silently overwritten.
    pub on_disk: Option<String>,
    /// Whether this session has already taken its backup.
    pub backed_up: bool,
}

/// Why a save didn't happen.
#[derive(Debug)]
pub enum SaveError {
    /// The file changed on disk since we read it.
    Conflict,
    Io(io::Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Conflict => write!(f, "the file was changed by another program"),
            SaveError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for SaveError {
    fn from(e: io::Error) -> Self {
        SaveError::Io(e)
    }
}

impl SshConfig {
    pub fn parse(path: PathBuf, text: &str) -> Self {
        let mut preamble = Vec::new();
        let mut blocks: Vec<Block> = Vec::new();

        for line in text.lines() {
            let kind = if keyword_is(line, "Host") {
                Some(BlockKind::Host)
            } else if keyword_is(line, "Match") {
                Some(BlockKind::Match)
            } else {
                None
            };

            match kind {
                Some(kind) => {
                    let previous = match blocks.last_mut() {
                        Some(block) => &mut block.body,
                        None => &mut preamble,
                    };
                    let leading = take_trailing_comments(previous);
                    blocks.push(Block {
                        kind,
                        leading,
                        header: line.trim().to_string(),
                        body: Vec::new(),
                    })
                }
                None => match blocks.last_mut() {
                    Some(block) => block.body.push(line.to_string()),
                    None => preamble.push(line.to_string()),
                },
            }
        }

        // Trailing blank lines are structural, not content: we re-add separators
        // ourselves when writing.
        for block in &mut blocks {
            while block.body.last().is_some_and(|l| l.trim().is_empty()) {
                block.body.pop();
            }
        }
        while preamble.last().is_some_and(|l| l.trim().is_empty()) {
            preamble.pop();
        }

        SshConfig {
            path,
            preamble,
            blocks,
            on_disk: Some(text.to_string()),
            backed_up: false,
        }
    }

    pub fn load(path: PathBuf) -> io::Result<Self> {
        match fs::read_to_string(&path) {
            Ok(text) => Ok(Self::parse(path, &text)),
            // A missing config is a perfectly normal starting point.
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(SshConfig {
                path,
                preamble: Vec::new(),
                blocks: Vec::new(),
                on_disk: None,
                backed_up: false,
            }),
            Err(e) => Err(e),
        }
    }

    pub fn to_text(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !self.preamble.is_empty() {
            parts.push(self.preamble.join("\n") + "\n");
        }
        parts.extend(self.blocks.iter().map(Block::to_text));
        parts.join("\n")
    }

    /// Writes the config atomically.
    ///
    /// Refuses with [`SaveError::Conflict`] if the file changed on disk since
    /// we last read or wrote it, unless `force` is set.
    ///
    /// The first write of a session copies the original file to `.bak`; later
    /// writes leave it alone. Since every change saves straight away, a backup
    /// per write would only ever hold the change before last — keeping the
    /// file as it was when you opened it is the useful undo.
    ///
    /// The temp file lives in the same directory so the rename cannot cross a
    /// filesystem boundary, and it is created 0600 so the config is never
    /// briefly world-readable.
    pub fn save(&mut self, force: bool) -> Result<(), SaveError> {
        let current = match fs::read_to_string(&self.path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        if !force && current != self.on_disk {
            return Err(SaveError::Conflict);
        }

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        if !self.backed_up && current.is_some() {
            fs::copy(&self.path, self.backup_path())?;
            self.backed_up = true;
        }

        let text = self.to_text();
        let tmp = self.path.with_extension("sshconfig-tmp");
        write_private(&tmp, &text)?;
        fs::rename(&tmp, &self.path)?;
        self.on_disk = Some(text);
        Ok(())
    }

    pub fn backup_path(&self) -> PathBuf {
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(".bak");
        self.path.with_file_name(name)
    }
}

fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()
}

/// `path` for display, with the home directory shown as `~`.
pub fn display_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

/// The default config path, honouring `$HOME`.
pub fn default_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ssh")
        .join("config")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# personal machines
Host *
    AddKeysToAgent yes

Host home-server-01
    Port 7869
    HostName 10.0.0.4
    User st
    LocalForward 8081 127.0.0.1:8081
    LocalForward 32400 127.0.0.1:32400

Match host bastion exec \"test -n \\\"$WORK\\\"\"
    ForwardAgent yes
";

    fn parse(text: &str) -> SshConfig {
        SshConfig::parse(PathBuf::from("/tmp/config"), text)
    }

    #[test]
    fn round_trips_unchanged() {
        let config = parse(SAMPLE);
        assert_eq!(config.to_text(), SAMPLE);
    }

    #[test]
    fn splits_blocks_and_preamble() {
        let config = parse(SAMPLE);
        // The header comment labels `Host *`, so it travels with that block.
        assert!(config.preamble.is_empty());
        assert_eq!(config.blocks[0].leading, ["# personal machines"]);
        assert_eq!(config.blocks.len(), 3);
        assert_eq!(config.blocks[1].label(), "home-server-01");
        assert_eq!(config.blocks[2].kind, BlockKind::Match);
    }

    #[test]
    fn match_block_body_is_not_absorbed_by_previous_host() {
        let config = parse(SAMPLE);
        assert!(config.blocks[1].get("ForwardAgent").is_none());
        assert_eq!(config.blocks[2].get("ForwardAgent").as_deref(), Some("yes"));
    }

    #[test]
    fn a_comment_separated_by_a_blank_line_stays_in_the_preamble() {
        let text = "# file header\n\nHost a\n    User st\n";
        let config = parse(text);
        assert_eq!(config.preamble, ["# file header"]);
        assert!(config.blocks[0].leading.is_empty());
        assert_eq!(config.to_text(), text);
    }

    #[test]
    fn a_labelled_host_keeps_its_comment_when_edited() {
        let mut config = parse("# the NAS\nHost nas\n    User st\n");
        config.blocks[0].set_body(&["User root".to_string()]);
        assert_eq!(config.to_text(), "# the NAS\nHost nas\n    User root\n");
    }

    #[test]
    fn reads_directives_case_insensitively() {
        let config = parse("Host a\n  hostname EXAMPLE.com\n  port=2222\n");
        assert_eq!(
            config.blocks[0].get("HostName").as_deref(),
            Some("EXAMPLE.com")
        );
        assert_eq!(config.blocks[0].get("Port").as_deref(), Some("2222"));
    }

    #[test]
    fn editing_one_host_preserves_unknown_directives_and_neighbours() {
        let mut config = parse(SAMPLE);
        config.blocks[1].set_body(&[
            "HostName 10.0.0.5".to_string(),
            "User st".to_string(),
            "Port 7869".to_string(),
            "LocalForward 8081 127.0.0.1:8081".to_string(),
            "LocalForward 32400 127.0.0.1:32400".to_string(),
        ]);

        let text = config.to_text();
        assert!(text.contains("    HostName 10.0.0.5"));
        // Both forwards survive, in their original order.
        let forwards: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("LocalForward"))
            .map(|l| l.trim())
            .collect();
        assert_eq!(
            forwards,
            [
                "LocalForward 8081 127.0.0.1:8081",
                "LocalForward 32400 127.0.0.1:32400"
            ]
        );
        // Untouched blocks are byte-identical.
        assert!(text.contains("Host *\n    AddKeysToAgent yes\n"));
        assert!(text.contains("    ForwardAgent yes\n"));
        // Fields left empty are dropped rather than written blank.
        assert!(!text.contains("IdentityFile\n"));
    }

    #[test]
    fn new_host_is_indented_consistently() {
        let mut block = Block::new_host("web");
        block.set_body(&[
            "HostName web.example.com".to_string(),
            "Compression yes".to_string(),
        ]);
        assert_eq!(
            block.body,
            ["    HostName web.example.com", "    Compression yes"]
        );
    }

    #[test]
    fn wildcards_and_match_blocks_are_not_connectable() {
        let config = parse(SAMPLE);
        assert!(config.blocks[0].is_pattern());
        assert!(!config.blocks[1].is_pattern());
        assert!(config.blocks[2].is_pattern());
    }

    #[test]
    fn multi_pattern_host_keeps_every_alias() {
        let mut config = parse("Host web-01 web-02 web-03\n    User deploy\n");
        assert_eq!(config.blocks[0].patterns(), ["web-01", "web-02", "web-03"]);
        config.blocks[0].set_patterns("web-01 web-02");
        assert_eq!(config.to_text(), "Host web-01 web-02\n    User deploy\n");
    }

    #[test]
    fn empty_and_missing_files_are_valid() {
        assert_eq!(parse("").to_text(), "");
        let config = SshConfig::load(PathBuf::from("/nonexistent/ssh/config")).unwrap();
        assert!(config.blocks.is_empty());
    }

    #[test]
    fn save_writes_atomically_with_a_backup_and_private_mode() {
        let dir = std::env::temp_dir().join(format!("sshconfig-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        fs::write(&path, "Host old\n").unwrap();

        let mut config = SshConfig::load(path.clone()).unwrap();
        config.blocks.push(Block::new_host("new"));
        config.save(false).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "Host old\n\nHost new\n");
        assert_eq!(
            fs::read_to_string(config.backup_path()).unwrap(),
            "Host old\n"
        );
        assert!(!dir.join("config.sshconfig-tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    fn scratch(name: &str, text: &str) -> (PathBuf, SshConfig) {
        let dir = std::env::temp_dir().join(format!("sshconfig-cfg-{}-{name}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        fs::write(&path, text).unwrap();
        let config = SshConfig::load(path.clone()).unwrap();
        (path, config)
    }

    #[test]
    fn refuses_to_overwrite_changes_made_by_another_program() {
        let (path, mut config) = scratch("conflict", "Host a\n");
        fs::write(&path, "Host a\n\nHost added-in-vim\n").unwrap();

        config.blocks.push(Block::new_host("mine"));
        assert!(matches!(config.save(false), Err(SaveError::Conflict)));
        assert!(
            fs::read_to_string(&path).unwrap().contains("added-in-vim"),
            "their edit is untouched"
        );

        // Forcing is an explicit choice to overwrite.
        config.save(true).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("Host mine"));
        // ...after which our own writes are no longer seen as a conflict.
        config.blocks.push(Block::new_host("again"));
        config.save(false).unwrap();
    }

    #[test]
    fn backs_up_the_original_once_per_session() {
        let (path, mut config) = scratch("backup-once", "Host original\n");
        for name in ["one", "two", "three"] {
            config.blocks.push(Block::new_host(name));
            config.save(false).unwrap();
        }
        assert_eq!(
            fs::read_to_string(config.backup_path()).unwrap(),
            "Host original\n",
            "the backup is the file as it was before this session"
        );
        assert!(fs::read_to_string(&path).unwrap().contains("Host three"));
    }

    #[test]
    fn a_missing_file_is_created_without_a_conflict() {
        let dir =
            std::env::temp_dir().join(format!("sshconfig-cfg-{}-missing", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("config");
        let mut config = SshConfig::load(path.clone()).unwrap();
        config.blocks.push(Block::new_host("first"));
        config.save(false).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "Host first\n");
        assert!(!config.backup_path().exists(), "nothing to back up");
    }
}

/// A throwaway check used by `cargo test -- --ignored real_config`: parses the
/// caller's actual config and asserts we would write it back unchanged.
#[cfg(test)]
mod real_config {
    use super::*;

    #[test]
    #[ignore = "reads the developer's own ~/.ssh/config"]
    fn round_trips_the_users_real_config() {
        let path = default_path();
        let Ok(original) = fs::read_to_string(&path) else {
            return;
        };
        let rendered = SshConfig::parse(path, &original).to_text();
        if rendered != original {
            let before: Vec<&str> = original.lines().collect();
            let after: Vec<&str> = rendered.lines().collect();
            for (i, (a, b)) in before.iter().zip(after.iter()).enumerate() {
                assert_eq!(a, b, "line {} differs", i + 1);
            }
            assert_eq!(before.len(), after.len(), "line count differs");
        }
    }
}
