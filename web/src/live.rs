use crate::transport::ConnectionState;
use leptos::prelude::*;
use network_monitor_web_types::{Health, HistoryPoint, LiveSnapshot, Probe, Protocol};

fn p95(values: &[u64]) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let mut ordered = values.to_vec();
    ordered.sort_unstable();
    ordered.get((ordered.len() * 95).div_ceil(100) - 1).copied()
}

fn line_segments(samples: &[(u64, Option<u64>, bool)], end: u64, scale: u64) -> Vec<String> {
    let mut segments = Vec::new();
    let mut path = String::new();
    let mut previous = None;
    for &(second, latency, gap) in samples {
        if second < end.saturating_sub(299) || second > end {
            continue;
        }
        if gap
            || latency.is_none()
            || previous.is_some_and(|last: u64| second != last.saturating_add(1))
        {
            if !path.is_empty() {
                segments.push(std::mem::take(&mut path));
            }
            previous = None;
        }
        if !gap && let Some(latency) = latency {
            let x = 744.0 - end.saturating_sub(second) as f64 * 704.0 / 299.0;
            let y = 184.0 - latency as f64 / scale.max(1) as f64 * 152.0;
            if path.is_empty() {
                path.push_str(&format!("M{x:.2} {y:.2}h.01"));
            } else {
                path.push_str(&format!("L{x:.2} {y:.2}"));
            }
            previous = Some(second);
        }
    }
    if !path.is_empty() {
        segments.push(path);
    }
    segments
}

pub fn duration(milliseconds: u64) -> String {
    let seconds = milliseconds / 1_000;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3_600,
        seconds / 60 % 60,
        seconds % 60
    )
}

fn milliseconds(value: Option<u64>) -> String {
    value.map_or_else(|| "--".to_owned(), |value| value.to_string())
}

fn age(value: Option<u64>) -> String {
    value.map_or_else(
        || "Unavailable".to_owned(),
        |value| {
            if value < 1_000 {
                "<1 sec".to_owned()
            } else {
                format!("{} sec", value / 1_000)
            }
        },
    )
}

fn headline(status: Health) -> &'static str {
    match status {
        Health::Starting => "Getting a first reading.",
        Health::Healthy => "Steady signal.",
        Health::Slow => "A slower connection.",
        Health::Partial => "Some checks are failing.",
        Health::Offline => "Connection unavailable.",
        Health::Stale => "Waiting for a fresh signal.",
    }
}

fn history_health(point: &HistoryPoint, slow_latency_ms: u64) -> Health {
    if point.explicit_gap || point.successes + point.failures == 0 {
        Health::Stale
    } else if point.successes == 0 {
        Health::Offline
    } else if point.failures > 0 {
        Health::Partial
    } else if point
        .latency_ms
        .is_some_and(|latency| latency >= slow_latency_ms)
    {
        Health::Slow
    } else {
        Health::Healthy
    }
}

