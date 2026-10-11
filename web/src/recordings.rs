use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use leptos::prelude::*;
use network_monitor_web_types::{LiveSnapshot, RecordingDetail, RecordingPoint, RecordingSummary};

use crate::map::RouteMap;

#[derive(Clone)]
struct DetailRequest {
    id: String,
    generation: u64,
}
#[derive(Default)]
struct RequestQueue {
    running: bool,
    generation: u64,
    pending: Option<DetailRequest>,
}

fn duration(milliseconds: u64) -> String {
    let seconds = milliseconds / 1000;
    if seconds >= 3600 {
        format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60)
    } else {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    }
}
fn title(session: &RecordingSummary) -> String {
    session
        .label
        .clone()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Untitled recording".into())
}
fn timestamp(value: &str) -> String {
    let time = value.trim_end_matches('Z').trim_end_matches("+00:00");
    time.split_once('.')
        .map_or(time, |(whole, _fraction)| whole)
        .replace('T', " ")
}
fn latency(value: Option<u64>) -> String {
    value
        .map(|value| format!("{value} ms"))
        .unwrap_or_else(|| "Unavailable".into())
}
fn traffic(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.2} Mbps"))
        .unwrap_or_else(|| "Unavailable".into())
}

pub fn retained_sample_index(
    previous: Option<&RecordingDetail>,
    next: &RecordingDetail,
    selected: usize,
) -> usize {
    let last = next.points.len().saturating_sub(1);
    let Some(previous) = previous else {
        return last;
    };
    if previous.session.id != next.session.id || selected >= previous.points.len().saturating_sub(1)
    {
        return last;
    }
    let Some(point) = previous.points.get(selected) else {
        return last;
    };
    let after = next
        .points
        .partition_point(|next| next.elapsed_ms < point.elapsed_ms);
    let lower = after.saturating_sub(1);
    match (next.points.get(lower), next.points.get(after)) {
        (Some(before), Some(after_point)) => {
            if point.elapsed_ms.abs_diff(before.elapsed_ms)
                <= point.elapsed_ms.abs_diff(after_point.elapsed_ms)
            {
                lower
            } else {
                after
            }
        }
        (Some(_before), None) => last,
        (None, Some(_after)) => 0,
        (None, None) => 0,
    }
}

#[derive(Clone, Copy)]
struct Explorer {
    sessions: RwSignal<Vec<RecordingSummary>>,
    selected: RwSignal<String>,
    detail: RwSignal<Option<Arc<RecordingDetail>>>,
    list_loading: RwSignal<bool>,
    list_error: RwSignal<Option<String>>,
    detail_loading: RwSignal<bool>,
    detail_error: RwSignal<Option<String>>,
    sample_index: RwSignal<usize>,
    queue: RwSignal<RequestQueue>,
    cancellation: StoredValue<Option<web_sys::AbortController>, LocalStorage>,
}

impl Explorer {
    fn load_list(self, alive: Arc<AtomicBool>) {
        if self.list_loading.get_untracked() {
            return;
        }
        self.list_loading.set(true);
        self.list_error.set(None);
        leptos::task::spawn_local(async move {
            if !alive.load(Ordering::Relaxed) {
                return;
            }
            let signal = self
                .cancellation
                .with_value(|controller| controller.as_ref().map(web_sys::AbortController::signal));
            let result = crate::api::recordings_with_signal(signal).await;
            if !alive.load(Ordering::Relaxed) {
                return;
            }
            self.list_loading.set(false);
            match result {
                Ok(sessions) => {
                    if !sessions
                        .iter()
                        .any(|session| session.id == self.selected.get_untracked())
                    {
                        self.selected.set(
                            sessions
                                .first()
                                .map(|session| session.id.clone())
                                .unwrap_or_default(),
                        );
                    }
                    self.sessions.set(sessions);
                }
                Err(error) => self.list_error.set(Some(error)),
            }
        });
    }

