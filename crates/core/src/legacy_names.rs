//! 名字的核心数据（docs/TERMS_NAMES.md）：按身份的名字 `term_names`、按文字键的显示名 `term_key_names` 与别名
//! `term_key_aliases`，以及由它们算出的显示投影的维护（`after_write`、`refresh_projection`）。
//!
//! 核心第 8 版（#201）起这三张表是唯一权威，名字命令直接写它们（`character_names`、`names_commands`）。原先由核心维护的
//! 兼容表 `names`、`display_names`、`name_aliases` 在升级时照旧表同步一次、逐行核对后删除（`retire_names`、
//! `retire_display_names`、`retire_aliases`）。
use crate::character_entities::ENTITIES;
use crate::character_links::LINKS;
use crate::{Error, Result};
use rusqlite::Connection;
use serde_json::{json, Map, Value};

pub const TABLES: &str = "
CREATE TABLE IF NOT EXISTS term_key_names(
 kind TEXT NOT NULL, key TEXT NOT NULL, lang TEXT NOT NULL, source TEXT NOT NULL, value TEXT NOT NULL,
 PRIMARY KEY(kind,key,lang,source,value)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS term_key_aliases(
 kind TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, position INTEGER NOT NULL,
 PRIMARY KEY(kind,key,value)) WITHOUT ROWID;
";

#[derive(Default, Debug)]
pub struct Report {
    pub preflight: Vec<Value>,
    pub changed: Map<String, Value>,
    pub tables: Map<String, Value>,
    pub committed: bool,
}

impl Report {
    pub fn clean(&self) -> bool {
        self.preflight.is_empty()
            && self.tables.values().all(|t| t["missing"].as_i64() == Some(0) && t["extra"].as_i64() == Some(0))
    }

    pub fn to_json(&self) -> Value {
        json!({"preflight": self.preflight, "changed": self.changed, "tables": self.tables, "committed": self.committed})
    }
}

fn count(c: &Connection, sql: &str) -> Result<i64> {
    Ok(c.query_row(&format!("SELECT count(*) FROM ({sql})"), [], |r| r.get(0))?)
}

fn samples(c: &Connection, sql: &str) -> Result<Vec<Value>> {
    let mut stmt = c.prepare(&format!("SELECT * FROM ({sql}) LIMIT 5"))?;
    let n = stmt.column_count();
    let rows = stmt.query_map([], |r| {
        (0..n)
            .map(|i| {
                Ok(match r.get_ref(i)? {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(v) => json!(v),
                    rusqlite::types::ValueRef::Real(v) => json!(v),
                    rusqlite::types::ValueRef::Text(v) => json!(String::from_utf8_lossy(v)),
                    rusqlite::types::ValueRef::Blob(_) => json!("<blob>"),
                })
            })
            .collect::<rusqlite::Result<Vec<Value>>>()
            .map(Value::Array)
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 核心第 8 版（#201）退役一张旧兼容表。旧表不在时什么都不做，返回 None。旧表还在时（升级前的库）它在第 7 版里是
/// 权威：先照它整表同步核心表（`sync`），再按 `columns` 双向逐行核对；一致就删掉旧表。返回仍不一致的行数。
fn retire(c: &Connection, legacy: &str, core: &str, columns: &str, sync: &str) -> Result<Option<i64>> {
    let exists: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_master WHERE type = 'table' AND name = ?1)",
        [legacy],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(None);
    }
    c.execute_batch(sync)?;
    let (old, new) = (format!("SELECT {columns} FROM {legacy}"), format!("SELECT {columns} FROM {core}"));
    let differs = count(c, &format!("{old} EXCEPT {new}"))? + count(c, &format!("{new} EXCEPT {old}"))?;
    if differs == 0 {
        c.execute_batch(&format!("DROP TABLE {legacy}"))?;
    }
    Ok(Some(differs))
}

/// 核心第 8 版：`term_key_aliases` 成为别名的唯一权威，旧表 `name_aliases` 同步、核对后删掉（`retire`）。
pub(crate) fn retire_aliases(c: &Connection) -> Result<()> {
    let sync = "DELETE FROM term_key_aliases
         WHERE (kind, key, value, position) NOT IN (SELECT kind, key, value, position FROM name_aliases);
         INSERT INTO term_key_aliases
         SELECT kind, key, value, position FROM name_aliases WHERE true
         ON CONFLICT(kind, key, value) DO UPDATE SET position = excluded.position
         WHERE position <> excluded.position;";
    if let Some(differs @ 1..) = retire(c, "name_aliases", "term_key_aliases", "kind, key, value, position", sync)? {
        return Err(Error::Schema(format!("旧别名表与词条核心不一致，没有升级：{differs} 行")));
    }
    Ok(())
}

/// 核心第 8 版：`term_key_names` 成为按文字键的显示名的唯一权威，旧表 `display_names` 同步、核对后删掉（`retire`）。
/// 读显示名的结果与行的先后无关（`names_view::choose_key`），所以旧表的插入顺序不用保留。
pub(crate) fn retire_display_names(c: &Connection) -> Result<()> {
    let sync = "DELETE FROM term_key_names
         WHERE (kind, key, lang, source, value) NOT IN (SELECT kind, key, lang, source, value FROM display_names);
         INSERT OR IGNORE INTO term_key_names SELECT kind, key, lang, source, value FROM display_names;";
    if let Some(differs @ 1..) = retire(c, "display_names", "term_key_names", "kind, key, lang, source, value", sync)? {
        return Err(Error::Schema(format!("旧显示名表与词条核心不一致，没有升级：{differs} 行")));
    }
    Ok(())
}

/// 核心第 8 版：`term_names` 成为角色、皮肤、作品名字的唯一权威（画师的名字本来就在这里）。所有库都给它加上名字的
/// 先后 `seq`（见 `character_names`）。旧表 `names` 还在时（升级前的库）它在第 7 版里是权威，核心每次写都照它同步
/// 镜像：先照它整表同步一次（之前版本留下的过期镜像随之补上），再逐行核对；一致就按旧表的 rowid 填 `seq`，删掉
/// 旧表。返回核对报告；不干净时什么都没删，调用方拒绝升级。
pub(crate) fn retire_names(c: &Connection) -> Result<Report> {
    let has_seq: bool =
        c.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('term_names') WHERE name = 'seq')", [], |r| {
            r.get(0)
        })?;
    if !has_seq {
        c.execute_batch(
            "ALTER TABLE term_names ADD COLUMN seq INTEGER;
             CREATE INDEX term_names_seq ON term_names(seq);",
        )?;
    }
    let mut r = Report::default();
    let exists: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_master WHERE type = 'table' AND name = 'names')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(r);
    }
    // 主数据补到与旧表一致：多的删、少的加（整行比较，首选、顺序变了就是一删一加）。只动 `term_tree` 里的词条。
    let in_tree = "term_id IN (SELECT term_id FROM term_tree)";
    let removed = c.execute(
        &format!(
            "DELETE FROM term_names
             WHERE {in_tree} AND (term_id, lang, value, source, role, preferred, position) NOT IN (
               SELECT t.id, n.lang, n.value, n.source, n.role, n.preferred, n.position
               FROM names n JOIN terms t ON t.key = n.entity_id)"
        ),
        [],
    )?;
    let added = c.execute(
        "INSERT INTO term_names(term_id, lang, value, source, role, preferred, position)
         SELECT t.id, n.lang, n.value, n.source, n.role, n.preferred, n.position
         FROM names n JOIN terms t ON t.key = n.entity_id JOIN term_tree r ON r.term_id = t.id
         WHERE NOT EXISTS(
           SELECT 1 FROM term_names x
           WHERE x.term_id = t.id AND x.role = n.role AND x.lang = n.lang AND x.source = n.source
             AND x.value = n.value AND x.preferred = n.preferred AND x.position = n.position)
         ON CONFLICT(term_id, role, lang, source, value) DO UPDATE SET
           preferred = excluded.preferred, position = excluded.position",
        [],
    )?;
    r.changed.insert("names".into(), json!(removed + added));
    let old = "SELECT entity_id, lang, value, source, role, preferred, position FROM names";
    let new = "SELECT t.key, n.lang, n.value, n.source, n.role, n.preferred, n.position
               FROM term_names n JOIN terms t ON t.id = n.term_id JOIN term_tree r ON r.term_id = t.id";
    let (missing_sql, extra_sql) = (format!("{old} EXCEPT {new}"), format!("{new} EXCEPT {old}"));
    let (missing, extra) = (count(c, &missing_sql)?, count(c, &extra_sql)?);
    let mut entry = json!({"old": count(c, old)?, "core": count(c, new)?, "missing": missing, "extra": extra});
    if missing > 0 {
        entry["missing_samples"] = json!(samples(c, &missing_sql)?);
    }
    if extra > 0 {
        entry["extra_samples"] = json!(samples(c, &extra_sql)?);
    }
    r.tables.insert("names".into(), entry);
    if !r.clean() {
        return Ok(r);
    }
    c.execute_batch(&format!(
        "UPDATE term_names SET seq = (
           SELECT n.rowid FROM names n JOIN terms t ON t.key = n.entity_id
           WHERE t.id = term_names.term_id AND n.role = term_names.role AND n.lang = term_names.lang
             AND n.source = term_names.source AND n.value = term_names.value)
         WHERE {in_tree};
         DROP TABLE names;"
    ))?;
    Ok(r)
}

