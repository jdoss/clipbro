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
    pub fn open(
        path: &Path,
        encrypt: bool,
    ) -> Result<Self, DbError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;

        if encrypt {
            let key = get_encryption_key()?;
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

    pub fn touch(&self, id: i64) -> Result<(), DbError> {
        let now = chrono::Utc::now().timestamp_millis();
        self.conn.execute(
            "UPDATE entries SET created_at = ? WHERE id = ?",
            params![now, id],
        )?;
        Ok(())
    }

    fn find_duplicate(
        &self,
        data: &MimeDataMap,
    ) -> Result<Option<i64>, DbError> {
        let text_mimes = [
            "text/plain;charset=utf-8",
            "text/plain",
        ];

        let text_content = text_mimes.iter().find_map(|m| {
            data.get(*m).and_then(|c| {
                let s = String::from_utf8_lossy(c);
                let t = s.trim().to_string();
                if t.is_empty() { None } else { Some(t) }
            })
        });

        if let Some(trimmed) = text_content {
            let mut stmt = self.conn.prepare(
                "SELECT c.entry_id FROM contents c
                 WHERE c.mime IN (?, ?)
                 AND TRIM(CAST(c.content AS TEXT)) = ?
                 LIMIT 1",
            )?;

            if let Ok(id) = stmt.query_row(
                params![
                    text_mimes[0],
                    text_mimes[1],
                    &trimmed,
                ],
                |row| row.get::<_, i64>(0),
            ) {
                return Ok(Some(id));
            }
        }

        for mime in ["image/png", "image/jpeg"] {
            let Some(content) = data.get(mime) else {
                continue;
            };

            let mut stmt = self.conn.prepare(
                "SELECT c.entry_id FROM contents c
                 WHERE c.mime = ? AND c.content = ?
                 LIMIT 1",
            )?;

            if let Ok(id) = stmt.query_row(
                params![mime, content],
                |row| row.get::<_, i64>(0),
            ) {
                return Ok(Some(id));
            }
        }

        Ok(None)
    }

    #[allow(dead_code)] // used by get_entry, future CLI commands
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

    pub fn list_entries_light(
        &self,
        limit: usize,
    ) -> Result<Vec<Entry>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, created_at, entry_type, favorite
             FROM entries
             ORDER BY favorite DESC, created_at DESC
             LIMIT ?",
        )?;

        let entry_rows: Vec<(i64, i64, String, bool)> =
            stmt.query_map(
                params![limit as i64],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get::<_, i64>(3)? != 0,
                    ))
                },
            )?
            .collect::<Result<_, _>>()?;

        let mut entries =
            Vec::with_capacity(entry_rows.len());
        for (id, created_at, type_str, favorite) in
            entry_rows
        {
            let contents =
                self.load_contents_light(id)?;
            entries.push(Entry {
                id,
                created_at,
                entry_type: EntryType::from_str(&type_str),
                favorite,
                contents,
            });
        }

        Ok(entries)
    }

    fn load_contents(
        &self,
        entry_id: i64,
    ) -> Result<MimeDataMap, DbError> {
        self.load_contents_filtered(entry_id, false)
    }

    fn load_contents_light(
        &self,
        entry_id: i64,
    ) -> Result<MimeDataMap, DbError> {
        self.load_contents_filtered(entry_id, true)
    }

    fn load_contents_filtered(
        &self,
        entry_id: i64,
        skip_images: bool,
    ) -> Result<MimeDataMap, DbError> {
        let sql = if skip_images {
            "SELECT mime, content FROM contents \
             WHERE entry_id = ? \
             AND mime NOT LIKE 'image/%'"
        } else {
            "SELECT mime, content FROM contents \
             WHERE entry_id = ?"
        };

        let mut stmt = self.conn.prepare(sql)?;
        let mut map = MimeDataMap::new();
        let rows = stmt.query_map(
            params![entry_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                ))
            },
        )?;

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

    pub fn add_content(
        &self,
        entry_id: i64,
        mime: &str,
        content: &[u8],
    ) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO contents \
             (entry_id, mime, content) VALUES (?, ?, ?)",
            params![entry_id, mime, content],
        )?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<(), DbError> {
        self.conn
            .execute("DELETE FROM entries WHERE id = ?", params![id])?;
        Ok(())
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_db() -> Database {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::open(&path, false).unwrap();
        std::mem::forget(dir);
        db
    }

    fn text_data(text: &str) -> MimeDataMap {
        let mut m = HashMap::new();
        m.insert(
            "text/plain;charset=utf-8".into(),
            text.as_bytes().to_vec(),
        );
        m
    }

    fn image_data(data: &[u8]) -> MimeDataMap {
        let mut m = HashMap::new();
        m.insert("image/png".into(), data.to_vec());
        m
    }

    #[test]
    fn insert_and_get_roundtrip() {
        let db = test_db();
        let data = text_data("hello world");
        let id = db.insert(data).unwrap();

        let entry = db.get_entry(id).unwrap().unwrap();
        assert_eq!(entry.id, id);
        assert_eq!(
            entry.text_content(),
            Some("hello world"),
        );
        assert_eq!(entry.entry_type, EntryType::Text);
        assert!(!entry.favorite);
    }

    #[test]
    fn insert_duplicate_text_returns_existing_id() {
        let db = test_db();
        let id1 = db.insert(text_data("same")).unwrap();
        std::thread::sleep(
            std::time::Duration::from_millis(5),
        );
        let id2 = db.insert(text_data("same")).unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn insert_duplicate_text_with_whitespace() {
        let db = test_db();
        let id1 = db.insert(text_data("hello")).unwrap();
        std::thread::sleep(
            std::time::Duration::from_millis(5),
        );
        let id2 =
            db.insert(text_data("  hello  ")).unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn insert_duplicate_image_returns_existing_id() {
        let db = test_db();
        let png = b"fakepngdata";
        let id1 = db.insert(image_data(png)).unwrap();
        std::thread::sleep(
            std::time::Duration::from_millis(5),
        );
        let id2 = db.insert(image_data(png)).unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn insert_duplicate_image() {
        let db = test_db();
        let png = b"fakepngdata";
        let id1 = db.insert(image_data(png)).unwrap();
        std::thread::sleep(
            std::time::Duration::from_millis(5),
        );
        let id2 = db.insert(image_data(png)).unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn insert_different_text_different_ids() {
        let db = test_db();
        let id1 = db.insert(text_data("alpha")).unwrap();
        std::thread::sleep(
            std::time::Duration::from_millis(5),
        );
        let id2 = db.insert(text_data("beta")).unwrap();
        assert_ne!(id1, id2);
    }

    #[test]
    fn list_entries_favorites_first() {
        let db = test_db();
        let id1 = db.insert(text_data("first")).unwrap();
        std::thread::sleep(
            std::time::Duration::from_millis(5),
        );
        let _id2 = db.insert(text_data("second")).unwrap();
        std::thread::sleep(
            std::time::Duration::from_millis(5),
        );

        db.toggle_favorite(id1).unwrap();

        let entries = db.list_entries(10).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, id1);
        assert!(entries[0].favorite);
    }

    #[test]
    fn list_entries_light_skips_images() {
        let db = test_db();
        let mut m = text_data("hello");
        m.insert("image/png".into(), b"imgdata".to_vec());
        let id = db.insert(m).unwrap();

        let light = db.list_entries_light(10).unwrap();
        let entry = light.iter().find(|e| e.id == id).unwrap();
        assert!(entry.image_data().is_none());
        assert!(entry.text_content().is_some());
    }

    #[test]
    fn touch_updates_timestamp() {
        let db = test_db();
        let id = db.insert(text_data("old")).unwrap();
        let before = db
            .get_entry(id)
            .unwrap()
            .unwrap()
            .created_at;

        std::thread::sleep(
            std::time::Duration::from_millis(5),
        );
        db.touch(id).unwrap();

        let after = db
            .get_entry(id)
            .unwrap()
            .unwrap()
            .created_at;
        assert!(after > before);
    }

    #[test]
    fn toggle_favorite_on_and_off() {
        let db = test_db();
        let id = db.insert(text_data("fav")).unwrap();

        assert!(!db.get_entry(id).unwrap().unwrap().favorite);
        db.toggle_favorite(id).unwrap();
        assert!(db.get_entry(id).unwrap().unwrap().favorite);
        db.toggle_favorite(id).unwrap();
        assert!(!db.get_entry(id).unwrap().unwrap().favorite);
    }

    #[test]
    fn delete_removes_entry() {
        let db = test_db();
        let id = db.insert(text_data("gone")).unwrap();
        db.delete(id).unwrap();
        assert!(db.get_entry(id).unwrap().is_none());
    }

    #[test]
    fn clear_removes_non_favorites_only() {
        let db = test_db();
        let fav_id =
            db.insert(text_data("keeper")).unwrap();
        std::thread::sleep(
            std::time::Duration::from_millis(5),
        );
        let _gone_id =
            db.insert(text_data("disposable")).unwrap();

        db.toggle_favorite(fav_id).unwrap();
        db.clear().unwrap();

        let entries = db.list_entries(10).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, fav_id);
    }

    #[test]
    fn add_content_to_existing_entry() {
        let db = test_db();
        let id = db.insert(text_data("base")).unwrap();
        db.add_content(id, "custom/mime", b"extra")
            .unwrap();

        let entry = db.get_entry(id).unwrap().unwrap();
        assert_eq!(
            entry.contents.get("custom/mime"),
            Some(&b"extra".to_vec()),
        );
    }

    #[test]
    fn get_nonexistent_entry_returns_none() {
        let db = test_db();
        assert!(db.get_entry(999999).unwrap().is_none());
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
