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
        _ => return Err(Error::Invalid("接口不存在".into())),
    };
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(app_commands::core_error)?;
    crate::schema::begin_writing(&tx).map_err(app_commands::core_error)?;
    crate::character_commands::command(&tx, &command).map_err(app_commands::core_error)?;
    tx.execute("INSERT INTO backend_effects(kind) VALUES ('verdict')", [])?;
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
