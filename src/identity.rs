//! Delegated identity only. Production endpoints come exclusively from the trusted issuer.
use crate::{
    App, Error, Result, crypto,
    store::{User, now},
};
use axum::{
    extract::{Query, State},
    http::{HeaderMap, header::SET_COOKIE},
    response::{IntoResponse, Redirect, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use reqwest::{Client, Method};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use url::Url;

pub const ISSUER: &str = "https://login.infomaniak.com";
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Deserialize)]
pub struct Metadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    pub code_challenge_methods_supported: Vec<String>,
}

type ConnectionLocks = Mutex<HashMap<(i64, String), Arc<Mutex<()>>>>;
pub struct Provider {
    pub metadata: Metadata,
    pub http: Client,
    pub api: String,
    pub mail: String,
    locks: ConnectionLocks,
}

impl Provider {
    pub fn http_client() -> Result<Client> {
        Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .user_agent("ensemble-mcp/0.1")
            .build()
            .map_err(|_| Error::internal())
    }

    pub async fn discover() -> Result<Self> {
        let http = Self::http_client()?;
        let metadata: Metadata = serde_json::from_value(
            Self::json(
                http.get(format!("{ISSUER}/.well-known/openid-configuration")),
                false,
            )
            .await?,
        )?;
        if metadata.issuer != ISSUER
            || !metadata
                .code_challenge_methods_supported
                .iter()
                .any(|s| s == "S256")
        {
            return Err(Error::config("Untrusted OIDC discovery"));
        }
        for endpoint in [
            &metadata.authorization_endpoint,
            &metadata.token_endpoint,
            &metadata.jwks_uri,
        ] {
            let url = Url::parse(endpoint).map_err(|_| Error::config("OIDC endpoint"))?;
            if url.origin().ascii_serialization() != ISSUER
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
            {
                return Err(Error::config("Untrusted OIDC endpoint"));
            }
        }
        Ok(Self::with_endpoints(
            metadata,
            http,
            "https://api.infomaniak.com".into(),
            "https://mail.infomaniak.com/api".into(),
        ))
    }

    /// Dependency injection for in-process provider contract tests; never configurable over HTTP or .env.
    pub fn with_endpoints(metadata: Metadata, http: Client, api: String, mail: String) -> Self {
        Self {
            metadata,
            http,
            api,
            mail,
            locks: Mutex::new(HashMap::new()),
        }
    }

