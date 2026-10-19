use super::*;
use crate::{
    execution::claude_cli::orchestration_protocol::Scope, store::cli_bridge::ORCHESTRATION_BACKEND,
};
use rusqlite::params;
use serde_json::json;
fn fixture() -> (Arc<Store>, BridgeSpec) {
    let store = Arc::new(Store::memory().unwrap());
    let run=store.tx(|tx| {
        tx.execute("INSERT INTO sessions(id,cwd,created_at,updated_at,origin,execution_backend) VALUES('s','/project',1,1,'{}',?1)",[ORCHESTRATION_BACKEND])?;
        tx.execute("INSERT INTO runs(session,state,started_at) VALUES('s','running',1)",[])?; Ok(tx.last_insert_rowid() as u64)
    }).unwrap();
    (
        store,
        BridgeSpec {
            id: "bridge".into(),
            scope: Scope {
                session: "s".into(),
                generation: 1,
                operation: "op".into(),
                run,
            },
            owner: "owner".into(),
            binding_hash: [1; 32],
            capability_hash: [0; 32],
        },
    )
}
#[test]
fn helper_configuration_is_closed_local_and_redacted() {
    let secret = Secret::fresh().unwrap();
    let valid =
        json!({"endpoint":"127.0.0.1:1234","secret":secret.to_hex(),"scope":"00".repeat(32)});
    let launch = Launch::from_environment_value(&valid.to_string()).unwrap();
    assert!(!format!("{launch:?}").contains(&secret.to_hex()));
    assert_eq!(
        Launch::from_environment_value(&launch.environment_value())
            .unwrap()
            .scope,
        [0; 32]
    );
    for endpoint in [
        "localhost:1234",
        "192.0.2.1:1234",
        "0.0.0.0:1234",
        "[::1]:1234",
        "127.0.0.1:0",
        "127.0.0.2:1234",
    ] {
        let mut bad = valid.clone();
        bad["endpoint"] = json!(endpoint);
        assert!(Launch::from_environment_value(&bad.to_string()).is_err());
    }
    let mut bad = valid.clone();
    bad["extra"] = json!(true);
    assert!(Launch::from_environment_value(&bad.to_string()).is_err());
    let duplicate = valid.to_string().replacen("{", "{\"scope\":\"other\",", 1);
    assert!(Launch::from_environment_value(&duplicate).is_err());
}

