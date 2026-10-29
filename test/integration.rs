use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
};
use ensemble_mcp::{
    App,
    config::Config,
    crypto,
    identity::{Credential, ISSUER, Metadata, Provider},
    modules, oauth,
    store::{Store, User, now},
    web,
};
use rusqlite::params;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};
use tower::ServiceExt;

#[path = "mcp_apps.rs"]
mod mcp_app_tests;
#[path = "membership.rs"]
mod membership_tests;

#[derive(Default)]
struct Mock {
    calls: Vec<(String, String, String, Value)>,
    nonce: String,
    claims: Value,
    fail: bool,
    large: bool,
    recurring: bool,
    refreshes: usize,
    responses: HashMap<String, Value>,
    response_statuses: HashMap<String, StatusCode>,
    queued_responses: HashMap<String, VecDeque<(StatusCode, Value)>>,
    refresh_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    request_gate: Option<(String, Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
}
type MockState = Arc<Mutex<Mock>>;
async fn upstream(State(state): State<MockState>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let query = request.uri().query().unwrap_or("").to_owned();
    let method = request.method().to_string();
    let auth = request
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let bytes = to_bytes(request.into_body(), 12 * 1024 * 1024)
        .await
        .unwrap();
    let body = serde_json::from_slice::<Value>(&bytes)
        .unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)));
    let gate = {
        let mut mock = state.lock().unwrap();
        mock.calls.push((
            method.clone(),
            format!("{path}?{query}"),
            auth.clone(),
            body.clone(),
        ));
        if let Some((target, started, resume)) = &mock.request_gate
            && *target == format!("{method} {path}")
        {
            Some((started.clone(), resume.clone()))
        } else if path == "/token" && body.as_str().unwrap_or("").contains("refresh_token") {
            mock.refresh_gate.clone()
        } else {
            None
        }
    };
    if let Some((started, resume)) = gate {
        started.notify_one();
        resume.notified().await;
    }
    let mut mock = state.lock().unwrap();
    if path == "/jwks" {
        return Json(serde_json::from_str::<Value>(include_str!("fixtures/jwks.json")).unwrap())
            .into_response();
    }
    if mock.fail {
        return (StatusCode::BAD_GATEWAY, "sensitive upstream error body").into_response();
    }
    if let Some((status, value)) = mock
        .queued_responses
        .get_mut(&format!("{method} {path}"))
        .and_then(VecDeque::pop_front)
    {
        return (status, Json(value)).into_response();
    }
    if let Some(status) = mock.response_statuses.get(&format!("{method} {path}")) {
        return (*status, "sensitive upstream error body").into_response();
    }
    if let Some(response) = mock
        .responses
        .get(&format!("{method} {path}?{query}"))
        .or_else(|| mock.responses.get(&format!("{method} {path}")))
    {
        return Json(response.clone()).into_response();
    }
    if path == "/token" {
        if body.as_str().unwrap_or("").contains("refresh_token") {
            mock.refreshes += 1;
        }
        let mut claims = json!({"iss":ISSUER,"sub":"employee-a","aud":"provider-client","iat":now(),"exp":now()+3600,"email":"alice@team-ensemble.ch","email_verified":true,"name":"Alice","nonce":mock.nonce});
        if let Some(overrides) = mock.claims.as_object() {
            for (k, v) in overrides {
                claims[k] = v.clone();
            }
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
        return Json(json!({"access_token":"delegated-a","refresh_token":"refresh-a","token_type":"Bearer","expires_in":3600,"scope":"openid profile email user_info drive workspace:mail workspace:calendar","id_token":token})).into_response();
    }
    if let Some(kchat) = path.strip_prefix("/kchat/ensemble/api/v4") {
        let post = |id: &str, root: &str, at: i64| json!({"id":id,"channel_id":"chan1","root_id":root,"user_id":"me1","message":format!("Message {id}"),"create_at":at});
        let value = match (method.as_str(), kchat) {
            ("GET", "/teams/name/ensemble") => json!({"id":"team1","name":"ensemble"}),
            ("GET", "/teams/team1/channels") => {
                json!([{"id":"chan1","team_id":"team1","type":"O","name":"town-square","display_name":"Town Square"}])
            }
            ("GET", "/users/me/teams/team1/channels") => {
                json!([{"id":"chan1","team_id":"team1","type":"O","name":"town-square"},{"id":"dm1","team_id":"","type":"D","name":"bob1__me1"}])
            }
            ("GET", "/users/me") => json!({"id":"me1","username":"alice"}),
            ("GET", "/users") => {
                json!([{"id":"me1","username":"alice"},{"id":"bob1","username":"bob"}])
            }
            ("GET", "/users/bob1") | ("GET", "/users/username/bob") => {
                json!({"id":"bob1","username":"bob"})
            }
            ("GET", "/channels/chan1/posts") => {
                json!({"order":["post2","post1"],"posts":{"post1":post("post1","",1),"post2":post("post2","",2)}})
            }
            ("GET", "/posts/post1/thread") => {
                json!({"order":["post3","post1"],"posts":{"post1":post("post1","",1),"post3":post("post3","post1",3)}})
            }
            ("POST", "/channels/direct") => json!({"id":"dm1","type":"D"}),
            ("POST", "/posts") => {
                json!({"id":"01a1624d-82b0-704f-960b-4456caa5f15e","channel_id":body["channel_id"],"root_id":body.get("root_id").cloned().unwrap_or(json!("")),"user_id":"me1","message":body["message"]})
            }
            ("POST", "/reactions") => body.clone(),
            _ => return StatusCode::NOT_FOUND.into_response(),
        };
        return Json(value).into_response();
    }
    if path.ends_with("/download") || path.contains("/attachment/") {
        let bytes = if mock.large {
            vec![0; ensemble_mcp::identity::MAX_BYTES + 1]
        } else {
            b"file bytes".to_vec()
        };
        return (
            [
                ("content-type", "text/plain"),
                ("content-disposition", "attachment; filename=\"report.txt\""),
            ],
            bytes,
        )
            .into_response();
    }
    let data = if path == "/mailbox" {
        json!([{"uuid":"box-a","email":"alice@team-ensemble.ch","is_primary":true,"aliases":["alias@team-ensemble.ch"],"hosting_id":5,"mailbox":"alice"}])
    } else if path.ends_with("/signatures") {
        json!({"signatures":[{"id":"sig-a","sender":"alice@team-ensemble.ch","full_name":"Alice","is_default":true}]})
    } else if path == "/2/profile" {
        json!({"email":"alice@team-ensemble.ch","display_name":"Alice","preferences":{"timezone":{"name":"Europe/Zurich"}}})
    } else if path == "/1/calendar/pim/calendar" {
        json!({"calendars":[{"id":"primary","name":"Personal","default":true}]})
    } else if path == "/1/calendar/pim/event/7" && method == "GET" {
        json!({"id":7,"title":"Existing","calendar_id":"actual-calendar","start":"2026-10-17 10:00:00","end":"2026-10-17 11:00:00","timezone_start":"Europe/Zurich","timezone_end":"Europe/Zurich","rrule":if mock.recurring{"FREQ=WEEKLY"}else{""},"attendees":[{"address":"person@example.org"}],"alarms":[{"id":1}],"attachments":[{"id":2}],"bookable_resource_id":"room-1","meet_room_url":"https://meet.infomaniak.com/room"})
    } else if path == "/1/calendar/pim/bookable-resources/all" {
        json!([{"uuid":"room-1","name":"Studio"}])
    } else if path == "/1/calendar/pim/event" && method == "GET" {
        json!([])
    } else if path.starts_with("/1/calendar/pim/event") && method != "GET" {
        if body.is_object() {
            let mut event = body.clone();
            if method == "POST" {
                event["id"] = json!(8);
            }
            event
        } else {
            json!({"deleted":true})
        }
    } else if path.ends_with("/folder") && method == "GET" {
        json!([{"id":"inbox","name":"Inbox","role":"INBOX","children":[]},{"id":"draft","name":"Drafts","role":"DRAFT","children":[]},{"id":"trash","name":"Trash","role":"TRASH","children":[]}])
    } else if path.ends_with("/message") && method == "GET" {
        json!({"threads":[{"uid":"thread-1","subject":"A message","unseen_messages":1,"messages":[{"uid":"12@inbox","folder_id":"inbox","preview":"Sensitive mail body"}]}]})
    } else if path.ends_with("/draft/draft-1") && method == "GET" {
        json!({"uuid":"draft-1","uid":"1","subject":"Keep me","body":"Original body","to":[{"email":"old@example.org"}],"cc":[],"attachments":["existing"],"identity_id":"sig-a"})
    } else if path.ends_with("/draft") && method == "POST" {
        json!({"uuid":"draft-1","uid":"1"})
    } else if path.ends_with("/draft/attachment") {
        json!({"uuid":"attachment-a"})
    } else if path == "/3/drive/9/upload" {
        json!({"id":43,"name":"uploaded.txt","type":"file","mime_type":"text/plain","parent_id":1,"extension_type":"text"})
    } else if path.ends_with("/directory") && method == "POST" {
        json!({"id":44,"name":body["name"],"type":"dir"})
    } else if path.ends_with("/files") && method == "GET" {
        json!([{"id":42,"name":if auth.contains("delegated-b"){"Bob's file"}else{"Alice's file"},"mime_type":"text/plain","type":"file","size":10}])
    } else if path.contains("/files/42") && method == "GET" {
        json!({"id":42,"name":"report.txt","mime_type":"text/plain","type":"file","size":10})
    } else if body.is_object() {
        body
    } else {
        json!({"ok":true})
    };
    Json(json!({"result":"success","data":data,"has_more":false,"cursor":null})).into_response()
}
struct Harness {
    app: App,
    mock: MockState,
    user: User,
    token: String,
    session: String,
    csrf: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Harness {
    async fn new() -> Self {
        let mock = Arc::new(Mutex::new(Mock::default()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .fallback(any(upstream))
            .with_state(mock.clone());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let config = cfg();
        let mut provider = Provider::with_endpoints(
            Metadata {
                issuer: ISSUER.into(),
                authorization_endpoint: format!("{base}/authorize"),
                token_endpoint: format!("{base}/token"),
                jwks_uri: format!("{base}/jwks"),
                code_challenge_methods_supported: vec!["S256".into()],
            },
            Provider::http_client().unwrap(),
            base.clone(),
            base.clone(),
        );
        provider.kchat = format!("{base}/kchat/{{team}}");
        let app = App::new(config, Store::open(":memory:").unwrap(), provider).unwrap();
        let user = app
            .store
            .admit(
                &app.config,
                ISSUER,
                "employee-a",
                "alice@team-ensemble.ch",
                "Alice",
            )
            .unwrap();
        let token = "mcp-access".to_owned();
        let session = "browser-session".to_owned();
        let csrf = "csrf-value".to_owned();
        app.store.with(|c|{
            c.execute("INSERT INTO clients(id,name,redirects,created) VALUES('client-a','Test client','[\"https://client.example/callback\"]',?)",[now()])?;
            c.execute("INSERT INTO grants(id,user_id,client_id,scopes) VALUES('grant-a',?,'client-a',?)",params![user.id,oauth::scopes(&app).join(" ")])?;
            c.execute("INSERT INTO tokens(hash,grant_id,kind,resource,expires,family) VALUES(?,'grant-a','access',?,?,'family-a')",params![crypto::hash(&token),app.config.resource(),now()+3600])?;
            c.execute("INSERT INTO sessions(hash,user_id,csrf,expires) VALUES(?,?,?,?)",params![crypto::hash(&session),user.id,csrf,now()+3600])?;
            c.execute("UPDATE modules SET enabled=1,verification='Mock verification'",[])?;
            c.execute("UPDATE policies SET level=2",[])?;Ok(())
        }).unwrap();
        credential(&app, user.id, "delegated-a", now() + 3600);
        Self {
            app,
            mock,
            user,
            token,
            session,
            csrf,
            task,
        }
    }
    async fn rpc(&self, method: &str, params: Value) -> Reply {
        request(
            &self.app,
            "POST",
            "/mcp",
            json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}),
            &[
                ("authorization", format!("Bearer {}", self.token)),
                ("accept", "application/json, text/event-stream".into()),
            ],
        )
        .await
    }
    async fn call(&self, name: &str, args: Value) -> Value {
        self.rpc("tools/call", json!({"name":name,"arguments":args}))
            .await
            .value
    }
    async fn deadline(&mut self, name: &str, args: Value, target: &str) -> Reply {
        // Keep the provider request pending beyond the overall MCP deadline.
        let mut provider = Provider::with_endpoints(
            self.app.provider.metadata.clone(),
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(200))
                .build()
                .unwrap(),
            self.app.provider.api.clone(),
            self.app.provider.mail.clone(),
        );
        provider.kchat = self.app.provider.kchat.clone();
        self.app.provider = Arc::new(provider);
        let started = Arc::new(tokio::sync::Notify::new());
        self.mock.lock().unwrap().request_gate = Some((
            target.into(),
            started.clone(),
            Arc::new(tokio::sync::Notify::new()),
        ));
        let app = self.app.clone();
        let bearer = self.token.clone();
        let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}});
        let operation = tokio::spawn(async move {
            request(
                &app,
                "POST",
                "/mcp",
                body,
                &[
                    ("authorization", format!("Bearer {bearer}")),
                    ("accept", "application/json, text/event-stream".into()),
                ],
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(3), started.notified())
            .await
            .unwrap();
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(91)).await;
        let reply = operation.await.unwrap();
        tokio::time::resume();
        reply
    }
    fn headers(&self) -> Vec<(&'static str, String)> {
        vec![
            ("cookie", format!("__Host-mcp_session={}", self.session)),
            ("origin", self.app.config.public_url.clone()),
            ("x-csrf-token", self.csrf.clone()),
        ]
    }
    async fn admin(&self, change: Value) -> Reply {
        request(&self.app, "POST", "/api/admin", change, &self.headers()).await
    }
    fn sql(&self, sql: &str) {
        self.app
            .store
            .with(|c| {
                c.execute_batch(sql)?;
                Ok(())
            })
            .unwrap();
    }
}
fn cfg() -> Config {
    Config {
        public_url: "https://mcp.example.org".into(),
        sqlite_path: ":memory:".into(),
        encryption_key: [7; 32],
        client_id: "provider-client".into(),
        client_secret: "provider-secret".into(),
        allowed_domains: ["team-ensemble.ch".into()].into_iter().collect(),
        bind_addr: "127.0.0.1:3000".into(),
        kchat_team: Some("ensemble".into()),
        membership: None,
    }
}
fn credential(app: &App, user: i64, token: &str, expires: i64) {
    let secret = crypto::seal(
        &app.config.encryption_key,
        &ensemble_mcp::identity::credential_context(user, modules::INFOMANIAK),
        &serde_json::to_vec(&Credential {
            access_token: token.into(),
            refresh_token: Some("refresh-token".into()),
        })
        .unwrap(),
    )
    .unwrap();
    let mut scopes: Vec<&str> = app
        .modules
        .modules
        .iter()
        .flat_map(|m| m.scopes.clone())
        .collect();
    scopes.dedup();
    app.store
        .with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO connections(user_id,provider,secret,scopes,expires) VALUES(?,?,?,?,?)",
                params![user, modules::INFOMANIAK, secret, scopes.join(" "), expires],
            )?;
            Ok(())
        })
        .unwrap();
}
struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    value: Value,
}
async fn request(
    app: &App,
    method: &str,
    uri: &str,
    value: Value,
    headers: &[(&str, String)],
) -> Reply {
    let body = if value.is_null() {
        String::new()
    } else {
        value.to_string()
    };
    raw_request(app, method, uri, body, "application/json", headers).await
}
async fn raw_request(
    app: &App,
    method: &str,
    uri: &str,
    body: String,
    content_type: &str,
    headers: &[(&str, String)],
) -> Reply {
    let mut req = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", content_type);
    for (key, value) in headers {
        req = req.header(*key, value);
    }
    let response = web::router(app.clone())
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 20 * 1024 * 1024)
        .await
        .unwrap();
    Reply {
        status,
        headers,
        value: serde_json::from_slice(&bytes).unwrap_or(json!(String::from_utf8_lossy(&bytes))),
    }
}
fn result(v: &Value) -> &Value {
    &v["result"]["structuredContent"]
}

