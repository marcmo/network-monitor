use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub tcp_targets: Vec<String>,
    pub tcp_interval_ms: u64,
    pub tcp_timeout_ms: u64,
    pub dns_name: String,
    pub dns_interval_ms: u64,
    pub dns_timeout_ms: u64,
    pub https_url: String,
    pub https_interval_ms: u64,
    pub https_timeout_ms: u64,
    pub slow_latency_ms: u64,
    pub stale_after_ms: u64,
    pub clock_gap_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tcp_targets: vec!["1.1.1.1:443".into(), "8.8.8.8:443".into()],
            tcp_interval_ms: 2000,
            tcp_timeout_ms: 1500,
            dns_name: "example.com".into(),
            dns_interval_ms: 15000,
            dns_timeout_ms: 2000,
            https_url: "https://www.gstatic.com/generate_204".into(),
            https_interval_ms: 30000,
            https_timeout_ms: 3000,
            slow_latency_ms: 250,
            stale_after_ms: 5000,
            clock_gap_ms: 5000,
        }
    }
}

#[derive(Debug, Error)]
#[error("invalid configuration: {0}")]
pub struct ConfigError(pub String);

impl Config {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.tcp_targets.len() != 2 || self.tcp_targets[0] == self.tcp_targets[1] {
            return Err(ConfigError(
                "exactly two distinct TCP IP:port targets are required".into(),
            ));
        }
        let mut previous_ip = None;
        for target in &self.tcp_targets {
            let address = target
                .parse::<std::net::SocketAddr>()
                .map_err(|_| ConfigError(format!("TCP target must be an IP:port: {target}")))?;
            if previous_ip == Some(address.ip().to_canonical()) {
                return Err(ConfigError(
                    "TCP targets must use distinct IP addresses".into(),
                ));
            }
            previous_ip = Some(address.ip().to_canonical());
        }
        for (name, interval, deadline) in [
            ("TCP", self.tcp_interval_ms, self.tcp_timeout_ms),
            ("DNS", self.dns_interval_ms, self.dns_timeout_ms),
            ("HTTPS", self.https_interval_ms, self.https_timeout_ms),
        ] {
            if interval < 1000 || deadline == 0 || deadline > interval || interval > 3_600_000 {
                return Err(ConfigError(format!(
                    "{name} interval must be 1000..=3600000 ms and timeout in 1..=interval"
                )));
            }
        }
        if self.stale_after_ms < self.tcp_interval_ms + self.tcp_timeout_ms
            || self.stale_after_ms > 60_000
            || self.slow_latency_ms == 0
            || self.slow_latency_ms > self.tcp_timeout_ms
            || !(3000..=60_000).contains(&self.clock_gap_ms)
        {
            return Err(ConfigError("staleness must cover one TCP cycle (at most 60s), slow threshold must fit its timeout, and gap threshold must be 3..=60s".into()));
        }
        if self.dns_name.is_empty()
            || self.dns_name.len() > 253
            || self
                .dns_name
                .contains(|c: char| c.is_whitespace() || !c.is_ascii())
        {
            return Err(ConfigError(
                "DNS name must be a nonempty ASCII hostname".into(),
            ));
        }
        let url =
            reqwest::Url::parse(&self.https_url).map_err(|error| ConfigError(error.to_string()))?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(ConfigError(
                "HTTPS URL must use https and contain no credentials".into(),
            ));
        }
        Ok(())
    }
}
