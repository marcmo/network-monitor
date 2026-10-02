use chrono::Utc;
use network_monitor::{
    app::{AppOptions, AppPorts, Control, run_session},
    config::Config,
    location::{LocationError, RawLocationEvent},
    model::*,
    scheduler::{Clock, ClockReading, ProbeRunner, ProbeSpec, SystemClock},
    storage::{read_events, read_sessions},
};
use std::{future::Future, pin::Pin};
use tokio::sync::{mpsc, oneshot, watch};

#[derive(Clone)]
struct Healthy;
impl ProbeRunner for Healthy {
    fn probe(&self, _: ProbeSpec) -> Pin<Box<dyn Future<Output = ProbeOutcome> + Send>> {
        Box::pin(async { ProbeOutcome::Success })
    }
}

struct DisplayFirstClock {
    start: tokio::time::Instant,
    utc: chrono::DateTime<Utc>,
    suspend: watch::Receiver<bool>,
    offset: watch::Sender<u64>,
    is_display: bool,
}

impl Clone for DisplayFirstClock {
    fn clone(&self) -> Self {
        Self {
            start: self.start,
            utc: self.utc,
            suspend: self.suspend.clone(),
            offset: self.offset.clone(),
            is_display: false,
        }
    }
}

impl Clock for DisplayFirstClock {
    fn reading(&self) -> ClockReading {
        if self.is_display && *self.suspend.borrow() {
            self.offset.send_replace(3000);
        }
        let awake_elapsed_ms = self.start.elapsed().as_millis() as u64;
        let elapsed_ms = awake_elapsed_ms + *self.offset.borrow();
        ClockReading {
            utc: self.utc + chrono::Duration::milliseconds(elapsed_ms as i64),
            elapsed_ms,
            awake_elapsed_ms,
        }
    }
}

#[derive(Clone)]
struct SwitchRunner(watch::Receiver<bool>);

impl ProbeRunner for SwitchRunner {
    fn probe(&self, _: ProbeSpec) -> Pin<Box<dyn Future<Output = ProbeOutcome> + Send>> {
        let healthy = *self.0.borrow();
        Box::pin(async move {
            if healthy {
                ProbeOutcome::Success
            } else {
                std::future::pending().await
            }
        })
    }
}

#[tokio::test(start_paused = true)]
async fn display_observing_short_suspend_before_sampler_cannot_publish_cached_health() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("rides.sqlite3");
    let (controls, control_rx) = mpsc::channel(8);
    let (_location_tx, locations) = mpsc::channel(128);
    let (_native_tx, location_result) = oneshot::channel();
    let (views, mut view_rx) = watch::channel(None);
    let (suspend_tx, suspend) = watch::channel(false);
    let (offset, _) = watch::channel(0);
    let (healthy_tx, healthy) = watch::channel(true);
    let task = tokio::spawn(run_session(
        AppOptions {
            config: Config::default(),
            label: None,
            database: database.clone(),
        },
        SwitchRunner(healthy),
        DisplayFirstClock {
            start: tokio::time::Instant::now(),
            utc: Utc::now(),
            suspend,
            offset,
            is_display: true,
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
            .borrow_and_update()
            .as_ref()
            .is_some_and(|view| view.snapshot.status == Status::Healthy)
        {
            break;
        }
    }
    healthy_tx.send(false).unwrap();
    suspend_tx.send(true).unwrap();
    view_rx.changed().await.unwrap();
    let view = view_rx.borrow_and_update().clone().unwrap();
    assert_eq!(view.snapshot.status, Status::Stale);
    assert_eq!(view.snapshot.generation, 1);
    assert!(view.snapshot.probes.iter().all(|probe| !probe.fresh));
    assert!(view.snapshot.history.iter().any(|point| point.explicit_gap));
    controls.send(Control::Quit).await.unwrap();
    task.await.unwrap().unwrap();
    let session = read_sessions(&database).unwrap().remove(0);
    let events = read_events(database, &session.id).unwrap();
    let gaps: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::Gap(gap) if gap.reason == GapReason::SleepOrSchedulingDelay => Some(gap),
            _ => None,
        })
        .collect();
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].at.generation, 1);
    assert!(
        events
            .iter()
            .filter(|event| event.stamp().generation > 0)
            .all(|event| event.stamp().elapsed_ms >= gaps[0].at.elapsed_ms)
    );
}

#[tokio::test]
async fn completed_samples_and_queued_denied_location_survive_quit_with_distinct_sessions() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("nested/rides.sqlite3");
    for _ in 0..2 {
        let (controls, control_rx) = mpsc::channel(8);
        let (location_tx, locations) = mpsc::channel(128);
        let (native_tx, location_result) = oneshot::channel();
        let (views, mut view_rx) = watch::channel(None);
        let task = tokio::spawn(run_session(
            AppOptions {
                config: Config::default(),
                label: Some("train".into()),
                database: database.clone(),
            },
            Healthy,
            SystemClock::new().unwrap(),
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
                .is_some_and(|view| view.snapshot.status == Status::Healthy)
            {
                break;
            }
        }
        location_tx
            .send(RawLocationEvent {
                received_utc: Utc::now(),
                state: LocationState::Denied,
            })
            .await
            .unwrap();
        controls.send(Control::Quit).await.unwrap();
        task.await.unwrap().unwrap();
        drop(native_tx);
    }
    let sessions = read_sessions(&database).unwrap();
    assert_eq!(sessions.len(), 2);
    assert_ne!(sessions[0].id, sessions[1].id);
    for session in sessions {
        assert_eq!(session.label.as_deref(), Some("train"));
        assert!(session.ended_utc.is_some());
        let events = read_events(&database, &session.id).unwrap();
        assert!(
            events
                .iter()
                .filter(|event| matches!(event, Event::Probe(_)))
                .count()
                >= 4
        );
        assert!(events.iter().any(|event| matches!(
            event,
            Event::Location(LocationEvent {
                state: LocationState::Denied,
                ..
            })
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            Event::Gap(Gap {
                reason: GapReason::Shutdown,
                ..
            })
        )));
    }
}

