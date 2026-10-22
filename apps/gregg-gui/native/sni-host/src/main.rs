//! A CI-only StatusNotifierItem **host** (a.k.a. "tray host") used to prove
//! that the Gregg GUI's Linux system tray really works.
//!
//! Slint 1.18 implements the Linux system tray with the `ksni` crate, and
//! `ksni` is a StatusNotifierItem *client*: it needs a StatusNotifierItem
//! *host* to talk to. On a normal desktop that host is the panel (KDE's
//! `plasmashell`, the GNOME Shell extension, and so on). A headless CI runner
//! has no such host, so without one the tray would silently not exist and a
//! green build would prove nothing at all.
//!
//! This helper *is* that host. It owns the well-known name
//! `org.kde.StatusNotifierWatcher`, serves the `org.kde.StatusNotifierWatcher`
//! interface at `/StatusNotifierWatcher`, and then it starts the GUI and
//! inspects the item the GUI registers.
//!
//! It runs in two modes:
//!
//! * `--mode with-host` — the watcher name is owned, so the GUI *must* find a
//!   tray host. The helper asserts that the GUI reports `tray=available`,
//!   registers an item, that the item's `ToolTip` carries the live fleet
//!   state rather than a placeholder, that the item's menu is the *context*
//!   menu — the tray's click opens the pulldown, and the pulldown carries the
//!   fleet — and that the GUI placed the pulldown inside the work area it
//!   reported.
//! * `--mode no-host` — the watcher name is deliberately *not* owned, so the
//!   GUI must find no tray host. The helper asserts that the GUI reports
//!   `tray=unavailable` and keeps running instead of dying.
//!
//! Every check prints a `sni-host: PASS <check> <detail>` line; a failed check
//! prints `sni-host: FAIL <check> <detail>` and makes the process exit
//! non-zero. Nothing here is ever skipped: if the session bus or the display
//! is missing, the helper fails.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, Value};

/// The well-known name a StatusNotifierItem host must own.
const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
/// The object path the watcher's interface is served at.
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
/// The object path a StatusNotifierItem is served at.
const ITEM_PATH: &str = "/StatusNotifierItem";
/// The StatusNotifierItem interface.
const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
/// The dbusmenu interface a StatusNotifierItem's menu is served under.
const MENU_INTERFACE: &str = "com.canonical.dbusmenu";
/// The bus daemon's own service name.
const DBUS_SERVICE: &str = "org.freedesktop.DBus";
/// The bus daemon's own object path.
const DBUS_PATH: &str = "/org/freedesktop/DBus";
/// The bus daemon's own interface.
const DBUS_INTERFACE: &str = "org.freedesktop.DBus";

/// The line the GUI prints when it managed to create a system tray.
const TRAY_AVAILABLE: &str = "gregg-gui: tray=available";

/// The native tray menu's one entry.
///
/// The tray's **click** opens the pulldown, and the pulldown carries the
/// fleet, the metrics and the forms. The native menu is the *context* menu,
/// and it carries the application's own way out and nothing else.
const QUIT_ENTRY: &str = "Quit Gregg";
/// The line the GUI prints when it could not create a system tray.
const TRAY_UNAVAILABLE: &str = "gregg-gui: tray=unavailable";

/// How many lines of GUI output are kept for failure messages.
const TRANSCRIPT_LIMIT: usize = 400;

/// The bus names of the items registered with the watcher.
type ItemList = Arc<Mutex<Vec<String>>>;

/// The GUI's captured output: every line it printed, tagged with the stream.
type Transcript = Arc<Mutex<Vec<String>>>;

/// The `org.kde.StatusNotifierWatcher` interface.
///
/// This is deliberately minimal, but it is not a stub: `ksni` refuses to
/// register an item unless `IsStatusNotifierHostRegistered` is `true`, so a
/// host that lies about that property would make the whole exercise vacuous.
#[derive(Clone, Default)]
struct Watcher {
    /// The bus names of the registered items, in registration order.
    items: ItemList,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    /// `RegisterStatusNotifierItem(s service)`.
    ///
    /// The `signal_context` argument is injected by the object server: it is
    /// how the `StatusNotifierItemRegistered` signal is emitted.
    async fn register_status_notifier_item(
        &self,
        #[zbus(signal_context)] emitter: SignalEmitter<'_>,
        service: &str,
    ) -> zbus::fdo::Result<()> {
        {
            let mut items = self
                .items
                .lock()
                .expect("the watcher item list was poisoned");
            if !items.iter().any(|item| item == service) {
                items.push(service.to_string());
            }
        }
        WatcherSignals::status_notifier_item_registered(&emitter, service)
            .await
            .map_err(|err| zbus::fdo::Error::Failed(err.to_string()))
    }

