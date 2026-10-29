use super::*;
use ensemble_mcp::{config::Membership, identity};

const MEMBER: &str = "/1/account/1480441/user/2002";

fn member() -> Value {
    json!({"id":2002,"email":"member@team-ensemble.ch","status":"valid","type":"normal","invalid_email":false,"is_blocked":false,"no_access":true,"workspace_only":true})
}

fn setup(h: &mut Harness) -> i64 {
    let mut config = (*h.app.config).clone();
    config.membership = Some(Membership {
        account_id: 1480441,
        reader_id: 9001,
    });
    h.app.config = Arc::new(config);
    let reader = h
        .app
        .store
        .admit(
            &h.app.config,
            ISSUER,
            "9001",
            "reader@team-ensemble.ch",
            "Directory reader",
        )
        .unwrap();
    h.app
        .store
        .with(|c| {
            c.execute("UPDATE users SET admin=1 WHERE id=?", [reader.id])?;
            c.execute("UPDATE modules SET default_level=2", [])?;
            Ok(())
        })
        .unwrap();
    identity::save_credential(
        &h.app,
        reader.id,
        modules::INFOMANIAK,
        &Credential {
            access_token: "directory-reader-token".into(),
            refresh_token: None,
        },
        "*",
        now() + 3600,
    )
    .unwrap();
    h.mock.lock().unwrap().responses.insert(
        format!("GET {MEMBER}"),
        json!({"result":"success","data":member()}),
    );
    reader.id
}

fn claims() -> Value {
    json!({"sub":"2002","email":"member@team-ensemble.ch","name":"Member","email_verified":false})
}

