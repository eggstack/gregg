//! Where the pulldown opens.
//!
//! # The shape of the problem
//!
//! A tray pulldown is supposed to appear *at the tray*: under the menu-bar
//! item on macOS, above the notification area on Windows, under the panel on
//! Linux. Slint 1.18 does not let the GUI have that:
//!
//! - `SystemTrayIcon` exposes only `icon`, `tooltip`, `title` and one
//!   `Menu`. There is no API for a panel anchored to the tray icon, and no
//!   API that reports where the tray icon is.
//! - The pointer's position is not exposed either, so "where the native menu
//!   was drawn" cannot be read back.
//!
//! What *is* available is the display the panel is on: `winit` — which Slint
//! re-exports, so this costs no new dependency — knows every monitor's
//! position and size.
//!
//! # What this module therefore does
//!
//! It anchors the panel to the **trailing edge of the display, on the side
//! where the desktop's own shell lives**, and clamps it so that the panel is
//! **fully inside** the display. The panel's vertical anchor is the
//! platform's convention:
//!
//! | Platform | The shell | The anchor |
//! | --- | --- | --- |
//! | macOS | the menu bar, at the top | the top-trailing corner |
//! | Windows | the taskbar, at the bottom | the bottom-trailing corner |
//! | Linux | the panel, usually at the top | the top-trailing corner |
//!
//! The shell's own strip is subtracted from the display, so the panel opens
//! *clear of* the menu bar, the taskbar or the panel rather than under it.
//! That strip's exact size is not exposed by any API this GUI may use, so it
//! is an approximation, and it is recorded as one.
//!
//! # The panel can be moved
//!
//! The panel is borderless, so it has no title bar to drag. It carries its
//! own: a strip across the top that moves the window, and a grip in the
//! corner that resizes it. The operator's chosen position and size are
//! remembered, and the position is re-clamped every time the panel opens, so
//! a display that changed underneath the GUI cannot leave the panel
//! off-screen.
//!
//! # How big the panel is
//!
//! A pulldown is a dropdown, so its height follows what it carries: a fleet
//! of one host is a short pulldown and a fleet of twenty is a taller one.
//!
//! Slint 1.18 cannot report a layout's preferred size back to a `.slint`
//! file, so the height cannot be measured in the UI. It is *computed* here,
//! from the rows the model has, and pushed into the component. The parts of
//! the height that are the pulldown's own chrome — the header, the footer,
//! the padding — are written down in [`metrics`], and the `.slint` file's
//! own numbers are checked against them, so the two cannot drift apart
//! without a test saying so.

use slint::{LogicalPosition, LogicalSize};

/// The pulldown's own measurements, in logical pixels.
///
/// These are the heights the `.slint` file gives the pulldown's elements.
/// They are written down here because the pulldown's height is *computed*
/// rather than measured, and a computed height is only as good as the
/// numbers it is computed from. `tests::the_slint_file_says_what_this_module_says`
/// checks the two against each other.
pub mod metrics {
    /// The room the shadow takes around the card, on each side.
    pub const MARGIN: f32 = 14.0;

    /// The header strip's height.
    pub const HEADER: f32 = 36.0;

    /// The footer's height.
    pub const FOOTER: f32 = 18.0;

    /// The padding inside the card, on each side.
    pub const PANE_PADDING: f32 = 12.0;

    /// The gap between the body's children.
    pub const BODY_SPACING: f32 = 6.0;

    /// The gaps between the body's children in the fleet view: the status
    /// line, the message, the tray warning and the fleet itself.
    pub const BODY_GAPS: f32 = 3.0 * BODY_SPACING;

    /// The gaps between the fleet's own children: the host list, the
    /// selected host's actions and the detail.
    pub const FLEET_GAPS: f32 = 2.0 * BODY_SPACING;

    /// The attachment's state line.
    pub const STATUS_LINE: f32 = 18.0;

    /// The last action's message, which takes two lines.
    pub const MESSAGE_LINE: f32 = 30.0;

    /// The tray warning's line.
    pub const TRAY_WARNING_LINE: f32 = 18.0;

    /// One host row in the host list.
    pub const HOST_ROW: f32 = 46.0;

    /// The most host rows the host list may show at once.
    ///
    /// The list is bounded so that a large fleet cannot push the metrics,
    /// the cron and the `EggPool` plane out of the pulldown. The bound is a
    /// number of *rows* and not a number of pixels, so the last row is a
    /// whole row and not half of one.
    pub const HOST_LIST_MAX_ROWS: usize = 4;