    /// `RegisterStatusNotifierHost(s service)`. Nothing to do: this helper
    /// only ever hosts items, it never renders them.
    async fn register_status_notifier_host(&self, _service: &str) -> zbus::fdo::Result<()> {
        Ok(())
    }

    /// `RegisteredStatusNotifierItems` — the bus names of the live items.
    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items
            .lock()
            .expect("the watcher item list was poisoned")
            .clone()
    }

    /// `IsStatusNotifierHostRegistered` — must be `true`, or `ksni` refuses to
    /// register its item and the tray never appears.
    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    /// `ProtocolVersion` — 0 is the version every host in the wild speaks.
    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    /// `StatusNotifierItemRegistered(s)`.
    #[zbus(signal)]
    async fn status_notifier_item_registered(
        _emitter: &SignalEmitter<'_>,
        _service: &str,
    ) -> zbus::Result<()>;
}

/// Which half of the exercise to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Own the watcher name: the GUI must find a tray host.
    WithHost,
    /// Own nothing: the GUI must find no tray host and must survive it.
    NoHost,
}

/// The command line.
struct Args {
    /// Which half to run.
    mode: Mode,
    /// The `gregg-gui` executable under test.
    gui: PathBuf,
    /// The `gregg` client executable the GUI is told to use.
    gregg: PathBuf,
    /// The configuration file the GUI is told to read.
    config: PathBuf,
    /// Text that must appear in the tray's tooltip.
    expect_status: String,
    /// Text that must appear in one of the tray menu's labels.
    expect_host_name: String,
    /// How long any single wait may take.
    timeout: Duration,
}

/// Reads the next flag value.
fn next_value(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    iter.next().ok_or_else(|| format!("{flag} needs a value"))
}

/// Parses the command line.
fn parse_args() -> Result<Args, String> {
    let mut mode: Option<Mode> = None;
    let mut gui: Option<PathBuf> = None;
    let mut gregg: Option<PathBuf> = None;
    let mut config: Option<PathBuf> = None;
    let mut expect_status: Option<String> = None;
    let mut expect_host_name: Option<String> = None;
    let mut timeout: Option<Duration> = None;

    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--mode" => {
                let value = next_value(&mut iter, "--mode")?;
                mode = Some(match value.as_str() {
                    "with-host" => Mode::WithHost,
                    "no-host" => Mode::NoHost,
                    other => {
                        return Err(format!(
                            "--mode must be `with-host` or `no-host`, not `{other}`"
                        ))
                    }
                });
            }
            "--gui" => gui = Some(PathBuf::from(next_value(&mut iter, "--gui")?)),
            "--gregg" => gregg = Some(PathBuf::from(next_value(&mut iter, "--gregg")?)),
            "--config" => config = Some(PathBuf::from(next_value(&mut iter, "--config")?)),
            "--expect-status" => {
                expect_status = Some(next_value(&mut iter, "--expect-status")?);
            }
            "--expect-host-name" => {
                expect_host_name = Some(next_value(&mut iter, "--expect-host-name")?);
            }
            "--timeout-secs" => {
                let value = next_value(&mut iter, "--timeout-secs")?;
                let secs: u64 = value
                    .parse()
                    .map_err(|_| format!("--timeout-secs must be a number, not `{value}`"))?;
                timeout = Some(Duration::from_secs(secs));
            }
            other => return Err(format!("unrecognized argument: {other}")),
        }
    }

    Ok(Args {
        mode: mode.ok_or("--mode is required")?,
        gui: gui.ok_or("--gui is required")?,
        gregg: gregg.ok_or("--gregg is required")?,
        config: config.ok_or("--config is required")?,
        expect_status: expect_status.ok_or("--expect-status is required")?,
        expect_host_name: expect_host_name.ok_or("--expect-host-name is required")?,
        timeout: timeout.unwrap_or(Duration::from_secs(90)),
    })
}

