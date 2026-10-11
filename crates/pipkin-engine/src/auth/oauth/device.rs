//! RFC 8628 device-code polling (reference `auth/oauth/device-code.ts`).

use std::future::Future;
use std::time::Duration;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::CANCELLED;

const TIMEOUT_MESSAGE: &str = "Device flow timed out";
const SLOW_DOWN_TIMEOUT_MESSAGE: &str = "Device flow timed out after one or more slow_down responses. This is often caused by clock drift in WSL or VM environments. Please sync or restart the VM clock and try again.";
const MINIMUM_INTERVAL: Duration = Duration::from_millis(1000);
const DEFAULT_INTERVAL_SECONDS: f64 = 5.0;
const SLOW_DOWN_INCREMENT: Duration = Duration::from_millis(5000);

pub enum Poll<T> {
    Pending,
    /// Optionally with the server's new required interval (seconds).
    SlowDown(Option<f64>),
    Failed(String),
    Complete(T),
}

pub struct PollOptions {
    pub interval_seconds: Option<f64>,
    pub expires_in_seconds: Option<f64>,
    pub wait_before_first_poll: bool,
    /// Fixed interval for every wait (tests, `PIPKIN_OAUTH_POLL_INTERVAL_MS`).
    pub fixed_interval: Option<Duration>,
}

async fn sleep(d: Duration, cancel: &CancellationToken) -> Result<(), String> {
    tokio::select! {
        _ = cancel.cancelled() => Err(CANCELLED.into()),
        _ = tokio::time::sleep(d) => Ok(()),
    }
}

pub async fn poll_device<T, F, Fut>(
    opts: PollOptions,
    cancel: &CancellationToken,
    mut poll: F,
) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Poll<T>, String>>,
{
    let deadline = opts
        .expires_in_seconds
        .filter(|s| s.is_finite())
        .map(|s| Instant::now() + Duration::from_secs_f64(s.max(0.0)));
    let clamp = |d: Duration| match opts.fixed_interval {
        Some(f) => f,
        None => d.max(MINIMUM_INTERVAL),
    };
    let mut interval = clamp(Duration::from_secs_f64(
        opts.interval_seconds
            .filter(|s| s.is_finite() && *s >= 0.0)
            .unwrap_or(DEFAULT_INTERVAL_SECONDS),
    ));
    let remaining =
        |deadline: Option<Instant>| deadline.map(|d| d.saturating_duration_since(Instant::now()));
    let mut slow_downs = 0u32;
    if opts.wait_before_first_poll {
        let wait = remaining(deadline).map_or(interval, |r| r.min(interval));
        if !wait.is_zero() {
            sleep(wait, cancel).await?;
        }
    }
    while deadline.is_none_or(|d| Instant::now() < d) {
        if cancel.is_cancelled() {
            return Err(CANCELLED.into());
        }
        match poll().await? {
            Poll::Complete(v) => return Ok(v),
            Poll::Failed(m) => return Err(m),
            Poll::Pending => {}
            Poll::SlowDown(server) => {
                slow_downs += 1;
                interval = match server.filter(|s| s.is_finite() && *s > 0.0) {
                    Some(s) => clamp(Duration::from_secs_f64(s)),
                    None => clamp(interval + SLOW_DOWN_INCREMENT),
                };
            }
        }
        let wait = match remaining(deadline) {
            Some(r) if r.is_zero() => break,
            Some(r) => r.min(interval),
            None => interval,
        };
        sleep(wait, cancel).await?;
    }
    Err(if slow_downs > 0 {
        SLOW_DOWN_TIMEOUT_MESSAGE.into()
    } else {
        TIMEOUT_MESSAGE.into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn fast() -> PollOptions {
        PollOptions {
            interval_seconds: Some(5.0),
            expires_in_seconds: None,
            wait_before_first_poll: true,
            fixed_interval: Some(Duration::from_millis(1)),
        }
    }

    #[tokio::test]
    async fn polls_until_complete_and_honours_failures() {
        let n = AtomicU32::new(0);
        let r = poll_device(fast(), &CancellationToken::new(), || async {
            Ok(match n.fetch_add(1, Ordering::SeqCst) {
                0 => Poll::Pending,
                1 => Poll::SlowDown(Some(7.0)),
                _ => Poll::Complete(42),
            })
        })
        .await;
        assert_eq!(r, Ok(42));
        assert_eq!(n.load(Ordering::SeqCst), 3);
        let r: Result<(), _> = poll_device(fast(), &CancellationToken::new(), || async {
            Ok(Poll::Failed("denied".into()))
        })
        .await;
        assert_eq!(r, Err("denied".into()));
    }

    #[tokio::test]
    async fn times_out_and_cancels() {
        let mut opts = fast();
        opts.expires_in_seconds = Some(0.05);
        let r: Result<(), _> = poll_device(opts, &CancellationToken::new(), || async {
            Ok(Poll::SlowDown(None))
        })
        .await;
        assert_eq!(r, Err(SLOW_DOWN_TIMEOUT_MESSAGE.into()));
        let mut opts = fast();
        opts.expires_in_seconds = Some(0.05);
        let r: Result<(), _> = poll_device(opts, &CancellationToken::new(), || async {
            Ok(Poll::Pending)
        })
        .await;
        assert_eq!(r, Err(TIMEOUT_MESSAGE.into()));
        let cancel = CancellationToken::new();
        cancel.cancel();
        let r: Result<(), _> = poll_device(fast(), &cancel, || async { Ok(Poll::Pending) }).await;
        assert_eq!(r, Err(CANCELLED.into()));
    }

    #[tokio::test(start_paused = true)]
    async fn reference_intervals_apply_without_an_override() {
        let start = Instant::now();
        let n = AtomicU32::new(0);
        let opts = PollOptions {
            interval_seconds: Some(0.0),
            expires_in_seconds: Some(900.0),
            wait_before_first_poll: false,
            fixed_interval: None,
        };
        let r = poll_device(opts, &CancellationToken::new(), || async {
            Ok(match n.fetch_add(1, Ordering::SeqCst) {
                0 => Poll::SlowDown(None),
                _ => Poll::Complete(()),
            })
        })
        .await;
        assert!(r.is_ok());
        // Minimum 1 s, plus 5 s after slow_down.
        assert_eq!(start.elapsed(), Duration::from_secs(6));
    }
}
