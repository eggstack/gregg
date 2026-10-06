//! Plan 166: rendering remote command output as inert text.
//!
//! `greggd` captures a bounded stdout/stderr tail for each scheduled job and
//! publishes it on `/v2/scheduler/history`. That text came from an arbitrary
//! program on a remote machine, so it is **hostile terminal input**, not
//! display text: a build script that prints `ESC [ 2 J` would otherwise clear
//! the operator's screen, `ESC ] 0 ; … BEL` would retitle their terminal, and
//! a bare `\r` would rewrite a line they were reading.
//!
//! # Why a pure helper
//!
//! This is deliberately a small, pure, dependency-free function rather than a
//! terminal-emulation layer. It has one job — make a string contain no
//! character a terminal would interpret — and it must be provable by unit test
//! rather than by inspection of a rendering path. It does no I/O, holds no
//! state, and never mutates what it is given.
//!
//! # The strategy: make the introducer visible, keep the payload literal
//!
//! The obvious alternative is to *strip* control characters, or to maintain a
//! full state machine that consumes a whole escape sequence and drops it. Both
//! lose information: a stripped string claims the remote output said nothing,
//! when in fact it said something unusual. This module instead renders each
//! control in the classic `cat -v` caret notation — `ESC` becomes the two
//! printable characters `^[`, `BEL` becomes `^G` — and lets the rest of the
//! sequence fall through as the ordinary printable text it already was:
//!
//! ~~~text
//! "\x1b[2J"  ->  "^[[2J"
//! "\x1b]0;pwn\x07" ->  "^[]0;pwn^G"
//! ~~~
//!
//! Both are inert: there is no `ESC` byte left for a terminal to act on, so no
//! sequence can be interpreted no matter how it is split. Both are also exactly
//! what the remote program emitted, so the operator sees the anomaly instead of
//! a quietly mangled log.
//!
//! # One rule, two modes: what a newline means
//!
//! [`sanitize`] treats `\n` as a **separator**, because job stdout and stderr
//! are multi-line bodies and the caller wants their lines.
//!
//! [`sanitize_single_line`] treats it as an **anomaly** and renders it `^J`,
//! because a drive name, job name, interface name, or hostname occupies exactly
//! one row. That distinction is load-bearing rather than stylistic: the cell
//! builder drops control graphemes instead of breaking the row, so an unescaped
//! newline in a one-row field is *deleted* and the text on either side of it
//! fuses into an identifier the remote never reported. Caret notation is what
//! keeps that from being a silent merge.
//!
//! # What is *not* escaped
//!
//! Printable Unicode passes through untouched, including wide (double-cell)
//! CJK and emoji, because a monitor that mangles legitimate output from a
//! localized build is not being safer — it is being useless. The escaping is
//! therefore about *interpretation*, never about non-ASCII content.

use std::fmt::Write as _;

use unicode_width::UnicodeWidthStr;

/// Columns a tab expands to.
///
/// Four, not a real tab stop: a tab stop depends on the column the text lands
/// in, and the same stored string is rendered at different columns in different
/// panes. A fixed expansion is predictable, and the renderer's own width budget
/// bounds the result.
const TAB_CELLS: &str = "    ";

/// Sanitized remote text plus what was changed to get there.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Sanitized {
    /// Text that is safe to place in Ratatui cells.
    ///
    /// May contain `\n` as a line separator. Every other byte that a terminal
    /// could interpret has been replaced by printable ASCII.
    pub text: String,
    /// Whether any control character had to be made visible.
    ///
    /// The renderer uses this to mark the output as containing control
    /// sequences, which is information an operator debugging a job wants.
    pub escaped: bool,
    /// Whether input lines were dropped because of the `max_lines` bound.
    pub dropped_lines: bool,
}

impl Sanitized {
    /// The sanitized text split into lines, with no trailing empty element
    /// from a final newline.
    #[must_use]
    pub fn lines(&self) -> Vec<&str> {
        if self.text.is_empty() {
            return Vec::new();
        }
        self.text.split('\n').collect()
    }

    /// Whether anything about the original was changed by sanitizing or
    /// bounding, i.e. whether the renderer should say the display is partial.
    #[must_use]
    pub fn is_altered(&self) -> bool {
        self.escaped || self.dropped_lines
    }
}

