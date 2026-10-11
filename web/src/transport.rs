use leptos::prelude::*;
use network_monitor_web_types::LiveSnapshot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Disconnected(String),
    Hidden,
    Stopped,
}
impl ConnectionState {
    pub fn label(&self) -> &str {
        match self {
            Self::Connecting => "Connecting",
            Self::Connected => "Connected to monitor",
            Self::Disconnected(reason) => reason,
            Self::Hidden => "Updates paused while hidden",
            Self::Stopped => "Monitor disconnected - reconnect to retry",
        }
    }
}

#[derive(Default)]
pub struct RetryBudget {
    failures: u8,
}
impl RetryBudget {
    pub fn failed(&mut self) -> Option<u32> {
        self.failures = self.failures.saturating_add(1);
        (self.failures < 5).then(|| 1000 << (self.failures - 1))
    }
    pub fn received(&mut self) {
        self.failures = 0;
    }
}

const FRESHNESS_WINDOW_MS: u32 = 7_000;

#[derive(Debug, PartialEq, Eq)]
enum FreshnessError {
    InvalidTimestamp,
    Expired,
    NotAdvancing,
}

impl FreshnessError {
    fn message(self) -> &'static str {
        match self {
            Self::InvalidTimestamp => "Monitor snapshot has an invalid timestamp",
            Self::Expired => "Monitor snapshot is out of date",
            Self::NotAdvancing => "Monitor has not published a newer snapshot",
        }
    }
}

#[derive(Default)]
struct SnapshotFreshness {
    last: Option<(String, u64)>,
}

impl SnapshotFreshness {
    fn observe(
        &mut self,
        session: &str,
        elapsed_ms: u64,
        source_utc_ms: f64,
        now_ms: f64,
    ) -> Result<u32, FreshnessError> {
        let age_ms = now_ms - source_utc_ms;
        if !age_ms.is_finite() || age_ms < -1_000.0 {
            return Err(FreshnessError::InvalidTimestamp);
        }
        if age_ms >= f64::from(FRESHNESS_WINDOW_MS) {
            return Err(FreshnessError::Expired);
        }
        if self
            .last
            .as_ref()
            .is_some_and(|(last_session, last_elapsed)| {
                last_session == session && elapsed_ms <= *last_elapsed
            })
        {
            return Err(FreshnessError::NotAdvancing);
        }
        self.last = Some((session.to_owned(), elapsed_ms));
        Ok((f64::from(FRESHNESS_WINDOW_MS) - age_ms.max(0.0))
            .floor()
            .max(1.0) as u32)
    }
}

pub fn is_demo() -> bool {
    web_sys::window()
        .and_then(|window| window.location().search().ok())
        .and_then(|search| web_sys::UrlSearchParams::new_with_str(&search).ok())
        .and_then(|params| params.get("demo"))
        .is_some_and(|value| value == "1")
}

pub fn connect() -> (RwSignal<Option<LiveSnapshot>>, RwSignal<ConnectionState>) {
    let live = RwSignal::new(None);
    let state = RwSignal::new(ConnectionState::Connecting);
    if is_demo() {
        live.set(Some(crate::demo::live()));
        state.set(ConnectionState::Connected);
        return (live, state);
    }
    browser::mount(live, state);
    (live, state)
}

mod browser {
    use super::*;
    use gloo_timers::callback::Timeout;
    use std::{
        cell::RefCell,
        rc::{Rc, Weak},
    };
    use wasm_bindgen::{JsCast, closure::Closure};
    use web_sys::{Event, EventSource, MessageEvent};

    struct Stream {
        source: Option<EventSource>,
        message: Option<Closure<dyn FnMut(MessageEvent)>>,
        error: Option<Closure<dyn FnMut(Event)>>,
        visibility: Option<Closure<dyn FnMut(Event)>>,
        reconnect: Option<Timeout>,
        freshness: Option<Timeout>,
        policy: RetryBudget,
        snapshot_freshness: SnapshotFreshness,
        live: RwSignal<Option<LiveSnapshot>>,
        state: RwSignal<ConnectionState>,
        disposed: bool,
    }
    impl Stream {
        fn close_source(&mut self) {
            if let Some(source) = self.source.take() {
                source.close();
                if let Some(callback) = &self.message {
                    let _ = source.remove_event_listener_with_callback(
                        "snapshot",
                        callback.as_ref().unchecked_ref(),
                    );
                }
                source.set_onerror(None);
            }
            self.message = None;
            self.error = None;
            self.freshness = None;
        }
        fn dispose(&mut self) {
            self.disposed = true;
            self.close_source();
            self.reconnect = None;
            if let Some(callback) = self.visibility.take()
                && let Some(document) = web_sys::window().and_then(|window| window.document())
            {
                let _ = document.remove_event_listener_with_callback(
                    "visibilitychange",
                    callback.as_ref().unchecked_ref(),
                );
            }
        }
    }

