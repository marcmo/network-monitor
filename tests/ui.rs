use chrono::Utc;
use network_monitor::{
    config::Config,
    model::*,
    ui::{View, draw},
};
use ratatui::{Terminal, backend::TestBackend};

#[test]
fn render_exposes_status_freshness_scopes_and_honest_location_age() {
    let config = Config::default();
    let now = Utc::now();
    let mut monitor = Monitor::new(config.clone());
    monitor.apply(Event::Location(LocationEvent {
        at: Stamp {
            utc: now,
            elapsed_ms: 0,
            generation: 0,
        },
        state: LocationState::Fix {
            latitude: 52.5,
            longitude: 13.4,
            horizontal_accuracy_m: 60.0,
            source_utc: now - chrono::Duration::minutes(8),
        },
    }));
    let view = View {
        snapshot: monitor.snapshot(10_000),
        config,
        utc: now,
        elapsed_ms: 10_000,
        session_id: "ride-id".into(),
        label: Some("Train ride".into()),
        database: "rides.sqlite3".into(),
    };
    let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
    terminal.draw(|frame| draw(frame, &view)).unwrap();
    let text = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    for expected in [
        "STARTING",
        "Train ride",
        "TCP",
        "1.1.1.1:443",
        "2.0s",
        "1.5s",
        "no observation",
        "480s",
        "60m",
        "52.50000",
        "5 minutes",
        "q quit",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    terminal.backend_mut().resize(30, 8);
    terminal.draw(|frame| draw(frame, &view)).unwrap();
    let text = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(text.contains("Terminal too small"));
}

#[test]
fn narrow_terminal_keeps_freshness_and_timing_visible_with_long_untrusted_text() {
    let config = Config::default();
    let now = Utc::now();
    let mut monitor = Monitor::new(config.clone());
    monitor.apply(Event::Probe(Observation {
        at: Stamp {
            utc: now,
            elapsed_ms: 0,
            generation: 0,
        },
        started_elapsed_ms: 0,
        duration_ms: 1500,
        kind: ProbeKind::Tcp,
        target: config.tcp_targets[0].clone(),
        outcome: ProbeOutcome::NetworkError(format!("\x1b[2J{}", "long error ".repeat(20))),
    }));
    let view = View {
        snapshot: monitor.snapshot(10_000),
        config,
        utc: now,
        elapsed_ms: 10_000,
        session_id: "ride".into(),
        label: Some("bad\x1b[2J\nlabel".into()),
        database: "rides.sqlite3".into(),
    };
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| draw(frame, &view)).unwrap();
    let text = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(!text.contains('\x1b'));
    assert!(text.contains("age 10.0s STALE"));
    assert!(text.contains("every 2.0s / limit 1.5s"));
    assert!(text.contains("1500ms"));
    assert!(text.contains("network error"));
}

#[test]
fn graph_draws_isolated_latency_bars_failures_and_explicit_missing_intervals() {
    let config = Config::default();
    let now = Utc::now();
    let mut monitor = Monitor::new(config.clone());
    for (elapsed_ms, outcome) in [
        (100_000, ProbeOutcome::Success),
        (200_000, ProbeOutcome::Timeout),
    ] {
        monitor.apply(Event::Probe(Observation {
            at: Stamp {
                utc: now,
                elapsed_ms,
                generation: 0,
            },
            started_elapsed_ms: elapsed_ms.saturating_sub(100),
            duration_ms: 100,
            kind: ProbeKind::Tcp,
            target: config.tcp_targets[0].clone(),
            outcome,
        }));
    }
    monitor.apply(Event::Gap(Gap {
        at: Stamp {
            utc: now,
            elapsed_ms: 290_000,
            generation: 1,
        },
        from_elapsed_ms: 280_000,
        reason: GapReason::SleepOrSchedulingDelay,
    }));
    let view = View {
        snapshot: monitor.snapshot(300_000),
        config,
        utc: now,
        elapsed_ms: 300_000,
        session_id: "ride".into(),
        label: None,
        database: "rides.sqlite3".into(),
    };
    let mut terminal = Terminal::new(TestBackend::new(320, 30)).unwrap();
    terminal.draw(|frame| draw(frame, &view)).unwrap();
    let buffer = terminal.backend().buffer();
    let graph_cells = buffer
        .content
        .iter()
        .enumerate()
        .take(320 * 13)
        .skip(320 * 2);
    let mut success_columns = std::collections::BTreeSet::new();
    let (mut failures, mut gaps, mut missing) = (0, 0, 0);
    for (index, cell) in graph_cells {
        match (cell.symbol(), cell.fg) {
            (symbol, ratatui::style::Color::Cyan) if is_bar(symbol) => {
                success_columns.insert(index % 320);
            }
            ("x", ratatui::style::Color::Red) => failures += 1,
            ("|", ratatui::style::Color::Gray) => gaps += 1,
            (".", ratatui::style::Color::Gray) => missing += 1,
            _ => {}
        }
    }
    assert_eq!(
        success_columns.len(),
        1,
        "the graph must not join isolated samples with invented timings"
    );
    assert_eq!(failures, 1);
    assert!(gaps >= 10);
    assert!(missing > 100);
}

