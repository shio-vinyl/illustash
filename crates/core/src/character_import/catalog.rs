//! 目录关系由原始身份和目录写入的结构边组成。证据留在原始身份上，撤回时只撤这条边。
use super::{entity, linked, Ids};
use crate::character_catalog::{self, Action, Catalog, Character, Relation};
use crate::character_commands::{begin_action, finish_action, hybrid, resolve};
use crate::character_entities::{ENTITIES, OF};
use crate::{character_graph, character_json, character_projection, Error, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// 缺少规范标签或本体时，目录先建立可承载这个身份的词条。
pub const CHARACTER_ORIGIN: &str = "catalog:character";
/// 已应用的修订号及该目录拥有的结构边。
pub const CATALOG_META: &str = "character_catalog";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct Tree {
    kind: String,
    work: Option<String>,
    variant: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Saved {
    action: Option<i64>,
    before: Option<Tree>,
    variant: Option<String>,
}

type Pair = (String, String);

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Change(String, String, #[serde(default, skip_serializing_if = "Option::is_none")] Option<Saved>);

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct State {
    revision: u32,
    merged: Vec<Change>,
    attached: Vec<Change>,
    groups: BTreeSet<String>,
    declined: BTreeSet<Pair>,
}

impl State {
    fn read(c: &Connection) -> Result<Self> {
        let stored: Option<String> = c
            .prepare_cached("SELECT value FROM meta WHERE key=?")?
            .query_row([CATALOG_META], |r| r.get(0))
            .optional()?;
        let Some(stored) = stored else { return Ok(Self::default()) };
        serde_json::from_str(&stored).map_err(|e| Error::invalid(format!("角色目录状态损坏：{e}")))
    }

    fn write(&self, c: &Connection) -> Result<()> {
        c.prepare_cached("INSERT OR REPLACE INTO meta VALUES (?,?)")?
            .execute(params![CATALOG_META, character_json::dump(&json!(self), true)])?;
        Ok(())
    }

    fn list_mut(&mut self, kind: &str) -> &mut Vec<Change> {
        if kind == "merged" {
            &mut self.merged
        } else {
            &mut self.attached
        }
    }

    fn declined(&self, a: &str, b: &str) -> bool {
        self.declined.contains(&(a.to_owned(), b.to_owned())) || self.declined.contains(&(b.to_owned(), a.to_owned()))
    }
}

pub(super) fn exact(c: &Connection, tag: &str) -> Result<Option<String>> {
    if let Some(id) = linked(c, "danbooru", tag)? {
        return Ok(Some(id));
    }
    linked(c, "wd", tag)
}

/// 目录合并须让后续导入继续将证据写到原始词条。
pub(super) fn managed_merge(c: &Connection, id: &str) -> Result<bool> {
    Ok(c.prepare_cached(
        "SELECT EXISTS(SELECT 1 FROM term_tree r JOIN terms t ON t.id=r.term_id
          JOIN meta m ON m.key=?2 JOIN json_each(m.value,'$.merged') p
          WHERE t.key=?1 AND r.merged_into=json_extract(p.value,'$[1]') AND json_extract(p.value,'$[0]')=?1)",
    )?
    .query_row([id, CATALOG_META], |r| r.get(0))?)
}

/// 读取用户主动拆开的关系；按两个原始身份匹配。
pub fn catalog_declined(c: &Connection, a: &str, b: &str) -> Result<bool> {
    Ok(State::read(c)?.declined(a, b))
}

/// 一次读取图库里由用户明确拆开的角色对。
pub fn declined_pairs(c: &Connection) -> Result<BTreeSet<(String, String)>> {
    Ok(State::read(c)?.declined)
}

fn approved(entry: &Character) -> bool {
    entry.action == Some(Action::Auto) && entry.relation == Some(Relation::Appearance)
}

fn tree(c: &Connection, id: &str) -> Result<Option<Tree>> {
    Ok(c.prepare_cached(&format!("SELECT kind,work_id,variant FROM {ENTITIES} WHERE id=?"))?
        .query_row([id], |r| Ok(Tree { kind: r.get(0)?, work: r.get(1)?, variant: r.get(2)? }))
        .optional()?)
}

fn passes(c: &Connection, id: &str, filter: &str) -> Result<bool> {
    Ok(c.prepare_cached(&format!("SELECT EXISTS(SELECT 1 FROM {ENTITIES} e WHERE e.id=? AND {filter})"))?
        .query_row([id], |r| r.get(0))?)
}

fn root(c: &Connection, id: &str) -> Result<bool> {
    passes(c, id, "e.parent_id IS NULL AND e.merged_into IS NULL AND e.kind='character'")
}

fn no_children(c: &Connection, id: &str) -> Result<bool> {
    Ok(c.prepare_cached("SELECT NOT EXISTS(SELECT 1 FROM term_tree WHERE parent_id=?)")?
        .query_row([id], |r| r.get(0))?)
}

fn referenced(c: &Connection, id: &str, sql: &str) -> Result<bool> {
    Ok(c.prepare_cached(sql)?.query_row([id], |r| r.get(0))?)
}

/// 用户对角色已有命名、确认、否定、编辑、人工合并或身份关系。自动修订保护这些端点。
pub fn user_touched(c: &Connection, id: &str) -> Result<bool> {
    if referenced(
        c,
        id,
        "SELECT EXISTS(SELECT 1 FROM terms t
        WHERE t.key=?1 AND (t.origin='user'
            OR EXISTS(SELECT 1 FROM term_names n WHERE n.term_id=t.id AND n.source='user')
            OR EXISTS(SELECT 1 FROM term_links l WHERE l.term_id=t.id AND l.source='user')
            OR EXISTS(SELECT 1 FROM term_verdicts v JOIN actions a ON a.id=v.action_id
                      WHERE v.term_id=t.id AND a.undone=0 AND v.origin IN ('user','legacy') AND v.via<>'merge')
            OR EXISTS(SELECT 1 FROM assertions x WHERE x.entity=t.key)
            OR EXISTS(SELECT 1 FROM reviews x WHERE x.entity=t.key)
            OR EXISTS(SELECT 1 FROM term_tree x WHERE x.term_id=t.id AND x.mascot_origin='user')
            OR EXISTS(SELECT 1 FROM prefs p JOIN json_each(p.value) j WHERE p.key='pins' AND j.value=t.key)))",
    )? {
        return Ok(true);
    }
    let changed = referenced(c, id, "SELECT EXISTS(
        SELECT 1 FROM actions a
        JOIN json_each(a.before_json,
          CASE WHEN json_type(a.before_json,'$[0].terms')='object' THEN '$[0].terms.before' ELSE '$' END) item
        WHERE a.undone=0 AND a.label NOT LIKE '角色词表：%'
        AND (json_extract(item.value,'$.merge.source')=?1 OR json_extract(item.value,'$.merge.target')=?1
            OR json_extract(item.value,'$.profile.entity')=?1 OR json_extract(item.value,'$.character_kind.entity')=?1
            OR json_extract(item.value,'$.character_mascot.entity')=?1
            OR json_extract(item.value,'$.identity_form.parent')=?1
            OR EXISTS(SELECT 1 FROM json_each(item.value,'$.identity_form.nodes') n
                      WHERE json_extract(n.value,'$.id')=?1)
            OR json_extract(item.value,'$.identity_relation.a')=?1 OR json_extract(item.value,'$.identity_relation.b')=?1)
    )")?;
    if changed || !hybrid(c)? {
        return Ok(changed);
    }
    referenced(
        c,
        id,
        "SELECT EXISTS(
        SELECT 1 FROM term_relations r JOIN term_relation_latest v ON v.relation_id=r.id
        JOIN terms t ON t.id IN (r.a,r.b) WHERE t.key=?1 AND v.origin IN ('user','legacy')
    )",
    )
}

