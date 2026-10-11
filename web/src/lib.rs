pub mod api;
pub mod app;
pub mod demo;
pub mod live;
pub mod map;
pub mod recordings;
pub mod transport;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    leptos::mount::mount_to_body(app::App);
}