    pub async fn bytes(
        builder: reqwest::RequestBuilder,
        mutation: bool,
    ) -> Result<(HeaderMap, Vec<u8>)> {
        let mut response = builder.send().await.map_err(|_| {
            if mutation {
                Error::new(
                    502,
                    "mutation_uncertain",
                    "Provider response was lost; verify the outcome before repeating",
                )
            } else {
                Error::new(
                    502,
                    "provider_unavailable",
                    "Provider unavailable; try again later",
                )
            }
        })?;
        let status = response.status();
        if status == 401 {
            return Err(Error::reconnect());
        }
        if status == 403 {
            return Err(Error::denied());
        }
        if status == 404 {
            return Err(Error::new(
                502,
                "provider_not_found",
                "Provider object was not found",
            ));
        }
        if !status.is_success() {
            return Err(Error::new(
                502,
                if mutation && status.is_server_error() {
                    "mutation_uncertain"
                } else {
                    "provider_error"
                },
                "Provider rejected the operation; verify any mutation outcome before repeating",
            )
            .with_context(provider_rejection(status.as_u16(), response).await));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return Err(Self::response_too_large(mutation));
        }
        let headers = response.headers().clone();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            Error::new(
                502,
                if mutation {
                    "mutation_uncertain"
                } else {
                    "provider_error"
                },
                "Provider response interrupted; verify mutations before repeating",
            )
        })? {
            if bytes.len() + chunk.len() > MAX_BYTES {
                return Err(Self::response_too_large(mutation));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((headers, bytes))
    }

    fn response_too_large(mutation: bool) -> Error {
        if mutation {
            Error::new(
                502,
                "mutation_uncertain",
                "Provider response exceeds 8 MiB; verify the mutation outcome before repeating",
            )
        } else {
            Error::new(
                413,
                "content_too_large",
                "Content exceeds the 8 MiB transfer limit",
            )
        }
    }

    pub async fn json(builder: reqwest::RequestBuilder, mutation: bool) -> Result<Value> {
        let (_, bytes) = Self::bytes(builder, mutation).await?;
        if bytes.is_empty() {
            return Ok(json!({"result":"success"}));
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
            Error::new(
                502,
                if mutation {
                    "mutation_uncertain"
                } else {
                    "provider_error"
                },
                "Invalid provider response; verify mutations before repeating",
            )
        })?;
        if value
            .get("result")
            .is_some_and(|v| v == "error" || v == "failed")
            || value.get("error").is_some()
        {
            return Err(Error::new(
                502,
                "provider_error",
                "Provider reported a failure; verify any partial outcome",
            ));
        }
        Ok(value)
    }

    pub async fn verify_id(
        &self,
        token: &str,
        client_id: &str,
        nonce: Option<&str>,
    ) -> Result<Claims> {
        let header = decode_header(token).map_err(|_| Error::unauthorized())?;
        if header.alg != Algorithm::RS256 {
            return Err(Error::unauthorized());
        }
        let jwks: JwkSet = serde_json::from_value(
            Self::json(self.http.get(&self.metadata.jwks_uri), false).await?,
        )
        .map_err(|_| Error::unauthorized())?;
        let key = jwks
            .find(header.kid.as_deref().ok_or_else(Error::unauthorized)?)
            .ok_or_else(Error::unauthorized)?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[ISSUER]);
        validation.set_audience(&[client_id]);
        validation.leeway = 30;
        validation.validate_nbf = true;
        let claims = decode::<Claims>(
            token,
            &DecodingKey::from_jwk(key).map_err(|_| Error::unauthorized())?,
            &validation,
        )
        .map_err(|_| Error::unauthorized())?
        .claims;
        if claims.sub.is_empty()
            || claims.iat > now() + 30
            || claims.iat > claims.exp as i64
            || !claims.email_verified
            || nonce.is_some_and(|n| claims.nonce.as_deref() != Some(n))
            || claims.azp.as_deref().is_some_and(|a| a != client_id)
            || claims
                .aud
                .as_array()
                .is_some_and(|a| a.len() > 1 && claims.azp.as_deref() != Some(client_id))
        {
            return Err(Error::unauthorized());
        }
        Ok(claims)
    }

    pub async fn access_token(&self, app: &App, user: &User, module: &str) -> Result<String> {
        let lock = {
            let mut locks = self.locks.lock().await;
            locks.retain(|_, v| Arc::strong_count(v) > 1);
            locks.entry((user.id, module.into())).or_default().clone()
        };
        let _guard = lock.lock().await;
        let (secret, scopes, expires) = app.store.with(|c| {
            c.query_row(
                "SELECT secret,scopes,expires FROM credentials WHERE user_id=? AND module=?",
                params![user.id, module],
                |r| {
                    Ok((
                        r.get::<_, Vec<u8>>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(Error::reconnect)
        })?;
        let context = format!("credential:{}:{module}", user.id);
        let mut credential: Credential = serde_json::from_slice(&crypto::open(
            &app.config.encryption_key,
            &context,
            &secret,
        )?)?;
        if expires > now() + 60 {
            return Ok(credential.access_token);
        }
        let refresh = credential
            .refresh_token
            .as_deref()
            .ok_or_else(Error::reconnect)?;
        let response = Self::json(
            self.http.post(&self.metadata.token_endpoint).form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh),
                ("client_id", &app.config.client_id),
                ("client_secret", &app.config.client_secret),
            ]),
            true,
        )
        .await
        .map_err(|_| Error::reconnect())?;
        if let Some(id) = response["id_token"].as_str() {
            let claims = self.verify_id(id, &app.config.client_id, None).await?;
            if claims.sub != user.subject {
                return Err(Error::denied());
            }
            app.store.admit(
                &app.config,
                &claims.iss,
                &claims.sub,
                &claims.email,
                &claims.name,
            )?;
        }
        let renewed_scopes = response["scope"].as_str().unwrap_or(&scopes);
        if !app
            .modules
            .get(module)?
            .scopes
            .iter()
            .all(|s| renewed_scopes.split_whitespace().any(|v| v == *s))
        {
            return Err(Error::reconnect());
        }
        credential.access_token = response["access_token"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(Error::reconnect)?
            .into();
        if let Some(token) = response["refresh_token"].as_str() {
            credential.refresh_token = Some(token.into());
        }
        let expires = now()
            + response["expires_in"]
                .as_i64()
                .filter(|n| *n > 0 && *n <= 86400 * 30)
                .ok_or_else(Error::reconnect)?;
        let previous = secret;
        let secret = crypto::seal(
            &app.config.encryption_key,
            &context,
            &serde_json::to_vec(&credential)?,
        )?;
        // Do not resurrect a connection revoked while its refresh request was in flight.
        app.store.with(|c| {
            let changed = c.execute(
                "UPDATE credentials SET secret=?,expires=?,scopes=? WHERE user_id=? AND module=? AND secret=?",
                params![secret, expires, renewed_scopes, user.id, module,previous],
            )?;
            if changed != 1 {
                return Err(Error::reconnect());
            }
            Ok(())
        })?;
        app.store
            .audit("refresh", Some(user.id), None, "credential.refresh", "ok")?;
        Ok(credential.access_token)
    }

    pub async fn request(
        &self,
        token: &str,
        method: Method,
        url: String,
        query: &[(String, String)],
        body: Option<Value>,
    ) -> Result<Value> {
        let mutation = method != Method::GET;
        let mut req = self
            .http
            .request(method, url)
            .bearer_auth(token)
            .query(query);
        if let Some(body) = body {
            req = req.json(&body);
        }
        Self::json(req, mutation).await
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Claims {
    pub iss: String,
    pub sub: String,
    pub aud: Value,
    pub exp: u64,
    pub iat: i64,
    pub email: String,
    pub email_verified: bool,
    #[serde(default)]
    pub name: String,
    pub nonce: Option<String>,
    pub azp: Option<String>,
}
#[derive(Serialize, Deserialize)]
pub struct Credential {
    pub access_token: String,
    pub refresh_token: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Login {
    verifier: String,
    nonce: String,
    module: Option<String>,
    user: Option<i64>,
    authorize: Option<String>,
}
#[derive(Deserialize)]
pub struct LoginQuery {
    pub authorize: Option<String>,
}

pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get("cookie")?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|c| {
            c.trim()
                .split_once('=')
                .filter(|(k, _)| *k == name)
                .map(|(_, v)| v.to_owned())
        })
}
pub fn set_cookie(name: &str, value: &str, age: i64) -> String {
    format!("{name}={value}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age={age}")
}

pub async fn start(
    app: &App,
    module: Option<String>,
    user: Option<i64>,
    authorize: Option<String>,
) -> Result<Response> {
    app.store.throttle("login", 60)?;
    app.store.prune()?;
    let state = crypto::random();
    let binding = crypto::random();
    let verifier = crypto::random();
    let nonce = crypto::random();
    let mut scopes = vec!["openid", "profile", "email"];
    if let Some(module) = module.as_deref() {
        scopes.extend(&app.modules.get(module)?.scopes);
    }
    let mut url =
        Url::parse(&app.provider.metadata.authorization_endpoint).map_err(|_| Error::internal())?;
    url.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", &app.config.client_id),
        ("redirect_uri", &app.config.callback()),
        ("scope", &scopes.join(" ")),
        ("state", &state),
        ("nonce", &nonce),
        ("code_challenge", &crypto::hash(&verifier)),
        ("code_challenge_method", "S256"),
    ]);
    let login = Login {
        verifier,
        nonce,
        module,
        user,
        authorize,
    };
    let secret = crypto::seal(
        &app.config.encryption_key,
        &format!("login:{}", crypto::hash(&state)),
        &serde_json::to_vec(&login)?,
    )?;
    app.store.with(|c| {
        c.execute(
            "INSERT INTO flows(hash,binding,secret,expires) VALUES(?,?,?,?)",
            params![
                crypto::hash(&state),
                crypto::hash(&binding),
                secret,
                now() + 600
            ],
        )?;
        Ok(())
    })?;
    Ok((
        [(SET_COOKIE, set_cookie("__Host-mcp_flow", &binding, 600))],
        Redirect::to(url.as_str()),
    )
        .into_response())
}

