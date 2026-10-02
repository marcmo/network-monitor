use std::path::PathBuf;

use chrono::{DateTime, Utc};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Sparkline, Widget},
};

use crate::{config::Config, model::*};

#[derive(Clone, Debug)]
pub struct View {
    pub snapshot: Snapshot,
    pub config: Config,
    pub utc: DateTime<Utc>,
    pub elapsed_ms: u64,
    pub session_id: String,
    pub label: Option<String>,
    pub database: PathBuf,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Screen {
    #[default]
    Overview,
    Details,
}

pub fn draw(frame: &mut Frame<'_>, view: &View) {
    draw_screen(frame, view, Screen::Overview);
}

pub fn draw_screen(frame: &mut Frame<'_>, view: &View, screen: Screen) {
    let area = frame.area();
    if area.width < 80 || area.height < 24 {
        let (status, color) = status_text(view.snapshot.status);
        let latency = current_latency(&view.snapshot)
            .and_then(|probe| probe.observation.as_ref())
            .map_or_else(
                || "--".into(),
                |observation| observation.duration_ms.to_string(),
            );
        frame.render_widget(
            Paragraph::new(vec![
                Line::from("Terminal too small"),
                Line::from("Resize to at least 80x24"),
                Line::styled(status, Style::default().fg(color)),
                Line::from(format!("{latency} ms (TCP max)")),
                Line::from(format!(
                    "Recording {}m {:02}s",
                    view.elapsed_ms / 60_000,
                    view.elapsed_ms / 1000 % 60
                )),
                Line::from("q Quit / Ctrl-C"),
            ]),
            area,
        );
        return;
    }
    match screen {
        Screen::Overview => draw_overview(frame, view),
        Screen::Details => draw_details(frame, view),
    }
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, view: &View, title: &'static str) {
    let block = Block::default().borders(Borders::BOTTOM);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let columns = Layout::horizontal([Constraint::Length(28), Constraint::Min(0)]).split(inner);
    frame.render_widget(
        Paragraph::new(title).style(Style::default().fg(Color::Gray)),
        columns[0],
    );
    frame.render_widget(
        Paragraph::new(terminal_text(
            view.label.as_deref().unwrap_or("Unlabelled ride"),
        ))
        .alignment(Alignment::Right)
        .style(Style::default().fg(Color::Gray)),
        columns[1],
    );
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, view: &View, screen: Screen) {
    let block = Block::default().borders(Borders::TOP);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let controls = match screen {
        Screen::Overview => "d Details | q Quit / Ctrl-C",
        Screen::Details => "d Overview / Esc Back | q Quit / Ctrl-C",
    };
    let columns = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(controls.len() as u16),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new(format!(
            "Recording {}m {:02}s",
            view.elapsed_ms / 60_000,
            view.elapsed_ms / 1000 % 60
        )),
        columns[0],
    );
    frame.render_widget(Paragraph::new(controls), columns[1]);
}

fn current_latency(snapshot: &Snapshot) -> Option<&ProbeState> {
    snapshot
        .probes
        .iter()
        .filter(|probe| probe.kind == ProbeKind::Tcp && probe.fresh)
        .filter(|probe| {
            probe
                .observation
                .as_ref()
                .is_some_and(|observation| observation.outcome.is_success())
        })
        .max_by_key(|probe| {
            probe
                .observation
                .as_ref()
                .map(|observation| observation.duration_ms)
        })
}

fn draw_overview(frame: &mut Frame<'_>, view: &View) {
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(20),
        Constraint::Length(2),
    ])
    .split(frame.area());
    draw_header(frame, rows[0], view, "NETWORK MONITOR");
    draw_footer(frame, rows[2], view, Screen::Overview);
    let columns = Layout::horizontal([Constraint::Min(0), Constraint::Length(27)]).split(rows[1]);
    let graph_rows =
        Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).split(columns[0]);
    frame.render_widget(
        LatencyGraph {
            history: &view.snapshot.history,
            now_second: view.elapsed_ms / 1000,
        },
        graph_rows[0],
    );
    frame.render_widget(
        Paragraph::new(" + OK  x fail  | gap  . missing\n TCP connection timing"),
        graph_rows[1],
    );
    let block = Block::default().borders(Borders::LEFT);
    let inner = block.inner(columns[1]);
    frame.render_widget(block, columns[1]);
    let sections = Layout::vertical([
        Constraint::Length(8),
        Constraint::Length(6),
        Constraint::Min(6),
    ])
    .split(inner);
    draw_latency(frame, sections[0], &view.snapshot);
    let (status, color) = status_text(view.snapshot.status);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(" CONNECTION"),
            Line::from(Span::styled(
                format!(" {status}"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )),
            Line::from(format!(" {}", status_reason(view))),
        ])
        .block(Block::default().borders(Borders::TOP)),
        sections[1],
    );
    let traffic = &view.snapshot.traffic;
    let (down, up, status) = traffic_values(traffic);
    let interface = traffic
        .interface
        .as_ref()
        .map_or_else(|| "--".into(), |interface| terminal_text(&interface.name));
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                format!(" Down: {down} Mbps"),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                format!(" Up: {up} Mbps"),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(format!(" On: {interface}")),
            Line::from(format!(" {status}")),
            Line::from(format!(" age {}", age_text(traffic.age_ms))),
        ])
        .block(
            Block::default()
                .borders(Borders::TOP)
                .title(" PASSIVE TRAFFIC "),
        ),
        sections[2],
    );
}

