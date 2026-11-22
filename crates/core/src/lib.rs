//! Illustash 连接内共享业务规则；不依赖应用宿主、HTTP、CLI 或平台实现。
//!
//! 注释里的「旧实现」指 #9 删除的 Python 应用（HTTP、Store 编排、识别导入等，原在 `tests/reference/`）。
//! 写着「沿用旧实现」的格式和规则是已存数据或既有响应依赖的兼容约束，改动前先确认没有数据或录制期望依赖它。
#![cfg_attr(not(test), warn(clippy::unwrap_used))]
pub mod backup;
pub mod casefold;
pub mod ccip_longtail;
pub mod character_catalog;
pub mod character_commands;
pub mod character_entities;
pub mod character_evidence;
mod character_extension;
pub mod character_graph;
pub mod character_identity;
pub mod character_import;
pub mod character_json;
pub mod character_labels;
pub mod character_learning;
pub mod character_links;
pub mod character_merge;
pub mod character_names;
pub mod character_profile;
pub mod character_projection;
pub mod character_relations;
pub mod character_review;
pub mod character_structure;
pub mod compat_text;
#[path = "http_unicode.rs"]
pub mod compat_unicode;
pub mod error;
pub mod human_conclusions;
pub mod name_codecs;
pub mod names_commands;
pub mod names_search;
pub mod names_view;
pub mod new_library;
pub mod origin;
pub mod package;
#[path = "schema.rs"]
mod schema_rules;
pub mod store;
pub mod strict_json;
pub mod undo_labels;
pub mod wd_extension;
#[cfg(not(test))]
pub mod schema {
    pub use super::schema_rules::*;
}
#[cfg(test)]
pub use test_support::schema;
pub mod legacy_characters;
pub mod legacy_labels;
pub mod legacy_names;
pub use error::{Error, Result};
use rusqlite::Connection;
pub use schema::{Kind, SCHEMA_VERSION};
use std::time::Duration;
pub fn configure(conn: &Connection) -> Result<()> {
    conn.set_prepared_statement_cache_capacity(128);
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(())
}
#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod test_support;
#[cfg(test)]
use test_support::{migrate, open, platform_backup, sqlite};