/// 名字版本 `meta.names_version`：这几张表每改一行就加一，显示投影据此判断是否过期（`projection_current`）。由应用层
/// 建库时装上（不在的表跳过）；核心第 8 版在旧表退役之后把 `entities`、`links`、`names`、`display_names` 上的四组换到
/// `term_tree`、`terms`、`term_links`、`term_names`、`term_key_names` 上（`versioned` 为真时调用 `version_triggers`）。
/// - `term_tree` 只在身份的列变化时计数，作品复核三列不算（原来在 `work_review` 里，不计数）；
/// - `terms` 只算角色、皮肤、作品改类型（原来 `entities.kind` 的变化）；
/// - `term_links`、`term_names` 只算角色、皮肤、作品（`term_tree` 里的词条）的行，画师的不算，与原来只数 `links`、
///   `names` 相同。
pub const VERSIONED: [&str; 5] = ["term_tree", "terms", "term_links", "term_names", "term_key_names"];

/// `term_tree` 里身份的列（原来 `entities` 的列）：改了它们才算身份变化。
const IDENTITY_COLUMNS: [&str; 10] = [
    "parent_id",
    "merged_into",
    "work_id",
    "work_text",
    "variant",
    "artist",
    "label_only",
    "mascot",
    "mascot_origin",
    "original",
];
/// 角色、皮肤、作品改类型（`terms` 上的触发器只看这个）。
const KIND_CHANGED: &str = "OLD.kind IS NOT NEW.kind AND NEW.id IN (SELECT term_id FROM term_tree)";

