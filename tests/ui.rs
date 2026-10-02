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
fn graph_draws_isolated_latency_points_failures_and_explicit_missing_intervals() {
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
    let graph_cells = buffer.content.iter().take(320 * 13).skip(320 * 2);
    let (mut successes, mut failures, mut gaps, mut missing) = (0, 0, 0, 0);
    for cell in graph_cells {
        match (cell.symbol(), cell.fg) {
            ("*", ratatui::style::Color::Cyan) => successes += 1,
            ("x", ratatui::style::Color::Red) => failures += 1,
            ("|", ratatui::style::Color::Gray) => gaps += 1,
            (".", ratatui::style::Color::Gray) => missing += 1,
            _ => {}
        }
    }
    assert_eq!(
        successes, 1,
        "the graph must not join isolated samples with invented timings"
    );
    assert_eq!(failures, 1);
    assert!(gaps >= 10);
    assert!(missing > 100);
}
