# Display

Reachable systems show five rows when their own snapshot has no network
telemetry, and six rows when it does. In a mixed fleet, online systems may
therefore have different base heights. All active
metric rows share
the same fleet-wide `bar_width` so the opening `[` and closing `]` columns
always align across every online system, and the metric rows are indented by
exactly four spaces:

```text
Deadpool · Ubuntu 24.04 x86_64 · Linux 6.8  IO 0.4%  L(8) 1.32/.91/.62
    CPU  [||||||||||||                                  ] 25.2% 8 cores
    MEM  [||||||||||||||||||                            ] 37.8% 5.9 GiB / 15.6 GiB
    SWP  [                                                ]  0.0% 0 B / 4.0 GiB
    DISK [||||||||||||                                  ] 25.0% 238.0 GiB / 952.0 GiB
    NET  [||||||||||||                                  ] 31.0% 39.0 MiB/s rx 5.0 MiB/s tx
```

The NET row appears only for systems whose own snapshot has network telemetry;
legacy systems omit it rather than showing an unavailable bar. Zero traffic
and unknown capacity remain valid NET telemetry, so those systems still show
the row. CPU detail appends the
current OS-reported clock after the core count when available, for example
`16 cores 2.40GHz`; this is not a base/max frequency claim, and macOS may
omit it.

The DISK suffix is `<used bytes> / <total bytes>` so the slash denominator
matches the percentage calculation; explicit caller-available capacity is
preserved by the normalized model and surfaced only through the expanded
per-drive rows. On Windows, the third row uses `COMMIT` (memory commit
charge) instead of `SWP`. Unreachable rows render `—` instead of fabricating
a `0.0%`. A host with no swap at all counts as unreachable rather than as
measurably empty: Linux reports swap with a zero total when there is none, and
that is an absent measurement, not a measured `0%`.

When the longest natural metric suffix across the entire online fleet exceeds
one quarter of the terminal width, every normal-view metric row collapses to
bar-only — the bars remain aligned, but the percentage, core counts, and byte
counts disappear until the terminal widens again. Resizing wider restores
them dynamically with no restart.

The header line omits the `IO` token entirely when CPU I/O wait is
unsupported (macOS) or no real value is available, rather than rendering a
placeholder; the remaining fields keep their normal separators.

Unreachable systems collapse to one row. With a configured nickname:

```text
deadpool@192.168.1.10:11310 offline
```

Without a nickname the host is rendered once:

```text
192.168.1.10:11310 offline
```

When the accepted poll failure carries provenance, the stable failure
category is appended inside the existing width budget:

```text
deadpool@192.168.1.10:11310 offline (refused)
192.168.1.10:11310 offline (http) HTTP 503
```

Pending systems (never polled) never carry a reason.

Press `d` to expand drive details. With live disk-I/O data, the expansion adds
`R/s` and `W/s` columns plus a daemon-supplied `I/O TOTAL` line. Per-drive
rates appear only for an exact, unambiguous drive/device association;
ambiguous values render `—`, and the aggregate is not recomputed from rows.

Press `n` to independently expand network details. The expansion shows an
aggregate Rx/s/Tx/s/capacity summary followed by interfaces, including
loopback when supplied. `Rx/s` and `Tx/s` are byte rates. Unknown capacity
leaves utilization unavailable while preserving raw rates; utilization uses
the maximum valid direction, so simultaneous full-duplex traffic does not
exceed 100% merely by combining Rx and Tx. Loopback is detail-only for
aggregate capacity.

Press `c` to independently expand cron details for the selected system. The
block lists every job the remote `greggd` schedules, with each job's state,
schedule, last result, and — for the job selected with `Shift-J` / `Shift-K` —
its recent run history with output. The three expansions are independent and
share one vertical budget, so cron takes whatever `d` and `n` did not claim.

Load-gated jobs are shown honestly. A load-delayed job states how long it has
been waiting, the load window, the observed load, the configured threshold, and
the retry time where the width allows. A **missing** load observation renders
`—`, never `0.00`: a gate that fired because telemetry was unavailable must not
read as a gate that observed a low load. A run that never started a child is
still listed as a terminal record, with no fabricated duration.

Scheduler failure is never rendered as system failure. A remote that does not
serve the scheduler routes is reported as *unsupported* — the normal state for
an older `greggd` — and a transient read failure keeps the last known data with
a stale marker. The metrics plane alone decides online/offline.

Remote command output is escaped, not stripped, using `cat -v` caret notation:
`ESC` becomes the two printable characters `^[`, so `ESC [ 2 J` displays as
`^[[2J` rather than clearing your screen, and an OSC title or hyperlink sequence
cannot retitle your terminal or plant a link in a monitoring display. Remote
truncation is marked separately from this pane's own row budget, because "the
remote kept five records" and "this pane can show two" are different facts.

Condensed view shows one comparison row per system with CPU, memory, disk, and
NET utilization, then LOAD/IOWAIT where the width tier allows. At narrow
widths the existing HOST truncation and tier fallback preserve numeric
columns. `v` toggles between normal and condensed views.

An offline system shows the same stable failure category in either view
(`offline (refused)`, `offline (http) HTTP 503`), so switching to the compact
layout never costs you the reason a system went down. A pending system shows
`pending` with no reason, because it has not been polled yet.
