use super::*;
use crate::config::{Env, Settings};
use crate::provider::http::{NetSettings, client_with, client_with_identity};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{ServerConfig, pki_types::PrivateKeyDer},
};

const CA: &[u8] = include_bytes!("../../../../fixtures/native-google-certificate/ca.pem");
const CERT: &[u8] = include_bytes!("../../../../fixtures/native-google-certificate/server.pem");
const KEY: &[u8] = include_bytes!("../../../../fixtures/native-google-certificate/server-key.pem");
const MALFORMED: &str = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n";
struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    async fn start() -> Self {
        let cert = CertificateDer::from_pem_slice(CERT).unwrap().into_owned();
        let key = PrivateKeyDer::from_pem_slice(KEY).unwrap();
        let tls = TlsAcceptor::from(Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![cert], key)
                .unwrap(),
        ));
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://localhost:{}", socket.local_addr().unwrap().port());
        let task = tokio::spawn(async move {
            let mut clients = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted=socket.accept()=>{
                        let Ok((tcp,_))=accepted else {break};let tls=tls.clone();
                        clients.spawn(async move {
                            let Ok(mut stream)=tls.accept(tcp).await else {return};
                            let mut head=Vec::new();
                            while !head.ends_with(b"\r\n\r\n") {let mut byte=[0];if head.len()>8192||stream.read(&mut byte).await.unwrap_or(0)==0 {return}head.push(byte[0]);}
                            let _=stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nverified").await;
                        });
                    },
                    _=clients.join_next(),if !clients.is_empty()=>{}
                }
            }
        });
        Self { url, task }
    }
}
fn env(path: &std::path::Path) -> Env {
    Env::isolated(std::collections::HashMap::from([(
        "NODE_EXTRA_CA_CERTS".into(),
        path.display().to_string(),
    )]))
}

#[tokio::test]
async fn extra_roots_match_ten_executed_node_undici_tls_cases() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/native-extra-ca/reference.json"
    ))
    .unwrap();
    let server = Server::start().await;
    let dir = tempfile::tempdir().unwrap();
    let ca = std::str::from_utf8(CA).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 10);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let path = dir.path().join(name);
        let content = match name {
            "unset" | "missing" => None,
            "valid" => Some(ca.to_owned()),
            "duplicate" => Some(ca.repeat(2)),
            "text-prefix" => Some(format!("ignored text\n{ca}")),
            "empty" => Some(String::new()),
            "garbage" => Some("not a certificate".into()),
            "malformed" => Some(MALFORMED.into()),
            "valid-then-malformed" => Some(format!("{ca}{MALFORMED}")),
            "malformed-then-valid" => Some(format!("{MALFORMED}{ca}")),
            _ => panic!("{name}"),
        };
        if let Some(content) = content {
            std::fs::write(&path, content).unwrap();
        }
        let input = if name == "unset" {
            Env::isolated(Default::default())
        } else {
            env(&path)
        };
        let net = NetSettings::new(&input, &Settings::default());
        assert_eq!(
            net.extra_roots.certificates.len(),
            case["roots"].as_u64().unwrap() as usize,
            "{name}"
        );
        assert_eq!(
            net.extra_roots.warning.is_some(),
            case["warning"].as_bool().unwrap(),
            "{name}"
        );
        let result = client_with(&net).get(&server.url).send().await;
        assert_eq!(
            result.is_ok(),
            case["ok"].as_bool().unwrap(),
            "{name}: {result:?}"
        );
        if let Ok(response) = result {
            assert_eq!(response.text().await.unwrap(), "verified");
        }
    }
}

#[tokio::test]
async fn extra_roots_reload_without_changing_active_snapshots_and_reach_identity_clients() {
    let server = Server::start().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ca.pem");
    std::fs::write(&path, CA).unwrap();
    let input = env(&path);
    let before = NetSettings::new(&input, &Settings::default());
    let old_client = client_with(&before);
    assert!(old_client.get(&server.url).send().await.is_ok());
    let identity_pem = [
        include_bytes!("../../../../fixtures/native-google-certificate/client.pem").as_slice(),
        include_bytes!("../../../../fixtures/native-google-certificate/client-key.pem").as_slice(),
    ]
    .concat();
    let client =
        client_with_identity(&before, reqwest::Identity::from_pem(&identity_pem).unwrap()).unwrap();
    assert!(client.get(&server.url).send().await.is_ok());
    std::fs::write(&path, "").unwrap();
    let after = NetSettings::new(&input, &Settings::default());
    assert_ne!(before, after);
    assert!(client_with(&after).get(&server.url).send().await.is_err());
    assert!(old_client.get(&server.url).send().await.is_ok());
    std::fs::write(&path, CA).unwrap();
    let restored = NetSettings::new(&input, &Settings::default());
    assert_eq!(before, restored);
    assert!(client_with(&restored).get(&server.url).send().await.is_ok());
}

#[test]
fn extra_root_files_are_bounded_and_invalid_paths_are_diagnostic_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bundle");
    assert!(
        ExtraRoots::from_env(&env(dir.path()))
            .warning
            .unwrap()
            .contains("regular file")
    );
    std::fs::write(&path, vec![b'x'; MAX_BUNDLE_BYTES as usize + 1]).unwrap();
    let roots = ExtraRoots::from_env(&env(&path));
    assert!(roots.warning.as_ref().unwrap().contains("1 MiB"));
    assert!(roots.certificates.is_empty());
    let masked = Env::isolated(std::collections::HashMap::from([(
        "NODE_EXTRA_CA_CERTS".into(),
        String::new(),
    )]));
    assert_eq!(ExtraRoots::from_env(&masked), ExtraRoots::default());
    assert!(!format!("{roots:?}").contains(path.to_str().unwrap()));
}

#[test]
fn extra_roots_refresh_in_the_running_engine_network_snapshot() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(Server::start());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("extra.pem");
    std::fs::write(&path, CA).unwrap();
    let mut config =
        crate::config::EngineConfig::new(dir.path().join("data"), dir.path().join("config"));
    config.env = env(&path);
    config.env.vars.insert("PI_OFFLINE".into(), "1".into());
    let engine = crate::runtime::EngineHandle::start(config).unwrap();
    let (original, client) = engine.shared().refresh_net_snapshot();
    assert!(
        rt.block_on(async { client.get(&server.url).send().await })
            .is_ok()
    );
    std::fs::write(&path, "").unwrap();
    let (revoked, client) = engine.shared().refresh_net_snapshot();
    assert_ne!(original, revoked);
    assert!(
        rt.block_on(async { client.get(&server.url).send().await })
            .is_err()
    );
    std::fs::write(&path, CA).unwrap();
    let (restored, client) = engine.shared().refresh_net_snapshot();
    assert_eq!(original, restored);
    assert!(
        rt.block_on(async { client.get(&server.url).send().await })
            .is_ok()
    );
}
