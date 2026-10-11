use std::{future::Future, pin::Pin, time::Duration};

use chrono::{DateTime, Utc};
use network_monitor::{
    config::Config,
    model::{Event, ProbeOutcome, WifiEvent, WifiSampleQuality},
    scheduler::{Clock, ClockReading, ProbeRunner, ProbeSpec, run_sampler_with_wifi},
    wifi::{WifiInterface, WifiLink, WifiMode, WifiSnapshot, WifiSource, WifiSourceError},
};
use tokio::sync::{mpsc, watch};

#[derive(Clone)]
struct TestClock {
    start: tokio::time::Instant,
    utc: DateTime<Utc>,
    sleep: watch::Receiver<u64>,
}

impl Clock for TestClock {
    fn reading(&self) -> ClockReading {
        let awake_elapsed_ms = self.start.elapsed().as_millis() as u64;
        let elapsed_ms = awake_elapsed_ms + *self.sleep.borrow();
        ClockReading {
            utc: self.utc + chrono::Duration::milliseconds(elapsed_ms as i64),
            elapsed_ms,
            awake_elapsed_ms,
        }
    }
}

#[derive(Clone)]
struct NoProbes;

impl ProbeRunner for NoProbes {
    fn probe(&self, _: ProbeSpec) -> Pin<Box<dyn Future<Output = ProbeOutcome> + Send>> {
        Box::pin(std::future::pending())
    }
}

#[derive(Clone)]
struct ControlledWifi(watch::Receiver<Result<WifiSnapshot, WifiSourceError>>);

