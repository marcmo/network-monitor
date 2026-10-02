use std::{future::Future, pin::Pin, time::Duration};

use chrono::{DateTime, Utc};
use network_monitor::{
    config::Config,
    model::{Event, Monitor, TrafficCounters, TrafficInterface, TrafficRate},
    scheduler::{Clock, ClockReading, ProbeRunner, ProbeSpec, run_sampler_with_traffic},
    traffic::{TrafficSource, TrafficSourceError},
};
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};

#[derive(Clone)]
struct TestClock {
    origin: Instant,
    utc: DateTime<Utc>,
}
impl Clock for TestClock {
    fn reading(&self) -> ClockReading {
        let elapsed_ms = self.origin.elapsed().as_millis() as u64;
        ClockReading {
            utc: self.utc + chrono::Duration::milliseconds(elapsed_ms as i64),
            elapsed_ms,
            awake_elapsed_ms: elapsed_ms,
        }
    }
}
#[derive(Clone)]
struct NoProbes;
impl ProbeRunner for NoProbes {
    fn probe(
        &self,
        _: ProbeSpec,
    ) -> Pin<Box<dyn Future<Output = network_monitor::model::ProbeOutcome> + Send>> {
        Box::pin(std::future::pending())
    }
}
#[derive(Clone)]
struct ControlledTraffic(watch::Receiver<Result<TrafficCounters, TrafficSourceError>>);
impl TrafficSource for ControlledTraffic {
    fn sample(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<TrafficCounters, TrafficSourceError>> + Send>> {
        let reading = self.0.borrow().clone();
        Box::pin(async move { reading })
    }
}
fn counters(received_bytes: u64, sent_bytes: u64) -> TrafficCounters {
    TrafficCounters {
        interface: TrafficInterface {
            name: "en0".into(),
            index: 4,
            change_id: 0,
        },
        received_bytes,
        sent_bytes,
    }
}
async fn settle() {
    for _ in 0..30 {
        tokio::task::yield_now().await;
    }
}
fn traffic(rx: &mut mpsc::Receiver<Event>) -> network_monitor::model::TrafficEvent {
    loop {
        if let Event::Traffic(event) = rx.try_recv().unwrap() {
            return event;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn passive_rates_use_byte_deltas_and_elapsed_time_and_distinguish_idle_from_missing() {
    let (source_tx, source) = watch::channel(Ok(counters(10_000, 20_000)));
    let (events_tx, mut events) = mpsc::channel(128);
    let (stop_tx, stop) = watch::channel(false);
    let task = tokio::spawn(run_sampler_with_traffic(
        Config::default(),
        NoProbes,
        ControlledTraffic(source),
        TestClock {
            origin: Instant::now(),
            utc: Utc::now(),
        },
        events_tx,
        stop,
    ));
    settle().await;
    let first = traffic(&mut events);
    assert_eq!(first.rate, TrafficRate::Baseline);
    assert_eq!(first.counters, Some(counters(10_000, 20_000)));
    source_tx.send(Ok(counters(1_260_000, 270_000))).unwrap();
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    let measured = traffic(&mut events);
    assert_eq!(
        measured.rate,
        TrafficRate::Valid {
            download_mbps: 10.0,
            upload_mbps: 2.0,
            interval_ms: 1000
        }
    );
    let mut monitor = Monitor::new(Config::default());
    monitor.apply(Event::Traffic(measured));
    let state = monitor.snapshot(1000).traffic;
    assert!(state.fresh);
    assert_eq!(state.age_ms, Some(0));
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(
        traffic(&mut events).rate,
        TrafficRate::Valid {
            download_mbps: 0.0,
            upload_mbps: 0.0,
            interval_ms: 1000
        }
    );
    assert!(!monitor.snapshot(4000).traffic.fresh);
    stop_tx.send(true).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn switching_interface_reset_and_unavailable_counters_require_new_baselines() {
    let (source_tx, source) = watch::channel(Ok(counters(100, 200)));
    let (events_tx, mut events) = mpsc::channel(128);
    let (stop_tx, stop) = watch::channel(false);
    let task = tokio::spawn(run_sampler_with_traffic(
        Config::default(),
        NoProbes,
        ControlledTraffic(source),
        TestClock {
            origin: Instant::now(),
            utc: Utc::now(),
        },
        events_tx,
        stop,
    ));
    settle().await;
    assert_eq!(traffic(&mut events).rate, TrafficRate::Baseline);
    let mut other = counters(2_000_000, 4_000_000);
    other.interface.name = "utun3".into();
    other.interface.index = 23;
    for (reading, expected) in [
        (Ok(other.clone()), TrafficRate::InterfaceChanged),
        (
            Ok(other.clone()),
            TrafficRate::Valid {
                download_mbps: 0.0,
                upload_mbps: 0.0,
                interval_ms: 1000,
            },
        ),
        (
            Ok(TrafficCounters {
                received_bytes: 0,
                ..other.clone()
            }),
            TrafficRate::CounterReset,
        ),
        (
            Err(TrafficSourceError("no primary interface".into())),
            TrafficRate::Unavailable {
                reason: "no primary interface".into(),
            },
        ),
        (Ok(other.clone()), TrafficRate::Baseline),
        (
            Ok(other),
            TrafficRate::Valid {
                download_mbps: 0.0,
                upload_mbps: 0.0,
                interval_ms: 1000,
            },
        ),
    ] {
        source_tx.send(reading).unwrap();
        tokio::time::advance(Duration::from_secs(1)).await;
        settle().await;
        assert_eq!(traffic(&mut events).rate, expected);
    }
    stop_tx.send(true).unwrap();
    task.await.unwrap().unwrap();
}

#[derive(Clone)]
struct AdjustableClock {
    base: TestClock,
    offset: watch::Receiver<(i64, i64)>,
}
impl Clock for AdjustableClock {
    fn reading(&self) -> ClockReading {
        let mut now = self.base.reading();
        let (elapsed, awake) = *self.offset.borrow();
        now.elapsed_ms = now.elapsed_ms.saturating_add_signed(elapsed);
        now.awake_elapsed_ms = now.awake_elapsed_ms.saturating_add_signed(awake);
        now
    }
}
#[derive(Clone)]
struct SlowTraffic(ControlledTraffic);
impl TrafficSource for SlowTraffic {
    fn sample(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<TrafficCounters, TrafficSourceError>> + Send>> {
        let value = self.0.0.borrow().clone();
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            value
        })
    }
}

#[tokio::test(start_paused = true)]
async fn sleep_spanning_counter_query_is_recorded_invalid_and_cannot_reseed_the_baseline() {
    let (_source_tx, source) = watch::channel(Ok(counters(100, 200)));
    let (events_tx, mut events) = mpsc::channel(128);
    let (stop_tx, stop) = watch::channel(false);
    let (offset_tx, offset) = watch::channel((0, 0));
    let task = tokio::spawn(run_sampler_with_traffic(
        Config::default(),
        NoProbes,
        SlowTraffic(ControlledTraffic(source)),
        AdjustableClock {
            base: TestClock {
                origin: Instant::now(),
                utc: Utc::now(),
            },
            offset,
        },
        events_tx,
        stop,
    ));
    settle().await;
    tokio::time::advance(Duration::from_millis(100)).await;
    settle().await;
    assert_eq!(traffic(&mut events).rate, TrafficRate::Baseline);
    tokio::time::advance(Duration::from_millis(900)).await;
    settle().await;
    offset_tx.send((3000, 0)).unwrap();
    tokio::time::advance(Duration::from_millis(100)).await;
    settle().await;
    let invalid = traffic(&mut events);
    assert_eq!(invalid.rate, TrafficRate::Gap);
    assert_eq!(invalid.counters, Some(counters(100, 200)));
    tokio::time::advance(Duration::from_millis(900)).await;
    settle().await;
    tokio::time::advance(Duration::from_millis(100)).await;
    settle().await;
    assert_eq!(traffic(&mut events).rate, TrafficRate::Baseline);
    stop_tx.send(true).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn delayed_delivery_is_invalid_even_when_query_future_returns_counters() {
    let (_source_tx, source) = watch::channel(Ok(counters(100, 200)));
    let (events_tx, mut events) = mpsc::channel(128);
    let (stop_tx, stop) = watch::channel(false);
    let task = tokio::spawn(run_sampler_with_traffic(
        Config::default(),
        NoProbes,
        SlowTraffic(ControlledTraffic(source)),
        TestClock {
            origin: Instant::now(),
            utc: Utc::now(),
        },
        events_tx,
        stop,
    ));
    settle().await;
    tokio::time::advance(Duration::from_millis(600)).await;
    settle().await;
    assert_eq!(traffic(&mut events).rate, TrafficRate::Late);
    stop_tx.send(true).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn backwards_and_long_counter_intervals_do_not_fabricate_rates() {
    let (_source_tx, source) = watch::channel(Ok(counters(100, 200)));
    let (events_tx, mut events) = mpsc::channel(128);
    let (stop_tx, stop) = watch::channel(false);
    let (offset_tx, offset) = watch::channel((0, 0));
    let task = tokio::spawn(run_sampler_with_traffic(
        Config::default(),
        NoProbes,
        ControlledTraffic(source),
        AdjustableClock {
            base: TestClock {
                origin: Instant::now(),
                utc: Utc::now(),
            },
            offset,
        },
        events_tx,
        stop,
    ));
    settle().await;
    assert_eq!(traffic(&mut events).rate, TrafficRate::Baseline);
    tokio::time::advance(Duration::from_secs(3)).await;
    settle().await;
    assert_eq!(traffic(&mut events).rate, TrafficRate::Gap);
    offset_tx.send((-2000, -2000)).unwrap();
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(traffic(&mut events).rate, TrafficRate::OutOfOrder);
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(traffic(&mut events).rate, TrafficRate::Baseline);
    stop_tx.send(true).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn blocking_counter_worker_has_bounded_mailbox_and_skips_expired_requests() {
    use network_monitor::traffic::{CounterReader, TrafficWorker};
    struct BlockedReader {
        calls: std::sync::mpsc::SyncSender<()>,
        release: std::sync::mpsc::Receiver<()>,
    }
    impl CounterReader for BlockedReader {
        fn read(&mut self) -> Result<TrafficCounters, TrafficSourceError> {
            self.calls.send(()).unwrap();
            self.release.recv().unwrap();
            Ok(counters(3_000_000_000, 9_000_000_000))
        }
    }
    let (calls_tx, calls) = std::sync::mpsc::sync_channel(4);
    let (release_tx, release) = std::sync::mpsc::sync_channel(4);
    let worker = TrafficWorker::new(BlockedReader {
        calls: calls_tx,
        release,
    })
    .unwrap();
    let first = tokio::spawn(worker.sample());
    while calls.try_recv().is_err() {
        tokio::task::yield_now().await;
    }
    let second = tokio::spawn(worker.sample());
    settle().await;
    let third = worker.sample().await;
    assert!(third.unwrap_err().to_string().contains("busy"));
    second.abort();
    let _ = second.await;
    release_tx.send(()).unwrap();
    assert_eq!(
        first.await.unwrap().unwrap(),
        counters(3_000_000_000, 9_000_000_000)
    );
    drop(worker);
    assert!(calls.recv_timeout(Duration::from_secs(1)).is_err());
}

#[tokio::test(start_paused = true)]
async fn full_ride_traffic_events_reach_session_storage_and_bounded_live_history() {
    use network_monitor::{
        app::{AppOptions, AppPorts, Control, run_session},
        storage::{read_events, read_sessions},
    };
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("traffic.sqlite3");
    let (controls, control_rx) = mpsc::channel(8);
    let (_locations_tx, locations) = mpsc::channel(8);
    let (_native_tx, location_result) = tokio::sync::oneshot::channel();
    let (views, mut view_rx) = watch::channel(None);
    let (source_tx, source) = watch::channel(Ok(counters(5_000_000_000, 9_000_000_000)));
    let task = tokio::spawn(run_session(
        AppOptions {
            config: Config::default(),
            label: Some("passive ride".into()),
            database: database.clone(),
        },
        NoProbes,
        ControlledTraffic(source),
        TestClock {
            origin: Instant::now(),
            utc: Utc::now(),
        },
        AppPorts {
            controls: control_rx,
            locations,
            location_result,
            views,
        },
    ));
    loop {
        view_rx.changed().await.unwrap();
        if view_rx
            .borrow()
            .as_ref()
            .is_some_and(|view| view.snapshot.traffic.observation.is_some())
        {
            break;
        }
    }
    for _ in 0..310 {
        tokio::time::advance(Duration::from_secs(1)).await;
        settle().await;
    }
    source_tx
        .send(Err(TrafficSourceError("route disappeared".into())))
        .unwrap();
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert!(view_rx.borrow().as_ref().unwrap().snapshot.history.len() <= 300);
    controls.send(Control::Quit).await.unwrap();
    task.await.unwrap().unwrap();
    let session = read_sessions(&database).unwrap().remove(0);
    assert_eq!(session.label.as_deref(), Some("passive ride"));
    assert!(session.ended_utc.is_some());
    let events: Vec<_> = read_events(database, &session.id)
        .unwrap()
        .into_iter()
        .filter_map(|event| match event {
            Event::Traffic(event) => Some(event),
            _ => None,
        })
        .collect();
    assert!(events.len() >= 311, "{}", events.len());
    assert_eq!(
        events[0].counters,
        Some(counters(5_000_000_000, 9_000_000_000))
    );
    assert_eq!(events[0].rate, TrafficRate::Baseline);
    assert!(events.iter().any(|event| matches!(
        event.rate,
        TrafficRate::Valid {
            download_mbps: 0.0,
            upload_mbps: 0.0,
            ..
        }
    )));
    assert!(events.last().unwrap().at.elapsed_ms > 300_000);
    assert!(matches!(
        events.last().unwrap().rate,
        TrafficRate::Unavailable { .. }
    ));
}

#[test]
fn live_traffic_history_keeps_interface_scope_across_unavailability_and_expires() {
    use network_monitor::model::{Stamp, TrafficEvent};
    let mut monitor = Monitor::new(Config::default());
    let at = |elapsed_ms| Stamp {
        utc: Utc::now(),
        elapsed_ms,
        generation: 0,
    };
    monitor.apply(Event::Traffic(TrafficEvent {
        at: at(1000),
        started_elapsed_ms: 1000,
        counters: Some(counters(100, 200)),
        rate: TrafficRate::Valid {
            download_mbps: 10.0,
            upload_mbps: 2.0,
            interval_ms: 1000,
        },
    }));
    monitor.apply(Event::Traffic(TrafficEvent {
        at: at(2000),
        started_elapsed_ms: 2000,
        counters: None,
        rate: TrafficRate::Unavailable {
            reason: "gone".into(),
        },
    }));
    let mut changed = counters(1000, 2000);
    changed.interface.index = 20;
    changed.interface.name = "en1".into();
    monitor.apply(Event::Traffic(TrafficEvent {
        at: at(3000),
        started_elapsed_ms: 3000,
        counters: Some(changed.clone()),
        rate: TrafficRate::Baseline,
    }));
    assert!(
        monitor
            .snapshot(3000)
            .history
            .iter()
            .all(|point| point.download_mbps.is_none())
    );
    monitor.apply(Event::Traffic(TrafficEvent {
        at: at(4000),
        started_elapsed_ms: 4000,
        counters: Some(changed),
        rate: TrafficRate::Valid {
            download_mbps: 1.0,
            upload_mbps: 0.0,
            interval_ms: 1000,
        },
    }));
    assert!(
        monitor
            .snapshot(4000)
            .history
            .iter()
            .any(|point| point.download_mbps == Some(1.0))
    );
    assert!(
        monitor
            .snapshot(305_000)
            .history
            .iter()
            .all(|point| point.download_mbps.is_none())
    );
}

#[test]
fn traffic_peaks_within_a_second_survive_compaction() {
    use network_monitor::model::{Stamp, TrafficEvent};
    let mut monitor = Monitor::new(Config::default());
    for (elapsed_ms, down, up) in [(1100, 10.0, 2.0), (1900, 1.0, 0.0)] {
        monitor.apply(Event::Traffic(TrafficEvent {
            at: Stamp {
                utc: Utc::now(),
                elapsed_ms,
                generation: 0,
            },
            started_elapsed_ms: elapsed_ms,
            counters: Some(counters(100, 200)),
            rate: TrafficRate::Valid {
                download_mbps: down,
                upload_mbps: up,
                interval_ms: 1000,
            },
        }));
    }
    let snapshot = monitor.snapshot(2000);
    let bucket = snapshot
        .history
        .iter()
        .find(|point| point.elapsed_second == 1)
        .unwrap();
    assert_eq!(bucket.download_mbps, Some(10.0));
    assert_eq!(bucket.upload_mbps, Some(2.0));
}
