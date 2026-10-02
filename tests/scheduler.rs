use chrono::Utc;
use network_monitor::{config::Config, model::*, scheduler::*};
use std::{future::Future, pin::Pin, time::Duration};
use tokio::sync::{mpsc, watch};

#[derive(Clone)]
struct TestClock {
    start: tokio::time::Instant,
    utc: chrono::DateTime<Utc>,
}
impl TestClock {
    fn new(utc: chrono::DateTime<Utc>) -> Self {
        Self {
            start: tokio::time::Instant::now(),
            utc,
        }
    }
}
impl Clock for TestClock {
    fn reading(&self) -> ClockReading {
        let elapsed_ms = self.start.elapsed().as_millis() as u64;
        ClockReading {
            utc: self.utc + chrono::Duration::milliseconds(elapsed_ms as i64),
            elapsed_ms,
            awake_elapsed_ms: elapsed_ms,
        }
    }
}
#[derive(Clone)]
struct NeverResponds;
impl ProbeRunner for NeverResponds {
    fn probe(&self, _spec: ProbeSpec) -> Pin<Box<dyn Future<Output = ProbeOutcome> + Send>> {
        Box::pin(std::future::pending())
    }
}

#[derive(Clone)]
struct SwitchRunner {
    healthy: watch::Receiver<bool>,
}
impl ProbeRunner for SwitchRunner {
    fn probe(&self, _spec: ProbeSpec) -> Pin<Box<dyn Future<Output = ProbeOutcome> + Send>> {
        let healthy = *self.healthy.borrow();
        Box::pin(async move {
            if healthy {
                ProbeOutcome::Success
            } else {
                std::future::pending().await
            }
        })
    }
}

async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn outage_after_last_success_is_detected_within_five_seconds_including_render_tick() {
    let config = Config::default();
    let (events_tx, mut events_rx) = mpsc::channel(128);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (healthy_tx, healthy) = watch::channel(true);
    let clock = TestClock::new(Utc::now());
    let task = tokio::spawn(run_sampler_with(
        config.clone(),
        SwitchRunner { healthy },
        clock,
        events_tx,
        shutdown_rx,
    ));
    let mut monitor = Monitor::new(config);
    settle().await;
    while let Ok(event) = events_rx.try_recv() {
        monitor.apply(event);
    }
    assert_eq!(monitor.snapshot(0).status, Status::Healthy);
    tokio::time::advance(Duration::from_millis(1)).await;
    healthy_tx.send(false).unwrap();
    tokio::time::advance(Duration::from_millis(1999)).await;
    settle().await;
    assert_eq!(monitor.snapshot(2000).status, Status::Healthy);
    tokio::time::advance(Duration::from_millis(1500)).await;
    settle().await;
    while let Ok(event) = events_rx.try_recv() {
        monitor.apply(event);
    }
    tokio::time::advance(Duration::from_millis(500)).await;
    assert_eq!(monitor.snapshot(4000).status, Status::Offline);
    healthy_tx.send(true).unwrap();
    settle().await;
    while let Ok(event) = events_rx.try_recv() {
        monitor.apply(event);
    }
    assert_eq!(monitor.snapshot(4000).status, Status::Healthy);
    shutdown_tx.send(true).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn queue_overload_is_an_explicit_error_carrying_the_completed_observation() {
    let config = Config::default();
    let (events_tx, _events_rx) = mpsc::channel(1);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let (_healthy_tx, healthy) = watch::channel(true);
    let result = run_sampler_with(
        config,
        SwitchRunner { healthy },
        TestClock::new(Utc::now()),
        events_tx,
        shutdown_rx,
    )
    .await;
    assert!(
        matches!(result, Err(SamplerError::Overloaded(event)) if event.iter().all(|event| matches!(event, Event::Probe(_))))
    );
}

#[derive(Clone)]
struct OffsetClock {
    base: TestClock,
    offset: watch::Receiver<(u64, i64)>,
}
impl Clock for OffsetClock {
    fn reading(&self) -> ClockReading {
        let mut reading = self.base.reading();
        let (elapsed, wall) = *self.offset.borrow();
        reading.elapsed_ms += elapsed;
        reading.utc += chrono::Duration::milliseconds(wall);
        reading
    }
}

#[tokio::test(start_paused = true)]
async fn sleep_cancels_old_generation_and_wall_clock_steps_are_separate_events() {
    let config = Config::default();
    let (events_tx, mut events_rx) = mpsc::channel(128);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (offset_tx, offset) = watch::channel((0, 0));
    let task = tokio::spawn(run_sampler_with(
        config,
        NeverResponds,
        OffsetClock {
            base: TestClock::new(Utc::now()),
            offset,
        },
        events_tx,
        shutdown_rx,
    ));
    settle().await;
    offset_tx.send((60_000, 60_000)).unwrap();
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    let event = events_rx.try_recv().unwrap();
    assert!(matches!(
        event,
        Event::Gap(Gap {
            at: Stamp {
                generation: 1,
                elapsed_ms: 61_000,
                ..
            },
            ..
        })
    ));
    assert!(events_rx.try_recv().is_err());
    offset_tx.send((60_000, -300_000)).unwrap();
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert!(matches!(
        events_rx.try_recv().unwrap(),
        Event::ClockAdjusted {
            delta_ms: -360_000,
            ..
        }
    ));
    tokio::time::advance(Duration::from_millis(600)).await;
    settle().await;
    while let Ok(event) = events_rx.try_recv() {
        assert_eq!(event.stamp().generation, 1);
    }
    shutdown_tx.send(true).unwrap();
    task.await.unwrap().unwrap();
}

#[derive(Clone)]
struct DelayedSuccess;
impl ProbeRunner for DelayedSuccess {
    fn probe(&self, _spec: ProbeSpec) -> Pin<Box<dyn Future<Output = ProbeOutcome> + Send>> {
        Box::pin(async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            ProbeOutcome::Success
        })
    }
}

