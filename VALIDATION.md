# Validation

Validated on 2026-10-02 with network-monitor 0.1.0, macOS 26.6.2, arm64
Mac16,8, and Rust 1.96.0. The declared minimum Rust version is 1.88; the full
suite was run with 1.96.0. Measurements use the packaged release executable:

```text
/Users/oliver.mueller/dev/network_monitor/target/Network Monitor.app/Contents/MacOS/network-monitor
```

The measured packaged executable's SHA-256 is
`a8f91a5b955c4c0cdc12b3a4fb7b5028232ea813aaf7e820d62cee098d4a8c63`.
The plain release binary differs because packaging applies an ad-hoc signature.

## Build and behavioral checks

From the repository root:

```sh
cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
./scripts/package-app.sh
```

All passed: 56 tests, formatting, Clippy without warnings, release build, and
ad-hoc package-signature verification. The release packaging build took 8.84 s
in the final recorded gate. These gates cover the passive-traffic and Sparkline
follow-up to `e3b48f2`.

Coverage includes healthy, slow, intermittent, offline, single-target failure,
DNS/HTTPS failure, recovery, explicit missing observations, bounded overload,
full-session persistence, distinct launches, and unavailable/denied/old location.
Test fixtures verify terminal errors and full writer queues drain completed
observations on ordinary shutdown. Persistence errors remain visible.

The default timing budget is 2 s TCP cadence + 1.5 s deadline + up to 1 s
application refresh + up to 0.5 s terminal poll, approximately 5 s with normal
scheduling and output. A deterministic test observes offline status at 4 s after
the last success. A separate regression sweeps delayed starts at 2001, 2251,
2501, 2751, and 2999 ms and exercises both display phases, asserting visible
outage within 5 s. This is controlled scheduler evidence, not a real-world
hard timing guarantee.

Sleep tests advance continuous and awake clocks separately, including a short
suspend below the normal stale threshold and a display-before-sampler race.
They verify a new generation/gap and reject late success from the old generation.
Wall-clock adjustments are tested separately. Physical sleep/resume was not run.

Local gate logs: `/tmp/nm-wp04-validation/cargo-test-all-targets.log`,
`/tmp/nm-wp04-validation/release-package.log`, and
`/tmp/nm-wp04/{fmt-final,clippy-final}.txt`.

New traffic tests cover exact byte-delta rate arithmetic, idle zero versus missing
data, interface/reset/unavailable invalidation, sleep-spanning and delayed raw
results, backward and long intervals, bounded worker saturation, expired queued
requests, full-session persistence, interface-scoped history, and peak retention.
UI tests cover 80-column headlines, narrow Sparkline peak buckets, failure/gap
precedence, zero latency, and stale readings. The native counter source also
passes `clang -fsyntax-only -Wall -Wextra -Werror native/traffic.c`.

## Passive counter cross-check

A separate local C reader queried the macOS primary interface and 64-bit native
byte counters once per second while the packaged application recorded ordinary
traffic. Sixty-four application observations were independently bracketed by the
reader's timestamps and monotonic received/sent counter values on `en0` (index
14). The totals exceeded 4 GiB, exercising the native 64-bit path. No test
downloads or uploads were generated.

The recording analysis recalculates each valid Mbps rate from its adjacent raw
counter totals and recorded elapsed interval. It checks interface identity and
generation continuity and rejects negative deltas. Deterministic tests cover
counter resets, route/interface changes, invalid sources, delayed results and
sleep; the live run does not replace those controlled cases.

Cross-check artifacts and aggregation are local to `/tmp/nm-wp04-validation`:
`external-counters.txt`, `counter-validation.c`, `check-traffic.py`, and each
run's `traffic-validation.json`. These whole-interface rates have a different
scope from the filtered application probe-cost estimates below.

## Release resource measurements

Both runs met the measured CPU and RSS targets of below 1% of one core and
below 50 MiB. The selected-filter traffic estimates were also below 10 MB/hour;
their attribution and overhead limits are detailed below.

| Metric | Default probes on en0 | Controlled timeouts |
| --- | ---: | ---: |
| Active run | 315.384 s | 315.268 s |
| Steady resource window | 304.784 s | 304.670 s |
| Mean CPU, one core | 0.1142% | 0.1043% |
| Mean / maximum RSS | 16.19 / 16.66 MiB | 14.06 / 14.73 MiB |
| Maximum footprint | 5.47 MiB | 5.30 MiB |
| Interrupt wakeups | 10.21/s | 10.79/s |
| Package-idle wakeups | 0/s | 0.01/s |

The healthy recording contains 316 successful TCP, 22 successful DNS, and 11
successful HTTPS checks, plus 316 passive counter events (315 valid rates and
one initial baseline). Its single session ended normally with 670 events;
38 events predate the final five-minute display window and remain in SQLite.
The final display showed 5m15s elapsed and `HEALTHY`. TCP start intervals ranged
from 2000 to 2004 ms; valid traffic intervals ranged from 1000 to 1004 ms. Native
counter queries completed in 0-1 ms at the recorded millisecond resolution.
All 315 rate calculations matched their raw byte deltas.

