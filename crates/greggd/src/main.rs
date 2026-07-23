//! `greggd` binary entry point.
//!
//! Selects the native collector at compile time and delegates to the
//! foreground [`greggd::run`] entry point.

#[cfg(target_os = "linux")]
type NativeCollector = greggd::collector::linux::LinuxCollector;

#[cfg(target_os = "macos")]
type NativeCollector = greggd::collector::macos::MacOsCollector;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = greggd::run::RunConfig::default();
    let collector = NativeCollector::new(None)?;
    greggd::run::run(collector, config).await
}