#[test]
fn environment_is_strict_and_domains_are_exact() {
    let values = [
        ("PUBLIC_URL", "https://mcp.example.org"),
        ("SQLITE_PATH", ":memory:"),
        (
            "CREDENTIAL_ENCRYPTION_KEY",
            "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
        ),
        ("INFOMANIAK_CLIENT_ID", "id"),
        ("INFOMANIAK_CLIENT_SECRET", "secret"),
        ("ALLOWED_EMAIL_DOMAINS", "TEAM-ENSEMBLE.CH"),
    ];
    let load = |key: &str| {
        values
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.to_string())
    };
    let config = Config::load(load).unwrap();
    assert!(config.membership.is_none());
    let configured = Config::load(|key| match key {
        "INFOMANIAK_ACCOUNT_ID" => Some("1480441".into()),
        "INFOMANIAK_MEMBERSHIP_READER_ID" => Some("9001".into()),
        _ => load(key),
    })
    .unwrap();
    assert_eq!(configured.membership.as_ref().unwrap().account_id, 1480441);
    for (account, reader) in [
        ("1480441", ""),
        ("", "9001"),
        ("0", "9001"),
        ("1480441", "../9001"),
        ("01", "9001"),
        ("1480441", "9223372036854775808"),
    ] {
        assert!(
            Config::load(|key| match key {
                "INFOMANIAK_ACCOUNT_ID" => Some(account.into()),
                "INFOMANIAK_MEMBERSHIP_READER_ID" => Some(reader.into()),
                _ => load(key),
            })
            .is_err()
        );
    }
    assert!(config.admits("a@TEAM-ENSEMBLE.CH"));
    for email in [
        "a@sub.team-ensemble.ch",
        "a@evilteam-ensemble.ch",
        "a@team-ensemble.ch.evil",
        "a@@team-ensemble.ch",
        "@team-ensemble.ch",
        "a@team-ensemble.ch ",
    ] {
        assert!(!config.admits(email));
    }
    for (key, value) in [
        ("PUBLIC_URL", "http://mcp.example.org"),
        ("PUBLIC_URL", "https://mcp.example.org/path"),
        ("PUBLIC_URL", "https://user:pass@mcp.example.org"),
        ("CREDENTIAL_ENCRYPTION_KEY", "short"),
        ("ALLOWED_EMAIL_DOMAINS", ""),
        ("ALLOWED_EMAIL_DOMAINS", "*.team-ensemble.ch"),
    ] {
        assert!(
            Config::load(|k| if k == key {
                Some(value.into())
            } else {
                load(k)
            })
            .is_err()
        );
    }
}
#[test]
fn credentials_require_the_correct_key_and_owner() {
    let secret = crypto::seal(&[7; 32], "credential:1:mail", b"secret").unwrap();
    assert_eq!(
        crypto::open(&[7; 32], "credential:1:mail", &secret).unwrap(),
        b"secret"
    );
    assert!(crypto::open(&[8; 32], "credential:1:mail", &secret).is_err());
    assert!(crypto::open(&[7; 32], "credential:2:mail", &secret).is_err());
    let mut tampered = secret;
    tampered[15] ^= 1;
    assert!(crypto::open(&[7; 32], "credential:1:mail", &tampered).is_err());
}
#[test]
fn calendar_dates_ignore_server_timezone_and_reject_dst_ambiguity() {
    use modules::calendar::local_time;
    assert_eq!(
        local_time("2026-10-17T08:00:00Z", "Europe/Zurich")
            .unwrap()
            .0,
        "2026-10-17 10:00:00"
    );
    assert_eq!(
        local_time("2026-10-17", "Europe/Zurich").unwrap().0,
        "2026-10-17 00:00:00"
    );
    assert_eq!(
        local_time("2026-10-17T10:00", "Europe/Zurich").unwrap().1,
        local_time("2026-10-17T08:00:00Z", "Europe/Zurich")
            .unwrap()
            .1
    );
    assert!(local_time("2026-03-29T02:30:00", "Europe/Zurich").is_err());
    assert!(local_time("2026-10-25T02:30:00", "Europe/Zurich").is_err());
}
#[test]
fn attachment_filenames_preserve_unicode_spaces_and_punctuation() {
    use modules::mail::attachment_filename;
    assert_eq!(
        attachment_filename(
            "attachment; filename=fallback.txt; filename*=UTF-8''R%C3%A9sum%C3%A9%20%2B%20notes.txt"
        )
        .as_deref(),
        Some("Résumé + notes.txt")
    );
    assert_eq!(
        attachment_filename("attachment; filename=\"notes; draft.txt\"").as_deref(),
        Some("notes; draft.txt")
    );
}
#[test]
fn module_registration_and_inputs_are_explicit() {
    assert!(
        modules::Registry::new(vec![modules::mail::module(), modules::mail::module()]).is_err()
    );
    let registry = modules::Registry::builtin().unwrap();
    assert_eq!(
        registry
            .modules
            .iter()
            .map(|m| m.tools.len())
            .sum::<usize>(),
        55
    );
    let (_, tool) = registry.tool("kdrive_info").unwrap();
    assert!(tool.validate(&json!({"drive_id":9,"file_id":42})).is_ok());
    for args in [
        json!({"drive_id":"9","file_id":42}),
        json!({"drive_id":9,"file_id":0}),
        json!({"drive_id":9,"file_id":42,"user_id":2}),
    ] {
        assert!(tool.validate(&args).is_err());
    }
}
#[tokio::test]
async fn mcp_requires_own_audience_token_and_valid_origin() {
    let h = Harness::new().await;
    let r = request(&h.app, "POST", "/mcp", json!({}), &[]).await;
    assert_eq!(r.status, 401);
    assert!(
        r.headers["www-authenticate"]
            .to_str()
            .unwrap()
            .contains("resource_metadata")
    );
    let r = request(
        &h.app,
        "POST",
        "/mcp",
        json!({}),
        &[("authorization", "Bearer delegated-a".into())],
    )
    .await;
    assert_eq!(r.status, 401);
    let r = request(
        &h.app,
        "GET",
        "/health",
        Value::Null,
        &[("origin", "https://evil.example".into())],
    )
    .await;
    assert_eq!(r.status, 403);
    h.sql("UPDATE tokens SET resource='https://elsewhere.example/mcp'");
    assert_eq!(h.rpc("tools/list", json!({})).await.status, 401);
}
#[tokio::test]
async fn missing_client_scopes_receive_transport_challenges_before_provider_dispatch() {
    let h = Harness::new().await;
    h.sql("UPDATE grants SET scopes='kdrive:read'");
    let denied = h
        .rpc(
            "tools/call",
            json!({"name":"kdrive_create_folder","arguments":{"drive_id":9,"name":"folder"}}),
        )
        .await;
    assert_eq!(denied.status, 403);
    assert_eq!(denied.value["error"], "insufficient_scope");
    let challenge = denied.headers["www-authenticate"].to_str().unwrap();
    assert!(challenge.contains("scope=\"kdrive:write\""));
    assert!(challenge.contains("error=\"insufficient_scope\""));
    assert!(challenge.contains("https://mcp.example.org/.well-known/oauth-protected-resource"));
    h.sql("UPDATE grants SET scopes='mail:read'");
    let denied = h
        .rpc("resources/read", json!({"uri":"kdrive://9/42"}))
        .await;
    assert_eq!(denied.status, 403);
    assert!(
        denied.headers["www-authenticate"]
            .to_str()
            .unwrap()
            .contains("scope=\"kdrive:read\"")
    );
    assert!(h.mock.lock().unwrap().calls.is_empty());
    h.sql("UPDATE modules SET enabled=0 WHERE module='kdrive'");
    let denied = h
        .rpc(
            "tools/call",
            json!({"name":"kdrive_info","arguments":{"drive_id":9,"file_id":42}}),
        )
        .await;
    assert_eq!(denied.status, 403);
    assert_eq!(denied.value["error"], "access_denied");
    assert!(!denied.headers.contains_key("www-authenticate"));
}
#[tokio::test]
async fn resources_recheck_client_and_policy_after_provider_renewal() {
    for (change, status, code) in [
        (
            "DELETE FROM tokens WHERE family='family-a'",
            401,
            "invalid_token",
        ),
        (
            "UPDATE grants SET scopes='mail:read'",
            403,
            "insufficient_scope",
        ),
        (
            "UPDATE modules SET enabled=0 WHERE module='kdrive'",
            403,
            "access_denied",
        ),
        (
            "UPDATE policies SET level=0 WHERE module='kdrive'",
            403,
            "access_denied",
        ),
    ] {
        let h = Harness::new().await;
        credential(&h.app, h.user.id, "expired", now() - 1);
        let started = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        h.mock.lock().unwrap().refresh_gate = Some((started.clone(), resume.clone()));
        let app = h.app.clone();
        let token = h.token.clone();
        let operation = tokio::spawn(async move {
            request(&app, "POST", "/mcp", json!({"jsonrpc":"2.0","id":1,"method":"resources/read","params":{"uri":"kdrive://9/42"}}), &[
                ("authorization",format!("Bearer {token}")),
                ("accept","application/json, text/event-stream".into()),
            ]).await
        });
        tokio::time::timeout(std::time::Duration::from_secs(3), started.notified())
            .await
            .unwrap();
        h.sql(change);
        resume.notify_one();
        let denied = operation.await.unwrap();
        assert_eq!(denied.status, status, "{}", denied.value);
        assert_eq!(denied.value["error"], code);
        if status == 401 || code == "insufficient_scope" {
            assert!(denied.headers.contains_key("www-authenticate"));
        }
        assert!(
            !h.mock
                .lock()
                .unwrap()
                .calls
                .iter()
                .any(|c| c.1.contains("/files/"))
        );
    }
}
#[tokio::test]
async fn tools_filter_and_direct_calls_apply_live_policy() {
    let h = Harness::new().await;
    assert_eq!(
        h.rpc("tools/list", json!({})).await.value["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        55
    );
    h.sql("UPDATE modules SET enabled=0 WHERE module='mail'");
    let v = h.rpc("tools/list", json!({})).await.value;
    assert!(
        !v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"].as_str().unwrap().starts_with("mail_"))
    );
    let v = h
        .call(
            "mail_read_email",
            json!({"folder_id":"inbox","message_id":"12"}),
        )
        .await;
    assert_eq!(v["error"], "access_denied");
    assert_eq!(h.mock.lock().unwrap().calls.len(), 0);
    h.sql("UPDATE users SET status='suspended'");
    assert_eq!(h.rpc("tools/list", json!({})).await.status, 403);
}
#[tokio::test]
async fn consent_and_credential_removal_take_effect_on_next_operation() {
    let h = Harness::new().await;
    h.sql("DELETE FROM connections");
    assert_eq!(
        h.call("kdrive_info", json!({"drive_id":9,"file_id":42}))
            .await["error"],
        "reconnection_required"
    );
    h.sql("UPDATE grants SET revoked=1");
    assert_eq!(h.rpc("tools/list", json!({})).await.status, 401);
}
#[tokio::test]
async fn personal_tokens_must_belong_to_the_user_and_connect_only_granted_services() {
    let h = Harness::new().await;
    h.sql("DELETE FROM connections; UPDATE policies SET level=0 WHERE module='calendar'");
    let headers = h.headers();
    let save = |token: &str| {
        request(
            &h.app,
            "POST",
            "/api/token",
            json!({ "token": token }),
            &headers,
        )
    };
    assert_eq!(save("two words").await.status, 400);
    let profile = "GET /2/profile".to_owned();
    for (status, body, code) in [
        (StatusCode::UNAUTHORIZED, json!({}), "token_rejected"),
        (
            StatusCode::OK,
            json!({"data":{"email":"bob@team-ensemble.ch"}}),
            "token_owner_mismatch",
        ),
        (
            StatusCode::OK,
            json!({"data":{"id":99,"email":"alice@team-ensemble.ch"}}),
            "token_owner_mismatch",
        ),
    ] {
        h.mock
            .lock()
            .unwrap()
            .queued_responses
            .insert(profile.clone(), [(status, body)].into());
        assert_eq!(save("personal-b").await.value["error"], code);
    }
    assert!(
        h.app
            .store
            .with(
                |c| Ok(c.query_row("SELECT COUNT(*) FROM connections", [], |r| r
                    .get::<_, i64>(0))?)
            )
            .unwrap()
            == 0
    );
    let saved = save(" personal-a\n").await;
    assert_eq!(saved.status, 200);
    assert_eq!(saved.value["connected"], json!(["mail", "kdrive", "kchat"]));
    let out = h
        .call("kdrive_info", json!({"drive_id":9,"file_id":42}))
        .await;
    assert!(out["result"].is_object());
    assert_eq!(
        h.mock.lock().unwrap().calls.last().unwrap().2,
        "Bearer personal-a"
    );
    assert_eq!(
        h.call("calendar_list_calendars", json!({})).await["error"],
        "access_denied"
    );
}
#[tokio::test]
async fn users_can_edit_client_grants_within_consent_rules() {
    let h = Harness::new().await;
    let headers = h.headers();
    let edit = |scopes: Value| {
        request(
            &h.app,
            "POST",
            "/api/clients/grant-a/scopes",
            json!({ "scopes": scopes }),
            &headers,
        )
    };
    let tools = || async {
        h.rpc("tools/list", json!({})).await.value["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(edit(json!([])).await.status, 400);
    assert_eq!(edit(json!(["mail:admin"])).await.status, 400);
    assert_eq!(
        request(
            &h.app,
            "POST",
            "/api/clients/grant-a/scopes",
            json!({"scopes":["mail:read"]}),
            &[("cookie", format!("__Host-mcp_session={}", h.session))],
        )
        .await
        .status,
        403
    );
    let saved = edit(json!(["mail:read", "calendar:read", "mail:read"])).await;
    assert_eq!(saved.status, 200);
    assert_eq!(saved.value["scopes"], json!(["mail:read", "calendar:read"]));
    let names = tools().await;
    assert!(names.iter().any(|n| n == "mail_search_emails"));
    assert!(names.iter().any(|n| n == "calendar_list_events"));
    assert!(
        !names
            .iter()
            .any(|n| n == "mail_send_email" || n.starts_with("kdrive_"))
    );
    // Existing access may be kept after its service becomes unavailable, but not widened.
    h.sql("UPDATE modules SET enabled=0 WHERE module='calendar'");
    assert_eq!(
        edit(json!(["mail:read", "calendar:read"])).await.status,
        200
    );
    assert_eq!(
        edit(json!(["mail:read", "calendar:read", "calendar:write"]))
            .await
            .status,
        403
    );
    assert_eq!(edit(json!(["mail:read", "mail:write"])).await.status, 200);
    assert!(tools().await.iter().any(|n| n == "mail_send_email"));
    let me = request(&h.app, "GET", "/api/me", Value::Null, &headers).await;
    assert!(
        me.value["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["scope"] == "calendar:read" && s["available"] == false)
    );
    let bob = h
        .app
        .store
        .admit(
            &h.app.config,
            ISSUER,
            "employee-b",
            "bob@team-ensemble.ch",
            "Bob",
        )
        .unwrap();
    h.sql(&format!(
        "INSERT INTO grants(id,user_id,client_id,scopes) VALUES('grant-b',{},'client-a','mail:read')",
        bob.id
    ));
    let other = request(
        &h.app,
        "POST",
        "/api/clients/grant-b/scopes",
        json!({"scopes":["mail:read"]}),
        &headers,
    )
    .await;
    assert_eq!(other.status, 404);
    h.sql("UPDATE grants SET revoked=1 WHERE id='grant-a'");
    assert_eq!(edit(json!(["mail:read"])).await.status, 404);
}
#[tokio::test]
async fn upstream_reconnection_failures_keep_http_status_for_tools_and_resources() {
    for target in ["GET /3/drive/9/files/42", "POST /token"] {
        for method in ["tools/call", "resources/read", "resources/list"] {
            let h = Harness::new().await;
            h.sql("INSERT INTO resource_refs(user_id,drive_id,file_id,name,mime) VALUES(1,9,42,'report.txt','text/plain')");
            if target == "POST /token" {
                credential(&h.app, h.user.id, "expired", now() - 1);
            }
            {
                let mut mock = h.mock.lock().unwrap();
                mock.response_statuses
                    .insert(target.into(), StatusCode::UNAUTHORIZED);
                // A revoked token fails everywhere, including the profile check.
                mock.response_statuses
                    .insert("GET /2/profile".into(), StatusCode::UNAUTHORIZED);
            }
            let params = match method {
                "tools/call" => {
                    json!({"name":"kdrive_info","arguments":{"drive_id":9,"file_id":42}})
                }
                "resources/read" => json!({"uri":"kdrive://9/42"}),
                _ => json!({}),
            };
            let reply = h.rpc(method, params).await;
            assert_eq!(
                reply.status,
                StatusCode::FORBIDDEN,
                "{method}: {}",
                reply.value
            );
            assert_eq!(reply.value["error"], "reconnection_required");
            assert!(!reply.headers.contains_key("www-authenticate"));
            assert!(!reply.value.to_string().contains("sensitive upstream"));
        }
    }
}
#[tokio::test]
async fn group_permissions_combine_without_admin_content_privileges() {
    let h = Harness::new().await;
    h.sql("UPDATE policies SET level=0; UPDATE users SET admin=1; INSERT INTO groups(id,name) VALUES(1,'Readers'); INSERT INTO memberships VALUES(1,1); INSERT INTO policies VALUES('group',1,'mail',1)");
    let tools = h.rpc("tools/list", json!({})).await.value;
    assert!(
        tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["name"].as_str().unwrap().starts_with("mail_")
                && t["annotations"]["readOnlyHint"] == true)
    );
    assert_eq!(
        h.call(
            "mail_send_email",
            json!({"to":"b@example.org","subject":"hi","body":"secret"})
        )
        .await["error"],
        "access_denied"
    );
}
#[tokio::test]
async fn identities_never_relink_by_email_and_suspension_survives_login() {
    let h = Harness::new().await;
    let other = h
        .app
        .store
        .admit(&h.app.config, ISSUER, "employee-b", &h.user.email, "Bob")
        .unwrap();
    assert_ne!(other.id, h.user.id);
    h.sql("UPDATE users SET status='suspended' WHERE id=1");
    let same = h
        .app
        .store
        .admit(
            &h.app.config,
            ISSUER,
            "employee-a",
            "changed@team-ensemble.ch",
            "Renamed",
        )
        .unwrap();
    assert_eq!(same.id, h.user.id);
    assert_eq!(same.status, "suspended");
}
#[tokio::test]
async fn manual_onboarding_gets_defaults_but_no_access() {
    let h = Harness::new().await;
    h.sql("UPDATE settings SET manual_approval=1;UPDATE modules SET default_level=1");
    let user = h
        .app
        .store
        .admit(&h.app.config, ISSUER, "new", "new@team-ensemble.ch", "New")
        .unwrap();
    assert_eq!(user.status, "pending");
    assert_eq!(h.app.store.local_level(user.id, "mail").unwrap(), 1);
    assert!(h.app.check_user(&user, true).is_err());
}
#[tokio::test]
async fn browser_mutations_require_csrf_and_admin_role() {
    let h = Harness::new().await;
    assert_eq!(
        h.admin(json!({"action":"onboarding","manual":true}))
            .await
            .status,
        403
    );
    h.sql("UPDATE users SET admin=1");
    let r = request(
        &h.app,
        "POST",
        "/api/admin",
        json!({"action":"onboarding","manual":true}),
        &[("cookie", format!("__Host-mcp_session={}", h.session))],
    )
    .await;
    assert_eq!(r.status, 403);
    assert_eq!(
        h.admin(json!({"action":"onboarding","manual":true}))
            .await
            .status,
        200
    );
    assert_eq!(h.admin(json!({"action":"module","module":"mail","enabled":true,"verification":"","default_level":0})).await.status,400);
    assert_eq!(
        h.admin(json!({"action":"user","user_id":1,"status":"suspended","admin":false}))
            .await
            .status,
        400
    );
}
#[tokio::test]
async fn two_users_never_select_each_others_credentials() {
    let h = Harness::new().await;
    let bob = h
        .app
        .store
        .admit(
            &h.app.config,
            ISSUER,
            "employee-b",
            "bob@team-ensemble.ch",
            "Bob",
        )
        .unwrap();
    credential(&h.app, bob.id, "delegated-b", now() + 3600);
    let out = h.call("kdrive_list", json!({"drive_id":9})).await;
    assert_eq!(result(&out)["data"][0]["name"], "Alice's file");
    assert!(
        h.mock
            .lock()
            .unwrap()
            .calls
            .iter()
            .all(|c| c.2 == "Bearer delegated-a")
    );
    assert!(
        h.call("kdrive_list", json!({"drive_id":9,"user_id":bob.id}))
            .await["error"]
            .is_object()
    );
}
#[tokio::test]
async fn binary_resources_and_discovered_files_are_user_scoped() {
    let h = Harness::new().await;
    h.call("kdrive_list", json!({"drive_id":9})).await;
    let refs = h.rpc("resources/list", json!({})).await.value;
    assert!(
        refs["result"]["resources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["uri"] == "kdrive://9/42")
    );
    let out = h
        .rpc("resources/read", json!({"uri":"kdrive://9/42"}))
        .await
        .value;
    assert_eq!(out["result"]["contents"][0]["blob"], "ZmlsZSBieXRlcw==");
    assert!(
        h.rpc("resources/read", json!({"uri":"file:///etc/passwd"}))
            .await
            .value["error"]
            .is_object()
    );
    h.sql("UPDATE policies SET level=0 WHERE module='kdrive'");
    assert_eq!(
        h.rpc("resources/read", json!({"uri":"kdrive://9/42"}))
            .await
            .status,
        403
    );
}
#[tokio::test]
async fn kdrive_metadata_must_match_the_requested_file_and_drive_before_download() {
    for item in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"id":0}),
        json!({"id":-1}),
        json!({"id":"42"}),
        json!({"id":43}),
        json!({"id":9007199254740992_i64}),
        json!({"id":42,"drive_id":10}),
        json!({"id":42,"drive_id":"9"}),
    ] {
        let h = Harness::new().await;
        h.call("kdrive_list", json!({"drive_id":9})).await;
        h.mock
            .lock()
            .unwrap()
            .responses
            .insert("GET /3/drive/9/files/42".into(), json!({"data":item}));
        for name in ["kdrive_info", "kdrive_download_file"] {
            let out = h.call(name, json!({"drive_id":9,"file_id":42})).await;
            assert_eq!(
                out["result"]["_meta"]["error_code"], "provider_error",
                "{name}: {out}"
            );
        }
        for (method, args) in [
            ("resources/list", json!({})),
            ("resources/read", json!({"uri":"kdrive://9/42"})),
        ] {
            let out = h.rpc(method, args).await;
            assert_eq!(out.status, 200);
            assert!(
                out.value["error"]["message"]
                    .as_str()
                    .unwrap()
                    .starts_with("provider_error:"),
                "{}",
                out.value
            );
        }
        assert!(
            !h.mock
                .lock()
                .unwrap()
                .calls
                .iter()
                .any(|c| c.1.contains("/download"))
        );
    }
}

#[tokio::test]
async fn malformed_kdrive_pages_never_poison_resource_discovery() {
    for (name, path, args) in [
        (
            "kdrive_list",
            "/3/drive/9/files/1/files",
            json!({"drive_id":9}),
        ),
        (
            "kdrive_search",
            "/3/drive/9/files/search",
            json!({"drive_id":9,"query":"report"}),
        ),
        (
            "kdrive_recents",
            "/3/drive/9/files/recents",
            json!({"drive_id":9}),
        ),
        (
            "kdrive_shared_with_me",
            "/3/drive/9/files/shared_with_me",
            json!({"drive_id":9}),
        ),
    ] {
        let h = Harness::new().await;
        for invalid in [
            json!(null),
            json!({}),
            json!({"id":-1}),
            json!({"id":42,"drive_id":10}),
        ] {
            h.mock.lock().unwrap().responses.insert(
                format!("GET {path}"),
                json!({"data":[{"id":41,"name":"valid.txt"},invalid],"has_more":false}),
            );
            let out = h.call(name, args.clone()).await;
            assert_eq!(
                out["result"]["_meta"]["error_code"], "provider_error",
                "{out}"
            );
            let refs: i64 = h
                .app
                .store
                .with(
                    |db| Ok(db.query_row("SELECT count(*) FROM resource_refs", [], |r| r.get(0))?),
                )
                .unwrap();
            assert_eq!(refs, 0);
        }
    }
}

#[tokio::test]
async fn invalid_kdrive_creation_results_are_uncertain_and_never_retried() {
    for (name, path, args, phase) in [
        (
            "kdrive_upload_file",
            "/3/drive/9/upload",
            json!({"drive_id":9,"directory_id":12,"file_name":"Private filename","text_content":"Private body"}),
            "upload_file",
        ),
        (
            "kdrive_create_folder",
            "/3/drive/9/files/12/directory",
            json!({"drive_id":9,"file_id":12,"name":"Private folder"}),
            "create_folder",
        ),
    ] {
        for invalid in [
            json!(null),
            json!([]),
            json!({}),
            json!({"id":0}),
            json!({"id":42,"drive_id":10}),
            json!({"id":42,"drive_id":"invalid"}),
            json!({"id":-1,"drive_id":10}),
        ] {
            let h = Harness::new().await;
            h.mock
                .lock()
                .unwrap()
                .responses
                .insert(format!("POST {path}"), json!({"data":invalid}));
            let out = h.call(name, args.clone()).await;
            assert_eq!(
                out["result"]["_meta"]["error_code"], "mutation_uncertain",
                "{out}"
            );
            let context = &out["result"]["_meta"]["error_context"];
            assert_eq!(context["drive_id"], 9);
            assert_eq!(context["phase"], phase);
            if invalid["id"] == 42 {
                assert_eq!(context["returned_file_id"], 42, "{out}");
            } else {
                assert!(context["returned_file_id"].is_null(), "{out}");
            }
            if invalid["drive_id"] == 10 {
                assert_eq!(context["returned_drive_id"], 10, "{out}");
            } else {
                assert!(context["returned_drive_id"].is_null(), "{out}");
            }
            assert_eq!(
                context[if name == "kdrive_upload_file" {
                    "directory_id"
                } else {
                    "file_id"
                }],
                12
            );
            assert!(!out.to_string().contains("Private"));
            assert_eq!(h.mock.lock().unwrap().calls.len(), 1);
        }
    }
}

#[tokio::test]
async fn kdrive_resource_ids_obey_the_same_numeric_bounds_as_tools() {
    let h = Harness::new().await;
    for uri in [
        "kdrive://0/42",
        "kdrive://9/-1",
        "kdrive://9007199254740992/42",
        "kdrive://9/9007199254740992",
        "kdrive://9/42/extra",
    ] {
        let out = h.rpc("resources/read", json!({"uri":uri})).await;
        assert_eq!(out.value["error"]["code"], -32602, "{}", out.value);
    }
    assert!(h.mock.lock().unwrap().calls.is_empty());
}

#[tokio::test]
async fn kdrive_sharing_identifiers_never_become_file_resources() {
    let h = Harness::new().await;
    h.mock.lock().unwrap().responses.insert(
        "POST /2/drive/9/files/42/link".into(),
        json!({"data":{"id":501,"name":"Public link","url":"https://kdrive.infomaniak.com/share/example"}}),
    );
    let out = h
        .call(
            "kdrive_share_link",
            json!({"drive_id":9,"file_id":42,"right":"public"}),
        )
        .await;
    assert_eq!(out["result"]["isError"], false, "{out}");
    assert_eq!(result(&out)["data"]["id"], 501);
    assert!(result(&out)["data"].get("source_url").is_none());
    assert_eq!(result(&out)["context"]["file_id"], 42);
    let resources = h.rpc("resources/list", json!({})).await.value;
    assert_eq!(
        resources["result"]["resources"].as_array().unwrap().len(),
        4
    );
    assert!(
        !h.mock
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|c| c.1.contains("/files/501"))
    );
}

