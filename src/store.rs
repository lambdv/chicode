//! Local sidebar metadata; OpenCode owns the actual conversation history.
use rusqlite::{Connection, OptionalExtension, params};
use std::{error::Error, fs, path::PathBuf};

pub struct Chat {
    pub id: i64,
    pub title: String,
    pub session: String,
    pub started: bool,
    pub project_id: i64,
}

pub struct Project {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub color: i64,
}

pub struct Store {
    db: Connection,
}

impl Store {
    /// Open a new database, leaving the old Pi app directory untouched.
    pub fn open() -> Result<Self, Box<dyn Error>> {
        let base = if let Some(path) = std::env::var_os("LOCALAPPDATA") {
            PathBuf::from(path)
        } else if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
            PathBuf::from(path)
        } else {
            PathBuf::from(std::env::var_os("HOME").ok_or("No home directory")?).join(".local/share")
        }
        .join("opencode-agent-desktop");
        fs::create_dir_all(&base)?;
        let db = Connection::open(base.join("chats.sqlite"))?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                session TEXT NOT NULL DEFAULT '',
                started INTEGER NOT NULL DEFAULT 0,
                closed INTEGER NOT NULL DEFAULT 0
            );
             CREATE TABLE IF NOT EXISTS preferences (
                key TEXT PRIMARY KEY,
                value REAL NOT NULL
             );
             CREATE TABLE IF NOT EXISTS projects (
                 id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE,
                 name TEXT NOT NULL, color INTEGER NOT NULL
             );",
        )?;
        // Persist sidebar tab order; old databases default to newest-first.
        let _ = db.execute(
            "ALTER TABLE sessions ADD COLUMN sort INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = db.execute("ALTER TABLE sessions ADD COLUMN project_id INTEGER", []);
        Ok(Self { db })
    }

    /// Register a folder once, preserving its identity and muted color.
    pub fn add_project(&self, path: &std::path::Path) -> rusqlite::Result<Project> {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let path = path.to_string_lossy().into_owned();
        let color = path.bytes().fold(0u64, |hash, byte| hash.wrapping_mul(31).wrapping_add(byte as u64)) % 8;
        self.db.execute("INSERT OR IGNORE INTO projects (path, name, color) VALUES (?1, ?2, ?3)", params![path, name, color as i64])?;
        self.db.query_row("SELECT id, path, name, color FROM projects WHERE path = ?1", [path], |row| {
            Ok(Project { id: row.get(0)?, path: row.get(1)?, name: row.get(2)?, color: row.get(3)? })
        })
    }

    /// Return the locally registered folders in registration order.
    pub fn projects(&self) -> rusqlite::Result<Vec<Project>> {
        let mut statement = self.db.prepare("SELECT id, path, name, color FROM projects ORDER BY id")?;
        statement.query_map([], |row| Ok(Project { id: row.get(0)?, path: row.get(1)?, name: row.get(2)?, color: row.get(3)? }))?.collect()
    }

    /// Backfill older chats with the launch folder without changing their sessions.
    pub fn migrate_projects(&self, path: &std::path::Path) -> rusqlite::Result<()> {
        let project = self.add_project(path)?;
        self.db.execute("UPDATE sessions SET project_id = ?1 WHERE project_id IS NULL", [project.id])?;
        if self.last_project()?.is_none() { self.select_project(project.id)?; }
        Ok(())
    }

    /// Get the last folder selected when composing a new chat.
    pub fn last_project(&self) -> rusqlite::Result<Option<i64>> {
        self.db.query_row("SELECT value FROM preferences WHERE key = 'last_project'", [], |row| row.get::<_, f64>(0))
            .optional().map(|id| id.map(|id| id as i64))
    }

    /// Save the default project for subsequent chats.
    pub fn select_project(&self, id: i64) -> rusqlite::Result<()> {
        self.db.execute("INSERT INTO preferences (key, value) VALUES ('last_project', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [id])?;
        Ok(())
    }

    /// Change only an unsent chat's project.
    pub fn set_project(&self, id: i64, project_id: i64) -> rusqlite::Result<bool> {
        Ok(self.db.execute("UPDATE sessions SET project_id = ?1 WHERE id = ?2 AND started = 0", params![project_id, id])? != 0)
    }

    /// List visible chats in sidebar order, newest local item first by default.
    pub fn list(&self) -> rusqlite::Result<Vec<Chat>> {
        let mut statement = self.db.prepare(
            "SELECT id, title, session, started, project_id FROM sessions WHERE closed = 0 ORDER BY sort ASC, id DESC",
        )?;
        statement
            .query_map([], |row| {
                Ok(Chat {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    session: row.get(2)?,
                    started: row.get(3)?,
                    project_id: row.get(4)?,
                })
            })?
            .collect()
    }

    /// Create an empty local sidebar entry on top before OpenCode assigns its ID.
    pub fn create(&self) -> rusqlite::Result<Chat> {
        let project_id = self.last_project()?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        // New tabs go above existing ones regardless of prior drag order.
        let min: Option<i64> = self
            .db
            .query_row("SELECT MIN(sort) FROM sessions", [], |row| row.get(0))
            .unwrap_or(None);
        let sort = min.unwrap_or(0) - 1;
        self.db.execute(
            "INSERT INTO sessions (title, sort, project_id) VALUES ('New chat', ?1, ?2)",
            params![sort, project_id],
        )?;
        Ok(Chat {
            id: self.db.last_insert_rowid(),
            title: "New chat".into(),
            session: String::new(),
            started: false,
            project_id,
        })
    }

    /// Save the sidebar order after a tab drag, top row first.
    pub fn reorder(&self, ids: &[i64]) -> rusqlite::Result<()> {
        for (position, id) in ids.iter().enumerate() {
            self.db.execute(
                "UPDATE sessions SET sort = ?1 WHERE id = ?2",
                params![position as i64, id],
            )?;
        }
        Ok(())
    }

    /// Link the local chat to its OpenCode session after creation.
    pub fn set_session(&self, id: i64, session: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE sessions SET session = ?1 WHERE id = ?2",
            params![session, id],
        )?;
        Ok(())
    }

    /// Record that the first prompt was sent.
    pub fn mark_started(&self, id: i64) -> rusqlite::Result<()> {
        self.db
            .execute("UPDATE sessions SET started = 1 WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Hide a chat without deleting its OpenCode session.
    pub fn close(&self, id: i64) -> rusqlite::Result<()> {
        self.db
            .execute("UPDATE sessions SET closed = 1 WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Save the sidebar title independently from the server session.
    pub fn rename(&self, id: i64, title: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE sessions SET title = ?1 WHERE id = ?2",
            params![title, id],
        )?;
        Ok(())
    }

    /// Load the preferred UI zoom, defaulting to the normal size.
    pub fn zoom(&self) -> rusqlite::Result<f32> {
        let zoom: Option<f32> = self
            .db
            .query_row(
                "SELECT value FROM preferences WHERE key = 'zoom'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(zoom
            .filter(|value| value.is_finite())
            .unwrap_or(1.0)
            .clamp(0.75, 2.0))
    }

    /// Save the UI zoom for the next app window or launch.
    pub fn set_zoom(&self, zoom: f32) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO preferences (key, value) VALUES ('zoom', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [zoom],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Closing a chat only hides its local entry; it never deletes history.
    #[test]
    fn sidebar_tracks_sessions() {
        let store = Store {
            db: Connection::open_in_memory().unwrap(),
        };
        store
            .db
            .execute_batch(
                "CREATE TABLE sessions (
             id INTEGER PRIMARY KEY, title TEXT NOT NULL, session TEXT NOT NULL DEFAULT '',
             started INTEGER NOT NULL DEFAULT 0, closed INTEGER NOT NULL DEFAULT 0,
             sort INTEGER NOT NULL DEFAULT 0, project_id INTEGER);
             CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, name TEXT NOT NULL, color INTEGER NOT NULL);
             CREATE TABLE preferences (key TEXT PRIMARY KEY, value REAL NOT NULL)",
            )
            .unwrap();
        store.add_project(std::path::Path::new("test-project")).unwrap();
        store.select_project(1).unwrap();
        let first = store.create().unwrap();
        store.set_session(first.id, "ses_test").unwrap();
        store.mark_started(first.id).unwrap();
        let blank = store.create().unwrap();
        assert_eq!(store.list().unwrap()[0].id, blank.id);
        store.close(blank.id).unwrap();
        let chats = store.list().unwrap();
        assert_eq!(chats.len(), 1);
        assert_eq!(chats[0].session, "ses_test");
        assert!(chats[0].started);
    }

    /// Dragging a tab persists the new sidebar order.
    #[test]
    fn sidebar_reorders_tabs() {
        let store = Store {
            db: Connection::open_in_memory().unwrap(),
        };
        store
            .db
            .execute_batch(
                "CREATE TABLE sessions (
             id INTEGER PRIMARY KEY, title TEXT NOT NULL, session TEXT NOT NULL DEFAULT '',
             started INTEGER NOT NULL DEFAULT 0, closed INTEGER NOT NULL DEFAULT 0,
             sort INTEGER NOT NULL DEFAULT 0, project_id INTEGER);
             CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, name TEXT NOT NULL, color INTEGER NOT NULL);
             CREATE TABLE preferences (key TEXT PRIMARY KEY, value REAL NOT NULL)",
            )
            .unwrap();
        store.add_project(std::path::Path::new("test-project")).unwrap();
        store.select_project(1).unwrap();
        let first = store.create().unwrap();
        let second = store.create().unwrap();
        // Newest tab starts on top.
        assert_eq!(store.list().unwrap()[0].id, second.id);
        // Drag the bottom tab to the top.
        store.reorder(&[first.id, second.id]).unwrap();
        let chats = store.list().unwrap();
        assert_eq!(chats[0].id, first.id);
        assert_eq!(chats[1].id, second.id);
    }

    /// Registered folders are unique, and only unsent chats may move.
    #[test]
    fn projects_bind_new_chats() {
        let store = Store { db: Connection::open_in_memory().unwrap() };
        store.db.execute_batch("CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, name TEXT NOT NULL, color INTEGER NOT NULL);
            CREATE TABLE preferences (key TEXT PRIMARY KEY, value REAL NOT NULL);
            CREATE TABLE sessions (id INTEGER PRIMARY KEY, title TEXT NOT NULL, session TEXT NOT NULL DEFAULT '',
                started INTEGER NOT NULL DEFAULT 0, closed INTEGER NOT NULL DEFAULT 0, sort INTEGER NOT NULL DEFAULT 0, project_id INTEGER)").unwrap();
        let first = store.add_project(std::path::Path::new("first")).unwrap();
        assert_eq!(store.add_project(std::path::Path::new("first")).unwrap().id, first.id);
        let second = store.add_project(std::path::Path::new("second")).unwrap();
        store.select_project(first.id).unwrap();
        let chat = store.create().unwrap();
        assert_eq!(chat.project_id, first.id);
        assert!(store.set_project(chat.id, second.id).unwrap());
        store.mark_started(chat.id).unwrap();
        assert!(!store.set_project(chat.id, first.id).unwrap());
        assert_eq!(store.list().unwrap()[0].project_id, second.id);
    }

    /// Legacy chat rows are attached to the original launch folder.
    #[test]
    fn existing_chats_get_launch_project() {
        let store = Store { db: Connection::open_in_memory().unwrap() };
        store.db.execute_batch("CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, name TEXT NOT NULL, color INTEGER NOT NULL);
            CREATE TABLE preferences (key TEXT PRIMARY KEY, value REAL NOT NULL);
            CREATE TABLE sessions (id INTEGER PRIMARY KEY, title TEXT NOT NULL, session TEXT NOT NULL DEFAULT '',
                started INTEGER NOT NULL DEFAULT 0, closed INTEGER NOT NULL DEFAULT 0, sort INTEGER NOT NULL DEFAULT 0, project_id INTEGER);
            INSERT INTO sessions (title, session, started) VALUES ('Old chat', 'ses_old', 1)").unwrap();
        store.migrate_projects(std::path::Path::new("launch-folder")).unwrap();
        let chat = store.list().unwrap().remove(0);
        assert_eq!(chat.session, "ses_old");
        assert_eq!(chat.project_id, store.last_project().unwrap().unwrap());
    }

    /// Zoom preferences survive reads from the same database.
    #[test]
    fn zoom_preference() {
        let store = Store {
            db: Connection::open_in_memory().unwrap(),
        };
        store
            .db
            .execute_batch("CREATE TABLE preferences (key TEXT PRIMARY KEY, value REAL NOT NULL)")
            .unwrap();
        assert_eq!(store.zoom().unwrap(), 1.0);
        store.set_zoom(1.8).unwrap();
        assert!((store.zoom().unwrap() - 1.8).abs() < 0.001);
    }
}
