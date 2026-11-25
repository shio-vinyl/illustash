//! 原生的角色资料 HTTP 编排。核心改动和投影共用一个事务。
use crate::error::invalid;
use crate::{
    app_commands::{core_error, initialized, last_action},
    http_json::Json,
    http_read, Error, Result,
};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};

pub(crate) const ROUTES: &[&str] = &[
    "/api/entities",
    "/api/character/profile",
    "/api/local-links",
    "/api/works",
    "/api/character/mascot",
    "/api/kind",
    "/api/oc/shared",
];
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
    }
}
fn tag(v: &Value) -> Result<String> {
    let Some(s) = v.as_str() else { return invalid("请选择 Danbooru 角色 tag") };
    let s = http_read::trim(s);
    if s.is_empty() || s.chars().count() > 200 {
        return invalid("请选择 Danbooru 角色 tag");
    }
    let s = s.to_lowercase().replace(' ', "_");
    if s.chars().any(http_read::whitespace) {
        return invalid("请选择 Danbooru 角色 tag");
    }
    Ok(s)
}
fn oc(v: &Value) -> Result<(String, String)> {
    let kind = v.get("kind").unwrap_or(&Value::Null);
    let kind = if kind.is_null() && v.get("kind").is_none() { "" } else { kind.as_str().unwrap_or("invalid") };
    let artist = v.get("artist").and_then(Value::as_str).or_else(|| v.get("artist").is_none().then_some(""));
    let Some(artist) = artist else { return invalid("画师名称不正确") };
    let artist = http_read::trim(artist);
    if !matches!(kind, "" | "oc") {
        return invalid("未知角色类型");
    }
    if artist.chars().count() > 120 || (!artist.is_empty() && kind != "oc") {
        return invalid("画师名称不正确");
    }
    Ok((kind.into(), artist.into()))
}
fn shown(c: &Connection, id: &str) -> Result<Json> {
    http_read::display(c, Json::object([("id", id.into())]))
}
/// handler 读取的 `body` 字段；`items`（/api/local-links）里每个对象则取 `item_fields`。
fn picked(body: &Json, fields: &[&str], item_fields: &[&str]) -> Json {
    fn pick(v: &Json, keys: &[&str]) -> Json {
        Json::object(keys.iter().filter_map(|k| v.get(k).map(|v| (*k, v.clone()))))
    }
    let mut selected = pick(body, fields);
    if let (true, Some(Json::Array(items))) = (fields.contains(&"items"), body.get("items")) {
        let items = items.iter().map(|item| match item {
            Json::Object(_) => pick(item, item_fields),
            _ => item.clone(),
        });
        selected.set("items", Json::Array(items.collect()));
    }
    selected
}
/// /api/local-links 的 `items`：1–200 个对象。
fn link_items(v: &Value) -> Result<&Vec<Value>> {
    match v["items"].as_array().filter(|a| (1..=200).contains(&a.len()) && a.iter().all(Value::is_object)) {
        Some(items) => Ok(items),
        None => invalid("请选择要关联的角色"),
    }
}
fn names_version(c: &Connection) -> Result<String> {
    Ok(c.query_row("SELECT value FROM meta WHERE key='names_version'", [], |r| r.get(0))?)
}
pub(crate) struct Prepared {
    pub works: std::collections::HashMap<String, String>,
    version: String,
}
pub(crate) fn prepare(
    c: &Connection,
    body: &Json,
    path: &str,
    source: &mut crate::app_name_sources::Source,
) -> Result<Prepared> {
    let tagged = body.get("tag").is_some_and(crate::app_service::truthy);
    let fields: &[&str] = match path {
        "/api/entities" if tagged => &["tag"],
        "/api/character/profile" if tagged => &["tag", "work"],
        "/api/local-links" => &["items"],
        _ => &[],
    };
    let selected = picked(body, fields, &["tag"]);
    let v: Value = serde_json::from_str(&selected.dump()).map_err(|_| Error::Invalid("关联参数不正确".into()))?;
    let version = names_version(c)?;
    let mut tags = Vec::new();
    match path {
        "/api/entities" if truthy(&v["tag"]) => {
            let t = tag(&v["tag"])?;
            if crate::character_profile::tag_owner(c, &t, None)?.is_none() {
                tags.push(t);
            }
        }
        "/api/character/profile" if truthy(&v["tag"]) && v["work"].is_null() => tags.push(tag(&v["tag"])?),
        "/api/local-links" => {
            for item in link_items(&v)? {
                tags.push(tag(&item["tag"])?);
            }
        }
        _ => {}
    }
    let mut works = std::collections::HashMap::new();
    for t in tags {
        if let std::collections::hash_map::Entry::Vacant(entry) = works.entry(t.clone()) {
            let cp = if crate::character_profile::local_work(c, &[crate::character_profile::qualifier(&t)])?.is_some() {
                String::new()
            } else {
                source.copyright(&t)?
            };
            entry.insert(cp);
        }
    }
    Ok(Prepared { works, version })
}
fn copyright(c: &Connection, tag: &str, prepared: Option<&Prepared>) -> Result<String> {
    if crate::character_profile::local_work(c, &[crate::character_profile::qualifier(tag)])?.is_some() {
        return Ok(String::new());
    }
    prepared.and_then(|p| p.works.get(tag)).cloned().ok_or_else(|| Error::Invalid("版权查询尚未准备".into()))
}
fn run(c: &Connection, args: &Value) -> Result<Value> {
    crate::character_commands::command(c, args).map_err(core_error)
}
/// 核心命令回复里的 `id`。
fn made(out: &Value) -> String {
    out["id"].as_str().expect("validated internal text field").to_owned()
}
/// 名称请求与改动共用事务；原生更新器确认应用成功。
fn names(c: &Connection, id: Option<&str>) -> Result<()> {
    let kind = format!("names:{}", crate::character_json::dump(&json!({"entity":id,"refresh":false}), false));
    c.execute("INSERT INTO backend_effects(kind) VALUES (?)", [kind])?;
    Ok(())
}
/// 在写事务内构造的请求回复。
enum Reply {
    /// 早已存在的身份，在事务内渲染；什么都没改，所以没有要登记的名称。
    Existing(Json),
    /// 身份的显示内容，取自这些字段；没有字段时取自已存的身份。
    Shown(String, Option<Json>),
    /// 保存好的资料，附带撤销。
    Saved(String),
    /// 关联好的身份，附带撤销。
    Linked(Json),
    /// 作品，与 /api/works 列出的形式相同。
    Work(String),
    /// 核心命令自己的回复。
    Answer(Json),
    /// 核心命令的回复，附带刚登记的撤销。
    AnswerWithUndo(Json),
}
impl Reply {
    /// 名称投影更新后构造回复，显示本次写入的数据。
    fn render(self, c: &Connection) -> Result<Json> {
        match self {
            Reply::Existing(v) | Reply::Answer(v) => Ok(v),
            Reply::AnswerWithUndo(mut v) => {
                v.set("undo", last_action(c)?);
                Ok(v)
            }
            Reply::Shown(_, Some(v)) => http_read::display(c, v),
            Reply::Shown(id, None) => shown(c, &id),
            Reply::Saved(id) => Ok(Json::object([("undo", last_action(c)?), ("id", id.into())])),
            Reply::Linked(ids) => Ok(Json::object([("undo", last_action(c)?), ("ids", ids)])),
            Reply::Work(id) => {
                let Json::Array(rows) = http_read::works(c, "")? else { unreachable!() };
                rows.into_iter()
                    .find(|r| r.get("id").is_some_and(|v| v.text() == id))
                    .ok_or_else(|| Error::Invalid("作品不存在".into()))
            }
        }
    }
}
pub(crate) fn request(c: &mut Connection, body: &Json, path: &str, prepared: Option<&Prepared>) -> Result<Json> {
    initialized(c)?;
    // 每个路由只读下面列出的字段，范围照旧实现的处理函数（#9 已删除）；用不到的 NaN 和大整数无关紧要。
    let linked = body.get("tag").is_some_and(crate::app_service::truthy);
    let fields: &[&str] = match path {
        "/api/entities" if linked => &["tag", "typed"],
        "/api/entities" => &["name", "kind", "artist"],
        "/api/character/profile" => &["entity", "name", "aliases", "work", "tag"],
        "/api/local-links" => &["items"],
        "/api/works" => &["action", "id", "name", "parent", "tag"],
        "/api/character/mascot" => &["entity", "mascot"],
        "/api/kind" => &["entity", "kind", "artist"],
        "/api/oc/shared" => &[],
        _ => return invalid("接口不存在"),
    };
    let mut selected = picked(body, fields, &["id", "tag"]);
    if path == "/api/entities" && linked && !matches!(body.get("typed"), Some(Json::Scalar(Value::String(_)))) {
        selected.set("typed", "".into());
    }
    let v: Value = serde_json::from_str(&selected.dump()).map_err(|e| Error::Invalid(e.to_string()))?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(p) = prepared {
        if names_version(&tx)? != p.version {
            return invalid("名称来源已改变，请重试");
        }
    }
    crate::schema::begin_writing(&tx)?;
    let reply = match path {
        "/api/entities" if truthy(&v["tag"]) => linked_entity(&tx, &v, prepared)?,
        "/api/entities" => new_entity(&tx, &v)?,
        "/api/oc/shared" => shared_oc(&tx)?,
        "/api/character/profile" => profile(&tx, v, prepared)?,
        "/api/local-links" => local_links(&tx, &v, prepared)?,
        "/api/works" => edit_work(&tx, v)?,
        _ => set_field(&tx, v, path)?,
    };
    crate::app_commands::commit_writing_reply(tx, |c| reply.render(c))
}
/// `sql` 查到的第一个身份，带显示内容；可能没有。
fn existing(c: &Connection, sql: &str, args: &[&str]) -> Result<Option<Reply>> {
    match http_read::rows(c, sql, args)?.into_iter().next() {
        Some(row) => Ok(Some(Reply::Existing(http_read::display(c, row)?))),
        None => Ok(None),
    }
}
/// Danbooru tag 所属的身份；还没有时新建一个，带上其作品的 copyright，并把输入的名字作为别名。
fn linked_entity(c: &Connection, v: &Value, prepared: Option<&Prepared>) -> Result<Reply> {
    let t = tag(&v["tag"])?;
    let id = if let Some(owner) = crate::character_profile::tag_owner(c, &t, None)? {
        owner
    } else {
        let cp = copyright(c, &t, prepared)?;
        let typed = http_read::trim(v["typed"].as_str().unwrap_or("")).chars().take(120).collect::<String>();
        let alias = (!typed.is_empty() && http_read::normalize(&typed.replace(' ', "_")) != http_read::normalize(&t))
            .then_some(typed);
        let readable = crate::names_view::readable(&cp, false);
        made(&run(c, &json!({"op":"create_linked","tag":t,"alias":alias,"copyright":cp,"readable":readable}))?)
    };
    // 等 after_write 重建了显示投影之后才渲染。
    names(c, Some(&id))?;
    Ok(Reply::Shown(id, None))
}
/// 用户起的名字里下一个没被占用的«原创角色 N»。
fn next_oc_name(c: &Connection) -> Result<String> {
    let taken = http_read::strings(
        c,
        "SELECT n.value
         FROM term_names n
         JOIN term_tree r ON r.term_id = n.term_id
         WHERE n.source = 'user' AND n.value LIKE '原创角色 %'",
        &[],
    )?;
    Ok((1..=taken.len() + 1)
        .map(|n| format!("原创角色 {n}"))
        .find(|n| !taken.contains(n))
        .expect("app_profiles internal invariant"))
}
/// 有名字的身份：已叫这个名字的未关联身份（或以该 tag 或别名关联的身份），否则新建一个。
fn new_entity(c: &Connection, v: &Value) -> Result<Reply> {
    let (kind, artist) = oc(v)?;
    let name = if kind == "oc" && (v["name"].is_null() || v["name"] == "") {
        next_oc_name(c)?
    } else {
        let Some(name) = v["name"].as_str() else { return invalid("角色名称应为 1–120 个字符") };
        let name = http_read::trim(name);
        if name.is_empty() || name.chars().count() > 120 {
            return invalid("角色名称应为 1–120 个字符");
        }
        name.into()
    };
    let sql = format!(
        "SELECT e.id,entity_name(e.id) AS name,e.kind,e.artist
         FROM {ENTITIES} e
         WHERE e.parent_id IS NULL AND e.kind<>'work' AND e.merged_into IS NULL
           AND (e.id IN (
                  SELECT l.entity_id FROM {LINKS} l
                  WHERE l.core_system IN ('danbooru','wd_character') AND l.key=? COLLATE NOCASE)
                OR e.id IN (
                  SELECT t.key FROM term_names n JOIN terms t ON t.id = n.term_id
                  WHERE n.value = ? COLLATE NOCASE AND n.role = 'alias')
                OR (e.id IN (
                      SELECT t.key FROM term_names n JOIN terms t ON t.id = n.term_id
                      WHERE n.value = ? COLLATE NOCASE AND n.source = 'user')
                    AND NOT EXISTS(SELECT 1 FROM {LINKS} l WHERE l.entity_id=e.id)))
         ORDER BY e.id",
        ENTITIES = crate::character_entities::ENTITIES,
        LINKS = crate::character_links::LINKS
    );
    if let Some(found) = existing(c, &sql, &[&name, &name, &name])? {
        return Ok(found);
    }
    let lang = crate::character_labels::lang_of(&name);
    let id = made(&run(c, &json!({"op":"create_entity","name":name,"lang":lang,"kind":kind,"artist":artist}))?);
    let fields = Json::object([
        ("id", id.as_str().into()),
        ("name", name.into()),
        ("kind", kind.into()),
        ("artist", artist.into()),
    ]);
    Ok(Reply::Shown(id, Some(fields)))
}
/// 唯一一个共用的原创角色，第一次用到时新建。
fn shared_oc(c: &Connection) -> Result<Reply> {
    let sql = format!(
        "SELECT e.id,entity_name(e.id) AS name,e.kind,e.artist
         FROM {ENTITIES} e
         WHERE label_only=1 AND original=1 AND merged_into IS NULL
         ORDER BY seq
         LIMIT 1",
        ENTITIES = crate::character_entities::ENTITIES
    );
    if let Some(found) = existing(c, &sql, &[])? {
        return Ok(found);
    }
    let id = made(&run(c, &json!({"op":"shared_oc"}))?);
    let fields = Json::object([
        ("id", id.as_str().into()),
        ("name", crate::character_structure::SHARED_OC.into()),
        ("kind", "oc".into()),
        ("artist", "".into()),
    ]);
    Ok(Reply::Shown(id, Some(fields)))
}
fn profile(c: &Connection, mut v: Value, prepared: Option<&Prepared>) -> Result<Reply> {
    // 新建的作品（{"create": name}）或按 Danbooru tag 选的作品（{"tag": tag}）在同一个事务里建立：
    // 资料写入失败不会留下孤立的作品，保存一次就是一个请求。
    if v["work"].is_object() {
        let work = v["work"].take();
        let mut w = json!({"op":"edit_work","ident":null,"id":null});
        if let Some(name) = work.get("create").and_then(Value::as_str) {
            w["action"] = json!("create");
            w["name"] = json!(name);
            w["lang"] = json!(crate::character_labels::lang_of(name));
        } else if let Some(t) = work.get("tag") {
            let t = tag(t)?;
            w["action"] = json!("from_tag");
            w["lang"] = json!(crate::character_labels::lang_of(""));
            w["readable"] = json!(crate::names_view::readable(&t, false));
            w["tag"] = json!(t);
        } else {
            return invalid("作品应为已有作品、新建名称或 Danbooru tag");
        }
        v["work"] = run(c, &w)?["id"].clone();
    }
    v["op"] = json!("set_profile");
    if truthy(&v["tag"]) {
        let t = tag(&v["tag"])?;
        v["copyright"] = json!(if v["work"].is_null() { copyright(c, &t, prepared)? } else { String::new() });
        v["tag"] = json!(t);
    }
    let id = made(&run(c, &v)?);
    c.execute("INSERT INTO backend_effects(kind) VALUES ('verdict')", [])?;
    names(c, Some(&id))?;
    Ok(Reply::Saved(id))
}
fn local_links(c: &Connection, v: &Value, prepared: Option<&Prepared>) -> Result<Reply> {
    let mut plan = Vec::new();
    // 在外部查询之前校验整个计划，照旧实现的 `_link_args`（#9 已删除）。
    for item in link_items(v)? {
        plan.push((item["id"].clone(), tag(&item["tag"])?));
    }
    let mut works = std::collections::HashMap::new();
    for (_, t) in &plan {
        works.insert(t.clone(), copyright(c, t, prepared)?);
    }
    let items: Vec<_> = plan.into_iter().map(|(id, t)| json!({"id":id,"copyright":works[&t],"tag":t})).collect();
    let out = run(c, &json!({"op":"link_local","items":items}))?;
    c.execute("INSERT INTO backend_effects(kind) VALUES ('verdict')", [])?;
    names(c, None)?;
    Ok(Reply::Linked(out["ids"].clone().into()))
}
fn edit_work(c: &Connection, mut v: Value) -> Result<Reply> {
    v["op"] = json!("edit_work");
    v["ident"] = v["id"].clone();
    v["id"] = Value::Null;
    v["lang"] = json!(crate::character_labels::lang_of(v["name"].as_str().unwrap_or("")));
    if v["action"] == "from_tag" {
        let t = tag(&v["tag"])?;
        v["readable"] = json!(crate::names_view::readable(&t, false));
        v["tag"] = json!(t);
    }
    Ok(Reply::Work(made(&run(c, &v)?)))
}
/// /api/kind 和 /api/character/mascot。
fn set_field(c: &Connection, mut v: Value, path: &str) -> Result<Reply> {
    v["op"] = json!(match path {
        "/api/kind" => "edit_kind",
        "/api/character/mascot" => "edit_mascot",
        _ => return invalid("接口不存在"),
    });
    if path == "/api/kind" {
        let (kind, artist) = oc(&v)?;
        v["kind"] = json!(kind);
        v["artist"] = json!(artist);
    }
    let mut out = run(c, &v)?;
    let action = out.as_object_mut().expect("角色标记命令返回对象").remove("action");
    let mut response: Json = out.into();
    if action.is_some() {
        Ok(Reply::AnswerWithUndo(response))
    } else {
        response.set("undo", Value::Null.into());
        Ok(Reply::Answer(response))
    }
}