#[tokio::test]
async fn kdrive_file_mutations_reject_mismatched_returned_identities() {
    for (name, method, path, args, phase) in [
        (
            "kdrive_rename",
            "POST",
            "/2/drive/9/files/42/rename",
            json!({"drive_id":9,"file_id":42,"name":"renamed"}),
            "rename",
        ),
        (
            "kdrive_move",
            "POST",
            "/3/drive/9/files/42/move/12",
            json!({"drive_id":9,"file_id":42,"dest_id":12}),
            "move",
        ),
        (
            "kdrive_restore",
            "POST",
            "/2/drive/9/trash/42/restore",
            json!({"drive_id":9,"file_id":42,"destination_directory_id":12}),
            "restore",
        ),
        (
            "kdrive_delete",
            "DELETE",
            "/2/drive/9/files/42",
            json!({"drive_id":9,"file_id":42}),
            "delete",
        ),
    ] {
        for invalid in [
            json!({"id":43,"drive_id":9}),
            json!({"drive_id":10}),
            json!({"drive_id":null}),
            json!({"id":null}),
            json!([]),
            json!(null),
            json!(true),
            json!("success"),
        ] {
            let h = Harness::new().await;
            h.mock
                .lock()
                .unwrap()
                .responses
                .insert(format!("{method} {path}"), json!({"data":invalid}));
            let out = h.call(name, args.clone()).await;
            assert_eq!(
                out["result"]["_meta"]["error_code"], "mutation_uncertain",
                "{name}: {out}"
            );
            let context = &out["result"]["_meta"]["error_context"];
            assert_eq!(context["file_id"], 42);
            assert_eq!(context["phase"], phase);
            if invalid["id"] == 43 {
                assert_eq!(context["returned_file_id"], 43, "{out}");
            }
            if invalid["drive_id"] == 10 {
                assert_eq!(context["returned_drive_id"], 10, "{out}");
            }
            assert_eq!(h.mock.lock().unwrap().calls.len(), 1);
        }
    }
}

#[tokio::test]
async fn kdrive_file_mutation_acknowledgements_can_omit_identities() {
    for (name, method, path, args) in [
        (
            "kdrive_rename",
            "POST",
            "/2/drive/9/files/42/rename",
            json!({"drive_id":9,"file_id":42,"name":"renamed"}),
        ),
        (
            "kdrive_move",
            "POST",
            "/3/drive/9/files/42/move/12",
            json!({"drive_id":9,"file_id":42,"dest_id":12}),
        ),
        (
            "kdrive_restore",
            "POST",
            "/2/drive/9/trash/42/restore",
            json!({"drive_id":9,"file_id":42,"destination_directory_id":12}),
        ),
        (
            "kdrive_delete",
            "DELETE",
            "/2/drive/9/files/42",
            json!({"drive_id":9,"file_id":42}),
        ),
    ] {
        for acknowledgement in [
            json!({"ok":true}),
            json!({"drive_id":9}),
            json!({"id":42,"drive_id":9}),
        ] {
            let h = Harness::new().await;
            h.mock
                .lock()
                .unwrap()
                .responses
                .insert(format!("{method} {path}"), json!({"data":acknowledgement}));
            let out = h.call(name, args.clone()).await;
            assert_eq!(out["result"]["isError"], false, "{name}: {out}");
            assert_eq!(result(&out)["context"]["file_id"], 42);
            assert_eq!(result(&out)["context"]["drive_id"], 9);
            assert_eq!(h.mock.lock().unwrap().calls.len(), 1);
        }
    }
}

