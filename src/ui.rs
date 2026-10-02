use std::path::PathBuf;

use chrono::{DateTime, Utc};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
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

pub fn draw(frame: &mut Frame<'_>, view: &View) {
    let area = frame.area();
    if area.width < 80 || area.height < 24 {
        frame.render_widget(Paragraph::new("Terminal too small\nResize to at least 80x24\nRecording continues.\nq quit / Ctrl-C"), area);
        return;
    }
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(8),
        Constraint::Length(3),
        Constraint::Length(2),
    ])
    .split(area);
    let (status, color) = status_text(view.snapshot.status);
    let latency_ms = view
        .snapshot
        .probes
        .iter()
        .filter(|probe| probe.kind == ProbeKind::Tcp && probe.fresh)
        .filter_map(|probe| probe.observation.as_ref())
        .filter(|observation| observation.outcome.is_success())
        .map(|observation| observation.duration_ms)
        .max();
    let latency = latency_ms.map_or_else(|| "--".into(), |value| format!("{value}ms"));
    let label = terminal_text(view.label.as_deref().unwrap_or("Unlabelled ride"));
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    format!(" {status} "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" Latency: {latency}"),
                    Style::default()
                        .fg(if latency_ms.is_some() {
                            Color::Cyan
                        } else {
                            Color::Gray
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(
                    " (TCP max) | {label} | elapsed {}m {:02}s",
                    view.elapsed_ms / 60_000,
                    view.elapsed_ms / 1000 % 60
                )),
            ]),
            traffic_headline(&view.snapshot.traffic),
            Line::from(format!(
                " Recording {} | display 1Hz | current sampled targets",
                view.session_id
            )),
        ]),
        rows[0],
    );
    frame.render_widget(
        LatencyGraph {
            history: &view.snapshot.history,
            now_second: view.elapsed_ms / 1000,
        },
        rows[1],
    );
    frame.render_widget(
        TrafficGraph {
            history: &view.snapshot.history,
            now_second: view.elapsed_ms / 1000,
        },
        rows[2],
    );
    frame.render_widget(
        Paragraph::new(" TCP: + OK x fail | gap . missing | traffic: _ zero . missing | passive"),
        rows[3],
    );
    let mut probes = Vec::with_capacity(8);
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
    frame.render_widget(Paragraph::new(probes), rows[4]);
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
        rows[5],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!(
                "Database: {}",
                terminal_text(&view.database.display().to_string())
            )),
            Line::from("q quit / Ctrl-C | full ride retained | slow threshold configurable"),
        ]),
        rows[6],
    );
}

fn traffic_headline(state: &TrafficState) -> Line<'static> {
    let interface = state
        .interface
        .as_ref()
        .map_or_else(|| "--".into(), |interface| terminal_text(&interface.name));
    let age = state
        .age_ms
        .map_or_else(|| "--".into(), |age| format!("{:.1}s", age as f64 / 1000.0));
    let (down, up, status) = match state.observation.as_ref().map(|event| &event.rate) {
        Some(TrafficRate::Valid {
            download_mbps,
            upload_mbps,
            ..
        }) if state.fresh => (
            format!("{download_mbps:.2}"),
            format!("{upload_mbps:.2}"),
            "fresh",
        ),
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
    };
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
        Span::raw(format!(" | {interface} | age {age} {status}")),
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