fn status_reason(view: &View) -> String {
    match view.snapshot.status {
        Status::Starting => "Waiting for TCP checks".into(),
        Status::Healthy => "All sampled checks pass".into(),
        Status::Slow => format!("TCP >= {} ms", view.config.slow_latency_ms),
        Status::Offline => "Both TCP targets failed".into(),
        Status::Stale => view
            .snapshot
            .probes
            .iter()
            .filter(|probe| probe.kind == ProbeKind::Tcp)
            .find_map(|probe| probe_issue(probe).map(|issue| format!("TCP check {issue}")))
            .unwrap_or_else(|| "TCP freshness unknown".into()),
        Status::Partial => {
            let successful_tcp = view
                .snapshot
                .probes
                .iter()
                .filter(|probe| probe.kind == ProbeKind::Tcp && probe.fresh)
                .filter(|probe| {
                    probe
                        .observation
                        .as_ref()
                        .is_some_and(|observation| observation.outcome.is_success())
                })
                .count();
            match successful_tcp {
                0 => "TCP failed; HTTPS passed".into(),
                1 => "One TCP target failed".into(),
                _ => view
                    .snapshot
                    .probes
                    .iter()
                    .find_map(|probe| {
                        let kind = match probe.kind {
                            ProbeKind::Tcp => "TCP",
                            ProbeKind::Dns => "DNS",
                            ProbeKind::Https => "HTTPS",
                        };
                        probe_issue(probe).map(|issue| format!("{kind} check {issue}"))
                    })
                    .unwrap_or_else(|| "Some checks incomplete".into()),
            }
        }
    }
}

fn probe_issue(probe: &ProbeState) -> Option<&'static str> {
    match probe
        .observation
        .as_ref()
        .map(|observation| &observation.outcome)
    {
        None => Some("pending"),
        Some(ProbeOutcome::Unavailable(_)) => Some("unavailable"),
        Some(ProbeOutcome::Cancelled) => Some("cancelled"),
        Some(_) if !probe.fresh => Some("stale"),
        Some(outcome) if !outcome.is_success() => Some("failed"),
        Some(_) => None,
    }
}

