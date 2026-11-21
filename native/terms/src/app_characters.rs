//! 角色裁决的 HTTP 参数与选区适配层；业务规则和历史记录用共享的核心。
use crate::{app_commands, character_commands, http::LibraryContext, http_json::Json, http_read, Error, Result};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};

pub(crate) fn request(c: &mut Connection, context: &LibraryContext, path: &str, body: &Json) -> Result<Json> {
    // 只解读会用到的字段。旧实现接受用不到的任意大小整数和非有限浮点数，这里照旧；把整个请求体经 serde_json 序列化会拒绝它们。
    let mut input = json!({});
    for key in ["entity", "action", "entities", "switch", "yes", "no", "later", "old", "new", "id"] {
        input[key] = argument(body.get(key));
    }
    if let Some(kind) = body.get("kind") {
        input["kind"] = argument(Some(kind));
    }
    let op = match path {
        "/api/change" => "change",
        "/api/audit/answer" => "answer_audit",
        "/api/ask/answer" => "answer",
        "/api/reassign" => "reassign",
        "/api/sweep" => "sweep",
        _ => return Err(Error::Invalid("接口不存在".into())),
    };
    let mut args = json!({"op":op,"entity":input["entity"],"action":input["action"],
        "entities":input["entities"],"switch":input["switch"],"confirm":true});
    if matches!(op, "answer" | "answer_audit") {
        for key in ["yes", "no", "later"] {
            args[key] = if character_commands::truthy(&input[key]) { input[key].clone() } else { json!([]) };
        }
        args["kind"] = input.get("kind").cloned().unwrap_or(json!("seed"));
        if op == "answer" {
            args["also_no"] = argument(body.get("also_no"));
        }
    }
    if op == "reassign" {
        args["old_entity"] = input["old"].clone();
        args["new_entity"] = input["new"].clone();
    }
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(app_commands::core_error)?;
    // 与 CLI 的守卫边界一致，包括它那些快速的裁决操作。
    if matches!(op, "change" | "sweep" | "answer") {
        tx.execute("INSERT OR REPLACE INTO meta(key,value) VALUES (?1,'1')", [crate::schema::WRITER_KEY])?;
    } else {
        crate::schema::begin_writing(&tx).map_err(app_commands::core_error)?;
    }
    if matches!(op, "change" | "reassign") {
        let selection = body.clone();
        // 业务上的选区拒绝留给核心处理，以保持它检查实体和参数的先后顺序。
        match app_commands::selected_scope(&tx, context, &selection, false) {
            Ok(ids) => args["asked"] = json!(ids),
            Err(Error::Invalid(message)) => args["asked_refused"] = json!(message),
            Err(e) => return Err(e),
        }
    }
    let out = character_commands::command(&tx, &args).map_err(app_commands::core_error)?;
    tx.execute("INSERT INTO backend_effects(kind) VALUES ('verdict')", [])?;
    crate::schema::commit_writing(tx).map_err(app_commands::core_error)?;
    http_read::register_names(c)?;
    // 只含 later 的回答没有操作，即使之前有可撤销的操作。
    let undo =
        if op == "answer" && out["action"].is_null() { Json::from(Value::Null) } else { app_commands::last_action(c)? };
    crate::app_effects::drain_after_commit(&context.db);
    Ok(Json::object([("undo", undo)]))
}

// 无法表示的数字当作为真的非字符串参数。核心在要求字符串或列表的地方都会拒绝它们，
// 而 HTTP 的 `value or []`（假值换成空列表）沿用旧实现的真值规则。
fn argument(value: Option<&Json>) -> Value {
    match value {
        None => Value::Null,
        Some(Json::Scalar(v)) => v.clone(),
        Some(Json::Float(v)) if v.is_finite() => json!(v),
        Some(Json::Float(_)) => json!(true),
        Some(Json::Integer(v)) => serde_json::from_str(v).unwrap_or(json!(true)),
        Some(Json::Array(values)) => Value::Array(values.iter().map(|v| argument(Some(v))).collect()),
        Some(Json::Object(fields)) => {
            Value::Object(fields.iter().map(|(k, v)| (k.clone(), argument(Some(v)))).collect())
        }
    }
}
