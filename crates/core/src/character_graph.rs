//! 逐行对应 web/identity.py _graph / _project 的语义，只支持 hybrid。
//! 对照基准的顺序是保留下来的旧版 rowid（seq）；代表取 b 侧。
use crate::character_entities::ENTITIES;
use crate::character_relations::{DECISIONS, DECISION_EVIDENCE, EVIDENCE, RELATIONS};
use crate::{character_json, character_projection, Error, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Debug)]
struct Node {
    id: String,
    parent: Option<String>,
    variant: Option<String>,
    kind: String,
    merged: Option<String>,
}
#[derive(Debug)]
pub struct Graph {
    nodes: Vec<Node>,
    mapping: Vec<usize>,
    hierarchy: Vec<Option<usize>>,
    families: Vec<usize>,
}
fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

pub fn baseline(c: &Connection) -> Result<()> {
    // seq 是 INTEGER PRIMARY KEY：不给值时按 SELECT 的行序追加（即身份本身的顺序）。
    c.prepare_cached(&format!(
        "INSERT OR IGNORE INTO term_identity_baseline(id,parent_id,variant,kind,merged_into)
        SELECT id,parent_id,variant,kind,merged_into FROM {ENTITIES} WHERE kind<>'work' ORDER BY seq"
    ))?
    .execute([])?;
    Ok(())
}

/// 新词表结构进入 hybrid 基线，同时登记它依赖的父角色和合并目标。普通候选不因此进入基线。
pub(crate) fn register_catalog(c: &Connection, changed: &BTreeSet<String>) -> Result<()> {
    if changed.is_empty() {
        return Ok(());
    }
    let count = c
        .prepare_cached(&format!(
            "WITH RECURSIVE selected(id) AS (
           SELECT value FROM json_each(?1)
           UNION SELECT e.parent_id FROM selected s JOIN {ENTITIES} e ON e.id=s.id WHERE e.parent_id IS NOT NULL
           UNION SELECT e.merged_into FROM selected s JOIN {ENTITIES} e ON e.id=s.id WHERE e.merged_into IS NOT NULL
         )
         INSERT OR IGNORE INTO term_identity_baseline(id,parent_id,variant,kind,merged_into)
         SELECT e.id,e.parent_id,e.variant,e.kind,e.merged_into
         FROM selected s JOIN {ENTITIES} e ON e.id=s.id
         WHERE e.kind<>'work' ORDER BY e.seq"
        ))?
        .execute([json!(changed).to_string()])?;
    if count != 0 {
        project(c, &graph(c)?)?;
    }
    Ok(())
}

