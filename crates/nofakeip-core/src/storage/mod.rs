//! SQLite 持久化。
//!
//! 表结构与迁移策略见 `doc/03-数据模型与存储.md`。
//! 迁移原则：**只增不改**（加列用 `ALTER TABLE ADD COLUMN`），保证老库可平滑升级。
//!
//! **范围说明**：不存储任何 Clash / OpenClash 配置模板。

use crate::benchmark::DomainBenchResult;
use crate::error::{CoreError, Result};
use crate::models::{DnsProtocol, DnsServer, Region};
use crate::pollution::RiskLevel;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 当前 schema 版本。
pub const SCHEMA_VERSION: i64 = 1;

/// 域名历史行。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainHistoryRow {
    pub domain: String,
    pub region: Region,
    #[serde(default)]
    pub risk: Option<RiskLevel>,
    #[serde(default)]
    pub fake_ip: Option<String>,
    #[serde(default)]
    pub last_ips: Vec<String>,
    pub query_count: i64,
    pub last_seen_at: String,
    pub first_seen_at: String,
}

/// 用户自定义域名规则行。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainRuleRow {
    pub id: i64,
    pub pattern: String,
    pub region: Region,
    pub priority: i64,
    #[serde(default)]
    pub note: Option<String>,
}

/// 存储句柄。
pub struct Storage {
    conn: Mutex<Connection>,
    path: Option<PathBuf>,
}

impl std::fmt::Debug for Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Storage").field("path", &self.path).finish()
    }
}