#[tokio::test]
async fn deleted_or_unshared_resources_do_not_break_discovery() {
    let h = Harness::new().await;
    for status in [StatusCode::NOT_FOUND, StatusCode::FORBIDDEN] {
        h.mock.lock().unwrap().response_statuses.clear();
        let known = h.call("kdrive_list", json!({"drive_id":9})).await;
        assert_eq!(known["result"]["isError"], false);
        h.mock
            .lock()
            .unwrap()
            .response_statuses
            .insert("GET /3/drive/9/files/42".into(), status);
        let discovered = h.rpc("resources/list", json!({})).await.value;
        let resources = discovered["result"]["resources"].as_array().unwrap();
        assert!(
            resources
                .iter()
                .all(|r| r["uri"].as_str().unwrap().starts_with("ui://")),
            "{discovered}"
        );
        let count = h
            .app
            .store
            .with(|db| {
                Ok(
                    db.query_row("SELECT count(*) FROM resource_refs", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                )
            })
            .unwrap();
        assert_eq!(count, 0);
        let direct = h
            .rpc("resources/read", json!({"uri":"kdrive://9/42"}))
            .await;
        if status == StatusCode::FORBIDDEN {
            assert_eq!(direct.status, 403);
            assert_eq!(direct.value["error"], "access_denied");
        } else {
            assert!(direct.value["error"].is_object());
        }
        assert!(!direct.value.to_string().contains("sensitive upstream"));
        assert!(
            !h.mock
                .lock()
                .unwrap()
                .calls
                .iter()
                .any(|call| call.1.contains("/download"))
        );
    }
}
#[tokio::test]
async fn resource_pagination_does_not_skip_files_when_stale_references_are_removed() {
    let h = Harness::new().await;
    h.app.store.with(|db| {
        for file in 1..=25 {
            db.execute("INSERT INTO resource_refs(user_id,drive_id,file_id,name,mime) VALUES(?,9,?,'file','text/plain')", params![h.user.id,file])?;
        }
        Ok(())
    }).unwrap();
    {
        let mut mock = h.mock.lock().unwrap();
        for file in 1..=25 {
            let key = format!("GET /3/drive/9/files/{file}");
            if file > 5 {
                mock.response_statuses.insert(key, StatusCode::NOT_FOUND);
            } else {
                mock.responses.insert(key, json!({"data":{"id":file,"name":format!("file-{file}"),"mime_type":"text/plain"}}));
            }
        }
    }
    let first = h.rpc("resources/list", json!({})).await.value;
    assert_eq!(
        first["result"]["resources"].as_array().unwrap().len(),
        4,
        "{first}"
    );
    let cursor = &first["result"]["nextCursor"];
    assert!(cursor.is_string());
    let last = h
        .rpc("resources/list", json!({"cursor":cursor}))
        .await
        .value;
    let files = last["result"]["resources"].as_array().unwrap();
    assert_eq!(files.len(), 5, "{last}");
    assert_eq!(files[0]["uri"], "kdrive://9/5");
    assert_eq!(files[4]["uri"], "kdrive://9/1");
    assert!(last["result"].get("nextCursor").is_none());
}
#[tokio::test]
async fn oversized_downloads_fail_without_exposing_provider_bodies() {
    let h = Harness::new().await;
    h.mock.lock().unwrap().large = true;
    let v = h
        .call("kdrive_download_file", json!({"drive_id":9,"file_id":42}))
        .await;
    assert_eq!(v["result"]["isError"], true);
    assert!(v.to_string().contains("content_too_large"));
}
#[tokio::test]
async fn oversized_mutation_responses_are_uncertain_and_never_retried() {
    let h = Harness::new().await;
    h.mock.lock().unwrap().responses.insert(
        "POST /3/drive/9/files/1/directory".into(),
        json!({"data":"s".repeat(ensemble_mcp::identity::MAX_BYTES + 1)}),
    );
    let out = h
        .call(
            "kdrive_create_folder",
            json!({"drive_id":9,"name":"folder"}),
        )
        .await;
    assert_eq!(out["result"]["_meta"]["error_code"], "mutation_uncertain");
    assert!(
        out["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("verify")
    );
    assert_eq!(h.mock.lock().unwrap().calls.len(), 1);
}
#[tokio::test]
async fn outgoing_mail_headers_are_validated_before_draft_or_attachment_writes() {
    for (name, field) in [
        ("mail_create_draft", "subject"),
        ("mail_send_email", "subject"),
        ("mail_send_email", "from_name"),
        ("mail_send_email", "in_reply_to"),
        ("mail_send_email", "references"),
        ("mail_update_draft", "subject"),
    ] {
        for invalid in ["Subject\r\nBcc: victim@example.org", "Hidden\0header"] {
            let h = Harness::new().await;
            let mut args = if name == "mail_update_draft" {
                json!({"draft_uuid":"draft-1","attachments":[{"filename":"a.txt","mime_type":"text/plain","base64_content":"YQ=="}]})
            } else {
                json!({"to":"person@example.org","subject":"Subject","body":"Line one\nLine two"})
            };
            args[field] = json!(invalid);
            let out = h.call(name, args).await;
            assert_eq!(
                out["result"]["_meta"]["error_code"], "invalid_request",
                "{out}"
            );
            assert!(h.mock.lock().unwrap().calls.iter().all(|c| c.0 == "GET"));
        }
    }
    let h = Harness::new().await;
    let out = h.call("mail_create_draft", json!({"to":"person@example.org","subject":"Überblick — notes","body":"Line one\nLine two","in_reply_to":"<message@example.org>","references":"<older@example.org> <message@example.org>"})).await;
    assert_eq!(out["result"]["isError"], false, "{out}");
    let mock = h.mock.lock().unwrap();
    let created = &mock.calls.iter().find(|c| c.0 == "POST").unwrap().3;
    assert!(
        created["body"]
            .as_str()
            .unwrap()
            .contains("Line one<br>Line two")
    );
    assert_eq!(
        created["references"],
        "<older@example.org> <message@example.org>"
    );
}

#[tokio::test]
async fn unsafe_provider_mail_headers_are_rejected_before_partial_mutations() {
    for (name, args, target, response) in [
        (
            "mail_send_email",
            json!({"to":"person@example.org","subject":"Subject","body":"Body"}),
            "GET /1/mail_hostings/5/mailboxes/alice/signatures",
            json!({"data":{"signatures":[{"sender":"alice@team-ensemble.ch","full_name":"Alice\r\nBcc: victim@example.org","is_default":true}]}}),
        ),
        (
            "mail_send_email",
            json!({"to":"person@example.org","subject":"Subject","body":"Body","in_reply_to_uid":"12@inbox"}),
            "GET /mail/box-a/folder/inbox/message/12",
            json!({"data":{"uid":"12@inbox","msg_id":"<message@example.org>","headers":{"references":"<older@example.org>\r\nBcc: victim@example.org"}}}),
        ),
        (
            "mail_update_draft",
            json!({"draft_uuid":"draft-1","attachments":[{"filename":"a.txt","mime_type":"text/plain","base64_content":"YQ=="}]}),
            "GET /mail/box-a/draft/draft-1",
            json!({"data":{"uuid":"draft-1","subject":"Subject\r\nBcc: victim@example.org"}}),
        ),
    ] {
        let h = Harness::new().await;
        h.mock
            .lock()
            .unwrap()
            .responses
            .insert(target.into(), response);
        let out = h.call(name, args).await;
        assert_eq!(
            out["result"]["_meta"]["error_code"], "provider_error",
            "{out}"
        );
        assert!(h.mock.lock().unwrap().calls.iter().all(|c| c.0 == "GET"));
        assert!(!out.to_string().contains("victim@example.org"));
    }
}

#[tokio::test]
async fn reused_draft_headers_and_attachment_metadata_are_checked_before_uploads() {
    for fields in [
        json!({"to":[{"email":"person@example.org","name":"Person\r\nBcc: victim@example.org"}]}),
        json!({"cc":[{"email":"person@example.org\0","name":"Person"}]}),
        json!({"bcc":[{"email":"person@example.org","name":"Hidden\tname"}]}),
        json!({"from":{"email":"alice@team-ensemble.ch","name":"Alice\r\nInjected: yes"}}),
        json!({"reply_to":{"email":"alice@team-ensemble.ch","name":"Alice\0"}}),
        json!({"headers":{"X-Custom":"Value\r\nInjected: yes"}}),
        json!({"attachments":[{"uuid":"old","filename":"report\t.txt","mime_type":"text/plain"}]}),
        json!({"attachments":[{"uuid":"old","filename":"report.txt","mime_type":"text/plain\r\nInjected: yes"}]}),
        json!({"subject":{"text":"Malformed"}}),
        json!({"to":[{"email":false}]}),
    ] {
        for name in ["mail_update_draft", "mail_send_draft"] {
            let h = Harness::new().await;
            let mut draft = json!({"uuid":"draft-1","body":"Line one\nLine two"});
            draft
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            h.mock.lock().unwrap().responses.insert(
                "GET /mail/box-a/draft/draft-1".into(),
                json!({"data":draft}),
            );
            let args = if name == "mail_update_draft" {
                json!({"draft_uuid":"draft-1","attachments":[{"filename":"new.txt","mime_type":"text/plain","base64_content":"YQ=="}]})
            } else {
                json!({"draft_uuid":"draft-1"})
            };
            let out = h.call(name, args).await;
            assert_eq!(
                out["result"]["_meta"]["error_code"], "provider_error",
                "{out}"
            );
            assert!(h.mock.lock().unwrap().calls.iter().all(|c| c.0 == "GET"));
            assert!(!out.to_string().contains("victim@example.org"));
        }
    }
    let h = Harness::new().await;
    let draft = json!({"uuid":"draft-1","subject":"Résumé","body":"Line one\nLine two", "to":[{"email":"person@example.org","name":"Élodie"}],"headers":{"X-Custom":"Value"},"attachments":[{"uuid":"old","filename":"überblick.txt","mime_type":"text/plain"}]});
    h.mock.lock().unwrap().responses.insert(
        "GET /mail/box-a/draft/draft-1".into(),
        json!({"data":draft}),
    );
    let out = h
        .call(
            "mail_update_draft",
            json!({"draft_uuid":"draft-1","subject":"Updated"}),
        )
        .await;
    assert_eq!(out["result"]["isError"], false, "{out}");
    let mock = h.mock.lock().unwrap();
    let saved = &mock.calls.iter().find(|c| c.0 == "PUT").unwrap().3;
    assert_eq!(saved["body"], draft["body"]);
    assert_eq!(saved["to"], draft["to"]);
    assert_eq!(saved["attachments"], json!(["old"]));
}

#[tokio::test]
async fn sending_identity_authorisation_failures_stop_before_draft_creation() {
    for (status, code) in [
        (StatusCode::UNAUTHORIZED, "reconnection_required"),
        (StatusCode::FORBIDDEN, "access_denied"),
    ] {
        let h = Harness::new().await;
        h.mock.lock().unwrap().response_statuses.insert(
            "GET /1/mail_hostings/5/mailboxes/alice/signatures".into(),
            status,
        );
        let reply = h.rpc("tools/call", json!({"name":"mail_send_email","arguments":{"to":"person@example.org","subject":"Subject","body":"Body"}})).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert_eq!(reply.value["error"], code);
        assert!(h.mock.lock().unwrap().calls.iter().all(|c| c.0 == "GET"));
    }
    let h = Harness::new().await;
    h.mock.lock().unwrap().response_statuses.insert(
        "GET /1/mail_hostings/5/mailboxes/alice/signatures".into(),
        StatusCode::BAD_GATEWAY,
    );
    let out = h
        .call(
            "mail_create_draft",
            json!({"to":"person@example.org","subject":"Subject","body":"Body"}),
        )
        .await;
    assert_eq!(out["result"]["isError"], false, "{out}");
    assert!(
        !result(&out)["data"]["warnings"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn invalid_attachments_are_rejected_before_any_provider_write() {
    let h = Harness::new().await;
    let valid = json!({"filename":"report.txt","mime_type":"text/plain","base64_content":"YQ=="});
    for invalid in [
        json!({"filename":"bad.txt","mime_type":"text/plain","base64_content":"%%%"}),
        json!({"filename":"bad\r\nheader","mime_type":"text/plain","base64_content":"YQ=="}),
        json!({"filename":"bad.txt","mime_type":"text/plain\r\nheader","base64_content":"YQ=="}),
        json!({"filename":"bad\t.txt","mime_type":"text/plain","base64_content":"YQ=="}),
        json!({"filename":"bad.txt","mime_type":"text/\tplain","base64_content":"YQ=="}),
    ] {
        for (name, mut args) in [
            (
                "mail_send_email",
                json!({"to":"person@example.org","subject":"Subject","body":"Private body"}),
            ),
            ("mail_update_draft", json!({"draft_uuid":"draft-1"})),
        ] {
            args["attachments"] = json!([valid, invalid]);
            let out = h.call(name, args).await;
            assert_eq!(
                out["result"]["_meta"]["error_code"], "invalid_request",
                "{out}"
            );
            assert!(h.mock.lock().unwrap().calls.is_empty());
        }
    }
}
#[tokio::test]
async fn failed_send_reports_the_created_draft_and_original_failure() {
    for (method, status, code, cause) in [
        (
            "PUT",
            StatusCode::UNAUTHORIZED,
            "reconnection_required",
            "reconnection_required",
        ),
        (
            "PUT",
            StatusCode::FORBIDDEN,
            "access_denied",
            "access_denied",
        ),
        (
            "PUT",
            StatusCode::BAD_GATEWAY,
            "mutation_uncertain",
            "mutation_uncertain",
        ),
    ] {
        let h = Harness::new().await;
        h.mock
            .lock()
            .unwrap()
            .response_statuses
            .insert(format!("{method} /mail/box-a/draft/draft-1"), status);
        let reply = h
            .rpc(
                "tools/call",
                json!({"name":"mail_send_email","arguments":{"to":"person@example.org","subject":"Subject","body":"Private body","attachments":[{"filename":"report.txt","mime_type":"text/plain","base64_content":"YQ=="}]}}),
            )
            .await;
        let out = &reply.value;
        let context = if code == "access_denied" || code == "reconnection_required" {
            assert_eq!(reply.status, 403);
            assert_eq!(out["error"], code);
            &out["context"]
        } else {
            assert_eq!(reply.status, 200);
            assert_eq!(out["result"]["_meta"]["error_code"], code, "{out}");
            let text = out["result"]["content"][0]["text"].as_str().unwrap();
            assert!(
                text.contains("draft-1")
                    && text.contains(cause)
                    && text.contains("Inspect Drafts and Sent")
            );
            &out["result"]["_meta"]["error_context"]
        };
        assert_eq!(context["draft_uuid"], "draft-1");
        assert_eq!(context["cause"], cause);
        if method == "PUT" {
            assert_eq!(context["progress"]["completed_attachment_uploads"], 1);
        }
        assert!(!out.to_string().contains("sensitive upstream"));
        let mock = h.mock.lock().unwrap();
        assert_eq!(
            mock.calls
                .iter()
                .filter(|c| c.0 == "POST" && c.1 == "/mail/box-a/draft?")
                .count(),
            1
        );
        assert_eq!(
            mock.calls.iter().filter(|c| c.0 == "PUT").count(),
            usize::from(method == "PUT")
        );
    }
}
#[tokio::test]
async fn interrupted_attachment_uploads_report_progress_without_sending_or_saving() {
    for (name, mut args) in [
        (
            "mail_send_email",
            json!({"to":"person@example.org","subject":"Subject","body":"Private body"}),
        ),
        ("mail_update_draft", json!({"draft_uuid":"draft-1"})),
    ] {
        let h = Harness::new().await;
        h.mock.lock().unwrap().queued_responses.insert(
            "POST /mail/box-a/draft/attachment".into(),
            VecDeque::from([
                (StatusCode::OK, json!({"data":{"uuid":"attachment-1"}})),
                (
                    StatusCode::FORBIDDEN,
                    json!({"error":"sensitive upstream error"}),
                ),
            ]),
        );
        args["attachments"] = json!([
            {"filename":"first.txt","mime_type":"text/plain","base64_content":"YQ=="},
            {"filename":"second.txt","mime_type":"text/plain","base64_content":"Yg=="},
        ]);
        let reply = h
            .rpc("tools/call", json!({"name":name,"arguments":args}))
            .await;
        assert_eq!(reply.status, 403);
        let out = reply.value;
        assert_eq!(out["error"], "access_denied", "{out}");
        let context = &out["context"];
        assert_eq!(context["draft_uuid"], "draft-1");
        assert_eq!(context["phase"], "upload_attachments");
        assert_eq!(context["cause"], "access_denied");
        assert_eq!(context["progress"]["completed_attachment_uploads"], 1);
        let mock = h.mock.lock().unwrap();
        assert_eq!(
            mock.calls
                .iter()
                .filter(|c| c.1 == "/mail/box-a/draft/attachment?")
                .count(),
            2
        );
        assert!(!mock.calls.iter().any(|c| c.0 == "PUT"));
        assert!(!out.to_string().contains("sensitive upstream"));
    }
}
#[tokio::test]
async fn failed_draft_save_reports_uploaded_attachments_and_preserves_uncertainty() {
    for (status, code) in [
        (StatusCode::FORBIDDEN, "access_denied"),
        (StatusCode::BAD_GATEWAY, "mutation_uncertain"),
    ] {
        let h = Harness::new().await;
        h.mock
            .lock()
            .unwrap()
            .response_statuses
            .insert("PUT /mail/box-a/draft/draft-1".into(), status);
        let reply = h.rpc("tools/call", json!({"name":"mail_update_draft","arguments":{"draft_uuid":"draft-1","attachments":[{"filename":"report.txt","mime_type":"text/plain","base64_content":"YQ=="}]}})).await;
        let out = reply.value;
        let context = if code == "access_denied" {
            assert_eq!(reply.status, 403);
            assert_eq!(out["error"], code);
            &out["context"]
        } else {
            assert_eq!(out["result"]["_meta"]["error_code"], code, "{out}");
            &out["result"]["_meta"]["error_context"]
        };
        assert_eq!(context["draft_uuid"], "draft-1");
        assert_eq!(context["phase"], "save_draft");
        assert_eq!(context["progress"]["completed_attachment_uploads"], 1);
        let mock = h.mock.lock().unwrap();
        let saves = mock
            .calls
            .iter()
            .filter(|c| c.0 == "PUT")
            .collect::<Vec<_>>();
        assert_eq!(saves.len(), 1);
        assert_eq!(saves[0].3["action"], "save");
    }
}

#[tokio::test]
async fn malformed_attachment_upload_identity_stops_with_draft_recovery_context() {
    for uuid in [
        json!(null),
        json!({}),
        json!(""),
        json!(".."),
        json!("bad/id"),
    ] {
        let h = Harness::new().await;
        h.mock.lock().unwrap().responses.insert(
            "POST /mail/box-a/draft/attachment".into(),
            json!({"data":{"uuid":uuid}}),
        );
        let out = h.call("mail_update_draft", json!({"draft_uuid":"draft-1","attachments":[{"filename":"report.txt","mime_type":"text/plain","base64_content":"YQ=="}]})).await;
        assert_eq!(
            out["result"]["_meta"]["error_code"], "mutation_uncertain",
            "{out}"
        );
        let context = &out["result"]["_meta"]["error_context"];
        assert_eq!(context["draft_uuid"], "draft-1");
        assert_eq!(context["phase"], "upload_attachments");
        assert_eq!(context["progress"]["completed_attachment_uploads"], 0);
        assert!(!h.mock.lock().unwrap().calls.iter().any(|c| c.0 == "PUT"));
    }
}
#[tokio::test]
async fn sending_an_existing_draft_retains_recovery_context() {
    let h = Harness::new().await;
    h.mock.lock().unwrap().response_statuses.insert(
        "PUT /mail/box-a/draft/draft-1".into(),
        StatusCode::BAD_GATEWAY,
    );
    let out = h
        .call(
            "mail_send_draft",
            json!({"draft_uuid":"draft-1","delay":30}),
        )
        .await;
    assert_eq!(out["result"]["_meta"]["error_code"], "mutation_uncertain");
    let context = &out["result"]["_meta"]["error_context"];
    assert_eq!(context["draft_uuid"], "draft-1");
    assert_eq!(context["phase"], "send");
    assert_eq!(context["progress"]["completed_attachment_uploads"], 0);
    let mock = h.mock.lock().unwrap();
    let sends = mock
        .calls
        .iter()
        .filter(|c| c.0 == "PUT")
        .collect::<Vec<_>>();
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0].3["action"], "send");
    assert_eq!(sends[0].3["delay"], 30);
}
#[tokio::test]
async fn operation_deadline_retains_the_known_draft_phase_and_upload_progress() {
    let mut h = Harness::new().await;
    let reply = h.deadline("mail_send_email", json!({"to":"person@example.org","subject":"Subject","body":"Private body","attachments":[{"filename":"report.txt","mime_type":"text/plain","base64_content":"YQ=="}]}), "PUT /mail/box-a/draft/draft-1").await;
    let out = reply.value;
    assert_eq!(reply.status, 200);
    assert_eq!(
        out["result"]["_meta"]["error_code"], "mutation_uncertain",
        "{out}"
    );
    let context = &out["result"]["_meta"]["error_context"];
    assert_eq!(context["draft_uuid"], "draft-1");
    assert_eq!(context["phase"], "send");
    assert_eq!(context["cause"], "mutation_uncertain");
    assert_eq!(context["progress"]["completed_attachment_uploads"], 1);
    assert!(
        out["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("draft-1")
    );
    assert_eq!(
        h.mock
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|c| c.0 == "PUT")
            .count(),
        1
    );
    let logs: String = h
        .app
        .store
        .with(|c| {
            Ok(c.query_row(
                "SELECT group_concat(action || outcome) FROM logs",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert!(!logs.contains("Private body") && !logs.contains("draft-1"));
}
#[tokio::test]
async fn mutation_failures_retain_object_context_and_transport_authorisation() {
    for (name, args, target, expected) in [
        (
            "calendar_update_event",
            json!({"event_id":"7","title":"Private title"}),
            "PUT /1/calendar/pim/event/7",
            json!({"event_id":"7","calendar_id":"actual-calendar","phase":"update_event"}),
        ),
        (
            "calendar_delete_event",
            json!({"event_id":"7"}),
            "DELETE /1/calendar/pim/event/7",
            json!({"event_id":"7","calendar_id":"actual-calendar","phase":"delete_event"}),
        ),
        (
            "kdrive_move",
            json!({"drive_id":9,"file_id":42,"dest_id":12}),
            "POST /3/drive/9/files/42/move/12",
            json!({"drive_id":9,"file_id":42,"dest_id":12,"phase":"move"}),
        ),
        (
            "kdrive_share_link",
            json!({"drive_id":9,"file_id":42,"right":"password","password":"private-sharing-password"}),
            "POST /2/drive/9/files/42/link",
            json!({"drive_id":9,"file_id":42,"phase":"share_link"}),
        ),
        (
            "mail_move_email",
            json!({"message_ids":["12"],"from_folder_id":"inbox","to_folder_id":"trash"}),
            "POST /mail/box-a/message/move",
            json!({"mailbox_uuid":"box-a","message_ids":["12"],"from_folder_id":"inbox","to_folder_id":"trash","phase":"move_email"}),
        ),
        (
            "mail_delete_draft",
            json!({"draft_uuid":"draft-1"}),
            "DELETE /mail/box-a/draft/draft-1",
            json!({"mailbox_uuid":"box-a","draft_uuid":"draft-1","phase":"delete_draft"}),
        ),
        (
            "mail_create_folder",
            json!({"name":"Private title","parent_folder_id":"inbox"}),
            "POST /mail/box-a/folder",
            json!({"mailbox_uuid":"box-a","parent_folder_id":"inbox","phase":"create_folder"}),
        ),
    ] {
        for status in [
            StatusCode::BAD_GATEWAY,
            StatusCode::BAD_REQUEST,
            StatusCode::FORBIDDEN,
        ] {
            let h = Harness::new().await;
            h.mock
                .lock()
                .unwrap()
                .response_statuses
                .insert(target.into(), status);
            let reply = h
                .rpc("tools/call", json!({"name":name,"arguments":args}))
                .await;
            let (code, context) = if status == StatusCode::FORBIDDEN {
                assert_eq!(reply.status, 403, "{}", reply.value);
                (&reply.value["error"], &reply.value["context"])
            } else {
                assert_eq!(reply.status, 200, "{}", reply.value);
                assert_eq!(reply.value["result"]["isError"], true);
                (
                    &reply.value["result"]["_meta"]["error_code"],
                    &reply.value["result"]["_meta"]["error_context"],
                )
            };
            assert_eq!(
                code,
                if status == StatusCode::FORBIDDEN {
                    "access_denied"
                } else if status.is_server_error() {
                    "mutation_uncertain"
                } else {
                    "provider_error"
                }
            );
            for (key, value) in expected.as_object().unwrap() {
                assert_eq!(&context[key], value, "{name}: {}", reply.value);
                if reply.status == 200 {
                    assert!(
                        reply.value["result"]["content"][0]["text"]
                            .as_str()
                            .unwrap()
                            .contains(key)
                    );
                }
            }
            assert_eq!(context["cause"], *code);
            assert!(!reply.value.to_string().contains("private-sharing-password"));
            assert!(!reply.value.to_string().contains("Private title"));
            assert_eq!(
                h.mock
                    .lock()
                    .unwrap()
                    .calls
                    .iter()
                    .filter(|c| format!("{} {}", c.0, c.1.split('?').next().unwrap()) == target)
                    .count(),
                1
            );
        }
    }
}

#[tokio::test]
async fn mutation_deadlines_retain_calendar_drive_and_mail_creation_context() {
    for (name, args, target, expected) in [
        (
            "calendar_create_event",
            json!({"title":"Private title","start":"2026-10-17T10:00","end":"2026-10-17T11:00"}),
            "POST /1/calendar/pim/event",
            json!({"calendar_id":"primary","phase":"create_event"}),
        ),
        (
            "calendar_update_event",
            json!({"event_id":"7","title":"Private title"}),
            "PUT /1/calendar/pim/event/7",
            json!({"event_id":"7","calendar_id":"actual-calendar","phase":"update_event"}),
        ),
        (
            "kdrive_upload_file",
            json!({"drive_id":9,"directory_id":12,"file_name":"Private filename","text_content":"Private file body"}),
            "POST /3/drive/9/upload",
            json!({"drive_id":9,"directory_id":12,"phase":"upload_file"}),
        ),
        (
            "kdrive_delete",
            json!({"drive_id":9,"file_id":42}),
            "DELETE /2/drive/9/files/42",
            json!({"drive_id":9,"file_id":42,"phase":"delete"}),
        ),
        (
            "mail_send_email",
            json!({"to":"person@example.org","subject":"Subject","body":"Private body"}),
            "POST /mail/box-a/draft",
            json!({"mailbox_uuid":"box-a","phase":"create_draft"}),
        ),
        (
            "mail_create_folder",
            json!({"name":"Private title","parent_folder_id":"inbox"}),
            "POST /mail/box-a/folder",
            json!({"mailbox_uuid":"box-a","parent_folder_id":"inbox","phase":"create_folder"}),
        ),
    ] {
        let mut h = Harness::new().await;
        let reply = h.deadline(name, args, target).await;
        assert_eq!(reply.status, 200);
        assert_eq!(
            reply.value["result"]["_meta"]["error_code"], "mutation_uncertain",
            "{}",
            reply.value
        );
        let context = &reply.value["result"]["_meta"]["error_context"];
        for (key, value) in expected.as_object().unwrap() {
            assert_eq!(&context[key], value, "{name}: {}", reply.value);
        }
        assert_eq!(context["cause"], "mutation_uncertain");
        assert!(!reply.value.to_string().contains("Private"));
        let writes = h
            .mock
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|c| c.0 != "GET")
            .count();
        assert_eq!(writes, 1);
        let logs: String = h
            .app
            .store
            .with(|c| {
                Ok(c.query_row(
                    "SELECT group_concat(action || outcome) FROM logs",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert!(
            !logs.contains("Private")
                && !logs.contains("actual-calendar")
                && !logs.contains("box-a")
        );
    }
}

#[tokio::test]
async fn calendar_mutations_reject_mismatched_events_and_invalid_calendar_ids() {
    for invalid in [
        json!({"id":"other-event","calendar_id":"actual-calendar"}),
        json!({"id":8,"calendar_id":"actual-calendar"}),
        json!({"id":7,"calendar_id":{}}),
        json!({"id":7,"calendar_id":[]}),
        json!({"id":7,"calendar_id":true}),
        json!({"id":7,"calendar_id":""}),
        json!({"id":7,"calendar_id":"../other"}),
        json!({"id":7,"calendar_id":0}),
    ] {
        for name in ["calendar_update_event", "calendar_delete_event"] {
            let h = Harness::new().await;
            h.mock.lock().unwrap().responses.insert(
                "GET /1/calendar/pim/event/7".into(),
                json!({"data":invalid}),
            );
            let out = h.call(name, json!({"event_id":"7"})).await;
            assert_eq!(
                out["result"]["_meta"]["error_code"], "provider_error",
                "{name}: {out}"
            );
            assert!(h.mock.lock().unwrap().calls.iter().all(|c| c.0 == "GET"));
        }
    }
}

#[tokio::test]
async fn calendar_mutation_results_require_usable_consistent_identities() {
    for (name, args, target, calendar) in [
        (
            "calendar_create_event",
            json!({"title":"Private title","start":"2026-10-17T10:00","end":"2026-10-17T11:00"}),
            "POST /1/calendar/pim/event",
            "primary",
        ),
        (
            "calendar_update_event",
            json!({"event_id":"7","title":"Private title"}),
            "PUT /1/calendar/pim/event/7",
            "actual-calendar",
        ),
    ] {
        let mut invalid = vec![
            Value::Null,
            json!([]),
            json!(false),
            json!({}),
            json!({"id":""}),
            json!({"id":"../other"}),
            json!({"id":0}),
            json!({"id":7,"calendar_id":"other-calendar"}),
            json!({"id":7,"calendar_id":null}),
            json!({"id":7,"calendar_id":{}}),
        ];
        if name == "calendar_update_event" {
            invalid.push(json!({"id":8,"calendar_id":calendar}));
        }
        for value in invalid {
            let h = Harness::new().await;
            h.mock
                .lock()
                .unwrap()
                .responses
                .insert(target.into(), json!({"data":value}));
            let out = h.call(name, args.clone()).await;
            assert_eq!(
                out["result"]["_meta"]["error_code"], "mutation_uncertain",
                "{name}: {out}"
            );
            let context = &out["result"]["_meta"]["error_context"];
            assert_eq!(context["calendar_id"], calendar);
            assert_eq!(context["phase"], name.strip_prefix("calendar_").unwrap());
            if name == "calendar_update_event" {
                assert_eq!(context["event_id"], "7");
            }
            if let Some(id) = value["id"].as_i64().filter(|id| *id > 0) {
                assert_eq!(context["returned_event_id"], id.to_string());
            }
            assert!(
                out["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains(calendar)
            );
            assert!(!out.to_string().contains("Private title"));
            assert_eq!(
                h.mock
                    .lock()
                    .unwrap()
                    .calls
                    .iter()
                    .filter(|c| c.0 != "GET")
                    .count(),
                1
            );
        }
    }
}

#[tokio::test]
async fn calendar_deletion_acknowledgements_validate_any_returned_identity() {
    for invalid in [
        json!(null),
        json!([]),
        json!(true),
        json!({"id":null}),
        json!({"id":"../unsafe"}),
        json!({"id":8}),
        json!({"calendar_id":"other-calendar"}),
        json!({"calendar_id":null}),
        json!({"id":7,"calendar_id":"other-calendar"}),
    ] {
        let h = Harness::new().await;
        h.mock.lock().unwrap().responses.insert(
            "DELETE /1/calendar/pim/event/7".into(),
            json!({"data":invalid}),
        );
        let out = h
            .call(
                "calendar_delete_event",
                json!({"event_id":"7","notify_attendees":true}),
            )
            .await;
        assert_eq!(
            out["result"]["_meta"]["error_code"], "mutation_uncertain",
            "{out}"
        );
        let context = &out["result"]["_meta"]["error_context"];
        assert_eq!(context["event_id"], "7");
        assert_eq!(context["calendar_id"], "actual-calendar");
        assert_eq!(context["phase"], "delete_event");
        if invalid["id"] == 7 || invalid["id"] == 8 {
            assert_eq!(context["returned_event_id"], invalid["id"].to_string());
        } else {
            assert!(context["returned_event_id"].is_null(), "{out}");
        }
        assert_eq!(
            h.mock
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|c| c.0 == "DELETE")
                .count(),
            1
        );
    }
    for acknowledgement in [
        json!({"deleted":true}),
        json!({"calendar_id":"actual-calendar"}),
        json!({"id":7,"calendar_id":"actual-calendar"}),
    ] {
        let h = Harness::new().await;
        h.mock.lock().unwrap().responses.insert(
            "DELETE /1/calendar/pim/event/7".into(),
            json!({"data":acknowledgement}),
        );
        let out = h
            .call(
                "calendar_delete_event",
                json!({"event_id":"7","notify_attendees":true}),
            )
            .await;
        assert_eq!(out["result"]["isError"], false, "{out}");
        assert_eq!(result(&out)["context"]["event_id"], "7");
        assert_eq!(
            result(&out)["context"]["notification_status"],
            "provider_accepted"
        );
    }
}

#[tokio::test]
async fn calendar_event_lists_validate_identities_and_preserve_provider_metadata() {
    for value in [
        Value::Null,
        json!(false),
        json!({}),
        json!({"id":"../other"}),
        json!({"id":7,"calendar_id":"other-calendar"}),
        json!({"id":7,"calendar_id":null}),
    ] {
        let h = Harness::new().await;
        h.mock
            .lock()
            .unwrap()
            .responses
            .insert("GET /1/calendar/pim/event".into(), json!({"data":[value]}));
        let out = h
            .call(
                "calendar_list_events",
                json!({"from":"2026-10-17","to":"2026-10-18"}),
            )
            .await;
        assert_eq!(
            out["result"]["_meta"]["error_code"], "provider_error",
            "{out}"
        );
    }
    let h = Harness::new().await;
    let event = json!({"id":7,"calendar_id":12,"title":"Meeting","timezone_start":"Europe/Zurich","bookable_resource_id":"room-1"});
    h.mock
        .lock()
        .unwrap()
        .responses
        .insert("GET /1/calendar/pim/event".into(), json!({"data":[event]}));
    let out = h
        .call(
            "calendar_list_events",
            json!({"from":"2026-10-17","to":"2026-10-18","calendar_id":"12"}),
        )
        .await;
    assert_eq!(out["result"]["isError"], false, "{out}");
    assert_eq!(result(&out)["data"][0]["id"], 7);
    assert_eq!(result(&out)["data"][0]["calendar_id"], 12);
    assert_eq!(result(&out)["data"][0]["timezone_start"], "Europe/Zurich");
    assert_eq!(result(&out)["data"][0]["bookable_resource_name"], "Studio");
    for status in [StatusCode::SERVICE_UNAVAILABLE, StatusCode::FORBIDDEN] {
        h.mock
            .lock()
            .unwrap()
            .response_statuses
            .insert("GET /1/calendar/pim/bookable-resources/all".into(), status);
        let out = h
            .call(
                "calendar_list_events",
                json!({"from":"2026-10-17","to":"2026-10-18","calendar_id":"12"}),
            )
            .await;
        assert_eq!(out["result"]["isError"], false, "{out}");
        assert_eq!(result(&out)["data"][0]["id"], 7);
        assert!(result(&out)["data"][0]["bookable_resource_name"].is_null());
        assert_eq!(
            result(&out)["context"]["warning"],
            "Room names unavailable from the provider"
        );
    }
}

#[tokio::test]
async fn calendar_incomplete_lists_never_report_success() {
    let h = Harness::new().await;
    h.mock.lock().unwrap().responses.insert(
        "GET /1/calendar/pim/event".into(),
        json!({"data":[{"id":7}],"has_more":true,"cursor":"next"}),
    );
    let out = h
        .call(
            "calendar_list_events",
            json!({"from":"2026-10-17","to":"2026-10-18"}),
        )
        .await;
    assert_eq!(
        out["result"]["_meta"]["error_code"], "provider_limitation",
        "{out}"
    );
    assert!(
        out["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("smaller date interval")
    );
    assert!(h.mock.lock().unwrap().calls.iter().all(|c| c.0 == "GET"));
}

#[tokio::test]
async fn malformed_provider_state_never_dispatches_draft_or_calendar_writes() {
    for value in [
        json!([]),
        json!(false),
        Value::Null,
        json!({}),
        json!({"uuid":"other-draft"}),
    ] {
        for (name, args, path) in [
            (
                "mail_update_draft",
                json!({"draft_uuid":"draft-1","subject":"New subject"}),
                "/mail/box-a/draft/draft-1",
            ),
            (
                "mail_send_draft",
                json!({"draft_uuid":"draft-1"}),
                "/mail/box-a/draft/draft-1",
            ),
            (
                "calendar_update_event",
                json!({"event_id":"7","title":"New title"}),
                "/1/calendar/pim/event/7",
            ),
            (
                "calendar_delete_event",
                json!({"event_id":"7"}),
                "/1/calendar/pim/event/7",
            ),
        ] {
            let h = Harness::new().await;
            h.mock
                .lock()
                .unwrap()
                .responses
                .insert(format!("GET {path}"), json!({"data":value}));
            let out = h.call(name, args).await;
            assert_eq!(
                out["result"]["_meta"]["error_code"], "provider_error",
                "{name}: {out}"
            );
            assert!(h.mock.lock().unwrap().calls.iter().all(|c| c.0 == "GET"));
        }
    }
}
#[tokio::test]
async fn malformed_draft_creation_is_uncertain_and_is_never_followed_by_a_send() {
    for value in [
        json!([]),
        json!(false),
        Value::Null,
        json!({}),
        json!({"uuid":""}),
    ] {
        for name in ["mail_create_draft", "mail_send_email"] {
            let h = Harness::new().await;
            h.mock
                .lock()
                .unwrap()
                .responses
                .insert("POST /mail/box-a/draft".into(), json!({"data":value}));
            let out = h
                .call(
                    name,
                    json!({"to":"person@example.org","subject":"Subject","body":"Private body"}),
                )
                .await;
            assert_eq!(
                out["result"]["_meta"]["error_code"], "mutation_uncertain",
                "{out}"
            );
            let mock = h.mock.lock().unwrap();
            assert_eq!(mock.calls.iter().filter(|c| c.0 == "POST").count(), 1);
            assert!(!mock.calls.iter().any(|c| c.0 == "PUT"));
        }
    }
}
#[tokio::test]
async fn draft_reads_validate_mailbox_identity_before_uploads_or_sending() {
    for name in ["mail_update_draft", "mail_send_draft"] {
        let h = Harness::new().await;
        h.mock.lock().unwrap().responses.insert(
            "GET /mail/box-a/draft/draft-1".into(),
            json!({"data":{"uuid":"draft-1","mailbox_uuid":"other-mailbox","subject":"Private subject","body":"Private body"}}),
        );
        let args = match name {
            "mail_update_draft" => {
                json!({"draft_uuid":"draft-1","attachments":[{"filename":"new.txt","mime_type":"text/plain","base64_content":"YQ=="}]})
            }
            "mail_send_email" => {
                json!({"to":"person@example.org","subject":"Subject","body":"Body","attachments":[{"filename":"new.txt","mime_type":"text/plain","base64_content":"YQ=="}]})
            }
            _ => json!({"draft_uuid":"draft-1"}),
        };
        let out = h.call(name, args).await;
        assert_eq!(
            out["result"]["_meta"]["error_code"],
            if name == "mail_send_email" {
                "partial_mutation"
            } else {
                "provider_error"
            },
            "{out}"
        );
        if name == "mail_send_email" {
            assert_eq!(
                out["result"]["_meta"]["error_context"]["draft_uuid"],
                "draft-1"
            );
        }
        assert!(!out.to_string().contains("Private subject"));
        let mock = h.mock.lock().unwrap();
        assert!(
            !mock
                .calls
                .iter()
                .any(|c| c.0 == "PUT" || c.1.contains("/draft/attachment"))
        );
    }
}

#[tokio::test]
async fn draft_mutation_results_validate_identities_and_retain_known_drafts() {
    for (name, method, path, phase, args) in [
        (
            "mail_create_draft",
            "POST",
            "/mail/box-a/draft",
            "create_draft",
            json!({"to":"person@example.org","subject":"Subject","body":"Private body"}),
        ),
        (
            "mail_send_email",
            "PUT",
            "/mail/box-a/draft/draft-1",
            "send",
            json!({"to":"person@example.org","subject":"Subject","body":"Private body"}),
        ),
        (
            "mail_update_draft",
            "PUT",
            "/mail/box-a/draft/draft-1",
            "save_draft",
            json!({"draft_uuid":"draft-1","subject":"Subject"}),
        ),
        (
            "mail_send_draft",
            "PUT",
            "/mail/box-a/draft/draft-1",
            "send",
            json!({"draft_uuid":"draft-1"}),
        ),
        (
            "mail_delete_draft",
            "DELETE",
            "/mail/box-a/draft/draft-1",
            "delete_draft",
            json!({"draft_uuid":"draft-1"}),
        ),
    ] {
        let mut responses = vec![
            json!({"uuid":"other-draft","mailbox_uuid":"other-mailbox"}),
            json!({"uuid":"draft-1","mailbox_uuid":"other-mailbox"}),
            json!({"uuid":"../unsafe"}),
            json!({"uuid":null}),
            json!([]),
            json!(null),
            json!(true),
            json!("success"),
        ];
        if name != "mail_create_draft" {
            responses.push(json!({"uuid":"other-draft","mailbox_uuid":"box-a"}));
        }
        for fields in responses {
            let h = Harness::new().await;
            h.mock
                .lock()
                .unwrap()
                .responses
                .insert(format!("{method} {path}"), json!({"data":fields}));
            let out = h.call(name, args.clone()).await;
            assert_eq!(
                out["result"]["_meta"]["error_code"], "mutation_uncertain",
                "{name}: {out}"
            );
            let context = &out["result"]["_meta"]["error_context"];
            assert_eq!(context["phase"], phase, "{out}");
            if name != "mail_create_draft" {
                assert_eq!(context["draft_uuid"], "draft-1", "{out}");
            }
            if fields["uuid"] == "other-draft" || fields["uuid"] == "draft-1" {
                assert_eq!(context["returned_draft_uuid"], fields["uuid"], "{out}");
            } else {
                assert!(context["returned_draft_uuid"].is_null(), "{out}");
            }
            assert!(!out.to_string().contains("Private body"));
            assert_eq!(
                h.mock
                    .lock()
                    .unwrap()
                    .calls
                    .iter()
                    .filter(|c| c.0 == method && c.1.starts_with(&format!("{path}?")))
                    .count(),
                1
            );
        }
    }
}

#[tokio::test]
async fn draft_mutation_acknowledgements_may_omit_identities() {
    for name in ["mail_update_draft", "mail_send_draft", "mail_delete_draft"] {
        for fields in [
            json!({"uid":123,"mailbox_uuid":"box-a"}),
            json!({"uuid":"draft-1","mailbox_uuid":"box-a"}),
            json!({"result":"success"}),
        ] {
            let h = Harness::new().await;
            let method = if name == "mail_delete_draft" {
                "DELETE"
            } else {
                "PUT"
            };
            {
                let mut mock = h.mock.lock().unwrap();
                mock.responses.insert(
                    "GET /mail/box-a/draft/draft-1".into(),
                    json!({"data":{"uuid":"draft-1","mailbox_uuid":"box-a"}}),
                );
                mock.responses.insert(
                    format!("{method} /mail/box-a/draft/draft-1"),
                    json!({"data":fields}),
                );
            }
            let out = h.call(name, json!({"draft_uuid":"draft-1"})).await;
            assert_eq!(out["result"]["isError"], false, "{name}: {out}");
        }
    }
}

#[tokio::test]
async fn sent_emails_reuse_the_accepted_draft_payload_and_report_provider_rejections() {
    let h = Harness::new().await;
    let out = h
        .call(
            "mail_send_email",
            json!({"to":"person@example.org","subject":"Subject","body":"Private body"}),
        )
        .await;
    assert_eq!(out["result"]["isError"], false, "{out}");
    {
        let mock = h.mock.lock().unwrap();
        assert!(
            !mock
                .calls
                .iter()
                .any(|c| c.0 == "GET" && c.1.contains("/draft/"))
        );
        let created = &mock.calls.iter().find(|c| c.0 == "POST").unwrap().3;
        let sent = &mock.calls.iter().find(|c| c.0 == "PUT").unwrap().3;
        for key in ["subject", "body", "to", "from", "mime_type", "identity_id"] {
            assert_eq!(sent[key], created[key], "{key}");
        }
        assert_eq!(sent["action"], "send");
        assert_eq!(sent["uuid"], "draft-1");
        assert_eq!(sent["uid"], "1");
        assert_eq!(sent["resource"], "/api/mail/box-a/draft/draft-1");
    }
    let h = Harness::new().await;
    h.mock.lock().unwrap().queued_responses.insert(
        "PUT /mail/box-a/draft/draft-1".into(),
        [(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"result":"error","error":{"code":"validation_failed","description":"Private detail"}}),
        )]
        .into(),
    );
    let out = h
        .call(
            "mail_send_email",
            json!({"to":"person@example.org","subject":"Subject","body":"Private body"}),
        )
        .await;
    let context = &out["result"]["_meta"]["error_context"];
    assert_eq!(out["result"]["_meta"]["error_code"], "partial_mutation");
    assert_eq!(context["draft_uuid"], "draft-1");
    assert_eq!(context["progress"]["provider_status"], 422);
    assert_eq!(context["progress"]["provider_code"], "validation_failed");
    assert!(!out.to_string().contains("Private detail"));
    let outcome = h
        .app
        .store
        .with(|c| {
            Ok(c.query_row(
                "SELECT outcome FROM logs WHERE action='mail_send_email'",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .unwrap();
    assert_eq!(
        outcome,
        "partial_mutation (provider 422 validation_failed /mail/box-a/draft/{id})"
    );
}
#[tokio::test]
async fn upload_validates_exclusive_content_and_handles_empty_text() {
    let h = Harness::new().await;
    assert_eq!(
        h.call(
            "kdrive_upload_file",
            json!({"drive_id":9,"file_name":"x","text_content":"","base64_content":""})
        )
        .await["result"]["isError"],
        true
    );
    let out = h
        .call(
            "kdrive_upload_file",
            json!({"drive_id":9,"file_name":"x","text_content":""}),
        )
        .await;
    assert_eq!(out["result"]["isError"], false, "{out}");
    assert_eq!(result(&out)["data"]["id"], 43);
    assert_eq!(result(&out)["data"]["drive_id"], 9);
    assert_eq!(
        result(&out)["data"]["source_url"],
        "https://ksuite.infomaniak.com/all/kdrive/app/drive/9/files/1/preview/text/43"
    );
}
#[tokio::test]
async fn every_kdrive_listing_accepts_and_preserves_provider_continuation() {
    let h = Harness::new().await;
    for (name, path, filters) in [
        (
            "kdrive_list",
            "/3/drive/9/files/1/files",
            json!({"type":"file"}),
        ),
        (
            "kdrive_search",
            "/3/drive/9/files/search",
            json!({"query":"report","directory_id":5,"depth":"child"}),
        ),
        (
            "kdrive_recents",
            "/3/drive/9/files/recents",
            json!({"type":"file"}),
        ),
        (
            "kdrive_shared_with_me",
            "/3/drive/9/files/shared_with_me",
            json!({"type":"dir"}),
        ),
    ] {
        let key = format!("GET {path}");
        h.mock.lock().unwrap().responses.insert(key.clone(), json!({"data":[{"id":42,"type":"file","name":"First"}],"has_more":true,"cursor":"next+/="}));
        let mut args = filters.clone();
        args["drive_id"] = json!(9);
        let first = h.call(name, args.clone()).await;
        assert_eq!(first["result"]["isError"], false, "{name}: {first}");
        assert_eq!(result(&first)["cursor"], "next+/=");
        assert_eq!(result(&first)["has_more"], true);
        assert_eq!(result(&first)["data"][0]["drive_id"], 9);
        h.mock.lock().unwrap().responses.insert(key, json!({"data":[{"id":43,"type":"file","name":"Second"}],"has_more":false,"cursor":"terminal"}));
        args["cursor"] = result(&first)["cursor"].clone();
        let second = h.call(name, args).await;
        assert_eq!(second["result"]["isError"], false, "{second}");
        assert_eq!(result(&second)["has_more"], false);
        assert!(result(&second)["cursor"].is_null());
        let mock = h.mock.lock().unwrap();
        let call = mock.calls.last().unwrap();
        let query = url::form_urlencoded::parse(call.1.split_once('?').unwrap().1.as_bytes())
            .into_owned()
            .collect::<HashMap<_, _>>();
        assert_eq!(query["cursor"], "next+/=");
        assert_eq!(query["limit"], "50");
        if let Some(kind) = filters["type"].as_str() {
            assert_eq!(query["type[]"], kind);
        }
        if name == "kdrive_search" {
            assert_eq!(query["directory_id"], "5");
            assert_eq!(query["depth"], "child");
            assert_eq!(query["query"], "report");
        }
    }
}
#[tokio::test]
async fn kdrive_incomplete_pages_require_a_usable_cursor() {
    let h = Harness::new().await;
    for cursor in [Value::Null, json!(""), json!(42), json!("previous")] {
        h.mock.lock().unwrap().responses.insert(
            "GET /3/drive/9/files/1/files".into(),
            json!({"data":[],"has_more":true,"cursor":cursor}),
        );
        let v = h
            .call("kdrive_list", json!({"drive_id":9,"cursor":"previous"}))
            .await;
        assert_eq!(v["result"]["isError"], true, "{v}");
        assert_eq!(v["result"]["_meta"]["error_code"], "provider_error");
    }
}
#[tokio::test]
async fn drafts_have_working_offset_pagination_and_string_identifiers() {
    let h = Harness::new().await;
    let key = "GET /mail/box-a/folder/draft/message";
    h.mock.lock().unwrap().responses.insert(
        key.into(),
        json!({"data":{"threads":[
        {"uid":1,"draft_uuid":"draft-1","messages":[{"uid":12,"folder_id":5}]},
        {"uid":2,"draft_uuid":"draft-2","messages":[{"uid":"13@draft","folder_id":"draft"}]}
    ]},"has_more":true}),
    );
    let first = h.call("mail_list_drafts", json!({"limit":2})).await;
    assert_eq!(first["result"]["isError"], false, "{first}");
    assert_eq!(result(&first)["next_offset"], 2);
    assert_eq!(result(&first)["data"][0]["thread_uid"], "1");
    assert_eq!(result(&first)["data"][0]["first_message_uid"], "12");
    assert_eq!(result(&first)["data"][0]["folder_id"], "5");
    assert_eq!(result(&first)["data"][0]["draft_uuid"], "draft-1");
    h.mock.lock().unwrap().responses.insert(key.into(), json!({"data":{"threads":[{"uid":3,"draft_uuid":"draft-3","messages":[]}]},"has_more":false}));
    let last = h
        .call(
            "mail_list_drafts",
            json!({"limit":2,"offset":result(&first)["next_offset"]}),
        )
        .await;
    assert_eq!(last["result"]["isError"], false, "{last}");
    assert_eq!(result(&last)["has_more"], false);
    assert!(result(&last)["next_offset"].is_null());
    assert_eq!(result(&last)["data"][0]["draft_uuid"], "draft-3");
    let mock = h.mock.lock().unwrap();
    let query = &mock.calls.last().unwrap().1;
    assert!(query.contains("offset=2"));
    assert!(query.contains("limit=2"));
}
#[tokio::test]
async fn calendar_partial_update_preserves_provider_fields() {
    let h = Harness::new().await;
    let v = h
        .call(
            "calendar_update_event",
            json!({"event_id":"7","title":"New"}),
        )
        .await;
    assert_eq!(v["result"]["isError"], false, "{v}");
    let mock = h.mock.lock().unwrap();
    let body = &mock.calls.iter().find(|c| c.0 == "PUT").unwrap().3;
    assert_eq!(body["calendar_id"], "actual-calendar");
    assert_eq!(body["attendees"][0]["address"], "person@example.org");
    assert_eq!(body["alarms"][0]["id"], 1);
    assert_eq!(body["attachments"][0]["id"], 2);
    assert_eq!(body["bookable_resource_id"], "room-1");
    assert_eq!(
        result(&v)["context"]["source_url"],
        "https://ksuite.infomaniak.com/calendar"
    );
}
#[tokio::test]
async fn calendar_listing_never_collapses_distinct_instants_in_a_repeated_hour() {
    let h = Harness::new().await;
    for (from, to) in [
        ("2026-10-25T02:15:00+02:00", "2026-10-25T02:15:00+01:00"),
        ("2026-10-25T02:15:00+02:00", "2026-10-25T02:45:00+02:00"),
        ("2026-10-25T01:00:00+02:00", "2026-10-25T02:45:00+01:00"),
    ] {
        let out = h
            .call("calendar_list_events", json!({"from":from,"to":to}))
            .await;
        assert_eq!(
            out["result"]["_meta"]["error_code"], "provider_limitation",
            "{out}"
        );
    }
    assert!(
        !h.mock
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|c| c.1.starts_with("/1/calendar/pim/event?"))
    );
    let out = h
        .call(
            "calendar_list_events",
            json!({"from":"2026-10-25T01:00:00+02:00","to":"2026-10-25T04:00:00+01:00"}),
        )
        .await;
    assert_eq!(out["result"]["isError"], false, "{out}");
}
#[tokio::test]
async fn calendar_updates_interpret_local_inputs_in_profile_timezone() {
    let h = Harness::new().await;
    h.mock.lock().unwrap().responses.insert("GET /1/calendar/pim/event/7".into(), json!({"data":{
        "id":7,"calendar_id":"actual-calendar","start":"2026-10-17 10:00:00","end":"2026-10-17 11:00:00",
        "timezone_start":"America/New_York","timezone_end":"America/New_York","attendees":[],"alarms":[{"id":1}]
    }}));
    let v = h
        .call(
            "calendar_update_event",
            json!({"event_id":"7","start":"2026-10-17T10:00","end":"2026-10-17T11:00"}),
        )
        .await;
    assert_eq!(v["result"]["isError"], false, "{v}");
    let data = &result(&v)["data"];
    assert_eq!(data["start"], "2026-10-17 04:00:00");
    assert_eq!(data["end"], "2026-10-17 05:00:00");
    assert_eq!(data["timezone_start"], "America/New_York");
    assert_eq!(data["alarms"][0]["id"], 1);
    assert_eq!(
        result(&v)["context"]["notification_status"],
        "not_requested"
    );
}
#[tokio::test]
async fn malformed_messages_and_attachment_metadata_fail_explicitly() {
    for value in [
        json!([]),
        json!(false),
        json!({"uid":"12@inbox","attachments":[false]}),
    ] {
        let h = Harness::new().await;
        h.mock.lock().unwrap().responses.insert(
            "GET /mail/box-a/folder/inbox/message/12".into(),
            json!({"data":value}),
        );
        let out = h
            .call(
                "mail_read_email",
                json!({"folder_id":"inbox","message_id":"12"}),
            )
            .await;
        assert_eq!(
            out["result"]["_meta"]["error_code"], "provider_error",
            "{out}"
        );
    }
}
#[tokio::test]
async fn calendar_explicit_offsets_preserve_both_sides_of_the_repeated_hour() {
    let h = Harness::new().await;
    let v = h.call("calendar_create_event", json!({"title":"DST meeting","start":"2026-10-25T02:30:00+02:00","end":"2026-10-25T02:30:00+01:00"})).await;
    assert_eq!(v["result"]["isError"], false, "{v}");
    let data = &result(&v)["data"];
    assert_eq!(data["start"], "2026-10-25 00:30:00");
    assert_eq!(data["end"], "2026-10-25 01:30:00");
    assert_eq!(data["timezone_start"], "UTC");
    assert_eq!(data["timezone_end"], "UTC");
    h.mock.lock().unwrap().responses.insert("GET /1/calendar/pim/event/7".into(), json!({"data":{
        "id":7,"calendar_id":"actual-calendar","start":"2026-10-25 02:30:00","end":"2026-10-25 03:30:00",
        "timezone_start":"Europe/Zurich","timezone_end":"Europe/Zurich"
    }}));
    let v = h
        .call(
            "calendar_update_event",
            json!({"event_id":"7","title":"Only title"}),
        )
        .await;
    assert_eq!(v["result"]["isError"], false, "{v}");
    assert_eq!(result(&v)["data"]["start"], "2026-10-25 02:30:00");
    assert_eq!(result(&v)["data"]["timezone_start"], "Europe/Zurich");
    let v = h
        .call(
            "calendar_create_event",
            json!({"title":"Invalid","start":"2026-10-25T02:30:00","end":"2026-10-25T03:30:00"}),
        )
        .await;
    assert_eq!(v["result"]["isError"], true);
}
#[tokio::test]
async fn calendar_recurring_mutations_require_series_and_retain_calendar() {
    let h = Harness::new().await;
    h.mock.lock().unwrap().recurring = true;
    assert_eq!(
        h.call("calendar_delete_event", json!({"event_id":"7"}))
            .await["result"]["isError"],
        true
    );
    assert_eq!(
        h.call(
            "calendar_delete_event",
            json!({"event_id":"7","recurrence_scope":"occurrence"})
        )
        .await["result"]["isError"],
        true
    );
    assert_eq!(
        h.call(
            "calendar_delete_event",
            json!({"event_id":"7","recurrence_scope":"series","notify_attendees":true})
        )
        .await["result"]["isError"],
        false
    );
    let mock = h.mock.lock().unwrap();
    let call = mock.calls.iter().find(|c| c.0 == "DELETE").unwrap();
    assert!(call.1.contains("actual-calendar"));
    assert_eq!(call.3["notifyAttendees"], true);
}
#[tokio::test]
async fn invitations_and_bookable_room_names_are_returned() {
    let h = Harness::new().await;
    let v=h.call("calendar_create_event",json!({"title":"Meeting","start":"2026-10-17T08:00:00Z","end":"2026-10-17T09:00:00Z","attendees":"[\"person@example.org\"]"})).await;
    assert_eq!(v["result"]["isError"], false, "{v}");
    assert_eq!(result(&v)["data"]["notifyAttendees"], true);
    assert_eq!(result(&v)["data"]["attendees"][0]["state"], "NEEDS-ACTION");
    let event = h.call("calendar_get_event", json!({"event_id":"7"})).await;
    assert_eq!(result(&event)["data"]["bookable_resource_name"], "Studio");
    assert_eq!(
        result(&event)["context"]["source_url"],
        "https://ksuite.infomaniak.com/calendar"
    );
}
#[tokio::test]
async fn mail_bulk_actions_reject_cross_folder_uids_and_default_to_trash() {
    let h = Harness::new().await;
    assert_eq!(
        h.call(
            "mail_mark_email",
            json!({"folder_id":"inbox","message_ids":["12@other"],"read":true})
        )
        .await["result"]["isError"],
        true
    );
    let v = h
        .call(
            "mail_delete_email",
            json!({"folder_id":"inbox","message_ids":["12"]}),
        )
        .await;
    assert_eq!(v["result"]["isError"], false, "{v}");
    let mock = h.mock.lock().unwrap();
    let call = mock
        .calls
        .iter()
        .find(|c| c.1.contains("/message/move"))
        .unwrap();
    assert_eq!(call.3["to"], "trash");
    assert_eq!(call.3["uids"][0], "12@inbox");
}
#[tokio::test]
async fn provider_identifiers_cannot_traverse_api_paths() {
    let h = Harness::new().await;
    for id in [".", "..", "../draft", "%2e%2e", "a\\b"] {
        let out = h.call("mail_delete_folder", json!({"folder_id":id})).await;
        assert_eq!(out["error"]["code"], -32602, "{out}");
        let out = h
            .call("calendar_delete_event", json!({"event_id":id}))
            .await;
        assert_eq!(out["error"]["code"], -32602, "{out}");
    }
    assert!(h.mock.lock().unwrap().calls.is_empty());
}

#[tokio::test]
async fn malformed_mailbox_and_folder_identities_never_select_provider_paths() {
    for id in [
        json!(null),
        json!({}),
        json!(""),
        json!(".."),
        json!("../other"),
        json!("%2e%2e"),
        json!("unsafe\0id"),
    ] {
        let h = Harness::new().await;
        h.mock.lock().unwrap().responses.insert(
            "GET /mailbox".into(),
            json!({"data":[{"uuid":id,"email":"alice@team-ensemble.ch","is_primary":true}]}),
        );
        let out = h.call("mail_create_folder", json!({"name":"Work"})).await;
        assert_eq!(
            out["result"]["_meta"]["error_code"], "provider_error",
            "{out}"
        );
        assert_eq!(h.mock.lock().unwrap().calls.len(), 1);

        let h = Harness::new().await;
        h.mock.lock().unwrap().responses.insert(
            "GET /mail/box-a/folder".into(),
            json!({"data":[{"id":id,"name":"Drafts","role":"DRAFT"}]}),
        );
        let out = h.call("mail_list_drafts", json!({})).await;
        assert_eq!(
            out["result"]["_meta"]["error_code"], "provider_error",
            "{out}"
        );
        assert_eq!(h.mock.lock().unwrap().calls.len(), 2);
    }
}

#[tokio::test]
async fn mail_folder_creation_requires_a_usable_identity_and_retains_parent_context() {
    for value in [
        Value::Null,
        json!(false),
        json!([]),
        json!({}),
        json!({"id":{}}),
        json!({"id":".."}),
        json!({"id":"../other"}),
        json!({"id":"unsafe\0folder"}),
    ] {
        let h = Harness::new().await;
        h.mock
            .lock()
            .unwrap()
            .responses
            .insert("POST /mail/box-a/folder".into(), json!({"data":value}));
        let out = h
            .call(
                "mail_create_folder",
                json!({"name":"Private folder name","parent_folder_id":"inbox"}),
            )
            .await;
        assert_eq!(
            out["result"]["_meta"]["error_code"], "mutation_uncertain",
            "{out}"
        );
        let context = &out["result"]["_meta"]["error_context"];
        assert_eq!(context["mailbox_uuid"], "box-a");
        assert_eq!(context["parent_folder_id"], "inbox");
        assert_eq!(context["phase"], "create_folder");
        assert!(!out.to_string().contains("Private folder name"));
        assert_eq!(
            h.mock
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|c| c.0 != "GET")
                .count(),
            1
        );
    }
    let h = Harness::new().await;
    h.mock.lock().unwrap().responses.insert(
        "POST /mail/box-a/folder".into(),
        json!({"data":{"id":42,"name":"Work"}}),
    );
    let out = h
        .call(
            "mail_create_folder",
            json!({"name":"Work","parent_folder_id":"inbox"}),
        )
        .await;
    assert_eq!(out["result"]["isError"], false, "{out}");
    assert_eq!(result(&out)["data"]["id"], "42");
    assert_eq!(result(&out)["data"]["name"], "Work");
}

#[tokio::test]
async fn mail_reads_and_reply_sources_must_match_the_requested_message_and_folder() {
    for value in [
        json!({"uid":"13@inbox","msg_id":"<private@example.org>"}),
        json!({"uid":"12@other","msg_id":"<private@example.org>"}),
        json!({"uid":"12@inbox","folder_id":"other","msg_id":"<private@example.org>"}),
        json!({"uid":12,"folder":{"id":"other"},"msg_id":"<private@example.org>"}),
        json!({"uid":12,"mailbox_uuid":"other","msg_id":"<private@example.org>"}),
        json!({"uid":{},"msg_id":"<private@example.org>"}),
        json!({"msg_id":"<private@example.org>"}),
    ] {
        for (name, args) in [
            (
                "mail_read_email",
                json!({"folder_id":"inbox","message_id":"12"}),
            ),
            (
                "mail_send_email",
                json!({"to":"person@example.org","subject":"Private subject","body":"Private body","in_reply_to_uid":"12@inbox"}),
            ),
        ] {
            let h = Harness::new().await;
            h.mock.lock().unwrap().responses.insert(
                "GET /mail/box-a/folder/inbox/message/12".into(),
                json!({"data":value}),
            );
            let out = h.call(name, args).await;
            assert_eq!(
                out["result"]["_meta"]["error_code"], "provider_error",
                "{name}: {out}"
            );
            assert!(!out.to_string().contains("private@example.org"));
            assert!(h.mock.lock().unwrap().calls.iter().all(|c| c.0 == "GET"));
        }
    }
}

#[tokio::test]
async fn mail_reads_and_replies_preserve_numeric_and_scoped_message_identities() {
    for uid in [json!(12), json!("12"), json!("12@inbox")] {
        let h = Harness::new().await;
        h.mock.lock().unwrap().responses.insert(
            "GET /mail/box-a/folder/inbox/message/12".into(),
            json!({"data":{"uid":uid,"folder_id":"inbox","msg_id":"<source@example.org>","body":"Original body","headers":{"references":"<previous@example.org>"},"attachments":[{"part_id":1,"name":"report.txt"}]}}),
        );
        let out = h
            .call(
                "mail_read_email",
                json!({"folder_id":"inbox","message_id":"12"}),
            )
            .await;
        assert_eq!(out["result"]["isError"], false, "{out}");
        assert_eq!(
            result(&out)["data"]["uid"],
            if uid.is_number() { json!("12") } else { uid }
        );
        assert_eq!(result(&out)["data"]["folder_id"], "inbox");
        assert_eq!(result(&out)["data"]["body"], "Original body");
        assert_eq!(result(&out)["data"]["attachments"][0]["id"], "1");
        let out = h.call("mail_create_draft", json!({"to":"person@example.org","subject":"Reply","body":"Reply body","in_reply_to_uid":"12@inbox"})).await;
        assert_eq!(out["result"]["isError"], false, "{out}");
        let mock = h.mock.lock().unwrap();
        let draft = &mock.calls.iter().find(|c| c.0 == "POST").unwrap().3;
        assert_eq!(draft["in_reply_to"], "<source@example.org>");
        assert_eq!(
            draft["references"],
            "<previous@example.org> <source@example.org>"
        );
    }
}

#[tokio::test]
async fn sending_identity_lookup_rejects_invalid_provider_path_selectors() {
    for fields in [
        json!({"mailbox":".."}),
        json!({"mailbox":"../other"}),
        json!({"mailbox":"%2e%2e"}),
        json!({"mailbox":{}}),
        json!({"hosting_id":-1}),
        json!({"hosting_id":"../other"}),
    ] {
        let h = Harness::new().await;
        let mut mailbox = json!({"uuid":"box-a","email":"alice@team-ensemble.ch","hosting_id":5,"mailbox":"alice"});
        mailbox
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        h.mock
            .lock()
            .unwrap()
            .responses
            .insert("GET /mailbox".into(), json!({"data":[mailbox]}));
        let out = h
            .call(
                "mail_send_email",
                json!({"to":"person@example.org","subject":"Subject","body":"Body"}),
            )
            .await;
        assert_eq!(
            out["result"]["_meta"]["error_code"], "provider_error",
            "{out}"
        );
        assert_eq!(h.mock.lock().unwrap().calls.len(), 1);
    }
}

#[tokio::test]
async fn mail_search_keeps_address_filters_and_continuation() {
    let h = Harness::new().await;
    let mut args = json!({"from":"alice@example.org","to":"person@example.org","subject":"Review","since":"2026-10-01","before":"2026-10-31","limit":1});
    h.mock.lock().unwrap().responses.insert("GET /mail/box-a/folder/inbox/message".into(), json!({"data":{"threads":[{"uid":"thread-1","messages":[{"uid":"12@inbox","folder_id":"inbox"}]}]},"has_more":true}));
    let first = h.call("mail_search_emails", args.clone()).await;
    assert_eq!(first["result"]["isError"], false, "{first}");
    args["offset"] = result(&first)["next_offset"].clone();
    h.mock.lock().unwrap().responses.insert(
        "GET /mail/box-a/folder/inbox/message".into(),
        json!({"data":{"threads":[]},"has_more":false}),
    );
    let last = h.call("mail_search_emails", args).await;
    assert_eq!(last["result"]["isError"], false, "{last}");
    assert_eq!(result(&last)["has_more"], false);
    let mock = h.mock.lock().unwrap();
    let queries = mock
        .calls
        .iter()
        .filter(|c| c.1.starts_with("/mail/box-a/folder/inbox/message?"))
        .map(|c| {
            url::Url::parse(&format!("https://mock.example{}", c.1))
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect::<HashMap<_, _>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(queries.len(), 2);
    for query in &queries {
        for (key, value) in [
            ("sfrom", "alice@example.org"),
            ("sto", "person@example.org"),
            ("ssubject", "Review"),
            ("sfromdate", "2026-10-01 00:00:00"),
            ("stodate", "2026-10-31 00:00:00"),
            ("severywhere", "1"),
            ("thread", "off"),
            ("limit", "1"),
        ] {
            assert_eq!(query.get(key).map(String::as_str), Some(value));
        }
    }
    assert_eq!(queries[0]["offset"], "0");
    assert_eq!(queries[1]["offset"], "1");
}

#[tokio::test]
async fn mail_continuation_offsets_remain_valid_tool_inputs() {
    for (name, mut args, folder) in [
        ("mail_list_emails", json!({"folder_id":"inbox"}), "inbox"),
        ("mail_search_emails", json!({"query":"Review"}), "inbox"),
        ("mail_list_drafts", json!({}), "draft"),
    ] {
        let h = Harness::new().await;
        args["offset"] = json!(1000000);
        let key = format!("GET /mail/box-a/folder/{folder}/message");
        h.mock.lock().unwrap().responses.insert(key.clone(), json!({"data":{"threads":[{"uid":"thread-1","messages":[{"uid":12,"folder_id":folder}]}]},"has_more":true}));
        let out = h.call(name, args.clone()).await;
        assert_eq!(out["result"]["isError"], false, "{name}: {out}");
        args["offset"] = result(&out)["next_offset"].clone();
        assert_eq!(args["offset"], 1000001);
        h.mock
            .lock()
            .unwrap()
            .responses
            .insert(key, json!({"data":{"threads":[]},"has_more":false}));
        let next = h.call(name, args).await;
        assert_eq!(next["result"]["isError"], false, "{name}: {next}");
        assert_eq!(result(&next)["has_more"], false);
        assert!(result(&next)["next_offset"].is_null());
    }
}

#[tokio::test]
async fn mail_never_returns_an_unrepresentable_continuation_offset() {
    let h = Harness::new().await;
    h.mock.lock().unwrap().responses.insert("GET /mail/box-a/folder/inbox/message".into(), json!({"data":{"threads":[{"uid":"thread-1","messages":[{"uid":"12@inbox"}]}]},"has_more":true}));
    let out = h
        .call(
            "mail_list_emails",
            json!({"folder_id":"inbox","offset":9007199254740991_i64}),
        )
        .await;
    assert_eq!(
        out["result"]["_meta"]["error_code"], "provider_error",
        "{out}"
    );
    assert!(out["result"].get("structuredContent").is_none());
}

#[tokio::test]
async fn mail_lists_retain_thread_identity_and_search_requires_criteria() {
    let h = Harness::new().await;
    assert_eq!(
        h.call("mail_search_emails", json!({})).await["result"]["isError"],
        true
    );
    assert_eq!(
        h.call(
            "mail_list_emails",
            json!({"folder_id":"inbox","unseen":true,"flagged":true})
        )
        .await["result"]["isError"],
        true
    );
    let v = h
        .call("mail_list_emails", json!({"folder_id":"inbox"}))
        .await;
    assert_eq!(result(&v)["data"][0]["thread_uid"], "thread-1");
    assert_eq!(result(&v)["data"][0]["first_message_uid"], "12");
    assert_eq!(result(&v)["data"][0]["folder_id"], "inbox");
}

#[tokio::test]
async fn mail_search_results_keep_the_actual_folder_for_subsequent_reads() {
    for message in [
        json!({"uid":"12@sent"}),
        json!({"uid":12,"folder":{"id":"sent","name":"Sent"}}),
        json!({"uid":"12@sent","folder_id":"sent","folder":{"id":"sent"}}),
    ] {
        let h = Harness::new().await;
        {
            let mut mock = h.mock.lock().unwrap();
            mock.responses.insert(
                "GET /mail/box-a/folder/inbox/message".into(),
                json!({"data":{"threads":[{"uid":"thread-1","messages":[message]}]},"has_more":false}),
            );
            mock.responses.insert(
                "GET /mail/box-a/folder/sent/message/12".into(),
                json!({"data":{"uid":"12@sent","folder_id":"sent","mailbox_uuid":"box-a","body":"Sent message"}}),
            );
        }
        let out = h
            .call("mail_search_emails", json!({"query":"Review"}))
            .await;
        assert_eq!(out["result"]["isError"], false, "{out}");
        let summary = &result(&out)["data"][0];
        assert_eq!(summary["folder_id"], "sent", "{out}");
        assert_eq!(summary["first_message_uid"], "12", "{out}");
        let read = h.call("mail_read_email", json!({"mailbox_uuid":result(&out)["context"]["mailbox_uuid"],"folder_id":summary["folder_id"],"message_id":summary["first_message_uid"]})).await;
        assert_eq!(read["result"]["isError"], false, "{read}");
        assert_eq!(result(&read)["data"]["body"], "Sent message");
        assert!(
            h.mock
                .lock()
                .unwrap()
                .calls
                .iter()
                .any(|c| c.1.starts_with("/mail/box-a/folder/sent/message/12?"))
        );
    }
}

#[tokio::test]
async fn mail_listings_reject_missing_or_conflicting_result_identities() {
    for (name, args, thread) in [
        (
            "mail_search_emails",
            json!({"query":"Review"}),
            json!({"uid":"thread-1","messages":[]}),
        ),
        (
            "mail_search_emails",
            json!({"query":"Review"}),
            json!({"uid":"thread-1","messages":[{"uid":"12"}]}),
        ),
        (
            "mail_search_emails",
            json!({"query":"Review"}),
            json!({"uid":"thread-1","messages":[{"uid":"12@sent","folder_id":"inbox"}]}),
        ),
        (
            "mail_list_emails",
            json!({"folder_id":"inbox"}),
            json!({"uid":"thread-1","messages":[{"uid":"12@inbox","folder_id":"inbox","folder":{"id":"sent"}}]}),
        ),
        (
            "mail_list_emails",
            json!({"folder_id":"inbox"}),
            json!({"uid":"thread-1","messages":[{"uid":"12@inbox","mailbox_uuid":"other-mailbox"}]}),
        ),
        (
            "mail_list_emails",
            json!({"folder_id":"inbox"}),
            json!({"uid":"thread-1","mailbox_uuid":"other-mailbox","messages":[{"uid":"12@inbox"}]}),
        ),
        (
            "mail_list_emails",
            json!({"folder_id":"inbox"}),
            json!({"messages":[{"uid":"12@inbox"}]}),
        ),
        (
            "mail_list_drafts",
            json!({}),
            json!({"uid":"thread-1","draft_uuid":"../unsafe","messages":[]}),
        ),
        (
            "mail_list_emails",
            json!({"folder_id":"inbox"}),
            json!({"uid":"thread-1","messages":[{"uid":"12@sent@inbox"}]}),
        ),
        (
            "mail_list_emails",
            json!({"folder_id":"inbox"}),
            json!({"uid":"thread-1","messages":[{"uid":"12","folder_id":"../sent"}]}),
        ),
    ] {
        let h = Harness::new().await;
        let folder = if name == "mail_list_drafts" {
            "draft"
        } else {
            "inbox"
        };
        let mut thread = thread;
        thread["subject"] = json!("Private subject");
        h.mock.lock().unwrap().responses.insert(
            format!("GET /mail/box-a/folder/{folder}/message"),
            json!({"data":{"threads":[thread]},"has_more":false}),
        );
        let out = h.call(name, args).await;
        assert_eq!(
            out["result"]["_meta"]["error_code"], "provider_error",
            "{name}: {out}"
        );
        assert!(!out.to_string().contains("Private subject"));
    }
}
#[tokio::test]
async fn draft_update_fetches_provider_state_and_never_sends() {
    let h = Harness::new().await;
    let v = h
        .call(
            "mail_update_draft",
            json!({"draft_uuid":"draft-1","subject":"New title"}),
        )
        .await;
    assert_eq!(v["result"]["isError"], false, "{v}");
    let mock = h.mock.lock().unwrap();
    let call = mock.calls.iter().find(|c| c.0 == "PUT").unwrap();
    assert_eq!(call.3["action"], "save");
    assert_eq!(call.3["body"], "Original body");
    assert_eq!(call.3["attachments"][0], "existing");
    assert_eq!(call.3["to"][0]["email"], "old@example.org");
}
#[tokio::test]
async fn sending_identity_is_validated_and_html_is_escaped() {
    let h = Harness::new().await;
    assert_eq!(h.call("mail_create_draft",json!({"to":"b@example.org","subject":"hello","body":"x","from_email":"evil@example.org"})).await["error"],"access_denied");
    let v = h
        .call(
            "mail_create_draft",
            json!({"to":"b@example.org","subject":"hello","body":"<script>x</script>"}),
        )
        .await;
    assert_eq!(v["result"]["isError"], false, "{v}");
    assert_eq!(
        result(&v)["data"]["resolved_from"]["email"],
        "alice@team-ensemble.ch"
    );
    let mock = h.mock.lock().unwrap();
    let call = mock
        .calls
        .iter()
        .find(|c| c.0 == "POST" && c.1.contains("/draft?"))
        .unwrap();
    assert!(call.3["body"].as_str().unwrap().contains("&lt;script&gt;"));
}
#[tokio::test]
async fn operational_logs_exclude_secrets_and_content() {
    let h = Harness::new().await;
    h.call(
        "mail_create_draft",
        json!({"to":"b@example.org","subject":"Private subject","body":"Private body"}),
    )
    .await;
    let logs = h
        .app
        .store
        .with(|c| {
            let mut stmt = c.prepare("SELECT action||outcome FROM logs")?;
            Ok(stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
                .join(" "))
        })
        .unwrap();
    for secret in [
        "Private subject",
        "Private body",
        "delegated-a",
        "provider-secret",
        "mcp-access",
    ] {
        assert!(!logs.contains(secret));
    }
}
#[tokio::test]
async fn refresh_is_automatic_and_serialized_per_user_connection() {
    let h = Harness::new().await;
    credential(&h.app, 1, "expired", now() - 1);
    let (a, b) = tokio::join!(
        h.call("kdrive_info", json!({"drive_id":9,"file_id":42})),
        h.call("kdrive_info", json!({"drive_id":9,"file_id":42}))
    );
    assert_eq!(a["result"]["isError"], false, "{a}");
    assert_eq!(b["result"]["isError"], false, "{b}");
    assert_eq!(h.mock.lock().unwrap().refreshes, 1);
}
#[tokio::test]
async fn client_revocation_during_provider_renewal_prevents_dispatch() {
    let h = Harness::new().await;
    credential(&h.app, h.user.id, "expired", now() - 1);
    let started = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    h.mock.lock().unwrap().refresh_gate = Some((started.clone(), resume.clone()));
    let app = h.app.clone();
    let token = h.token.clone();
    let operation = tokio::spawn(async move {
        request(&app, "POST", "/mcp", json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"kdrive_upload_file","arguments":{"drive_id":9,"file_name":"private.txt","text_content":"private content"}}}), &[
            ("authorization",format!("Bearer {token}")),
            ("accept","application/json, text/event-stream".into()),
        ]).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), started.notified())
        .await
        .unwrap();
    let revoked = request(
        &h.app,
        "POST",
        "/api/clients/grant-a/revoke",
        json!({}),
        &h.headers(),
    )
    .await;
    assert_eq!(revoked.status, 200);
    resume.notify_one();
    let reply = operation.await.unwrap();
    assert_eq!(reply.status, 401);
    assert_eq!(reply.value["error"], "invalid_token");
    assert!(
        !h.mock
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|call| call.1.contains("/upload"))
    );
}

fn cookies(headers: &HeaderMap) -> String {
    headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|c| c.to_str().ok()?.split(';').next().map(String::from))
        .collect::<Vec<_>>()
        .join("; ")
}
async fn login_callback(h: &Harness, overrides: Value) -> Reply {
    let start = request(&h.app, "GET", "/auth/login", Value::Null, &[]).await;
    assert_eq!(start.status, 303);
    let url = url::Url::parse(start.headers["location"].to_str().unwrap()).unwrap();
    let query = url
        .query_pairs()
        .into_owned()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(query["scope"], "openid profile email");
    assert_eq!(query["code_challenge_method"], "S256");
    {
        let mut mock = h.mock.lock().unwrap();
        mock.nonce = query["nonce"].clone();
        mock.claims = overrides;
    }
    request(
        &h.app,
        "GET",
        &format!(
            "/auth/infomaniak/callback?state={}&code=provider-code",
            query["state"]
        ),
        Value::Null,
        &[("cookie", cookies(&start.headers))],
    )
    .await
}
#[tokio::test]
async fn browser_login_validates_oidc_and_rotates_session() {
    let h = Harness::new().await;
    let callback = login_callback(&h, json!({})).await;
    assert_eq!(callback.status, 303, "{}", callback.value);
    let session = cookies(&callback.headers);
    assert!(session.contains("__Host-mcp_session="));
    let me = request(
        &h.app,
        "GET",
        "/api/me",
        Value::Null,
        &[("cookie", session)],
    )
    .await;
    assert_eq!(me.status, 200);
    assert_eq!(me.value["user"]["id"], h.user.id);
    let call = h
        .mock
        .lock()
        .unwrap()
        .calls
        .iter()
        .find(|c| c.1.starts_with("/token?"))
        .unwrap()
        .clone();
    assert!(call.3.as_str().unwrap().contains("code_verifier="));
}
#[tokio::test]
async fn oidc_claim_failures_never_create_trusted_sessions() {
    for overrides in [
        json!({"email_verified":false}),
        json!({"email":"a@evil.example"}),
        json!({"nonce":"forged"}),
        json!({"iss":"https://evil.example"}),
        json!({"aud":"another-client"}),
        json!({"exp":now()-3600}),
        json!({"iat":now()+3600}),
        json!({"iat":null}),
        json!({"sub":""}),
        json!({"aud":["provider-client","second"],"azp":"second"}),
    ] {
        let h = Harness::new().await;
        let response = login_callback(&h, overrides.clone()).await;
        assert!(
            !response.status.is_success() && !response.status.is_redirection(),
            "Accepted {overrides}"
        );
        assert!(!cookies(&response.headers).contains("__Host-mcp_session="));
    }
}
#[tokio::test]
async fn failed_browser_sign_in_reports_safe_reasons_without_creating_sessions() {
    for (failure, expected) in [
        ("cookie", "flow_cookie_missing"),
        ("expired", "flow_invalid_or_expired"),
        ("binding", "flow_invalid_or_expired"),
        ("email", "email_not_verified"),
        ("nonce", "nonce_mismatch"),
        ("jwt", "id_token_invalid"),
    ] {
        let h = Harness::new().await;
        let start = request(&h.app, "GET", "/auth/login", Value::Null, &[]).await;
        let url = url::Url::parse(start.headers["location"].to_str().unwrap()).unwrap();
        let query = url.query_pairs().into_owned().collect::<HashMap<_, _>>();
        {
            let mut mock = h.mock.lock().unwrap();
            mock.nonce = query["nonce"].clone();
            mock.claims = match failure {
                "email" => json!({"email_verified":false}),
                "nonce" => json!({"nonce":"private-wrong-nonce"}),
                "jwt" => json!({"aud":"private-wrong-audience"}),
                _ => json!({}),
            };
        }
        if failure == "expired" {
            h.app
                .store
                .with(|c| {
                    c.execute(
                        "UPDATE flows SET expires=? WHERE hash=?",
                        params![now() - 1, crypto::hash(&query["state"])],
                    )?;
                    Ok(())
                })
                .unwrap();
        }
        let headers = match failure {
            "cookie" => vec![],
            "binding" => vec![("cookie", "__Host-mcp_flow=private-wrong-binding".into())],
            _ => vec![("cookie", cookies(&start.headers))],
        };
        let before = h
            .app
            .store
            .with(|c| Ok(c.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0))?))
            .unwrap();
        let response = request(
            &h.app,
            "GET",
            &format!(
                "/auth/infomaniak/callback?state={}&code=private-provider-code",
                query["state"]
            ),
            Value::Null,
            &headers,
        )
        .await;
        assert_eq!(response.status, 401);
        assert_eq!(response.value["error"], "invalid_token");
        assert_eq!(
            response.value["context"]["authentication_failure"],
            expected
        );
        assert!(!cookies(&response.headers).contains("__Host-mcp_session="));
        let (after, outcome, correlation, logs) = h.app.store.with(|c| {
            let after = c.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0))?;
            let (outcome,correlation) = c.query_row("SELECT outcome,request_id FROM logs WHERE action='authentication.callback' ORDER BY id DESC LIMIT 1", [], |r| Ok((r.get::<_, String>(0)?,r.get::<_, String>(1)?)))?;
            let logs = c.query_row("SELECT group_concat(action||outcome||request_id) FROM logs", [], |r| r.get::<_, String>(0))?;
            Ok((after,outcome,correlation,logs))
        }).unwrap();
        assert_eq!(before, after);
        assert_eq!(outcome, format!("rejected {expected}"));
        assert_eq!(
            correlation,
            response.headers["x-request-id"].to_str().unwrap()
        );
        let diagnostic = format!("{} {logs}", response.value);
        for secret in [
            &query["state"],
            &query["nonce"],
            "private-provider-code",
            "private-wrong-nonce",
            "private-wrong-audience",
            "private-wrong-binding",
            "delegated-a",
            "refresh-a",
            "alice@team-ensemble.ch",
        ] {
            assert!(!diagnostic.contains(secret), "Leaked {secret}");
        }
        if matches!(failure, "cookie" | "expired" | "binding") {
            assert!(
                !h.mock
                    .lock()
                    .unwrap()
                    .calls
                    .iter()
                    .any(|call| call.1.starts_with("/token?"))
            );
        }
    }
}
#[tokio::test]
async fn oauth_state_is_cookie_bound_and_single_use() {
    let h = Harness::new().await;
    let start = request(&h.app, "GET", "/auth/login", Value::Null, &[]).await;
    let url = url::Url::parse(start.headers["location"].to_str().unwrap()).unwrap();
    let query = url
        .query_pairs()
        .into_owned()
        .collect::<std::collections::HashMap<_, _>>();
    h.mock.lock().unwrap().nonce = query["nonce"].clone();
    let callback = format!(
        "/auth/infomaniak/callback?state={}&code=code",
        query["state"]
    );
    assert_eq!(
        request(
            &h.app,
            "GET",
            &callback,
            Value::Null,
            &[("cookie", "__Host-mcp_flow=wrong-browser".into())]
        )
        .await
        .status,
        401
    );
    assert_eq!(
        request(
            &h.app,
            "GET",
            &callback,
            Value::Null,
            &[("cookie", cookies(&start.headers))]
        )
        .await
        .status,
        303
    );
    assert_eq!(
        request(
            &h.app,
            "GET",
            &callback,
            Value::Null,
            &[("cookie", cookies(&start.headers))]
        )
        .await
        .status,
        401
    );
}
async fn authorization_code(h: &Harness) -> (String, String) {
    let registration=request(&h.app,"POST","/register",json!({"client_name":"Assistant","redirect_uris":["https://client.example/callback"],"token_endpoint_auth_method":"none"}),&[]).await;
    assert_eq!(registration.status, 201);
    let client = registration.value["client_id"]
        .as_str()
        .unwrap()
        .to_string();
    let verifier = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMN";
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.extend_pairs([
        ("client_id", client.as_str()),
        ("redirect_uri", "https://client.example/callback"),
        ("response_type", "code"),
        ("state", "client-state"),
        ("code_challenge", &crypto::hash(verifier)),
        ("code_challenge_method", "S256"),
        ("scope", "kdrive:read"),
        ("resource", &h.app.config.resource()),
    ]);
    let start = request(
        &h.app,
        "GET",
        &format!("/authorize?{}", query.finish()),
        Value::Null,
        &[],
    )
    .await;
    assert_eq!(start.status, 303, "{}", start.value);
    let loc = start.headers["location"].to_str().unwrap();
    let flow = loc.strip_prefix("/?authorize=").unwrap();
    let mut headers = h.headers();
    headers[0]
        .1
        .push_str(&format!("; {}", cookies(&start.headers)));
    let consent = request(
        &h.app,
        "POST",
        "/api/authorize",
        json!({"flow":flow,"approve":true,"scopes":["kdrive:read"]}),
        &headers,
    )
    .await;
    assert_eq!(consent.status, 200, "{}", consent.value);
    let redirect = url::Url::parse(consent.value["redirect"].as_str().unwrap()).unwrap();
    let values = redirect
        .query_pairs()
        .into_owned()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(values["state"], "client-state");
    assert_eq!(values["iss"], h.app.config.public_url);
    (client, values["code"].clone())
}
async fn exchange(h: &Harness, client: &str, code: &str, verifier: &str) -> Reply {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("grant_type", "authorization_code"),
            ("client_id", client),
            ("resource", &h.app.config.resource()),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", "https://client.example/callback"),
        ])
        .finish();
    raw_request(
        &h.app,
        "POST",
        "/token",
        body,
        "application/x-www-form-urlencoded",
        &[],
    )
    .await
}
async fn refresh(h: &Harness, client: &str, token: &str) -> Reply {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("grant_type", "refresh_token"),
            ("client_id", client),
            ("resource", &h.app.config.resource()),
            ("refresh_token", token),
        ])
        .finish();
    raw_request(
        &h.app,
        "POST",
        "/token",
        body,
        "application/x-www-form-urlencoded",
        &[],
    )
    .await
}
#[tokio::test]
async fn client_oauth_pkce_consent_refresh_rotation_and_replay_work_end_to_end() {
    let h = Harness::new().await;
    let (client, code) = authorization_code(&h).await;
    assert_eq!(
        exchange(
            &h,
            &client,
            &code,
            "incorrect-verifier-that-is-long-enough-for-validation"
        )
        .await
        .status,
        400
    );
    let issued = exchange(
        &h,
        &client,
        &code,
        "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMN",
    )
    .await;
    assert_eq!(issued.status, 200, "{}", issued.value);
    assert_eq!(issued.value["scope"], "kdrive:read");
    assert_eq!(
        exchange(
            &h,
            &client,
            &code,
            "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMN"
        )
        .await
        .status,
        400
    );
    let token = issued.value["access_token"].as_str().unwrap();
    let rt = issued.value["refresh_token"].as_str().unwrap();
    let tools = request(
        &h.app,
        "POST",
        "/mcp",
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        &[
            ("authorization", format!("Bearer {token}")),
            ("accept", "application/json, text/event-stream".into()),
        ],
    )
    .await;
    assert!(
        tools.value["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["name"].as_str().unwrap().starts_with("kdrive_")
                && t["annotations"]["readOnlyHint"] == true)
    );
    let renewed = refresh(&h, &client, rt).await;
    assert_eq!(renewed.status, 200);
    assert_ne!(renewed.value["refresh_token"], rt);
    assert_eq!(refresh(&h, &client, rt).await.status, 400);
    let new_access = renewed.value["access_token"].as_str().unwrap();
    assert_eq!(
        request(
            &h.app,
            "POST",
            "/mcp",
            json!({}),
            &[("authorization", format!("Bearer {new_access}"))]
        )
        .await
        .status,
        401
    );
}
#[tokio::test]
async fn redirects_and_registration_never_allow_unsafe_targets() {
    let h = Harness::new().await;
    for target in [
        "http://client.example/cb",
        "https://user:pass@client.example/cb",
        "javascript:alert(1)",
        "https://client.example/cb#fragment",
    ] {
        assert_eq!(
            request(
                &h.app,
                "POST",
                "/register",
                json!({"redirect_uris":[target]}),
                &[]
            )
            .await
            .status,
            400
        );
    }
    assert!(oauth::valid_redirect("http://127.0.0.1:1234/cb"));
    assert!(oauth::valid_redirect(
        "https://claude.ai/api/mcp/auth_callback"
    ));
}
#[tokio::test]
async fn provider_outages_never_retry_mutations_or_disable_other_modules() {
    let h = Harness::new().await;
    h.mock.lock().unwrap().fail = true;
    let v = h
        .call("kdrive_delete", json!({"drive_id":9,"file_id":42}))
        .await;
    assert_eq!(v["result"]["isError"], true);
    assert!(!v.to_string().contains("sensitive upstream error body"));
    assert_eq!(h.mock.lock().unwrap().calls.len(), 1);
    let health: String = h
        .app
        .store
        .with(|c| {
            Ok(
                c.query_row("SELECT health FROM connections WHERE user_id=1", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap();
    // A service outage never marks the shared credential as unhealthy.
    assert_eq!(health, "connected");
    assert_eq!(
        h.rpc("tools/list", json!({})).await.value["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        55
    );
}
#[tokio::test]
async fn sqlite_logs_sessions_and_credentials_survive_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("persistent.sqlite");
    let store = Store::open(path.to_str().unwrap()).unwrap();
    let user = store
        .admit(&cfg(), ISSUER, "persistent", "p@team-ensemble.ch", "P")
        .unwrap();
    store
        .audit("request", Some(user.id), None, "test.persist", "ok")
        .unwrap();
    let secret = crypto::seal(
        &cfg().encryption_key,
        "credential:1:infomaniak",
        b"persistent-secret",
    )
    .unwrap();
    store
        .with(|c| {
            c.execute("INSERT INTO modules(module) VALUES('mail')", [])?;
            c.execute(
                "INSERT INTO connections(user_id,provider,secret,scopes,expires) VALUES(1,'infomaniak',?,'*',?)",
                params![secret, now() + 3600],
            )?;
            c.execute(
                "INSERT INTO sessions VALUES('hashed-session',1,'csrf',?)",
                [now() + 3600],
            )?;
            Ok(())
        })
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(format!("{}-wal", path.display()))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    drop(store);
    let reopened = Store::open(path.to_str().unwrap()).unwrap();
    assert_eq!(reopened.user(user.id).unwrap().subject, "persistent");
    let count: i64 = reopened
        .with(|c| Ok(c.query_row("SELECT COUNT(*) FROM logs", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(count, 1);
    reopened
        .with(|c| {
            let secret: Vec<u8> =
                c.query_row("SELECT secret FROM connections WHERE user_id=1", [], |r| {
                    r.get(0)
                })?;
            assert_eq!(
                crypto::open(&cfg().encryption_key, "credential:1:infomaniak", &secret).unwrap(),
                b"persistent-secret"
            );
            assert_eq!(
                c.query_row(
                    "SELECT user_id FROM sessions WHERE hash='hashed-session'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn administrators_see_per_client_effective_access_without_credentials() {
    let h = Harness::new().await;
    h.sql("UPDATE users SET admin=1; UPDATE modules SET enabled=0 WHERE module='mail'");
    let out = request(&h.app, "GET", "/api/admin", Value::Null, &h.headers()).await;
    assert_eq!(out.status, 200);
    let scopes = out.value["clients"][0]["effective_scopes"]
        .as_array()
        .unwrap();
    assert_eq!(scopes.len(), 6);
    assert!(
        !scopes
            .iter()
            .any(|s| s.as_str().unwrap().starts_with("mail:"))
    );
    assert!(!out.value.to_string().contains("delegated-a"));
    assert!(!out.value.to_string().contains("refresh-token"));
}
#[tokio::test]
async fn changed_domain_allowlist_invalidates_existing_sessions_and_tokens() {
    let h = Harness::new().await;
    let mut app = h.app.clone();
    let mut config = (*app.config).clone();
    config.allowed_domains = ["elsewhere.example".into()].into_iter().collect();
    app.config = Arc::new(config);
    assert_eq!(
        request(&app, "GET", "/api/me", Value::Null, &h.headers())
            .await
            .status,
        403
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/mcp",
            json!({}),
            &[("authorization", format!("Bearer {}", h.token))]
        )
        .await
        .status,
        403
    );
}
#[tokio::test]
async fn newly_verified_ineligible_email_invalidates_old_sessions_and_other_services() {
    for refresh in [false, true] {
        let h = Harness::new().await;
        if refresh {
            credential(&h.app, h.user.id, "expired", now() - 1);
            h.mock.lock().unwrap().claims = json!({"email":"alice@outside.example"});
            let denied = h
                .rpc(
                    "tools/call",
                    json!({"name":"kdrive_info","arguments":{"drive_id":9,"file_id":42}}),
                )
                .await;
            assert_eq!(denied.status, 403, "{}", denied.value);
            assert!(
                !h.mock
                    .lock()
                    .unwrap()
                    .calls
                    .iter()
                    .any(|c| c.1.contains("/files/"))
            );
        } else {
            assert_eq!(
                login_callback(&h, json!({"email":"alice@outside.example"}))
                    .await
                    .status,
                403
            );
        }
        assert_eq!(
            h.app.store.user(h.user.id).unwrap().email,
            "alice@outside.example"
        );
        assert_eq!(
            request(&h.app, "GET", "/api/me", Value::Null, &h.headers())
                .await
                .status,
            403
        );
        assert_eq!(h.rpc("tools/list", json!({})).await.status, 403);
        assert_eq!(
            h.rpc("tools/call", json!({"name":"mail_list_mailboxes"}))
                .await
                .status,
            403
        );
        assert_eq!(h.app.store.user(h.user.id).unwrap().subject, h.user.subject);
    }
}
#[tokio::test]
async fn renewal_updates_only_validated_claims_for_the_same_identity() {
    for (claims, expected_email) in [
        (
            json!({"email":"new@team-ensemble.ch"}),
            "new@team-ensemble.ch",
        ),
        (
            json!({"email":"new@team-ensemble.ch","email_verified":false}),
            "alice@team-ensemble.ch",
        ),
        (
            json!({"email":"new@team-ensemble.ch","sub":"other-employee"}),
            "alice@team-ensemble.ch",
        ),
    ] {
        let h = Harness::new().await;
        credential(&h.app, h.user.id, "expired", now() - 1);
        h.mock.lock().unwrap().claims = claims;
        h.rpc(
            "tools/call",
            json!({"name":"kdrive_info","arguments":{"drive_id":9,"file_id":42}}),
        )
        .await;
        assert_eq!(h.app.store.user(h.user.id).unwrap().email, expected_email);
    }
}
#[tokio::test]
async fn logs_have_retention_and_wrong_encryption_keys_fail_at_startup() {
    let h = Harness::new().await;
    h.app.store.with(|c|{
        let tx=c.transaction()?;for _ in 0..10010{tx.execute("INSERT INTO logs(at,severity,request_id,action,outcome) VALUES(?,'info','test','test','ok')",[now()])?;}tx.commit()?;Ok(())
    }).unwrap();
    h.app
        .store
        .audit("bound", None, None, "test.retention", "ok")
        .unwrap();
    assert_eq!(
        h.app
            .store
            .with(|c| Ok(c.query_row("SELECT COUNT(*) FROM logs", [], |r| r.get::<_, i64>(0))?))
            .unwrap(),
        10000
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("key.sqlite");
    let provider = || {
        Provider::with_endpoints(
            h.app.provider.metadata.clone(),
            Provider::http_client().unwrap(),
            String::new(),
            String::new(),
        )
    };
    let app = App::new(
        cfg(),
        Store::open(path.to_str().unwrap()).unwrap(),
        provider(),
    )
    .unwrap();
    drop(app);
    let mut wrong = cfg();
    wrong.encryption_key = [1; 32];
    assert!(
        App::new(
            wrong,
            Store::open(path.to_str().unwrap()).unwrap(),
            provider()
        )
        .is_err()
    );
}
#[tokio::test]
async fn registered_module_http_routes_cannot_bypass_shared_access() {
    let h = Harness::new().await;
    let mut module = modules::mail::module();
    module.routes.push(modules::HttpRoute {
        path: "/modules/mail/status",
        write: false,
        handler: axum::routing::get(|| async { Json(json!({"ok":true})) }),
    });
    let mut app = h.app.clone();
    app.modules = Arc::new(modules::Registry::new(vec![module]).unwrap());
    assert_eq!(
        request(&app, "GET", "/modules/mail/status", Value::Null, &[])
            .await
            .status,
        401
    );
    assert_eq!(
        request(
            &app,
            "GET",
            "/modules/mail/status",
            Value::Null,
            &h.headers()
        )
        .await
        .status,
        200
    );
    h.sql("UPDATE modules SET enabled=0 WHERE module='mail'");
    assert_eq!(
        request(
            &app,
            "GET",
            "/modules/mail/status",
            Value::Null,
            &h.headers()
        )
        .await
        .status,
        403
    );
}

#[tokio::test]
async fn module_http_routes_recheck_browser_logout_after_credential_renewal() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let h = Harness::new().await;
    let invoked = Arc::new(AtomicUsize::new(0));
    let handled = invoked.clone();
    let mut module = modules::mail::module();
    module.routes.push(modules::HttpRoute {
        path: "/modules/mail/status",
        write: false,
        handler: axum::routing::get(move || {
            let handled = handled.clone();
            async move {
                handled.fetch_add(1, Ordering::SeqCst);
                Json(json!({"ok":true}))
            }
        }),
    });
    let mut app = h.app.clone();
    app.modules = Arc::new(modules::Registry::new(vec![module]).unwrap());
    credential(&app, h.user.id, "expired", now() - 1);
    let started = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    h.mock.lock().unwrap().refresh_gate = Some((started.clone(), resume.clone()));
    let running_app = app.clone();
    let headers = h.headers();
    let operation = tokio::spawn(async move {
        request(
            &running_app,
            "GET",
            "/modules/mail/status",
            Value::Null,
            &headers,
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), started.notified())
        .await
        .unwrap();
    assert_eq!(
        request(&app, "POST", "/api/logout", json!({}), &h.headers())
            .await
            .status,
        200
    );
    resume.notify_one();
    let denied = operation.await.unwrap();
    assert_eq!(denied.status, 401, "{}", denied.value);
    assert_eq!(denied.value["error"], "invalid_token");
    assert_eq!(invoked.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn registered_modules_supply_oauth_scopes_without_kernel_edits() {
    let h = Harness::new().await;
    let mut registered = modules::Registry::builtin().unwrap().modules;
    registered.push(modules::Module {
        id: "notes",
        name: "Notes",
        namespace: "notes",
        provider: modules::INFOMANIAK,
        scopes: vec!["notes"],
        tools: vec![],
        routes: vec![],
        resources: None,
        handler: |_, _| Box::pin(async { Ok(json!({})) }),
    });
    let mut app = h.app.clone();
    app.modules = Arc::new(modules::Registry::new(registered).unwrap());
    h.sql("INSERT INTO modules(module,enabled,verification) VALUES('notes',1,'mock'); INSERT INTO policies(kind,principal,module,level) VALUES('user',1,'notes',2)");
    credential(&app, h.user.id, "notes-token", now() + 3600);
    let scopes = oauth::scopes(&app);
    assert_eq!(scopes.len(), 10);
    for path in [
        "/.well-known/oauth-authorization-server",
        "/.well-known/oauth-protected-resource",
    ] {
        let metadata = request(&app, "GET", path, Value::Null, &[]).await;
        assert_eq!(metadata.value["scopes_supported"], json!(scopes));
    }
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("client_id", "client-a".into()),
            ("redirect_uri", "https://client.example/callback".into()),
            ("response_type", "code".into()),
            ("code_challenge_method", "S256".into()),
            ("code_challenge", crypto::hash(&crypto::random())),
            ("resource", app.config.resource()),
            ("scope", scopes.join(" ")),
        ])
        .finish();
    let authorization = request(
        &app,
        "GET",
        &format!("/authorize?{query}"),
        Value::Null,
        &[],
    )
    .await;
    assert_eq!(authorization.status, 303, "{}", authorization.value);
    let location = authorization.headers["location"].to_str().unwrap();
    let flow = location.strip_prefix("/?authorize=").unwrap();
    let mut headers = h.headers();
    headers[0]
        .1
        .push_str(&format!("; {}", cookies(&authorization.headers)));
    let consent = request(
        &app,
        "POST",
        "/api/authorize",
        json!({"flow":flow,"approve":true,"scopes":scopes}),
        &headers,
    )
    .await;
    assert_eq!(consent.status, 200, "{}", consent.value);
    assert!(
        consent.value["redirect"]
            .as_str()
            .unwrap()
            .contains("code=")
    );
}

const KCHAT: &str = "/kchat/ensemble/api/v4";

#[tokio::test]
async fn kchat_reads_use_the_configured_team_and_validate_identities() {
    let h = Harness::new().await;
    let out = h.call("kchat_list_channels", json!({"limit":1})).await;
    let result = &out["result"]["structuredContent"];
    assert_eq!(result["data"][0]["id"], "chan1", "{out}");
    assert_eq!(result["has_more"], true);
    assert_eq!(result["context"]["next_page"], 1);
    assert_eq!(
        result["context"]["source_url"],
        "https://ensemble.kchat.infomaniak.com/ensemble"
    );
    let call = h.mock.lock().unwrap().calls.last().unwrap().clone();
    assert_eq!(
        call.1,
        format!("{KCHAT}/teams/team1/channels?page=0&per_page=1")
    );
    assert_eq!(call.2, "Bearer delegated-a");
    let out = h.call("kchat_list_my_channels", json!({})).await;
    assert_eq!(
        out["result"]["structuredContent"]["data"][1]["type"], "D",
        "{out}"
    );
    let out = h
        .call("kchat_get_channel_history", json!({"channel_id":"chan1"}))
        .await;
    let result = &out["result"]["structuredContent"];
    assert_eq!(result["data"][0]["id"], "post2", "{out}");
    assert_eq!(
        result["data"][0]["source_url"],
        "https://ensemble.kchat.infomaniak.com/ensemble/pl/post2"
    );
    assert_eq!(result["has_more"], false);
    assert!(result["context"]["next_page"].is_null());
    let out = h
        .call("kchat_get_thread_replies", json!({"thread_id":"post1"}))
        .await;
    let result = &out["result"]["structuredContent"];
    assert_eq!(result["data"][0]["id"], "post1", "{out}");
    assert_eq!(result["data"][1]["root_id"], "post1");
    assert_eq!(result["context"]["channel_id"], "chan1");
    let out = h.call("kchat_get_users", json!({})).await;
    assert_eq!(
        out["result"]["structuredContent"]["data"][1]["username"], "bob",
        "{out}"
    );
    let out = h
        .call("kchat_get_user_profile", json!({"user_id":"bob1"}))
        .await;
    assert_eq!(
        out["result"]["structuredContent"]["data"]["id"], "bob1",
        "{out}"
    );
    for (endpoint, tool, args, invalid) in [
        (
            "GET /users/bob1",
            "kchat_get_user_profile",
            json!({"user_id":"bob1"}),
            json!({"id":"other1"}),
        ),
        (
            "GET /teams/name/ensemble",
            "kchat_list_channels",
            json!({}),
            json!({"id":"team1","name":"other"}),
        ),
        (
            "GET /teams/team1/channels",
            "kchat_list_channels",
            json!({}),
            json!([{"id":"chan9","team_id":"other"}]),
        ),
        (
            "GET /channels/chan1/posts",
            "kchat_get_channel_history",
            json!({"channel_id":"chan1"}),
            json!({"order":["post9"],"posts":{"post9":{"id":"post9","channel_id":"other"}}}),
        ),
        (
            "GET /posts/post1/thread",
            "kchat_get_thread_replies",
            json!({"thread_id":"post1"}),
            json!({"order":["post9"],"posts":{"post9":{"id":"post9","channel_id":"chan1","root_id":"other"}}}),
        ),
        (
            "GET /posts/post1/thread",
            "kchat_get_thread_replies",
            json!({"thread_id":"post1"}),
            json!({"order":[],"posts":{},"has_next":true}),
        ),
    ] {
        let (method, path) = endpoint.split_once(' ').unwrap();
        h.mock
            .lock()
            .unwrap()
            .responses
            .insert(format!("{method} {KCHAT}{path}"), invalid);
        let out = h.call(tool, args).await;
        assert!(
            ["provider_error", "provider_limitation"]
                .contains(&out["result"]["_meta"]["error_code"].as_str().unwrap_or("")),
            "{out}"
        );
        h.mock.lock().unwrap().responses.clear();
    }
    for args in [json!({"user_id":"a_b"}), json!({"user_id":"../x"})] {
        let out = h.call("kchat_get_user_profile", args).await;
        assert!(
            out["error"].is_object() || out["result"]["_meta"]["error_code"] == "invalid_request",
            "{out}"
        );
    }
}

#[tokio::test]
async fn kchat_writes_record_recovery_context_and_validate_results() {
    let h = Harness::new().await;
    let out = h
        .call(
            "kchat_post_message",
            json!({"channel_id":"chan1","text":"Hello"}),
        )
        .await;
    let post = &out["result"]["structuredContent"]["data"];
    assert_eq!(post["id"], "01a1624d-82b0-704f-960b-4456caa5f15e", "{out}");
    assert_eq!(
        post["source_url"],
        "https://ensemble.kchat.infomaniak.com/ensemble/pl/01a1624d-82b0-704f-960b-4456caa5f15e"
    );
    let body = h.mock.lock().unwrap().calls.last().unwrap().3.clone();
    assert_eq!(body, json!({"channel_id":"chan1","message":"Hello"}));
    let out = h
        .call(
            "kchat_reply_to_thread",
            json!({"channel_id":"chan1","thread_id":"post1","text":"Reply"}),
        )
        .await;
    assert_eq!(
        out["result"]["structuredContent"]["data"]["root_id"], "post1",
        "{out}"
    );
    let out = h
        .call(
            "kchat_add_reaction",
            json!({"post_id":"post1","reaction":":thumbsup:"}),
        )
        .await;
    assert_eq!(
        out["result"]["structuredContent"]["data"]["emoji_name"], "thumbsup",
        "{out}"
    );
    let body = h.mock.lock().unwrap().calls.last().unwrap().3.clone();
    assert_eq!(
        body,
        json!({"user_id":"me1","post_id":"post1","emoji_name":"thumbsup"})
    );
    let out = h
        .call(
            "kchat_send_direct_message",
            json!({"username":"@Bob","text":"Hi"}),
        )
        .await;
    assert_eq!(
        out["result"]["structuredContent"]["data"]["channel_id"], "dm1",
        "{out}"
    );
    let calls = h.mock.lock().unwrap().calls.clone();
    let direct = calls
        .iter()
        .find(|c| c.1.starts_with(&format!("{KCHAT}/channels/direct")))
        .unwrap();
    assert_eq!(direct.3, json!(["me1", "bob1"]));

    h.mock.lock().unwrap().responses.insert(
        format!("POST {KCHAT}/posts"),
        json!({"id":"new9","channel_id":"other"}),
    );
    let out = h
        .call(
            "kchat_post_message",
            json!({"channel_id":"chan1","text":"Hello"}),
        )
        .await;
    assert_eq!(
        out["result"]["_meta"]["error_code"], "mutation_uncertain",
        "{out}"
    );
    let context = &out["result"]["_meta"]["error_context"];
    assert_eq!(context["channel_id"], "chan1");
    assert_eq!(context["phase"], "post_message");
    assert_eq!(context["returned_post_id"], "new9");

    let out = h
        .call(
            "kchat_send_direct_message",
            json!({"username":"bob","text":"Hi"}),
        )
        .await;
    let context = &out["result"]["_meta"]["error_context"];
    assert_eq!(context["phase"], "post_message", "{out}");
    assert_eq!(context["channel_id"], "dm1");
    assert_eq!(context["completed"], json!(["open_direct_channel"]));

    h.mock.lock().unwrap().responses.insert(
        format!("POST {KCHAT}/channels/direct"),
        json!({"id":"chan1","type":"O"}),
    );
    let posts = |h: &Harness| {
        h.mock
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|c| c.0 == "POST" && c.1.starts_with(&format!("{KCHAT}/posts")))
            .count()
    };
    let before = posts(&h);
    let out = h
        .call(
            "kchat_send_direct_message",
            json!({"username":"bob","text":"Hi"}),
        )
        .await;
    assert_eq!(
        out["result"]["_meta"]["error_code"], "mutation_uncertain",
        "{out}"
    );
    assert_eq!(
        out["result"]["_meta"]["error_context"]["phase"],
        "open_direct_channel"
    );
    assert_eq!(posts(&h), before);
}

#[tokio::test]
async fn kchat_without_a_configured_team_exposes_no_data() {
    let mut h = Harness::new().await;
    let mut config = cfg();
    config.kchat_team = None;
    h.app.config = Arc::new(config);
    let out = h.call("kchat_list_channels", json!({})).await;
    assert_eq!(
        out["result"]["_meta"]["error_code"], "module_unconfigured",
        "{out}"
    );
    assert!(
        !h.mock
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|c| c.1.starts_with("/kchat/"))
    );
    let load = |team: &'static str| {
        Config::load(move |k| {
            match k {
                "KCHAT_TEAM" => Some(team),
                "PUBLIC_URL" => Some("https://mcp.example.org"),
                "CREDENTIAL_ENCRYPTION_KEY" => Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="),
                "ALLOWED_EMAIL_DOMAINS" => Some("team-ensemble.ch"),
                "BIND_ADDR" | "INFOMANIAK_ACCOUNT_ID" | "INFOMANIAK_MEMBERSHIP_READER_ID" => None,
                _ => Some("value"),
            }
            .map(String::from)
        })
    };
    assert_eq!(
        load(" Ensemble ").ok().unwrap().kchat_team.as_deref(),
        Some("ensemble")
    );
    assert!(load("").ok().unwrap().kchat_team.is_none());
    for team in ["evil.example.org/", "a.b", "-team", "team@x"] {
        assert!(load(team).is_err(), "{team}");
    }
}

#[tokio::test]
async fn calendar_updates_send_only_writable_fields() {
    let h = Harness::new().await;
    // Read-back events carry read-only metadata and offset timestamps.
    h.mock.lock().unwrap().responses.insert(
        "GET /1/calendar/pim/event/7".into(),
        json!({"data":{"id":7,"title":"Existing","calendar_id":"actual-calendar","start":"2026-10-17T10:00:00+02:00","end":"2026-10-17T09:00:00Z","timezone_start":"Europe/Zurich","timezone_end":"Europe/Zurich","description":null,"rrule":"","user_id":5,"created_at":1,"updated_at":2,"permissions":{"write":true},"categories":[],"attendees":[{"address":"person@example.org"}],"bookable_resource_id":"room-1"}}),
    );
    let out = h
        .call(
            "calendar_update_event",
            json!({"event_id":"7","start":"2026-10-17T10:30:00+02:00"}),
        )
        .await;
    assert_eq!(out["result"]["isError"], false, "{out}");
    let mock = h.mock.lock().unwrap();
    let body = &mock.calls.iter().find(|c| c.0 == "PUT").unwrap().3;
    let writable = [
        "id",
        "calendar_id",
        "title",
        "description",
        "start",
        "end",
        "timezone_start",
        "timezone_end",
        "freebusy",
        "type",
        "fullday",
        "private",
        "location",
        "color",
        "attendees",
        "alarms",
        "attachments",
        "rrule",
        "meet_room_url",
        "bookable_resource_id",
        "notifyAttendees",
        "parent_updated",
        "imip_request",
    ];
    for key in body.as_object().unwrap().keys() {
        assert!(writable.contains(&key.as_str()), "{key}: {body}");
    }
    assert_eq!(body["id"], 7);
    assert_eq!(body["start"], "2026-10-17 10:30:00");
    assert_eq!(body["end"], "2026-10-17 11:00:00");
    assert_eq!(body["description"], "");
    assert_eq!(body["attendees"][0]["address"], "person@example.org");
    assert_eq!(body["bookable_resource_id"], "room-1");
}

#[tokio::test]
async fn one_personal_token_serves_services_granted_later() {
    let h = Harness::new().await;
    h.sql("UPDATE users SET admin=1; DELETE FROM connections; UPDATE modules SET enabled=0 WHERE module='kchat'");
    let saved = request(
        &h.app,
        "POST",
        "/api/token",
        json!({"token":"personal-a"}),
        &h.headers(),
    )
    .await;
    assert_eq!(saved.status, 200);
    assert!(!saved.value["connected"].to_string().contains("kchat"));
    let enable = json!({"action":"module","module":"kchat","enabled":true,"verification":"Mock verification","default_level":2});
    assert_eq!(h.admin(enable).await.status, 200);
    let out = h.call("kchat_list_my_channels", json!({})).await;
    assert_eq!(out["result"]["isError"], false, "{out}");
    assert_eq!(
        h.mock.lock().unwrap().calls.last().unwrap().2,
        "Bearer personal-a"
    );
    let me = request(&h.app, "GET", "/api/me", Value::Null, &h.headers()).await;
    assert_eq!(me.value["connections"][0]["provider"], "infomaniak");
    assert_eq!(me.value["connections"][0]["scopes"], "*");
    assert_eq!(me.value["connections"][0]["health"], "connected");
    // Disconnect removes the one token for every service.
    let out = request(
        &h.app,
        "POST",
        "/api/disconnect/infomaniak",
        Value::Null,
        &h.headers(),
    )
    .await;
    assert_eq!(out.status, 200);
    for (tool, args) in [
        ("kchat_list_my_channels", json!({})),
        ("kdrive_info", json!({"drive_id":9,"file_id":42})),
    ] {
        assert_eq!(h.call(tool, args).await["error"], "reconnection_required");
    }
}

#[tokio::test]
async fn per_service_credentials_migrate_to_one_credential_per_user() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sqlite");
    let store = Store::open(path.to_str().unwrap()).unwrap();
    let alice = store
        .admit(
            &cfg(),
            ISSUER,
            "employee-a",
            "alice@team-ensemble.ch",
            "Alice",
        )
        .unwrap();
    let bob = store
        .admit(&cfg(), ISSUER, "employee-b", "bob@team-ensemble.ch", "Bob")
        .unwrap();
    let key = cfg().encryption_key;
    let legacy = |user: i64, module: &str, token: &str, refresh: Option<&str>| {
        crypto::seal(
            &key,
            &format!("credential:{user}:{module}"),
            &serde_json::to_vec(&Credential {
                access_token: token.into(),
                refresh_token: refresh.map(String::from),
            })
            .unwrap(),
        )
        .unwrap()
    };
    store
        .with(|c| {
            c.execute_batch("DROP TABLE connections; PRAGMA user_version=0; CREATE TABLE credentials(user_id INTEGER,module TEXT,secret BLOB NOT NULL,scopes TEXT NOT NULL,expires INTEGER NOT NULL,health TEXT NOT NULL DEFAULT 'connected',checked INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(user_id,module))")?;
            for (user, module, secret, scopes, health, checked) in [
                (alice.id, "mail", legacy(alice.id, "mail", "rejected-a", None), "workspace:mail user_info", "reconnection_required", 9),
                (alice.id, "kdrive", legacy(alice.id, "kdrive", "personal-a", None), "drive user_info", "connected", 5),
                (alice.id, "calendar", vec![0; 40], "workspace:calendar user_info", "connected", 10),
                (bob.id, "mail", legacy(bob.id, "mail", "delegated-b", Some("refresh-b")), "workspace:mail user_info", "connected", 1),
            ] {
                c.execute(
                    "INSERT INTO credentials(user_id,module,secret,scopes,expires,health,checked) VALUES(?,?,?,?,?,?,?)",
                    params![user, module, secret, scopes, now() + 3600, health, checked],
                )?;
            }
            Ok(())
        })
        .unwrap();
    let metadata = Metadata {
        issuer: ISSUER.into(),
        authorization_endpoint: String::new(),
        token_endpoint: String::new(),
        jwks_uri: String::new(),
        code_challenge_methods_supported: vec!["S256".into()],
    };
    let provider = Provider::with_endpoints(
        metadata,
        Provider::http_client().unwrap(),
        String::new(),
        String::new(),
    );
    let app = App::new(cfg(), store, provider).unwrap();
    app.store
        .with(|c| {
            let legacy: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='credentials')",
                [],
                |r| r.get(0),
            )?;
            assert!(!legacy);
            let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
            assert_eq!(version, ensemble_mcp::migrations::VERSION);
            for (user, token, scopes, health) in [
                (alice.id, "personal-a", "*", "connected"),
                (
                    bob.id,
                    "delegated-b",
                    "workspace:mail user_info",
                    "connected",
                ),
            ] {
                let (secret, stored_scopes, stored_health): (Vec<u8>, String, String) = c
                    .query_row(
                        "SELECT secret,scopes,health FROM connections WHERE user_id=? AND provider='infomaniak'",
                        [user],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )?;
                let credential: Credential = serde_json::from_slice(
                    &crypto::open(&key, &format!("credential:{user}:infomaniak"), &secret).unwrap(),
                )
                .unwrap();
                assert_eq!(credential.access_token, token);
                assert_eq!(stored_scopes, scopes);
                assert_eq!(stored_health, health);
            }
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn credentials_are_shared_only_within_a_provider() {
    let h = Harness::new().await;
    let mut registered = modules::Registry::builtin().unwrap().modules;
    registered.push(modules::Module {
        id: "notes",
        name: "Notes",
        namespace: "notes",
        provider: "notes-provider",
        scopes: vec!["notes"],
        tools: vec![],
        routes: vec![],
        resources: None,
        handler: |_, _| Box::pin(async { Ok(json!({})) }),
    });
    let mut app = h.app.clone();
    app.modules = Arc::new(modules::Registry::new(registered).unwrap());
    h.sql("INSERT INTO modules(module,enabled,verification) VALUES('notes',1,'mock'); INSERT INTO policies(kind,principal,module,level) VALUES('user',1,'notes',2)");
    assert!(
        app.provider
            .access_token(&app, &h.user, "kdrive")
            .await
            .is_ok()
    );
    assert_eq!(
        app.provider
            .access_token(&app, &h.user, "notes")
            .await
            .err()
            .unwrap()
            .code,
        "reconnection_required"
    );
    assert!(!oauth::available_scope(&app, &h.user, "notes:read").unwrap());
    let start = request(
        &app,
        "POST",
        "/api/connect/notes",
        Value::Null,
        &h.headers(),
    )
    .await;
    assert_eq!(start.status, 400);
}

#[test]
fn databases_newer_than_the_server_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("future.sqlite");
    let store = Store::open(path.to_str().unwrap()).unwrap();
    store
        .with(|c| {
            c.pragma_update(None, "user_version", ensemble_mcp::migrations::VERSION + 1)?;
            Ok(())
        })
        .unwrap();
    drop(store);
    let mut connection = rusqlite::Connection::open(&path).unwrap();
    assert!(ensemble_mcp::migrations::run(&mut connection, &cfg().encryption_key).is_err());
}
