use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::entry::{Entry, EntryType, MimeDataMap, detect_entry_type};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Keychain error: {0}")]
    Keychain(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self, DbError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;

        let key = match get_encryption_key() {
            Ok(key) => key,
            Err(e) => {
                tracing::warn!("Keychain unavailable, using unencrypted DB: {e}");
                String::new()
            }
        };

        if !key.is_empty() {
            conn.pragma_update(None, "key", &key)?;
        }

        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;

        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<(), DbError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS entries (
                id INTEGER PRIMARY KEY,
                created_at INTEGER NOT NULL,
                entry_type TEXT NOT NULL,
                favorite INTEGER NOT NULL DEFAULT 0,
                favorite_position INTEGER
            );

            CREATE TABLE IF NOT EXISTS contents (
                entry_id INTEGER NOT NULL,
                mime TEXT NOT NULL,
                content BLOB NOT NULL,
                PRIMARY KEY (entry_id, mime),
                FOREIGN KEY (entry_id)
                    REFERENCES entries(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_entries_created
                ON entries(created_at);
            CREATE INDEX IF NOT EXISTS idx_entries_type
                ON entries(entry_type);",
        )?;
        Ok(())
    }

    pub fn insert(&self, data: MimeDataMap) -> Result<i64, DbError> {
        let now = chrono::Utc::now().timestamp_millis();
        let entry_type = detect_entry_type(&data);

        if let Some(existing_id) = self.find_duplicate(&data)? {
            self.conn.execute(
                "UPDATE entries SET created_at = ? WHERE id = ?",
                params![now, existing_id],
            )?;
            return Ok(existing_id);
        }

        self.conn.execute(
            "INSERT INTO entries (id, created_at, entry_type) VALUES (?, ?, ?)",
            params![now, now, entry_type.as_str()],
        )?;
        let id = now;

        for (mime, content) in &data {
            self.conn.execute(
                "INSERT INTO contents (entry_id, mime, content) VALUES (?, ?, ?)",
                params![id, mime, content],
            )?;
        }

        Ok(id)
    }

    fn find_duplicate(&self, data: &MimeDataMap) -> Result<Option<i64>, DbError> {
        for mime in [
            "text/plain;charset=utf-8",
            "text/plain",
            "image/png",
            "image/jpeg",
        ] {
            let Some(content) = data.get(mime) else {
                continue;
            };

            let mut stmt = self.conn.prepare(
                "SELECT c.entry_id FROM contents c
                 WHERE c.mime = ? AND c.content = ?
                 LIMIT 1",
            )?;

            if let Ok(id) =
                stmt.query_row(params![mime, content], |row| row.get::<_, i64>(0))
            {
                return Ok(Some(id));
            }
        }

        Ok(None)
    }

    pub fn list_entries(&self, limit: usize) -> Result<Vec<Entry>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, created_at, entry_type, favorite
             FROM entries
             ORDER BY favorite DESC, created_at DESC
             LIMIT ?",
        )?;

        let entry_rows: Vec<(i64, i64, String, bool)> = stmt
            .query_map(params![limit as i64], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get::<_, i64>(3)? != 0,
                ))
            })?
            .collect::<Result<_, _>>()?;

        let mut entries = Vec::with_capacity(entry_rows.len());
        for (id, created_at, entry_type_str, favorite) in entry_rows {
            let contents = self.load_contents(id)?;
            entries.push(Entry {
                id,
                created_at,
                entry_type: EntryType::from_str(&entry_type_str),
                favorite,
                contents,
            });
        }

        Ok(entries)
    }

    fn load_contents(&self, entry_id: i64) -> Result<MimeDataMap, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT mime, content FROM contents WHERE entry_id = ?",
        )?;

        let mut map = MimeDataMap::new();
        let rows = stmt.query_map(params![entry_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;

        for row in rows {
            let (mime, content) = row?;
            map.insert(mime, content);
        }

        Ok(map)
    }

    pub fn get_entry(&self, id: i64) -> Result<Option<Entry>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, created_at, entry_type, favorite
             FROM entries WHERE id = ?",
        )?;

        let entry = stmt
            .query_row(params![id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)? != 0,
                ))
            })
            .optional()?;

        let Some((id, created_at, entry_type_str, favorite)) = entry
        else {
            return Ok(None);
        };

        let contents = self.load_contents(id)?;
        Ok(Some(Entry {
            id,
            created_at,
            entry_type: EntryType::from_str(&entry_type_str),
            favorite,
            contents,
        }))
    }

    pub fn delete(&self, id: i64) -> Result<(), DbError> {
        self.conn
            .execute("DELETE FROM entries WHERE id = ?", params![id])?;
        Ok(())
    }

    #[allow(dead_code)] // UI favorite toggle not yet wired
    pub fn toggle_favorite(&self, id: i64) -> Result<(), DbError> {
        self.conn.execute(
            "UPDATE entries SET favorite = NOT favorite WHERE id = ?",
            params![id],
        )?;
        Ok(())
    }

    pub fn clear(&self) -> Result<(), DbError> {
        self.conn.execute("DELETE FROM entries WHERE favorite = 0", [])?;
        Ok(())
    }
}

fn get_encryption_key() -> Result<String, DbError> {
    let rt = tokio::runtime::Handle::try_current();
    let key = if let Ok(handle) = rt {
        handle.block_on(async { load_or_create_key().await })
    } else {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| DbError::Keychain(e.to_string()))?;
        rt.block_on(load_or_create_key())
    };
    key
}

async fn load_or_create_key() -> Result<String, DbError> {
    let keyring = oo7::Keyring::new()
        .await
        .map_err(|e| DbError::Keychain(e.to_string()))?;

    let attrs = vec![("application", "clipbro"), ("purpose", "db-encryption")];
    let items = keyring
        .search_items(&attrs)
        .await
        .map_err(|e| DbError::Keychain(e.to_string()))?;

    if let Some(item) = items.first() {
        let secret = item
            .secret()
            .await
            .map_err(|e| DbError::Keychain(e.to_string()))?;
        return Ok(String::from_utf8_lossy(&secret).into_owned());
    }

    let key: String = {
        use rand::Rng;
        let mut rng = rand::rng();
        (0..64).map(|_| rng.sample(rand::distr::Alphanumeric) as char).collect()
    };

    keyring
        .create_item(
            "clipbro database encryption key",
            &attrs,
            key.as_bytes(),
            true,
        )
        .await
        .map_err(|e| DbError::Keychain(e.to_string()))?;

    tracing::info!("Created new database encryption key in system keychain");
    Ok(key)
}
