# TUI Manual Test Checklist

This document records manual verification results for TUI behaviors that
cannot be exercised by automated unit tests. Each item should be tested
on at least one representative terminal configuration and the result
recorded with the date and environment.

## Test Environment

| Property | Value |
|----------|-------|
| Date | 2026-07-23 |
| Platform | macOS ARM64 (Apple Silicon) |
| Terminal | [Fill in: iTerm2 / Terminal.app / Alacritty / etc.] |
| tmux version | [Fill in] |
| TERM | [Fill in: xterm-256color, etc.] |

## Checklist

### Terminal Multiplexer Behavior

- [ ] **tmux small pane (40x10)**: Rendering adapts correctly, no corruption
- [ ] **tmux resize while running**: Content reflows, no stale characters
- [ ] **zellij small pane**: Same as tmux checks
- [ ] **Multiple panes with gregg**: Each pane renders independently

### SSH Sessions

- [ ] **SSH with variable latency**: Polling continues, UI remains responsive
- [ ] **SSH disconnect/SIGHUP**: Terminal restored, process exits cleanly
- [ ] **SSH with ControlMaster**: Connection reuse does not affect behavior

### Terminal Configuration

- [ ] **TERM=xterm-256color**: Full rendering with colors
- [ ] **TERM=xterm-16color**: Reduced but functional color palette
- [ ] **TERM=dumb**: Graceful degradation (no raw mode crash)
- [ ] **NO_COLOR=1**: Monochrome rendering, no escape codes
- [ ] **--no-color flag**: Same as NO_COLOR

### Exit Behavior

- [ ] **Ctrl-C**: Terminal restored, cursor visible, no raw mode residue
- [ ] **q key**: Clean exit, terminal restored
- [ ] **Esc key**: Clean exit, terminal restored
- [ ] **Window close (X button)**: SIGHUP handled, terminal restored
- [ ] **kill -TERM**: SIGTERM handled gracefully
- [ ] **kill -KILL**: Process killed (no restoration possible, expected)

### Panic Recovery

- [ ] **Panic hook restores terminal**: Inject panic, verify terminal usable
- [ ] **Double panic**: Second panic does not hang

### Unicode and Display

- [ ] **CJK system names**: Display correctly, truncation works
- [ ] **Emoji in system names**: Display without corruption
- [ ] **Very long system names**: Truncated at terminal edge
- [ ] **Mixed online/offline**: Reordering works correctly

### Fleet Behavior

- [ ] **1 system**: Single row rendering
- [ ] **10 systems**: All visible, scrolling works
- [ ] **50+ systems**: Scroll performance acceptable
- [ ] **All hosts offline**: Graceful empty state
- [ ] **Host goes offline during display**: Reorder smoothly
- [ ] **Host comes back online**: Reorder smoothly

### Resize

- [ ] **Shrink terminal**: Content reflows, no crash
- [ ] **Grow terminal**: More content visible
- [ ] **Rapid resize spam**: No crash, final state correct

## Results

[Fill in results for each tested item with date and pass/fail]
