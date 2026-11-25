//! 身份与作品的结构命令：建身份、改类型、当作标签、归作品、作品编辑、
//! 作品证据（`works.apply_evidence`）。逐行照 tests/reference/web/store.py（#9 已删除）、tests/reference/web/works.py（#9 已删除）；作品的可读名由调用方给，
//! 当前显示名由核心读（character_labels）。
use crate::character_commands::{begin_action, finish_action, hybrid, require_entity, text};
use crate::character_entities::{self, New, ENTITIES, OF};
use crate::character_links::{check_free, LINKS};
use crate::character_names::TERM_OF;
use crate::character_profile::{new_id, require_work, set_work, tag_owner, work_for};
use crate::character_review::{OF as REVIEW_OF, OPEN as REVIEW_OPEN};
use crate::error::invalid;
use crate::{character_graph, character_labels, character_names, character_projection, Error, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

fn oc_fields(args: &Value) -> Result<(String, String)> {
    let kind = args["kind"].as_str().unwrap_or("");
    if !matches!(kind, "" | "oc") {
        return invalid("未知角色类型");
    }
    let artist = match &args["artist"] {
        Value::Null => "",
        Value::String(s) => s.trim(),
        _ => return invalid("画师名称不正确"),
    };
    if artist.chars().count() > 120 || (!artist.is_empty() && kind != "oc") {
        return invalid("画师名称不正确");
    }
    Ok((kind.to_owned(), artist.to_owned()))
}

/// 新身份尚无候选或人工归属；图重建已刷新受影响图片，不再对全库重复派生。
fn created(c: &Connection) -> Result<()> {
    if hybrid(c)? {
        character_graph::rebuild(c)?;
        character_projection::refresh_delayed(c)?;
    }
    Ok(())
}

/// `create_entity`（调用方已确认没有同名同链接的身份可复用；原创角色的「原创角色 N」也由调用方算好）。
fn create_entity(c: &Connection, args: &Value) -> Result<Value> {
    let (kind, artist) = oc_fields(args)?;
    let name = text(&args["name"], "角色名称应为 1–120 个字符")?.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return invalid("角色名称应为 1–120 个字符");
    }
    let id = new_id(&args["id"])?;
    let new = New { id: &id, kind: "character", artist: &artist, original: (kind == "oc") as i64, ..New::default() };
    character_entities::create(c, &new, "user")?;
    character_names::put_name(c, &id, text(&args["lang"], "缺少名字的语言")?, "user", name, "display", 0, 1)?;
    created(c)?;
    Ok(json!({"id": id}))
}

fn create_linked(c: &Connection, args: &Value) -> Result<Value> {
    let tag = text(&args["tag"], "请选择 Danbooru 角色 tag")?;
    if let Some(owner) = tag_owner(c, tag, None)? {
        return Ok(json!({"id": owner, "existing": true}));
    }
    let id = new_id(&args["id"])?;
    character_entities::create(c, &New { id: &id, kind: "character", ..New::default() }, "user")?;
    check_free(c, "danbooru", tag)?;
    c.execute(
        &format!("INSERT INTO term_links(system, key, term_id, source) VALUES ('danbooru', ?, {TERM_OF}, 'user')"),
        [tag, &id],
    )?;
    let work = work_for(
        c,
        tag,
        args["copyright"].as_str().unwrap_or(""),
        args["readable"].as_str().unwrap_or(""),
        &args["work_id"],
    )?;
    set_work(c, &id, work.as_deref())?;
    if let Some(alias) = args["alias"].as_str().filter(|a| !a.is_empty()) {
        character_names::put_name(c, &id, "", "user", alias, "alias", 0, 0)?;
    }
    created(c)?;
    Ok(json!({"id": id}))
}

pub const SHARED_OC: &str = "原创角色（未细分）";