/// Make `text` safe for terminal cells, keeping at most `max_lines` lines.
///
/// The line bound is a *display* bound: the caller keeps the original string,
/// so widening the viewport shows the text that was always there. `max_lines`
/// of `0` therefore returns no text at all, which is what a too-small terminal
/// wants instead of indexing outside its buffer.
///
/// [`Sanitized::dropped_lines`] reports whether the bound actually bit, and is
/// distinct from the remote daemon's own truncation flag: this one is about
/// local viewport pressure, that one is about what the remote ring retained.
///
/// A newline here is a *separator*: the result may span several rows. For a
/// field that occupies exactly one row, use [`sanitize_single_line`] instead —
/// this function's `\n` handling is wrong for that case, not merely different.
#[must_use]
pub fn sanitize(text: &str, max_lines: usize) -> Sanitized {
    sanitize_inner(text, max_lines, true)
}

/// Make `text` safe for a field that occupies exactly one terminal row.
///
/// The same escaping as [`sanitize`], except that a newline is an anomaly
/// rather than a separator: it is rendered as `^J` (and a CR/LF pair as
/// `^M^J`) instead of splitting the row.
///
/// This is not cosmetic. Ratatui builds cells through
/// `symbol.contains(char::is_control)` and *drops* a control grapheme rather
/// than breaking the row, so an unescaped `\n` inside a drive name, job name,
/// interface name, or hostname is silently deleted at render time and the text
/// on either side of it fuses into a single identifier that the remote never
/// reported — `/dev/sda1\nroot` renders as `/dev/sda1root`, a device that does
/// not exist, with no marker that anything was lost. Caret notation keeps the
/// anomaly visible instead, which is the same rule this module applies to every
/// other control.
///
/// There is no line bound: a single-line field is one row by definition, and
/// bounding it here would report a truncation that is really the renderer's
/// width budget. `dropped_lines` is therefore always `false`.
#[must_use]
pub fn sanitize_single_line(text: &str) -> Sanitized {
    // `max_lines` is unreachable in single-line mode — no character closes a
    // line, so `emitted` never advances — but the guard is a bound, not a
    // promise, so it stays finite and unremarkable.
    sanitize_inner(text, usize::MAX, false)
}

fn sanitize_inner(text: &str, max_lines: usize, newline_breaks_line: bool) -> Sanitized {
    let mut out = Sanitized::default();
    let mut current = String::new();
    let mut current_escaped = false;
    let mut emitted = 0_usize;
    let mut characters = text.chars().peekable();

    // A single streaming pass, so a remote process that emits a megabyte of
    // newlines costs one bounded line buffer rather than a `Vec<&str>` of a
    // million entries. The line bound is checked at the point a line is
    // *closed*, which is also the only point where we know another line began.
    while let Some(character) = characters.next() {
        match character {
            '\n' if newline_breaks_line => {
                if emitted == max_lines {
                    out.dropped_lines = true;
                    return out;
                }
                if emitted > 0 {
                    out.text.push('\n');
                }
                out.text.push_str(&current);
                current.clear();
                out.escaped |= current_escaped;
                current_escaped = false;
                emitted += 1;
            }
            // In single-line mode nothing closes a row, so a break must be made
            // visible instead of acting as one.
            '\n' => {
                current.push_str("^J");
                current_escaped = true;
            }
            '\r' => {
                // A CR/LF pair is one line break, so CRLF output does not
                // acquire a phantom blank line. A *lone* CR is an overwrite
                // attempt rather than a line break, and rewriting it as one
                // here would let output defeat the line accounting above, so it
                // is made visible instead.
                //
                // In single-line mode the LF is escaped rather than consumed,
                // so both halves stay visible: a `^M^J` says what arrived.
                if newline_breaks_line && characters.peek() == Some(&'\n') {
                    continue;
                }
                current.push_str("^M");
                current_escaped = true;
            }
            // A tab is normalized, not neutralized: expanding it is layout, not
            // an injection attempt, so it does not set `escaped`.
            '\t' => current.push_str(TAB_CELLS),
            other => match escape(other) {
                Some(replacement) => {
                    current.push_str(&replacement);
                    current_escaped = true;
                }
                None => current.push(other),
            },
        }
    }

    // Whatever follows the final newline is a line of its own, unless it is
    // empty. A trailing newline therefore does not add a phantom empty line.
    // Single-line mode has no final newline to account for, and its text is
    // always flushed below.
    if !current.is_empty() {
        if newline_breaks_line && emitted == max_lines {
            out.dropped_lines = true;
            return out;
        }
        if emitted > 0 {
            out.text.push('\n');
        }
        out.text.push_str(&current);
        out.escaped |= current_escaped;
    }
    out
}

