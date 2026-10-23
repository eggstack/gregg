//! Where the operator left the pulldown.
//!
//! # Why this exists
//!
//! The pulldown's position and its size are the operator's: they can drag it
//! somewhere they like and drag its corner to a size they like. Slint keeps
//! both in the component, and the component does not outlive the process — so
//! without a file, the placement is forgotten on every restart, and a
//! pulldown that jumps back to a corner every time is worse than one that
//! stays where it was put.
//!
//! # Where the file is
//!
//! Beside the configuration, and named after it: `<config-path>.gui-state`.
//! Deriving the name from the configuration's path is what keeps two
//! configurations from sharing one file, and it puts the file where the
//! operator already looks for this configuration's other files — the lock
//! file beside it is named the same way.
//!
//! # What is in it
//!
//! Two optional numbers, in a deliberately dull format:
//!
//! ```text
//! offset=12.5,0.0
//! size=420.0,512.0
//! ```
//!
//! It is not a format this GUI needs a parser library for, and it is not a
//! format anything else reads: the file is the GUI's own, and a GUI that
//! cannot parse it says so by ignoring it.
//!
//! # The placement is an offset, not a position
//!
//! The `offset` is how far the pulldown sat from the **anchor corner of the
//! display it was on** — the gap at the trailing edge and the gap at the
//! edge the pulldown hangs from. It is not a position, and the difference is
//! the whole point: a position names a place on one display, and a pulldown
//! that remembers one and opens it on a *different* display opens in the
//! wrong place — on screen, the right size, and nowhere near the tray whose
//! click opened it. An offset means the same thing on every display.
//!
//! # What an older file says
//!
//! An older file held `position=1234.5,678.9`: an absolute place on whichever
//! display was in use when it was written. That key is **not read**. The
//! display it was measured on may be a display that is not there any more,
//! and a position cannot be carried to a display it was never measured
//! against — reading it would reproduce the very defect the offset exists to
//! fix. A file that says only `position=` is therefore read as no placement
//! at all, and the pulldown opens against the corner, which is where a
//! pulldown belongs.
//!
//! # What is *not* in it
//!
//! Nothing about the fleet, the configuration or the daemon. The file holds
//! where the pulldown was left and how big it was, and it holds nothing
//! else, so a stale file cannot contradict the truth about the fleet.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::placement::Offset;

/// The most the state file may be read as.
///
/// The file holds two numbers and a key each. A megabyte of it is not a
/// geometry this GUI wrote, and reading it would be a way to make the GUI do
/// work it has no reason to do.
const MAX_STATE_BYTES: u64 = 4096;

/// The largest coordinate the state file may claim.
///
/// A coordinate beyond this is not a place a display is, so it is not an
/// offset the operator chose either.
const MAX_COORDINATE: f32 = 1_000_000.0;

/// The smallest size the state file may claim, in logical pixels.
///
/// A size of nothing is not a size the operator chose; the smallest the
/// pulldown may be is larger than this.
const MIN_SIZE: f32 = 1.0;

/// The largest size the state file may claim, in logical pixels.
///
/// A pulldown is a dropdown on a display. It is not ten kilometres wide.
const MAX_SIZE: f32 = 100_000.0;

/// The pulldown's geometry, as the operator left it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PanelGeometry {
    /// The pulldown's placement, relative to the display's anchor corner.
    pub offset: Option<Offset>,
    /// The pulldown's size, in logical pixels.
    pub size: Option<(f32, f32)>,
}

impl PanelGeometry {
    /// Whether there is anything worth writing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.offset.is_none() && self.size.is_none()
    }
}

/// The file the geometry is kept in.
///
/// The name is derived from the configuration's path, so the file is always
/// beside the configuration it belongs to and two configurations never share
/// one file.
#[must_use]
pub fn path_for(config: &Path) -> PathBuf {
    let mut name = config.as_os_str().to_owned();
    name.push(".gui-state");
    PathBuf::from(name)
}

/// Read the geometry the operator left.
///
/// A file that is missing, unreadable, too large or malformed is not an
/// error: it means the operator has not left a placement yet, and the
/// pulldown opens where the platform's convention says it should.
#[must_use]
pub fn load(path: &Path) -> PanelGeometry {
    let Ok(mut file) = fs::File::open(path) else {
        return PanelGeometry::default();
    };
    // The read is bounded: the file is read up to the limit and no further,
    // so a file that grew between the open and the read cannot make the GUI
    // read it all.
    let mut bytes = Vec::new();
    let read = Read::by_ref(&mut file)
        .take(MAX_STATE_BYTES + 1)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() as u64 > MAX_STATE_BYTES {
        return PanelGeometry::default();
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return PanelGeometry::default();
    };
    parse(&text)
}