/// 这个库装过名字版本触发器（应用层建的库）。
pub(crate) fn versioned(c: &Connection) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_master WHERE type = 'trigger' AND name GLOB 'names_version_*')",
        [],
        |r| r.get(0),
    )?)
}

/// `term_names` / `term_links` 的一行是不是角色、皮肤、作品的（触发器里用）。
fn in_tree(event: &str) -> &'static str {
    match event {
        "INSERT" => "NEW.term_id IN (SELECT term_id FROM term_tree)",
        "DELETE" => "OLD.term_id IN (SELECT term_id FROM term_tree)",
        _ => "OLD.term_id IN (SELECT term_id FROM term_tree) OR NEW.term_id IN (SELECT term_id FROM term_tree)",
    }
}

pub fn version_triggers(c: &Connection) -> Result<()> {
    for table in VERSIONED {
        let physical = crate::human_conclusions::visibility::storage(c, table)?;
        let exists: bool =
            c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)", [physical], |r| {
                r.get(0)
            })?;
        if !exists {
            continue;
        }
        for event in ["INSERT", "UPDATE", "DELETE"] {
            let when = match (table, event) {
                ("terms", "UPDATE") => format!(" WHEN {KIND_CHANGED}"),
                ("terms", _) => continue,
                ("term_tree", "UPDATE") => format!(" WHEN {}", changed(&IDENTITY_COLUMNS)),
                ("term_names" | "term_links", _) => format!(" WHEN {}", in_tree(event)),
                _ => String::new(),
            };
            c.execute_batch(&format!(
                "CREATE TRIGGER IF NOT EXISTS names_version_{table}_{} AFTER {event} ON {physical}{when}\n                        BEGIN UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='names_version'; END",
                event.to_ascii_lowercase()
            ))?;
        }
    }
    Ok(())
}

pub const NAMES_IN_CORE: &str = "terms_names";
/// 显示投影已同步的 `names_version`（`VERSIONED` 几张表的触发器递增它）。
const PROJECTED: &str = "names_projected";
/// 事务中途重建过显示投影时的版本，避免提交时再次重建（`refresh_projection`）。
const DISPLAY_PROJECTED: &str = "names_display_projected";