/// The printable, inert rendering of one control character.
///
/// `None` means the character is already safe to emit, or is a normalization
/// rather than an escape: `\t` and `\n` are handled structurally by
/// [`sanitize`] and are deliberately absent here, so a routine tab in ordinary
/// build output does not get reported to the operator as an injection attempt.
fn escape(character: char) -> Option<String> {
    match character {
        // Structural, and handled by `sanitize` itself: a newline closes a
        // line and a tab is expanded. Neither is ever emitted as a cell
        // character, so neither is an escape.
        '\n' | '\t' => None,
        '\u{0}'..='\u{1f}' => Some(caret(u32::from(character as u8) + 0x40)),
        '\u{7f}' => Some("^?".to_owned()),
        // C1 controls are the 8-bit forms of ESC, CSI, and friends. A UTF-8
        // string can carry them literally, and a terminal in UTF-8 mode still
        // interprets some of them, so they are escaped rather than trusted.
        '\u{80}'..='\u{9f}' => Some(format!("M-{}", caret(character as u32 - 0x40))),
        // Bidi controls reorder the *displayed* glyphs without changing the
        // bytes, which is how a log line can read as something it does not
        // say. They are rendered in explicit `\uXXXX` form rather than dropped,
        // so the attempt stays visible.
        '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}' => {
            let mut text = String::with_capacity(8);
            let _ = write!(text, "\\u{:04x}", character as u32);
            Some(text)
        }
        // U+2028/U+2029 are line/paragraph separators: some terminals and many
        // pagers treat them as breaks, which would desynchronize the caller's
        // line accounting from what is on screen.
        '\u{2028}' | '\u{2029}' => Some(format!("\\u{:04x}", character as u32)),
        _ => None,
    }
}

/// Caret notation for a byte in the printable ASCII range.
fn caret(character: u32) -> String {
    char::from_u32(character).map_or_else(|| format!("^{character}"), |c| format!("^{c}"))
}

/// Whether a character is emitted to terminal cells unchanged.
///
/// This is exactly "does not need caret escaping": a `\t` or `\n` is inert
/// because [`sanitize`] normalizes it structurally, and a printable character
/// is inert because there is nothing to neutralize.
#[must_use]
pub fn is_inert(character: char) -> bool {
    escape(character).is_none()
}

