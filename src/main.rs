//! sshconfig — a terminal UI for managing ~/.ssh/config.

mod app;
mod config;
mod editor;
mod forward;
mod keys;
mod schema;
mod textinput;
mod ui;
mod validate;

use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};

use app::App;
use config::SshConfig;

/// How long to wait for a key when nothing is scheduled.
const IDLE_WAKE: Duration = Duration::from_secs(60);

const USAGE: &str = "\
sshconfig — a terminal UI for ~/.ssh/config

Usage:
  sshconfig [options] [host]

  Naming a host opens its editor straight away.

Options:
  -f, --file <path>   Config file to edit (default: ~/.ssh/config)
  -h, --help          Show this help
  -V, --version       Show the version
";

fn main() -> io::Result<()> {
    let Args { path, host } = match parse_args() {
        Ok(Some(args)) => args,
        Ok(None) => return Ok(()),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };

    let config = SshConfig::load(path)
        .map_err(|e| io::Error::new(e.kind(), format!("could not read config: {e}")))?;
    let mut app = App::new(config);
    if let Some(host) = host {
        app.open_host(&host);
    }

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    result
}

struct Args {
    path: PathBuf,
    /// A host to open in the editor on start.
    host: Option<String>,
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut args = std::env::args().skip(1);
    let mut path = None;
    let mut host = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("sshconfig {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-f" | "--file" => {
                let value = args.next().ok_or("error: --file needs a path")?;
                path = Some(PathBuf::from(value));
            }
            other if other.starts_with('-') || host.is_some() => {
                return Err(format!("error: unknown argument '{other}'\n\n{USAGE}"));
            }
            other => host = Some(other.to_string()),
        }
    }
    Ok(Some(Args {
        path: path.unwrap_or_else(config::default_path),
        host,
    }))
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, app))?;

        // Sleep until a key arrives or the status message is due to vanish,
        // whichever comes first. Resizes also wake us, and simply redraw.
        let timeout = app.status_deadline().map_or(IDLE_WAKE, |deadline| {
            deadline.saturating_duration_since(Instant::now())
        });
        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
        {
            app.on_key(key);
        }
        app.expire_status(Instant::now());

        if let Some(host) = app.connect_to.take() {
            // Hand the terminal back to ssh, then take it again afterwards.
            ratatui::restore();
            let status = Command::new("ssh").arg(&host).status();
            *terminal = ratatui::init();
            terminal.clear()?;

            match status {
                Ok(status) if status.success() => {}
                Ok(status) => app.note_disconnect(&host, status.code()),
                Err(e) => app.note_ssh_error(&host, &e),
            }
        }
    }
    Ok(())
}