    fn request_detail(self, id: String, alive: Arc<AtomicBool>) {
        if id.is_empty() {
            return;
        }
        if self.detail.with_untracked(|detail| {
            detail
                .as_ref()
                .is_some_and(|detail| detail.session.id != id)
        }) {
            self.detail.set(None);
        }
        self.detail_error.set(None);
        self.detail_loading.set(true);
        let mut start = false;
        self.queue.update(|queue| {
            queue.generation = queue.generation.wrapping_add(1);
            queue.pending = Some(DetailRequest {
                id,
                generation: queue.generation,
            });
            if !queue.running {
                queue.running = true;
                start = true;
            }
        });
        if !start {
            return;
        }
        leptos::task::spawn_local(async move {
            loop {
                if !alive.load(Ordering::Relaxed) {
                    return;
                }
                let mut request = None;
                self.queue.update(|queue| {
                    request = queue.pending.take();
                    if request.is_none() {
                        queue.running = false;
                    }
                });
                let Some(request) = request else {
                    return;
                };
                let signal = self.cancellation.with_value(|controller| {
                    controller.as_ref().map(web_sys::AbortController::signal)
                });
                let result = crate::api::recording_with_signal(&request.id, signal).await;
                if !alive.load(Ordering::Relaxed) {
                    return;
                }
                let current = self
                    .queue
                    .with_untracked(|queue| queue.generation == request.generation)
                    && self.selected.get_untracked() == request.id;
                if current {
                    self.detail_loading.set(false);
                    match result {
                        Ok(detail) => {
                            if detail.session.id != request.id {
                                self.detail_error.set(Some("The monitor returned a different recording. Retry to load the selected session.".into()));
                                continue;
                            }
                            let index = self.detail.with_untracked(|previous| {
                                retained_sample_index(
                                    previous.as_deref(),
                                    &detail,
                                    self.sample_index.get_untracked(),
                                )
                            });
                            self.sample_index.set(index);
                            self.detail.set(Some(Arc::new(detail)));
                        }
                        Err(error) => self.detail_error.set(Some(error)),
                    }
                }
            }
        });
    }
}