fn eligible(c: &Connection, state: &State, a: &str, b: &str) -> Result<bool> {
    Ok(a != b
        && !state.declined(a, b)
        && !crate::character_identity::kept_apart(c, a, b)?
        && !user_touched(c, a)?
        && !user_touched(c, b)?)
}

fn variant_name(entry: &Character) -> &str {
    entry.variant.as_deref().unwrap_or(&entry.tag)
}

fn attach(c: &Connection, child: &str, parent: &str, name: &str) -> Result<()> {
    c.prepare_cached(&format!("UPDATE term_tree SET parent_id=?2,variant=?3,work_id=NULL WHERE {OF}"))?
        .execute(params![child, parent, name])?;
    c.prepare_cached("UPDATE terms SET kind='skin' WHERE key=?")?.execute([child])?;
    Ok(())
}

fn merge(c: &Connection, source: &str, target: &str) -> Result<()> {
    c.prepare_cached(&format!("UPDATE term_tree SET merged_into=?2 WHERE {OF}"))?.execute([source, target])?;
    Ok(())
}

fn new_current(c: &Connection, ids: &Ids, tag: &str) -> Result<String> {
    if let Some(id) = exact(c, tag)? {
        return resolve(c, &id);
    }
    let work = crate::character_profile::qualifier(tag);
    entity(c, ids, "danbooru", tag, tag, work, CHARACTER_ORIGIN)
}