#[component]
pub fn Live(
    live: RwSignal<Option<LiveSnapshot>>,
    connection: RwSignal<ConnectionState>,
    on_recordings: Callback<()>,
) -> impl IntoView {
    let demo = crate::transport::is_demo();
    let current = Memo::new(move |_| {
        matches!(connection.get(), ConnectionState::Connected)
            && live.with(|snapshot| {
                snapshot.as_ref().is_some_and(|snapshot| {
                    !matches!(snapshot.status, Health::Stale | Health::Starting)
                })
            })
    });
    let latest = Memo::new(move |_| {
        live.with(|snapshot| {
            snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .probes
                    .iter()
                    .filter(|probe| {
                        probe.protocol == Protocol::Tcp
                            && probe.fresh
                            && probe.successful == Some(true)
                    })
                    .filter_map(|probe| probe.latency_ms)
                    .max()
            })
        })
    });
    let percentile = Memo::new(move |_| {
        live.with(|snapshot| {
            snapshot.as_ref().and_then(|snapshot| {
                let values: Vec<u64> = snapshot
                    .history
                    .iter()
                    .filter(|point| !point.explicit_gap)
                    .filter_map(|point| point.latency_ms)
                    .collect();
                p95(&values)
            })
        })
    });
    let tcp_summary = Memo::new(move |_| {
        live.with(|snapshot| {
            snapshot.as_ref().map_or_else(
                || "Waiting for probes".to_owned(),
                |snapshot| {
                    let configured = snapshot
                        .probes
                        .iter()
                        .filter(|probe| probe.protocol == Protocol::Tcp)
                        .count();
                    let successful = snapshot
                        .probes
                        .iter()
                        .filter(|probe| {
                            probe.protocol == Protocol::Tcp
                                && probe.fresh
                                && probe.successful == Some(true)
                        })
                        .count();
                    format!("{successful} of {configured} TCP targets reachable")
                },
            )
        })
    });
    view! {
        <section id="live" aria-labelledby="tab-live">
            <div class="page-heading">
                <div>
                    <div class="eyebrow">"Current session"</div>
                    <h1>{move || if matches!(connection.get(), ConnectionState::Connected) {
                        live.with(|snapshot| snapshot.as_ref().map_or("Waiting for the monitor.", |snapshot| headline(snapshot.status)))
                    } else if live.with(Option::is_some) { "Last received signal." } else { "Waiting for the monitor." }}</h1>
                    <p class="subline">
                        <span>{move || live.with(|snapshot| snapshot.as_ref().map_or_else(|| "No snapshot received".to_owned(), |snapshot| snapshot.label.clone().unwrap_or_else(|| format!("Session {}", snapshot.session_id.chars().take(8).collect::<String>()))))}</span>
                        <span class="sep">"/"</span>
                        <span class="mono">{move || live.with(|snapshot| snapshot.as_ref().map_or_else(|| "--:--:--".to_owned(), |snapshot| duration(snapshot.elapsed_ms)))}</span>
                    </p>
                </div>
                <div class="heading-side">
                    <span class="recording"><i class:muted-dot=move || !current.get()></i>{move || if demo { "Demo snapshot" } else if current.get() { "Recording to disk" } else { "Awaiting live data" }}</span>
                    <button class="plain-button" on:click=move |_| on_recordings.run(())>"View recordings"<span class="mini-arrow">"\u{2197}"</span></button>
                </div>
            </div>
            <Show when=move || live.with(Option::is_some) fallback=|| view! {
                <div class="empty-state"><div class="eyebrow">"Live measurements"</div><h2>"Your first snapshot will appear here."</h2><p>"The monitor publishes connectivity, interface traffic, and location as it records."</p></div>
            }>
                <div class="instrument" class:stale-readings=move || !current.get()>
                    <div class="signal">
                        <div class="section-header"><h2>"TCP connect latency"</h2><span class="range-label mono">"Last 5 minutes"</span></div>
                        <div class="latency-header">
                            <div><div class="primary-reading mono">{move || milliseconds(latest.get())}<span class="unit">"ms"</span></div><span class="reading-caption">{move || if current.get() { "Highest successful TCP result / latest" } else { "Last successful TCP result / retained" }}</span></div>
                            <div class="minor-reading mono">{move || milliseconds(percentile.get())}<span class="unit">"ms"</span><span class="reading-caption">"p95 / sampled last 5m"</span></div>
                        </div>
                        <LatencyChart live/>
                        <div class="plot-legend"><div class="legend-items"><span><i class="legend-line"></i>"Highest TCP time"</span><span>"Gaps = no measurement"</span></div></div>
                        <div class="signal-footer"><span>{move || if current.get() { tcp_summary.get() } else { "Current reachability unknown".to_owned() }}</span><span class="mono">"Bounded 5m history"</span></div>
                    </div>
                    <aside class="status-rail" aria-label="Current network status">
                        <div class="rail-block">
                            <div class="eyebrow">"Connectivity"</div>
                            <div class="health"><strong class=move || if current.get() { live.with(|snapshot| snapshot.as_ref().map_or("unknown", |snapshot| snapshot.status.css())) } else { "unknown" }>{move || if current.get() { live.with(|snapshot| snapshot.as_ref().map_or("Unknown", |snapshot| snapshot.status.label())) } else { "Unknown" }}</strong><span class="snapshot-label">{move || if current.get() { "Current" } else { "Not current" }}</span></div>
                            <div class="health-bars" aria-label="Recent TCP observations">
                                {move || live.with(|snapshot| snapshot.as_ref().map(|snapshot| {
                                    let len = snapshot.history.len();
                                    snapshot.history.iter().skip(len.saturating_sub(45)).map(|point| {
                                        let state = history_health(point, snapshot.slow_latency_ms);
                                        view! { <i class=state.css() title=format!("{} at {}", state.label(), duration(point.elapsed_second * 1_000))></i> }
                                    }).collect_view()
                                }))}
                            </div>
                            <div class="health-foot"><span>"Recent TCP samples"</span><span>"Now"</span></div>
                        </div>
                        <div class="rail-block">
                            <div class="rail-label"><span class="eyebrow">"Interface traffic"</span><span class="mono">{move || live.with(|snapshot| snapshot.as_ref().and_then(|snapshot| snapshot.traffic.interface.clone()).unwrap_or_else(|| "--".to_owned()))}</span></div>
                            <TrafficReading live receive=true/>
                            <TrafficReading live receive=false/>
                            <p class="traffic-note">"Passive interface activity."<br/>"Not a bandwidth or speed test."</p>
                        </div>
                        <div class="rail-block">
                            <div class="eyebrow">"Connection"</div>
                            <div class="interface-row"><span>"Traffic sample age"</span><span class="mono">{move || live.with(|snapshot| age(snapshot.as_ref().and_then(|snapshot| snapshot.traffic.age_ms)))}</span></div>
                            <div class="interface-row"><span>"Probe targets"</span><span>{move || live.with(|snapshot| snapshot.as_ref().map_or(0, |snapshot| snapshot.probes.len()))}</span></div>
                            <div class="interface-row"><span>"Slow threshold"</span><span class="mono">{move || live.with(|snapshot| snapshot.as_ref().map_or_else(|| "--".to_owned(), |snapshot| format!("{} ms", snapshot.slow_latency_ms)))}</span></div>
                        </div>
                        <div class="rail-block location-block">
                            <div class="eyebrow">"Last location"</div>
                            <div class="location-title">{move || live.with(|snapshot| snapshot.as_ref().map_or_else(|| "Unavailable".to_owned(), |snapshot| snapshot.location.state.clone()))}</div>
                            <div class="coordinate">{move || live.with(|snapshot| snapshot.as_ref().and_then(|snapshot| snapshot.location.fix.as_ref()).map_or_else(|| "No location fix".to_owned(), |fix| format!("{:.5}, {:.5}", fix.latitude, fix.longitude)))}</div>
                            <div class="location-foot">{move || live.with(|snapshot| snapshot.as_ref().and_then(|snapshot| snapshot.location.fix.as_ref()).map_or_else(|| "Route points will appear when location is available.".to_owned(), |fix| format!("Reported accuracy +/- {:.0} m", fix.accuracy_m)))}</div>
                            <div class="location-foot">{move || live.with(|snapshot| snapshot.as_ref().and_then(|snapshot| snapshot.location.source_age_ms).map_or_else(String::new, |age| if age >= 0 { format!("Source age {} sec", age / 1_000) } else { "Source timestamp is in the future".to_owned() }))}</div>
                        </div>
                    </aside>
                </div>
                <div class="diagnostics">
                    <div class="diagnostics-heading"><h2>"Protocol diagnostics"</h2><span>"Latest observed result"</span></div>
                    <div class="probe-row header" aria-hidden="true"><span></span><span>"Probe"</span><span>"Target"</span><span>"Latency"</span><span>"Observed"</span><span>"Interval"</span><span></span></div>
                    <For
                        each=move || live.with(|snapshot| snapshot.as_ref().map_or_else(Vec::new, |snapshot| snapshot.probes.iter().map(|probe| (probe.protocol, probe.target.clone())).collect::<Vec<_>>()))
                        key=|(protocol, target)| (protocol.label(), target.clone())
                        children=move |(protocol, target)| view! { <ProbeRow live current protocol target/> }
                    />
                </div>
                <div class="bottom-note"><span>"Connection failures are failed attempts, not packet loss."</span><span class="mono">{move || live.with(|snapshot| snapshot.as_ref().map_or_else(String::new, |snapshot| format!("Snapshot {}", snapshot.utc)))}</span></div>
            </Show>
        </section>
    }
}