fn is_bar(symbol: &str) -> bool {
    matches!(
        symbol,
        "\u{2581}"
            | "\u{2582}"
            | "\u{2583}"
            | "\u{2584}"
            | "\u{2585}"
            | "\u{2586}"
            | "\u{2587}"
            | "\u{2588}"
    )
}

#[test]
fn narrow_sparkline_keeps_peaks_and_failures_when_samples_share_a_column() {
    let config = Config::default();
    let mut monitor = Monitor::new(config.clone());
    let mut snapshot = monitor.snapshot(300_000);
    snapshot.history = vec![
        GraphPoint {
            elapsed_second: 100,
            latency_ms: Some(250),
            successes: 1,
            failures: 1,
            ..GraphPoint::default()
        },
        GraphPoint {
            elapsed_second: 101,
            latency_ms: Some(25),
            successes: 1,
            explicit_gap: true,
            ..GraphPoint::default()
        },
    ];
    let view = View {
        snapshot,
        config,
        utc: Utc::now(),
        elapsed_ms: 300_000,
        session_id: "ride".into(),
        label: None,
        database: "rides.sqlite3".into(),
    };
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| draw(frame, &view)).unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(26, 4)].symbol(), "\u{2588}");
    assert_eq!(buffer[(26, 4)].fg, ratatui::style::Color::Cyan);
    assert_eq!(buffer[(27, 4)].symbol(), " ");
    assert_eq!(buffer[(26, 5)].symbol(), "x");
    assert_eq!(buffer[(26, 5)].fg, ratatui::style::Color::Red);
}

#[test]
fn zero_latency_is_a_success_and_expires_as_the_sparkline_window_advances() {
    let config = Config::default();
    let now = Utc::now();
    let mut monitor = Monitor::new(config.clone());
    monitor.apply(Event::Probe(Observation {
        at: Stamp {
            utc: now,
            elapsed_ms: 300_000,
            generation: 0,
        },
        started_elapsed_ms: 300_000,
        duration_ms: 0,
        kind: ProbeKind::Tcp,
        target: config.tcp_targets[0].clone(),
        outcome: ProbeOutcome::Success,
    }));
    let mut view = View {
        snapshot: monitor.snapshot(300_000),
        config,
        utc: now,
        elapsed_ms: 300_000,
        session_id: "ride".into(),
        label: None,
        database: "rides.sqlite3".into(),
    };
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| draw(frame, &view)).unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(78, 5)].symbol(), "+");
    assert_eq!(buffer[(78, 4)].symbol(), " ");
    assert_eq!(buffer[(77, 5)].symbol(), ".");

    view.elapsed_ms = 601_000;
    view.snapshot = monitor.snapshot(view.elapsed_ms);
    terminal.draw(|frame| draw(frame, &view)).unwrap();
    let buffer = terminal.backend().buffer();
    for x in 1..79 {
        assert_eq!(buffer[(x, 5)].symbol(), ".");
        for y in 4..5 {
            assert_eq!(buffer[(x, y)].symbol(), " ");
        }
    }
}

