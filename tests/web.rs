use chrono::Utc;
use network_monitor::{
    config::Config,
    model::*,
    storage::{Recorder, Session},
    ui::View,
    web,
};
use network_monitor_web_types::{
    Health, LiveSnapshot, RecordingDetail, RecordingState, RecordingSummary,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;

fn view() -> View {
    let config = Config::default();
    View {
        snapshot: Monitor::new(config.clone()).snapshot(0),
        config,
        utc: Utc::now(),
        elapsed_ms: 0,
        session_id: "current-session".into(),
        label: Some("Train ride".into()),
        database: "unused.sqlite3".into(),
    }
}

#[tokio::test]
async fn live_snapshot_starts_empty_and_tracks_the_monitor_view() {
    let directory = tempfile::tempdir().unwrap();
    let (sender, receiver) = watch::channel(None);
    let server = web::start(
        0,
        directory.path().join("rides.sqlite3"),
        receiver,
        directory.path().to_path_buf(),
    )
    .await
    .unwrap();
    let url = format!("http://{}/api/live", server.address);
    assert_eq!(
        reqwest::get(&url).await.unwrap().text().await.unwrap(),
        "null"
    );
    sender.send(Some(view())).unwrap();
    let body = reqwest::get(&url).await.unwrap().text().await.unwrap();
    let snapshot: LiveSnapshot = serde_json::from_str(&body).unwrap();
    assert_eq!(snapshot.session_id, "current-session");
    assert_eq!(snapshot.label.as_deref(), Some("Train ride"));
    assert_eq!(snapshot.status, Health::Starting);
    assert!(
        snapshot
            .probes
            .iter()
            .all(|probe| probe.successful.is_none())
    );
    server.shutdown().await.unwrap();
}

fn stamp(start: chrono::DateTime<Utc>, elapsed_ms: u64) -> Stamp {
    Stamp {
        utc: start + chrono::Duration::milliseconds(elapsed_ms as i64),
        elapsed_ms,
        generation: 0,
    }
}

fn successful_checks(config: &Config, start: chrono::DateTime<Utc>, elapsed_ms: u64) -> Vec<Event> {
    [
        (ProbeKind::Tcp, config.tcp_targets[0].clone()),
        (ProbeKind::Tcp, config.tcp_targets[1].clone()),
        (ProbeKind::Dns, config.dns_name.clone()),
        (ProbeKind::Https, config.https_url.clone()),
    ]
    .into_iter()
    .map(|(kind, target)| {
        Event::Probe(Observation {
            at: stamp(start, elapsed_ms),
            started_elapsed_ms: elapsed_ms,
            duration_ms: 20,
            kind,
            target,
            outcome: ProbeOutcome::Success,
        })
    })
    .collect()
}

async fn get_json<T: serde::de::DeserializeOwned>(address: std::net::SocketAddr, path: &str) -> T {
    let response = reqwest::get(format!("http://{address}{path}"))
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    serde_json::from_str(&response.text().await.unwrap()).unwrap()
}

#[tokio::test]
async fn recordings_replay_complete_probe_groups_and_expose_unknown_recordings_as_404() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rides.sqlite3");
    let config = Config::default();
    let session = Session::new(Some("Recorded ride".into()), &config).unwrap();
    let id = session.id.clone();
    let started = session.started_utc;
    let recorder = Recorder::open(&path, session).await.unwrap();
    for event in successful_checks(&config, started, 0) {
        recorder.record(event).await.unwrap();
    }
    for target in config.tcp_targets {
        recorder
            .record(Event::Probe(Observation {
                at: stamp(started, 1000),
                started_elapsed_ms: 1000,
                duration_ms: 1500,
                kind: ProbeKind::Tcp,
                target,
                outcome: ProbeOutcome::Timeout,
            }))
            .await
            .unwrap();
    }
    recorder
        .finish(started + chrono::Duration::seconds(1))
        .await
        .unwrap();
    let (_sender, receiver) = watch::channel(None);
    let server = web::start(0, path, receiver, directory.path().to_path_buf())
        .await
        .unwrap();
    let sessions: Vec<RecordingSummary> = get_json(server.address, "/api/recordings").await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].state, RecordingState::Complete);
    let recording: RecordingDetail =
        get_json(server.address, &format!("/api/recordings/{id}")).await;
    assert_eq!(
        recording
            .points
            .iter()
            .map(|point| point.status)
            .collect::<Vec<_>>(),
        vec![Health::Healthy, Health::Offline]
    );
    assert_eq!(recording.healthy_percent, Some(50.0));
    assert_eq!(recording.median_latency_ms, Some(20));
    assert_eq!(recording.event_count, 6);
    assert!(!recording.truncated);
    let missing = reqwest::get(format!("http://{}/api/recordings/unknown", server.address))
        .await
        .unwrap();
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);
    assert!(
        serde_json::from_str::<network_monitor_web_types::ApiError>(&missing.text().await.unwrap())
            .is_ok()
    );
    server.shutdown().await.unwrap();
}