impl WifiSource for ControlledWifi {
    fn sample(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<WifiSnapshot, WifiSourceError>> + Send>> {
        let reading = self.0.borrow().clone();
        Box::pin(async move { reading })
    }
}

fn associated(ssid: Option<&str>) -> WifiSnapshot {
    WifiSnapshot {
        interfaces: vec![WifiInterface {
            name: "en0".into(),
            power_reported_on: true,
            mode: WifiMode::Station,
            link: WifiLink::Active,
            ssid: ssid.map(str::to_owned),
            bssid: None,
        }],
    }
}

async fn settle() {
    for _ in 0..30 {
        tokio::task::yield_now().await;
    }
}

fn wifi(events: &mut mpsc::Receiver<Event>) -> WifiEvent {
    loop {
        if let Event::Wifi(event) = events.try_recv().unwrap() {
            return event;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn wifi_link_changes_are_timestamped_independently_of_internet_failures() {
    let (source_tx, source) = watch::channel(Ok(associated(Some("Train phone"))));
    let (_sleep_tx, sleep) = watch::channel(0);
    let (events_tx, mut events) = mpsc::channel(128);
    let (stop_tx, stop) = watch::channel(false);
    let utc = "2026-10-10T06:00:00Z".parse().unwrap();
    let task = tokio::spawn(run_sampler_with_wifi(
        Config::default(),
        NoProbes,
        ControlledWifi(source),
        TestClock {
            start: tokio::time::Instant::now(),
            utc,
            sleep,
        },
        events_tx,
        stop,
    ));
    settle().await;
    let first = wifi(&mut events);
    assert_eq!(first.snapshot, Some(associated(Some("Train phone"))));
    assert_eq!(first.quality, WifiSampleQuality::Valid);
    assert_eq!(first.started_elapsed_ms, 0);

    source_tx.send(Ok(associated(None))).unwrap();
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    let hidden = wifi(&mut events);
    assert_eq!(hidden.snapshot, Some(associated(None)));
    assert_eq!(hidden.at.elapsed_ms, 1000);
    assert_eq!(hidden.at.utc, utc + chrono::Duration::seconds(1));

    let mut disconnected = associated(None);
    disconnected.interfaces[0].mode = WifiMode::None;
    disconnected.interfaces[0].link = WifiLink::Inactive;
    source_tx.send(Ok(disconnected.clone())).unwrap();
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    let lost = wifi(&mut events);
    assert_eq!(lost.snapshot, Some(disconnected));
    assert_eq!(lost.at.elapsed_ms, 2000);
    stop_tx.send(true).unwrap();
    task.await.unwrap().unwrap();
}

#[derive(Clone)]
struct DelayedWifi(Duration);

impl WifiSource for DelayedWifi {
    fn sample(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<WifiSnapshot, WifiSourceError>> + Send>> {
        let delay = self.0;
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            Ok(associated(None))
        })
    }
}

#[tokio::test(start_paused = true)]
async fn late_and_sleep_spanning_wifi_reads_retain_raw_evidence_without_claiming_validity() {
    for (advance_ms, sleep_ms, quality) in [
        (600, 0, WifiSampleQuality::Late),
        (100, 3000, WifiSampleQuality::Gap),
    ] {
        let (sleep_tx, sleep) = watch::channel(0);
        let (events_tx, mut events) = mpsc::channel(128);
        let (stop_tx, stop) = watch::channel(false);
        let task = tokio::spawn(run_sampler_with_wifi(
            Config::default(),
            NoProbes,
            DelayedWifi(Duration::from_millis(100)),
            TestClock {
                start: tokio::time::Instant::now(),
                utc: Utc::now(),
                sleep,
            },
            events_tx,
            stop,
        ));
        settle().await;
        sleep_tx.send(sleep_ms).unwrap();
        tokio::time::advance(Duration::from_millis(advance_ms)).await;
        settle().await;
        let observation = wifi(&mut events);
        assert_eq!(observation.quality, quality);
        assert_eq!(observation.snapshot, Some(associated(None)));
        assert_eq!(observation.started_elapsed_ms, 0);
        assert_eq!(observation.at.elapsed_ms, advance_ms + sleep_ms);
        stop_tx.send(true).unwrap();
        task.await.unwrap().unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn wifi_state_and_source_errors_are_saved_in_the_same_ride_as_other_trace_events() {
    use network_monitor::{
        app::{AppOptions, AppPorts, Control, run_session_with_wifi},
        model::{GapReason, Status},
        storage::{read_events, read_sessions},
        traffic::NoTraffic,
    };
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("wifi.sqlite3");
    let (controls, control_rx) = mpsc::channel(8);
    let (_locations_tx, locations) = mpsc::channel(8);
    let (_native_tx, location_result) = tokio::sync::oneshot::channel();
    let (views, mut view_rx) = watch::channel(None);
    let (_sleep_tx, sleep) = watch::channel(0);
    let (source_tx, source) = watch::channel(Ok(associated(Some("Train phone"))));
    let task = tokio::spawn(run_session_with_wifi(
        AppOptions {
            config: Config::default(),
            label: Some("Wi-Fi trace".into()),
            database: database.clone(),
        },
        NoProbes,
        NoTraffic,
        Some(ControlledWifi(source)),
        TestClock {
            start: tokio::time::Instant::now(),
            utc: Utc::now(),
            sleep,
        },
        AppPorts {
            controls: control_rx,
            locations,
            location_result,
            views,
        },
    ));
    view_rx.changed().await.unwrap();
    settle().await;
    for reading in [
        Ok(associated(None)),
        Err(WifiSourceError::Native {
            reason: "Wi-Fi service unavailable".into(),
        }),
        Ok(WifiSnapshot { interfaces: vec![] }),
        Ok(associated(Some("Train phone"))),
    ] {
        source_tx.send(reading).unwrap();
        tokio::time::advance(Duration::from_secs(1)).await;
        settle().await;
    }
    assert_ne!(
        view_rx.borrow().as_ref().unwrap().snapshot.status,
        Status::Healthy
    );
    controls.send(Control::Quit).await.unwrap();
    task.await.unwrap().unwrap();

    let session = read_sessions(&database).unwrap().remove(0);
    assert!(session.ended_utc.is_some());
    let events = read_events(&database, &session.id).unwrap();
    let wifi: Vec<_> = events
        .iter()
        .filter_map(|event| {
            if let Event::Wifi(wifi) = event {
                Some(wifi)
            } else {
                None
            }
        })
        .collect();
    assert!(wifi.len() >= 5);
    assert!(
        wifi.iter()
            .any(|event| event.snapshot == Some(associated(None)))
    );
    assert!(
        wifi.iter()
            .any(|event| event.snapshot == Some(WifiSnapshot { interfaces: vec![] }))
    );
    assert!(
        wifi.iter().any(|event| matches!(&event.quality,
        WifiSampleQuality::Unavailable { reason } if reason.contains("Wi-Fi service unavailable")))
    );
    assert!(events.iter().any(|event| matches!(event, Event::Probe(_))));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Gap(gap) if gap.reason == GapReason::Shutdown))
    );
}

#[tokio::test(start_paused = true)]
async fn wifi_timeouts_are_observations_and_shutdown_cancels_the_next_read_promptly() {
    let (_sleep_tx, sleep) = watch::channel(0);
    let (events_tx, mut events) = mpsc::channel(128);
    let (stop_tx, stop) = watch::channel(false);
    let task = tokio::spawn(run_sampler_with_wifi(
        Config::default(),
        NoProbes,
        DelayedWifi(Duration::from_secs(60)),
        TestClock {
            start: tokio::time::Instant::now(),
            utc: Utc::now(),
            sleep,
        },
        events_tx,
        stop,
    ));
    settle().await;
    tokio::time::advance(Duration::from_millis(501)).await;
    settle().await;
    let timeout = wifi(&mut events);
    assert_eq!(timeout.quality, WifiSampleQuality::Timeout);
    assert_eq!(timeout.snapshot, None);
    assert_eq!(timeout.at.elapsed_ms, 501);
    tokio::time::advance(Duration::from_millis(499)).await;
    settle().await;
    let before_quit = tokio::time::Instant::now();
    stop_tx.send(true).unwrap();
    task.await.unwrap().unwrap();
    assert_eq!(before_quit.elapsed(), Duration::ZERO);
    assert!(events.try_recv().is_err());
}

#[tokio::test(start_paused = true)]
async fn overloaded_wifi_trace_returns_the_completed_sample_for_storage_drain() {
    use network_monitor::scheduler::SamplerError;
    let (_source_tx, source) = watch::channel(Ok(associated(None)));
    let (_sleep_tx, sleep) = watch::channel(0);
    let (events_tx, mut events) = mpsc::channel(1);
    let (_stop_tx, stop) = watch::channel(false);
    let task = tokio::spawn(run_sampler_with_wifi(
        Config::default(),
        NoProbes,
        ControlledWifi(source),
        TestClock {
            start: tokio::time::Instant::now(),
            utc: Utc::now(),
            sleep,
        },
        events_tx,
        stop,
    ));
    settle().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    let Err(SamplerError::Overloaded(pending)) = task.await.unwrap() else {
        panic!("completed Wi-Fi evidence must survive queue overload");
    };
    assert_eq!(wifi(&mut events).at.elapsed_ms, 0);
    assert_eq!(pending.len(), 1);
    let Event::Wifi(event) = &pending[0] else {
        panic!("expected retained Wi-Fi event");
    };
    assert_eq!(event.snapshot, Some(associated(None)));
    assert_eq!(event.quality, WifiSampleQuality::Valid);
    assert_eq!(event.at.elapsed_ms, 1000);
}
