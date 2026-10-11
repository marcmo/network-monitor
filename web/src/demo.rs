use network_monitor_web_types::*;

pub fn sessions() -> Vec<RecordingSummary> {
    vec![
        RecordingSummary {
            id: "demo-current".into(),
            label: Some("Berlin to Leipzig".into()),
            started_utc: "2026-10-10T07:12:00Z".into(),
            ended_utc: None,
            state: RecordingState::Current,
        },
        RecordingSummary {
            id: "demo-past".into(),
            label: Some("Leipzig to Berlin".into()),
            started_utc: "2026-10-08T15:05:00Z".into(),
            ended_utc: Some("2026-10-08T16:36:00Z".into()),
            state: RecordingState::Complete,
        },
        RecordingSummary {
            id: "demo-no-location".into(),
            label: Some("Office - no location".into()),
            started_utc: "2026-10-07T08:00:00Z".into(),
            ended_utc: None,
            state: RecordingState::Unfinished,
        },
    ]
}

pub fn live() -> LiveSnapshot {
    LiveSnapshot {
        session_id: "demo-current".into(),
        label: Some("Berlin to Leipzig".into()),
        utc: "2026-10-10T08:36:00Z".into(),
        elapsed_ms: 5_040_000,
        status: Health::Healthy,
        slow_latency_ms: 250,
        probes: vec![
            probe(Protocol::Tcp, "1.1.1.1:443", 43, 1_000, 2_000),
            probe(Protocol::Tcp, "8.8.8.8:443", 38, 1_000, 2_000),
            probe(Protocol::Dns, "example.com", 18, 6_000, 15_000),
            probe(
                Protocol::Https,
                "https://www.gstatic.com/generate_204",
                64,
                12_000,
                30_000,
            ),
        ],
        history: (4740..=5040)
            .map(|second| HistoryPoint {
                elapsed_second: second,
                latency_ms: if (4860..4870).contains(&second) || (4960..4970).contains(&second) {
                    None
                } else {
                    Some(if second == 5040 {
                        43
                    } else if second == 4810 || second == 4920 {
                        280
                    } else {
                        30 + (second * 7 % 23)
                    })
                },
                successes: u32::from(
                    !(4860..4870).contains(&second) && !(4960..4970).contains(&second),
                ),
                failures: u32::from((4860..4870).contains(&second)),
                explicit_gap: (4960..4970).contains(&second),
                download_mbps: Some(12.4),
                upload_mbps: Some(0.8),
            })
            .collect(),
        traffic: Traffic {
            interface: Some("en0".into()),
            download_mbps: Some(12.4),
            upload_mbps: Some(0.8),
            age_ms: Some(1_000),
            fresh: true,
        },
        location: Location {
            fix: Some(Fix {
                latitude: 51.345,
                longitude: 12.377,
                accuracy_m: 65.0,
                source_utc: "2026-10-10T08:35:58Z".into(),
            }),
            source_age_ms: Some(2_000),
            state: "Available".into(),
        },
    }
}

fn probe(protocol: Protocol, target: &str, latency: u64, age: u64, interval: u64) -> Probe {
    Probe {
        protocol,
        target: target.into(),
        successful: Some(true),
        outcome: "Success".into(),
        latency_ms: Some(latency),
        age_ms: Some(age),
        fresh: true,
        interval_ms: interval,
    }
}