    /// The selected host's own actions row.
    pub const SELECTED_ROW: f32 = 18.0;

    /// One measured metric row.
    pub const METRIC_ROW: f32 = 18.0;

    /// One drive or interface row.
    pub const DETAIL_ROW: f32 = 18.0;

    /// One scheduler job row.
    pub const JOB_ROW: f32 = 22.0;

    /// One `EggPool` row.
    pub const EGGPOOL_ROW: f32 = 18.0;

    /// One line of retained scheduler records.
    pub const RECORD_LINE: f32 = 14.0;

    /// The attribution text, which takes four lines.
    pub const ABOUT_LINES: f32 = 4.0;
    pub const ABOUT_LINE: f32 = 14.0;

    /// The thin rules between the panes.
    pub const SEPARATOR: f32 = 1.0;

    /// The form's height.
    pub const FORM_HEIGHT: f32 = 220.0;

    /// The pulldown's width.
    pub const DEFAULT_WIDTH: f32 = 420.0;

    /// The shortest the detail below the host list may be.
    pub const MIN_DETAIL: f32 = 120.0;

    /// The tallest the detail below the host list may be.
    pub const MAX_DETAIL: f32 = 320.0;
}

/// What the pulldown is showing, as the height calculation sees it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Content {
    /// How many hosts the fleet has.
    pub hosts: usize,
    /// Whether a host is selected.
    pub selected: bool,
    /// How many measured rows the selected host has.
    pub metrics: usize,
    /// How many drive and interface rows it has.
    pub details: usize,
    /// How many scheduler jobs it has.
    pub jobs: usize,
    /// How many `EggPool` rows are shown.
    pub eggpool: usize,
    /// How many lines the retained scheduler records take.
    pub record_lines: usize,
    /// Whether the attribution text is shown.
    pub about: bool,
    /// Whether the attachment's state is shown.
    pub status: bool,
    /// Whether the last action's message is shown.
    pub message: bool,
    /// Whether the tray warning is shown.
    pub warning: bool,
    /// Whether the form is showing instead of the fleet.
    pub form: bool,
}

/// The height of the bounded host list.
///
/// The bound is a number of rows, so the list is always a whole number of
/// rows tall and the last row is never half of one.
#[must_use]
pub fn host_list_height(hosts: usize) -> f32 {
    hosts.min(metrics::HOST_LIST_MAX_ROWS) as f32 * metrics::HOST_ROW
}

/// The height of the detail below the host list.
///
/// The detail is one scroll container, so this is an estimate of what it
/// holds: the estimate only decides how much of it is on screen, and the
/// scroll container keeps the rest reachable when the estimate is short.
#[must_use]
fn detail_height(content: &Content) -> f32 {
    let mut height = 3.0 * metrics::SEPARATOR;
    height += content.metrics as f32 * metrics::METRIC_ROW;
    height += content.details as f32 * metrics::DETAIL_ROW;
    height += content.jobs as f32 * metrics::JOB_ROW;
    height += content.eggpool as f32 * metrics::EGGPOOL_ROW;
    height += content.record_lines as f32 * metrics::RECORD_LINE;
    if content.about {
        height += metrics::ABOUT_LINES * metrics::ABOUT_LINE;
    }
    height.clamp(metrics::MIN_DETAIL, metrics::MAX_DETAIL)
}

/// The pulldown's height, for the content it is carrying.
///
/// The pulldown is a dropdown: it is as tall as what it shows, up to the
/// bounds its panes have.
#[must_use]
pub fn content_height(content: &Content) -> f32 {
    let mut height = 2.0 * metrics::MARGIN
        + metrics::HEADER
        + metrics::FOOTER
        + 2.0 * metrics::PANE_PADDING
        + metrics::BODY_GAPS;
    if content.status {
        height += metrics::STATUS_LINE;
    }
    if content.message {
        height += metrics::MESSAGE_LINE;
    }
    if content.warning {
        height += metrics::TRAY_WARNING_LINE;
    }
    if content.form {
        height += metrics::FORM_HEIGHT;
    } else {
        height += metrics::FLEET_GAPS + host_list_height(content.hosts);
        if content.selected {
            height += metrics::SELECTED_ROW;
        }
        height += detail_height(content);
    }
    height
}

