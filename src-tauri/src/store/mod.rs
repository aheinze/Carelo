mod credentials;

use crate::fs::models::{FileEntry, FsError, FsResult};
use crate::fs::remote::RemoteVolumeConfig;
use credentials::{OsRemoteCredentialStore, RemoteCredentialStore};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const STORE_DIRECTORY: &str = ".local/share/carelo";
const STORE_FILE: &str = "carelo.store";
const FAVORITES_SEEDED_KEY: &str = "favorites.seeded.v1";
const DEFAULT_FAVORITE_GROUP_ID: &str = "favorites";
const DEFAULT_FAVORITE_GROUP_NAME: &str = "Favorites";
const WINDOW_DIMENSIONS_KEY: &str = "window.dimensions.v1";
const RECENT_LOCATIONS_LIMIT: i64 = 50;
const APP_SETTINGS_KEY: &str = "app.settings.v1";
const REMOTE_VOLUMES_KEY: &str = "remote.volumes.v1";
const OPERATION_JOURNAL_FINISHED_LIMIT: i64 = 200;
const OPERATION_JOURNAL_JSON_LIMIT_BYTES: usize = 256 * 1024;
const OPERATION_JOURNAL_ID_LIMIT: usize = 160;
const OPERATION_JOURNAL_OPERATION_LIMIT: usize = 96;
const OPERATION_JOURNAL_LABEL_LIMIT: usize = 240;
const OPERATION_JOURNAL_DETAIL_LIMIT: usize = 4096;
const MIN_WINDOW_WIDTH: f64 = 960.0;
const MIN_WINDOW_HEIGHT: f64 = 640.0;
const MAX_WINDOW_DIMENSION: f64 = 10000.0;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedRemoteVolumeConfig {
    id: String,
    name: String,
    scheme: String,
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    options: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    credential_refs: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OperationJournalEntry {
    pub id: String,
    pub operation: String,
    pub label: String,
    pub detail: String,
    pub status: String,
    pub payload: Value,
    pub progress: Value,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationJournalInput {
    pub id: String,
    pub operation: String,
    pub label: String,
    #[serde(default)]
    pub detail: String,
    pub status: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub progress: Value,
    #[serde(default)]
    pub created_at: Option<i64>,
    #[serde(default)]
    pub updated_at: Option<i64>,
    #[serde(default)]
    pub finished_at: Option<i64>,
}

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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentLocationEntry {
    pub path: String,
    pub name: String,
    pub visited_at: i64,
}

pub struct AppStoreState {
    connection: Mutex<Connection>,
    path: PathBuf,
    remote_credentials: Arc<dyn RemoteCredentialStore>,
    // In-memory cache of path -> color so directory listings don't hit the DB.
    tags: Mutex<HashMap<String, String>>,
}

impl AppStoreState {
    pub fn initialize() -> FsResult<Self> {
        Self::initialize_at(store_path()?)
    }

    fn initialize_at(path: PathBuf) -> FsResult<Self> {
        Self::initialize_at_with_credentials(path, Arc::new(OsRemoteCredentialStore))
    }

    fn initialize_at_with_credentials(
        path: PathBuf,
        remote_credentials: Arc<dyn RemoteCredentialStore>,
    ) -> FsResult<Self> {
        prepare_store_path(&path)?;

        let connection = Connection::open(&path)
            .map_err(|error| store_sql_error("Unable to open app store", &path, error))?;
        secure_store_file_permissions(&path)?;
        connection
            .execute_batch("PRAGMA secure_delete = ON;")
            .map_err(|error| {
                store_sql_error("Unable to enable secure app store deletion", &path, error)
            })?;

        migrate(&connection, &path)?;
        reconcile_operation_journal(&connection, &path)?;
        seed_default_favorites(&connection, &path)?;
        let tags = load_file_tags(&connection, &path)?;

        let state = Self {
            connection: Mutex::new(connection),
            path,
            remote_credentials,
            tags: Mutex::new(tags),
        };
        state.cleanup_pending_remote_credentials_best_effort();
        Ok(state)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Snapshot of every path -> color tag (for the frontend tag map).
    pub fn all_file_tags(&self) -> HashMap<String, String> {
        self.tags
            .lock()
            .map(|tags| tags.clone())
            .unwrap_or_default()
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
                        .map_err(|error| {
                            store_sql_error("Unable to clear file tag", &self.path, error)
                        })?;
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

    pub fn list_recent_locations(&self) -> FsResult<Vec<RecentLocationEntry>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT path, name, visited_at FROM recent_locations ORDER BY visited_at DESC LIMIT ?1",
            )
            .map_err(|error| store_sql_error("Unable to read recent locations", &self.path, error))?;
        let rows = statement
            .query_map(params![RECENT_LOCATIONS_LIMIT], |row| {
                Ok(RecentLocationEntry {
                    path: row.get(0)?,
                    name: row.get(1)?,
                    visited_at: row.get(2)?,
                })
            })
            .map_err(|error| {
                store_sql_error("Unable to read recent locations", &self.path, error)
            })?;

        let mut entries = Vec::new();

        for row in rows {
            entries.push(row.map_err(|error| {
                store_sql_error("Unable to read recent locations", &self.path, error)
            })?);
        }

        Ok(entries)
    }

    pub fn record_recent_location(&self, path: String, name: String) -> FsResult<()> {
        let path = path.trim().to_string();

        if path.is_empty() {
            return Ok(());
        }

        let connection = self.connection()?;
        connection
            .execute(
                "INSERT INTO recent_locations(path, name, visited_at) VALUES(?1, ?2, ?3) ON CONFLICT(path) DO UPDATE SET name = excluded.name, visited_at = excluded.visited_at",
                params![path, name, unix_timestamp_millis()],
            )
            .map_err(|error| store_sql_error("Unable to record recent location", &self.path, error))?;
        // Keep only the most-recent entries so the table never grows unbounded.
        connection
            .execute(
                "DELETE FROM recent_locations WHERE path NOT IN (SELECT path FROM recent_locations ORDER BY visited_at DESC LIMIT ?1)",
                params![RECENT_LOCATIONS_LIMIT],
            )
            .map_err(|error| store_sql_error("Unable to trim recent locations", &self.path, error))?;

        Ok(())
    }

    pub fn remove_recent_location(&self, path: String) -> FsResult<()> {
        let connection = self.connection()?;
        connection
            .execute(
                "DELETE FROM recent_locations WHERE path = ?1",
                params![path],
            )
            .map_err(|error| {
                store_sql_error("Unable to remove recent location", &self.path, error)
            })?;

        Ok(())
    }

    pub fn clear_recent_locations(&self) -> FsResult<()> {
        let connection = self.connection()?;
        connection
            .execute("DELETE FROM recent_locations", [])
            .map_err(|error| {
                store_sql_error("Unable to clear recent locations", &self.path, error)
            })?;

        Ok(())
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

    pub fn list_operation_journal_entries(&self) -> FsResult<Vec<OperationJournalEntry>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, operation, label, detail, status, payload_json, progress_json,
                        created_at, updated_at, finished_at
                 FROM operation_journal
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(|error| {
                store_sql_error("Unable to read the operation journal", &self.path, error)
            })?;
        let rows = statement
            .query_map([], operation_journal_entry_from_row)
            .map_err(|error| {
                store_sql_error("Unable to read the operation journal", &self.path, error)
            })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(|error| {
            store_sql_error("Unable to read the operation journal", &self.path, error)
        })
    }

    pub fn upsert_operation_journal_entry(
        &self,
        input: OperationJournalInput,
    ) -> FsResult<OperationJournalEntry> {
        let input = validate_operation_journal_input(input)?;
        let connection = self.connection()?;
        let existing_entry: Option<OperationJournalEntry> = connection
            .query_row(
                "SELECT id, operation, label, detail, status, payload_json, progress_json,
                        created_at, updated_at, finished_at
                 FROM operation_journal WHERE id = ?1",
                params![input.id],
                operation_journal_entry_from_row,
            )
            .optional()
            .map_err(|error| {
                store_sql_error("Unable to read the operation journal", &self.path, error)
            })?;
        let now = unix_timestamp_millis();
        let created_at = existing_entry
            .as_ref()
            .map(|entry| entry.created_at)
            .or(input.created_at)
            .unwrap_or(now);
        let mut updated_at = input.updated_at.unwrap_or(now);

        if updated_at < created_at {
            return Err(invalid_operation_journal(
                "Operation journal updatedAt cannot be earlier than createdAt.",
            ));
        }

        let finished_at = if operation_journal_status_is_finished(&input.status) {
            Some(input.finished_at.unwrap_or(updated_at))
        } else {
            None
        };

        if let Some(finished_at) = finished_at {
            if finished_at < created_at {
                return Err(invalid_operation_journal(
                    "Operation journal finishedAt cannot be earlier than createdAt.",
                ));
            }
            updated_at = updated_at.max(finished_at);
        }

        let entry = OperationJournalEntry {
            id: input.id,
            operation: input.operation,
            label: input.label,
            detail: input.detail,
            status: input.status,
            payload: input.payload,
            progress: input.progress,
            created_at,
            updated_at,
            finished_at,
        };

        // Finished records are immutable, and out-of-order progress writes must
        // not move an operation backwards. This is also enforced in the UPSERT
        // below so a second store connection cannot win between this read and
        // the write.
        if let Some(existing) = existing_entry {
            if existing.finished_at.is_some() || entry.updated_at < existing.updated_at {
                return Ok(existing);
            }
        }

        let payload_json = serialize_operation_journal_json(&entry.payload, "payload")?;
        let progress_json = serialize_operation_journal_json(&entry.progress, "progress")?;
        let transaction = connection.unchecked_transaction().map_err(|error| {
            store_sql_error("Unable to update the operation journal", &self.path, error)
        })?;

        transaction
            .execute(
                "INSERT INTO operation_journal
                 (id, operation, label, detail, status, payload_json, progress_json,
                  created_at, updated_at, finished_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(id) DO UPDATE SET
                   operation = excluded.operation,
                   label = excluded.label,
                   detail = excluded.detail,
                   status = excluded.status,
                   payload_json = excluded.payload_json,
                   progress_json = excluded.progress_json,
                   updated_at = excluded.updated_at,
                   finished_at = excluded.finished_at
                 WHERE operation_journal.finished_at IS NULL
                   AND excluded.updated_at >= operation_journal.updated_at",
                params![
                    entry.id,
                    entry.operation,
                    entry.label,
                    entry.detail,
                    entry.status,
                    payload_json,
                    progress_json,
                    entry.created_at,
                    entry.updated_at,
                    entry.finished_at,
                ],
            )
            .map_err(|error| {
                store_sql_error("Unable to update the operation journal", &self.path, error)
            })?;
        let stored_entry = transaction
            .query_row(
                "SELECT id, operation, label, detail, status, payload_json, progress_json,
                        created_at, updated_at, finished_at
                 FROM operation_journal WHERE id = ?1",
                params![entry.id],
                operation_journal_entry_from_row,
            )
            .map_err(|error| {
                store_sql_error(
                    "Unable to read the updated operation journal",
                    &self.path,
                    error,
                )
            })?;
        prune_finished_operation_journal_entries(&transaction, &self.path)?;
        transaction.commit().map_err(|error| {
            store_sql_error("Unable to update the operation journal", &self.path, error)
        })?;

        Ok(stored_entry)
    }

    pub fn remove_operation_journal_entry(&self, id: &str) -> FsResult<bool> {
        let id = validate_operation_journal_identifier(id, "id", OPERATION_JOURNAL_ID_LIMIT)?;
        let connection = self.connection()?;
        let removed = connection
            .execute("DELETE FROM operation_journal WHERE id = ?1", params![id])
            .map_err(|error| {
                store_sql_error(
                    "Unable to remove an operation journal entry",
                    &self.path,
                    error,
                )
            })?;
        Ok(removed > 0)
    }

    pub fn clear_finished_operation_journal_entries(&self) -> FsResult<u64> {
        let connection = self.connection()?;
        let removed = connection
            .execute(
                "DELETE FROM operation_journal WHERE finished_at IS NOT NULL",
                [],
            )
            .map_err(|error| {
                store_sql_error(
                    "Unable to clear finished operation journal entries",
                    &self.path,
                    error,
                )
            })?;
        Ok(removed as u64)
    }

    pub fn list_remote_volume_configs(&self) -> FsResult<Vec<RemoteVolumeConfig>> {
        let original_configs = self.read_persisted_remote_volume_configs()?;
        let mut persisted_configs = original_configs.clone();

        match self.migrate_legacy_remote_credentials(&mut persisted_configs) {
            Ok(true) => {
                if self
                    .write_remote_volume_configs(&persisted_configs)
                    .is_err()
                {
                    // Loading must remain available when a migration cannot be
                    // committed. The legacy row is already protected by the
                    // store's private file permissions and will be retried next
                    // launch.
                    persisted_configs = original_configs;
                } else {
                    self.vacuum_store_best_effort();
                }
            }
            Ok(false) => {}
            Err(_) => {
                // A locked or unavailable desktop keyring must not prevent
                // Carelo from starting. Keep legacy credentials in memory for
                // this session and leave SQLite untouched for a later retry.
                persisted_configs = original_configs;
            }
        }

        Ok(persisted_configs
            .into_iter()
            .map(|config| self.hydrate_remote_volume_config(config))
            .collect())
    }

    pub fn save_remote_volume_config(&self, config: RemoteVolumeConfig) -> FsResult<()> {
        let mut configs = self.read_persisted_remote_volume_configs()?;
        let migrated_legacy = self.migrate_legacy_remote_credentials(&mut configs)?;

        let existing_refs = configs
            .iter()
            .find(|existing| existing.id == config.id)
            .map(|existing| existing.credential_refs.clone())
            .unwrap_or_default();
        let (persisted, new_refs) = self.secure_remote_volume_config(config, &existing_refs)?;
        let retained_refs = persisted
            .credential_refs
            .values()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        let stale_refs = existing_refs
            .values()
            .filter(|reference| !retained_refs.contains(*reference))
            .cloned()
            .collect::<std::collections::HashSet<_>>();

        if let Some(existing) = configs
            .iter_mut()
            .find(|existing| existing.id == persisted.id)
        {
            *existing = persisted;
        } else {
            configs.push(persisted);
        }

        configs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        if let Err(error) = self.write_remote_volume_configs_with_cleanup(&configs, &stale_refs) {
            for reference in new_refs {
                let _ = self.remote_credentials.delete(&reference);
            }
            return Err(error);
        }
        if migrated_legacy {
            self.vacuum_store_best_effort();
        }

        self.cleanup_pending_remote_credentials_best_effort();

        Ok(())
    }

    pub fn remove_remote_volume_config(&self, id: &str) -> FsResult<bool> {
        let mut configs = self.read_persisted_remote_volume_configs()?;
        let mut removed_refs = configs
            .iter()
            .find(|config| config.id == id)
            .map(|config| {
                config
                    .credential_refs
                    .values()
                    .cloned()
                    .collect::<std::collections::HashSet<_>>()
            })
            .unwrap_or_default();
        let original_len = configs.len();
        configs.retain(|config| config.id != id);
        let removed = configs.len() != original_len;

        if !removed {
            return Ok(false);
        }

        let retained_refs = configs
            .iter()
            .flat_map(|config| config.credential_refs.values().cloned())
            .collect::<std::collections::HashSet<_>>();
        removed_refs.retain(|reference| !retained_refs.contains(reference));
        self.write_remote_volume_configs_with_cleanup(&configs, &removed_refs)?;
        self.cleanup_pending_remote_credentials_best_effort();

        Ok(true)
    }

    fn read_persisted_remote_volume_configs(&self) -> FsResult<Vec<PersistedRemoteVolumeConfig>> {
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

    fn migrate_legacy_remote_credentials(
        &self,
        configs: &mut [PersistedRemoteVolumeConfig],
    ) -> FsResult<bool> {
        let mut credentials = Vec::new();
        let mut reserved_refs = configs
            .iter()
            .flat_map(|config| config.credential_refs.values().cloned())
            .collect::<std::collections::HashSet<_>>();

        for (config_index, config) in configs.iter().enumerate() {
            for (key, value) in &config.options {
                if !remote_option_contains_secret(key, value) {
                    continue;
                }

                if value.is_empty() {
                    credentials.push((config_index, key.clone(), None, None));
                    continue;
                }

                let legacy_reference = remote_credential_reference(&config.id, key);
                let reference = if reserved_refs.contains(&legacy_reference) {
                    new_remote_credential_reference(&reserved_refs)
                } else {
                    legacy_reference
                };
                reserved_refs.insert(reference.clone());
                credentials.push((
                    config_index,
                    key.clone(),
                    Some(reference),
                    Some(value.clone()),
                ));
            }
        }

        if credentials.is_empty() {
            return Ok(false);
        }

        // Do not modify the persisted representation until every credential
        // has been accepted by the secure store. A partial failure therefore
        // leaves the complete legacy JSON available for a later retry.
        for (_, _, reference, secret) in &credentials {
            if let (Some(reference), Some(secret)) = (reference, secret) {
                self.remote_credentials.store(reference, secret)?;
            }
        }

        for (config_index, key, reference, _) in credentials {
            let config = &mut configs[config_index];
            config.options.remove(&key);

            if let Some(reference) = reference {
                config.credential_refs.insert(key, reference);
            } else {
                config.credential_refs.remove(&key);
            }
        }

        Ok(true)
    }

    fn secure_remote_volume_config(
        &self,
        config: RemoteVolumeConfig,
        existing_refs: &HashMap<String, String>,
    ) -> FsResult<(PersistedRemoteVolumeConfig, Vec<String>)> {
        let mut options = HashMap::new();
        // A hydrated config can legitimately omit a secret while the desktop
        // keyring is locked. Preserve committed references unless the caller
        // explicitly replaces that option or supplies an empty sensitive value
        // to clear it.
        let mut credential_refs = existing_refs.clone();
        let mut credentials = Vec::new();
        let mut reserved_refs = existing_refs
            .values()
            .cloned()
            .collect::<std::collections::HashSet<_>>();

        for (key, value) in config.options {
            if remote_option_contains_secret(&key, &value) {
                if value.is_empty() {
                    credential_refs.remove(&key);
                    continue;
                }

                // Never overwrite a credential referenced by committed JSON.
                // A fresh reference makes the keyring write + SQLite commit a
                // two-phase update: the old reconnect path stays intact until
                // the JSON atomically switches to the new reference.
                let reference = new_remote_credential_reference(&reserved_refs);
                reserved_refs.insert(reference.clone());
                credentials.push((reference.clone(), value));
                credential_refs.insert(key, reference);
            } else {
                credential_refs.remove(&key);
                options.insert(key, value);
            }
        }

        let mut stored_refs: Vec<String> = Vec::new();

        for (reference, secret) in credentials {
            if let Err(error) = self.remote_credentials.store(&reference, &secret) {
                for stored_reference in stored_refs {
                    let _ = self.remote_credentials.delete(&stored_reference);
                }
                return Err(error);
            }
            stored_refs.push(reference);
        }

        Ok((
            PersistedRemoteVolumeConfig {
                id: config.id,
                name: config.name,
                scheme: config.scheme,
                root: config.root,
                options,
                credential_refs,
            },
            stored_refs,
        ))
    }

    fn hydrate_remote_volume_config(
        &self,
        mut config: PersistedRemoteVolumeConfig,
    ) -> RemoteVolumeConfig {
        for (key, reference) in config.credential_refs {
            // Missing or temporarily unavailable credentials leave the safe
            // config visible. The affected volume will report its normal
            // connection error when used instead of aborting app startup.
            if let Ok(Some(secret)) = self.remote_credentials.load(&reference) {
                config.options.insert(key, secret);
            }
        }

        RemoteVolumeConfig {
            id: config.id,
            name: config.name,
            scheme: config.scheme,
            root: config.root,
            options: config.options,
        }
    }

    fn vacuum_store_best_effort(&self) {
        if let Ok(connection) = self.connection() {
            let _ = connection.execute_batch("VACUUM;");
        }
        let _ = secure_store_file_permissions(&self.path);
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

    fn write_remote_volume_configs(&self, configs: &[PersistedRemoteVolumeConfig]) -> FsResult<()> {
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

    fn write_remote_volume_configs_with_cleanup(
        &self,
        configs: &[PersistedRemoteVolumeConfig],
        credential_refs: &std::collections::HashSet<String>,
    ) -> FsResult<()> {
        let value = serde_json::to_string(configs).map_err(|error| {
            FsError::new(
                "store_serialize_error",
                format!("Unable to serialize remote volumes: {error}"),
                Some(self.path.to_string_lossy().into_owned()),
            )
        })?;
        let connection = self.connection()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| store_sql_error("Unable to save remote volumes", &self.path, error))?;

        transaction
            .execute(
                "INSERT OR REPLACE INTO store_metadata (key, value) VALUES (?1, ?2)",
                params![REMOTE_VOLUMES_KEY, value],
            )
            .map_err(|error| store_sql_error("Unable to save remote volumes", &self.path, error))?;

        for reference in credential_refs {
            transaction
                .execute(
                    "INSERT OR IGNORE INTO remote_credential_cleanup (reference, created_at)
                     VALUES (?1, ?2)",
                    params![reference, unix_timestamp_millis()],
                )
                .map_err(|error| {
                    store_sql_error(
                        "Unable to schedule remote credential cleanup",
                        &self.path,
                        error,
                    )
                })?;
        }

        transaction
            .commit()
            .map_err(|error| store_sql_error("Unable to save remote volumes", &self.path, error))
    }

    fn cleanup_pending_remote_credentials_best_effort(&self) {
        let references = (|| -> FsResult<Vec<String>> {
            let connection = self.connection()?;
            let mut statement = connection
                .prepare(
                    "SELECT reference FROM remote_credential_cleanup ORDER BY created_at, reference",
                )
                .map_err(|error| {
                    store_sql_error(
                        "Unable to read pending remote credential cleanup",
                        &self.path,
                        error,
                    )
                })?;
            let rows = statement
                .query_map([], |row| row.get(0))
                .map_err(|error| {
                    store_sql_error(
                        "Unable to read pending remote credential cleanup",
                        &self.path,
                        error,
                    )
                })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(|error| {
                store_sql_error(
                    "Unable to read pending remote credential cleanup",
                    &self.path,
                    error,
                )
            })
        })()
        .unwrap_or_default();

        for reference in references {
            if self.remote_credentials.delete(&reference).is_err() {
                continue;
            }

            if let Ok(connection) = self.connection() {
                let _ = connection.execute(
                    "DELETE FROM remote_credential_cleanup WHERE reference = ?1",
                    params![reference],
                );
            }
        }
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

fn validate_operation_journal_input(
    mut input: OperationJournalInput,
) -> FsResult<OperationJournalInput> {
    input.id = validate_operation_journal_identifier(&input.id, "id", OPERATION_JOURNAL_ID_LIMIT)?;
    input.operation = validate_operation_journal_identifier(
        &input.operation,
        "operation",
        OPERATION_JOURNAL_OPERATION_LIMIT,
    )?;
    input.label = validate_operation_journal_text(
        &input.label,
        "label",
        OPERATION_JOURNAL_LABEL_LIMIT,
        false,
    )?;
    input.detail = validate_operation_journal_text(
        &input.detail,
        "detail",
        OPERATION_JOURNAL_DETAIL_LIMIT,
        true,
    )?;

    if operation_journal_string_is_sensitive(&input.label)
        || operation_journal_string_is_sensitive(&input.detail)
    {
        return Err(invalid_operation_journal(
            "Operation journal text must not contain credentials.",
        ));
    }

    input.status = input.status.trim().to_ascii_lowercase();

    if !matches!(
        input.status.as_str(),
        "queued"
            | "pending"
            | "running"
            | "pausing"
            | "paused"
            | "cancelling"
            | "completed"
            | "failed"
            | "cancelled"
            | "interrupted"
    ) {
        return Err(invalid_operation_journal(
            "Operation journal status is not supported.",
        ));
    }

    for (name, timestamp) in [
        ("createdAt", input.created_at),
        ("updatedAt", input.updated_at),
        ("finishedAt", input.finished_at),
    ] {
        if timestamp.is_some_and(|value| value < 0) {
            return Err(invalid_operation_journal(format!(
                "Operation journal {name} cannot be negative."
            )));
        }
    }

    validate_operation_journal_json(&input.payload, "payload")?;
    validate_operation_journal_json(&input.progress, "progress")?;
    Ok(input)
}

fn validate_operation_journal_identifier(
    value: &str,
    name: &str,
    max_bytes: usize,
) -> FsResult<String> {
    let value = value.trim();

    if value.is_empty() || value.len() > max_bytes {
        return Err(invalid_operation_journal(format!(
            "Operation journal {name} must be between 1 and {max_bytes} bytes."
        )));
    }

    if !value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
    }) {
        return Err(invalid_operation_journal(format!(
            "Operation journal {name} contains unsupported characters."
        )));
    }

    Ok(value.to_string())
}

fn validate_operation_journal_text(
    value: &str,
    name: &str,
    max_bytes: usize,
    allow_empty: bool,
) -> FsResult<String> {
    let value = value.trim();

    if (!allow_empty && value.is_empty()) || value.len() > max_bytes {
        return Err(invalid_operation_journal(format!(
            "Operation journal {name} is empty or exceeds {max_bytes} bytes."
        )));
    }

    Ok(value.to_string())
}

fn validate_operation_journal_json(value: &Value, name: &str) -> FsResult<()> {
    if operation_journal_json_contains_credentials(value) {
        return Err(invalid_operation_journal(format!(
            "Operation journal {name} cannot contain credentials or secrets."
        )));
    }

    let _ = serialize_operation_journal_json(value, name)?;
    Ok(())
}

fn serialize_operation_journal_json(value: &Value, name: &str) -> FsResult<String> {
    let serialized = serde_json::to_string(value).map_err(|error| {
        FsError::new(
            "store_serialize_error",
            format!("Unable to serialize operation journal {name}: {error}"),
            None,
        )
    })?;

    if serialized.len() > OPERATION_JOURNAL_JSON_LIMIT_BYTES {
        return Err(invalid_operation_journal(format!(
            "Operation journal {name} exceeds {OPERATION_JOURNAL_JSON_LIMIT_BYTES} bytes."
        )));
    }

    Ok(serialized)
}

fn operation_journal_json_contains_credentials(value: &Value) -> bool {
    match value {
        Value::Object(values) => values.iter().any(|(key, value)| {
            operation_journal_key_is_sensitive(key)
                || operation_journal_json_contains_credentials(value)
        }),
        Value::Array(values) => values
            .iter()
            .any(operation_journal_json_contains_credentials),
        Value::String(value) => operation_journal_string_is_sensitive(value),
        _ => false,
    }
}

fn operation_journal_string_is_sensitive(value: &str) -> bool {
    let value = value.trim();
    let lowercase = value.to_ascii_lowercase();

    remote_option_url_contains_secret(value)
        || lowercase.starts_with("bearer ")
        || lowercase.starts_with("basic ")
        || (value.starts_with("-----BEGIN ") && value.contains("PRIVATE KEY-----"))
}

fn operation_journal_key_is_sensitive(key: &str) -> bool {
    let compact = key
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();

    remote_option_key_is_sensitive(key)
        || compact.contains("credential")
        || compact.contains("authorization")
        || matches!(
            compact.as_str(),
            "auth"
                | "authentication"
                | "bearer"
                | "cookie"
                | "cookies"
                | "header"
                | "headers"
                | "httpheaders"
                | "setcookie"
        )
}

fn operation_journal_status_is_finished(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled" | "interrupted")
}

fn operation_journal_entry_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<OperationJournalEntry> {
    let payload_json: String = row.get(5)?;
    let progress_json: String = row.get(6)?;
    let payload = serde_json::from_str(&payload_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let progress = serde_json::from_str(&progress_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(error))
    })?;

    Ok(OperationJournalEntry {
        id: row.get(0)?,
        operation: row.get(1)?,
        label: row.get(2)?,
        detail: row.get(3)?,
        status: row.get(4)?,
        payload,
        progress,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        finished_at: row.get(9)?,
    })
}

fn prune_finished_operation_journal_entries(
    transaction: &rusqlite::Transaction<'_>,
    path: &Path,
) -> FsResult<()> {
    transaction
        .execute(
            "DELETE FROM operation_journal
             WHERE id IN (
               SELECT id FROM operation_journal
               WHERE finished_at IS NOT NULL
               ORDER BY finished_at DESC, updated_at DESC, id DESC
               LIMIT -1 OFFSET ?1
             )",
            params![OPERATION_JOURNAL_FINISHED_LIMIT],
        )
        .map_err(|error| store_sql_error("Unable to prune the operation journal", path, error))?;
    Ok(())
}

fn invalid_operation_journal(message: impl Into<String>) -> FsError {
    FsError::new("invalid_operation_journal", message.into(), None)
}

fn remote_option_contains_secret(key: &str, value: &str) -> bool {
    remote_option_key_is_sensitive(key) || remote_option_url_contains_secret(value)
}

fn remote_option_key_is_sensitive(key: &str) -> bool {
    let normalized = key
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let segments = normalized
        .split('_')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let compact = normalized.replace('_', "");

    segments.iter().any(|segment| {
        matches!(
            *segment,
            "password" | "passwd" | "passphrase" | "secret" | "token"
        )
    }) || compact.contains("password")
        || compact.contains("passphrase")
        || compact.contains("token")
        || compact.contains("secret")
        || matches!(
            normalized.as_str(),
            "key"
                | "api_key"
                | "application_key"
                | "authorization"
                | "auth_header"
                | "server_side_encryption_customer_key"
                | "server_side_encryption_customer_key_md5"
                | "temp_url_key"
        )
        || matches!(
            compact.as_str(),
            "apikey" | "applicationkey" | "customerkey" | "privatekey" | "tempurlkey"
        )
        || compact.ends_with("customerkey")
        || normalized.contains("private_key")
        || (normalized.ends_with("_key") && !normalized.ends_with("public_key"))
        || normalized == "sig"
        || normalized.contains("signature")
}

fn remote_option_url_contains_secret(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value.trim()) else {
        return false;
    };

    if url.password().is_some_and(|password| !password.is_empty()) {
        return true;
    }

    url.query_pairs()
        .any(|(key, _)| remote_option_key_is_sensitive(&key))
}

fn remote_credential_reference(remote_id: &str, option_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(remote_id.as_bytes());
    hasher.update([0]);
    hasher.update(option_key.as_bytes());
    encode_remote_credential_reference(&hasher.finalize())
}

fn new_remote_credential_reference(existing_refs: &std::collections::HashSet<String>) -> String {
    loop {
        let reference = encode_remote_credential_reference(&rand::random::<[u8; 16]>());

        if !existing_refs.contains(&reference) {
            return reference;
        }
    }
}

fn encode_remote_credential_reference(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        let _ = write!(encoded, "{byte:02x}");
    }

    format!("remote.v1.{encoded}")
}

fn prepare_store_path(path: &Path) -> FsResult<()> {
    let Some(parent) = path.parent() else {
        return Err(FsError::new(
            "invalid_store_path",
            "The app store path has no parent directory.",
            Some(path.to_string_lossy().into_owned()),
        ));
    };

    fs::create_dir_all(parent)
        .map_err(|error| store_io_error("Unable to create app store directory", parent, error))?;
    secure_store_directory_permissions(parent)?;

    #[cfg(unix)]
    {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).mode(0o600);
        let file = options
            .open(path)
            .map_err(|error| store_io_error("Unable to create app store", path, error))?;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| {
                store_io_error("Unable to secure app store permissions", path, error)
            })?;
    }

    Ok(())
}