The controlled failure recording contains 314 completed TCP timeouts (mean
1501.83 ms), 11 HTTPS timeouts (mean 3001.45 ms), and 22 successful localhost DNS
checks. Its single session ended normally with 667 events, including 38 older
than the final display window. The final display showed `OFFLINE` at 5m15s,
with failure marks across the window. TCP start intervals ranged from 2000 to
2004 ms. Outstanding checks are canceled at ordinary shutdown.

Passive traffic remained independent of those failures: 316 counter events,
one baseline and 315 valid rates, all arithmetically verified. Intervals ranged
from 1000 to 1003 ms and native query durations from 0 to 2 ms. Both runs exited
with code 0 and restored the terminal after `q` in about 0.51-0.52 s.

Local results are in `/tmp/nm-wp04-validation/{healthy,failing}/analysis.json`,
with raw resource samples in each `usage.json`. Both test recordings remain in
the parent directory. All application, harness, fixture, and capture processes
owned by this validation were stopped after completion.

Both runs use a real controlling PTY. The harness waits for the first rendered
`Recording` view before starting the active-run clock, then excludes the first
10 s from resource measurement. This prevents pre-execution macOS validation
delay from diluting the result. CPU, resident memory, footprint, and wakeups are
sampled for the application PID using `proc_pid_rusage` from libproc. CPU is the
change in user plus system CPU time divided by elapsed time, as a percentage of
one core. On this Mac the CPU counters require the Mach timebase conversion
125/3; a 2 s busy-loop calibration against Python `process_time` gave a ratio of
0.999897 after conversion.

The application has one process, including the native location integration.
The PTY wrapper, timeout fixture, packet capture, and nettop are test tools and
are excluded from application PID resources. Their presence may still affect
host scheduling. Shared OS service costs for CoreLocation and SystemConfiguration
are not isolated by this measurement. Wakeup counts are reported; CPU and wakeups
alone do not establish battery-life impact.

The healthy run uses default probe settings on the current `en0` connection,
through gateway `192.168.0.10`. This run used the current local network; the
earlier pre-traffic baseline used a phone hotspot. The controlled failure run uses
separate IPv4 and IPv6 loopback listeners with full listen queues: both TCP probes time
out at 1.5 s and HTTPS at 3 s; DNS resolves `localhost`. It exercises bounded
timeout behavior without changing the active network. It does not reproduce
every failure mode of a phone or mobile network.

### Traffic accounting

| Metric | Default probes on en0 | Controlled timeouts |
| --- | ---: | ---: |
| Captured packets | 2,349 | 2,017 |
| Original captured frame bytes | 186,729 | 143,300 |
| Captured-frame estimate | 2.131 MB/hour | 1.636 MB/hour |
| Ethernet-equivalent estimate | 2.776 MB/hour | 2.419 MB/hour |

The healthy capture reported zero kernel drops. Its first-to-last packet span
was 315.912 s; the hourly denominator is the 315.384 s active run. Per-PID nettop
provided 310 samples, reaching byte counters of 6,984 inbound and 1,701 outbound.
These process counters and filtered capture totals have different scope.

The loopback capture also reported zero kernel drops, with a first-to-last packet
span of 315.413 s and an active-run denominator of 315.268 s. All 310 nettop
samples reported zero byte counters while pcap still recorded connection attempts
and retransmissions. This illustrates why application byte counters alone are
insufficient for protocol-overhead accounting.

The healthy capture is non-promiscuous on `en0`, filtered to TCP port 443 for the
two TCP endpoints and the HTTPS addresses resolved immediately before the run,
plus all DNS port 53 traffic. Failure traffic is captured on `lo0`, filtered to
the two fixture ports. Original packet lengths from pcap records are summed,
even when the stored packet data is truncated to the 256-byte snapshot limit.
Per-PID nettop counters provide a separate attribution check.

Packet capture surrounds the whole launch, including warmup and shutdown. The
hourly estimate divides those captured bytes by the actual active-run duration,
not the shorter resource sampling window. Extra captured startup/shutdown traffic
therefore contributes to the estimate.

The capture can include another process using those endpoints and unrelated DNS;
the HTTPS address set can change after the initial resolution. Resolver and
CoreLocation service traffic cannot be fully attributed to this PID. The packet
capture does not measure Wi-Fi radio overhead, link retries, or hotspot/mobile
carrier overhead. Loopback framing is not a physical network measurement.
An Ethernet-equivalent calculation replaces the 4-byte loopback pseudoheader
with a 14-byte Ethernet header where needed, then includes a minimum 60-byte
frame plus 24 bytes for FCS, preamble/start delimiter, and inter-frame gap. It is
an explicit wire-cost model, not a Wi-Fi measurement. Decimal MB/hour is
extrapolated from a short run and must not be interpreted as a guaranteed hourly
maximum or available bandwidth.

## Terminal, shutdown, and storage failures