/// Prints a passing check.
fn pass(check: &str, detail: impl std::fmt::Display) {
    println!("sni-host: PASS {check} {detail}");
}

/// Prints the GUI's captured output, so a failure says what the GUI said.
fn transcript_text(transcript: &Transcript) -> String {
    let lines = transcript
        .lock()
        .map(|lines| lines.join("\n"))
        .unwrap_or_else(|_| "<the transcript could not be read>".to_string());
    if lines.trim().is_empty() {
        "<the GUI printed nothing>".to_string()
    } else {
        lines
    }
}

/// Where the pulldown was put, and where it was allowed to be put.
///
/// The GUI reports both, so that a native exercise can check the *position*
/// and not only that a window appeared.
#[derive(Debug, Clone, Copy)]
struct Placement {
    /// The pulldown's left edge.
    x: f64,
    /// The pulldown's top edge.
    y: f64,
    /// The pulldown's width.
    width: f64,
    /// The pulldown's height.
    height: f64,
    /// The work area's left edge.
    area_left: f64,
    /// The work area's top edge.
    area_top: f64,
    /// The work area's width.
    area_width: f64,
    /// The work area's height.
    area_height: f64,
}

impl Placement {
    /// Whether the pulldown is inside the work area it reported.
    fn is_inside(&self) -> bool {
        self.x >= self.area_left
            && self.y >= self.area_top
            && self.x + self.width <= self.area_left + self.area_width
            && self.y + self.height <= self.area_top + self.area_height
    }
}

/// Reads the pulldown's placement out of one of the GUI's output lines.
///
/// The line is `gregg-gui: pulldown=placed x=.. y=.. width=.. height=.. area=l,t,wxh`.
fn parse_placement(line: &str) -> Option<Placement> {
    if !line.contains("pulldown=placed") {
        return None;
    }
    let mut x = None;
    let mut y = None;
    let mut width = None;
    let mut height = None;
    let mut area = None;
    for field in line.split_whitespace() {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        match key {
            "x" => x = value.parse().ok(),
            "y" => y = value.parse().ok(),
            "width" => width = value.parse().ok(),
            "height" => height = value.parse().ok(),
            "area" => {
                let (left, rest) = value.split_once(',')?;
                let (top, rest) = rest.split_once(',')?;
                let (area_width, area_height) = rest.split_once('x')?;
                area = Some((
                    left.parse().ok()?,
                    top.parse().ok()?,
                    area_width.parse().ok()?,
                    area_height.parse().ok()?,
                ));
            }
            _ => {}
        }
    }
    let (area_left, area_top, area_width, area_height) = area?;
    Some(Placement {
        x: x?,
        y: y?,
        width: width?,
        height: height?,
        area_left,
        area_top,
        area_width,
        area_height,
    })
}

/// The pulldown's placement, from the GUI's output.
fn find_placement(transcript: &Transcript) -> Option<Placement> {
    let lines = transcript.lock().ok()?;
    lines.iter().find_map(|line| parse_placement(line))
}

/// Reads one of the GUI's output streams line by line.
fn pipe_lines<R: Read + Send + 'static>(
    stream: R,
    name: &'static str,
    sender: mpsc::Sender<String>,
    transcript: Transcript,
) {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            {
                let mut lines = transcript.lock().expect("the GUI transcript was poisoned");
                if lines.len() < TRANSCRIPT_LIMIT {
                    lines.push(format!("{name}: {line}"));
                }
            }
            if sender.send(line).is_err() {
                break;
            }
        }
    });
}

