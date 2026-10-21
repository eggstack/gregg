//! Plan 188 Part B: the optional Slint desktop frontend for Gregg.
//!
//! The frontend is a *second* way to look at the same fleet, not a second
//! poller. It attaches to the per-config client daemon over the same local
//! protocol the TUI uses, so both frontends share one poll budget and one
//! set of reducers.
//!
//! # Layout
//!
//! - [`bridge`] owns the client-daemon attachment and publishes what it
//!   learns. It knows nothing about Slint.
//! - `src/ui/main.slint` defines the two top-level components: the monitor
//!   window and the system-tray entry.
//!
//! # Threading
//!
//! Slint owns the main thread. The bridge runs on a background thread with
//! its own Tokio runtime and publishes over a channel; a Slint timer brings
//! those messages onto the event-loop thread, which is the only thread
//! allowed to touch a component.

mod bridge;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc};

slint::include_modules!();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let window = MainWindow::new()?;
    let tray = TrayIcon::new()?;

    // A tray icon is only created once a non-empty image has been assigned,
    // so the image is generated here rather than left unset.
    tray.set_tray_icon(tray_icon());

    // The bridge is shared between the callbacks (which request a reload)
    // and the timer (which drains what the bridge published). Every user of
    // it lives on the event-loop thread, so `Rc<RefCell<_>>` is the right
    // shape — it is never shared across threads.
    let bridge = Rc::new(RefCell::new(bridge::Bridge::spawn(
        gregg::config::Config::default_path(),
    )));

    let bridge_for_reload = Rc::clone(&bridge);
    window.on_refresh_requested(move || {
        bridge_for_reload.borrow_mut().request_reload();
    });
    let bridge_for_reload = Rc::clone(&bridge);
    tray.on_refresh_requested(move || {
        bridge_for_reload.borrow_mut().request_reload();
    });

    window.on_quit_requested(|| {
        let _ = slint::quit_event_loop();
    });
    tray.on_quit_requested(|| {
        let _ = slint::quit_event_loop();
    });

    let window_for_tray = window.as_weak();
    tray.on_show_window(move || {
        if let Some(window) = window_for_tray.upgrade() {
            let _ = window.show();
        }
    });
    // The bridge publishes on its own thread; this timer is what brings its
    // messages onto the event-loop thread.
    let bridge_for_timer = Rc::clone(&bridge);
    let window_for_timer = window.as_weak();
    let tray_for_timer = tray.as_weak();
    // The generation of the last document applied to the window.
    //
    // Documents are complete, so a newer one may skip an older one, but an
    // older one must never replace a newer one. This is the same rule the
    // TUI's reducer applies.
    let mut last_generation = 0u64;

    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(250),
        move || {
            let messages = bridge_for_timer.borrow_mut().drain();
            if messages.is_empty() {
                return;
            }
            let (Some(window), Some(tray)) =
                (window_for_timer.upgrade(), tray_for_timer.upgrade())
            else {
                return;
            };
            for message in messages {
                apply(&window, &tray, message, &mut last_generation);
            }
        },
    );

    window.show()?;
    tray.show()?;
    slint::run_event_loop()?;
    Ok(())
}

/// Apply one bridge message to the UI.
fn apply(
    window: &MainWindow,
    tray: &TrayIcon,
    message: bridge::BridgeMessage,
    last_generation: &mut u64,
) {
    match message {
        bridge::BridgeMessage::Attached {
            protocol_version,
            daemon_id,
        } => {
            let status = format!(
                "connected to the client daemon (local protocol {protocol_version}, {daemon_id})"
            );
            window.set_status_text(status.clone().into());
            tray.set_status_text(status.into());
        }
        bridge::BridgeMessage::Fleet {
            generation,
            systems,
        } => {
            if !(*last_generation == 0 || generation > *last_generation) {
                return;
            }
            *last_generation = generation;
            let rows: Vec<SystemRow> = systems
                .iter()
                .map(|system| SystemRow {
                    name: system.name.clone().into(),
                    endpoint: system.endpoint.clone().into(),
                    status: system.status.clone().into(),
                    cpu: system.cpu.clone().into(),
                    memory: system.memory.clone().into(),
                    online: system.online,
                })
                .collect();
            window.set_systems(ModelRc::from(rows.as_slice()));
        }
        bridge::BridgeMessage::Failed { reason } => {
            window.set_status_text(reason.clone().into());
            tray.set_status_text(reason.into());
        }
        bridge::BridgeMessage::ShuttingDown => {
            window.set_status_text("the client daemon stopped; run `gregg daemon run` to start it again".into());
            tray.set_status_text("the client daemon stopped".into());
        }
    }
}

/// Build the tray icon.
///
/// The icon is generated rather than shipped as an asset: a tray entry needs
/// a non-empty image, and a procedurally drawn one keeps the repository free
/// of a binary blob that nothing else uses.
fn tray_icon() -> slint::Image {
    const SIZE: u32 = 22;
    const COLOR: slint::Rgba8Pixel = slint::Rgba8Pixel {
        r: 0x4f,
        g: 0xc1,
        b: 0x5a,
        a: 0xff,
    };

    let mut pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(SIZE, SIZE);
    {
        let slice = pixels.make_mut_slice();
        let center = (SIZE as f32 - 1.0) / 2.0;
        let radius = center;
        for y in 0..SIZE {
            for x in 0..SIZE {
                let dx = x as f32 - center;
                let dy = y as f32 - center;
                let inside = (dx * dx + dy * dy).sqrt() <= radius;
                slice[(y * SIZE + x) as usize] = if inside {
                    COLOR
                } else {
                    slint::Rgba8Pixel {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 0,
                    }
                };
            }
        }
    }
    slint::Image::from_rgba8(pixels)
}
