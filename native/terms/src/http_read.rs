//! 确定性的 HTTP 读取，共用一个读事务和既有的名字规则。
use crate::character_entities::ENTITIES;
use crate::{
    http::LibraryContext,
    http_json::Json,
    names_view::{Choice, View},
    sql_fragments::{hybrid, visible, SWITCHED},
    Error, Result,
};
use rusqlite::{params, params_from_iter, types::ValueRef, Connection, OpenFlags, OptionalExtension};
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, OnceLock},
};
use unicode_normalization::UnicodeNormalization;

const ARTIST_DIRECTORY: &str = "/api/artist-directory";

#[cfg(test)]
pub const CORE_ROUTES: &[&str] = &[
    "/api/assets",
    "/api/assets/count",
    "/api/ids",
    "/api/detail",
    "/api/home",
    "/api/calendar",
    "/api/galleries",
    "/api/galleries/covers",
    "/api/fold",
    "/api/review-pools",
    "/api/ambiguous",
    "/api/characters",
    "/api/tagged",
    "/api/character/profile",
    "/api/character/activity",
    "/api/character/stats",
    crate::http_characters::TOGETHER,
    "/api/identity/count",
    "/api/identity/queue",
    "/api/bootstrap",
    "/api/user-tags",
    "/api/general-tags",
    "/api/artists",
    ARTIST_DIRECTORY,
    "/api/artist",
    "/api/artist-audit",
    "/api/entities",
    "/api/works",
    "/api/ask",
    "/api/similar",
    "/api/clusters",
];
/// 作品和它的全部子作品（`UNION` 防旧数据成环），参数是作品 id。
pub const TREE: &str = "WITH RECURSIVE tree(id) AS (
  SELECT t.key
  FROM terms t
  JOIN term_tree r ON r.term_id=t.id
  WHERE t.key=? AND t.kind='work'
  UNION
  SELECT t.key
  FROM term_tree r
  JOIN tree p ON r.parent_id=p.id
  JOIN terms t ON t.id=r.term_id
  WHERE t.kind='work')";

/// 一个作品及其全部子作品；不存在（或已被合并）的作品会被拒绝。
pub(crate) fn work_tree(c: &Connection, work: &str) -> Result<Vec<String>> {
    let found = format!("SELECT EXISTS(SELECT 1 FROM {ENTITIES} WHERE id=? AND kind='work' AND merged_into IS NULL)");
    if !c.query_row(&found, [work], |r| r.get::<_, bool>(0))? {
        return Err(Error::Invalid("作品不存在".into()));
    }
    strings(c, &format!("{TREE} SELECT id FROM tree"), &[work])
}

