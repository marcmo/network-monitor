use std::{future::Future, pin::Pin, time::Duration};

use chrono::{DateTime, Utc};
use thiserror::Error;
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinSet,
    time::{Instant, MissedTickBehavior},
};

use crate::{
    config::{Config, ConfigError},
    model::*,
};

pub const EVENT_QUEUE_CAPACITY: usize = 128;
// Separate clock reads and millisecond rounding must not be mistaken for a suspend.
const SUSPEND_CLOCK_TOLERANCE_MS: u64 = 50;

#[derive(Clone, Debug)]
pub struct ProbeSpec {
    pub kind: ProbeKind,
    pub target: String,
    pub interval_ms: u64,
    pub timeout_ms: u64,
}

pub trait ProbeRunner: Clone + Send + 'static {
    fn probe(&self, spec: ProbeSpec) -> Pin<Box<dyn Future<Output = ProbeOutcome> + Send>>;
}

#[derive(Clone, Debug)]
pub struct ClockReading {
    pub utc: DateTime<Utc>,
    pub elapsed_ms: u64,
    pub awake_elapsed_ms: u64,
}

pub trait Clock: Clone + Send + 'static {
    fn reading(&self) -> ClockReading;
}

pub(crate) fn clock_has_gap(previous: &ClockReading, now: &ClockReading, config: &Config) -> bool {
    let elapsed_delta = now.elapsed_ms.saturating_sub(previous.elapsed_ms);
    let awake_delta = now
        .awake_elapsed_ms
        .saturating_sub(previous.awake_elapsed_ms);
    elapsed_delta.saturating_sub(awake_delta) > SUSPEND_CLOCK_TOLERANCE_MS
        || elapsed_delta >= config.clock_gap_ms
}

#[derive(Clone)]
pub struct SystemClock {
    #[cfg(target_os = "macos")]
    origin_ns: u128,
    #[cfg(target_os = "macos")]
    awake_origin_ns: u128,
    #[cfg(target_os = "macos")]
    timebase: Timebase,
    #[cfg(not(target_os = "macos"))]
    origin: Instant,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct Timebase {
    numerator: u32,
    denominator: u32,
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn mach_continuous_time() -> u64;
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut Timebase) -> i32;
}

impl SystemClock {
    pub fn new() -> Result<Self, SamplerError> {
        #[cfg(target_os = "macos")]
        {
            let mut timebase = Timebase {
                numerator: 0,
                denominator: 0,
            };
            // The ABI writes only the two u32 fields and does not retain the pointer.
            let status = unsafe { mach_timebase_info(&mut timebase) };
            if status != 0 || timebase.denominator == 0 {
                return Err(SamplerError::Clock("mach_timebase_info failed".into()));
            }
            let origin_ns = continuous_ns(timebase);
            let awake_origin_ns = awake_ns(timebase);
            Ok(Self {
                origin_ns,
                awake_origin_ns,
                timebase,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(Self {
                origin: Instant::now(),
            })
        }
    }
}

#[cfg(target_os = "macos")]
fn continuous_ns(timebase: Timebase) -> u128 {
    // Unlike mach_absolute_time, continuous time includes time spent asleep.
    u128::from(unsafe { mach_continuous_time() }) * u128::from(timebase.numerator)
        / u128::from(timebase.denominator)
}

#[cfg(target_os = "macos")]
fn awake_ns(timebase: Timebase) -> u128 {
    // This clock excludes suspend, allowing short sleeps to be distinguished from scheduling stalls.
    u128::from(unsafe { mach_absolute_time() }) * u128::from(timebase.numerator)
        / u128::from(timebase.denominator)
}

impl Clock for SystemClock {
    fn reading(&self) -> ClockReading {
        #[cfg(target_os = "macos")]
        let elapsed_ms =
            u64::try_from(continuous_ns(self.timebase).saturating_sub(self.origin_ns) / 1_000_000)
                .unwrap_or(u64::MAX);
        #[cfg(not(target_os = "macos"))]
        let elapsed_ms = u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX);
        #[cfg(target_os = "macos")]
        let awake_elapsed_ms =
            u64::try_from(awake_ns(self.timebase).saturating_sub(self.awake_origin_ns) / 1_000_000)
                .unwrap_or(u64::MAX);
        #[cfg(not(target_os = "macos"))]
        let awake_elapsed_ms = elapsed_ms;
        ClockReading {
            utc: Utc::now(),
            elapsed_ms,
            awake_elapsed_ms,
        }
    }
}

#[derive(Debug, Error)]
pub enum SamplerError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("sampling event queue is full; monitoring stopped with unrecorded events")]
    Overloaded(Vec<Event>),
    #[error("sampling event consumer closed")]
    ConsumerClosed,
    #[error("probe task failed: {0}")]
    Task(String),
    #[error("monotonic clock failed: {0}")]
    Clock(String),
}

