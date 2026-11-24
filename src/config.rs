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
    /// Individual lowercase addresses admitted in addition to `allowed_domains`.
    pub allowed_emails: HashSet<String>,
    pub bind_addr: String,
    /// Optional kChat team name; a lowercase DNS label forming the kChat host.
    pub kchat_team: Option<String>,
    /// Portal language as a BCP 47 tag from [`UI_LOCALES`].
    pub ui_locale: &'static str,
}

/// Languages the portal ships translations for; the first is the default.
pub const UI_LOCALES: [&str; 2] = ["en", "de-CH"];

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
        if !allowed_domains.iter().all(|d| valid_domain(d)) {
            return Err(Error::config("ALLOWED_EMAIL_DOMAINS"));
        }
        let allowed_emails = get("ALLOWED_EMAILS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect::<HashSet<_>>();
        if !allowed_emails.iter().all(|e| {
            e.rsplit_once('@').is_some_and(|(local, domain)| {
                !local.is_empty()
                    && !local.contains('@')
                    && !local.chars().any(|c| c.is_whitespace() || c.is_control())
                    && valid_domain(domain)
            })
        }) {
            return Err(Error::config("ALLOWED_EMAILS"));
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
        let ui_locale = match get("UI_LOCALE").filter(|v| !v.trim().is_empty()) {
            None => UI_LOCALES[0],
            Some(v) => UI_LOCALES
                .into_iter()
                .find(|l| l.eq_ignore_ascii_case(v.trim()))
                .ok_or_else(|| Error::config("UI_LOCALE"))?,
        };
        Ok(Self {
            public_url: origin.origin().ascii_serialization(),
            sqlite_path: required("SQLITE_PATH")?,
            encryption_key,
            client_id: required("INFOMANIAK_CLIENT_ID")?,
            client_secret: required("INFOMANIAK_CLIENT_SECRET")?,
            allowed_domains,
            allowed_emails,
            bind_addr,
            kchat_team,
            ui_locale,
        })
    }

    pub fn admits(&self, email: &str) -> bool {
        let Some((local, domain)) = email.rsplit_once('@') else {
            return false;
        };
        !local.is_empty()
            && !local.contains('@')
            && !email.chars().any(|c| c.is_whitespace() || c.is_control())
            && (self.allowed_domains.contains(&domain.to_ascii_lowercase())
                || self.allowed_emails.contains(&email.to_ascii_lowercase()))
    }

    pub fn resource(&self) -> String {
        format!("{}/mcp", self.public_url)
    }
    pub fn callback(&self) -> String {
        format!("{}/auth/infomaniak/callback", self.public_url)
    }
}

fn valid_domain(d: &str) -> bool {
    d.contains('.')
        && d.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}