fn shared_oc(c: &Connection, args: &Value) -> Result<Value> {
    let row: Option<String> = c
        .query_row(
            &format!("SELECT id FROM {ENTITIES} WHERE label_only=1 AND original=1 AND merged_into IS NULL ORDER BY seq LIMIT 1"),
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = row {
        return Ok(json!({"id": id, "existing": true}));
    }
    let id = new_id(&args["id"])?;
    let new = New { id: &id, kind: "character", original: 1, label_only: 1, ..New::default() };
    character_entities::create(c, &new, "user")?;
    character_names::put_name(c, &id, "zh", "user", SHARED_OC, "display", 0, 1)?;
    created(c)?;
    Ok(json!({"id": id}))
}

#[derive(Serialize, Deserialize)]
struct OriginalBefore {
    entity: String,
    original: i64,
    artist: String,
    work: String,
    work_id: Option<String>,
    pinned: Option<PinnedName>,
}

/// 固定显示名时也会清掉同语言其他来源的首选标记。只记录这次会改到的行。
#[derive(Serialize, Deserialize)]
struct PinnedName {
    lang: String,
    value: String,
    user_preferred: Option<i64>,
    preferred: Vec<(String, String)>,
}

impl PinnedName {
    fn snapshot(c: &Connection, entity: &str, shown: &str) -> Result<Self> {
        let lang = character_labels::lang_of(shown).to_owned();
        let value = character_names::strip(shown).to_owned();
        let user_preferred = c.query_row(
            &format!("SELECT preferred FROM term_names WHERE term_id={TERM_OF} AND role='display' AND lang=? AND source='user' AND value=?"),
            params![entity, lang, value],
            |r| r.get(0),
        ).optional()?;
        let preferred = c.prepare(&format!(
            "SELECT source,value FROM term_names WHERE term_id={TERM_OF} AND role='display' AND lang=? AND preferred=1 ORDER BY source,value"
        ))?.query_map(params![entity, lang], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(Self { lang, value, user_preferred, preferred })
    }

    fn restore(self, c: &Connection, entity: &str) -> Result<()> {
        if let Some(was) = self.user_preferred {
            c.execute(&format!(
                "UPDATE term_names SET preferred=?4 WHERE {OF} AND role='display' AND lang=?2 AND source='user' AND value=?3"
            ), params![entity, self.lang, self.value, was])?;
        } else {
            c.execute(&format!(
                "DELETE FROM term_names WHERE term_id={TERM_OF} AND role='display' AND lang=? AND source='user' AND value=?"
            ), params![entity, self.lang, self.value])?;
        }
        let mut restore = c.prepare(&format!(
            "UPDATE term_names SET preferred=1 WHERE term_id={TERM_OF} AND role='display' AND lang=? AND source=? AND value=? AND preferred=0"
        ))?;
        for (source, value) in self.preferred {
            restore.execute(params![entity, self.lang, source, value])?;
        }
        Ok(())
    }
}

fn set_kind(c: &Connection, args: &Value, history: bool) -> Result<Value> {
    let (kind, artist) = oc_fields(args)?;
    let (entity, parent) = require_entity(c, &args["entity"])?;
    if parent.is_some() {
        return invalid("只能标记角色本身，不能标记子标签");
    }
    let (mut before, label_only) = c.query_row(
        &format!("SELECT original,artist,work,work_id,label_only FROM {ENTITIES} WHERE id=?"),
        [&entity],
        |r| {
            Ok((
                OriginalBefore {
                    entity: entity.clone(),
                    original: r.get(0)?,
                    artist: r.get(1)?,
                    work: r.get(2)?,
                    work_id: r.get(3)?,
                    pinned: None,
                },
                r.get::<_, i64>(4)?,
            ))
        },
    )?;
    if kind != "oc" && label_only != 0 {
        return invalid("未细分的原创角色只能是原创角色");
    }
    let oc = kind == "oc";
    let mut out = json!({"id": entity, "kind": kind, "artist": artist});
    if history
        && before.original == oc as i64
        && before.artist == artist
        && (!oc || (before.work_id.is_none() && before.work.is_empty()))
    {
        return Ok(out);
    }
    let shown = (history || (oc && before.original != 1))
        .then(|| character_labels::Shown::new().name(c, &entity))
        .transpose()?;
    if oc && before.original != 1 {
        // 标成原创角色：把现在的显示名固定成用户名字（旧代码读 entity_name）。
        let shown = shown.as_deref().expect("标成原创时已读取名字");
        if history && !character_names::strip(shown).is_empty() {
            before.pinned = Some(PinnedName::snapshot(c, &entity, shown)?);
        }
        character_names::put_name(c, &entity, character_labels::lang_of(shown), "user", shown, "display", 0, 1)?;
    }
    let (work, work_id) = if oc { ("", None) } else { (before.work.as_str(), before.work_id.as_deref()) };
    write_kind(c, &entity, oc as i64, &artist, work, work_id)?;
    if history {
        let name = shown.expect("写历史已读取名字");
        let label =
            if oc { format!("{name} 已标为原创角色") } else { format!("{name} 不再标为原创角色") };
        let action = begin_action(c, &label)?;
        finish_action(c, action, "character_kind", vec![json!({"character_kind": before})])?;
        out["action"] = json!(action);
    }
    Ok(out)
}

fn write_kind(
    c: &Connection,
    entity: &str,
    original: i64,
    artist: &str,
    work: &str,
    work_id: Option<&str>,
) -> Result<()> {
    c.execute(
        &format!("UPDATE term_tree SET original=?2,artist=?3,work_text=?4,work_id=?5 WHERE {OF}"),
        params![entity, original, artist, work, work_id],
    )?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct MascotBefore {
    entity: String,
    mascot: i64,
    origin: Option<String>,
}

fn set_mascot(c: &Connection, args: &Value, history: bool) -> Result<Value> {
    let Some(mascot) = args["mascot"].as_bool() else { return invalid("参数不正确") };
    let (entity, parent) = require_entity(c, &args["entity"])?;
    if parent.is_some() {
        return invalid("只能标记角色本身，不能标记子标签");
    }
    let mut out = json!({"id": entity, "mascot": mascot});
    let before = if history {
        let (value, origin): (i64, Option<String>) =
            c.query_row(&format!("SELECT mascot,mascot_origin FROM term_tree WHERE {OF}"), [&entity], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
        if value == mascot as i64 && origin.as_deref() == Some("user") {
            return Ok(out);
        }
        Some(MascotBefore { entity: entity.clone(), mascot: value, origin })
    } else {
        None
    };
    write_mascot(c, &entity, mascot as i64, Some("user"))?;
    if let Some(before) = before {
        let name = character_labels::Shown::new().name(c, &entity)?;
        let label = if mascot {
            format!("{name} 不再算角色，只当标签")
        } else {
            format!("{name} 已恢复成角色")
        };
        let action = begin_action(c, &label)?;
        finish_action(c, action, "character_mascot", vec![json!({"character_mascot": before})])?;
        out["action"] = json!(action);
    }
    Ok(out)
}

fn write_mascot(c: &Connection, entity: &str, mascot: i64, origin: Option<&str>) -> Result<()> {
    c.execute(&format!("UPDATE term_tree SET mascot=?2,mascot_origin=?3 WHERE {OF}"), params![entity, mascot, origin])?;
    let uuids: Vec<String> = c
        .prepare(&format!(
            "SELECT uuid FROM family_evidence WHERE entity=?1 UNION
             SELECT s.uuid FROM assertions s JOIN {ENTITIES} x ON x.id=s.entity WHERE coalesce(x.parent_id,x.id)=?1"
        ))?
        .query_map([entity], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if !uuids.is_empty() {
        character_projection::evidence(c, Some(&uuids))?;
        character_projection::memberships(c, Some(&uuids))?;
    }
    Ok(())
}

pub(crate) fn undo_kind(c: &Connection, before: &Value) -> Result<()> {
    let snap: OriginalBefore =
        serde_json::from_value(before.clone()).map_err(|_| Error::invalid("原创角色快照不完整"))?;
    write_kind(c, &snap.entity, snap.original, &snap.artist, &snap.work, snap.work_id.as_deref())?;
    if let Some(pinned) = snap.pinned {
        pinned.restore(c, &snap.entity)?;
    }
    Ok(())
}

pub(crate) fn undo_mascot(c: &Connection, before: &Value) -> Result<()> {
    let snap: MascotBefore =
        serde_json::from_value(before.clone()).map_err(|_| Error::invalid("只当标签快照不完整"))?;
    write_mascot(c, &snap.entity, snap.mascot, snap.origin.as_deref())
}

fn set_character_work(c: &Connection, args: &Value) -> Result<Value> {
    let (entity, parent) = require_entity(c, &args["entity"])?;
    if parent.is_some() {
        return invalid("皮肤跟随角色的作品");
    }
    let work = crate::character_commands::work_arg(&args["work"])?;
    if let Some(w) = work {
        require_work(c, w)?;
        let original: i64 =
            c.query_row(&format!("SELECT original FROM {ENTITIES} WHERE id=?"), [&entity], |r| r.get(0))?;
        if original != 0 {
            return invalid("原创角色不能归属作品");
        }
    }
    set_work(c, &entity, work)?;
    Ok(json!({"id": entity, "work_id": work}))
}

// ---- 作品（tests/reference/web/works.py（#9 已删除）） ----

/// works.TREE：作品和它的全部子作品（UNION 防旧数据成环）。
fn descendants(c: &Connection, ident: &str) -> Result<Vec<String>> {
    require_work(c, ident)?;
    Ok(c.prepare(&format!(
        "WITH RECURSIVE tree(id) AS (SELECT id FROM {ENTITIES} WHERE id=? AND kind='work'
          UNION SELECT e.id FROM {ENTITIES} e JOIN tree t ON e.parent_id=t.id WHERE e.kind='work') SELECT id FROM tree"
    ))?
    .query_map([ident], |r| r.get(0))?
    .collect::<rusqlite::Result<_>>()?)
}

fn work_create(c: &Connection, args: &Value) -> Result<String> {
    let title = args["name"].as_str().map(str::trim).unwrap_or("");
    if title.is_empty() || title.chars().count() > 120 {
        return invalid("请输入 1–120 字的作品名");
    }
    let parent = args["parent"].as_str().filter(|p| !p.is_empty());
    if let Some(p) = parent {
        require_work(c, p)?;
    }
    let ident = new_id(&args["id"])?;
    character_entities::create(c, &New { id: &ident, kind: "work", parent_id: parent, ..New::default() }, "user")?;
    c.execute(REVIEW_OPEN, params![ident, "pending", "", 1])?;
    character_names::put_name(c, &ident, text(&args["lang"], "缺少名字的语言")?, "user", title, "display", 0, 1)?;
    if let Some(tag) = args["tag"].as_str().filter(|t| !t.is_empty()) {
        work_link(c, &ident, tag)?;
    }
    Ok(ident)
}

fn work_link(c: &Connection, ident: &str, tag: &str) -> Result<()> {
    require_work(c, ident)?;
    let tag = tag.trim();
    if tag.is_empty() || tag.chars().count() > 200 || tag.chars().any(char::is_whitespace) {
        return invalid("请输入有效的 Danbooru 作品 tag");
    }
    let tag = tag.to_lowercase();
    let owner: Option<String> = c
        .query_row(
            &format!("SELECT l.entity_id FROM {LINKS} l WHERE l.core_system = 'danbooru' AND l.key = ?"),
            [&tag],
            |r| r.get(0),
        )
        .optional()?;
    if owner.as_deref().is_some_and(|o| o != ident) {
        return invalid("此 tag 已链接到其他身份，不能自动合并");
    }
    c.execute(
        &format!("UPDATE term_links SET current = 0 WHERE term_id = {TERM_OF} AND system = 'danbooru'"),
        [ident],
    )?;
    check_free(c, "danbooru", &tag)?;
    c.execute(
        &format!(
            "INSERT INTO term_links(system, key, term_id, current, source) VALUES ('danbooru', ?, {TERM_OF}, 1, 'user')
             ON CONFLICT(system, key) DO UPDATE SET current = 1, source = 'user'"
        ),
        [&tag, ident],
    )?;
    c.execute(
        &format!("UPDATE term_tree SET review_status = 'confirmed', review_note = '' WHERE {REVIEW_OF}"),
        [ident],
    )?;
    Ok(())
}

fn work_move(c: &Connection, ident: &str, parent: Option<&str>) -> Result<()> {
    require_work(c, ident)?;
    if let Some(p) = parent {
        require_work(c, p)?;
        if descendants(c, ident)?.iter().any(|d| d == p) {
            return invalid("作品不能挂到自身或自己的子作品下面");
        }
    }
    c.execute(&format!("UPDATE term_tree SET parent_id=?2 WHERE {OF}"), params![ident, parent])?;
    c.execute(&format!("UPDATE term_tree SET parent_locked = 1 WHERE {REVIEW_OF}"), [ident])?;
    Ok(())
}

/// `Store.edit_work`。
fn edit_work(c: &Connection, args: &Value) -> Result<Value> {
    let action = args["action"].as_str().unwrap_or("");
    let parent = args["parent"].as_str().filter(|p| !p.is_empty());
    let ident = match action {
        "create" => work_create(c, args)?,
        "edit" => {
            let ident = text(&args["ident"], "作品不存在")?.to_owned();
            require_work(c, &ident)?;
            let name = args["name"].as_str().unwrap_or("");
            if name.trim().is_empty() || name.chars().count() > 120 {
                return invalid("请输入作品名");
            }
            work_move(c, &ident, parent)?;
            match args.get("tag") {
                Some(Value::String(tag)) if !tag.is_empty() => work_link(c, &ident, tag)?,
                Some(Value::String(_)) => {
                    c.execute(
                        &format!("UPDATE term_links SET current = 0 WHERE term_id = {TERM_OF} AND system = 'danbooru'"),
                        [&ident],
                    )?;
                    c.execute(
                        &format!(
                            "UPDATE term_tree SET review_status = 'confirmed', review_note = ?2 WHERE {REVIEW_OF}"
                        ),
                        [ident.as_str(), "外部链接已由用户清空"],
                    )?;
                }
                _ => {}
            }
            character_names::put_name(
                c,
                &ident,
                text(&args["lang"], "缺少名字的语言")?,
                "user",
                name,
                "display",
                0,
                1,
            )?;
            ident
        }
        "from_tag" => {
            // 在作品框里选中一个 Danbooru 版权：已有的作品，否则新建一个链接好的作品（名字是调用方给的可读名）。
            let tag = text(&args["tag"], "请选择 Danbooru 角色 tag")?;
            match work_for(c, "x", tag, args["readable"].as_str().unwrap_or(""), &args["id"])? {
                Some(w) => w,
                None => return invalid("作品不存在"),
            }
        }
        "move" => {
            let ident = text(&args["ident"], "作品不存在")?.to_owned();
            work_move(c, &ident, parent)?;
            ident
        }
        "link" => {
            let ident = text(&args["ident"], "作品不存在")?.to_owned();
            work_link(c, &ident, args["tag"].as_str().unwrap_or(""))?;
            ident
        }
        "rename" => {
            let ident = text(&args["ident"], "作品不存在")?.to_owned();
            require_work(c, &ident)?;
            let name = args["name"].as_str().unwrap_or("");
            if name.trim().is_empty() || name.chars().count() > 120 {
                return invalid("请输入作品名");
            }
            character_names::put_name(
                c,
                &ident,
                text(&args["lang"], "缺少名字的语言")?,
                "user",
                name,
                "display",
                0,
                1,
            )?;
            ident
        }
        _ => return invalid("未知作品操作"),
    };
    Ok(json!({"id": ident}))
}

/// `works.apply_evidence`（原 tools/reconcile_works.py --apply，#186 删除）。readable：新建作品的可读名（tag → 名字）；
/// ids：新建作品用的 id（双跑对齐用，可省）。
fn work_evidence(c: &Connection, args: &Value) -> Result<Value> {
    let evidence = &args["evidence"];
    let mut tags: BTreeSet<String> = BTreeSet::new();
    for t in evidence["tags"].as_array().into_iter().flatten() {
        if t.get("category").and_then(Value::as_i64) == Some(3) && !t.get("is_deprecated").map(truthy).unwrap_or(false)
        {
            if let Some(name) = t["name"].as_str() {
                tags.insert(name.to_owned());
            }
        }
    }
    let mut ids: BTreeMap<String, String> = c
        .prepare(&format!(
            "SELECT l.key, l.entity_id FROM {LINKS} l JOIN {ENTITIES} e ON e.id = l.entity_id
             WHERE l.core_system = 'danbooru' AND l.current = 1 AND e.kind = 'work'"
        ))?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    // dict(...) 的顺序就是查询的行序：遍历 qualifier 保持原查询的行序。
    let qualifiers: Vec<(String, String)> = {
        let mut seen: Vec<(String, String)> = Vec::new();
        for row in c
            .prepare(&format!("SELECT l.key, l.entity_id FROM {LINKS} l WHERE l.core_system = 'qualifier'"))?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        {
            let (k, v): (String, String) = row?;
            match seen.iter_mut().find(|(key, _)| *key == k) {
                Some(entry) => entry.1 = v,
                None => seen.push((k, v)),
            }
        }
        seen
    };
    let (mut linked, mut pending, mut edges) = (Vec::new(), Vec::new(), Vec::new());
    for (key, ident) in &qualifiers {
        let status: Option<String> = c
            .query_row(&format!("SELECT review_status FROM term_tree WHERE {REVIEW_OF}"), [ident], |r| r.get(0))
            .optional()?;
        if status.as_deref() == Some("confirmed") {
            continue;
        }
        let owner: Option<String> = c
            .query_row(
                &format!("SELECT l.entity_id FROM {LINKS} l WHERE l.core_system = 'danbooru' AND l.key = ?"),
                [key],
                |r| r.get(0),
            )
            .optional()?;
        if !tags.contains(key) || owner.as_ref().is_some_and(|o| o != ident) {
            let note = if !tags.contains(key) { "需人工核对作品 tag" } else { "链接已属于另一个身份" };
            c.execute(&format!("UPDATE term_tree SET review_note = ?2 WHERE {REVIEW_OF}"), [ident.as_str(), note])?;
            pending.push(json!({"id": ident, "qualifier": key, "reason": note}));
            continue;
        }
        check_free(c, "danbooru", key)?;
        c.execute(
            &format!(
                "INSERT OR IGNORE INTO term_links(system, key, term_id, source) VALUES ('danbooru', ?, {TERM_OF}, 'check')"
            ),
            [key, ident],
        )?;
        c.execute(
            &format!("UPDATE term_tree SET review_status = 'verified', review_note = '' WHERE {REVIEW_OF}"),
            [ident],
        )?;
        ids.insert(key.clone(), ident.clone());
        linked.push(key.clone());
    }
    let mut parents: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in evidence["implications"].as_array().into_iter().flatten() {
        let (Some(a), Some(b)) = (row["antecedent_name"].as_str(), row["consequent_name"].as_str()) else { continue };
        if row.get("status").and_then(Value::as_str) == Some("active") && tags.contains(a) && tags.contains(b) {
            parents.entry(a.to_owned()).or_default().insert(b.to_owned());
        }
    }
    let mut needed: BTreeSet<String> = evidence["watch_tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|t| tags.contains(*t))
        .map(str::to_owned)
        .collect();
    let mut frontier: BTreeSet<String> = ids.keys().cloned().chain(needed.iter().cloned()).collect();
    while !frontier.is_empty() {
        let added: BTreeSet<String> = frontier
            .iter()
            .flat_map(|child| parents.get(child).into_iter().flatten().cloned())
            .filter(|p| !needed.contains(p) && !ids.contains_key(p))
            .collect();
        needed.extend(added.iter().cloned());
        frontier = added;
    }
    let new_ids = &args["ids"];
    let missing: Vec<String> = needed.iter().filter(|t| !ids.contains_key(*t)).cloned().collect();
    for tag in missing {
        if c.query_row(
            &format!("SELECT 1 FROM {LINKS} l WHERE l.core_system = 'danbooru' AND l.key = ?"),
            [&tag],
            |_| Ok(()),
        )
        .optional()?
        .is_some()
        {
            continue;
        }
        // 调用方给了 ids（对象）时，缺的 id 向它要（Error::NeedId，指针 /ids/<tag>，按 JSON Pointer 转义）。
        let given = match new_ids.get(&tag) {
            Some(v) => v.clone(),
            None if new_ids.is_object() => Value::Bool(false),
            None => Value::Null,
        };
        let ident = new_id(&given).map_err(|e| match e {
            Error::NeedId(_) => Error::NeedId(format!("/ids/{}", tag.replace('~', "~0").replace('/', "~1"))),
            e => e,
        })?;
        character_entities::create(c, &New { id: &ident, kind: "work", ..New::default() }, "user")?;
        let readable =
            args["readable"][&tag].as_str().ok_or_else(|| Error::Invalid(format!("缺少作品 {tag} 的可读名")))?;
        character_names::put_name(c, &ident, "en", "index", readable, "display", 0, 0)?;
        check_free(c, "danbooru", &tag)?;
        c.execute(
            &format!("INSERT INTO term_links(system, key, term_id, source) VALUES ('danbooru', ?, {TERM_OF}, 'check')"),
            [&tag, &ident],
        )?;
        c.execute(REVIEW_OPEN, params![ident, "verified", "", 0])?;
        ids.insert(tag.clone(), ident.clone());
    }
    for (tag, choices) in &parents {
        let Some(ident) = ids.get(tag).cloned() else { continue };
        let lock: Option<i64> = c
            .query_row(&format!("SELECT parent_locked FROM term_tree WHERE {REVIEW_OF}"), [&ident], |r| r.get(0))
            .optional()?;
        if lock.unwrap_or(0) != 0 {
            continue;
        }
        let mut note = None;
        if choices.len() != 1 {
            note = Some(format!(
                "多个作品上级，需人工选择：{works}",
                works = choices.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        } else {
            let parent_tag = choices.iter().next().expect("一个");
            let Some(parent) = ids.get(parent_tag).cloned() else { continue };
            if descendants(c, &ident)?.contains(&parent) {
                note = Some("蕴含关系成环，需人工确认".to_owned());
            } else {
                c.execute(&format!("UPDATE term_tree SET parent_id=?2 WHERE {OF}"), [&ident, &parent])?;
                edges.push(json!([tag, parent_tag]));
            }
        }
        if let Some(note) = note {
            c.execute(&format!("UPDATE term_tree SET review_note = ?2 WHERE {REVIEW_OF}"), [&ident, &note])?;
            pending.push(json!({"id": ident, "tag": tag, "reason": note}));
        }
    }
    Ok(json!({"exact_links": linked.len(), "tree_edges": edges, "pending": pending}))
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

pub fn command(c: &Connection, op: &str, args: &Value) -> Result<Option<Value>> {
    let result = match op {
        "create_entity" => create_entity(c, args)?,
        "create_linked" => create_linked(c, args)?,
        "shared_oc" => shared_oc(c, args)?,
        // 用户编辑走带历史的命令，原有结构命令继续供 CLI 调用。
        "set_kind" | "edit_kind" => set_kind(c, args, op == "edit_kind")?,
        "set_mascot" | "edit_mascot" => set_mascot(c, args, op == "edit_mascot")?,
        "set_work" => set_character_work(c, args)?,
        // 新建作品要调用方给 id 时（`"id": false`），指针是 /id。
        "edit_work" => edit_work(c, args).map_err(|e| match e {
            Error::NeedId(p) if p.is_empty() => Error::NeedId("/id".into()),
            e => e,
        })?,
        "work_evidence" => work_evidence(c, args)?,
        "set_profile" => crate::character_profile::set_profile(c, args)?,
        "link_local" => crate::character_profile::link_local(c, args)?,
        _ => return Ok(None),
    };
    if op != "work_evidence" {
        crate::human_conclusions::revision::changed(c)?;
    }
    Ok(Some(result))
}
