use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Stamp {
    pub utc: DateTime<Utc>,
    pub elapsed_ms: u64,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeKind {
    Tcp,
    Dns,
    Https,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "detail", rename_all = "snake_case")]
pub enum ProbeOutcome {
    Success,
    Timeout,
    Refused,
    NetworkError(String),
    DnsError(String),
    HttpStatus(u16),
    TlsOrHttpError(String),
    Unavailable(String),
    Cancelled,
}

impl ProbeOutcome {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }
    pub fn is_measurement(&self) -> bool {
        !matches!(self, Self::Unavailable(_) | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Observation {
    pub at: Stamp,
    pub started_elapsed_ms: u64,
    pub duration_ms: u64,
    pub kind: ProbeKind,
    pub target: String,
    pub outcome: ProbeOutcome,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GapReason {
    SleepOrSchedulingDelay,
    Shutdown,
    Overload,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Gap {
    pub at: Stamp,
    pub from_elapsed_ms: u64,
    pub reason: GapReason,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocationState {
    Fix {
        latitude: f64,
        longitude: f64,
        horizontal_accuracy_m: f64,
        source_utc: DateTime<Utc>,
    },
    Unavailable {
        reason: String,
    },
    Denied,
    Pending,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LocationEvent {
    pub at: Stamp,
    pub state: LocationState,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum Event {
    Probe(Observation),
    Gap(Gap),
    Location(LocationEvent),
    ClockAdjusted { at: Stamp, delta_ms: i64 },
}

impl Event {
    pub fn stamp(&self) -> &Stamp {
        match self {
            Self::Probe(v) => &v.at,
            Self::Gap(v) => &v.at,
            Self::Location(v) => &v.at,
            Self::ClockAdjusted { at, .. } => at,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Starting,
    Healthy,
    Slow,
    Partial,
    Offline,
    Stale,
}

#[derive(Clone, Debug)]
pub struct ProbeState {
    pub kind: ProbeKind,
    pub target: String,
    pub observation: Option<Observation>,
    pub age_ms: Option<u64>,
    pub fresh: bool,
}

#[derive(Clone, Debug, Default)]
pub struct GraphPoint {
    pub elapsed_second: u64,
    pub latency_ms: Option<u64>,
    pub successes: u32,
    pub failures: u32,
    pub explicit_gap: bool,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub status: Status,
    pub probes: Vec<ProbeState>,
    pub history: Vec<GraphPoint>,
    pub location: Option<LocationEvent>,
    pub generation: u64,
}

pub struct Monitor {
    config: Config,
    probes: Vec<ProbeState>,
    history: VecDeque<GraphPoint>,
    generation: u64,
    location: Option<LocationEvent>,
    has_observations: bool,
}

impl Monitor {
    pub fn new(config: Config) -> Self {
        let probes = config
            .tcp_targets
            .iter()
            .map(|target| (ProbeKind::Tcp, target.clone()))
            .chain([
                (ProbeKind::Dns, config.dns_name.clone()),
                (ProbeKind::Https, config.https_url.clone()),
            ])
            .map(|(kind, target)| ProbeState {
                kind,
                target,
                observation: None,
                age_ms: None,
                fresh: false,
            })
            .collect();
        Self {
            config,
            probes,
            history: VecDeque::with_capacity(301),
            generation: 0,
            location: None,
            has_observations: false,
        }
    }

    pub fn apply(&mut self, event: Event) {
        if event.stamp().generation < self.generation {
            return;
        }
        match event {
            Event::Probe(observation) => {
                if observation.at.generation != self.generation
                    || matches!(observation.outcome, ProbeOutcome::Cancelled)
                {
                    return;
                }
                let second = observation.at.elapsed_ms / 1000;
                self.advance(second);
                if observation.kind == ProbeKind::Tcp && observation.outcome.is_measurement() {
                    if let Some(point) = self
                        .history
                        .iter_mut()
                        .find(|point| point.elapsed_second == second)
                    {
                        if observation.outcome.is_success() {
                            point.successes += 1;
                            point.latency_ms =
                                Some(point.latency_ms.map_or(observation.duration_ms, |old| {
                                    old.max(observation.duration_ms)
                                }));
                        } else {
                            point.failures += 1;
                        }
                    }
                }
                if let Some(probe) = self.probes.iter_mut().find(|probe| {
                    probe.kind == observation.kind && probe.target == observation.target
                }) {
                    if probe
                        .observation
                        .as_ref()
                        .is_none_or(|old| observation.at.elapsed_ms >= old.at.elapsed_ms)
                    {
                        probe.observation = Some(observation);
                        self.has_observations = true;
                    }
                }
            }
            Event::Gap(gap) => {
                self.generation = gap.at.generation;
                for probe in &mut self.probes {
                    probe.observation = None;
                }
                self.advance(gap.at.elapsed_ms / 1000);
                for point in &mut self.history {
                    if point.elapsed_second >= gap.from_elapsed_ms / 1000 {
                        point.explicit_gap = true;
                    }
                }
            }
            Event::Location(location) => self.location = Some(location),
            Event::ClockAdjusted { .. } => {}
        }
    }

    fn advance(&mut self, now_second: u64) {
        let earliest = now_second.saturating_sub(299);
        while self
            .history
            .front()
            .is_some_and(|point| point.elapsed_second < earliest)
        {
            self.history.pop_front();
        }
        let start = self.history.back().map_or(earliest, |point| {
            point.elapsed_second.saturating_add(1).max(earliest)
        });
        for elapsed_second in start..=now_second {
            self.history.push_back(GraphPoint {
                elapsed_second,
                ..GraphPoint::default()
            });
        }
    }

    pub fn snapshot(&mut self, now_ms: u64) -> Snapshot {
        self.advance(now_ms / 1000);
        for probe in &mut self.probes {
            probe.age_ms = probe
                .observation
                .as_ref()
                .map(|value| now_ms.saturating_sub(value.at.elapsed_ms));
            let stale_after = match probe.kind {
                ProbeKind::Tcp => self.config.stale_after_ms,
                ProbeKind::Dns => self.config.dns_interval_ms + self.config.dns_timeout_ms + 1000,
                ProbeKind::Https => {
                    self.config.https_interval_ms + self.config.https_timeout_ms + 1000
                }
            };
            probe.fresh = probe.age_ms.is_some_and(|age| age <= stale_after)
                && probe
                    .observation
                    .as_ref()
                    .is_some_and(|value| value.outcome.is_measurement());
        }
        let tcp: Vec<_> = self
            .probes
            .iter()
            .filter(|probe| probe.kind == ProbeKind::Tcp)
            .collect();
        let fresh_count = tcp.iter().filter(|probe| probe.fresh).count();
        let succeeded = tcp
            .iter()
            .filter(|probe| {
                probe.fresh
                    && probe
                        .observation
                        .as_ref()
                        .is_some_and(|value| value.outcome.is_success())
            })
            .count();
        let latest_tcp_start = tcp
            .iter()
            .filter_map(|probe| {
                probe
                    .observation
                    .as_ref()
                    .map(|value| value.started_elapsed_ms)
            })
            .max();
        let contemporaneous_https = self.probes.iter().any(|probe| {
            probe.kind == ProbeKind::Https
                && probe.fresh
                && probe.observation.as_ref().is_some_and(|value| {
                    value.outcome.is_success()
                        && latest_tcp_start.is_some_and(|started| value.at.elapsed_ms >= started)
                })
        });
        let status = if fresh_count < 2 {
            if self.has_observations {
                Status::Stale
            } else {
                Status::Starting
            }
        } else if succeeded == 0 {
            if contemporaneous_https {
                Status::Partial
            } else {
                Status::Offline
            }
        } else if succeeded == 1
            || self.probes.iter().any(|probe| {
                !probe.fresh
                    || probe
                        .observation
                        .as_ref()
                        .is_some_and(|value| !value.outcome.is_success())
            })
        {
            Status::Partial
        } else if tcp.iter().any(|probe| {
            probe
                .observation
                .as_ref()
                .is_some_and(|value| value.duration_ms >= self.config.slow_latency_ms)
        }) {
            Status::Slow
        } else {
            Status::Healthy
        };
        Snapshot {
            status,
            probes: self.probes.clone(),
            history: self.history.iter().cloned().collect(),
            location: self.location.clone(),
            generation: self.generation,
        }
    }
}
