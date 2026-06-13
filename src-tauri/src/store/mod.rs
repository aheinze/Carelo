use crate::fs::models::{FileEntry, FsError, FsResult};
use crate::fs::remote::RemoteVolumeConfig;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const STORE_DIRECTORY: &str = ".local/share/carelo";
const STORE_FILE: &str = "carelo.store";
const FAVORITES_SEEDED_KEY: &str = "favorites.seeded.v1";
const DEFAULT_FAVORITE_GROUP_ID: &str = "favorites";
const DEFAULT_FAVORITE_GROUP_NAME: &str = "Favorites";
const WINDOW_DIMENSIONS_KEY: &str = "window.dimensions.v1";
const APP_SETTINGS_KEY: &str = "app.settings.v1";
const REMOTE_VOLUMES_KEY: &str = "remote.volumes.v1";
const MIN_WINDOW_WIDTH: f64 = 960.0;
const MIN_WINDOW_HEIGHT: f64 = 640.0;
const MAX_WINDOW_DIMENSION: f64 = 10000.0;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteEntry {
    pub id: String,
    pub group_id: String,
    pub name: String,
    pub path: String,
    pub icon: String,
    pub color: Option<String>,
    pub sort_order: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteGroupEntry {
    pub id: String,
    pub name: String,
    pub sort_order: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteInput {
    pub group_id: Option<String>,
    pub name: Option<String>,
    pub path: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub sort_order: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowDimensions {
    pub width: f64,
    pub height: f64,
    pub saved_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenWithDefaultEntry {
    pub file_type_key: String,
    pub app_id: String,
    pub app_name: String,
    pub updated_at: i64,
}

pub struct AppStoreState {
    connection: Mutex<Connection>,
    path: PathBuf,
    // In-memory cache of path -> color so directory listings don't hit the DB.
    tags: Mutex<HashMap<String, String>>,
}

impl AppStoreState {
    pub fn initialize() -> FsResult<Self> {
        Self::initialize_at(store_path()?)
    }

    fn initialize_at(path: PathBuf) -> FsResult<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                store_io_error("Unable to create app store directory", parent, error)
            })?;
        }

        let connection = Connection::open(&path)
            .map_err(|error| store_sql_error("Unable to open app store", &path, error))?;

        migrate(&connection, &path)?;
        seed_default_favorites(&connection, &path)?;
        let tags = load_file_tags(&connection, &path)?;

        Ok(Self {
            connection: Mutex::new(connection),
            path,
            tags: Mutex::new(tags),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Stamp cached color tags onto a freshly listed set of entries.
    pub fn apply_file_tags(&self, entries: &mut [FileEntry]) {
        let Ok(tags) = self.tags.lock() else {
            return;
        };

        if tags.is_empty() {
            return;
        }

        for entry in entries.iter_mut() {
            if let Some(color) = tags.get(&entry.path) {
                entry.tag_color = Some(color.clone());
            }
        }
    }

    /// Set (Some) or clear (None) the color tag for a path.
    pub fn set_file_tag(&self, path: String, color: Option<String>) -> FsResult<()> {
        {
            let connection = self.connection()?;

            match color.as_deref() {
                Some(value) => {
                    connection
                        .execute(
                            "INSERT INTO file_tags(path, color, updated_at) VALUES(?1, ?2, ?3) ON CONFLICT(path) DO UPDATE SET color = excluded.color, updated_at = excluded.updated_at",
                            params![path, value, unix_timestamp()],
                        )
                        .map_err(|error| store_sql_error("Unable to save file tag", &self.path, error))?;
                }
                None => {
                    connection
                        .execute("DELETE FROM file_tags WHERE path = ?1", params![path])
                        .map_err(|error| store_sql_error("Unable to clear file tag", &self.path, error))?;
                }
            }
        }

        let mut tags = self.tags_guard()?;

        match color {
            Some(value) => {
                tags.insert(path, value);
            }
            None => {
                tags.remove(&path);
            }
        }

        Ok(())
    }

    /// Re-key a tag when its file is renamed or moved, so the tag follows it.
    pub fn move_file_tag(&self, from: &str, to: &str) -> FsResult<()> {
        if from == to {
            return Ok(());
        }

        let color = self.tags_guard()?.get(from).cloned();
        let Some(color) = color else {
            return Ok(());
        };

        {
            let connection = self.connection()?;
            connection
                .execute("DELETE FROM file_tags WHERE path = ?1", params![from])
                .map_err(|error| store_sql_error("Unable to move file tag", &self.path, error))?;
            connection
                .execute(
                    "INSERT INTO file_tags(path, color, updated_at) VALUES(?1, ?2, ?3) ON CONFLICT(path) DO UPDATE SET color = excluded.color, updated_at = excluded.updated_at",
                    params![to, color, unix_timestamp()],
                )
                .map_err(|error| store_sql_error("Unable to move file tag", &self.path, error))?;
        }

        let mut tags = self.tags_guard()?;
        tags.remove(from);
        tags.insert(to.to_string(), color);
        Ok(())
    }

    fn tags_guard(&self) -> FsResult<std::sync::MutexGuard<'_, HashMap<String, String>>> {
        self.tags.lock().map_err(|error| {
            FsError::new(
                "store_lock_failed",
                format!("Unable to lock file tags: {error}"),
                None,
            )
        })
    }

    pub fn list_favorites(&self) -> FsResult<Vec<FavoriteEntry>> {
        let connection = self.connection()?;
        list_favorites(&connection, &self.path)
    }

    pub fn list_favorite_groups(&self) -> FsResult<Vec<FavoriteGroupEntry>> {
        let connection = self.connection()?;
        list_favorite_groups(&connection, &self.path)
    }

    pub fn add_favorite_group(&self, name: String) -> FsResult<FavoriteGroupEntry> {
        let connection = self.connection()?;
        let name = normalize_favorite_group_name(&name)?;

        if let Some(existing) = favorite_group_by_name(&connection, &self.path, &name)? {
            return Ok(existing);
        }

        let now = unix_timestamp();
        let entry = FavoriteGroupEntry {
            id: favorite_group_id_for_name(&name, now),
            name,
            sort_order: next_favorite_group_sort_order(&connection, &self.path)?,
            created_at: now,
            updated_at: now,
        };

        connection
            .execute(
                "INSERT INTO favorite_groups (id, name, sort_order, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    entry.id,
                    entry.name,
                    entry.sort_order,
                    entry.created_at,
                    entry.updated_at
                ],
            )
            .map_err(|error| store_sql_error("Unable to add favorite group", &self.path, error))?;

        favorite_group_by_id(&connection, &self.path, &entry.id)?.ok_or_else(|| {
            FsError::new(
                "store_error",
                "Favorite group was not created.",
                Some(entry.name),
            )
        })
    }

    pub fn remove_favorite_group(&self, id: String) -> FsResult<()> {
        let connection = self.connection()?;
        let id = id.trim();

        if id.is_empty() || id == DEFAULT_FAVORITE_GROUP_ID {
            return Err(FsError::new(
                "invalid_favorite_group",
                "The default Favorites group cannot be removed.",
                None,
            ));
        }

        if favorite_group_by_id(&connection, &self.path, id)?.is_none() {
            return Ok(());
        }

        let transaction = connection.unchecked_transaction().map_err(|error| {
            store_sql_error("Unable to remove favorite group", &self.path, error)
        })?;

        transaction
            .execute("DELETE FROM favorites WHERE group_id = ?1", params![id])
            .map_err(|error| {
                store_sql_error(
                    "Unable to remove favorite group shortcuts",
                    &self.path,
                    error,
                )
            })?;

        transaction
            .execute("DELETE FROM favorite_groups WHERE id = ?1", params![id])
            .map_err(|error| {
                store_sql_error("Unable to remove favorite group", &self.path, error)
            })?;

        transaction.commit().map_err(|error| {
            store_sql_error("Unable to save favorite group changes", &self.path, error)
        })?;

        Ok(())
    }

    pub fn add_favorite(&self, favorite: FavoriteInput) -> FsResult<FavoriteEntry> {
        let connection = self.connection()?;
        let path = normalize_favorite_path(&favorite.path)?;
        let now = unix_timestamp();
        let existing = favorite_by_path(&connection, &self.path, &path)?;

        if let Some(existing) = existing {
            return Ok(existing);
        }

        let group_id = favorite
            .group_id
            .as_deref()
            .and_then(|group_id| {
                favorite_group_id_if_exists(&connection, &self.path, group_id).transpose()
            })
            .transpose()?
            .unwrap_or_else(|| DEFAULT_FAVORITE_GROUP_ID.to_string());
        let next_sort_order = favorite.sort_order.unwrap_or(next_favorite_sort_order(
            &connection,
            &self.path,
            &group_id,
        )?);
        let entry = FavoriteEntry {
            id: favorite_id_for_path(&path),
            group_id,
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
                "INSERT INTO favorites (id, group_id, name, path, icon, color, sort_order, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    entry.id,
                    entry.group_id,
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

        reorder_favorites_in_group(&connection, &self.path, &entry.group_id)?;
        favorite_by_id(&connection, &self.path, &entry.id)?.ok_or_else(|| {
            FsError::new("store_error", "Favorite was not created.", Some(entry.path))
        })
    }

    pub fn remove_favorite(&self, id: String) -> FsResult<()> {
        let connection = self.connection()?;
        let group_id = favorite_by_id(&connection, &self.path, &id)?
            .map(|favorite| favorite.group_id)
            .unwrap_or_else(|| DEFAULT_FAVORITE_GROUP_ID.to_string());

        connection
            .execute("DELETE FROM favorites WHERE id = ?1", params![id])
            .map_err(|error| store_sql_error("Unable to remove favorite", &self.path, error))?;

        reorder_favorites_in_group(&connection, &self.path, &group_id)
    }

    pub fn move_favorite(
        &self,
        id: String,
        target_group_id: Option<String>,
        target_index: i64,
    ) -> FsResult<Vec<FavoriteEntry>> {
        let connection = self.connection()?;
        let Some(existing) = favorite_by_id(&connection, &self.path, &id)? else {
            return list_favorites(&connection, &self.path);
        };

        let source_group_id = existing.group_id;
        let target_group_id = target_group_id
            .as_deref()
            .and_then(|group_id| {
                favorite_group_id_if_exists(&connection, &self.path, group_id).transpose()
            })
            .transpose()?
            .unwrap_or_else(|| source_group_id.clone());

        let mut source_favorites =
            list_favorites_in_group(&connection, &self.path, &source_group_id)?;
        let Some(source_index) = source_favorites
            .iter()
            .position(|favorite| favorite.id == id)
        else {
            return list_favorites(&connection, &self.path);
        };

        let mut favorite = source_favorites.remove(source_index);
        favorite.group_id = target_group_id.clone();

        let same_group = source_group_id == target_group_id;
        let mut target_favorites = if same_group {
            source_favorites.clone()
        } else {
            list_favorites_in_group(&connection, &self.path, &target_group_id)?
        };

        let target_limit = target_favorites.len() + usize::from(same_group);
        let target_index = target_index.max(0).min(target_limit as i64) as usize;
        let target_index = if same_group && source_index < target_index {
            target_index.saturating_sub(1)
        } else {
            target_index
        };
        target_favorites.insert(target_index, favorite);

        let now = unix_timestamp();
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| store_sql_error("Unable to reorder favorites", &self.path, error))?;

        if !same_group {
            write_favorite_order(
                &transaction,
                &self.path,
                &source_group_id,
                &source_favorites,
                now,
            )?;
        }
        write_favorite_order(
            &transaction,
            &self.path,
            &target_group_id,
            &target_favorites,
            now,
        )?;

        transaction
            .commit()
            .map_err(|error| store_sql_error("Unable to save favorite order", &self.path, error))?;

        list_favorites(&connection, &self.path)
    }

    pub fn window_dimensions(&self) -> FsResult<Option<WindowDimensions>> {
        let connection = self.connection()?;
        let value: Option<String> = connection
            .query_row(
                "SELECT value FROM store_metadata WHERE key = ?1",
                params![WINDOW_DIMENSIONS_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| {
                store_sql_error("Unable to read window dimensions", &self.path, error)
            })?;

        let Some(value) = value else {
            return Ok(None);
        };

        let dimensions: WindowDimensions = serde_json::from_str(&value).map_err(|error| {
            FsError::new(
                "store_parse_error",
                format!("Unable to parse saved window dimensions: {error}"),
                Some(self.path.to_string_lossy().into_owned()),
            )
        })?;

        Ok(normalize_window_dimensions(dimensions))
    }

    pub fn save_window_dimensions(&self, dimensions: WindowDimensions) -> FsResult<()> {
        let Some(dimensions) = normalize_window_dimensions(dimensions) else {
            return Ok(());
        };

        let value = serde_json::to_string(&dimensions).map_err(|error| {
            FsError::new(
                "store_serialize_error",
                format!("Unable to serialize window dimensions: {error}"),
                Some(self.path.to_string_lossy().into_owned()),
            )
        })?;
        let connection = self.connection()?;

        connection
            .execute(
                "INSERT OR REPLACE INTO store_metadata (key, value) VALUES (?1, ?2)",
                params![WINDOW_DIMENSIONS_KEY, value],
            )
            .map_err(|error| {
                store_sql_error("Unable to save window dimensions", &self.path, error)
            })?;

        Ok(())
    }

    pub fn app_settings(&self) -> FsResult<Option<Value>> {
        let connection = self.connection()?;
        let value: Option<String> = connection
            .query_row(
                "SELECT value FROM store_metadata WHERE key = ?1",
                params![APP_SETTINGS_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| store_sql_error("Unable to read app settings", &self.path, error))?;

        let Some(value) = value else {
            return Ok(None);
        };

        serde_json::from_str(&value).map(Some).map_err(|error| {
            FsError::new(
                "store_parse_error",
                format!("Unable to parse saved app settings: {error}"),
                Some(self.path.to_string_lossy().into_owned()),
            )
        })
    }

    pub fn save_app_settings(&self, settings: Value) -> FsResult<()> {
        let value = serde_json::to_string(&settings).map_err(|error| {
            FsError::new(
                "store_serialize_error",
                format!("Unable to serialize app settings: {error}"),
                Some(self.path.to_string_lossy().into_owned()),
            )
        })?;
        let connection = self.connection()?;

        connection
            .execute(
                "INSERT OR REPLACE INTO store_metadata (key, value) VALUES (?1, ?2)",
                params![APP_SETTINGS_KEY, value],
            )
            .map_err(|error| store_sql_error("Unable to save app settings", &self.path, error))?;

        Ok(())
    }

    pub fn list_remote_volume_configs(&self) -> FsResult<Vec<RemoteVolumeConfig>> {
        let connection = self.connection()?;
        let value: Option<String> = connection
            .query_row(
                "SELECT value FROM store_metadata WHERE key = ?1",
                params![REMOTE_VOLUMES_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| store_sql_error("Unable to read remote volumes", &self.path, error))?;

        let Some(value) = value else {
            return Ok(Vec::new());
        };

        serde_json::from_str(&value).map_err(|error| {
            FsError::new(
                "store_parse_error",
                format!("Unable to parse saved remote volumes: {error}"),
                Some(self.path.to_string_lossy().into_owned()),
            )
        })
    }

    pub fn save_remote_volume_config(&self, config: RemoteVolumeConfig) -> FsResult<()> {
        let mut configs = self.list_remote_volume_configs()?;

        if let Some(existing) = configs.iter_mut().find(|existing| existing.id == config.id) {
            *existing = config;
        } else {
            configs.push(config);
        }

        configs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        self.write_remote_volume_configs(&configs)
    }

    pub fn remove_remote_volume_config(&self, id: &str) -> FsResult<bool> {
        let mut configs = self.list_remote_volume_configs()?;
        let original_len = configs.len();
        configs.retain(|config| config.id != id);
        let removed = configs.len() != original_len;

        if removed {
            self.write_remote_volume_configs(&configs)?;
        }

        Ok(removed)
    }

    pub fn open_with_default(&self, file_type_key: &str) -> FsResult<Option<OpenWithDefaultEntry>> {
        let connection = self.connection()?;

        connection
            .query_row(
                "SELECT file_type_key, app_id, app_name, updated_at
                 FROM open_with_defaults
                 WHERE file_type_key = ?1",
                params![file_type_key],
                open_with_default_from_row,
            )
            .optional()
            .map_err(|error| store_sql_error("Unable to read open-with default", &self.path, error))
    }

    pub fn save_open_with_default(
        &self,
        file_type_key: &str,
        app_id: &str,
        app_name: &str,
    ) -> FsResult<()> {
        let key = file_type_key.trim();
        let app_id = app_id.trim();
        let app_name = app_name.trim();

        if key.is_empty() || app_id.is_empty() || app_name.is_empty() {
            return Err(FsError::new(
                "invalid_open_with_default",
                "Unable to save an incomplete open-with default.",
                None,
            ));
        }

        let connection = self.connection()?;
        connection
            .execute(
                "INSERT OR REPLACE INTO open_with_defaults
                 (file_type_key, app_id, app_name, updated_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![key, app_id, app_name, unix_timestamp()],
            )
            .map_err(|error| {
                store_sql_error("Unable to save open-with default", &self.path, error)
            })?;

        Ok(())
    }

    pub fn clear_open_with_default(&self, file_type_key: &str) -> FsResult<()> {
        let key = file_type_key.trim();

        if key.is_empty() {
            return Ok(());
        }

        let connection = self.connection()?;
        connection
            .execute(
                "DELETE FROM open_with_defaults WHERE file_type_key = ?1",
                params![key],
            )
            .map_err(|error| {
                store_sql_error("Unable to clear open-with default", &self.path, error)
            })?;

        Ok(())
    }

    fn write_remote_volume_configs(&self, configs: &[RemoteVolumeConfig]) -> FsResult<()> {
        let value = serde_json::to_string(configs).map_err(|error| {
            FsError::new(
                "store_serialize_error",
                format!("Unable to serialize remote volumes: {error}"),
                Some(self.path.to_string_lossy().into_owned()),
            )
        })?;
        let connection = self.connection()?;

        connection
            .execute(
                "INSERT OR REPLACE INTO store_metadata (key, value) VALUES (?1, ?2)",
                params![REMOTE_VOLUMES_KEY, value],
            )
            .map_err(|error| store_sql_error("Unable to save remote volumes", &self.path, error))?;

        Ok(())
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
              group_id TEXT NOT NULL DEFAULT 'favorites',
              name TEXT NOT NULL,
              path TEXT NOT NULL UNIQUE,
              icon TEXT NOT NULL DEFAULT 'folder',
              color TEXT,
              sort_order INTEGER NOT NULL,
              created_at INTEGER NOT NULL,
              updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS favorite_groups (
              id TEXT PRIMARY KEY,
              name TEXT NOT NULL,
              sort_order INTEGER NOT NULL,
              created_at INTEGER NOT NULL,
              updated_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_favorites_sort_order ON favorites(sort_order);
            CREATE INDEX IF NOT EXISTS idx_favorite_groups_sort_order ON favorite_groups(sort_order);

            CREATE TABLE IF NOT EXISTS open_with_defaults (
              file_type_key TEXT PRIMARY KEY,
              app_id TEXT NOT NULL,
              app_name TEXT NOT NULL,
              updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS file_tags (
              path TEXT PRIMARY KEY,
              color TEXT NOT NULL,
              updated_at INTEGER NOT NULL
            );
            "#,
        )
        .map_err(|error| store_sql_error("Unable to migrate app store", path, error))?;

    ensure_favorites_group_column(connection, path)?;
    connection
        .execute(
            "CREATE INDEX IF NOT EXISTS idx_favorites_group_sort_order ON favorites(group_id, sort_order)",
            [],
        )
        .map_err(|error| store_sql_error("Unable to index favorite groups", path, error))?;
    seed_default_favorite_group(connection, path)
}

fn ensure_favorites_group_column(connection: &Connection, path: &Path) -> FsResult<()> {
    if table_has_column(connection, path, "favorites", "group_id")? {
        return Ok(());
    }

    connection
        .execute(
            "ALTER TABLE favorites ADD COLUMN group_id TEXT NOT NULL DEFAULT 'favorites'",
            [],
        )
        .map_err(|error| store_sql_error("Unable to migrate favorite groups", path, error))?;

    Ok(())
}

fn table_has_column(
    connection: &Connection,
    path: &Path,
    table_name: &str,
    column_name: &str,
) -> FsResult<bool> {
    let sql = format!("PRAGMA table_info({table_name})");
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| store_sql_error("Unable to inspect app store", path, error))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| store_sql_error("Unable to inspect app store", path, error))?;

    for row in rows {
        if row.map_err(|error| store_sql_error("Unable to inspect app store", path, error))?
            == column_name
        {
            return Ok(true);
        }
    }

    Ok(false)
}

fn seed_default_favorite_group(connection: &Connection, path: &Path) -> FsResult<()> {
    let now = unix_timestamp();

    connection
        .execute(
            "INSERT OR IGNORE INTO favorite_groups
             (id, name, sort_order, created_at, updated_at)
             VALUES (?1, ?2, 0, ?3, ?3)",
            params![DEFAULT_FAVORITE_GROUP_ID, DEFAULT_FAVORITE_GROUP_NAME, now],
        )
        .map_err(|error| store_sql_error("Unable to seed favorite groups", path, error))?;

    Ok(())
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
                 (id, group_id, name, path, icon, color, sort_order, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    id,
                    DEFAULT_FAVORITE_GROUP_ID,
                    name,
                    favorite_path,
                    icon,
                    color,
                    sort_order,
                    now,
                    now
                ],
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

    reorder_favorites_in_group(connection, path, DEFAULT_FAVORITE_GROUP_ID)
}

fn list_favorites(connection: &Connection, path: &Path) -> FsResult<Vec<FavoriteEntry>> {
    let mut statement = connection
        .prepare(
            "SELECT id, group_id, name, path, icon, color, sort_order, created_at, updated_at
             FROM favorites
             ORDER BY group_id ASC, sort_order ASC, name COLLATE NOCASE ASC",
        )
        .map_err(|error| store_sql_error("Unable to read favorites", path, error))?;

    let rows = statement
        .query_map([], favorite_from_row)
        .map_err(|error| store_sql_error("Unable to read favorites", path, error))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| store_sql_error("Unable to read favorites", path, error))
}

fn list_favorites_in_group(
    connection: &Connection,
    path: &Path,
    group_id: &str,
) -> FsResult<Vec<FavoriteEntry>> {
    let mut statement = connection
        .prepare(
            "SELECT id, group_id, name, path, icon, color, sort_order, created_at, updated_at
             FROM favorites
             WHERE group_id = ?1
             ORDER BY sort_order ASC, name COLLATE NOCASE ASC",
        )
        .map_err(|error| store_sql_error("Unable to read favorites", path, error))?;

    let rows = statement
        .query_map(params![group_id], favorite_from_row)
        .map_err(|error| store_sql_error("Unable to read favorites", path, error))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| store_sql_error("Unable to read favorites", path, error))
}

fn list_favorite_groups(connection: &Connection, path: &Path) -> FsResult<Vec<FavoriteGroupEntry>> {
    let mut statement = connection
        .prepare(
            "SELECT id, name, sort_order, created_at, updated_at
             FROM favorite_groups
             ORDER BY sort_order ASC, name COLLATE NOCASE ASC",
        )
        .map_err(|error| store_sql_error("Unable to read favorite groups", path, error))?;

    let rows = statement
        .query_map([], favorite_group_from_row)
        .map_err(|error| store_sql_error("Unable to read favorite groups", path, error))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| store_sql_error("Unable to read favorite groups", path, error))
}

fn favorite_by_path(
    connection: &Connection,
    path: &Path,
    favorite_path: &str,
) -> FsResult<Option<FavoriteEntry>> {
    connection
        .query_row(
            "SELECT id, group_id, name, path, icon, color, sort_order, created_at, updated_at
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
            "SELECT id, group_id, name, path, icon, color, sort_order, created_at, updated_at
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
        group_id: row.get(1)?,
        name: row.get(2)?,
        path: row.get(3)?,
        icon: row.get(4)?,
        color: row.get(5)?,
        sort_order: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn favorite_group_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FavoriteGroupEntry> {
    Ok(FavoriteGroupEntry {
        id: row.get(0)?,
        name: row.get(1)?,
        sort_order: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

fn open_with_default_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OpenWithDefaultEntry> {
    Ok(OpenWithDefaultEntry {
        file_type_key: row.get(0)?,
        app_id: row.get(1)?,
        app_name: row.get(2)?,
        updated_at: row.get(3)?,
    })
}

fn favorite_group_by_id(
    connection: &Connection,
    path: &Path,
    id: &str,
) -> FsResult<Option<FavoriteGroupEntry>> {
    connection
        .query_row(
            "SELECT id, name, sort_order, created_at, updated_at
             FROM favorite_groups
             WHERE id = ?1",
            params![id],
            favorite_group_from_row,
        )
        .optional()
        .map_err(|error| store_sql_error("Unable to read favorite group", path, error))
}

fn favorite_group_by_name(
    connection: &Connection,
    path: &Path,
    name: &str,
) -> FsResult<Option<FavoriteGroupEntry>> {
    connection
        .query_row(
            "SELECT id, name, sort_order, created_at, updated_at
             FROM favorite_groups
             WHERE name = ?1 COLLATE NOCASE",
            params![name],
            favorite_group_from_row,
        )
        .optional()
        .map_err(|error| store_sql_error("Unable to read favorite group", path, error))
}

fn favorite_group_id_if_exists(
    connection: &Connection,
    path: &Path,
    id: &str,
) -> FsResult<Option<String>> {
    let id = id.trim();

    if id.is_empty() {
        return Ok(None);
    }

    Ok(favorite_group_by_id(connection, path, id)?.map(|group| group.id))
}

fn next_favorite_sort_order(connection: &Connection, path: &Path, group_id: &str) -> FsResult<i64> {
    connection
        .query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM favorites WHERE group_id = ?1",
            params![group_id],
            |row| row.get(0),
        )
        .map_err(|error| store_sql_error("Unable to read favorite order", path, error))
}

fn next_favorite_group_sort_order(connection: &Connection, path: &Path) -> FsResult<i64> {
    connection
        .query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM favorite_groups",
            [],
            |row| row.get(0),
        )
        .map_err(|error| store_sql_error("Unable to read favorite group order", path, error))
}

fn reorder_favorites_in_group(
    connection: &Connection,
    path: &Path,
    group_id: &str,
) -> FsResult<()> {
    let favorites = list_favorites_in_group(connection, path, group_id)?;
    let now = unix_timestamp();
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| store_sql_error("Unable to normalize favorite order", path, error))?;

    write_favorite_order(&transaction, path, group_id, &favorites, now)?;

    transaction
        .commit()
        .map_err(|error| store_sql_error("Unable to normalize favorite order", path, error))?;

    Ok(())
}

fn write_favorite_order(
    transaction: &rusqlite::Transaction<'_>,
    path: &Path,
    group_id: &str,
    favorites: &[FavoriteEntry],
    now: i64,
) -> FsResult<()> {
    for (index, favorite) in favorites.iter().enumerate() {
        transaction
            .execute(
                "UPDATE favorites
                 SET group_id = ?1, sort_order = ?2, updated_at = ?3
                 WHERE id = ?4",
                params![group_id, index as i64, now, favorite.id],
            )
            .map_err(|error| store_sql_error("Unable to reorder favorites", path, error))?;
    }

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

fn normalize_favorite_group_name(name: &str) -> FsResult<String> {
    let value = name.trim();

    if value.is_empty() {
        return Err(FsError::new(
            "invalid_favorite_group",
            "Sidebar group names cannot be empty.",
            None,
        ));
    }

    Ok(value.to_string())
}

fn favorite_id_for_path(path: &str) -> String {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    format!("favorite-{:x}", hasher.finish())
}

fn favorite_group_id_for_name(name: &str, created_at: i64) -> String {
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    created_at.hash(&mut hasher);
    format!("favorite-group-{:x}", hasher.finish())
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
            .next_back()
            .unwrap_or(rest)
            .to_string();
    }

    clean_path
        .split('/')
        .filter(|part| !part.is_empty() && *part != "~")
        .next_back()
        .unwrap_or(clean_path)
        .to_string()
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn load_file_tags(connection: &Connection, path: &Path) -> FsResult<HashMap<String, String>> {
    let mut statement = connection
        .prepare("SELECT path, color FROM file_tags")
        .map_err(|error| store_sql_error("Unable to read file tags", path, error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| store_sql_error("Unable to read file tags", path, error))?;

    let mut map = HashMap::new();

    for row in rows {
        let (entry_path, color) =
            row.map_err(|error| store_sql_error("Unable to read file tags", path, error))?;
        map.insert(entry_path, color);
    }

    Ok(map)
}

pub fn window_dimensions(width: f64, height: f64) -> WindowDimensions {
    WindowDimensions {
        width,
        height,
        saved_at: unix_timestamp(),
    }
}

fn normalize_window_dimensions(dimensions: WindowDimensions) -> Option<WindowDimensions> {
    if !dimensions.width.is_finite() || !dimensions.height.is_finite() {
        return None;
    }

    Some(WindowDimensions {
        width: dimensions
            .width
            .round()
            .clamp(MIN_WINDOW_WIDTH, MAX_WINDOW_DIMENSION),
        height: dimensions
            .height
            .round()
            .clamp(MIN_WINDOW_HEIGHT, MAX_WINDOW_DIMENSION),
        saved_at: if dimensions.saved_at > 0 {
            dimensions.saved_at
        } else {
            unix_timestamp()
        },
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct TestStorePath {
        directory: PathBuf,
        store: PathBuf,
    }

    impl TestStorePath {
        fn new(name: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should be after UNIX epoch")
                .as_nanos();
            let directory =
                std::env::temp_dir().join(format!("carelo-store-test-{}-{unique}", name));
            let store = directory.join("carelo.store");
            Self { directory, store }
        }
    }

    impl Drop for TestStorePath {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn persists_remote_volume_configs() {
        let path = TestStorePath::new("remote-volumes");
        let config = RemoteVolumeConfig {
            id: "remote_one".to_string(),
            name: "Remote One".to_string(),
            scheme: "fs".to_string(),
            root: Some("/tmp/remote-one".to_string()),
            options: HashMap::from([("custom".to_string(), "value".to_string())]),
        };

        {
            let store =
                AppStoreState::initialize_at(path.store.clone()).expect("store should initialize");
            store
                .save_remote_volume_config(config.clone())
                .expect("remote config should save");
            let configs = store
                .list_remote_volume_configs()
                .expect("remote configs should list");
            assert_eq!(configs.len(), 1);
            assert_eq!(configs[0].id, config.id);
            assert_eq!(configs[0].name, config.name);
            assert_eq!(configs[0].scheme, config.scheme);
            assert_eq!(configs[0].root, config.root);
            assert_eq!(configs[0].options, config.options);
        }

        let store = AppStoreState::initialize_at(path.store.clone()).expect("store should reopen");
        assert_eq!(
            store
                .list_remote_volume_configs()
                .expect("remote configs should reload")
                .len(),
            1
        );
        assert!(store
            .remove_remote_volume_config(&config.id)
            .expect("remote config should remove"));
        assert!(store
            .list_remote_volume_configs()
            .expect("remote configs should list after removal")
            .is_empty());
    }
}
