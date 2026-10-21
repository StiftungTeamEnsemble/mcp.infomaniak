use crate::{App, Error, Result, crypto, identity, oauth, store::now};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};

pub fn session(
    app: &App,
    headers: &HeaderMap,
    active: bool,
) -> Result<(crate::store::User, String)> {
    let raw = identity::cookie(headers, "__Host-mcp_session").ok_or_else(Error::unauthorized)?;
    let (id, csrf) = app.store.with(|c| {
        c.query_row(
            "SELECT user_id,csrf FROM sessions WHERE hash=? AND expires>?",
            params![crypto::hash(&raw), now()],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?
        .ok_or_else(Error::unauthorized)
    })?;
    let user = app.store.user(id)?;
    app.check_user(&user, active)?;
    Ok((user, csrf))
}
pub fn csrf(app: &App, headers: &HeaderMap) -> Result<()> {
    let (_, csrf) = session(app, headers, false)?;
    if headers.get("origin").and_then(|v| v.to_str().ok()) != Some(&app.config.public_url)
        || headers.get("x-csrf-token").and_then(|v| v.to_str().ok()) != Some(&csrf)
    {
        return Err(Error::denied());
    }
    Ok(())
}
pub fn request_id(headers: &HeaderMap) -> &str {
    headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("internal")
}
pub fn router(app: App) -> Router {
    let mut router = Router::new()
        .route("/", get(index))
        .route("/assets/app.js", get(script))
        .route("/assets/style.css", get(style))
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/auth/login", get(identity::login))
        .route("/auth/infomaniak/callback", get(identity::callback))
        .route(
            "/.well-known/oauth-protected-resource",
            get(oauth::resource),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(oauth::resource),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth::metadata),
        )
        .route("/register", post(oauth::register))
        .route("/authorize", get(oauth::authorize))
        .route("/token", post(oauth::token))
        .route("/revoke", post(oauth::revoke))
        .route("/mcp", post(crate::mcp::handle))
        .route("/api/me", get(me))
        .route("/api/logout", post(logout))
        .route("/api/connect/{module}", post(connect))
        .route("/api/token", post(save_token))
        .route("/api/disconnect/{module}", post(disconnect))
        .route("/api/clients/{id}/revoke", post(revoke_client))
        .route("/api/clients/{id}/scopes", post(edit_client))
        .route("/api/authorize", get(consent_view).post(oauth::consent))
        .route("/api/admin", get(admin_view).post(admin_change));
    for module in &app.modules.modules {
        for route in &module.routes {
            let state = (app.clone(), module.id.to_owned(), route.write);
            router = router.route(
                route.path,
                route
                    .handler
                    .clone()
                    .route_layer(middleware::from_fn_with_state(state, module_access)),
            );
        }
    }
    router
        .layer(DefaultBodyLimit::max(12 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(app.clone(), security))
        .with_state(app)
}
async fn module_access(
    State((app, module, write)): State<(App, String, bool)>,
    mut request: Request,
    next: Next,
) -> Result<Response> {
    let headers = request.headers();
    let actor = if headers.contains_key(header::AUTHORIZATION) {
        oauth::authenticate(&app, headers)?
    } else {
        let (user, _) = session(&app, headers, true)?;
        if write {
            csrf(&app, headers)?;
        }
        oauth::Actor {
            user,
            client: "browser".into(),
            scopes: vec![format!("{module}:read"), format!("{module}:write")],
            bearer_hash: None,
        }
    };
    oauth::allowed(&app, &actor, &module, write)?;
    let token = app
        .provider
        .access_token(&app, &actor.user, &module)
        .await?;
    if !headers.contains_key(header::AUTHORIZATION) {
        session(&app, headers, true)?;
    }
    oauth::allowed(&app, &actor, &module, write)?;
    request.extensions_mut().insert(actor.clone());
    request.extensions_mut().insert(UpstreamToken(token));
    let response = next.run(request).await;
    app.store.audit(
        "module-http",
        Some(actor.user.id),
        Some(&actor.client),
        &format!("{module}.http"),
        if response.status().is_success() {
            "ok"
        } else {
            "failed"
        },
    )?;
    Ok(response)
}
#[derive(Clone)]
pub struct UpstreamToken(pub String);
async fn security(State(app): State<App>, mut request: Request, next: Next) -> Response {
    let correlation = crypto::random();
    if let Ok(value) = HeaderValue::from_str(&correlation) {
        request.headers_mut().insert("x-request-id", value);
    }
    // Public origin only; forwarded headers never select URLs, identity, or trusted origins.
    if request
        .headers()
        .get("origin")
        .is_some_and(|v| v.to_str().ok() != Some(&app.config.public_url))
    {
        return Error::denied().into_response();
    }
    let path = request.uri().path().to_owned();
    let action = if path.starts_with("/auth/") {
        "http.authentication"
    } else if path == "/mcp" {
        "http.mcp"
    } else if path.starts_with("/api/admin") {
        "http.admin"
    } else {
        "http.request"
    };
    let mut response = next.run(request).await;
    let outcome = if response.status().is_success() || response.status().is_redirection() {
        "ok"
    } else {
        "rejected"
    };
    if app
        .store
        .audit(&correlation, None, None, action, outcome)
        .is_err()
    {
        return Error::internal().into_response();
    }
    let unauthorized = response.status() == 401;
    let headers = response.headers_mut();
    for (key, value) in [
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("x-frame-options", "DENY"),
        (
            "content-security-policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
        ),
        ("strict-transport-security", "max-age=31536000"),
    ] {
        headers.insert(
            header::HeaderName::from_static(key),
            HeaderValue::from_static(value),
        );
    }
    if let Ok(value) = HeaderValue::from_str(&correlation) {
        headers.insert("x-request-id", value);
    }
    if unauthorized {
        let value = format!(
            "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource\", error=\"invalid_token\"",
            app.config.public_url
        );
        if let Ok(value) = HeaderValue::from_str(&value) {
            headers.insert(header::WWW_AUTHENTICATE, value);
        }
    }
    response
}
async fn index() -> Html<&'static str> {
    Html(include_str!("../web/index.html"))
}
async fn script() -> Result<Response> {
    let bytes = tokio::fs::read("web/dist/app.js").await.map_err(|_| {
        Error::new(
            503,
            "ui_not_built",
            "Run npm ci and npm run build before starting the server",
        )
    })?;
    Ok((
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        bytes,
    )
        .into_response())
}
async fn style() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../web/style.css"),
    )
        .into_response()
}

