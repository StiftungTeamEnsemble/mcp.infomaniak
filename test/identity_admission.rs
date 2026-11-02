use super::*;
use ensemble_mcp::identity;

fn reported_identity() -> Value {
    json!({"sub":"2002","email":"member@team-ensemble.ch","name":"Member","email_verified":false})
}

#[tokio::test]
async fn reported_email_allows_sign_in_and_own_key_setup_without_administrator_credentials() {
    let h = Harness::new().await;
    h.sql("DELETE FROM connections; UPDATE modules SET default_level=2");
    let callback = login_callback(&h, reported_identity()).await;
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
            let count: i64 = c.query_row("SELECT COUNT(*) FROM connections", [], |r| r.get(0))?;
            assert_eq!(count, 1);
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
            Ok(())
        })
        .unwrap();
    let mock = h.mock.lock().unwrap();
    assert!(!mock.calls.iter().any(|call| call.1.contains("/account/")));
    assert!(
        mock.calls
            .iter()
            .any(|call| call.1 == "/2/profile?" && call.2 == "Bearer member-personal-key")
    );
    assert!(
        !format!("{}{}{}", callback.value, me.value, saved.value).contains("member-personal-key")
    );
}

#[tokio::test]
async fn reported_email_is_admitted_independently_of_the_verification_flag() {
    for flag in [
        Some(json!(true)),
        Some(json!(false)),
        Some(json!(null)),
        None,
    ] {
        let h = Harness::new().await;
        h.sql("DELETE FROM connections");
        let start = request(&h.app, "GET", "/auth/login", Value::Null, &[]).await;
        let url = url::Url::parse(start.headers["location"].to_str().unwrap()).unwrap();
        let query = url.query_pairs().into_owned().collect::<HashMap<_, _>>();
        let mut claims = json!({"iss":ISSUER,"sub":"2002","aud":"provider-client","iat":now(),"exp":now()+3600,"email":"member@team-ensemble.ch","nonce":query["nonce"]});
        if let Some(flag) = flag {
            claims["email_verified"] = flag;
        }
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some("test-key".into());
        let token = jsonwebtoken::encode(
            &header,
            &claims,
            &jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!("fixtures/oidc-private.pem"))
                .unwrap(),
        )
        .unwrap();
        h.mock
            .lock()
            .unwrap()
            .responses
            .insert("POST /token".into(), json!({"id_token":token}));
        let callback = request(
            &h.app,
            "GET",
            &format!(
                "/auth/infomaniak/callback?state={}&code=provider-code",
                query["state"]
            ),
            Value::Null,
            &[("cookie", cookies(&start.headers))],
        )
        .await;
        assert_eq!(callback.status, 303, "{}", callback.value);
        let mock = h.mock.lock().unwrap();
        assert!(
            mock.calls
                .iter()
                .all(|call| call.1.starts_with("/token?") || call.1.starts_with("/jwks?"))
        );
    }
}

#[tokio::test]
async fn reported_email_admission_keeps_local_suspension_and_manual_approval() {
    for suspended in [false, true] {
        let h = Harness::new().await;
        h.sql("DELETE FROM connections");
        if suspended {
            let user = h
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
                user.id
            ));
        } else {
            h.sql("UPDATE settings SET manual_approval=1");
        }
        let callback = login_callback(&h, reported_identity()).await;
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
