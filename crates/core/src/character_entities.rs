//! 角色、皮肤、作品的身份：词条（`terms`：key = 身份 id、kind）加结构与属性（`term_tree`）。核心第 8 版（#201）起它们
//! 是唯一权威，命令直接读写；原来的兼容表 `entities` 在升级时核对后删除（`legacy_characters::retire_entities`）。
//!
//! 属性列 `variant`、`artist`、`label_only`、`mascot`、`mascot_origin`、`original` 在第 8 版从 `terms.attrs` 提升为
//! `term_tree` 的普通列（用户 2026-10-20 决定）。`seq` 是身份的先后（原来 `entities` 的 rowid，名字拼接顺序、「第一个
//! 只当标签的身份」都按它），新身份取当前最大值加一，与 SQLite 分配 rowid 的规则相同。
use crate::Result;
use rusqlite::{params, Connection};

/// 与原来的 `entities` 同形的读法（核心从不承载的 `name`、`external_key`、`tag` 三列不在内），只含 `term_tree` 里的
/// 词条；另带 `seq`（原来的 rowid 顺序）和 `term_id`。按 `id` 找走 `terms.key` 的唯一索引，按 `parent_id`、
/// `work_id` 找走 `term_tree` 的索引。
pub const ENTITIES: &str = "(SELECT t.key AS id, t.kind AS kind, r.work_text AS work, r.variant AS variant,
    r.parent_id AS parent_id, r.merged_into AS merged_into, r.artist AS artist, r.label_only AS label_only,
    r.mascot AS mascot, r.mascot_origin AS mascot_origin, r.work_id AS work_id, r.original AS original,
    r.seq AS seq, r.term_id AS term_id
  FROM term_tree r
  JOIN terms t ON t.id = r.term_id)";

/// SQL 条件：这些固定 SQL 表达式指向的人物身份里没有只作标签的团体。
pub fn without_groups(keys: &[&str]) -> String {
    let keys = keys.join(",");
    format!(
        "NOT EXISTS(SELECT 1 FROM terms t JOIN term_tree r ON r.term_id=t.id
            WHERE t.key IN ({keys}) AND r.label_only=1 AND r.original=0)"
    )
}

/// 一个身份在 `term_tree` 里的行（身份 id 是参数 `?1`）：原来的 `UPDATE entities … WHERE id = ?`。
pub const OF: &str = "term_id = (SELECT id FROM terms WHERE key = ?1)";

/// 新身份的先后：当前最大值加一。
pub const NEXT_SEQ: &str = "(SELECT coalesce(max(seq), 0) + 1 FROM term_tree)";

/// 一个新身份（原来的 `INSERT INTO entities`）。没给的列取原表的默认值。
#[derive(Default)]
pub struct New<'a> {
    pub id: &'a str,
    pub kind: &'a str,
    pub work: &'a str,
    pub variant: &'a str,
    pub parent_id: Option<&'a str>,
    pub artist: &'a str,
    pub label_only: i64,
    pub mascot: i64,
    pub mascot_origin: Option<&'a str>,
    pub work_id: Option<&'a str>,
    pub original: i64,
}

/// 建一个身份：词条（`origin` 记来源）和它的结构行。
pub fn create(c: &Connection, e: &New, origin: &str) -> Result<()> {
    c.prepare_cached("INSERT INTO terms(key, kind, origin) VALUES (?1, ?2, ?3)")?
        .execute(params![e.id, e.kind, origin])?;
    let term: i64 = c.query_row("SELECT id FROM terms WHERE key=?1", [e.id], |r| r.get(0))?;
    c.prepare_cached(&format!(
        "INSERT INTO term_tree(term_id, parent_id, merged_into, work_id, work_text, variant, artist, label_only, mascot,
           mascot_origin, original, seq)
         VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, {NEXT_SEQ})"
    ))?
    .execute(params![
        term,
        e.parent_id,
        e.work_id,
        e.work,
        e.variant,
        e.artist,
        e.label_only,
        e.mascot,
        e.mascot_origin,
        e.original
    ])?;
    Ok(())
}
