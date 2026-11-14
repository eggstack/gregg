# Display

The normal view uses five rows for each online system without network telemetry,
and six when network telemetry is available. Mixed fleets can have different
row counts. Metric bars use a fleet-wide width so their brackets align, and each
row is indented by four spaces.

```text
Deadpool · Ubuntu 24.04 x86_64 · Linux 6.8  IO 0.4%  L(8) 1.32/.91/.62
    CPU  [||||||||||||                                  ] 25.2% 8 cores
    MEM  [||||||||||||||||||                            ] 37.8% 5.9 GiB / 15.6 GiB
    SWP  [                                                ]  0.0% 0 B / 4.0 GiB
    DISK [||||||||||||                                  ] 25.0% 238.0 GiB / 952.0 GiB
    NET  [||||||||||||                                  ] 31.0% 39.0 MiB/s rx 5.0 MiB/s tx
```

The NET row appears only when that system's snapshot includes network data.
Zero traffic and unknown capacity are valid measurements. CPU detail may include
the current OS-reported frequency, which is not a base or maximum frequency.
macOS may omit it.

The DISK suffix shows used and total capacity. On Windows, the third row shows
memory commit charge instead of swap. Missing measurements are shown as
unavailable, not as fabricated zeroes. A host with no swap is not reported as
having zero swap usage.

When a metric suffix would take more than one quarter of the terminal width,
all normal-view metric rows switch to bars only. Widening the terminal restores
the suffixes without a restart. The header omits `IO` when no CPU I/O-wait value
is available.

## Offline systems

Offline systems collapse to one row and appear after reachable systems. A
configured nickname is shown with the endpoint; without a nickname the host is
shown once. When available, the row includes a failure category such as
`offline (refused)` or `offline (http) HTTP 503`. A system that has not yet
been polled is shown as pending without a failure reason.

If a selected system goes offline, selection moves to the reachable system that
takes its place. The visible window stays with the live systems rather than
following the failed system to the bottom of the list. The offline entry remains
visible with its reason.

## Drive and network details

Press `d` to expand drive details. When disk-I/O data is available, the view
includes read and write rates and a daemon-provided total. Per-drive rates are
shown only for an unambiguous device match. The aggregate is not recalculated
from the displayed drive rows.

Press `n` to independently expand network details. The view shows aggregate
rates and capacity, followed by interfaces. Receive and transmit rates are byte
rates. Utilization uses the larger valid direction, so full-duplex traffic does
not exceed 100 percent by adding receive and transmit. Loopback can appear in
the interface list but does not contribute to aggregate capacity.

## Scheduled-job details

Press `c` to expand scheduler details for the selected system. The block lists
configured jobs, their state and latest result, and recent history for the job
selected with `Shift-J` or `Shift-K`. The drive, network, and scheduler details
are independent and share the available vertical space.

Load-gated jobs show the load observation and threshold that delayed a run. A
missing load observation is shown as unavailable, never as zero. Records for
runs that did not start a child have no fabricated duration. Older daemons that
do not provide the scheduler routes are shown as unsupported, not offline.
Transient scheduler failures retain the last known data and mark it stale.

Schedules use the remote host's local time. Run record timestamps are UTC
because the remote timezone is not provided. Relative durations show elapsed
time. The selected job stays visible when the table is taller than the terminal;
hidden rows and remote output truncation are reported separately.

Remote command output is escaped before display. Control characters use
printable `cat -v` notation, so terminal control sequences cannot clear or
retitle your screen.

## Condensed view

Press `v` to switch between normal and condensed views. Condensed view shows one
comparison row per system with CPU, memory, disk, and network utilization, then
load and I/O wait when the width permits. At narrow widths, host truncation and
layout fallbacks preserve the numeric columns. Offline rows retain their failure
category in either view.