#[cfg(unix)]
fn secure_store_directory_permissions(path: &Path) -> FsResult<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
        store_io_error(
            "Unable to secure app store directory permissions",
            path,
            error,
        )
    })
}

#[cfg(not(unix))]
fn secure_store_directory_permissions(_path: &Path) -> FsResult<()> {
    Ok(())
}

#[cfg(unix)]
fn secure_store_file_permissions(path: &Path) -> FsResult<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| store_io_error("Unable to secure app store permissions", path, error))
}

#[cfg(not(unix))]
fn secure_store_file_permissions(_path: &Path) -> FsResult<()> {
    Ok(())
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

            CREATE TABLE IF NOT EXISTS recent_locations (
              path TEXT PRIMARY KEY,
              name TEXT NOT NULL,
              visited_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_recent_locations_visited_at ON recent_locations(visited_at);

            CREATE TABLE IF NOT EXISTS operation_journal (
              id TEXT PRIMARY KEY,
              operation TEXT NOT NULL,
              label TEXT NOT NULL,
              detail TEXT NOT NULL DEFAULT '',
              status TEXT NOT NULL CHECK (
                status IN (
                  'queued', 'pending', 'running', 'pausing', 'paused', 'cancelling',
                  'completed', 'failed', 'cancelled', 'interrupted'
                )
              ),
              payload_json TEXT NOT NULL DEFAULT 'null',
              progress_json TEXT NOT NULL DEFAULT 'null',
              created_at INTEGER NOT NULL,
              updated_at INTEGER NOT NULL,
              finished_at INTEGER
            );

            CREATE INDEX IF NOT EXISTS idx_operation_journal_status_updated
              ON operation_journal(status, updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_operation_journal_finished
              ON operation_journal(finished_at DESC, updated_at DESC);

            CREATE TABLE IF NOT EXISTS remote_credential_cleanup (
              reference TEXT PRIMARY KEY,
              created_at INTEGER NOT NULL
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

fn reconcile_operation_journal(connection: &Connection, path: &Path) -> FsResult<()> {
    let now = unix_timestamp_millis();
    let transaction = connection.unchecked_transaction().map_err(|error| {
        store_sql_error(
            "Unable to reconcile interrupted operation journal entries",
            path,
            error,
        )
    })?;
    transaction
        .execute(
            "UPDATE operation_journal
             SET status = 'interrupted',
                 updated_at = MAX(created_at, updated_at, ?1),
                 finished_at = MAX(created_at, updated_at, ?1)
             WHERE status IN ('running', 'pausing', 'paused', 'cancelling')",
            params![now],
        )
        .map_err(|error| {
            store_sql_error(
                "Unable to reconcile interrupted operation journal entries",
                path,
                error,
            )
        })?;
    prune_finished_operation_journal_entries(&transaction, path)?;
    transaction.commit().map_err(|error| {
        store_sql_error(
            "Unable to reconcile interrupted operation journal entries",
            path,
            error,
        )
    })?;
    Ok(())
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

// Epoch milliseconds, used where sub-second ordering matters (recent locations
// are recorded in rapid succession as the user navigates).
fn unix_timestamp_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
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

    #[derive(Default)]
    struct TestRemoteCredentialStore {
        values: Mutex<HashMap<String, String>>,
        store_fail_after: Mutex<Option<usize>>,
        fail_load: Mutex<bool>,
        fail_delete: Mutex<bool>,
    }

    impl TestRemoteCredentialStore {
        fn set_store_failure(&self, fail: bool) {
            *self.store_fail_after.lock().expect("lock store failure") = fail.then_some(0);
        }

        fn fail_store_after(&self, successful_writes: usize) {
            *self.store_fail_after.lock().expect("lock store failure") = Some(successful_writes);
        }

        fn set_load_failure(&self, fail: bool) {
            *self.fail_load.lock().expect("lock load failure") = fail;
        }

        fn set_delete_failure(&self, fail: bool) {
            *self.fail_delete.lock().expect("lock delete failure") = fail;
        }

        fn values(&self) -> HashMap<String, String> {
            self.values.lock().expect("lock credentials").clone()
        }

        fn clear(&self) {
            self.values.lock().expect("lock credentials").clear();
        }
    }

    impl RemoteCredentialStore for TestRemoteCredentialStore {
        fn store(&self, reference: &str, secret: &str) -> FsResult<()> {
            let mut fail_after = self.store_fail_after.lock().expect("lock store failure");
            if let Some(remaining) = fail_after.as_mut() {
                if *remaining == 0 {
                    return Err(test_credential_error());
                }
                *remaining -= 1;
            }
            drop(fail_after);

            self.values
                .lock()
                .expect("lock credentials")
                .insert(reference.to_string(), secret.to_string());
            Ok(())
        }

        fn load(&self, reference: &str) -> FsResult<Option<String>> {
            if *self.fail_load.lock().expect("lock load failure") {
                return Err(test_credential_error());
            }

            Ok(self
                .values
                .lock()
                .expect("lock credentials")
                .get(reference)
                .cloned())
        }

        fn delete(&self, reference: &str) -> FsResult<()> {
            if *self.fail_delete.lock().expect("lock delete failure") {
                return Err(test_credential_error());
            }

            self.values
                .lock()
                .expect("lock credentials")
                .remove(reference);
            Ok(())
        }
    }

    fn test_credential_error() -> FsError {
        FsError::new(
            "credential_store_unavailable",
            "Test credential store is unavailable.",
            None,
        )
    }

    fn test_store(
        path: &TestStorePath,
        credentials: Arc<TestRemoteCredentialStore>,
    ) -> AppStoreState {
        AppStoreState::initialize_at_with_credentials(path.store.clone(), credentials)
            .expect("test store should initialize")
    }

    fn raw_remote_volume_json(store: &AppStoreState) -> Option<String> {
        store
            .connection()
            .expect("lock test store")
            .query_row(
                "SELECT value FROM store_metadata WHERE key = ?1",
                params![REMOTE_VOLUMES_KEY],
                |row| row.get(0),
            )
            .optional()
            .expect("read raw remote metadata")
    }

    fn write_raw_remote_volume_json(store: &AppStoreState, value: &str) {
        store
            .connection()
            .expect("lock test store")
            .execute(
                "INSERT OR REPLACE INTO store_metadata (key, value) VALUES (?1, ?2)",
                params![REMOTE_VOLUMES_KEY, value],
            )
            .expect("write raw remote metadata");
    }

    fn store_file_contains(path: &Path, value: &str) -> bool {
        let bytes = fs::read(path).expect("read raw test store file");
        bytes
            .windows(value.len())
            .any(|window| window == value.as_bytes())
    }

    fn operation_journal_input(id: &str, status: &str) -> OperationJournalInput {
        OperationJournalInput {
            id: id.to_string(),
            operation: "copy".to_string(),
            label: "Copy files".to_string(),
            detail: "Preparing".to_string(),
            status: status.to_string(),
            payload: serde_json::json!({
                "sources": ["/tmp/source"],
                "destination": "/tmp/destination"
            }),
            progress: serde_json::json!({
                "processedBytes": 10,
                "totalBytes": 100
            }),
            created_at: Some(100),
            updated_at: Some(200),
            finished_at: operation_journal_status_is_finished(status).then_some(200),
        }
    }

    #[test]
    fn persists_remote_volume_configs() {
        let path = TestStorePath::new("remote-volumes");
        let credentials = Arc::new(TestRemoteCredentialStore::default());
        let config = RemoteVolumeConfig {
            id: "remote_one".to_string(),
            name: "Remote One".to_string(),
            scheme: "fs".to_string(),
            root: Some("/tmp/remote-one".to_string()),
            options: HashMap::from([("custom".to_string(), "value".to_string())]),
        };

        {
            let store = test_store(&path, credentials.clone());
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

        let store = test_store(&path, credentials);
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

    #[test]
    fn remote_credentials_are_referenced_not_serialized_and_round_trip() {
        let path = TestStorePath::new("remote-credential-storage");
        let credentials = Arc::new(TestRemoteCredentialStore::default());
        let store = test_store(&path, credentials.clone());
        let config = RemoteVolumeConfig {
            id: "secure_remote".to_string(),
            name: "Secure Remote".to_string(),
            scheme: "s3".to_string(),
            root: Some("documents".to_string()),
            options: HashMap::from([
                (
                    "endpoint".to_string(),
                    "https://test-user:test-only-url-password@storage.test".to_string(),
                ),
                ("access_key_id".to_string(), "TEST-KEY-ID".to_string()),
                (
                    "secret_access_key".to_string(),
                    "test-only-secret-access-value".to_string(),
                ),
                (
                    "session_token".to_string(),
                    "test-only-session-token-value".to_string(),
                ),
                ("custom".to_string(), "kept-in-sqlite".to_string()),
            ]),
        };

        store
            .save_remote_volume_config(config.clone())
            .expect("secure remote should save");

        let raw = raw_remote_volume_json(&store).expect("remote metadata should exist");
        assert!(!raw.contains("test-only-secret-access-value"));
        assert!(!raw.contains("test-only-session-token-value"));
        assert!(!raw.contains("test-only-url-password"));
        assert!(raw.contains("credentialRefs"));
        assert!(raw.contains("kept-in-sqlite"));
        assert!(raw.contains("TEST-KEY-ID"));

        let loaded = store
            .list_remote_volume_configs()
            .expect("secure remote should load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].options, config.options);
        assert_eq!(credentials.values().len(), 3);
    }

    #[test]
    fn legacy_remote_credentials_migrate_and_reconnect_after_reopen() {
        let path = TestStorePath::new("remote-credential-migration");
        let credentials = Arc::new(TestRemoteCredentialStore::default());
        let store = test_store(&path, credentials.clone());
        let legacy_config = RemoteVolumeConfig {
            id: "legacy_remote".to_string(),
            name: "Legacy Remote".to_string(),
            scheme: "webdav".to_string(),
            root: None,
            options: HashMap::from([
                ("endpoint".to_string(), "https://dav.test".to_string()),
                (
                    "password".to_string(),
                    "test-only-legacy-password".to_string(),
                ),
                ("username".to_string(), "carelo-test".to_string()),
            ]),
        };
        let legacy_json =
            serde_json::to_string(&vec![legacy_config.clone()]).expect("serialize legacy config");
        write_raw_remote_volume_json(&store, &legacy_json);
        assert!(store_file_contains(
            &path.store,
            "test-only-legacy-password"
        ));

        let migrated = store
            .list_remote_volume_configs()
            .expect("legacy remote should migrate");
        assert_eq!(migrated[0].options, legacy_config.options);
        let raw = raw_remote_volume_json(&store).expect("migrated metadata should exist");
        assert!(!raw.contains("test-only-legacy-password"));
        assert!(raw.contains("credentialRefs"));
        drop(store);
        assert!(!store_file_contains(
            &path.store,
            "test-only-legacy-password"
        ));

        let reopened = test_store(&path, credentials);
        let reloaded = reopened
            .list_remote_volume_configs()
            .expect("migrated remote should reload");
        assert_eq!(reloaded[0].options, legacy_config.options);
    }

    #[test]
    fn failed_legacy_migration_keeps_json_and_session_reconnect_usable() {
        let path = TestStorePath::new("remote-credential-migration-failure");
        let credentials = Arc::new(TestRemoteCredentialStore::default());
        let store = test_store(&path, credentials.clone());
        let legacy_config = RemoteVolumeConfig {
            id: "legacy_remote".to_string(),
            name: "Legacy Remote".to_string(),
            scheme: "webdav".to_string(),
            root: None,
            options: HashMap::from([
                ("endpoint".to_string(), "https://dav.test".to_string()),
                (
                    "password".to_string(),
                    "test-only-degraded-password".to_string(),
                ),
                (
                    "access_token".to_string(),
                    "test-only-degraded-token".to_string(),
                ),
                ("username".to_string(), "carelo-test".to_string()),
            ]),
        };
        let legacy_json =
            serde_json::to_string(&vec![legacy_config.clone()]).expect("serialize legacy config");
        write_raw_remote_volume_json(&store, &legacy_json);
        credentials.fail_store_after(1);

        let loaded = store
            .list_remote_volume_configs()
            .expect("legacy load should degrade without aborting startup");
        assert_eq!(loaded[0].options, legacy_config.options);
        assert_eq!(
            raw_remote_volume_json(&store).as_deref(),
            Some(legacy_json.as_str())
        );
    }

    #[test]
    fn failed_legacy_database_commit_keeps_json_and_retries_cleanly() {
        let path = TestStorePath::new("remote-credential-migration-db-failure");
        let credentials = Arc::new(TestRemoteCredentialStore::default());
        let store = test_store(&path, credentials.clone());
        let legacy_config = RemoteVolumeConfig {
            id: "legacy_remote".to_string(),
            name: "Legacy Remote".to_string(),
            scheme: "webdav".to_string(),
            root: None,
            options: HashMap::from([
                ("endpoint".to_string(), "https://dav.test".to_string()),
                (
                    "password".to_string(),
                    "test-only-db-failure-password".to_string(),
                ),
            ]),
        };
        let legacy_json =
            serde_json::to_string(&vec![legacy_config.clone()]).expect("serialize legacy config");
        write_raw_remote_volume_json(&store, &legacy_json);
        store
            .connection()
            .expect("lock test store")
            .execute_batch("PRAGMA query_only = ON")
            .expect("make test database read-only");

        let loaded = store
            .list_remote_volume_configs()
            .expect("legacy config should load after migration commit failure");
        assert_eq!(loaded[0].options, legacy_config.options);
        assert_eq!(
            raw_remote_volume_json(&store).as_deref(),
            Some(legacy_json.as_str())
        );
        store
            .connection()
            .expect("lock test store")
            .execute_batch("PRAGMA query_only = OFF")
            .expect("restore test database writes");

        let retried = store
            .list_remote_volume_configs()
            .expect("legacy migration should retry");
        assert_eq!(retried[0].options, legacy_config.options);
        let migrated_json = raw_remote_volume_json(&store).expect("migrated metadata");
        assert!(!migrated_json.contains("test-only-db-failure-password"));
        assert_eq!(credentials.values().len(), 1);
    }

    #[test]
    fn unavailable_sanitized_credentials_leave_safe_config_visible() {
        let path = TestStorePath::new("remote-credential-degraded-load");
        let credentials = Arc::new(TestRemoteCredentialStore::default());
        let store = test_store(&path, credentials.clone());
        let config = RemoteVolumeConfig {
            id: "safe_remote".to_string(),
            name: "Safe Remote".to_string(),
            scheme: "webdav".to_string(),
            root: None,
            options: HashMap::from([
                ("endpoint".to_string(), "https://dav.test".to_string()),
                (
                    "password".to_string(),
                    "test-only-unavailable-password".to_string(),
                ),
                ("username".to_string(), "carelo-test".to_string()),
            ]),
        };
        store
            .save_remote_volume_config(config)
            .expect("secure remote should save");
        credentials.set_load_failure(true);

        let loaded = store
            .list_remote_volume_configs()
            .expect("sanitized config should remain visible");
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].options.get("endpoint"),
            Some(&"https://dav.test".to_string())
        );
        assert_eq!(
            loaded[0].options.get("username"),
            Some(&"carelo-test".to_string())
        );
        assert!(!loaded[0].options.contains_key("password"));

        credentials.set_load_failure(false);
        credentials.clear();
        let missing = store
            .list_remote_volume_configs()
            .expect("missing credential should not hide config");
        assert!(!missing[0].options.contains_key("password"));
    }

    #[test]
    fn saving_while_keyring_is_locked_preserves_omitted_credentials() {
        let path = TestStorePath::new("remote-credential-locked-save");
        let credentials = Arc::new(TestRemoteCredentialStore::default());
        let store = test_store(&path, credentials.clone());
        let config = RemoteVolumeConfig {
            id: "locked_remote".to_string(),
            name: "Locked Remote".to_string(),
            scheme: "webdav".to_string(),
            root: None,
            options: HashMap::from([
                ("endpoint".to_string(), "https://dav.test".to_string()),
                (
                    "password".to_string(),
                    "test-only-preserved-password".to_string(),
                ),
            ]),
        };
        store
            .save_remote_volume_config(config)
            .expect("secure remote should save");
        assert_eq!(credentials.values().len(), 1);

        credentials.set_load_failure(true);
        let mut sanitized = store
            .list_remote_volume_configs()
            .expect("safe config should remain editable")
            .remove(0);
        assert!(!sanitized.options.contains_key("password"));
        sanitized.name = "Renamed while locked".to_string();
        store
            .save_remote_volume_config(sanitized)
            .expect("unrelated changes should preserve hidden references");

        credentials.set_load_failure(false);
        let reloaded = store
            .list_remote_volume_configs()
            .expect("credential should hydrate after unlock");
        assert_eq!(reloaded[0].name, "Renamed while locked");
        assert_eq!(
            reloaded[0].options.get("password").map(String::as_str),
            Some("test-only-preserved-password")
        );
        assert_eq!(credentials.values().len(), 1);

        let mut cleared = reloaded[0].clone();
        cleared
            .options
            .insert("password".to_string(), String::new());
        store
            .save_remote_volume_config(cleared)
            .expect("an explicit empty secret should clear it");
        assert!(!store
            .list_remote_volume_configs()
            .expect("cleared config should load")[0]
            .options
            .contains_key("password"));
        assert!(credentials.values().is_empty());
    }

    #[test]
    fn non_secret_remote_config_does_not_require_credential_store() {
        let path = TestStorePath::new("remote-without-credentials");
        let credentials = Arc::new(TestRemoteCredentialStore::default());
        credentials.set_store_failure(true);
        credentials.set_load_failure(true);
        let store = test_store(&path, credentials);
        let config = RemoteVolumeConfig {
            id: "local_remote".to_string(),
            name: "Local Remote".to_string(),
            scheme: "fs".to_string(),
            root: Some("/tmp".to_string()),
            options: HashMap::from([("custom".to_string(), "value".to_string())]),
        };

        store
            .save_remote_volume_config(config.clone())
            .expect("non-secret config should save");
        let loaded = store
            .list_remote_volume_configs()
            .expect("non-secret config should load");
        assert_eq!(loaded[0].options, config.options);
    }

    #[test]
    fn new_credential_save_fails_closed_when_secure_store_is_unavailable() {
        let path = TestStorePath::new("remote-credential-save-failure");
        let credentials = Arc::new(TestRemoteCredentialStore::default());
        credentials.set_store_failure(true);
        let store = test_store(&path, credentials);
        let config = RemoteVolumeConfig {
            id: "failed_remote".to_string(),
            name: "Failed Remote".to_string(),
            scheme: "webdav".to_string(),
            root: None,
            options: HashMap::from([(
                "password".to_string(),
                "test-only-unsaved-password".to_string(),
            )]),
        };

        let error = store
            .save_remote_volume_config(config)
            .expect_err("credential-bearing save should fail closed");
        assert_eq!(error.code, "credential_store_unavailable");
        assert!(raw_remote_volume_json(&store).is_none());
    }

    #[test]
    fn failed_database_commit_preserves_old_credential_reference() {
        let path = TestStorePath::new("remote-credential-db-rollback");
        let credentials = Arc::new(TestRemoteCredentialStore::default());
        let store = test_store(&path, credentials.clone());
        let mut config = RemoteVolumeConfig {
            id: "transactional_remote".to_string(),
            name: "Transactional Remote".to_string(),
            scheme: "webdav".to_string(),
            root: None,
            options: HashMap::from([
                ("endpoint".to_string(), "https://dav.test".to_string()),
                (
                    "password".to_string(),
                    "test-only-original-password".to_string(),
                ),
            ]),
        };
        store
            .save_remote_volume_config(config.clone())
            .expect("initial config should save");
        let original_json = raw_remote_volume_json(&store).expect("initial metadata");
        assert_eq!(credentials.values().len(), 1);

        store
            .connection()
            .expect("lock test store")
            .execute_batch("PRAGMA query_only = ON")
            .expect("make test database read-only");
        config.options.insert(
            "password".to_string(),
            "test-only-replacement-password".to_string(),
        );
        let error = store
            .save_remote_volume_config(config)
            .expect_err("read-only database should reject update");
        assert_eq!(error.code, "store_sql_error");
        store
            .connection()
            .expect("lock test store")
            .execute_batch("PRAGMA query_only = OFF")
            .expect("restore test database writes");

        assert_eq!(
            raw_remote_volume_json(&store).as_deref(),
            Some(original_json.as_str())
        );
        assert_eq!(credentials.values().len(), 1);
        let loaded = store
            .list_remote_volume_configs()
            .expect("original config should still load");
        assert_eq!(
            loaded[0].options.get("password").map(String::as_str),
            Some("test-only-original-password")
        );
    }

    #[test]
    fn replaced_credentials_commit_before_old_keyring_cleanup_and_retry() {
        let path = TestStorePath::new("remote-credential-replacement-cleanup");
        let credentials = Arc::new(TestRemoteCredentialStore::default());
        let store = test_store(&path, credentials.clone());
        let mut config = RemoteVolumeConfig {
            id: "replaced_remote".to_string(),
            name: "Replaced Remote".to_string(),
            scheme: "webdav".to_string(),
            root: None,
            options: HashMap::from([(
                "password".to_string(),
                "test-only-old-password".to_string(),
            )]),
        };
        store
            .save_remote_volume_config(config.clone())
            .expect("initial credential should save");

        credentials.set_delete_failure(true);
        config
            .options
            .insert("password".to_string(), "test-only-new-password".to_string());
        store
            .save_remote_volume_config(config)
            .expect("metadata replacement should commit before cleanup");
        assert_eq!(credentials.values().len(), 2);
        let loaded = store
            .list_remote_volume_configs()
            .expect("new reference should already be active");
        assert_eq!(
            loaded[0].options.get("password").map(String::as_str),
            Some("test-only-new-password")
        );

        credentials.set_delete_failure(false);
        drop(store);
        let reopened = test_store(&path, credentials.clone());
        assert_eq!(credentials.values().len(), 1);
        assert_eq!(
            reopened
                .list_remote_volume_configs()
                .expect("replacement should survive cleanup retry")[0]
                .options
                .get("password")
                .map(String::as_str),
            Some("test-only-new-password")
        );
    }

    #[test]
    fn remote_config_removal_commits_before_keyring_cleanup_and_retries() {
        let path = TestStorePath::new("remote-credential-delete-retry");
        let credentials = Arc::new(TestRemoteCredentialStore::default());
        let store = test_store(&path, credentials.clone());
        let config = RemoteVolumeConfig {
            id: "removed_remote".to_string(),
            name: "Removed Remote".to_string(),
            scheme: "webdav".to_string(),
            root: None,
            options: HashMap::from([(
                "password".to_string(),
                "test-only-removed-password".to_string(),
            )]),
        };
        store
            .save_remote_volume_config(config)
            .expect("remote should save");
        assert_eq!(credentials.values().len(), 1);

        credentials.set_delete_failure(true);
        assert!(store
            .remove_remote_volume_config("removed_remote")
            .expect("metadata removal should not depend on keyring cleanup"));
        assert_eq!(raw_remote_volume_json(&store).as_deref(), Some("[]"));
        assert_eq!(credentials.values().len(), 1);
        let pending: i64 = store
            .connection()
            .expect("lock test store")
            .query_row(
                "SELECT COUNT(*) FROM remote_credential_cleanup",
                [],
                |row| row.get(0),
            )
            .expect("count pending cleanup");
        assert_eq!(pending, 1);

        credentials.set_delete_failure(false);
        drop(store);
        let reopened = test_store(&path, credentials.clone());
        assert!(credentials.values().is_empty());
        let pending: i64 = reopened
            .connection()
            .expect("lock reopened test store")
            .query_row(
                "SELECT COUNT(*) FROM remote_credential_cleanup",
                [],
                |row| row.get(0),
            )
            .expect("count retried cleanup");
        assert_eq!(pending, 0);
        assert!(reopened
            .list_remote_volume_configs()
            .expect("removed config should stay removed")
            .is_empty());
    }

    #[test]
    fn failed_remote_config_removal_commit_preserves_referenced_credentials() {
        let path = TestStorePath::new("remote-credential-delete-db-failure");
        let credentials = Arc::new(TestRemoteCredentialStore::default());
        let store = test_store(&path, credentials.clone());
        let config = RemoteVolumeConfig {
            id: "removed_remote".to_string(),
            name: "Removed Remote".to_string(),
            scheme: "webdav".to_string(),
            root: None,
            options: HashMap::from([(
                "password".to_string(),
                "test-only-retained-password".to_string(),
            )]),
        };
        store
            .save_remote_volume_config(config)
            .expect("remote should save");
        let original_json = raw_remote_volume_json(&store).expect("saved remote metadata");
        store
            .connection()
            .expect("lock test store")
            .execute_batch("PRAGMA query_only = ON")
            .expect("make test database read-only");
        let database_error = store
            .remove_remote_volume_config("removed_remote")
            .expect_err("database failure should roll credential deletion back");
        assert_eq!(database_error.code, "store_sql_error");
        store
            .connection()
            .expect("lock test store")
            .execute_batch("PRAGMA query_only = OFF")
            .expect("restore test database writes");
        assert_eq!(
            raw_remote_volume_json(&store).as_deref(),
            Some(original_json.as_str())
        );
        assert_eq!(credentials.values().len(), 1);
        let pending: i64 = store
            .connection()
            .expect("lock test store")
            .query_row(
                "SELECT COUNT(*) FROM remote_credential_cleanup",
                [],
                |row| row.get(0),
            )
            .expect("count pending cleanup");
        assert_eq!(pending, 0);
    }

    #[cfg(unix)]
    #[test]
    fn app_store_uses_private_unix_permissions() {
        let path = TestStorePath::new("private-permissions");
        fs::create_dir_all(&path.directory).expect("create permissive test directory");
        fs::set_permissions(&path.directory, fs::Permissions::from_mode(0o755))
            .expect("set permissive test directory mode");
        fs::write(&path.store, []).expect("create permissive test store");
        fs::set_permissions(&path.store, fs::Permissions::from_mode(0o644))
            .expect("set permissive test store mode");

        let _store = test_store(&path, Arc::new(TestRemoteCredentialStore::default()));
        let secure_delete: i64 = _store
            .connection()
            .expect("lock test store")
            .query_row("PRAGMA secure_delete", [], |row| row.get(0))
            .expect("read secure delete setting");
        let directory_mode = fs::metadata(&path.directory)
            .expect("read directory metadata")
            .permissions()
            .mode()
            & 0o777;
        let store_mode = fs::metadata(&path.store)
            .expect("read store metadata")
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(secure_delete, 1);
        assert_eq!(directory_mode, 0o700);
        assert_eq!(store_mode, 0o600);
    }

    #[test]
    fn operation_journal_persists_and_reconciles_inflight_entries() {
        let path = TestStorePath::new("operation-journal-reconcile");
        let credentials = Arc::new(TestRemoteCredentialStore::default());
        let store = test_store(&path, credentials.clone());
        let inflight_statuses = ["running", "pausing", "paused", "cancelling"];

        for (index, status) in inflight_statuses.iter().enumerate() {
            let mut input = operation_journal_input(&format!("job-{index}"), status);
            input.payload = serde_json::json!({
                "sources": [format!("/tmp/source-{index}")],
                "destination": "/tmp/destination"
            });
            store
                .upsert_operation_journal_entry(input)
                .expect("in-flight journal entry should save");
        }

        let completed = operation_journal_input("job-completed", "completed");
        store
            .upsert_operation_journal_entry(completed.clone())
            .expect("completed journal entry should save");
        drop(store);

        let reopened = test_store(&path, credentials);
        let entries = reopened
            .list_operation_journal_entries()
            .expect("journal should reload");
        assert_eq!(entries.len(), 5);

        for (index, _) in inflight_statuses.iter().enumerate() {
            let entry = entries
                .iter()
                .find(|entry| entry.id == format!("job-{index}"))
                .expect("reconciled journal entry");
            assert_eq!(entry.status, "interrupted");
            assert!(entry.finished_at.is_some());
            assert_eq!(
                entry.payload["sources"][0],
                Value::String(format!("/tmp/source-{index}"))
            );
            assert_eq!(entry.progress["processedBytes"], Value::from(10));
        }

        let completed_entry = entries
            .iter()
            .find(|entry| entry.id == "job-completed")
            .expect("completed entry should persist");
        assert_eq!(completed_entry.status, "completed");
        assert_eq!(completed_entry.payload, completed.payload);
    }

    #[test]
    fn operation_journal_prunes_only_old_finished_entries() {
        let path = TestStorePath::new("operation-journal-prune");
        let store = test_store(&path, Arc::new(TestRemoteCredentialStore::default()));
        store
            .upsert_operation_journal_entry(operation_journal_input("active-job", "queued"))
            .expect("active entry should save");

        for index in 0..205_i64 {
            let mut input = operation_journal_input(&format!("finished-{index:03}"), "completed");
            let timestamp = index + 1;
            input.created_at = Some(timestamp);
            input.updated_at = Some(timestamp);
            input.finished_at = Some(timestamp);
            store
                .upsert_operation_journal_entry(input)
                .expect("finished entry should save");
        }

        let entries = store
            .list_operation_journal_entries()
            .expect("journal should list");
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.finished_at.is_some())
                .count(),
            OPERATION_JOURNAL_FINISHED_LIMIT as usize
        );
        assert!(entries.iter().any(|entry| entry.id == "active-job"));
        assert!(!entries.iter().any(|entry| entry.id == "finished-000"));
        assert!(!entries.iter().any(|entry| entry.id == "finished-004"));
        assert!(entries.iter().any(|entry| entry.id == "finished-005"));
        assert!(entries.iter().any(|entry| entry.id == "finished-204"));
    }

    #[test]
    fn operation_journal_rejects_invalid_or_credential_bearing_inputs() {
        let path = TestStorePath::new("operation-journal-validation");
        let store = test_store(&path, Arc::new(TestRemoteCredentialStore::default()));

        let mut invalid_id = operation_journal_input("bad/id", "running");
        assert_eq!(
            store
                .upsert_operation_journal_entry(invalid_id.clone())
                .expect_err("invalid id should fail")
                .code,
            "invalid_operation_journal"
        );

        invalid_id.id = "valid-id".to_string();
        invalid_id.status = "unknown".to_string();
        assert_eq!(
            store
                .upsert_operation_journal_entry(invalid_id)
                .expect_err("invalid status should fail")
                .code,
            "invalid_operation_journal"
        );

        let mut credential_payload = operation_journal_input("credential-payload", "running");
        credential_payload.payload = serde_json::json!({
            "request": { "credentials": { "clientSecret": "test-only-value" } }
        });
        assert_eq!(
            store
                .upsert_operation_journal_entry(credential_payload)
                .expect_err("credential field should fail")
                .code,
            "invalid_operation_journal"
        );

        let mut authorization_value = operation_journal_input("credential-value", "running");
        authorization_value.progress = Value::String("Bearer test-only-value".to_string());
        assert_eq!(
            store
                .upsert_operation_journal_entry(authorization_value)
                .expect_err("authorization value should fail")
                .code,
            "invalid_operation_journal"
        );

        let mut credential_url = operation_journal_input("credential-url", "running");
        credential_url.progress = serde_json::json!({
            "endpoint": "https://user:test-only-value@example.test/files"
        });
        assert_eq!(
            store
                .upsert_operation_journal_entry(credential_url)
                .expect_err("credential-bearing URL should fail")
                .code,
            "invalid_operation_journal"
        );

        let mut credential_detail = operation_journal_input("credential-detail", "failed");
        credential_detail.detail = "Bearer test-only-value".to_string();
        assert_eq!(
            store
                .upsert_operation_journal_entry(credential_detail)
                .expect_err("credential-bearing detail should fail")
                .code,
            "invalid_operation_journal"
        );

        let mut oversized = operation_journal_input("oversized", "running");
        oversized.payload = Value::String("x".repeat(OPERATION_JOURNAL_JSON_LIMIT_BYTES));
        assert_eq!(
            store
                .upsert_operation_journal_entry(oversized)
                .expect_err("oversized payload should fail")
                .code,
            "invalid_operation_journal"
        );

        assert!(store
            .list_operation_journal_entries()
            .expect("journal should list")
            .is_empty());
    }

    #[test]
    fn operation_journal_clear_and_remove_keep_active_entries() {
        let path = TestStorePath::new("operation-journal-remove");
        let store = test_store(&path, Arc::new(TestRemoteCredentialStore::default()));
        store
            .upsert_operation_journal_entry(operation_journal_input("active-job", "queued"))
            .expect("active entry should save");
        let mut active_update = operation_journal_input("active-job", "running");
        active_update.created_at = Some(999);
        active_update.updated_at = Some(300);
        active_update.detail = "Running".to_string();
        let updated = store
            .upsert_operation_journal_entry(active_update)
            .expect("active entry should update");
        assert_eq!(updated.created_at, 100);
        assert_eq!(updated.updated_at, 300);
        assert_eq!(updated.detail, "Running");
        store
            .upsert_operation_journal_entry(operation_journal_input("finished-job", "completed"))
            .expect("finished entry should save");

        assert_eq!(
            store
                .clear_finished_operation_journal_entries()
                .expect("finished entries should clear"),
            1
        );
        let entries = store
            .list_operation_journal_entries()
            .expect("journal should list");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "active-job");
        assert!(store
            .remove_operation_journal_entry("active-job")
            .expect("active entry should remove"));
        assert!(!store
            .remove_operation_journal_entry("active-job")
            .expect("missing entry removal should be a no-op"));
    }

    #[test]
    fn operation_journal_ignores_stale_updates_and_never_resurrects_finished_entries() {
        let path = TestStorePath::new("operation-journal-stale-writes");
        let store = test_store(&path, Arc::new(TestRemoteCredentialStore::default()));
        let mut running = operation_journal_input("ordered-job", "running");
        running.updated_at = Some(300);
        running.detail = "Latest progress".to_string();
        store
            .upsert_operation_journal_entry(running)
            .expect("running entry should save");

        let mut stale = operation_journal_input("ordered-job", "paused");
        stale.updated_at = Some(250);
        stale.detail = "Stale pause".to_string();
        let retained = store
            .upsert_operation_journal_entry(stale)
            .expect("stale writes should be harmless");
        assert_eq!(retained.status, "running");
        assert_eq!(retained.detail, "Latest progress");
        assert_eq!(retained.updated_at, 300);

        let mut completed = operation_journal_input("ordered-job", "completed");
        completed.updated_at = Some(400);
        completed.finished_at = Some(400);
        completed.detail = "Done".to_string();
        let completed = store
            .upsert_operation_journal_entry(completed)
            .expect("completion should save");
        assert_eq!(completed.status, "completed");
        assert_eq!(completed.finished_at, Some(400));

        let mut late_progress = operation_journal_input("ordered-job", "running");
        late_progress.updated_at = Some(500);
        late_progress.detail = "Late progress".to_string();
        let retained = store
            .upsert_operation_journal_entry(late_progress)
            .expect("late progress should be ignored");
        assert_eq!(retained.status, "completed");
        assert_eq!(retained.detail, "Done");
        assert_eq!(retained.updated_at, 400);
        assert_eq!(retained.finished_at, Some(400));

        let listed = store
            .list_operation_journal_entries()
            .expect("journal should remain readable");
        assert_eq!(listed, vec![retained]);
    }

    #[test]
    fn operation_journal_does_not_change_existing_store_data() {
        let path = TestStorePath::new("operation-journal-existing-data");
        let credentials = Arc::new(TestRemoteCredentialStore::default());
        let settings = serde_json::json!({ "appearanceMode": "dark", "restoreSession": true });
        fs::create_dir_all(&path.directory).expect("create legacy store directory");
        let legacy_connection = Connection::open(&path.store).expect("open legacy store");
        legacy_connection
            .execute_batch(
                "CREATE TABLE store_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
            )
            .expect("create legacy metadata table");
        legacy_connection
            .execute(
                "INSERT INTO store_metadata (key, value) VALUES (?1, ?2)",
                params![
                    APP_SETTINGS_KEY,
                    serde_json::to_string(&settings).expect("serialize legacy settings")
                ],
            )
            .expect("seed legacy settings");
        drop(legacy_connection);

        let store = test_store(&path, credentials.clone());
        assert_eq!(
            store.app_settings().expect("legacy settings should load"),
            Some(settings.clone())
        );
        store
            .record_recent_location("/tmp/existing".to_string(), "Existing".to_string())
            .expect("recent location should save");
        let remote = RemoteVolumeConfig {
            id: "existing_remote".to_string(),
            name: "Existing Remote".to_string(),
            scheme: "fs".to_string(),
            root: Some("/tmp/existing".to_string()),
            options: HashMap::from([("custom".to_string(), "preserved".to_string())]),
        };
        store
            .save_remote_volume_config(remote.clone())
            .expect("remote should save");
        store
            .upsert_operation_journal_entry(operation_journal_input("journal-job", "completed"))
            .expect("journal entry should save");
        drop(store);

        let reopened = test_store(&path, credentials);
        assert_eq!(
            reopened.app_settings().expect("settings should reload"),
            Some(settings)
        );
        let recent = reopened
            .list_recent_locations()
            .expect("recent locations should reload");
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].path, "/tmp/existing");
        let remotes = reopened
            .list_remote_volume_configs()
            .expect("remotes should reload");
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].id, remote.id);
        assert_eq!(remotes[0].options, remote.options);
    }

    #[test]
    fn recent_locations_dedupe_order_and_persist() {
        let path = TestStorePath::new("recent-locations");

        {
            let store =
                AppStoreState::initialize_at(path.store.clone()).expect("store should initialize");

            // Brief pauses keep the millisecond timestamps distinct so the
            // visited_at ordering is deterministic.
            let tick = || std::thread::sleep(std::time::Duration::from_millis(2));

            store
                .record_recent_location("/tmp/a".to_string(), "a".to_string())
                .expect("record a");
            tick();
            store
                .record_recent_location("/tmp/b".to_string(), "b".to_string())
                .expect("record b");
            tick();
            // Re-visiting a known path moves it to the front without duplicating.
            store
                .record_recent_location("/tmp/a".to_string(), "a".to_string())
                .expect("re-record a");

            let recents = store.list_recent_locations().expect("list recents");
            let paths: Vec<String> = recents.iter().map(|entry| entry.path.clone()).collect();
            assert_eq!(paths, vec!["/tmp/a".to_string(), "/tmp/b".to_string()]);

            // Blank paths are ignored.
            store
                .record_recent_location("   ".to_string(), "blank".to_string())
                .expect("blank is a no-op");
            assert_eq!(store.list_recent_locations().expect("list").len(), 2);

            store
                .remove_recent_location("/tmp/a".to_string())
                .expect("remove a");
            let after_remove = store.list_recent_locations().expect("list after remove");
            assert_eq!(after_remove.len(), 1);
            assert_eq!(after_remove[0].path, "/tmp/b");
        }

        // Reopen to confirm persistence, then verify the size cap and clear.
        let store = AppStoreState::initialize_at(path.store.clone()).expect("store should reopen");
        assert_eq!(store.list_recent_locations().expect("reload").len(), 1);

        for index in 0..(RECENT_LOCATIONS_LIMIT + 10) {
            store
                .record_recent_location(format!("/tmp/dir-{index}"), format!("dir-{index}"))
                .expect("record many");
        }

        assert_eq!(
            store.list_recent_locations().expect("list capped").len() as i64,
            RECENT_LOCATIONS_LIMIT
        );

        store.clear_recent_locations().expect("clear recents");
        assert!(store
            .list_recent_locations()
            .expect("list after clear")
            .is_empty());
    }
}