fn draw_latency(frame: &mut Frame<'_>, area: Rect, snapshot: &Snapshot) {
    const DIGIT_ROWS: [[&str; 10]; 5] = [
        [
            "111", "010", "111", "111", "101", "111", "111", "111", "111", "111",
        ],
        [
            "101", "110", "001", "001", "101", "100", "100", "001", "101", "101",
        ],
        [
            "101", "010", "111", "111", "111", "111", "111", "001", "111", "111",
        ],
        [
            "101", "010", "100", "001", "001", "001", "101", "001", "101", "001",
        ],
        [
            "111", "111", "111", "111", "001", "111", "111", "001", "111", "111",
        ],
    ];
    let probe = current_latency(snapshot);
    let latency = probe
        .and_then(|probe| probe.observation.as_ref())
        .map_or_else(
            || "--".into(),
            |observation| observation.duration_ms.to_string(),
        );
    let freshness = probe.and_then(|probe| probe.age_ms).map_or_else(
        || "No fresh TCP success".into(),
        |age| format!("Updated {:.1}s ago", age as f64 / 1000.0),
    );
    let color = if probe.is_some() {
        Color::Cyan
    } else {
        Color::Gray
    };
    let mut lines = vec![Line::from(" LATENCY NOW")];
    for (row, patterns) in DIGIT_ROWS.iter().enumerate() {
        let mut line = String::new();
        if latency.len() <= 5 {
            for digit in latency.chars() {
                line.push(' ');
                let pattern = match digit.to_digit(10) {
                    Some(digit) => patterns[digit as usize],
                    None if row == 2 => "111",
                    None => "000",
                };
                line.extend(
                    pattern
                        .chars()
                        .map(|pixel| if pixel == '1' { '\u{2588}' } else { ' ' }),
                );
            }
            if row == 4 {
                line.push_str(" ms");
            }
        } else if row == 2 {
            line = format!(" {latency} ms");
        }
        lines.push(Line::styled(
            line,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
    lines.push(Line::from(if latency.len() <= 5 {
        format!(" {latency} ms (TCP max)")
    } else {
        " TCP max (fresh)".into()
    }));
    lines.push(Line::from(format!(" {freshness}")));
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_details(frame: &mut Frame<'_>, view: &View) {
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Length(2),
    ])
    .split(frame.area());
    draw_header(frame, rows[0], view, "NETWORK MONITOR / DETAILS");
    let (status, color) = status_text(view.snapshot.status);
    let latency = current_latency(&view.snapshot)
        .and_then(|probe| probe.observation.as_ref())
        .map_or_else(
            || "--".into(),
            |observation| format!("{}ms", observation.duration_ms),
        );
    let traffic = &view.snapshot.traffic;
    let (_, _, traffic_status) = traffic_values(traffic);
    let interface = traffic
        .interface
        .as_ref()
        .map_or_else(|| "--".into(), |interface| terminal_text(&interface.name));
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    format!(" {status}"),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" | Latency: {latency} (TCP max)")),
            ]),
            traffic_headline(traffic),
            Line::from(format!(
                " Traffic: age {} {traffic_status} | Interface: {interface}",
                age_text(traffic.age_ms)
            )),
        ]),
        rows[1],
    );
    let traffic_rows =
        Layout::vertical([Constraint::Length(1), Constraint::Length(2)]).split(rows[2]);
    frame.render_widget(
        Paragraph::new(" TRAFFIC HISTORY | last 5 minutes | _ zero . missing | gap"),
        traffic_rows[0],
    );
    frame.render_widget(
        TrafficGraph {
            history: &view.snapshot.history,
            now_second: view.elapsed_ms / 1000,
        },
        traffic_rows[1],
    );
    let mut probes = Vec::with_capacity(9);
    for probe in &view.snapshot.probes {
        let (kind, interval, timeout) = match probe.kind {
            ProbeKind::Tcp => (
                "TCP",
                view.config.tcp_interval_ms,
                view.config.tcp_timeout_ms,
            ),
            ProbeKind::Dns => (
                "DNS",
                view.config.dns_interval_ms,
                view.config.dns_timeout_ms,
            ),
            ProbeKind::Https => (
                "HTTPS",
                view.config.https_interval_ms,
                view.config.https_timeout_ms,
            ),
        };
        let result = probe.observation.as_ref().map_or_else(
            || "no observation".into(),
            |observation| {
                format!(
                    "{}ms / {}",
                    observation.duration_ms,
                    outcome_text(&observation.outcome)
                )
            },
        );
        let age = probe
            .age_ms
            .map_or_else(|| "--".into(), |age| format!("{:.1}s", age as f64 / 1000.0));
        let freshness = if probe.fresh {
            "fresh"
        } else {
            "STALE/unknown"
        };
        let color = if !probe.fresh {
            Color::Gray
        } else if probe
            .observation
            .as_ref()
            .is_some_and(|observation| observation.outcome.is_success())
        {
            Color::Green
        } else {
            Color::Red
        };
        probes.push(Line::from(format!(
            " {kind} {}",
            terminal_text(&probe.target)
        )));
        probes.push(Line::from(Span::styled(
            format!(
                "   age {age} {freshness} | every {:.1}s / limit {:.1}s | {}",
                interval as f64 / 1000.0,
                timeout as f64 / 1000.0,
                terminal_text(&result)
            ),
            Style::default().fg(color),
        )));
    }
    probes.push(Line::from(format!(
        " display 1Hz | TCP slow >= {}ms | full ride retained",
        view.config.slow_latency_ms
    )));
    frame.render_widget(Paragraph::new(probes), rows[3]);
    let location = match view
        .snapshot
        .location
        .as_ref()
        .map(|location| &location.state)
    {
        None | Some(LocationState::Pending) => {
            "Location: pending / no fix; monitoring continues".into()
        }
        Some(LocationState::Denied) => {
            "Location: permission denied / no fix; monitoring continues".into()
        }
        Some(LocationState::Unavailable { reason }) => {
            format!("Location: unavailable ({reason}); monitoring continues")
        }
        Some(LocationState::Fix {
            latitude,
            longitude,
            horizontal_accuracy_m,
            source_utc,
        }) => {
            let age = view.utc.signed_duration_since(*source_utc).num_seconds();
            let age_text = if age < 0 {
                format!("{}s in future (clock changed?)", age.unsigned_abs())
            } else {
                format!("{age}s old")
            };
            format!(
                "Location: {latitude:.5}, {longitude:.5} | accuracy {horizontal_accuracy_m:.0}m\nSource fix: {} | {age_text}; age is not receipt age",
                source_utc.format("%Y-%m-%d %H:%M:%S UTC")
            )
        }
    };
    frame.render_widget(
        Paragraph::new(
            location
                .lines()
                .map(terminal_text)
                .map(Line::from)
                .collect::<Vec<_>>(),
        )
        .block(Block::default().borders(Borders::TOP)),
        rows[4],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!("Recording ID: {}", terminal_text(&view.session_id))),
            Line::from(format!(
                "Database: {}",
                terminal_text(&view.database.display().to_string())
            )),
        ]),
        rows[5],
    );
    draw_footer(frame, rows[6], view, Screen::Details);
}

