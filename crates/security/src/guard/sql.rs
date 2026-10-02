//! SQL statement classification (permission model §6.3 D4).

use std::sync::OnceLock;

use orkestr_domain::DangerCategory;
use regex::Regex;

use super::DangerClass;

/// Splits SQL into statements on `;`, ignoring quoted text and comments.
fn statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = sql.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' | '"' | '`' => {
                cur.push(c);
                i += 1;
                while i < chars.len() {
                    cur.push(chars[i]);
                    if chars[i] == c {
                        break;
                    }
                    i += 1;
                }
            }
            '-' if chars.get(i + 1) == Some(&'-') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                cur.push(' ');
                continue;
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
                cur.push(' ');
                continue;
            }
            ';' => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
        i += 1;
    }
    out.push(cur);
    out.into_iter().map(|s| s.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|s| !s.is_empty()).collect()
}

struct SqlRules {
    drop: Regex,
    truncate: Regex,
    delete: Regex,
    update: Regex,
    alter_drop: Regex,
    grant: Regex,
    alter_user: Regex,
    where_clause: Regex,
    mongo: Regex,
    redis: Regex,
}

fn rules() -> &'static SqlRules {
    static R: OnceLock<SqlRules> = OnceLock::new();
    R.get_or_init(|| {
        let re = |s: &str| Regex::new(s).expect("valid SQL guard regex");
        SqlRules {
            drop: re(r"(?i)^\s*drop\s+(table|database|schema|view|index|user|role|function|procedure|trigger|sequence|materialized\s+view|keyspace|collection)\b"),
            truncate: re(r"(?i)^\s*truncate\b"),
            delete: re(r"(?i)^\s*delete\s+(from\s+)?\S+"),
            update: re(r"(?i)^\s*update\s+\S+\s+set\b"),
            alter_drop: re(r"(?i)^\s*alter\s+table\b.*\bdrop\b"),
            grant: re(r"(?i)^\s*(grant|revoke)\b"),
            alter_user: re(r"(?i)^\s*alter\s+(user|role)\b"),
            where_clause: re(r"(?i)\bwhere\b"),
            mongo: re(r"(?i)(dropDatabase\s*\(|\.drop\s*\(|\.deleteMany\s*\(\s*\{\s*\}|\.remove\s*\(\s*\{\s*\}|dropAllUsers)"),
            redis: re(r"(?i)\b(flushall|flushdb)\b"),
        }
    })
}

/// Classifies one SQL text (possibly several statements).
#[must_use]
pub fn classify_sql(sql: &str) -> DangerClass {
    let r = rules();
    let danger =
        |reason: &str| DangerClass::Dangerous { category: DangerCategory::DataDestruction, reason: reason.to_owned() };
    if r.mongo.is_match(sql) {
        return danger("MongoDB drop/mass delete");
    }
    if r.redis.is_match(sql) {
        return danger("Redis FLUSHALL/FLUSHDB");
    }
    for stmt in statements(sql) {
        if r.drop.is_match(&stmt) {
            return danger("SQL DROP statement");
        }
        if r.truncate.is_match(&stmt) {
            return danger("SQL TRUNCATE statement");
        }
        if r.delete.is_match(&stmt) && !r.where_clause.is_match(&stmt) {
            return danger("SQL DELETE without WHERE");
        }
        if r.update.is_match(&stmt) && !r.where_clause.is_match(&stmt) {
            return danger("SQL UPDATE without WHERE");
        }
        if r.alter_drop.is_match(&stmt) {
            return danger("ALTER TABLE … DROP");
        }
        if r.grant.is_match(&stmt) || r.alter_user.is_match(&stmt) {
            return danger("privilege change (GRANT/REVOKE/ALTER USER)");
        }
    }
    DangerClass::Safe
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_table() {
        let dangerous = [
            "DROP TABLE users",
            "drop database prod;",
            "TRUNCATE users",
            "DELETE FROM users",
            "delete from users;",
            "UPDATE users SET admin = 1",
            "ALTER TABLE users DROP COLUMN email",
            "GRANT ALL ON db.* TO x",
            "REVOKE SELECT ON t FROM y",
            "ALTER USER postgres WITH PASSWORD 'x'",
            "SELECT 1; DROP TABLE t",
            "/* hi */ DROP SCHEMA public CASCADE",
            "db.dropDatabase()",
            "db.users.deleteMany({})",
            "FLUSHALL",
        ];
        for s in dangerous {
            assert!(matches!(classify_sql(s), DangerClass::Dangerous { .. }), "{s}");
        }
        let safe = [
            "SELECT * FROM users",
            "DELETE FROM users WHERE id = 1",
            "UPDATE users SET name='x' WHERE id=2",
            "INSERT INTO t VALUES (1)",
            "SELECT 'DROP TABLE x'",
            "-- DROP TABLE x\nSELECT 1",
            "CREATE TABLE t (id int)",
        ];
        for s in safe {
            assert_eq!(classify_sql(s), DangerClass::Safe, "{s}");
        }
    }
}