#[tokio::test]
async fn member_can_sign_in_and_save_only_their_own_personal_key() {
    let mut h = Harness::new().await;
    let reader = setup(&mut h);
    let callback = login_callback(&h, claims()).await;
    assert_eq!(callback.status, 303, "{}", callback.value);
    let cookie = cookies(&callback.headers);
    let me = request(
        &h.app,
        "GET",
        "/api/me",
        Value::Null,
        &[("cookie", cookie.clone())],
    )
    .await;
    assert_eq!(me.status, 200);
    assert_eq!(me.value["user"]["subject"], "2002");
    assert_eq!(me.value["user"]["admin"], false);
    assert_eq!(me.value["connections"], json!([]));
    assert!(
        me.value["modules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["level"] == 2)
    );
    let headers = [
        ("cookie", cookie),
        ("origin", h.app.config.public_url.clone()),
        ("x-csrf-token", me.value["csrf"].as_str().unwrap().into()),
    ];
    h.mock.lock().unwrap().responses.insert(
        "GET /2/profile".into(),
        json!({"data":{"id":9001,"email":"member@team-ensemble.ch"}}),
    );
    let wrong = request(
        &h.app,
        "POST",
        "/api/token",
        json!({"token":"wrong-personal-key"}),
        &headers,
    )
    .await;
    assert_eq!(wrong.status, 403);
    assert_eq!(wrong.value["error"], "token_owner_mismatch");
    h.mock.lock().unwrap().responses.insert(
        "GET /2/profile".into(),
        json!({"data":{"id":2002,"email":"member@team-ensemble.ch"}}),
    );
    let saved = request(
        &h.app,
        "POST",
        "/api/token",
        json!({"token":"member-personal-key"}),
        &headers,
    )
    .await;
    assert_eq!(saved.status, 200, "{}", saved.value);
    let user_id = me.value["user"]["id"].as_i64().unwrap();
    h.app
        .store
        .with(|c| {
            let blob: Vec<u8> = c.query_row(
                "SELECT secret FROM connections WHERE user_id=? AND provider='infomaniak'",
                [user_id],
                |r| r.get(0),
            )?;
            let key: Credential = serde_json::from_slice(&crypto::open(
                &h.app.config.encryption_key,
                &identity::credential_context(user_id, modules::INFOMANIAK),
                &blob,
            )?)?;
            assert_eq!(key.access_token, "member-personal-key");
            let reader_blob: Vec<u8> = c.query_row(
                "SELECT secret FROM connections WHERE user_id=? AND provider='infomaniak'",
                [reader],
                |r| r.get(0),
            )?;
            let reader_key: Credential = serde_json::from_slice(&crypto::open(
                &h.app.config.encryption_key,
                &identity::credential_context(reader, modules::INFOMANIAK),
                &reader_blob,
            )?)?;
            assert_eq!(reader_key.access_token, "directory-reader-token");
            Ok(())
        })
        .unwrap();
    let mock = h.mock.lock().unwrap();
    assert!(mock.calls.iter().any(|call|call.1==format!("{MEMBER}?")&&call.2=="Bearer directory-reader-token"));
    assert!(
        mock.calls
            .iter()
            .any(|call| call.1 == "/2/profile?" && call.2 == "Bearer member-personal-key")
    );
    assert!(
        !format!("{}{}{}", callback.value, me.value, saved.value)
            .contains("directory-reader-token")
    );
}

#[tokio::test]
async fn membership_requires_matching_identity_and_explicit_valid_member_fields() {
    let mut malformed = member();
    malformed.as_object_mut().unwrap().remove("is_blocked");
    for data in [
        json!(null),
        json!([]),
        malformed,
        {
            let mut v = member();
            v["id"] = json!(2003);
            v
        },
        {
            let mut v = member();
            v["account_id"] = json!(2);
            v
        },
        {
            let mut v = member();
            v["email"] = json!("other@team-ensemble.ch");
            v
        },
        {
            let mut v = member();
            v["status"] = json!("pending");
            v
        },
        {
            let mut v = member();
            v["type"] = json!("external");
            v
        },
        {
            let mut v = member();
            v["is_blocked"] = json!(true);
            v
        },
        {
            let mut v = member();
            v["invalid_email"] = json!(true);
            v
        },
    ] {
        let mut h = Harness::new().await;
        setup(&mut h);
        h.mock
            .lock()
            .unwrap()
            .responses
            .insert(format!("GET {MEMBER}"), json!({"data":data}));
        let callback = login_callback(&h, claims()).await;
        assert_eq!(callback.status, 401, "{}", callback.value);
        assert!(!cookies(&callback.headers).contains("__Host-mcp_session="));
        let count = h
            .app
            .store
            .with(|c| {
                Ok(
                    c.query_row("SELECT COUNT(*) FROM users WHERE subject='2002'", [], |r| {
                        r.get::<_, i64>(0)
                    })?,
                )
            })
            .unwrap();
        assert_eq!(count, 0);
    }
}

#[tokio::test]
async fn membership_never_bypasses_oidc_validation_or_unsafe_subject_checks() {
    for (key, value) in [
        ("nonce", json!("forged")),
        ("aud", json!("other-client")),
        ("iss", json!("https://evil.example")),
        ("sub", json!("../2002")),
        ("sub", json!("02002")),
        ("exp", json!(now() - 3600)),
    ] {
        let mut h = Harness::new().await;
        setup(&mut h);
        let mut input = claims();
        input[key] = value;
        let callback = login_callback(&h, input).await;
        assert_eq!(callback.status, 401);
        assert!(!cookies(&callback.headers).contains("__Host-mcp_session="));
        assert!(
            !h.mock
                .lock()
                .unwrap()
                .calls
                .iter()
                .any(|call| call.1.contains("/account/"))
        );
    }
}

#[tokio::test]
async fn membership_reader_failures_and_provider_outages_fail_closed() {
    for failure in [
        "missing",
        "not_admin",
        "suspended",
        "expired",
        "reader_domain",
        "401",
        "403",
        "404",
        "503",
    ] {
        let mut h = Harness::new().await;
        let reader = setup(&mut h);
        match failure {
            "missing" => h.sql(&format!("DELETE FROM connections WHERE user_id={reader}")),
            "not_admin" => h.sql(&format!("UPDATE users SET admin=0 WHERE id={reader}")),
            "suspended" => h.sql(&format!(
                "UPDATE users SET status='suspended' WHERE id={reader}"
            )),
            "expired" => h.sql(&format!(
                "UPDATE connections SET expires=0 WHERE user_id={reader}"
            )),
            "reader_domain" => h.sql(&format!(
                "UPDATE users SET email='reader@outside.example' WHERE id={reader}"
            )),
            code => {
                h.mock.lock().unwrap().response_statuses.insert(
                    format!("GET {MEMBER}"),
                    StatusCode::from_u16(code.parse().unwrap()).unwrap(),
                );
            }
        }
        let callback = login_callback(&h, claims()).await;
        assert_eq!(
            callback.status,
            if failure == "404" { 401 } else { 503 },
            "{failure}: {}",
            callback.value
        );
        assert!(!cookies(&callback.headers).contains("__Host-mcp_session="));
    }
}

#[tokio::test]
async fn membership_rechecks_the_exact_reader_credential_after_the_lookup() {
    for failure in ["disconnect", "replace", "suspend"] {
        let mut h = Harness::new().await;
        let reader = setup(&mut h);
        let start = request(&h.app, "GET", "/auth/login", Value::Null, &[]).await;
        let url = url::Url::parse(start.headers["location"].to_str().unwrap()).unwrap();
        let query = url.query_pairs().into_owned().collect::<HashMap<_, _>>();
        let started = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        {
            let mut mock = h.mock.lock().unwrap();
            mock.nonce = query["nonce"].clone();
            mock.claims = claims();
            mock.request_gate = Some((format!("GET {MEMBER}"), started.clone(), resume.clone()));
        }
        let app = h.app.clone();
        let cookie = cookies(&start.headers);
        let path = format!(
            "/auth/infomaniak/callback?state={}&code=provider-code",
            query["state"]
        );
        let operation = tokio::spawn(async move {
            request(&app, "GET", &path, Value::Null, &[("cookie", cookie)]).await
        });
        tokio::time::timeout(std::time::Duration::from_secs(3), started.notified())
            .await
            .unwrap();
        match failure {
            "disconnect" => h.sql(&format!("DELETE FROM connections WHERE user_id={reader}")),
            "suspend" => h.sql(&format!(
                "UPDATE users SET status='suspended' WHERE id={reader}"
            )),
            _ => identity::save_credential(
                &h.app,
                reader,
                modules::INFOMANIAK,
                &Credential {
                    access_token: "replacement-key".into(),
                    refresh_token: None,
                },
                "*",
                now() + 3600,
            )
            .unwrap(),
        }
        resume.notify_one();
        let callback = operation.await.unwrap();
        assert_eq!(callback.status, 503);
        assert!(!cookies(&callback.headers).contains("__Host-mcp_session="));
    }
}

#[tokio::test]
async fn membership_admission_keeps_local_suspension_and_manual_approval() {
    for suspended in [false, true] {
        let mut h = Harness::new().await;
        setup(&mut h);
        if suspended {
            let u = h
                .app
                .store
                .admit(
                    &h.app.config,
                    ISSUER,
                    "2002",
                    "member@team-ensemble.ch",
                    "Member",
                )
                .unwrap();
            h.sql(&format!(
                "UPDATE users SET status='suspended' WHERE id={}",
                u.id
            ));
        } else {
            h.sql("UPDATE settings SET manual_approval=1");
        }
        let callback = login_callback(&h, claims()).await;
        assert_eq!(callback.status, if suspended { 403 } else { 303 });
        if !suspended {
            let cookie = cookies(&callback.headers);
            let me = request(
                &h.app,
                "GET",
                "/api/me",
                Value::Null,
                &[("cookie", cookie.clone())],
            )
            .await;
            assert_eq!(me.value["user"]["status"], "pending");
            let save = request(
                &h.app,
                "POST",
                "/api/token",
                json!({"token":"key"}),
                &[
                    ("cookie", cookie),
                    ("origin", h.app.config.public_url.clone()),
                    ("x-csrf-token", me.value["csrf"].as_str().unwrap().into()),
                ],
            )
            .await;
            assert_eq!(save.status, 403);
        }
    }
}

#[tokio::test]
async fn member_identity_renewal_updates_domain_policy_without_switching_subjects() {
    for foreign_subject in [false, true] {
        let mut h = Harness::new().await;
        setup(&mut h);
        h.sql("UPDATE users SET subject='2002' WHERE subject='employee-a'");
        h.user = h.app.store.user(h.user.id).unwrap();
        credential(&h.app, h.user.id, "expired", now() - 1);
        let mut input = claims();
        input["email"] = json!("member@outside.example");
        if foreign_subject {
            input["sub"] = json!("2003");
        }
        h.mock.lock().unwrap().claims = input;
        let mut record = member();
        record["email"] = json!("member@outside.example");
        h.mock
            .lock()
            .unwrap()
            .responses
            .insert(format!("GET {MEMBER}"), json!({"data":record}));
        let denied = h
            .rpc(
                "tools/call",
                json!({"name":"kdrive_info","arguments":{"drive_id":9,"file_id":42}}),
            )
            .await;
        assert_eq!(denied.status, 403, "{}", denied.value);
        let user = h.app.store.user(h.user.id).unwrap();
        assert_eq!(
            user.email,
            if foreign_subject {
                "alice@team-ensemble.ch"
            } else {
                "member@outside.example"
            }
        );
        if foreign_subject {
            assert!(
                !h.mock
                    .lock()
                    .unwrap()
                    .calls
                    .iter()
                    .any(|call| call.1.contains("/account/"))
            );
        } else {
            assert_eq!(
                request(&h.app, "GET", "/api/me", Value::Null, &h.headers())
                    .await
                    .status,
                403
            );
            assert_eq!(h.rpc("tools/list", json!({})).await.status, 403);
        }
        assert!(
            !h.mock
                .lock()
                .unwrap()
                .calls
                .iter()
                .any(|call| call.1.contains("/files/"))
        );
    }
}