/// 内置名称规则或词表变更时，在追踪写入前让投影失效。提交时按当前输入重建两张投影表，即使图库里的名字行没有变化。
pub fn invalidate_display(c: &Connection) -> Result<()> {
    c.execute("DELETE FROM meta WHERE key IN (?1,?2)", [PROJECTED, DISPLAY_PROJECTED])?;
    Ok(())
}

pub fn names_in_core(c: &Connection) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE key=?1 AND value='core')", [NAMES_IN_CORE], |r| r.get(0))?)
}

/// 显示投影与名字的兼容表一致：名字已在核心，最近一次改名字相关数据的是核心写事务，提交时维护过投影（#105）。读侧据此
/// 直接读投影；不一致时（例如有写入绕过了 `schema::commit_writing`）调用方应改为现算。
pub fn projection_current(c: &Connection) -> Result<bool> {
    if !names_in_core(c)? {
        return Ok(false);
    }
    let version = meta(c, "names_version")?;
    Ok(version.is_some() && version == meta(c, PROJECTED)?)
}

fn meta(c: &Connection, key: &str) -> Result<Option<String>> {
    use rusqlite::OptionalExtension;
    Ok(c.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0)).optional()?)
}

/// 只在当前连接的写事务内追踪名字变化；旧投影过期（事务开始时 `names_version` 与 `names_projected` 不同）时退回完整
/// 重建。TEMP 表与触发器不改变持久 schema，事务回滚会一并撤销追踪状态。
///
/// 身份或链接变化时（#104 A4），显示投影要同时按变化前、变化后两侧求依赖闭包：BEFORE 触发器在一个身份第一次被改动
/// 时，把它在事务开始时的行（`names_old_entities`，按 `character_entities::ENTITIES` 的形状，`present=0` 表示原来
/// 没有）和当前链接（`names_old_links`，取过的身份记在 `names_old_links_seen`）存下来。`INSERT OR REPLACE` 顶掉的行
/// 不会触发 DELETE 触发器，所以插入前也先存下会被顶掉的行（按 `term_tree` 的主键、`term_links (system,key)`）。只改与
/// 名字无关的列（如作品复核、`term_links.source`）不记。`names_dirty_full` 只剩值 2：事务开始时投影已过期，提交时
/// 整表重建。
pub fn track(c: &Connection) -> Result<()> {
    if !names_in_core(c)? {
        return Ok(());
    }
    c.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS names_dirty_entities(id TEXT PRIMARY KEY);
        CREATE TEMP TABLE IF NOT EXISTS names_dirty_keys(kind TEXT,key TEXT,PRIMARY KEY(kind,key));
        CREATE TEMP TABLE IF NOT EXISTS names_dirty_full(value INTEGER PRIMARY KEY);
        CREATE TEMP TABLE IF NOT EXISTS names_old_entities(id TEXT PRIMARY KEY,present INTEGER NOT NULL,kind TEXT,
         parent_id TEXT,variant TEXT,merged_into TEXT,original INTEGER,work TEXT);
        CREATE TEMP TABLE IF NOT EXISTS names_old_links_seen(id TEXT PRIMARY KEY);
        CREATE TEMP TABLE IF NOT EXISTS names_old_links(system TEXT,key TEXT,entity_id TEXT,current INTEGER,added_at TEXT);
        CREATE TEMP TABLE IF NOT EXISTS names_scope_ids(id TEXT PRIMARY KEY);
        CREATE TEMP TABLE IF NOT EXISTS names_scope_keys(kind TEXT,key TEXT,PRIMARY KEY(kind,key));
        DELETE FROM names_dirty_entities; DELETE FROM names_dirty_keys;
        DELETE FROM names_dirty_full; DELETE FROM names_old_entities; DELETE FROM names_old_links_seen;
        DELETE FROM names_old_links; DELETE FROM names_scope_ids; DELETE FROM names_scope_keys;",
    )?;
    if meta(c, "names_version")? != meta(c, PROJECTED)? {
        c.execute("INSERT INTO names_dirty_full VALUES (2)", [])?;
    }
    structure_triggers(c)?;
    // 外层 UPSERT 的冲突策略可能覆盖触发器的 OR IGNORE，用存在性条件避免重复插入。
    for table in VERSIONED {
        for event in ["INSERT", "UPDATE", "DELETE"] {
            let mut body = String::new();
            if table == "terms" {
                if event == "UPDATE" {
                    body.push_str(
                        "INSERT INTO names_dirty_entities SELECT NEW.key
                         WHERE NOT EXISTS(SELECT 1 FROM names_dirty_entities WHERE id=NEW.key);",
                    );
                }
            } else if matches!(table, "term_tree" | "term_links") {
                // 链接的变化由 BEFORE 触发器记下（`names_old_links_seen`），这里只记改过的身份。
                if table == "term_tree" {
                    for side in ["old", "new"] {
                        if (event == "INSERT" && side == "old") || (event == "DELETE" && side == "new") {
                            continue;
                        }
                        body.push_str(&format!(
                            "INSERT INTO names_dirty_entities SELECT t.key FROM main.terms t
                             WHERE t.id={side}.term_id AND NOT EXISTS(SELECT 1 FROM names_dirty_entities WHERE id=t.key);"
                        ));
                    }
                }
            } else {
                for side in ["old", "new"] {
                    if (event == "INSERT" && side == "old") || (event == "DELETE" && side == "new") {
                        continue;
                    }
                    if table == "term_names" {
                        // 只记角色、皮肤、作品的名字（画师的名字也在这张表里，与显示投影无关）。
                        body.push_str(&format!(
                            "INSERT INTO names_dirty_entities SELECT t.key FROM main.terms t JOIN main.term_tree r ON r.term_id=t.id
                             WHERE t.id={side}.term_id AND NOT EXISTS(SELECT 1 FROM names_dirty_entities WHERE id=t.key);"
                        ));
                    } else {
                        body.push_str(&format!(
                            "INSERT INTO names_dirty_keys SELECT {side}.kind,{side}.key WHERE NOT EXISTS(SELECT 1 FROM names_dirty_keys WHERE kind={side}.kind AND key={side}.key);"
                        ));
                    }
                }
            }
            let when = match (table, event) {
                ("term_tree", "UPDATE") => format!(" WHEN {}", changed(&TRACKED_COLUMNS)),
                ("terms", "UPDATE") => format!(" WHEN {KIND_CHANGED}"),
                ("term_links", "UPDATE") => format!(" WHEN {}", changed(&LINK_COLUMNS)),
                _ => String::new(),
            };
            if body.is_empty() {
                continue;
            }
            let physical = crate::human_conclusions::visibility::storage(c, table)?;
            c.execute_batch(&format!(
                "CREATE TEMP TRIGGER IF NOT EXISTS track_names_{table}_{event}
                AFTER {event} ON main.{physical}{when} BEGIN {body} END;"
            ))?;
        }
    }
    Ok(())
}

