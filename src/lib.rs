pub mod config;
pub mod crypto;
pub mod identity;
pub mod mcp;
pub mod modules;
pub mod oauth;
pub mod store;
pub mod web;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::sync::Arc;

pub type Result<T> = std::result::Result<T, Error>;
pub struct Error {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: &'static str,
    pub context: Option<serde_json::Value>,
}
impl Error {
    pub fn new(status: u16, code: &'static str, message: &'static str) -> Self {
        Self {
            status: StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            code,
            message,
            context: None,
        }
    }
    /// Merges object contexts so provider diagnostics survive added recovery details.
    pub fn with_context(mut self, context: serde_json::Value) -> Self {
        self.context = match (self.context.take(), context) {
            (Some(serde_json::Value::Object(mut old)), serde_json::Value::Object(new)) => {
                old.extend(new);
                Some(serde_json::Value::Object(old))
            }
            (_, context) => Some(context),
        };
        self
    }
    /// Audit-safe provider rejection summary found anywhere in the context.
    pub fn provider_detail(&self) -> Option<String> {
        fn find(v: &serde_json::Value) -> Option<String> {
            let o = v.as_object()?;
            if let Some(status) = o.get("provider_status").and_then(|s| s.as_u64()) {
                let code = o
                    .get("provider_code")
                    .and_then(|c| c.as_str())
                    .unwrap_or("-");
                return Some(format!("provider {status} {code}"));
            }
            o.values().find_map(find)
        }
        find(self.context.as_ref()?)
    }
    pub fn internal() -> Self {
        Self::new(
            500,
            "internal_error",
            "The operation failed; see audit metadata",
        )
    }
    pub fn config(message: &'static str) -> Self {
        Self::new(500, "invalid_configuration", message)
    }
    pub fn invalid(message: &'static str) -> Self {
        Self::new(400, "invalid_request", message)
    }
    pub fn unauthorized() -> Self {
        Self::new(401, "invalid_token", "Sign in or reconnect")
    }
    pub fn denied() -> Self {
        Self::new(
            403,
            "access_denied",
            "Access denied by current policy or consent",
        )
    }
    pub fn reconnect() -> Self {
        Self::new(
            403,
            "reconnection_required",
            "Reconnect this service in the user interface",
        )
    }
}
impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}
impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self {
        Self::internal()
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::invalid("Invalid JSON")
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let challenge = self
            .context
            .as_ref()
            .filter(|_| self.code == "insufficient_scope")
            .and_then(|c| Some((c["scope"].as_str()?, c["resource_metadata"].as_str()?)))
            .and_then(|(scope, metadata)| {
                let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
                axum::http::HeaderValue::from_str(&format!(
                    "Bearer error=\"insufficient_scope\", scope=\"{}\", resource_metadata=\"{}\"",
                    quote(scope),
                    quote(metadata)
                ))
                .ok()
            });
        let mut body = json!({"error":self.code,"error_description":self.message});
        if let Some(context) = self.context {
            body["context"] = context;
        }
        let mut response = (self.status, Json(body)).into_response();
        if let Some(challenge) = challenge {
            response
                .headers_mut()
                .insert(axum::http::header::WWW_AUTHENTICATE, challenge);
        }
        response
    }
}

#[derive(Clone)]
pub struct App {
    pub config: Arc<config::Config>,
    pub store: Arc<store::Store>,
    pub provider: Arc<identity::Provider>,
    pub modules: Arc<modules::Registry>,
}
impl App {
    pub fn new(
        config: config::Config,
        store: store::Store,
        provider: identity::Provider,
    ) -> Result<Self> {
        let modules = modules::Registry::builtin()?;
        store.with(|c| {
            use rusqlite::OptionalExtension;
            let sentinel: Option<Vec<u8>> = c
                .query_row(
                    "SELECT value FROM secrets WHERE purpose='encryption-key'",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(sentinel) = sentinel {
                crypto::open(&config.encryption_key, "encryption-key", &sentinel).map_err(
                    |_| Error::config("CREDENTIAL_ENCRYPTION_KEY does not match this database"),
                )?;
            } else {
                let value =
                    crypto::seal(&config.encryption_key, "encryption-key", b"ensemble-mcp")?;
                c.execute(
                    "INSERT INTO secrets(purpose,value) VALUES('encryption-key',?)",
                    [value],
                )?;
            }
            for m in &modules.modules {
                c.execute("INSERT OR IGNORE INTO modules(module) VALUES(?)", [m.id])?;
            }
            Ok(())
        })?;
        Ok(Self {
            config: Arc::new(config),
            store: Arc::new(store),
            provider: Arc::new(provider),
            modules: Arc::new(modules),
        })
    }
    pub fn check_user(&self, user: &store::User, active: bool) -> Result<()> {
        if !self.config.admits(&user.email)
            || user.status == "suspended"
            || (active && user.status != "active")
        {
            return Err(Error::denied());
        }
        Ok(())
    }
}