pub fn recording(id: &str) -> Result<RecordingDetail, String> {
    let session = sessions()
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| "Recording not found".to_string())?;
    let no_location = id == "demo-no-location";
    let reverse = id == "demo-past";
    let (date, started_second, duration_ms) = if reverse {
        ("2026-10-08", 15 * 3600 + 5 * 60, 5_460_000)
    } else if no_location {
        ("2026-10-07", 8 * 3600, 5_040_000)
    } else {
        ("2026-10-10", 7 * 3600 + 12 * 60, 5_040_000)
    };
    let points: Vec<_> = (0..=84)
        .map(|i| {
            let fraction = f64::from(i) / 84.0;
            let p = if reverse { 1.0 - fraction } else { fraction };
            let gap = (54..60).contains(&i);
            let status = if (32..35).contains(&i) {
                Health::Offline
            } else if (27..32).contains(&i) {
                Health::Slow
            } else if gap {
                Health::Stale
            } else {
                Health::Healthy
            };
            let elapsed_ms = duration_ms * i as u64 / 84;
            let utc_second = started_second + elapsed_ms / 1000;
            let utc = format!(
                "{date}T{:02}:{:02}:{:02}Z",
                utc_second / 3600,
                utc_second % 3600 / 60,
                utc_second % 60
            );
            RecordingPoint {
                elapsed_ms,
                utc: utc.clone(),
                status,
                latency_ms: if status == Health::Offline || status == Health::Stale {
                    None
                } else {
                    Some(if status == Health::Slow {
                        284
                    } else {
                        30 + (i as u64 * 7 % 23)
                    })
                },
                download_mbps: Some(12.4),
                upload_mbps: Some(0.8),
                location: if no_location || gap {
                    None
                } else {
                    Some(Fix {
                        latitude: 52.476 - 1.131 * p,
                        longitude: 13.365 - 0.988 * p - (p * std::f64::consts::PI).sin() * 0.23,
                        accuracy_m: 65.0,
                        source_utc: utc,
                    })
                },
                break_before: i == 0 || gap || i == 60,
            }
        })
        .collect();
    let classified = points
        .iter()
        .filter(|point| {
            matches!(
                point.status,
                Health::Healthy | Health::Slow | Health::Partial | Health::Offline
            )
        })
        .count();
    let healthy = points
        .iter()
        .filter(|point| point.status == Health::Healthy)
        .count();
    let healthy_percent = (classified > 0).then(|| healthy as f64 / classified as f64 * 100.0);
    let mut latencies = points
        .iter()
        .filter_map(|point| point.latency_ms)
        .collect::<Vec<_>>();
    latencies.sort_unstable();
    let median_latency_ms = latencies.get(latencies.len() / 2).copied();
    let event_count = points.len() as u64;
    Ok(RecordingDetail {
        session,
        duration_ms,
        points,
        healthy_percent,
        median_latency_ms,
        event_count,
        truncated: false,
        location_max_age_ms: 30_000,
        location_max_accuracy_m: 1_000.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_demo_matches_the_current_recording_endpoint() {
        let live = live();
        let detail = recording("demo-current").expect("current fixture");
        let endpoint = detail.points.last().expect("last fixture observation");
        let live_fix = live.location.fix.as_ref().expect("live location");
        let endpoint_fix = endpoint.location.as_ref().expect("recorded location");
        assert_eq!(live.utc, endpoint.utc);
        assert_eq!(live.elapsed_ms, endpoint.elapsed_ms);
        assert!((live_fix.latitude - endpoint_fix.latitude).abs() < 0.000001);
        assert!((live_fix.longitude - endpoint_fix.longitude).abs() < 0.000001);
        assert_eq!(
            live.probes
                .iter()
                .filter(|probe| probe.protocol == Protocol::Tcp)
                .filter_map(|probe| probe.latency_ms)
                .max(),
            endpoint.latency_ms
        );
        assert_eq!(
            live.history.last().and_then(|point| point.latency_ms),
            endpoint.latency_ms
        );
    }

    #[test]
    fn past_demo_observations_match_the_session_clock_and_computed_summary() {
        let detail = recording("demo-past").expect("past fixture");
        assert_eq!(
            detail.points.first().map(|point| point.utc.as_str()),
            Some("2026-10-08T15:05:00Z")
        );
        assert_eq!(
            detail.points.last().map(|point| point.utc.as_str()),
            Some("2026-10-08T16:36:00Z")
        );
        assert_eq!(
            detail.points.last().map(|point| point.elapsed_ms),
            Some(5_460_000)
        );
        assert_eq!(detail.event_count, 85);
        assert_eq!(detail.healthy_percent, Some(71.0 / 79.0 * 100.0));
        assert_eq!(detail.median_latency_ms, Some(42));
        assert!(detail.points.iter().all(|point| {
            point
                .location
                .as_ref()
                .is_none_or(|fix| fix.source_utc == point.utc)
        }));
        let no_location = recording("demo-no-location").expect("no location fixture");
        assert_eq!(
            no_location.points.first().map(|point| point.utc.as_str()),
            Some("2026-10-07T08:00:00Z")
        );
        assert_eq!(
            no_location.points.last().map(|point| point.utc.as_str()),
            Some("2026-10-07T09:24:00Z")
        );
        assert!(
            no_location
                .points
                .iter()
                .all(|point| point.location.is_none())
        );
    }
}
