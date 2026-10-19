//! OAuth for MCP clients, distinct from delegated upstream credentials.
use crate::{
    App, Error, Result, crypto, identity,
    store::{User, now},
};
use axum::{
    Form, Json,
    extract::{Query, State},
    http::{HeaderMap, header::SET_COOKIE},
    response::{IntoResponse, Redirect, Response},
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

pub fn scopes(app: &App) -> Vec<String> {
    app.modules
        .modules
        .iter()
        .flat_map(|module| {
            [
                format!("{}:read", module.id),
                format!("{}:write", module.id),
            ]
        })
        .collect()
}

pub async fn resource(State(app): State<App>) -> Json<Value> {
    Json(
        json!({"resource":app.config.resource(),"authorization_servers":[app.config.public_url],"scopes_supported":scopes(&app),"bearer_methods_supported":["header"]}),
    )
}
pub async fn metadata(State(app): State<App>) -> Json<Value> {
    let base = &app.config.public_url;
    Json(
        json!({"issuer":base,"authorization_endpoint":format!("{base}/authorize"),"token_endpoint":format!("{base}/token"),"registration_endpoint":format!("{base}/register"),"revocation_endpoint":format!("{base}/revoke"),"response_types_supported":["code"],"grant_types_supported":["authorization_code","refresh_token"],"code_challenge_methods_supported":["S256"],"token_endpoint_auth_methods_supported":["none"],"scopes_supported":scopes(&app)}),
    )
}

#[derive(Deserialize)]
pub struct Registration {
    pub client_name: Option<String>,
    pub redirect_uris: Vec<String>,
    pub token_endpoint_auth_method: Option<String>,
    pub grant_types: Option<Vec<String>>,
    pub response_types: Option<Vec<String>>,
}
pub fn valid_redirect(raw: &str) -> bool {
    let Ok(url) = Url::parse(raw) else {
        return false;
    };
    let secure = url.scheme() == "https" && url.host_str().is_some();
    let loopback = url.scheme() == "http"
        && matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"));
    (secure || loopback)
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && raw.len() <= 2048
}
pub async fn register(
    State(app): State<App>,
    headers: HeaderMap,
    Json(r): Json<Registration>,
) -> Result<Response> {
    app.store.throttle("registration", 20)?;
    app.store.prune()?;
    if r.redirect_uris.is_empty()
        || r.redirect_uris.len() > 10
        || r.redirect_uris.iter().any(|s| !valid_redirect(s))
        || r.token_endpoint_auth_method
            .as_deref()
            .is_some_and(|a| a != "none")
        || r.grant_types.as_ref().is_some_and(|g| {
            g.iter()
                .any(|v| v != "authorization_code" && v != "refresh_token")
        })
        || r.response_types
            .as_ref()
            .is_some_and(|g| g.as_slice() != ["code"])
    {
        return Err(Error::invalid(
            "Invalid client metadata; public clients use PKCE S256",
        ));
    }
    let name = r.client_name.unwrap_or_else(|| "MCP client".into());
    if name.trim().is_empty() || name.len() > 100 || name.chars().any(char::is_control) {
        return Err(Error::invalid("Invalid client name"));
    }
    let client = crypto::random();
    app.store.with(|c| {
        let count: i64 = c.query_row("SELECT COUNT(*) FROM clients", [], |r| r.get(0))?;
        if count >= 1000 {
            return Err(Error::new(
                429,
                "registration_limit",
                "Client registration limit reached",
            ));
        }
        c.execute(
            "INSERT INTO clients(id,name,redirects,created) VALUES(?,?,?,?)",
            params![
                client,
                name,
                serde_json::to_string(&r.redirect_uris)?,
                now()
            ],
        )?;
        Ok(())
    })?;
    app.store.audit(
        crate::web::request_id(&headers),
        None,
        Some(&client),
        "oauth.register",
        "ok",
    )?;
    Ok((axum::http::StatusCode::CREATED,Json(json!({"client_id":client,"client_name":name,"redirect_uris":r.redirect_uris,"token_endpoint_auth_method":"none","grant_types":["authorization_code","refresh_token"],"response_types":["code"]}))).into_response())
}

#[derive(Serialize, Deserialize)]
pub struct Authorization {
    pub client_id: String,
    pub redirect_uri: String,
    pub response_type: String,
    pub state: Option<String>,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub resource: String,
    pub scope: Option<String>,
}
fn parse_scopes(app: &App, scope: Option<&str>) -> Result<Vec<String>> {
    let values = scope
        .unwrap_or("")
        .split_whitespace()
        .map(String::from)
        .collect::<Vec<_>>();
    let supported = scopes(app);
    if values.is_empty()
        || values.len() > supported.len()
        || values.iter().any(|s| !supported.contains(s))
    {
        return Err(Error::new(
            400,
            "invalid_scope",
            "Select explicit service read/write scopes",
        ));
    }
    Ok(values)
}
pub async fn authorize(State(app): State<App>, Query(q): Query<Authorization>) -> Result<Response> {
    app.store.throttle("authorize", 60)?;
    app.store.prune()?;
    let redirects = app.store.with(|c| {
        c.query_row(
            "SELECT redirects FROM clients WHERE id=?",
            [&q.client_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| Error::invalid("Unknown client"))
    })?;
    if !serde_json::from_str::<Vec<String>>(&redirects)?.contains(&q.redirect_uri) {
        return Err(Error::invalid("Unregistered redirect URI"));
    }
    if q.response_type != "code"
        || q.code_challenge_method != "S256"
        || q.code_challenge.len() != 43
        || !q
            .code_challenge
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        || q.resource != app.config.resource()
        || q.state.as_ref().is_some_and(|s| s.len() > 2048)
    {
        return Err(Error::invalid("Invalid OAuth request, PKCE or resource"));
    }
    parse_scopes(&app, q.scope.as_deref())?;
    let flow = crypto::random();
    let binding = crypto::random();
    let hash = crypto::hash(&flow);
    let secret = crypto::seal(
        &app.config.encryption_key,
        &format!("authorize:{hash}"),
        &serde_json::to_vec(&q)?,
    )?;
    app.store.with(|c| {
        c.execute(
            "INSERT INTO flows(hash,binding,secret,expires) VALUES(?,?,?,?)",
            params![hash, crypto::hash(&binding), secret, now() + 600],
        )?;
        Ok(())
    })?;
    Ok((
        [(
            SET_COOKIE,
            identity::set_cookie("__Host-mcp_authorize", &binding, 600),
        )],
        Redirect::to(&format!("/?authorize={flow}")),
    )
        .into_response())
}

pub fn pending(app: &App, headers: &HeaderMap, flow: &str) -> Result<Authorization> {
    let binding =
        identity::cookie(headers, "__Host-mcp_authorize").ok_or_else(Error::unauthorized)?;
    let hash = crypto::hash(flow);
    let secret = app.store.with(|c| {
        c.query_row(
            "SELECT secret FROM flows WHERE hash=? AND binding=? AND expires>?",
            params![hash, crypto::hash(&binding), now()],
            |r| r.get::<_, Vec<u8>>(0),
        )
        .optional()?
        .ok_or_else(Error::unauthorized)
    })?;
    serde_json::from_slice(&crypto::open(
        &app.config.encryption_key,
        &format!("authorize:{hash}"),
        &secret,
    )?)
    .map_err(Into::into)
}

pub fn available_scope(app: &App, user: &User, scope: &str) -> Result<bool> {
    let (module, access) = scope
        .split_once(':')
        .ok_or_else(|| Error::invalid("Invalid scope"))?;
    let level = if access == "write" { 2 } else { 1 };
    if !app.store.module_enabled(module)? || app.store.local_level(user.id, module)? < level {
        return Ok(false);
    }
    let scopes = app.store.with(|c| {
        Ok(c.query_row(
            "SELECT scopes FROM credentials WHERE user_id=? AND module=?",
            params![user.id, module],
            |r| r.get::<_, String>(0),
        )
        .optional()?)
    })?;
    Ok(scopes.is_some_and(|s| {
        app.modules.get(module).is_ok_and(|m| {
            m.scopes
                .iter()
                .all(|v| s.split_whitespace().any(|s| s == *v))
        })
    }))
}

#[derive(Deserialize)]
pub struct Consent {
    pub flow: String,
    pub scopes: Vec<String>,
    pub approve: bool,
}
pub async fn consent(
    State(app): State<App>,
    headers: HeaderMap,
    Json(q): Json<Consent>,
) -> Result<Json<Value>> {
    let (user, _) = crate::web::session(&app, &headers, true)?;
    crate::web::csrf(&app, &headers)?;
    let pending = pending(&app, &headers, &q.flow)?;
    let requested = parse_scopes(&app, pending.scope.as_deref())?;
    if q.approve
        && (q.scopes.is_empty()
            || q.scopes.len() > requested.len()
            || q.scopes.iter().any(|s| !requested.contains(s)))
    {
        return Err(Error::invalid("Choose a subset of the requested scopes"));
    }
    for scope in &q.scopes {
        if !available_scope(&app, &user, scope)? {
            return Err(Error::denied());
        }
    }
    let code = crypto::random();
    let grant = crypto::random();
    app.store.with(|c|{
        let tx=c.transaction()?;
        let deleted=tx.execute("DELETE FROM flows WHERE hash=?",[crypto::hash(&q.flow)])?;
        if deleted!=1 {return Err(Error::unauthorized())}
        if q.approve {
            tx.execute("INSERT INTO grants(id,user_id,client_id,scopes) VALUES(?,?,?,?)",params![grant,user.id,pending.client_id,q.scopes.join(" ")])?;
            tx.execute("INSERT INTO codes(hash,grant_id,challenge,redirect,resource,expires) VALUES(?,?,?,?,?,?)",params![crypto::hash(&code),grant,pending.code_challenge,pending.redirect_uri,pending.resource,now()+120])?;
        }
        tx.commit()?;Ok(())
    })?;
    let mut redirect = Url::parse(&pending.redirect_uri).map_err(|_| Error::internal())?;
    if q.approve {
        redirect.query_pairs_mut().append_pair("code", &code);
    } else {
        redirect
            .query_pairs_mut()
            .append_pair("error", "access_denied");
    }
    if let Some(state) = pending.state {
        redirect.query_pairs_mut().append_pair("state", &state);
    }
    redirect
        .query_pairs_mut()
        .append_pair("iss", &app.config.public_url);
    app.store.audit(
        crate::web::request_id(&headers),
        Some(user.id),
        Some(&pending.client_id),
        "oauth.consent",
        if q.approve { "ok" } else { "denied" },
    )?;
    Ok(Json(json!({"redirect":redirect.to_string()})))
}

#[derive(Deserialize)]
pub struct TokenRequest {
    pub grant_type: String,
    pub client_id: String,
    pub resource: String,
    pub code: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_verifier: Option<String>,
    pub refresh_token: Option<String>,
}
pub async fn token(
    State(app): State<App>,
    headers: HeaderMap,
    Form(q): Form<TokenRequest>,
) -> Result<Json<Value>> {
    app.store.throttle("token", 120)?;
    if q.resource != app.config.resource() {
        return Err(Error::new(
            400,
            "invalid_target",
            "Resource does not match this server",
        ));
    }
    let access = crypto::random();
    let refresh = crypto::random();
    let (grant,scopes,user)=app.store.with(|c|{
        let tx=c.transaction()?;
        let (grant,family)=match q.grant_type.as_str() {
            "authorization_code"=>{
                let code=q.code.as_deref().ok_or_else(||Error::invalid("Missing code"))?;
                let verifier=q.code_verifier.as_deref().ok_or_else(||Error::invalid("Missing verifier"))?;
                if !(43..=128).contains(&verifier.len()) || !verifier.bytes().all(|c|c.is_ascii_alphanumeric() || b"-._~".contains(&c)) {return Err(Error::invalid("Invalid verifier"))}
                let entry=tx.query_row("SELECT c.grant_id,c.challenge,c.redirect FROM codes c JOIN grants g ON g.id=c.grant_id WHERE c.hash=? AND c.expires>? AND c.resource=? AND g.client_id=? AND g.revoked=0",params![crypto::hash(code),now(),q.resource,q.client_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).optional()?.ok_or_else(||Error::new(400,"invalid_grant","Invalid or expired authorization code"))?;
                if crypto::hash(verifier)!=entry.1 || q.redirect_uri.as_deref()!=Some(&entry.2) {return Err(Error::new(400,"invalid_grant","PKCE or redirect mismatch"))}
                tx.execute("DELETE FROM codes WHERE hash=?",[crypto::hash(code)])?;
                (entry.0,crypto::random())
            },
            "refresh_token"=>{
                let raw=q.refresh_token.as_deref().ok_or_else(||Error::invalid("Missing refresh token"))?;
                let entry=tx.query_row("SELECT t.grant_id,t.family,t.used FROM tokens t JOIN grants g ON g.id=t.grant_id WHERE t.hash=? AND t.kind='refresh' AND t.resource=? AND t.expires>? AND g.client_id=? AND g.revoked=0",params![crypto::hash(raw),q.resource,now(),q.client_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,bool>(2)?))).optional()?.ok_or_else(||Error::new(400,"invalid_grant","Invalid refresh token"))?;
                if entry.2 {
                    tx.execute("DELETE FROM tokens WHERE family=?",[entry.1])?; tx.commit()?;
                    return Err(Error::new(400,"invalid_grant","Refresh token replay; reconnect client"));
                }
                tx.execute("UPDATE tokens SET used=1 WHERE hash=?",[crypto::hash(raw)])?;
                (entry.0,entry.1)
            },
            _=>return Err(Error::new(400,"unsupported_grant_type","Unsupported grant type")),
        };
        let (scopes,user)=tx.query_row("SELECT scopes,user_id FROM grants WHERE id=?",[&grant],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?)))?;
        let (status,email):(String,String)=tx.query_row("SELECT status,email FROM users WHERE id=?",[user],|r|Ok((r.get(0)?,r.get(1)?)))?;
        if status!="active" || !app.config.admits(&email) {return Err(Error::denied())}
        for (raw,kind,expires) in [(&access,"access",now()+900),(&refresh,"refresh",now()+30*86400)] {
            tx.execute("INSERT INTO tokens(hash,grant_id,kind,resource,expires,family) VALUES(?,?,?,?,?,?)",params![crypto::hash(raw),grant,kind,q.resource,expires,family])?;
        }
        tx.commit()?;Ok((grant,scopes,user))
    })?;
    let _ = grant;
    app.store.audit(
        crate::web::request_id(&headers),
        Some(user),
        Some(&q.client_id),
        "oauth.token",
        "ok",
    )?;
    Ok(Json(
        json!({"access_token":access,"token_type":"Bearer","expires_in":900,"refresh_token":refresh,"scope":scopes,"resource":q.resource}),
    ))
}

