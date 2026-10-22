//! Versioned schema changes; SQLite's `user_version` records the applied version.
//! Fresh databases are created at `VERSION` from `schema.sql`. Databases created
//! before versioning (2026-10-22) have version 0. Migrations are frozen history:
//! never edit a released step, add the next one instead.
use crate::{Error, Result, crypto, identity};
use rusqlite::{Connection, Transaction, params};

pub const VERSION: i64 = 1;

/// Applies pending migrations in order, each atomically with its version bump.
pub fn run(c: &mut Connection, key: &[u8; 32]) -> Result<()> {
    let mut version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > VERSION {
        return Err(Error::config("Database schema is newer than this server"));
    }
    while version < VERSION {
        let tx = c.transaction()?;
        match version {
            0 => provider_credentials(&tx, key)?,
            _ => return Err(Error::internal()),
        }
        version += 1;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

/// v0 → v1: per-service credentials become one credential per user and provider.
/// Every earlier service was an Infomaniak service using the same token. Prefers
/// a token Infomaniak has not rejected, then the most recently used one; personal
/// tokens get the all-services marker; undecryptable rows are dropped.
fn provider_credentials(tx: &Transaction, key: &[u8; 32]) -> Result<()> {
    tx.execute_batch("CREATE TABLE IF NOT EXISTS connections(user_id INTEGER REFERENCES users(id),provider TEXT NOT NULL,secret BLOB NOT NULL,scopes TEXT NOT NULL,expires INTEGER NOT NULL,health TEXT NOT NULL DEFAULT 'connected',checked INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(user_id,provider));")?;
    let rows = {
        let mut stmt = tx.prepare("SELECT user_id,module,secret,scopes,expires,health,checked FROM credentials ORDER BY user_id,health='reconnection_required',checked DESC")?;
        stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?
    };
    let provider = "infomaniak";
    for (user, module, secret, scopes, expires, health, checked) in rows {
        let Ok(plain) = crypto::open(key, &format!("credential:{user}:{module}"), &secret) else {
            continue;
        };
        let Ok(credential) = serde_json::from_slice::<identity::Credential>(&plain) else {
            continue;
        };
        let scopes = if credential.refresh_token.is_none() {
            identity::PERSONAL_SCOPES.to_owned()
        } else {
            scopes
        };
        let secret = crypto::seal(key, &identity::credential_context(user, provider), &plain)?;
        tx.execute(
            "INSERT OR IGNORE INTO connections(user_id,provider,secret,scopes,expires,health,checked) VALUES(?,?,?,?,?,?,?)",
            params![user, provider, secret, scopes, expires, health, checked],
        )?;
    }
    tx.execute("DROP TABLE credentials", [])?;
    Ok(())
}
