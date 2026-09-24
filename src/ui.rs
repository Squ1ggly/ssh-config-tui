//! All drawing. Colours stick to the 16 ANSI slots plus a couple of indexed
//! greys so the TUI inherits whatever theme the user's terminal already has.
//!
//! The layout follows the terminal's size. With room, the host list and the
//! details sit side by side; narrower, the details move under the list; when
//! both width and height are tight, the list is shown alone. Dialogs float
//! in the middle when there is space around them and take the whole screen
//! when there isn't. Text that doesn't fit is cut with "…" rather than wrapped
//! into the next column.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block as Border, BorderType, Clear, List, ListItem, Padding, Paragraph};

use crate::app::{App, Mode, StatusKind};
use crate::config::{BlockKind, display_path, split_directive};
use crate::editor::{Editor, ForwardForm, Overlay, Row, RowSearch};
use crate::forward::{Forward, ForwardKind};
use crate::schema::Kind;

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;

/// Below this there isn't room for anything useful, so we say so instead.
pub const MIN_WIDTH: u16 = 36;
pub const MIN_HEIGHT: u16 = 10;
/// From this width the details pane sits beside the list...
const SIDE_BY_SIDE_WIDTH: u16 = 72;
/// ...and below it, under the list, if the body is at least this tall.
const STACKED_HEIGHT: u16 = 20;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        draw_too_small(frame, area);
        return;
    }

    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, app, header);

    // A confirm dialog opened from the editor sits over the editor.
    if app.editor.is_some() && matches!(app.mode, Mode::Edit | Mode::Confirm) {
        draw_editor(frame, app, body);
    } else {
        draw_hosts(frame, app, body);
    }

    match app.mode {
        Mode::Confirm => draw_confirm(frame, app, body),
        Mode::Help => draw_help(frame, body),
        _ => {}
    }

    draw_footer(frame, app, footer);
}

fn draw_too_small(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::styled("sshconfig", Style::new().fg(ACCENT).bold()),
        Line::styled(
            format!("needs {MIN_WIDTH}×{MIN_HEIGHT}"),
            Style::new().fg(Color::Gray),
        ),
        Line::styled(
            format!("this is {}×{}", area.width, area.height),
            Style::new().fg(MUTED),
        ),
    ];
    let top = area.height.saturating_sub(lines.len() as u16) / 2;
    let area = Rect {
        y: area.y + top,
        height: area.height - top,
        ..area
    };
    frame.render_widget(
        Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center),
        area,
    );
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let hosts = app
        .config
        .blocks
        .iter()
        .filter(|b| b.kind == BlockKind::Host)
        .count();
    let title = " sshconfig ";
    let count = format!("  {hosts} host{}", if hosts == 1 { "" } else { "s" });

    let mut spans = vec![
        Span::styled(title, Style::new().fg(Color::Black).bg(ACCENT).bold()),
        Span::raw(" "),
    ];
    let room = (area.width as usize).saturating_sub(title.len() + 1 + count.len());
    // A path cut down to a few characters says nothing; drop it instead.
    if room >= 8 {
        spans.push(Span::styled(
            truncate_left(&display_path(&app.config.path), room),
            Style::new().fg(MUTED),
        ));
    }
    spans.push(Span::styled(count, Style::new().fg(MUTED)));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The host list, with the details pane beside it, under it, or not at all.
