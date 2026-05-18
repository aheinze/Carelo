use crate::fs::models::{FsError, FsResult};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const STORE_DIRECTORY: &str = ".local/share/carelo";
const STORE_FILE: &str = "carelo.store";
const FAVORITES_SEEDED_KEY: &str = "favorites.seeded.v1";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteEntry {
    pub id: String,
    pub name: String,
    pub path: String,
    pub icon: String,
    pub color: Option<String>,
    pub sort_order: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteInput {
    pub name: Option<String>,
    pub path: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub sort_order: Option<i64>,
}

pub struct AppStoreState {
    connection: Mutex<Connection>,
    path: PathBuf,
}

impl AppStoreState {
    pub fn initialize() -> FsResult<Self> {
        let path = store_path()?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                store_io_error("Unable to create app store directory", parent, error)
            })?;
        }

        let connection = Connection::open(&path)
            .map_err(|error| store_sql_error("Unable to open app store", &path, error))?;

        migrate(&connection, &path)?;
        seed_default_favorites(&connection, &path)?;

        Ok(Self {
            connection: Mutex::new(connection),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list_favorites(&self) -> FsResult<Vec<FavoriteEntry>> {
        let connection = self.connection()?;
        list_favorites(&connection, &self.path)
    }

    pub fn add_favorite(&self, favorite: FavoriteInput) -> FsResult<FavoriteEntry> {
        let connection = self.connection()?;
        let path = normalize_favorite_path(&favorite.path)?;
        let now = unix_timestamp();
        let existing = favorite_by_path(&connection, &self.path, &path)?;

        if let Some(existing) = existing {
            return Ok(existing);
        }

        let next_sort_order = favorite
            .sort_order
            .unwrap_or(next_favorite_sort_order(&connection, &self.path)?);
        let entry = FavoriteEntry {
            id: favorite_id_for_path(&path),
            name: favorite
                .name
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| favorite_name_for_path(&path)),
            path,
            icon: favorite
                .icon
                .map(|icon| icon.trim().to_string())
                .filter(|icon| !icon.is_empty())
                .unwrap_or_else(|| "folder".to_string()),
            color: favorite
                .color
                .map(|color| color.trim().to_string())
                .filter(|color| !color.is_empty()),
            sort_order: next_sort_order,
            created_at: now,
            updated_at: now,
        };

        connection
            .execute(
                "INSERT INTO favorites (id, name, path, icon, color, sort_order, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    entry.id,
                    entry.name,
                    entry.path,
                    entry.icon,
                    entry.color,
                    entry.sort_order,
                    entry.created_at,
                    entry.updated_at
                ],
            )
            .map_err(|error| store_sql_error("Unable to add favorite", &self.path, error))?;

        reorder_favorites(&connection, &self.path)?;
        favorite_by_id(&connection, &self.path, &entry.id)?.ok_or_else(|| {
            FsError::new("store_error", "Favorite was not created.", Some(entry.path))
        })
    }

    pub fn remove_favorite(&self, id: String) -> FsResult<()> {
        let connection = self.connection()?;

        connection
            .execute("DELETE FROM favorites WHERE id = ?1", params![id])
            .map_err(|error| store_sql_error("Unable to remove favorite", &self.path, error))?;

        reorder_favorites(&connection, &self.path)
    }

    pub fn move_favorite(&self, id: String, target_index: i64) -> FsResult<Vec<FavoriteEntry>> {
        let connection = self.connection()?;
        let mut favorites = list_favorites(&connection, &self.path)?;
        let Some(source_index) = favorites.iter().position(|favorite| favorite.id == id) else {
            return Ok(favorites);
        };

        let favorite = favorites.remove(source_index);
        let target_index = target_index.max(0).min(favorites.len() as i64) as usize;
        let target_index = if source_index < target_index {
            target_index.saturating_sub(1)
        } else {
            target_index
        };
        favorites.insert(target_index, favorite);

        let now = unix_timestamp();
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| store_sql_error("Unable to reorder favorites", &self.path, error))?;

        for (index, favorite) in favorites.iter().enumerate() {
            transaction
                .execute(
                    "UPDATE favorites SET sort_order = ?1, updated_at = ?2 WHERE id = ?3",
                    params![index as i64, now, favorite.id],
                )
                .map_err(|error| {
                    store_sql_error("Unable to reorder favorites", &self.path, error)
                })?;
        }

        transaction
            .commit()
            .map_err(|error| store_sql_error("Unable to save favorite order", &self.path, error))?;

        list_favorites(&connection, &self.path)
    }

    fn connection(&self) -> FsResult<std::sync::MutexGuard<'_, Connection>> {
        self.connection.lock().map_err(|error| {
            FsError::new(
                "store_lock_failed",
                format!("Unable to lock app store: {error}"),
                Some(self.path.to_string_lossy().into_owned()),
            )
        })
    }
}

