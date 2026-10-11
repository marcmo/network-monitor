use std::path::Path;

use chrono::{DateTime, Utc};
use network_monitor_web_types::{
    Fix, Health, HistoryPoint, LiveSnapshot, Location, Probe, Protocol, RecordingDetail,
    RecordingPoint, RecordingState, RecordingSummary, Traffic,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::{
    config::Config,
    model::{
        Event, LocationEvent, LocationState, Monitor, ProbeKind, ProbeOutcome, Snapshot, Status,
        TrafficRate,
    },
    storage::{self, Session, StorageError},
    ui::View,
};

const MAX_POINTS: usize = 10_000;
const LOCATION_MAX_AGE_MS: u64 = 30_000;
const LOCATION_MAX_ACCURACY_M: f64 = 1000.0;

pub(super) fn live(view: &View) -> LiveSnapshot {
    LiveSnapshot {
        session_id: view.session_id.clone(),
        label: view.label.clone(),
        utc: view.utc.to_rfc3339(),
        elapsed_ms: view.elapsed_ms,
        status: health(view.snapshot.status),
        probes: view
            .snapshot
            .probes
            .iter()
            .map(|probe| {
                let (protocol, interval_ms) = match probe.kind {
                    ProbeKind::Tcp => (Protocol::Tcp, view.config.tcp_interval_ms),
                    ProbeKind::Dns => (Protocol::Dns, view.config.dns_interval_ms),
                    ProbeKind::Https => (Protocol::Https, view.config.https_interval_ms),
                };
                Probe {
                    protocol,
                    target: probe.target.clone(),
                    successful: probe
                        .observation
                        .as_ref()
                        .filter(|observation| observation.outcome.is_measurement())
                        .map(|observation| observation.outcome.is_success()),
                    outcome: probe.observation.as_ref().map_or_else(
                        || "Pending".into(),
                        |observation| outcome(&observation.outcome),
                    ),
                    latency_ms: probe
                        .observation
                        .as_ref()
                        .filter(|observation| observation.outcome.is_success())
                        .map(|observation| observation.duration_ms),
                    age_ms: probe.age_ms,
                    fresh: probe.fresh,
                    interval_ms,
                }
            })
            .collect(),
        history: view
            .snapshot
            .history
            .iter()
            .map(|point| HistoryPoint {
                elapsed_second: point.elapsed_second,
                latency_ms: point.latency_ms,
                successes: point.successes,
                failures: point.failures,
                explicit_gap: point.explicit_gap,
                download_mbps: point.download_mbps,
                upload_mbps: point.upload_mbps,
            })
            .collect(),
        traffic: traffic(&view.snapshot.traffic),
        location: location(
            view.snapshot.location.as_ref(),
            view.utc,
            view.snapshot.generation,
        ),
        slow_latency_ms: view.config.slow_latency_ms,
    }
}

fn outcome(outcome: &ProbeOutcome) -> String {
    match outcome {
        ProbeOutcome::Success => "Success".into(),
        ProbeOutcome::Timeout => "Timeout".into(),
        ProbeOutcome::Refused => "Refused".into(),
        ProbeOutcome::NetworkError(reason) => format!("Network error: {reason}"),
        ProbeOutcome::DnsError(reason) => format!("DNS error: {reason}"),
        ProbeOutcome::HttpStatus(status) => format!("HTTP {status}"),
        ProbeOutcome::TlsOrHttpError(reason) => format!("HTTPS error: {reason}"),
        ProbeOutcome::Unavailable(reason) => format!("Unavailable: {reason}"),
        ProbeOutcome::Cancelled => "Cancelled".into(),
    }
}

fn traffic(traffic: &crate::model::TrafficState) -> Traffic {
    let rate = traffic
        .observation
        .as_ref()
        .and_then(|observation| match observation.rate {
            TrafficRate::Valid {
                download_mbps,
                upload_mbps,
                ..
            } if traffic.fresh => Some((download_mbps, upload_mbps)),
            TrafficRate::Valid { .. }
            | TrafficRate::Baseline
            | TrafficRate::InterfaceChanged
            | TrafficRate::CounterReset
            | TrafficRate::Gap
            | TrafficRate::Late
            | TrafficRate::OutOfOrder
            | TrafficRate::Unavailable { .. } => None,
        });
    Traffic {
        interface: traffic
            .interface
            .as_ref()
            .map(|interface| interface.name.clone()),
        download_mbps: rate.map(|(download, _upload)| download),
        upload_mbps: rate.map(|(_download, upload)| upload),
        age_ms: traffic.age_ms,
        fresh: traffic.fresh,
    }
}

fn health(status: Status) -> Health {
    match status {
        Status::Starting => Health::Starting,
        Status::Healthy => Health::Healthy,
        Status::Slow => Health::Slow,
        Status::Partial => Health::Partial,
        Status::Offline => Health::Offline,
        Status::Stale => Health::Stale,
    }
}

fn summary(session: &Session, current: Option<&str>) -> RecordingSummary {
    RecordingSummary {
        id: session.id.clone(),
        label: session.label.clone(),
        started_utc: session.started_utc.to_rfc3339(),
        ended_utc: session.ended_utc.map(|ended| ended.to_rfc3339()),
        state: if current == Some(session.id.as_str()) {
            RecordingState::Current
        } else if session.ended_utc.is_some() {
            RecordingState::Complete
        } else {
            RecordingState::Unfinished
        },
    }
}

pub(super) fn recordings(
    path: &Path,
    current: Option<&str>,
) -> Result<Vec<RecordingSummary>, StorageError> {
    Ok(storage::read_sessions(path)?
        .iter()
        .rev()
        .map(|session| summary(session, current))
        .collect())
}

fn read_session(connection: &Connection, id: &str) -> Result<Option<Session>, StorageError> {
    let row = connection
        .query_row(
            "SELECT label,started_utc,ended_utc,app_version,config_json FROM sessions WHERE id=?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    row.map(|(label, started, ended, app_version, config_json)| {
        Ok(Session {
            id: id.to_owned(),
            label,
            started_utc: started.parse()?,
            ended_utc: ended.map(|value| value.parse()).transpose()?,
            app_version,
            config_json,
        })
    })
    .transpose()
}