/// 显示投影读到的身份的列（`names_view::View`）在 `term_tree` 里的那些（`kind` 在 `terms` 上，另有触发器）。
const TRACKED_COLUMNS: [&str; 5] = ["parent_id", "variant", "merged_into", "original", "work_text"];
/// 显示投影读到的 `term_links` 列（身份是 `term_id`，系统名是存储里的写法）。
const LINK_COLUMNS: [&str; 5] = ["system", "key", "term_id", "current", "added_at"];

fn changed(columns: &[&str]) -> String {
    columns.iter().map(|col| format!("OLD.{col} IS NOT NEW.{col}")).collect::<Vec<_>>().join(" OR ")
}

/// 变化前的身份和链接（见 `track`）。
fn structure_triggers(c: &Connection) -> Result<()> {
    // 一行身份第一次被改动时存下它原来的样子，后面的改动不覆盖；`present=0`：事务开始时没有这一行。
    let keep = |rows: &str| {
        format!(
            "INSERT INTO names_old_entities SELECT id,1,kind,parent_id,variant,merged_into,original,work
             FROM {ENTITIES} WHERE ({rows}) AND id NOT IN (SELECT id FROM names_old_entities);"
        )
    };
    let absent = "INSERT INTO names_old_entities(id,present) SELECT t.key,0 FROM main.terms t
        WHERE t.id=NEW.term_id AND NOT EXISTS(SELECT 1 FROM names_old_entities WHERE id=t.key);";
    let entity_bodies = [
        ("term_tree", "INSERT", String::new(), format!("{}{absent}", keep("term_id=NEW.term_id"))),
        (
            "term_tree",
            "UPDATE",
            format!(" WHEN {}", changed(&TRACKED_COLUMNS)),
            format!("{}{absent}", keep("term_id IN (OLD.term_id,NEW.term_id)")),
        ),
        ("term_tree", "DELETE", String::new(), keep("term_id=OLD.term_id")),
        ("terms", "UPDATE", format!(" WHEN {KIND_CHANGED}"), keep("term_id=OLD.id")),
    ];
    for (table, event, when, body) in entity_bodies {
        let physical = crate::human_conclusions::visibility::storage(c, table)?;
        c.execute_batch(&format!(
            "CREATE TEMP TRIGGER IF NOT EXISTS track_names_old_{table}_{event}
            BEFORE {event} ON main.{physical}{when} BEGIN {body} END;"
        ))?;
    }
    // 一个身份的链接第一次被改动时存下它当时全部的链接，包括会被 REPLACE 顶掉的那一行的主人。
    let keep_links = |owner: &str| {
        format!(
            "INSERT INTO names_old_links SELECT l.system,l.key,l.entity_id,l.current,l.added_at
             FROM ({owner}) o JOIN {LINKS} l ON l.entity_id=o.id WHERE o.id NOT IN (SELECT id FROM names_old_links_seen);
             INSERT INTO names_old_links_seen SELECT DISTINCT o.id FROM ({owner}) o
             WHERE o.id IS NOT NULL AND o.id NOT IN (SELECT id FROM names_old_links_seen);"
        )
    };
    // 链接属于哪个身份（画师等别的词条的链接没有身份，不记）。
    let owner = |side: &str| {
        format!(
            "SELECT t.key AS id FROM main.terms t JOIN main.term_tree r ON r.term_id=t.id WHERE t.id={side}.term_id"
        )
    };
    let holder = "SELECT t.key AS id FROM main.term_links x JOIN main.terms t ON t.id=x.term_id
                  JOIN main.term_tree r ON r.term_id=x.term_id WHERE x.system=NEW.system AND x.key=NEW.key";
    let link_bodies = [
        ("INSERT", String::new(), format!("{}{}", keep_links(&owner("NEW")), keep_links(holder))),
        (
            "UPDATE",
            format!(" WHEN {}", changed(&LINK_COLUMNS)),
            format!("{}{}{}", keep_links(&owner("OLD")), keep_links(&owner("NEW")), keep_links(holder)),
        ),
        ("DELETE", String::new(), keep_links(&owner("OLD"))),
    ];
    for (event, when, body) in link_bodies {
        c.execute_batch(&format!(
            "CREATE TEMP TRIGGER IF NOT EXISTS track_names_old_links_{event}
            BEFORE {event} ON main.term_links{when} BEGIN {body} END;"
        ))?;
    }
    Ok(())
}