fn age_text(age_ms: Option<u64>) -> String {
    age_ms.map_or_else(|| "--".into(), |age| format!("{:.1}s", age as f64 / 1000.0))
}

fn rate_text(rate: f64) -> String {
    if rate >= 1_000_000_000.0 {
        format!("{rate:.2e}")
    } else {
        format!("{rate:.2}")
    }
}

fn traffic_values(state: &TrafficState) -> (String, String, &'static str) {
    match state.observation.as_ref().map(|event| &event.rate) {
        Some(TrafficRate::Valid {
            download_mbps,
            upload_mbps,
            ..
        }) if state.fresh => (rate_text(*download_mbps), rate_text(*upload_mbps), "fresh"),
        rate => (
            "--".into(),
            "--".into(),
            match rate {
                Some(TrafficRate::Valid { .. }) => "STALE",
                Some(TrafficRate::InterfaceChanged) => "changed / baseline",
                Some(TrafficRate::CounterReset) => "reset / baseline",
                Some(TrafficRate::Gap) => "gap / unknown",
                Some(TrafficRate::Late) => "late / unknown",
                Some(TrafficRate::OutOfOrder) => "order / unknown",
                Some(TrafficRate::Unavailable { .. }) => "unavailable",
                Some(TrafficRate::Baseline) | None => "unknown / baseline",
            },
        ),
    }
}

fn traffic_headline(state: &TrafficState) -> Line<'static> {
    let (down, up, _) = traffic_values(state);
    Line::from(vec![
        Span::styled(
            format!(" Down: {down} Mbps"),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  Up: {up} Mbps"),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | passive traffic"),
    ])
}

struct TrafficGraph<'a> {
    history: &'a [GraphPoint],
    now_second: u64,
}
impl Widget for TrafficGraph<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        for (row, download, color, label) in [
            (0, true, Color::Green, "Down"),
            (1, false, Color::Magenta, "Up"),
        ] {
            if area.height <= row || area.width <= 23 {
                continue;
            }
            let plot = Rect::new(area.x + 23, area.y + row, area.width - 23, 1);
            let mut values: Vec<Option<f64>> = vec![None; usize::from(plot.width)];
            let mut gaps = vec![false; usize::from(plot.width)];
            for point in self.history {
                let age = self.now_second.saturating_sub(point.elapsed_second);
                if age > 299 {
                    continue;
                }
                let offset = ((299 - age) * u64::from(plot.width - 1) / 299) as usize;
                gaps[offset] |= point.explicit_gap;
                if let Some(value) = if download {
                    point.download_mbps
                } else {
                    point.upload_mbps
                } {
                    values[offset] = Some(values[offset].map_or(value, |old| old.max(value)));
                }
            }
            let maximum = values
                .iter()
                .flatten()
                .copied()
                .fold(0.0_f64, f64::max)
                .max(0.1);
            buffer.set_stringn(
                area.x,
                area.y + row,
                format!(" {label} 0..{maximum:.1} Mbps"),
                23,
                Style::default().fg(color),
            );
            Sparkline::default()
                .data(
                    values
                        .iter()
                        .map(|value| value.map(|value| (value / maximum * 1000.0).round() as u64)),
                )
                .max(1000)
                .style(Style::default().fg(color))
                .absent_value_symbol(".")
                .absent_value_style(Style::default().fg(Color::Gray))
                .render(plot, buffer);
            for (offset, value) in values.iter().enumerate() {
                let cell = &mut buffer[(plot.x + offset as u16, plot.y)];
                if *value == Some(0.0) {
                    cell.set_symbol("_").set_fg(color);
                } else if value.is_none() && gaps[offset] {
                    cell.set_symbol("|").set_fg(Color::Gray);
                }
            }
        }
    }
}