#[component]
fn TrafficReading(live: RwSignal<Option<LiveSnapshot>>, receive: bool) -> impl IntoView {
    view! {
        <div class="traffic-pair">
            <span class="traffic-name"><b>{if receive { "\u{2193}" } else { "\u{2191}" }}</b>{if receive { "Receive" } else { "Transmit" }}</span>
            <span class="traffic-value">{move || live.with(|snapshot| snapshot.as_ref().and_then(|snapshot| {
                if snapshot.traffic.fresh {
                    if receive { snapshot.traffic.download_mbps } else { snapshot.traffic.upload_mbps }
                } else { None }
            }).map_or_else(|| "--".to_owned(), |value| format!("{value:.1}")))}<small>"Mbps"</small></span>
        </div>
    }
}

#[component]
fn ProbeRow(
    live: RwSignal<Option<LiveSnapshot>>,
    current: Memo<bool>,
    protocol: Protocol,
    target: String,
) -> impl IntoView {
    let target_search = target.clone();
    let probe = Memo::new(move |_| {
        live.with(|snapshot| {
            snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .probes
                    .iter()
                    .find(|probe| probe.protocol == protocol && probe.target == target_search)
                    .cloned()
            })
        })
    });
    let state = move || {
        probe.with(|probe| {
            probe.as_ref().map_or("unknown", |probe| {
                if !current.get() || !probe.fresh {
                    "unknown"
                } else {
                    match probe.successful {
                        Some(true) => "healthy",
                        Some(false) => "offline",
                        None => "unknown",
                    }
                }
            })
        })
    };
    let result = move || {
        probe.with(|probe| {
            probe.as_ref().map_or_else(
                || "Waiting".to_owned(),
                |probe| {
                    if !current.get() || !probe.fresh {
                        "Stale".to_owned()
                    } else {
                        match probe.successful {
                            Some(true) => format!("{} ms", milliseconds(probe.latency_ms)),
                            Some(false) => "Failed".to_owned(),
                            None => "Waiting".to_owned(),
                        }
                    }
                },
            )
        })
    };
    view! {
        <details class="probe-detail">
            <summary class="probe-row">
                <i class=move || format!("status-dot {}", state())></i>
                <span class="probe-type">{protocol.label()}</span>
                <span class="target">{target}</span>
                <span class=move || format!("result {}", state())>{result}</span>
                <span class="observed">{move || probe.with(|probe| age(probe.as_ref().and_then(|probe| probe.age_ms)))}</span>
                <span class="interval">{move || probe.with(|probe| probe.as_ref().map_or_else(|| "--".to_owned(), |probe| format!("{:.1} sec", probe.interval_ms as f64 / 1_000.0)))}</span>
                <span class="expand" aria-hidden="true">"+"</span>
            </summary>
            <div class="diagnostic-detail"><span>{move || probe.with(|probe| probe.as_ref().map_or_else(|| "No result yet.".to_owned(), |probe: &Probe| format!("Outcome: {}. Last duration: {} ms. Observed {} ago.", probe.outcome, milliseconds(probe.latency_ms), age(probe.age_ms))))}</span><p>"Stale results are retained as history and do not establish current reachability."</p></div>
        </details>
    }
}