pub fn graph(c: &Connection) -> Result<Graph> {
    let nodes: Vec<Node> = c
        .prepare_cached("SELECT id,parent_id,variant,kind,merged_into FROM term_identity_baseline ORDER BY seq")?
        .query_map([], |r| {
            Ok(Node { id: r.get(0)?, parent: r.get(1)?, variant: r.get(2)?, kind: r.get(3)?, merged: r.get(4)? })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let positions: HashMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
    let index = |s: &str| positions.get(s).copied().ok_or_else(|| Error::invalid("关系端点不存在"));
    let accepted: Vec<(String, String, String)> = c
        .prepare_cached(&format!(
            "SELECT r.a, r.b, r.kind FROM {RELATIONS} r WHERE r.status = 'accepted' ORDER BY r.id"
        ))?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut parent: Vec<usize> = (0..nodes.len()).collect();
    for (i, n) in nodes.iter().enumerate() {
        if let Some(j) = n.merged.as_deref().and_then(|s| positions.get(s)) {
            let a = find(&mut parent, i);
            let b = find(&mut parent, *j);
            parent[a] = b;
        }
    }
    for (a, b, kind) in &accepted {
        if kind == "duplicate_of" {
            let a = find(&mut parent, index(a)?);
            let b = find(&mut parent, index(b)?);
            parent[a] = b;
        }
    }
    let mapping: Vec<usize> = (0..nodes.len()).map(|i| find(&mut parent, i)).collect();
    let mut hierarchy = vec![None; nodes.len()];
    let attach = |a: usize, b: usize, hierarchy: &mut Vec<Option<usize>>| -> Result<()> {
        let (child, person) = (mapping[a], mapping[b]);
        if child == person {
            return Err(Error::invalid("变体不能与其人物重复"));
        }
        if hierarchy[child].is_some_and(|p| p != person) {
            return Err(Error::invalid("变体已有另一个父人物，需先撤销旧关系"));
        }
        hierarchy[child] = Some(person);
        Ok(())
    };
    for (i, n) in nodes.iter().enumerate() {
        if let Some(p) = n.parent.as_deref().filter(|s| !s.is_empty()) {
            attach(i, index(p)?, &mut hierarchy)?;
        }
    }
    for (a, b, kind) in &accepted {
        if kind == "different_from" && mapping[index(a)?] == mapping[index(b)?] {
            return Err(Error::invalid("确认不同与重复等价组冲突"));
        }
        if kind == "form_of" {
            attach(index(a)?, index(b)?, &mut hierarchy)?;
        }
    }
    let mut families = Vec::new();
    for &mapped in &mapping {
        let mut i = mapped;
        let mut seen = HashSet::new();
        while let Some(p) = hierarchy[i] {
            if !seen.insert(i) {
                return Err(Error::invalid("人物—变体关系存在环"));
            }
            i = p;
        }
        families.push(i);
    }
    for (a, b, kind) in &accepted {
        if kind == "different_from" && families[index(a)?] == families[index(b)?] {
            return Err(Error::invalid("确认不同与人物—变体关系冲突"));
        }
    }
    Ok(Graph { nodes, mapping, hierarchy, families })
}

/// 还没复核的继承层级所在的人物：已接受的皮肤从属关系带着继承层级的证据，而关系最新的一条决议没有复核过这条证据。
/// 每条关系最新的决议先算一次，不在每行里重算决议片段。
fn pending_query(c: &Connection) -> Result<String> {
    let portable: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='human_relation_review_context')",
        [],
        |r| r.get(0),
    )?;
    let local = format!(
        "WITH latest(relation_id, id) AS (
           SELECT d.relation_id, max(d.id) FROM {DECISIONS} d GROUP BY d.relation_id
         )
         SELECT DISTINCT n.character_id
         FROM {RELATIONS} r
         JOIN identity_nodes n ON n.id = r.a
         JOIN {EVIDENCE} e ON e.relation_id = r.id AND e.source = 'legacy-hierarchy'
         LEFT JOIN latest l ON l.relation_id = r.id
         WHERE r.kind = 'form_of' AND r.status = 'accepted' AND NOT EXISTS(
           SELECT 1 FROM {DECISION_EVIDENCE} x WHERE x.fingerprint = e.fingerprint AND x.decision_id = l.id
         )"
    );
    if !portable {
        return Ok(local);
    }
    // 本地新增证据仍按原指纹逐条检查；仅当前有效决议携带的便携上下文参与未复核判定。
    Ok(format!("WITH seen(relation_id,fingerprint) AS (
        SELECT e.relation_id,'legacy-hierarchy:'||json_extract(e.data,'$.legacy_fingerprint')
            FROM term_evidence e WHERE e.source='legacy-hierarchy'
        UNION SELECT v.relation_id,h.fingerprint FROM human_relation_review_context h
            JOIN term_relation_latest v ON v.id=h.verdict_id WHERE h.fingerprint LIKE 'legacy-hierarchy:%'
        ) SELECT DISTINCT n.character_id FROM {RELATIONS} r
        JOIN identity_nodes n ON n.id=r.a JOIN term_relation_latest v ON v.relation_id=r.core_id
        JOIN seen s ON s.relation_id=r.core_id
        WHERE r.kind='form_of' AND r.status='accepted'
        AND NOT EXISTS(SELECT 1 FROM human_relation_review_context h
            WHERE h.verdict_id=v.id AND h.fingerprint=s.fingerprint AND h.reviewed=1)
        AND NOT EXISTS(SELECT 1 FROM json_each(v.reviewed) j JOIN term_evidence e ON e.id=j.value
            WHERE e.source='legacy-hierarchy' AND 'legacy-hierarchy:'||json_extract(e.data,'$.legacy_fingerprint')=s.fingerprint)"))
}