/// Write the geometry the operator left.
///
/// The write is atomic: the bytes go to a temporary file beside the target
/// and the target is replaced by a rename, so a reader never sees half a
/// file and a crash never leaves one.
pub fn store(path: &Path, geometry: PanelGeometry) -> io::Result<()> {
    let mut text = String::new();
    if let Some(offset) = geometry.offset {
        text.push_str(&format!("offset={},{}\n", offset.x, offset.y));
    }
    if let Some((width, height)) = geometry.size {
        text.push_str(&format!("size={width},{height}\n"));
    }
    write_atomic(path, text.as_bytes())
}

/// Read the geometry out of the file's text.
fn parse(text: &str) -> PanelGeometry {
    let mut geometry = PanelGeometry::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "offset" => {
                geometry.offset = parse_pair(value, MAX_COORDINATE).map(|(x, y)| Offset { x, y });
            }
            "size" => geometry.size = parse_pair(value, MAX_SIZE),
            // A key this version does not know is ignored, so a file written
            // by a later version is still read for what it does mean.
            //
            // `position` is one of those keys: it is an absolute place on
            // the display it was measured on, and it is not read, because a
            // place on one display is not a place on another. See the
            // module's notes.
            _ => {}
        }
    }
    geometry
}

/// Read one `x,y` pair, if it is two finite numbers within the bound.
fn parse_pair(value: &str, bound: f32) -> Option<(f32, f32)> {
    let (x, y) = value.split_once(',')?;
    let x = x.trim().parse::<f32>().ok()?;
    let y = y.trim().parse::<f32>().ok()?;
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    if x.abs() > bound || y.abs() > bound {
        return None;
    }
    if bound == MAX_SIZE && (x < MIN_SIZE || y < MIN_SIZE) {
        return None;
    }
    Some((x, y))
}

