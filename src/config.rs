use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{collections::HashSet, env};
use url::Url;

#[derive(Clone)]
pub struct Config {
    pub public_url: String,
    pub sqlite_path: String,
    pub encryption_key: [u8; 32],
    pub client_id: String,
    pub client_secret: String,
    pub allowed_domains: HashSet<String>,
    pub bind_addr: String,
    /// Optional kChat team name; a lowercase DNS label forming the kChat host.
    pub kchat_team: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Self::load(|k| env::var(k).ok())
    }

    pub fn load(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let required = |k| {
            get(k)
                .filter(|v| !v.trim().is_empty() && !v.contains('<'))
                .ok_or_else(|| Error::config(k))
        };
        let raw = required("PUBLIC_URL")?;
        let origin = Url::parse(&raw).map_err(|_| Error::config("PUBLIC_URL"))?;
        if origin.scheme() != "https"
            || origin.host_str().is_none()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
        {
            return Err(Error::config("PUBLIC_URL must be an HTTPS origin"));
        }
        let key = STANDARD
            .decode(required("CREDENTIAL_ENCRYPTION_KEY")?)
            .map_err(|_| Error::config("CREDENTIAL_ENCRYPTION_KEY"))?;
        let encryption_key = key
            .try_into()
            .map_err(|_| Error::config("CREDENTIAL_ENCRYPTION_KEY must decode to 32 bytes"))?;
        let allowed_domains = required("ALLOWED_EMAIL_DOMAINS")?
            .split(',')
            .map(|s| s.trim().to_ascii_lowercase())
            .collect::<HashSet<_>>();
        if allowed_domains.iter().any(|d| {
            d.is_empty()
                || !d.contains('.')
                || d.starts_with('.')
                || d.ends_with('.')
                || d.split('.').any(|label| {
                    label.is_empty()
                        || label.starts_with('-')
                        || label.ends_with('-')
                        || !label
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
                })
        }) {
            return Err(Error::config("ALLOWED_EMAIL_DOMAINS"));
        }
        let bind_addr = get("BIND_ADDR").unwrap_or_else(|| "127.0.0.1:3000".into());
        bind_addr
            .parse::<std::net::SocketAddr>()
            .map_err(|_| Error::config("BIND_ADDR"))?;
        let kchat_team = get("KCHAT_TEAM")
            .map(|v| v.trim().to_ascii_lowercase())
            .filter(|v| !v.is_empty());
        if kchat_team.as_ref().is_some_and(|t| {
            t.len() > 63
                || t.starts_with('-')
                || t.ends_with('-')
                || !t.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        }) {
            return Err(Error::config("KCHAT_TEAM"));
        }
        Ok(Self {
            public_url: origin.origin().ascii_serialization(),
            sqlite_path: required("SQLITE_PATH")?,
            encryption_key,
            client_id: required("INFOMANIAK_CLIENT_ID")?,
            client_secret: required("INFOMANIAK_CLIENT_SECRET")?,
            allowed_domains,
            bind_addr,
            kchat_team,
        })
    }

    pub fn admits(&self, email: &str) -> bool {
        let Some((local, domain)) = email.rsplit_once('@') else {
            return false;
        };
        !local.is_empty()
            && !local.contains('@')
            && !email.chars().any(|c| c.is_whitespace() || c.is_control())
            && self.allowed_domains.contains(&domain.to_ascii_lowercase())
    }

    pub fn resource(&self) -> String {
        format!("{}/mcp", self.public_url)
    }
    pub fn callback(&self) -> String {
        format!("{}/auth/infomaniak/callback", self.public_url)
    }
}
