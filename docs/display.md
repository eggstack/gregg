# Display

Reachable systems show five rows (normal view). All four metric rows share
the same fleet-wide `bar_width` so the opening `[` and closing `]` columns
always align across every online system, and the metric rows are indented by
exactly four spaces:

```text
Deadpool · Ubuntu 24.04 x86_64 · Linux 6.8  IO 0.4%  L(8) 1.32/.91/.62
    CPU  [||||||||||||                                  ] 25.2% 8 cores
    MEM  [||||||||||||||||||                            ] 37.8% 5.9 GiB / 15.6 GiB
    SWP  [                                                ]  0.0% 0 B / 4.0 GiB
    DISK [||||||||||||                                  ] 25.0% 238.0 GiB / 952.0 GiB
```

The DISK suffix is `<used bytes> / <total bytes>` so the slash denominator
matches the percentage calculation; explicit caller-available capacity is
preserved by the normalized model and surfaced only through the expanded
per-drive rows. On Windows, the third row uses `COMMIT` (memory commit
charge) instead of `SWP`. Unreachable rows render `—` instead of fabricating
a `0.0%`.

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

Condensed view shows one comparison row per system with CPU, memory, disk,
load, and I/O-wait columns.
