# Dashboard design and validation

The Live and Recordings tabs use the Instrument design: a dark, compact layout
with clear numeric readings and a map-centered recording explorer.

## Architecture

- [web/src](../web/src) contains the Leptos client, compiled to WebAssembly using
  client-side rendering. [scripts/build-web.sh](../scripts/build-web.sh) builds
  the release assets into `web/dist` using Trunk.
- [src/web/mod.rs](../src/web/mod.rs) serves those assets and read-only API routes
  from the existing monitor process. The default listener is `127.0.0.1:8742`;
  `--web-port` changes the port. Host validation allows the matching loopback
  address or localhost, and responses do not enable CORS.
- Live data comes from the same `watch::Receiver<Option<View>>` used by the
  terminal. `/api/live` returns the latest snapshot; `/api/live/stream` sends an
  initial `snapshot` event and subsequent updates through server-sent events.
  A slow browser receives the latest published state without blocking monitoring.
- `/api/recordings` and `/api/recordings/{id}` read the configured SQLite database
  on blocking workers. Detail projection streams event rows through the existing
  `Monitor` model in a read transaction; it does not load all raw events into a
  collection or change the recording writer.
- [web-types/src/lib.rs](../web-types/src/lib.rs) is the shared serialized API
  contract. [src/web/projection.rs](../src/web/projection.rs) owns display
  projection and location-join rules.

The sampler, recording actor, CoreLocation ownership, and terminal lifecycle
retain their existing roles. Browser disconnects do not stop monitoring. Explicit
server shutdown closes live streams before waiting for the listener to finish.
Missing built assets return a build instruction page with HTTP 503.

Shutdown cancels client connections, including clients that stop reading an
asset response. Already-running SQLite reads on blocking workers can finish
after their HTTP request is cancelled. There is no hard shutdown latency
guarantee for a pathological database or stalled filesystem.

## Display semantics and limits

Live connection status is a monitor snapshot, separate from browser connection
status. If fresh snapshots stop arriving, the page retains the last values while
showing an interrupted connection; those values do not establish current health.
The browser pauses live transport when hidden and resumes when visible. Automatic
retries are bounded, with an explicit reconnect action afterward.

Recording status is current only when its identifier matches the published live
session. An absent end timestamp alone means unfinished, not current. Empty
recordings have no points, healthy percentage, or median latency.

Recorded events with the same elapsed timestamp are applied before the snapshot
at that timestamp. Timeline samples span the whole recording at a uniform
interval of at least one second, with the final observation included. The display
is bounded to 10,000 points. When a wider interval is needed, the API sets
`truncated` and the page displays a sampling notice. This flag describes reduced
display resolution; raw SQLite events remain intact. Brief connectivity changes
between samples can be absent from the displayed timeline.

Healthy percentage is the fraction of classified display samples that are
healthy. Slow, partial, and offline samples are classified; starting and unknown
samples are excluded. It is neither packet delivery rate nor exact time-weighted
uptime. Latency aggregates use available fresh, successful TCP timings; traffic
is passive interface activity, not available network capacity.

A map point requires finite coordinates, source age between 0 and 30,000 ms,
reported horizontal accuracy between 0 and 1,000 m, and the current observation
generation. Future-dated, stale, inaccurate, denied, pending, and unavailable
fixes cannot establish a mapped observation. Missing fixes, sleep/generation
changes, and location discontinuities between display samples split the route.
Raw location events remain available for analysis with different explicit limits.

The SVG route and coverage layers require no map service. Coverage means
connectivity observed at sampled locations; it does not predict conditions
between observations or show carrier coverage. The optional Street map control
loads `tile.openstreetmap.org` images only after it is enabled. These external
requests use the monitored connection and reveal the viewed map area. Background
tile failure does not invalidate locally rendered route observations.

## Build and automated checks