    fn hidden() -> bool {
        web_sys::window()
            .and_then(|window| window.document())
            .is_some_and(|document| document.hidden())
    }

    fn retry(owner: &Rc<RefCell<Stream>>, reason: String) {
        let weak = Rc::downgrade(owner);
        let mut stream = owner.borrow_mut();
        if stream.disposed || hidden() {
            return;
        }
        stream.state.set(ConnectionState::Disconnected(reason));
        if let Some(source) = &stream.source {
            source.close();
        }
        stream.freshness = None;
        match stream.policy.failed() {
            Some(delay) => {
                stream.reconnect = Some(Timeout::new(delay, move || {
                    if let Some(owner) = weak.upgrade() {
                        open(&owner);
                    }
                }))
            }
            None => {
                stream.reconnect = None;
                stream.state.set(ConnectionState::Stopped);
            }
        }
    }

    fn arm_freshness(owner: &Rc<RefCell<Stream>>, delay_ms: u32) {
        let weak = Rc::downgrade(owner);
        owner.borrow_mut().freshness = Some(Timeout::new(delay_ms, move || {
            if let Some(owner) = weak.upgrade() {
                retry(&owner, "No fresh update from monitor".into());
            }
        }));
    }

    fn open(owner: &Rc<RefCell<Stream>>) {
        {
            let mut stream = owner.borrow_mut();
            stream.close_source();
            stream.reconnect = None;
            if stream.disposed {
                return;
            }
            if hidden() {
                stream.state.set(ConnectionState::Hidden);
                return;
            }
            stream.state.set(ConnectionState::Connecting);
        }
        let source = match EventSource::new("/api/live/stream") {
            Ok(source) => source,
            Err(error) => {
                retry(owner, format!("Cannot connect: {error:?}"));
                return;
            }
        };
        let weak: Weak<RefCell<Stream>> = Rc::downgrade(owner);
        let message = Closure::wrap(Box::new(move |event: MessageEvent| {
            let Some(owner) = weak.upgrade() else {
                return;
            };
            let Some(data) = event.data().as_string() else {
                retry(&owner, "Invalid monitor update".into());
                return;
            };
            match serde_json::from_str::<Option<LiveSnapshot>>(&data) {
                Ok(Some(snapshot)) => {
                    let source_utc_ms = js_sys::Date::parse(&snapshot.utc);
                    let mut stream = owner.borrow_mut();
                    let freshness = stream.snapshot_freshness.observe(
                        &snapshot.session_id,
                        snapshot.elapsed_ms,
                        source_utc_ms,
                        js_sys::Date::now(),
                    );
                    match freshness {
                        Ok(delay_ms) => {
                            stream.policy.received();
                            stream.live.set(Some(snapshot));
                            stream.state.set(ConnectionState::Connected);
                            drop(stream);
                            arm_freshness(&owner, delay_ms);
                        }
                        Err(error) => {
                            drop(stream);
                            retry(&owner, error.message().to_owned());
                        }
                    }
                }
                Ok(None) => {}
                Err(error) => retry(&owner, format!("Invalid monitor update: {error}")),
            }
        }) as Box<dyn FnMut(MessageEvent)>);
        let weak = Rc::downgrade(owner);
        let error = Closure::wrap(Box::new(move |_event: Event| {
            if let Some(owner) = weak.upgrade() {
                retry(&owner, "Monitor disconnected - retrying".into());
            }
        }) as Box<dyn FnMut(Event)>);
        if let Err(error) =
            source.add_event_listener_with_callback("snapshot", message.as_ref().unchecked_ref())
        {
            source.close();
            retry(owner, format!("Cannot subscribe: {error:?}"));
            return;
        }
        source.set_onerror(Some(error.as_ref().unchecked_ref()));
        let mut stream = owner.borrow_mut();
        stream.source = Some(source);
        stream.message = Some(message);
        stream.error = Some(error);
        drop(stream);
        arm_freshness(owner, FRESHNESS_WINDOW_MS);
    }