fn draw_hosts(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.width >= SIDE_BY_SIDE_WIDTH {
        let list_width = (area.width * 38 / 100).clamp(26, 48);
        let [left, right] =
            Layout::horizontal([Constraint::Length(list_width), Constraint::Min(0)]).areas(area);
        draw_list(frame, app, left);
        draw_details(frame, app, right);
    } else if area.height >= STACKED_HEIGHT {
        let wanted = app.visible().len() as u16 + 2;
        let list_height = wanted.clamp(5, area.height / 2);
        let [top, bottom] =
            Layout::vertical([Constraint::Length(list_height), Constraint::Min(0)]).areas(area);
        draw_list(frame, app, top);
        draw_details(frame, app, bottom);
    } else {
        draw_list(frame, app, area);
    }
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let visible = app.visible();
    // The border takes two columns and the marker three.
    let room = (area.width as usize).saturating_sub(5);

    let items: Vec<ListItem> = visible
        .iter()
        .map(|i| {
            let block = &app.config.blocks[*i];
            let (marker, marker_style) = match block.kind {
                BlockKind::Match => ("≡", Style::new().fg(Color::Magenta)),
                BlockKind::Host if block.is_pattern() => ("*", Style::new().fg(Color::Yellow)),
                BlockKind::Host => ("●", Style::new().fg(Color::Green)),
            };
            let label = truncate(&block.label(), room);
            let mut spans = vec![
                Span::styled(format!(" {marker} "), marker_style),
                Span::styled(label.clone(), Style::new().bold()),
            ];

            // user@host goes after the alias only while there's space for
            // enough of it to be recognisable.
            let target = match (block.get("User"), block.get("HostName")) {
                (Some(user), Some(host)) => format!("{user}@{host}"),
                (None, Some(host)) => host,
                _ => String::new(),
            };
            let target_room = room.saturating_sub(width(&label) + 2);
            if !target.is_empty() && target_room >= 6 {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    truncate(&target, target_room),
                    Style::new().fg(MUTED),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let title = if app.filter.is_empty() {
        " Hosts ".to_string()
    } else {
        format!(" Hosts · {} of {} ", visible.len(), app.config.blocks.len())
    };

    let list = List::new(items)
        .block(bordered(
            title,
            app.mode == Mode::List || app.mode == Mode::Search,
        ))
        .highlight_style(Style::new().bg(ACCENT).fg(Color::Black).bold())
        .highlight_symbol("");

    frame.render_stateful_widget(list, area, &mut app.list_state);
}

fn draw_details(frame: &mut Frame, app: &App, area: Rect) {
    let border = bordered(" Details ".into(), false).padding(Padding::horizontal(1));
    let inner = border.inner(area);
    frame.render_widget(border, area);
    let room = inner.width as usize;

    let Some(block) = app.selected() else {
        frame.render_widget(
            Paragraph::new(vec![
                Line::raw(""),
                Line::styled(truncate("No hosts yet.", room), Style::new().fg(MUTED)),
                Line::styled(
                    truncate("Press a to add one.", room),
                    Style::new().fg(MUTED),
                ),
            ]),
            inner,
        );
        return;
    };

    let mut lines = vec![Line::from(vec![
        Span::styled("Host  ", Style::new().fg(MUTED)),
        Span::styled(
            truncate(&block.patterns().join(" "), room.saturating_sub(6)),
            Style::new().fg(ACCENT).bold(),
        ),
    ])];
    for comment in &block.leading {
        lines.push(Line::styled(
            truncate(comment.trim(), room),
            Style::new().fg(MUTED).italic(),
        ));
    }
    lines.push(Line::raw(""));

    // (label, value, label colour, whether the row is switched off)
    enum Entry {
        Pair(String, String, Color, bool),
        Note(String),
    }
    let entries: Vec<Entry> = block
        .body
        .iter()
        .map(|raw| match split_directive(raw) {
            // Forwards read better as what they do than as their arguments.
            Some((key, value)) => match Forward::parse(key, value) {
                Some(forward) => Entry::Pair(
                    forward.kind.label().into(),
                    forward.describe(),
                    Color::Magenta,
                    false,
                ),
                None => Entry::Pair(key.into(), value.into(), Color::Blue, false),
            },
            None => match Forward::parse_disabled(raw) {
                Some(forward) => Entry::Pair(
                    forward.kind.label().into(),
                    format!("{}  off", forward.describe()),
                    MUTED,
                    true,
                ),
                None => Entry::Note(raw.trim().to_string()),
            },
        })
        .collect();

    // Labels get the space they need, up to a third of the pane.
    let longest = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Pair(label, ..) => Some(width(label)),
            Entry::Note(_) => None,
        })
        .max()
        .unwrap_or(0);
    let label_width = longest.min((room / 3).max(6));
    let value_room = room.saturating_sub(label_width + 2);

    if entries.is_empty() {
        lines.push(Line::styled("(no options)", Style::new().fg(MUTED)));
    }
    for entry in entries {
        lines.push(match entry {
            Entry::Pair(label, value, colour, off) => Line::from(vec![
                Span::styled(
                    pad(&truncate(&label, label_width), label_width + 2),
                    Style::new().fg(colour),
                ),
                Span::styled(
                    truncate(&value, value_room),
                    if off {
                        Style::new().fg(MUTED)
                    } else {
                        Style::new()
                    },
                ),
            ]),
            Entry::Note(text) => {
                Line::styled(truncate(&text, room), Style::new().fg(MUTED).italic())
            }
        });
    }

    if block.kind == BlockKind::Host && !block.is_pattern() {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled("l ", Style::new().fg(Color::Green)),
            Span::styled(
                truncate(&format!("ssh {}", block.label()), room.saturating_sub(2)),
                Style::new().fg(MUTED),
            ),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let room = area.width as usize;

    if app.mode == Mode::Search {
        let hint = "   ↵ accept · esc clear";
        let filter_room = room.saturating_sub(3 + hint.len());
        let line = Line::from(vec![
            Span::styled(" /", Style::new().fg(ACCENT).bold()),
            Span::raw(truncate_left(&app.filter, filter_room)),
            Span::styled("▌", Style::new().fg(ACCENT)),
            Span::styled(if room > 30 { hint } else { "" }, Style::new().fg(MUTED)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    if let Some(editor) = app.editor.as_ref().filter(|_| app.mode == Mode::Edit)
        && let Some(search) = editor.search.as_ref()
    {
        draw_row_search(frame, editor, search, area);
        return;
    }

    if let Some(status) = &app.status {
        let style = match status.kind {
            StatusKind::Info => Style::new().fg(Color::Green),
            StatusKind::Error => Style::new().fg(Color::Red).bold(),
        };
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!(" {}", truncate(&status.text, room.saturating_sub(1))),
                style,
            )),
            area,
        );
        return;
    }

    let (keys, pinned): (Vec<Hint>, Option<Hint>) = match app.mode {
        Mode::Edit => editor_hints(app.editor.as_ref()),
        Mode::Confirm => (vec![("any other key", "cancel")], None),
        Mode::Help => (vec![("any key", "close")], None),
        _ => (
            vec![
                ("l", "login"),
                ("↵", "edit"),
                ("a", "add"),
                ("d", "delete"),
                ("/", "search"),
                ("y", "dup"),
                ("r", "reload"),
                ("q", "quit"),
            ],
            Some(("?", "help")),
        ),
    };
    frame.render_widget(Paragraph::new(fit_hints(&keys, pinned, room)), area);
}

type Hint = (&'static str, &'static str);

/// The editor's `/` prompt: the query, where the cursor is among the matches,
/// and the keys, as far as they fit.
fn draw_row_search(frame: &mut Frame, editor: &Editor, search: &RowSearch, area: Rect) {
    let room = area.width as usize;
    let query = search.query.value();
    let matches = editor.search_matches(&query);
    let (count, count_style) = if query.trim().is_empty() {
        (String::new(), Style::new())
    } else if matches.is_empty() {
        ("no match".to_string(), Style::new().fg(Color::Red).bold())
    } else {
        let position = matches.iter().position(|&i| i == editor.cursor);
        let text = match position {
            Some(position) => format!("{} of {}", position + 1, matches.len()),
            None => format!("{} matches", matches.len()),
        };
        (text, Style::new().fg(Color::Yellow))
    };

    let hint = "  ↵ jump · ↑↓ next · esc back";
    let fixed = 3 + 2 + width(&count);
    let show_hint = room >= fixed + 8 + width(hint);
    let query_room = room.saturating_sub(fixed + if show_hint { width(hint) } else { 0 });
    let line = Line::from(vec![
        Span::styled(" /", Style::new().fg(ACCENT).bold()),
        Span::raw(truncate_left(&query, query_room)),
        Span::styled("▌", Style::new().fg(ACCENT)),
        Span::raw("  "),
        Span::styled(count, count_style),
        Span::styled(if show_hint { hint } else { "" }, Style::new().fg(MUTED)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// The keys that do something right now in the editor, and the one that
/// must never be squeezed out: how to get back.
fn editor_hints(editor: Option<&Editor>) -> (Vec<Hint>, Option<Hint>) {
    let Some(editor) = editor else {
        return (Vec::new(), None);
    };
    if editor.inline.is_some() {
        return (vec![("↵", "accept")], Some(("esc", "discard")));
    }
    let keys = match &editor.overlay {
        Some(Overlay::Picker(_)) => vec![("↵", "add"), ("↑↓", "move")],
        Some(Overlay::Choice(_)) => vec![("↵", "choose"), ("↑↓", "move")],
        Some(Overlay::Forward(_)) => vec![("↵", "done"), ("↑↓", "field"), ("←→", "direction")],
        Some(Overlay::ConfirmRemove(_)) => {
            return (vec![("y", "remove")], Some(("any other key", "keep")));
        }
        None => {
            let mut keys = vec![("↵", "edit")];
            match editor.current_row() {
                Row::Forward(i) if editor.forwards[i].enabled => keys.push(("space", "switch off")),
                Row::Forward(_) => keys.push(("space", "switch on")),
                row if editor
                    .help_for_cursor()
                    .is_some_and(|k| k.values == ["yes", "no"])
                    && row != Row::Alias =>
                {
                    keys.push(("space", "toggle"))
                }
                _ => {}
            }
            keys.extend([("a", "add forward"), ("d", "remove"), ("/", "search")]);
            return (keys, Some(("esc", "done")));
        }
    };
    (keys, Some(("esc", "back")))
}

/// The host editor: a scrolling list of rows, with dialogs on top. It takes
/// the whole body — it's a screen of its own, not a popup over the list.
fn draw_editor(frame: &mut Frame, app: &App, area: Rect) {
    let Some(editor) = app.editor.as_ref() else {
        return;
    };
    frame.render_widget(Clear, area);

    let title = if editor.is_new() {
        " New host ".to_string()
    } else {
        format!(
            " Edit {} ",
            truncate(&editor.alias, (area.width as usize).saturating_sub(12))
        )
    };
    let outer = bordered(title, true).padding(Padding::horizontal(1));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    // A line or two of prose about whatever the cursor is on, so option names
    // aren't something you have to already know. Dropped when space is short.
    let prose_height = match inner.height {
        16.. => 2,
        10.. => 1,
        _ => 0,
    };
    let [rows_area, prose_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(prose_height)]).areas(inner);

    draw_editor_rows(frame, editor, rows_area);
    if prose_height > 0 {
        draw_prose(frame, editor, prose_area);
    }

    match &editor.overlay {
        Some(Overlay::Picker(_)) => draw_picker(frame, editor, area),
        Some(Overlay::Choice(_)) => draw_choice(frame, editor, area),
        Some(Overlay::Forward(_)) => draw_forward_form(frame, editor, area),
        Some(Overlay::ConfirmRemove(row)) => draw_confirm_box(
            frame,
            area,
            " Remove ",
            &format!(
                "Remove {}? It is taken out of the file straight away.",
                editor.describe_row(*row)
            ),
            &[('y', "remove")],
        ),
        None => {}
    }
}

fn draw_prose(frame: &mut Frame, editor: &Editor, area: Rect) {
    let text = match editor.help_for_cursor() {
        Some(keyword) => match keyword.default {
            Some(default) => format!("{} (default: {default})", keyword.desc),
            None => keyword.desc.to_string(),
        },
        None => match editor.current_row() {
            Row::Alias => {
                "The name you type after ssh. Space-separate to give a host several aliases.".into()
            }
            Row::Forward(i) if !editor.forwards[i].enabled => {
                "Switched off: kept in the file as a comment. Space switches it back on.".into()
            }
            Row::Forward(_) | Row::AddForward => "Tunnel a port through this connection.".into(),
            Row::AddOption => "Search all 99 ssh_config options by name or description.".into(),
            _ => String::new(),
        },
    };
    let lines: Vec<Line> = fit_lines(wrap_words(&text, area.width as usize), area)
        .into_iter()
        .map(|line| Line::styled(line, Style::new().fg(Color::Gray)))
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// The first lines of wrapped text that fit in `area`, with the last one
/// ending in "…" if any were left out.
fn fit_lines(mut lines: Vec<String>, area: Rect) -> Vec<String> {
    let height = area.height as usize;
    if lines.len() > height && height > 0 {
        lines.truncate(height);
        let last = lines.last_mut().unwrap();
        *last = truncate(&format!("{last} …"), area.width as usize);
        if !last.ends_with('…') {
            last.push('…');
        }
    }
    lines
}

/// Builds the rows, with section headings, and scrolls to keep the cursor in view.
fn draw_editor_rows(frame: &mut Frame, editor: &Editor, area: Rect) {
    let rows = editor.rows();
    let room = area.width as usize;
    // While searching, every match is marked, not just the one under the cursor.
    let matches = editor
        .search
        .as_ref()
        .map(|search| editor.search_matches(&search.query.value()))
        .unwrap_or_default();

    // Labels get the width of the longest one, but never more than two fifths
    // of the row, so the values always have space.
    let longest = rows
        .iter()
        .filter(|row| matches!(row, Row::Alias | Row::Essential(_) | Row::Option(_)))
        .map(|row| width(&editor.row_label(*row)))
        .max()
        .unwrap_or(4);
    let label_width = longest.min((room * 2 / 5).max(4));
    let value_room = room.saturating_sub(2 + label_width + 2).max(1);

    // Values are typed in place, so a long one (a ProxyCommand, say) has to
    // scroll sideways to keep the cursor on screen.
    let inline_scroll = editor.inline.as_ref().map_or(0, |input| {
        input.col.saturating_sub(value_room.saturating_sub(1))
    });

    // (row index this line belongs to, rendered line)
    let mut lines: Vec<(Option<usize>, Line)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        match row {
            Row::Essential(0) => section(&mut lines, "Connection"),
            Row::Forward(0) | Row::AddForward if !matches!(rows[index - 1], Row::Forward(_)) => {
                section(&mut lines, "Port forwards")
            }
            Row::Option(0) => section(&mut lines, "Options"),
            Row::AddOption if !matches!(rows[index - 1], Row::Option(_)) => {
                section(&mut lines, "Options")
            }
            _ => {}
        }

        let focused = index == editor.cursor;
        let matched = !focused && matches.contains(&index);
        let match_style = Style::new().fg(Color::Yellow).bold();
        let marker = if focused {
            Span::styled("▸ ", Style::new().fg(ACCENT))
        } else if matched {
            Span::styled("› ", match_style)
        } else {
            Span::raw("  ")
        };
        let emphasis = if focused {
            Style::new().fg(ACCENT).bold()
        } else {
            Style::new()
        };

        let line = match row {
            Row::AddForward | Row::AddOption => Line::from(vec![
                marker,
                Span::styled(
                    truncate(&editor.row_label(*row), room.saturating_sub(2)),
                    if focused {
                        emphasis
                    } else {
                        Style::new().fg(MUTED)
                    },
                ),
            ]),
            Row::Forward(i) => {
                let forward = &editor.forwards[*i];
                // A filled dot for a live forward, hollow for a switched-off one.
                let (dot, dot_style, text_style) = if forward.enabled {
                    ("● ", Style::new().fg(Color::Green), emphasis)
                } else {
                    ("○ ", Style::new().fg(MUTED), Style::new().fg(MUTED))
                };
                let mut spans = vec![
                    marker,
                    Span::styled(dot, dot_style),
                    Span::styled(
                        format!("{:<7}", forward.kind.label()),
                        if matched {
                            match_style
                        } else if forward.enabled {
                            Style::new().fg(Color::Magenta)
                        } else {
                            Style::new().fg(MUTED)
                        },
                    ),
                ];
                let tail = if forward.enabled { "" } else { "  off" };
                let describe_room = room.saturating_sub(2 + 2 + 7 + tail.len());
                spans.push(Span::styled(
                    truncate(&forward.describe(), describe_room),
                    text_style,
                ));
                spans.push(Span::styled(tail, Style::new().fg(Color::Yellow)));
                Line::from(spans)
            }
            _ => {
                let label = editor.row_label(*row);
                let value = editor.row_value(*row);
                let is_comment = label == "#";
                let inline = editor.inline.as_ref().filter(|_| focused);
                let (shown, value_style) = if let Some(input) = inline {
                    let shown: String = input
                        .value()
                        .chars()
                        .skip(inline_scroll)
                        .take(value_room)
                        .collect();
                    (shown, Style::new().fg(Color::Black).bg(ACCENT))
                } else if value.trim().is_empty() {
                    ("—".to_string(), Style::new().fg(MUTED))
                } else if is_comment {
                    (
                        truncate(&value, value_room),
                        Style::new().fg(MUTED).italic(),
                    )
                } else {
                    (truncate(&value, value_room), Style::new())
                };
                let label_style = if matched {
                    match_style
                } else if is_comment {
                    Style::new().fg(MUTED)
                } else if focused {
                    emphasis
                } else {
                    Style::new().fg(Color::Blue)
                };
                Line::from(vec![
                    marker,
                    Span::styled(
                        pad(&truncate(&label, label_width), label_width + 2),
                        label_style,
                    ),
                    Span::styled(shown, value_style),
                ])
            }
        };
        lines.push((Some(index), line));
    }

    // Scroll so the focused row stays on screen, with a line of context below.
    let focus_line = lines
        .iter()
        .position(|(i, _)| *i == Some(editor.cursor))
        .unwrap_or(0);
    let height = area.height as usize;
    let offset = (focus_line + 2)
        .saturating_sub(height)
        .min(lines.len().saturating_sub(height));

    let visible: Vec<Line> = lines
        .into_iter()
        .skip(offset)
        .take(height)
        .map(|(_, line)| line)
        .collect();
    frame.render_widget(Paragraph::new(visible), area);

    // Park the terminal cursor inside the value being typed.
    if let Some(input) = editor.inline.as_ref() {
        let y = area.y + focus_line.saturating_sub(offset) as u16;
        let x = area.x + (2 + label_width + 2 + input.col - inline_scroll) as u16;
        if y < area.y + area.height && x < area.x + area.width {
            frame.set_cursor_position((x, y));
        }
    }
}

fn section(lines: &mut Vec<(Option<usize>, Line<'static>)>, title: &str) {
    if !lines.is_empty() {
        lines.push((None, Line::raw("")));
    }
    lines.push((
        None,
        Line::styled(
            title.to_string(),
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
    ));
}

/// The searchable catalogue of every ssh_config keyword.
fn draw_picker(frame: &mut Frame, editor: &Editor, area: Rect) {
    let Some(Overlay::Picker(picker)) = editor.overlay.as_ref() else {
        return;
    };

    let popup = dialog(area, dialog_width(area, 74), area.height);
    frame.render_widget(Clear, popup);
    let outer = bordered(" Add an option ".into(), true).padding(Padding::horizontal(1));
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);

    let description_height = match inner.height {
        12.. => 2,
        7.. => 1,
        _ => 0,
    };
    let gap = u16::from(inner.height >= 8);
    let [search_area, _, list_area, desc_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(gap),
        Constraint::Min(1),
        Constraint::Length(description_height),
    ])
    .areas(inner);

    let room = inner.width as usize;
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("search ", Style::new().fg(MUTED)),
            Span::styled(
                truncate_left(&picker.query.value(), room.saturating_sub(8)),
                Style::new().fg(ACCENT).bold(),
            ),
            Span::styled("▌", Style::new().fg(ACCENT)),
        ])),
        search_area,
    );

    let matches = picker.matches();
    // The type column goes first when space runs out.
    let show_kind = room >= 40;
    let name_width = if show_kind {
        28.min(room.saturating_sub(2 + 11))
    } else {
        room.saturating_sub(2)
    };

    let height = list_area.height as usize;
    let offset = picker.index.saturating_sub(height.saturating_sub(1));
    let lines: Vec<Line> = matches
        .iter()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(index, keyword)| {
            let focused = index == picker.index;
            let style = if focused {
                Style::new().fg(Color::Black).bg(ACCENT).bold()
            } else if keyword.common {
                Style::new()
            } else {
                Style::new().fg(Color::Gray)
            };
            let mut spans = vec![
                Span::styled(if focused { "▸ " } else { "  " }, Style::new().fg(ACCENT)),
                Span::styled(pad(&truncate(keyword.name, name_width), name_width), style),
            ];
            if show_kind {
                spans.push(Span::styled(
                    format!(" {}", kind_label(keyword.kind)),
                    Style::new().fg(if focused { ACCENT } else { MUTED }),
                ));
            }
            Line::from(spans)
        })
        .collect();
    let lines = if lines.is_empty() {
        vec![Line::styled("no match", Style::new().fg(MUTED))]
    } else {
        lines
    };
    frame.render_widget(Paragraph::new(lines), list_area);

    if let Some(keyword) = picker.selected().filter(|_| description_height > 0) {
        let text = match keyword.default {
            Some(default) => format!("{} (default: {default})", keyword.desc),
            None => keyword.desc.to_string(),
        };
        let lines: Vec<Line> = fit_lines(wrap_words(&text, room), desc_area)
            .into_iter()
            .map(|line| Line::styled(line, Style::new().fg(Color::Gray)))
            .collect();
        frame.render_widget(Paragraph::new(lines), desc_area);
    }
}

/// The yes/no and one-of-N value list.
fn draw_choice(frame: &mut Frame, editor: &Editor, area: Rect) {
    let Some(Overlay::Choice(choice)) = editor.overlay.as_ref() else {
        return;
    };

    let popup_width = dialog_width(area, 56);
    let room = (popup_width as usize).saturating_sub(4);
    let mut description = wrap_words(choice.desc, room);
    description.truncate(3);

    // Borders, description, a gap, then the values — minus the gap and then
    // the description if that is more than fits.
    let values = choice.values.len();
    let mut lines: Vec<Line> = Vec::new();
    let full = 2 + description.len() + 1 + values;
    if full <= area.height as usize {
        lines.extend(
            description
                .iter()
                .map(|d| Line::styled(d.clone(), Style::new().fg(Color::Gray))),
        );
        lines.push(Line::raw(""));
    }
    for (index, value) in choice.values.iter().enumerate() {
        let focused = index == choice.index;
        let is_default = choice.default == Some(*value);
        lines.push(Line::from(vec![
            Span::styled(if focused { "▸ " } else { "  " }, Style::new().fg(ACCENT)),
            Span::styled(
                format!("{value:<12}"),
                if focused {
                    Style::new().fg(ACCENT).bold()
                } else {
                    Style::new()
                },
            ),
            Span::styled(
                if is_default { "default" } else { "" },
                Style::new().fg(MUTED).italic(),
            ),
        ]));
    }

    let popup = dialog(area, popup_width, lines.len() as u16 + 2);
    frame.render_widget(Clear, popup);
    let outer = bordered(format!(" {} ", choice.keyword), true).padding(Padding::horizontal(1));
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);

    // Keep the highlighted value in view when the list is cut short.
    let focus_line = lines.len() - values + choice.index;
    let offset = (focus_line + 1).saturating_sub(inner.height as usize);
    frame.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), inner);
}