From the repository root, with macOS Command Line Tools and Rust 1.88 or newer:

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk --version 0.21.14 --locked
./scripts/build-web.sh
cargo test --test web
cargo test -p network-monitor-web --lib
cargo check -p network-monitor-web --target wasm32-unknown-unknown
cargo clippy -p network-monitor --lib --test web -- -D warnings
```

`./scripts/package-app.sh` runs the dashboard build and copies the assets into
`target/Network Monitor.app/Contents/Resources/web`. A packaged launch can then
serve the dashboard without Trunk running. Trunk is a build tool, not a required
runtime server.

Backend validation completed during implementation:

| Check | Result |
| --- | --- |
| `cargo test --test web --test web_shutdown` | 13 HTTP and lifecycle tests passed |
| `cargo clippy -p network-monitor --lib --test web -- -D warnings` | Passed |

The HTTP tests use real temporary recordings and cover initial/live snapshots,
stale and unavailable values, SSE updates and connected-client shutdown, grouped
probe replay, recording lookup and unknown identifiers, current versus unfinished
sessions, concurrent recording writes, empty statistics, location exclusions and
route discontinuities, static assets and Host validation, and bounded display of
an entire long recording including its final point.

These tests do not establish browser rendering performance, real mobile-network
coverage, macOS positioning accuracy, or physical sleep/resume behavior.

## Browser fixture and manual checks

For a runtime integration fixture, build the website and run:

```sh
./scripts/build-web.sh
cargo run --example web_fixture
```

Open [http://127.0.0.1:8742](http://127.0.0.1:8742). The fixture labels itself
`SIMULATED browser validation`, writes generated events to a temporary SQLite
database, publishes live snapshots, and serves the actual HTTP/SSE interfaces.
It creates synthetic failures and a location gap; it does not probe a real
network or obtain real location fixes. Ctrl-C shuts it down and removes its
temporary recording database. The default port must be free.

The separate
[demo view](http://127.0.0.1:8742/?demo=1&tab=recordings) uses browser-side synthetic
data instead of the live and recording APIs. The visible Demo data label is
intentional. Use the fixture without `demo=1` to validate transport and database
integration.

Browser verification recipe:

1. Check both tabs at desktop and narrow mobile widths. Verify readable units,
   keyboard focus, recording selection, map toggles, zoom, and timeline scrubbing.
2. In the fixture, confirm live elapsed time advances and its recording appears
   as current. Inspect the generated failure section and the broken route at the
   location gap.
3. Leave Street map off and confirm the page requests no external map assets.
   Then enable it explicitly, verify attribution and tiles, and confirm the route
   remains visible if external tile requests fail.
4. Hide and restore the page. Stop the fixture while Live is open and confirm the
   retained values are marked disconnected rather than current. Restart it and
   exercise reconnect, including the explicit reconnect control after retries.
5. Measure first render, steady live updates, recording selection, timeline
   scrubbing, and long-recording rendering on the release build. Record browser,
   viewport, fixture size, elapsed measurement interval, and whether external
   tiles were enabled. Keep browser and monitor-process resource measurements
   separate.

## Browser and performance evidence

Verified on 2026-10-10 with a release build, Apple M4 Pro, 48 GiB RAM,
macOS 26.6.2, Rust 1.99.0, Trunk 0.21.14, and the Chromium 155 in-app browser.
No fixed latency or CPU guarantee is implied by these local samples.

- Both tabs rendered at 1280 x 720 and 390 x 844. Document width matched the
  viewport at both sizes. Map zoom/fit, coverage layers, optional street tiles,
  recording selection, and keyboard timeline inspection worked. The timeline's
  keyboard focus remained visible. No application console warnings or errors
  appeared during the connected workflow.
- The explicit demo covered current, completed, and unfinished recordings,
  including a recording with no location fixes. Street tiles loaded after the
  control was enabled; the route was usable before that request.
- The HTTP/SSE fixture updated the real Leptos page once per second. Its live
  history stayed at 300 points after more than eight minutes. Stopping the server
  changed the page to retained, non-current readings. Bounded retries ended with
  a visible Reconnect control.
  Restarting the fixture and pressing Reconnect restored a new current snapshot;
  command plus DOM verification took 93 ms.
- A packaged monitor was launched in a terminal with a separate validation
  database and port 8744. Its actual network observations reached the browser.
  Pressing `q` exited successfully, closed the listener, and set the recording's
  end timestamp. No location permission prompt was approved during validation.
- All 93 workspace tests passed, along with workspace Clippy for all targets
  with warnings denied, formatting, diff checks, the optimized WASM build, and
  app packaging/signature verification.

Process CPU was measured from macOS `ps` cumulative CPU-time differences over
30-second intervals, divided by elapsed time; percentages refer to one CPU core.
The renderer was attributed to the page by its creation time. These are process
measurements, not browser main-thread or GPU measurements, and other development
work was running on the machine.

| Workload | Measured result |
| --- | --- |
| Live tab, one snapshot/second, no interaction, tiles off | Renderer 0.21 CPU seconds / 30.04 seconds, approximately 0.70% of one core; fixture server 0.03 CPU seconds, approximately 0.10% |
| Same live interval, renderer resident memory | 213.2 MiB before, 202.8 MiB after |
| Idle demo recording, 85 samples, tiles off | No CPU-time increase at 0.01-second counter resolution over 30.02 seconds; resident memory 172.8 MiB before and after |
| Three demo recording selections, command plus DOM verification | 17, 12, and 11 ms |
| Keyboard timeline Home command | 11 ms |
| Maximum-size recording, 10,000 samples / 9,991 locations / 49,992 events | Release frontend rendered all points and the final time; reload to populated summary took 1,107 ms, keyboard Home 19 ms, and zoom plus fit commands together 557 ms |

Interaction timings are automation round-trip proxies, not input-to-paint or INP
measurements. Initial-paint and incoming-message-to-paint timings were not
measured. Browser visibility/discard recovery and GPU cost were not measured;
visibility cleanup is implemented but still warrants manual lifecycle testing.
The maximum-size recording used a temporary debug-build server and a 2.99 MB
JSON response; its load measurement includes HTTP replay and decoding. It does
not establish a release-server latency limit. Temporary stress-fixture sources
were removed after verification.

The release WASM asset is approximately 772 KiB before HTTP compression.
