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
//! corner that resizes it. The operator's chosen placement and size are
//! remembered, and the placement is re-clamped every time the panel opens, so
//! a display that changed underneath the GUI cannot leave the panel
//! off-screen.
//!
//! # What is remembered is an offset, not a position
//!
//! A position is a place on *one* display. Remembering one and opening it on
//! a *different* display puts the panel in the wrong place — and the wrong
//! place is not obviously wrong: it is on screen, it is the right size, and
//! it is nowhere near the tray whose click opened it. The panel that was
//! placed on a laptop's 1512-point display and opened on a 3840-point one
//! landed 2382 points from the tray, which is the middle of the desk.
//!
//! So what is remembered is an [`Offset`]: how far the panel sits from the
//! **anchor corner of the display it is on**. An offset means the same thing
//! on every display, at every scale, on either side of a resolution change,
//! and it is re-clamped on every open so a display that shrank cannot leave
//! the panel off-screen.
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
    pub const HEADER: f32 = 28.0;

    /// The footer's height.
    ///
    /// It is the links' own height plus the padding that keeps them off the
    /// card's bottom edge.
    pub const FOOTER: f32 = 24.0;

    /// The padding inside the card, on each side.
    pub const PANE_PADDING: f32 = 12.0;

    /// The padding above the body's content.
    ///
    /// It is smaller than the padding on the sides: the header strip is
    /// already above the content, and the space between the title and the
    /// first host is a gap, not a margin.
    pub const PANE_PADDING_TOP: f32 = 6.0;

    /// The padding below the body's content.
    pub const PANE_PADDING_BOTTOM: f32 = 12.0;

    /// The gap between the body's groups.
    ///
    /// The body's own layout spacing is zero, because a layout adds its
    /// spacing between **every** cell and a cell that is empty still takes
    /// part in the layout: three empty notice rows would push the fleet down
    /// by eighteen pixels and say nothing at all. Each group carries the gap
    /// that follows it instead, so the gap is there only when there is
    /// something on both sides of it.
    pub const BODY_SPACING: f32 = 6.0;

    /// The gap inside the fleet or the detail: between the table's headings
    /// and its rows, or between the selected host's actions and its panes.
    pub const BLOCK_GAP: f32 = BODY_SPACING;

    /// The attachment's state line.
    pub const STATUS_LINE: f32 = 18.0;

    /// The last action's message, which takes two lines.
    pub const MESSAGE_LINE: f32 = 30.0;

    /// The tray warning's line.
    pub const TRAY_WARNING_LINE: f32 = 18.0;

    /// The height of one host's header line in the fleet view.
    ///
    /// The header is the host's name and the readings that are not used
    /// shares: the I/O wait, the load, the core count and the frequency.
    pub const HOST_HEADER: f32 = 16.0;

    /// The height of one measured row in a host's block.
    ///
    /// A row is a label, a bar and the bar's share, so it is one line.
    pub const FLEET_ROW: f32 = 13.0;

    /// The height of one host's block: its header and its five rows.
    pub const HOST_BLOCK: f32 = HOST_HEADER + 5.0 * FLEET_ROW;

    /// The gap between two hosts' blocks.
    pub const HOST_BLOCK_GAP: f32 = 6.0;

    /// The most the host list may be tall.
    ///
    /// The list is bounded so that a large fleet cannot push the pulldown off
    /// the desktop. The bound is a number of pixels, so the list scrolls when
    /// the fleet is larger than the bound, and a block at the edge may be
    /// half in view — which is the scroll's own sign that there is more.
    pub const HOST_LIST_MAX_HEIGHT: f32 = 4.0 * (HOST_BLOCK + HOST_BLOCK_GAP);

    /// The height of one group's heading in the detail view.
    ///
    /// A group is a heading and the rows under it, and the heading is what
    /// makes a list of facts readable.
    pub const DETAIL_HEADING: f32 = 15.0;

    /// One row of a group: a name and its value.
    pub const DETAIL_ROW: f32 = 15.0;

    /// One scheduler job row.
    pub const JOB_ROW: f32 = 22.0;

    /// One line of retained scheduler records.
    pub const RECORD_LINE: f32 = 14.0;

    /// The attribution text, which takes four lines.
    pub const ABOUT_LINES: f32 = 4.0;
    pub const ABOUT_LINE: f32 = 14.0;

    /// The form's height.
    pub const FORM_HEIGHT: f32 = 220.0;

    /// The width the pane's content gets, in logical pixels.
    ///
    /// It is the fleet table's rows: the label column, the bar and the share
    /// column. The vertical scrollbar takes its own room out of this width
    /// when the fleet is long enough to scroll. The table is measured, not
    /// guessed, so the width follows it.
    pub const PANE_CONTENT_WIDTH: f32 = 554.0;

    /// The pulldown's width.
    ///
    /// It is the pane's content width plus the card's own chrome: the pane's
    /// padding on each side and the shadow's margin.
    pub const DEFAULT_WIDTH: f32 = PANE_CONTENT_WIDTH + 2.0 * PANE_PADDING + 2.0 * MARGIN;

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
    /// How many of them answered their last poll.
    ///
    /// An online host's block is a header and five rows; a host that is not
    /// online has a header alone, because it has no readings to show.
    pub online_hosts: usize,
    /// Whether a host is selected.
    pub selected: bool,
    /// Whether the selected host answered its last poll.
    ///
    /// A host that is not online has a header line alone in its block, so the
    /// block is shorter — and the detail pane is shorter with it.
    pub selected_online: bool,
    /// How many groups of detail the selected host has.
    pub detail_sections: usize,
    /// How many rows those groups hold.
    pub detail_rows: usize,
    /// How many scheduler jobs it has.
    pub jobs: usize,
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
    /// Whether the selected host's detail is showing instead of the fleet.
    pub detail: bool,
}

