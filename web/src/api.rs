use gloo_net::http::Request;
use gloo_timers::callback::Timeout;
use network_monitor_web_types::{ApiError, RecordingDetail, RecordingSummary};
use serde::de::DeserializeOwned;
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{AbortController, AbortSignal, Event};

struct RequestLifetime {
    controller: AbortController,
    _timeout: Timeout,
    parent: Option<ParentCancellation>,
}
struct ParentCancellation {
    signal: AbortSignal,
    callback: Closure<dyn FnMut(Event)>,
}
impl Drop for RequestLifetime {
    fn drop(&mut self) {
        self.controller.abort();
        if let Some(parent) = &self.parent {
            let _ = parent.signal.remove_event_listener_with_callback(
                "abort",
                parent.callback.as_ref().unchecked_ref(),
            );
        }
    }
}

async fn get<T: DeserializeOwned>(url: &str, parent: Option<AbortSignal>) -> Result<T, String> {
    let controller =
        AbortController::new().map_err(|error| format!("Cannot prepare request: {error:?}"))?;
    let parent = if let Some(signal) = parent {
        if signal.aborted() {
            controller.abort();
        }
        let child = controller.clone();
        let callback =
            Closure::wrap(Box::new(move |_event: Event| child.abort()) as Box<dyn FnMut(Event)>);
        signal
            .add_event_listener_with_callback("abort", callback.as_ref().unchecked_ref())
            .map_err(|error| format!("Cannot observe request cancellation: {error:?}"))?;
        Some(ParentCancellation { signal, callback })
    } else {
        None
    };
    let timeout_controller = controller.clone();
    let lifetime = RequestLifetime {
        controller,
        _timeout: Timeout::new(15_000, move || timeout_controller.abort()),
        parent,
    };
    let response = Request::get(url)
        .abort_signal(Some(&lifetime.controller.signal()))
        .send()
        .await
        .map_err(|error| {
            format!("Cannot read from the monitor (request may have timed out): {error}")
        })?;
    if !response.ok() {
        return Err(response.json::<ApiError>().await.map_or_else(
            |error| format!("Monitor returned HTTP {}: {error}", response.status()),
            |error| error.message,
        ));
    }
    response
        .json()
        .await
        .map_err(|error| format!("Invalid monitor response: {error}"))
}

pub async fn recordings() -> Result<Vec<RecordingSummary>, String> {
    recordings_with_signal(None).await
}
pub async fn recordings_with_signal(
    signal: Option<AbortSignal>,
) -> Result<Vec<RecordingSummary>, String> {
    if crate::transport::is_demo() {
        return Ok(crate::demo::sessions());
    }
    get("/api/recordings", signal).await
}

pub async fn recording(id: &str) -> Result<RecordingDetail, String> {
    recording_with_signal(id, None).await
}
pub async fn recording_with_signal(
    id: &str,
    signal: Option<AbortSignal>,
) -> Result<RecordingDetail, String> {
    if crate::transport::is_demo() {
        return crate::demo::recording(id);
    }
    let encoded = js_sys::encode_uri_component(id);
    get(&format!("/api/recordings/{encoded}"), signal).await
}