async fn read_until(stream: &mut tokio::net::TcpStream, needle: &str) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let mut text = String::new();
        let mut buffer = [0_u8; 8192];
        while !text.contains(needle) {
            let length = stream.read(&mut buffer).await.unwrap();
            assert!(length > 0, "stream closed before {needle}: {text}");
            text.push_str(&String::from_utf8_lossy(&buffer[..length]));
        }
        text
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn live_stream_emits_initial_and_updated_snapshots_and_closes_during_shutdown() {
    let directory = tempfile::tempdir().unwrap();
    let (sender, receiver) = watch::channel(None);
    let server = web::start(
        0,
        directory.path().join("rides.sqlite3"),
        receiver,
        directory.path().to_path_buf(),
    )
    .await
    .unwrap();
    let mut stream = tokio::net::TcpStream::connect(server.address)
        .await
        .unwrap();
    stream
        .write_all(
            format!(
                "GET /api/live/stream HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                server.address
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let initial = read_until(&mut stream, "data: null").await;
    assert!(initial.contains("event: snapshot"));
    sender.send(Some(view())).unwrap();
    let update = read_until(&mut stream, "current-session").await;
    assert!(update.contains("event: snapshot"));
    tokio::time::timeout(std::time::Duration::from_secs(2), server.shutdown())
        .await
        .unwrap()
        .unwrap();
    let mut remaining = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        stream.read_to_end(&mut remaining),
    )
    .await
    .unwrap()
    .unwrap();
}

fn location(
    start: chrono::DateTime<Utc>,
    elapsed_ms: u64,
    generation: u64,
    age_ms: i64,
    accuracy: f64,
) -> Event {
    let mut at = stamp(start, elapsed_ms);
    at.generation = generation;
    Event::Location(LocationEvent {
        state: LocationState::Fix {
            latitude: 53.5,
            longitude: 10.0 + elapsed_ms as f64 / 1_000_000.0,
            horizontal_accuracy_m: accuracy,
            source_utc: at.utc - chrono::Duration::milliseconds(age_ms),
        },
        at,
    })
}

#[tokio::test]
async fn recording_route_excludes_unusable_fixes_and_breaks_across_missing_location_and_sleep() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rides.sqlite3");
    let config = Config::default();
    let session = Session::new(None, &config).unwrap();
    let id = session.id.clone();
    let started = session.started_utc;
    let recorder = Recorder::open(&path, session).await.unwrap();
    let mut events = successful_checks(&config, started, 0);
    events.extend([
        location(started, 0, 0, 0, 20.0),
        Event::Location(LocationEvent {
            at: stamp(started, 2000),
            state: LocationState::Unavailable {
                reason: "no fix".into(),
            },
        }),
        location(started, 4000, 0, 0, 20.0),
        location(started, 5000, 0, 0, 2000.0),
        location(started, 6000, 0, -1000, 20.0),
        location(started, 7000, 0, 0, 20.0),
        Event::Gap(Gap {
            at: Stamp {
                generation: 1,
                ..stamp(started, 10_000)
            },
            from_elapsed_ms: 8000,
            reason: GapReason::SleepOrSchedulingDelay,
        }),
        location(started, 11_000, 1, 0, 20.0),
        location(started, 42_000, 1, 31_000, 20.0),
    ]);
    for event in events {
        recorder.record(event).await.unwrap();
    }
    recorder
        .finish(started + chrono::Duration::seconds(42))
        .await
        .unwrap();
    let (_sender, receiver) = watch::channel(None);
    let server = web::start(0, path, receiver, directory.path().to_path_buf())
        .await
        .unwrap();
    let detail: RecordingDetail = get_json(server.address, &format!("/api/recordings/{id}")).await;
    let point = |elapsed| {
        detail
            .points
            .iter()
            .find(|point| point.elapsed_ms == elapsed)
            .unwrap()
    };
    assert!(point(0).location.is_some());
    assert!(point(0).break_before);
    assert!(point(1000).location.is_some());
    assert!(!point(1000).break_before);
    for elapsed in [2000, 3000, 5000, 6000, 8000, 9000, 10_000, 42_000] {
        assert!(
            point(elapsed).location.is_none(),
            "location must be absent at {elapsed}"
        );
    }
    for elapsed in [4000, 7000, 11_000] {
        assert!(point(elapsed).location.is_some());
        assert!(point(elapsed).break_before);
    }
    assert_eq!(point(8000).status, Health::Stale);
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn server_serves_the_built_website_and_rejects_nonlocal_hosts() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("index.html"),
        "<!doctype html><title>Network monitor</title>",
    )
    .unwrap();
    let (_sender, receiver) = watch::channel(None);
    let server = web::start(
        0,
        directory.path().join("rides.sqlite3"),
        receiver,
        directory.path().to_path_buf(),
    )
    .await
    .unwrap();
    let url = format!("http://{}", server.address);
    let response = reqwest::get(&url).await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert!(response.text().await.unwrap().contains("Network monitor"));
    let rebinding = reqwest::Client::new()
        .get(format!("{url}/api/live"))
        .header("host", "attacker.example")
        .send()
        .await
        .unwrap();
    assert_eq!(rebinding.status(), reqwest::StatusCode::FORBIDDEN);
    let unknown = reqwest::get(format!("{url}/api/no-such-endpoint"))
        .await
        .unwrap();
    assert_eq!(unknown.status(), reqwest::StatusCode::NOT_FOUND);
    assert!(
        serde_json::from_str::<network_monitor_web_types::ApiError>(&unknown.text().await.unwrap())
            .is_ok()
    );
    let live = reqwest::get(format!("{url}/api/live")).await.unwrap();
    assert_eq!(live.headers().get("cache-control").unwrap(), "no-store");
    assert!(!live.headers().contains_key("access-control-allow-origin"));
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn live_unknown_and_stale_values_are_not_presented_as_fresh_failures_or_traffic() {
    let directory = tempfile::tempdir().unwrap();
    let mut current = view();
    let mut monitor = Monitor::new(current.config.clone());
    for event in successful_checks(&current.config, current.utc, 0) {
        monitor.apply(event);
    }
    monitor.apply(Event::Probe(Observation {
        at: stamp(current.utc, 0),
        started_elapsed_ms: 0,
        duration_ms: 0,
        kind: ProbeKind::Dns,
        target: current.config.dns_name.clone(),
        outcome: ProbeOutcome::Unavailable("no resolver".into()),
    }));
    monitor.apply(location(current.utc, 0, 0, 0, 20.0));
    monitor.apply(Event::Traffic(TrafficEvent {
        at: stamp(current.utc, 0),
        started_elapsed_ms: 0,
        counters: None,
        rate: TrafficRate::Valid {
            download_mbps: 7.0,
            upload_mbps: 2.0,
            interval_ms: 1000,
        },
    }));
    current.snapshot = monitor.snapshot(0);
    let (sender, receiver) = watch::channel(Some(current.clone()));
    let server = web::start(
        0,
        directory.path().join("rides.sqlite3"),
        receiver,
        directory.path().to_path_buf(),
    )
    .await
    .unwrap();
    let fresh: LiveSnapshot = get_json(server.address, "/api/live").await;
    assert_eq!(fresh.probes[2].successful, None);
    assert_eq!(fresh.status, Health::Partial);
    assert_eq!(fresh.traffic.download_mbps, Some(7.0));
    assert!(fresh.location.fix.is_some());
    current.elapsed_ms = 31_000;
    current.utc += chrono::Duration::seconds(31);
    current.snapshot = monitor.snapshot(31_000);
    sender.send(Some(current)).unwrap();
    let stale: LiveSnapshot = get_json(server.address, "/api/live").await;
    assert_eq!(stale.status, Health::Stale);
    assert!(!stale.traffic.fresh);
    assert_eq!(stale.traffic.download_mbps, None);
    assert_eq!(stale.traffic.upload_mbps, None);
    assert_eq!(stale.location.source_age_ms, Some(31_000));
    assert!(stale.location.fix.is_none());
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn recording_list_distinguishes_current_from_unfinished_and_reads_current_writes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rides.sqlite3");
    let config = Config::default();
    let unfinished = Session::new(Some("Interrupted ride".into()), &config).unwrap();
    let unfinished_id = unfinished.id.clone();
    drop(Recorder::open(&path, unfinished).await.unwrap());
    let current = Session::new(None, &config).unwrap();
    let current_id = current.id.clone();
    let started = current.started_utc;
    let recorder = Recorder::open(&path, current).await.unwrap();
    let mut live = view();
    live.session_id = current_id.clone();
    let (_sender, receiver) = watch::channel(Some(live));
    let server = web::start(0, path, receiver, directory.path().to_path_buf())
        .await
        .unwrap();
    let sessions: Vec<RecordingSummary> = get_json(server.address, "/api/recordings").await;
    assert_eq!(
        sessions
            .iter()
            .find(|session| session.id == unfinished_id)
            .unwrap()
            .state,
        RecordingState::Unfinished
    );
    assert_eq!(
        sessions
            .iter()
            .find(|session| session.id == current_id)
            .unwrap()
            .state,
        RecordingState::Current
    );
    let route = format!("/api/recordings/{current_id}");
    let empty: RecordingDetail = get_json(server.address, &route).await;
    assert!(empty.points.is_empty());
    assert_eq!(empty.healthy_percent, None);
    assert_eq!(empty.median_latency_ms, None);
    recorder
        .record(location(started, 0, 0, 0, 20.0))
        .await
        .unwrap();
    let populated = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let detail: RecordingDetail = get_json(server.address, &route).await;
            if detail.event_count > 0 {
                break detail;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(populated.points.len(), 1);
    assert!(populated.points[0].location.is_some());
    assert_eq!(populated.healthy_percent, None);
    recorder.finish(Utc::now()).await.unwrap();
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn long_recordings_keep_the_whole_route_with_bounded_samples_and_discontinuities() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rides.sqlite3");
    let session = Session::new(None, &Config::default()).unwrap();
    let id = session.id.clone();
    let started = session.started_utc;
    let recorder = Recorder::open(&path, session).await.unwrap();
    for event in [
        location(started, 0, 0, 0, 20.0),
        Event::Location(LocationEvent {
            at: stamp(started, 500),
            state: LocationState::Denied,
        }),
        location(started, 600, 0, 0, 20.0),
        location(started, 20_000_000, 0, 0, 20.0),
    ] {
        recorder.record(event).await.unwrap();
    }
    recorder
        .finish(started + chrono::Duration::milliseconds(20_000_000))
        .await
        .unwrap();
    let (_sender, receiver) = watch::channel(None);
    let server = web::start(0, path, receiver, directory.path().to_path_buf())
        .await
        .unwrap();
    let detail: RecordingDetail = get_json(server.address, &format!("/api/recordings/{id}")).await;
    assert!(detail.truncated);
    assert!(detail.points.len() <= 10_000);
    assert_eq!(detail.points.first().unwrap().elapsed_ms, 0);
    assert_eq!(detail.points.last().unwrap().elapsed_ms, 20_000_000);
    assert!(detail.points.last().unwrap().location.is_some());
    assert!(detail.points[1].location.is_some());
    assert!(
        detail.points[1].break_before,
        "denial and recovery between display samples must split the route"
    );
    assert_eq!(detail.duration_ms, 20_000_000);
    assert_eq!(detail.event_count, 4);
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_gap_invalidates_earlier_samples_when_a_clock_event_precedes_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rides.sqlite3");
    let config = Config::default();
    let session = Session::new(None, &config).unwrap();
    let id = session.id.clone();
    let started = session.started_utc;
    let recorder = Recorder::open(&path, session).await.unwrap();
    for event in successful_checks(&config, started, 0) {
        recorder.record(event).await.unwrap();
    }
    recorder
        .record(location(started, 0, 0, 0, 20.0))
        .await
        .unwrap();
    let adjusted = Stamp {
        utc: started + chrono::Duration::seconds(7),
        elapsed_ms: 4000,
        generation: 0,
    };
    recorder
        .record(Event::ClockAdjusted {
            at: adjusted.clone(),
            delta_ms: 3000,
        })
        .await
        .unwrap();
    recorder
        .record(Event::Gap(Gap {
            at: Stamp {
                generation: 1,
                ..adjusted
            },
            from_elapsed_ms: 1000,
            reason: GapReason::SleepOrSchedulingDelay,
        }))
        .await
        .unwrap();
    recorder
        .finish(started + chrono::Duration::seconds(7))
        .await
        .unwrap();
    let (_sender, receiver) = watch::channel(None);
    let server = web::start(0, path, receiver, directory.path().to_path_buf())
        .await
        .unwrap();
    let detail: RecordingDetail = get_json(server.address, &format!("/api/recordings/{id}")).await;
    assert_eq!(detail.points[0].status, Health::Healthy);
    for point in &detail.points[1..] {
        assert_eq!(
            point.status,
            Health::Stale,
            "gap status at {}",
            point.elapsed_ms
        );
        assert!(
            point.location.is_none(),
            "gap location at {}",
            point.elapsed_ms
        );
        assert!(point.latency_ms.is_none());
        assert!(point.break_before);
    }
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn recording_median_averages_the_middle_pair_for_even_sample_counts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rides.sqlite3");
    let config = Config::default();
    let session = Session::new(None, &config).unwrap();
    let id = session.id.clone();
    let started = session.started_utc;
    let recorder = Recorder::open(&path, session).await.unwrap();
    for (elapsed_ms, latency_ms) in [(0, 20), (1000, 40)] {
        for event in successful_checks(&config, started, elapsed_ms) {
            if let Event::Probe(mut observation) = event {
                observation.duration_ms = latency_ms;
                recorder.record(Event::Probe(observation)).await.unwrap();
            }
        }
    }
    recorder
        .finish(started + chrono::Duration::seconds(1))
        .await
        .unwrap();
    let (_sender, receiver) = watch::channel(None);
    let server = web::start(0, path, receiver, directory.path().to_path_buf())
        .await
        .unwrap();
    let detail: RecordingDetail = get_json(server.address, &format!("/api/recordings/{id}")).await;
    assert_eq!(detail.median_latency_ms, Some(30));
    server.shutdown().await.unwrap();
}