/// The height of the bounded host list.
///
/// The list is the fleet's blocks: an online host's block is a header line and
/// five rows, and a host that is not online has a header line alone. The
/// height is bounded, so a large fleet scrolls instead of pushing the
/// pulldown off the desktop.
#[must_use]
pub fn host_list_height(content: &Content) -> f32 {
    let online = content.online_hosts.min(content.hosts);
    let offline = content.hosts - online;
    let height = online as f32 * metrics::HOST_BLOCK
        + offline as f32 * metrics::HOST_HEADER
        + content.hosts.saturating_sub(1) as f32 * metrics::HOST_BLOCK_GAP;
    height.min(metrics::HOST_LIST_MAX_HEIGHT)
}

/// The height of the detail below the host list.
///
/// The detail is one scroll container, so this is an estimate of what it
/// holds: the estimate only decides how much of it is on screen, and the
/// scroll container keeps the rest reachable when the estimate is short.
///
/// It is the selected host's **own block** — the same block the fleet view
/// draws — and then the groups of detail under it.
#[must_use]
fn detail_height(content: &Content) -> f32 {
    let mut height = if content.selected_online {
        metrics::HOST_BLOCK
    } else {
        metrics::HOST_HEADER
    };
    height += content.detail_sections as f32 * metrics::DETAIL_HEADING;
    height += content.detail_rows as f32 * metrics::DETAIL_ROW;
    height += content.jobs as f32 * metrics::JOB_ROW;
    height += content.record_lines as f32 * metrics::RECORD_LINE;
    height.clamp(metrics::MIN_DETAIL, metrics::MAX_DETAIL)
}

