//! 名字搜索（docs/TERMS_NAMES.md 方案 N4）：逐条移植 tests/reference/web/names.py（#9 已删除） 的 `search_hits`。
//!
//! 给一个查询，返回 {身份: 排名}：完全相同 = 2、开头 = 1、包含 = 0，同一身份取最大。匹配面：身份的作品、画师、皮肤文字
//! （原样与 `_` 换成空格）；当前与退役的链接 key（人物 tag 另比可读形式）；身份的名字；按文字键的名字与别名（作品、皮肤）；
//! 作品词表 `WORK_NAMES`；作品的名字或链接命中时，它整棵子树下的角色（`works.TREE`，`UNION` 防环）；皮肤词 `skin_zh`；
//! 「父 · 皮肤」的组合查询（两半各搜一次，交集至少算开头）。
//!
//! 与旧代码相同地先用 SQLite 的 `LIKE`（只对 ASCII 不分大小写）筛，再做 Unicode 完全大小写折叠后比较
//! （`casefold.rs`，与旧实现的 `str.casefold()` 相同）；SQL 语句逐字照搬，行的顺序也就相同（作品命中那一段按行的顺序后者覆盖前者，旧代码如此）。
use crate::casefold::casefold;
use crate::character_entities::ENTITIES;
use crate::character_links::LINKS;
use crate::names_view::{readable, skin_zh, WORK_NAMES};
use crate::Result;
use rusqlite::{params_from_iter, Connection};
use std::collections::HashMap;

pub type Hits = HashMap<String, i64>;

fn rank(folded: &str, needle: &str) -> i64 {
    if folded == needle {
        2
    } else if folded.starts_with(needle) {
        1
    } else {
        0
    }
}

fn raise(hits: &mut Hits, ident: String, rank: i64) {
    let slot = hits.entry(ident).or_insert(-1);
    *slot = (*slot).max(rank);
}

struct Search<'a> {
    c: &'a Connection,
    like: String,
    needle: String,
    hits: Hits,
}

impl Search<'_> {
    /// 旧代码的 `add`：`character_tag` 时同一个 key 还比它的可读形式。
    fn add(&mut self, sql: &str, args: &[&str], character_tag: bool) -> Result<()> {
        let mut stmt = self.c.prepare(sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (ident, term) = row?;
            let Some(term) = term.filter(|t| !t.is_empty()) else { continue };
            let mut values = vec![term.clone()];
            if character_tag {
                values.push(readable(&term, true));
            }
            for value in values {
                let folded = casefold(&value);
                if folded.contains(&self.needle) {
                    let r = rank(&folded, &self.needle);
                    raise(&mut self.hits, ident.clone(), r);
                }
            }
        }
        Ok(())
    }
}

