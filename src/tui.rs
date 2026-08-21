//! Live full-screen table view (the "top for APRS").
//!
//! A background thread reads the APRS-IS stream and feeds parsed packets to the
//! render loop, which keeps one row per station (latest wins). Rows are colored
//! by distance, your own callsigns stand out, and the list can be sorted,
//! paused, searched, and drilled into.
//!
//! Keys: up/down (or j/k) move, `s` sort, `p` pause, `/` search, Enter details,
//! `q`/Esc quits (or backs out).

use std::collections::{HashMap, VecDeque};
use std::io::Stdout;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap};

use crate::{aprsis, packet};

struct Entry {
    kind: &'static str,
    icon: &'static str,
    pos: Option<(f64, f64)>,
    dist: Option<f64>,
    info: String,
    last: Instant,
    first: Instant,
    trail: Vec<(f64, f64)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    Search,
    Detail,
}

/// Type filters cycled with `t`. `None` means show everything.
const TYPE_FILTERS: [(&str, Option<&str>); 5] = [
    ("all", None),
    ("mobile", Some("mic-e")),
    ("weather", Some("wx")),
    ("fixed", Some("pos")),
    ("object", Some("obj")),
];

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
    let mut sort_dist = false;
    let mut paused = false;
    let mut mode = Mode::Normal;
    let mut query = String::new();
    let mut type_idx = 0usize;
    let mut pkt_times: VecDeque<Instant> = VecDeque::new();
    let start = Instant::now();
    let mut state = TableState::default();
    let mut selected: usize = 0;
    let mut detail_call: Option<String> = None;
    let mut prev: (usize, u8) = (usize::MAX, 9);

    loop {
        if !paused {
            while let Ok(p) = rx.try_recv() {
                pkt_times.push_back(Instant::now());
                let dist = match (home, p.position) {
                    (Some(h), Some(pp)) => Some(packet::distance_mi(h, pp)),
                    _ => None,
                };
                let icon = p.icon();
                let e = entries.entry(p.source.clone()).or_insert_with(|| Entry {
                    kind: p.kind,
                    icon,
                    pos: None,
                    dist: None,
                    info: String::new(),
                    last: Instant::now(),
                    first: Instant::now(),
                    trail: Vec::new(),
                });
                // Append to the trail only when the station actually moved.
                if let Some(pos) = p.position {
                    if e.trail.last().map_or(true, |&l| l != pos) {
                        e.trail.push(pos);
                        let n = e.trail.len();
                        if n > 12 {
                            e.trail.drain(0..n - 12);
                        }
                    }
                }
                e.kind = p.kind;
                e.icon = icon;
                e.pos = p.position;
                e.dist = dist;
                e.info = p.info;
                e.last = Instant::now();
            }
        }

        let q = query.to_uppercase();
        let type_kind = TYPE_FILTERS[type_idx].1;
        let mut list: Vec<(&String, &Entry)> = entries
            .iter()
            .filter(|(call, _)| q.is_empty() || call.to_uppercase().contains(&q))
            .filter(|(_, e)| type_kind.map_or(true, |k| e.kind == k))
            .collect();
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

        // Repaint fully when the row count or mode changes (prevents ghosting,
        // and clears the detail popup when it closes).
        let mode_id = mode as u8;
        if (list.len(), mode_id) != prev {
            terminal.clear()?;
            prev = (list.len(), mode_id);
        }

        // Pin the detail popup to a callsign, not a row index, so a re-sort
        // caused by a fresh packet cannot swap what we are looking at.
        let detail = if mode == Mode::Detail {
            detail_call.as_ref().and_then(|c| entries.get_key_value(c))
        } else {
            None
        };
        while pkt_times.front().is_some_and(|t| t.elapsed() > Duration::from_secs(60)) {
            pkt_times.pop_front();
        }
        // Normalize to a real per-minute rate, even before the 60s window fills.
        let span = start.elapsed().as_secs_f64().clamp(1.0, 60.0);
        let ppm = (pkt_times.len() as f64 * 60.0 / span).round() as usize;
        let uniq = entries.len();
        let type_label = TYPE_FILTERS[type_idx].0;
        terminal.draw(|frame| {
            render(frame, &list, &mut state, home, label, mycall, sort_dist, paused, mode, &query, type_label, ppm, uniq, detail)
        })?;

        if !event::poll(Duration::from_millis(300))? {
            continue;
        }
        let Event::Key(k) = event::read()? else { continue };
        if k.kind != KeyEventKind::Press {
            continue;
        }

        match mode {
            Mode::Normal => match k.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('s') => sort_dist = !sort_dist,
                KeyCode::Char('p') => paused = !paused,
                KeyCode::Char('/') => mode = Mode::Search,
                KeyCode::Enter => {
                    if let Some((c, _)) = list.get(selected) {
                        detail_call = Some((*c).clone());
                        mode = Mode::Detail;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if !list.is_empty() {
                        selected = (selected + 1).min(list.len() - 1);
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
                KeyCode::Char('o') => {
                    if let Some((c, _)) = list.get(selected) {
                        open_url(&format!("https://aprs.fi/#!call=a%2F{c}"));
                    }
                }
                KeyCode::Char('t') => type_idx = (type_idx + 1) % TYPE_FILTERS.len(),
                _ => {}
            },
            Mode::Search => match k.code {
                KeyCode::Esc => {
                    query.clear();
                    mode = Mode::Normal;
                }
                KeyCode::Enter => mode = Mode::Normal,
                KeyCode::Backspace => {
                    query.pop();
                }
                KeyCode::Char(c) if !c.is_control() => query.push(c),
                _ => {}
            },
            Mode::Detail => match k.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                    mode = Mode::Normal;
                    detail_call = None;
                }
                KeyCode::Char('o') => {
                    if let Some(c) = &detail_call {
                        open_url(&format!("https://aprs.fi/#!call=a%2F{c}"));
                    }
                }
                _ => {}
            },
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render(
    frame: &mut ratatui::Frame,
    list: &[(&String, &Entry)],
    state: &mut TableState,
    home: Option<(f64, f64)>,
    label: &str,
    mycall: &str,
    sort_dist: bool,
    paused: bool,
    mode: Mode,
    query: &str,
    type_label: &str,
    ppm: usize,
    uniq: usize,
    detail: Option<(&String, &Entry)>,
) {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(frame.area());

    // Per-band counts.
    let (mut near, mut mid, mut wide, mut far) = (0u32, 0u32, 0u32, 0u32);
    for (_, e) in list {
        match e.dist {
            Some(d) if d < 25.0 => near += 1,
            Some(d) if d < 100.0 => mid += 1,
            Some(d) if d < 300.0 => wide += 1,
            _ => far += 1,
        }
    }

    // Header: legend with live counts. The label is clipped to the terminal
    // width automatically by the Paragraph, so it is never cut mid-number here.
    let head = Line::from(vec![
        Span::styled(
            " claprs ",
            Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(format!("<25mi {near}"), Style::default().fg(Color::Green)),
        Span::raw("  "),
        Span::styled(format!("<100 {mid}"), Style::default().fg(Color::Yellow)),
        Span::raw("  "),
        Span::styled(format!("<300 {wide}"), Style::default().fg(Color::White)),
        Span::raw("  "),
        Span::styled(format!("far {far}"), Style::default().fg(Color::DarkGray)),
        Span::raw("   "),
        Span::styled(format!("{ppm}/min {uniq}stn"), Style::default().fg(Color::Cyan)),
        Span::raw("   "),
        Span::styled(label, Style::default().fg(Color::Gray)),
    ]);
    frame.render_widget(Paragraph::new(head), chunks[0]);

    // Table.
    let header = Row::new(["", "STATION", "TYPE", "POSITION", "DIST", "AGE", "INFO"])
        .style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan));
    let rows = list.iter().map(|(call, e)| {
        let mine = !mycall.is_empty() && call.to_uppercase().starts_with(mycall);
        let base = if e.first.elapsed() < Duration::from_secs(5) {
            // new-station flash
            Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
        } else if mine {
            Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(dist_color(e.dist))
        };
        let pos = e.pos.map(|(la, lo)| format!("{la:.4},{lo:.4}")).unwrap_or_default();
        let dist = e.dist.map(|d| format!("{d:.0}mi")).unwrap_or_default();
        Row::new(vec![
            Cell::from(e.icon),
            Cell::from((*call).clone()),
            Cell::from(e.kind.to_string()),
            Cell::from(pos),
            Cell::from(dist),
            Cell::from(fmt_age(e.last.elapsed().as_secs())),
            Cell::from(e.info.clone()),
        ])
        .style(base)
    });
    let widths = [
        Constraint::Length(2),
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

    // Footer changes with mode.
    let foot = match mode {
        Mode::Search => Line::from(vec![
            Span::styled(" search ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::raw(format!("  {query}_")),
            Span::styled("   enter", Style::default().fg(Color::Cyan)),
            Span::raw(" apply  "),
            Span::styled("esc", Style::default().fg(Color::Cyan)),
            Span::raw(" clear"),
        ]),
        Mode::Detail => Line::from(vec![
            Span::styled(" detail ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::raw("   "),
            Span::styled("o", Style::default().fg(Color::Cyan)),
            Span::raw(" aprs.fi   "),
            Span::styled("esc/enter", Style::default().fg(Color::Cyan)),
            Span::raw(" close"),
        ]),
        Mode::Normal => {
            let sel = state.selected().map(|i| i + 1).unwrap_or(0);
            let mut spans = vec![
                Span::styled(
                    format!(" {sel}/{} ", list.len()),
                    Style::default().fg(Color::Black).bg(Color::Gray),
                ),
                Span::raw(format!("  sort:{}  ", if sort_dist { "dist" } else { "recent" })),
            ];
            if !query.is_empty() {
                spans.push(Span::styled(format!("filter:{query}  "), Style::default().fg(Color::Yellow)));
            }
            if type_label != "all" {
                spans.push(Span::styled(format!("type:{type_label}  "), Style::default().fg(Color::Yellow)));
            }
            if paused {
                spans.push(Span::styled(
                    " PAUSED ",
                    Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::raw("  "));
            }
            for (kk, lbl) in [("up/dn", "move"), ("s", "sort"), ("t", "type"), ("p", "pause"), ("/", "search"), ("enter", "detail"), ("o", "aprs.fi"), ("q", "quit")] {
                spans.push(Span::styled(kk, Style::default().fg(Color::Cyan)));
                spans.push(Span::raw(format!(" {lbl}  ")));
            }
            Line::from(spans)
        }
    };
    frame.render_widget(Paragraph::new(foot), chunks[2]);

    // Detail popup, pinned to a callsign (see table_loop) so it stays put.
    if let Some((call, e)) = detail {
        draw_detail(frame, call, e, home);
    }
}

fn draw_detail(frame: &mut ratatui::Frame, call: &str, e: &Entry, home: Option<(f64, f64)>) {
    let area = centered_rect(64, 62, frame.area());

    let pos = e
        .pos
        .map(|(la, lo)| format!("{la:.5}, {lo:.5}"))
        .unwrap_or_else(|| "none".to_string());
    let dist = match (home, e.pos) {
        (Some(h), Some(p)) => {
            let d = packet::distance_mi(h, p);
            let b = bearing(h, p);
            format!("{d:.1} mi  bearing {b:.0} ({})", compass(b))
        }
        _ => "unknown".to_string(),
    };

    let field = Style::default().fg(Color::Cyan);
    let mut lines = vec![
        Line::from(vec![Span::styled("Type      ", field), Span::raw(e.kind)]),
        Line::from(vec![Span::styled("Position  ", field), Span::raw(pos)]),
        Line::from(vec![Span::styled("Distance  ", field), Span::raw(dist)]),
        Line::from(vec![
            Span::styled("Heard     ", field),
            Span::raw(format!("{} ago", fmt_age(e.last.elapsed().as_secs()))),
        ]),
        Line::raw(""),
        Line::from(Span::styled("Comment", field)),
        Line::raw(if e.info.is_empty() { "(none)".to_string() } else { e.info.clone() }),
    ];

    // Trail: positions seen since claprs started watching this station.
    if e.trail.len() > 1 {
        let moved: f64 = e.trail.windows(2).map(|w| packet::distance_mi(w[0], w[1])).sum();
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            format!("Trail  {} points, {moved:.1} mi moved (newest first)", e.trail.len()),
            field,
        )));
        for &(la, lo) in e.trail.iter().rev().take(6) {
            lines.push(Line::raw(format!("  {la:.4}, {lo:.4}")));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {call} "))
        .title_style(Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD));

    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(Text::from(lines)).block(block).wrap(Wrap { trim: true }), area);
}

fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let v = Layout::vertical([
        Constraint::Percentage((100 - pct_y) / 2),
        Constraint::Percentage(pct_y),
        Constraint::Percentage((100 - pct_y) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - pct_x) / 2),
        Constraint::Percentage(pct_x),
        Constraint::Percentage((100 - pct_x) / 2),
    ])
    .split(v[1])[1]
}

fn bearing(from: (f64, f64), to: (f64, f64)) -> f64 {
    let (lat1, lon1) = (from.0.to_radians(), from.1.to_radians());
    let (lat2, lon2) = (to.0.to_radians(), to.1.to_radians());
    let dlon = lon2 - lon1;
    let y = dlon.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlon.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}

fn compass(deg: f64) -> &'static str {
    const D: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    D[(((deg + 22.5) % 360.0) / 45.0) as usize % 8]
}

fn dist_color(dist: Option<f64>) -> Color {
    match dist {
        Some(d) if d < 25.0 => Color::Green,
        Some(d) if d < 100.0 => Color::Yellow,
        Some(d) if d < 300.0 => Color::White,
        _ => Color::DarkGray,
    }
}

/// Open a URL in the default browser (best effort, non-blocking).
fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
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
