//! S3 关系命令：只追加的核心决定，按旧版形状（`character_relations`）读回；完整的图校验和撤销审计。
//! 事务归调用方。
use crate::character_entities::{ENTITIES, OF};
use crate::character_relations::RELATIONS;
use crate::{character_graph, character_json, legacy_characters, Error, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// 对两个独立角色不再问归属的偏好；值为无向角色对的 JSON 数组。
pub const KEPT_APART_PREF: &str = "identity_kept_apart";

/// 按两个角色的稳定 ID 读取「分成两个，不再问」。
pub fn kept_apart_pairs(c: &Connection) -> Result<BTreeSet<(String, String)>> {
    let value: Option<String> =
        c.query_row("SELECT value FROM prefs WHERE key=?", [KEPT_APART_PREF], |r| r.get(0)).optional()?;
    match value {
        Some(value) => {
            let pairs: Vec<(String, String)> =
                serde_json::from_str(&value).map_err(|e| Error::invalid(e.to_string()))?;
            Ok(pairs.into_iter().map(|(a, b)| if a <= b { (a, b) } else { (b, a) }).collect())
        }
        None => Ok(BTreeSet::new()),
    }
}

fn ordered(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.into(), b.into())
    } else {
        (b.into(), a.into())
    }
}

/// 查询指定角色对的整理偏好，不读取或写入身份裁决。
pub fn kept_apart(c: &Connection, a: &str, b: &str) -> Result<bool> {
    Ok(c.query_row(&format!("SELECT NOT ({})", asks_about("?1", "?2")), [a, b], |r| r.get(0))?)
}

/// 只改这一个角色对。撤销时保留其他途径后来写入的偏好。
pub fn set_kept_apart(c: &Connection, a: &str, b: &str, on: bool) -> Result<bool> {
    let key = ordered(a, b);
    let mut pairs = kept_apart_pairs(c)?;
    let changed = if on { pairs.insert(key) } else { pairs.remove(&key) };
    if changed {
        if pairs.is_empty() {
            c.execute("DELETE FROM prefs WHERE key=?", [KEPT_APART_PREF])?;
        } else {
            c.execute(
                "INSERT OR REPLACE INTO prefs(key,value) VALUES (?,?)",
                params![KEPT_APART_PREF, json!(pairs).to_string()],
            )?;
        }
    }
    Ok(changed)
}

/// SQL 条件：对这两个身份仍可出题。调用方传入固定的 SQL 列名或参数。
pub fn asks_about(a: &str, b: &str) -> String {
    format!(
        "NOT EXISTS(SELECT 1 FROM json_each((SELECT value FROM prefs WHERE key='{KEPT_APART_PREF}')) p
         WHERE (json_extract(p.value,'$[0]')={a} AND json_extract(p.value,'$[1]')={b})
            OR (json_extract(p.value,'$[0]')={b} AND json_extract(p.value,'$[1]')={a}))"
    )
}

/// SQL 条件：仍可询问这两个备选人物。团体标签不是身份候选。
pub fn asks_about_people(a: &str, b: &str) -> String {
    let preference = asks_about(a, b);
    let people = crate::character_entities::without_groups(&[a, b]);
    format!("({preference} AND {people})")
}