pub(crate) use crate::compat_text::{python_repr, trim, whitespace};
fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
pub(crate) fn like(s: &str) -> String {
    format!("%{}%", s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
}
pub(crate) fn normalize(s: &str) -> String {
    crate::casefold::casefold(
        &s.nfkc().filter(|c| !whitespace(*c) && !['・', '·', '･'].contains(c)).collect::<String>(),
    )
}
fn decode(s: &str) -> String {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = |x: u8| (x as char).to_digit(16).map(|v| v as u8);
                if let (Some(a), Some(z)) = (hex(b[i + 1]), hex(b[i + 2])) {
                    out.push(a * 16 + z);
                    i += 2;
                } else {
                    out.push(b[i]);
                }
            }
            v => out.push(v),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
pub fn query(url: &str) -> HashMap<String, String> {
    let mut result = HashMap::new();
    let url = url.split('#').next().unwrap_or(url);
    if let Some((_, q)) = url.split_once('?') {
        for part in q.split('&') {
            let (k, v) = part.split_once('=').unwrap_or((part, ""));
            if !v.is_empty() {
                result.insert(decode(k), decode(v));
            }
        }
    }
    result
}
pub fn remote(url: &str) -> bool {
    let path = url.split('?').next().unwrap_or(url);
    ["/api/entities", "/api/works"].contains(&path) && query(url).get("remote").is_some_and(|v| !v.is_empty())
}
pub fn flag(c: &Connection, key: &str) -> Result<bool> {
    Ok(c.query_row("SELECT value FROM meta WHERE key=?", [key], |r| r.get::<_, String>(0)).optional()?.as_deref()
        == Some("core"))
}
pub fn rows(c: &Connection, sql: &str, args: &[&str]) -> Result<Vec<Json>> {
    query_rows(c, sql, params_from_iter(args.iter()))
}
pub(crate) fn typed_rows(c: &Connection, sql: &str, args: &[rusqlite::types::Value]) -> Result<Vec<Json>> {
    query_rows(c, sql, params_from_iter(args.iter()))
}
fn query_rows(c: &Connection, sql: &str, args: impl rusqlite::Params) -> Result<Vec<Json>> {
    let mut s = c.prepare(sql)?;
    let columns: Vec<String> = s.column_names().map(str::to_owned).collect();
    let result = s
        .query_map(args, |r| {
            let mut fields = Vec::new();
            for (i, k) in columns.iter().enumerate() {
                let v = match r.get_ref(i)? {
                    ValueRef::Null => Json::from(None::<String>),
                    ValueRef::Integer(v) => v.into(),
                    ValueRef::Real(v) => Json::Float(v),
                    ValueRef::Text(v) => String::from_utf8_lossy(v).into_owned().into(),
                    ValueRef::Blob(_) => {
                        return Err(rusqlite::Error::InvalidColumnType(i, k.clone(), rusqlite::types::Type::Blob))
                    }
                };
                fields.push((k.clone(), v));
            }
            Ok(Json::Object(fields))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(result)
}
pub fn strings(c: &Connection, sql: &str, args: &[&str]) -> Result<Vec<String>> {
    Ok(c.prepare(sql)?.query_map(params_from_iter(args.iter()), |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}
fn choice(v: Choice) -> Json {
    Json::object([
        ("name", v.name.into()),
        ("name_source", v.source.into()),
        (
            "names",
            Json::object([
                ("zh", v.names[0].clone().into()),
                ("ja", v.names[1].clone().into()),
                ("en", v.names[2].clone().into()),
            ]),
        ),
    ])
}
// 给非中文页面显示的名字（#173 第 3 阶段）。页面在请求头里说明自己的语言（app_http.rs）；不带这个请求头的请求、
// 所有后台任务和所有写入看到的名字都与以前完全相同。
thread_local! {
    static ROMAN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
/// 存活期间，本线程的读取按 `language`（`X-Illustash-Language`）页面的语言显示名字。
pub(crate) struct PageLanguage(bool);
impl PageLanguage {
    pub(crate) fn set(language: Option<&str>) -> Self {
        let roman = language.is_some_and(|l| {
            let primary = l.split('-').next().unwrap_or("").to_ascii_lowercase();
            !primary.is_empty() && primary != "zh"
        });
        PageLanguage(ROMAN.replace(roman))
    }
    pub(crate) fn with<T>(self, read: impl FnOnce() -> T) -> T {
        read()
    }
}
impl Drop for PageLanguage {
    fn drop(&mut self) {
        ROMAN.set(self.0);
    }
}
fn roman() -> bool {
    ROMAN.get()
}
/// 在其他语言页面上显示的名字：用户自己起的名字保留；否则用英文名，其次是整理成可读形式的 Danbooru 标签（如
/// Hatsune Miku，日文角色的标签就是罗马字）；最后才是选定的名字。
pub(crate) fn shown_name(name: String, source: &str, en: Option<String>, tag: &str, character: bool) -> String {
    if !roman() || source == "user" {
        return name;
    }
    match en.filter(|en| !en.is_empty()) {
        Some(en) => en,
        None if !tag.is_empty() => crate::names_view::readable(tag, character),
        None => name,
    }
}
pub(crate) fn shown_info(mut info: Json) -> Json {
    if roman() {
        let name = info.get("name").map(|v| v.text().to_owned()).unwrap_or_default();
        let source = info.get("name_source").map(|v| v.text().to_owned()).unwrap_or_default();
        let en = info.get("names").and_then(|n| n.get("en")).map(|v| v.text().to_owned()).filter(|en| !en.is_empty());
        let tag = info.get("tag").map(|v| v.text().to_owned()).unwrap_or_default();
        info.set("name", shown_name(name, &source, en, &tag, true).into());
    }
    info
}
pub(crate) fn register_names(c: &Connection) -> Result<()> {
    if flag(c, "terms_names")? {
        register_entity_name(c)?;
        register_entity_info(c)?;
        register_work_name(c)
    } else {
        register_snapshot_names(c)
    }
}
const NAME_FUNCTION: rusqlite::functions::FunctionFlags = rusqlite::functions::FunctionFlags::SQLITE_UTF8;
/// 名字不在核心里的图库的名字：所有身份的一份快照。
fn register_snapshot_names(c: &Connection) -> Result<()> {
    let fallback = Arc::new(View::load(c)?);
    let mut shown = HashMap::new();
    // 快照已经包含所有身份；不再读第二遍 entities（#102）。
    for id in fallback.ids().cloned().collect::<Vec<_>>() {
        for skin in [0, 1] {
            if let Some((v, tag)) = fallback.entity(&id, skin != 0)? {
                let mut info = choice(v);
                info.set("tag", tag.into());
                shown.insert((id.clone(), skin), info);
            }
        }
    }
    let shown = Arc::new(shown);
    let names = shown.clone();
    c.create_scalar_function("entity_name", 1, NAME_FUNCTION, move |ctx| {
        let id = ctx.get::<Option<String>>(0)?.unwrap_or_default();
        Ok(names
            .get(&(id, 0))
            .map(|v| shown_info(v.clone()))
            .and_then(|v| v.get("name").map(|v| v.text().to_owned()))
            .unwrap_or_default())
    })?;
    c.create_scalar_function("entity_info", 2, NAME_FUNCTION, move |ctx| {
        let id = ctx.get::<Option<String>>(0)?.unwrap_or_default();
        let skin = ctx.get::<i64>(1)?;
        Ok(shown.get(&(id, i64::from(skin != 0))).cloned().map(shown_info).unwrap_or(Json::Object(Vec::new())).dump())
    })?;
    c.create_scalar_function("work_name", 1, NAME_FUNCTION, move |ctx| {
        let key = ctx.get::<String>(0)?;
        fallback
            .choose("work", &key)
            .map(|v| shown_name(v.name, &v.source, v.names[2].clone(), &key, false))
            .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))
    })?;
    Ok(())
}
// 下面的投影名字只渲染实际请求到的名字，按本连接和读事务记忆结果，并且按页面语言分开（一个连接可能比一个请求
// 活得久）。不做全图库快照，也不做跨请求缓存：写入和切换图库不会复用过期的名字，空页面也不会加载所有身份的
// 旧名字。
fn register_entity_name(c: &Connection) -> Result<()> {
    let names = RefCell::new(HashMap::<(bool, String), String>::new());
    c.create_scalar_function("entity_name", 1, NAME_FUNCTION, move |ctx| {
        let id = ctx.get::<Option<String>>(0)?.unwrap_or_default();
        let key = (roman(), id);
        if let Some(name) = names.borrow().get(&key) {
            return Ok(name.clone());
        }
        // SAFETY: HTTP 在一个线程上持有这个连接。不拥有连接的句柄只在这个回调内使用，读的是同一个请求事务。
        let c = unsafe { ctx.get_connection()? };
        let name: String = if key.0 {
            c.query_row(
                "SELECT name,name_source,en,tag FROM term_name_display WHERE entity=? AND skin_only=0",
                [&key.1],
                |r| Ok(shown_name(r.get(0)?, &r.get::<_, String>(1)?, r.get(2)?, &r.get::<_, String>(3)?, true)),
            )
        } else {
            c.query_row("SELECT name FROM term_name_display WHERE entity=? AND skin_only=0", [&key.1], |r| r.get(0))
        }
        .optional()?
        .unwrap_or_default();
        names.borrow_mut().insert(key, name.clone());
        Ok(name)
    })?;
    Ok(())
}
fn register_entity_info(c: &Connection) -> Result<()> {
    let shown = RefCell::new(HashMap::<(bool, String, i64), String>::new());
    c.create_scalar_function("entity_info", 2, NAME_FUNCTION, move |ctx| {
        let id = ctx.get::<Option<String>>(0)?.unwrap_or_default();
        let skin = i64::from(ctx.get::<i64>(1)? != 0);
        let key = (roman(), id, skin);
        if let Some(info) = shown.borrow().get(&key) {
            return Ok(info.clone());
        }
        // SAFETY: 同 register_entity_name；连接引用不会逃出回调。
        let c = unsafe { ctx.get_connection()? };
        let info = c
            .query_row(
                "SELECT name,name_source,zh,ja,en,tag FROM term_name_display WHERE entity=? AND skin_only=?",
                params![&key.1, skin],
                |r| {
                    Ok(shown_info(Json::object([
                        ("name", r.get::<_, String>(0)?.into()),
                        ("name_source", r.get::<_, String>(1)?.into()),
                        (
                            "names",
                            Json::object([
                                ("zh", r.get::<_, Option<String>>(2)?.into()),
                                ("ja", r.get::<_, Option<String>>(3)?.into()),
                                ("en", r.get::<_, Option<String>>(4)?.into()),
                            ]),
                        ),
                        ("tag", r.get::<_, String>(5)?.into()),
                    ]))
                    .dump())
                },
            )
            .optional()?
            .unwrap_or_else(|| "{}".into());
        shown.borrow_mut().insert(key, info.clone());
        Ok(info)
    })?;
    Ok(())
}
fn register_work_name(c: &Connection) -> Result<()> {
    let chosen = RefCell::new(HashMap::<(bool, String), String>::new());
    c.create_scalar_function("work_name", 1, NAME_FUNCTION, move |ctx| {
        let key = (roman(), ctx.get::<String>(0)?);
        if let Some(name) = chosen.borrow().get(&key) {
            return Ok(name.clone());
        }
        // SAFETY: 同 register_entity_name；回退路径也读这个事务，从不打开新连接。
        let c = unsafe { ctx.get_connection()? };
        let name: Option<String> = if key.0 {
            c.query_row("SELECT name,name_source,en FROM term_name_choice WHERE kind='work' AND key=?", [&key.1], |r| {
                Ok(shown_name(r.get(0)?, &r.get::<_, String>(1)?, r.get(2)?, &key.1, false))
            })
        } else {
            c.query_row("SELECT name FROM term_name_choice WHERE kind='work' AND key=?", [&key.1], |r| r.get(0))
        }
        .optional()?;
        let name = match name {
            Some(name) => name,
            // 投影里没有的 key：只从这个 key 自己的行里选（#105），不加载整个图库。
            None => crate::names_view::choose_key(&c, "work", &key.1)
                .map(|v| shown_name(v.name, &v.source, v.names[2].clone(), &key.1, false))
                .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?,
        };
        chosen.borrow_mut().insert(key, name.clone());
        Ok(name)
    })?;
    Ok(())
}
pub(crate) fn display(c: &Connection, v: Json) -> Result<Json> {
    display_skin(c, v, false)
}
pub(crate) fn display_skin(c: &Connection, mut v: Json, skin: bool) -> Result<Json> {
    let id = v.field("id").text().to_owned();
    let info: String = c.query_row("SELECT entity_info(?,?)", params![&id, skin as i64], |r| r.get(0))?;
    v.extend(serde_json::from_str(&info).map_err(|e| Error::Invalid(e.to_string()))?);
    let sql = format!(
        "SELECT e.kind,e.original,coalesce(p.work_id,e.work_id)
         FROM {ENTITIES} e
         LEFT JOIN terms pt ON pt.key=e.parent_id AND e.kind='skin'
         LEFT JOIN term_tree p ON p.term_id=pt.id
         WHERE e.id=?"
    );
    let entity: Option<(String, bool, Option<String>)> =
        c.query_row(&sql, [&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
    let mut work = None;
    if let Some((kind, original, w)) = entity {
        if v.get("kind").is_some() && kind != "work" {
            v.set("kind", if original { "oc" } else { "" }.into());
        }
        v.set("work_id", w.clone().into());
        work = w;
    }
    if v.get("work").is_some() {
        let name = if let Some(w) = work {
            c.query_row("SELECT entity_name(?)", [w], |r| r.get::<_, String>(0))?
        } else {
            String::new()
        };
        v.set("work_name", name.into());
    }
    if hybrid(c)? {
        let node = "SELECT node_type,character_id,variant_id,canonical_id FROM identity_nodes WHERE id=?";
        if let Some(node) = rows(c, node, &[&id])?.into_iter().next() {
            v.extend(node);
        }
    }
    Ok(v)
}
/// 把名字匹配 `q` 的身份填进临时表 `name_matches`（`entity`、`rank`），每个身份换成它被合并进的身份，
/// 取最好的排名；`q` 为空时表保持为空。
pub(crate) fn match_names(c: &Connection, q: &str) -> Result<()> {
    let table = "name_matches(entity TEXT PRIMARY KEY,rank INTEGER NOT NULL)";
    c.execute_batch(&format!("CREATE TEMP TABLE IF NOT EXISTS {table}; DELETE FROM name_matches;"))?;
    if q.is_empty() {
        return Ok(());
    }
    let mut hits: HashMap<String, i64> = HashMap::new();
    for (id, rank) in crate::names_search::hits(c, q)? {
        let id = crate::character_commands::resolve(c, &id)?;
        let v = hits.entry(id).or_insert(-1);
        *v = (*v).max(rank);
    }
    for (id, rank) in hits {
        c.execute("INSERT INTO name_matches VALUES (?,?)", params![id, rank])?;
    }
    Ok(())
}
fn entities(c: &Connection, q: &str) -> Result<Json> {
    let q = cut(trim(q), 160);
    match_names(c, &q)?;
    let matched = if q.is_empty() { "" } else { "JOIN name_matches sm ON sm.entity=e.id" };
    let ranking = if q.is_empty() { "" } else { "sm.rank DESC," };
    // 只在扩展仍提供候选，或词条已有归属或人工资料时返回扩展新建的角色。
    let sql = format!(
        "SELECT e.id,entity_name(e.id) AS name,e.work,e.kind,e.artist,e.label_only,e.mascot,e.parent_id,
           (SELECT count(*)
            FROM (SELECT uuid FROM memberships WHERE entity=e.id
                  UNION ALL
                  SELECT uuid FROM tag_memberships WHERE entity=e.id) m
            WHERE {visible}) AS count,
           (SELECT group_concat(value,' · ') FROM (
              SELECT DISTINCT n.value
              FROM term_names n
              WHERE n.term_id = (SELECT id FROM terms WHERE key = e.id)
                AND n.value LIKE ? ESCAPE '\\' AND n.value <> entity_name(e.id)
              LIMIT 2)) AS matched
         FROM {ENTITIES} e
         {matched}
         WHERE e.parent_id IS NULL AND e.kind<>'work' AND e.merged_into IS NULL
           AND NOT EXISTS (
               SELECT 1 FROM terms t WHERE t.id=e.term_id AND t.origin=?2
               AND NOT EXISTS (SELECT 1 FROM candidates v WHERE v.entity=e.id)
               AND NOT EXISTS (SELECT 1 FROM term_projection p WHERE p.term_id=t.id)
               AND NOT EXISTS (SELECT 1 FROM term_names n WHERE n.term_id=t.id AND n.source='user')
               AND NOT EXISTS (SELECT 1 FROM term_links l WHERE l.term_id=t.id AND l.source='user'))
         ORDER BY {ranking}count DESC,entity_name(e.id),e.id
         LIMIT 30",
        visible = visible("m")
    );
    let origin = crate::character_import::EXTENSION_CHARACTER_ORIGIN;
    let result = rows(c, &sql, &[&like(&q), origin])?.into_iter().map(|v| display(c, v)).collect::<Result<Vec<_>>>()?;
    c.execute("DELETE FROM name_matches", [])?;
    Ok(Json::Array(result))
}
pub(crate) fn works(c: &Connection, q: &str) -> Result<Json> {
    let needle = normalize(trim(q));
    let mut result = Vec::new();
    let sql = format!(
        "SELECT e.id, e.parent_id, entity_name(e.id) AS name,
           (SELECT l.key FROM {LINKS} l
            WHERE l.entity_id = e.id AND l.core_system = 'danbooru' AND l.current = 1 LIMIT 1) AS tag,
           (SELECT l.key FROM {LINKS} l WHERE l.entity_id = e.id AND l.core_system = 'qualifier' LIMIT 1) AS qualifier,
           coalesce(w.status, 'pending') AS status, w.note
         FROM {ENTITIES} e
         LEFT JOIN {REVIEWS} w ON w.entity_id = e.id
         WHERE e.kind = 'work' AND e.merged_into IS NULL",
        LINKS = crate::character_links::LINKS,
        REVIEWS = crate::character_review::REVIEWS
    );
    let counted = format!(
        "{TREE}
         SELECT count(DISTINCT m.uuid)
         FROM memberships m
         JOIN {ENTITIES} e ON e.id=m.entity
         WHERE e.work_id IN (SELECT id FROM tree) AND {visible}",
        visible = visible("m")
    );
    for mut v in rows(c, &sql, &[])? {
        let id = v.field("id").text();
        let count: i64 = c.query_row(&counted, [id], |r| r.get(0))?;
        let mut values: Vec<String> =
            ["name", "tag", "qualifier"].iter().map(|k| v.field(k).text().to_owned()).collect();
        values.extend(strings(
            c,
            "SELECT value FROM term_names WHERE term_id = (SELECT id FROM terms WHERE key = ?)",
            &[id],
        )?);
        let watchable = !v.field("tag").text().is_empty();
        v.set("count", count.into());
        v.set("watchable", watchable.into());
        if needle.is_empty()
            || values
                .iter()
                .any(|s| normalize(s).contains(&needle) || normalize(&s.replace('_', " ")).contains(&needle))
        {
            result.push(v);
        }
    }
    result.sort_by(|a, b| {
        b.field("count")
            .integer()
            .cmp(&a.field("count").integer())
            .then_with(|| a.field("name").text().cmp(b.field("name").text()))
            .then_with(|| a.field("id").text().cmp(b.field("id").text()))
    });
    Ok(Json::Array(result))
}
/// 作品配色与切片共用同一组可见图片。一个 UUID 即使匹配多个角色也只出现一次。
fn work_covers(c: &Connection, works: &mut Json, nsfw: &str) -> Result<()> {
    let Json::Array(works) = works else { return Ok(()) };
    let rating = crate::sql_fragments::rating("a", nsfw)?;
    let sql = format!(
        "{TREE} SELECT DISTINCT a.uuid, a.added_at
         FROM memberships m JOIN {ENTITIES} e ON e.id=m.entity
         JOIN assets a ON a.uuid=m.uuid
         WHERE e.work_id IN (SELECT id FROM tree) AND {visible}{rating}
         ORDER BY a.added_at DESC,a.uuid LIMIT 4",
        visible = visible("a"),
    );
    let mut statement = c.prepare(&sql)?;
    for work in works {
        let covers = statement
            .query_map([work.field("id").text()], |row| Ok(Json::object([("uuid", row.get::<_, String>(0)?.into())])))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        work.set("covers", Json::Array(covers));
    }
    Ok(())
}
pub(crate) fn sqlite_uri(path: &std::path::Path) -> String {
    let path = crate::sqlite::path(path);
    let path = path.to_string_lossy();
    let encoded: String = path
        .as_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"/-._~:".contains(b) {
                (*b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("file:{encoded}?mode=ro")
}

// 这个 CSV 不会变（旧实现每次启动只加载一次）。只缓存这份词表，以私有的启动 generation 和 CSV 路径为键；
// 从不保留图库连接。
type TagVocabulary = (String, std::path::PathBuf, Arc<Vec<String>>);
pub(crate) fn tag_vocabulary(context: &LibraryContext) -> Result<Arc<Vec<String>>> {
    static TAGS: std::sync::Mutex<Option<TagVocabulary>> = std::sync::Mutex::new(None);
    let mut cached = TAGS.lock().map_err(|_| Error::Invalid("CSV 缓存锁损坏".into()))?;
    if let Some((generation, path, tags)) = &*cached {
        if generation == &context.generation && path == &context.tags {
            return Ok(tags.clone());
        }
    }
    let mut csv = csv::Reader::from_path(&context.tags).map_err(|e| Error::Invalid(e.to_string()))?;
    let headers = csv.headers().map_err(|e| Error::Invalid(e.to_string()))?.clone();
    let name = headers.iter().position(|v| v == "name").ok_or_else(|| Error::Invalid("CSV 缺少 name".into()))?;
    let category =
        headers.iter().position(|v| v == "category").ok_or_else(|| Error::Invalid("CSV 缺少 category".into()))?;
    let mut tags = Vec::new();
    for r in csv.records() {
        let r = r.map_err(|e| Error::Invalid(e.to_string()))?;
        if r.get(category) == Some("0") {
            tags.push(r.get(name).unwrap_or("").to_owned());
        }
    }
    let tags = Arc::new(tags);
    *cached = Some((context.generation.clone(), context.tags.clone(), tags.clone()));
    Ok(tags)
}

const BROWSE_ROUTES: &[&str] = &[
    "/api/assets",
    "/api/assets/count",
    "/api/ids",
    "/api/detail",
    "/api/home",
    "/api/calendar",
    "/api/fold",
    "/api/review-pools",
    "/api/ambiguous",
];
/// 会渲染名字的模块读取。计数、ID 以及原始的身份和偏好读取不渲染；它们的查询路径保持在本地。
const NAMED_ROUTES: &[&str] = &[
    "/api/ambiguous",
    "/api/assets",
    "/api/detail",
    "/api/home",
    "/api/calendar",
    "/api/galleries/covers",
    "/api/characters",
    "/api/tagged",
    "/api/character/profile",
    "/api/identity/queue",
    "/api/bootstrap",
];
fn refused(message: String) -> Option<(Json, u16)> {
    Some((Json::object([("error", message.into())]), 400))
}
pub fn read(context: &LibraryContext, url: &str) -> Result<Option<(Json, u16)>> {
    let q = query(url);
    let path = url.split('?').next().unwrap_or(url);
    if path == "/api/general-tags" {
        let needle = q.get("q").map(String::as_str).unwrap_or("");
        let tags = if q.get("format").is_some_and(|format| format == "menu") {
            general_tag_menu(context, needle)?
        } else {
            general_tags(context, needle)?
        };
        return Ok(Some((tags, 200)));
    }
    let audit_n = match audit_size(path, &q) {
        Ok(n) => n,
        Err(message) => return Ok(refused(message)),
    };
    let local = crate::http_local::ROUTES.contains(&path);
    let browse = BROWSE_ROUTES.contains(&path) || matches!(path, "/api/galleries" | "/api/galleries/covers");
    let characters = crate::http_characters::ROUTES.contains(&path);
    let validation = if browse {
        crate::http_browse::validate(path, &q)
    } else if characters {
        crate::http_characters::validate(path, &q)
    } else {
        Ok(())
    };
    match validation {
        Ok(()) => {}
        Err(Error::Invalid(m)) => return Ok(refused(m)),
        Err(e) => return Err(e),
    }
    if path == "/api/detail" && context.source.is_none() {
        return Ok(None);
    }
    if path == "/api/bootstrap" && context.token.is_none() {
        return Ok(None);
    }
    let mut c =
        crate::sqlite::open_with_flags(&context.db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI)?;
    c.busy_timeout(std::time::Duration::from_secs(10))?;
    let tx = c.transaction()?;
    if path == "/api/bootstrap" && context.read_only.is_none() && !switched(&tx)? {
        return Ok(None);
    }
    attach_provenance(&tx, context, path)?;
    if !(characters || browse || local || path == "/api/bootstrap") {
        // 新读接口可拒绝参数。库中其他接口保留原来的错误处理。
        let result = read_library(&tx, context, path, &q, audit_n);
        return match result {
            Ok(mut payload) => {
                crate::app_colors::decorate_response(&tx, context, path, &q, &mut payload);
                tx.commit()?;
                Ok(Some((payload, 200)))
            }
            Err(Error::Invalid(m)) if path == ARTIST_DIRECTORY || crate::app_colors::wants_work_covers(path, &q) => {
                Ok(refused(m))
            }
            Err(e) => Err(e),
        };
    }
    // Store.connect 在交出连接之前会探查名字开关，即使没有名字渲染器也一样。
    flag(&tx, "terms_names")?;
    if local || NAMED_ROUTES.contains(&path) {
        register_names(&tx)?;
    }
    let result = if local {
        crate::http_local::read(&tx, path, &q)
    } else if characters {
        crate::http_characters::read(&tx, context, path, &q)
    } else if browse {
        crate::http_browse::read(&tx, context, path, &q)
    } else {
        crate::http_app::read(&tx, context, &q)
    };
    match result {
        Ok((mut body, status)) => {
            crate::app_colors::decorate_response(&tx, context, path, &q, &mut body);
            tx.commit()?;
            Ok(Some((body, status)))
        }
        Err(Error::Invalid(m)) => Ok(refused(m)),
        Err(e) => Err(e),
    }
}
fn general_tags(context: &LibraryContext, q: &str) -> Result<Json> {
    let q = cut(&crate::casefold::casefold(trim(q)), 80);
    let vocabulary = tag_vocabulary(context)?;
    let tags = vocabulary.iter().filter(|n| crate::casefold::casefold(n).contains(&q)).take(30);
    Ok(Json::Array(tags.map(|n| n.as_str().into()).collect()))
}

#[derive(serde::Deserialize)]
struct GeneralTagNames {
    zh: Option<String>,
    ja: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
}

fn tag_search_key(text: &str) -> String {
    let normalized: String = text.nfkc().map(|c| if whitespace(c) { '_' } else { c }).collect();
    crate::casefold::casefold(&normalized)
}

fn general_tag_menu(context: &LibraryContext, q: &str) -> Result<Json> {
    static NAMES: OnceLock<HashMap<String, GeneralTagNames>> = OnceLock::new();
    let names = NAMES.get_or_init(|| {
        serde_json::from_str(include_str!("../../../tags/general_names.json")).expect("内置通用标签对照表必须有效")
    });
    let q = cut(&tag_search_key(trim(q)), 80);
    let vocabulary = tag_vocabulary(context)?;
    let mut matches = Vec::new();
    for tag in vocabulary.iter() {
        let entry = names.get(tag);
        let localized_match = || {
            entry.is_some_and(|entry| {
                entry.zh.iter().chain(&entry.ja).chain(&entry.aliases).any(|name| tag_search_key(name).contains(&q))
            })
        };
        if !q.is_empty() && !tag_search_key(tag).contains(&q) && !localized_match() {
            continue;
        }
        let zh = entry.and_then(|entry| entry.zh.as_deref());
        let ja = entry.and_then(|entry| entry.ja.as_deref());
        matches.push(Json::object([("tag", tag.as_str().into()), ("zh", zh.into()), ("ja", ja.into())]));
        if matches.len() == 30 {
            break;
        }
    }
    Ok(Json::Array(matches))
}
/// /api/artist-audit 抽查多少张图（其他读取为 50），或者请求被拒绝的原因。
fn audit_size(path: &str, q: &HashMap<String, String>) -> std::result::Result<usize, String> {
    if path != "/api/artist-audit" {
        return Ok(50);
    }
    let n = python_int(q.get("n").map(String::as_str).unwrap_or("50"))?;
    if !(1..=500).contains(&n) {
        return Err("抽查张数要在 1–500 之间".into());
    }
    Ok(n as usize)
}
/// 四个模块是否都已在核心里。启动时在既有的后台切换完成之前就发布了就绪状态，切换结果在发布之后
/// 还可能变化。四个模块全部就绪之前，bootstrap 必须用实时的拒绝结果，而不是过期的启动值。
fn switched(c: &Connection) -> Result<bool> {
    Ok(c.query_row(SWITCHED, [], |r| r.get::<_, bool>(0))?)
}
/// 仍需要 provenance.sqlite 的画师读取以只读方式把它挂载为 `prov`。
fn attach_provenance(c: &Connection, context: &LibraryContext, path: &str) -> Result<()> {
    let auxiliary_needed = path == "/api/artist-audit" || (path == "/api/artist" && !flag(c, "terms_artists")?);
    if let Some(library) = context.library.as_ref().filter(|_| auxiliary_needed) {
        let prov = library.join("provenance.sqlite");
        if prov.exists() {
            c.execute("ATTACH DATABASE ? AS prov", [sqlite_uri(&prov)])?;
        }
    }
    Ok(())
}
/// browse、characters、local 和 app 模块之外的读取。
fn read_library(
    c: &Connection,
    context: &LibraryContext,
    path: &str,
    q: &HashMap<String, String>,
    audit_n: usize,
) -> Result<Json> {
    let value = |key: &str| q.get(key).map(String::as_str).unwrap_or("");
    let library = context.library.is_some();
    Ok(match path {
        "/api/user-tags" => {
            let limit =
                q.get("limit").map(|value| python_int(value).map_err(Error::Invalid)).transpose()?.unwrap_or(60);
            if !(1..=1000).contains(&limit) {
                return Err(Error::invalid("分页参数不正确"));
            }
            let sql = format!(
                "SELECT tag,count(*) AS count
                 FROM asset_tags m
                 JOIN assets USING(uuid)
                 WHERE {visible} AND tag LIKE ? ESCAPE '\\'
                 GROUP BY tag
                 ORDER BY count DESC,tag
                 LIMIT {limit}",
                visible = visible("m")
            );
            Json::Array(rows(c, &sql, &[&like(&cut(trim(value("q")), 60))])?)
        }
        ARTIST_DIRECTORY => crate::http_artists::directory(c, library, q)?,
        "/api/artists" => {
            let sql = format!(
                "SELECT artist,count(*) AS characters
                 FROM {ENTITIES}
                 WHERE original=1 AND artist<>'' AND merged_into IS NULL
                 GROUP BY artist
                 ORDER BY characters DESC,artist"
            );
            Json::Array(rows(c, &sql, &[])?)
        }
        "/api/entities" => {
            register_names(c)?;
            entities(c, value("q"))?
        }
        "/api/works" => {
            register_names(c)?;
            let mut result = works(c, value("q"))?;
            if crate::app_colors::wants_work_covers(path, q) {
                work_covers(c, &mut result, value("nsfw"))?;
            }
            result
        }
        "/api/artist" => {
            Json::object([("id", value("id").into()), ("name", crate::http_artists::name_of(c, library, value("id"))?)])
        }
        "/api/artist-audit" => crate::http_artists::audit(c, library, audit_n)?,
        _ => return Err(Error::Invalid("未迁移接口".into())),
    })
}
pub(crate) fn python_int(s: &str) -> std::result::Result<i64, String> {
    let mut converted = String::new();
    for c in s.trim_matches(|c| whitespace(c) && !('\u{1c}'..='\u{1f}').contains(&c)).chars() {
        converted.push(match crate::compat_unicode::decimal(c) {
            Some(n) => (b'0' + n) as char,
            None => c,
        });
    }
    let digits = converted.trim_start_matches(['+', '-']);
    let valid = !digits.is_empty()
        && digits.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && digits.as_bytes().last().is_some_and(u8::is_ascii_digit)
        && !digits.contains("__")
        && digits.bytes().all(|c| c.is_ascii_digit() || c == b'_')
        && converted.chars().filter(|c| *c == '+' || *c == '-').count() <= 1;
    if valid {
        let digits = digits.bytes().filter(u8::is_ascii_digit).count();
        if digits > 4300 {
            return Err(format!(
                "Exceeds the limit (4300 digits) for integer string conversion: value has {digits} digits; \
                 use sys.set_int_max_str_digits() to increase the limit"
            ));
        }
        return Ok(converted.replace('_', "").parse::<i64>().unwrap_or(if converted.starts_with('-') {
            i64::MIN
        } else {
            i64::MAX
        }));
    }
    Err(format!("invalid literal for int() with base 10: {}", python_repr(s)))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT);
             INSERT INTO meta VALUES ('terms_names','core');
             CREATE TABLE term_links(system TEXT,key TEXT,term_id INTEGER,current INTEGER,source TEXT,added_at TEXT);
             CREATE TABLE terms(id INTEGER PRIMARY KEY,key TEXT,kind TEXT,origin TEXT NOT NULL);
             INSERT INTO terms VALUES (1,'x','character','user');
             CREATE TABLE term_tree(term_id INTEGER PRIMARY KEY,parent_id TEXT,merged_into TEXT,work_id TEXT,
               work_text TEXT,variant TEXT,artist TEXT,label_only INTEGER,mascot INTEGER,mascot_origin TEXT,
               original INTEGER,seq INTEGER);
             INSERT INTO term_tree VALUES (1,NULL,NULL,NULL,'','','',0,0,NULL,0,1);
             CREATE TABLE term_names(term_id INTEGER,lang TEXT,value TEXT,source TEXT,preferred INTEGER,role TEXT,
               seq INTEGER);
             INSERT INTO term_names VALUES (1,'zh','x','user',1,'display',1);
             CREATE TABLE term_key_names(kind TEXT,key TEXT,lang TEXT,source TEXT,value TEXT);
             INSERT INTO term_key_names VALUES ('work','unprojected','zh','user','未投影🙂');
             CREATE TABLE term_key_aliases(kind TEXT,key TEXT,value TEXT,position INTEGER);
             CREATE TABLE assets(uuid TEXT PRIMARY KEY);
             CREATE TABLE candidates(uuid TEXT,entity TEXT,source TEXT,score REAL);
             CREATE TABLE term_projection(term_id INTEGER);
             CREATE TABLE memberships(uuid TEXT,entity TEXT);
             CREATE TABLE tag_memberships(uuid TEXT,entity TEXT);
             CREATE TABLE duplicates(uuid TEXT,hidden INTEGER);
             CREATE TABLE excluded(uuid TEXT);",
        )
        .unwrap();
        c.execute_batch(crate::names_view::TABLES).unwrap();
        c.execute("INSERT INTO term_name_display VALUES ('x',0,'before','user','中','日',NULL,'tag')", []).unwrap();
        c.execute("INSERT INTO term_name_choice VALUES ('work','covered','projected','user',NULL,NULL,NULL)", [])
            .unwrap();
    }
    #[test]
    fn a_page_in_another_language_shows_english_or_romanized_names() {
        let c = Connection::open_in_memory().unwrap();
        fixture(&c);
        c.execute(
            "INSERT INTO term_name_display
             VALUES ('m',0,'初音未来','bangumi','初音未来','初音ミク',NULL,'hatsune_miku_(append)')",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO term_name_display VALUES ('r',0,'雷姆','bangumi','雷姆','レム','Rem','rem_(re:zero)')",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO term_name_choice VALUES ('work','genshin_impact','原神','builtin',NULL,NULL,NULL)", [])
            .unwrap();
        register_names(&c).unwrap();
        let shown = |c: &Connection| -> Vec<String> {
            [
                "entity_name('m')",
                "entity_name('r')",
                "entity_name('x')",
                "work_name('genshin_impact')",
                "json_extract(entity_info('m',0),'$.name')",
            ]
            .iter()
            .map(|f| c.query_row(&format!("SELECT {f}"), [], |r| r.get(0)).unwrap())
            .collect()
        };
        assert_eq!(shown(&c), ["初音未来", "雷姆", "before", "原神", "初音未来"]);
        {
            let _page = PageLanguage::set(Some("en"));
            // 自己起的名字保留；其次是英文名；再次是整理成可读形式的标签。
            assert_eq!(shown(&c), ["Hatsune Miku", "Rem", "before", "Genshin Impact", "Hatsune Miku"]);
        }
        let _page = PageLanguage::set(Some("zh-TW"));
        assert_eq!(shown(&c), ["初音未来", "雷姆", "before", "原神", "初音未来"]);
    }
    #[test]
    fn projected_names_do_not_load_legacy_tables() {
        let c = Connection::open_in_memory().unwrap();
        fixture(&c);
        // 查投影里的身份和已覆盖的作品时，没有理由构建 View。
        c.execute_batch("DROP TABLE term_links; DROP TABLE term_names; DROP TABLE term_key_names;").unwrap();
        register_names(&c).unwrap();
        let values: (String, String, String, String) = c
            .query_row(
                "SELECT entity_name('x'),entity_info('x',0),entity_info('absent',1),work_name('covered')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(values.0, "before");
        assert_eq!(
            values.1,
            r#"{"name": "before", "name_source": "user", "names": {"zh": "中", "ja": "日", "en": null}, "tag": "tag"}"#
        );
        assert_eq!(values.2, "{}");
        assert_eq!(values.3, "projected");
        // 投影之外的作品 key 只读取它自己的显示名（#105）。
        let c = Connection::open_in_memory().unwrap();
        fixture(&c);
        c.execute_batch("DROP TABLE term_tree; DROP TABLE term_links; DROP TABLE term_names;").unwrap();
        register_names(&c).unwrap();
        let name: String = c.query_row("SELECT work_name('unprojected')", [], |r| r.get(0)).unwrap();
        assert_eq!(name, "未投影🙂");
    }
    #[test]
    fn lazy_projected_names_keep_snapshot_and_refresh_on_next_request() {
        let path = std::env::temp_dir().join(format!(
            "illustash-http-lazy-names-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let mut c = crate::sqlite::open(&path).unwrap();
        c.pragma_update(None, "journal_mode", "WAL").unwrap();
        fixture(&c);
        {
            let tx = c.transaction().unwrap();
            register_names(&tx).unwrap();
            let writer = crate::sqlite::open(&path).unwrap();
            writer
                .execute_batch(
                    "UPDATE term_name_display SET name='after';
                     UPDATE term_name_choice SET name='new work';
                     UPDATE term_key_names SET value='新🙂';",
                )
                .unwrap();
            // 首次使用是在并发提交之后，包括旧版回退路径。
            for _ in 0..2 {
                let values: (String, String, String, String) = tx
                    .query_row(
                        "SELECT entity_name('x'),entity_info('x',0),work_name('covered'),work_name('unprojected')",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .unwrap();
                assert_eq!(values.0, "before");
                assert!(values.1.contains(r#""name": "before""#));
                assert_eq!(values.2, "projected");
                assert_eq!(values.3, "未投影🙂");
            }
            tx.commit().unwrap();
        }
        {
            let tx = c.transaction().unwrap();
            register_names(&tx).unwrap();
            let values: (String, String, String, String) = tx
                .query_row(
                    "SELECT entity_name('x'),entity_info('x',0),work_name('covered'),work_name('unprojected')",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .unwrap();
            assert_eq!(values.0, "after");
            assert!(values.1.contains(r#""name": "after""#));
            assert_eq!(values.2, "new work");
            assert_eq!(values.3, "新🙂");
        }
        drop(c);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn names_business_queries_share_snapshot_and_missing_key_fallbacks() {
        let path = std::env::temp_dir().join(format!(
            "illustash-http-tx-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let mut c = crate::sqlite::open(&path).unwrap();
        c.pragma_update(None, "journal_mode", "WAL").unwrap();
        fixture(&c);
        {
            let tx = c.transaction().unwrap();
            register_names(&tx).unwrap();
            let writer = crate::sqlite::open(&path).unwrap();
            writer.execute("UPDATE term_name_display SET name='after' WHERE entity='x'", []).unwrap();
            let values: (String, String) = tx
                .query_row("SELECT entity_name('x'),(SELECT name FROM term_name_display WHERE entity='x')", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .unwrap();
            assert_eq!(values, ("before".into(), "before".into()));
            let missing: (String, String, String, String) = tx
                .query_row(
                    "SELECT entity_name('absent'),entity_info('absent',0),
                            work_name('unprojected'),work_name('covered')",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .unwrap();
            assert_eq!(missing, ("".into(), "{}".into(), "未投影🙂".into(), "projected".into()));
            assert_ne!(entities(&tx, "x").unwrap().dump(), "[]");
            assert_eq!(tx.query_row("SELECT count(*) FROM name_matches", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
            assert_eq!(entities(&tx, "absent").unwrap().dump(), "[]");
            assert_ne!(entities(&tx, "").unwrap().dump(), "[]");
            tx.commit().unwrap();
        }
        {
            let tx = c.transaction().unwrap();
            register_names(&tx).unwrap();
            assert_eq!(tx.query_row("SELECT entity_name('x')", [], |r| r.get::<_, String>(0)).unwrap(), "after");
            tx.commit().unwrap();
        }
        drop(c);
        std::fs::remove_file(&path).unwrap();
    }
    #[test]
    fn python_query_and_integer_contract() {
        assert_eq!(query("/api/entities?q=a&q=&q=%FF%20b&remote=&n=2#n=3").get("q").unwrap(), "� b");
        assert!(!remote("/api/entities?remote="));
        assert!(remote("/api/entities?remote=0"));
        assert_eq!(python_int(" １２_３ ").unwrap(), 123);
        assert_eq!(python_int("\u{1c}2\u{1f}").unwrap_err(), "invalid literal for int() with base 10: '\\x1c2\\x1f'");
        assert_eq!(python_int("\u{200b}").unwrap_err(), "invalid literal for int() with base 10: '\\u200b'");
        assert_eq!(normalize(" Ｓｅｒｉｅｓ・Ｑ "), "seriesq");
        assert_eq!(unicode_normalization::UNICODE_VERSION, (15, 0, 0));
    }
}
