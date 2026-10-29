//! Optional admission evidence; never supplies a member's service credentials.
use super::{Claims, Credential, ISSUER, Provider, authentication_error, credential_context};
use crate::{App, Error, Result, config::provider_id, crypto, store::now};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

fn unavailable() -> Error {
    Error::new(
        503,
        "membership_unavailable",
        "Organisation membership could not be checked; contact your administrator",
    )
    .with_context(json!({"authentication_failure":"membership_lookup_unavailable"}))
}

fn id(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(provider_id))
}

impl Provider {
    pub(super) async fn verify_member(&self, app: &App, claims: &Claims) -> Result<()> {
        let membership = app
            .config
            .membership
            .as_ref()
            .ok_or_else(|| authentication_error("email_not_verified"))?;
        let subject = provider_id(&claims.sub)
            .ok_or_else(|| authentication_error("membership_identity_mismatch"))?;
        // Reuse the designated administrator's existing, owner-checked personal token.
        // No directory token is copied into a member's connection or returned to clients.
        let reader_id = membership.reader_id.to_string();
        let (reader, secret) = app.store.with(|c| {
            c.query_row(
                "SELECT u.id,c.secret FROM users u JOIN connections c ON c.user_id=u.id WHERE u.issuer=? AND u.subject=? AND u.admin=1 AND u.status='active' AND c.provider='infomaniak' AND c.scopes='*' AND c.expires>? AND c.health='connected'",
                params![ISSUER,reader_id,now()],
                |r| Ok((r.get::<_,i64>(0)?,r.get::<_,Vec<u8>>(1)?)),
            ).optional()?.ok_or_else(unavailable)
        })?;
        app.check_user(&app.store.user(reader)?, true)
            .map_err(|_| unavailable())?;
        let credential: Credential = serde_json::from_slice(&crypto::open(
            &app.config.encryption_key,
            &credential_context(reader, "infomaniak"),
            &secret,
        )?)?;
        if credential.refresh_token.is_some() || credential.access_token.is_empty() {
            return Err(unavailable());
        }
        // Verified live 2026-10-29: GET https://api.infomaniak.com/1/account/{account}/user/{subject}.
        let response = self
            .request(
                &credential.access_token,
                reqwest::Method::GET,
                format!(
                    "{}/1/account/{}/user/{subject}",
                    self.api, membership.account_id
                ),
                &[],
                None,
            )
            .await
            .map_err(|error| {
                if error.code == "provider_not_found" {
                    authentication_error("membership_not_confirmed")
                } else {
                    unavailable()
                }
            })?;
        let member = response
            .get("data")
            .and_then(Value::as_object)
            .ok_or_else(|| authentication_error("membership_not_confirmed"))?;
        if member.get("id").and_then(id) != Some(subject)
            || member
                .get("account_id")
                .is_some_and(|value| id(value) != Some(membership.account_id))
            || member
                .get("email")
                .and_then(Value::as_str)
                .is_none_or(|email| !email.eq_ignore_ascii_case(&claims.email))
        {
            return Err(authentication_error("membership_identity_mismatch"));
        }
        if member.get("status").and_then(Value::as_str) != Some("valid")
            || member.get("type").and_then(Value::as_str) != Some("normal")
            || member.get("is_blocked").and_then(Value::as_bool) != Some(false)
            || member.get("invalid_email").and_then(Value::as_bool) != Some(false)
        {
            return Err(authentication_error("membership_not_confirmed"));
        }
        // Recheck the exact credential and current reader policy after the asynchronous read.
        let current: bool = app.store.with(|c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM users u JOIN connections c ON c.user_id=u.id WHERE u.id=? AND u.issuer=? AND u.subject=? AND u.admin=1 AND u.status='active' AND c.provider='infomaniak' AND c.scopes='*' AND c.expires>? AND c.health='connected' AND c.secret=?)",
                params![reader,ISSUER,reader_id,now(),secret], |r| r.get(0),
            )?)
        })?;
        if !current || app.check_user(&app.store.user(reader)?, true).is_err() {
            return Err(unavailable());
        }
        Ok(())
    }
}
