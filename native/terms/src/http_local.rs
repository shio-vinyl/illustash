//! 本地候选挑选和缓存的研究结果；不调用 worker，也不联网。
use crate::character_entities::ENTITIES;
use crate::{
    http::LibraryContext,
    http_json::Json,
    http_read::{display, rows},
    sql_fragments::{learnable, proto_hints, rating, unasked, unrejected, visible, ASK_DEFERRED, HINTS},
    Error, Result,
};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
pub(crate) const ROUTES: &[&str] = &["/api/ask", "/api/similar", "/api/clusters"];
/// 认一认自己的几种问题（`ask` 给出的 `kind`）。角色级的七天暂缓只看这几种问题里的「以后再说」：别处的
/// 「以后再说」（如「可能认错」的 `ambiguous`）只针对那一问，不触发角色级暂缓。单张图照旧不分 `kind`
/// 按「问过」处理（`unasked`），同一张图不会在认一认里再问一遍。
const ASK_KINDS: &str = "'hint','seed','sample'";
fn get<'a>(q: &'a HashMap<String, String>, k: &str) -> &'a str {
    q.get(k).map(String::as_str).unwrap_or("")
}
fn off() -> Json {
    Json::object([("status", "off".into())])
}
pub(crate) fn read(c: &Connection, path: &str, q: &HashMap<String, String>) -> Result<(Json, u16)> {
    let payload = match path {
        "/api/ask" => Json::object([(
            "batch",
            ask(c, get(q, "nsfw"), &get(q, "skip").split(',').filter(|s| !s.is_empty()).collect::<Vec<_>>())?,
        )]),
        "/api/similar" => Json::object([
            // 不带 seen=1 时回复和以前逐字节相同（#427）。
            ("groups", crate::http_browse::similar_groups(c, get(q, "kind"), get(q, "nsfw"), get(q, "seen") == "1")?),
            ("state", off()),
        ]),
        "/api/clusters" => {
            Json::object([("clusters", crate::http_browse::clusters(c, get(q, "nsfw"))?), ("learner", off())])
        }
        _ => return Err(Error::Invalid("未迁移本地接口".into())),
    };
    Ok((payload, 200))
}
fn choose(pool: &[Json]) -> Result<Option<&Json>> {
    if pool.is_empty() {
        return Ok(None);
    }
    let n = pool.len() as u64;
    let limit = u64::MAX - u64::MAX % n;
    loop {
        let mut bytes = [0; 8];
        getrandom::fill(&mut bytes).map_err(|e| Error::Invalid(e.to_string()))?;
        let draw = u64::from_ne_bytes(bytes);
        if draw < limit {
            return Ok(Some(&pool[(draw % n) as usize]));
        }
    }
}
fn ask(c: &Connection, nsfw: &str, skip: &[&str]) -> Result<Json> {
    if skip.len() > 50 {
        return Err(Error::Invalid("参数不正确".into()));
    }
    let seen = format!("{}{}", visible("m"), rating("m", nsfw)?);
    let hints = proto_hints("w.uuid,w.entity,w.score");
    let rest = format!("SELECT DISTINCT entity FROM asked WHERE kind IN ({ASK_KINDS}) AND {ASK_DEFERRED}");
    let wanted = |r: &Json| !skip.contains(&r.field("entity").text());
    let counted = format!(
        "SELECT h.entity,count(*) AS n
         FROM ({hints}) h
         JOIN assets m ON m.uuid=h.uuid
         WHERE {seen} AND h.entity NOT IN ({rest})
         GROUP BY h.entity
         HAVING n>=2
         ORDER BY n DESC"
    );
    let hinted: Vec<_> = rows(c, &counted, &[])?.into_iter().filter(wanted).take(12).collect();
    let (kind, pick) = if let Some(r) = choose(&hinted)? {
        ("hint", r.field("entity").text().to_owned())
    } else {
        let sql = format!(
            "SELECT m.entity,count(*) AS n,
               (SELECT count(*) FROM memberships x WHERE x.entity=m.entity AND x.status='confirmed') AS conf
             FROM memberships m
             JOIN single_assets o ON o.uuid=m.uuid
             WHERE m.status='machine' AND {seen} AND {unasked} AND {learnable} AND m.entity NOT IN ({rest})
             GROUP BY m.entity
             HAVING n>=2",
            unasked = unasked("m"),
            learnable = learnable("m")
        );
        let pool: Vec<_> = rows(c, &sql, &[])?.into_iter().filter(wanted).collect();
        let mut short: Vec<_> = pool.iter().filter(|r| r.field("conf").integer() < 5).cloned().collect();
        short.sort_by_key(|r| -r.field("n").integer());
        short.truncate(12);
        let kind = if short.is_empty() { "sample" } else { "seed" };
        let Some(r) = choose(if short.is_empty() { &pool } else { &short })? else {
            return Ok(None::<String>.into());
        };
        (kind, r.field("entity").text().to_owned())
    };
    let items = if kind == "hint" {
        let sql = format!(
            "SELECT h.uuid,m.width,m.height
             FROM ({hints}) h
             JOIN assets m ON m.uuid=h.uuid
             WHERE h.entity=? AND {seen}
             ORDER BY h.score DESC
             LIMIT 6"
        );
        rows(c, &sql, &[&pick])?
    } else {
        let sql = format!(
            "SELECT m.uuid,a.width,a.height
             FROM memberships m
             JOIN single_assets o ON o.uuid=m.uuid
             JOIN assets a ON a.uuid=m.uuid
             WHERE m.entity=? AND m.status='machine' AND {seen} AND {unasked}
             ORDER BY random()
             LIMIT 6",
            unasked = unasked("m")
        );
        rows(c, &sql, &[&pick])?
    };
    let name =
        c.query_row(&format!("SELECT entity_name(id) FROM {ENTITIES} WHERE id=?"), [&pick], |r| r.get::<_, String>(0))?;
    let confirmed =
        c.query_row("SELECT count(*) FROM memberships WHERE entity=? AND status='confirmed'", [&pick], |r| {
            r.get::<_, i64>(0)
        })?;
    Ok(Json::object([
        ("kind", kind.into()),
        ("entity", pick.into()),
        ("name", name.into()),
        ("confirmed", confirmed.into()),
        ("items", Json::Array(items)),
    ]))
}
pub(crate) fn suggest(context: &LibraryContext, body: &Json) -> Result<Json> {
    let mut c = crate::sqlite::open_with_flags(&context.db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    c.busy_timeout(std::time::Duration::from_secs(10))?;
    let tx = c.transaction()?;
    crate::http_read::register_names(&tx)?;
    let mut body = body.clone();
    body.set("confirm", true.into());
    let ids = crate::app_commands::selected_scope(&tx, context, &body, false)?;
    tx.execute_batch("CREATE TEMP TABLE picked(uuid TEXT PRIMARY KEY)")?;
    for id in ids.iter().take(2000) {
        tx.execute("INSERT INTO picked VALUES (?)", [id])?;
    }
    let sql = format!(
        "SELECT e.id,entity_name(e.id) AS name,e.work,count(DISTINCT v.uuid) AS count
         FROM {HINTS} v
         JOIN picked p ON p.uuid=v.uuid
         JOIN {ENTITIES} e ON e.id=v.entity
         WHERE NOT EXISTS(SELECT 1 FROM memberships m WHERE m.uuid=v.uuid AND m.entity=v.entity)
           AND {unrejected}
         GROUP BY e.id
         ORDER BY count DESC,min(v.weak),max(v.score) DESC,name
         LIMIT 9",
        unrejected = unrejected("v")
    );
    let selected = rows(&tx, &sql, &[])?;
    let mut result = selected.into_iter().map(|r| display(&tx, r)).collect::<Result<Vec<_>>>()?;
    crate::http_role_labels::RoleLabels::rows(&tx, &mut result)?;
    tx.commit()?;
    Ok(Json::Array(result))
}