/// The height of the notices above the content.
///
/// The notices are the attachment's state, the last action's message and the
/// tray's warning. They are one group in the body's layout, and they carry
/// their own gaps: the gap between two notices, and the gap that separates
/// them from the content below.
///
/// A fleet that is healthy has no notices at all, and then the group takes
/// **no room**: a group that is empty but still takes its gap is a band of
/// dead space between the title and the fleet.
#[must_use]
pub fn notices_height(content: &Content) -> f32 {
    let mut height = 0.0;
    if content.status {
        height += metrics::STATUS_LINE;
    }
    if content.message {
        height += metrics::MESSAGE_LINE;
        // The gap above the message, when there is a status line above it.
        if content.status {
            height += metrics::BODY_SPACING;
        }
    }
    if content.warning {
        height += metrics::TRAY_WARNING_LINE;
        // The gap above the warning, when there is a message above it.
        if content.message {
            height += metrics::BODY_SPACING;
        }
    }
    // The gap that separates the notices from the content below them.
    if content.status || content.message || content.warning {
        height += metrics::BODY_SPACING;
    }
    height
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
        + metrics::PANE_PADDING_TOP
        + metrics::PANE_PADDING_BOTTOM;
    height += notices_height(content);
    if content.about {
        // The attribution is outside the fleet and the detail, so it is
        // readable from either of them, and it carries the gap above it.
        height += metrics::ABOUT_LINES * metrics::ABOUT_LINE + metrics::BODY_SPACING;
    }
    if content.form {
        height += metrics::FORM_HEIGHT;
    } else if content.detail {
        // The detail view: the selected host's own block and the groups of
        // detail under it.
        height += metrics::BLOCK_GAP;
        height += detail_height(content);
    } else {
        // The fleet view: the blocks, one per host.
        height += host_list_height(content);
    }
    height
}