fn appearance(
    c: &Connection,
    state: &mut State,
    entry: &Character,
    child: &str,
    parent: &str,
    historical: bool,
) -> Result<bool> {
    if !root(c, child)? || !root(c, parent)? || !no_children(c, child)? || !eligible(c, state, child, parent)? {
        return Ok(false);
    }
    let before = tree(c, child)?;
    let name = variant_name(entry);
    let change = Change(child.into(), parent.into(), Some(Saved { action: None, before, variant: Some(name.into()) }));
    attach(c, child, parent, name)?;
    record(c, state, "attached", change, historical)?;
    Ok(true)
}

/// 只对本次创建的原始身份改结构。hybrid 也在新角色进入身份基线以前执行。
pub(super) fn fresh(c: &Connection, ids: &Ids, id: &str, tag: &str) -> Result<()> {
    let catalog = character_catalog::current();
    let Some(entry) = catalog.character(tag) else { return Ok(()) };
    if tag == entry.tag && entry.tags.is_empty() && !approved(entry) && !entry.group {
        return Ok(());
    }
    let mut state = State::read(c)?;
    let mut active = id.to_owned();
    let target = if tag != entry.tag {
        Some(new_current(c, ids, &entry.tag)?)
    } else {
        // 旧图库可能只有旧标签。新标签入库时沿用这个未被人工区分的既有角色。
        let mut old = BTreeSet::new();
        for tag in &entry.tags {
            if let Some(owner) = exact(c, tag)? {
                old.insert(resolve(c, &owner)?);
            }
        }
        if old.len() == 1 {
            old.pop_first()
        } else {
            None
        }
    };
    if let Some(target) = target.filter(|target| target != id) {
        if !eligible(c, &state, id, &target)? {
            return Ok(());
        }
        merge(c, id, &target)?;
        record(
            c,
            &mut state,
            "merged",
            Change(id.into(), target.clone(), Some(Saved { action: None, before: None, variant: None })),
            false,
        )?;
        active = target;
    }
    if entry.group {
        label_group(c, &mut state, &active)?;
    }
    if approved(entry) {
        let already_in_baseline =
            hybrid(c)? && referenced(c, &active, "SELECT EXISTS(SELECT 1 FROM term_identity_baseline WHERE id=?1)")?;
        let preserve = active != id
            && !passes(c, &active, "e.term_id IN (SELECT id FROM terms WHERE origin='catalog:character')")?;
        if !already_in_baseline && !preserve {
            if let Some(parent) = catalog.parent(entry) {
                let parent = new_current(c, ids, &parent.tag)?;
                appearance(c, &mut state, entry, &active, &parent, false)?;
            }
        }
    }
    state.write(c)
}

