use std::sync::Arc;

use leptos::prelude::*;
use network_monitor_web_types::{Health, RecordingDetail, RecordingPoint};

const WIDTH: f64 = 1000.0;
const HEIGHT: f64 = 500.0;
const TILE_SIZE: f64 = 512.0;

#[derive(Clone, Debug, PartialEq)]
pub struct MapObservation {
    pub sample_index: usize,
    pub x: f64,
    pub y: f64,
    pub status: Health,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MapTile {
    pub x: f64,
    pub y: f64,
    pub url: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapGeometry {
    pub observations: Vec<MapObservation>,
    pub route_segments: Vec<String>,
    pub tiles: Vec<MapTile>,
    pub scale_m: f64,
    pub scale_px: f64,
}

impl MapGeometry {
    pub fn new(points: &[RecordingPoint], zoom_offset: i8) -> Self {
        let positions = points
            .iter()
            .enumerate()
            .filter_map(|(index, point)| {
                let fix = point.location.as_ref()?;
                if !fix.latitude.is_finite()
                    || !fix.longitude.is_finite()
                    || !(-90.0..=90.0).contains(&fix.latitude)
                    || !(-180.0..=180.0).contains(&fix.longitude)
                    || !fix.accuracy_m.is_finite()
                    || fix.accuracy_m < 0.0
                {
                    return None;
                }
                let x = (fix.longitude + 180.0) / 360.0;
                let latitude = fix.latitude.clamp(-85.05112878, 85.05112878).to_radians();
                let y = (1.0 - latitude.tan().asinh() / std::f64::consts::PI) / 2.0;
                Some((index, x, y))
            })
            .collect::<Vec<_>>();
        let Some(first) = positions.first() else {
            return Self::default();
        };
        let reference = first.1;
        let positions = positions
            .into_iter()
            .map(|(index, x, y)| {
                let wrapped_x = reference + (x - reference + 0.5).rem_euclid(1.0) - 0.5;
                (index, wrapped_x, y)
            })
            .collect::<Vec<_>>();
        let (min_x, max_x, min_y, max_y) = positions.iter().fold(
            (
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
            ),
            |(min_x, max_x, min_y, max_y), (_, x, y)| {
                (min_x.min(*x), max_x.max(*x), min_y.min(*y), max_y.max(*y))
            },
        );
        let fit_scale = ((WIDTH - 140.0) / (max_x - min_x).max(0.00001))
            .min((HEIGHT - 140.0) / (max_y - min_y).max(0.00001));
        let zoom = ((fit_scale / TILE_SIZE).log2().floor() as i8 + zoom_offset).clamp(0, 18);
        let tile_count = 2_i32.pow(zoom as u32);
        let world_size = TILE_SIZE * f64::from(tile_count);
        let center_x = (min_x + max_x) / 2.0;
        let center_y = (min_y + max_y) / 2.0;
        let left = center_x * world_size - WIDTH / 2.0;
        let top = center_y * world_size - HEIGHT / 2.0;
        let observations = positions
            .iter()
            .map(|(index, x, y)| MapObservation {
                sample_index: *index,
                x: x * world_size - left,
                y: y * world_size - top,
                status: points[*index].status,
            })
            .collect::<Vec<_>>();
        let mut route_segments = Vec::new();
        let mut segment = Vec::new();
        let mut previous = None;
        for observation in &observations {
            let connected = previous.is_some_and(|index| index + 1 == observation.sample_index)
                && !points[observation.sample_index].break_before;
            if !connected {
                if segment.len() > 1 {
                    route_segments.push(segment.join(" "));
                }
                segment.clear();
            }
            segment.push(format!("{:.2},{:.2}", observation.x, observation.y));
            previous = Some(observation.sample_index);
        }
        if segment.len() > 1 {
            route_segments.push(segment.join(" "));
        }
        let mut tiles = Vec::new();
        let start_x = (left / TILE_SIZE).floor() as i32;
        let start_y = (top / TILE_SIZE).floor() as i32;
        for tile_y in start_y..=((top + HEIGHT) / TILE_SIZE).floor() as i32 {
            if !(0..tile_count).contains(&tile_y) {
                continue;
            }
            for tile_x in start_x..=((left + WIDTH) / TILE_SIZE).floor() as i32 {
                tiles.push(MapTile {
                    x: f64::from(tile_x) * TILE_SIZE - left,
                    y: f64::from(tile_y) * TILE_SIZE - top,
                    url: format!(
                        "https://tile.openstreetmap.org/{}/{}/{}.png",
                        zoom,
                        tile_x.rem_euclid(tile_count),
                        tile_y
                    ),
                });
            }
        }
        let latitude = (std::f64::consts::PI * (1.0 - 2.0 * center_y))
            .sinh()
            .atan();
        let meters_per_pixel = 40_075_016.686 * latitude.cos() / world_size;
        let target_m = meters_per_pixel * 120.0;
        let magnitude = 10_f64.powf(target_m.log10().floor());
        let normalized = target_m / magnitude;
        let scale_m = if normalized >= 5.0 {
            5.0 * magnitude
        } else if normalized >= 2.0 {
            2.0 * magnitude
        } else {
            magnitude
        };
        Self {
            observations,
            route_segments,
            tiles,
            scale_m,
            scale_px: scale_m / meters_per_pixel,
        }
    }

    pub fn observation(&self, sample_index: usize) -> Option<&MapObservation> {
        self.observations
            .binary_search_by_key(&sample_index, |observation| observation.sample_index)
            .ok()
            .and_then(|index| self.observations.get(index))
    }
}

#[component]
pub fn RouteMap(
    recording: RwSignal<Option<Arc<RecordingDetail>>>,
    sample_index: RwSignal<usize>,
) -> impl IntoView {
    let route = RwSignal::new(true);
    let coverage = RwSignal::new(true);
    let background = RwSignal::new(false);
    let zoom = RwSignal::new(0_i8);
    let geometry = Memo::new(move |_| {
        let offset = zoom.get();
        recording.with(|recording| {
            Arc::new(match recording {
                Some(recording) => MapGeometry::new(&recording.points, offset),
                None => MapGeometry::default(),
            })
        })
    });
    let marker =
        Memo::new(move |_| geometry.with(|map| map.observation(sample_index.get()).cloned()));
    Effect::new(move |_| {
        let id = recording.with(|recording| recording.as_ref().map(|item| item.session.id.clone()));
        if id.is_none() {
            zoom.set(0);
        }
    });
    view! {
        <div class="rec-map-tools" aria-label="Map layers">
            <div class="rec-map-label">"Recorded route"</div>
            <button class="plain-button" aria-pressed=move || route.get().to_string() on:click=move |_| route.update(|value| *value = !*value)>"Route"</button>
            <button class="plain-button" aria-pressed=move || coverage.get().to_string() on:click=move |_| coverage.update(|value| *value = !*value)>"Coverage"</button>
            <button class="plain-button" aria-pressed=move || background.get().to_string() title="Load map tiles from OpenStreetMap. This uses the monitored connection." on:click=move |_| background.update(|value| *value = !*value)>"Street map"</button>
        </div>
        <div class="rec-map-canvas">
            <svg viewBox="0 0 1000 500" role="img" aria-label="Recorded route with observed connectivity. Missing locations are not connected.">
                <defs><pattern id="recording-grid" width="50" height="50" patternUnits="userSpaceOnUse"><path d="M 50 0 L 0 0 0 50" fill="none" stroke="#2b4048" stroke-width="0.6"/></pattern></defs>
                <rect width="1000" height="500" fill="#16262d"/>
                <rect width="1000" height="500" fill="url(#recording-grid)"/>
                <g class="rec-map-tiles">{move || if background.get() {
                    geometry.get().tiles.iter().map(|tile| view! { <image href=tile.url.clone() x=tile.x y=tile.y width="512" height="512"/> }).collect_view().into_any()
                } else { ().into_any() }}</g>
                <g class="rec-map-routes" style:display=move || if route.get() { "inline" } else { "none" }>{move || geometry.get().route_segments.iter().map(|points| view! { <polyline points=points.clone() fill="none" stroke="#7bd9e7" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/> }).collect_view()}</g>
                <g style:display=move || if coverage.get() { "inline" } else { "none" }>{move || geometry.get().observations.iter().map(|point| {
                    let index = point.sample_index;
                    let label = format!("Observation {}: {}", index + 1, point.status.label());
                    view! { <circle class=format!("rec-map-sample {}", point.status.css()) cx=point.x cy=point.y r="4" on:click=move |_| sample_index.set(index)><title>{label}</title></circle> }
                }).collect_view()}</g>
                {move || marker.get().map(|point| view! {
                    <g class="rec-map-marker" transform=format!("translate({:.2},{:.2})", point.x, point.y)>
                        <circle r="13" fill="#7bd9e7" opacity="0.18"/><circle r="7" fill="#d4f8ff" stroke="#182b32" stroke-width="3"/>
                    </g>
                })}
                {move || geometry.with(|map| (!map.observations.is_empty()).then(|| {
                    let label = if map.scale_m >= 1000.0 { format!("{:.1} km", map.scale_m / 1000.0) } else { format!("{:.0} m", map.scale_m) };
                    view! { <g transform="translate(30,465)" class="rec-map-scale"><path d=format!("M 0 -5 V 0 H {:.2} V -5", map.scale_px) fill="none" stroke="#c4dbe0" stroke-width="2"/><text x="0" y="-12" fill="#c4dbe0" font-size="13">{label}</text></g> }
                }))}
            </svg>
            <div class="rec-map-key" aria-label="Connectivity legend">
                {[Health::Healthy, Health::Slow, Health::Partial, Health::Offline, Health::Stale].into_iter().map(|health| view! { <span><i class=format!("rec-key-dot {}", health.css())></i>{health.label()}</span> }).collect_view()}
            </div>
            <Show when=move || geometry.with(|map| map.observations.is_empty())>
                <div class="rec-map-empty"><strong>{move || if recording.with(Option::is_some) { "No reliable locations" } else { "Recording map" }}</strong><p>{move || if recording.with(Option::is_some) { "Connectivity was recorded, but there are no usable location fixes for this view." } else { "Waiting for the selected recording data." }}</p></div>
            </Show>
            <div class="rec-map-zoom" aria-label="Map zoom">
                <button aria-label="Zoom in on map" disabled={move || zoom.get() >= 6} on:click=move |_| zoom.update(|value| *value = (*value + 1).min(6))>"+"</button>
                <button aria-label="Zoom out on map" disabled={move || zoom.get() <= -4} on:click=move |_| zoom.update(|value| *value = (*value - 1).max(-4))>"-"</button>
                <button class="rec-fit" on:click=move |_| zoom.set(0)>"Fit"</button>
            </div>
        </div>
        <div class="rec-map-note">
            <span>{move || if background.get() { "Street tiles use your connection; route and observations stay local." } else { "Offline coordinate map. Enable Street map for geographic detail." }}</span>
            <Show when=move || background.get()><a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noopener noreferrer">"Map data (c) OpenStreetMap contributors"</a></Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use network_monitor_web_types::Fix;

    fn point(longitude: f64, latitude: f64, break_before: bool) -> RecordingPoint {
        RecordingPoint {
            elapsed_ms: 0,
            utc: "2026-10-10T10:00:00Z".into(),
            status: Health::Healthy,
            latency_ms: Some(20),
            download_mbps: None,
            upload_mbps: None,
            location: Some(Fix {
                latitude,
                longitude,
                accuracy_m: 10.0,
                source_utc: "2026-10-10T10:00:00Z".into(),
            }),
            break_before,
        }
    }

    #[test]
    fn antimeridian_crossing_stays_local_and_tiles_stay_bounded() {
        let geometry = MapGeometry::new(&[point(179.9, 0.0, false), point(-179.9, 0.0, false)], 0);
        assert_eq!(geometry.route_segments.len(), 1);
        assert_eq!(geometry.observations.len(), 2);
        let distance = (geometry.observations[1].x - geometry.observations[0].x).abs();
        assert!(distance > 100.0 && distance < 860.0);
        assert!(geometry.tiles.len() <= 9);
        assert!(
            geometry
                .observations
                .iter()
                .all(|point| point.x.is_finite() && point.y.is_finite())
        );
    }

    #[test]
    fn invalid_locations_are_omitted_and_stationary_polar_fixes_remain_finite() {
        let geometry = MapGeometry::new(
            &[
                point(f64::NAN, 10.0, false),
                point(200.0, 10.0, false),
                point(13.0, 90.0, false),
                point(13.0, 90.0, false),
            ],
            0,
        );
        assert_eq!(geometry.observations.len(), 2);
        assert_eq!(geometry.route_segments.len(), 1);
        assert!(
            geometry
                .observations
                .iter()
                .all(|point| point.x.is_finite() && point.y.is_finite())
        );
        assert!(geometry.scale_m.is_finite() && geometry.scale_m > 0.0);
        assert!(geometry.scale_px.is_finite() && geometry.scale_px > 0.0);
    }

    #[test]
    fn route_preserves_location_gaps_in_the_rendered_geometry() {
        let mut missing = point(13.3, 52.5, false);
        missing.location = None;
        let geometry = MapGeometry::new(
            &[
                point(13.0, 52.5, false),
                point(13.1, 52.5, false),
                missing,
                point(13.4, 52.5, false),
                point(13.5, 52.5, true),
                point(13.6, 52.5, false),
            ],
            0,
        );
        assert_eq!(geometry.route_segments.len(), 2);
        assert_eq!(geometry.observations.len(), 5);
        assert!(geometry.observation(2).is_none());
        assert!(geometry.observation(3).is_some());
    }
}
