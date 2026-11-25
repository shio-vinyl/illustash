//! HTTP 身份包装，共用现有的角色事务和撤销协议。
use crate::{app_commands, character_json, http_json::Json, Error, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};

fn argument(body: &Json, key: &str) -> Result<Value> {
    body.get(key)
        .map(|value| crate::strict_json::parse(&value.dump()).map_err(Error::Invalid))
        .unwrap_or(Ok(Value::Null))
}

pub(crate) fn request(c: &mut Connection, path: &str, body: &Json) -> Result<Json> {
    let command = match path {
        "/api/identity/skip" => return skip(c, body),
        "/api/identity/pair" => return pair(c, body),
        "/api/identity/decision" => {
            // 先检查关系 id 是不是正整数（旧实现 Store 的顺序），核心再检查策略和端点。
            let relation = argument(body, "relation_id")?;
            if !relation.is_null() && relation.as_i64().is_none_or(|id| id <= 0) {
                return Err(Error::Invalid("关系 ID 不正确".into()));
            }
            json!({"op":"decide", "a":argument(body,"a")?, "b":argument(body,"b")?,
                "kind":argument(body,"kind")?, "relation_id":relation,
                "status":argument(body,"status")?, "reason":argument(body,"reason")?,
                "evidence":argument(body,"evidence")?})
        }
        "/api/merge" => json!({"op":"merge", "source":argument(body,"source")?, "target":argument(body,"target")?}),
        "/api/catalog/split" => json!({
            "op":"catalog_split", "entity":argument(body,"entity")?,
            "owner":argument(body,"owner")?, "kind":argument(body,"kind")?
        }),
        _ => return Err(Error::Invalid("接口不存在".into())),
    };
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(app_commands::core_error)?;
    crate::schema::begin_writing(&tx).map_err(app_commands::core_error)?;
    crate::character_commands::command(&tx, &command).map_err(app_commands::core_error)?;
    tx.execute("INSERT INTO backend_effects(kind) VALUES ('verdict')", [])?;
    let undo = app_commands::commit_writing_reply(tx, app_commands::last_action).map_err(app_commands::core_error)?;
    Ok(Json::object([("undo", undo)]))
}

fn pair(c: &mut Connection, body: &Json) -> Result<Json> {
    let a = argument(body, "a")?;
    let b = argument(body, "b")?;
    let (a, b) = match (a.as_str(), b.as_str()) {
        (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() && a != b => (a, b),
        _ => return Err(Error::invalid("请选择两个角色")),
    };
    let answer = argument(body, "answer")?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    crate::schema::begin_writing(&tx).map_err(app_commands::core_error)?;
    match answer.as_str() {
        Some("resume") => {
            // 使用偏好原先保存的端点。角色可能已被改名、合并或挂作造型，解除仍然只碰这一对偏好。
            if !crate::character_identity::set_kept_apart(&tx, a, b, false)? {
                return Err(Error::invalid("这对角色没有设置不再追问"));
            }
            tx.execute(
                "INSERT INTO actions(label,before_json) VALUES (?,?)",
                params!["已允许再次询问这对角色", json!([{"identity_resumed":[a,b]}]).to_string()],
            )?;
        }
        Some("apart") => {
            crate::character_identity::require_pair(&tx, a, b).map_err(app_commands::core_error)?;
            if crate::character_identity::set_kept_apart(&tx, a, b, true)? {
                tx.execute(
                    "INSERT INTO actions(label,before_json) VALUES (?,?)",
                    params!["已分成两个角色，不再问这一对", json!([{"identity_kept_apart":[a,b]}]).to_string()],
                )?;
            }
        }
        Some(answer @ ("form" | "duplicate")) => {
            crate::character_identity::require_pair(&tx, a, b).map_err(app_commands::core_error)?;
            if crate::character_identity::kept_apart(&tx, a, b)? {
                return Err(Error::Invalid("这对角色已有整理决定，请刷新".into()));
            }
            if crate::character_projection::policy(&tx)? {
                let kind = if answer == "form" { "form_of" } else { "duplicate_of" };
                crate::character_commands::command(
                    &tx,
                    &json!({
                        "op":"decide", "a":a, "b":b, "kind":kind, "status":"accepted", "reason":"身份整理"
                    }),
                )
                .map_err(app_commands::core_error)?;
            } else if answer == "form" {
                crate::character_identity::attach_form(&tx, a, b).map_err(app_commands::core_error)?;
            } else {
                return Err(Error::Invalid("不支持此角色对的回答".into()));
            }
            tx.execute("INSERT INTO backend_effects(kind) VALUES ('verdict')", [])?;
        }
        _ => return Err(Error::Invalid("不支持此角色对的回答".into())),
    }
    let undo = app_commands::commit_writing_reply(tx, app_commands::last_action).map_err(app_commands::core_error)?;
    Ok(Json::object([("undo", undo)]))
}

fn skip(c: &mut Connection, body: &Json) -> Result<Json> {
    let a = argument(body, "a")?;
    let b = argument(body, "b")?;
    let (a, b) = match (a.as_str(), b.as_str()) {
        (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() && a != b => (a, b),
        _ => return Err(Error::Invalid("请选择两个角色".into())),
    };
    // 队列跳过有意接受任意两个不同的键，不解析角色或策略。
    let key = if a < b { format!("{a}|{b}") } else { format!("{b}|{a}") };
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let before: Option<String> =
        tx.query_row("SELECT value FROM prefs WHERE key='identity_skipped'", [], |r| r.get(0)).optional()?;
    let mut skipped: Vec<String> = serde_json::from_str(before.as_deref().filter(|s| !s.is_empty()).unwrap_or("[]"))
        .map_err(|e| Error::Invalid(e.to_string()))?;
    if !skipped.contains(&key) {
        skipped.push(key);
    }
    tx.execute(
        "INSERT OR REPLACE INTO prefs VALUES ('identity_skipped',?)",
        [character_json::dump(&json!(skipped), true)],
    )?;
    tx.execute(
        "INSERT INTO actions(label,before_json) VALUES (?,?)",
        params!["已暂不判断一对角色", json!([{"identity_skip":before}]).to_string()],
    )?;
    Ok(Json::object([("undo", app_commands::commit_undo(tx)?)]))
}
