//! All drawing. Colours stick to the 16 ANSI slots plus a couple of indexed
//! greys so the TUI inherits whatever theme the user's terminal already has.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block as Border, BorderType, Clear, List, ListItem, Padding, Paragraph, Wrap,
};

use crate::app::{App, Mode, StatusKind};
use crate::config::{BlockKind, split_directive};
use crate::editor::{Editor, ForwardForm, Overlay, Row};
use crate::forward::{Forward, ForwardKind};
use crate::schema::Kind;

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, app, header);

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)]).areas(body);
    draw_list(frame, app, left);
    draw_details(frame, app, right);

    draw_footer(frame, app, footer);

    match app.mode {
        Mode::Edit => draw_editor(frame, app, frame.area()),
        Mode::ConfirmDelete => draw_confirm(frame, app, frame.area()),
        Mode::Help => draw_help(frame, frame.area()),
        _ => {}
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let hosts = app
        .config
        .blocks
        .iter()
        .filter(|b| b.kind == BlockKind::Host)
        .count();
    let line = Line::from(vec![
        Span::styled(
            " sshconfig ",
            Style::new().fg(Color::Black).bg(ACCENT).bold(),
        ),
        Span::raw(" "),
        Span::styled(
            app.config.path.display().to_string(),
            Style::new().fg(MUTED),
        ),
        Span::styled(format!("  {hosts} hosts"), Style::new().fg(MUTED)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let visible = app.visible();
    let items: Vec<ListItem> = visible
        .iter()
        .map(|i| {
            let block = &app.config.blocks[*i];
            let (marker, marker_style) = match block.kind {
                BlockKind::Match => ("≡", Style::new().fg(Color::Magenta)),
                BlockKind::Host if block.is_pattern() => ("*", Style::new().fg(Color::Yellow)),
                BlockKind::Host => ("●", Style::new().fg(Color::Green)),
            };
            let target = match (block.get("User"), block.get("HostName")) {
                (Some(user), Some(host)) => format!("{user}@{host}"),
                (None, Some(host)) => host,
                _ => String::new(),
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {marker} "), marker_style),
                Span::styled(block.label(), Style::new().bold()),
                Span::raw("  "),
                Span::styled(target, Style::new().fg(MUTED)),
            ]))
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
    let Some(block) = app.selected() else {
        let hint = Text::from(vec![
            Line::raw(""),
            Line::styled("  No hosts yet.", Style::new().fg(MUTED)),
            Line::styled("  Press a to add one.", Style::new().fg(MUTED)),
        ]);
        frame.render_widget(
            Paragraph::new(hint).block(bordered(" Details ".into(), false)),
            area,
        );
        return;
    };

    let mut lines = vec![Line::from(vec![
        Span::styled("Host  ", Style::new().fg(MUTED)),
        Span::styled(block.patterns().join(" "), Style::new().fg(ACCENT).bold()),
    ])];
    for comment in &block.leading {
        lines.push(Line::styled(
            comment.trim().to_string(),
            Style::new().fg(MUTED).italic(),
        ));
    }
    lines.push(Line::raw(""));

    if block.body.is_empty() {
        lines.push(Line::styled("  (no options)", Style::new().fg(MUTED)));
    }
    for raw in &block.body {
        lines.push(match split_directive(raw) {
            // Forwards read better as what they do than as their arguments.
            Some((key, value)) if Forward::parse(key, value).is_some() => {
                let forward = Forward::parse(key, value).unwrap();
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{:<18}", forward.kind.label()),
                        Style::new().fg(Color::Magenta),
                    ),
                    Span::raw(forward.describe()),
                ])
            }
            Some((key, value)) => Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{key:<18}"), Style::new().fg(Color::Blue)),
                Span::raw(value.to_string()),
            ]),
            // Comments and blank lines, shown as-is.
            None => Line::styled(format!("  {}", raw.trim()), Style::new().fg(MUTED).italic()),
        });
    }

    if block.kind == BlockKind::Host && !block.is_pattern() {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled("  l ", Style::new().fg(Color::Green)),
            Span::styled(format!("ssh {}", block.label()), Style::new().fg(MUTED)),
        ]));
    }

    let paragraph = Paragraph::new(lines)
        .block(bordered(" Details ".into(), false).padding(Padding::horizontal(1)))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    if app.mode == Mode::Search {
        let line = Line::from(vec![
            Span::styled(" /", Style::new().fg(ACCENT).bold()),
            Span::raw(app.filter.clone()),
            Span::styled("▌", Style::new().fg(ACCENT)),
            Span::styled("   ↵ accept · esc clear", Style::new().fg(MUTED)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    if let Some((message, kind)) = &app.status {
        let style = match kind {
            StatusKind::Info => Style::new().fg(Color::Green),
            StatusKind::Error => Style::new().fg(Color::Red).bold(),
        };
        frame.render_widget(
            Paragraph::new(Line::styled(format!(" {message}"), style)),
            area,
        );
        return;
    }

    let keys = [
        ("l", "login"),
        ("↵", "edit"),
        ("a", "add"),
        ("d", "delete"),
        ("y", "dup"),
        ("/", "search"),
        ("r", "reload"),
        ("?", "help"),
        ("q", "quit"),
    ];
    let mut spans = vec![Span::raw(" ")];
    for (key, label) in keys {
        spans.push(Span::styled(key, Style::new().fg(ACCENT).bold()));
        spans.push(Span::styled(format!(" {label}  "), Style::new().fg(MUTED)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The host editor: a scrolling list of rows, with overlays on top.
fn draw_editor(frame: &mut Frame, app: &App, area: Rect) {
    let Some(editor) = app.editor.as_ref() else {
        return;
    };

    let popup = centered(78, area.height.saturating_sub(2), area);
    frame.render_widget(Clear, popup);

    let title = if editor.is_new() {
        " New host "
    } else {
        " Edit host "
    };
    let outer = bordered(title.into(), true).padding(Padding::horizontal(1));
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);

    let [list_area, help_area, keys_area] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .areas(inner);

    draw_editor_rows(frame, editor, list_area);

    // A line of prose about whatever the cursor is on, so the option names are
    // not something you have to already know.
    let help = match editor.help_for_cursor() {
        Some(keyword) => {
            let mut spans = vec![Span::styled(keyword.desc, Style::new().fg(Color::Gray))];
            if let Some(default) = keyword.default {
                spans.push(Span::styled(
                    format!(" (default: {default})"),
                    Style::new().fg(MUTED).italic(),
                ));
            }
            Line::from(spans)
        }
        None => Line::styled(
            match editor.current_row() {
                Row::Alias => {
                    "The name you type after ssh. Space-separate to give a host several aliases."
                }
                Row::Forward(_) | Row::AddForward => "Tunnel a port through this connection.",
                Row::AddOption => "Search all 99 ssh_config options by name or description.",
                _ => "",
            },
            Style::new().fg(Color::Gray),
        ),
    };
    frame.render_widget(Paragraph::new(help).wrap(Wrap { trim: true }), help_area);

    let keys: &[(&str, &str)] = if editor.inline.is_some() {
        &[("↵", "accept"), ("esc", "discard"), ("^s", "save host")]
    } else {
        &[
            ("↵", "edit"),
            ("a", "add forward"),
            ("space", "toggle"),
            ("d", "remove"),
            ("^s", "save"),
            ("esc", "cancel"),
        ]
    };
    frame.render_widget(Paragraph::new(key_hints(keys)), keys_area);

    match &editor.overlay {
        Some(Overlay::Picker(_)) => draw_picker(frame, editor, area),
        Some(Overlay::Choice(_)) => draw_choice(frame, editor, area),
        Some(Overlay::Forward(_)) => draw_forward_form(frame, editor, area),
        None => {}
    }
}

/// Builds the rows, with section headings, and scrolls to keep the cursor in view.
fn draw_editor_rows(frame: &mut Frame, editor: &Editor, area: Rect) {
    let rows = editor.rows();
    let label_width = 18usize;
    // Values are typed in place, so a long one (a ProxyCommand, say) has to
    // scroll sideways to keep the cursor on screen.
    let value_width = (area.width as usize).saturating_sub(label_width + 3).max(8);
    let inline_scroll = editor.inline.as_ref().map_or(0, |input| {
        input.col.saturating_sub(value_width.saturating_sub(1))
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
        let marker = if focused { "▸ " } else { "  " };
        let name_style = if focused {
            Style::new().fg(ACCENT).bold()
        } else {
            Style::new()
        };

        let line = match row {
            Row::AddForward | Row::AddOption => Line::from(vec![
                Span::styled(marker, Style::new().fg(ACCENT)),
                Span::styled(
                    editor.row_label(*row),
                    if focused {
                        Style::new().fg(ACCENT).bold()
                    } else {
                        Style::new().fg(MUTED)
                    },
                ),
            ]),
            Row::Forward(i) => {
                let forward = &editor.forwards[*i];
                Line::from(vec![
                    Span::styled(marker, Style::new().fg(ACCENT)),
                    Span::styled(
                        format!("{:<8}", forward.kind.label()),
                        Style::new().fg(Color::Magenta),
                    ),
                    Span::styled(forward.describe(), name_style),
                ])
            }
            _ => {
                let label = editor.row_label(*row);
                let value = editor.row_value(*row);
                let is_comment = label == "#";
                let inline = editor.inline.as_ref().filter(|_| focused);
                let (value_span, value_style) = if let Some(input) = inline {
                    let shown: String = input
                        .value()
                        .chars()
                        .skip(inline_scroll)
                        .take(value_width)
                        .collect();
                    (shown, Style::new().fg(Color::Black).bg(ACCENT))
                } else if value.trim().is_empty() {
                    ("—".to_string(), Style::new().fg(MUTED))
                } else if is_comment {
                    (value.clone(), Style::new().fg(MUTED).italic())
                } else {
                    (value.clone(), Style::new())
                };
                Line::from(vec![
                    Span::styled(marker, Style::new().fg(ACCENT)),
                    Span::styled(
                        format!("{label:<label_width$}"),
                        if is_comment {
                            Style::new().fg(MUTED)
                        } else {
                            name_style.patch(Style::new().fg(if focused {
                                ACCENT
                            } else {
                                Color::Blue
                            }))
                        },
                    ),
                    Span::styled(value_span, value_style),
                ])
            }
        };
        lines.push((Some(index), line));
    }

    // Scroll so the focused row stays on screen.
    let focus_line = lines
        .iter()
        .position(|(i, _)| *i == Some(editor.cursor))
        .unwrap_or(0);
    let height = area.height as usize;
    let offset = focus_line
        .saturating_sub(height.saturating_sub(2))
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
        let y = area.y + (focus_line.saturating_sub(offset)) as u16;
        let x = area.x + 2 + label_width as u16 + (input.col - inline_scroll) as u16;
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
            format!("  {title}"),
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
    ));
}

/// The searchable catalogue of every ssh_config keyword.
fn draw_picker(frame: &mut Frame, editor: &Editor, area: Rect) {
    let Some(Overlay::Picker(picker)) = editor.overlay.as_ref() else {
        return;
    };

    let popup = centered(74, area.height.saturating_sub(4), area);
    frame.render_widget(Clear, popup);
    let outer = bordered(" Add an option ".into(), true).padding(Padding::horizontal(1));
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);

    let [search_area, list_area, desc_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(3),
    ])
    .areas(inner);

    let query = picker.query.value();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("search ", Style::new().fg(MUTED)),
            Span::styled(query.clone(), Style::new().fg(ACCENT).bold()),
            Span::styled("▌", Style::new().fg(ACCENT)),
        ])),
        search_area,
    );

    let matches = picker.matches();
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
            Line::from(vec![
                Span::styled(if focused { "▸ " } else { "  " }, Style::new().fg(ACCENT)),
                Span::styled(format!("{:<28}", keyword.name), style),
                Span::styled(
                    format!(" {}", kind_label(keyword.kind)),
                    Style::new().fg(if focused { ACCENT } else { MUTED }),
                ),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), list_area);

    let description = match picker.selected() {
        Some(keyword) => {
            let mut spans = vec![Span::styled(keyword.desc, Style::new().fg(Color::Gray))];
            if let Some(default) = keyword.default {
                spans.push(Span::styled(
                    format!(" (default: {default})"),
                    Style::new().fg(MUTED),
                ));
            }
            Line::from(spans)
        }
        None => Line::styled("  no match", Style::new().fg(MUTED)),
    };
    frame.render_widget(
        Paragraph::new(vec![
            description,
            key_hints(&[("↵", "add"), ("↑↓", "move"), ("esc", "back")]),
        ])
        .wrap(Wrap { trim: true }),
        desc_area,
    );
}

