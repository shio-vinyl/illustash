//! 保存画廊的条件、手选成员、封面和顺序。封面沿用同一事务内的图库选区查询。

use crate::{app_commands, http::LibraryContext, http_browse, http_json::Json, Error, Result};
use illustash_core::error::invalid;
use rusqlite::{
    params, params_from_iter, types::Value as SqlValue, Connection, OptionalExtension, TransactionBehavior,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

const SAVED: &[&str] =
    &["has", "not", "from", "to", "q", "tag", "utag", "work", "artist", "day", "like", "like_by", "order"];
const COLUMNS: &str = "id,name,params,cover,position,created_at,updated_at,kind";

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    #[default]
    Condition,
    Manual,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Condition => "condition",
            Self::Manual => "manual",
        }
    }

    fn is_condition(&self) -> bool {
        matches!(self, Self::Condition)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Row {
    id: i64,
    name: String,
    params: String,
    cover: Option<String>,
    position: i64,
    created_at: String,
    updated_at: String,
    #[serde(default, skip_serializing_if = "Kind::is_condition")]
    kind: Kind,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Before {
    id: i64,
    row: Option<Row>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    members: Option<Members>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Members {
    Added(Vec<String>),
    Removed(Vec<String>),
}

impl Row {
    fn load(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            params: row.get(2)?,
            cover: row.get(3)?,
            position: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
            kind: if row.get_ref(7)?.as_str()? == "manual" { Kind::Manual } else { Kind::Condition },
        })
    }

    fn params(&self) -> Result<HashMap<String, String>> {
        if matches!(self.kind, Kind::Manual) {
            let token = format!("gallery:{}", self.id);
            return Ok(HashMap::from([("has".into(), json!([token]).to_string())]));
        }
        serde_json::from_str(&self.params)
            .map_err(|e| Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))
    }

    fn describe(&self) -> Result<Json> {
        let params = if matches!(self.kind, Kind::Manual) {
            json!(self.params()?)
        } else {
            serde_json::from_str(&self.params)
                .map_err(|e| Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?
        };
        let mut gallery = Json::object([
            ("id", self.id.into()),
            ("name", self.name.clone().into()),
            ("params", params.into()),
            ("position", self.position.into()),
        ]);
        if matches!(self.kind, Kind::Manual) {
            gallery.set("kind", self.kind.as_str().into());
        }
        Ok(gallery)
    }

    fn require_manual(&self) -> Result<()> {
        if self.kind.is_condition() {
            return invalid("请选择手选画廊");
        }
        Ok(())
    }

    fn current_cover(&self, c: &Connection, context: &LibraryContext, nsfw: &str) -> Result<Option<String>> {
        let mut filter = self.params()?;
        if !nsfw.is_empty() {
            filter.insert("nsfw".into(), nsfw.into());
        }
        match http_browse::selection(c, context, &filter) {
            Ok((where_, args, order)) => self.resolve_cover(c, &where_, &args, &order),
            // 条件失效时仍能列出画廊供用户编辑，打开图片列表会按原接口报告具体的 400。
            Err(Error::Invalid(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn resolve_cover(&self, c: &Connection, where_: &str, args: &[SqlValue], order: &str) -> Result<Option<String>> {
        if let Some(cover) = &self.cover {
            let sql = format!("SELECT a.uuid FROM assets a WHERE {where_} AND a.uuid=?");
            let bound = args.iter().cloned().chain([SqlValue::Text(cover.clone())]);
            if let Some(valid) = c.query_row(&sql, params_from_iter(bound), |row| row.get(0)).optional()? {
                return Ok(Some(valid));
            }
        }
        let sql = format!("SELECT a.uuid FROM assets a WHERE {where_} ORDER BY {order} LIMIT 1");
        Ok(c.query_row(&sql, params_from_iter(args), |row| row.get(0)).optional()?)
    }
}

pub(crate) fn read(c: &Connection, nsfw: &str) -> Result<Json> {
    crate::sql_fragments::rating("a", nsfw)?;
    let mut statement = c.prepare(&format!("SELECT {COLUMNS} FROM galleries ORDER BY position,id"))?;
    let galleries = statement
        .query_map([], Row::load)?
        .map(|row| {
            let row = row?;
            let mut gallery = row.describe()?;
            gallery.set("cover_manual", row.cover.is_some().into());
            Ok(gallery)
        })
        .collect::<Result<_>>()?;
    Ok(Json::Array(galleries))
}

pub(crate) fn covers(c: &Connection, context: &LibraryContext, nsfw: &str, ids: &str) -> Result<Json> {
    crate::sql_fragments::rating("a", nsfw)?;
    serde_json::from_str::<Vec<i64>>(ids).map_err(|_| Error::invalid("画廊编号应为整数数组"))?;
    let sql =
        format!("SELECT {COLUMNS} FROM galleries WHERE id IN (SELECT value FROM json_each(?)) ORDER BY position,id");
    let galleries = c.prepare(&sql)?.query_map([ids], Row::load)?.collect::<rusqlite::Result<Vec<_>>>()?;
    let covers = galleries
        .iter()
        .map(|row| Ok(Json::object([("id", row.id.into()), ("cover", row.current_cover(c, context, nsfw)?.into())])))
        .collect::<Result<_>>()?;
    Ok(Json::Array(covers))
}

fn stored(c: &Connection, id: i64) -> Result<Row> {
    let sql = format!("SELECT {COLUMNS} FROM galleries WHERE id=?");
    c.query_row(&sql, [id], Row::load).optional()?.ok_or_else(|| Error::invalid("画廊不存在"))
}

fn text(json: &Json) -> Option<&str> {
    match json {
        Json::Scalar(Value::String(text)) => Some(text),
        _ => None,
    }
}

fn integer(json: &Json) -> Option<i64> {
    match json {
        Json::Scalar(Value::Number(number)) => number.as_i64(),
        Json::Integer(number) => number.parse().ok(),
        _ => None,
    }
}

fn canonical(c: &Connection, context: &LibraryContext, params: &Json) -> Result<BTreeMap<String, String>> {
    let bad = || Error::invalid("筛选参数不正确");
    let Json::Object(fields) = params else {
        return Err(bad());
    };
    let mut filter = HashMap::new();
    for (key, value) in fields {
        if !SAVED.contains(&key.as_str()) {
            return Err(bad());
        }
        filter.insert(key.to_owned(), text(value).ok_or_else(bad)?.to_owned());
    }
    http_browse::selection(c, context, &filter)?;
    for (key, terms) in ["has", "not"].into_iter().zip(http_browse::condition_lists(&filter)?) {
        if terms.is_empty() {
            filter.remove(key);
        } else {
            filter.insert(key.into(), serde_json::to_string(&terms).expect("筛选条件可序列化"));
        }
    }
    Ok(filter.into_iter().collect())
}

fn default_name(filter: &BTreeMap<String, String>) -> String {
    let terms = |key| {
        filter
            .get(key)
            .map(|raw| serde_json::from_str::<Vec<String>>(raw).expect("已规范化的筛选数组"))
            .unwrap_or_default()
    };
    let label = |value: String| -> String {
        match value.as_str() {
            "fav" => "收藏".into(),
            "source:pixiv" => "Pixiv".into(),
            "source:x" => "X".into(),
            "source:other" => "其他".into(),
            _ => value.split_once(':').map_or(value.clone(), |(_, key)| key.to_owned()),
        }
    };
    let mut name: Vec<_> = terms("has").into_iter().map(label).collect();
    name.extend(terms("not").into_iter().map(|term| format!("不含 {}", label(term))));
    name.extend(["q", "tag", "utag", "work", "artist", "day"].into_iter().filter_map(|key| filter.get(key).cloned()));
    let from = filter.get("from");
    let to = filter.get("to");
    if let (Some(from), Some(to)) = (from, to) {
        if from.ends_with("-01-01") && to.ends_with("-12-31") && from[..4] == to[..4] {
            name.push(from[..4].to_owned());
        } else {
            name.push(format!("{from} 至 {to}"));
        }
    } else {
        name.extend(from.or(to).cloned());
    }
    let joined = name.join(" · ");
    if joined.trim().is_empty() {
        "图库".into()
    } else {
        joined.chars().take(60).collect()
    }
}

fn name(body: &Json) -> Result<Option<String>> {
    body.get("name")
        .map(|value| {
            text(value)
                .map(crate::http_read::trim)
                .filter(|name| (1..=60).contains(&name.chars().count()))
                .map(str::to_owned)
                .ok_or_else(|| Error::invalid("画廊名称应为 1–60 个字符"))
        })
        .transpose()
}

fn change_members(c: &Connection, id: i64, ids: &[String], add: bool) -> Result<Vec<String>> {
    let ids = json!(ids).to_string();
    let sql = if add {
        "INSERT INTO gallery_members(gallery_id,uuid) SELECT ?1,value FROM json_each(?2) WHERE 1
         ON CONFLICT(gallery_id,uuid) DO NOTHING RETURNING uuid"
    } else {
        "DELETE FROM gallery_members WHERE gallery_id=?1 AND uuid IN (SELECT value FROM json_each(?2)) RETURNING uuid"
    };
    let mut stmt = c.prepare(sql)?;
    let changed = stmt.query_map(params![id, ids], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(changed)
}

fn create(c: &Connection, context: &LibraryContext, body: &Json, now: &str) -> Result<i64> {
    let kind = match body.get("kind") {
        None => Kind::Condition,
        Some(value) if text(value) == Some("condition") => Kind::Condition,
        Some(value) if text(value) == Some("manual") => Kind::Manual,
        _ => return invalid("画廊类型不正确"),
    };
    let (name, saved, ids) = match kind {
        Kind::Condition => {
            let filter = canonical(c, context, body.get("params").ok_or_else(|| Error::invalid("筛选参数不正确"))?)?;
            let name = name(body)?.unwrap_or_else(|| default_name(&filter));
            (name, json!(filter).to_string(), Vec::new())
        }
        Kind::Manual => {
            if body.get("params").is_some() {
                return invalid("手选画廊使用图片列表");
            }
            let name = name(body)?.ok_or_else(|| Error::invalid("画廊名称应为 1–60 个字符"))?;
            let ids = match (body.get("ids"), body.get("filter"), body.get("exclude")) {
                (None, None, None) => Vec::new(),
                (Some(Json::Array(ids)), None, None) if ids.is_empty() => Vec::new(),
                _ => app_commands::selected(c, context, body)?,
            };
            (name, "{}".into(), ids)
        }
    };
    let last: i64 = c.query_row("SELECT coalesce(max(position),-1) FROM galleries", [], |r| r.get(0))?;
    let position = last.checked_add(1).ok_or_else(|| Error::invalid("画廊顺序不正确"))?;
    c.execute(
        "INSERT INTO galleries(name,params,position,created_at,updated_at,kind) VALUES (?1,?2,?3,?4,?4,?5)",
        params![name, saved, position, now, kind.as_str()],
    )?;
    let id = c.last_insert_rowid();
    if !ids.is_empty() {
        change_members(c, id, &ids, true)?;
    }
    Ok(id)
}

pub(crate) fn write(c: &mut Connection, context: &LibraryContext, body: &Json) -> Result<Json> {
    app_commands::initialized(c)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let action = body.get("action").and_then(text).unwrap_or("");
    let id = || body.get("id").and_then(integer).ok_or_else(|| Error::invalid("画廊不存在"));
    let now: String = tx.query_row("SELECT CURRENT_TIMESTAMP", [], |r| r.get(0))?;
    let (before, label) = match action {
        "create" => {
            let id = create(&tx, context, body, &now)?;
            (Before { id, row: None, members: None }, "已保存画廊")
        }
        "update" => {
            let id = id()?;
            let before = stored(&tx, id)?;
            let name = name(body)?.unwrap_or_else(|| before.name.clone());
            let params = if let Some(params) = body.get("params") {
                if !before.kind.is_condition() {
                    return invalid("手选画廊使用图片列表");
                }
                serde_json::to_string(&canonical(&tx, context, params)?).expect("筛选参数可序列化")
            } else {
                before.params.clone()
            };
            let position = match body.get("position") {
                Some(value) => integer(value)
                    .filter(|n| (0..i64::MAX).contains(n))
                    .ok_or_else(|| Error::invalid("画廊顺序不正确"))?,
                None => before.position,
            };
            let cover = match body.get("cover") {
                None => before.cover.clone(),
                Some(Json::Scalar(Value::Null)) => None,
                Some(value) => {
                    let uuid = text(value).filter(|id| !id.is_empty()).ok_or_else(|| Error::invalid("图片不存在"))?;
                    let exists: bool =
                        tx.query_row("SELECT EXISTS(SELECT 1 FROM assets WHERE uuid=?)", [uuid], |r| r.get(0))?;
                    if !exists {
                        return invalid("图片不存在");
                    }
                    Some(uuid.to_owned())
                }
            };
            tx.execute(
                "UPDATE galleries SET name=?,params=?,cover=?,position=?,updated_at=? WHERE id=?",
                params![name, params, cover, position, now, id],
            )?;
            (Before { id, row: Some(before), members: None }, "已更新画廊")
        }
        "add" | "remove" => {
            let id = id()?;
            let before = stored(&tx, id)?;
            before.require_manual()?;
            let ids = app_commands::selected(&tx, context, body)?;
            let (members, label) = if action == "add" {
                (Members::Added(change_members(&tx, id, &ids, true)?), "已放进画廊")
            } else {
                (Members::Removed(change_members(&tx, id, &ids, false)?), "已移出画廊")
            };
            (Before { id, row: Some(before), members: Some(members) }, label)
        }
        "delete" => {
            let id = id()?;
            let before = stored(&tx, id)?;
            let members = if matches!(before.kind, Kind::Manual) {
                let mut stmt = tx.prepare("SELECT uuid FROM gallery_members WHERE gallery_id=? ORDER BY uuid")?;
                let ids = stmt.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                Some(Members::Removed(ids))
            } else {
                None
            };
            tx.execute("DELETE FROM galleries WHERE id=?", [id])?;
            (Before { id, row: Some(before), members }, "已删除画廊")
        }
        _ => return invalid("画廊操作不正确"),
    };
    let id = before.id;
    let snapshot = json!([{"gallery":before}]);
    tx.execute("INSERT INTO actions(label,before_json) VALUES (?,?)", [label, &snapshot.to_string()])?;
    let undo = app_commands::last_action(&tx)?;
    let mut result = Json::object([("undo", undo)]);
    if action != "delete" {
        // 写请求没有界面分级状态，返回隐藏 R18 的封面；按需封面读取使用调用方分级。
        let row = stored(&tx, id)?;
        let mut gallery = row.describe()?;
        gallery.set("cover", row.current_cover(&tx, context, "hide")?.into());
        gallery.set("cover_manual", row.cover.is_some().into());
        result.set("gallery", gallery);
    }
    tx.commit()?;
    Ok(result)
}

pub(crate) fn restore(c: &Connection, before: &Value) -> Result<()> {
    if before.get("row").is_none() {
        return Err(Error::Io(std::io::Error::other("画廊历史缺少原来的数据")));
    }
    let before: Before = serde_json::from_value(before.clone())
        .map_err(|e| Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
    if let Some(row) = before.row {
        if row.id != before.id || (row.kind.is_condition() && before.members.is_some()) {
            return Err(Error::Io(std::io::Error::other("画廊历史不一致")));
        }
        c.execute(
            "INSERT INTO galleries(id,name,params,cover,position,created_at,updated_at,kind) VALUES (?,?,?,?,?,?,?,?)
             ON CONFLICT(id) DO UPDATE SET name=excluded.name,params=excluded.params,cover=excluded.cover,
               position=excluded.position,created_at=excluded.created_at,updated_at=excluded.updated_at,kind=excluded.kind",
            params![row.id, row.name, row.params, row.cover, row.position, row.created_at, row.updated_at, row.kind.as_str()],
        )?;
        if let Some(members) = before.members {
            let (ids, add) = match members {
                Members::Added(ids) => (ids, false),
                Members::Removed(ids) => (ids, true),
            };
            change_members(c, row.id, &ids, add)?;
        }
    } else if before.members.is_some() {
        return Err(Error::Io(std::io::Error::other("画廊历史不一致")));
    } else {
        c.execute("DELETE FROM galleries WHERE id=?", [before.id])?;
    }
    Ok(())
}
