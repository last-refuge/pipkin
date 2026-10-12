#[test]
fn credential_cache_is_bounded_and_respects_expiration_and_recency() {
    let mut cache = CredentialCache::default();
    let mut c = AwsCredentials::new("ID", "synthetic", None);
    c.expires_at = Some(now_secs() + 3600);
    for i in 0..64 {
        cache.insert(&i.to_string(), &c);
    }
    assert!(cache.get("0").is_some());
    cache.insert("new", &c);
    assert_eq!(cache.entries.len(), 64);
    assert!(cache.get("1").is_none());
    assert!(cache.get("0").is_some());
    c.expires_at = Some(now_secs() + 100);
    cache.insert("near-expiry", &c);
    assert!(cache.get("near-expiry").is_none());
    cache.entries.get_mut("0").unwrap().0.expires_at = Some(0);
    assert!(cache.get("0").is_none());
    c.expires_at = None;
    cache.insert("permanent", &c);
    assert!(cache.get("permanent").is_none());
}

#[tokio::test]
async fn process_cache_separates_homes_commands_and_effective_environments() {
    let config = r#"[profile shared]
credential_process = printf x >> "$HOME/calls"; printf '{"Version":1,"AccessKeyId":"%s","SecretAccessKey":"synthetic","Expiration":"2099-01-01T00:00:00Z"}' "$HELPER_ID"
"#;
    let mut a = Fixture::new(config, "");
    let mut b = Fixture::new(config, "");
    a.set("HELPER_ID", "A");
    b.set("HELPER_ID", "B");
    for f in [&a, &b, &a, &b] {
        assert_eq!(
            f.resolve(Some("shared"), None).await.unwrap().access_key_id,
            f.vars["HELPER_ID"]
        );
    }
    assert_eq!(
        std::fs::read_to_string(a.dir.path().join("calls")).unwrap(),
        "x"
    );
    assert_eq!(
        std::fs::read_to_string(b.dir.path().join("calls")).unwrap(),
        "x"
    );
    a.set("HELPER_ID", "CHANGED");
    assert_eq!(
        a.resolve(Some("shared"), None).await.unwrap().access_key_id,
        "CHANGED"
    );
    std::fs::write(
        a.dir.path().join("config"),
        config.replace("$HELPER_ID", "COMMAND"),
    )
    .unwrap();
    assert_eq!(
        a.resolve(Some("shared"), None).await.unwrap().access_key_id,
        "COMMAND"
    );
    assert_eq!(
        std::fs::read_to_string(a.dir.path().join("calls")).unwrap(),
        "xxx"
    );
}

