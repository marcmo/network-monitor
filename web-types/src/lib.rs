use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Starting,
    Healthy,
    Slow,
    Partial,
    Offline,
    Stale,
}

impl Health {
    pub fn label(self) -> &'static str {
        match self {
            Self::Starting => "Starting",
            Self::Healthy => "Healthy",
            Self::Slow => "Slow",
            Self::Partial => "Partial",
            Self::Offline => "Offline",
            Self::Stale => "Unknown",
        }
    }
    pub fn css(self) -> &'static str {
        match self {
            Self::Starting => "unknown",
            Self::Healthy => "healthy",
            Self::Slow => "slow",
            Self::Partial => "partial",
            Self::Offline => "offline",
            Self::Stale => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Tcp,
    Dns,
    Https,
}
impl Protocol {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Dns => "DNS",
            Self::Https => "HTTPS",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Probe {
    pub protocol: Protocol,
    pub target: String,
    pub successful: Option<bool>,
    pub outcome: String,
    pub latency_ms: Option<u64>,
    pub age_ms: Option<u64>,
    pub fresh: bool,
    pub interval_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HistoryPoint {
    pub elapsed_second: u64,
    pub latency_ms: Option<u64>,
    pub successes: u32,
    pub failures: u32,
    pub explicit_gap: bool,
    pub download_mbps: Option<f64>,
    pub upload_mbps: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Traffic {
    pub interface: Option<String>,
    pub download_mbps: Option<f64>,
    pub upload_mbps: Option<f64>,
    pub age_ms: Option<u64>,
    pub fresh: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Fix {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy_m: f64,
    pub source_utc: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Location {
    pub fix: Option<Fix>,
    pub source_age_ms: Option<i64>,
    pub state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LiveSnapshot {
    pub session_id: String,
    pub label: Option<String>,
    pub utc: String,
    pub elapsed_ms: u64,
    pub status: Health,
    pub probes: Vec<Probe>,
    pub history: Vec<HistoryPoint>,
    pub traffic: Traffic,
    pub location: Location,
    pub slow_latency_ms: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecordingState {
    Current,
    Complete,
    Unfinished,
}
impl RecordingState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Current => "Recording now",
            Self::Complete => "Complete",
            Self::Unfinished => "Unfinished",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RecordingSummary {
    pub id: String,
    pub label: Option<String>,
    pub started_utc: String,
    pub ended_utc: Option<String>,
    pub state: RecordingState,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RecordingPoint {
    pub elapsed_ms: u64,
    pub utc: String,
    pub status: Health,
    pub latency_ms: Option<u64>,
    pub download_mbps: Option<f64>,
    pub upload_mbps: Option<f64>,
    pub location: Option<Fix>,
    pub break_before: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RecordingDetail {
    pub session: RecordingSummary,
    pub duration_ms: u64,
    pub points: Vec<RecordingPoint>,
    pub healthy_percent: Option<f64>,
    pub median_latency_ms: Option<u64>,
    pub event_count: u64,
    pub truncated: bool,
    pub location_max_age_ms: u64,
    pub location_max_accuracy_m: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub message: String,
}