#[component]
fn LatencyChart(live: RwSignal<Option<LiveSnapshot>>) -> impl IntoView {
    let chart = Memo::new(move |_| {
        live.with(|snapshot| {
            snapshot.as_ref().map_or_else(
                || (Vec::new(), 100, 0),
                |snapshot| {
                    let scale = snapshot
                        .history
                        .iter()
                        .filter_map(|point| point.latency_ms)
                        .max()
                        .unwrap_or(100)
                        .max(100)
                        .div_ceil(100)
                        .saturating_mul(100);
                    let end = snapshot.elapsed_ms / 1_000;
                    let samples: Vec<_> = snapshot
                        .history
                        .iter()
                        .map(|point| (point.elapsed_second, point.latency_ms, point.explicit_gap))
                        .collect();
                    (line_segments(&samples, end, scale), scale, end)
                },
            )
        })
    });
    view! {
        <div class="chart-host">
            <svg viewBox="0 0 768 222" role="img" aria-label="TCP latency over the last five minutes. Gaps indicate missing measurements.">
                <g class="chart-grid"><path d="M40 32H744M40 108H744M40 184H744"/></g>
                <g class="chart-axis">
                    <text x="0" y="36">{move || chart.with(|chart| chart.1.to_string())}</text>
                    <text x="0" y="112">{move || chart.with(|chart| (chart.1 / 2).to_string())}</text>
                    <text x="0" y="188">"0"</text>
                    <text x="40" y="214">"-5m"</text><text x="376" y="214">"-2m 30s"</text><text x="718" y="214">"Now"</text>
                </g>
                <g class="chart-series">{move || chart.with(|chart| chart.0.iter().map(|path| view! { <path d=path.clone()/> }).collect_view())}</g>
            </svg>
            <Show when=move || chart.with(|chart| chart.0.is_empty())><div class="chart-empty">"No successful TCP measurements in this window"</div></Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_uses_nearest_rank_and_handles_empty_history() {
        assert_eq!(p95(&[]), None);
        assert_eq!(p95(&[40]), Some(40));
        let values: Vec<u64> = (1..=20).rev().collect();
        assert_eq!(p95(&values), Some(19));
    }

    #[test]
    fn chart_breaks_at_unknown_explicit_and_missing_samples() {
        let samples = vec![
            (1, Some(20), false),
            (2, Some(30), false),
            (3, None, false),
            (4, Some(40), false),
            (5, Some(50), true),
            (6, Some(60), false),
            (9, Some(70), false),
        ];
        let segments = line_segments(&samples, 9, 100);
        assert_eq!(segments.len(), 4);
        assert_eq!(segments[0].matches('L').count(), 1);
        assert!(segments.iter().all(|segment| segment.starts_with('M')));
    }

    #[test]
    fn chart_does_not_draw_samples_older_than_window() {
        let samples = vec![(1, Some(20), false), (400, Some(40), false)];
        let segments = line_segments(&samples, 400, 100);
        assert_eq!(segments.len(), 1);
    }
}