/// The yes/no and one-of-N value list.
fn draw_choice(frame: &mut Frame, editor: &Editor, area: Rect) {
    let Some(Overlay::Choice(choice)) = editor.overlay.as_ref() else {
        return;
    };

    let height = choice.values.len() as u16 + 5;
    let popup = centered(56, height, area);
    frame.render_widget(Clear, popup);
    let outer = bordered(format!(" {} ", choice.keyword), true).padding(Padding::horizontal(1));
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);

    let mut lines = vec![
        Line::styled(choice.desc, Style::new().fg(Color::Gray)),
        Line::raw(""),
    ];
    for (index, value) in choice.values.iter().enumerate() {
        let focused = index == choice.index;
        let is_default = choice.default == Some(*value);
        lines.push(Line::from(vec![
            Span::styled(if focused { " ▸ " } else { "   " }, Style::new().fg(ACCENT)),
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
    lines.push(key_hints(&[("↵", "choose"), ("esc", "back")]));
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The port-forward form.
fn draw_forward_form(frame: &mut Frame, editor: &Editor, area: Rect) {
    let Some(Overlay::Forward(form)) = editor.overlay.as_ref() else {
        return;
    };

    let popup = centered(64, 16, area);
    frame.render_widget(Clear, popup);
    let title = if form.index.is_some() {
        " Edit port forward "
    } else {
        " New port forward "
    };
    let outer = bordered(title.into(), true).padding(Padding::horizontal(1));
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);

    let mut lines = Vec::new();

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
    lines.push(Line::from(direction));
    lines.push(Line::styled(
        format!("    {}", form.forward.kind.summary()),
        Style::new().fg(Color::Gray),
    ));
    lines.push(Line::raw(""));

    let visible = form.visible_fields();
    for field in 0..visible {
        if field == 0 {
            lines.push(Line::styled(
                format!("  {}", form.forward.kind.listen_label()),
                Style::new().fg(Color::Yellow),
            ));
        }
        if field == 2 {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                format!("  {}", form.forward.kind.dest_label()),
                Style::new().fg(Color::Yellow),
            ));
        }
        let focused = form.field == field + 1;
        let value = form.inputs[field].value();
        lines.push(Line::from(vec![
            Span::styled(
                if focused { "  ▸ " } else { "    " },
                Style::new().fg(ACCENT),
            ),
            Span::styled(
                format!("{:<26}", ForwardForm::FIELDS[field]),
                Style::new().fg(if focused { ACCENT } else { Color::Blue }),
            ),
            Span::styled(
                if value.is_empty() {
                    "—".to_string()
                } else {
                    value
                },
                if focused {
                    Style::new().fg(Color::Black).bg(ACCENT)
                } else {
                    Style::new()
                },
            ),
        ]));
    }

    // A live preview of the line this will write, so there is no mystery about
    // what lands in the file.
    let complete = form.forward.is_complete();
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("  writes  ", Style::new().fg(MUTED)),
        Span::styled(
            form.forward.line(),
            if complete {
                Style::new().fg(Color::Green)
            } else {
                Style::new().fg(MUTED).italic()
            },
        ),
    ]));
    if let Some(error) = &form.error {
        lines.push(Line::styled(
            format!("  {error}"),
            Style::new().fg(Color::Red).bold(),
        ));
    }
    lines.push(key_hints(&[
        ("←→", "direction"),
        ("↑↓", "field"),
        ("↵", "done"),
        ("esc", "back"),
    ]));

    frame.render_widget(Paragraph::new(lines), inner);

    if form.field > 0 {
        let field = form.field - 1;
        let row = lines_before_field(field);
        let x = inner.x + 4 + 26 + form.inputs[field].col as u16;
        let y = inner.y + row;
        if x < inner.x + inner.width && y < inner.y + inner.height {
            frame.set_cursor_position((x, y));
        }
    }
}

