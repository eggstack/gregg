//! Slow drive/filesystem refresh isolation.
//!
//! Moved without behavioral change from `greggd::collector::DriveRefreshCache`
//! (Plan 134): one worker per cache, immediate first request, 30-second
//! steady refresh, bounded channels, last-good retention, panic
//! containment/backoff, nonblocking poll, and drop that never joins a worker
//! blocked in an uninterruptible native filesystem call.

use crate::error::{CollectError, CollectErrorKind};
use crate::model::DriveMetrics;

const DRIVE_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
const DRIVE_REFRESH_RETRY_START: std::time::Duration = std::time::Duration::from_millis(10);

/// Drive enumeration cache: best-effort, never fails core readiness.
///
/// `poll()` keeps serving the last `latest` while the refresh worker is
/// retrying or blocked. A worker blocked indefinitely inside a native
/// filesystem call blocks drive refresh only; sampling of CPU, memory, and
/// other families continues. There is deliberately no watchdog that aborts
/// the blocked call and no failure propagated to readiness.
#[derive(Debug)]
pub struct DriveRefreshCache {
    request_tx: Option<std::sync::mpsc::SyncSender<()>>,
    result_rx: std::sync::mpsc::Receiver<Result<Vec<DriveMetrics>, CollectError>>,
    latest: Option<Vec<DriveMetrics>>,
}

impl DriveRefreshCache {
    /// Create a cache that refreshes by calling `collect(&source)` on a
    /// dedicated worker thread.
    pub fn new<S, F>(source: S, collect: F) -> Self
    where
        S: Send + 'static,
        F: Fn(&S) -> Result<Vec<DriveMetrics>, CollectError> + Send + 'static,
    {
        let (request_tx, request_rx) = std::sync::mpsc::sync_channel(1);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("gregg-host-drive-refresh".into())
            .spawn(move || {
                let mut retry_delay = std::time::Duration::ZERO;
                loop {
                    let wait = if retry_delay.is_zero() {
                        DRIVE_REFRESH_INTERVAL
                    } else {
                        retry_delay
                    };
                    let request = request_rx.recv_timeout(wait);
                    if matches!(
                        request,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
                    ) {
                        break;
                    }
                    let caught =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| collect(&source)));
                    let (result, panicked) = if let Ok(result) = caught {
                        (result, false)
                    } else {
                        tracing::warn!("drive refresh worker collector panicked; retrying");
                        (
                            Err(CollectError::new(
                                CollectErrorKind::SourceUnavailable,
                                "drive refresh collector panicked",
                            )),
                            true,
                        )
                    };
                    retry_delay = if panicked {
                        if retry_delay.is_zero() {
                            DRIVE_REFRESH_RETRY_START
                        } else {
                            retry_delay
                                .checked_mul(2)
                                .map_or(DRIVE_REFRESH_INTERVAL, |delay| {
                                    delay.min(DRIVE_REFRESH_INTERVAL)
                                })
                        }
                    } else {
                        std::time::Duration::ZERO
                    };
                    if result_tx.send(result).is_err() {
                        break;
                    }
                }
            })
            .expect("drive refresh worker spawn");
        drop(worker);
        let _ = request_tx.try_send(());
        Self {
            request_tx: Some(request_tx),
            result_rx,
            latest: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn request(&self) {
        if let Some(sender) = &self.request_tx {
            let _ = sender.try_send(());
        }
    }

    /// Poll completed results without blocking. Returns the latest known
    /// list (`None` before the first successful refresh).
    pub fn poll(&mut self) -> Option<Vec<DriveMetrics>> {
        while let Ok(result) = self.result_rx.try_recv() {
            match result {
                Ok(drives) => self.latest = Some(drives),
                Err(error) => tracing::debug!(kind = ?error.kind),
            }
        }
        self.latest.clone()
    }
}

impl Drop for DriveRefreshCache {
    fn drop(&mut self) {
        let _ = self.request_tx.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if condition() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(condition(), "worker did not reach expected state");
    }

    #[test]
    fn blocked_refresh_does_not_block_cache_drop() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let started = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let started_for_worker = Arc::clone(&started);
        let release_for_worker = Arc::clone(&release);
        let mut cache = DriveRefreshCache::new((), move |()| {
            started_for_worker.store(true, Ordering::Release);
            while !release_for_worker.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            Ok(Vec::new())
        });

        wait_until(|| started.load(Ordering::Acquire));
        assert_eq!(cache.poll(), None);
        let before = std::time::Instant::now();
        drop(cache);
        assert!(before.elapsed() < std::time::Duration::from_millis(100));
        release.store(true, Ordering::Release);
    }

    #[test]
    fn retains_last_success_after_failure() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_worker = Arc::clone(&calls);
        let mut cache = DriveRefreshCache::new((), move |()| {
            let call = calls_for_worker.fetch_add(1, Ordering::AcqRel);
            if call == 0 {
                Ok(vec![DriveMetrics {
                    name: "root".to_string(),
                    used_bytes: 1,
                    total_bytes: 2,
                    available_bytes: Some(1),
                }])
            } else {
                Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    "refresh failed",
                ))
            }
        });

        wait_until(|| cache.poll().is_some());
        let first = cache.poll().expect("first drive result");
        assert_eq!(first[0].name, "root");
        cache.request();
        wait_until(|| calls.load(Ordering::Acquire) >= 2);
        assert_eq!(
            cache.poll().expect("last good drive result")[0].used_bytes,
            1
        );
    }
}
