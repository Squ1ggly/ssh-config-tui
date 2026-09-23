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
cargo install --path .
```

## You don't have to know the directive names

The host editor is a list of rows. Move with `↑`/`↓`, press `enter` to change
whatever you are on, `space` to flip a yes/no, `d` to remove it, `a` to add a
port forward from anywhere, `ctrl-s` to save. Every row explains itself in a line of prose at the bottom of the form.

```
╭ Edit host ───────────────────────────────────────────╮
│ ▸ Host              nas                              │
│                                                      │
│   Connection                                         │
│   HostName          10.0.0.4                         │
│   User              st                               │
│   Port              —                                │
│   IdentityFile      —                                │
│                                                      │
│   Port forwards                                      │
│   local   8081 → 127.0.0.1:8081                      │
│   local   32400 → 127.0.0.1:32400                    │
│   + add a port forward                               │
│                                                      │
│   Options                                            │
│   ServerAliveInterval  60                            │
│   + add an option                                    │
╰──────────────────────────────────────────────────────╯
```

**Port forwards** get their own form. Pick a direction — local, remote or SOCKS
— fill in the ports, and it writes the `LocalForward` / `RemoteForward` /
`DynamicForward` line for you. The form shows the exact line it will write as
you type, names the fields after what they do ("Listen on this machine",
"Reaching, from the server") and refuses to save a half-finished tunnel.

**Every other option** comes from a searchable catalogue of all 99 documented
`ssh_config` keywords. You can search by name, by description, or by what you
are actually trying to do — "socks", "bastion", "keepalive", "idle", "multiplex",
"legacy" all lead somewhere useful. Pick one and you get an editor that matches
its type: a value list for yes/no and enum options (so you never have to
remember whether it's `ask` or `confirm`), a text box for the rest, with the
documented default shown next to it.

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
adds a port forward, `space` flips a yes/no, `d` removes the row, `ctrl-s`
saves, `esc` cancels or closes a dialog.

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

Saving happens immediately on add, edit, duplicate and delete. Every write:

- copies the previous file to `~/.ssh/config.bak` first,
- writes to a temp file in the same directory and renames it into place, so an
  interrupted write can't leave you with half a config,
- creates the file `0600`.

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