pub async fn login(State(app): State<App>, Query(q): Query<LoginQuery>) -> Result<Response> {
    start(&app, None, None, q.authorize).await
}
#[derive(Deserialize)]
pub struct Callback {
    pub state: String,
    pub code: Option<String>,
    pub error: Option<String>,
}
pub async fn callback(
    State(app): State<App>,
    headers: HeaderMap,
    Query(q): Query<Callback>,
) -> Result<Response> {
    let binding = cookie(&headers, "__Host-mcp_flow").ok_or_else(Error::unauthorized)?;
    let state_hash = crypto::hash(&q.state);
    let secret = app.store.with(|c| {
        c.query_row(
            "DELETE FROM flows WHERE hash=? AND binding=? AND expires>? RETURNING secret",
            params![state_hash, crypto::hash(&binding), now()],
            |r| r.get::<_, Vec<u8>>(0),
        )
        .optional()?
        .ok_or_else(Error::unauthorized)
    })?;
    let flow: Login = serde_json::from_slice(&crypto::open(
        &app.config.encryption_key,
        &format!("login:{state_hash}"),
        &secret,
    )?)?;
    if q.error.is_some() {
        return Err(Error::new(
            400,
            "consent_denied",
            "Infomaniak sign-in or consent was declined",
        ));
    }
    let code = q
        .code
        .ok_or_else(|| Error::invalid("Missing authorization code"))?;
    let result = Provider::json(
        app.provider
            .http
            .post(&app.provider.metadata.token_endpoint)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("code_verifier", &flow.verifier),
                ("redirect_uri", &app.config.callback()),
                ("client_id", &app.config.client_id),
                ("client_secret", &app.config.client_secret),
            ]),
        true,
    )
    .await?;
    let claims = app
        .provider
        .verify_id(
            result["id_token"]
                .as_str()
                .ok_or_else(Error::unauthorized)?,
            &app.config.client_id,
            Some(&flow.nonce),
        )
        .await?;
    let user = app.store.admit(
        &app.config,
        &claims.iss,
        &claims.sub,
        &claims.email,
        &claims.name,
    )?;
    app.check_user(&user, false)?;
    if let Some(expected) = flow.user {
        let (session_user, _) = crate::web::session(&app, &headers, false)?;
        if user.id != expected || session_user.id != expected {
            return Err(Error::denied());
        }
    }
    if let Some(module) = flow.module {
        app.check_user(&user, true)?;
        if app.store.local_level(user.id, &module)? == 0 || !app.store.module_enabled(&module)? {
            return Err(Error::denied());
        }
        let scopes = result["scope"].as_str().ok_or_else(Error::reconnect)?;
        if !app
            .modules
            .get(&module)?
            .scopes
            .iter()
            .all(|s| scopes.split_whitespace().any(|v| v == *s))
        {
            return Err(Error::reconnect());
        }
        let credential = Credential {
            access_token: result["access_token"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(Error::reconnect)?
                .into(),
            refresh_token: result["refresh_token"].as_str().map(String::from),
        };
        let expires = now()
            + result["expires_in"]
                .as_i64()
                .filter(|n| *n > 0 && *n <= 86400 * 30)
                .ok_or_else(Error::reconnect)?;
        let secret = crypto::seal(
            &app.config.encryption_key,
            &format!("credential:{}:{module}", user.id),
            &serde_json::to_vec(&credential)?,
        )?;
        app.store.with(|c|{c.execute("INSERT INTO credentials(user_id,module,secret,scopes,expires) VALUES(?,?,?,?,?) ON CONFLICT(user_id,module) DO UPDATE SET secret=excluded.secret,scopes=excluded.scopes,expires=excluded.expires,health='connected',checked=0",params![user.id,module,secret,scopes,expires])?;Ok(())})?;
        app.store.audit(
            crate::web::request_id(&headers),
            Some(user.id),
            None,
            "credential.connect",
            "ok",
        )?;
    }
    if let Some(old) = cookie(&headers, "__Host-mcp_session") {
        app.store.with(|c| {
            c.execute("DELETE FROM sessions WHERE hash=?", [crypto::hash(&old)])?;
            Ok(())
        })?;
    }
    let session = crypto::random();
    let csrf = crypto::random();
    app.store.with(|c| {
        c.execute(
            "INSERT INTO sessions(hash,user_id,csrf,expires) VALUES(?,?,?,?)",
            params![crypto::hash(&session), user.id, csrf, now() + 28800],
        )?;
        Ok(())
    })?;
    app.store.audit(
        crate::web::request_id(&headers),
        Some(user.id),
        None,
        "authentication.login",
        "ok",
    )?;
    let dest = flow
        .authorize
        .map(|a| {
            format!(
                "/?authorize={}",
                url::form_urlencoded::byte_serialize(a.as_bytes()).collect::<String>()
            )
        })
        .unwrap_or_else(|| "/".into());
    let mut response = Redirect::to(&dest).into_response();
    response.headers_mut().append(
        SET_COOKIE,
        set_cookie("__Host-mcp_session", &session, 28800)
            .parse()
            .map_err(|_| Error::internal())?,
    );
    response.headers_mut().append(
        SET_COOKIE,
        set_cookie("__Host-mcp_flow", "", 0)
            .parse()
            .map_err(|_| Error::internal())?,
    );
    Ok(response)
}

/// Safe diagnostics only: the status and the provider's machine-readable error code, never descriptions or bodies.
async fn provider_rejection(status: u16, mut response: reqwest::Response) -> Value {
    let mut bytes = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        if bytes.len() + chunk.len() > 65536 {
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    let code = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| {
            v.pointer("/error/code")
                .or_else(|| v.get("error"))
                .and_then(Value::as_str)
                .map(String::from)
        })
        .filter(|c| {
            c.len() <= 64
                && c.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-:".contains(&b))
        });
    json!({"provider_status":status,"provider_code":code})
}

pub fn binary(uri: String, filename: String, mime: String, bytes: Vec<u8>) -> Value {
    let encoded = STANDARD.encode(&bytes);
    let content = if mime.starts_with("image/") {
        json!({"type":"image","mimeType":mime,"data":encoded})
    } else {
        json!({"type":"resource","resource":{"uri":uri,"mimeType":mime,"blob":encoded}})
    };
    json!({"filename":filename,"mime_type":mime,"size":bytes.len(),"content":[content]})
}