fn end() -> Instant {
    Instant::now() + Duration::from_secs(10)
}
fn state(store: &Store) -> String {
    store
        .read(|c| {
            Ok(c.query_row(
                "SELECT state FROM cli_tool_bridges WHERE id='bridge'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap()
}
#[tokio::test]
async fn native_handoff_commits_catalog_binding_separately_from_secret_capability() {
    let store = Arc::new(Store::memory().unwrap());
    let binding = store
        .create_cli_agent_session(&crate::store::cli_agent::tests::create("s", "/project"), 1)
        .unwrap()
        .0;
    let run = store
        .tx(|tx| {
            tx.execute(
                "INSERT INTO runs(session,state,started_at) VALUES('s','running',1)",
                [],
            )?;
            Ok(tx.last_insert_rowid() as u64)
        })
        .unwrap();
    let spec = BridgeSpec {
        id: "bridge".into(),
        scope: crate::execution::claude_cli::orchestration_protocol::Scope {
            session: "s".into(),
            generation: 1,
            operation: "op".into(),
            run,
        },
        owner: "owner".into(),
        binding_hash: binding.digest,
        capability_hash: [0; 32],
    };
    let original = spec.binding_hash;
    let catalog = [7; 32];
    let server = Server::bind_native(store.clone(), spec, catalog, end())
        .await
        .unwrap();
    let identity = server.identity().clone();
    assert_eq!(
        identity.binding_hash,
        native_binding_digest(original, catalog)
    );
    assert_ne!(identity.capability_hash, catalog);
    assert_ne!(
        identity.binding_hash,
        native_binding_digest(original, [8; 32])
    );
    assert_ne!(
        identity.binding_hash,
        native_binding_digest([2; 32], catalog)
    );
    let launch = server.launch();
    let (host, client) = tokio::join!(server.accept(), launch.connect(end()));
    let host = host.unwrap();
    assert_eq!(host.native_catalog(), Some(catalog));
    assert_eq!(host.parts.lease.scope(), &identity.scope);
    drop(client);
    drop(host);
    retire(store, identity).await.unwrap();
}
#[tokio::test]
async fn native_binding_mismatch_or_unbound_session_cannot_prepare_a_channel() {
    let (store, spec) = fixture();
    assert!(
        Server::bind_native(store.clone(), spec, [7; 32], end())
            .await
            .is_err()
    );
    let binding = store
        .create_cli_agent_session(
            &crate::store::cli_agent::tests::create("bound", "/project"),
            1,
        )
        .unwrap()
        .0;
    let run = store
        .tx(|tx| {
            tx.execute(
                "INSERT INTO runs(session,state,started_at) VALUES('bound','running',1)",
                [],
            )?;
            Ok(tx.last_insert_rowid() as u64)
        })
        .unwrap();
    let mut wrong = binding.digest;
    wrong[0] ^= 1;
    let spec = BridgeSpec {
        id: "bound-bridge".into(),
        scope: crate::execution::claude_cli::orchestration_protocol::Scope {
            session: "bound".into(),
            generation: 1,
            operation: "op".into(),
            run,
        },
        owner: "owner".into(),
        binding_hash: wrong,
        capability_hash: [0; 32],
    };
    assert!(
        Server::bind_native(store.clone(), spec, [7; 32], end())
            .await
            .is_err()
    );
    assert_eq!(
        store
            .read(|c| Ok(
                c.query_row("SELECT COUNT(*) FROM cli_tool_bridges", [], |r| r
                    .get::<_, i64>(0))?
            ))
            .unwrap(),
        0
    );
}
#[tokio::test]
async fn authenticated_peer_receives_channel_only_after_durable_claim() {
    let (store, spec) = fixture();
    let server = Server::bind(store.clone(), spec, end()).await.unwrap();
    let identity = server.identity().clone();
    let launch = server.launch();
    let (host, client) = tokio::join!(server.accept(), launch.connect(end()));
    let mut host = host.unwrap().into_parts();
    let (_reader, mut writer) = client.unwrap().split();
    assert_eq!(state(&store), "connected");
    assert!(store.claim_cli_tool_bridge(&identity, 2).unwrap().is_none());
    assert_eq!(host.lease.scope(), &identity.scope);
    let (sent, received) = tokio::join!(
        writer.write(b"{\"method\":\"tools/list\"}", end()),
        host.reader.read(end())
    );
    sent.unwrap();
    assert_eq!(received.unwrap(), b"{\"method\":\"tools/list\"}");
    // Transport only: a valid frame did not execute or create any native tool intent.
    assert_eq!(
        store
            .read(
                |c| Ok(c.query_row("SELECT COUNT(*) FROM tool_calls", [], |r| r
                    .get::<_, i64>(0))?)
            )
            .unwrap(),
        0
    );
    host.reader.revoke();
    retire(store.clone(), identity).await.unwrap();
    assert_eq!(state(&store), "fenced");
}
#[tokio::test]
async fn unauthenticated_or_foreign_scope_peers_do_not_consume_or_revoke_real_owner() {
    let (store, spec) = fixture();
    let server = Server::bind(store.clone(), spec, end()).await.unwrap();
    let launch = server.launch();
    let task = tokio::spawn(server.accept());
    for wrong_scope in [false, true] {
        let mut bad = launch.clone();
        if wrong_scope {
            bad.scope[0] ^= 1;
        } else {
            bad.secret = Secret::fresh().unwrap();
        }
        assert!(bad.connect(end()).await.is_err());
        assert_eq!(state(&store), "prepared");
    }
    let client = launch.connect(end()).await.unwrap();
    let host = task.await.unwrap().unwrap();
    assert_eq!(state(&store), "connected");
    drop(client);
    drop(host);
}
#[tokio::test]
async fn failed_claim_commit_produces_no_commit_proof_and_retires_identity() {
    let (store, spec) = fixture();
    let server = Server::bind(store.clone(), spec, end()).await.unwrap();
    let launch = server.launch();
    store.fail_next_write();
    let (host, client) = tokio::join!(server.accept(), launch.connect(end()));
    assert!(host.is_err());
    assert!(client.is_err());
    assert_eq!(state(&store), "fenced");
}
#[tokio::test]
async fn stop_before_claim_and_changed_binding_refuse_authority() {
    let (store, spec) = fixture();
    let server = Server::bind(store.clone(), spec, end()).await.unwrap();
    let identity = server.identity().clone();
    let launch = server.launch();
    let mut foreign = identity.clone();
    foreign.binding_hash[0] ^= 1;
    assert!(retire(store.clone(), foreign).await.is_err());
    assert_eq!(state(&store), "prepared");
    store
        .tx(|tx| {
            tx.execute(
                "UPDATE runs SET stop_requested_at=2 WHERE id=?1",
                params![identity.scope.run as i64],
            )?;
            Ok(())
        })
        .unwrap();
    let (host, client) = tokio::join!(server.accept(), launch.connect(end()));
    assert!(host.is_err());
    assert!(client.is_err());
    assert_eq!(state(&store), "fenced");
}
#[tokio::test]
async fn lost_connection_cannot_rebind_existing_run_or_resume_native_intent() {
    let (store, spec) = fixture();
    let server = Server::bind(store.clone(), spec, end()).await.unwrap();
    let identity = server.identity().clone();
    let launch = server.launch();
    let (host, client) = tokio::join!(server.accept(), launch.connect(end()));
    let mut host = host.unwrap().into_parts();
    let claim = store
        .admit_cli_bridge_call(&host.lease, &json!(1), "tool", &json!({}), 2)
        .unwrap();
    assert!(matches!(claim, crate::store::cli_bridge::Admission::New(_)));
    drop(client);
    assert!(host.reader.read(end()).await.is_err());
    retire(store.clone(), identity.clone()).await.unwrap();
    assert!(Server::bind(store.clone(), identity, end()).await.is_err());
    assert_eq!(
        store
            .read(
                |c| Ok(c.query_row("SELECT state FROM cli_bridge_calls", [], |r| r
                    .get::<_, String>(0))?)
            )
            .unwrap(),
        "unknown"
    );
}
#[tokio::test]
async fn explicit_owner_cleanup_after_cancelled_accept_retires_prepared_identity() {
    let (store, spec) = fixture();
    let server = Server::bind(store.clone(), spec, end()).await.unwrap();
    let identity = server.identity().clone();
    let task = tokio::spawn(server.accept());
    task.abort();
    let _ = task.await;
    retire(store.clone(), identity.clone()).await.unwrap();
    assert_eq!(state(&store), "fenced");
    assert!(store.claim_cli_tool_bridge(&identity, 3).unwrap().is_none());
}
