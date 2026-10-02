use std::{path::PathBuf, time::Duration};

use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::MissedTickBehavior;

use crate::{
    config::Config,
    location::{LocationError, RawLocationEvent},
    model::{Event, Gap, GapReason, LocationEvent, LocationState, Monitor, Stamp},
    scheduler::{
        Clock, ClockReading, EVENT_QUEUE_CAPACITY, ProbeRunner, SamplerError, clock_has_gap,
        run_sampler_with_refresh,
    },
    storage::{Recorder, Session, StorageError, TryRecordError},
    traffic::TrafficSource,
    ui::View,
};

pub struct AppOptions {
    pub config: Config,
    pub label: Option<String>,
    pub database: PathBuf,
}

#[derive(Debug)]
pub enum Control {
    Quit,
    TerminalFailed(String),
}

pub struct AppPorts {
    pub controls: mpsc::Receiver<Control>,
    pub locations: mpsc::Receiver<RawLocationEvent>,
    pub location_result: oneshot::Receiver<Result<(), LocationError>>,
    pub views: watch::Sender<Option<View>>,
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Sampler(#[from] SamplerError),
    #[error("cannot prepare recording directory: {0}")]
    Directory(#[from] std::io::Error),
    #[error("recording queue filled; monitoring stopped and completed events were drained")]
    RecordingOverloaded,
    #[error("sampling queue filled; monitoring stopped and completed events were drained")]
    SamplingOverloaded,
    #[error("terminal failed: {0}")]
    Terminal(String),
    #[error("sampling task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

pub async fn run_session<P: ProbeRunner, T: TrafficSource, C: Clock>(
    options: AppOptions,
    runner: P,
    traffic: T,
    clock: C,
    mut ports: AppPorts,
) -> Result<(), AppError> {
    if let Some(parent) = options
        .database
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let session = Session::new(options.label.clone(), &options.config)?;
    let recorder = Recorder::open(&options.database, session.clone()).await?;
    let mut recording_errors = recorder.errors();
    let mut monitor = Monitor::new(options.config.clone());
    let (events_tx, mut events_rx) = mpsc::channel(EVENT_QUEUE_CAPACITY);
    let (stop_tx, stop_rx) = watch::channel(false);
    let (refresh_tx, refresh_rx) = mpsc::channel(1);
    let mut refresh_pending: Option<oneshot::Receiver<ClockReading>> = None;
    let mut sampler = tokio::spawn(run_sampler_with_refresh(
        options.config.clone(),
        runner,
        traffic,
        clock.clone(),
        events_tx,
        stop_rx,
        refresh_rx,
    ));
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut generation = 0;
    let mut pending = Vec::new();
    let mut location_open = true;
    let mut native_pending = true;
    let mut sampler_done = false;
    let mut result = loop {
        tokio::select! {
            biased;
            command = ports.controls.recv() => break match command {
                Some(Control::Quit) | None => Ok(()),
                Some(Control::TerminalFailed(error)) => Err(AppError::Terminal(error)),
            },
            changed = recording_errors.changed() => {
                let reason = recording_errors.borrow().clone();
                if let Some(reason) = reason {
                    break Err(AppError::Storage(StorageError::Closed(reason)));
                }
                if changed.is_err() {
                    break Err(AppError::Storage(StorageError::Closed("recording writer disconnected".into())));
                }
            }
            completed = &mut sampler => {
                sampler_done = true;
                break sampler_result(completed, &mut pending);
            }
            event = events_rx.recv() => {
                match event {
                    Some(event) => if let Err(error) = accept(&recorder, &mut monitor, &mut generation, event, &mut pending) { break Err(error); },
                    None => break Err(AppError::Sampler(SamplerError::ConsumerClosed)),
                }
            }
            location = ports.locations.recv(), if location_open => {
                match location {
                    Some(raw) => {
                        let event = location_event(raw, clock.reading(), generation);
                        if let Err(error) = accept(&recorder, &mut monitor, &mut generation, event, &mut pending) { break Err(error); }
                    }
                    None => location_open = false,
                }
            }
            native_result = &mut ports.location_result, if native_pending => {
                native_pending = false;
                let reason = match native_result {
                    Ok(Err(error)) => error.to_string(),
                    Ok(Ok(())) => "location capture stopped".into(),
                    Err(_) => "location capture disconnected".into(),
                };
                let now = clock.reading();
                let event = location_event(RawLocationEvent {received_utc: now.utc, state: LocationState::Unavailable {reason}}, now, generation);
                if let Err(error) = accept(&recorder, &mut monitor, &mut generation, event, &mut pending) { break Err(error); }
            }
            checked = async {
                match refresh_pending.as_mut() {
                    Some(receiver) => receiver.await,
                    None => std::future::pending().await,
                }
            } => {
                refresh_pending = None;
                let checked = match checked {
                    Ok(checked) => checked,
                    Err(_) => break Err(AppError::Sampler(SamplerError::ConsumerClosed)),
                };
                let now = clock.reading();
                // A suspension after the sampler's reply still needs its ordered gap first.
                if clock_has_gap(&checked, &now, &options.config) {
                    match request_refresh(&refresh_tx) {
                        Ok(receiver) => refresh_pending = Some(receiver),
                        Err(error) => break Err(error),
                    }
                    continue;
                }
                let view = View {
                    snapshot: monitor.snapshot(now.elapsed_ms), config: options.config.clone(),
                    utc: now.utc, elapsed_ms: now.elapsed_ms, session_id: session.id.clone(),
                    label: session.label.clone(), database: options.database.clone(),
                };
                ports.views.send_replace(Some(view));
            }
            _ = interval.tick(), if refresh_pending.is_none() => {
                match request_refresh(&refresh_tx) {
                    Ok(receiver) => refresh_pending = Some(receiver),
                    Err(error) => break Err(error),
                }
            }
        }
    };
    let _ = stop_tx.send(true);
    ports.locations.close();
    if !sampler_done && let Err(error) = sampler_result(sampler.await, &mut pending) {
        result = result.and(Err(error));
    }
    while let Ok(event) = events_rx.try_recv() {
        pending.push(event);
    }
    generation = pending
        .iter()
        .map(|event| event.stamp().generation)
        .fold(generation, u64::max);
    while let Ok(raw) = ports.locations.try_recv() {
        pending.push(location_event(raw, clock.reading(), generation));
    }
    if native_pending && let Ok(Err(error)) = ports.location_result.try_recv() {
        let now = clock.reading();
        pending.push(location_event(
            RawLocationEvent {
                received_utc: now.utc,
                state: LocationState::Unavailable {
                    reason: error.to_string(),
                },
            },
            now,
            generation,
        ));
    }
    pending.sort_by_key(|event| event.stamp().elapsed_ms);
    for event in pending {
        generation = generation.max(event.stamp().generation);
        if let Err(error) = recorder.record(event).await {
            result = Err(AppError::Storage(error));
            break;
        }
    }
    let now = clock.reading();
    if let Err(error) = recorder
        .record(Event::Gap(Gap {
            at: stamp(&now, generation),
            from_elapsed_ms: now.elapsed_ms,
            reason: GapReason::Shutdown,
        }))
        .await
    {
        result = Err(AppError::Storage(error));
    }
    if let Err(error) = recorder.finish(now.utc).await {
        result = Err(AppError::Storage(error));
    }
    result
}

fn request_refresh(
    sender: &mpsc::Sender<oneshot::Sender<ClockReading>>,
) -> Result<oneshot::Receiver<ClockReading>, AppError> {
    let (reply, receiver) = oneshot::channel();
    sender
        .try_send(reply)
        .map_err(|_| AppError::Sampler(SamplerError::ConsumerClosed))?;
    Ok(receiver)
}

fn accept(
    recorder: &Recorder,
    monitor: &mut Monitor,
    generation: &mut u64,
    event: Event,
    pending: &mut Vec<Event>,
) -> Result<(), AppError> {
    *generation = (*generation).max(event.stamp().generation);
    monitor.apply(event.clone());
    match recorder.try_record(event) {
        Ok(()) => Ok(()),
        Err(TryRecordError::Full(event)) => {
            pending.push(*event);
            Err(AppError::RecordingOverloaded)
        }
        Err(TryRecordError::Closed(error)) => Err(AppError::Storage(error)),
    }
}

fn sampler_result(
    result: Result<Result<(), SamplerError>, tokio::task::JoinError>,
    pending: &mut Vec<Event>,
) -> Result<(), AppError> {
    match result? {
        Ok(()) => Ok(()),
        Err(SamplerError::Overloaded(events)) => {
            pending.extend(events);
            Err(AppError::SamplingOverloaded)
        }
        Err(error) => Err(AppError::Sampler(error)),
    }
}

fn stamp(now: &ClockReading, generation: u64) -> Stamp {
    Stamp {
        utc: now.utc,
        elapsed_ms: now.elapsed_ms,
        generation,
    }
}

fn location_event(raw: RawLocationEvent, now: ClockReading, generation: u64) -> Event {
    Event::Location(LocationEvent {
        at: Stamp {
            utc: raw.received_utc,
            ..stamp(&now, generation)
        },
        state: raw.state,
    })
}