/// 本事务的名字相关改动都被追踪到了：显示投影按记下的身份和键增量维护（结构变化也一样，#104）。若开始时投影已经
/// 过期（值 2），来源未被本事务完整追踪，整表重建。
fn tracked_master(c: &Connection) -> Result<bool> {
    let exists: bool =
        c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_temp_master WHERE name='names_dirty_full')", [], |r| r.get(0))?;
    Ok(exists
        && !c.query_row("SELECT EXISTS(SELECT 1 FROM names_dirty_full WHERE value=2)", [], |r| r.get::<_, bool>(0))?)
}

/// 每个核心写事务提交前（schema::commit_writing）：名字已在核心、且这个事务改到了名字相关的数据，就在同一事务里
/// 维护显示投影。名字行使用依赖闭包，结构变更 / 过期状态回退整表。名字本身由命令直接写 `term_*`（核心第 8 版）。
pub fn after_write(c: &Connection) -> Result<()> {
    if !names_in_core(c)? {
        return Ok(());
    }
    let version = meta(c, "names_version")?;
    if version.is_some() && version == meta(c, PROJECTED)? {
        return Ok(());
    }
    refresh_projection(c, &mut None)?;
    c.execute(
        "INSERT OR REPLACE INTO meta(key,value) VALUES (?1,?2)",
        rusqlite::params![PROJECTED, version.unwrap_or_default()],
    )?;
    Ok(())
}