fn label_group(c: &Connection, state: &mut State, id: &str) -> Result<bool> {
    if user_touched(c, id)? {
        return Ok(false);
    }
    let changed = c
        .prepare_cached(&format!(
            "UPDATE term_tree SET label_only=1
        WHERE {OF} AND label_only=0 AND original=0 AND parent_id IS NULL AND merged_into IS NULL"
        ))?
        .execute([id])?
        > 0;
    if changed {
        state.groups.insert(id.to_owned());
    }
    Ok(changed)
}

fn record(c: &Connection, state: &mut State, kind: &str, mut change: Change, historical: bool) -> Result<()> {
    if historical {
        let mut shown = crate::character_labels::Shown::new();
        let left = shown.name(c, &change.0)?;
        let right = shown.name(c, &change.1)?;
        let label = if kind == "attached" {
            format!("角色词表：已将 {left} 挂到 {right} 下")
        } else {
            format!("角色词表：已将 {left} 合并到 {right}（D 站改名前后的 tag）")
        };
        let id = begin_action(c, &label)?;
        change.2.as_mut().expect("新目录记录含快照").action = Some(id);
        finish_action(c, id, "catalog_change", vec![json!({"catalog_change":{"kind":kind,"entity":change.0}})])?;
    }
    let list = state.list_mut(kind);
    list.retain(|prior| prior.0 != change.0);
    list.push(change);
    Ok(())
}

fn links(c: &Connection, id: &str) -> Result<Vec<String>> {
    Ok(c.prepare_cached(
        "SELECT l.key FROM term_links l JOIN terms t ON t.id=l.term_id
         WHERE t.key=? AND l.system IN ('danbooru','wd_character') ORDER BY l.system,l.key",
    )?
    .query_map([id], |r| r.get(0))?
    .collect::<rusqlite::Result<_>>()?)
}

fn belongs(catalog: &Catalog, tags: &[String], entry: &Character) -> bool {
    tags.iter().any(|tag| catalog.character(tag).is_some_and(|e| e.id == entry.id))
}

fn still_published(c: &Connection, catalog: &Catalog, kind: &str, change: &Change) -> Result<bool> {
    let source = links(c, &change.0)?;
    let target = links(c, &change.1)?;
    Ok(source.iter().filter_map(|tag| catalog.character(tag)).any(|entry| {
        if kind == "merged" {
            belongs(catalog, &target, entry)
        } else {
            approved(entry)
                && catalog.parent(entry).is_some_and(|p| belongs(catalog, &target, p))
                && change.2.as_ref().and_then(|s| s.variant.as_deref()).is_none_or(|old| old == variant_name(entry))
        }
    }))
}

