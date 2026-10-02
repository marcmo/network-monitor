# Network Monitor

A Rust terminal application for monitoring the Mac's current internet connection
and retaining complete ride recordings. No daemon, speed tests, bandwidth claims,
or comparison of alternative networks.

## Build and launch

Requires Rust and the Xcode Command Line Tools on macOS. The application uses
Tokio, Ratatui/Crossterm, reqwest/Rustls, bundled SQLite, and Apple's CoreLocation.

```sh
cd /Users/oliver.mueller/dev/network_monitor
./scripts/package-app.sh
"./target/Network Monitor.app/Contents/MacOS/network-monitor" --label "Train ride"
```

The package contains one executable. Launch the executable from your terminal;
keep the terminal open while recording. Every launch starts a new session.
Press `q` or Ctrl-C to finish and restore the terminal.

A plain `cargo build --release` also builds the executable, but the minimal app
bundle was necessary to obtain a location prompt in the tested launch context.

## Checks and interpretation

The defaults are deliberately small and bounded:

| Check | Default target | Interval | Deadline | What it measures |
| --- | --- | --- | --- | --- |
| TCP | `1.1.1.1:443` (Cloudflare), `8.8.8.8:443` (Google) | 2 s each | 1.5 s | Connection establishment, including round trips and local scheduling |
| DNS | `example.com` | 15 s | 2 s | System resolver success, possibly from cache |
| HTTPS | `https://www.gstatic.com/generate_204` | 30 s | 3 s | TLS validation and a successful HTTP HEAD response |

A 250 ms TCP threshold flags slow interactive connectivity. It is a practical
starting point for browsing and remote shells, not a throughput estimate or a
service guarantee. Configure thresholds and probe timing with a TOML file using
`--config PATH`; start with [config.example.toml](config.example.toml). Unknown keys
and invalid limits are rejected. Longer configured intervals also lengthen
outage detection; the five-second target applies to the defaults.

TCP failures are **failed connection attempts**, not packet loss. Two independent
TCP destinations provide quick reachability evidence; one failed destination is
a partial failure. DNS and HTTPS are separate slower checks. The UI shows each
check's outcome, scope, age, and freshness. A small successful probe cannot prove
that an SSH host, agent provider, or large transfer is working.

Probes follow system IP routing, including applicable VPN routes. HTTPS uses a
direct connection and does not use HTTP proxy environment variables or automatic
proxy configuration. A proxy-only network may therefore work in a browser while
these checks fail. The DNS check uses the normal system resolver, including its
cache and split-DNS behavior; it does not force an uncached DNS transaction.

Current status means:

- `STARTING`: no usable TCP observations yet.
- `STALE / UNKNOWN`: at least one TCP check lacks a recent measured result.
- `OFFLINE`: both independent TCP destinations have recent measured failures,
  with no HTTPS success from the current failed attempts or later. A concurrent
  successful HTTPS check keeps the status `PARTIAL`.
- `PARTIAL`: one TCP target failed, or DNS/HTTPS is missing, stale, unavailable,
  or failing.
- `SLOW`: all checks succeeded but a TCP timing exceeds the configured threshold.
- `HEALTHY`: all checks are current and successful, with TCP timings below the
  threshold. This is a statement about the sampled targets.

Only one system DNS worker exists, with a bounded mailbox. An uncancellable
`getaddrinfo` call can make DNS/HTTPS unavailable until it returns; it cannot
create an accumulating pool of stuck resolver threads. All probes have bounded
application deadlines and no application retry loops.

## Five-minute display and complete recordings

The graph shows the last 300 elapsed seconds and advances once per second,
including during an outage or while no observations are arriving. Missing
observations and explicit gaps are different from measured failures. Latencies
are connection timings, not ICMP round-trip measurements.

Recordings use SQLite with WAL journaling and `synchronous=FULL`. Raw events are
stored for the entire session, not just the visible window. The recording writer
owns its connection on a dedicated thread. Bounded message queues connect the
sampler, application, and writer; overload or persistence errors are visible and
stop monitoring instead of silently pretending that recording continues.

Ordinary shutdown cancels outstanding probes, drains completed observations, and
marks the recording ended. A crash can lose events still in bounded in-memory
queues; already committed SQLite events survive according to SQLite and the
filesystem's durability guarantees. A session with no end timestamp was not
closed normally. Recordings are retained until you delete them yourself.

The database schema is versioned with `PRAGMA user_version`:

- `sessions`: session UUID, optional label, start/end UTC, application version,
  and the complete probe/threshold configuration as JSON.
- `events`: ordered event ID, session UUID, UTC receipt/completion time,
  continuous elapsed milliseconds, observation generation, event type, and typed
  JSON payload. Probe payloads include target, protocol, start elapsed time,
  duration, and outcome. Location payloads retain coordinates, reported accuracy,
  and the source fix timestamp separately from receipt time.

Use SQLite's JSON functions for later analysis. For example, after selecting the
recording database path:

```sql
SELECT id, label, started_utc, ended_utc FROM sessions ORDER BY started_utc;
SELECT utc, elapsed_ms,
       json_extract(payload_json, '$.data.target') AS target,
       json_extract(payload_json, '$.data.duration_ms') AS duration_ms,
       json_extract(payload_json, '$.data.outcome.type') AS outcome
FROM events WHERE event_type = 'probe' ORDER BY id;
```

No location fix is silently attached to a later probe. Join source timestamps
explicitly and choose age/accuracy limits appropriate for your analysis.

## Location permissions and gaps

Allow the macOS location prompt if you want approximate locations recorded
locally. Denied, pending, temporarily unavailable, stale, or inaccurate location
does not stop connectivity monitoring. If necessary, inspect System Settings >
Privacy & Security > Location Services for Network Monitor and your terminal.
Keep the app bundle at a stable path between launches.

The native request uses kilometer desired accuracy and a 100 m distance filter
to keep positioning best effort and avoid requesting navigation-grade updates.
These are requests to CoreLocation, not guarantees about update frequency or
accuracy. Every returned valid fix retains its reported accuracy.

The early test on this Mac confirmed the prompt and authorization for the bundled
executable after the user approved. The first returned fix was roughly eight
minutes old with about 60 m reported accuracy. This is why the application keeps
source time and receipt time separate. Mac location depends on the environment;
continuous useful positioning on a moving train is not guaranteed.

Elapsed time on macOS uses `mach_continuous_time`, which includes sleep. A detected suspension (continuous versus awake time, with 50 ms tolerance) or a
long scheduling delay records a gap and starts a new observation generation. Late results
from earlier generations cannot restore current health. UTC clock adjustments
are recorded independently so future analysis can account for wall-clock jumps.

Apple references: [location usage description](https://developer.apple.com/documentation/bundleresources/information-property-list/nslocationusagedescription),
[single-file executable metadata](https://developer.apple.com/library/archive/documentation/Security/Conceptual/CodeSigningGuide/Procedures/Procedures.html#//apple_ref/doc/uid/TP40005929-CH5-SW14),
[permission attribution and launch context](https://developer.apple.com/forums/thread/732431),
[Mac location](https://support.apple.com/en-gb/guide/mac-help/mh27621/mac).