/// The part of the display the panel may use.
///
/// The coordinates are **logical** pixels, in the same space as Slint's
/// window positions, so no scale factor is applied anywhere else.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkArea {
    /// The left edge, in logical pixels from the display's left.
    pub left: f32,
    /// The top edge, in logical pixels from the display's top.
    pub top: f32,
    /// The width, in logical pixels.
    pub width: f32,
    /// The height, in logical pixels.
    pub height: f32,
}

impl WorkArea {
    /// The whole area, with no margin.
    #[must_use]
    pub fn new(left: f32, top: f32, width: f32, height: f32) -> Self {
        Self {
            left,
            top,
            width,
            height,
        }
    }

    /// The right edge, in logical pixels.
    #[must_use]
    pub fn right(&self) -> f32 {
        self.left + self.width
    }

    /// The bottom edge, in logical pixels.
    #[must_use]
    pub fn bottom(&self) -> f32 {
        self.top + self.height
    }

    /// Whether a rectangle at this position and size fits entirely inside.
    #[must_use]
    pub fn holds(&self, position: (f32, f32), size: (f32, f32)) -> bool {
        let (x, y) = position;
        let (width, height) = size;
        x >= self.left && y >= self.top && x + width <= self.right() && y + height <= self.bottom()
    }

    /// Move a rectangle so that it sits entirely inside the area.
    ///
    /// A rectangle larger than the area is placed at the area's top-left
    /// corner: it cannot fit, and pretending otherwise would hide part of
    /// it off the other edge.
    #[must_use]
    pub fn clamp(&self, position: (f32, f32), size: (f32, f32)) -> (f32, f32) {
        let (mut x, mut y) = position;
        let (width, height) = size;

        if width >= self.width {
            x = self.left;
        } else {
            x = x.clamp(self.left, self.right() - width);
        }
        if height >= self.height {
            y = self.top;
        } else {
            y = y.clamp(self.top, self.bottom() - height);
        }
        let clamped = (x, y);
        if self.holds(clamped, size) {
            clamped
        } else {
            // A degenerate area — a display smaller than the panel, or a
            // zero-sized one — leaves nothing that fits. The corner is the
            // least-bad answer, and it is still on the display.
            (self.left, self.top)
        }
    }

    /// The corner this anchor hangs from.
    #[must_use]
    pub fn anchor_position(&self, anchor: Anchor, size: (f32, f32)) -> (f32, f32) {
        let (width, height) = size;
        let x = self.right() - width;
        let y = match anchor {
            Anchor::TopRight => self.top,
            Anchor::BottomRight => self.bottom() - height,
        };
        (x, y)
    }
}

/// Which corner of the display the panel hangs from.
///
/// Each platform constructs only one of the two — the top-trailing corner
/// everywhere but Windows, and the bottom-trailing corner on Windows — so
/// the other one is dead code there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Anchor {
    /// The top-trailing corner: the macOS menu bar and the usual Linux panel.
    TopRight,
    /// The bottom-trailing corner: the Windows taskbar.
    BottomRight,
}

impl Anchor {
    /// The anchor this platform's shell implies.
    ///
    /// The macOS menu bar is always at the top, so a tray pulldown opens
    /// downwards from it. The Windows notification area is in the taskbar,
    /// which is at the bottom, so its menu opens upwards. The Linux panel is
    /// at the top on the common desktops.
    #[must_use]
    pub fn for_platform() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::BottomRight
        }
        #[cfg(not(target_os = "windows"))]
        {
            Self::TopRight
        }
    }
}

/// The desktop shell's strip, in logical pixels, on the side the panel
/// anchors to.
///
/// The strip is subtracted so the panel opens clear of the menu bar, the
/// taskbar or the panel instead of under it. No API this GUI may use reports
/// its real size — `winit` 0.30 knows the monitor's bounds but not its work
/// area — so these are approximations, and they are recorded as
/// approximations.
#[cfg(target_os = "windows")]
const SHELL_INSET: f32 = 44.0;

/// The desktop shell's strip, in logical pixels, on the side the panel
/// anchors to. See the Windows note.
#[cfg(not(target_os = "windows"))]
const SHELL_INSET: f32 = 28.0;

/// The gap between the panel and the edge of the display.
const EDGE_MARGIN: f32 = 8.0;

