# Phase 8: compact Ratatui TUI

## Objective

Implement the terminal interface for `gregg` as a compact, keyboard-first projection of `AppState`. The TUI must remain usable in a narrow terminal-multiplexer pane, consume exactly four rows per reachable system and one row per unreachable system, adapt to terminal resize, and preserve terminal state on every exit path.

The TUI does not perform network or filesystem I/O directly. It receives state changes and emits typed actions.

## Terminal stack

Use Ratatui with Crossterm unless implementation evidence shows a substantially better fit. Keep terminal setup in one module:

```text
crates/gregg/src/terminal.rs
crates/gregg/src/input.rs
crates/gregg/src/ui/
├── mod.rs
├── layout.rs
├── system_block.rs
├── bar.rs
├── text.rs
└── diagnostics.rs
```

Terminal startup should:

1. Enable raw mode.
2. Enter alternate screen.
3. Hide cursor if appropriate.
4. Optionally enable bracketed paste only if used.
5. Avoid enabling mouse capture in version 1 unless it is essentially free and fully restored.

Terminal teardown must reverse every enabled mode on normal quit, error propagation, Ctrl-C, and panic. Install a panic hook that restores the terminal before delegating to the previous hook, without masking the original panic.

## Event loop

The event loop integrates:

- Keyboard input.
- Terminal resize events.
- Poll-batch/state-update events.
- Scheduler status changes.
- Shutdown signals.

Rendering should be event-driven. A modest maximum redraw rate may coalesce bursts, but do not run a continuous 30/60 FPS loop. Do not miss state changes merely because input is idle.

Use `Frame::area()` on every draw as the authoritative terminal dimensions. Resize events trigger a state/action update, but layout correctness must not depend solely on receiving every resize event.

## Required key map

```text
j / Down       select next system
k / Up         select previous system
PageDown       advance one logical viewport
PageUp         retreat one logical viewport
g / Home       select first system
G / End        select last system
r              poll immediately
q / Esc        quit
```

Only `j`, `k`, Up, and Down are mandatory from the original contract; the remaining bindings are low-cost usability additions. Avoid a large command mode or help overlay in version 1. A concise one-line key hint may appear only when vertical space permits and must not reduce the four-line host contract.

Unknown keys are ignored. Key repeat should behave naturally and not enqueue unbounded actions.

## Online system rendering

Render exactly four rows with no surrounding border:

```text
Deadpool · Ubuntu 24.04 x86_64 · Linux 6.8  IO 0.4%  L(8) 1.32/.91/.62
CPU  [||||||||||||                                  ] 25.2%
MEM  [||||||||||||||||||                            ] 37.8%  5.9/15.6 GiB
SWAP [                                                ]  0.0%  0/4.0 GiB
```

The first line is a priority-aware composition, not a preformatted daemon string. Maintain ordered segments:

1. Display name or hostname.
2. I/O-wait value or explicit unsupported marker.
3. Load averages.
4. Logical core count colocated with load.
5. OS name/version.
6. Kernel release.
7. Architecture.

As width decreases, drop or abbreviate lower-priority segments before truncating higher-priority values.

Suggested degradation:

```text
Wide:
Deadpool · Ubuntu 24.04 x86_64 · Linux 6.8.0-134  IO 0.4%  L(8) 1.32/0.91/0.62

Medium:
Deadpool · Ubuntu 24.04 · 6.8.0-134  IO 0.4%  L8 1.32/.91/.62

Narrow:
Deadpool  IO .4%  L8 1.3/.9/.6
```

For macOS, render `IO —`. Do not render `0%` for unsupported I/O wait.

If configured name and daemon-reported name differ, prefer the configured name for stable operator identity while retaining the daemon identity internally. Document this policy.

## Usage bars

Implement a reusable bar renderer rather than depending on Ratatui Gauge behavior if a custom renderer gives tighter control over exact text shape.

Required conceptual format:

```text
CPU  [||||||||        ] 25.2%
MEM  [||||||||||||    ] 37.8%
SWAP [                ]  0.0%
```

Layout calculation:

```text
available bar width = total width
                    - label
                    - spaces/brackets
                    - percentage field
                    - optional memory value field
```

Requirements:

- Never underflow width arithmetic.
- Clamp display percentage to valid bounds only after protocol validation.
- Use deterministic rounding for filled cells.
- Preserve a fixed-width percentage field where possible to prevent jitter.
- Use ASCII `|` by default for broad terminal compatibility; a Unicode mode is unnecessary for version 1.
- Memory/swap human-readable values should use binary units consistently and degrade away before the percentage/bar.
- Zero-total swap renders a valid empty bar and `0.0%`, with total optionally shown as `0` or omitted according to width.