fn emit(sender: &mpsc::Sender<Event>, event: Event) -> Result<(), SamplerError> {
    sender.try_send(event).map_err(|error| match error {
        mpsc::error::TrySendError::Full(event) => SamplerError::Overloaded(vec![event]),
        mpsc::error::TrySendError::Closed(_) => SamplerError::ConsumerClosed,
    })
}

pub fn probe_specs(config: &Config) -> Vec<ProbeSpec> {
    config
        .tcp_targets
        .iter()
        .map(|target| ProbeSpec {
            kind: ProbeKind::Tcp,
            target: target.clone(),
            interval_ms: config.tcp_interval_ms,
            timeout_ms: config.tcp_timeout_ms,
        })
        .chain([
            ProbeSpec {
                kind: ProbeKind::Dns,
                target: config.dns_name.clone(),
                interval_ms: config.dns_interval_ms,
                timeout_ms: config.dns_timeout_ms,
            },
            ProbeSpec {
                kind: ProbeKind::Https,
                target: config.https_url.clone(),
                interval_ms: config.https_interval_ms,
                timeout_ms: config.https_timeout_ms,
            },
        ])
        .collect()
}

pub async fn run_sampler_with<P: ProbeRunner, C: Clock>(
    config: Config,
    runner: P,
    clock: C,
    events: mpsc::Sender<Event>,
    shutdown: watch::Receiver<bool>,
) -> Result<(), SamplerError> {
    run_sampler(config, runner, clock, events, shutdown, None).await
}

pub(crate) async fn run_sampler_with_refresh<P: ProbeRunner, C: Clock>(
    config: Config,
    runner: P,
    clock: C,
    events: mpsc::Sender<Event>,
    shutdown: watch::Receiver<bool>,
    refresh: mpsc::Receiver<oneshot::Sender<ClockReading>>,
) -> Result<(), SamplerError> {
    run_sampler(config, runner, clock, events, shutdown, Some(refresh)).await
}

