# sshconfig

A terminal UI for managing `~/.ssh/config`, built with [ratatui](https://ratatui.rs/).

Run `sshconfig` and you get your hosts in a list: arrow around, hit `l` to
log in to one, `enter` to edit it, `a` to add one.

```
 sshconfig  /Users/you/.ssh/config  3 hosts
╭ Hosts ───────────────────────────╮╭ Details ──────────────────────────────╮
│ * *                              ││ Host  nas                             │
│ ● nas  st@10.0.0.4               ││ # the NAS in the cupboard             │
│ ● web-01  deploy@web.example.com ││                                       │
│ ≡ Match host bastion             ││   HostName       10.0.0.4             │
│                                  ││   User           st                   │
│                                  ││   local          8081 → 127.0.0.1:8081│
│                                  ││                                       │
│                                  ││   l ssh nas                           │
╰──────────────────────────────────╯╰───────────────────────────────────────╯
 l login  ↵ edit  a add  d delete  y dup  / search  r reload  ? help  q quit
```

## Install

```sh
cargo install ssh-config-tui --locked
```

That installs a command called `sshconfig`. It needs Rust 1.88 or newer
(`rustup update` if yours is older).

To build from a checkout instead: `cargo install --path . --locked`.

## You don't have to know the directive names

The host editor is a list of rows. Move with `↑`/`↓`, press `enter` to change
whatever you are on, `space` to switch a forward on or off or flip a yes/no,
`d` to remove a row, `a` to add a port forward from anywhere, and `esc` when
you're done. There is no save step: every change is written as you make it.
Every row explains itself in a line of prose at the bottom of the screen.

```
╭ Edit nas ────────────────────────────────────────────╮
│ ▸ Host          nas                                  │
│                                                      │
│ Connection                                           │
│   HostName      10.0.0.4                             │
│   User          st                                   │
│   Port          —                                    │
│   IdentityFile  —                                    │
│                                                      │
│ Port forwards                                        │
│   ● local  8081 → 127.0.0.1:8081                     │
│   ○ local  32400 → 127.0.0.1:32400  off              │
│   + add a port forward                               │
│                                                      │
│ Options                                              │
│   ServerAliveInterval  60                            │
│   + add an option                                    │
╰──────────────────────────────────────────────────────╯
```

**Port forwards** get their own form. Pick a direction — local, remote or SOCKS
— fill in the ports, and it writes the `LocalForward` / `RemoteForward` /
`DynamicForward` line for you. The form shows the exact line it will write as
you type, names the fields after what they do ("Listen on this machine",
"Reaching, from the server") and refuses to save a half-finished tunnel.

`space` on a forward **switches it off** without losing it: the line is
written commented out, `# LocalForward 8081 127.0.0.1:8081`, and `space`
again switches it back on. Forwards you've commented out by hand are picked
up the same way. Comments that merely mention a forward ("# LocalForward for
the database") are left alone — only a line that is a valid forward counts.

**Every other option** comes from a searchable catalogue of all 99 documented
`ssh_config` keywords. You can search by name, by description, or by what you
are actually trying to do — "socks", "bastion", "keepalive", "idle", "multiplex",
"legacy" all lead somewhere useful. Pick one and you get an editor that matches
its type: a value list for yes/no and enum options (so you never have to
remember whether it's `ask` or `confirm`), a text box for the rest, with the
documented default shown next to it.

Values are checked before they are written, using the same rules as ssh's own
parser — a port must be 1–65535, a time looks like `30`, `5m` or `1h30m`, a
choice must be one of its values. That matters because ssh refuses to run at
all, for every host, when the config has a value it can't parse. A rejected
value stays in its box with an explanation, so it can be fixed rather than
retyped.

The catalogue is generated from OpenSSH's own sources — the keyword list,
value types and allowed values come from its parser (`readconf.c`), the
descriptions and defaults from `ssh_config.5` — so it cannot drift from what
your ssh actually accepts:

```sh
python3 tools/gen_schema.py     # regenerates src/schema.rs
```

## Keys

| Key | Action |
| --- | --- |
| `j` `k` `↑` `↓` | move between hosts |
| `g` `G` | jump to first / last |
| `l` | log in: ssh to the selected host |
| `/` | filter by alias, hostname or user |
| `a` | add a host |
| `enter` / `e` | edit the selected host |
| `y` | duplicate the selected host |
| `d` | delete (asks first) |
| `r` | reload the file from disk |
| `?` | help |
| `q` | quit |

In the editor: `↑`/`↓` move, `enter` edits the row or opens its dialog, `a`
adds a port forward, `space` switches a forward on/off or flips a yes/no, `d`
removes the row (asks first), `esc` closes a dialog or leaves the editor.

`/` searches the host's rows by name or value — `/alive` jumps to
`ServerAliveInterval`, `/8081` to the forward on that port. The cursor
follows the first match as you type and every match is marked; `↑`/`↓` step
through them, `enter` stays put, `esc` goes back to where you were. After a
search, `n` and `N` jump to the next and previous match.

The footer always shows the keys that do something right now.

## Small terminals

The layout follows the window. From 72 columns the host list and details sit
side by side; narrower, the details move under the list; when the window is
short as well, the list is shown alone. Dialogs float in the middle while
there's room around them and take the whole window when there isn't. Text
that doesn't fit is cut short with `…` rather than wrapped out of line, and
the key hints drop the least important ones first — the way out (`esc`, `?`)
always stays. Below 36×10 it says how much room it needs.

## How it treats your file

This is a round-trip editor, not a reformatter. Editing one host leaves every
other byte of the file alone — comments, blank lines, indentation, `Match`
blocks and directives it has never heard of all survive untouched. A comment
sitting directly above a `Host` line is treated as that host's label and moves,
copies and deletes along with it.

Within a host you edit, the body is rewritten in a predictable order:
connection details, then forwards, then everything else in the order it was
already in. Directive names are normalised to their canonical spelling
(`port` becomes `Port`). Nothing is dropped: options the editor doesn't model —
unix-socket forwards, say — stay editable as raw text.

Every change is saved the moment it's made. Every write:

- checks the file hasn't been changed by another program since sshconfig read
  it — if it has, it asks whether to overwrite it or reload theirs, rather than
  silently clobbering an edit you made in your editor,
- writes to a temp file in the same directory and renames it into place, so an
  interrupted write can't leave you with half a config,
- creates the file `0600`.

The first write of a session copies the file to `~/.ssh/config.bak`. Later
writes leave the backup alone, so it always holds the file as it was before
you started — with every change saving straight away, that's the useful undo.

Anything that throws something away asks first: deleting a host, removing a
row, overwriting another program's change, and reloading or quitting while a
change couldn't be saved. Status messages clear themselves after a couple of
seconds (errors stay up a little longer).

Entries whose alias contains a wildcard (`Host *`) and `Match` blocks are shown
but can't be connected to. `Match` blocks are shown read-only — their semantics
don't fit the host form, so edit those in your editor.

`Include` directives are displayed as-is; included files aren't followed. Use
`sshconfig --file <path>` to edit one of them directly.

## Usage

```
sshconfig [-f|--file <path>]   # default: ~/.ssh/config
sshconfig --help
sshconfig --version
```

## Development

```sh
cargo test                                     # unit + UI tests
cargo test screens -- --ignored --nocapture    # dump every screen as text
cargo test -- --ignored real_config            # round-trip your own config
cargo test -- --ignored real_hosts             # edit every host, check nothing changes
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this project by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
