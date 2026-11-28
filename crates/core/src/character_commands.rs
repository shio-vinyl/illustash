//! S3 角色业务命令（docs/TERMS_CHARACTERS.md 方案第 8 节）：一个命令一个核心事务，逐行照 tests/reference/web/store.py（#9 已删除） 的旧写法。
//!
//! 写法：人工结论追加成 `term_verdicts`（挂在这一步上，它是权威），同时维护派生表 `assertions`（每个 (图, 身份) 的
//! 有效结论）。身份、结构、
//! 链接、名字、作品复核直接写 `terms` / `term_tree` / `term_links` / `term_names`（核心第 8 版，#201）。
//! 历史文案由核心按旧代码逐字生成（character_labels，名字 N5）；按筛选条件的选择集由调用方算好传进来（算不出时交来
//! 旧代码的拒绝文案 `asked_refused`，核心在旧代码算选择集的那一步报出，检查的先后不变）；
//! 选择集的复制品展开、目标身份、原归属这些核心在自己的事务里重新核验。
//! 新步骤的 `before_json` 是 `[{"terms":{"v":2,"type":…,"before":[旧形状的快照…]}}]`。
use crate::character_entities::ENTITIES;
use crate::character_labels::{works, Shown};
use crate::error::invalid;
use crate::{character_json, character_projection, undo_labels::UndoLabel, Error, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

pub(crate) fn text<'a>(v: &'a Value, message: &str) -> Result<&'a str> {
    v.as_str().ok_or_else(|| Error::Invalid(message.into()))
}

fn strings(v: &Value, message: &str) -> Result<Vec<String>> {
    v.as_array()
        .ok_or_else(|| Error::Invalid(message.into()))?
        .iter()
        .map(|x| x.as_str().map(str::to_owned).ok_or_else(|| Error::Invalid(message.into())))
        .collect()
}

/// 沿用旧实现的真值规则：null、false、0、空串、空数组、空对象为假。
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// 作品参数（`if work: works.require(c, work)`）：假值是不归作品；不是字符串的真值在旧代码里查不到作品。
pub(crate) fn work_arg(v: &Value) -> Result<Option<&str>> {
    if !truthy(v) {
        return Ok(None);
    }
    match v.as_str() {
        Some(w) => Ok(Some(w)),
        None => invalid("作品不存在"),
    }
}

/// 身份层是否启用（hybrid）。策略未设置时 false；其他策略在 `policy` 里拒绝。
pub(crate) fn hybrid(c: &Connection) -> Result<bool> {
    character_projection::policy(c)
}

// ---- 选择集与身份 ----

/// `Store._resolve`：沿破坏式合并找到现在代表它的身份（最多 16 跳）。
pub fn resolve(c: &Connection, ident: &str) -> Result<String> {
    let mut ident = ident.to_owned();
    for _ in 0..16 {
        let row: Option<Option<String>> = c
            .prepare_cached(&format!("SELECT merged_into FROM {ENTITIES} WHERE id=?"))?
            .query_row([&ident], |r| r.get(0))
            .optional()?;
        match row {
            Some(Some(next)) if !next.is_empty() => ident = next,
            _ => return Ok(ident),
        }
    }
    Ok(ident)
}