/// The smallest the panel may be.
pub const MIN_PANEL_SIZE: (f32, f32) = (360.0, 220.0);

/// The work area of the display the window is on.
///
/// Returns `None` when the display cannot be asked — a backend that is not
/// winit, or a window that has no native window yet. The caller then leaves
/// the position to the window manager rather than guessing.
#[must_use]
pub fn work_area(window: &slint::Window) -> Option<WorkArea> {
    use slint::winit_030::WinitWindowAccessor;

    let scale = window.scale_factor();
    if scale <= 0.0 {
        return None;
    }

    let bounds = window.with_winit_window(|native| {
        let monitor = native.current_monitor()?;
        let size = monitor.size();
        let position = monitor.position();
        Some((position.x, position.y, size.width, size.height))
    })??;

    let (x, y, width, height) = bounds;
    let area = WorkArea::new(
        x as f32 / scale,
        y as f32 / scale,
        width as f32 / scale,
        height as f32 / scale,
    );
    Some(inset(area, Anchor::for_platform()))
}

/// Take the desktop shell's strip off the display's bounds.
///
/// The strip is on the side the panel anchors to: the menu bar is at the top
/// on macOS and Linux, the taskbar is at the bottom on Windows. Taking the
/// anchor as an argument rather than reading the platform here keeps this one
/// function, so both of its cases are tested on every platform — a platform
/// conditional here would only ever be exercised on that platform.
fn inset(area: WorkArea, anchor: Anchor) -> WorkArea {
    let width = (area.width - 2.0 * EDGE_MARGIN).max(0.0);
    let height = (area.height - 2.0 * EDGE_MARGIN - SHELL_INSET).max(0.0);
    match anchor {
        Anchor::TopRight => WorkArea::new(
            area.left + EDGE_MARGIN,
            area.top + EDGE_MARGIN + SHELL_INSET,
            width,
            height,
        ),
        Anchor::BottomRight => WorkArea::new(
            area.left + EDGE_MARGIN,
            area.top + EDGE_MARGIN,
            width,
            height,
        ),
    }
}

/// Where the panel should open.
///
/// A remembered position wins, because the operator may have moved the panel
/// and a pulldown that jumps back to a corner every time is worse than one
/// that stays where it was put. The remembered position is still clamped, so
/// a display that changed underneath the GUI cannot leave the panel
/// off-screen.
#[must_use]
pub fn open_position(
    remembered: Option<(f32, f32)>,
    size: (f32, f32),
    area: Option<&WorkArea>,
) -> Option<LogicalPosition> {
    let area = area?;
    let desired = match remembered {
        Some(position) => position,
        None => area.anchor_position(Anchor::for_platform(), size),
    };
    let (x, y) = area.clamp(desired, size);
    Some(LogicalPosition::new(x, y))
}