#[test]
fn headline_latency_uses_only_fresh_successful_tcp_measurements() {
    let config = Config::default();
    let now = Utc::now();
    let mut monitor = Monitor::new(config.clone());
    for (target, duration_ms, kind) in [
        (config.tcp_targets[0].clone(), 40, ProbeKind::Tcp),
        (config.tcp_targets[1].clone(), 80, ProbeKind::Tcp),
        (config.dns_name.clone(), 999, ProbeKind::Dns),
    ] {
        monitor.apply(Event::Probe(Observation {
            at: Stamp {
                utc: now,
                elapsed_ms: 1000,
                generation: 0,
            },
            started_elapsed_ms: 1000 - duration_ms,
            duration_ms,
            kind,
            target,
            outcome: ProbeOutcome::Success,
        }));
    }
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    for (elapsed_ms, expected) in [
        (1000, "Latency: 80ms"),
        (2000, "Latency: 40ms"),
        (7000, "Latency: --"),
    ] {
        if elapsed_ms == 2000 {
            monitor.apply(Event::Probe(Observation {
                at: Stamp {
                    utc: now,
                    elapsed_ms,
                    generation: 0,
                },
                started_elapsed_ms: 500,
                duration_ms: 1500,
                kind: ProbeKind::Tcp,
                target: config.tcp_targets[1].clone(),
                outcome: ProbeOutcome::Timeout,
            }));
        }
        let view = View {
            snapshot: monitor.snapshot(elapsed_ms),
            config: config.clone(),
            utc: now,
            elapsed_ms,
            session_id: "ride".into(),
            label: None,
            database: "rides.sqlite3".into(),
        };
        terminal.draw(|frame| draw(frame, &view)).unwrap();
        let headline = terminal.backend().buffer().content[..80]
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(headline.contains(expected), "{headline}");
    }
}

#[test]
fn passive_traffic_headline_and_history_distinguish_fresh_idle_missing_and_stale_at_80_columns() {
    let config = Config::default();
    let mut monitor = Monitor::new(config.clone());
    for (elapsed_ms, download_mbps, upload_mbps) in [(290_000, 10.0, 2.0), (300_000, 0.0, 0.0)] {
        monitor.apply(Event::Traffic(TrafficEvent {
            at: Stamp {
                utc: Utc::now(),
                elapsed_ms,
                generation: 0,
            },
            started_elapsed_ms: elapsed_ms,
            counters: Some(TrafficCounters {
                interface: TrafficInterface {
                    name: "en0".into(),
                    index: 4,
                    change_id: 0,
                },
                received_bytes: 500,
                sent_bytes: 100,
            }),
            rate: TrafficRate::Valid {
                download_mbps,
                upload_mbps,
                interval_ms: 1000,
            },
        }));
    }
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    for (elapsed_ms, expected) in [(300_000, "Down: 0.00 Mbps"), (304_000, "Down: -- Mbps")] {
        let view = View {
            snapshot: monitor.snapshot(elapsed_ms),
            config: config.clone(),
            utc: Utc::now(),
            elapsed_ms,
            session_id: "ride".into(),
            label: None,
            database: "rides.sqlite3".into(),
        };
        terminal.draw(|frame| draw(frame, &view)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains(expected), "{text}");
        for expected in [
            "Up:",
            "en0",
            "5 minutes",
            "q quit",
            "every 2.0s",
            "Down 0..10.0",
            "Up 0..2.0",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        if elapsed_ms == 300_000 {
            assert!(text.contains("fresh"));
            for color in [ratatui::style::Color::Green, ratatui::style::Color::Magenta] {
                assert!(
                    terminal
                        .backend()
                        .buffer()
                        .content
                        .iter()
                        .any(|cell| cell.symbol() == "_" && cell.fg == color)
                );
                assert!(
                    terminal
                        .backend()
                        .buffer()
                        .content
                        .iter()
                        .any(|cell| is_bar(cell.symbol()) && cell.fg == color)
                );
            }
        } else {
            assert!(text.contains("STALE"));
        }
    }
}
