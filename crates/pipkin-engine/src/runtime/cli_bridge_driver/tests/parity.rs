//! Real authenticated transport/native store; owned tools and files, no CLI inference.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
struct Gate {
    name: &'static str,
    sequential: bool,
    cooperative: bool,
    entered: mpsc::UnboundedSender<usize>,
    gates: Vec<Arc<Semaphore>>,
    live: Arc<AtomicUsize>,
}
struct Live(Arc<AtomicUsize>);
impl Drop for Live {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl Tool for Gate {
    fn spec(&self) -> crate::provider::ToolSpec {
        crate::provider::ToolSpec {
            name: self.name.into(),
            description: "owned gate".into(),
            parameters: json!({"type":"object","required":["i"],"properties":{"i":{"type":"integer"}}}),
            constrained_sampling: None,
        }
    }
    fn sequential(&self) -> bool {
        self.sequential
    }
    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolCtx,
    ) -> crate::provider::BoxFuture<'a, Result<tools::ToolOutput, tools::ToolError>> {
        Box::pin(async move {
            let i = args["i"].as_u64().unwrap() as usize;
            self.live.fetch_add(1, Ordering::SeqCst);
            let _live = Live(self.live.clone());
            self.entered.send(i).unwrap();
            if self.cooperative {
                tokio::select! { _=ctx.cancel.cancelled()=>return Err(tools::ToolError::new("cancelled")),p=self.gates[i].acquire()=>p.unwrap().forget() }
            } else {
                self.gates[i].acquire().await.unwrap().forget();
            }
            Ok(tools::ToolOutput::default())
        })
    }
}
fn gate(
    name: &'static str,
    sequential: bool,
    cooperative: bool,
) -> (Arc<Gate>, mpsc::UnboundedReceiver<usize>) {
    let (entered, rx) = mpsc::unbounded_channel();
    (
        Arc::new(Gate {
            name,
            sequential,
            cooperative,
            entered,
            gates: (0..10).map(|_| Arc::new(Semaphore::new(0))).collect(),
            live: Arc::default(),
        }),
        rx,
    )
}
async fn entered(rx: &mut mpsc::UnboundedReceiver<usize>) -> usize {
    timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap()
}
async fn ping(r: &mut Reader<ReadHalf<TcpStream>>, w: &mut Writer<WriteHalf<TcpStream>>) {
    send(w, json!({"jsonrpc":"2.0","id":"ping","method":"ping"})).await;
    assert_eq!(receive(r).await["id"], "ping");
}
fn cancel_call(id: u64) -> Value {
    json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":id}})
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_parallel_limit_fifo_barrier_and_out_of_order_results() {
    let dir = tempfile::tempdir().unwrap();
    let (parallel, mut arrivals) = gate("parallel", false, true);
    let (barrier, mut barriers) = gate("barrier", true, true);
    let (_s, (mut r, mut w), cancel, task) =
        start(dir.path(), vec![parallel.clone(), barrier.clone()]).await;
    for i in 0..4 {
        send(&mut w, call(i as u64, "parallel", json!({"i":i}))).await;
    }
    let mut first = vec![];
    for _ in 0..4 {
        first.push(entered(&mut arrivals).await);
    }
    first.sort();
    assert_eq!(first, vec![0, 1, 2, 3]);
    send(&mut w, call(4, "parallel", json!({"i":4}))).await;
    send(&mut w, call(5, "barrier", json!({"i":5}))).await;
    send(&mut w, call(6, "parallel", json!({"i":6}))).await;
    ping(&mut r, &mut w).await;
    assert!(arrivals.try_recv().is_err());
    assert!(barriers.try_recv().is_err());
    assert_eq!(parallel.live.load(Ordering::SeqCst), 4);
    parallel.gates[3].add_permits(1);
    assert_eq!(receive(&mut r).await["id"], 3);
    assert_eq!(entered(&mut arrivals).await, 4);
    for i in [0, 1, 2, 4] {
        parallel.gates[i].add_permits(1);
    }
    for _ in 0..4 {
        assert!(
            !receive(&mut r).await["result"]["isError"]
                .as_bool()
                .unwrap()
        );
    }
    assert_eq!(entered(&mut barriers).await, 5);
    assert!(arrivals.try_recv().is_err());
    assert_eq!(parallel.live.load(Ordering::SeqCst), 0);
    barrier.gates[5].add_permits(1);
    assert_eq!(receive(&mut r).await["id"], 5);
    assert_eq!(entered(&mut arrivals).await, 6);
    parallel.gates[6].add_permits(1);
    assert_eq!(receive(&mut r).await["id"], 6);
    cancel.cancel();
    assert!(task.await.unwrap().is_err());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_cancel_queued_and_active_calls_preserves_siblings_and_exact_replay() {
    let dir = tempfile::tempdir().unwrap();
    let (barrier, mut arrivals) = gate("barrier", true, true);
    let mut offered = tools::builtin_tools();
    offered.push(barrier);
    let (s, (mut r, mut w), cancel, task) = start(dir.path(), offered).await;
    send(&mut w, call(1, "barrier", json!({"i":1}))).await;
    assert_eq!(entered(&mut arrivals).await, 1);
    let queued = call(2, "write", json!({"path":"never","content":"no"}));
    send(&mut w, queued.clone()).await;
    send(&mut w, cancel_call(2)).await;
    let refused = receive(&mut r).await;
    assert_eq!(refused["id"], 2);
    assert_eq!(refused["result"]["isError"], true);
    send(&mut w, queued).await;
    assert_eq!(receive(&mut r).await, refused);
    assert!(!dir.path().join("never").exists());
    send(&mut w, cancel_call(1)).await;
    let aborted = receive(&mut r).await;
    assert_eq!(aborted["id"], 1);
    assert_eq!(aborted["result"]["isError"], true);
    send(&mut w, cancel_call(1)).await;
    send(
        &mut w,
        call(3, "write", json!({"path":"sibling","content":"once"})),
    )
    .await;
    assert_eq!(receive(&mut r).await["result"]["isError"], false);
    assert_eq!(
        s.read(|c| Store::tools_in_state(c, "s", "done"))
            .unwrap()
            .len(),
        3
    );
    cancel.cancel();
    assert!(task.await.unwrap().is_err());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_stop_joins_all_parallel_workers_and_never_starts_queued_effects() {
    let dir = tempfile::tempdir().unwrap();
    let (parallel, mut arrivals) = gate("parallel", false, true);
    let mut offered = tools::builtin_tools();
    offered.push(parallel.clone());
    let (s, (mut r, mut w), cancel, task) = start(dir.path(), offered).await;
    for i in 0..4 {
        send(&mut w, call(i as u64, "parallel", json!({"i":i}))).await;
    }
    for _ in 0..4 {
        entered(&mut arrivals).await;
    }
    send(
        &mut w,
        call(4, "write", json!({"path":"never","content":"no"})),
    )
    .await;
    ping(&mut r, &mut w).await;
    cancel.cancel();
    assert!(task.await.unwrap().is_err());
    assert_eq!(parallel.live.load(Ordering::SeqCst), 0);
    assert!(!dir.path().join("never").exists());
    assert_eq!(
        s.read(|c| Store::tools_in_state(c, "s", "interrupted"))
            .unwrap()
            .len(),
        4
    );
    assert!(
        s.read(|c| Store::tools_in_state(c, "s", "done"))
            .unwrap()
            .is_empty()
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_noncooperative_call_cancel_fences_and_joins_before_return() {
    let dir = tempfile::tempdir().unwrap();
    let (tool, mut arrivals) = gate("stubborn", false, false);
    let (s, (mut r, mut w), _cancel, task) = start(dir.path(), vec![tool.clone()]).await;
    send(&mut w, call(1, "stubborn", json!({"i":1}))).await;
    entered(&mut arrivals).await;
    send(&mut w, cancel_call(1)).await;
    assert_eq!(
        timeout(Duration::from_secs(12), task)
            .await
            .unwrap()
            .unwrap(),
        Err(DriverError::Cancelled)
    );
    assert_eq!(tool.live.load(Ordering::SeqCst), 0);
    assert!(r.read(end()).await.is_err());
    assert_eq!(
        s.read(|c| Store::tools_in_state(c, "s", "interrupted"))
            .unwrap()
            .len(),
        1
    );
}
struct Image;
impl Tool for Image {
    fn spec(&self) -> crate::provider::ToolSpec {
        crate::provider::ToolSpec {
            name: "image".into(),
            description: "fixture image".into(),
            parameters: json!({"type":"object"}),
            constrained_sampling: None,
        }
    }
    fn execute<'a>(
        &'a self,
        _: Value,
        _: &'a ToolCtx,
    ) -> crate::provider::BoxFuture<'a, Result<tools::ToolOutput, tools::ToolError>> {
        Box::pin(async {
            Ok(tools::ToolOutput {
                content: Some(vec![pipkin_engine_protocol::Block::Image {
                    data: "A".repeat(2 * 1024 * 1024),
                    mime_type: "image/png".into(),
                    extra: Default::default(),
                }]),
                details: Some(json!({"detail":"d".repeat(2*1024*1024)})),
                ..Default::default()
            })
        })
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_large_utf8_write_and_structured_image_results_replay_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let mut offered = tools::builtin_tools();
    offered.push(Arc::new(Image));
    let (s, (mut r, mut w), cancel, task) = start(dir.path(), offered).await;
    let text = "λ\n\0".repeat(180_000);
    send(
        &mut w,
        call(1, "write", json!({"path":"large","content":text})),
    )
    .await;
    assert_eq!(receive(&mut r).await["result"]["isError"], false);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("large")).unwrap(),
        text
    );
    send(&mut w, call(2, "image", json!({}))).await;
    let result = receive(&mut r).await;
    assert_eq!(
        result["result"]["content"][0]["data"]
            .as_str()
            .unwrap()
            .len(),
        2 * 1024 * 1024
    );
    assert_eq!(result["result"]["content"][0]["mimeType"], "image/png");
    send(&mut w, call(2, "image", json!({}))).await;
    assert_eq!(receive(&mut r).await, result);
    assert_eq!(s.entry_count("s").unwrap(), 4);
    cancel.cancel();
    assert!(task.await.unwrap().is_err());
}