/// 卡片提交时必须仍然是两个独立角色。过期端点不能重定向到别的人物。
pub fn require_pair(c: &Connection, a: &str, b: &str) -> Result<()> {
    let count: i64 = c
        .prepare_cached(&format!(
            "SELECT count(*) FROM {ENTITIES} WHERE id IN (?,?) AND kind<>'work'
         AND parent_id IS NULL AND merged_into IS NULL AND mascot=0 AND label_only=0"
        ))?
        .query_row([a, b], |r| r.get(0))?;
    if count != 2 {
        return Err(Error::invalid("请选择两个不同的角色（不能是子标签）"));
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct FormNode {
    id: String,
    parent: Option<String>,
    kind: String,
    variant: String,
    work_id: Option<String>,
    work: String,
}

fn refresh_forms(c: &Connection, nodes: &[FormNode]) -> Result<()> {
    let ids = nodes.iter().map(|node| node.id.clone()).collect();
    let images: Vec<_> = character_graph::affected_images(c, &ids)?.into_iter().collect();
    if !images.is_empty() {
        crate::character_projection::evidence(c, Some(&images))?;
        crate::character_projection::memberships(c, Some(&images))?;
    }
    Ok(())
}

/// 用户把 `child` 挂作 `parent` 的造型。直接模式保留独立的身份和原始结论，只改层级。
pub fn attach_form(c: &Connection, child: &str, parent: &str) -> Result<()> {
    require_pair(c, child, parent)?;
    if crate::character_projection::policy(c)? {
        return Err(Error::invalid("请通过身份关系保存造型"));
    }
    let mut shown = crate::character_labels::Shown::new();
    let variant = shown.name(c, child)?;
    let name = shown.name(c, parent)?;
    let work: String = c.query_row(&format!("SELECT work FROM {ENTITIES} WHERE id=?"), [parent], |r| r.get(0))?;
    let nodes = c
        .prepare_cached(&format!(
            "SELECT id,parent_id,kind,variant,work_id,work FROM {ENTITIES}
         WHERE id=?1 OR (parent_id=?1 AND kind<>'work') ORDER BY id"
        ))?
        .query_map([child], |r| {
            Ok(FormNode {
                id: r.get(0)?,
                parent: r.get(1)?,
                kind: r.get(2)?,
                variant: r.get(3)?,
                work_id: r.get(4)?,
                work: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for node in &nodes {
        let label = if node.id == child { &variant } else { &node.variant };
        c.prepare_cached(&format!(
            "UPDATE term_tree SET parent_id=?2,variant=?3,work_id=NULL,work_text=?4 WHERE {OF}"
        ))?
        .execute(params![node.id, parent, label, work])?;
        c.prepare_cached("UPDATE terms SET kind='skin' WHERE key=? AND kind<>'skin'")?.execute([&node.id])?;
    }
    refresh_forms(c, &nodes)?;
    let action = crate::character_commands::begin_action(
        c,
        &format!("{left} 已作为 {right} 的一个造型", left = variant, right = name),
    )?;
    crate::character_commands::finish_action(
        c,
        action,
        "identity_form",
        vec![json!({"identity_form":{"parent":parent,"nodes":nodes}})],
    )
}

pub(crate) fn undo_form(c: &Connection, before: &Value) -> Result<()> {
    let parent = text(&before["parent"], "造型快照不完整")?;
    let nodes: Vec<FormNode> =
        serde_json::from_value(before["nodes"].clone()).map_err(|_| Error::invalid("造型快照不完整"))?;
    for node in &nodes {
        let changed = c
            .prepare_cached(&format!(
                "UPDATE term_tree SET parent_id=?2,variant=?3,work_id=?4,work_text=?5 WHERE {OF} AND parent_id=?6"
            ))?
            .execute(params![node.id, node.parent, node.variant, node.work_id, node.work, parent])?;
        if changed != 1 {
            return Err(Error::Conflict("造型关系已变更，不能撤销".into()));
        }
        c.prepare_cached("UPDATE terms SET kind=?2 WHERE key=?1 AND kind IS NOT ?2")?
            .execute([&node.id, &node.kind])?;
    }
    refresh_forms(c, &nodes)
}

fn text<'a>(v: &'a Value, message: &str) -> Result<&'a str> {
    v.as_str().ok_or_else(|| Error::invalid(message))
}
fn relation_core(c: &Connection, id: i64) -> Result<i64> {
    c.query_row("SELECT core_id FROM term_legacy_ids WHERE kind='relation' AND legacy_id=?", [id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| Error::invalid("关系候选不存在"))
}
/// 一条关系现在的状态（旧编号 `id`），没有这条关系为 None。
fn relation_status(c: &Connection, id: i64) -> Result<Option<String>> {
    Ok(c.prepare_cached(&format!("SELECT r.status FROM {RELATIONS} r WHERE r.id = ?1"))?
        .query_row([id], |r| r.get(0))
        .optional()?)
}
/// 一对身份之间的重复关系（两个方向都算）的旧编号。原来的查询按两个方向依次走唯一索引，先 `a = ?1` 的方向。
fn duplicate(c: &Connection, a: &str, b: &str) -> Result<Option<i64>> {
    Ok(c.prepare_cached(&format!(
        "SELECT r.id
             FROM {RELATIONS} r
             WHERE r.kind = 'duplicate_of' AND ((r.a = ?1 AND r.b = ?2) OR (r.a = ?2 AND r.b = ?1))
             ORDER BY r.a = ?1 DESC
             LIMIT 1"
    ))?
    .query_row(params![a, b], |r| r.get(0))
    .optional()?)
}
fn next_id(c: &Connection, kind: &str) -> Result<i64> {
    Ok(c.query_row("SELECT coalesce(max(legacy_id),0)+1 FROM term_legacy_ids WHERE kind=?", [kind], |r| r.get(0))?)
}
fn semantic(v: &Value) -> Value {
    match v {
        Value::Object(o) => Value::Object(
            o.iter()
                .filter(|(k, _)| !matches!(k.as_str(), "response_ids" | "response_id" | "fetched_at"))
                .map(|(k, v)| (k.clone(), semantic(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(semantic).collect()),
        _ => v.clone(),
    }
}

pub fn propose(c: &Connection, a: &str, b: &str, kind: &str, evidence: Option<&Value>, source: &str) -> Result<i64> {
    if a == b || !matches!(kind, "duplicate_of" | "form_of" | "different_from") {
        return Err(Error::invalid("无效身份关系"));
    }
    let count: i64 = c.query_row(
        &format!("SELECT count(*) FROM {ENTITIES} WHERE id IN (?,?) AND kind<>'work'"),
        params![a, b],
        |r| r.get(0),
    )?;
    if count != 2 {
        return Err(Error::invalid("关系端点不存在"));
    }
    let (a, b) = if kind == "different_from" && a > b { (b, a) } else { (a, b) };
    let existing: Option<i64> = if kind == "duplicate_of" {
        duplicate(c, a, b)?
    } else {
        c.prepare_cached(&format!("SELECT r.id FROM {RELATIONS} r WHERE r.a = ?1 AND r.b = ?2 AND r.kind = ?3"))?
            .query_row(params![a, b, kind], |r| r.get(0))
            .optional()?
    };
    let id = match existing {
        Some(id) => id,
        None => {
            let ta: i64 = c.query_row("SELECT id FROM terms WHERE key=?", [a], |r| r.get(0))?;
            let tb: i64 = c.query_row("SELECT id FROM terms WHERE key=?", [b], |r| r.get(0))?;
            use crate::schema::RelationKind;
            let (ta, tb, relation_kind) = match kind {
                "form_of" => (tb, ta, RelationKind::Contains),
                "duplicate_of" => (ta, tb, RelationKind::SameAs),
                _ => (ta, tb, RelationKind::DifferentFrom),
            };
            let core = crate::store::relation(c, ta, tb, relation_kind)?;
            if let Some(id) = c
                .query_row("SELECT legacy_id FROM term_legacy_ids WHERE kind='relation' AND core_id=?1", [core], |r| {
                    r.get(0)
                })
                .optional()?
            {
                // 导入撤销保留历史编号；普通入口重新激活物理关系时沿用它。
                id
            } else {
                let id = next_id(c, "relation")?;
                c.execute(
                    "INSERT INTO term_legacy_ids(kind,legacy_id,core_id) VALUES ('relation',?,?)",
                    params![id, core],
                )?;
                id
            }
        }
    };
    if let Some(evidence) = evidence {
        let core = relation_core(c, id)?;
        let payload = character_json::dump(evidence, false);
        let fp = crate::strict_json::sha256_hex(
            format!("{source}\n{}", character_json::dump(&semantic(evidence), false)).as_bytes(),
        );
        let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM term_evidence WHERE relation_id=? AND method='identity_evidence' AND version=?)",params![core,fp],|r|r.get(0))?;
        if !exists {
            let stamp: String = c.query_row("SELECT CURRENT_TIMESTAMP", [], |r| r.get(0))?;
            let data = json!({"legacy_fingerprint":fp,"payload":payload,"created_at":stamp});
            c.execute("INSERT INTO term_evidence(relation_id,source,method,version,data,fingerprint) VALUES (?,?,'identity_evidence',?,?,?)",params![core,source,fp,data.to_string(),legacy_characters::fingerprint16(&fp)])?;
        }
    }
    Ok(id)
}

/// action 为 None 表示非用户发起的批次，不伪造撤销栈条目。
pub fn decide(
    c: &Connection,
    id: i64,
    status: &str,
    reason: &str,
    action: Option<i64>,
    batch_kind: &str,
) -> Result<Value> {
    if !matches!(status, "accepted" | "rejected" | "proposed") {
        return Err(Error::invalid("请保存明确的关系决议及理由"));
    }
    let core = relation_core(c, id)?;
    let previous = relation_status(c, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    let reviewed: Vec<i64> = c
        .prepare("SELECT id FROM term_evidence WHERE relation_id=? AND method='identity_evidence' ORDER BY id")?
        .query_map([core], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let (batch, anchor) = match action {
        Some(a) => (None, a),
        None => {
            let anchor: i64 = c.query_row("SELECT coalesce(max(id),0) FROM actions", [], |r| r.get(0))?;
            c.execute(
                "INSERT INTO term_batches(kind,anchor,label) VALUES (?,?,?)",
                params![batch_kind, anchor, reason],
            )?;
            (Some(c.last_insert_rowid()), anchor)
        }
    };
    let verdict = match status {
        "accepted" => "yes",
        "rejected" => "no",
        _ => "clear",
    };
    c.execute(
        "INSERT INTO term_verdicts(action_id,batch_id,anchor,relation_id,verdict,origin,reason,reviewed,via)
        VALUES (?,?,?,?,?,?,?,?,?)",
        params![
            action,
            batch,
            anchor,
            core,
            verdict,
            if action.is_none() && batch_kind == "migration" { "legacy" } else { "user" },
            reason,
            json!(reviewed).to_string(),
            if batch_kind == "undo_audit" {
                "undo"
            } else if action.is_none() {
                "migration"
            } else {
                "review"
            }
        ],
    )?;
    let decision_core = c.last_insert_rowid();
    crate::human_conclusions::relations::acknowledge(c, decision_core)?;
    let decision = next_id(c, "decision")?;
    c.execute(
        "INSERT INTO term_legacy_ids(kind,legacy_id,core_id) VALUES ('decision',?,?)",
        params![decision, decision_core],
    )?;
    Ok(json!({"relation_id":id,"previous_status":previous,"decision_id":decision,"status":status}))
}

pub fn command(c: &Connection, args: &Value) -> Result<Value> {
    if !crate::character_projection::policy(c)? {
        return Err(Error::invalid("请先完成两级身份迁移"));
    }
    let op = text(&args["op"], "请选择身份命令")?;
    match op {
        "rebuild" => character_graph::rebuild(c),
        "propose" => {
            let id = propose(
                c,
                text(&args["a"], "无效身份关系")?,
                text(&args["b"], "无效身份关系")?,
                text(&args["kind"], "无效身份关系")?,
                args.get("evidence").filter(|v| !v.is_null()),
                args.get("source").and_then(Value::as_str).unwrap_or("manual"),
            )?;
            Ok(json!({"relation_id":id}))
        }
        "decide" => {
            // 与 Store.decide_identity 的检查顺序一致（去掉预演后，同时有几处不对时报哪一条也要相同）：
            // 关系 id 的类型（宿主在调用前查，见 app_identity）→ 策略（上面）→ 没给 id 时先提议（端点）→ 状态与理由 → 关系存在。
            let id = match args.get("relation_id").filter(|v| !v.is_null()) {
                Some(v) => v.as_i64().filter(|i| *i > 0).ok_or_else(|| Error::invalid("关系 ID 不正确"))?,
                None => {
                    let fallback = json!({"manual_review": args.get("reason").cloned().unwrap_or(Value::Null)});
                    propose(
                        c,
                        text(&args["a"], "无效身份关系")?,
                        text(&args["b"], "无效身份关系")?,
                        text(&args["kind"], "无效身份关系")?,
                        Some(args.get("evidence").filter(|v| !v.is_null()).unwrap_or(&fallback)),
                        "manual",
                    )?
                }
            };
            // 没给时默认是 proposed（沿用旧实现）；给了但不是字符串（含 null）照旧拒绝。
            let status = match args.get("status") {
                None => "proposed",
                Some(Value::String(s)) => s.as_str(),
                Some(_) => return Err(Error::invalid("请保存明确的关系决议及理由")),
            };
            let reason = args.get("reason").and_then(Value::as_str).unwrap_or("");
            if !matches!(status, "accepted" | "rejected" | "proposed") || reason.trim().is_empty() {
                return Err(Error::invalid("请保存明确的关系决议及理由"));
            }
            c.execute("INSERT INTO actions(label,before_json) VALUES (?,'[]')", ["已保存身份关系决议"])?;
            let action = c.last_insert_rowid();
            let result = decide(c, id, status, reason.trim(), Some(action), "migration")?;
            character_graph::rebuild(c)?;
            let before = json!([{"terms":{"v":2,"type":"identity_relation","before":[{"identity_relation":result}]}}]);
            c.execute("UPDATE actions SET before_json=? WHERE id=?", params![before.to_string(), action])?;
            Ok(json!({"action":action,"decision":result}))
        }
        "accept-many" => {
            character_graph::baseline(c)?;
            let ids = args["ids"].as_array().ok_or_else(|| Error::invalid("关系 ID 应为数组"))?;
            let reason = text(&args["reason"], "请保存明确的关系决议及理由")?;
            let mut accepted = Vec::new();
            let mut skipped = Vec::new();
            for id in ids {
                let id = id.as_i64().ok_or_else(|| Error::invalid("关系 ID 不正确"))?;
                let previous = relation_status(c, id)?;
                if previous.as_deref() != Some("proposed") {
                    continue;
                }
                c.execute_batch("SAVEPOINT accept_relation")?;
                decide(c, id, "accepted", reason, None, "migration")?;
                match character_graph::graph(c) {
                    Ok(_) => {
                        c.execute_batch("RELEASE accept_relation")?;
                        accepted.push(id);
                    }
                    Err(Error::Invalid(reason)) => {
                        c.execute_batch("ROLLBACK TO accept_relation;RELEASE accept_relation")?;
                        skipped.push(json!({"relation_id":id,"reason":reason}));
                    }
                    Err(e) => return Err(e),
                }
            }
            if !accepted.is_empty() {
                character_graph::rebuild(c)?;
            }
            Ok(json!({"accepted":accepted,"skipped":skipped}))
        }
        "undo" => crate::character_commands::undo(c, args),
        _ => Err(Error::invalid("未知身份命令")),
    }
}

/// 撤销一条人工关系决议（旧形状 `identity_relation`）：追加回到 previous_status 的撤销审计决议，
/// 记下当时关系上的全部证据，再重建图（= 旧版 `decide(previous)`）。调用方已把这一步标为撤销。
pub fn undo_relation(c: &Connection, r: &Value) -> Result<()> {
    if !keys(r, &["relation_id", "previous_status", "decision_id", "status"])
        || r["decision_id"].as_i64().is_none_or(|i| i <= 0)
        || !matches!(r["status"].as_str(), Some("accepted" | "rejected" | "proposed"))
        || !matches!(r["previous_status"].as_str(), Some("accepted" | "rejected" | "proposed"))
    {
        return Err(Error::invalid("身份撤销快照不完整"));
    }
    let relation = r["relation_id"].as_i64().ok_or_else(|| Error::invalid("身份撤销快照不完整"))?;
    decide(c, relation, text(&r["previous_status"], "身份撤销快照不完整")?, "撤销人工关系决议", None, "undo_audit")?;
    character_graph::rebuild(c)?;
    Ok(())
}

/// hybrid 下撤销激活前留下的破坏式合并：对应的 `duplicate_of` 退回 proposed（撤销审计），搬走的结论不回来。
pub fn undo_merge(c: &Connection, m: &Value) -> Result<()> {
    if !keys(
        m,
        &[
            "source",
            "target",
            "moved",
            "kept",
            "candidates",
            "target_prev",
            "variants",
            "aliases",
            "pins",
            "reviews",
            "uuids",
        ],
    ) || ["moved", "kept", "candidates", "target_prev", "variants", "aliases", "reviews", "uuids"]
        .iter()
        .any(|k| !m[k].is_array())
        || !(m["pins"].is_null() || m["pins"].is_string())
    {
        return Err(Error::invalid("合并快照不完整"));
    }
    let (a, b) = (text(&m["source"], "合并快照不完整")?, text(&m["target"], "合并快照不完整")?);
    let relation = duplicate(c, a, b)?.ok_or_else(|| Error::invalid("旧合并缺少迁移记录，请先核验备份"))?;
    decide(c, relation, "proposed", "撤销历史合并；原归属无法区分的认领待确认", None, "undo_audit")?;
    character_graph::rebuild(c)?;
    Ok(())
}

fn keys(v: &Value, expected: &[&str]) -> bool {
    v.as_object().is_some_and(|o| o.len() == expected.len() && expected.iter().all(|k| o.contains_key(*k)))
}