async fn run_sampler<P: ProbeRunner, C: Clock>(
    config: Config,
    runner: P,
    clock: C,
    events: mpsc::Sender<Event>,
    mut shutdown: watch::Receiver<bool>,
    mut refresh: Option<mpsc::Receiver<oneshot::Sender<ClockReading>>>,
) -> Result<(), SamplerError> {
    config.validate()?;
    let specs = probe_specs(&config);
    let mut next_due = vec![Instant::now(); specs.len()];
    let mut running = vec![false; specs.len()];
    let mut tasks = JoinSet::new();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut generation = 0;
    let mut previous = clock.reading();
    let mut pending_completion = None;
    let result = 'sampling: loop {
        if *shutdown.borrow() {
            break Ok(());
        }
        enum Wake {
            Tick,
            Shutdown,
            Probe(Option<Result<(usize, Observation), tokio::task::JoinError>>),
            Refresh(Option<oneshot::Sender<ClockReading>>),
        }
        let due = next_due
            .iter()
            .zip(&running)
            .filter_map(|(due, running)| (!running).then_some(*due))
            .min();
        let wake = tokio::select! {
            biased;
            changed = shutdown.changed() => { let _ = changed; Wake::Shutdown },
            request = async {
                match refresh.as_mut() {
                    Some(receiver) => receiver.recv().await,
                    None => std::future::pending().await,
                }
            } => Wake::Refresh(request),
            _ = async {
                match due {
                    Some(due) => tokio::time::sleep_until(due).await,
                    None => std::future::pending().await,
                }
            } => Wake::Tick,
            _ = heartbeat.tick() => Wake::Tick,
            result = tasks.join_next(), if !tasks.is_empty() => Wake::Probe(result),
        };
        let refresh_reply = match wake {
            Wake::Shutdown => break Ok(()),
            Wake::Refresh(Some(reply)) => Some(reply),
            Wake::Refresh(None) => {
                refresh = None;
                None
            }
            Wake::Probe(Some(result)) => {
                match result {
                    Ok(completed) => pending_completion = Some(completed),
                    Err(error) if error.is_cancelled() => {}
                    Err(error) => break Err(SamplerError::Task(error.to_string())),
                }
                None
            }
            Wake::Tick | Wake::Probe(None) => None,
        };
        let now = clock.reading();
        let elapsed_delta = now.elapsed_ms.saturating_sub(previous.elapsed_ms);
        let wall_delta = now
            .utc
            .signed_duration_since(previous.utc)
            .num_milliseconds();
        let clock_delta =
            wall_delta.saturating_sub(i64::try_from(elapsed_delta).unwrap_or(i64::MAX));
        if clock_delta.unsigned_abs() > 2000
            && let Err(error) = emit(
                &events,
                Event::ClockAdjusted {
                    at: Stamp {
                        utc: now.utc,
                        elapsed_ms: now.elapsed_ms,
                        generation,
                    },
                    delta_ms: clock_delta,
                },
            )
        {
            break Err(error);
        }
        if clock_has_gap(&previous, &now, &config) {
            generation += 1;
            tasks.abort_all();
            if let Err(error) = emit(
                &events,
                Event::Gap(Gap {
                    at: Stamp {
                        utc: now.utc,
                        elapsed_ms: now.elapsed_ms,
                        generation,
                    },
                    from_elapsed_ms: previous.elapsed_ms,
                    reason: GapReason::SleepOrSchedulingDelay,
                }),
            ) {
                break Err(error);
            }
            while let Some(result) = tasks.join_next().await {
                if let Ok((_, observation)) = result
                    && let Err(error) = emit(&events, Event::Probe(observation))
                {
                    break 'sampling Err(error);
                }
            }
            running.fill(false);
            next_due.fill(Instant::now());
        }
        previous = now.clone();
        if let Some((index, observation)) = pending_completion.take() {
            if observation.at.generation == generation {
                running[index] = false;
            }
            if let Err(error) = emit(&events, Event::Probe(observation)) {
                break Err(error);
            }
        }
        if let Some(reply) = refresh_reply {
            // The app drains prior events before publishing this clock-checked snapshot.
            let _ = reply.send(now);
        }
        let now_instant = Instant::now();
        for (index, spec) in specs.iter().enumerate() {
            if !running[index] && next_due[index] <= now_instant {
                running[index] = true;
                next_due[index] = now_instant + Duration::from_millis(spec.interval_ms);
                let spec = spec.clone();
                let runner = runner.clone();
                let clock = clock.clone();
                tasks.spawn(async move {
                    let started = clock.reading();
                    let outcome = match tokio::time::timeout(
                        Duration::from_millis(spec.timeout_ms),
                        runner.probe(spec.clone()),
                    )
                    .await
                    {
                        Ok(outcome) => outcome,
                        Err(_) => ProbeOutcome::Timeout,
                    };
                    let ended = clock.reading();
                    let duration_ms = ended.elapsed_ms.saturating_sub(started.elapsed_ms);
                    let awake_duration_ms = ended
                        .awake_elapsed_ms
                        .saturating_sub(started.awake_elapsed_ms);
                    let outcome = if duration_ms.saturating_sub(awake_duration_ms)
                        > SUSPEND_CLOCK_TOLERANCE_MS
                    {
                        ProbeOutcome::Cancelled
                    } else if duration_ms > spec.timeout_ms
                        && !matches!(outcome, ProbeOutcome::Timeout)
                    {
                        ProbeOutcome::Unavailable(
                            "probe result was processed after its deadline".into(),
                        )
                    } else {
                        outcome
                    };
                    (
                        index,
                        Observation {
                            at: Stamp {
                                utc: ended.utc,
                                elapsed_ms: ended.elapsed_ms,
                                generation,
                            },
                            started_elapsed_ms: started.elapsed_ms,
                            duration_ms,
                            kind: spec.kind,
                            target: spec.target,
                            outcome,
                        },
                    )
                });
            }
        }
    };
    tasks.abort_all();
    let mut undelivered = Vec::new();
    let mut result = match result {
        Err(SamplerError::Overloaded(events)) => {
            undelivered = events;
            Ok(())
        }
        other => other,
    };
    if let Some((_, observation)) = pending_completion.take() {
        undelivered.push(Event::Probe(observation));
    }
    while let Some(completed) = tasks.join_next().await {
        if let Ok((_, observation)) = completed {
            if undelivered.is_empty() {
                match emit(&events, Event::Probe(observation)) {
                    Err(SamplerError::Overloaded(events)) => undelivered.extend(events),
                    Err(error) => result = Err(error),
                    Ok(()) => {}
                }
            } else {
                undelivered.push(Event::Probe(observation));
            }
        }
    }
    if undelivered.is_empty() {
        result
    } else {
        Err(SamplerError::Overloaded(undelivered))
    }
}