/// The port-forward form.
fn draw_forward_form(frame: &mut Frame, editor: &Editor, area: Rect) {
    let Some(Overlay::Forward(form)) = editor.overlay.as_ref() else {
        return;
    };

    let popup_width = dialog_width(area, 64);
    let room = (popup_width as usize).saturating_sub(4);
    let label_width = 13;
    let value_room = room.saturating_sub(4 + label_width).max(1);

    // Each line, whether it can be dropped when space is short, and for the
    // field being typed into, where its value starts.
    struct FormLine {
        line: Line<'static>,
        optional: bool,
        cursor: Option<usize>,
    }
    let mut lines: Vec<FormLine> = Vec::new();
    let mut push = |line: Line<'static>, optional: bool, cursor: Option<usize>| {
        lines.push(FormLine {
            line,
            optional,
            cursor,
        })
    };

    // Direction, as three selectable words rather than a keyword to remember.
    let mut direction = vec![Span::styled(
        if form.field == 0 { "▸ " } else { "  " },
        Style::new().fg(ACCENT),
    )];
    for kind in ForwardKind::ALL {
        let selected = kind == form.forward.kind;
        direction.push(Span::styled(
            format!(" {} ", kind.label()),
            if selected {
                Style::new().fg(Color::Black).bg(ACCENT).bold()
            } else {
                Style::new().fg(MUTED)
            },
        ));
        direction.push(Span::raw(" "));
    }
    push(Line::from(direction), false, None);
    for summary in wrap_words(form.forward.kind.summary(), room.saturating_sub(2)) {
        push(
            Line::styled(format!("  {summary}"), Style::new().fg(Color::Gray)),
            true,
            None,
        );
    }

    for field in 0..form.visible_fields() {
        if field == 0 || field == 2 {
            push(Line::raw(""), true, None);
            let heading = if field == 0 {
                form.forward.kind.listen_label()
            } else {
                form.forward.kind.dest_label()
            };
            push(
                Line::styled(truncate(heading, room), Style::new().fg(Color::Yellow)),
                false,
                None,
            );
        }
        let focused = form.field == field + 1;
        let input = &form.inputs[field];
        let value = input.value();
        let scroll = if focused {
            input.col.saturating_sub(value_room.saturating_sub(1))
        } else {
            0
        };
        let (shown, style) = if value.is_empty() && !focused {
            // The bind address is the one field that is fine to leave empty.
            let placeholder = if field == 0 { "optional" } else { "—" };
            (placeholder.to_string(), Style::new().fg(MUTED))
        } else if focused {
            (
                value.chars().skip(scroll).take(value_room).collect(),
                Style::new().fg(Color::Black).bg(ACCENT),
            )
        } else {
            (truncate(&value, value_room), Style::new())
        };
        push(
            Line::from(vec![
                Span::styled(if focused { "▸ " } else { "  " }, Style::new().fg(ACCENT)),
                Span::styled(
                    pad(ForwardForm::FIELDS[field], label_width + 2),
                    Style::new().fg(if focused { ACCENT } else { Color::Blue }),
                ),
                Span::styled(shown, style),
            ]),
            false,
            focused.then_some(2 + label_width + 2 + input.col - scroll),
        );
    }

    // A live preview of the line this will write, so there is no mystery about
    // what lands in the file.
    push(Line::raw(""), true, None);
    let preview_style = if form.forward.is_complete() {
        Style::new().fg(Color::Green)
    } else {
        Style::new().fg(MUTED).italic()
    };
    // Blanks show as <port>/<host>, so the preview reads as a template
    // rather than a broken line.
    let mut preview = form.forward.clone();
    for (value, placeholder) in [
        (&mut preview.listen, "<port>"),
        (&mut preview.host, "<host>"),
        (&mut preview.port, "<port>"),
    ] {
        if value.trim().is_empty() {
            *value = placeholder.to_string();
        }
    }
    push(
        Line::from(vec![
            Span::styled("writes ", Style::new().fg(MUTED)),
            Span::styled(
                truncate(&preview.file_line(), room.saturating_sub(7)),
                preview_style,
            ),
        ]),
        false,
        None,
    );
    if let Some(error) = &form.error {
        for part in wrap_words(error, room) {
            push(
                Line::styled(part, Style::new().fg(Color::Red).bold()),
                false,
                None,
            );
        }
    }

    // Too tall for the screen: drop the spacing and the summary first.
    let available = area.height.saturating_sub(2) as usize;
    if lines.len() > available {
        lines.retain(|line| !line.optional);
    }

    let title = if form.index.is_some() {
        " Edit port forward "
    } else {
        " New port forward "
    };
    let popup = dialog(area, popup_width, lines.len() as u16 + 2);
    frame.render_widget(Clear, popup);
    let outer = bordered(title.into(), true).padding(Padding::horizontal(1));
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);

    // Still too tall: scroll to keep the field being edited in view.
    let focus_line = lines
        .iter()
        .position(|line| line.cursor.is_some())
        .unwrap_or(0);
    let offset = (focus_line + 1).saturating_sub(inner.height as usize);
    if let Some(column) = lines.get(focus_line).and_then(|line| line.cursor) {
        let x = inner.x + column as u16;
        let y = inner.y + (focus_line - offset) as u16;
        if x < inner.x + inner.width && y < inner.y + inner.height {
            frame.set_cursor_position((x, y));
        }
    }
    let lines: Vec<Line> = lines.into_iter().map(|line| line.line).collect();
    frame.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), inner);
}

fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Flag => "yes/no",
        Kind::Choice => "choice",
        Kind::Int => "number",
        Kind::Time => "time",
        Kind::Path => "path",
        Kind::Command => "command",
        Kind::Algorithms => "algorithms",
        Kind::Text => "text",
    }
}

/// As many key hints as fit in `room`, in order, with `pinned` always last.
fn fit_hints(keys: &[(&str, &str)], pinned: Option<(&str, &str)>, room: usize) -> Line<'static> {
    let cost = |(key, label): &(&str, &str)| width(key) + 1 + width(label) + 2;
    let mut budget = room.saturating_sub(1 + pinned.as_ref().map_or(0, cost));

    let mut spans = vec![Span::raw(" ")];
    let add = |spans: &mut Vec<Span<'static>>, (key, label): (&str, &str)| {
        spans.push(Span::styled(
            key.to_string(),
            Style::new().fg(ACCENT).bold(),
        ));
        spans.push(Span::styled(format!(" {label}  "), Style::new().fg(MUTED)));
    };
    for hint in keys {
        if cost(hint) > budget {
            break;
        }
        budget -= cost(hint);
        add(&mut spans, *hint);
    }
    if let Some(hint) = pinned {
        add(&mut spans, hint);
    }
    Line::from(spans)
}

