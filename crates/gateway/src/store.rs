//! Gateway persistence on SQLite (`<data-dir>/gateway.db`). PostgreSQL is the
//! documented target for multi-instance deployments (ADR-0030 deviation); the
//! whole surface is this one type, so swapping the backend is local.

use std::path::Path;

use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Hash of a bearer secret (only hashes are stored).
#[must_use]
pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// A registered passkey.
#[derive(Debug, Clone)]
pub struct CredentialRow {
    /// Account.
    pub account_id: String,
    /// Credential.
    pub credential: orkestr_security::webauthn::Credential,
}

/// A core.
#[derive(Debug, Clone)]
pub struct CoreRow {
    /// Core id.
    pub core_id: String,
    /// Account.
    pub account_id: String,
    /// CIK (SEC1, base64url).
    pub cik_public: String,
    /// Display name.
    pub name: String,
    /// Last tunnel connection (unix seconds).
    pub last_seen: Option<i64>,
}

/// SQLite store.
#[derive(Debug)]
pub struct Store {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS accounts (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at INTEGER NOT NULL,
  totp_secret BLOB, totp_enabled INTEGER NOT NULL DEFAULT 0, totp_last_step INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS credentials (
  id BLOB PRIMARY KEY, account_id TEXT NOT NULL, public_key BLOB NOT NULL, sign_count INTEGER NOT NULL, created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS sessions (
  token_hash TEXT PRIMARY KEY, account_id TEXT NOT NULL, created_at INTEGER NOT NULL, last_seen INTEGER NOT NULL, expires_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS enrollment_tokens (
  token_hash TEXT PRIMARY KEY, account_id TEXT NOT NULL, expires_at INTEGER NOT NULL, used INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS cores (
  core_id TEXT PRIMARY KEY, account_id TEXT NOT NULL, cik_public TEXT NOT NULL, core_token_hash TEXT NOT NULL,
  name TEXT NOT NULL, created_at INTEGER NOT NULL, last_seen INTEGER);
CREATE TABLE IF NOT EXISTS devices (
  device_id TEXT PRIMARY KEY, core_id TEXT NOT NULL, name TEXT NOT NULL, public_key TEXT NOT NULL,
  role TEXT NOT NULL, status TEXT NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS pairings (
  pairing_id TEXT PRIMARY KEY, core_id TEXT NOT NULL, account_id TEXT NOT NULL, status TEXT NOT NULL,
  device_id TEXT, result TEXT, created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS audit (
  id INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER NOT NULL, account_id TEXT, action TEXT NOT NULL,
  ip TEXT, details TEXT NOT NULL);
";

/// Session idle timeout (30 min) and absolute lifetime (12 h).
pub const SESSION_IDLE_SECS: i64 = 30 * 60;
/// Absolute session lifetime.
pub const SESSION_TTL_SECS: i64 = 12 * 3600;

impl Store {
    /// Opens (and migrates) `<dir>/gateway.db`.
    ///
    /// # Errors
    /// SQLite failure.
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let conn = Connection::open(dir.join("gateway.db"))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    // ------------------------------------------------------------ accounts

    /// The single owner account, if any.
    #[must_use]
    pub fn account(&self) -> Option<String> {
        self.conn.lock().query_row("SELECT id FROM accounts LIMIT 1", [], |r| r.get(0)).optional().ok().flatten()
    }

    /// Creates the owner account.
    ///
    /// # Errors
    /// SQLite failure.
    pub fn create_account(&self, name: &str) -> rusqlite::Result<String> {
        let id = uuid::Uuid::now_v7().to_string();
        self.conn
            .lock()
            .execute("INSERT INTO accounts(id,name,created_at) VALUES (?1,?2,?3)", params![id, name, now()])?;
        Ok(id)
    }

    /// Adds a passkey.
    ///
    /// # Errors
    /// SQLite failure (e.g. duplicate id).
    pub fn add_credential(&self, account: &str, c: &orkestr_security::webauthn::Credential) -> rusqlite::Result<()> {
        self.conn.lock().execute(
            "INSERT INTO credentials(id,account_id,public_key,sign_count,created_at) VALUES (?1,?2,?3,?4,?5)",
            params![c.id, account, c.public_key, c.sign_count, now()],
        )?;
        Ok(())
    }

    /// Credential by id.
    #[must_use]
    pub fn credential(&self, id: &[u8]) -> Option<CredentialRow> {
        self.conn
            .lock()
            .query_row("SELECT account_id, public_key, sign_count FROM credentials WHERE id=?1", params![id], |r| {
                Ok(CredentialRow {
                    account_id: r.get(0)?,
                    credential: orkestr_security::webauthn::Credential {
                        id: id.to_vec(),
                        public_key: r.get(1)?,
                        sign_count: r.get(2)?,
                    },
                })
            })
            .optional()
            .ok()
            .flatten()
    }

    /// All credential ids (for `allowCredentials`).
    #[must_use]
    pub fn credential_ids(&self) -> Vec<Vec<u8>> {
        let c = self.conn.lock();
        let Ok(mut st) = c.prepare("SELECT id FROM credentials") else { return vec![] };
        st.query_map([], |r| r.get(0)).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
    }

    /// Updates a signature counter.
    pub fn set_sign_count(&self, id: &[u8], count: u32) {
        let _ = self.conn.lock().execute("UPDATE credentials SET sign_count=?2 WHERE id=?1", params![id, count]);
    }

    /// TOTP secret + enabled + last step.
    #[must_use]
    pub fn totp(&self, account: &str) -> Option<(Vec<u8>, bool, u64)> {
        self.conn
            .lock()
            .query_row(
                "SELECT totp_secret, totp_enabled, totp_last_step FROM accounts WHERE id=?1",
                params![account],
                |r| {
                    let s: Option<Vec<u8>> = r.get(0)?;
                    let enabled = r.get::<_, i64>(1)? != 0;
                    let last = u64::try_from(r.get::<_, i64>(2)?).unwrap_or(0);
                    Ok(s.map(|s| (s, enabled, last)))
                },
            )
            .optional()
            .ok()
            .flatten()
            .flatten()
    }

    /// Stores a (not yet enabled) TOTP secret.
    pub fn set_totp(&self, account: &str, secret: &[u8], enabled: bool) {
        let _ = self.conn.lock().execute(
            "UPDATE accounts SET totp_secret=?2, totp_enabled=?3 WHERE id=?1",
            params![account, secret, i64::from(enabled)],
        );
    }

    /// Records the last accepted TOTP step (no reuse).
    pub fn set_totp_step(&self, account: &str, step: u64) {
        let _ = self.conn.lock().execute(
            "UPDATE accounts SET totp_last_step=?2 WHERE id=?1",
            params![account, i64::try_from(step).unwrap_or(i64::MAX)],
        );
    }

    // ------------------------------------------------------------ sessions

    /// Creates a session; returns the bearer token.
    #[must_use]
    pub fn create_session(&self, account: &str) -> String {
        let token = crate::random_token();
        let t = now();
        let _ = self.conn.lock().execute(
            "INSERT INTO sessions(token_hash,account_id,created_at,last_seen,expires_at) VALUES (?1,?2,?3,?3,?4)",
            params![token_hash(&token), account, t, t + SESSION_TTL_SECS],
        );
        token
    }

    /// Validates (and touches) a session; returns the account.
    #[must_use]
    pub fn session(&self, token: &str) -> Option<String> {
        let h = token_hash(token);
        let c = self.conn.lock();
        let t = now();
        let row: Option<(String, i64, i64)> = c
            .query_row("SELECT account_id, last_seen, expires_at FROM sessions WHERE token_hash=?1", params![h], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()
            .ok()
            .flatten();
        let (account, last_seen, expires) = row?;
        if t > expires || t - last_seen > SESSION_IDLE_SECS {
            let _ = c.execute("DELETE FROM sessions WHERE token_hash=?1", params![h]);
            return None;
        }
        let _ = c.execute("UPDATE sessions SET last_seen=?2 WHERE token_hash=?1", params![h, t]);
        Some(account)
    }

    /// Deletes every session of an account ("panic").
    pub fn delete_sessions(&self, account: &str) -> usize {
        self.conn.lock().execute("DELETE FROM sessions WHERE account_id=?1", params![account]).unwrap_or(0)
    }

    /// Deletes a session.
    pub fn delete_session(&self, token: &str) {
        let _ = self.conn.lock().execute("DELETE FROM sessions WHERE token_hash=?1", params![token_hash(token)]);
    }

    // ------------------------------------------------------------ cores

    /// Issues a one-time enrollment token (15 min).
    #[must_use]
    pub fn create_enrollment_token(&self, account: &str) -> (String, i64) {
        let token = crate::random_token();
        let exp = now() + 15 * 60;
        let _ = self.conn.lock().execute(
            "INSERT INTO enrollment_tokens(token_hash,account_id,expires_at) VALUES (?1,?2,?3)",
            params![token_hash(&token), account, exp],
        );
        (token, exp)
    }

    /// Consumes an enrollment token and registers the core; returns
    /// `(core_id, account_id, core_token)`.
    #[must_use]
    pub fn enroll(&self, token: &str, cik_public: &str, name: &str) -> Option<(String, String, String)> {
        let h = token_hash(token);
        let c = self.conn.lock();
        let account: String = c
            .query_row(
                "SELECT account_id FROM enrollment_tokens WHERE token_hash=?1 AND used=0 AND expires_at>=?2",
                params![h, now()],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()?;
        c.execute("UPDATE enrollment_tokens SET used=1 WHERE token_hash=?1", params![h]).ok()?;
        let core_id = uuid::Uuid::now_v7().to_string();
        let core_token = crate::random_token();
        c.execute(
            "INSERT INTO cores(core_id,account_id,cik_public,core_token_hash,name,created_at) VALUES (?1,?2,?3,?4,?5,?6)",
            params![core_id, account, cik_public, token_hash(&core_token), name, now()],
        )
        .ok()?;
        Some((core_id, account, core_token))
    }

    /// Core by id.
    #[must_use]
    pub fn core(&self, core_id: &str) -> Option<CoreRow> {
        self.conn
            .lock()
            .query_row(
                "SELECT core_id, account_id, cik_public, name, last_seen FROM cores WHERE core_id=?1",
                params![core_id],
                |r| {
                    Ok(CoreRow {
                        core_id: r.get(0)?,
                        account_id: r.get(1)?,
                        cik_public: r.get(2)?,
                        name: r.get(3)?,
                        last_seen: r.get(4)?,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    /// Cores of an account.
    #[must_use]
    pub fn cores(&self, account: &str) -> Vec<CoreRow> {
        let c = self.conn.lock();
        let Ok(mut st) =
            c.prepare("SELECT core_id, account_id, cik_public, name, last_seen FROM cores WHERE account_id=?1")
        else {
            return vec![];
        };
        st.query_map(params![account], |r| {
            Ok(CoreRow {
                core_id: r.get(0)?,
                account_id: r.get(1)?,
                cik_public: r.get(2)?,
                name: r.get(3)?,
                last_seen: r.get(4)?,
            })
        })
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
    }

    /// Marks a core as seen now.
    pub fn touch_core(&self, core_id: &str) {
        let _ = self.conn.lock().execute("UPDATE cores SET last_seen=?2 WHERE core_id=?1", params![core_id, now()]);
    }

    /// Account owning a core token.
    #[must_use]
    pub fn core_token_account(&self, token: &str) -> Option<String> {
        self.conn
            .lock()
            .query_row("SELECT account_id FROM cores WHERE core_token_hash=?1", params![token_hash(token)], |r| {
                r.get(0)
            })
            .optional()
            .ok()
            .flatten()
    }

    // ------------------------------------------------------------ devices mirror / pairings

    /// Upserts a mirrored device.
    pub fn upsert_device(
        &self,
        core_id: &str,
        device_id: &str,
        name: &str,
        public_key: &str,
        role: &str,
        status: &str,
    ) {
        let _ = self.conn.lock().execute(
            "INSERT INTO devices(device_id,core_id,name,public_key,role,status,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(device_id) DO UPDATE SET name=?3, public_key=?4, role=?5, status=?6, updated_at=?7",
            params![device_id, core_id, name, public_key, role, status, now()],
        );
    }

    /// Marks a mirrored device revoked.
    pub fn revoke_device(&self, device_id: &str) {
        let _ = self.conn.lock().execute(
            "UPDATE devices SET status='revoked', updated_at=?2 WHERE device_id=?1",
            params![device_id, now()],
        );
    }

    /// Mirrored devices of a core: `(device_id, name, role, status)`.
    #[must_use]
    pub fn devices(&self, core_id: &str) -> Vec<(String, String, String, String)> {
        let c = self.conn.lock();
        let Ok(mut st) = c.prepare("SELECT device_id, name, role, status FROM devices WHERE core_id=?1") else {
            return vec![];
        };
        st.query_map(params![core_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    /// Records a claimed pairing.
    pub fn put_pairing(
        &self,
        pairing_id: &str,
        core_id: &str,
        account: &str,
        status: &str,
        device_id: Option<&str>,
        result: &Value,
    ) {
        let _ = self.conn.lock().execute(
            "INSERT INTO pairings(pairing_id,core_id,account_id,status,device_id,result,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(pairing_id) DO UPDATE SET status=?4, device_id=COALESCE(?5, device_id), result=?6",
            params![pairing_id, core_id, account, status, device_id, result.to_string(), now()],
        );
    }

    /// Pairing: `(account, core_id, status, result)`.
    #[must_use]
    pub fn pairing(&self, pairing_id: &str) -> Option<(String, String, String, Value)> {
        self.conn
            .lock()
            .query_row(
                "SELECT account_id, core_id, status, result FROM pairings WHERE pairing_id=?1",
                params![pairing_id],
                |r| {
                    let res: Option<String> = r.get(3)?;
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        res.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null),
                    ))
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    // ------------------------------------------------------------ audit

    /// Appends an audit entry.
    pub fn audit(&self, account: Option<&str>, action: &str, ip: Option<&str>, details: &Value) {
        let _ = self.conn.lock().execute(
            "INSERT INTO audit(ts,account_id,action,ip,details) VALUES (?1,?2,?3,?4,?5)",
            params![now(), account, action, ip, details.to_string()],
        );
    }

    /// Latest audit entries (newest first).
    #[must_use]
    pub fn audit_list(&self, account: &str, limit: u32) -> Vec<Value> {
        let c = self.conn.lock();
        let Ok(mut st) = c.prepare(
            "SELECT ts, action, ip, details FROM audit WHERE account_id=?1 OR account_id IS NULL ORDER BY id DESC LIMIT ?2",
        ) else {
            return vec![];
        };
        st.query_map(params![account, limit], |r| {
            let details: String = r.get(3)?;
            Ok(serde_json::json!({
                "ts": r.get::<_, i64>(0)?, "action": r.get::<_, String>(1)?, "ip": r.get::<_, Option<String>>(2)?,
                "details": serde_json::from_str::<Value>(&details).unwrap_or(Value::Null)
            }))
        })
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrollment_is_one_time_and_sessions_validate() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let acc = s.create_account("owner").unwrap();
        assert_eq!(s.account().as_deref(), Some(acc.as_str()));
        let (tok, _) = s.create_enrollment_token(&acc);
        let (core, a2, core_token) = s.enroll(&tok, "pk", "laptop").unwrap();
        assert_eq!(a2, acc);
        assert!(s.enroll(&tok, "pk", "laptop").is_none(), "token is single-use");
        assert_eq!(s.core(&core).unwrap().cik_public, "pk");
        assert_eq!(s.core_token_account(&core_token).as_deref(), Some(acc.as_str()));
        let sess = s.create_session(&acc);
        assert_eq!(s.session(&sess).as_deref(), Some(acc.as_str()));
        s.delete_session(&sess);
        assert!(s.session(&sess).is_none());
        assert!(s.session("bogus").is_none());
    }
}
