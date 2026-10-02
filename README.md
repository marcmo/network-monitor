# Network Monitor

A Rust terminal application for monitoring the Mac's current internet connection
and retaining complete ride recordings. No daemon, speed tests, bandwidth claims,
or comparison of alternative networks.

## Build and launch

Requires macOS, Rust 1.88 or newer, and the Xcode Command Line Tools. The
application uses Tokio, Ratatui/Crossterm, reqwest/Rustls, bundled SQLite, and
Apple's CoreLocation. No separate SQLite installation is needed.

```sh
cd /Users/oliver.mueller/dev/network_monitor
./scripts/package-app.sh
"/Users/oliver.mueller/dev/network_monitor/target/Network Monitor.app/Contents/MacOS/network-monitor" --label "Train ride"
```

The script builds a release executable, creates the app bundle, and applies and
verifies an ad-hoc signature. The package contains one executable and has no
application helper process. Launch the executable from your terminal; keep the
terminal open while recording. Every launch starts a new session. Use a terminal
of at least 80 columns by 24 rows; recording continues if it becomes smaller.
Press `q` or Ctrl-C to finish and restore the terminal. SIGINT, SIGTERM, and SIGHUP
also request an orderly shutdown.

A plain `cargo build --release` also builds the executable, but the minimal app
bundle was necessary to obtain a location prompt in the tested launch context.

The default recording database is
`~/Library/Application Support/network-monitor/rides.sqlite3`; parent directories
are created automatically. Override the configuration, database, and optional
ride label independently:

```sh
"/Users/oliver.mueller/dev/network_monitor/target/Network Monitor.app/Contents/MacOS/network-monitor" \
  --config "/Users/oliver.mueller/dev/network_monitor/config.example.toml" \
  --db "$HOME/Library/Application Support/network-monitor/test-rides.sqlite3" \
  --label "Train ride"
```

Use `--help` for options and `--version` for the application version. Monitoring
requires an interactive terminal for both input and output; redirected launches
are rejected before creating a recording.

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

The default detection budget is approximately 2 s until the next TCP attempt +
1.5 s deadline + 1 s application refresh + 0.5 s terminal poll = 5 s. This assumes
normal OS scheduling and responsive terminal output. Deterministic tests cover
delayed probe starts and both display phases; physical outages and arbitrary OS
stalls do not have a hard real-time guarantee.

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
- `SLOW`: all checks succeeded but a TCP timing meets or exceeds the configured
  threshold.
- `HEALTHY`: all checks are current and successful, with TCP timings below the
  threshold. This is a statement about the sampled targets.

TCP observations become stale after 5 s by default. DNS and HTTPS freshness
limits include their slower cadence and deadline plus 1 s: 18 s and 34 s
respectively. An unavailable or canceled check is not a measured network
failure. Location availability does not affect connectivity status.

Only one system DNS worker exists, with a bounded mailbox. An uncancellable
`getaddrinfo` call can make DNS/HTTPS unavailable until it returns; it cannot
create an accumulating pool of stuck resolver threads. All probes have bounded
application deadlines and no application retry loops.

## Five-minute display

The graph shows the last 300 elapsed seconds and advances once per second,
including during an outage or while no observations are arriving. Missing
observations and explicit gaps are different from measured failures. Latencies
are connection timings, not ICMP round-trip measurements.

Ratatui sparklines draw the latency bars. Each terminal column shows the highest
observed TCP latency in its time bucket, preserving short spikes when the window
is compressed. Empty buckets stay blank. The row below uses `+` for success,
`x` for failure, `|` for a gap, and `.` for no observation. Failures take precedence
over gaps or successes sharing a column; the latency bar can still show a
successful probe in that same bucket. A zero-millisecond result remains visible
as `+` without inventing a positive latency.
The headline shows the highest fresh, successful TCP latency, or `--` when no
current successful TCP timing is available.

## Passive download and upload traffic

The traffic display reads macOS's 64-bit received and sent byte counters once
per second and shows their change over elapsed time in decimal Mbps. It measures
the selected interface's current traffic, including other applications and local
network transfers. It does not run downloads, estimate available bandwidth, or
attribute traffic to individual applications.

The display names the macOS primary interface being measured. Primary IPv4 is
preferred, with primary IPv6 as a fallback. Multiple active interfaces and VPN
routes can carry additional traffic elsewhere; these rates describe the named
interface rather than the sum of all routes on the Mac.

The first reading establishes a baseline. Switching interfaces, decreasing
counters, suspension, unavailable readings, or stale observations invalidate the
rate; a new valid sample pair is required. An observed zero rate is different
from an unknown rate. Raw counters and timestamps are retained with the ride.

