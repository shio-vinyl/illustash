//! 用户标签、分级、WD 题材在词条核心里（docs/TERMS_LABELS.md）。
//!
//! - 分级、WD 题材：机器证据按证据包交来（链接 `wd_rating:<值>`、`wd:<标签>`），判定器见 schema.rs。
//! - 用户标签：用户自建的概念（kind=concept，显示名来源 `user`）；「加上」是裁决 yes，「去掉」是 clear（人收回，不是否定）。
//! - `asset_tags`、`general_hits`、`asset_rating` 是核心之上同名同形的视图（`VIEWS`），读查询一行不改。新建图库由
//!   `new_library` 换上；原来从旧表收编的切换命令随 #201 第 4 步删除。

use crate::schema::Kind;
use crate::store::{create_term, TermId};
use crate::Result;
use rusqlite::{Connection, OptionalExtension};

// 视图和页内读取使用同一份标签投影。裁决的生效顺序由 term_effective_verdicts 维护。
macro_rules! asset_tag_rows {
    () => {
        "
 SELECT s.uuid,n.value,'review',v.created_at FROM term_effective_verdicts v
 JOIN terms t ON t.id=v.term_id AND t.kind='concept'
 JOIN term_names n ON n.term_id=t.id AND n.role='display' AND n.source='user' AND n.lang=''
 JOIN term_assets a ON a.id=v.asset_id AND a.namespace='library'
 JOIN assets s ON s.uuid=a.key
 WHERE v.verdict='yes'"
    };
}

/// 宿主按原表名读的三张表换成同名同形的视图（列名、含义不变）。只显示仍在 `assets` 里的图。
pub const VIEWS: &str = concat!(
    "
CREATE VIEW general_hits(uuid,value) AS
 SELECT s.uuid,l.key FROM term_projection p
 JOIN terms t ON t.id=p.term_id AND t.kind='topic'
 JOIN term_links l ON l.term_id=t.id AND l.system='wd'
 JOIN term_assets a ON a.id=p.asset_id AND a.namespace='library'
 JOIN assets s ON s.uuid=a.key
 WHERE p.state IN ('machine','confirmed');
CREATE VIEW asset_rating(uuid,rating) AS
 SELECT s.uuid,l.key FROM term_projection p
 JOIN terms t ON t.id=p.term_id AND t.kind='rating'
 JOIN term_links l ON l.term_id=t.id AND l.system='wd_rating'
 JOIN term_assets a ON a.id=p.asset_id AND a.namespace='library'
 JOIN assets s ON s.uuid=a.key
 WHERE p.state IN ('machine','confirmed');
CREATE VIEW asset_tags(uuid,tag,origin,created_at) AS",
    asset_tag_rows!(),
    ";\n"
);

/// 读取当前页图片的用户标签，按标签排序。`uuids` 是当前读事务中这页的 UUID JSON 数组。
pub fn page_asset_tags(conn: &Connection, uuids: &str) -> Result<Vec<(String, String)>> {
    let mut find = conn
        .prepare("SELECT id FROM term_assets WHERE namespace='library' AND key IN (SELECT value FROM json_each(?))")?;
    let ids = find.query_map([uuids], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // 将绑定的资产 ID 下推到有效裁决的窗口分区。用子查询取 ID 时，SQLite 会先计算全库裁决。
    let marks = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "WITH page_tags(uuid,tag,origin,created_at) AS ({} AND v.asset_id IN ({marks}))
         SELECT uuid,tag FROM page_tags ORDER BY tag",
        asset_tag_rows!(),
    );
    let mut tags = conn.prepare(&sql)?;
    let rows = tags.query_map(rusqlite::params_from_iter(ids), |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 用户概念（标签）按名字找；`create` 时没有就建（显示名来源 `user`）。
pub fn concept_by_name(conn: &Connection, name: &str, create: Option<&str>) -> Result<Option<TermId>> {
    let found = conn
        .query_row(
            "SELECT t.id FROM terms t JOIN term_names n ON n.term_id=t.id AND n.role='display' AND n.source='user' AND n.lang=''
             WHERE t.kind='concept' AND n.value=?1 ORDER BY t.id LIMIT 1",
            [name],
            |r| r.get(0),
        )
        .optional()?;
    match (found, create) {
        (Some(id), _) => Ok(Some(id)),
        (None, Some(origin)) => {
            let id = create_term(conn, Kind::Concept, None, origin)?;
            crate::store::add_name(conn, id, "", name, "display", "user")?;
            Ok(Some(id))
        }
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
#[path = "legacy_labels/tests.rs"]
mod tests;
