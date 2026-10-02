# Validation

Validated on 2026-10-02 with network-monitor 0.1.0, macOS 26.6.2, arm64
Mac16,8, and Rust 1.96.0. The declared minimum Rust version is 1.88; the full
suite was run with 1.96.0. Measurements use the packaged release executable:

```text
/Users/oliver.mueller/dev/network_monitor/target/Network Monitor.app/Contents/MacOS/network-monitor
```

The measured packaged executable's SHA-256 is
`92aef8d00f2bce3abb63cb631414f3769a6457033340f580774baf27e1364f5e`.
The plain release binary differs because packaging applies an ad-hoc signature.

## Build and behavioral checks

From the repository root:

```sh
cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
./scripts/package-app.sh
```

All passed: 43 tests, formatting, Clippy without warnings, release build, and
ad-hoc package-signature verification. The release packaging build took 8.32 s
in the final recorded gate. The source checkpoint for these gates is `22ff8bf`;
documentation and validation-script updates follow that checkpoint.

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

Local gate logs: `/tmp/nm-final-validation/cargo-test.log`,
`/tmp/nm-final-validation/release-package.log`, and
`/tmp/nm-wp02-fix/{fmt,clippy}.log`.

## Release resource measurements

Both runs met the measured CPU and RSS targets of below 1% of one core and
below 50 MiB. The selected-filter traffic estimates were also below 10 MB/hour;
their attribution and overhead limits are detailed below.

| Metric | Healthy hotspot | Controlled timeouts |
| --- | ---: | ---: |
| Active run | 315.245 s | 315.313 s |
| Steady resource window | 304.649 s | 304.727 s |
| Mean CPU, one core | 0.0761% | 0.0630% |
| Mean / maximum RSS | 16.17 / 16.69 MiB | 14.47 / 14.97 MiB |
| Maximum footprint | 5.14 MiB | 4.81 MiB |
| Interrupt wakeups | 7.14/s | 7.93/s |
| Package-idle wakeups | 0/s | 0/s |

The healthy recording contains 316 successful TCP, 22 successful DNS, and 11
successful HTTPS checks. Its single session ended normally with 355 events;
23 events predate the final five-minute display window and remain in SQLite.
The final display showed 5m15s elapsed and `HEALTHY`. TCP start intervals ranged
from 2000 to 2005 ms during this run.

The controlled failure recording contains 316 TCP timeouts (mean 1501.91 ms),
11 HTTPS timeouts (mean 3001.55 ms), and 22 successful localhost DNS checks.
Its single session also ended normally with 355 events, including 23 older than
the final display window. The final display showed `OFFLINE` at 5m15s, with
failure marks across the window. TCP start intervals ranged from 2000 to 2004 ms.
Both runs exited with code 0 and restored the terminal after `q` in about 0.51 s.

Local results are in `/tmp/nm-final-validation/{healthy,failing}/analysis.json`,
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
host scheduling. CoreLocation's shared OS service cost is not isolated by this
measurement. Wakeup counts are reported; CPU and wakeups alone do not establish
battery-life impact.

The healthy run uses default probe settings on the current `en0` connection,
through hotspot gateway `172.20.10.1`. The controlled failure run uses separate
IPv4 and IPv6 loopback listeners with full listen queues: both TCP probes time
out at 1.5 s and HTTPS at 3 s; DNS resolves `localhost`. It exercises bounded
timeout behavior without changing the active network. It does not reproduce
every failure mode of a phone or mobile network.

### Traffic accounting

| Metric | Healthy hotspot | Controlled timeouts |
| --- | ---: | ---: |
| Captured packets | 2,795 | 2,017 |
| Original captured frame bytes | 247,350 | 143,300 |
| Captured-frame estimate | 2.825 MB/hour | 1.636 MB/hour |
| Ethernet-equivalent estimate | 3.591 MB/hour | 2.419 MB/hour |

The healthy capture reported zero kernel drops. Its first-to-last packet span
was 316.122 s; the hourly denominator is the 315.245 s active run. Per-PID nettop
provided 310 samples, reaching byte counters of 6,985 inbound and 2,445 outbound.
These process counters and filtered capture totals have different scope.
The capture includes 687 DNS packets totaling 90,408 bytes from the system-wide
DNS filter. Independent packet inspection matched 158 initial TCP SYNs per
default TCP target to the 316 recorded successes and confirmed captured HTTPS
traffic to the resolved gstatic IPv6 endpoint.

The loopback capture also reported zero kernel drops, with a first-to-last packet
span of 315.340 s and an active-run denominator of 315.313 s. All 310 nettop
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

The five orderly exits took 0.44-0.51 s in these short cleanup runs. They created
five distinct sessions, all with an end timestamp. Terminal comparison ignores
only macOS's transient `PENDIN` input-retype flag and compares all substantive
attributes. Both storage failures emitted a visible diagnostic. The runtime
failure used a trigger in a disposable test database that fails the next event
insert; four events remained committed and the session had no end timestamp, as
expected for failed persistence. No user recording was modified.

A crash or panic can lose queued observations and leave a session open. A
permanently blocked stdout can delay terminal-thread joining and restoration.
Those cases are not covered by the orderly-shutdown claims.

Local evidence: `/tmp/nm-final-validation/cleanup-results.json`,
`/tmp/nm-final-validation/quit-*`, and the `storage-open-failure` and
`runtime-storage-failure` directories. Exact cleanup orchestration is preserved
locally in `/tmp/nm-run-cleanup.py`.

## Location evidence

The user approved the macOS location prompt for the bundled executable. An early
native feasibility run returned a fix about 503 s old with roughly 60 m accuracy.
A packaged Rust run subsequently returned a fix about 176 s old with 40 m reported
accuracy, then an unavailable state. Each final measurement received two real
fixes. Stored source and receipt timestamps were checked separately. These
observations establish permission and real location
ingress on this Mac; they do not establish useful continuous positioning on a
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
`/tmp/nm-resource-run.py` and `/tmp/nm-analyze-validation.py`.

Local artifacts under `/tmp` are evidence for this run, not repository fixtures
or permanent archives. The committed harnesses and commands support rerunning
the checks; results can vary with hardware, network, OS services, and terminal.
