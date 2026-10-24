//! What's kept for players who connect with a key: their nickname, settings
//! and single-player records. Guests aren't stored.

use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use snake_core::config::GameConfig;
use snake_core::records::Records;
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS players (
    fingerprint TEXT PRIMARY KEY,
    nick        TEXT NOT NULL,
    -- the same key=value text the local game keeps in ~/.snaketui/config
    config      TEXT NOT NULL DEFAULT '',
    last_seen   INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS solo_records (
    fingerprint TEXT NOT NULL,
    record_key  TEXT NOT NULL,
    best        INTEGER NOT NULL,
    PRIMARY KEY (fingerprint, record_key)
);
";

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

/// What's saved for a returning player.
pub struct Saved {
    pub nick: String,
    pub cfg: GameConfig,
    pub records: Records,
}

impl Store {
    /// Opens the database at `url` (`sqlite:///path/to/file.db`, or
    /// `sqlite::memory:` for tests), creating it and its tables if needed.
    pub async fn open(url: &str) -> Result<Self> {
        let options = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        // Each connection to an in-memory database is a separate database.
        let connections = if url.contains(":memory:") { 1 } else { 4 };
        let pool = SqlitePoolOptions::new()
            .max_connections(connections)
            .connect_with(options)
            .await?;
        sqlx::raw_sql(SCHEMA).execute(&pool).await?;
        Ok(Self { pool })
    }

    /// Waits for writes in progress, then closes the database.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Notes a visit by this key and returns what's saved for it. A first
    /// visit saves `nick` as the player's name.
    pub async fn visit(&self, fingerprint: &str, nick: &str) -> Result<Saved> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        sqlx::query(
            "INSERT INTO players (fingerprint, nick, last_seen) VALUES (?1, ?2, ?3)
             ON CONFLICT (fingerprint) DO UPDATE SET last_seen = excluded.last_seen",
        )
        .bind(fingerprint)
        .bind(nick)
        .bind(now)
        .execute(&self.pool)
        .await?;

        let player = sqlx::query("SELECT nick, config FROM players WHERE fingerprint = ?1")
            .bind(fingerprint)
            .fetch_one(&self.pool)
            .await?;
        let mut records = Records::default();
        let rows = sqlx::query("SELECT record_key, best FROM solo_records WHERE fingerprint = ?1")
            .bind(fingerprint)
            .fetch_all(&self.pool)
            .await?;
        for row in rows {
            let key: String = row.try_get(0)?;
            let best: i64 = row.try_get(1)?;
            records.update(&key, best as u32);
        }
        Ok(Saved {
            nick: player.try_get(0)?,
            cfg: GameConfig::from_saved_text(&player.try_get::<String, _>(1)?),
            records,
        })
    }

    pub async fn set_nick(&self, fingerprint: &str, nick: &str) -> Result<()> {
        sqlx::query("UPDATE players SET nick = ?2 WHERE fingerprint = ?1")
            .bind(fingerprint)
            .bind(nick)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn save_config(&self, fingerprint: &str, cfg: &GameConfig) -> Result<()> {
        sqlx::query("UPDATE players SET config = ?2 WHERE fingerprint = ?1")
            .bind(fingerprint)
            .bind(cfg.to_saved_text())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Raises a record to `best`; never lowers it, even if saves arrive out
    /// of order.
    pub async fn save_record(&self, fingerprint: &str, key: &str, best: u32) -> Result<()> {
        sqlx::query(
            "INSERT INTO solo_records (fingerprint, record_key, best) VALUES (?1, ?2, ?3)
             ON CONFLICT (fingerprint, record_key) DO UPDATE SET best = MAX(best, excluded.best)",
        )
        .bind(fingerprint)
        .bind(key)
        .bind(i64::from(best))
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[actix_rt::test]
    async fn settings_and_records_survive_a_reconnect() {
        let store = Store::open("sqlite::memory:").await.unwrap();
        let first = store.visit("SHA256:abc", "alice").await.unwrap();
        assert_eq!(first.nick, "alice");
        assert_eq!(first.cfg, GameConfig::default());

        let mut cfg = GameConfig::default();
        cfg.adjust(0, 3);
        store.save_config("SHA256:abc", &cfg).await.unwrap();
        store
            .save_record("SHA256:abc", "size:30x20", 12)
            .await
            .unwrap();
        store
            .save_record("SHA256:abc", "size:30x20", 7)
            .await
            .unwrap();

        let again = store.visit("SHA256:abc", "renamed").await.unwrap();
        assert_eq!(again.nick, "alice", "the first name sticks");
        store.set_nick("SHA256:abc", "alicia").await.unwrap();
        assert_eq!(store.visit("SHA256:abc", "x").await.unwrap().nick, "alicia");
        assert_eq!(again.cfg, cfg);
        assert_eq!(again.records.get("size:30x20"), 12, "records only go up");
        let other = store.visit("SHA256:xyz", "bob").await.unwrap();
        assert_eq!(other.records.get("size:30x20"), 0);
    }
}