fn live(c: &Connection, kind: &str, change: &Change) -> Result<bool> {
    let column = if kind == "merged" { "merged_into" } else { "parent_id" };
    let table = if hybrid(c)? { "term_identity_baseline" } else { ENTITIES };
    let current: Option<(Option<String>, Option<String>, Option<String>)> = c
        .prepare_cached(&format!("SELECT {column},variant,kind FROM {table} WHERE id=?"))?
        .query_row([&change.0], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    let variant = change.2.as_ref().and_then(|s| s.variant.as_deref());
    Ok(current.is_some_and(|(point, name, kind)| {
        point.as_deref() == Some(&change.1)
            && variant.is_none_or(|variant| name.as_deref() == Some(variant) && kind.as_deref() == Some("skin"))
    }))
}

fn refresh(c: &Connection, roles: &BTreeSet<String>) -> Result<()> {
    if roles.is_empty() {
        return Ok(());
    }
    let mut affected = BTreeSet::new();
    let mut stmt = c.prepare_cached(
        "SELECT a.key FROM terms t JOIN term_current_evidence e ON e.term_id=t.id
         JOIN term_assets a ON a.id=e.asset_id AND a.namespace='library' WHERE t.key=?
         UNION SELECT uuid FROM candidates WHERE entity=?1
         UNION SELECT uuid FROM assertions WHERE entity=?1",
    )?;
    for role in roles {
        for row in stmt.query_map([role], |r| r.get::<_, String>(0))? {
            affected.insert(row?);
        }
    }
    let images: Vec<_> = affected.into_iter().collect();
    if !images.is_empty() {
        character_projection::refresh_candidates(c, &images)?;
        character_projection::evidence(c, Some(&images))?;
        character_projection::single(c, Some(&images))?;
        character_projection::memberships(c, Some(&images))?;
    }
    Ok(())
}

fn remove(c: &Connection, kind: &str, change: &Change) -> Result<()> {
    let Change(id, other, details) = change;
    let identity = hybrid(c)?;
    if kind == "merged" {
        if details.is_none() {
            undo_old_merge(c, id, other)?;
        } else if identity {
            c.prepare_cached("UPDATE term_identity_baseline SET merged_into=NULL WHERE id=? AND merged_into=?")?
                .execute([id, other])?;
        } else {
            c.prepare_cached(&format!("UPDATE term_tree SET merged_into=NULL WHERE {OF}"))?.execute([id])?;
        }
    } else {
        let prior = details.as_ref().and_then(|s| s.before.as_ref());
        let (kind, work, variant) = prior
            .map(|tree| (tree.kind.as_str(), tree.work.as_deref(), tree.variant.as_str()))
            .unwrap_or(("character", None, ""));
        if identity {
            c.prepare_cached(
                "UPDATE term_identity_baseline SET parent_id=NULL,variant=?3,kind=?4 WHERE id=?1 AND parent_id=?2",
            )?
            .execute(params![id, other, variant, kind])?;
            c.prepare_cached(&format!("UPDATE term_tree SET work_id=?2 WHERE {OF} AND work_id IS NULL"))?
                .execute(params![id, work])?;
        } else {
            c.prepare_cached(&format!(
                "UPDATE term_tree SET parent_id=NULL,work_id=coalesce(work_id,?2),variant=?3 WHERE {OF}"
            ))?
            .execute(params![id, work, variant])?;
            c.prepare_cached("UPDATE terms SET kind=?2 WHERE key=?1")?.execute([id, kind])?;
        }
    }
    if let Some(action) = details.as_ref().and_then(|s| s.action) {
        c.prepare_cached("UPDATE actions SET undone=1 WHERE id=?")?.execute([action])?;
    }
    Ok(())
}

fn undo_old_merge(c: &Connection, entity: &str, other: &str) -> Result<()> {
    let mut rows =
        c.prepare("SELECT id,before_json FROM actions WHERE undone=0 AND label LIKE '角色词表：%' ORDER BY id DESC")?;
    for row in rows.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
        let (id, before) = row?;
        let before = crate::strict_json::parse(&before).map_err(Error::Invalid)?;
        let items = crate::character_commands::owns(&before)?.unwrap_or_default();
        let found = items.iter().find_map(|i| i.get("merge")).filter(|m| m["source"] == entity && m["target"] == other);
        if let Some(snapshot) = found {
            c.prepare_cached("UPDATE actions SET undone=1 WHERE id=?")?.execute([id])?;
            return crate::character_merge::undo(c, id, snapshot);
        }
    }
    Err(Error::invalid("找不到这次合并的记录"))
}

struct LibraryRole {
    kind: String,
    parent: Option<String>,
    merged: Option<String>,
}

struct LibraryRoles {
    tags: BTreeMap<String, Vec<String>>,
    roles: BTreeMap<String, LibraryRole>,
}