/// The app-level confirm dialog (delete, overwrite, reload, quit).
fn draw_confirm(frame: &mut Frame, app: &App, area: Rect) {
    let Some(confirm) = app.confirm.as_ref() else {
        return;
    };
    let choices: Vec<(char, &str)> = confirm
        .choices
        .iter()
        .map(|(key, label, _)| (*key, *label))
        .collect();
    draw_confirm_box(frame, area, confirm.title, &confirm.message, &choices);
}

/// A red-bordered question with its answers, sized to its text.
fn draw_confirm_box(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    message: &str,
    choices: &[(char, &str)],
) {
    let popup_width = dialog_width(area, 60);
    let room = (popup_width as usize).saturating_sub(4);

    let mut lines: Vec<Line> = wrap_words(message, room)
        .into_iter()
        .map(Line::raw)
        .collect();
    lines.push(Line::raw(""));
    for (key, label) in choices {
        lines.push(Line::from(vec![
            Span::styled(format!("{key}  "), Style::new().fg(Color::Red).bold()),
            Span::raw(truncate(label, room.saturating_sub(3))),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled("any other key  ", Style::new().fg(MUTED)),
        Span::styled("cancel", Style::new().fg(MUTED)),
    ]));

    let popup = dialog(area, popup_width, lines.len() as u16 + 2);
    frame.render_widget(Clear, popup);
    let border = Border::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::Red))
        .title(Span::styled(
            title.to_string(),
            Style::new().add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::horizontal(1));
    frame.render_widget(Paragraph::new(lines).block(border), popup);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let rows = [
        ("j / k / ↑ / ↓", "move between hosts"),
        ("g / G", "jump to first / last"),
        ("ctrl+d / ctrl+u", "page down / up (also pgdn / pgup)"),
        ("l", "log in (ssh to the selected host)"),
        ("/", "filter by alias, hostname or user"),
        ("a", "add a new host"),
        ("enter / e", "edit the selected host"),
        ("y", "duplicate the selected host"),
        ("d", "delete (asks first)"),
        ("r", "reload the file from disk"),
        ("q / esc", "quit"),
        ("", ""),
        ("in the editor", ""),
        ("↑ / ↓", "move between rows"),
        ("ctrl+d / ctrl+u", "page down / up"),
        ("enter", "edit the row, or open its dialog"),
        ("space", "switch a forward on/off, flip a yes/no"),
        ("a", "add a port forward"),
        ("d", "remove the row (asks first)"),
        ("/", "search the rows; n / N for next / previous"),
        ("esc", "done, or close a dialog"),
        ("", ""),
        ("", "Every change is saved as you make it."),
        ("", "The file as it was before this session is kept"),
        ("", "as config.bak."),
    ];

    let popup = dialog(area, dialog_width(area, 64), rows.len() as u16 + 2);
    frame.render_widget(Clear, popup);
    let border = bordered(" Keys ".into(), true).padding(Padding::horizontal(1));
    let inner = border.inner(popup);
    frame.render_widget(border, popup);

    let room = inner.width as usize;
    let key_width = if room >= 50 { 16 } else { 11 };
    let mut lines: Vec<Line> = rows
        .iter()
        .map(|(key, description)| {
            if key.is_empty() {
                return Line::styled(truncate(description, room), Style::new().fg(MUTED));
            }
            Line::from(vec![
                Span::styled(pad(key, key_width + 2), Style::new().fg(ACCENT).bold()),
                Span::styled(
                    truncate(description, room.saturating_sub(key_width + 2)),
                    Style::new().fg(Color::Gray),
                ),
            ])
        })
        .collect();

    // Say so when the list doesn't fit, rather than silently cutting it.
    let height = inner.height as usize;
    if lines.len() > height && height > 0 {
        lines.truncate(height - 1);
        lines.push(Line::styled(
            truncate("… a taller window shows the rest", room),
            Style::new().fg(MUTED).italic(),
        ));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn bordered(title: String, focused: bool) -> Border<'static> {
    let style = if focused {
        Style::new().fg(ACCENT)
    } else {
        Style::new().fg(MUTED)
    };
    Border::bordered()
        .border_type(BorderType::Rounded)
        .border_style(style)
        .title(Span::styled(
            title,
            Style::new().add_modifier(Modifier::BOLD),
        ))
}

/// The width a dialog asking for `wanted` columns gets: all of it when there's
/// a margin to spare, or the screen less that margin while that still leaves a
/// usable dialog, or else the whole screen.
fn dialog_width(area: Rect, wanted: u16) -> u16 {
    let with_margin = area.width.saturating_sub(4);
    if wanted <= with_margin {
        wanted
    } else if with_margin >= 40 {
        with_margin
    } else {
        area.width
    }
}

/// Where a dialog `width` (from [`dialog_width`]) by `height` goes: floating
/// in the middle when it fits with a margin, otherwise the whole of `area`. A
/// popup that meets the screen edge on some sides but not others cuts through
/// the borders underneath and only looks broken.
fn dialog(area: Rect, width: u16, height: u16) -> Rect {
    if width >= area.width || height + 2 > area.height {
        return area;
    }
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// Display width in columns. Everything we draw is single-width.
fn width(text: &str) -> usize {
    text.chars().count()
}

/// `text` cut to `room` columns, ending in "…" if anything was lost.
fn truncate(text: &str, room: usize) -> String {
    if width(text) <= room {
        return text.to_string();
    }
    if room == 0 {
        return String::new();
    }
    let mut cut: String = text.chars().take(room - 1).collect();
    cut.push('…');
    cut
}

/// Like [`truncate`], but keeps the end: `…/.ssh/config`.
fn truncate_left(text: &str, room: usize) -> String {
    let len = width(text);
    if len <= room {
        return text.to_string();
    }
    if room == 0 {
        return String::new();
    }
    let tail: String = text.chars().skip(len - (room - 1)).collect();
    format!("…{tail}")
}

/// `text` padded with spaces to `room` columns.
fn pad(text: &str, room: usize) -> String {
    format!("{text:<room$}")
}

/// Greedy word wrap to `room` columns; words longer than a line are cut.
fn wrap_words(text: &str, room: usize) -> Vec<String> {
    if room == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let word = truncate(word, room);
        if !line.is_empty() && width(&line) + 1 + width(&word) > room {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::config::SshConfig;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use std::path::PathBuf;

    const SAMPLE: &str = "\
Host *
    AddKeysToAgent yes
    IdentityFile ~/.ssh/id_ed25519

# the NAS in the cupboard
Host nas
    HostName 10.0.0.4
    User st
    LocalForward 8081 127.0.0.1:8081
    LocalForward 32400 127.0.0.1:32400

Host web-01 web-02
    HostName web.example.com
    User deploy
    ProxyJump bastion

Match host bastion
    ForwardAgent yes
";

    fn render(app: &mut App) -> String {
        render_at(app, 96, 26)
    }

    fn render_at(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn sample_app() -> App {
        App::new(SshConfig::parse(
            PathBuf::from("/Users/you/.ssh/config"),
            SAMPLE,
        ))
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn press(app: &mut App, c: char) {
        app.on_key(key(KeyCode::Char(c)));
    }

    #[test]
    fn every_screen_renders_without_panicking() {
        let mut app = sample_app();
        render(&mut app);
        for key in ['j', 'e'] {
            press(&mut app, key);
        }
        render(&mut app);
        app.on_key(KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        press(&mut app, 'd');
        render(&mut app);
    }

    #[test]
    fn renders_in_a_tiny_terminal() {
        let mut app = sample_app();
        press(&mut app, 'e');
        let mut terminal = Terminal::new(TestBackend::new(20, 6)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    }

    /// Walks the editor's cursor onto the first row whose rendered label matches.
    fn focus(app: &mut App, label: &str) {
        let editor = app.editor.as_mut().unwrap();
        let rows = editor.rows();
        editor.cursor = rows
            .iter()
            .position(|row| editor.row_label(*row) == label)
            .unwrap_or_else(|| panic!("no row labelled {label}"));
    }

    #[test]
    fn a_long_value_scrolls_so_the_cursor_stays_visible() {
        let long = "ssh -W %h:%p bastion.a-very-long-hostname.example.internal.company.com";
        let text = format!("Host jump\n    ProxyCommand {long}\n");
        let mut app = App::new(SshConfig::parse(
            PathBuf::from("/Users/you/.ssh/config"),
            &text,
        ));
        press(&mut app, 'e');
        focus(&mut app, "ProxyCommand");
        app.on_key(key(KeyCode::Enter)); // start editing; cursor lands at the end

        let screen = render(&mut app);
        assert!(
            screen.contains("company.com"),
            "tail should be visible:\n{screen}"
        );
    }

    #[test]
    fn the_editor_search_prompt_shows_where_you_are_among_the_matches() {
        let mut app = sample_app();
        press(&mut app, 'j');
        press(&mut app, 'e');
        for c in "/local".chars() {
            press(&mut app, c);
        }
        let screen = render(&mut app);
        let footer = screen.lines().last().unwrap();
        assert!(footer.contains("/local▌"), "{footer}");
        assert!(footer.contains("1 of 2"), "{footer}");
        assert!(
            screen.contains("› ● local"),
            "the other match is marked:\n{screen}"
        );

        for c in "zz".chars() {
            press(&mut app, c);
        }
        assert!(
            render(&mut app)
                .lines()
                .last()
                .unwrap()
                .contains("no match")
        );
    }

    #[test]
    fn a_host_with_many_rows_scrolls_to_follow_the_cursor() {
        let forwards: String = (0..20)
            .map(|i| format!("    LocalForward {} 127.0.0.1:{}\n", 3000 + i, 3000 + i))
            .collect();
        let text = format!("Host busy\n{forwards}");
        let mut app = App::new(SshConfig::parse(
            PathBuf::from("/Users/you/.ssh/config"),
            &text,
        ));
        press(&mut app, 'e');

        let screen = render(&mut app);
        assert!(
            screen.contains("3000 → 127.0.0.1:3000"),
            "top row visible:\n{screen}"
        );
        assert!(
            !screen.contains("3019 → 127.0.0.1:3019"),
            "bottom row is off-screen"
        );

        // Walk to the last forward; the list should have scrolled to it.
        let editor = app.editor.as_mut().unwrap();
        editor.cursor = editor
            .rows()
            .iter()
            .position(|r| *r == Row::AddOption)
            .unwrap();
        let screen = render(&mut app);
        assert!(
            screen.contains("3019 → 127.0.0.1:3019"),
            "scrolled to the end:\n{screen}"
        );
        assert!(screen.contains("+ add an option"));
    }

    #[test]
    #[ignore = "prints small-terminal screens: cargo test small_screens -- --ignored --nocapture"]
    fn small_screens() {
        for (w, h) in [(96, 26), (60, 24), (60, 16), (44, 14), (30, 8)] {
            let mut app = sample_app();
            press(&mut app, 'j');
            println!("\n=== LIST {w}x{h} ===\n{}", render_at(&mut app, w, h));
            press(&mut app, 'e');
            println!("\n=== EDITOR {w}x{h} ===\n{}", render_at(&mut app, w, h));
            focus(&mut app, "+ add a port forward");
            app.on_key(key(KeyCode::Enter));
            println!("\n=== FORWARD {w}x{h} ===\n{}", render_at(&mut app, w, h));
        }

        // Searching the editor's rows.
        for (w, h) in [(96, 26), (44, 14)] {
            let mut app = sample_app();
            press(&mut app, 'j');
            press(&mut app, 'e');
            for c in "/local".chars() {
                press(&mut app, c);
            }
            println!(
                "\n=== ROW SEARCH {w}x{h} ===\n{}",
                render_at(&mut app, w, h)
            );
        }

        // Switched-off forward, and the dialogs, at a mid and a small size.
        for (w, h) in [(60, 24), (44, 14)] {
            let mut app = sample_app();
            press(&mut app, 'j');
            press(&mut app, 'e');
            let editor = app.editor.as_mut().unwrap();
            editor.forwards[0].enabled = false;
            editor.cursor = editor
                .rows()
                .iter()
                .position(|r| *r == Row::Forward(0))
                .unwrap();
            println!(
                "\n=== FORWARD OFF {w}x{h} ===\n{}",
                render_at(&mut app, w, h)
            );
            press(&mut app, 'd');
            println!(
                "\n=== REMOVE ROW {w}x{h} ===\n{}",
                render_at(&mut app, w, h)
            );

            let mut app = sample_app();
            press(&mut app, 'j');
            press(&mut app, 'd');
            println!(
                "\n=== DELETE HOST {w}x{h} ===\n{}",
                render_at(&mut app, w, h)
            );
            let mut app = sample_app();
            press(&mut app, '?');
            println!("\n=== HELP {w}x{h} ===\n{}", render_at(&mut app, w, h));
        }
    }

    #[test]
    #[ignore = "prints the screens for eyeballing: cargo test screens -- --ignored --nocapture"]
    fn screens() {
        let mut app = sample_app();
        println!("\n=== LIST ===\n{}", render(&mut app));
        press(&mut app, 'j');
        println!("\n=== SELECTED HOST ===\n{}", render(&mut app));
        press(&mut app, 'e');
        println!("\n=== EDIT FORM ===\n{}", render(&mut app));
        app.on_key(KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        press(&mut app, '?');
        println!("\n=== HELP ===\n{}", render(&mut app));
        press(&mut app, 'q');
        press(&mut app, 'd');
        println!("\n=== CONFIRM DELETE ===\n{}", render(&mut app));

        let mut app = sample_app();
        press(&mut app, 'j');
        press(&mut app, 'e');
        focus(&mut app, "+ add a port forward");
        app.on_key(key(KeyCode::Enter));
        println!("\n=== FORWARD FORM ===\n{}", render(&mut app));

        let mut app = sample_app();
        press(&mut app, 'e');
        focus(&mut app, "+ add an option");
        app.on_key(key(KeyCode::Enter));
        for c in "agent".chars() {
            press(&mut app, c);
        }
        println!("\n=== OPTION PICKER ===\n{}", render(&mut app));
    }
}