/// `Store._require_entity`：(id, parent_id)。
pub(crate) fn require_entity(c: &Connection, entity: &Value) -> Result<(String, Option<String>)> {
    let Some(entity) = entity.as_str() else { return invalid("请选择角色") };
    let id = resolve(c, entity)?;
    c.prepare_cached(&format!("SELECT id,parent_id FROM {ENTITIES} WHERE kind<>'work' AND id=?"))?
        .query_row([&id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .ok_or_else(|| Error::Invalid("角色不存在".into()))
}

/// `Store._resolve_ids(c, ids, None, [], confirm, copies)`：调用方已按筛选条件算好的图；核心核对它们还在图库里，
/// 并在自己的事务里按 `duplicates` 展开复制品（不信任调用方）。
pub(crate) fn resolve_ids(c: &Connection, ids: &Value, confirm: bool, copies: bool) -> Result<Vec<String>> {
    let Some(list) = ids.as_array().filter(|a| !a.is_empty()) else { return invalid("请选择图片") };
    let mut ids: Vec<String> = Vec::new();
    for x in list {
        let Some(x) = x.as_str() else { return invalid("请选择图片") };
        ids.push(x.to_owned());
    }
    if ids.len() > 500 && !confirm {
        return invalid("超过 500 张，请确认数量并携带 confirm:true；整批可撤销");
    }
    let mut seen = BTreeSet::new();
    ids.retain(|i| seen.insert(i.clone()));
    for ident in &ids {
        if c.prepare_cached("SELECT 1 FROM assets WHERE uuid=?")?.query_row([ident], |_| Ok(())).optional()?.is_none() {
            return invalid("所选图片不存在，请刷新");
        }
    }
    if copies {
        let mut stmt = c.prepare_cached(
            "SELECT y.uuid FROM duplicates d JOIN duplicates y ON y.grp=d.grp AND y.uuid<>d.uuid WHERE d.uuid=? ORDER BY y.pos",
        )?;
        for ident in ids.clone() {
            for copy in stmt.query_map([&ident], |r| r.get::<_, String>(0))? {
                let copy = copy?;
                if seen.insert(copy.clone()) {
                    ids.push(copy);
                }
            }
        }
    }
    Ok(ids)
}

// ---- 快照 ----

fn row_json(c: &Connection, sql: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Value> {
    let mut stmt = c.prepare_cached(sql)?;
    let n = stmt.column_count();
    let row = stmt
        .query_row(args, |r| {
            (0..n)
                .map(|i| {
                    Ok(match r.get_ref(i)? {
                        rusqlite::types::ValueRef::Null => Value::Null,
                        rusqlite::types::ValueRef::Integer(v) => json!(v),
                        rusqlite::types::ValueRef::Real(v) => json!(v),
                        rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                        rusqlite::types::ValueRef::Blob(_) => Value::Null,
                    })
                })
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .optional()?;
    Ok(row.map(Value::Array).unwrap_or(Value::Null))
}

/// `identity.scope_snapshot`。
fn scope_snapshot(c: &Connection, u: &str, e: Option<&str>) -> Result<Value> {
    Ok(json!({
        "identity_scopes": row_json(c, "SELECT members_json FROM identity_scopes WHERE uuid=? AND entity=?", &[&u, &e])?,
        "identity_suspended": row_json(c, "SELECT reason,options_json FROM identity_suspended WHERE uuid=? AND entity=?", &[&u, &e])?,
        "identity_uncertain": row_json(c, "SELECT options_json,reason,created_at FROM identity_uncertain WHERE uuid=?", &[&u])?,
    }))
}

/// `Store._snapshot`。
pub(crate) fn snapshot(c: &Connection, ident: &str, entity: Option<&str>) -> Result<Value> {
    let entity = entity.filter(|e| !e.is_empty());
    let assertion = match entity {
        Some(e) => c
            .prepare_cached("SELECT status,origin FROM assertions WHERE uuid=? AND entity=?")?
            .query_row([ident, e], |r| Ok(json!({"status": r.get::<_, String>(0)?, "origin": r.get::<_, String>(1)?})))
            .optional()?
            .unwrap_or(Value::Null),
        None => Value::Null,
    };
    let deferred =
        c.prepare_cached("SELECT 1 FROM deferred WHERE uuid=?")?.query_row([ident], |_| Ok(())).optional()?.is_some();
    let mut out = json!({"uuid": ident, "entity": entity, "assertion": assertion, "deferred": deferred});
    if hybrid(c)? {
        out["identity"] = scope_snapshot(c, ident, entity)?;
    }
    Ok(out)
}

/// `identity.restore_scope`。
fn restore_scope(c: &Connection, u: &str, e: Option<&str>, snap: &Value) -> Result<()> {
    for (table, width) in [("identity_scopes", 1), ("identity_suspended", 2)] {
        c.prepare_cached(&format!("DELETE FROM {table} WHERE uuid=? AND entity=?"))?.execute(params![u, e])?;
        if let Some(row) = snap.get(table).and_then(Value::as_array) {
            let marks = vec!["?"; 2 + width].join(",");
            let mut values: Vec<rusqlite::types::Value> = vec![u.to_owned().into(), e.map(str::to_owned).into()];
            values.extend(row.iter().map(sql_value));
            c.prepare_cached(&format!("INSERT INTO {table} VALUES ({marks})"))?
                .execute(rusqlite::params_from_iter(values))?;
        }
    }
    c.prepare_cached("DELETE FROM identity_uncertain WHERE uuid=?")?.execute([u])?;
    if let Some(row) = snap.get("identity_uncertain").and_then(Value::as_array) {
        let mut values: Vec<rusqlite::types::Value> = vec![u.to_owned().into()];
        values.extend(row.iter().map(sql_value));
        c.prepare_cached("INSERT INTO identity_uncertain VALUES (?,?,?,?)")?
            .execute(rusqlite::params_from_iter(values))?;
    }
    Ok(())
}

pub(crate) fn sql_value(v: &Value) -> rusqlite::types::Value {
    match v {
        Value::Null => rusqlite::types::Value::Null,
        Value::Bool(b) => (*b as i64).into(),
        Value::Number(n) if n.is_i64() => n.as_i64().expect("i64").into(),
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN).into(),
        Value::String(s) => s.clone().into(),
        other => other.to_string().into(),
    }
}

// ---- 人工结论：裁决历史 + 派生表 `assertions` ----

/// hybrid 下 `assertions` 上的触发器语义（`identity.activate`）：写一条结论就删掉该 (图, 身份) 的 scope / suspended，
/// 所在等价组多于一个成员时按组成员重写 scope。切换后 `assertions` 上没有这些触发器，由这里代替。
pub(crate) fn scope_on_insert(c: &Connection, u: &str, e: &str) -> Result<()> {
    c.prepare_cached("DELETE FROM identity_scopes WHERE uuid=? AND entity=?")?.execute([u, e])?;
    c.prepare_cached("DELETE FROM identity_suspended WHERE uuid=? AND entity=?")?.execute([u, e])?;
    c.prepare_cached("INSERT INTO identity_scopes SELECT ?1,?2,json_group_array(entity)
          FROM (SELECT entity FROM identity_canonical WHERE canonical=(SELECT canonical FROM identity_canonical WHERE entity=?2) ORDER BY entity)
          HAVING count(*)>1")?.execute(
        [u, e],
    )?;
    Ok(())
}

fn scope_on_delete(c: &Connection, u: &str, e: &str) -> Result<()> {
    c.prepare_cached("DELETE FROM identity_scopes WHERE uuid=? AND entity=?")?.execute([u, e])?;
    c.prepare_cached("DELETE FROM identity_suspended WHERE uuid=? AND entity=?")?.execute([u, e])?;
    Ok(())
}

/// 这一步的裁决写到哪里。
#[derive(Clone, Copy)]
pub(crate) enum At {
    Action(i64),
    Batch(i64, i64),
}

fn term_id(c: &Connection, key: &str) -> Result<i64> {
    c.prepare_cached("SELECT t.id FROM terms t JOIN term_tree r ON r.term_id=t.id WHERE t.key=?")?
        .query_row([key], |r| r.get(0))
        .optional()?
        .ok_or_else(|| Error::Conflict(format!("身份 {key} 不在词条核心里")))
}

fn asset_id(c: &Connection, uuid: &str) -> Result<i64> {
    if let Some(id) = crate::store::library_asset(c, uuid)? {
        return Ok(id);
    }
    // 已移出图库的图也可以留着人工结论（S2 同样登记为已移除）。
    if let Some(id) = c
        .prepare_cached("SELECT id FROM term_assets WHERE namespace='library' AND key=?")?
        .query_row([uuid], |r| r.get(0))
        .optional()?
    {
        return Ok(id);
    }
    c.prepare_cached("INSERT INTO term_assets(namespace,key,removed_at) VALUES ('library',?,CURRENT_TIMESTAMP)")?
        .execute([uuid])?;
    Ok(c.last_insert_rowid())
}

/// 追加一条 (图, 身份) 裁决。verdict：yes / no / clear；via 是 `assertions.origin`。
pub(crate) fn verdict(c: &Connection, at: At, uuid: &str, entity: &str, verdict: &str, via: &str) -> Result<()> {
    let (action, batch, anchor) = match at {
        At::Action(a) => (Some(a), None, a),
        At::Batch(b, anchor) => (None, Some(b), anchor),
    };
    let origin = if via == "album" && batch.is_some() { "album" } else { "user" };
    c.prepare_cached("INSERT INTO term_verdicts(action_id,batch_id,anchor,asset_id,term_id,verdict,origin,via) VALUES (?,?,?,?,?,?,?,?)")?.execute(
        params![action, batch, anchor, asset_id(c, uuid)?, term_id(c, entity)?, verdict, origin, via],
    )?;
    Ok(())
}

/// `INSERT OR REPLACE INTO assertions VALUES (u,e,status,origin)`，连同触发器语义与裁决历史。
pub(crate) fn put_assertion(c: &Connection, at: At, u: &str, e: &str, status: &str, origin: &str) -> Result<()> {
    c.prepare_cached("INSERT OR REPLACE INTO assertions VALUES (?,?,?,?)")?.execute([u, e, status, origin])?;
    if hybrid(c)? {
        scope_on_insert(c, u, e)?;
    }
    verdict(c, at, u, e, if status == "confirmed" { "yes" } else { "no" }, origin)
}

/// `DELETE FROM assertions WHERE uuid=? AND entity=?`（有行时才有触发器效果），连同裁决历史（clear）。
pub(crate) fn delete_assertion(c: &Connection, at: At, u: &str, e: &str, via: &str) -> Result<()> {
    if c.prepare_cached("DELETE FROM assertions WHERE uuid=? AND entity=?")?.execute([u, e])? > 0 && hybrid(c)? {
        scope_on_delete(c, u, e)?;
    }
    verdict(c, at, u, e, "clear", via)
}

/// 撤销后：`assertions` 的这一行 = 核心里的有效裁决（与旧实现按快照 DELETE + INSERT 的效果相同，含触发器语义）。
pub(crate) fn refresh_assertion(c: &Connection, u: &str, e: &str) -> Result<()> {
    let effective: Option<(String, String)> = c.prepare_cached("SELECT v.verdict,v.via FROM term_effective_verdicts v JOIN term_assets a ON a.id=v.asset_id AND a.namespace='library'
              JOIN terms t ON t.id=v.term_id WHERE a.key=? AND t.key=?")?.query_row(
            [u, e],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let existed = c.prepare_cached("DELETE FROM assertions WHERE uuid=? AND entity=?")?.execute([u, e])? > 0;
    let identity = hybrid(c)?;
    if existed && identity {
        scope_on_delete(c, u, e)?;
    }
    if let Some((v, via)) = effective {
        c.prepare_cached("INSERT INTO assertions VALUES (?,?,?,?)")?.execute([
            u,
            e,
            if v == "yes" { "confirmed" } else { "rejected" },
            via.as_str(),
        ])?;
        if identity {
            scope_on_insert(c, u, e)?;
        }
    }
    Ok(())
}

// ---- 步骤 ----

/// 先占一个 action 行（id 与旧版一致：旧版在同一事务末尾插入，中间没有别的步骤），最后写入快照。
pub(crate) fn begin_action(c: &Connection, label: &str) -> Result<i64> {
    if label.trim().is_empty() {
        return invalid("缺少历史文案");
    }
    c.prepare_cached("INSERT INTO actions(label,before_json) VALUES (?,'[]')")?.execute([label])?;
    Ok(c.last_insert_rowid())
}

/// 按张数写提示的步骤（`undo_labels`）：提示之外，种类和张数存进 `label_kind`、`label_count`，读的一边用它们把提示里的
/// 角色换成现在的名字（核心第 8 版，#201）。
fn begin_counted(c: &Connection, kind: UndoLabel, n: usize, name: &str, other: &str) -> Result<i64> {
    let count = n.to_string();
    let action = begin_action(c, &kind.fill(&count, name, other))?;
    c.prepare_cached("UPDATE actions SET label_kind=?,label_count=? WHERE id=?")?.execute(params![
        kind.name(),
        count,
        action
    ])?;
    Ok(action)
}

pub(crate) fn finish_action(c: &Connection, action: i64, kind: &str, before: Vec<Value>) -> Result<()> {
    let envelope = json!([{"terms": {"v": 2, "type": kind, "before": before}}]);
    c.prepare_cached("UPDATE actions SET before_json=? WHERE id=?")?.execute(params![envelope.to_string(), action])?;
    Ok(())
}

/// 写好文案之后改 action 行的文案（文案要用写完之后的名字时）。
pub(crate) fn relabel(c: &Connection, action: i64, label: &str) -> Result<()> {
    c.prepare_cached("UPDATE actions SET label=? WHERE id=?")?.execute(params![label, action])?;
    Ok(())
}

/// 调用方按筛选条件算好的选择集；旧代码在这一步拒绝的，调用方交来拒绝文案，在这里报出。
fn asked(c: &Connection, args: &Value, confirm: bool) -> Result<Vec<String>> {
    if let Some(refused) = args.get("asked_refused").and_then(Value::as_str) {
        return Err(Error::Invalid(refused.into()));
    }
    resolve_ids(c, &args["asked"], confirm, false)
}

fn refresh(c: &Connection, ids: &[String]) -> Result<()> {
    character_projection::memberships(c, Some(ids))
}

// ---- change ----

fn change(c: &Connection, args: &Value) -> Result<Value> {
    let action = text(&args["action"], "未知操作")?;
    if !matches!(action, "confirmed" | "rejected" | "unknown" | "resume" | "add_variant" | "remove_variant") {
        return invalid("未知操作");
    }
    let entities = args.get("entities").filter(|v| !v.is_null());
    let entity = args.get("entity").cloned().unwrap_or(Value::Null);
    if let Some(list) = entities {
        let n = list.as_array().map(Vec::len).unwrap_or(0);
        if action != "confirmed" || !entity.is_null() || !list.is_array() || !(1..=20).contains(&n) {
            return invalid("请选择角色");
        }
    }
    let confirm = args["confirm"].as_bool().unwrap_or(false);
    // asked：调用方按筛选条件选好、不含复制品的图；ids：展开复制品。
    let asked = asked(c, args, confirm)?;
    let mut ids = resolve_ids(c, &json!(asked), true, true)?;
    let mut targets: Vec<(String, Vec<String>)> = Vec::new();
    if matches!(action, "confirmed" | "rejected" | "add_variant" | "remove_variant") {
        let requested: Vec<Value> = match entities {
            Some(list) => list.as_array().cloned().unwrap_or_default(),
            None => vec![entity.clone()],
        };
        for requested in requested {
            let (target, parent) = require_entity(c, &requested)?;
            if matches!(action, "add_variant" | "remove_variant") && parent.is_none() {
                return invalid("请选择子标签");
            }
            let scope = match (&parent, action) {
                (Some(parent), "confirmed" | "add_variant") => {
                    let removed: BTreeSet<String> = c
                        .prepare_cached("SELECT uuid FROM assertions WHERE entity=? AND status='rejected'")?
                        .query_map([parent], |r| r.get(0))?
                        .collect::<rusqlite::Result<_>>()?;
                    if asked.iter().any(|a| removed.contains(a)) {
                        return invalid("请先恢复该图片的父角色归属");
                    }
                    ids.iter().filter(|i| !removed.contains(*i)).cloned().collect()
                }
                _ => ids.clone(),
            };
            if !targets.iter().any(|(t, _)| *t == target) {
                targets.push((target, scope));
            }
        }
    }
    let mut before = Vec::new();
    for (entity, scope) in &targets {
        for ident in scope {
            before.push(snapshot(c, ident, Some(entity))?);
        }
    }
    if before.is_empty() {
        for ident in &ids {
            before.push(snapshot(c, ident, None)?);
        }
    }
    let status = match action {
        "add_variant" => "confirmed",
        "remove_variant" => "rejected",
        other => other,
    };
    if !targets.is_empty() {
        let used: BTreeSet<&String> = targets.iter().flat_map(|(_, s)| s).collect();
        ids.retain(|i| used.contains(i));
    }
    let mut shown = Shown::new();
    let mut names = Vec::new();
    for (t, _) in &targets {
        names.push(shown.name(c, t)?);
    }
    let (name, n) = (names.join("、"), works(c, &ids)?);
    let step = match action {
        "confirmed" => begin_counted(c, UndoLabel::Set, n, &name, "")?,
        "rejected" => begin_counted(c, UndoLabel::Remove, n, &name, "")?,
        "unknown" => begin_action(c, &format!("已跳过 {n} 张"))?,
        "resume" => begin_action(c, &format!("已恢复 {n} 张"))?,
        "add_variant" => begin_counted(c, UndoLabel::Add, n, &name, "")?,
        _ => begin_counted(c, UndoLabel::Drop, n, &name, "")?,
    };
    for (entity, scope) in &targets {
        for ident in scope {
            put_assertion(c, At::Action(step), ident, entity, status, "review")?;
            clear_uncertain(c, ident, step)?;
        }
    }
    for ident in &ids {
        if action == "unknown" {
            c.prepare_cached("INSERT OR IGNORE INTO deferred VALUES (?)")?.execute([ident])?;
            crate::human_conclusions::state::present(
                c,
                crate::human_conclusions::state::Domain::Deferred,
                ident,
                step,
            )?;
        } else {
            clear_deferred(c, ident, step, action == "resume")?;
        }
    }
    refresh(c, &ids)?;
    finish_action(c, step, "snapshot", before)?;
    Ok(json!({"action": step}))
}

// ---- answer / answer_audit ----

/// 一次回答怎么记 `no` 里的图。A 是请求的 `entity`，B 是对手角色：解析请求时是请求里的值，核对之后是角色 ID。
enum Mode<B> {
    /// 只回答 A：`yes` 是 A，`no` 不是 A。
    Plain,
    /// 「是 B」「点掉的是…」（`switch`）：`no` 不是 A，是 B。
    Swap(B),
    /// 「可能认错」的「都不是」（`also_no`）：`no` 既不是 A，也不是 B，和 A 的否定是同一步。
    AlsoNo(B),
}

fn answer(c: &Connection, args: &Value) -> Result<Value> {
    let kind = text(&args["kind"], "回答格式不正确")?;
    if !matches!(kind, "ambiguous" | "seed" | "sample" | "audit" | "hint") {
        return invalid("回答格式不正确");
    }
    let yes = strings(&args["yes"], "回答格式不正确")?;
    let no = strings(&args["no"], "回答格式不正确")?;
    let later = strings(&args["later"], "回答格式不正确")?;
    let total = yes.len() + no.len() + later.len();
    let unique: BTreeSet<&String> = yes.iter().chain(&no).chain(&later).collect();
    if total == 0 || total > 60 || unique.len() != total {
        return invalid("回答格式不正确");
    }
    // 读库之前拒绝混合请求：`switch` 要非空的 `no`、空的 `yes`；`also_no` 只用于「可能认错」，整张卡单独提交，
    // 只带 `no`。
    let given = |key: &str| args.get(key).filter(|v| !v.is_null());
    let mode = match (given("switch"), given("also_no")) {
        (None, None) => Mode::Plain,
        (Some(b), None) if !no.is_empty() && yes.is_empty() => Mode::Swap(b),
        (None, Some(b)) if kind == "ambiguous" && !no.is_empty() && yes.is_empty() && later.is_empty() => {
            Mode::AlsoNo(b)
        }
        _ => return invalid("回答格式不正确"),
    };
    let (entity, parent) = require_entity(c, &args["entity"])?;
    if parent.is_some() {
        return invalid("请选择父角色");
    }
    let rival = |b: &Value| -> Result<String> {
        let (b, parent) = require_entity(c, b)?;
        if parent.is_some() || b == entity {
            return invalid("请选择不同的父角色");
        }
        Ok(b)
    };
    let mode = match mode {
        Mode::Plain => Mode::Plain,
        Mode::Swap(b) => Mode::Swap(rival(b)?),
        Mode::AlsoNo(b) => Mode::AlsoNo(rival(b)?),
    };
    let mut writes: Vec<(Vec<String>, String, &str)> = Vec::new();
    for (asked, status) in [(&yes, "confirmed"), (&no, "rejected")] {
        if asked.is_empty() {
            continue;
        }
        let once = resolve_ids(c, &json!(asked), true, false)?;
        let scope = resolve_ids(c, &json!(once), true, true)?;
        writes.push((scope.clone(), entity.clone(), status));
        if status == "rejected" {
            match &mode {
                Mode::Plain => {}
                Mode::Swap(b) => writes.push((scope, b.clone(), "confirmed")),
                Mode::AlsoNo(b) => {
                    require_ambiguous_pair(c, &once, &entity, b)?;
                    writes.push((scope, b.clone(), "rejected"));
                }
            }
        }
    }
    let mut step = None;
    if !writes.is_empty() {
        let mut before = Vec::new();
        for (scope, e, _) in &writes {
            for ident in scope {
                before.push(snapshot(c, ident, Some(e))?);
            }
        }
        // 只有「都不是」在撤销时放回它覆盖的旧问答；别的回答撤销后照旧只删掉这一步的问答。
        if let Mode::AlsoNo(b) = &mode {
            for ident in &no {
                for e in [&entity, b] {
                    before.extend(ask_history(c, ident, e)?);
                }
            }
        }
        let mut shown = Shown::new();
        let name = shown.name(c, &entity)?;
        let action = if yes.is_empty() {
            match &mode {
                Mode::Plain => begin_counted(c, UndoLabel::Remove, no.len(), &name, "")?,
                Mode::Swap(b) => begin_counted(c, UndoLabel::Swap, no.len(), &name, &shown.name(c, b)?)?,
                Mode::AlsoNo(b) => {
                    let both = format!("{name}、{}", shown.name(c, b)?);
                    begin_counted(c, UndoLabel::Remove, no.len(), &both, "")?
                }
            }
        } else {
            // 写成整句，页面才能逐句翻译（web/static/i18n.js tb()）。
            let n = yes.len();
            begin_action(
                c,
                &match no.len() {
                    0 => format!("已确认 {n} 张是 {name}"),
                    removed => format!("已确认 {n} 张是 {name}，移出 {removed} 张"),
                },
            )?
        };
        let mut touched: Vec<String> = Vec::new();
        for (scope, e, status) in &writes {
            for ident in scope {
                put_assertion(c, At::Action(action), ident, e, status, "review")?;
                clear_uncertain(c, ident, action)?;
                clear_deferred(c, ident, action, false)?;
                if !touched.contains(ident) {
                    touched.push(ident.clone());
                }
            }
        }
        refresh(c, &touched)?;
        finish_action(c, action, "snapshot", before)?;
        step = Some(action);
    }
    let mut put =
        c.prepare_cached("INSERT OR REPLACE INTO asked(uuid,entity,kind,answer,action_id) VALUES (?,?,?,?,?)")?;
    let mut answers = vec![(&yes, entity.as_str(), "yes"), (&no, &entity, "no"), (&later, &entity, "later")];
    if let Mode::AlsoNo(b) = &mode {
        answers.push((&no, b, "no"));
    }
    for (ids, e, answer) in answers {
        for i in ids {
            put.execute(params![i, e, kind, answer, if answer != "later" { step } else { None }])?;
        }
    }
    Ok(json!({"action": step}))
}

/// 卡片可能在另一次裁决或识别刷新后过期，核对原选区上的有向角色对。
fn require_ambiguous_pair(c: &Connection, ids: &[String], entity: &str, rival: &str) -> Result<()> {
    let mut stmt = c.prepare_cached(&format!(
        "SELECT 1 FROM ambiguous_claims WHERE uuid=? AND entity=? AND rival=? AND {}",
        crate::character_identity::asks_about_people("entity", "rival"),
    ))?;
    for id in ids {
        if stmt.query_row(params![id, entity, rival], |_| Ok(())).optional()?.is_none() {
            return invalid("图片的备选归属已变更，请刷新");
        }
    }
    Ok(())
}

/// 问答快照存的 `asked` 列：拍快照（`ask_history`）和撤销时放回（`restore_ask_history`）都照这个次序。
/// 这个次序已经写进图库里的撤销快照，只能在末尾追加列，不能重排或删列，否则旧快照会按错位解释。
const ASKED_COLUMNS: [&str; 6] = ["uuid", "entity", "kind", "answer", "action_id", "asked_at"];

/// 一步覆盖（图，角色）的问答之前 `asked` 里原来的那一行，包成撤销快照 `{"ask_history": […]}`；没有时为 `None`。
fn ask_history(c: &Connection, uuid: &str, entity: &str) -> Result<Option<Value>> {
    let sql = format!("SELECT {} FROM asked WHERE uuid=? AND entity=?", ASKED_COLUMNS.join(","));
    let row = row_json(c, &sql, &[&uuid, &entity])?;
    Ok((!row.is_null()).then(|| json!({"ask_history": row})))
}

/// 撤销时放回 `ask_history` 存下的一行。这一步之后同一对又记下的问答（只会是不进撤销的「以后再说」）保留。
fn restore_ask_history(c: &Connection, row: &Value) -> Result<()> {
    let row = row
        .as_array()
        .filter(|row| row.len() == ASKED_COLUMNS.len())
        .ok_or_else(|| Error::invalid("问答快照不完整"))?;
    let marks = ["?"; ASKED_COLUMNS.len()].join(",");
    let sql = format!("INSERT OR IGNORE INTO asked({}) VALUES ({marks})", ASKED_COLUMNS.join(","));
    c.prepare_cached(&sql)?.execute(rusqlite::params_from_iter(row.iter().map(sql_value)))?;
    Ok(())
}

fn answer_audit(c: &Connection, args: &Value) -> Result<Value> {
    let yes = strings(&args["yes"], "回答格式不正确")?;
    let no = strings(&args["no"], "回答格式不正确")?;
    let ids: Vec<&String> = yes.iter().chain(&no).collect();
    let unique: BTreeSet<&&String> = ids.iter().collect();
    if ids.is_empty() || ids.len() > 500 || unique.len() != ids.len() {
        return invalid("回答格式不正确");
    }
    let mut rows = Vec::new();
    for (list, status) in [(&yes, "confirmed"), (&no, "rejected")] {
        for u in list {
            let e: Option<String> = c
                .prepare_cached("SELECT entity FROM audit_sample WHERE uuid=?")?
                .query_row([u], |r| r.get(0))
                .optional()?;
            let Some(e) = e else { return invalid("有图片不在随机抽查里") };
            rows.push((u.clone(), e, status));
        }
    }
    let mut writes = Vec::new();
    for (u, e, st) in &rows {
        for i in resolve_ids(c, &json!([u]), true, true)? {
            writes.push((i, e.clone(), *st));
        }
    }
    let mut touched: Vec<String> = Vec::new();
    for (i, _, _) in &writes {
        if !touched.contains(i) {
            touched.push(i.clone());
        }
    }
    let before = writes.iter().map(|(i, e, _)| snapshot(c, i, Some(e))).collect::<Result<Vec<_>>>()?;
    let label = match (yes.len(), no.len()) {
        (0, 0) => "随机抽查：".to_owned(),
        (yes, 0) => format!("随机抽查：{yes} 张是"),
        (0, no) => format!("随机抽查：{no} 张不是"),
        (yes, no) => format!("随机抽查：{yes} 张是，{no} 张不是"),
    };
    let action = begin_action(c, &label)?;
    for (i, e, st) in &writes {
        put_assertion(c, At::Action(action), i, e, st, "review")?;
    }
    for u in &touched {
        clear_uncertain(c, u, action)?;
        clear_deferred(c, u, action, false)?;
    }
    refresh(c, &touched)?;
    finish_action(c, action, "snapshot", before)?;
    for (u, e, st) in &rows {
        c.prepare_cached("INSERT OR REPLACE INTO asked(uuid,entity,kind,answer,action_id) VALUES (?,?,'audit',?,?)")?
            .execute(params![u, e, if *st == "confirmed" { "yes" } else { "no" }, action])?;
    }
    Ok(json!({"action": action}))
}

// ---- reassign ----

fn reassign(c: &Connection, args: &Value) -> Result<Value> {
    let wanted: Vec<Value> = match &args["new_entity"] {
        Value::Array(list) => list.clone(),
        other => vec![other.clone()],
    };
    if wanted.is_empty() || wanted.len() > 20 {
        return invalid("请选择要换成的角色");
    }
    let (old, old_parent) = require_entity(c, &args["old_entity"])?;
    let news = wanted.iter().map(|x| require_entity(c, x)).collect::<Result<Vec<_>>>()?;
    let mut new_ids: Vec<String> = Vec::new();
    for (n, _) in &news {
        if !new_ids.contains(n) {
            new_ids.push(n.clone());
        }
    }
    if old_parent.is_some() || news.iter().any(|(_, p)| p.is_some()) || new_ids.contains(&old) {
        return invalid("请选择不同的父角色");
    }
    let confirm = args["confirm"].as_bool().unwrap_or(false);
    let asked = asked(c, args, confirm)?;
    let ids = resolve_ids(c, &json!(asked), true, true)?;
    for ident in &asked {
        if c.prepare_cached("SELECT 1 FROM memberships WHERE uuid=? AND entity=?")?
            .query_row([ident, &old], |_| Ok(()))
            .optional()?
            .is_none()
        {
            return invalid("原角色归属已变化，请刷新图片");
        }
    }
    let mut shown = Shown::new();
    let mut names = vec![shown.name(c, &old)?];
    for n in &new_ids {
        names.push(shown.name(c, n)?);
    }
    let action = begin_counted(c, UndoLabel::Swap, works(c, &ids)?, &names[0], &names[1..].join("、"))?;
    let mut before = Vec::new();
    for ident in &ids {
        let pairs =
            std::iter::once((old.as_str(), "rejected")).chain(new_ids.iter().map(|n| (n.as_str(), "confirmed")));
        for (entity, status) in pairs {
            before.push(snapshot(c, ident, Some(entity))?);
            put_assertion(c, At::Action(action), ident, entity, status, "review")?;
        }
        clear_uncertain(c, ident, action)?;
        clear_deferred(c, ident, action, false)?;
    }
    refresh(c, &ids)?;
    finish_action(c, action, "snapshot", before)?;
    Ok(json!({"action": action}))
}

// ---- sweep ----

fn sweep(c: &Connection, args: &Value) -> Result<Value> {
    let (entity, parent) = require_entity(c, &args["entity"])?;
    if parent.is_some() {
        return invalid("检查完了仅适用于父角色");
    }
    let ids: Vec<String> = c
        .prepare_cached("SELECT uuid FROM memberships WHERE entity=? AND status='machine' ORDER BY uuid")?
        .query_map([&entity], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let before = ids.iter().map(|i| snapshot(c, i, Some(&entity))).collect::<Result<Vec<_>>>()?;
    let action = begin_counted(c, UndoLabel::Sweep, works(c, &ids)?, &Shown::new().name(c, &entity)?, "")?;
    for i in &ids {
        put_assertion(c, At::Action(action), i, &entity, "confirmed", "sweep")?;
    }
    refresh(c, &ids)?;
    finish_action(c, action, "snapshot", before)?;
    c.prepare_cached("INSERT INTO reviews(entity,action_id) VALUES (?,?)")?.execute(params![entity, action])?;
    Ok(json!({"action": action}))
}

// ---- 身份未定 ----

fn uncertain(c: &Connection, args: &Value) -> Result<Value> {
    let options = args["options"].as_array().cloned().unwrap_or_default();
    let unique: BTreeSet<String> = options.iter().filter_map(Value::as_str).map(str::to_owned).collect();
    if options.len() < 2 || options.len() > 20 || unique.len() != options.len() {
        return invalid("未定状态需要 2–20 个不同身份候选");
    }
    let reason = args["reason"].as_str().map(str::trim).unwrap_or("");
    if reason.is_empty() {
        return invalid("请保存未定理由");
    }
    if !hybrid(c)? {
        return invalid("请先完成两级身份迁移");
    }
    for option in &options {
        require_entity(c, option)?;
    }
    let ids = asked(c, args, false)?;
    let before = ids.iter().map(|u| snapshot(c, u, None)).collect::<Result<Vec<_>>>()?;
    let action = begin_action(c, "已保留身份未定")?;
    let options = character_json::dump(&Value::Array(options), true);
    for u in &ids {
        c.prepare_cached("INSERT OR REPLACE INTO identity_uncertain(uuid,options_json,reason) VALUES (?,?,?)")?
            .execute(params![u, options, reason])?;
        crate::human_conclusions::state::present(c, crate::human_conclusions::state::Domain::Uncertain, u, action)?;
    }
    refresh(c, &ids)?;
    finish_action(c, action, "snapshot", before)?;
    Ok(json!({"action": action}))
}

fn clear_uncertain(c: &Connection, key: &str, action: i64) -> Result<()> {
    if c.prepare_cached("DELETE FROM identity_uncertain WHERE uuid=?")?.execute([key])? > 0 {
        crate::human_conclusions::state::absent(c, crate::human_conclusions::state::Domain::Uncertain, key, action)?;
    }
    Ok(())
}

fn clear_deferred(c: &Connection, key: &str, action: i64, explicit: bool) -> Result<()> {
    let changed = c.prepare_cached("DELETE FROM deferred WHERE uuid=?")?.execute([key])? > 0;
    if changed || explicit {
        crate::human_conclusions::state::absent(c, crate::human_conclusions::state::Domain::Deferred, key, action)?;
    }
    Ok(())
}

// ---- 锚点修正 ----
// 入口随 v2 删除（2026-09-30）；历史里已有的锚点步骤仍能撤销（undo_anchor）。

// ---- 分派 ----

pub fn command(c: &Connection, args: &Value) -> Result<Value> {
    hybrid(c)?; // 其他策略直接拒绝
    let op = text(&args["op"], "请选择命令")?;
    match op {
        "change" => change(c, args),
        "answer" => answer(c, args),
        "answer_audit" => answer_audit(c, args),
        "reassign" => reassign(c, args),
        "sweep" => sweep(c, args),
        "uncertain" => uncertain(c, args),
        "merge" => crate::character_merge::merge(c, args),
        "undo" => undo(c, args),
        "propose" | "decide" | "accept-many" | "rebuild" => crate::character_identity::command(c, args),
        other => match crate::character_structure::command(c, other, args)? {
            Some(result) => Ok(result),
            None => match crate::character_import::command(c, other, args)? {
                Some(result) => Ok(result),
                None => invalid("未知命令"),
            },
        },
    }
}

// ---- 统一撤销 ----

/// 一步 `before_json` 归不归角色核心撤：
/// - 新步骤：恰好一项 `{"terms":{"v":2,"type":…,"before":[…]}}`；
/// - 旧步骤：非空，每项都是角色的旧形状（快照、合并、身份关系、锚点、资料）。
///
/// `[]`、`curation`、`identity_skip`、标签、画师仍归各自的主人；角色形状与别的形状混在一步里、或形状不认识，拒绝。
/// tests/reference/web/terms_core.py（#9 已删除） 的 `character_owns` 用同一条规则。
pub fn owns(before: &Value) -> Result<Option<Vec<Value>>> {
    let Some(items) = before.as_array() else { return Ok(None) };
    if items.len() == 1 {
        if let Some(t) = items[0].get("terms").filter(|t| t.get("v").is_some()) {
            if items[0].as_object().map(Map::len) != Some(1)
                || t.get("v") != Some(&json!(2))
                || !t["type"].is_string()
                || !t["before"].is_array()
            {
                return invalid("未知或混合撤销形状");
            }
            return Ok(Some(t["before"].as_array().cloned().unwrap_or_default()));
        }
    }
    if items.is_empty() {
        return Ok(None);
    }
    let ours = items.iter().filter(|i| shape(i).is_some()).count();
    if ours == 0 {
        return Ok(None);
    }
    if ours != items.len() {
        return invalid("未知或混合撤销形状");
    }
    Ok(Some(items.clone()))
}

fn has_keys(v: &Value, keys: &[&str]) -> bool {
    v.as_object().is_some_and(|o| o.len() == keys.len() && keys.iter().all(|k| o.contains_key(*k)))
}

/// 旧形状的种类。
fn shape(item: &Value) -> Option<&'static str> {
    let o = item.as_object()?;
    if o.len() == 1 {
        for key in ["merge", "identity_relation", "identity_anchor", "profile"] {
            if o.contains_key(key) {
                return Some(key);
            }
        }
        return None;
    }
    if has_keys(item, &["uuid", "entity", "assertion", "deferred"])
        || has_keys(item, &["uuid", "entity", "assertion", "deferred", "identity"])
    {
        return Some("snapshot");
    }
    None
}

/// v2 信封新增的快照形状，裸列表不认。
fn edit_shape(item: &Value) -> Option<&'static str> {
    [
        "character_kind",
        "character_mascot",
        "ask_history",
        "identity_form",
        "identity_pair",
        "catalog_change",
        "catalog_split",
    ]
    .into_iter()
    .find(|key| has_keys(item, &[*key]))
}

/// 撤销一步角色步骤（`args["action"]`），在调用方的事务里执行。它必须是最近一个没撤销的步骤，否则是冲突；
/// 不归角色核心（见 [`owns`]）或有认不出的快照形状时拒绝。先删掉这一步记下的问答，再按快照恢复：每张图的结论、
/// 暂缓和身份范围，以及这一步覆盖的旧问答；合并、身份关系、锚点、资料和角色结构交给各自的撤销。
pub fn undo(c: &Connection, args: &Value) -> Result<Value> {
    let id = args["action"].as_i64().ok_or_else(|| Error::Invalid("步骤 ID 不正确".into()))?;
    let last: Option<i64> =
        c.prepare_cached("SELECT max(id) FROM actions WHERE undone=0")?.query_row([], |r| r.get(0))?;
    if last != Some(id) {
        return Err(Error::Conflict(crate::store::HISTORY_CHANGED.into()));
    }
    let before: String =
        c.prepare_cached("SELECT before_json FROM actions WHERE id=?")?.query_row([id], |r| r.get(0))?;
    let before = crate::strict_json::parse(&before).map_err(Error::Invalid)?;
    let Some(items) = owns(&before)? else { return invalid("这一步不归角色核心撤销") };
    for item in &items {
        if item.is_object() && shape(item).is_none() && edit_shape(item).is_none() {
            return invalid("未知或混合撤销形状");
        }
    }
    c.prepare_cached("DELETE FROM asked WHERE action_id=?")?.execute([id])?;
    c.prepare_cached("UPDATE actions SET undone=1 WHERE id=?")?.execute([id])?;
    let mut uuids: Vec<String> = Vec::new();
    let mut profile = false;
    let mut profile_links_changed = false;
    for item in &items {
        match shape(item).or_else(|| edit_shape(item)) {
            Some("snapshot") => {
                let u = text(&item["uuid"], "快照不完整")?;
                let e = item["entity"].as_str();
                if let Some(e) = e.filter(|e| !e.is_empty()) {
                    refresh_assertion(c, u, e)?;
                }
                c.prepare_cached("DELETE FROM deferred WHERE uuid=?")?.execute([u])?;
                if item["deferred"].as_bool().unwrap_or(false) {
                    c.prepare_cached("INSERT INTO deferred VALUES (?)")?.execute([u])?;
                }
                if let Some(snap) = item.get("identity") {
                    restore_scope(c, u, e, snap)?;
                }
                uuids.push(u.to_owned());
            }
            Some("merge") => crate::character_merge::undo(c, id, &item["merge"])?,
            Some("identity_relation") => crate::character_identity::undo_relation(c, &item["identity_relation"])?,
            Some("identity_pair") => crate::character_identity::undo_pair_relation(c, &item["identity_pair"])?,
            Some("catalog_change") => crate::character_import::undo_catalog_change(c, &item["catalog_change"])?,
            Some("catalog_split") => crate::character_import::undo_catalog_split(c, &item["catalog_split"])?,
            Some("identity_form") => crate::character_identity::undo_form(c, &item["identity_form"])?,
            Some("identity_anchor") => undo_anchor(c, &item["identity_anchor"])?,
            Some("profile") => {
                profile_links_changed |= crate::character_profile::undo(c, &item["profile"])?;
                profile = true;
            }
            Some("ask_history") => restore_ask_history(c, &item["ask_history"])?,
            Some("character_kind") => crate::character_structure::undo_kind(c, &item["character_kind"])?,
            Some("character_mascot") => crate::character_structure::undo_mascot(c, &item["character_mascot"])?,
            _ => return invalid("未知或混合撤销形状"),
        }
    }
    let mut seen = BTreeSet::new();
    uuids.retain(|u| seen.insert(u.clone()));
    if !uuids.is_empty() {
        refresh(c, &uuids)?;
    }
    if profile {
        crate::character_profile::after_undo(c, profile_links_changed)?;
    }
    c.prepare_cached("DELETE FROM reviews WHERE action_id=?")?.execute([id])?;
    crate::human_conclusions::state::undo(c, id)?;
    Ok(json!({"action": id, "status": "undone"}))
}

fn undo_anchor(c: &Connection, anchor: &Value) -> Result<()> {
    let entity = text(&anchor["entity"], "锚点快照不完整")?;
    let reference = text(&anchor["reference"], "锚点快照不完整")?;
    c.prepare_cached("DELETE FROM identity_anchor_decisions WHERE entity=? AND reference=?")?
        .execute([entity, reference])?;
    if let Some(previous) = anchor["previous"].as_array() {
        let mut values: Vec<rusqlite::types::Value> = vec![entity.to_owned().into(), reference.to_owned().into()];
        values.extend(previous.iter().map(sql_value));
        c.prepare_cached("INSERT INTO identity_anchor_decisions VALUES (?,?,?,?,?)")?
            .execute(rusqlite::params_from_iter(values))?;
    }
    c.prepare_cached("INSERT INTO identity_anchor_history(entity,reference,decision,reason) VALUES (?,?,'undo','撤销锚点修正；需要重建和校准')")?.execute(
        [entity, reference],
    )?;
    character_projection::memberships(c, None)
}