impl LibraryRoles {
    fn read(c: &Connection) -> Result<Self> {
        let mut tags = BTreeMap::<String, Vec<String>>::new();
        let links = format!(
            "SELECT l.key,l.entity_id FROM {} l WHERE l.core_system IN ('danbooru','wd_character')
             ORDER BY l.key,l.core_system",
            crate::character_links::LINKS
        );
        for row in c.prepare(&links)?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))? {
            let (tag, id) = row?;
            tags.entry(tag).or_default().push(id);
        }
        let roles = c
            .prepare(&format!("SELECT id,kind,parent_id,merged_into FROM {ENTITIES}"))?
            .query_map([], |r| Ok((r.get(0)?, LibraryRole { kind: r.get(1)?, parent: r.get(2)?, merged: r.get(3)? })))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Self { tags, roles })
    }

    fn linked<'a>(&'a self, entry: &Character) -> impl Iterator<Item = &'a String> {
        std::iter::once(&entry.tag).chain(&entry.tags).flat_map(|tag| self.tags.get(tag).into_iter().flatten())
    }

    fn resolved<'a>(&'a self, mut id: &'a str) -> Option<(&'a str, &'a LibraryRole)> {
        for _ in 0..16 {
            let row = self.roles.get(id)?;
            if let Some(next) = &row.merged {
                id = next;
            } else {
                return Some((id, row));
            }
        }
        None
    }
}

/// 对既有图库应用当前目录。旧目录撤回时，只撤仍由目录拥有的边；hybrid 的既有角色等待用户判断。
pub fn catalog_apply(c: &Connection) -> Result<Value> {
    let catalog = character_catalog::current();
    let mut state = State::read(c)?;
    if state.revision >= catalog.revision {
        return Ok(json!({"skipped":true}));
    }
    let identity = hybrid(c)?;
    let mut touched = BTreeSet::new();
    for kind in ["merged", "attached"] {
        let previous = std::mem::take(state.list_mut(kind));
        for change in previous {
            if !live(c, kind, &change)? {
                continue;
            }
            let edited_legacy = change.2.is_none() && (user_touched(c, &change.0)? || user_touched(c, &change.1)?);
            if still_published(c, &catalog, kind, &change)? || edited_legacy {
                state.list_mut(kind).push(change);
                continue;
            }
            remove(c, kind, &change)?;
            touched.extend([change.0, change.1]);
        }
    }
    for id in std::mem::take(&mut state.groups) {
        let still_group = links(c, &id)?.iter().any(|tag| catalog.character(tag).is_some_and(|entry| entry.group));
        if still_group {
            state.groups.insert(id);
        } else if c
            .prepare_cached(&format!("UPDATE term_tree SET label_only=0 WHERE {OF} AND original=0"))?
            .execute([&id])?
            != 0
        {
            touched.insert(id);
        }
    }
    let mut library = LibraryRoles::read(c)?;
    for entry in catalog.characters().iter().filter(|entry| entry.group) {
        for raw in library.linked(entry) {
            if let Some((id, _)) = library.resolved(raw) {
                if label_group(c, &mut state, id)? {
                    touched.insert(id.to_owned());
                }
            }
        }
    }
    let (mut merged, mut attached) = (0, 0);
    if !identity {
        let mut roles = BTreeMap::new();
        for entry in catalog.characters() {
            let mut found = Vec::new();
            for raw in library.linked(entry) {
                let Some((id, role)) = library.resolved(raw) else { continue };
                if (role.kind == "character" || role.kind == "skin") && !found.iter().any(|s| s == id) {
                    found.push(id.to_owned());
                }
            }
            let Some((target, sources)) = found.split_first() else { continue };
            let mut combined = vec![target];
            let parent = library.roles[target].parent.clone();
            for source in sources {
                let origin = &library.roles[source].parent;
                if (origin.is_some() && *origin != parent) || !no_children(c, source)? {
                    continue;
                }
                let permitted = combined
                    .iter()
                    .try_fold(true, |permitted, id| Ok::<_, Error>(permitted && eligible(c, &state, source, id)?))?;
                if !permitted {
                    continue;
                }
                merge(c, source, target)?;
                library.roles.get_mut(source).expect("来自已读取的图库角色").merged = Some(target.clone());
                record(
                    c,
                    &mut state,
                    "merged",
                    Change(source.clone(), target.clone(), Some(Saved { action: None, before: None, variant: None })),
                    true,
                )?;
                combined.push(source);
                touched.extend([source.clone(), target.clone()]);
                merged += 1;
            }
            if combined.len() == found.len() {
                roles.insert(entry.id, target.clone());
            }
        }

        let ids = Ids(&Value::Null);
        for entry in catalog.characters().iter().filter(|entry| approved(entry)) {
            let (Some(child), Some(parent)) = (roles.get(&entry.id).cloned(), catalog.parent(entry)) else { continue };
            let parent_id = if let Some(id) = roles.get(&parent.id) {
                id.clone()
            } else if library.linked(parent).next().is_none()
                && root(c, &child)?
                && no_children(c, &child)?
                && !user_touched(c, &child)?
            {
                let id = new_current(c, &ids, &parent.tag)?;
                roles.insert(parent.id, id.clone());
                id
            } else {
                continue;
            };
            if appearance(c, &mut state, entry, &child, &parent_id, true)? {
                touched.extend([child, parent_id]);
                attached += 1;
            }
        }
    }
    if identity && !touched.is_empty() {
        character_graph::rebuild(c)?;
    }
    refresh(c, &touched)?;
    state.revision = catalog.revision;
    state.write(c)?;
    Ok(json!({"merged":merged,"attached":attached}))
}