#[derive(Deserialize)]
pub struct Revoke {
    pub token: String,
    pub client_id: String,
}
pub async fn revoke(
    State(app): State<App>,
    Form(q): Form<Revoke>,
) -> Result<axum::http::StatusCode> {
    app.store.throttle("revoke", 120)?;
    app.store.with(|c|{c.execute("UPDATE grants SET revoked=1 WHERE client_id=? AND id IN (SELECT grant_id FROM tokens WHERE hash=?)",params![q.client_id,crypto::hash(&q.token)])?;Ok(())})?;
    Ok(axum::http::StatusCode::OK)
}

#[derive(Clone)]
pub struct Actor {
    pub user: User,
    pub client: String,
    pub scopes: Vec<String>,
    pub(crate) bearer_hash: Option<String>,
}
pub fn authenticate(app: &App, headers: &HeaderMap) -> Result<Actor> {
    let raw = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .filter(|s| s.len() <= 256)
        .ok_or_else(Error::unauthorized)?;
    let (user,client,scopes)=app.store.with(|c|c.query_row("SELECT g.user_id,g.client_id,g.scopes FROM tokens t JOIN grants g ON g.id=t.grant_id WHERE t.hash=? AND t.kind='access' AND t.used=0 AND t.expires>? AND t.resource=? AND g.revoked=0",params![crypto::hash(raw),now(),app.config.resource()],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).optional()?.ok_or_else(Error::unauthorized))?;
    let user = app.store.user(user)?;
    app.check_user(&user, true)?;
    Ok(Actor {
        user,
        client,
        scopes: scopes.split_whitespace().map(String::from).collect(),
        bearer_hash: Some(crypto::hash(raw)),
    })
}
pub fn allowed(app: &App, actor: &Actor, module: &str, write: bool) -> Result<()> {
    app.check_user(&app.store.user(actor.user.id)?, true)?;
    let scope = format!("{module}:{}", if write { "write" } else { "read" });
    // Provider renewal can yield to a concurrent revocation. Recheck the exact
    // token and its live grant before dispatch, including refresh-family replay.
    let live_scopes = if let Some(hash) = &actor.bearer_hash {
        Some(app.store.with(|c| {
            Ok(c.query_row(
                "SELECT g.scopes FROM tokens t JOIN grants g ON g.id=t.grant_id WHERE t.hash=? AND t.kind='access' AND t.used=0 AND t.expires>? AND t.resource=? AND g.revoked=0 AND g.user_id=? AND g.client_id=?",
                params![hash, now(), app.config.resource(), actor.user.id, actor.client],
                |r| r.get::<_, String>(0),
            ).optional()?)
        })?.ok_or_else(Error::unauthorized)?)
    } else {
        None
    };
    if !app.store.module_enabled(module)?
        || app.store.local_level(actor.user.id, module)? < (if write { 2 } else { 1 })
    {
        return Err(Error::denied());
    }
    if !actor.scopes.contains(&scope)
        || live_scopes
            .as_ref()
            .is_some_and(|s| !s.split_whitespace().any(|s| s == scope))
    {
        return Err(insufficient_scope(app, &scope));
    }
    if !available_scope(app, &actor.user, &scope)? {
        return Err(Error::reconnect());
    }
    Ok(())
}

fn insufficient_scope(app: &App, scope: &str) -> Error {
    Error::new(403, "insufficient_scope", "Approve the required scope in a new client connection")
        .with_context(json!({"scope":scope,"resource_metadata":format!("{}/.well-known/oauth-protected-resource",app.config.public_url)}))
}