fn store_path() -> FsResult<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| {
        FsError::new(
            "home_directory_unavailable",
            "Unable to locate the home directory for the app store.",
            None,
        )
    })?;

    Ok(PathBuf::from(home).join(STORE_DIRECTORY).join(STORE_FILE))
}

fn migrate(connection: &Connection, path: &Path) -> FsResult<()> {
    connection
        .execute_batch(
            r#"
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS store_metadata (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS favorites (
              id TEXT PRIMARY KEY,
              name TEXT NOT NULL,
              path TEXT NOT NULL UNIQUE,
              icon TEXT NOT NULL DEFAULT 'folder',
              color TEXT,
              sort_order INTEGER NOT NULL,
              created_at INTEGER NOT NULL,
              updated_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_favorites_sort_order ON favorites(sort_order);
            "#,
        )
        .map_err(|error| store_sql_error("Unable to migrate app store", path, error))
}

fn seed_default_favorites(connection: &Connection, path: &Path) -> FsResult<()> {
    let seeded: Option<String> = connection
        .query_row(
            "SELECT value FROM store_metadata WHERE key = ?1",
            params![FAVORITES_SEEDED_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| store_sql_error("Unable to read app store metadata", path, error))?;

    if seeded.is_some() {
        return Ok(());
    }

    let now = unix_timestamp();
    let defaults = [
        ("favorite-home", "Home", "~", "home", "#f5a22a", 0_i64),
        (
            "favorite-desktop",
            "Desktop",
            "~/Desktop",
            "columns",
            "#f5a22a",
            1_i64,
        ),
        (
            "favorite-documents",
            "Documents",
            "~/Documents",
            "folder",
            "#5ca8ff",
            2_i64,
        ),
        (
            "favorite-downloads",
            "Downloads",
            "~/Downloads",
            "download",
            "#31db68",
            3_i64,
        ),
    ];
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| store_sql_error("Unable to seed favorites", path, error))?;

    for (id, name, favorite_path, icon, color, sort_order) in defaults {
        transaction
            .execute(
                "INSERT OR IGNORE INTO favorites
                 (id, name, path, icon, color, sort_order, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![id, name, favorite_path, icon, color, sort_order, now, now],
            )
            .map_err(|error| store_sql_error("Unable to seed favorites", path, error))?;
    }

    transaction
        .execute(
            "INSERT OR REPLACE INTO store_metadata (key, value) VALUES (?1, ?2)",
            params![FAVORITES_SEEDED_KEY, "1"],
        )
        .map_err(|error| store_sql_error("Unable to save app store metadata", path, error))?;

    transaction
        .commit()
        .map_err(|error| store_sql_error("Unable to save default favorites", path, error))?;

    reorder_favorites(connection, path)
}

fn list_favorites(connection: &Connection, path: &Path) -> FsResult<Vec<FavoriteEntry>> {
    let mut statement = connection
        .prepare(
            "SELECT id, name, path, icon, color, sort_order, created_at, updated_at
             FROM favorites
             ORDER BY sort_order ASC, name COLLATE NOCASE ASC",
        )
        .map_err(|error| store_sql_error("Unable to read favorites", path, error))?;

    let rows = statement
        .query_map([], favorite_from_row)
        .map_err(|error| store_sql_error("Unable to read favorites", path, error))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| store_sql_error("Unable to read favorites", path, error))
}

fn favorite_by_path(
    connection: &Connection,
    path: &Path,
    favorite_path: &str,
) -> FsResult<Option<FavoriteEntry>> {
    connection
        .query_row(
            "SELECT id, name, path, icon, color, sort_order, created_at, updated_at
             FROM favorites
             WHERE path = ?1",
            params![favorite_path],
            favorite_from_row,
        )
        .optional()
        .map_err(|error| store_sql_error("Unable to read favorite", path, error))
}

fn favorite_by_id(
    connection: &Connection,
    path: &Path,
    id: &str,
) -> FsResult<Option<FavoriteEntry>> {
    connection
        .query_row(
            "SELECT id, name, path, icon, color, sort_order, created_at, updated_at
             FROM favorites
             WHERE id = ?1",
            params![id],
            favorite_from_row,
        )
        .optional()
        .map_err(|error| store_sql_error("Unable to read favorite", path, error))
}

fn favorite_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FavoriteEntry> {
    Ok(FavoriteEntry {
        id: row.get(0)?,
        name: row.get(1)?,
        path: row.get(2)?,
        icon: row.get(3)?,
        color: row.get(4)?,
        sort_order: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn next_favorite_sort_order(connection: &Connection, path: &Path) -> FsResult<i64> {
    connection
        .query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM favorites",
            [],
            |row| row.get(0),
        )
        .map_err(|error| store_sql_error("Unable to read favorite order", path, error))
}

fn reorder_favorites(connection: &Connection, path: &Path) -> FsResult<()> {
    let favorites = list_favorites(connection, path)?;
    let now = unix_timestamp();
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| store_sql_error("Unable to normalize favorite order", path, error))?;

    for (index, favorite) in favorites.iter().enumerate() {
        transaction
            .execute(
                "UPDATE favorites SET sort_order = ?1, updated_at = ?2 WHERE id = ?3",
                params![index as i64, now, favorite.id],
            )
            .map_err(|error| store_sql_error("Unable to normalize favorite order", path, error))?;
    }

    transaction
        .commit()
        .map_err(|error| store_sql_error("Unable to normalize favorite order", path, error))?;

    Ok(())
}

fn normalize_favorite_path(path: &str) -> FsResult<String> {
    let value = path.trim();

    if value.is_empty() {
        return Err(FsError::invalid_path(path));
    }

    if value == "/" || value == "~" {
        return Ok(value.to_string());
    }

    Ok(value.trim_end_matches('/').to_string())
}

fn favorite_id_for_path(path: &str) -> String {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    format!("favorite-{:x}", hasher.finish())
}

fn favorite_name_for_path(path: &str) -> String {
    let clean_path = path.trim_end_matches('/');

    if clean_path.is_empty() || clean_path == "~" {
        return "Home".to_string();
    }

    if clean_path == "/" {
        return "Root".to_string();
    }

    if let Some(rest) = clean_path.strip_prefix("remote://") {
        return rest
            .split('/')
            .filter(|part| !part.is_empty())
            .last()
            .unwrap_or(rest)
            .to_string();
    }

    clean_path
        .split('/')
        .filter(|part| !part.is_empty() && *part != "~")
        .last()
        .unwrap_or(clean_path)
        .to_string()
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn store_io_error(action: &str, path: &Path, error: std::io::Error) -> FsError {
    FsError::new(
        "store_io_error",
        format!("{action}: {error}"),
        Some(path.to_string_lossy().into_owned()),
    )
}

fn store_sql_error(action: &str, path: &Path, error: rusqlite::Error) -> FsError {
    FsError::new(
        "store_sql_error",
        format!("{action}: {error}"),
        Some(path.to_string_lossy().into_owned()),
    )
}