/// Write the bytes to the path, atomically.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    fs::create_dir_all(dir)?;

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    let temp = dir.join(format!(".gregg-gui-{}-{stamp}.tmp", std::process::id()));

    let result = write_temp(&temp, bytes).and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        // A temporary file that outlived its rename is litter, and it is
        // this process's litter.
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Write the bytes to the temporary file, and flush them to the disk.
fn write_temp(temp: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(temp)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary directory that goes away with the test.
    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("gregg-gui-state-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temporary directory");
        dir
    }

    #[test]
    fn the_file_is_named_after_the_configuration() {
        let path = path_for(Path::new("/home/u/.config/gregg/config.toml"));
        assert_eq!(
            path,
            PathBuf::from("/home/u/.config/gregg/config.toml.gui-state")
        );
        assert_eq!(
            path.parent(),
            Some(Path::new("/home/u/.config/gregg")),
            "the file is beside the configuration"
        );
    }

    #[test]
    fn two_configurations_never_share_a_file() {
        let first = path_for(Path::new("/home/u/.config/gregg/config.toml"));
        let second = path_for(Path::new("/home/u/.config/gregg/other.toml"));
        assert_ne!(first, second);
    }

    #[test]
    fn a_missing_file_means_no_placement() {
        let dir = temp_dir("missing");
        let geometry = load(&dir.join("nothing.gui-state"));
        assert_eq!(geometry, PanelGeometry::default());
        assert!(geometry.is_empty());
    }

    #[test]
    fn a_placement_survives_a_round_trip() {
        let dir = temp_dir("round-trip");
        let path = dir.join("config.toml.gui-state");
        let geometry = PanelGeometry {
            offset: Some(Offset {
                x: 1234.5,
                y: 678.25,
            }),
            size: Some((420.0, 512.0)),
        };
        store(&path, geometry).expect("the geometry is written");
        assert_eq!(load(&path), geometry);
    }

    #[test]
    fn an_unchanged_placement_is_not_rewritten_into_nothing() {
        let dir = temp_dir("partial");
        let path = dir.join("config.toml.gui-state");
        store(
            &path,
            PanelGeometry {
                offset: Some(Offset { x: 10.0, y: 20.0 }),
                size: None,
            },
        )
        .expect("the geometry is written");
        let geometry = load(&path);
        assert_eq!(
            geometry.offset,
            Some(Offset { x: 10.0, y: 20.0 }),
            "the placement is still there"
        );
        assert_eq!(geometry.size, None);
    }

    #[test]
    fn a_malformed_file_is_ignored_rather_than_trusted() {
        let dir = temp_dir("malformed");
        let path = dir.join("config.toml.gui-state");
        fs::write(&path, "offset=not-a-number,12\nsize=1,2\n").expect("the file is written");
        let geometry = load(&path);
        assert_eq!(geometry.offset, None, "a bad offset is not an offset");
        assert_eq!(
            geometry.size,
            Some((1.0, 2.0)),
            "the rest of the file still reads"
        );
    }

    #[test]
    fn an_offset_outside_any_display_is_ignored() {
        let dir = temp_dir("out-of-range");
        let path = dir.join("config.toml.gui-state");
        fs::write(&path, "offset=99999999,0\n").expect("the file is written");
        assert_eq!(load(&path).offset, None);
    }

    #[test]
    fn an_older_absolute_position_is_not_read() {
        // An older file held a place on the display it was written on. That
        // place means nothing on another display, and reading it is how the
        // pulldown came to open in the middle of the desk.
        let dir = temp_dir("old-position");
        let path = dir.join("config.toml.gui-state");
        fs::write(&path, "position=844,65\nsize=606,478\n").expect("the file is written");
        let geometry = load(&path);
        assert_eq!(
            geometry.offset, None,
            "an absolute position is not a placement that can be carried"
        );
        assert_eq!(
            geometry.size,
            Some((606.0, 478.0)),
            "the size is still a size, and it still reads"
        );
    }

    #[test]
    fn a_size_of_nothing_is_ignored() {
        let dir = temp_dir("no-size");
        let path = dir.join("config.toml.gui-state");
        fs::write(&path, "size=0,0\n").expect("the file is written");
        assert_eq!(load(&path).size, None);
    }

    #[test]
    fn a_file_larger_than_the_limit_is_not_read() {
        let dir = temp_dir("too-large");
        let path = dir.join("config.toml.gui-state");
        let mut text = String::new();
        while text.len() < MAX_STATE_BYTES as usize + 16 {
            text.push_str("offset=1,2\n");
        }
        fs::write(&path, text).expect("the file is written");
        assert_eq!(
            load(&path),
            PanelGeometry::default(),
            "a file this large is not one this GUI wrote"
        );
    }

    #[test]
    fn an_unknown_key_is_ignored() {
        let dir = temp_dir("unknown-key");
        let path = dir.join("config.toml.gui-state");
        fs::write(&path, "colour=green\noffset=5,6\n").expect("the file is written");
        assert_eq!(load(&path).offset, Some(Offset { x: 5.0, y: 6.0 }));
    }

    #[test]
    fn a_comment_is_ignored() {
        let dir = temp_dir("comment");
        let path = dir.join("config.toml.gui-state");
        fs::write(&path, "# gregg-gui\noffset=5,6\n").expect("the file is written");
        assert_eq!(load(&path).offset, Some(Offset { x: 5.0, y: 6.0 }));
    }

    #[test]
    fn the_write_leaves_no_temporary_file_behind() {
        let dir = temp_dir("no-litter");
        let path = dir.join("config.toml.gui-state");
        store(
            &path,
            PanelGeometry {
                offset: Some(Offset { x: 1.0, y: 2.0 }),
                size: Some((3.0, 4.0)),
            },
        )
        .expect("the geometry is written");
        let litter: Vec<_> = fs::read_dir(&dir)
            .expect("the directory is readable")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(litter.is_empty(), "the temporary file was renamed away");
    }

    #[test]
    fn the_write_replaces_what_was_there() {
        let dir = temp_dir("replace");
        let path = dir.join("config.toml.gui-state");
        store(&path, PanelGeometry::default()).expect("the first write");
        fs::write(&path, "offset=7,8\n").expect("the file is written");
        store(
            &path,
            PanelGeometry {
                offset: Some(Offset { x: 1.0, y: 2.0 }),
                size: None,
            },
        )
        .expect("the second write");
        assert_eq!(load(&path).offset, Some(Offset { x: 1.0, y: 2.0 }));
    }
}