#[component]
pub fn Recordings(live: RwSignal<Option<LiveSnapshot>>) -> impl IntoView {
    let explorer = Explorer {
        sessions: RwSignal::new(Vec::new()),
        selected: RwSignal::new(String::new()),
        detail: RwSignal::new(None),
        list_loading: RwSignal::new(false),
        list_error: RwSignal::new(None),
        detail_loading: RwSignal::new(false),
        detail_error: RwSignal::new(None),
        sample_index: RwSignal::new(0),
        queue: RwSignal::new(RequestQueue::default()),
        cancellation: StoredValue::new_local(web_sys::AbortController::new().ok()),
    };
    let refresh = RwSignal::new(0_u64);
    let last_refresh = RwSignal::new(None::<(String, u64)>);
    let alive = Arc::new(AtomicBool::new(true));
    let cleanup_alive = alive.clone();
    on_cleanup(move || {
        cleanup_alive.store(false, Ordering::Relaxed);
        explorer.cancellation.with_value(|controller| {
            if let Some(controller) = controller {
                controller.abort();
            }
        });
    });
    explorer.load_list(alive.clone());
    let fetch_alive = alive.clone();
    Effect::new(move |_| {
        let id = explorer.selected.get();
        refresh.get();
        explorer.request_detail(id, fetch_alive.clone());
    });
    Effect::new(move |_| {
        let selected = explorer.selected.get();
        let clock = live.with(|live| {
            live.as_ref()
                .map(|snapshot| (snapshot.session_id.clone(), snapshot.elapsed_ms))
        });
        if let Some((id, elapsed_ms)) = clock
            && selected == id
        {
            let changed = last_refresh.with_untracked(|previous| {
                previous
                    .as_ref()
                    .is_some_and(|(previous_id, previous_bucket)| {
                        previous_id == &id && elapsed_ms.saturating_sub(*previous_bucket) >= 10_000
                    })
            });
            let new_session = last_refresh.with_untracked(|previous| {
                previous
                    .as_ref()
                    .is_none_or(|(previous_id, _elapsed)| previous_id != &id)
            });
            if changed || new_session {
                last_refresh.set(Some((id, elapsed_ms)));
            }
            if changed {
                refresh.update(|value| *value = value.wrapping_add(1));
            }
        }
    });
    let retry_list_alive = StoredValue::new(alive.clone());
    let refresh_list_alive = alive;
    let selected_point = Memo::new(move |_| {
        explorer.detail.with(|detail| {
            detail
                .as_ref()
                .and_then(|detail| detail.points.get(explorer.sample_index.get()).cloned())
        })
    });
    view! {
        <section id="recordings" class="rec-explorer" aria-labelledby="tab-recordings">
            <div class="page-heading recordings-heading"><div><div class="eyebrow">"Session explorer"</div><h1>"A signal, in context."</h1><p class="subline">"Follow the route. See where the connection changed."</p></div><span class="rec-count mono">{move || format!("{} recordings", explorer.sessions.with(Vec::len))}</span></div>
            <Show when=move || explorer.list_loading.get()><p class="rec-notice" role="status">"Loading recordings..."</p></Show>
            <Show when=move || explorer.list_error.with(Option::is_some)>
                <div class="rec-notice rec-error" role="alert"><span>{move || explorer.list_error.get().unwrap_or_default()}</span><button class="plain-button" disabled=move || explorer.list_loading.get() on:click=move |_| explorer.load_list(retry_list_alive.get_value())>"Retry recordings"</button></div>
            </Show>
            <Show when=move || !explorer.list_loading.get() && explorer.list_error.with(Option::is_none) && explorer.sessions.with(Vec::is_empty)>
                <div class="rec-empty"><h2>"No recordings yet"</h2><p>"Sessions will appear here once the monitor has saved a recording."</p></div>
            </Show>
            <Show when=move || !explorer.sessions.with(Vec::is_empty)>
                <div class="rec-workspace">
                    <div class="rec-map-column">
                        <div class="rec-selection"><label class="eyebrow" for="recording-select">"Recording"</label>
                            <select id="recording-select" prop:value=move || explorer.selected.get() on:change=move |event| explorer.selected.set(event_target_value(&event))>
                                <For each=move || explorer.sessions.get() key=|session| session.id.clone() children=move |session| {
                                    let label = format!("{} / {}", title(&session), timestamp(&session.started_utc));
                                    view! { <option value=session.id>{label}</option> }
                                }/>
                            </select>
                            <span class="rec-load-state" role="status">{move || if explorer.detail_loading.get() { "Updating..." } else { "Saved locally" }}</span>
                        </div>
                        <Show when=move || explorer.detail_error.with(Option::is_some)>
                            <div class="rec-notice rec-error" role="alert"><span>{move || explorer.detail_error.get().unwrap_or_default()}</span><button class="plain-button" disabled=move || explorer.detail_loading.get() on:click=move |_| refresh.update(|value| *value = value.wrapping_add(1))>"Retry session"</button></div>
                        </Show>
                        <Show when=move || explorer.detail_loading.get() && explorer.detail.with(Option::is_none)><p class="rec-notice" role="status">"Loading the selected recording..."</p></Show>
                        <RouteMap recording=explorer.detail sample_index=explorer.sample_index/>
                        <div class="rec-timeline">
                            <div class="rec-timeline-top"><span>"Explore the recording"</span><span class="mono cyan">{move || selected_point.with(|point| point.as_ref().map(|point| format!("{} UTC", timestamp(&point.utc))).unwrap_or_else(|| "No observations".into()))}</span></div>
                            <div class="rec-timeline-track">
                                <svg viewBox="0 0 1000 24" preserveAspectRatio="none" aria-hidden="true">{move || explorer.detail.with(|detail| detail.as_ref().map(|detail| {
                                    let length = detail.points.len().max(1) as f64;
                                    detail.points.iter().enumerate().map(|(index, point)| view! { <rect class=format!("rec-health-strip {}", point.status.css()) x={index as f64 / length * 1000.0} y="0" width={1000.0 / length + 0.1} height="24"/> }).collect_view()
                                }))}</svg>
                                <input type="range" min="0" step="1" max=move || explorer.detail.with(|detail| detail.as_ref().map(|detail| detail.points.len().saturating_sub(1)).unwrap_or_default()) prop:value=move || explorer.sample_index.get() disabled=move || explorer.detail.with(|detail| detail.as_ref().is_none_or(|detail| detail.points.is_empty())) aria-label="Recording timeline" aria-valuetext=move || selected_point.with(|point| point.as_ref().map(|point| format!("{}, {}", timestamp(&point.utc), point.status.label())).unwrap_or_else(|| "No observations".into())) on:input=move |event| {
                                    if let Ok(index) = event_target_value(&event).parse::<usize>() { explorer.sample_index.set(index); }
                                }/>
                            </div>
                            <div class="rec-timeline-labels"><span>"Session start"</span><span>{move || explorer.detail.with(|detail| detail.as_ref().map(|detail| duration(detail.duration_ms)).unwrap_or_default())}</span></div>
                            <p class="rec-help">"Drag or use arrow keys to inspect an observation. Missing locations leave breaks in the route."</p>
                            <Show when=move || explorer.detail.with(|detail| detail.as_ref().is_some_and(|detail| detail.truncated))><p class="rec-warning">"Timeline sampled for display; full events remain recorded."</p></Show>
                        </div>
                    </div>
                    <aside class="rec-detail" aria-label="Selected recording details">
                        {move || explorer.detail.get().map(|detail| {
                            let state = detail.session.state.label();
                            let name = title(&detail.session);
                            let started = format!("{} UTC", timestamp(&detail.session.started_utc));
                            let healthy = detail.healthy_percent.map(|value| format!("{value:.1}%")).unwrap_or_else(|| "Unavailable".into());
                            let length = duration(detail.duration_ms);
                            let median = latency(detail.median_latency_ms);
                            let fixes = detail.points.iter().filter(|point| point.location.is_some()).count();
                            let location_rule = format!("Locations need accuracy within {:.0} m and an age under {} seconds. No fix is drawn across a gap.", detail.location_max_accuracy_m, detail.location_max_age_ms / 1000);
                            view! {
                                <div class="rec-detail-head"><span class="eyebrow">{state}</span><h2>{name}</h2><p class="rec-detail-meta">{started}</p></div>
                                <div class="rec-metrics"><div class="rec-kpi"><span>"Healthy samples"</span><strong class="green">{healthy}</strong></div><p class="rec-help">"Of classified connectivity observations"</p><div class="rec-kpi"><span>"Duration"</span><strong>{length}</strong></div><div class="rec-kpi"><span>"Median TCP"</span><strong>{median}</strong></div><div class="rec-kpi"><span>"Located samples"</span><span class="mono">{format!("{} / {}", fixes, detail.points.len())}</span></div></div>
                                <div class="rec-detail-section"><div class="eyebrow">"Location quality"</div><p class="rec-help">{location_rule}</p></div>
                            }
                        })}
                        <div class="rec-detail-section"><div class="eyebrow">"Selected observation"</div>
                            {move || selected_point.get().map(|point| view! { <Observation point=point/> })}
                            <Show when=move || selected_point.with(Option::is_none)><p class="rec-help">"No observation selected."</p></Show>
                        </div>
                        <div class="rec-detail-section"><div class="eyebrow">"Reading this map"</div><p class="rec-help">"Colored dots show connectivity measured at each location. Areas between observations are unknown."</p><p class="rec-help">"This records your journey; it does not predict carrier coverage."</p></div>
                    </aside>
                </div>
            </Show>
            <div class="rec-index"><div class="rec-index-heading"><h2>"Recordings"</h2><button class="plain-button" disabled=move || explorer.list_loading.get() on:click=move |_| explorer.load_list(refresh_list_alive.clone())>"Refresh list"</button></div>
                <For each=move || explorer.sessions.get() key=|session| session.id.clone() children=move |session| {
                    let id = session.id.clone();
                    let active_id = id.clone();
                    let name = title(&session);
                    let date = format!("{} UTC", timestamp(&session.started_utc));
                    let state = session.state.label();
                    view! { <button class="rec-session-row" aria-pressed=move || (explorer.selected.get() == active_id).to_string() on:click=move |_| explorer.selected.set(id.clone())><span class="rec-session-name">{name}</span><span class="rec-session-date mono">{date}</span><span class="rec-session-state">{state}</span><span class="cyan" aria-hidden="true">">"</span></button> }
                }/>
            </div>
        </section>
    }
}