impl Storage {
    /// 打开（或创建）数据库文件。
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::init(conn, Some(path.to_path_buf()))
    }

    /// 内存数据库（测试用）。
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn, None)
    }

    fn init(conn: Connection, path: Option<PathBuf>) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "foreign_keys", "ON").ok();
        let s = Self {
            conn: Mutex::new(conn),
            path,
        };
        s.migrate()?;
        Ok(s)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// 默认数据库路径（用户数据目录）。
    pub fn default_path() -> Result<PathBuf> {
        let base = default_data_dir()?;
        Ok(base.join("nofakeip.db"))
    }

    /// 默认数据目录（规则文件也放这里）。
    pub fn default_data_dir() -> Result<PathBuf> {
        default_data_dir()
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| CoreError::Storage("数据库锁已中毒".into()))
    }

    /// 建表与迁移（幂等）。
    pub fn migrate(&self) -> Result<()> {
        let conn = self.lock()?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS schema_meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS dns_server (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                name         TEXT    NOT NULL,
                protocol     TEXT    NOT NULL CHECK (protocol IN ('udp','tcp','dot','doh')),
                address      TEXT    NOT NULL,
                host         TEXT,
                bootstrap_ip TEXT,
                region       TEXT    NOT NULL CHECK (region IN ('china','global','unknown')),
                enabled      INTEGER NOT NULL DEFAULT 1,
                builtin      INTEGER NOT NULL DEFAULT 0,
                tags         TEXT    NOT NULL DEFAULT '[]',
                remark       TEXT,
                created_at   TEXT    NOT NULL,
                updated_at   TEXT    NOT NULL,
                UNIQUE (protocol, address)
            );
            CREATE INDEX IF NOT EXISTS idx_dns_server_enabled ON dns_server(enabled);

            CREATE TABLE IF NOT EXISTS benchmark_result (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                domain       TEXT    NOT NULL,
                region       TEXT    NOT NULL,
                confidence   REAL,
                server_id    INTEGER,
                server_name  TEXT    NOT NULL,
                protocol     TEXT    NOT NULL,
                ok           INTEGER NOT NULL,
                error        TEXT,
                samples      INTEGER NOT NULL DEFAULT 0,
                min_ms       REAL,
                avg_ms       REAL,
                max_ms       REAL,
                p95_ms       REAL,
                connect_ms   REAL,
                tls_ms       REAL,
                query_ms     REAL,
                answers      TEXT,
                ttl          INTEGER,
                rcode        INTEGER,
                score        REAL,
                batch_id     TEXT,
                created_at   TEXT    NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_bench_domain  ON benchmark_result(domain, created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_bench_batch   ON benchmark_result(batch_id);
            CREATE INDEX IF NOT EXISTS idx_bench_created ON benchmark_result(created_at DESC);

            CREATE TABLE IF NOT EXISTS domain_history (
                domain        TEXT PRIMARY KEY,
                region        TEXT    NOT NULL,
                risk          TEXT,
                fake_ip       TEXT,
                last_ips      TEXT,
                query_count   INTEGER NOT NULL DEFAULT 0,
                last_seen_at  TEXT    NOT NULL,
                first_seen_at TEXT    NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_domain_last_seen ON domain_history(last_seen_at DESC);

            CREATE TABLE IF NOT EXISTS user_config (
                key        TEXT PRIMARY KEY,
                value      TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS domain_rule (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                pattern    TEXT NOT NULL UNIQUE,
                region     TEXT NOT NULL,
                priority   INTEGER NOT NULL DEFAULT 100,
                note       TEXT,
                created_at TEXT NOT NULL
            );
            "#,
        )?;
        conn.execute(
            "INSERT INTO schema_meta(key, value) VALUES('version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![SCHEMA_VERSION.to_string()],
        )?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64> {
        let conn = self.lock()?;
        let v: Option<String> = conn
            .query_row("SELECT value FROM schema_meta WHERE key='version'", [], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
    }

    // —————————————————————— dns_server ——————————————————————

    /// 插入或更新服务器（按 protocol + address 去重），返回 ID。
    pub fn upsert_server(&self, s: &DnsServer) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();
        let tags = serde_json::to_string(&s.tags).unwrap_or_else(|_| "[]".into());
        let conn = self.lock()?;
        conn.execute(
            r#"INSERT INTO dns_server
                 (name, protocol, address, host, bootstrap_ip, region, enabled, builtin, tags, remark, created_at, updated_at)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?11)
               ON CONFLICT(protocol, address) DO UPDATE SET
                 name = excluded.name,
                 host = excluded.host,
                 bootstrap_ip = excluded.bootstrap_ip,
                 region = excluded.region,
                 enabled = excluded.enabled,
                 tags = excluded.tags,
                 remark = excluded.remark,
                 updated_at = excluded.updated_at"#,
            params![
                s.name,
                s.protocol.as_str(),
                s.address,
                s.host,
                s.bootstrap_ip,
                s.region.as_str(),
                s.enabled as i32,
                s.builtin as i32,
                tags,
                s.remark,
                now,
            ],
        )?;
        let id: i64 = conn.query_row(
            "SELECT id FROM dns_server WHERE protocol = ?1 AND address = ?2",
            params![s.protocol.as_str(), s.address],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    /// 列出服务器。
    pub fn list_servers(&self, only_enabled: bool) -> Result<Vec<DnsServer>> {
        let conn = self.lock()?;
        let sql = if only_enabled {
            "SELECT id,name,protocol,address,host,bootstrap_ip,region,enabled,builtin,tags,remark
             FROM dns_server WHERE enabled = 1 ORDER BY region, name"
        } else {
            "SELECT id,name,protocol,address,host,bootstrap_ip,region,enabled,builtin,tags,remark
             FROM dns_server ORDER BY region, name"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], row_to_server)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get_server(&self, id: i64) -> Result<Option<DnsServer>> {
        let conn = self.lock()?;
        let s = conn
            .query_row(
                "SELECT id,name,protocol,address,host,bootstrap_ip,region,enabled,builtin,tags,remark
                 FROM dns_server WHERE id = ?1",
                params![id],
                row_to_server,
            )
            .optional()?;
        Ok(s)
    }

    pub fn delete_server(&self, id: i64) -> Result<()> {
        let conn = self.lock()?;
        conn.execute("DELETE FROM dns_server WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn set_server_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE dns_server SET enabled = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, enabled as i32, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    // —————————————————————— benchmark_result ——————————————————————

    /// 写入一个域名的全部服务器测速结果，返回写入行数。
    pub fn insert_benchmark(&self, r: &DomainBenchResult) -> Result<usize> {
        let conn = self.lock()?;
        let now = chrono::Utc::now().to_rfc3339();
        let tx = conn.unchecked_transaction()?;
        let mut n = 0usize;
        for s in &r.results {
            let answers = serde_json::to_string(
                &s.answers.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
            )
            .unwrap_or_else(|_| "[]".into());
            tx.execute(
                r#"INSERT INTO benchmark_result
                    (domain, region, confidence, server_id, server_name, protocol, ok, error,
                     samples, min_ms, avg_ms, max_ms, p95_ms, connect_ms, tls_ms, query_ms,
                     answers, ttl, rcode, score, batch_id, created_at)
                   VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)"#,
                params![
                    r.domain,
                    r.region.as_str(),
                    r.confidence,
                    s.server_id,
                    s.server_name,
                    s.protocol.as_str(),
                    s.ok as i32,
                    s.error,
                    s.samples,
                    s.min_ms,
                    s.avg_ms,
                    s.max_ms,
                    s.p95_ms,
                    s.connect_ms,
                    s.tls_ms,
                    s.query_ms,
                    answers,
                    s.ttl,
                    s.rcode,
                    s.score,
                    r.batch_id,
                    now,
                ],
            )?;
            n += 1;
        }
        tx.commit()?;
        Ok(n)
    }

    /// 查询测速结果（按域名聚合）。
    pub fn list_benchmarks(&self, domain: Option<&str>, limit: u32) -> Result<Vec<DomainBenchResult>> {
        let conn = self.lock()?;
        let (sql, bind): (&str, Vec<String>) = match domain {
            Some(d) => (
                "SELECT domain, region, confidence, server_id, server_name, protocol, ok, error,
                        samples, min_ms, avg_ms, max_ms, p95_ms, connect_ms, tls_ms, query_ms,
                        answers, ttl, rcode, score, batch_id, created_at
                 FROM benchmark_result WHERE domain = ?1 ORDER BY created_at DESC LIMIT ?2",
                vec![d.to_string(), limit.to_string()],
            ),
            None => (
                "SELECT domain, region, confidence, server_id, server_name, protocol, ok, error,
                        samples, min_ms, avg_ms, max_ms, p95_ms, connect_ms, tls_ms, query_ms,
                        answers, ttl, rcode, score, batch_id, created_at
                 FROM benchmark_result ORDER BY created_at DESC LIMIT ?1",
                vec![limit.to_string()],
            ),
        };
        let mut stmt = conn.prepare(sql)?;
        let rows: Vec<RowBench> = match bind.len() {
            1 => {
                let it = stmt.query_map(params![bind[0]], row_to_bench)?;
                it.collect::<std::result::Result<Vec<_>, _>>()?
            }
            _ => {
                let it = stmt.query_map(params![bind[0], bind[1]], row_to_bench)?;
                it.collect::<std::result::Result<Vec<_>, _>>()?
            }
        };
        Ok(group_bench_rows(rows))
    }

    // —————————————————————— domain_history ——————————————————————

    /// 记录一次域名查询（存在则累加计数并更新状态）。
    pub fn touch_domain(
        &self,
        domain: &str,
        region: Region,
        risk: Option<RiskLevel>,
        fake_ip: Option<&str>,
        ips: &[String],
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let ips_json = serde_json::to_string(ips).unwrap_or_else(|_| "[]".into());
        let conn = self.lock()?;
        conn.execute(
            r#"INSERT INTO domain_history
                 (domain, region, risk, fake_ip, last_ips, query_count, last_seen_at, first_seen_at)
               VALUES (?1,?2,?3,?4,?5,1,?6,?6)
               ON CONFLICT(domain) DO UPDATE SET
                 region = excluded.region,
                 risk = excluded.risk,
                 fake_ip = excluded.fake_ip,
                 last_ips = excluded.last_ips,
                 query_count = domain_history.query_count + 1,
                 last_seen_at = excluded.last_seen_at"#,
            params![
                domain,
                region.as_str(),
                risk.map(|r| r.as_str().to_string()),
                fake_ip,
                ips_json,
                now,
            ],
        )?;
        Ok(())
    }

    pub fn list_domains(&self, limit: u32) -> Result<Vec<DomainHistoryRow>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT domain, region, risk, fake_ip, last_ips, query_count, last_seen_at, first_seen_at
             FROM domain_history ORDER BY last_seen_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            let region: String = r.get(1)?;
            let risk: Option<String> = r.get(2)?;
            let last_ips: Option<String> = r.get(4)?;
            Ok(DomainHistoryRow {
                domain: r.get(0)?,
                region: Region::parse(&region).unwrap_or(Region::Unknown),
                risk: risk.as_deref().and_then(RiskLevel::parse),
                fake_ip: r.get(3)?,
                last_ips: last_ips
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default(),
                query_count: r.get(5)?,
                last_seen_at: r.get(6)?,
                first_seen_at: r.get(7)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // —————————————————————— user_config ——————————————————————

    pub fn get_config(&self, key: &str) -> Result<Option<String>> {
        let conn = self.lock()?;
        let v = conn
            .query_row(
                "SELECT value FROM user_config WHERE key = ?1",
                params![key],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(v)
    }

    pub fn set_config(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO user_config(key, value, updated_at) VALUES(?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn all_config(&self) -> Result<Vec<(String, String)>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT key, value FROM user_config ORDER BY key")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // —————————————————————— domain_rule ——————————————————————

    pub fn upsert_rule(&self, pattern: &str, region: Region, priority: i64, note: Option<&str>) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO domain_rule(pattern, region, priority, note, created_at)
             VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(pattern) DO UPDATE SET
               region = excluded.region, priority = excluded.priority, note = excluded.note",
            params![
                pattern,
                region.as_str(),
                priority,
                note,
                chrono::Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn list_rules(&self) -> Result<Vec<DomainRuleRow>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, pattern, region, priority, note FROM domain_rule ORDER BY priority, pattern",
        )?;
        let rows = stmt.query_map([], |r| {
            let region: String = r.get(2)?;
            Ok(DomainRuleRow {
                id: r.get(0)?,
                pattern: r.get(1)?,
                region: Region::parse(&region).unwrap_or(Region::Unknown),
                priority: r.get(3)?,
                note: r.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn delete_rule(&self, id: i64) -> Result<()> {
        let conn = self.lock()?;
        conn.execute("DELETE FROM domain_rule WHERE id = ?1", params![id])?;
        Ok(())
    }

    // —————————————————————— 维护 ——————————————————————

    /// 清理过期测速记录（保留最近 `keep` 行）。
    pub fn prune(&self, keep: u32) -> Result<usize> {
        let conn = self.lock()?;
        let n = conn.execute(
            "DELETE FROM benchmark_result WHERE id NOT IN (
                 SELECT id FROM benchmark_result ORDER BY created_at DESC LIMIT ?1
             )",
            params![keep],
        )?;
        conn.execute_batch("VACUUM").ok();
        Ok(n)
    }

    /// 统计信息（供 doctor / 首页展示）。
    pub fn stats(&self) -> Result<StorageStats> {
        let conn = self.lock()?;
        let servers: i64 = conn.query_row("SELECT COUNT(*) FROM dns_server", [], |r| r.get(0))?;
        let benchmarks: i64 =
            conn.query_row("SELECT COUNT(*) FROM benchmark_result", [], |r| r.get(0))?;
        let domains: i64 = conn.query_row("SELECT COUNT(*) FROM domain_history", [], |r| r.get(0))?;
        let rules: i64 = conn.query_row("SELECT COUNT(*) FROM domain_rule", [], |r| r.get(0))?;
        Ok(StorageStats {
            servers,
            benchmarks,
            domains,
            rules,
            schema_version: SCHEMA_VERSION,
            path: self.path.as_ref().map(|p| p.display().to_string()),
        })
    }
}

/// 存储统计。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStats {
    pub servers: i64,
    pub benchmarks: i64,
    pub domains: i64,
    pub rules: i64,
    pub schema_version: i64,
    #[serde(default)]
    pub path: Option<String>,
}

struct RowBench {
    domain: String,
    region: String,
    confidence: Option<f32>,
    server_id: Option<i64>,
    server_name: String,
    protocol: String,
    ok: bool,
    error: Option<String>,
    samples: u32,
    min_ms: f64,
    avg_ms: f64,
    max_ms: f64,
    p95_ms: f64,
    connect_ms: f64,
    tls_ms: f64,
    query_ms: f64,
    answers: Vec<std::net::IpAddr>,
    ttl: Option<u32>,
    rcode: Option<u8>,
    score: f64,
    batch_id: Option<String>,
    created_at: String,
}

fn row_to_bench(r: &rusqlite::Row<'_>) -> rusqlite::Result<RowBench> {
    let answers_json: Option<String> = r.get(16)?;
    let answers: Vec<std::net::IpAddr> = answers_json
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .map(|v| v.iter().filter_map(|s| s.parse().ok()).collect())
        .unwrap_or_default();
    Ok(RowBench {
        domain: r.get(0)?,
        region: r.get(1)?,
        confidence: r.get(2)?,
        server_id: r.get(3)?,
        server_name: r.get(4)?,
        protocol: r.get(5)?,
        ok: r.get::<_, i32>(6)? != 0,
        error: r.get(7)?,
        samples: r.get(8)?,
        min_ms: r.get::<_, Option<f64>>(9)?.unwrap_or(0.0),
        avg_ms: r.get::<_, Option<f64>>(10)?.unwrap_or(0.0),
        max_ms: r.get::<_, Option<f64>>(11)?.unwrap_or(0.0),
        p95_ms: r.get::<_, Option<f64>>(12)?.unwrap_or(0.0),
        connect_ms: r.get::<_, Option<f64>>(13)?.unwrap_or(0.0),
        tls_ms: r.get::<_, Option<f64>>(14)?.unwrap_or(0.0),
        query_ms: r.get::<_, Option<f64>>(15)?.unwrap_or(0.0),
        answers,
        ttl: r.get::<_, Option<i64>>(17)?.map(|v| v as u32),
        rcode: r.get::<_, Option<i64>>(18)?.map(|v| v as u8),
        score: r.get::<_, Option<f64>>(19)?.unwrap_or(0.0),
        batch_id: r.get(20)?,
        created_at: r.get(21)?,
    })
}

fn group_bench_rows(rows: Vec<RowBench>) -> Vec<DomainBenchResult> {
    let mut out: Vec<DomainBenchResult> = Vec::new();
    for row in rows {
        let server = crate::benchmark::ServerBenchResult {
            server_id: row.server_id.unwrap_or(0),
            server_name: row.server_name.clone(),
            protocol: DnsProtocol::parse(&row.protocol).unwrap_or(DnsProtocol::Udp),
            region: Region::parse(&row.region).unwrap_or(Region::Unknown),
            address: String::new(),
            ok: row.ok,
            error: row.error.clone(),
            samples: row.samples,
            min_ms: row.min_ms,
            avg_ms: row.avg_ms,
            max_ms: row.max_ms,
            p95_ms: row.p95_ms,
            connect_ms: row.connect_ms,
            tls_ms: row.tls_ms,
            query_ms: row.query_ms,
            answers: row.answers.clone(),
            ttl: row.ttl,
            rcode: row.rcode,
            score: row.score,
            geo: Vec::new(),
        };
        match out
            .iter_mut()
            .find(|d| d.domain == row.domain && d.created_at == row.created_at)
        {
            Some(existing) => existing.results.push(server),
            None => out.push(DomainBenchResult {
                domain: row.domain.clone(),
                region: Region::parse(&row.region).unwrap_or(Region::Unknown),
                confidence: row.confidence.unwrap_or(0.0),
                results: vec![server],
                best_china: None,
                best_global: None,
                recommended: None,
                batch_id: row.batch_id.clone(),
                created_at: row.created_at.clone(),
            }),
        }
    }
    out
}

fn row_to_server(r: &rusqlite::Row<'_>) -> rusqlite::Result<DnsServer> {
    let protocol: String = r.get(2)?;
    let region: String = r.get(6)?;
    let tags_json: String = r.get(9)?;
    Ok(DnsServer {
        id: r.get(0)?,
        name: r.get(1)?,
        protocol: DnsProtocol::parse(&protocol).unwrap_or(DnsProtocol::Udp),
        address: r.get(3)?,
        host: r.get(4)?,
        bootstrap_ip: r.get(5)?,
        region: Region::parse(&region).unwrap_or(Region::Unknown),
        enabled: r.get::<_, i32>(7)? != 0,
        builtin: r.get::<_, i32>(8)? != 0,
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        remark: r.get(10)?,
    })
}

/// 默认数据目录（跨平台）。
fn default_data_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("NOFAKEIP_DATA_DIR") {
        if !dir.trim().is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return Ok(PathBuf::from(appdata).join("NoFakeIP"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return Ok(PathBuf::from(home).join("Library/Application Support/NoFakeIP"));
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            if !xdg.trim().is_empty() {
                return Ok(PathBuf::from(xdg).join("nofakeip"));
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            return Ok(PathBuf::from(home).join(".local/share/nofakeip"));
        }
    }
    Ok(std::env::temp_dir().join("nofakeip"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmark::ServerBenchResult;
    use std::net::IpAddr;

    fn storage() -> Storage {
        Storage::open_in_memory().unwrap()
    }

    fn sample_result(domain: &str) -> DomainBenchResult {
        DomainBenchResult {
            domain: domain.into(),
            region: Region::Global,
            confidence: 0.95,
            results: vec![ServerBenchResult {
                server_id: 1,
                server_name: "Cloudflare".into(),
                protocol: DnsProtocol::Doh,
                region: Region::Global,
                address: "https://cloudflare-dns.com/dns-query".into(),
                ok: true,
                error: None,
                samples: 3,
                min_ms: 110.0,
                avg_ms: 120.0,
                max_ms: 130.0,
                p95_ms: 130.0,
                connect_ms: 20.0,
                tls_ms: 40.0,
                query_ms: 60.0,
                answers: vec!["142.250.72.4".parse::<IpAddr>().unwrap()],
                ttl: Some(300),
                rcode: Some(0),
                score: 85.0,
                geo: vec![],
            }],
            best_china: None,
            best_global: Some("Cloudflare".into()),
            recommended: Some("Cloudflare".into()),
            batch_id: Some("b1".into()),
            created_at: "2026-10-14T00:00:00Z".into(),
        }
    }

    #[test]
    fn migrate_is_idempotent() {
        let s = storage();
        s.migrate().unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn server_upsert_and_list() {
        let s = storage();
        let srv = DnsServer::new("阿里 DoH", DnsProtocol::Doh, "https://dns.alidns.com/dns-query")
            .with_region(Region::China)
            .with_tags(["cn", "doh"]);
        let id = s.upsert_server(&srv).unwrap();
        assert!(id > 0);

        // 同 protocol+address 再插入应更新而非新增
        let mut srv2 = srv.clone();
        srv2.name = "阿里 DoH（改名）".into();
        s.upsert_server(&srv2).unwrap();

        let list = s.list_servers(false).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "阿里 DoH（改名）");
        assert_eq!(list[0].tags, vec!["cn", "doh"]);

        s.set_server_enabled(id, false).unwrap();
        assert!(s.list_servers(true).unwrap().is_empty());
        assert_eq!(s.list_servers(false).unwrap().len(), 1);

        assert!(s.get_server(id).unwrap().is_some());
        s.delete_server(id).unwrap();
        assert!(s.list_servers(false).unwrap().is_empty());
    }

    #[test]
    fn benchmark_insert_and_list() {
        let s = storage();
        let r = sample_result("github.com");
        assert_eq!(s.insert_benchmark(&r).unwrap(), 1);

        let list = s.list_benchmarks(Some("github.com"), 10).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].results.len(), 1);
        assert_eq!(list[0].results[0].server_name, "Cloudflare");
        assert_eq!(list[0].results[0].answers.len(), 1);
        assert_eq!(list[0].results[0].ttl, Some(300));
    }

    #[test]
    fn domain_history_accumulates() {
        let s = storage();
        s.touch_domain("a.com", Region::Global, Some(RiskLevel::High), Some("normal"), &["1.2.3.4".into()])
            .unwrap();
        s.touch_domain("a.com", Region::Global, Some(RiskLevel::Low), Some("normal"), &["1.2.3.5".into()])
            .unwrap();
        let rows = s.list_domains(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].query_count, 2);
        assert_eq!(rows[0].last_ips, vec!["1.2.3.5"]);
        assert_eq!(rows[0].risk, Some(RiskLevel::Low));
    }

    #[test]
    fn config_kv_roundtrip() {
        let s = storage();
        assert!(s.get_config("k").unwrap().is_none());
        s.set_config("k", "v1").unwrap();
        s.set_config("k", "v2").unwrap();
        assert_eq!(s.get_config("k").unwrap().as_deref(), Some("v2"));
        assert_eq!(s.all_config().unwrap().len(), 1);
    }

    #[test]
    fn domain_rules_crud() {
        let s = storage();
        s.upsert_rule("*.corp.example.com", Region::China, 10, Some("内网")).unwrap();
        s.upsert_rule("*.corp.example.com", Region::Global, 20, None).unwrap();
        let rules = s.list_rules().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].region, Region::Global);
        s.delete_rule(rules[0].id).unwrap();
        assert!(s.list_rules().unwrap().is_empty());
    }

    #[test]
    fn stats_reports_counts() {
        let s = storage();
        s.upsert_server(&DnsServer::new("x", DnsProtocol::Udp, "1.1.1.1:53")).unwrap();
        s.insert_benchmark(&sample_result("a.com")).unwrap();
        s.touch_domain("a.com", Region::Global, None, None, &[]).unwrap();
        let st = s.stats().unwrap();
        assert_eq!(st.servers, 1);
        assert_eq!(st.benchmarks, 1);
        assert_eq!(st.domains, 1);
    }

    #[test]
    fn prune_keeps_recent() {
        let s = storage();
        for i in 0..5 {
            let mut r = sample_result(&format!("d{}.com", i));
            r.created_at = format!("2026-10-14T00:00:0{}Z", i);
            s.insert_benchmark(&r).unwrap();
        }
        let removed = s.prune(2).unwrap();
        assert_eq!(removed, 3);
    }

    #[test]
    fn default_path_is_under_app_dir() {
        let p = Storage::default_path().unwrap();
        assert!(p.to_string_lossy().contains("nofakeip.db"));
    }
}