/// Where the panel sits, relative to the display's anchor corner.
///
/// The offset is measured in logical pixels, and it is measured to the
/// **edges** rather than to the panel's own corner:
///
/// - `x` is the gap between the panel's **trailing** edge and the work area's
///   trailing edge. Zero means the panel's trailing edge is flush with the
///   display's; a positive number means the panel sits that far inside it.
/// - `y` is the gap between the panel's **anchor-side** edge and the work
///   area's anchor-side edge: the panel's top edge against the work area's
///   top on macOS and Linux, and its bottom edge against the work area's
///   bottom on Windows.
///
/// The default offset is zero, and a zero offset *is* the anchor corner.
///
/// # Why an offset and not a position
///
/// A position names a place on one display; an offset names a place
/// *relative to the display's own corner*, so it survives a change of
/// display, of resolution and of scale. See the module's own notes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Offset {
    /// The gap at the trailing edge, in logical pixels.
    pub x: f32,
    /// The gap at the anchor edge, in logical pixels.
    pub y: f32,
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
        self.place(Offset::default(), size, anchor)
    }

    /// Where a panel with this offset goes.
    ///
    /// The offset is measured to the panel's edges, so the panel's trailing
    /// edge stays where the offset says and the panel's anchor-side edge
    /// stays where the offset says. That is what makes the panel grow *away*
    /// from the corner when its content gets taller.
    #[must_use]
    pub fn place(&self, offset: Offset, size: (f32, f32), anchor: Anchor) -> (f32, f32) {
        let (width, height) = size;
        let x = self.right() - offset.x - width;
        let y = match anchor {
            Anchor::TopRight => self.top + offset.y,
            Anchor::BottomRight => self.bottom() - offset.y - height,
        };
        (x, y)
    }

    /// The offset a panel at this position has.
    ///
    /// This is the inverse of [`WorkArea::place`]: placing an offset and
    /// measuring it back gives the same position, for either anchor.
    #[must_use]
    pub fn offset_of(&self, position: (f32, f32), size: (f32, f32), anchor: Anchor) -> Offset {
        let (x, y) = position;
        let (width, height) = size;
        Offset {
            x: self.right() - (x + width),
            y: match anchor {
                Anchor::TopRight => y - self.top,
                Anchor::BottomRight => self.bottom() - (y + height),
            },
        }
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
/// A remembered offset wins, because the operator may have moved the panel
/// and a pulldown that jumps back to a corner every time is worse than one
/// that stays where it was put. The offset is relative to the display's own
/// anchor corner, so it means the same thing on every display; the result is
/// still clamped, so a display that changed underneath the GUI cannot leave
/// the panel off-screen.
#[must_use]
pub fn open_position(
    remembered: Option<Offset>,
    size: (f32, f32),
    area: Option<&WorkArea>,
) -> Option<LogicalPosition> {
    let area = area?;
    let anchor = Anchor::for_platform();
    let desired = match remembered {
        Some(offset) => area.place(offset, size, anchor),
        None => area.anchor_position(anchor, size),
    };
    let (x, y) = area.clamp(desired, size);
    Some(LogicalPosition::new(x, y))
}

/// The panel's size, clamped so it fits the work area and never collapses.
///
/// The size is the **content's** size: a pulldown is a dropdown, and a
/// dropdown is as big as what it carries. There is no grip in its corner to
/// drag, so there is no size of the operator's to remember — and a size that
/// was remembered and could not be changed again is a size that goes stale
/// the moment the fleet changes.
///
/// The width has a floor: the fleet table needs its own width, and a panel
/// narrower than that would clip the columns rather than lay them out.
#[must_use]
pub fn panel_size(content: (f32, f32), area: Option<&WorkArea>) -> LogicalSize {
    let (mut width, mut height) = content;
    width = width.max(metrics::DEFAULT_WIDTH);
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
    fn a_remembered_offset_wins_over_the_anchor() {
        let area = area();
        let position = open_position(
            Some(Offset { x: 10.0, y: 20.0 }),
            (560.0, 520.0),
            Some(&area),
        )
        .expect("a position");
        assert_eq!(
            (position.x, position.y),
            area.place(
                Offset { x: 10.0, y: 20.0 },
                (560.0, 520.0),
                Anchor::for_platform()
            )
        );
    }

    #[test]
    fn a_remembered_offset_is_still_clamped() {
        let area = area();
        let position = open_position(
            Some(Offset {
                x: 5000.0,
                y: 5000.0,
            }),
            (560.0, 520.0),
            Some(&area),
        )
        .expect("a position");
        assert!(area.holds((position.x, position.y), (560.0, 520.0)));
    }

    #[test]
    fn without_a_remembered_offset_the_anchor_is_used() {
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
    fn an_offset_is_measured_back_to_the_position_it_came_from() {
        // The two are inverses, for either anchor: the offset describes the
        // placement, and the placement describes the offset, and neither
        // loses anything on the way round.
        let area = area();
        for anchor in [Anchor::TopRight, Anchor::BottomRight] {
            for position in [(0.0, 0.0), (100.0, 250.0), (1360.0, 560.0)] {
                let offset = area.offset_of(position, (560.0, 520.0), anchor);
                assert_eq!(
                    area.place(offset, (560.0, 520.0), anchor),
                    position,
                    "the round trip is exact"
                );
            }
        }
    }

    #[test]
    fn the_same_offset_means_the_same_thing_on_a_wider_display() {
        // This is the whole point of the offset: a placement remembered on a
        // small display opens against the corner of a large one, and not
        // against the middle of it.
        let small = WorkArea::new(0.0, 0.0, 1512.0, 982.0);
        let large = WorkArea::new(0.0, 0.0, 3840.0, 1080.0);
        let offset = Offset::default();

        let (small_x, _) = small.place(offset, (606.0, 478.0), Anchor::TopRight);
        let (large_x, _) = large.place(offset, (606.0, 478.0), Anchor::TopRight);
        assert_eq!(
            small_x, 906.0,
            "the panel is against the small display's corner"
        );
        assert_eq!(large_x, 3234.0, "and against the large one's");
        assert_ne!(
            small_x, large_x,
            "an absolute position would have put it in the middle of the large display"
        );
    }

    #[test]
    fn the_trailing_edge_stays_put_when_the_panel_gets_wider() {
        // The offset is measured to the trailing edge, so a panel that grows
        // wider grows away from the corner and keeps its place against it.
        let area = area();
        let offset = area.offset_of((1360.0, 0.0), (560.0, 520.0), Anchor::TopRight);
        let (x, _) = area.place(offset, (660.0, 520.0), Anchor::TopRight);
        assert_eq!(x + 660.0, 1360.0 + 560.0, "the trailing edge did not move");
    }

    #[test]
    fn the_anchor_edge_stays_put_when_the_panel_gets_taller() {
        // The same, the other way: the panel grows away from the edge it
        // hangs from, so a taller fleet does not walk the pulldown down the
        // screen.
        let area = area();
        let offset = area.offset_of((1360.0, 0.0), (560.0, 520.0), Anchor::TopRight);
        let (_, y) = area.place(offset, (560.0, 720.0), Anchor::TopRight);
        assert_eq!(y, 0.0, "the top edge did not move");
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
        let size = panel_size((10.0, 10.0), None);
        assert!(size.width >= MIN_PANEL_SIZE.0);
        assert!(size.height >= MIN_PANEL_SIZE.1);
    }

    #[test]
    fn the_panel_is_never_bigger_than_the_work_area() {
        let area = WorkArea::new(0.0, 0.0, 400.0, 300.0);
        let size = panel_size((2000.0, 2000.0), Some(&area));
        assert!(size.width <= 400.0);
        assert!(size.height <= 300.0);
    }

    #[test]
    fn the_panel_is_never_narrower_than_the_fleet_table() {
        // The fleet table's columns have to fit: a pulldown narrower than
        // the table would clip the columns rather than lay them out.
        let size = panel_size((320.0, 400.0), None);
        assert_eq!(size.width, metrics::DEFAULT_WIDTH);
    }

    #[test]
    fn the_content_decides_the_size() {
        // There is no grip in the corner, so the size is the content's size
        // and nothing else: a pulldown that remembered a size it could not
        // change again would go stale the moment the fleet changed.
        let size = panel_size((700.0, 300.0), None);
        assert_eq!((size.width, size.height), (700.0, 300.0));
    }

    #[test]
    fn the_height_follows_the_fleet() {
        let empty = content_height(&Content::default());
        let one = content_height(&Content {
            hosts: 1,
            online_hosts: 1,
            ..Content::default()
        });
        let many = content_height(&Content {
            hosts: 20,
            online_hosts: 20,
            ..Content::default()
        });
        assert!(one > empty, "one host is taller than none");
        assert!(
            many > one,
            "twenty hosts are taller than one, up to the bound"
        );
    }

    #[test]
    fn an_offline_host_is_shorter_than_an_online_one() {
        // A host that is not online has a header line alone: it has no
        // readings to show, so it takes no room for rows it cannot fill.
        let online = content_height(&Content {
            hosts: 1,
            online_hosts: 1,
            ..Content::default()
        });
        let offline = content_height(&Content {
            hosts: 1,
            online_hosts: 0,
            ..Content::default()
        });
        assert!(
            online > offline,
            "an offline host's block is its header line and nothing more"
        );
    }

    #[test]
    fn the_height_is_bounded_by_the_host_list() {
        let a_fleet = content_height(&Content {
            hosts: 200,
            online_hosts: 200,
            ..Content::default()
        });
        let the_bound = content_height(&Content {
            hosts: 10,
            online_hosts: 10,
            ..Content::default()
        });
        assert_eq!(
            a_fleet, the_bound,
            "a fleet past the host list's bound does not make the pulldown taller"
        );
    }

    /// A healthy fleet has no notices, and no notices take no room.
    #[test]
    fn no_notices_take_no_room() {
        assert_eq!(notices_height(&Content::default()), 0.0);
        let healthy = content_height(&Content {
            hosts: 4,
            online_hosts: 4,
            ..Content::default()
        });
        let with_a_notice = content_height(&Content {
            hosts: 4,
            online_hosts: 4,
            status: true,
            ..Content::default()
        });
        assert_eq!(
            with_a_notice - healthy,
            metrics::STATUS_LINE + metrics::BODY_SPACING,
            "the notice is a line and the gap that follows it"
        );
    }

    /// Two notices are a gap apart, and the gap is only there when both are.
    #[test]
    fn two_notices_are_a_gap_apart() {
        let one = notices_height(&Content {
            status: true,
            ..Content::default()
        });
        let two = notices_height(&Content {
            status: true,
            message: true,
            ..Content::default()
        });
        assert_eq!(
            two - one,
            metrics::MESSAGE_LINE + metrics::BODY_SPACING,
            "the message is a line and the gap above it"
        );
    }

    /// The fleet's title and its first host are close together.
    ///
    /// The gap between them is the header strip's own padding and the body's
    /// top padding, and nothing else: the panel used to carry a band of dead
    /// space there, and it was the first thing anyone noticed.
    #[test]
    fn the_title_and_the_first_host_are_close_together() {
        // The header strip's height and the body's top padding are the whole
        // of it.
        let gap = metrics::HEADER - 16.0 + metrics::PANE_PADDING_TOP;
        assert!(
            gap <= 24.0,
            "the gap between the title and the first host is {gap} pixels"
        );
    }

    #[test]
    fn the_detail_is_bounded() {
        let few = content_height(&Content {
            detail: true,
            detail_sections: 1,
            detail_rows: 1,
            ..Content::default()
        });
        let many = content_height(&Content {
            detail: true,
            detail_sections: 500,
            detail_rows: 500,
            ..Content::default()
        });
        assert!(many > few, "more rows make the pulldown taller");
        assert!(
            many - few <= metrics::MAX_DETAIL,
            "the detail's own bound still holds"
        );
    }

    #[test]
    fn the_fleet_view_does_not_carry_the_detail() {
        // The detail is behind the pulldown's own control, so the fleet
        // view's height is the host list and nothing else.
        let fleet = content_height(&Content {
            hosts: 5,
            detail_sections: 500,
            detail_rows: 500,
            ..Content::default()
        });
        let without_detail = content_height(&Content {
            hosts: 5,
            ..Content::default()
        });
        assert_eq!(
            fleet, without_detail,
            "the fleet view does not grow with the detail's rows"
        );
    }

    #[test]
    fn the_form_does_not_carry_the_fleet() {
        let few = content_height(&Content {
            form: true,
            hosts: 1,
            ..Content::default()
        });
        let many = content_height(&Content {
            form: true,
            hosts: 200,
            ..Content::default()
        });
        assert_eq!(few, many, "a form does not carry the fleet");
    }

    /// The room the vertical scrollbar takes, as the `.slint` file says it.
    ///
    /// It lives on the pulldown and not on the block, because it is the
    /// pulldown's own scrollbar.
    fn scrollbar_reserve(slint: &str) -> f32 {
        let needle = "scrollbar-reserve: ";
        let start = slint
            .find(needle)
            .unwrap_or_else(|| panic!("the `.slint` file has no `scrollbar-reserve`"))
            + needle.len();
        let rest = &slint[start..];
        let end = rest.find(';').expect("the literal ends");
        rest[..end]
            .trim()
            .trim_end_matches("px")
            .parse()
            .expect("the scrollbar's reserve is a number")
    }

    #[test]
    fn the_bar_takes_the_rest_of_the_row() {
        // The bar reaches the panel's own edge: it takes everything the row
        // does not spend on the label, the share and the gaps between them.
        //
        // The numbers are read out of the `HostBlock` component, because that
        // is where the block's geometry lives now — the fleet view and the
        // detail view draw their host with it, so one set of numbers is the
        // whole panel's.
        let slint = include_str!("ui/main.slint");
        let block = slint
            .split("component HostBlock inherits Rectangle {")
            .nth(1)
            .and_then(|rest| rest.split("component SectionHeading").next())
            .expect("the `.slint` file has a `HostBlock` component");
        let literal = |name: &str| -> f32 {
            let needle = format!("{name}: ");
            let start = block
                .find(&needle)
                .unwrap_or_else(|| panic!("the block has no `{name}`"))
                + needle.len();
            let rest = &block[start..];
            let end = rest.find(';').expect("the literal ends");
            rest[..end]
                .trim()
                .trim_end_matches("px")
                .parse()
                .unwrap_or_else(|_| panic!("`{name}` is not a number"))
        };

        let content = metrics::DEFAULT_WIDTH
            - 2.0 * metrics::MARGIN
            - 2.0 * metrics::PANE_PADDING
            - scrollbar_reserve(slint);
        let label = literal("label-width");
        let pct = literal("pct-width");
        let gap = literal("gap");
        let bar = content - label - pct - 3.0 * gap;

        assert!(bar > 0.0, "the bar has room in the row");
        assert!(
            bar > content / 2.0,
            "the bar is the row's own width, not a column of it"
        );
        assert!(
            block.contains(
                "bar-width: root.content-width - root.label-width - root.pct-width - 3 * root.gap"
            ),
            "the bar's width is the row less the label, the share and the gaps"
        );
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
            ("the padding at the top", "pane-padding-top: 6px"),
            ("the padding at the bottom", "pane-padding-bottom: 12px"),
            ("the gap between the body's groups", "body-gap: 6px"),
            ("the spacing", "spacing: 6px"),
            ("the header", "height: 28px"),
            ("the footer", "height: 24px"),
            ("the footer's bottom padding", "padding-bottom: 6px"),
            ("the host's header line", "header-height: 16px"),
            ("the gap between two hosts", "host-block-gap: 6px"),
            ("the fleet's row", "row-height: 13px"),
            ("the fleet's bar", "bar-height: 6px"),
            ("the label's width", "label-width: 64px"),
            ("the share's width", "pct-width: 32px"),
            ("the fleet's gap", "gap: 6px"),
            ("the job row", "height: 22px"),
            ("the group's heading", "height: 15px"),
            ("the status line", "0px : 18px"),
            ("the message", "0px : 30px"),
            ("the tray warning", "0px : 18px"),
            ("the form's height", "form-height: 220px"),
        ] {
            assert!(
                slint.contains(expected),
                "the `.slint` file no longer says {what} is `{expected}`"
            );
        }
        assert_eq!(metrics::MARGIN, 14.0);
        assert_eq!(metrics::PANE_PADDING, 12.0);
        assert_eq!(metrics::PANE_PADDING_TOP, 6.0);
        assert_eq!(metrics::PANE_PADDING_BOTTOM, 12.0);
        assert_eq!(metrics::BODY_SPACING, 6.0);
        assert_eq!(metrics::HEADER, 28.0);
        assert_eq!(metrics::FOOTER, 24.0);
        assert_eq!(metrics::HOST_HEADER, 16.0);
        assert_eq!(metrics::FLEET_ROW, 13.0);
        assert_eq!(metrics::HOST_BLOCK, 81.0);
        assert_eq!(metrics::HOST_BLOCK_GAP, 6.0);
        assert_eq!(metrics::HOST_LIST_MAX_HEIGHT, 348.0);
        assert_eq!(metrics::JOB_ROW, 22.0);
        assert_eq!(metrics::DETAIL_HEADING, 15.0);
        assert_eq!(metrics::DETAIL_ROW, 15.0);
        assert_eq!(metrics::STATUS_LINE, 18.0);
        assert_eq!(metrics::MESSAGE_LINE, 30.0);
        assert_eq!(metrics::TRAY_WARNING_LINE, 18.0);
        assert_eq!(metrics::FORM_HEIGHT, 220.0);
        assert_eq!(metrics::DEFAULT_WIDTH, 606.0);
    }

    /// The body's layout spacing is zero, and it has to stay zero.
    ///
    /// A layout adds its spacing between **every** cell, and a cell that is
    /// empty still takes part in the layout. The body's notices are one cell
    /// that is empty most of the time, so any spacing in the body's layout
    /// is a band of dead space between the title and the fleet — the band the
    /// panel used to have, and the one this removes.
    #[test]
    fn the_body_adds_no_spacing_of_its_own() {
        let slint = include_str!("ui/main.slint");
        let body = slint
            .split("// --------------------------------------------------- the body")
            .nth(1)
            .and_then(|rest| rest.split("if root.mode == PanelMode.Fleet").next())
            .expect("the `.slint` file has a body");
        assert!(
            body.contains("spacing: 0px"),
            "the body's own spacing is zero, and every group carries its own gap"
        );
    }

    /// The corner grip is gone.
    ///
    /// It was a hairline square in the corner that looked like a button and
    /// did nothing anybody could see, and the size it controlled was a size
    /// nobody could change again once the fleet changed.
    #[test]
    fn there_is_no_grip_in_the_corner() {
        let slint = include_str!("ui/main.slint");
        assert!(
            !slint.contains("resized"),
            "the grip is gone, and so is the callback it owned"
        );
        // The card's own hairline is the only border left in the panel: a
        // second one was the corner square.
        assert_eq!(
            slint.matches("border-width: 1px").count(),
            1,
            "the card's own edge is the only border the panel draws"
        );
    }
}