fn status_text(status: Status) -> (&'static str, Color) {
    match status {
        Status::Starting => ("STARTING", Color::Gray),
        Status::Healthy => ("HEALTHY", Color::Green),
        Status::Slow => ("SLOW", Color::Yellow),
        Status::Partial => ("PARTIAL", Color::Yellow),
        Status::Offline => ("OFFLINE", Color::Red),
        Status::Stale => ("STALE / UNKNOWN", Color::Gray),
    }
}

fn outcome_text(outcome: &ProbeOutcome) -> String {
    match outcome {
        ProbeOutcome::Success => "success".into(),
        ProbeOutcome::Timeout => "timeout".into(),
        ProbeOutcome::Refused => "refused".into(),
        ProbeOutcome::NetworkError(error) => format!("network error: {error}"),
        ProbeOutcome::DnsError(error) => format!("DNS error: {error}"),
        ProbeOutcome::HttpStatus(status) => format!("HTTP {status}"),
        ProbeOutcome::TlsOrHttpError(error) => format!("TLS/HTTP error: {error}"),
        ProbeOutcome::Unavailable(error) => format!("unavailable: {error}"),
        ProbeOutcome::Cancelled => "cancelled".into(),
    }
}

struct LatencyGraph<'a> {
    history: &'a [GraphPoint],
    now_second: u64,
}

impl Widget for LatencyGraph<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let maximum = self
            .history
            .iter()
            .filter_map(|point| point.latency_ms)
            .max()
            .unwrap_or(250)
            .max(250);
        let block = Block::default().borders(Borders::ALL).title(format!(
            " TCP latency (ms), last 5 minutes | 0..{maximum}ms "
        ));
        let inner = block.inner(area);
        block.render(area, buffer);
        if inner.width == 0 || inner.height < 3 {
            return;
        }
        let plot_height = inner.height - 2;
        let baseline = inner.y + plot_height;
        let mut latencies: Vec<Option<u64>> = vec![None; usize::from(inner.width)];
        for x in inner.x..inner.right() {
            buffer[(x, baseline)].set_symbol(".").set_fg(Color::Gray);
        }
        for point in self.history {
            let age = self.now_second.saturating_sub(point.elapsed_second);
            if age > 299 {
                continue;
            }
            let offset = (299 - age) * u64::from(inner.width.saturating_sub(1)) / 299;
            let x = inner.x + offset as u16;
            if point.explicit_gap && buffer[(x, baseline)].symbol() != "x" {
                buffer[(x, baseline)].set_symbol("|").set_fg(Color::Gray);
            }
            if point.failures > 0 {
                buffer[(x, baseline)].set_symbol("x").set_fg(Color::Red);
            } else if point.successes > 0 && buffer[(x, baseline)].symbol() == "." {
                buffer[(x, baseline)].set_symbol("+").set_fg(Color::Cyan);
            }
            if let Some(latency) = point.latency_ms {
                let column = &mut latencies[offset as usize];
                *column = Some(column.map_or(latency, |old| old.max(latency)));
            }
        }
        Sparkline::default()
            .data(latencies)
            .max(maximum)
            .style(Style::default().fg(Color::Cyan))
            .absent_value_style(Style::default().fg(Color::Gray))
            .render(
                Rect::new(inner.x, inner.y, inner.width, plot_height),
                buffer,
            );
        buffer.set_string(
            inner.x,
            inner.bottom() - 1,
            "-5m",
            Style::default().fg(Color::Gray),
        );
        buffer.set_string(
            inner.right().saturating_sub(3),
            inner.bottom() - 1,
            "now",
            Style::default().fg(Color::Gray),
        );
    }
}

fn terminal_text(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                '?'
            } else {
                character
            }
        })
        .collect()
}
