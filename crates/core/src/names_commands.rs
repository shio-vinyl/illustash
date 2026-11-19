//! 名字的写命令（docs/TERMS_NAMES.md 方案 N3）：`illustash-terms names-command --db <库>`，标准输入给一个 JSON
//! `{"op": …}`（或 `{"op": "batch", "ops": [...]}`），一个核心事务。NameUpdater（联网补名字）和相册标题的写入走这里，
//! 逐行照 tests/reference/web/names.py（#9 已删除） 的 `put`、`tag_entities`、`NameUpdater.replace` / `aliases` / `seed` 与 tests/reference/web/store.py（#9 已删除） 的
//! `Store.alias`。名字切到核心之后，旧表是兼容表（写保护），提交时由 `legacy_names::after_write` 补齐主数据、重建投影。
use crate::character_entities::ENTITIES;
use crate::character_links::{core_system, LINKS};
use crate::character_names::{alias, put_name, TERM_OF};
use crate::error::invalid;
use crate::schema::{begin_writing, commit_writing};
use crate::{names_view, Error, Result};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};

fn text<'a>(v: &'a Value, message: &str) -> Result<&'a str> {
    v.as_str().ok_or_else(|| Error::Invalid(message.into()))
}

/// 去掉首尾空白（空白集同 `character_names::strip`）后非空的字符串；其他（null、全空白）一律当作没有。
fn given(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| !s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)).is_empty())
}

/// `names.tag_entities`：名字存在这个 tag 下的活跃顶层角色身份，系统按具体程度排。
/// 已合并、皮肤（有父角色）、原创或作品实体不作为角色名写入目标。
pub fn tag_entities(c: &Connection, tag: &str) -> Result<Vec<String>> {
    let active = format!(
        "SELECT l.entity_id FROM {LINKS} l
         JOIN {ENTITIES} e ON e.id = l.entity_id
         WHERE l.core_system = ? AND l.key = ? AND l.current = 1
           AND e.merged_into IS NULL AND e.parent_id IS NULL
           AND e.kind NOT IN ('oc','work') AND coalesce(e.original,0) = 0
         ORDER BY e.seq"
    );
    for system in ["danbooru", "wd", "photos_album", "ccip"] {
        let ids: Vec<String> = c
            .prepare_cached(&active)?
            .query_map([core_system(system), tag], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        if !ids.is_empty() {
            return Ok(ids);
        }
    }
    Ok(c.prepare_cached(&format!(
        "SELECT l.entity_id FROM {LINKS} l
         JOIN {ENTITIES} e ON e.id = l.entity_id
         WHERE l.core_system = 'ccip_base' AND l.current = 1 AND json_extract(l.key, '$[0]') = ?
           AND e.merged_into IS NULL AND e.parent_id IS NULL
           AND e.kind NOT IN ('oc','work') AND coalesce(e.original,0) = 0
         ORDER BY e.seq"
    ))?
    .query_map([tag], |r| r.get(0))?
    .collect::<rusqlite::Result<_>>()?)
}

/// 作品文字（qualifier 链接）的身份。
static QUALIFIED: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!("SELECT l.entity_id FROM {LINKS} l WHERE l.core_system = 'qualifier' AND l.key = ?")
});

fn entities_for(c: &Connection, sql: &str, key: &str) -> Result<Vec<String>> {
    Ok(c.prepare_cached(sql)?.query_map([key], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}

/// `names.put`：按文字键的显示名。人物写进身份的 `names`；作品与皮肤词按文字存（作品同时写 qualifier 对应的作品身份）。
pub fn put(c: &Connection, kind: &str, key: &str, lang: &str, source: &str, value: &Value) -> Result<()> {
    if !matches!(lang, "zh" | "ja" | "en") || !names_view::PRIORITY.contains(&source) {
        return invalid("显示名来源或语言不正确");
    }
    if source == "danbooru" {
        return Ok(());
    }
    if kind == "character" {
        for entity in tag_entities(c, key)? {
            if let Some(v) = value.as_str() {
                put_name(c, &entity, lang, source, v, "display", 0, 0)?;
            }
        }
    } else if let Some(v) = given(value) {
        if kind == "work" {
            for entity in entities_for(c, &QUALIFIED, key)? {
                put_name(c, &entity, lang, source, v, "display", 0, 0)?;
            }
        }
        let v = v.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c));
        c.prepare_cached("INSERT OR IGNORE INTO term_key_names VALUES (?,?,?,?,?)")?
            .execute([kind, key, lang, source, v])?;
    }
    Ok(())
}

/// `NameUpdater.replace`：这个来源在这个键上的显示名整批替换（空列表 = 成功的空结果，旧的全部删掉）。
fn replace(c: &Connection, kind: &str, key: &str, source: &str, values: &[Value]) -> Result<()> {
    if kind == "character" {
        for entity in tag_entities(c, key)? {
            c.prepare_cached(&format!(
                "DELETE FROM term_names WHERE term_id = {TERM_OF} AND source = ? AND role = 'display'"
            ))?
            .execute([&entity, source])?;
        }
    } else {
        if kind == "work" {
            c.prepare_cached(
                "DELETE FROM term_names WHERE role = 'display' AND source = ? AND term_id IN (
                   SELECT x.term_id FROM term_links x JOIN term_tree r ON r.term_id = x.term_id
                   WHERE x.system = 'qualifier' AND x.key = ?)",
            )?
            .execute([source, key])?;
        }
        c.prepare_cached("DELETE FROM term_key_names WHERE kind=? AND key=? AND source=?")?
            .execute([kind, key, source])?;
    }
    for pair in values {
        let lang = text(&pair[0], "显示名来源或语言不正确")?;
        put(c, kind, key, lang, source, &pair[1])?;
    }
    Ok(())
}