    pub fn mount(live: RwSignal<Option<LiveSnapshot>>, state: RwSignal<ConnectionState>) {
        let owner = Rc::new(RefCell::new(Stream {
            source: None,
            message: None,
            error: None,
            visibility: None,
            reconnect: None,
            freshness: None,
            policy: RetryBudget::default(),
            snapshot_freshness: SnapshotFreshness::default(),
            live,
            state,
            disposed: false,
        }));
        let weak = Rc::downgrade(&owner);
        let visibility = Closure::wrap(Box::new(move |_event: Event| {
            if let Some(owner) = weak.upgrade() {
                if hidden() {
                    let mut stream = owner.borrow_mut();
                    stream.close_source();
                    stream.reconnect = None;
                    stream.state.set(ConnectionState::Hidden);
                } else {
                    owner.borrow_mut().policy.received();
                    open(&owner);
                }
            }
        }) as Box<dyn FnMut(Event)>);
        if let Some(document) = web_sys::window().and_then(|window| window.document())
            && let Err(error) = document.add_event_listener_with_callback(
                "visibilitychange",
                visibility.as_ref().unchecked_ref(),
            )
        {
            state.set(ConnectionState::Disconnected(format!(
                "Cannot observe page visibility: {error:?}"
            )));
            return;
        }
        owner.borrow_mut().visibility = Some(visibility);
        open(&owner);
        let stored = StoredValue::new_local(owner);
        on_cleanup(move || stored.with_value(|owner| owner.borrow_mut().dispose()));
    }
}

#[cfg(test)]
mod tests {
    use super::{FreshnessError, RetryBudget, SnapshotFreshness};
    #[test]
    fn recovery_stops_after_five_failures_and_a_sample_resets_the_budget() {
        let mut policy = RetryBudget::default();
        assert_eq!(policy.failed(), Some(1000));
        assert_eq!(policy.failed(), Some(2000));
        assert_eq!(policy.failed(), Some(4000));
        assert_eq!(policy.failed(), Some(8000));
        assert_eq!(policy.failed(), None);
        policy.received();
        assert_eq!(policy.failed(), Some(1000));
    }

    #[test]
    fn fresh_snapshot_expires_from_its_source_timestamp() {
        let mut freshness = SnapshotFreshness::default();
        assert_eq!(freshness.observe("a", 1000, 4500.0, 5000.0), Ok(6500));
        assert_eq!(freshness.observe("a", 2000, 5500.0, 6000.0), Ok(6500));
    }

    #[test]
    fn reconnect_replay_and_backwards_samples_cannot_restore_freshness() {
        let mut freshness = SnapshotFreshness::default();
        assert_eq!(freshness.observe("a", 1000, 5000.0, 5000.0), Ok(7000));
        assert_eq!(
            freshness.observe("a", 1000, 5000.0, 6000.0),
            Err(FreshnessError::NotAdvancing)
        );
        assert_eq!(
            freshness.observe("a", 900, 6000.0, 6000.0),
            Err(FreshnessError::NotAdvancing)
        );
        assert_eq!(freshness.observe("a", 2000, 6500.0, 6500.0), Ok(7000));
    }

    #[test]
    fn first_snapshot_and_new_sessions_must_have_recent_valid_timestamps() {
        let mut freshness = SnapshotFreshness::default();
        assert_eq!(
            freshness.observe("a", 1000, 0.0, 7000.0),
            Err(FreshnessError::Expired)
        );
        assert_eq!(
            freshness.observe("a", 1000, f64::NAN, 7000.0),
            Err(FreshnessError::InvalidTimestamp)
        );
        assert_eq!(
            freshness.observe("a", 1000, 9000.0, 7000.0),
            Err(FreshnessError::InvalidTimestamp)
        );
        assert_eq!(freshness.observe("a", 1000, 7000.0, 7000.0), Ok(7000));
        assert_eq!(
            freshness.observe("b", 0, 0.0, 7000.0),
            Err(FreshnessError::Expired)
        );
        assert_eq!(freshness.observe("b", 0, 7000.0, 7000.0), Ok(7000));
    }

    #[test]
    fn rejected_sample_does_not_advance_last_accepted_position() {
        let mut freshness = SnapshotFreshness::default();
        assert_eq!(freshness.observe("a", 1000, 7000.0, 7000.0), Ok(7000));
        assert_eq!(
            freshness.observe("a", 3000, 0.0, 7000.0),
            Err(FreshnessError::Expired)
        );
        assert_eq!(freshness.observe("a", 2000, 7000.0, 7000.0), Ok(7000));
    }
}
