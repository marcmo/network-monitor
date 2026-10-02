use chrono::Utc;
use network_monitor::{
    config::Config,
    model::*,
    ui::{Screen, View, draw, draw_screen},
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
    terminal
        .draw(|frame| draw_screen(frame, &view, Screen::Details))
        .unwrap();
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
        "q Quit",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    terminal.backend_mut().resize(30, 8);
    terminal
        .draw(|frame| draw_screen(frame, &view, Screen::Details))
        .unwrap();
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
    terminal
        .draw(|frame| draw_screen(frame, &view, Screen::Details))
        .unwrap();
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
    let mut terminal = Terminal::new(TestBackend::new(360, 30)).unwrap();
    terminal.draw(|frame| draw(frame, &view)).unwrap();
    let buffer = terminal.backend().buffer();
    let (plot, baseline) = latency_plot(buffer);
    let mut success_columns = std::collections::BTreeSet::new();
    let (mut failures, mut gaps, mut missing) = (0, 0, 0);
    for y in plot.y..=baseline {
        for x in plot.x..plot.right() {
            let cell = &buffer[(x, y)];
            match (cell.symbol(), cell.fg) {
                (symbol, ratatui::style::Color::Cyan) if is_bar(symbol) => {
                    success_columns.insert(x);
                }
                ("x", ratatui::style::Color::Red) => failures += 1,
                ("|", ratatui::style::Color::Gray) => gaps += 1,
                (".", ratatui::style::Color::Gray) => missing += 1,
                _ => {}
            }
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
    let (plot, baseline) = latency_plot(buffer);
    let failure_columns: Vec<_> = (plot.x..plot.right())
        .filter(|x| buffer[(*x, baseline)].symbol() == "x")
        .collect();
    assert_eq!(failure_columns.len(), 1);
    let x = failure_columns[0];
    assert_eq!(buffer[(x, plot.y)].symbol(), "\u{2588}");
    assert_eq!(buffer[(x, plot.y)].fg, ratatui::style::Color::Cyan);
    assert_eq!(buffer[(x + 1, plot.y)].symbol(), " ");
    assert_eq!(buffer[(x, baseline)].fg, ratatui::style::Color::Red);
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
    let (plot, baseline) = latency_plot(buffer);
    assert_eq!(buffer[(plot.right() - 1, baseline)].symbol(), "+");
    assert_eq!(buffer[(plot.right() - 1, plot.bottom() - 1)].symbol(), " ");
    assert_eq!(buffer[(plot.right() - 2, baseline)].symbol(), ".");

    view.elapsed_ms = 601_000;
    view.snapshot = monitor.snapshot(view.elapsed_ms);
    terminal.draw(|frame| draw(frame, &view)).unwrap();
    let buffer = terminal.backend().buffer();
    let (plot, baseline) = latency_plot(buffer);
    for x in plot.x..plot.right() {
        assert_eq!(buffer[(x, baseline)].symbol(), ".");
        for y in plot.y..plot.bottom() {
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
        (1000, "80 ms (TCP max)"),
        (2000, "40 ms (TCP max)"),
        (7000, "-- ms (TCP max)"),
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
        let headline = buffer_text(terminal.backend().buffer());
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
        terminal
            .draw(|frame| draw_screen(frame, &view, Screen::Details))
            .unwrap();
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
            "q Quit",
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

fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
    buffer.content.iter().map(|cell| cell.symbol()).collect()
}

fn empty_view() -> View {
    let config = Config::default();
    View {
        snapshot: Monitor::new(config.clone()).snapshot(0),
        config,
        utc: Utc::now(),
        elapsed_ms: 0,
        session_id: "ride-id".into(),
        label: Some("Train ride".into()),
        database: "rides.sqlite3".into(),
    }
}

#[test]
fn default_overview_reserves_sidebar_and_large_graph_at_80_by_24() {
    let view = empty_view();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| draw(frame, &view)).unwrap();
    let buffer = terminal.backend().buffer();
    let text = buffer_text(buffer);
    for expected in [
        "NETWORK MONITOR",
        "Train ride",
        "LATENCY NOW",
        "-- ms",
        "No fresh TCP success",
        "CONNECTION",
        "STARTING",
        "Down: -- Mbps",
        "Up: -- Mbps",
        "Recording 0m 00s",
        "d Details",
        "q Quit",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    for hidden in [
        "1.1.1.1:443",
        "every 2.0s",
        "Location:",
        "ride-id",
        "rides.sqlite3",
        "Down 0..",
    ] {
        assert!(
            !text.contains(hidden),
            "diagnostic leaked into overview: {hidden}"
        );
    }
    let sidebar = buffer
        .content
        .chunks(80)
        .flat_map(|row| &row[53..])
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(sidebar.contains("LATENCY NOW"));
    assert!(sidebar.contains("STARTING"));
    assert!(sidebar.contains("Up: -- Mbps"));
    assert!(
        buffer
            .content
            .chunks(80)
            .filter(|row| row[0].symbol() == "\u{2502}")
            .count()
            >= 12
    );
}

fn successful_view(latency_ms: u64) -> View {
    let mut view = empty_view();
    let mut monitor = Monitor::new(view.config.clone());
    for (kind, target) in [
        (ProbeKind::Tcp, view.config.tcp_targets[0].clone()),
        (ProbeKind::Tcp, view.config.tcp_targets[1].clone()),
        (ProbeKind::Dns, view.config.dns_name.clone()),
        (ProbeKind::Https, view.config.https_url.clone()),
    ] {
        monitor.apply(Event::Probe(Observation {
            at: Stamp {
                utc: view.utc,
                elapsed_ms: 10_000,
                generation: 0,
            },
            started_elapsed_ms: 9000,
            duration_ms: latency_ms,
            kind,
            target,
            outcome: ProbeOutcome::Success,
        }));
    }
    view.elapsed_ms = 11_000;
    view.snapshot = monitor.snapshot(view.elapsed_ms);
    view
}

#[test]
fn overview_latency_has_large_digits_and_readable_fallback_for_long_values() {
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    for latency_ms in [25, 1000, 99999, u64::MAX] {
        let mut view = successful_view(25);
        for probe in &mut view.snapshot.probes {
            probe.observation.as_mut().unwrap().duration_ms = latency_ms;
        }
        view.label = Some("untrusted\x1b[2J\n".repeat(40));
        terminal.draw(|frame| draw(frame, &view)).unwrap();
        let buffer = terminal.backend().buffer();
        let text = buffer_text(buffer);
        for expected in [
            format!("{latency_ms} ms"),
            "Updated 1.0s ago".into(),
            "HEALTHY".into(),
            "Recording 0m 11s".into(),
            "q Quit".into(),
        ] {
            assert!(text.contains(&expected), "missing {expected}: {text}");
        }
        assert!(!text.contains('\x1b'));
        if latency_ms < 100_000 {
            let blocks = buffer
                .content
                .chunks(80)
                .flat_map(|row| &row[53..])
                .filter(|cell| cell.symbol() == "\u{2588}")
                .count();
            assert!(blocks >= 15, "small latency needs block numerals");
        }
    }
}

#[test]
fn overview_quality_reasons_follow_real_probe_states() {
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let cases = [
        (
            "healthy",
            Status::Healthy,
            "All sampled checks pass",
            ratatui::style::Color::Green,
        ),
        (
            "slow",
            Status::Slow,
            "TCP >= 250 ms",
            ratatui::style::Color::Yellow,
        ),
        (
            "one_tcp_failed",
            Status::Partial,
            "One TCP target failed",
            ratatui::style::Color::Yellow,
        ),
        (
            "https_veto",
            Status::Partial,
            "TCP failed; HTTPS passed",
            ratatui::style::Color::Yellow,
        ),
        (
            "dns_failed",
            Status::Partial,
            "DNS check failed",
            ratatui::style::Color::Yellow,
        ),
        (
            "https_failed",
            Status::Partial,
            "HTTPS check failed",
            ratatui::style::Color::Yellow,
        ),
        (
            "dns_unavailable",
            Status::Partial,
            "DNS check unavailable",
            ratatui::style::Color::Yellow,
        ),
        (
            "https_cancelled",
            Status::Partial,
            "HTTPS check pending",
            ratatui::style::Color::Yellow,
        ),
        (
            "dns_missing",
            Status::Partial,
            "DNS check pending",
            ratatui::style::Color::Yellow,
        ),
        (
            "dns_stale",
            Status::Partial,
            "DNS check stale",
            ratatui::style::Color::Yellow,
        ),
        (
            "tcp_stale",
            Status::Stale,
            "TCP check stale",
            ratatui::style::Color::Gray,
        ),
        (
            "tcp_unavailable",
            Status::Stale,
            "TCP check unavailable",
            ratatui::style::Color::Gray,
        ),
        (
            "offline",
            Status::Offline,
            "Both TCP targets failed",
            ratatui::style::Color::Red,
        ),
    ];
    for (case, expected_status, reason, color) in cases {
        let mut view = successful_view(if case == "slow" { 300 } else { 25 });
        let mut monitor = Monitor::new(view.config.clone());
        for (index, probe) in view.snapshot.probes.iter().enumerate() {
            if case == "dns_missing" && index == 2 {
                continue;
            }
            let mut observation = probe.observation.clone().unwrap();
            observation.outcome = match (case, index) {
                ("one_tcp_failed", 0) | ("https_veto" | "offline", 0 | 1) => ProbeOutcome::Timeout,
                ("dns_failed", 2) => ProbeOutcome::DnsError("failure".into()),
                ("https_failed" | "offline", 3) => ProbeOutcome::TlsOrHttpError("failure".into()),
                ("dns_unavailable", 2) | ("tcp_unavailable", 0) => {
                    ProbeOutcome::Unavailable("unavailable".into())
                }
                ("https_cancelled", 3) => ProbeOutcome::Cancelled,
                _ => ProbeOutcome::Success,
            };
            if case == "tcp_stale" && index == 0 {
                observation.at.elapsed_ms = 0;
            }
            if case == "dns_stale" && index == 2 {
                observation.at.elapsed_ms = 0;
            }
            if case == "dns_stale" && index != 2 {
                observation.at.elapsed_ms = 30_000;
            }
            monitor.apply(Event::Probe(observation));
        }
        if case == "dns_stale" {
            view.elapsed_ms = 30_000;
        }
        view.snapshot = monitor.snapshot(view.elapsed_ms);
        assert_eq!(view.snapshot.status, expected_status, "bad fixture {case}");
        terminal.draw(|frame| draw(frame, &view)).unwrap();
        let buffer = terminal.backend().buffer();
        let text = buffer_text(buffer);
        assert!(text.contains(reason), "{case}: missing {reason}: {text}");
        let status = match expected_status {
            Status::Healthy => "HEALTHY",
            Status::Slow => "SLOW",
            Status::Partial => "PARTIAL",
            Status::Offline => "OFFLINE",
            Status::Stale => "STALE / UNKNOWN",
            Status::Starting => "STARTING",
        };
        let status_cells = buffer
            .content
            .windows(status.len())
            .find(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>() == status)
            .unwrap();
        assert!(status_cells.iter().all(|cell| cell.fg == color));
        if matches!(case, "https_veto" | "offline") {
            assert!(text.contains("No fresh TCP success"));
        }
    }
}

#[test]
fn overview_traffic_preserves_units_interface_and_truthful_freshness_for_all_rates() {
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    for (rate, fresh, expected_rate, reason) in [
        (
            TrafficRate::Valid {
                download_mbps: 0.0,
                upload_mbps: 0.0,
                interval_ms: 1000,
            },
            true,
            "0.00",
            "fresh",
        ),
        (
            TrafficRate::Valid {
                download_mbps: 1.5e100,
                upload_mbps: 1.5e100,
                interval_ms: 1000,
            },
            true,
            "1.50e100",
            "fresh",
        ),
        (
            TrafficRate::Valid {
                download_mbps: 5.0,
                upload_mbps: 2.0,
                interval_ms: 1000,
            },
            false,
            "--",
            "STALE",
        ),
        (TrafficRate::Baseline, false, "--", "baseline"),
        (
            TrafficRate::InterfaceChanged,
            false,
            "--",
            "changed / baseline",
        ),
        (TrafficRate::CounterReset, false, "--", "reset / baseline"),
        (TrafficRate::Gap, false, "--", "gap / unknown"),
        (TrafficRate::Late, false, "--", "late / unknown"),
        (TrafficRate::OutOfOrder, false, "--", "order / unknown"),
        (
            TrafficRate::Unavailable {
                reason: "counter access failed".into(),
            },
            false,
            "--",
            "unavailable",
        ),
    ] {
        let mut view = empty_view();
        view.snapshot.traffic = TrafficState {
            interface: Some(TrafficInterface {
                name: "en0\x1b[2J".repeat(40),
                index: 4,
                change_id: 0,
            }),
            observation: Some(TrafficEvent {
                at: Stamp {
                    utc: view.utc,
                    elapsed_ms: 0,
                    generation: 0,
                },
                started_elapsed_ms: 0,
                counters: None,
                rate,
            }),
            age_ms: Some(4000),
            fresh,
        };
        terminal.draw(|frame| draw(frame, &view)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        for expected in [
            format!("Down: {expected_rate} Mbps"),
            format!("Up: {expected_rate} Mbps"),
            "On: en0".into(),
            reason.into(),
            "age 4.0s".into(),
            "STARTING".into(),
            "q Quit".into(),
        ] {
            assert!(text.contains(&expected), "missing {expected}: {text}");
        }
        assert!(!text.contains('\x1b'));
    }
}

#[test]
fn details_retains_diagnostics_at_80_by_24_with_clear_return_controls() {
    let view = successful_view(25);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal
        .draw(|frame| draw_screen(frame, &view, Screen::Details))
        .unwrap();
    let text = buffer_text(terminal.backend().buffer());
    for expected in [
        "NETWORK MONITOR / DETAILS",
        "Train ride",
        "HEALTHY",
        "Latency: 25ms",
        "TCP 1.1.1.1:443",
        "TCP 8.8.8.8:443",
        "DNS example.com",
        "HTTPS https://www.gstatic.com/generate_204",
        "age 1.0s fresh",
        "every 2.0s / limit 1.5s",
        "every 15.0s / limit 2.0s",
        "every 30.0s / limit 3.0s",
        "Down 0..",
        "Up 0..",
        "last 5 minutes",
        "Location:",
        "Recording ID: ride-id",
        "Database: rides.sqlite3",
        "Recording 0m 11s",
        "Esc Back",
        "d Overview",
        "q Quit",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
}

fn latency_plot(buffer: &ratatui::buffer::Buffer) -> (ratatui::layout::Rect, u16) {
    let width = usize::from(buffer.area.width);
    let top = buffer
        .content
        .chunks(width)
        .position(|row| {
            row.iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
                .contains("TCP latency")
        })
        .unwrap();
    let row = &buffer.content[top * width..(top + 1) * width];
    let left = row
        .iter()
        .position(|cell| cell.symbol() == "\u{250c}")
        .unwrap();
    let right = row
        .iter()
        .position(|cell| cell.symbol() == "\u{2510}")
        .unwrap();
    let bottom = buffer
        .content
        .chunks(width)
        .enumerate()
        .skip(top + 1)
        .find(|(_, row)| row[left].symbol() == "\u{2514}")
        .unwrap()
        .0;
    let baseline = bottom - 2;
    (
        ratatui::layout::Rect::new(
            (left + 1) as u16,
            (top + 1) as u16,
            (right - left - 1) as u16,
            (baseline - top - 1) as u16,
        ),
        baseline as u16,
    )
}

#[test]
fn both_screens_keep_essential_readings_when_terminal_is_small() {
    let view = successful_view(25);
    for screen in [Screen::Overview, Screen::Details] {
        let mut terminal = Terminal::new(TestBackend::new(30, 8)).unwrap();
        terminal
            .draw(|frame| draw_screen(frame, &view, screen))
            .unwrap();
        let text = buffer_text(terminal.backend().buffer());
        for expected in [
            "Terminal too small",
            "80x24",
            "HEALTHY",
            "25 ms",
            "Recording 0m 11s",
            "q Quit",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
    }
}

#[test]
fn overview_shows_the_full_native_interface_name_at_minimum_size() {
    let mut view = empty_view();
    view.snapshot.traffic.interface = Some(TrafficInterface {
        name: "interface123456".into(),
        index: 4,
        change_id: 0,
    });
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| draw(frame, &view)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("interface123456"), "{text}");
}