fn split(c: &Connection, kind: &str, entity: &str, manual: bool) -> Result<()> {
    let mut state = State::read(c)?;
    let list = state.list_mut(kind);
    let Some(i) = list.iter().position(|item| item.0 == entity) else {
        return Err(Error::invalid("这个角色不是角色词表合并或挂上的"));
    };
    let change = list.remove(i);
    if !live(c, kind, &change)? {
        return Err(Error::invalid("这处关系已经被改过，不能再拆开"));
    }
    remove(c, kind, &change)?;
    state.declined.insert((change.0.clone(), change.1.clone()));
    state.write(c)?;
    if hybrid(c)? {
        character_graph::rebuild(c)?;
    }
    if manual {
        let mut shown = crate::character_labels::Shown::new();
        begin_action(c, &format!("已拆开角色词表的 {} 和 {}", shown.name(c, &change.0)?, shown.name(c, &change.1)?))?;
    }
    refresh(c, &BTreeSet::from([change.0, change.1]))
}

/// 逐条拆开词表的合并或造型，并保留用户的拆开决定。
pub fn catalog_split(c: &Connection, args: &Value) -> Result<Value> {
    let Some(entity) = args["entity"].as_str() else { return Err(Error::invalid("请选择角色")) };
    let state = State::read(c)?;
    let kind = if state.merged.iter().any(|p| p.0 == entity) { "merged" } else { "attached" };
    split(c, kind, entity, true)?;
    Ok(json!({"split":kind}))
}

pub(crate) fn undo_catalog_change(c: &Connection, item: &Value) -> Result<()> {
    let entity = crate::character_commands::text(&item["entity"], "角色词表步骤缺少角色")?;
    let kind = crate::character_commands::text(&item["kind"], "角色词表步骤缺少改动类型")?;
    if !matches!(kind, "merged" | "attached") {
        return Err(Error::invalid("角色词表步骤类型不正确"));
    }
    split(c, kind, entity, false)
}

#[derive(Default)]
pub(crate) struct PortableTree {
    pub merged: bool,
    pub attached: bool,
    pub group: bool,
    pub kind: Option<String>,
    pub work: Option<String>,
    pub variant: Option<String>,
}