#[tokio::test]
async fn native_setup_failure_is_recorded_without_stopping_connectivity() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("rides.sqlite3");
    let (controls, control_rx) = mpsc::channel(8);
    let (_location_tx, locations) = mpsc::channel(128);
    let (native_tx, location_result) = oneshot::channel();
    let (views, mut view_rx) = watch::channel(None);
    native_tx.send(Err(LocationError::RunLoopSetup)).unwrap();
    let task = tokio::spawn(run_session(
        AppOptions {
            config: Config::default(),
            label: None,
            database: database.clone(),
        },
        Healthy,
        SystemClock::new().unwrap(),
        AppPorts {
            controls: control_rx,
            locations,
            location_result,
            views,
        },
    ));
    loop {
        view_rx.changed().await.unwrap();
        let view = view_rx.borrow();
        if view.as_ref().is_some_and(|view| {
            view.snapshot.status == Status::Healthy
                && matches!(
                    view.snapshot
                        .location
                        .as_ref()
                        .map(|location| &location.state),
                    Some(LocationState::Unavailable { .. })
                )
        }) {
            break;
        }
    }
    controls.send(Control::Quit).await.unwrap();
    task.await.unwrap().unwrap();
    assert!(read_sessions(database).unwrap()[0].ended_utc.is_some());
}

#[tokio::test]
async fn a_terminal_error_stops_sampling_but_drains_completed_events() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("rides.sqlite3");
    let (controls, control_rx) = mpsc::channel(8);
    let (_location_tx, locations) = mpsc::channel(128);
    let (_native_tx, location_result) = oneshot::channel();
    let (views, mut view_rx) = watch::channel(None);
    let task = tokio::spawn(run_session(
        AppOptions {
            config: Config::default(),
            label: None,
            database: database.clone(),
        },
        Healthy,
        SystemClock::new().unwrap(),
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
            .is_some_and(|view| view.snapshot.status == Status::Healthy)
        {
            break;
        }
    }
    controls
        .send(Control::TerminalFailed("stdout disconnected".into()))
        .await
        .unwrap();
    let error = task.await.unwrap().unwrap_err().to_string();
    assert!(error.contains("stdout disconnected"));
    let sessions = read_sessions(&database).unwrap();
    assert!(sessions[0].ended_utc.is_some());
    assert!(
        read_events(database, &sessions[0].id)
            .unwrap()
            .iter()
            .filter(|event| matches!(event, Event::Probe(_)))
            .count()
            >= 4
    );
}

#[tokio::test]
async fn recording_overload_salvages_every_accepted_location_before_stopping() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("rides.sqlite3");
    let (_controls, control_rx) = mpsc::channel(8);
    let (location_tx, locations) = mpsc::channel(128);
    let (_native_tx, location_result) = oneshot::channel();
    let (views, mut view_rx) = watch::channel(None);
    let task = tokio::spawn(run_session(
        AppOptions {
            config: Config::default(),
            label: None,
            database: database.clone(),
        },
        Healthy,
        SystemClock::new().unwrap(),
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
            .is_some_and(|view| view.snapshot.status == Status::Healthy)
        {
            break;
        }
    }
    let lock = rusqlite::Connection::open(&database).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut accepted = 0;
    for _ in 0..400 {
        if location_tx
            .send(RawLocationEvent {
                received_utc: Utc::now(),
                state: LocationState::Denied,
            })
            .await
            .is_err()
        {
            break;
        }
        accepted += 1;
    }
    lock.execute_batch("ROLLBACK").unwrap();
    let error = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(
        matches!(error, network_monitor::app::AppError::RecordingOverloaded),
        "{error}"
    );
    let session = read_sessions(&database).unwrap().remove(0);
    assert!(session.ended_utc.is_some());
    let recorded = read_events(database, &session.id)
        .unwrap()
        .iter()
        .filter(|event| matches!(event, Event::Location(_)))
        .count();
    assert_eq!(recorded, accepted);
    assert!(accepted > 128);
}

#[tokio::test]
async fn a_native_failure_already_queued_at_quit_is_still_recorded() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("rides.sqlite3");
    let (controls, control_rx) = mpsc::channel(8);
    let (_location_tx, locations) = mpsc::channel(128);
    let (native_tx, location_result) = oneshot::channel();
    let (views, mut view_rx) = watch::channel(None);
    let task = tokio::spawn(run_session(
        AppOptions {
            config: Config::default(),
            label: None,
            database: database.clone(),
        },
        Healthy,
        SystemClock::new().unwrap(),
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
            .is_some_and(|view| view.snapshot.status == Status::Healthy)
        {
            break;
        }
    }
    native_tx.send(Err(LocationError::RunLoopSetup)).unwrap();
    controls.send(Control::Quit).await.unwrap();
    task.await.unwrap().unwrap();
    let session = read_sessions(&database).unwrap().remove(0);
    let events = read_events(database, &session.id).unwrap();
    assert!(events.iter().any(|event| matches!(event, Event::Location(LocationEvent {state:LocationState::Unavailable{reason},..}) if reason.contains("run-loop setup failed"))));
}
