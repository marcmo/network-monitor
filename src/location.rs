use chrono::{DateTime, Utc};

use crate::model::LocationState;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LocationError {
    #[error("macOS location capture must run on the process main thread")]
    WrongThread,
    #[error(
        "location ingress is full; location capture stopped rather than silently dropping fixes"
    )]
    Backpressure,
    #[error("location event receiver closed")]
    ReceiverClosed,
    #[error("CoreLocation run-loop setup failed")]
    RunLoopSetup,
    #[error("CoreLocation bridge failed: {0}")]
    NativeFailure(i32),
    #[error("CoreLocation capture requires macOS")]
    UnsupportedPlatform,
}

pub struct LocationIngress {
    sender: tokio::sync::mpsc::Sender<RawLocationEvent>,
}

pub fn run_main(
    sender: tokio::sync::mpsc::Sender<RawLocationEvent>,
    stop: std::os::unix::net::UnixStream,
) -> Result<(), LocationError> {
    #[cfg(target_os = "macos")]
    {
        macos::run(sender, stop)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (sender, stop);
        Err(LocationError::UnsupportedPlatform)
    }
}

impl LocationIngress {
    pub fn new(sender: tokio::sync::mpsc::Sender<RawLocationEvent>) -> Self {
        Self { sender }
    }

    pub fn record(
        &self,
        native: NativeLocationEvent,
        received_utc: DateTime<Utc>,
    ) -> Result<(), LocationError> {
        self.sender
            .try_send(native.into_raw(received_utc))
            .map_err(|error| match error {
                tokio::sync::mpsc::error::TrySendError::Full(_) => LocationError::Backpressure,
                tokio::sync::mpsc::error::TrySendError::Closed(_) => LocationError::ReceiverClosed,
            })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawLocationEvent {
    pub received_utc: DateTime<Utc>,
    pub state: LocationState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NativeLocationEvent {
    Pending,
    Denied,
    Unavailable {
        reason: String,
    },
    Fix {
        latitude: f64,
        longitude: f64,
        horizontal_accuracy_m: f64,
        source_unix_seconds: f64,
    },
}

impl NativeLocationEvent {
    pub fn into_raw(self, received_utc: DateTime<Utc>) -> RawLocationEvent {
        RawLocationEvent {
            received_utc,
            state: match self {
                Self::Denied => LocationState::Denied,
                Self::Pending => LocationState::Pending,
                Self::Unavailable { reason } => LocationState::Unavailable { reason },
                Self::Fix {
                    latitude,
                    longitude,
                    horizontal_accuracy_m,
                    source_unix_seconds,
                } if valid_coordinates(latitude, longitude, horizontal_accuracy_m) => {
                    match source_time(source_unix_seconds) {
                        Some(source_utc) => LocationState::Fix {
                            latitude,
                            longitude,
                            horizontal_accuracy_m,
                            source_utc,
                        },
                        None => LocationState::Unavailable {
                            reason: "CoreLocation returned an invalid source timestamp".into(),
                        },
                    }
                }
                Self::Fix { .. } => LocationState::Unavailable {
                    reason: "CoreLocation returned invalid coordinates or horizontal accuracy"
                        .into(),
                },
            },
        }
    }
}

fn valid_coordinates(latitude: f64, longitude: f64, accuracy: f64) -> bool {
    latitude.is_finite()
        && longitude.is_finite()
        && accuracy.is_finite()
        && (-90.0..=90.0).contains(&latitude)
        && (-180.0..=180.0).contains(&longitude)
        && accuracy >= 0.0
}

fn source_time(seconds: f64) -> Option<DateTime<Utc>> {
    if !seconds.is_finite() {
        return None;
    }
    let whole_seconds = seconds.floor();
    let nanos = ((seconds - whole_seconds) * 1_000_000_000.0).round();
    if nanos >= 1_000_000_000.0 {
        DateTime::from_timestamp((whole_seconds as i64).checked_add(1)?, 0)
    } else {
        DateTime::from_timestamp(whole_seconds as i64, nanos as u32)
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::{
        ffi::{CStr, c_char, c_int, c_void},
        os::{fd::AsRawFd, unix::net::UnixStream},
    };

    use super::*;

    type Callback =
        unsafe extern "C" fn(*mut c_void, c_int, f64, f64, f64, f64, *const c_char) -> c_int;

    unsafe extern "C" {
        fn nm_location_run(callback: Callback, context: *mut c_void, stop_fd: c_int) -> c_int;
    }

    struct CallbackContext {
        ingress: LocationIngress,
        failure: Option<LocationError>,
    }

    unsafe extern "C" fn receive(
        context: *mut c_void,
        kind: c_int,
        latitude: f64,
        longitude: f64,
        horizontal_accuracy_m: f64,
        source_unix_seconds: f64,
        message: *const c_char,
    ) -> c_int {
        // The native delegate only calls synchronously on this main thread and is
        // disconnected before nm_location_run releases its borrowed stack context.
        let context = unsafe { &mut *context.cast::<CallbackContext>() };
        let event = match kind {
            0 => NativeLocationEvent::Pending,
            1 => NativeLocationEvent::Denied,
            2 => NativeLocationEvent::Unavailable {
                reason: if message.is_null() {
                    "CoreLocation did not provide an error description".into()
                } else {
                    // The native UTF-8 message remains alive for the callback.
                    unsafe { CStr::from_ptr(message) }
                        .to_string_lossy()
                        .into_owned()
                },
            },
            3 => NativeLocationEvent::Fix {
                latitude,
                longitude,
                horizontal_accuracy_m,
                source_unix_seconds,
            },
            other => NativeLocationEvent::Unavailable {
                reason: format!("CoreLocation returned an unknown event kind: {other}"),
            },
        };
        match context.ingress.record(event, Utc::now()) {
            Ok(()) => 1,
            Err(error) => {
                context.failure = Some(error);
                0
            }
        }
    }

    pub(super) fn run(
        sender: tokio::sync::mpsc::Sender<RawLocationEvent>,
        stop: UnixStream,
    ) -> Result<(), LocationError> {
        let mut context = CallbackContext {
            ingress: LocationIngress::new(sender),
            failure: None,
        };
        // The fd and callback context remain owned by this frame for the entire
        // native run loop; native teardown disconnects both before returning.
        let status = unsafe {
            nm_location_run(
                receive,
                (&mut context as *mut CallbackContext).cast(),
                stop.as_raw_fd(),
            )
        };
        if let Some(error) = context.failure {
            return match error {
                LocationError::ReceiverClosed => Ok(()),
                error => Err(error),
            };
        }
        match status {
            0 => Ok(()),
            1 => Err(LocationError::WrongThread),
            2 => Err(LocationError::RunLoopSetup),
            other => Err(LocationError::NativeFailure(other)),
        }
    }
}
