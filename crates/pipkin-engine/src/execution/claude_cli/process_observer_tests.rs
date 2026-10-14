//! Fake local processes only; never invokes an installed CLI or inference.
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn spec(script: &str, cwd: &std::path::Path) -> Spec {
    Spec {
        executable: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        cwd: cwd.into(),
        env: std::env::vars_os().collect(),
        input: Vec::new(),
        mode: OutputMode::Capture,
        timeout: Duration::from_secs(5),
        grace: Duration::from_millis(100),
        max_stdout: 128 * 1024,
        max_stderr: 128 * 1024,
    }
}

#[tokio::test]
async fn observer_delivers_only_stdout_in_bounded_chunks_and_caller_finishes_decoder() {
    let dir = tempfile::tempdir().unwrap();
    let id = "11111111-1111-4111-8111-111111111111";
    let input = format!(
        "{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"{id}\"}}\n{}\n{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"session_id\":\"{id}\"}}",
        serde_json::json!({"type": "assistant", "message": "hëllo 🦀".repeat(2048)})
    );
    // Includes a final record without LF; callback chunks need not be valid UTF-8 alone.
    for mode in [OutputMode::Capture, OutputMode::Stream] {
        let capture = matches!(mode, OutputMode::Capture);
        let mut s = spec("cat; printf 'stderr-not-for-observer' >&2", dir.path());
        s.mode = mode;
        s.input = input.as_bytes().to_vec();
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(64);
        let report = run_observed(
            s,
            CancellationToken::new(),
            None,
            Some(Box::new(move |bytes| {
                assert!(!bytes.is_empty() && bytes.len() <= MAX_OBSERVER_CHUNK_BYTES);
                tx.try_send(bytes.to_vec()).map_err(|_| "queue rejected")
            })),
        )
        .await
        .unwrap();
        assert!(report.clean_exit(), "{report:?}");
        let mut decoder = Decoder::default();
        let mut delivered = Vec::new();
        let mut chunks = 0;
        while let Some(bytes) = rx.recv().await {
            decoder.feed(&bytes).unwrap();
            delivered.extend_from_slice(&bytes);
            chunks += 1;
        }
        assert!(chunks > 1);
        assert_eq!(delivered, input.as_bytes());
        assert_eq!(report.stdout_bytes, delivered.len() as u64);
        assert!(report.stderr_bytes > 0);
        let summary = decoder.finish().unwrap();
        assert!(summary.result_success);
        assert_eq!(summary.records, 3);
        if capture {
            assert_eq!(report.captured, delivered);
        } else {
            assert!(report.captured.is_empty());
            assert!(report.stream.unwrap().result_success);
        }
    }
}

#[tokio::test]
async fn observer_rejection_precedes_capture_or_decoding_and_fences_cleanup_once() {
    for mode in [OutputMode::Capture, OutputMode::Stream] {
        let dir = tempfile::tempdir().unwrap();
        let script = "trap 'test -f fence && echo yes > term-after-fence; exit 0' TERM; printf 'secret-invalid-json\n'; while :; do :; done";
        let mut s = spec(script, dir.path());
        s.mode = mode;
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let fences = Arc::new(AtomicUsize::new(0));
        let fenced = fences.clone();
        let marker = dir.path().join("fence");
        let report = run_observed(
            s,
            CancellationToken::new(),
            Some(Box::new(move || {
                fenced.fetch_add(1, Ordering::SeqCst);
                std::fs::write(&marker, b"revoked").is_ok()
            })),
            Some(Box::new(move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
                Err("secret-callback-error-do-not-expose")
            })),
        )
        .await
        .unwrap();
        assert_eq!(report.stop, Some(Stop::ObserverFailure));
        assert_eq!(report.stream_error, Some("stdout observer failed"));
        assert!(!report.clean_exit());
        assert!(report.root_reaped && report.pipes_drained && !report.cleanup_timed_out);
        assert!(report.captured.is_empty());
        if let Some(summary) = &report.stream {
            assert_eq!(summary.records, 0);
            assert!(!summary.result_success);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(fences.load(Ordering::SeqCst), 1);
        assert_eq!(report.approval_revocation_confirmed, Some(true));
        assert_eq!(
            std::fs::read(dir.path().join("term-after-fence")).unwrap(),
            b"yes\n"
        );
        for diagnostic in [
            format!("{report:?}"),
            serde_json::to_string(&report).unwrap(),
        ] {
            assert!(!diagnostic.contains("secret"));
        }
    }
}

