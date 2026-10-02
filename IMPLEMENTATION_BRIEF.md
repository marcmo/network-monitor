# Network monitor implementation brief

Implement a lightweight Rust CLI application with a live TUI for monitoring a
MacBook's internet connectivity during train rides over a phone hotspot. The
user confirmed the design below on 2026-10-02 and requested this implementation
handoff. Proceed to implementation; the design interview is complete.

Work in `/Users/oliver.mueller/dev/network_monitor`. This directory contained no
application files when this brief was prepared. Inspect its current contents and
applicable repository instructions before making changes; preserve any work
added since this handoff.

## Confirmed behavior

- Optimize the information shown for browsing, SSH/remote development, and agent
  usage. Measure general internet quality, rather than availability of a
  particular SSH host or agent provider.
- Monitor the active connection used by the Mac. The intended connection is a
  phone hotspot; comparison of alternative networks is outside the first version.
- Run monitoring only while the TUI is open. No daemon or automatic login launch.
- Show both an interpreted current status and the underlying measurements.
- Show a continuously advancing, live graph of the last five minutes. Refresh
  the display once per second. Include latency and failed probes/outages, and
  distinguish measured failures from intervals with no observations.
- Show measurement freshness explicitly. Startup, sleep, unavailable probes, and
  stale measurements must not leave an apparently current healthy status.
- Target detection of an internet outage within roughly five seconds.
- Use small probes to independent destinations plus DNS/HTTPS checks. A single
  destination failure must not automatically mean that the internet is offline.
- Do not run automatic speed tests or claim available bandwidth from small probes.
- Keep quality thresholds configurable and explain the chosen defaults.
- Start a full recording automatically on each launch, with an optional ride
  label. Restarting the application creates a new recording. Sleep produces an
  explicit observation gap within the existing recording.
- Persist complete rides locally until the user deletes them. Five minutes is
  the display window, not the retention limit.
- Attempt best-effort macOS location capture in the first version. Record each
  fix's coordinates, source timestamp, and reported accuracy. Continue monitoring
  if location is denied, unavailable, inaccurate, or temporarily missing.
- Preserve data suitable for later geographic correlation across rides. Maps,
  route matching, prediction, and phone GPS import are future work.

## Resource targets

Verify these targets with a release build on the user's Mac:

- Average CPU below 1% of one CPU core during steady monitoring.
- Resident memory below 50 MiB.
- Application probe traffic below 10 MB per hour, including protocol overhead
  where measurable.

Document the measurement method, duration, connection state, and limitations.
Measure healthy and failing-network behavior: retries and timeouts must remain
bounded. Account for any helper process introduced. A short measurement
extrapolated to an hour must be labeled as an estimate. CPU alone does not prove
low battery impact; observe wakeups or energy impact where practical.

## Implementation decisions to make

The user chose the behavior, not a dependency list or storage format. Choose
simple maintained libraries and document decisions. Rust with Tokio is required
by the user's coding conventions. A Rust TUI library and local SQLite storage
are reasonable candidates, not previously selected requirements.

1. Select probe protocols, independent targets, intervals, deadlines, and status
   rules that support the five-second outage target within the resource budgets.
   Define and test detection timing end to end, including scheduling and timeout
   delays. Make the scope and refresh rate of each check visible and documented.
2. Distinguish reachability, DNS resolution, and HTTPS success. ICMP may be
   filtered even when browsing works. Do not call TCP/HTTPS probe failure rates
   packet loss. A partial check failure should remain distinguishable from total
   outage. Follow normal system routing and explain any proxy limitations.
3. Keep sampling independent of rendering and persistence. Use bounded tasks,
   queues, and buffers, with cancellation and backpressure appropriate to a small
   application. Avoid subprocesses per sample and busy polling.
4. Store timestamped raw observations, not only derived quality scores. Include
   session identity, optional label, probe target/type, duration, typed outcome,
   and the configuration/version needed to interpret measurements later.
5. Use UTC timestamps for future joins and appropriate monotonic clocks for
   elapsed measurements. Handle sleep/resume and wall-clock changes explicitly.
   Preserve location fix times separately from receipt times. Do not silently
   attach an old location fix to new measurements as if it were current.
6. Keep the in-memory graph window bounded while recordings grow on disk. Make
   storage errors visible. Preserve completed observations on ordinary shutdown
   and document any buffering or crash-loss limits. Keep stored data accessible
   for later analysis through a documented schema or simple export.
7. Restore the terminal reliably on exit and errors. Support resize, readable
   status text alongside colors, and clean cancellation of outstanding work.

## macOS location feasibility

Validate permission attribution from the user's actual terminal early. Research
has not established that a separate helper is necessary. Start with the smallest
viable implementation; a single executable with embedded application metadata
is a candidate, not a proven solution. If packaging in a minimal `.app` is needed,
retain a CLI launch experience and document setup.

Apple documents a macOS location usage description and embedding an Info.plist
in a single-file executable. That does not prove a specific Rust executable will
obtain permission correctly in every terminal launch context:

- [macOS location usage description](https://developer.apple.com/documentation/bundleresources/information-property-list/nslocationusagedescription)
- [Single-file executable metadata](https://developer.apple.com/library/archive/documentation/Security/Conceptual/CodeSigningGuide/Procedures/Procedures.html#//apple_ref/doc/uid/TP40005929-CH5-SW14)
- [Apple discussion of launch context and location permissions](https://developer.apple.com/forums/thread/732431)
- [Existing CoreLocationCLI source and packaging](https://github.com/fulldecent/corelocationcli)

Mac location is approximated from nearby Wi-Fi networks, so useful continuous
train positioning is uncertain until tested. Store fix quality and gaps honestly;
do not substitute IP geolocation or invented positions.

- [Apple explanation of Mac location](https://support.apple.com/en-gb/guide/mac-help/mh27621/mac)

If a macOS permission dialog needs the user's interaction, explain what it is
for and continue independent implementation work. Record what was actually
verified; a mocked location test does not establish real permission behavior.

## Execution and acceptance

Use test-driven development for new behavior, with deterministic clocks and
controllable probe/location sources where appropriate. Implement vertical slices
through sampling, storage, and the TUI, then harden failure behavior.

Before declaring completion:

- Run meaningful tests covering healthy, slow, intermittent, and offline
  scenarios; single-target failures; DNS/HTTPS failures; and recovery.
- Verify the approximate five-second outage target and ensure canceled or late
  results cannot incorrectly restore a healthy status.
- Verify five-minute rolling history advances during outages, and sleep/resume
  and missing observations appear as gaps instead of fabricated measurements.
- Verify full-ride persistence, launch boundaries, optional labels, timestamps,
  location accuracy/age, and denied or unavailable location behavior.
- Exercise storage failures and shutdown/terminal cleanup. Inspect the running
  TUI in a real terminal, including resize and quit behavior.
- Run formatting, type/build checks, tests, and Clippy. Fix warnings rather than
  suppressing them. Report exact commands and actual results.
- Measure resource consumption in release mode and report any unmet target or
  unverified limitation explicitly. Do not disrupt the user's active network to
  simulate outages; use controlled tests unless separately authorized.
- Provide installation/run instructions, configuration and status semantics,
  storage location, location-permission setup, and the exact launch command.

Deliver working code and a concise report of what works, how it was verified,
and remaining limitations. Resolve routine implementation choices autonomously;
ask only for material scope changes or unavoidable user interaction.

## Approved follow-up: passive traffic and at-a-glance latency (2026-10-02)

Use Ratatui's Sparkline widget for live history. Make the current latency and
current download/upload rates easy to read at a glance, with recent history.
The user explicitly selected passive current download/upload traffic, not
available capacity or an active speed test. Read macOS interface counters for
the active connection without test downloads or subprocesses per sample.
Show the measured interface and label rates clearly. Preserve the existing
five-minute view, full-ride recordings, bounded owned state and message passing.
Interface changes, counter resets, unavailable counters and sleep must not
create fabricated rates or leave stale values apparently current. Record raw
counters/timestamps and rate validity so later analysis can interpret readings.
These are traffic rates for the selected interface, not bandwidth capacity or
per-application attribution. Keep measurement/rendering/storage independent and
verify deterministic rate arithmetic, invalidation, persistence and terminal UI.

## User coding and Git rules

- Favor safety and correctness, typed errors with meaningful messages, enums,
  traits, and generics. Use Tokio for async work, channels for message passing,
  and actors where interaction complexity warrants them.
- Avoid unnecessary allocation/cloning in hot paths. No `unwrap` or `expect` in
  production code and no match fall-through behavior.
- Code and comments must be ASCII. Comments explain non-obvious reasons.
- Add meaningful unit/integration/property tests as appropriate; use TDD for new
  functionality. Tests and type checks must pass before claiming completion.
- Never commit secrets, credentials, or `.env` files.
- Never push without explicit user confirmation. If creating a branch, use
  `<type>/omueller/<topic>` and never include `claude` or `codex` in its name.
  Rename any such inherited branch before a later authorized push.
- If making commits, use `<type>(<scope>): <imperative summary>` with a summary
  of at most 72 characters. Scope names the affected crate/module. Use a body
  only when needed for reasoning, with bullets for multiple changes. Allowed
  types: feat, fix, refactor, perf, docs, test, build, ci, chore.
- Never mention AI tools, assistance, or co-authors in commits or PRs.

## Approved presentation: graph with sidebar (2026-10-02)

The user selected sketch 3, "Graph with sidebar", after reviewing three layout
alternatives. Make current latency and connection quality legible at a glance.
The primary screen has a large five-minute TCP latency Sparkline on the left and
an always-visible fixed sidebar on the right: large numeric latency with ms,
freshness, a colored and textual quality state with a short factual reason, and
current passive Down/Up Mbps with the measured interface. Keep the ride label
and recording duration in a quiet header/footer. Hide raw targets, cadence,
location, recording identifiers/path and traffic history under a Details view,
toggled by d; Escape returns to the primary screen. Preserve q/Ctrl-C/signals.

Use the existing status and latency semantics: highest fresh successful TCP
latency, explicit unknown when none exists, separate partial/slow/offline/stale
states, and traffic activity independent of connectivity. Never invent scores,
capacity estimates or a stale zero. Details retain the existing diagnostic and
traffic-history information. The selected sketch is a layout reference, not
live data or a new measurement specification.

Keep sampling, recording and timing unchanged. Own presentation state on the
terminal thread; do not add shared state or move UI choices into the data actor.
Both screens must work at 80x24, preserve untrusted-text sanitization, and handle
small terminals, long labels and large/unknown values without hiding the key
reading, status, recording duration or quit controls. Test through the existing
rendering and real-terminal seams, including all quality/freshness states and
Details toggling. Run full tests, format, Clippy, release packaging, and targeted
PTY checks; do not repeat the previous five-minute resource measurements unless
an observed regression requires it.
