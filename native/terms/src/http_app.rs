//! 本地应用读取。会话元数据来自已有的私有启动状态。
use crate::{
    http::LibraryContext,
    http_json::{parse_python, Json},
    http_read::{display, rows, strings},
    sql_fragments::{hybrid, rating, visible, SWITCHED},
    undo_labels::UndoLabel,
    Error, Result,
};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

fn entity_name(c: &Connection, id: &str) -> Result<Option<String>> {
    let value = display(c, Json::object([("id", id.into())]))?;
    Ok(value.get("name").map(|v| v.text().to_owned()))
}
/// 最近一步尚可撤销的操作，连同 `action_label` 要读的字段。
pub(crate) const LAST_ACTION: &str = "
    SELECT id, label, ts, before_json, label_kind, label_count
    FROM actions
    WHERE undone = 0
    ORDER BY id DESC
    LIMIT 1";

/// 带计数的操作的种类和数量（core v8，#201）：随操作一起保存，更早的操作由升级补上。
fn counted(r: &Json) -> Option<(UndoLabel, String)> {
    let text = |key| match r.get(key) {
        Some(Json::Scalar(serde_json::Value::String(s))) => Some(s.as_str()),
        _ => None,
    };
    Some((UndoLabel::parse(text("label_kind")?)?, text("label_count")?.to_owned()))
}

pub(crate) fn action_summary(c: &Connection, r: &Json) -> Result<Json> {
    let (label, removal) = action_label(c, r)?;
    let mut summary =
        Json::object([("id", r.field("id").clone()), ("label", label.into()), ("ts", r.field("ts").clone())]);
    if let Some(removal) = removal {
        summary.set("removal", removal);
    }
    Ok(summary)
}

fn action_label(c: &Connection, r: &Json) -> Result<(String, Option<Json>)> {
    let label = r.field("label").text();
    let before = parse_python(r.field("before_json").text())?;
    let before = if let Json::Array(v) = &before {
        if v.len() == 1 && matches!(&v[0], Json::Object(fields) if fields.len()==1) {
            v[0].get("terms")
                .filter(|t| t.get("v").is_some_and(|v| v.integer() == 2))
                .and_then(|t| t.get("before"))
                .unwrap_or(&before)
        } else {
            &before
        }
    } else {
        &before
    };
    let Json::Array(before) = before else { return Err(Error::Invalid("历史快照应为列表".into())) };
    let merge = before.iter().find_map(|b| b.get("merge"));
    if let Some(merge) = merge.filter(|v| match v {
        Json::Object(v) => !v.is_empty(),
        Json::Array(v) => !v.is_empty(),
        Json::Scalar(v) => crate::character_commands::truthy(v),
        Json::Integer(v) => v != "0",
        Json::Float(v) => *v != 0.0,
    }) {
        let field = |key| {
            merge
                .get(key)
                .ok_or_else(|| Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, "历史合并快照缺少字段")))
        };
        if let (Some(left), Some(right)) =
            (entity_name(c, field("source")?.text())?, entity_name(c, field("target")?.text())?)
        {
            let moved = match field("moved")? {
                Json::Array(v) => v.len(),
                Json::Object(v) => v.len(),
                Json::Scalar(serde_json::Value::String(v)) => v.chars().count(),
                _ => return Err(Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, "历史移动列表无效"))),
            };
            return Ok((format!("已将 {left} 合并到 {right}（{moved} 条人工结论）"), None));
        }
    }
    let mut seen = HashSet::new();
    let mut ids: Vec<String> = before
        .iter()
        .filter_map(|b| b.get("entity"))
        .map(|v| v.text().to_owned())
        .filter(|v| !v.is_empty() && seen.insert(v.clone()))
        .collect();
    let counted = counted(r);
    if let Some((kind @ (UndoLabel::Swap | UndoLabel::Remove), n)) = counted.as_ref().filter(|_| ids.len() == 2) {
        if let (Some(left), Some(right)) = (entity_name(c, &ids[0])?, entity_name(c, &ids[1])?) {
            if *kind == UndoLabel::Swap {
                return Ok((kind.fill(n, &left, &right), None));
            }
            // 「都不是」要随页面语言连接两个名字；角色名本身也可含顿号，从快照的角色 ID 取名字。
            let removal =
                Json::object([("count", n.as_str().into()), ("names", Json::Array(vec![left.into(), right.into()]))]);
            return Ok((label.into(), Some(removal)));
        }
    }
    let sweep = counted.as_ref().filter(|(kind, _)| *kind == UndoLabel::Sweep).map(|(_, n)| n);
    if sweep.is_some() && ids.is_empty() {
        ids = strings(c, "SELECT entity FROM reviews WHERE action_id=?", &[&r.field("id").integer().to_string()])?;
    }
    if ids.len() == 1 {
        if let Some(name) = entity_name(c, &ids[0])? {
            if let Some(n) = sweep {
                return Ok((UndoLabel::Sweep.fill(n, &name, ""), None));
            }
            if let Some((kind, n)) = counted.as_ref().filter(|(kind, _)| *kind != UndoLabel::Swap) {
                return Ok((kind.fill(n, &name, ""), None));
            }
        }
    }
    Ok((label.into(), None))
}
pub fn read(c: &Connection, context: &LibraryContext, q: &HashMap<String, String>) -> Result<(Json, u16)> {
    let nsfw = q.get("nsfw").map(String::as_str).unwrap_or("");
    let rated = rating("a", nsfw)?;
    let visible = visible("a");
    let total: i64 = c.query_row(&format!("SELECT count(*) FROM assets a WHERE {visible}{rated}"), [], |r| r.get(0))?;
    let confirmed = if hybrid(c)? {
        "SELECT uuid FROM family_confirmed"
    } else {
        "SELECT uuid FROM assertions WHERE status='confirmed'"
    };
    let confirmed: i64 = c.query_row(
        &format!("SELECT count(DISTINCT uuid) FROM ({confirmed}) a JOIN assets USING(uuid) WHERE {visible}"),
        [],
        |r| r.get(0),
    )?;
    let indexed: String = c.query_row("SELECT value FROM meta WHERE key='indexed'", [], |r| r.get(0))?;
    let undo = if let Some(r) = rows(c, LAST_ACTION, &[])?.into_iter().next() {
        action_summary(c, &r)?
    } else {
        None::<String>.into()
    };
    let read_only = if let Some(reason) = &context.read_only {
        Some(reason.clone())
    } else {
        if c.query_row(SWITCHED, [], |r| r.get::<_, bool>(0))? {
            None
        } else {
            Some("图库正在切换到词条核心，完成前只读".into())
        }
    };
    Ok((
        Json::object([
            ("total", total.into()),
            ("confirmed", confirmed.into()),
            ("indexed", indexed.into()),
            ("undo", undo),
            ("token", context.token.clone().into()),
            ("read_only", read_only.into()),
        ]),
        200,
    ))
}