/// Display width of sanitized text, in terminal cells.
///
/// Width-aware rather than `chars().count()`, so a row containing CJK is laid
/// out against the columns it actually occupies.
#[must_use]
pub fn cells(text: &str) -> usize {
    // A line break occupies no column of its own; the caller accounts for the
    // row it starts.
    text.lines().map(UnicodeWidthStr::width).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sanitized(text: &str) -> Sanitized {
        sanitize(text, 64)
    }

    #[test]
    fn plain_text_is_untouched() {
        let out = sanitized("build finished: 42 targets");
        assert_eq!(out.text, "build finished: 42 targets");
        assert!(!out.escaped);
        assert!(!out.dropped_lines);
        assert!(!out.is_altered());
    }

    #[test]
    fn ansi_colour_becomes_visible_and_inert() {
        // "\x1b[31mred\x1b[0m" is the most ordinary thing a build tool prints.
        let out = sanitized("\x1b[31mred\x1b[0m");
        assert_eq!(out.text, "^[[31mred^[[0m");
        assert!(out.escaped);
        assert!(!out.text.contains('\u{1b}'));
    }

    #[test]
    fn cursor_movement_and_clear_screen_never_reach_the_terminal() {
        for hostile in [
            "\x1b[2J\x1b[H",    // clear screen + home
            "\x1b[10;20H",      // cursor position
            "\x1b[1A\x1b[2B",   // cursor up/down
            "\x1b[?25l",        // hide cursor
            "\x1b7\x1b8",       // save/restore cursor
            "\x1bP1$r0m\x1b\\", // DCS, terminated by ST
        ] {
            let out = sanitized(hostile);
            assert!(
                !out.text.chars().any(|c| c.is_control() && c != '\n'),
                "control byte survived in {:?}",
                out.text
            );
            assert!(out.escaped, "not reported as escaped: {hostile:?}");
        }
    }

    #[test]
    fn osc_title_and_hyperlink_are_inert() {
        // A retitle and a clickable link are the two OSC payloads that reach
        // outside the grid.
        let title = sanitized("\x1b]0;pwned\x07");
        assert_eq!(title.text, "^[]0;pwned^G");
        let link = sanitized("\x1b]8;;http://evil.example\x1b\\click\x1b]8;;\x1b\\");
        assert!(
            !link.text.chars().any(|c| c.is_control() && c != '\n'),
            "control byte survived in {:?}",
            link.text
        );
        // The URL is still readable, so the attempt is visible rather than
        // silently removed.
        assert!(link.text.contains("evil.example"), "{}", link.text);
    }

    #[test]
    fn osc_terminated_by_string_terminator_is_still_inert() {
        // ESC \ terminates an OSC; the ESC is escaped, so the terminator is
        // literal text and cannot close a real sequence.
        let out = sanitized("\x1b]0;title\x1b\\");
        assert!(!out.text.contains('\u{1b}'));
        assert!(out.escaped);
    }

    #[test]
    fn bel_and_backspace_are_visible() {
        assert_eq!(sanitized("ding\x07").text, "ding^G");
        assert_eq!(sanitized("a\x08b").text, "a^Hb");
    }

    #[test]
    fn carriage_return_is_visible_and_does_not_overwrite() {
        // "ok\rBAD" is a progress-bar overwrite. Rendered literally the
        // operator must see that it happened.
        let out = sanitized("ok\rBAD");
        assert_eq!(out.text, "ok^MBAD");
        assert!(!out.text.contains('\r'));
    }

    #[test]
    fn crlf_is_normalized_to_one_line_break() {
        let out = sanitized("a\r\nb\r\n");
        assert_eq!(out.text, "a\nb");
        assert_eq!(out.lines(), vec!["a", "b"]);
    }

    #[test]
    fn a_line_of_only_newlines_cannot_inflate_the_line_count() {
        // The classic accounting attack: "\r" repeated to hide content.
        let out = sanitized(&"\r".repeat(10_000));
        assert!(!out.text.contains('\r'));
        assert_eq!(out.lines().len(), 1, "each CR must stay on one line");
        assert!(out.escaped);
    }

    #[test]
    fn c1_controls_are_escaped_even_though_they_are_not_c0() {
        // U+009B is the 8-bit CSI. A UTF-8 string can carry it literally.
        let out = sanitized("a\u{9b}31mb");
        assert!(!out.text.contains('\u{9b}'));
        assert!(out.text.contains("M-^["), "{}", out.text);
        assert!(out.escaped);
    }

    #[test]
    fn delete_is_escaped() {
        assert_eq!(sanitized("a\u{7f}b").text, "a^?b");
    }

    #[test]
    fn bidi_overrides_are_neutralised_but_stay_visible() {
        // U+202E right-to-left override can make a log line read backwards.
        let out = sanitized("pass\u{202e}FAIL");
        assert!(!out.text.contains('\u{202e}'));
        assert!(out.text.contains("\\u202e"), "{}", out.text);
        assert!(out.escaped);
    }

    #[test]
    fn line_separators_do_not_desynchronize_line_accounting() {
        let out = sanitized("a\u{2028}b\u{2029}c");
        assert_eq!(out.lines().len(), 1);
        assert!(out.escaped);
    }

    #[test]
    fn printable_unicode_survives_intact() {
        // A monitor that mangles a localized build log is not safer, it is
        // useless. Wide glyphs must pass through and keep their cell width.
        let out = sanitized("ビルド成功 ✓ 🎉 done");
        assert_eq!(out.text, "ビルド成功 ✓ 🎉 done");
        assert!(!out.escaped);
        assert_eq!(
            cells(&out.text),
            UnicodeWidthStr::width("ビルド成功 ✓ 🎉 done")
        );
    }

    #[test]
    fn replacement_characters_from_daemon_conversion_are_preserved() {
        // A lossy UTF-8 conversion on the daemon side already produced U+FFFD.
        // That is a real character the operator should see.
        let out = sanitized("caf\u{fffd} error");
        assert_eq!(out.text, "caf\u{fffd} error");
        assert!(!out.escaped);
    }

    #[test]
    fn tabs_expand_to_a_fixed_number_of_cells() {
        let out = sanitized("a\tb");
        assert_eq!(out.text, format!("a{TAB_CELLS}b"));
        assert_eq!(cells(&out.text), 6);
        // A tab is layout, not an injection attempt: expanding it must not make
        // the renderer claim the output contained control sequences.
        assert!(!out.escaped);
    }

    #[test]
    fn the_line_bound_drops_the_tail_and_says_so() {
        let out = sanitize("1\n2\n3\n4\n5", 3);
        assert_eq!(out.lines(), vec!["1", "2", "3"]);
        assert!(out.dropped_lines);
        assert!(out.is_altered());
    }

    #[test]
    fn the_line_bound_does_not_claim_truncation_it_did_not_perform() {
        let out = sanitize("1\n2", 5);
        assert_eq!(out.lines(), vec!["1", "2"]);
        assert!(!out.dropped_lines);
    }

    #[test]
    fn a_zero_line_bound_returns_nothing_rather_than_overflowing() {
        // This is the too-small-terminal path: no indexing outside the buffer.
        let out = sanitize("anything", 0);
        assert_eq!(out.text, "");
        assert!(out.dropped_lines);
    }

    #[test]
    fn empty_input_is_not_reported_as_truncated() {
        let out = sanitize("", 5);
        assert_eq!(out.text, "");
        assert!(!out.dropped_lines);
        assert!(!out.escaped);
        assert_eq!(out.lines(), Vec::<&str>::new());
    }

    #[test]
    fn a_flood_of_newlines_is_bounded_by_the_line_bound() {
        let out = sanitize(&"\n".repeat(100_000), 8);
        assert!(out.dropped_lines);
        assert!(out.lines().len() <= 8);
    }

    #[test]
    fn sanitizing_never_panics_on_arbitrary_bytes() {
        // Every byte value, as a char, plus the replacement char and a lone
        // surrogate stand-in the daemon could emit.
        for code in 0_u32..=0x10_FFFF {
            if let Some(character) = char::from_u32(code) {
                let _ = sanitize(&character.to_string(), 4);
            }
        }
    }

    #[test]
    fn is_inert_agrees_with_the_escaped_flag_for_every_single_character() {
        // The two are the same question asked twice. If they ever disagree, a
        // caller trusting one of them renders something the other calls unsafe.
        for code in 0_u32..=0x2FF {
            let Some(character) = char::from_u32(code) else {
                continue;
            };
            let out = sanitized(&character.to_string());
            assert_eq!(
                out.escaped,
                !is_inert(character),
                "disagreement at U+{code:04X}: escaped={} inert={} text={:?}",
                out.escaped,
                is_inert(character),
                out.text
            );
        }
    }

    /// A newline inside a one-row field is deleted by the cell builder, so it
    /// must be made visible instead. Without this the two halves of the name
    /// fuse into an identifier the remote never reported.
    #[test]
    fn single_line_mode_caret_escapes_a_newline_instead_of_splitting_it() {
        let out = sanitize_single_line("/dev/sda1\nroot");
        assert_eq!(out.text, "/dev/sda1^Jroot");
        assert!(out.escaped);
        assert!(!out.text.contains('\n'));
        assert_eq!(
            out.lines(),
            vec!["/dev/sda1^Jroot"],
            "still exactly one row"
        );
        // The multi-line mode is unchanged: it is the stdout/stderr path and
        // must keep splitting there.
        assert_eq!(
            sanitize("/dev/sda1\nroot", 8).lines(),
            vec!["/dev/sda1", "root"]
        );
    }

    #[test]
    fn single_line_mode_keeps_both_halves_of_a_crlf_visible() {
        // Multi-line mode folds CRLF into one break. Single-line mode escapes
        // both halves, because `^M^J` is what actually arrived and a lone `^M`
        // would claim a CR that was not there.
        assert_eq!(sanitize_single_line("a\r\nb").text, "a^M^Jb");
        assert_eq!(sanitize_single_line("a\rb").text, "a^Mb");
        assert_eq!(sanitize("a\r\nb", 8).text, "a\nb");
    }

    #[test]
    fn single_line_mode_never_claims_a_truncation_it_did_not_perform() {
        // The width budget is the renderer's decision, not this function's, so
        // reporting `dropped_lines` here would conflate the two facts the pane
        // keeps separate.
        let out = sanitize_single_line("1\n2\n3");
        assert!(!out.dropped_lines);
        assert_eq!(out.text, "1^J2^J3");
    }

    #[test]
    fn single_line_mode_agrees_with_the_multi_line_mode_on_every_other_control() {
        // The two modes differ on newlines only. Anything else must be escaped
        // identically, or a one-row field would be a weaker filter than a body.
        for code in 0_u32..=0x10_FFFF {
            let Some(character) = char::from_u32(code) else {
                continue;
            };
            if matches!(character, '\n' | '\r') {
                continue;
            }
            let single = sanitize_single_line(&character.to_string());
            let multi = sanitized(&character.to_string());
            assert_eq!(
                single.text, multi.text,
                "disagreement at U+{code:04X}: single={:?} multi={:?}",
                single.text, multi.text
            );
            assert_eq!(
                single.escaped, multi.escaped,
                "disagreement at U+{code:04X}"
            );
        }
    }
}
