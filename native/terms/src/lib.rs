//! Illustash 应用宿主：进程内应用服务、CLI/HTTP 适配、任务编排与平台装配。
//!
//! 连接内业务规则、证据、人工裁决、关系、历史和投影由 `illustash-core` 单一实现。
//! 本宿主保留旧 Rust 模块路径的转导出与必需备份/SQLite 路径适配；见 docs/ARCHITECTURE_DIRECTION.md。

#![cfg_attr(not(test), warn(clippy::unwrap_used))]

pub mod app_accounts;
mod app_archives;
mod app_characters;
mod app_commands;
pub mod app_create;
mod app_curation;
mod app_desktop;
mod app_effects;
mod app_files;
mod app_folder;
mod app_http;
mod app_identity;
mod app_imports;
mod app_integrity;
mod app_libraries;
pub mod app_library;
mod app_local;
mod app_migrate;
mod app_migrate_data;
pub mod app_models;
mod app_onboarding;
pub mod app_service;
use illustash_platform::app_paths;
mod app_photos;
mod app_platform;
mod app_preferences;
mod app_profiles;
pub mod app_recognition;
mod app_rescue;
pub mod app_research;
pub mod app_resources;
mod app_settings;
mod app_sources;
mod app_sync;
pub mod app_tasks;
mod app_updates;
mod app_upgrade;
mod image_samples;
pub mod index_compute;
pub mod provenance_compute;
pub mod recognition_compute;
pub mod research_compute;
pub use illustash_core::casefold;
pub use illustash_core::character_commands;
pub use illustash_core::character_entities;
pub use illustash_core::character_evidence;
pub use illustash_core::character_graph;
pub use illustash_core::character_identity;
pub use illustash_core::character_import;
pub use illustash_core::character_json;
pub use illustash_core::character_labels;
pub use illustash_core::character_learning;
pub use illustash_core::character_links;
pub use illustash_core::character_merge;
pub use illustash_core::character_names;
pub use illustash_core::character_profile;
pub use illustash_core::character_projection;
pub use illustash_core::character_relations;
pub use illustash_core::character_review;
pub use illustash_core::character_structure;
pub mod cli;
pub(crate) use illustash_core::compat_text;

pub(crate) use illustash_core::compat_unicode;
pub mod desktop_shell;
pub(crate) use illustash_core::error;
pub mod http;
mod http_app;
mod http_artists;
mod http_browse;
mod http_characters;
mod http_json;
mod http_local;
mod http_read;
#[cfg(feature = "test-read-host")]
pub mod read_harness;
mod sql_fragments;
pub use illustash_core::legacy_characters;
pub use illustash_core::legacy_labels;
pub use illustash_core::legacy_names;
pub use illustash_core::names_commands;
pub use illustash_core::names_search;
pub use illustash_core::names_view;
pub use illustash_core::new_library;
pub use illustash_core::origin;
pub use illustash_core::package;
use illustash_core::schema as schema_rules;
use illustash_platform::platform_backup;
use illustash_platform::platform_credentials;
use illustash_platform::platform_filesystem;
use illustash_platform::platform_folder;
use illustash_platform::platform_photos;
use illustash_platform::platform_system;
use illustash_platform::platform_time;
/// Existing API facade: every direct file-backed call receives the mandatory platform backup.
pub mod schema {
    pub use crate::schema_rules::*;
    pub fn migrate(conn: &mut rusqlite::Connection) -> crate::Result<i64> {
        crate::schema_rules::migrate_with_backup(conn, &crate::platform_backup::capability())
    }
    #[cfg(test)]
    pub(crate) fn migrate_to(conn: &mut rusqlite::Connection, target: i64) -> crate::Result<i64> {
        crate::schema_rules::migrate_to_with_backup(conn, target, &crate::platform_backup::capability())
    }
}
mod service;
use illustash_platform::source_rows;
pub mod sqlite;
pub use illustash_core::store;
pub use illustash_core::strict_json;
pub(crate) mod table_delta;
mod task_state;
mod wd_pending;
mod worker_protocol;

pub use error::{Error, Result};
pub use schema::{migrate, Kind, SCHEMA_VERSION};

use rusqlite::Connection;
use std::path::Path;

/// 打开一个库并设置本连接的约定（外键、忙等待）。不迁移，也不改日志模式：日志模式归宿主决定。
pub fn open(path: &Path) -> Result<Connection> {
    let conn = crate::sqlite::open(path)?;
    configure(&conn)?;
    Ok(conn)
}

pub use illustash_core::configure;

/// How long background writers (sync, recognition imports, name fetching) wait for another writer before they give
/// up with "database is locked". Background work such as a research publication or a batch import holds the write
/// lock for seconds on a large library; a quarter of a second was not enough (#275). Waiting writers of the WAL
/// application database block no reader; a rollback-journal source writer blocks new readers only while it commits.
pub(crate) const WRITER_BUSY: std::time::Duration = std::time::Duration::from_secs(10);

mod app_name_sources;
mod app_names;
pub(crate) use illustash_core::name_codecs;
pub(crate) use illustash_core::undo_labels;

mod app_index;
pub(crate) mod app_provenance;
mod index_generation;

use illustash_platform::app_file_lock;
pub mod platform_process;