The packaged executable ran in a real PTY, with ANSI output consumed by pyte and
rendered to a PNG for visual inspection. The normal full view, failed probes,
five-minute history, and resize from 120x34 to 80x24 were inspected. An actual
native iTerm window was not inspected; the evidence is PTY output plus the
emulator rendering.

| Case | Exit code | Terminal attributes restored | Left alternate screen |
| --- | ---: | --- | --- |
| `q`, including resize | 0 | Yes | Yes |
| Ctrl-C | 0 | Yes | Yes |
| SIGINT | 0 | Yes | Yes |
| SIGTERM | 0 | Yes | Yes |
| SIGHUP | 0 | Yes | Yes |
| Database path is a directory | 1 | Yes | Yes |
| Runtime SQLite insert failure | 1 | Yes | Yes |

The five orderly exits took 0.44-0.52 s in these short cleanup runs. They created
five distinct sessions, all with an end timestamp. Terminal comparison ignores
only macOS's transient `PENDIN` input-retype flag and compares all substantive
attributes. Both storage failures emitted a visible diagnostic. The runtime
failure used a trigger in a disposable test database that fails the next event
insert; already committed events remained and the session had no end timestamp,
as expected for failed persistence. No user recording was modified.

A crash or panic can lose queued observations and leave a session open. A
permanently blocked stdout can delay terminal-thread joining and restoration.
Those cases are not covered by the orderly-shutdown claims.

Local evidence: `/tmp/nm-wp04-validation/cleanup-results.json`,
`/tmp/nm-wp04-validation/quit-*`, and the `storage-open-failure` and
`runtime-storage-failure` directories. Exact cleanup orchestration is preserved
locally in `/tmp/nm-wp04-validation/run-cleanup.py`.

## Location evidence

The user approved the macOS location prompt for the bundled executable. An early
native feasibility run returned a fix about 503 s old with roughly 60 m accuracy.
A packaged Rust run subsequently returned a fix about 176 s old with 40 m reported
accuracy, then an unavailable state. Each final measurement received one real
fix. Stored source and receipt timestamps were checked separately. These
observations establish permission and real location ingress on this Mac; they do not establish useful continuous positioning on a
moving train. No coordinates or raw recordings are committed.

## Reproduce

The resource/terminal harness requires Python 3, pyte, and Pillow. Run from the
repository root with a fresh local output directory and disposable database:

```sh
python3 scripts/verify_terminal.py --duration 315 --output /tmp/nm-healthy -- \
  "target/Network Monitor.app/Contents/MacOS/network-monitor" \
  --db /tmp/nm-healthy.sqlite3 --label "Healthy validation"
```

To repeat bounded timeouts, leave this fixture running in a second terminal:

```sh
python3 scripts/timeout_targets.py /tmp/nm-timeouts.toml
```

Then run the same release binary with that configuration, and stop the fixture
with Ctrl-C after the measurement:

```sh
python3 scripts/verify_terminal.py --duration 315 --output /tmp/nm-failing -- \
  "target/Network Monitor.app/Contents/MacOS/network-monitor" \
  --config /tmp/nm-timeouts.toml --db /tmp/nm-failing.sqlite3 \
  --label "Timeout validation"
```

Use `--resize` and `--quit q`, `--quit ctrl-c`, `--quit SIGINT`,
`--quit SIGTERM`, or `--quit SIGHUP` with shorter runs to repeat terminal checks.
`usage.json` contains resource summaries and raw samples; `terminal-state.json`,
`screen.txt`, `screen.png`, and `terminal.bin` contain terminal evidence. Screen
and recording artifacts may include locations; keep them local and uncommitted.

To reproduce storage-open failure, pass an existing directory as `--db`. For
runtime failure, launch with a new disposable database; after the recording
appears, run this SQL against that test database in another terminal:

```sql
CREATE TRIGGER injected_failure BEFORE INSERT ON events
BEGIN SELECT RAISE(FAIL, 'injected recording failure'); END;
```

The next event insert should stop the application with exit code 1 and a visible
error, leaving the test session incomplete. Use a fresh database for subsequent
runs so that this deliberately failing trigger cannot affect other recordings.

For traffic attribution, read the PID in the output directory's `active.json`
and sample it while the run is active:

```sh
nettop -n -p APP_PID -P -x -J bytes_in,bytes_out -L 310 -s 1
```

Capture separately with `tcpdump -p -i INTERFACE -s 256 -U -w traffic.pcap FILTER`,
using the interface and endpoint/port filters described above and available local
capture permissions. Stop it with SIGINT to flush its output. Sum original pcap
record lengths, not captured snapshot bytes. The exact run filters are in each
local `capture-filter.json`; orchestration and aggregation are retained in
`/tmp/nm-wp04-validation/resource-run.py` and
`/tmp/nm-wp04-validation/analyze-validation.py`.

Local artifacts under `/tmp` are evidence for this run, not repository fixtures
or permanent archives. The committed harnesses and commands support rerunning
the checks; results can vary with hardware, network, OS services, and terminal.
