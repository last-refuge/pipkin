use super::*;

#[test]
fn concurrent_duplicate_admission_returns_exactly_one_dispatch_claim() {
    let s = std::sync::Arc::new(Store::memory().unwrap());
    let (_, lease) = connected(&s, "s");
    let lease = std::sync::Arc::new(lease);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(16));
    let threads: Vec<_> = (0..16)
        .map(|_| {
            let s = s.clone();
            let lease = lease.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                match s
                    .admit_cli_bridge_call(&lease, &json!(1), "tool", &json!({}), 4)
                    .unwrap()
                {
                    Admission::New(_) => 1,
                    Admission::Pending => 0,
                    other => panic!("{other:?}"),
                }
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .sum::<usize>(),
        1
    );
}
#[test]
fn retained_request_identity_limit_does_not_evict_or_reuse_old_ids() {
    let s = Store::memory().unwrap();
    let (spec, l) = connected(&s, "s");
    // Seed already completed synthetic receipts to test the distinct-ID boundary without
    // thousands of transactions. No native tools/effects are involved.
    s.tx(|tx| {
        tx.execute("WITH RECURSIVE nums(n) AS (VALUES(0) UNION ALL SELECT n+1 FROM nums WHERE n+1<?3)
            INSERT INTO cli_bridge_calls(bridge,session,rpc_key,call_id,fingerprint,name,arguments,state,created_at,updated_at)
            SELECT ?1,?2,CAST(n AS TEXT),'seed-'||n,zeroblob(32),'tool','{}','intent',4,4 FROM nums",
            params![spec.id,spec.scope.session,MAX_REQUESTS as i64])?;
        tx.execute("UPDATE cli_bridge_calls SET state='completed',result='{}'",[])?; Ok(())
    }).unwrap();
    assert!(matches!(
        s.admit_cli_bridge_call(&l, &json!(MAX_REQUESTS), "tool", &json!({}), 5)
            .unwrap(),
        Admission::Fenced
    ));
    let count = s
        .read(|tx| {
            Ok(tx.query_row(
                "SELECT COUNT(*) FROM cli_bridge_calls WHERE state='completed'",
                [],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .unwrap();
    assert_eq!(count, MAX_REQUESTS as i64);
    assert_eq!(state(&s, &spec), "fenced");
}
#[test]
fn invalid_or_oversize_results_retire_without_synthetic_completion() {
    let s = Store::memory().unwrap();
    for (i, result) in [json!([]), json!({"text":"x".repeat(MAX_RESULT_BYTES)})]
        .iter()
        .enumerate()
    {
        let (spec, l) = connected(&s, &format!("s{i}"));
        let c = admit(&s, &l, json!(1));
        assert_eq!(
            s.settle_cli_bridge_call(&l, &c, result, 5).unwrap(),
            Settlement::Fenced
        );
        assert_eq!(receipt(&s, c.call_id()), ("unknown".into(), None));
        assert_eq!(state(&s, &spec), "fenced");
        assert_eq!(
            s.settle_cli_bridge_call(&l, &c, &json!({}), 6).unwrap(),
            Settlement::Unknown
        );
    }
}
#[test]
fn invalid_and_oversize_utf8_arguments_never_create_an_intent() {
    let s = Store::memory().unwrap();
    let (_, l) = connected(&s, "s");
    for args in [json!([]), json!({"text":"界".repeat(MAX_FRAME_BYTES/2)})] {
        assert!(
            s.admit_cli_bridge_call(&l, &json!(1), "tool", &args, 4)
                .is_err()
        );
    }
    assert!(matches!(
        s.admit_cli_bridge_call(&l, &json!(1), "tool", &json!({}), 5)
            .unwrap(),
        Admission::New(_)
    ));
}
