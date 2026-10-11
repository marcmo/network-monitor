use crate::live::Live;
use crate::recordings::Recordings;
use crate::transport::{self, ConnectionState};
use leptos::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Live,
    Recordings,
}

#[component]
pub fn App() -> impl IntoView {
    let (live, connection) = transport::connect();
    let initial_tab = if web_sys::window()
        .and_then(|window| window.location().search().ok())
        .is_some_and(|search| {
            search
                .trim_start_matches('?')
                .split('&')
                .any(|part| part == "tab=recordings")
        }) {
        Tab::Recordings
    } else {
        Tab::Live
    };
    let tab = RwSignal::new(initial_tab);
    let show_recordings = Callback::new(move |()| tab.set(Tab::Recordings));
    let demo = transport::is_demo();
    view! {
        <a class="skip-link" href="#main-content">"Skip to content"</a>
        <header class="top">
            <a class="brand" href="/" aria-label="Network monitor home">
                <svg width="25" height="25" viewBox="0 0 25 25" fill="none" aria-hidden="true">
                    <path d="M2 14h5l3-8 5 14 3-8h5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
                "network monitor"
            </a>
            <nav class="tabs" aria-label="Main navigation">
                <button class="tab" id="tab-live" aria-current=move || if tab.get() == Tab::Live { Some("page") } else { None } aria-controls="live" on:click=move |_| tab.set(Tab::Live)>"Live"</button>
                <button class="tab" id="tab-recordings" aria-current=move || if tab.get() == Tab::Recordings { Some("page") } else { None } aria-controls="recordings" on:click=move |_| tab.set(Tab::Recordings)>"Recordings"</button>
            </nav>
            <div class="top-right">
                <span class="connection" role="status">
                    <i class=move || if matches!(connection.get(), ConnectionState::Connected) { "status-dot healthy" } else { "status-dot unknown" }></i>
                    {move || match connection.get() {
                        ConnectionState::Connecting => "Connecting",
                        ConnectionState::Connected => if demo { "Demo snapshot" } else { "Monitor connected" },
                        ConnectionState::Disconnected(_) => "Monitor disconnected",
                        ConnectionState::Hidden => "Updates paused",
                        ConnectionState::Stopped => "Monitor disconnected",
                    }}
                </span>
                {demo.then(|| view! { <span class="sample">"Demo data"</span> })}
            </div>
        </header>
        <main id="main-content">
            <Show when=move || !matches!(connection.get(), ConnectionState::Connected)>
                <div class="connection-banner" role="status">
                    <div>
                        <strong>{move || match connection.get() {
                            ConnectionState::Connecting => "Connecting to the monitor",
                            ConnectionState::Connected => "Connected",
                            ConnectionState::Disconnected(_) => "Live connection interrupted",
                            ConnectionState::Hidden => "Live updates paused while this page is hidden",
                            ConnectionState::Stopped => "Unable to reconnect automatically",
                        }}</strong>
                        <p>{move || match connection.get() {
                            ConnectionState::Connecting => "Waiting for a current snapshot.".to_owned(),
                            ConnectionState::Connected => String::new(),
                            ConnectionState::Disconnected(message) => format!("{message} Last received values may be out of date."),
                            ConnectionState::Hidden => "Updates resume when you return to this page.".to_owned(),
                            ConnectionState::Stopped => "Last received values are retained. Check that the monitor is running, then reconnect.".to_owned(),
                        }}</p>
                    </div>
                    <Show when=move || matches!(connection.get(), ConnectionState::Disconnected(_) | ConnectionState::Stopped)>
                        <button class="plain-button" on:click=move |_| {
                            if let Some(window) = web_sys::window() {
                                let _result = window.location().reload();
                            }
                        }>"Reconnect"</button>
                    </Show>
                </div>
            </Show>
            <Show when=move || tab.get() == Tab::Live fallback=move || view! { <Recordings live/> }>
                <Live live connection on_recordings=show_recordings/>
            </Show>
        </main>
        <footer class="site-footer"><span>"Local network observatory"</span><span>"Live and recorded observations"</span></footer>
    }
}
