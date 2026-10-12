use super::*;
use pipkin_core::*;
use std::sync::mpsc;
use std::time::Duration;

fn event(n: usize) -> BackendEvent {
    BackendEvent {
        conversation: ConversationId(1),
        generation: 2,
        op: Some(OperationId(3)),
        kind: EventKind::Token("é".repeat(n)),
    }
}

fn receive(rx: &EventReceiver) -> Lease {
    let end = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(v) = rx.try_recv() {
            return v;
        }
        assert!(std::time::Instant::now() < end);
        std::thread::yield_now();
    }
}

#[test]
fn byte_permit_covers_the_dequeued_batch_and_keeps_fifo_lossless() {
    let first = event(500);
    let (tx, rx) = bounded(128, size::event_bytes(&first));
    tx.send_blocking(first.clone()).unwrap();
    let mut lease = receive(&rx);
    assert_eq!(lease.take(), first);
    let (done, finished) = mpsc::channel();
    let producer = tx.clone();
    let join = std::thread::spawn(move || {
        producer.send_blocking(event(501)).unwrap();
        done.send(()).unwrap();
    });
    assert!(
        finished.recv_timeout(Duration::from_millis(50)).is_err(),
        "dequeue must not release the batch's charge"
    );
    assert_eq!(tx.stats().0, 1);
    drop(lease);
    finished.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut second = receive(&rx);
    assert_eq!(second.take(), event(501), "oversize event remains whole");
    assert!(tx.stats().1 > size::event_bytes(&first));
    drop(second);
    join.join().unwrap();
    assert_eq!(tx.stats(), (0, 0, 0));
}

#[test]
fn count_limit_and_receiver_drop_wake_blocked_producers() {
    let (tx, rx) = bounded(1, usize::MAX);
    tx.send_blocking(event(0)).unwrap();
    let producer = tx.clone();
    let (done, finished) = mpsc::channel();
    let join = std::thread::spawn(move || {
        done.send(producer.send_blocking(event(1))).unwrap();
    });
    assert!(finished.recv_timeout(Duration::from_millis(50)).is_err());
    drop(rx);
    assert!(
        finished
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .is_err()
    );
    join.join().unwrap();
    assert_eq!(tx.stats(), (0, 0, 0));
}

#[test]
fn payload_accounting_includes_spare_capacity_nested_text_and_vector_storage() {
    let mut text = String::with_capacity(1024);
    text.push('x');
    let mut items = Vec::with_capacity(5);
    items.push(TranscriptItem {
        native_only: false,
        id: ItemId(1),
        at: 0,
        kind: ItemKind::User {
            text,
            attachments: vec![Attachment {
                path: "path".into(),
                name: "name".into(),
                size: None,
                error: Some("error".into()),
            }],
            delivery: Delivery::Sent,
            steer: false,
        },
    });
    let expected = size_of::<BackendEvent>()
        + items.capacity() * size_of::<TranscriptItem>()
        + 1024
        + size_of::<Attachment>()
        + 4
        + 4
        + 5;
    let mut e = event(0);
    e.kind = EventKind::Synced { items };
    assert_eq!(size::event_bytes(&e), expected);
    e.kind = EventKind::UiState {
        requests: vec![UiRequest {
            id: "id".into(),
            kind: UiRequestKind::Input,
            title: "title".into(),
            message: Some("message".into()),
            items: vec![UiRequestItem {
                value: "v".into(),
                label: "l".into(),
                description: Some("d".into()),
            }],
            placeholder: Some("p".into()),
            default_value: Some("x".into()),
            deadline: None,
        }],
        status: vec![("key".into(), "value".into())],
        notices: vec![UiNotice {
            id: "id".into(),
            level: UiNoticeLevel::Info,
            message: "notice".into(),
        }],
    };
    assert_eq!(
        size::event_bytes(&e),
        size_of::<BackendEvent>()
            + size_of::<UiRequest>()
            + size_of::<UiRequestItem>()
            + size_of::<(String, String)>()
            + size_of::<UiNotice>()
            + 2
            + 5
            + 7
            + 3
            + 1
            + 1
            + 3
            + 5
            + 2
            + 6
    );
}