/// `NameUpdater.aliases(fetch=True)` 写库的那一半：Danbooru wiki 的别名按顺序整批替换。
fn danbooru_aliases(c: &Connection, kind: &str, tag: &str, values: &[Value]) -> Result<()> {
    let values: Vec<&str> = values.iter().map(|v| text(v, "别名不正确")).collect::<Result<_>>()?;
    let write = |entity: &str| -> Result<()> {
        c.prepare_cached(&format!(
            "DELETE FROM term_names WHERE term_id = {TERM_OF} AND role = 'alias' AND source = 'danbooru'"
        ))?
        .execute([entity])?;
        for (i, value) in values.iter().enumerate() {
            put_name(c, entity, "", "danbooru", value, "alias", i as i64, 0)?;
        }
        Ok(())
    };
    if kind == "character" {
        for entity in tag_entities(c, tag)? {
            write(&entity)?;
        }
    } else {
        c.prepare_cached("DELETE FROM term_key_aliases WHERE kind=? AND key=?")?.execute([kind, tag])?;
        let mut insert = c.prepare_cached("INSERT INTO term_key_aliases VALUES (?,?,?,?)")?;
        for (i, value) in values.iter().enumerate() {
            insert.execute(rusqlite::params![kind, tag, value, i as i64])?;
        }
        if kind == "work" {
            for entity in entities_for(c, &QUALIFIED, tag)? {
                write(&entity)?;
            }
        }
    }
    Ok(())
}

fn seed_index(c: &Connection, entity: &str, legacy_key: Option<&str>, names: &Value) -> Result<bool> {
    let mut changed = c
        .prepare_cached(&format!(
            "DELETE FROM term_names WHERE term_id = {TERM_OF} AND role = 'display' AND source = 'index'"
        ))?
        .execute([entity])?
        != 0;
    for lang in ["zh", "ja", "en"] {
        if let Some(key) = legacy_key {
            put(c, "character", key, lang, "index", &names[lang])?;
        } else if let Some(value) = names[lang].as_str().filter(|v| !crate::character_names::strip(v).is_empty()) {
            put_name(c, entity, lang, "index", value, "display", 0, 0)?;
            changed = true;
        }
    }
    Ok(changed)
}

pub fn one(c: &Connection, op: &Value) -> Result<()> {
    let s = |k: &str| text(&op[k], "名字命令缺少参数");
    match s("op")? {
        "put" => put(c, s("kind")?, s("key")?, s("lang")?, s("source")?, &op["value"]),
        "replace" => {
            replace(c, s("kind")?, s("key")?, s("source")?, op["values"].as_array().map(Vec::as_slice).unwrap_or(&[]))
        }
        "danbooru_aliases" => {
            danbooru_aliases(c, s("kind")?, s("tag")?, op["values"].as_array().map(Vec::as_slice).unwrap_or(&[]))
        }
        // NameUpdater.character：Bangumi 信息框里的中文原名，作为排在最后的别名。
        "bangumi_alias" => {
            for entity in tag_entities(c, s("tag")?)? {
                put_name(c, &entity, "zh", "bangumi", s("value")?, "alias", 10000, 0)?;
            }
            Ok(())
        }
        // NameUpdater.seed：索引给的名字，先删掉这个身份原有的索引名。
        "seed_index" => seed_index(c, s("entity")?, Some(s("key")?), &op["names"]).map(|_| ()),
        // Store.alias（相册标题等）。
        "alias" => alias(c, s("entity")?, s("value")?, s("source")?, op["lang"].as_str().unwrap_or("")).map(|_| ()),
        other => Err(Error::Invalid(format!("未知名字命令：{other}"))),
    }
}

/// 一个核心事务执行一条或一批名字命令。
pub fn run(conn: &mut Connection, args: &Value) -> Result<Value> {
    write(conn, |c| {
        let ops: Vec<&Value> = match args["op"].as_str() {
            Some("batch") => args["ops"].as_array().map(|a| a.iter().collect()).unwrap_or_default(),
            _ => vec![args],
        };
        for op in &ops {
            one(c, op)?;
        }
        Ok(json!({"done": ops.len()}))
    })
}

/// 在同一个核心写事务内复核来源，再按已复核的身份替换索引名。返回本次提交是否写了索引名。
pub fn seed_checked(
    conn: &mut Connection,
    resolve: impl FnOnce(&Connection) -> Result<Vec<(String, Value)>>,
) -> Result<bool> {
    write(conn, |c| {
        let mut changed = false;
        for (entity, names) in resolve(c)? {
            changed |= seed_index(c, &entity, None, &names)?;
        }
        Ok(changed)
    })
}

fn write<T>(conn: &mut Connection, apply: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    crate::schema::require_current(conn)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if !crate::legacy_names::names_in_core(&tx)? {
        return Err(Error::Conflict("名字还没切到词条核心".into()));
    }
    begin_writing(&tx)?;
    let value = apply(&tx)?;
    // 别名镜像由提交时的 `legacy_names::after_write` 按受影响的键同步，一个事务只同步一次。
    commit_writing(tx)?;
    Ok(value)
}