pub(super) fn recording(
    path: &Path,
    id: &str,
    current: Option<&str>,
) -> Result<Option<RecordingDetail>, StorageError> {
    let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(std::time::Duration::from_secs(2))?;
    let transaction = connection.transaction()?;
    let Some(session) = read_session(&transaction, id)? else {
        return Ok(None);
    };
    let config: Config = serde_json::from_str(&session.config_json)?;
    let (event_count, duration_ms): (i64, i64) = transaction.query_row(
        "SELECT COUNT(*),COALESCE(MAX(elapsed_ms),0) FROM events WHERE session_id=?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let event_count = u64::try_from(event_count).map_err(|_| StorageError::IntegerRange)?;
    let duration_ms = u64::try_from(duration_ms).map_err(|_| StorageError::IntegerRange)?;
    let mut replay = Replay::new(config, session.started_utc, duration_ms);
    let mut statement = transaction
        .prepare("SELECT payload_json FROM events WHERE session_id=?1 ORDER BY elapsed_ms,id")?;
    let mut rows = statement.query([id])?;
    while let Some(row) = rows.next()? {
        let payload: String = row.get(0)?;
        replay.apply(serde_json::from_str(&payload)?);
    }
    replay.finish();
    let recognized = replay
        .points
        .iter()
        .filter(|point| {
            matches!(
                point.status,
                Health::Healthy | Health::Slow | Health::Partial | Health::Offline
            )
        })
        .count();
    let healthy = replay
        .points
        .iter()
        .filter(|point| point.status == Health::Healthy)
        .count();
    let mut latencies: Vec<u64> = replay
        .points
        .iter()
        .filter_map(|point| point.latency_ms)
        .collect();
    latencies.sort_unstable();
    let median_latency_ms = latencies
        .get(latencies.len().saturating_sub(1) / 2)
        .zip(latencies.get(latencies.len() / 2))
        .map(|(lower, upper)| lower + (upper - lower) / 2);
    Ok(Some(RecordingDetail {
        session: summary(&session, current),
        duration_ms,
        points: replay.points,
        healthy_percent: (recognized > 0).then(|| healthy as f64 * 100.0 / recognized as f64),
        median_latency_ms,
        event_count,
        truncated: replay.truncated,
        location_max_age_ms: LOCATION_MAX_AGE_MS,
        location_max_accuracy_m: LOCATION_MAX_ACCURACY_M,
    }))
}

struct Replay {
    monitor: Monitor,
    at: (u64, DateTime<Utc>),
    last_location: Option<LocationEvent>,
    generation: u64,
    duration_ms: u64,
    next_sample: u64,
    step_ms: u64,
    has_events: bool,
    break_pending: bool,
    points: Vec<RecordingPoint>,
    truncated: bool,
}

impl Replay {
    fn new(config: Config, started: DateTime<Utc>, duration_ms: u64) -> Self {
        let step_ms = duration_ms.div_ceil((MAX_POINTS - 1) as u64).max(1000);
        Self {
            monitor: Monitor::new(config),
            at: (0, started),
            last_location: None,
            generation: 0,
            duration_ms,
            next_sample: 0,
            step_ms,
            has_events: false,
            break_pending: true,
            points: Vec::new(),
            truncated: step_ms > 1000,
        }
    }

    fn apply(&mut self, event: Event) {
        let stamp = event.stamp();
        let gap_from = match &event {
            Event::Gap(gap) => Some(gap.from_elapsed_ms),
            Event::Probe(_)
            | Event::Traffic(_)
            | Event::Wifi(_)
            | Event::Location(_)
            | Event::ClockAdjusted { .. } => None,
        };
        while self.next_sample < stamp.elapsed_ms {
            let in_gap = gap_from.is_some_and(|from| self.next_sample >= from);
            self.push(self.next_sample, self.utc_at(self.next_sample), in_gap);
            self.next_sample = self.next_sample.saturating_add(self.step_ms);
        }
        if let Some(previous) = self.last_location.as_ref()
            && location(Some(previous), stamp.utc, self.generation)
                .fix
                .is_none()
        {
            self.break_pending = true;
        }
        match &event {
            Event::Location(value) => {
                if location(Some(value), stamp.utc, self.generation)
                    .fix
                    .is_none()
                {
                    self.break_pending = true;
                }
                self.last_location = Some(value.clone());
            }
            Event::Gap(gap) => {
                // A clock adjustment can arrive before the gap for the same timestamp.
                for point in self
                    .points
                    .iter_mut()
                    .rev()
                    .take_while(|point| point.elapsed_ms >= gap.from_elapsed_ms)
                {
                    point.status = Health::Stale;
                    point.latency_ms = None;
                    point.download_mbps = None;
                    point.upload_mbps = None;
                    point.location = None;
                    point.break_before = true;
                }
                self.break_pending = true;
                self.generation = gap.at.generation;
            }
            Event::Probe(_) | Event::Traffic(_) | Event::Wifi(_) | Event::ClockAdjusted { .. } => {}
        }
        self.at = (stamp.elapsed_ms, stamp.utc);
        self.has_events = true;
        self.monitor.apply(event);
    }

    fn finish(&mut self) {
        if !self.has_events {
            return;
        }
        while self.next_sample <= self.duration_ms {
            self.push(self.next_sample, self.utc_at(self.next_sample), false);
            self.next_sample = self.next_sample.saturating_add(self.step_ms);
        }
        if self
            .points
            .last()
            .is_some_and(|point| point.elapsed_ms < self.duration_ms)
        {
            self.push(self.duration_ms, self.utc_at(self.duration_ms), false);
        }
    }

    fn utc_at(&self, elapsed_ms: u64) -> DateTime<Utc> {
        self.at.1 + chrono::Duration::milliseconds(elapsed_ms.saturating_sub(self.at.0) as i64)
    }

    fn push(&mut self, elapsed_ms: u64, utc: DateTime<Utc>, in_gap: bool) {
        if self.points.len() == MAX_POINTS {
            self.truncated = true;
            return;
        }
        let snapshot = self.monitor.snapshot(elapsed_ms);
        let traffic = traffic(&snapshot.traffic);
        let fix = if in_gap {
            None
        } else {
            location(snapshot.location.as_ref(), utc, snapshot.generation).fix
        };
        let break_before = self.break_pending || fix.is_none();
        self.break_pending = fix.is_none();
        self.points.push(RecordingPoint {
            elapsed_ms,
            utc: utc.to_rfc3339(),
            status: if in_gap {
                Health::Stale
            } else {
                health(snapshot.status)
            },
            latency_ms: if in_gap { None } else { latency(&snapshot) },
            download_mbps: if in_gap { None } else { traffic.download_mbps },
            upload_mbps: if in_gap { None } else { traffic.upload_mbps },
            location: fix,
            break_before,
        });
    }
}

fn latency(snapshot: &Snapshot) -> Option<u64> {
    snapshot
        .probes
        .iter()
        .filter(|probe| probe.kind == ProbeKind::Tcp && probe.fresh)
        .filter_map(|probe| probe.observation.as_ref())
        .filter(|observation| observation.outcome.is_success())
        .map(|observation| observation.duration_ms)
        .max()
}

fn location(event: Option<&LocationEvent>, utc: DateTime<Utc>, generation: u64) -> Location {
    let Some(event) = event else {
        return Location {
            state: "Waiting for location".into(),
            ..Location::default()
        };
    };
    match &event.state {
        LocationState::Fix {
            latitude,
            longitude,
            horizontal_accuracy_m,
            source_utc,
        } => {
            let source_age_ms = utc.signed_duration_since(*source_utc).num_milliseconds();
            let valid_coordinates = latitude.is_finite()
                && longitude.is_finite()
                && (-90.0..=90.0).contains(latitude)
                && (-180.0..=180.0).contains(longitude);
            let accurate = horizontal_accuracy_m.is_finite()
                && (0.0..=LOCATION_MAX_ACCURACY_M).contains(horizontal_accuracy_m);
            let fresh = (0..=LOCATION_MAX_AGE_MS as i64).contains(&source_age_ms)
                && event.at.generation == generation;
            Location {
                fix: (valid_coordinates && accurate && fresh).then(|| Fix {
                    latitude: *latitude,
                    longitude: *longitude,
                    accuracy_m: *horizontal_accuracy_m,
                    source_utc: source_utc.to_rfc3339(),
                }),
                source_age_ms: Some(source_age_ms),
                state: if !valid_coordinates {
                    "Invalid coordinates"
                } else if !accurate {
                    "Low accuracy"
                } else if !fresh {
                    "Location outdated"
                } else {
                    "Location available"
                }
                .into(),
            }
        }
        LocationState::Unavailable { reason } => Location {
            state: format!("Unavailable: {reason}"),
            ..Location::default()
        },
        LocationState::Denied => Location {
            state: "Location permission denied".into(),
            ..Location::default()
        },
        LocationState::Pending => Location {
            state: "Waiting for location".into(),
            ..Location::default()
        },
    }
}