/// Spawns the GUI with both output streams captured.
fn spawn_gui(
    gui: &Path,
    gregg: &Path,
    config: &Path,
) -> Result<(Child, Receiver<String>, Transcript), String> {
    let mut child = Command::new(gui)
        .arg("--config")
        .arg(config)
        .arg("--gregg")
        .arg(gregg)
        // The pulldown is the GUI's only window, and it opens from the tray.
        // This exercise drives the tray, so the panel is opened at startup
        // instead: that way the panel really is rendered on a real X11
        // session while the tray is checked.
        .arg("--panel")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("cannot start the GUI at {}: {err}", gui.display()))?;

    let (sender, receiver) = mpsc::channel::<String>();
    let transcript = Arc::new(Mutex::new(Vec::new()));

    if let Some(stdout) = child.stdout.take() {
        pipe_lines(stdout, "stdout", sender.clone(), Arc::clone(&transcript));
    }
    if let Some(stderr) = child.stderr.take() {
        pipe_lines(stderr, "stderr", sender.clone(), Arc::clone(&transcript));
    }
    drop(sender);

    Ok((child, receiver, transcript))
}

/// Waits for the GUI's `tray=` startup line.
///
/// `want_available` says which value the line must carry. A GUI that prints
/// the *other* value is a failure, not something to shrug at: the whole point
/// of the two passes is that the value tracks the presence of a tray host.
fn wait_for_tray_line(
    receiver: &Receiver<String>,
    child: &mut Child,
    transcript: &Transcript,
    deadline: Instant,
    want_available: bool,
) -> Result<String, String> {
    let wanted = if want_available {
        TRAY_AVAILABLE
    } else {
        TRAY_UNAVAILABLE
    };
    let unwanted = if want_available {
        TRAY_UNAVAILABLE
    } else {
        TRAY_AVAILABLE
    };

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "timed out waiting for `{wanted}` from the GUI; its output was:\n{}",
                transcript_text(transcript)
            ));
        }
        match receiver.recv_timeout(remaining) {
            Ok(line) => {
                if line.contains(wanted) {
                    return Ok(line);
                }
                if line.contains(unwanted) {
                    return Err(format!(
                        "the GUI printed `{unwanted}` where `{wanted}` was expected; \
                         its output was:\n{}",
                        transcript_text(transcript)
                    ));
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                return Err(format!(
                    "timed out waiting for `{wanted}` from the GUI; its output was:\n{}",
                    transcript_text(transcript)
                ));
            }
            Err(RecvTimeoutError::Disconnected) => {
                let status = child
                    .try_wait()
                    .ok()
                    .flatten()
                    .map_or_else(|| "unknown".to_string(), |status| status.to_string());
                return Err(format!(
                    "the GUI exited ({status}) before printing `{wanted}`; its output was:\n{}",
                    transcript_text(transcript)
                ));
            }
        }
    }
}

/// The bus name that currently owns `WATCHER_NAME`, if any.
async fn watcher_owner() -> Result<Option<String>, String> {
    let connection = zbus::Connection::session()
        .await
        .map_err(|err| format!("there is no D-Bus session bus to talk to: {err}"))?;
    let bus = zbus::Proxy::new(&connection, DBUS_SERVICE, DBUS_PATH, DBUS_INTERFACE)
        .await
        .map_err(|err| format!("cannot reach the bus daemon: {err}"))?;
    match bus.call_method("GetNameOwner", &WATCHER_NAME).await {
        Ok(reply) => {
            let owner: String = reply
                .body()
                .deserialize()
                .map_err(|err| format!("cannot read the owner of {WATCHER_NAME}: {err}"))?;
            Ok(Some(owner))
        }
        // No owner: the name is free, which is what the caller asked about.
        Err(_) => Ok(None),
    }
}