The compact Down and Up sparklines cover the same five-minute window and retain
the highest rate in each display column. Each has its own labeled Mbps scale.
`_` marks measured zero, `.` missing data, and `|` an explicit gap without a
measurement. Changing the interface clears the live traffic plots so their
scope matches the displayed interface; earlier raw events remain recorded.

## Complete recordings

Recordings use SQLite with WAL journaling and `synchronous=FULL`. Raw events are
stored for the entire session, not just the visible window. The recording writer
owns its connection on a dedicated thread. Bounded message queues connect the
sampler, application, and writer; overload or persistence errors are visible and
stop monitoring instead of silently pretending that recording continues.

Ordinary shutdown cancels outstanding probes, drains completed observations, and
marks the recording ended when storage is writable. A crash or panic can lose
events still in bounded in-memory queues and leave the session open; already
committed SQLite events survive according to SQLite and the filesystem's
durability guarantees. A persistence failure stops monitoring and may also leave
an incomplete session. A session with no end timestamp was not closed normally.
Recordings are retained until you delete them yourself. Permanently blocked
terminal output can delay terminal-thread shutdown and restoration.

The database schema is version 1 (`PRAGMA user_version`):

- `sessions`: session UUID, optional label, start/end UTC, application version,
  and the complete probe/threshold configuration as JSON.
- `events`: ordered event ID, session UUID, UTC receipt/completion time,
  continuous elapsed milliseconds, observation generation, event type, and typed
  JSON payload. Probe payloads include target, protocol, start elapsed time,
  duration, and outcome. Location payloads retain coordinates, reported accuracy,
  and the source fix timestamp separately from receipt time.

Traffic events (`event_type = 'traffic'`) retain the interface name/index and
last-change identity, 64-bit received/sent byte totals, query start/completion time, and a typed rate
state. Valid rates include download/upload Mbps and the interval used to derive
them. Baselines, interface changes, resets, gaps, late/out-of-order readings, and
source errors remain explicit instead of being stored as zero traffic. Readings
have a 500 ms deadline and valid rates become stale after 2.5 s.

`event_type` is `probe`, `gap`, `location`, `traffic`, or `clock_adjusted`. JSON payloads use
an `event` discriminator and a `data` object. Probe outcomes use `type` and, when
needed, `detail`; for example `success`, `timeout`, `refused`, `network_error`,
`dns_error`, `http_status`, `tls_or_http_error`, `unavailable`, or `cancelled`.
Location state is `fix`, `pending`, `denied`, or `unavailable`. A gap records its
start and reason; a clock adjustment records its signed UTC change in milliseconds.

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
minutes old with about 60 m reported accuracy. The packaged Rust application also
received a real fix, about 176 s old with 40 m reported accuracy, followed by a
temporarily unavailable state. This is why the application keeps source time and
receipt time separate. Mac location depends on the environment; continuous useful
positioning on a moving train is not guaranteed.

Elapsed time on macOS uses `mach_continuous_time`, which includes sleep. A detected
suspension (continuous versus awake time, with 50 ms tolerance) or a long scheduling
delay records a gap and starts a new observation generation. Late results from
earlier generations cannot restore current health. UTC clock adjustments are
recorded independently so future analysis can account for wall-clock jumps.
Sleep behavior has deterministic test coverage; physical sleep/resume on this Mac
has not been exercised.

Apple references: [location usage description](https://developer.apple.com/documentation/bundleresources/information-property-list/nslocationusagedescription),
[single-file executable metadata](https://developer.apple.com/library/archive/documentation/Security/Conceptual/CodeSigningGuide/Procedures/Procedures.html#//apple_ref/doc/uid/TP40005929-CH5-SW14),
[permission attribution and launch context](https://developer.apple.com/forums/thread/732431),
[Mac location](https://support.apple.com/en-gb/guide/mac-help/mh27621/mac).

## Validation and resource use

See [VALIDATION.md](VALIDATION.md) for the release measurements, exact verification
commands, terminal cleanup results, and limits of the evidence. Resource targets
are below 1% of one CPU core, 50 MiB resident memory, and 10 MB/hour of application
probe traffic. Traffic estimates describe probe cost, not available bandwidth.

On this Mac, two 315 s release runs with passive traffic and sparklines measured
0.114% CPU / 16.66 MiB maximum RSS on the current `en0` network and 0.104% /
14.73 MiB under controlled timeouts. Filtered captures estimated 2.78 and
2.42 MB/hour including modeled Ethernet overhead. These short estimates include
attribution limits and do not measure Wi-Fi or mobile-radio overhead, battery
life, or every possible network failure.
