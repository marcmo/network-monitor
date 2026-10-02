use chrono::Utc;
use network_monitor::{config::Config, model::*};

fn observation(target: &str, elapsed_ms: u64, outcome: ProbeOutcome) -> Event {
    Event::Probe(Observation {
        at: Stamp {
            utc: Utc::now(),
            elapsed_ms,
            generation: 0,
        },
        started_elapsed_ms: elapsed_ms.saturating_sub(20),
        duration_ms: 20,
        kind: ProbeKind::Tcp,
        target: target.to_owned(),
        outcome,
    })
}

#[test]
fn healthy_requires_current_observations_and_one_failed_target_is_partial() {
    let config = Config::default();
    let mut monitor = Monitor::new(config.clone());
    assert_eq!(monitor.snapshot(0).status, Status::Starting);
    monitor.apply(observation(
        &config.tcp_targets[0],
        20,
        ProbeOutcome::Success,
    ));
    monitor.apply(observation(
        &config.tcp_targets[1],
        30,
        ProbeOutcome::Timeout,
    ));
    assert_eq!(monitor.snapshot(30).status, Status::Partial);
    monitor.apply(observation(
        &config.tcp_targets[1],
        2020,
        ProbeOutcome::Success,
    ));
    for (kind, target) in [
        (ProbeKind::Dns, config.dns_name.clone()),
        (ProbeKind::Https, config.https_url.clone()),
    ] {
        let Event::Probe(mut value) = observation(&target, 2020, ProbeOutcome::Success) else {
            unreachable!()
        };
        value.kind = kind;
        monitor.apply(Event::Probe(value));
    }
    assert_eq!(monitor.snapshot(2020).status, Status::Healthy);
    assert_eq!(monitor.snapshot(9000).status, Status::Stale);
}

#[test]
fn stopped_sampling_advances_window_and_sleep_rejects_late_health() {
    let config = Config::default();
    let mut monitor = Monitor::new(config.clone());
    for target in &config.tcp_targets {
        monitor.apply(observation(target, 100, ProbeOutcome::Success));
    }
    monitor.apply(Event::Gap(Gap {
        at: Stamp {
            utc: Utc::now(),
            elapsed_ms: 601000,
            generation: 1,
        },
        from_elapsed_ms: 1000,
        reason: GapReason::SleepOrSchedulingDelay,
    }));
    for target in &config.tcp_targets {
        monitor.apply(observation(target, 601001, ProbeOutcome::Success));
    }
    let snapshot = monitor.snapshot(601001);
    assert_eq!(snapshot.status, Status::Stale);
    assert_eq!(snapshot.history.len(), 300);
    assert!(
        snapshot
            .history
            .iter()
            .all(|point| point.explicit_gap && point.successes == 0)
    );
    assert_eq!(snapshot.history.first().unwrap().elapsed_second, 302);
}

#[test]
fn cancelled_probes_are_missing_observations_not_offline_failures() {
    let config = Config::default();
    let mut monitor = Monitor::new(config.clone());
    for target in &config.tcp_targets {
        monitor.apply(observation(target, 100, ProbeOutcome::Cancelled));
    }
    assert_eq!(monitor.snapshot(100).status, Status::Starting);
}

#[test]
fn unavailable_probes_are_gaps_and_cannot_claim_offline() {
    let config = Config::default();
    let mut monitor = Monitor::new(config.clone());
    for target in &config.tcp_targets {
        monitor.apply(observation(
            target,
            100,
            ProbeOutcome::Unavailable("worker unavailable".into()),
        ));
    }
    let snapshot = monitor.snapshot(100);
    assert_eq!(snapshot.status, Status::Stale);
    assert!(
        snapshot
            .history
            .iter()
            .all(|point| point.failures == 0 && point.successes == 0)
    );
    assert!(!snapshot.probes[0].fresh);
}

#[test]
fn browser_checks_must_be_fresh_before_claiming_healthy() {
    let config = Config::default();
    let mut monitor = Monitor::new(config.clone());
    for target in &config.tcp_targets {
        monitor.apply(observation(target, 100, ProbeOutcome::Success));
    }
    assert_eq!(monitor.snapshot(100).status, Status::Partial);
}

#[test]
fn slow_intermittent_dns_and_https_failures_recover_independently() {
    let config = Config::default();
    let mut monitor = Monitor::new(config.clone());
    let values = [
        (ProbeKind::Tcp, config.tcp_targets[0].clone()),
        (ProbeKind::Tcp, config.tcp_targets[1].clone()),
        (ProbeKind::Dns, config.dns_name.clone()),
        (ProbeKind::Https, config.https_url.clone()),
    ];
    for (kind, target) in &values {
        let Event::Probe(mut value) = observation(target, 1000, ProbeOutcome::Success) else {
            unreachable!()
        };
        value.kind = *kind;
        if *kind == ProbeKind::Tcp {
            value.duration_ms = 600;
        }
        monitor.apply(Event::Probe(value));
    }
    assert_eq!(monitor.snapshot(1000).status, Status::Slow);
    for (kind, target) in &values {
        let Event::Probe(mut value) = observation(target, 2000, ProbeOutcome::Success) else {
            unreachable!()
        };
        value.kind = *kind;
        monitor.apply(Event::Probe(value));
    }
    assert_eq!(monitor.snapshot(2000).status, Status::Healthy);
    for (kind, target, failure) in [
        (
            ProbeKind::Dns,
            &config.dns_name,
            ProbeOutcome::DnsError("SERVFAIL".into()),
        ),
        (
            ProbeKind::Https,
            &config.https_url,
            ProbeOutcome::HttpStatus(503),
        ),
        (
            ProbeKind::Dns,
            &config.dns_name,
            ProbeOutcome::Unavailable("busy resolver".into()),
        ),
    ] {
        let Event::Probe(mut value) = observation(target, 2100, failure) else {
            unreachable!()
        };
        value.kind = kind;
        monitor.apply(Event::Probe(value.clone()));
        assert_eq!(monitor.snapshot(2100).status, Status::Partial);
        value.outcome = ProbeOutcome::Success;
        monitor.apply(Event::Probe(value));
        assert_eq!(monitor.snapshot(2100).status, Status::Healthy);
    }
    let snapshot = monitor.snapshot(310_000);
    assert_eq!(snapshot.status, Status::Stale);
    assert_eq!(snapshot.history.len(), 300);
    assert!(
        snapshot
            .history
            .iter()
            .all(|point| point.latency_ms.is_none() && point.failures == 0)
    );
}

#[test]
fn contemporaneous_https_success_vetoes_offline_but_old_https_or_dns_cannot() {
    let config = Config::default();
    let mut monitor = Monitor::new(config.clone());
    for target in &config.tcp_targets {
        monitor.apply(observation(target, 10_000, ProbeOutcome::Refused));
    }
    let Event::Probe(mut https) = observation(&config.https_url, 10_000, ProbeOutcome::Success)
    else {
        unreachable!()
    };
    https.kind = ProbeKind::Https;
    monitor.apply(Event::Probe(https));
    assert_eq!(monitor.snapshot(10_000).status, Status::Partial);
    for target in &config.tcp_targets {
        monitor.apply(observation(target, 12_000, ProbeOutcome::Timeout));
    }
    let Event::Probe(mut dns) = observation(&config.dns_name, 12_000, ProbeOutcome::Success) else {
        unreachable!()
    };
    dns.kind = ProbeKind::Dns;
    monitor.apply(Event::Probe(dns));
    assert_eq!(monitor.snapshot(12_000).status, Status::Offline);
}
