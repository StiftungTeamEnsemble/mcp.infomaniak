use crate::{Error, Result, config::Config};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Mutex;

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub struct Store {
    pub connection: Mutex<Connection>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub issuer: String,
    pub subject: String,
    pub email: String,
    pub name: String,
    pub status: String,
    pub admin: bool,
}

impl Store {
    pub fn open(path: &str) -> Result<Self> {
        if let Some(parent) = std::path::Path::new(path)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|_| Error::internal())?;
        }
        let mut c = Connection::open(path)?;
        #[cfg(unix)]
        if path != ":memory:" {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|_| Error::internal())?;
        }
        c.busy_timeout(std::time::Duration::from_secs(3))?;
        c.execute_batch("PRAGMA journal_mode=WAL;PRAGMA foreign_keys=ON;PRAGMA secure_delete=ON;PRAGMA journal_size_limit=1048576;")?;
        // Fresh databases start at the current version; others migrate in App::new.
        let fresh: bool = c.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table')",
            [],
            |r| r.get(0),
        )?;
        if fresh {
            let tx = c.transaction()?;
            tx.execute_batch(include_str!("schema.sql"))?;
            tx.pragma_update(None, "user_version", crate::migrations::VERSION)?;
            tx.commit()?;
        }
        Ok(Self {
            connection: Mutex::new(c),
        })
    }

    pub fn with<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut connection = self.connection.lock().map_err(|_| Error::internal())?;
        f(&mut connection)
    }

    pub fn user(&self, id: i64) -> Result<User> {
        self.with(|c| {
            c.query_row(
                "SELECT id,issuer,subject,email,name,status,admin FROM users WHERE id=?",
                [id],
                |r| {
                    Ok(User {
                        id: r.get(0)?,
                        issuer: r.get(1)?,
                        subject: r.get(2)?,
                        email: r.get(3)?,
                        name: r.get(4)?,
                        status: r.get(5)?,
                        admin: r.get(6)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(Error::unauthorized)
        })
    }

    pub fn admit(
        &self,
        cfg: &Config,
        issuer: &str,
        sub: &str,
        email: &str,
        name: &str,
    ) -> Result<User> {
        let admitted = cfg.admits(email);
        let id = self.with(|c| {
            let tx = c.transaction()?;
            let existing: Option<i64> = tx.query_row("SELECT id FROM users WHERE issuer=? AND subject=?", params![issuer,sub], |r|r.get(0)).optional()?;
            let id = if let Some(id) = existing {
                tx.execute("UPDATE users SET email=?,name=? WHERE id=?",params![email,name,id])?; id
            } else {
                if !admitted { return Err(Error::denied()) }
                let manual: bool = tx.query_row("SELECT manual_approval FROM settings WHERE id=1", [], |r|r.get(0))?;
                tx.execute("INSERT INTO users(issuer,subject,email,name,status) VALUES(?,?,?,?,?)",params![issuer,sub,email,name,if manual {"pending"} else {"active"}])?;
                let id = tx.last_insert_rowid();
                tx.execute("INSERT INTO policies(kind,principal,module,level) SELECT 'user',?,module,default_level FROM modules",[id])?;
                id
            };
            tx.commit()?; Ok(id)
        })?;
        // Retain a known identity's latest verified claims even when its new
        // domain is ineligible, so older sessions and grants fail closed too.
        if !admitted {
            return Err(Error::denied());
        }
        self.user(id)
    }

    pub fn local_level(&self, user: i64, module: &str) -> Result<i64> {
        self.with(|c| Ok(c.query_row("SELECT COALESCE(MAX(level),0) FROM policies WHERE module=? AND ((kind='user' AND principal=?) OR (kind='group' AND principal IN (SELECT group_id FROM memberships WHERE user_id=?)))",params![module,user,user],|r|r.get(0))?))
    }

    pub fn module_enabled(&self, module: &str) -> Result<bool> {
        self.with(|c| {
            Ok(c.query_row(
                "SELECT enabled AND length(verification)>0 FROM modules WHERE module=?",
                [module],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
        })
    }

    pub fn audit(
        &self,
        request: &str,
        actor: Option<i64>,
        client: Option<&str>,
        action: &str,
        outcome: &str,
    ) -> Result<()> {
        self.with(|c| {
            c.execute("INSERT INTO logs(at,severity,request_id,actor,client,action,outcome) VALUES(?,?,?,?,?,?,?)",params![now(),if outcome=="ok" {"info"} else {"warning"},request,actor,client,action,outcome])?;
            c.execute("DELETE FROM logs WHERE at<? OR id <= (SELECT COALESCE(MAX(id),0)-10000 FROM logs)",[now()-30*86400])?;
            Ok(())
        })
    }

    pub fn prune(&self) -> Result<()> {
        self.with(|c| {
            for table in ["flows", "sessions", "codes", "tokens"] {
                c.execute(&format!("DELETE FROM {table} WHERE expires<?"), [now()])?;
            }
            c.execute(
                "DELETE FROM clients WHERE created<? AND id NOT IN (SELECT client_id FROM grants)",
                [now() - 86400],
            )?;
            Ok(())
        })
    }

    pub fn throttle(&self, action: &str, max: i64) -> Result<()> {
        self.with(|c| {
            c.execute("INSERT INTO limits(action,window,count) VALUES(?,?,1) ON CONFLICT(action) DO UPDATE SET count=CASE WHEN window=excluded.window THEN count+1 ELSE 1 END,window=excluded.window",params![action,now()/60])?;
            let count: i64 = c.query_row("SELECT count FROM limits WHERE action=?",[action],|r|r.get(0))?;
            if count > max { return Err(Error::new(429,"rate_limited","Try again in a minute")) } Ok(())
        })
    }

    pub fn user_summary(&self, user: &User) -> Result<Value> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT provider,scopes,expires,health,checked FROM connections WHERE user_id=?")?;
            let connections = stmt.query_map([user.id],|r| Ok(json!({"provider":r.get::<_,String>(0)?,"scopes":r.get::<_,String>(1)?,"expires":r.get::<_,i64>(2)?,"health":r.get::<_,String>(3)?,"checked":r.get::<_,i64>(4)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
            let mut stmt=c.prepare("SELECT g.id,c.name,g.scopes FROM grants g JOIN clients c ON c.id=g.client_id WHERE g.user_id=? AND g.revoked=0")?;
            let grants=stmt.query_map([user.id],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"scopes":r.get::<_,String>(2)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
            Ok(json!({"user":user,"connections":connections,"clients":grants}))
        })
    }
}