#[component]
fn Observation(point: RecordingPoint) -> impl IntoView {
    let location = point
        .location
        .as_ref()
        .map(|fix| format!("{:.5}, {:.5}", fix.latitude, fix.longitude))
        .unwrap_or_else(|| "Unavailable".into());
    let accuracy = point
        .location
        .as_ref()
        .map(|fix| format!("{:.0} m", fix.accuracy_m))
        .unwrap_or_else(|| "Unavailable".into());
    view! {
        <div class="rec-sample-time"><span class="mono">{duration(point.elapsed_ms)}</span><span class=point.status.css()>{point.status.label()}</span></div>
        <dl class="rec-detail-list"><dt>"TCP latency"</dt><dd>{latency(point.latency_ms)}</dd><dt>"Location"</dt><dd>{location}</dd><dt>"Accuracy"</dt><dd>{accuracy}</dd><dt>"Receive"</dt><dd>{traffic(point.download_mbps)}</dd><dt>"Transmit"</dt><dd>{traffic(point.upload_mbps)}</dd></dl>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refreshing_a_recording_preserves_the_inspected_time_or_follows_the_end() {
        let mut previous = crate::demo::recording("demo-current").expect("demo recording");
        previous.points.truncate(3);
        for (index, point) in previous.points.iter_mut().enumerate() {
            point.elapsed_ms = index as u64 * 1_000;
        }
        let mut next = previous.clone();
        next.points = (0..=6)
            .map(|index| {
                let mut point = previous.points[0].clone();
                point.elapsed_ms = index * 500;
                point
            })
            .collect();
        assert_eq!(retained_sample_index(Some(&previous), &next, 1), 2);
        assert_eq!(retained_sample_index(Some(&previous), &next, 2), 6);
        next.session.id = "another-recording".into();
        assert_eq!(retained_sample_index(Some(&previous), &next, 1), 6);
    }
}
