use std::{future::Future, pin::Pin};

use serde::{Deserialize, Serialize};

pub const WIFI_INTERVAL_MS: u64 = 1000;
pub const WIFI_TIMEOUT_MS: u64 = 500;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WifiSnapshot {
    pub interfaces: Vec<WifiInterface>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WifiInterface {
    pub name: String,
    pub power_reported_on: bool,
    pub mode: WifiMode,
    pub link: WifiLink,
    pub ssid: Option<String>,
    pub bssid: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WifiMode {
    None,
    Station,
    Ibss,
    HostAp,
    Unknown(i64),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WifiLink {
    Active,
    Inactive,
    Unavailable { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WifiSourceError {
    #[error("Wi-Fi observation worker is busy")]
    Busy,
    #[error("Wi-Fi observation worker stopped")]
    Stopped,
    #[error("Wi-Fi observation worker disconnected")]
    Disconnected,
    #[error("Wi-Fi observation failed: {reason}")]
    Native { reason: String },
    #[error("Wi-Fi observations require macOS")]
    UnsupportedPlatform,
    #[error("invalid native Wi-Fi observation: {reason}")]
    InvalidData { reason: String },
}

pub trait WifiSource: Clone + Send + 'static {
    fn sample(&self)
    -> Pin<Box<dyn Future<Output = Result<WifiSnapshot, WifiSourceError>> + Send>>;
}

#[derive(Clone)]
pub struct NoWifi;

impl WifiSource for NoWifi {
    fn sample(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<WifiSnapshot, WifiSourceError>> + Send>> {
        Box::pin(std::future::pending())
    }
}

pub trait WifiReader: Send + 'static {
    fn read(&mut self) -> Result<WifiSnapshot, WifiSourceError>;
}

type WifiReply = tokio::sync::oneshot::Sender<Result<WifiSnapshot, WifiSourceError>>;

#[derive(Clone)]
pub struct WifiWorker {
    requests: tokio::sync::mpsc::Sender<WifiReply>,
}

impl WifiWorker {
    pub fn new(mut reader: impl WifiReader) -> Result<Self, std::io::Error> {
        let (requests, mut receiver) = tokio::sync::mpsc::channel::<WifiReply>(1);
        std::thread::Builder::new()
            .name("wifi-observations".into())
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

impl WifiSource for WifiWorker {
    fn sample(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<WifiSnapshot, WifiSourceError>> + Send>> {
        let requests = self.requests.clone();
        Box::pin(async move {
            let (reply, response) = tokio::sync::oneshot::channel();
            requests.try_send(reply).map_err(|error| match error {
                tokio::sync::mpsc::error::TrySendError::Full(_) => WifiSourceError::Busy,
                tokio::sync::mpsc::error::TrySendError::Closed(_) => WifiSourceError::Stopped,
            })?;
            response.await.map_err(|_| WifiSourceError::Disconnected)?
        })
    }
}

pub struct NativeWifi;

impl WifiReader for NativeWifi {
    fn read(&mut self) -> Result<WifiSnapshot, WifiSourceError> {
        native_read()
    }
}

#[cfg(target_os = "macos")]
fn native_read() -> Result<WifiSnapshot, WifiSourceError> {
    const MAX_INTERFACES: usize = 16;

    #[derive(Clone, Copy)]
    #[repr(C)]
    struct NativeInterface {
        mode: i64,
        power_reported_on: u32,
        link_state: i32,
        has_ssid: u32,
        ssid_length: u32,
        has_bssid: u32,
        bssid_length: u32,
        name: [u8; 16],
        ssid: [u8; 128],
        bssid: [u8; 18],
        link_error: [u8; 256],
    }

    #[repr(C)]
    struct NativeSnapshot {
        count: u32,
        interfaces: [NativeInterface; MAX_INTERFACES],
    }

    unsafe extern "C" {
        fn nm_read_wifi(out: *mut NativeSnapshot, error: *mut u8, error_size: usize) -> i32;
    }

    let mut raw = NativeSnapshot {
        count: 0,
        interfaces: [NativeInterface {
            mode: 0,
            power_reported_on: 0,
            link_state: 0,
            has_ssid: 0,
            ssid_length: 0,
            has_bssid: 0,
            bssid_length: 0,
            name: [0; 16],
            ssid: [0; 128],
            bssid: [0; 18],
            link_error: [0; 256],
        }; MAX_INTERFACES],
    };
    let mut error = [0; 256];
    // The native reader writes these fixed ABI buffers and retains no pointers.
    let status = unsafe { nm_read_wifi(&mut raw, error.as_mut_ptr(), error.len()) };
    if status == 1 {
        return Err(WifiSourceError::Native {
            reason: native_c_string(&error, "query error")?,
        });
    } else if status != 0 {
        return Err(invalid_data(format!("unexpected result code {status}")));
    }
    let count = raw.count as usize;
    if count > MAX_INTERFACES {
        return Err(invalid_data(format!(
            "interface count {count} exceeds {MAX_INTERFACES}"
        )));
    }
    let mut interfaces = Vec::with_capacity(count);
    for record in &raw.interfaces[..count] {
        let name = native_c_string(&record.name, "interface name")?;
        if name.is_empty() {
            return Err(invalid_data("interface name is empty"));
        }
        let power_reported_on = if record.power_reported_on == 0 {
            false
        } else if record.power_reported_on == 1 {
            true
        } else {
            return Err(invalid_data("power state is not a boolean"));
        };
        let mode = if record.mode == 0 {
            WifiMode::None
        } else if record.mode == 1 {
            WifiMode::Station
        } else if record.mode == 2 {
            WifiMode::Ibss
        } else if record.mode == 3 {
            WifiMode::HostAp
        } else {
            WifiMode::Unknown(record.mode)
        };
        let link = if record.link_state == -1 {
            WifiLink::Unavailable {
                reason: native_c_string(&record.link_error, "link error")?,
            }
        } else if record.link_state == 0 {
            WifiLink::Inactive
        } else if record.link_state == 1 {
            WifiLink::Active
        } else {
            return Err(invalid_data(format!(
                "unexpected link state {}",
                record.link_state
            )));
        };
        interfaces.push(WifiInterface {
            name,
            power_reported_on,
            mode,
            link,
            ssid: native_optional_string(
                &record.ssid,
                record.has_ssid,
                record.ssid_length,
                "SSID",
            )?,
            bssid: native_optional_string(
                &record.bssid,
                record.has_bssid,
                record.bssid_length,
                "BSSID",
            )?,
        });
    }
    interfaces.sort_unstable_by(|first, second| first.name.cmp(&second.name));
    Ok(WifiSnapshot { interfaces })
}

#[cfg(target_os = "macos")]
fn invalid_data(reason: impl Into<String>) -> WifiSourceError {
    WifiSourceError::InvalidData {
        reason: reason.into(),
    }
}

#[cfg(target_os = "macos")]
fn native_c_string(bytes: &[u8], field: &str) -> Result<String, WifiSourceError> {
    let length = bytes
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| invalid_data(format!("{field} has no terminator")))?;
    native_string(&bytes[..length], field)
}

#[cfg(target_os = "macos")]
fn native_string(bytes: &[u8], field: &str) -> Result<String, WifiSourceError> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|error| invalid_data(format!("{field} is not UTF-8: {error}")))
}

#[cfg(target_os = "macos")]
fn native_optional_string(
    bytes: &[u8],
    present: u32,
    length: u32,
    field: &str,
) -> Result<Option<String>, WifiSourceError> {
    if present == 0 && length == 0 {
        return Ok(None);
    }
    if present != 1 || length as usize > bytes.len() {
        return Err(invalid_data(format!("invalid {field} presence or length")));
    }
    native_string(&bytes[..length as usize], field).map(Some)
}

#[cfg(not(target_os = "macos"))]
fn native_read() -> Result<WifiSnapshot, WifiSourceError> {
    Err(WifiSourceError::UnsupportedPlatform)
}