#[tokio::test]
async fn bounded_observer_queue_pressure_stops_flood_without_waiting_for_consumer() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = spec("yes private-stdout", dir.path());
    s.max_stdout = 16 * 1024 * 1024;
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(1);
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let report = run_observed(
        s,
        CancellationToken::new(),
        None,
        Some(Box::new(move |bytes| {
            observed.fetch_add(1, Ordering::SeqCst);
            tx.try_send(bytes.to_vec())
                .map_err(|_| "private-queue-error")
        })),
    )
    .await
    .unwrap();
    assert_eq!(report.stop, Some(Stop::ObserverFailure));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(!report.clean_exit());
    assert!(report.root_reaped && report.pipes_drained && !report.cleanup_timed_out);
    assert!(report.elapsed_ms < 3000);
    assert!(report.captured.len() <= MAX_OBSERVER_CHUNK_BYTES);
    assert!(rx.try_recv().unwrap().len() <= MAX_OBSERVER_CHUNK_BYTES);
    assert!(rx.try_recv().is_err());
    assert!(!format!("{report:?}").contains("private"));
}

#[tokio::test]
async fn byte_budget_is_checked_before_observer_delivery() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = spec("printf 'not-observed'; sleep 30", dir.path());
    s.max_stdout = 0;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let report = run_observed(
        s,
        CancellationToken::new(),
        None,
        Some(Box::new(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })),
    )
    .await
    .unwrap();
    assert_eq!(report.stop, Some(Stop::OutputLimit));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(report.captured.is_empty());
    assert!(report.root_reaped && report.pipes_drained && !report.cleanup_timed_out);
}

#[tokio::test]
async fn cancellation_preempts_queued_observer_delivery() {
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let cancellation = cancel.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let report = run_observed(
        spec("yes stdout", dir.path()),
        cancel,
        None,
        Some(Box::new(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            cancellation.cancel();
            Ok(())
        })),
    )
    .await
    .unwrap();
    assert_eq!(report.stop, Some(Stop::Cancelled));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(report.root_reaped && report.pipes_drained && !report.cleanup_timed_out);
    assert!(!report.clean_exit());
}

#[tokio::test]
async fn observed_future_drop_still_revokes_before_group_kill() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("fence");
    let written = marker.clone();
    let (tx, mut rx) = mpsc::channel(1);
    {
        let future = run_observed(
            spec("printf 'ready'; sleep 30", dir.path()),
            CancellationToken::new(),
            Some(Box::new(move || {
                std::fs::write(&written, b"revoked").is_ok()
            })),
            Some(Box::new(move |_| {
                tx.try_send(()).map_err(|_| "queue rejected")
            })),
        );
        tokio::pin!(future);
        tokio::select! {
            result = &mut future => panic!("unexpected early exit: {result:?}"),
            ready = tokio::time::timeout(Duration::from_secs(3), rx.recv()) => {
                assert_eq!(ready.unwrap(), Some(()));
            }
        }
    }
    assert_eq!(std::fs::read(marker).unwrap(), b"revoked");
    assert_eq!(rx.recv().await, None);
}

#[tokio::test]
async fn callback_panics_still_fence_once_and_cleanup_without_becoming_success() {
    let dir = tempfile::tempdir().unwrap();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let report = run_observed(
        spec("printf 'output'; sleep 30", dir.path()),
        CancellationToken::new(),
        Some(Box::new(move || {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            panic!("fixture fence panic");
        })),
        Some(Box::new(|_| panic!("fixture observer panic"))),
    )
    .await
    .unwrap();
    assert_eq!(report.stop, Some(Stop::ObserverFailure));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(report.approval_revocation_confirmed, Some(false));
    assert!(report.root_reaped && report.pipes_drained && !report.cleanup_timed_out);
    assert!(!report.clean_exit());
}

#[tokio::test]
async fn observer_failure_and_failed_revocation_cannot_become_clean_success() {
    let dir = tempfile::tempdir().unwrap();
    let report = run_observed(
        spec("printf 'output'; sleep 30", dir.path()),
        CancellationToken::new(),
        Some(Box::new(|| false)),
        Some(Box::new(|_| Err("private-callback-error"))),
    )
    .await
    .unwrap();
    assert_eq!(report.stop, Some(Stop::ObserverFailure));
    assert_eq!(report.approval_revocation_confirmed, Some(false));
    assert!(report.root_reaped && report.pipes_drained && !report.cleanup_timed_out);
    assert!(!report.clean_exit());
}