/// Waits until the GUI has registered an item, and returns its bus name.
async fn wait_for_registered_item(
    items: &ItemList,
    transcript: &Transcript,
    deadline: Instant,
) -> Result<String, String> {
    loop {
        let registered = items
            .lock()
            .expect("the watcher item list was poisoned")
            .clone();
        if let Some(first) = registered.first() {
            return Ok(first.clone());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for the GUI to register a StatusNotifierItem; \
                 its output was:\n{}",
                transcript_text(transcript)
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Reads a string out of a D-Bus value.
fn value_as_str(value: &Value<'_>) -> Option<String> {
    match value {
        Value::Str(text) => Some(text.as_str().to_string()),
        _ => None,
    }
}

/// Reads a string or an object path out of an owned D-Bus value.
fn owned_as_text(value: &OwnedValue) -> Option<String> {
    match Value::from(value.clone()) {
        Value::Str(text) => Some(text.as_str().to_string()),
        Value::ObjectPath(path) => Some(path.as_str().to_string()),
        _ => None,
    }
}

/// The `ToolTip` property, as `(icon_name, icon_pixmap, title, description)`.
struct ToolTip {
    /// The tooltip's title: the GUI's fleet status.
    title: String,
    /// The tooltip's description.
    description: String,
}

/// Reads the `ToolTip` property.
fn read_tooltip(value: &OwnedValue) -> Result<ToolTip, String> {
    let Value::Structure(fields) = Value::from(value.clone()) else {
        return Err("the ToolTip property is not a structure".to_string());
    };
    let fields = fields.fields();
    let text = |index: usize| fields.get(index).and_then(value_as_str).unwrap_or_default();
    Ok(ToolTip {
        title: text(2),
        description: text(3),
    })
}

/// Reads every label of the item's dbusmenu.
async fn read_menu_labels(
    connection: &zbus::Connection,
    item_bus_name: &str,
    menu_path: &str,
) -> Result<Vec<String>, String> {
    let menu = zbus::Proxy::new(connection, item_bus_name, menu_path, MENU_INTERFACE)
        .await
        .map_err(|err| format!("cannot reach the tray menu at {menu_path}: {err}"))?;
    // An empty id list means "every item" in the dbusmenu specification.
    let reply = menu
        .call_method(
            "GetGroupProperties",
            &(Vec::<i32>::new(), vec!["label".to_string()]),
        )
        .await
        .map_err(|err| format!("cannot read the tray menu: {err}"))?;
    let items: Vec<(i32, HashMap<String, OwnedValue>)> = reply
        .body()
        .deserialize()
        .map_err(|err| format!("cannot decode the tray menu: {err}"))?;

    let mut labels = Vec::new();
    for (_id, properties) in items {
        if let Some(label) = properties.get("label") {
            if let Some(text) = owned_as_text(label) {
                labels.push(text);
            }
        }
    }
    Ok(labels)
}

/// The observable state of one registered tray item.
struct ItemState {
    /// The `Title` property.
    title: String,
    /// The `ToolTip` property's title: the GUI's fleet status.
    tooltip_title: String,
    /// The `ToolTip` property's description.
    tooltip_description: String,
    /// The `IconName` property.
    icon_name: String,
    /// How many pixmaps the `IconPixmap` property carries.
    icon_entries: usize,
    /// The `Menu` property: the dbusmenu's object path.
    menu_path: String,
    /// Every label in the tray menu.
    labels: Vec<String>,
}

/// Reads every property of the registered tray item.
async fn read_item_state(
    connection: &zbus::Connection,
    item_bus_name: &str,
) -> Result<ItemState, String> {
    let item = zbus::Proxy::new(connection, item_bus_name, ITEM_PATH, ITEM_INTERFACE)
        .await
        .map_err(|err| format!("cannot reach the tray item at {ITEM_PATH}: {err}"))?;

    let title: String = item
        .get_property("Title")
        .await
        .map_err(|err| format!("cannot read the Title property: {err}"))?;
    let icon_name: String = item
        .get_property("IconName")
        .await
        .map_err(|err| format!("cannot read the IconName property: {err}"))?;
    let tooltip = read_tooltip(
        &item
            .get_property::<OwnedValue>("ToolTip")
            .await
            .map_err(|err| format!("cannot read the ToolTip property: {err}"))?,
    )?;
    let icon_pixmap = item
        .get_property::<OwnedValue>("IconPixmap")
        .await
        .map_err(|err| format!("cannot read the IconPixmap property: {err}"))?;
    let menu_path = owned_as_text(
        &item
            .get_property::<OwnedValue>("Menu")
            .await
            .map_err(|err| format!("cannot read the Menu property: {err}"))?,
    )
    .unwrap_or_default();

    let icon_entries = match Value::from(icon_pixmap) {
        Value::Array(array) => array.len(),
        _ => 0,
    };

    let labels = read_menu_labels(connection, item_bus_name, &menu_path).await?;

    Ok(ItemState {
        title,
        tooltip_title: tooltip.title,
        tooltip_description: tooltip.description,
        icon_name,
        icon_entries,
        menu_path,
        labels,
    })
}

/// Runs the `with-host` pass: the GUI must find a tray host.
async fn run_with_host(args: &Args) -> Result<(), String> {
    if let Some(owner) = watcher_owner().await? {
        return Err(format!(
            "{WATCHER_NAME} is already owned by {owner}; this pass needs to own it"
        ));
    }

    let watcher = Watcher::default();
    let items = Arc::clone(&watcher.items);
    let connection = zbus::connection::Builder::session()
        .map_err(|err| format!("there is no D-Bus session bus to talk to: {err}"))?
        .serve_at(WATCHER_PATH, watcher)
        .map_err(|err| format!("cannot serve {WATCHER_NAME}: {err}"))?
        .name(WATCHER_NAME)
        .map_err(|err| format!("cannot request the name {WATCHER_NAME}: {err}"))?
        .build()
        .await
        .map_err(|err| {
            format!("cannot become the {WATCHER_NAME} host (is another tray host running?): {err}")
        })?;
    pass(
        "watcher-host-owned",
        format!("this helper owns {WATCHER_NAME} at {WATCHER_PATH}"),
    );

    let deadline = Instant::now() + args.timeout;
    let (mut child, receiver, transcript) = spawn_gui(&args.gui, &args.gregg, &args.config)?;
    wait_for_tray_line(&receiver, &mut child, &transcript, deadline, true)?;
    pass(
        "gui-reported-tray-available",
        "the GUI printed `gregg-gui: tray=available` with a tray host present",
    );

    let item_bus_name = wait_for_registered_item(&items, &transcript, deadline).await?;
    pass(
        "gui-registered-an-item",
        format!("the GUI registered {item_bus_name}"),
    );

    // The watcher's own property must report the item, so the registration
    // really went through the interface and not through some side channel.
    let watcher_proxy = zbus::Proxy::new(&connection, WATCHER_NAME, WATCHER_PATH, WATCHER_NAME)
        .await
        .map_err(|err| format!("cannot reach the watcher interface: {err}"))?;
    let listed: Vec<String> = watcher_proxy
        .get_property("RegisteredStatusNotifierItems")
        .await
        .map_err(|err| format!("cannot read RegisteredStatusNotifierItems: {err}"))?;
    if !listed.contains(&item_bus_name) {
        return Err(format!(
            "RegisteredStatusNotifierItems is {listed:?}, which does not contain {item_bus_name}"
        ));
    }
    pass(
        "watcher-lists-the-item",
        format!("RegisteredStatusNotifierItems is {listed:?}"),
    );

    // The tray item starts out carrying a placeholder tooltip and an empty
    // menu; the live fleet state only lands once the first fleet document
    // has arrived. So the properties are polled, not read once.
    let state = loop {
        let candidate = read_item_state(&connection, item_bus_name.as_str()).await?;
        let tooltip_carries = candidate.tooltip_title.contains(&args.expect_status)
            && candidate.tooltip_title.contains(&args.expect_host_name);
        let menu_carries = candidate
            .labels
            .iter()
            .any(|label| label.contains(QUIT_ENTRY));
        if tooltip_carries && menu_carries {
            break candidate;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "the tray never carried the live fleet state: the tooltip was {:?} \
                 (expected it to contain {:?} and {:?}) and the menu labels were {:?} \
                 (expected one to mention {:?})",
                candidate.tooltip_title,
                args.expect_status,
                args.expect_host_name,
                candidate.labels,
                QUIT_ENTRY
            ));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };

    println!("sni-host: obs Title={:?}", state.title);
    println!("sni-host: obs ToolTip.title={:?}", state.tooltip_title);
    println!(
        "sni-host: obs ToolTip.description={:?}",
        state.tooltip_description
    );
    println!("sni-host: obs IconName={:?}", state.icon_name);
    println!("sni-host: obs IconPixmap.entries={}", state.icon_entries);
    println!("sni-host: obs Menu={:?}", state.menu_path);
    println!("sni-host: obs Menu.labels={:?}", state.labels);

    if state.title.trim().is_empty() {
        return Err("the tray item has an empty Title".to_string());
    }
    if state.tooltip_title.trim().is_empty() {
        return Err("the tray item has an empty ToolTip title".to_string());
    }
    pass(
        "tray-tooltip-carries-the-fleet-status",
        format!("the tooltip is {:?}", state.tooltip_title),
    );
    pass(
        "tray-tooltip-carries-the-fleet",
        format!("the tooltip mentions {:?}", args.expect_host_name),
    );
    pass(
        "tray-menu-is-the-context-menu",
        format!("a menu label mentions {QUIT_ENTRY:?}"),
    );

    // The pulldown's *placement*, asserted rather than assumed.
    //
    // The GUI reports where it put the pulldown and where it was allowed to
    // put it, so the exercise can check that the pulldown is inside the work
    // area — which is the thing that is easy to get wrong and hard to see.
    let placement = find_placement(&transcript).ok_or_else(|| {
        format!(
            "the GUI never reported where it put the pulldown; its output was:\n{}",
            transcript_text(&transcript)
        )
    })?;
    if !placement.is_inside() {
        return Err(format!(
            "the pulldown is not inside the work area it reported: {placement:?}"
        ));
    }
    pass(
        "pulldown-is-placed-inside-the-work-area",
        format!("{placement:?}"),
    );

    if state.icon_entries == 0 && state.icon_name.trim().is_empty() {
        return Err("the tray item has neither an icon name nor an icon pixmap".to_string());
    }
    pass(
        "tray-item-has-an-icon",
        format!(
            "IconName={:?}, IconPixmap.entries={}",
            state.icon_name, state.icon_entries
        ),
    );

    if state.menu_path.trim().is_empty() || state.menu_path == "/" {
        return Err(format!(
            "the tray item's Menu property is {:?}",
            state.menu_path
        ));
    }
    pass(
        "tray-item-has-a-menu",
        format!("the menu is at {}", state.menu_path),
    );

    if let Some(status) = child
        .try_wait()
        .map_err(|err| format!("cannot poll the GUI process: {err}"))?
    {
        return Err(format!("the GUI exited ({status}) while its tray was live"));
    }
    pass(
        "gui-is-still-running",
        "the GUI is still running with a live tray",
    );

    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

/// Runs the `no-host` pass: the GUI must find no tray host and survive it.
async fn run_no_host(args: &Args) -> Result<(), String> {
    if let Some(owner) = watcher_owner().await? {
        return Err(format!(
            "{WATCHER_NAME} is owned by {owner}; this pass needs no tray host at all"
        ));
    }
    pass(
        "no-tray-host-present",
        format!("nobody owns {WATCHER_NAME}"),
    );

    let deadline = Instant::now() + args.timeout;
    let (mut child, receiver, transcript) = spawn_gui(&args.gui, &args.gregg, &args.config)?;
    let line = wait_for_tray_line(&receiver, &mut child, &transcript, deadline, false)?;
    pass(
        "gui-reported-tray-unavailable",
        format!("the GUI printed `{line}`"),
    );

    // The GUI must degrade to "no tray", not die: the operator still gets a
    // window. Give it a few seconds to prove it stays up.
    for _ in 0..15 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if let Some(status) = child
            .try_wait()
            .map_err(|err| format!("cannot poll the GUI process: {err}"))?
        {
            return Err(format!(
                "the GUI exited ({status}) instead of running without a tray; \
                 its output was:\n{}",
                transcript_text(&transcript)
            ));
        }
    }
    pass(
        "gui-survives-without-a-tray",
        "the GUI is still running three seconds after reporting `tray=unavailable`",
    );

    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

/// The entry point.
#[tokio::main]
async fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            println!("sni-host: FAIL arguments {err}");
            std::process::exit(2);
        }
    };

    let outcome = match args.mode {
        Mode::WithHost => run_with_host(&args).await,
        Mode::NoHost => run_no_host(&args).await,
    };

    match outcome {
        Ok(()) => {
            println!("sni-host: PASS all-checks mode={:?}", args.mode);
            std::process::exit(0);
        }
        Err(err) => {
            println!("sni-host: FAIL {err}");
            std::process::exit(1);
        }
    }
}