pub fn version(c: &Connection) -> Result<String> {
    let rows: Vec<Value> = c
        .prepare_cached("SELECT id,parent_id,canonical_id FROM identity_nodes ORDER BY id")?
        .query_map([], |r| Ok(json!([r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?])))?
        .collect::<rusqlite::Result<_>>()?;
    let pending: BTreeSet<String> =
        c.prepare_cached(&pending_query(c)?)?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(crate::strict_json::sha256_hex(character_json::dump(&json!([rows, pending]), true).as_bytes()))
}

pub fn affected_images(c: &Connection, ids: &BTreeSet<String>) -> Result<BTreeSet<String>> {
    if ids.is_empty() {
        return Ok(BTreeSet::new());
    }
    c.execute_batch("CREATE TEMP TABLE IF NOT EXISTS reprojected(id TEXT PRIMARY KEY);DELETE FROM reprojected;")?;
    let mut insert = c.prepare_cached("INSERT INTO reprojected VALUES (?)")?;
    for id in ids {
        insert.execute([id])?;
    }
    Ok(c.prepare_cached(
        "SELECT uuid FROM candidates WHERE entity IN (SELECT id FROM reprojected)
        UNION SELECT uuid FROM proto_claims WHERE entity IN (SELECT id FROM reprojected)
        UNION SELECT uuid FROM assertions WHERE entity IN (SELECT id FROM reprojected)",
    )?
    .query_map([], |r| r.get(0))?
    .collect::<rusqlite::Result<_>>()?)
}