/// 快照只导出人工决定和旧版无来源的结构；词表自己可重新建立的边不用随快照搬运。
pub(crate) fn portable_trees(c: &Connection) -> Result<BTreeMap<String, PortableTree>> {
    let state = State::read(c)?;
    let mut result: BTreeMap<String, PortableTree> = BTreeMap::new();
    for change in &state.merged {
        if live(c, "merged", change)? {
            result.entry(change.0.clone()).or_default().merged = true;
        }
    }
    for change in &state.attached {
        if live(c, "attached", change)? {
            let tree = result.entry(change.0.clone()).or_default();
            tree.attached = true;
            if let Some(prior) = change.2.as_ref().and_then(|s| s.before.as_ref()) {
                tree.kind = Some(prior.kind.clone());
                tree.work = prior.work.clone();
                tree.variant = Some(prior.variant.clone());
            }
        }
    }
    for id in &state.groups {
        if passes(c, id, "e.label_only=1 AND e.original=0")? {
            result.entry(id.clone()).or_default().group = true;
        }
    }
    Ok(result)
}

#[derive(Deserialize, Serialize)]
pub(crate) struct SuppressedCatalogChange {
    kind: String,
    change: Change,
}

/// 便携恢复「保持分开」时，目标图库若已有相同的词表关系，只撤掉这条自动边。
pub(crate) fn suppress_catalog_change(c: &Connection, a: &str, b: &str) -> Result<Option<SuppressedCatalogChange>> {
    let mut state = State::read(c)?;
    for kind in ["attached", "merged"] {
        let list = state.list_mut(kind);
        let Some(index) = list.iter().position(|p| (p.0 == a && p.1 == b) || (p.0 == b && p.1 == a)) else {
            continue;
        };
        let change = list.remove(index);
        if !live(c, kind, &change)? || change.2.is_none() {
            continue;
        }
        remove(c, kind, &change)?;
        state.write(c)?;
        if hybrid(c)? {
            character_graph::rebuild(c)?;
        }
        refresh(c, &BTreeSet::from([a.to_owned(), b.to_owned()]))?;
        return Ok(Some(SuppressedCatalogChange { kind: kind.into(), change }));
    }
    Ok(None)
}

/// 撤销便携恢复时复原它关闭的那条词表边。端点其间已变化时让宿主回滚该次撤销。
pub(crate) fn restore_catalog_change(c: &Connection, suppressed: &SuppressedCatalogChange) -> Result<()> {
    let SuppressedCatalogChange { kind, change: Change(id, other, detail) } = suppressed;
    let Some(detail) = detail else { return Err(Error::invalid("角色词表快照缺失")) };
    if !root(c, id)? || !no_children(c, id)? || !passes(c, other, "e.merged_into IS NULL")? {
        return Err(Error::Conflict("词表关系端点已变化，无法撤销".into()));
    }
    let identity = hybrid(c)?;
    if kind == "merged" {
        if identity {
            c.prepare_cached("UPDATE term_identity_baseline SET merged_into=?2 WHERE id=?1")?.execute([id, other])?;
        } else {
            merge(c, id, other)?;
        }
    } else {
        let Some(name) = detail.variant.as_deref() else { return Err(Error::invalid("造型名快照缺失")) };
        if detail.before != tree(c, id)? || !root(c, other)? {
            return Err(Error::Conflict("词表造型端点已变化，无法撤销".into()));
        }
        if identity {
            c.prepare_cached("UPDATE term_identity_baseline SET parent_id=?2,variant=?3,kind='skin' WHERE id=?1")?
                .execute(params![id, other, name])?;
            c.prepare_cached(&format!("UPDATE term_tree SET work_id=NULL WHERE {OF}"))?.execute([id])?;
        } else {
            attach(c, id, other, name)?;
        }
    }
    if let Some(action) = detail.action {
        c.prepare_cached("UPDATE actions SET undone=0 WHERE id=?")?.execute([action])?;
    }
    let mut state = State::read(c)?;
    state.list_mut(kind).push(suppressed.change.clone());
    state.write(c)?;
    if identity {
        character_graph::rebuild(c)?;
    }
    refresh(c, &BTreeSet::from([id.clone(), other.clone()]))
}