At very narrow widths, abbreviate labels (`CPU`, `MEM`, `SWP`) and reduce precision before dropping the bar entirely.

## Offline and pending rendering

An unreachable system consumes one row:

```text
Deadpool@192.168.182.8:11310 offline
```

If no configured name exists, use hostname/IP. IPv6 uses bracketed endpoint form.

Startup-pending systems may use `pending` until the first poll batch completes, but after classification all failures collapse visually to `offline`. Keep detailed failure state internally. Do not display long error messages in the compact row.

All offline systems appear after online systems through the state projection. The renderer must not sort independently.

## Selection indicator

Indicate selection without adding rows or borders. Suitable approaches:

- Reverse/bold the first line of the selected entry.
- Use a leading marker occupying a reserved one-character column.
- Apply style across the four rows while preserving bar readability.

Do not rely solely on color; terminals may be monochrome and users may have custom palettes. Keep styling restrained and avoid making the TUI “fancy.”

## Viewport and clipping

Scrolling operates by system entry. Use state-provided ordering/selection and layout helpers to determine visible entries.

Rules:

- Never split a four-row online entry across top or bottom viewport boundaries.
- If usable height is below four rows, show `gregg: terminal too small` rather than a partial online block.
- Offline one-row entries can fill remaining complete rows.
- Keep selected entry visible after next/previous, page movement, poll-driven online/offline reorder, endpoint removal, and resize.
- Avoid reserving a permanent header/footer that reduces fleet capacity. Any transient status should overlay or appear only when space permits.

Choose a minimum width such as 24 columns based on buffer tests. Below it, show a one-line terminal-too-small diagnostic that itself never panics.

## Width and Unicode safety

System names and OS labels may contain Unicode. Layout must measure terminal column width, not UTF-8 byte length. Use a well-maintained display-width utility if Ratatui’s text primitives do not fully cover truncation needs.

Truncate at valid character/grapheme boundaries and append an ellipsis only when there is room. Never panic on combining characters or wide East Asian glyphs. Tests should include at least accented Latin, CJK, and emoji-containing display names, while documentation may recommend concise names for narrow panes.

## Empty and error states

Empty config:

```text
No systems configured. Use: gregg add HOST[:PORT]
```

Invalid configuration should be reported before entering the alternate screen where possible. If a runtime reload becomes invalid, preserve the last valid in-memory config and show a concise nonfatal diagnostic without destroying terminal layout.

Fatal polling infrastructure errors should restore the terminal and exit nonzero. Individual endpoint failures are ordinary state, not fatal errors.

## Testing

Use Ratatui `TestBackend` buffer assertions or stable snapshot testing. Cover a matrix including:

- Widths: below minimum, 24, 32, 40, 60, 80, 120.
- Heights: 1, 3, 4, 5, 8, 12, and mixed viewport sizes.
- Linux online system with I/O wait.
- macOS online system without I/O wait.
- Zero swap and nonzero swap.
- Long names and OS/kernel strings.
- Unicode display names.
- Mixed online/offline ordering.
- Selection on online and offline entries.
- Resize from wide/tall to narrow/short and back.
- Empty configuration.
- Bars at 0%, fractional values, approximately 50%, 99.9%, and 100%.

Input tests should verify key-to-action mapping separately from state transitions. Terminal lifecycle tests should use an abstraction or test backend rather than manipulating the real CI terminal.

## Acceptance criteria

Phase 8 is complete when:

1. Reachable systems render in exactly four rows and unreachable systems in exactly one row.
2. No borders, headers, or permanent footer consume additional required rows.
3. Width degradation preserves name, I/O-wait availability, load, and cores before lower-priority identity fields.
4. macOS unsupported I/O wait renders as `—`, never zero.
5. CPU, memory, and swap bars remain valid without width underflow from minimum through wide sizes.
6. `j`/Down and `k`/Up move selection in current display order and keep it visible.
7. Page, first/last, immediate-refresh, and quit actions behave as documented if included.
8. Mixed-height viewport logic never renders partial online entries.
9. Resize uses the current frame area and cannot panic at tiny dimensions.
10. Unicode truncation is column-aware and safe.
11. Rendering and input modules perform no network or config-file I/O.
12. Terminal modes are restored on normal quit, error, signal, and panic paths.
13. The TUI redraws on events without a continuous high-FPS idle loop.
14. Buffer tests cover the full width/height/platform/status matrix.

## Handoff to phase 9

Produce representative text captures or screenshots and expose runtime counters/logging sufficient to measure redraw frequency, polling cadence, and resource use without adding visible monitoring complexity to the TUI.
