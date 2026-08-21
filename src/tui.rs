//! Live full-screen table view (the "top for APRS").
//!
//! A background thread reads the APRS-IS stream and feeds parsed packets to the
//! render loop, which keeps one row per station (latest wins). Rows are colored
//! by distance, your own callsigns stand out, and the selection scrolls so the
//! list can be longer than the screen.
//!
//! Keys: up/down (or j/k) move the selection, `s` toggles sort, `q`/Esc quits.

use std::collections::HashMap;
use std::io::Stdout;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};

use crate::{aprsis, packet};

struct Entry {
    kind: &'static str,
    pos: Option<(f64, f64)>,
    dist: Option<f64>,
    info: String,
    last: Instant,
}

/// Run the live table until the user quits.
pub fn run_table(
    server: String,
    login: String,
    filter: String,
    home: Option<(f64, f64)>,
    label: String,
) -> Result<()> {
    let mycall = login.split('-').next().unwrap_or("").to_uppercase();

    let (tx, rx) = mpsc::channel::<packet::Packet>();
    std::thread::spawn(move || {
        let _ = aprsis::stream(&server, &login, &filter, |line| {
            if let Some(p) = packet::parse(line) {
                let _ = tx.send(p);
            }
        });
    });

    // ratatui::init() enables raw mode, enters the alternate screen, and sets a
    // panic hook that restores the terminal.
    let mut terminal = ratatui::init();
    let result = table_loop(&mut terminal, &rx, home, &label, &mycall);
    ratatui::restore();
    result
}

fn table_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    rx: &mpsc::Receiver<packet::Packet>,
    home: Option<(f64, f64)>,
    label: &str,
    mycall: &str,
) -> Result<()> {
    let mut entries: HashMap<String, Entry> = HashMap::new();
    let mut sort_dist = false; // default to most-recent
    let mut paused = false;
    let mut state = TableState::default();
    let mut selected: usize = 0;
    let mut prev_len: usize = usize::MAX;

    loop {
        if !paused {
            while let Ok(p) = rx.try_recv() {
                let dist = match (home, p.position) {
                    (Some(h), Some(pp)) => Some(packet::distance_mi(h, pp)),
                    _ => None,
                };
                entries.insert(
                    p.source.clone(),
                    Entry {
                        kind: p.kind,
                        pos: p.position,
                        dist,
                        info: p.info,
                        last: Instant::now(),
                    },
                );
            }
        }

        let mut list: Vec<(&String, &Entry)> = entries.iter().collect();
        if sort_dist {
            list.sort_by(|a, b| match (a.1.dist, b.1.dist) {
                (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => b.1.last.cmp(&a.1.last),
            });
        } else {
            list.sort_by(|a, b| b.1.last.cmp(&a.1.last));
        }

        if list.is_empty() {
            selected = 0;
            state.select(None);
        } else {
            selected = selected.min(list.len() - 1);
            state.select(Some(selected));
        }

        // A full clear whenever the row count changes prevents a previous,
        // shorter frame from ghosting behind the new one.
        if list.len() != prev_len {
            terminal.clear()?;
            prev_len = list.len();
        }

        terminal.draw(|frame| render(frame, &list, &mut state, label, mycall, sort_dist, paused))?;

        if event::poll(Duration::from_millis(300))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char('s') => sort_dist = !sort_dist,
                        KeyCode::Char('p') => paused = !paused,
                        KeyCode::Down | KeyCode::Char('j') => {
                            if !list.is_empty() {
                                selected = (selected + 1).min(list.len() - 1);
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            selected = selected.saturating_sub(1);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(())
}

fn render(
    frame: &mut ratatui::Frame,
    list: &[(&String, &Entry)],
    state: &mut TableState,
    label: &str,
    mycall: &str,
    sort_dist: bool,
    paused: bool,
) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(0),    // table
        Constraint::Length(1), // footer
    ])
    .split(frame.area());

    // Header bar with a distance-color legend.
    let label_short: String = label.chars().take(40).collect();
    let head = Line::from(vec![
        Span::styled(
            " claprs ",
            Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  dist: "),
        Span::styled("<25mi", Style::default().fg(Color::Green)),
        Span::raw(" "),
        Span::styled("<100", Style::default().fg(Color::Yellow)),
        Span::raw(" "),
        Span::styled("<300", Style::default().fg(Color::White)),
        Span::raw(" "),
        Span::styled("far", Style::default().fg(Color::DarkGray)),
        Span::raw("  "),
        Span::styled("you", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
        Span::raw("   "),
        Span::styled(label_short, Style::default().fg(Color::Gray)),
    ]);
    frame.render_widget(Paragraph::new(head), chunks[0]);

    // Table.
    let header = Row::new(["STATION", "TYPE", "POSITION", "DIST", "AGE", "INFO"])
        .style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan));

    let rows = list.iter().map(|(call, e)| {
        let mine = !mycall.is_empty() && call.to_uppercase().starts_with(mycall);
        let base = if mine {
            Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(dist_color(e.dist))
        };
        let pos = e.pos.map(|(la, lo)| format!("{la:.4},{lo:.4}")).unwrap_or_default();
        let dist = e.dist.map(|d| format!("{d:.0}mi")).unwrap_or_default();
        let age = fmt_age(e.last.elapsed().as_secs());
        Row::new(vec![
            Cell::from((*call).clone()),
            Cell::from(e.kind.to_string()),
            Cell::from(pos),
            Cell::from(dist),
            Cell::from(age),
            Cell::from(e.info.clone()),
        ])
        .style(base)
    });

    let widths = [
        Constraint::Length(9),
        Constraint::Length(6),
        Constraint::Length(17),
        Constraint::Length(6),
        Constraint::Length(5),
        Constraint::Min(10),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    frame.render_stateful_widget(table, chunks[1], state);

    // Footer bar.
    let sort_label = if sort_dist { "distance" } else { "recent" };
    let sel = state.selected().map(|i| i + 1).unwrap_or(0);
    let key = |c| Style::default().fg(c);
    let mut spans = vec![
        Span::styled(
            format!(" {sel}/{} stations ", list.len()),
            Style::default().fg(Color::Black).bg(Color::Gray),
        ),
        Span::raw(format!("  sort: {sort_label}   ")),
    ];
    if paused {
        spans.push(Span::styled(
            " PAUSED ",
            Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw("   "));
    }
    spans.extend([
        Span::styled("up/dn", key(Color::Cyan)),
        Span::raw(" move  "),
        Span::styled("s", key(Color::Cyan)),
        Span::raw(" sort  "),
        Span::styled("p", key(Color::Cyan)),
        Span::raw(" pause  "),
        Span::styled("q", key(Color::Cyan)),
        Span::raw(" quit"),
    ]);
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[2]);
}

fn dist_color(dist: Option<f64>) -> Color {
    match dist {
        Some(d) if d < 25.0 => Color::Green,
        Some(d) if d < 100.0 => Color::Yellow,
        Some(d) if d < 300.0 => Color::White,
        _ => Color::DarkGray, // very far, or no position at all
    }
}

fn fmt_age(s: u64) -> String {
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else {
        format!("{}h", s / 3600)
    }
}