#[tokio::test]
async fn assumed_role_cache_separates_source_identity_and_session_parameters() {
    let server = MockServer::start(|req, _| {
        let id = if req
            .header("authorization")
            .unwrap()
            .contains("Credential=A/")
        {
            "ROLE-A"
        } else {
            "ROLE-B"
        };
        let mut r = MockResponse::json(200, &serde_json::json!({}));
        r.chunks = vec![STS_OK.replace("ASIAROLE", id).into_bytes()];
        r
    })
    .await;
    let config = "[profile role]\nrole_arn = arn:aws:iam::123:role/test\nsource_profile = base\nrole_session_name = first\n";
    let mut f = Fixture::new(
        config,
        "[base]\naws_access_key_id = A\naws_secret_access_key = synthetic\n",
    );
    f.set("AWS_ENDPOINT_URL_STS", &server.url);
    for _ in 0..2 {
        assert_eq!(
            f.resolve(Some("role"), None).await.unwrap().access_key_id,
            "ROLE-A"
        );
    }
    std::fs::write(
        f.dir.path().join("credentials"),
        "[base]\naws_access_key_id = B\naws_secret_access_key = synthetic\n",
    )
    .unwrap();
    assert_eq!(
        f.resolve(Some("role"), None).await.unwrap().access_key_id,
        "ROLE-B"
    );
    std::fs::write(
        f.dir.path().join("config"),
        config.replace("first", "second") + "external_id = tenant-b\n",
    )
    .unwrap();
    assert_eq!(
        f.resolve(Some("role"), None).await.unwrap().access_key_id,
        "ROLE-B"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    let body = String::from_utf8_lossy(&requests[2].body);
    assert!(
        body.contains("RoleSessionName=second") && body.contains("ExternalId=tenant-b"),
        "{body}"
    );
}

#[tokio::test]
async fn web_identity_cache_reloads_rotating_token_and_session() {
    let server = MockServer::start(|req, _| {
        let id = if String::from_utf8_lossy(&req.body).contains("WebIdentityToken=first") {
            "WEB-A"
        } else {
            "WEB-B"
        };
        let mut r = MockResponse::json(200, &serde_json::json!({}));
        r.chunks = vec![
            STS_OK
                .replace("AssumeRole", "AssumeRoleWithWebIdentity")
                .replace("ASIAROLE", id)
                .into_bytes(),
        ];
        r
    })
    .await;
    let mut f = Fixture::new("", "");
    let path = f.dir.path().join("token");
    std::fs::write(&path, "first").unwrap();
    f.set("AWS_WEB_IDENTITY_TOKEN_FILE", path.to_str().unwrap());
    f.set("AWS_ROLE_ARN", "arn:aws:iam::1:role/web");
    f.set("AWS_ROLE_SESSION_NAME", "s1");
    f.set("AWS_ENDPOINT_URL_STS", &server.url);
    for _ in 0..2 {
        assert_eq!(f.resolve(None, None).await.unwrap().access_key_id, "WEB-A");
    }
    std::fs::write(&path, "second").unwrap();
    assert_eq!(f.resolve(None, None).await.unwrap().access_key_id, "WEB-B");
    f.set("AWS_ROLE_SESSION_NAME", "s2");
    f.resolve(None, None).await.unwrap();
    assert_eq!(server.requests().len(), 3);
    assert!(String::from_utf8_lossy(&server.requests()[2].body).contains("RoleSessionName=s2"));
    std::fs::write(&path, vec![b'x'; 1024 * 1024 + 1]).unwrap();
    assert!(
        f.resolve(None, None)
            .await
            .unwrap_err()
            .contains("too large")
    );
}

#[tokio::test]
async fn container_cache_separates_authorization_and_rotating_token_files() {
    let server=MockServer::start(|req,_|MockResponse::json(200,&serde_json::json!({"AccessKeyId":req.header("authorization").unwrap(),"SecretAccessKey":"synthetic","Expiration":"2099-01-01T00:00:00Z"}))).await;
    let mut f = Fixture::new("", "");
    f.set("AWS_CONTAINER_CREDENTIALS_FULL_URI", &server.url);
    f.set("AWS_CONTAINER_AUTHORIZATION_TOKEN", "A");
    assert_eq!(f.resolve(None, None).await.unwrap().access_key_id, "A");
    f.set("AWS_CONTAINER_AUTHORIZATION_TOKEN", "B");
    assert_eq!(f.resolve(None, None).await.unwrap().access_key_id, "B");
    let path = f.dir.path().join("token");
    std::fs::write(&path, "C").unwrap();
    f.set(
        "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
        path.to_str().unwrap(),
    );
    assert_eq!(f.resolve(None, None).await.unwrap().access_key_id, "C");
    std::fs::write(&path, "D").unwrap();
    for _ in 0..2 {
        assert_eq!(f.resolve(None, None).await.unwrap().access_key_id, "D");
    }
    assert_eq!(server.requests().len(), 4);
}

#[tokio::test]
async fn imds_cache_is_scoped_to_metadata_endpoint() {
    let mut servers = vec![];
    for id in ["INSTANCE-A", "INSTANCE-B"] {
        servers.push(MockServer::start(move |req,_| {
            let mut r=MockResponse::json(200,&serde_json::json!({"AccessKeyId":id,"SecretAccessKey":"synthetic","Expiration":"2099-01-01T00:00:00Z"}));
            if req.method=="PUT" {r.chunks=vec![b"metadata-session".to_vec()];}
            else if req.path.ends_with('/') {r.chunks=vec![b"role".to_vec()];}r
        }).await);
    }
    let mut f = Fixture::new("", "");
    f.vars.remove("AWS_EC2_METADATA_DISABLED");
    for i in [0, 1, 0, 1] {
        f.set("AWS_EC2_METADATA_SERVICE_ENDPOINT", &servers[i].url);
        assert_eq!(
            f.resolve(None, None).await.unwrap().access_key_id,
            if i == 0 { "INSTANCE-A" } else { "INSTANCE-B" }
        );
    }
    for s in servers {
        assert_eq!(s.requests().len(), 3);
    }
}

#[tokio::test]
async fn token_and_response_bounds_fail_without_returning_partial_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("token");
    let cancel = CancellationToken::new();
    assert!(
        read_token_file(dir.path().to_str().unwrap(), &cancel)
            .await
            .unwrap_err()
            .contains("regular file")
    );
    std::fs::write(&path, [0xff]).unwrap();
    assert!(
        read_token_file(path.to_str().unwrap(), &cancel)
            .await
            .unwrap_err()
            .contains("UTF-8")
    );
    cancel.cancel();
    assert!(
        read_token_file(path.to_str().unwrap(), &cancel)
            .await
            .unwrap_err()
            .contains("aborted")
    );
    let server = MockServer::start(|_, _| {
        let mut response = MockResponse::json(200, &serde_json::json!({}));
        response.chunks = vec![vec![b'x'; 1024 * 1024 + 1]];
        response
    })
    .await;
    let http = crate::provider::http::client();
    assert!(
        send_with_cancel(http.get(&server.url), &CancellationToken::new())
            .await
            .unwrap_err()
            .contains("too large")
    );
}
