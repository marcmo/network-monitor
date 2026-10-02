use std::{future::Future, pin::Pin};

use crate::{model::*, scheduler::ClockReading};

pub trait TrafficSource: Clone + Send + 'static {
    fn sample(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<TrafficCounters, TrafficSourceError>> + Send>>;
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("{0}")]
pub struct TrafficSourceError(pub String);

#[derive(Clone)]
pub struct NoTraffic;
impl TrafficSource for NoTraffic {
    fn sample(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<TrafficCounters, TrafficSourceError>> + Send>> {
        Box::pin(std::future::pending())
    }
}

pub(crate) struct Sample {
    pub started: ClockReading,
    pub ended: ClockReading,
    pub generation: u64,
    pub counters: Result<TrafficCounters, TrafficSourceError>,
}

#[derive(Default)]
pub(crate) struct Rates {
    baseline: Option<(Stamp, TrafficCounters)>,
}
impl Rates {
    pub fn invalidate(&mut self) {
        self.baseline = None;
    }

    pub fn observe(&mut self, sample: Sample, now: &ClockReading, generation: u64) -> TrafficEvent {
        let at = Stamp {
            utc: sample.ended.utc,
            elapsed_ms: sample.ended.elapsed_ms,
            generation: sample.generation,
        };
        let previous = self.baseline.take();
        let duration = sample
            .ended
            .elapsed_ms
            .saturating_sub(sample.started.elapsed_ms);
        let awake_duration = sample
            .ended
            .awake_elapsed_ms
            .saturating_sub(sample.started.awake_elapsed_ms);
        let invalid =
            if sample.generation != generation || duration.saturating_sub(awake_duration) > 50 {
                Some(TrafficRate::Gap)
            } else if sample.ended.elapsed_ms < sample.started.elapsed_ms
                || now.elapsed_ms < sample.ended.elapsed_ms
            {
                Some(TrafficRate::OutOfOrder)
            } else if duration > TRAFFIC_TIMEOUT_MS
                || now.elapsed_ms - sample.ended.elapsed_ms > TRAFFIC_TIMEOUT_MS
            {
                Some(TrafficRate::Late)
            } else {
                None
            };
        let (counters, error) = match sample.counters {
            Ok(counters) => (Some(counters), None),
            Err(error) => (None, Some(error)),
        };
        let rate = if let Some(invalid) = invalid {
            invalid
        } else if let Some(counters) = &counters {
            let rate = match previous {
                None => TrafficRate::Baseline,
                Some((before, _)) if at.elapsed_ms <= before.elapsed_ms => TrafficRate::OutOfOrder,
                Some((_, old)) if counters.interface != old.interface => {
                    TrafficRate::InterfaceChanged
                }
                Some((_, old))
                    if counters.received_bytes < old.received_bytes
                        || counters.sent_bytes < old.sent_bytes =>
                {
                    TrafficRate::CounterReset
                }
                Some((before, _)) if at.elapsed_ms - before.elapsed_ms > TRAFFIC_STALE_AFTER_MS => {
                    TrafficRate::Gap
                }
                Some((before, old)) => {
                    let interval_ms = at.elapsed_ms - before.elapsed_ms;
                    TrafficRate::Valid {
                        download_mbps: (counters.received_bytes - old.received_bytes) as f64
                            * 0.008
                            / interval_ms as f64,
                        upload_mbps: (counters.sent_bytes - old.sent_bytes) as f64 * 0.008
                            / interval_ms as f64,
                        interval_ms,
                    }
                }
            };
            if !matches!(rate, TrafficRate::OutOfOrder) {
                self.baseline = Some((at.clone(), counters.clone()));
            }
            rate
        } else {
            TrafficRate::Unavailable {
                reason: error.map_or_else(
                    || "interface counters missing".into(),
                    |error| error.to_string(),
                ),
            }
        };
        TrafficEvent {
            at,
            started_elapsed_ms: sample.started.elapsed_ms,
            counters,
            rate,
        }
    }
}

pub trait CounterReader: Send + 'static {
    fn read(&mut self) -> Result<TrafficCounters, TrafficSourceError>;
}

type CounterReply = tokio::sync::oneshot::Sender<Result<TrafficCounters, TrafficSourceError>>;

#[derive(Clone)]
pub struct TrafficWorker {
    requests: tokio::sync::mpsc::Sender<CounterReply>,
}
impl TrafficWorker {
    pub fn new(mut reader: impl CounterReader) -> Result<Self, std::io::Error> {
        let (requests, mut receiver) = tokio::sync::mpsc::channel::<CounterReply>(1);
        std::thread::Builder::new()
            .name("interface-counters".into())
            .spawn(move || {
                while let Some(reply) = receiver.blocking_recv() {
                    if !reply.is_closed() {
                        let _ = reply.send(reader.read());
                    }
                }
            })?;
        Ok(Self { requests })
    }
}
impl TrafficSource for TrafficWorker {
    fn sample(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<TrafficCounters, TrafficSourceError>> + Send>> {
        let requests = self.requests.clone();
        Box::pin(async move {
            let (reply, response) = tokio::sync::oneshot::channel();
            requests.try_send(reply).map_err(|error| {
                TrafficSourceError(match error {
                    tokio::sync::mpsc::error::TrySendError::Full(_) => {
                        "interface counter worker is busy".into()
                    }
                    tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                        "interface counter worker stopped".into()
                    }
                })
            })?;
            response
                .await
                .map_err(|_| TrafficSourceError("interface counter worker disconnected".into()))?
        })
    }
}

pub struct NativeCounters;
impl CounterReader for NativeCounters {
    fn read(&mut self) -> Result<TrafficCounters, TrafficSourceError> {
        native_read()
    }
}

#[cfg(target_os = "macos")]
fn native_read() -> Result<TrafficCounters, TrafficSourceError> {
    #[repr(C)]
    struct Native {
        received_bytes: u64,
        sent_bytes: u64,
        change_id: u64,
        index: u32,
        name: [std::ffi::c_char; 16],
    }
    unsafe extern "C" {
        fn nm_read_traffic(
            out: *mut Native,
            error: *mut std::ffi::c_char,
            error_size: usize,
        ) -> i32;
    }
    let mut raw = Native {
        received_bytes: 0,
        sent_bytes: 0,
        change_id: 0,
        index: 0,
        name: [0; 16],
    };
    let mut error = [0; 256];
    // The native function writes these fixed ABI buffers and retains neither pointer.
    let status = unsafe { nm_read_traffic(&mut raw, error.as_mut_ptr(), error.len()) };
    if status != 0 {
        let bytes: Vec<_> = error
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect();
        return Err(TrafficSourceError(
            String::from_utf8_lossy(&bytes).into_owned(),
        ));
    }
    let bytes: Vec<_> = raw
        .name
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect();
    let name = String::from_utf8(bytes)
        .map_err(|_| TrafficSourceError("interface name is not UTF-8".into()))?;
    Ok(TrafficCounters {
        interface: TrafficInterface {
            name,
            index: raw.index,
            change_id: raw.change_id,
        },
        received_bytes: raw.received_bytes,
        sent_bytes: raw.sent_bytes,
    })
}

#[cfg(not(target_os = "macos"))]
fn native_read() -> Result<TrafficCounters, TrafficSourceError> {
    Err(TrafficSourceError(
        "passive interface counters require macOS".into(),
    ))
}