/// The panel's size, clamped so it fits the work area and never collapses.
///
/// A remembered size wins: it is the size the operator chose, and a
/// pulldown that forgets it is a pulldown that fights the operator. The
/// content's size is what the pulldown takes when the operator has never
/// resized it.
#[must_use]
pub fn panel_size(
    remembered: Option<(f32, f32)>,
    content: (f32, f32),
    area: Option<&WorkArea>,
) -> LogicalSize {
    let (mut width, mut height) = remembered.unwrap_or(content);
    if let Some(area) = area {
        width = width.min(area.width).max(MIN_PANEL_SIZE.0.min(area.width));
        height = height
            .min(area.height)
            .max(MIN_PANEL_SIZE.1.min(area.height));
    } else {
        width = width.max(MIN_PANEL_SIZE.0);
        height = height.max(MIN_PANEL_SIZE.1);
    }
    LogicalSize::new(width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1920x1080 display with a 28-pixel shell strip at the top.
    fn area() -> WorkArea {
        WorkArea::new(0.0, 0.0, 1920.0, 1080.0)
    }

    #[test]
    fn a_rectangle_inside_the_area_is_left_alone() {
        let area = area();
        assert_eq!(area.clamp((100.0, 100.0), (560.0, 520.0)), (100.0, 100.0));
    }

    #[test]
    fn a_rectangle_past_the_right_edge_is_pulled_back() {
        let area = area();
        let (x, _) = area.clamp((1800.0, 100.0), (560.0, 520.0));
        assert_eq!(x, 1360.0, "the panel's right edge lands on the display's");
        assert!(area.holds((x, 100.0), (560.0, 520.0)));
    }

    #[test]
    fn a_rectangle_past_the_bottom_edge_is_pulled_back() {
        let area = area();
        let (_, y) = area.clamp((100.0, 1000.0), (560.0, 520.0));
        assert_eq!(y, 560.0);
        assert!(area.holds((100.0, y), (560.0, 520.0)));
    }

    #[test]
    fn a_rectangle_before_the_top_left_corner_is_pushed_in() {
        let area = area();
        let (x, y) = area.clamp((-50.0, -20.0), (560.0, 520.0));
        assert_eq!((x, y), (0.0, 0.0));
    }

    #[test]
    fn a_rectangle_bigger_than_the_area_sits_at_its_corner() {
        let area = WorkArea::new(0.0, 0.0, 200.0, 100.0);
        let (x, y) = area.clamp((50.0, 50.0), (560.0, 520.0));
        assert_eq!((x, y), (0.0, 0.0));
    }

    #[test]
    fn the_top_right_anchor_hangs_from_the_top_trailing_corner() {
        let area = area();
        let (x, y) = area.anchor_position(Anchor::TopRight, (560.0, 520.0));
        assert_eq!((x, y), (1360.0, 0.0));
    }

    #[test]
    fn the_bottom_right_anchor_hangs_from_the_bottom_trailing_corner() {
        let area = area();
        let (x, y) = area.anchor_position(Anchor::BottomRight, (560.0, 520.0));
        assert_eq!((x, y), (1360.0, 560.0));
    }

    #[test]
    fn a_remembered_position_wins_over_the_anchor() {
        let area = area();
        let position =
            open_position(Some((10.0, 20.0)), (560.0, 520.0), Some(&area)).expect("a position");
        assert_eq!((position.x, position.y), (10.0, 20.0));
    }

    #[test]
    fn a_remembered_position_is_still_clamped() {
        let area = area();
        let position =
            open_position(Some((5000.0, 5000.0)), (560.0, 520.0), Some(&area)).expect("a position");
        assert!(area.holds((position.x, position.y), (560.0, 520.0)));
    }

    #[test]
    fn without_a_remembered_position_the_anchor_is_used() {
        let area = area();
        let position = open_position(None, (560.0, 520.0), Some(&area)).expect("a position");
        let expected = area.anchor_position(Anchor::for_platform(), (560.0, 520.0));
        assert_eq!((position.x, position.y), expected);
    }

    #[test]
    fn without_a_work_area_there_is_no_position_to_claim() {
        assert!(open_position(None, (560.0, 520.0), None).is_none());
    }

    #[test]
    fn the_shell_strip_is_taken_off_the_side_the_panel_anchors_to() {
        let display = WorkArea::new(0.0, 0.0, 1920.0, 1080.0);

        // The menu bar is at the top on macOS and Linux, so the strip comes
        // off the top there.
        let top = inset(display, Anchor::TopRight);
        assert!(top.right() <= display.right());
        assert!(top.bottom() <= display.bottom());
        assert!(
            top.top >= display.top + EDGE_MARGIN + SHELL_INSET,
            "the menu bar's strip is the shell's at the top"
        );

        // The taskbar is at the bottom on Windows, so the strip comes off the
        // bottom there. Both cases are checked on every platform, so a
        // platform conditional in the arithmetic cannot hide here.
        let bottom = inset(display, Anchor::BottomRight);
        assert!(bottom.right() <= display.right());
        assert!(bottom.bottom() <= display.bottom());
        assert!(
            bottom.bottom() <= display.bottom() - EDGE_MARGIN - SHELL_INSET,
            "the taskbar's strip is the shell's at the bottom"
        );
    }

    #[test]
    fn the_platforms_anchor_on_the_side_their_shell_lives() {
        // The macOS menu bar and the usual Linux panel are at the top; the
        // Windows taskbar is at the bottom.
        #[cfg(target_os = "windows")]
        assert_eq!(Anchor::for_platform(), Anchor::BottomRight);
        #[cfg(not(target_os = "windows"))]
        assert_eq!(Anchor::for_platform(), Anchor::TopRight);
    }

    #[test]
    fn the_panel_never_collapses() {
        let size = panel_size(Some((10.0, 10.0)), (420.0, 300.0), None);
        assert!(size.width >= MIN_PANEL_SIZE.0);
        assert!(size.height >= MIN_PANEL_SIZE.1);
    }

    #[test]
    fn the_panel_is_never_bigger_than_the_work_area() {
        let area = WorkArea::new(0.0, 0.0, 400.0, 300.0);
        let size = panel_size(Some((2000.0, 2000.0)), (420.0, 300.0), Some(&area));
        assert!(size.width <= 400.0);
        assert!(size.height <= 300.0);
    }

    #[test]
    fn a_remembered_size_wins_over_the_content() {
        let size = panel_size(Some((500.0, 400.0)), (420.0, 300.0), None);
        assert_eq!((size.width, size.height), (500.0, 400.0));
    }

    #[test]
    fn the_content_size_is_used_when_the_operator_never_resized() {
        let size = panel_size(None, (420.0, 300.0), None);
        assert_eq!((size.width, size.height), (420.0, 300.0));
    }

    #[test]
    fn the_height_follows_the_fleet() {
        let empty = content_height(&Content::default());
        let one = content_height(&Content {
            hosts: 1,
            ..Content::default()
        });
        let many = content_height(&Content {
            hosts: 20,
            ..Content::default()
        });
        assert!(one > empty, "one host is taller than none");
        assert!(
            many > one,
            "twenty hosts are taller than one, up to the bound"
        );
    }

    #[test]
    fn the_height_is_bounded_by_the_host_list() {
        let a_fleet = content_height(&Content {
            hosts: 200,
            ..Content::default()
        });
        let the_bound = content_height(&Content {
            hosts: 10,
            ..Content::default()
        });
        assert_eq!(
            a_fleet, the_bound,
            "a fleet past the host list's bound does not make the pulldown taller"
        );
    }

    #[test]
    fn the_detail_is_bounded() {
        let few = content_height(&Content {
            hosts: 1,
            metrics: 1,
            ..Content::default()
        });
        let many = content_height(&Content {
            hosts: 1,
            metrics: 500,
            ..Content::default()
        });
        assert!(many > few, "more rows make the pulldown taller");
        assert!(
            many - few <= metrics::MAX_DETAIL,
            "the detail's own bound still holds"
        );
    }

    #[test]
    fn the_form_is_shorter_than_the_fleet() {
        let fleet = content_height(&Content {
            hosts: 5,
            ..Content::default()
        });
        let form = content_height(&Content {
            form: true,
            ..Content::default()
        });
        assert!(form < fleet, "a form does not carry the fleet");
    }

    #[test]
    fn the_slint_file_says_what_this_module_says() {
        // The pulldown's height is computed from the numbers in `metrics`,
        // and the `.slint` file is what gives the pulldown its elements'
        // heights. If the two drift apart, the pulldown is either cut off or
        // has a gap in it, and nothing else would say so.
        let slint = include_str!("ui/main.slint");
        for (what, expected) in [
            ("the margin", "margin: 14px"),
            ("the padding", "pane-padding: 12px"),
            ("the spacing", "spacing: 6px"),
            ("the header", "height: 36px"),
            ("the footer", "height: 18px"),
            ("the host row", "height: 46px"),
            ("the job row", "height: 22px"),
            ("the metric row", "metric-row-height: 18px"),
            ("the metric bar", "metric-bar-height: 6px"),
            ("the status line", "0px : 18px"),
            ("the message", "0px : 30px"),
            ("the tray warning", "0px : 18px"),
            ("the selected host row", "0px : 18px"),
            ("the form's height", "form-height: 220px"),
        ] {
            assert!(
                slint.contains(expected),
                "the `.slint` file no longer says {what} is `{expected}`"
            );
        }
        assert_eq!(metrics::MARGIN, 14.0);
        assert_eq!(metrics::PANE_PADDING, 12.0);
        assert_eq!(metrics::BODY_SPACING, 6.0);
        assert_eq!(metrics::HEADER, 36.0);
        assert_eq!(metrics::FOOTER, 18.0);
        assert_eq!(metrics::HOST_ROW, 46.0);
        assert_eq!(metrics::JOB_ROW, 22.0);
        assert_eq!(metrics::STATUS_LINE, 18.0);
        assert_eq!(metrics::MESSAGE_LINE, 30.0);
        assert_eq!(metrics::TRAY_WARNING_LINE, 18.0);
        assert_eq!(metrics::SELECTED_ROW, 18.0);
        assert_eq!(metrics::FORM_HEIGHT, 220.0);
    }
}