/// 对照核对的开关（#102；#38 2026-10-12 用户决定）：只在测试特性 `verify-incremental` 下编译。设置了环境变量
/// `ILLUSTASH_TEST_VERIFY_INCREMENTAL`（一个记录文件的路径）时，每次走增量路径都在同一事务里核对「增量结果 = 整表
/// 重建」，并往记录文件追加一行核对的种类，测试据此证明核对确实执行过。产品构建里恒为关闭。
#[cfg(feature = "verify-incremental")]
pub fn verifying() -> bool {
    std::env::var_os(VERIFY_ENV).is_some_and(|v| !v.is_empty())
}
#[cfg(not(feature = "verify-incremental"))]
pub fn verifying() -> bool {
    false
}
#[cfg(feature = "verify-incremental")]
const VERIFY_ENV: &str = "ILLUSTASH_TEST_VERIFY_INCREMENTAL";

/// 记下一次已通过的核对（见 `verifying`）。
#[cfg(feature = "verify-incremental")]
pub fn record_verified(kind: &str) -> Result<()> {
    use std::io::Write;
    let Some(path) = std::env::var_os(VERIFY_ENV).filter(|v| !v.is_empty()) else { return Ok(()) };
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    f.write_all(format!("{kind}\n").as_bytes())?;
    Ok(())
}
#[cfg(not(feature = "verify-incremental"))]
pub fn record_verified(_kind: &str) -> Result<()> {
    Ok(())
}

/// 局部维护显示投影之后、同一事务里（#103）：两张投影表与保存点里整表重建的结果逐行逐列相同；随后回滚保存点。
/// 按 `names_view::Maintained` 分别记为 `projection`（只改名字行）、`projection_structure`（改过身份或链接，#104）、
/// `projection_whole`（退回了整表重建，核对只证明整表重建可重复）。
fn verify_projection(c: &Connection, kind: &str) -> Result<()> {
    if !verifying() {
        return Ok(());
    }
    fn rows(c: &Connection) -> Result<Vec<Vec<rusqlite::types::Value>>> {
        let mut all = Vec::new();
        for sql in [
            "SELECT 'display',* FROM term_name_display ORDER BY entity,skin_only",
            "SELECT 'choice',* FROM term_name_choice ORDER BY kind,key",
        ] {
            let mut st = c.prepare(sql)?;
            let n = st.column_count();
            for row in st.query_map([], |r| (0..n).map(|i| r.get(i)).collect::<rusqlite::Result<Vec<_>>>())? {
                all.push(row?);
            }
        }
        Ok(all)
    }
    let scoped = rows(c)?;
    c.execute_batch("SAVEPOINT verify_projection")?;
    let full = crate::names_view::rebuild(c).and_then(|_| rows(c));
    c.execute_batch("ROLLBACK TO verify_projection; RELEASE verify_projection")?;
    let full = full?;
    if scoped != full {
        let differs = scoped.iter().zip(&full).find(|(a, b)| a != b);
        return Err(Error::Conflict(format!(
            "局部显示投影与整表重建不一致：局部 {scoped} 行、整表 {full} 行，第一处不同：{differs}",
            scoped = scoped.len(),
            full = full.len(),
            differs = format_args!("{differs:?}")
        )));
    }
    record_verified(kind)
}

/// 事务中途要读显示名（核心生成历史文案，character_labels）：这个事务改过名字相关的数据就先重建显示投影，不记
/// `names_projected`：显示版本单独记录，提交时只在版本再次变化时重建投影。
/// `seen`：调用方上次重建时的版本，没变就不再重建。
pub fn refresh_projection(c: &Connection, seen: &mut Option<String>) -> Result<()> {
    let version = meta(c, "names_version")?;
    if version.is_some()
        && (version == *seen || version == meta(c, PROJECTED)? || version == meta(c, DISPLAY_PROJECTED)?)
    {
        return Ok(());
    }
    if tracked_master(c)? {
        let kind = match crate::names_view::rebuild_changed(c)? {
            crate::names_view::Maintained::Names => "projection",
            crate::names_view::Maintained::Structure => "projection_structure",
            crate::names_view::Maintained::Whole => "projection_whole",
        };
        verify_projection(c, kind)?;
    } else {
        crate::names_view::rebuild(c)?;
    }
    c.execute(
        "INSERT OR REPLACE INTO meta(key,value) VALUES (?1,?2)",
        rusqlite::params![DISPLAY_PROJECTED, version.clone().unwrap_or_default()],
    )?;
    *seen = version;
    Ok(())
}
