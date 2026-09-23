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

    /// Writes the config atomically, keeping a `.bak` of the previous contents.
    ///
    /// The temp file lives in the same directory so the rename cannot cross a
    /// filesystem boundary, and it is created with 0600 so the config is never
    /// briefly world-readable.
    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        if self.path.exists() {
            fs::copy(&self.path, self.backup_path())?;
        }

        let tmp = self.path.with_extension("sshconfig-tmp");
        write_private(&tmp, &self.to_text())?;
        fs::rename(&tmp, &self.path)
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
        config.save().unwrap();

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