/// `names.search_hits(c, query)`。
pub fn hits(c: &Connection, query: &str) -> Result<Hits> {
    let like = format!("%{}%", query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
    let mut s = Search { c, like, needle: casefold(query), hits: Hits::new() };
    let like = s.like.clone();
    for field in ["work", "artist", "variant"] {
        s.add(
            &format!("SELECT coalesce(parent_id,id),{field} FROM {ENTITIES} WHERE {field} LIKE ? ESCAPE '\\'"),
            &[&like],
            false,
        )?;
    }
    for field in ["work", "variant"] {
        s.add(
            &format!(
                "SELECT coalesce(parent_id,id),replace({field},'_',' ') FROM {ENTITIES} WHERE replace({field},'_',' ') LIKE ? ESCAPE '\\'"
            ),
            &[&like],
            false,
        )?;
    }
    let tags = format!(
        "SELECT coalesce(e.parent_id, e.id), {{0}}
         FROM {LINKS} l
         JOIN {ENTITIES} e ON e.id = l.entity_id
         WHERE e.kind <> 'work'
           AND l.core_system IN ('danbooru', 'wd_character', 'photos_album', 'ccip', 'ccip_base', 'bangumi')
           AND {{0}} LIKE ? ESCAPE '\\'"
    );
    s.add(&tags.replace("{0}", "l.key"), &[&like], true)?;
    s.add(&tags.replace("{0}", "replace(l.key,'_',' ')"), &[&like], false)?;
    s.add(
        &format!(
            "SELECT coalesce(e.parent_id, e.id), n.value
             FROM term_names n
             JOIN terms t ON t.id = n.term_id
             JOIN {ENTITIES} e ON e.id = t.key
             WHERE n.value LIKE ? ESCAPE '\\'"
        ),
        &[&like],
        false,
    )?;
    // 离线 Wikidata 中文名来自目录，只出现在名称投影里。
    s.add(
        "SELECT d.entity, d.name FROM term_name_display d
         JOIN terms t ON t.key=d.entity JOIN term_tree r ON r.term_id=t.id
         WHERE d.skin_only=0 AND d.name_source='builtin' AND d.name LIKE ? ESCAPE '\\'
           AND t.kind='character' AND r.parent_id IS NULL",
        &[&like],
        false,
    )?;
    for (table, field) in [("term_key_names", "value"), ("term_key_aliases", "value")] {
        for (kind, column) in [("work", "work"), ("skin", "variant")] {
            let join = format!("JOIN {ENTITIES} e ON e.{column}=t.key");
            let source = if table == "term_key_names" { "AND t.source<>'danbooru'" } else { "" };
            s.add(
                &format!(
                    "SELECT coalesce(e.parent_id,e.id),t.{field} FROM {table} t {join} WHERE t.kind=? AND t.{field} LIKE ? ESCAPE '\\' {source}"
                ),
                &[kind, &like],
                false,
            )?;
        }
    }
    let needle = s.needle.clone();
    let mut hits = std::mem::take(&mut s.hits);
    let builtin: Vec<&str> =
        WORK_NAMES.iter().filter(|(_, v)| casefold(v).contains(&needle)).map(|(k, _)| *k).collect();
    if !builtin.is_empty() {
        let marks = vec!["?"; builtin.len()].join(",");
        let mut stmt =
            c.prepare(&format!("SELECT coalesce(parent_id,id),work FROM {ENTITIES} WHERE work IN ({marks})"))?;
        let rows =
            stmt.query_map(params_from_iter(builtin.iter()), |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (ident, work) = row?;
            let value = WORK_NAMES.iter().find(|(k, _)| *k == work).map(|(_, v)| casefold(v)).unwrap_or_default();
            raise(&mut hits, ident, rank(&value, &needle));
        }
    }
    // 作品的名字或链接命中：它整棵子树下的角色，任意深度。
    let mut work_hits: Vec<(String, i64)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    {
        let mut stmt = c.prepare(&format!(
            "SELECT t.key, n.value
             FROM term_names n
             JOIN terms t ON t.id = n.term_id
             JOIN {ENTITIES} e ON e.id = t.key
             WHERE e.kind = 'work'
             UNION
             SELECT l.entity_id, l.key
             FROM {LINKS} l
             JOIN {ENTITIES} e ON e.id = l.entity_id
             WHERE e.kind = 'work'"
        ))?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (ident, term) = row?;
            let folded = casefold(&term);
            if folded.contains(&needle) || casefold(&term.replace('_', " ")).contains(&needle) {
                let r = if needle == folded { 2 } else { 0 };
                match at.get(&ident) {
                    Some(&i) => work_hits[i].1 = r, // 旧代码是 dict 赋值：后面的行覆盖前面的
                    None => {
                        at.insert(ident.clone(), work_hits.len());
                        work_hits.push((ident, r));
                    }
                }
            }
        }
    }
    {
        let mut stmt = c.prepare(&format!(
            "WITH RECURSIVE tree(id) AS (SELECT id FROM {ENTITIES} WHERE id=? AND kind='work'
 UNION SELECT e.id FROM {ENTITIES} e JOIN tree t ON e.parent_id=t.id WHERE e.kind='work') SELECT id FROM {ENTITIES} WHERE kind='character' AND work_id IN (SELECT id FROM tree)"
        ))?;
        for (ident, r) in &work_hits {
            let rows = stmt.query_map([ident], |row| row.get::<_, String>(0))?;
            for character in rows {
                raise(&mut hits, character?, *r);
            }
        }
    }
    {
        let mut stmt = c.prepare(&format!(
            "SELECT parent_id,variant FROM {ENTITIES} WHERE parent_id IS NOT NULL AND variant<>''"
        ))?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (ident, variant) = row?;
            if let Some(translated) = skin_zh(&variant) {
                let value = casefold(&translated);
                if value.contains(&needle) {
                    raise(&mut hits, ident, rank(&value, &needle));
                }
            }
        }
    }
    if let Some((parent, skin)) = query.rsplit_once(" · ") {
        let left = self::hits(c, parent)?;
        let right = self::hits(c, skin)?;
        for ident in left.keys().filter(|k| right.contains_key(*k)) {
            raise(&mut hits, ident.clone(), 1);
        }
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn casefold_follows_python() {
        assert_eq!(casefold("Straße"), "strasse");
        assert_eq!(casefold("ΣΑΣ"), "σασ");
        assert_eq!(casefold("ﬁ"), "fi");
        assert_eq!(casefold("Ｂ名"), "ｂ名");
        assert_eq!(casefold("İ"), "i\u{307}");
    }

    #[test]
    fn ranks_and_the_combined_query() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE terms(id INTEGER PRIMARY KEY, key TEXT, kind TEXT);
             CREATE TABLE term_tree(term_id INTEGER PRIMARY KEY, parent_id TEXT, merged_into TEXT, work_id TEXT,
               work_text TEXT, variant TEXT, artist TEXT, label_only INTEGER, mascot INTEGER, mascot_origin TEXT,
               original INTEGER, seq INTEGER);
             CREATE TABLE term_links(system TEXT, key TEXT, term_id INTEGER, current INTEGER, source TEXT, added_at TEXT);
             CREATE TABLE term_names(term_id INTEGER, value TEXT);
             CREATE TABLE term_name_display(entity TEXT, skin_only INTEGER, name_source TEXT, name TEXT);
             INSERT INTO term_name_display VALUES ('a',0,'builtin','离线中文名');
             CREATE TABLE term_key_names(kind TEXT, key TEXT, lang TEXT, source TEXT, value TEXT);
             CREATE TABLE term_key_aliases(kind TEXT, key TEXT, value TEXT, position INTEGER);
             INSERT INTO terms VALUES (1,'a','character'),(2,'w','work'),(3,'s','skin');
             INSERT INTO term_tree(term_id, parent_id, work_id, work_text, variant, artist, seq)
             VALUES (2,NULL,NULL,'','','',1),(1,NULL,'w','blue_archive','','',2),(3,'a',NULL,'','swimsuit','',3);
             INSERT INTO term_links VALUES ('danbooru','alice_(blue_archive)',1,1,'',''),('danbooru','ip_tag',2,1,'','');
             INSERT INTO term_names VALUES (1,'爱丽丝');",
        )
        .unwrap();
        assert_eq!(hits(&c, "离线中文名").unwrap().get("a"), Some(&2));
        let h = hits(&c, "爱丽丝").unwrap();
        assert_eq!(h.get("a"), Some(&2));
        assert_eq!(hits(&c, "ALICE").unwrap().get("a"), Some(&2)); // 原 tag 是开头（1），可读形式 "Alice" 完全相同（2）：取大
        assert_eq!(hits(&c, "ali").unwrap().get("a"), Some(&1));
        assert_eq!(hits(&c, "蔚蓝").unwrap().get("a"), Some(&1));
        assert_eq!(hits(&c, "泳装").unwrap().get("a"), Some(&2));
        assert_eq!(hits(&c, "ip_tag").unwrap().get("a"), Some(&2)); // 作品链接命中：子树下的角色
        assert_eq!(hits(&c, "丽丝 · 泳装").unwrap().get("a"), Some(&1));
        assert!(hits(&c, "没有").unwrap().is_empty());
    }
}