fn stored(s: &str) -> Result<Value> {
    crate::strict_json::parse(s).map_err(Error::Invalid)
}
type EntityState = HashMap<String, (Option<String>, Option<String>)>;
fn entity_state(c: &Connection) -> Result<EntityState> {
    Ok(c.prepare_cached(&format!("SELECT id,parent_id,merged_into FROM {ENTITIES}"))?
        .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn project(c: &Connection, g: &Graph) -> Result<Value> {
    project_with(c, g, false)
}

fn project_with(c: &Connection, g: &Graph, refresh_exclusions: bool) -> Result<Value> {
    let before = entity_state(c)?;
    let mapping: HashMap<&str, &str> =
        g.nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), g.nodes[g.mapping[i]].id.as_str())).collect();
    let is_split = |v: &Value| -> Result<bool> {
        let mut roots = HashSet::new();
        for id in v.as_array().ok_or_else(|| Error::invalid("身份范围应为数组"))? {
            let id = id.as_str().ok_or_else(|| Error::invalid("身份范围应为字符串"))?;
            roots.insert(mapping.get(id).copied().unwrap_or(id));
        }
        Ok(roots.len() > 1)
    };
    let mut suspended = BTreeSet::new();
    let scopes: Vec<(String, String, String)> = c
        .prepare_cached("SELECT uuid,entity,members_json FROM identity_scopes")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (u, e, members) in scopes {
        if is_split(&stored(&members)?)? {
            c.prepare_cached("INSERT OR REPLACE INTO identity_suspended VALUES (?,?,?,?)")?.execute(params![
                u,
                e,
                "equivalence_split",
                members
            ])?;
            suspended.insert(u);
        }
    }
    c.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS graph_projection(
      id TEXT PRIMARY KEY,parent_id TEXT,variant TEXT,kind TEXT,merged_into TEXT);
      DELETE FROM graph_projection;",
    )?;
    let mut put = c.prepare_cached("INSERT INTO graph_projection VALUES (?,?,?,?,?)")?;
    for (i, n) in g.nodes.iter().enumerate() {
        let parent = g.hierarchy[g.mapping[i]].map(|_| g.nodes[g.families[i]].id.as_str());
        let variant = if parent.is_none() {
            String::new()
        } else if let Some(v) = n.variant.as_ref().filter(|v| !v.is_empty()) {
            v.clone()
        } else {
            c.prepare_cached(
                &format!(
                    "SELECT l.key FROM {} l WHERE l.entity_id = ? AND l.core_system = 'danbooru' ORDER BY l.current DESC LIMIT 1",
                    crate::character_links::LINKS
                ),
            )?
            .query_row([&n.id], |r| r.get(0))
            .optional()?
            .unwrap_or_else(|| n.id.clone())
        };
        let kind = if parent.is_some() { "skin" } else { n.kind.as_str() };
        let canonical = &g.nodes[g.mapping[i]].id;
        let merged = (canonical != &n.id).then_some(canonical.as_str());
        put.execute(params![n.id, parent, variant, kind, merged])?;
    }
    drop(put);
    // 代表映射来自同一张完整图，只改增删项和真正改变的映射。
    c.execute_batch(
        "DELETE FROM identity_canonical WHERE entity NOT IN (SELECT id FROM graph_projection);
        INSERT INTO identity_canonical(entity,canonical)
        SELECT id,coalesce(merged_into,id) FROM graph_projection WHERE true
        ON CONFLICT(entity) DO UPDATE SET canonical=excluded.canonical
        WHERE identity_canonical.canonical IS NOT excluded.canonical;",
    )?;
    // 只改真正变化的行：名字版本只需在可见结构改变时失效，无差别更新全部节点会触发全库写放大。
    c.execute(
        "UPDATE term_tree SET parent_id=p.parent_id,variant=p.variant,merged_into=p.merged_into
      FROM graph_projection p JOIN terms t ON t.key=p.id WHERE term_tree.term_id=t.id
      AND (term_tree.parent_id IS NOT p.parent_id OR term_tree.variant IS NOT p.variant
        OR term_tree.merged_into IS NOT p.merged_into)",
        [],
    )?;
    c.execute(
        "UPDATE terms SET kind=p.kind FROM graph_projection p WHERE terms.key=p.id AND terms.kind IS NOT p.kind",
        [],
    )?;
    let version = version(c)?;
    let previous: Option<String> = c
        .prepare_cached("SELECT value FROM meta WHERE key='identity_graph_version'")?
        .query_row([], |r| r.get(0))
        .optional()?;
    let changed = previous.as_deref() != Some(version.as_str());
    let mut affected = BTreeSet::new();
    if changed {
        let current = entity_state(c)?;
        let ids =
            current.iter().filter(|(id, state)| before.get(*id) != Some(*state)).map(|(id, _)| id.clone()).collect();
        affected = affected_images(c, &ids)?;
        affected.extend(suspended);
        let uuids: Vec<String> = affected.iter().cloned().collect();
        character_projection::evidence(c, Some(&uuids))?;
        if refresh_exclusions {
            character_projection::memberships_before_excluded(c, Some(&uuids))?;
            crate::character_import::refresh_exclusions_and_fold(c)?;
        } else {
            character_projection::memberships(c, Some(&uuids))?;
        }
    }
    c.prepare_cached("INSERT OR REPLACE INTO meta VALUES ('identity_graph_version',?)")?.execute([&version])?;
    Ok(json!({"version":version,"changed":changed,"affected":affected}))
}

pub fn rebuild(c: &Connection) -> Result<Value> {
    rebuild_with(c, false)
}

pub(crate) fn rebuild_pair(c: &Connection) -> Result<Value> {
    rebuild_with(c, true)
}

fn rebuild_with(c: &Connection, refresh_exclusions: bool) -> Result<Value> {
    if !character_projection::policy(c)? {
        return Err(Error::invalid("请先完成两级身份迁移"));
    }
    baseline(c)?;
    let g = graph(c)?;
    project_with(c, &g, refresh_exclusions)
}