async fn me(State(app): State<App>, headers: HeaderMap) -> Result<Json<Value>> {
    let (user, csrf) = session(&app, &headers, false)?;
    let mut out = app.store.user_summary(&user)?;
    let mut modules = vec![];
    for m in &app.modules.modules {
        modules.push(json!({"id":m.id,"name":m.name,"enabled":app.store.module_enabled(m.id)?,"level":app.store.local_level(user.id,m.id)?,"scopes":m.scopes}));
    }
    out["modules"] = json!(modules);
    let mut scopes = vec![];
    if user.status == "active" {
        for s in oauth::scopes(&app) {
            scopes.push(json!({"scope":s,"available":oauth::available_scope(&app,&user,&s)?}));
        }
    }
    out["scopes"] = json!(scopes);
    out["csrf"] = json!(csrf);
    out["endpoint"] = json!(app.config.resource());
    Ok(Json(out))
}
async fn logout(State(app): State<App>, headers: HeaderMap) -> Result<Response> {
    csrf(&app, &headers)?;
    let raw = identity::cookie(&headers, "__Host-mcp_session").ok_or_else(Error::unauthorized)?;
    app.store.with(|c| {
        c.execute("DELETE FROM sessions WHERE hash=?", [crypto::hash(&raw)])?;
        Ok(())
    })?;
    Ok((
        [(
            header::SET_COOKIE,
            identity::set_cookie("__Host-mcp_session", "", 0),
        )],
        Json(json!({"ok":true})),
    )
        .into_response())
}
async fn connect(
    State(app): State<App>,
    headers: HeaderMap,
    Path(module): Path<String>,
    Query(query): Query<identity::LoginQuery>,
) -> Result<Response> {
    let (user, _) = session(&app, &headers, true)?;
    csrf(&app, &headers)?;
    app.modules.get(&module)?;
    if !app.store.module_enabled(&module)? || app.store.local_level(user.id, &module)? == 0 {
        return Err(Error::denied());
    }
    // Fetch receives a URL rather than following an OAuth redirect as an API request.
    let response = identity::start(&app, Some(module), Some(user.id), query.authorize).await?;
    let location = response
        .headers()
        .get(header::LOCATION)
        .cloned()
        .ok_or_else(Error::internal)?;
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .cloned()
        .ok_or_else(Error::internal)?;
    Ok((
        [(header::SET_COOKIE, cookie)],
        Json(json!({"redirect":location.to_str().map_err(|_|Error::internal())?})),
    )
        .into_response())
}
#[derive(Deserialize)]
struct PersonalToken {
    token: String,
}
/// Personal tokens do not report their expiry; provider 401s mark them for replacement.
const PERSONAL_TOKEN_LIFETIME: i64 = 86400 * 365 * 10;
async fn save_token(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<PersonalToken>,
) -> Result<Json<Value>> {
    let (user, _) = session(&app, &headers, true)?;
    csrf(&app, &headers)?;
    app.store.throttle("personal_token", 20)?;
    let token = input.token.trim();
    if token.is_empty() || token.len() > 512 || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(Error::invalid(
            "Paste the API token exactly as Infomaniak shows it",
        ));
    }
    let rejected = || {
        Error::new(
            400,
            "token_rejected",
            "Infomaniak rejected this token; create a new one with all listed scopes",
        )
    };
    let profile = crate::modules::data(
        app.provider
            .request(
                token,
                reqwest::Method::GET,
                format!("{}/2/profile", app.provider.api),
                &[],
                None,
            )
            .await
            .map_err(|e| match e.code {
                "reconnection_required" | "access_denied" | "provider_not_found" => rejected(),
                _ => e,
            })?,
    );
    // The token must belong to the signed-in Infomaniak account, never a colleague's.
    let email = profile["email"].as_str().ok_or_else(rejected)?;
    let id = profile["id"]
        .as_i64()
        .map(|id| id.to_string())
        .or_else(|| profile["id"].as_str().map(String::from));
    if !email.eq_ignore_ascii_case(&user.email) || id.is_some_and(|id| id != user.subject) {
        return Err(Error::new(
            403,
            "token_owner_mismatch",
            "This token belongs to another Infomaniak account",
        ));
    }
    let mut connected = vec![];
    for module in &app.modules.modules {
        if !app.store.module_enabled(module.id)? || app.store.local_level(user.id, module.id)? == 0
        {
            continue;
        }
        let secret = crypto::seal(
            &app.config.encryption_key,
            &format!("credential:{}:{}", user.id, module.id),
            &serde_json::to_vec(&identity::Credential {
                access_token: token.into(),
                refresh_token: None,
            })?,
        )?;
        app.store.with(|c|{c.execute("INSERT INTO credentials(user_id,module,secret,scopes,expires) VALUES(?,?,?,?,?) ON CONFLICT(user_id,module) DO UPDATE SET secret=excluded.secret,scopes=excluded.scopes,expires=excluded.expires,health='connected',checked=0",params![user.id,module.id,secret,module.scopes.join(" "),now()+PERSONAL_TOKEN_LIFETIME])?;Ok(())})?;
        connected.push(module.id);
    }
    app.store.audit(
        request_id(&headers),
        Some(user.id),
        None,
        "credential.token",
        "ok",
    )?;
    Ok(Json(json!({"ok":true,"connected":connected})))
}
async fn disconnect(
    State(app): State<App>,
    headers: HeaderMap,
    Path(module): Path<String>,
) -> Result<Json<Value>> {
    let (user, _) = session(&app, &headers, true)?;
    csrf(&app, &headers)?;
    app.modules.get(&module)?;
    app.store.with(|c| {
        c.execute(
            "DELETE FROM credentials WHERE user_id=? AND module=?",
            params![user.id, module],
        )?;
        Ok(())
    })?;
    app.store.audit(
        request_id(&headers),
        Some(user.id),
        None,
        "credential.disconnect",
        "ok",
    )?;
    Ok(Json(
        json!({"ok":true,"upstream_revocation":"Local access removed. Delete your API token in the Infomaniak Manager to invalidate it upstream."}),
    ))
}
async fn revoke_client(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let (user, _) = session(&app, &headers, true)?;
    csrf(&app, &headers)?;
    app.store.with(|c| {
        c.execute(
            "UPDATE grants SET revoked=1 WHERE id=? AND user_id=?",
            params![id, user.id],
        )?;
        Ok(())
    })?;
    app.store.audit(
        request_id(&headers),
        Some(user.id),
        None,
        "oauth.revoke",
        "ok",
    )?;
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
struct ClientScopes {
    scopes: Vec<String>,
}
/// Changes an existing client grant with the same rules as consent; takes effect on the next request.
async fn edit_client(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(q): Json<ClientScopes>,
) -> Result<Json<Value>> {
    let (user, _) = session(&app, &headers, true)?;
    csrf(&app, &headers)?;
    let current = app.store.with(|c| {
        c.query_row(
            "SELECT scopes FROM grants WHERE id=? AND user_id=? AND revoked=0",
            params![id, user.id],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| Error::new(404, "not_found", "Client grant not found"))
    })?;
    let known = oauth::scopes(&app);
    let mut scopes: Vec<String> = vec![];
    for scope in &q.scopes {
        if !known.contains(scope) {
            return Err(Error::invalid("Unknown scope"));
        }
        if !scopes.contains(scope) {
            scopes.push(scope.clone());
        }
    }
    if scopes.is_empty() {
        return Err(Error::invalid(
            "Keep at least one permission or revoke the client",
        ));
    }
    // Newly added access needs the same availability as consent; existing access may be kept or narrowed.
    for scope in &scopes {
        if !current.split_whitespace().any(|s| s == scope)
            && !oauth::available_scope(&app, &user, scope)?
        {
            return Err(Error::denied());
        }
    }
    app.store.with(|c| {
        c.execute(
            "UPDATE grants SET scopes=? WHERE id=? AND user_id=? AND revoked=0",
            params![scopes.join(" "), id, user.id],
        )?;
        Ok(())
    })?;
    app.store.audit(
        request_id(&headers),
        Some(user.id),
        None,
        "oauth.grant_edit",
        "ok",
    )?;
    Ok(Json(json!({"ok":true,"scopes":scopes})))
}
#[derive(Deserialize)]
struct FlowQuery {
    flow: String,
}
async fn consent_view(
    State(app): State<App>,
    headers: HeaderMap,
    Query(q): Query<FlowQuery>,
) -> Result<Json<Value>> {
    let (user, _) = session(&app, &headers, true)?;
    let pending = oauth::pending(&app, &headers, &q.flow)?;
    let client = app.store.with(|c| {
        Ok(c.query_row(
            "SELECT name FROM clients WHERE id=?",
            [pending.client_id],
            |r| r.get::<_, String>(0),
        )?)
    })?;
    let mut scopes = vec![];
    for s in pending.scope.unwrap_or_default().split_whitespace() {
        scopes.push(json!({"scope":s,"available":oauth::available_scope(&app,&user,s)?}));
    }
    Ok(Json(
        json!({"client":client,"redirect_uri":pending.redirect_uri,"scopes":scopes}),
    ))
}
fn admin(app: &App, headers: &HeaderMap) -> Result<crate::store::User> {
    let (user, _) = session(app, headers, true)?;
    if !user.admin {
        return Err(Error::denied());
    }
    Ok(user)
}
async fn admin_view(State(app): State<App>, headers: HeaderMap) -> Result<Json<Value>> {
    admin(&app, &headers)?;
    let mut out=app.store.with(|c| {
        let mut stmt=c.prepare("SELECT id,email,name,status,admin FROM users ORDER BY id LIMIT 500")?;
        let users=stmt.query_map([],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"email":r.get::<_,String>(1)?,"name":r.get::<_,String>(2)?,"status":r.get::<_,String>(3)?,"admin":r.get::<_,bool>(4)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let mut stmt=c.prepare("SELECT module,enabled,verification,default_level FROM modules")?;
        let modules=stmt.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"enabled":r.get::<_,bool>(1)?,"verification":r.get::<_,String>(2)?,"default_level":r.get::<_,i64>(3)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let mut stmt=c.prepare("SELECT id,name FROM groups ORDER BY name LIMIT 500")?;
        let groups=stmt.query_map([],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"name":r.get::<_,String>(1)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let mut stmt=c.prepare("SELECT user_id,group_id FROM memberships LIMIT 10000")?;
        let memberships=stmt.query_map([],|r|Ok(json!({"user_id":r.get::<_,i64>(0)?,"group_id":r.get::<_,i64>(1)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let mut stmt=c.prepare("SELECT kind,principal,module,level FROM policies LIMIT 10000")?;
        let policies=stmt.query_map([],|r|Ok(json!({"kind":r.get::<_,String>(0)?,"principal":r.get::<_,i64>(1)?,"module":r.get::<_,String>(2)?,"level":r.get::<_,i64>(3)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let mut stmt=c.prepare("SELECT user_id,module,scopes,expires,health,checked FROM credentials LIMIT 1500")?;
        let connections=stmt.query_map([],|r|Ok(json!({"user_id":r.get::<_,i64>(0)?,"module":r.get::<_,String>(1)?,"scopes":r.get::<_,String>(2)?,"expires":r.get::<_,i64>(3)?,"health":r.get::<_,String>(4)?,"checked":r.get::<_,i64>(5)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let mut stmt=c.prepare("SELECT at,severity,request_id,actor,client,action,outcome FROM logs ORDER BY id DESC LIMIT 100")?;
        let logs=stmt.query_map([],|r|Ok(json!({"at":r.get::<_,i64>(0)?,"severity":r.get::<_,String>(1)?,"request_id":r.get::<_,String>(2)?,"actor":r.get::<_,Option<i64>>(3)?,"client":r.get::<_,Option<String>>(4)?,"action":r.get::<_,String>(5)?,"outcome":r.get::<_,String>(6)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let manual:bool=c.query_row("SELECT manual_approval FROM settings WHERE id=1",[],|r|r.get(0))?;
        let mut stmt=c.prepare("SELECT g.id,g.user_id,c.name,g.scopes FROM grants g JOIN clients c ON c.id=g.client_id WHERE g.revoked=0 LIMIT 1500")?;
        let clients=stmt.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"user_id":r.get::<_,i64>(1)?,"name":r.get::<_,String>(2)?,"scopes":r.get::<_,String>(3)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
        Ok(json!({"users":users,"modules":modules,"groups":groups,"memberships":memberships,"policies":policies,"connections":connections,"logs":logs,"manual_approval":manual,"clients":clients}))
    })?;
    for client in out["clients"].as_array_mut().ok_or_else(Error::internal)? {
        let user = app
            .store
            .user(client["user_id"].as_i64().ok_or_else(Error::internal)?)?;
        let mut effective = vec![];
        for scope in client["scopes"].as_str().unwrap_or("").split_whitespace() {
            if app.check_user(&user, true).is_ok() && oauth::available_scope(&app, &user, scope)? {
                effective.push(scope.to_owned());
            }
        }
        client["effective_scopes"] = json!(effective);
    }
    Ok(Json(out))
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdminChange {
    Onboarding {
        manual: bool,
    },
    Module {
        module: String,
        enabled: bool,
        verification: String,
        default_level: i64,
    },
    User {
        user_id: i64,
        status: String,
        admin: bool,
    },
    Group {
        name: String,
    },
    Membership {
        user_id: i64,
        group_id: i64,
        member: bool,
    },
    Policy {
        kind: String,
        principal: i64,
        module: String,
        level: i64,
    },
    Revoke {
        user_id: i64,
        module: String,
    },
}
async fn admin_change(
    State(app): State<App>,
    headers: HeaderMap,
    Json(change): Json<AdminChange>,
) -> Result<Json<Value>> {
    let actor = admin(&app, &headers)?;
    csrf(&app, &headers)?;
    app.store.with(|c|{
        match change {
            AdminChange::Onboarding {manual}=>{c.execute("UPDATE settings SET manual_approval=? WHERE id=1",[manual])?;},
            AdminChange::Module {module,enabled,verification,default_level}=>{
                app.modules.get(&module)?;
                if !(0..=2).contains(&default_level) || verification.len()>1000 || (enabled && verification.trim().is_empty()) {return Err(Error::invalid("Enable only after recording two-employee delegated-access verification; default level is 0, 1 or 2"))}
                c.execute("UPDATE modules SET enabled=?,verification=?,default_level=? WHERE module=?",params![enabled,verification,default_level,module])?;
            },
            AdminChange::User {user_id,status,admin}=>{
                if !["active","pending","suspended"].contains(&status.as_str()) {return Err(Error::invalid("Invalid user status"))}
                let target_admin:bool=c.query_row("SELECT admin FROM users WHERE id=?",[user_id],|r|r.get(0))?;
                if target_admin && (!admin || status!="active") {
                    let others:i64=c.query_row("SELECT COUNT(*) FROM users WHERE id<>? AND admin=1 AND status='active'",[user_id],|r|r.get(0))?;
                    if others==0{return Err(Error::invalid("Keep at least one active administrator"))}
                }
                c.execute("UPDATE users SET status=?,admin=? WHERE id=?",params![status,admin,user_id])?;
            },
            AdminChange::Group {name}=>{
                if name.trim().is_empty() || name.len()>100 {return Err(Error::invalid("Group name must contain 1 to 100 characters"))}
                c.execute("INSERT INTO groups(name) VALUES(?)",[name.trim()])?;
            },
            AdminChange::Membership {user_id,group_id,member}=>{
                if member{c.execute("INSERT OR IGNORE INTO memberships(user_id,group_id) VALUES(?,?)",params![user_id,group_id])?;}else{c.execute("DELETE FROM memberships WHERE user_id=? AND group_id=?",params![user_id,group_id])?;}
            },
            AdminChange::Policy {kind,principal,module,level}=>{
                app.modules.get(&module)?;
                if !(0..=2).contains(&level) || !["user","group"].contains(&kind.as_str()){return Err(Error::invalid("Invalid permission policy"))}
                let table=if kind=="user"{"users"}else{"groups"};
                if !c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=?)"),[principal],|r|r.get::<_,bool>(0))? {return Err(Error::invalid("Unknown policy principal"))}
                c.execute("INSERT INTO policies(kind,principal,module,level) VALUES(?,?,?,?) ON CONFLICT(kind,principal,module) DO UPDATE SET level=excluded.level",params![kind,principal,module,level])?;
            },
            AdminChange::Revoke {user_id,module}=>{app.modules.get(&module)?;c.execute("DELETE FROM credentials WHERE user_id=? AND module=?",params![user_id,module])?;},
        } Ok(())
    })?;
    app.store.audit(
        request_id(&headers),
        Some(actor.id),
        None,
        "admin.change",
        "ok",
    )?;
    Ok(Json(json!({"ok":true})))
}