/// Where a forward-form field lands vertically, given the headings above it.
fn lines_before_field(field: usize) -> u16 {
    // direction + summary + blank + "listen on" heading
    let mut row = 4u16;
    row += field as u16;
    if field >= 2 {
        row += 2; // blank + destination heading
    }
    row
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

fn key_hints(keys: &[(&str, &str)]) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (key, label) in keys {
        spans.push(Span::styled(
            (*key).to_string(),
            Style::new().fg(ACCENT).bold(),
        ));
        spans.push(Span::styled(format!(" {label}  "), Style::new().fg(MUTED)));
    }
    Line::from(spans)
}

fn draw_confirm(frame: &mut Frame, app: &App, area: Rect) {
    let Some(block) = app.selected() else { return };
    let popup = centered(54, 7, area);
    frame.render_widget(Clear, popup);

    let text = Text::from(vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  Delete "),
            Span::styled(block.label(), Style::new().fg(ACCENT).bold()),
            Span::raw("?"),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("  y", Style::new().fg(Color::Red).bold()),
            Span::styled(" delete   ", Style::new().fg(MUTED)),
            Span::styled("any other key", Style::new().bold()),
            Span::styled(" cancel", Style::new().fg(MUTED)),
        ]),
    ]);
    let block = Border::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::Red))
        .title(" Confirm ");
    frame.render_widget(Paragraph::new(text).block(block), popup);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let rows = [
        ("j / k / ↑ / ↓", "move between hosts"),
        ("g / G", "jump to first / last"),
        ("l", "log in (ssh to the selected host)"),
        ("/", "filter by alias, hostname or user"),
        ("a", "add a new host"),
        ("enter / e", "edit the selected host"),
        ("y", "duplicate the selected host"),
        ("d", "delete (asks first)"),
        ("r", "reload the file from disk"),
        ("q / esc", "quit"),
        ("", ""),
        ("in the host editor:", ""),
        ("↑ / ↓", "move between rows"),
        ("enter", "edit the row, or open its dialog"),
        ("a", "add a port forward"),
        ("space", "flip a yes/no option"),
        ("d", "remove the row"),
        ("ctrl-s", "save to ~/.ssh/config"),
        ("esc", "cancel, or close a dialog"),
        ("", ""),
        ("add a port forward", "for a local, remote or SOCKS tunnel"),
        ("add an option", "search all 99 ssh_config options"),
    ];

    let popup = centered(62, rows.len() as u16 + 4, area);
    frame.render_widget(Clear, popup);

    let mut lines = vec![Line::raw("")];
    lines.extend(rows.iter().map(|(key, description)| {
        Line::from(vec![
            Span::styled(format!("  {key:>16}  "), Style::new().fg(ACCENT).bold()),
            Span::styled(*description, Style::new().fg(Color::Gray)),
        ])
    }));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "  Changes are written immediately; the previous file is kept as config.bak.",
        Style::new().fg(MUTED),
    ));

    frame.render_widget(
        Paragraph::new(lines)
            .block(bordered(" Keys ".into(), true))
            .wrap(Wrap { trim: false }),
        popup,
    );
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

/// A popup of at most `width` x `height`, centred, clamped to the screen.
fn centered(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width.saturating_sub(2)).max(1);
    let height = height.min(area.height.saturating_sub(2)).max(1);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
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
        let mut terminal = Terminal::new(TestBackend::new(96, 26)).unwrap();
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