#[tokio::test(start_paused = true)]
async fn completion_after_sleep_is_recordable_but_cannot_restore_health_or_graph_points() {
    let config = Config::default();
    let (events_tx, mut events_rx) = mpsc::channel(128);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (offset_tx, offset) = watch::channel((0, 0));
    let task = tokio::spawn(run_sampler_with(
        config.clone(),
        DelayedSuccess,
        OffsetClock {
            base: TestClock::new(Utc::now()),
            offset,
        },
        events_tx,
        shutdown_rx,
    ));
    settle().await;
    offset_tx.send((60_000, 60_000)).unwrap();
    tokio::time::advance(Duration::from_millis(500)).await;
    settle().await;
    let mut monitor = Monitor::new(config);
    let first = events_rx.try_recv().unwrap();
    assert!(matches!(first, Event::Gap(_)));
    monitor.apply(first);
    while let Ok(event) = events_rx.try_recv() {
        monitor.apply(event);
    }
    let snapshot = monitor.snapshot(60_500);
    assert_ne!(snapshot.status, Status::Healthy);
    assert!(snapshot.history.iter().all(|point| point.successes == 0));
    shutdown_tx.send(true).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn short_suspend_invalidates_spanning_probes_below_the_long_stall_threshold() {
    let config = Config::default();
    let (events_tx, mut events_rx) = mpsc::channel(128);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (offset_tx, offset) = watch::channel((0, 0));
    let task = tokio::spawn(run_sampler_with(
        config.clone(),
        DelayedSuccess,
        OffsetClock {
            base: TestClock::new(Utc::now()),
            offset,
        },
        events_tx,
        shutdown_rx,
    ));
    settle().await;
    offset_tx.send((3000, 3000)).unwrap();
    tokio::time::advance(Duration::from_millis(500)).await;
    settle().await;
    let first = events_rx.try_recv().unwrap();
    assert!(matches!(
        first,
        Event::Gap(Gap {
            at: Stamp {
                generation: 1,
                elapsed_ms: 3500,
                ..
            },
            ..
        })
    ));
    let mut monitor = Monitor::new(config);
    monitor.apply(first);
    while let Ok(event) = events_rx.try_recv() {
        if let Event::Probe(observation) = &event {
            assert!(!observation.outcome.is_success());
        }
        monitor.apply(event);
    }
    let snapshot = monitor.snapshot(3500);
    assert!(
        snapshot
            .history
            .iter()
            .all(|point| point.successes == 0 && point.failures == 0)
    );
    shutdown_tx.send(true).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn delayed_scheduling_does_not_turn_an_over_deadline_completion_into_success() {
    let config = Config::default();
    let (events_tx, mut events_rx) = mpsc::channel(128);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let task = tokio::spawn(run_sampler_with(
        config,
        DelayedSuccess,
        TestClock::new(Utc::now()),
        events_tx,
        shutdown_rx,
    ));
    settle().await;
    tokio::time::advance(Duration::from_millis(2000)).await;
    settle().await;
    let mut tcp_count = 0;
    while let Ok(event) = events_rx.try_recv() {
        assert!(!matches!(event, Event::Gap(_)));
        if let Event::Probe(observation) = event {
            if observation.kind == ProbeKind::Tcp {
                tcp_count += 1;
                assert!(!observation.outcome.is_success());
            }
        }
    }
    assert_eq!(tcp_count, 2);
    shutdown_tx.send(true).unwrap();
    task.await.unwrap().unwrap();
}
