//! 角色与身份的 HTTP 读取。查询顺序、事后的稳定排序和 dict 插入顺序都与 Store 一致。
use crate::character_entities::ENTITIES;
use crate::character_links::LINKS;
use crate::character_projection::VARIANT_MEMBERSHIPS;
use crate::character_relations::{EVIDENCE, RELATIONS};
use crate::error::invalid;
use crate::{
    http::LibraryContext,
    http_json::{parse_python, Json},
    http_read::{display, python_int, rows, strings, trim},
    sql_fragments::{hybrid, rating, visible},
    Error, Result,
};
use rusqlite::Connection;
use std::collections::{BTreeSet, HashMap, HashSet};

pub(crate) const TOGETHER: &str = "/api/character/together";
const TOGETHER_LIMIT: &str = "6";

pub const ROUTES: &[&str] = &[
    "/api/characters",
    "/api/tagged",
    "/api/character/profile",
    "/api/character/activity",
    "/api/character/stats",
    TOGETHER,
    "/api/identity/count",
    "/api/identity/queue",
];
fn text(j: &Json, k: &str) -> String {
    j.field(k).text().to_owned()
}
fn number(j: &Json, k: &str) -> i64 {
    j.field(k).integer()
}
fn array(v: Vec<String>) -> Json {
    Json::Array(v.into_iter().map(Json::from).collect())
}
fn remove(j: &mut Json, k: &str) -> Json {
    if let Json::Object(v) = j {
        if let Some(i) = v.iter().position(|(x, _)| x == k) {
            return v.remove(i).1;
        }
    }
    None::<String>.into()
}
fn truth(v: &Json) -> bool {
    match v {
        Json::Scalar(x) => crate::character_commands::truthy(x),
        Json::Float(x) => *x != 0.,
        Json::Array(x) => !x.is_empty(),
        Json::Object(x) => !x.is_empty(),
        Json::Integer(x) => x != "0",
    }
}
fn shown(c: &Connection, mut r: Json, skin: bool, identity: bool) -> Result<Json> {
    r = display(c, r)?;
    if skin {
        let s: String = c.query_row("SELECT entity_info(?,1)", [text(&r, "id")], |r| r.get(0))?;
        r.extend(parse_python(&s)?);
    }
    if !identity {
        for k in ["node_type", "character_id", "variant_id", "canonical_id"] {
            remove(&mut r, k);
        }
    }
    Ok(r)
}
fn characters(c: &Connection, q: &HashMap<String, String>, minimum: i64) -> Result<Json> {
    if minimum < 0 {
        return invalid("最小成员数不正确");
    }
    let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
    let nsfw = get("nsfw");
    let summary = get("summary") == "1";
    let mut result =
        character_rows(c, q, minimum)?.into_iter().map(|r| shown(c, r, false, !summary)).collect::<Result<Vec<_>>>()?;
    if result.is_empty() {
        return Ok(Json::Array(result));
    }
    let ids = Json::Array(result.iter().map(|r| r.field("id").clone()).collect()).dump();
    let mut variants = skins(c, &ids, nsfw, summary)?;
    for r in &mut result {
        r.set("variants", Json::Array(variants.remove(&text(r, "id")).unwrap_or_default()));
        if summary {
            r.set("summary", true.into());
        }
    }
    choose_covers(c, &mut result, &ids, nsfw)?;
    if summary {
        result = result.into_iter().map(summarized).collect();
    }
    Ok(Json::Array(result))
}
/// /api/characters 列出的角色（整理成显示格式之前）：按 `q` 里的搜索、作品、分级和数量筛选。
fn character_rows(c: &Connection, q: &HashMap<String, String>, minimum: i64) -> Result<Vec<Json>> {
    let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
    let query: String = trim(get("q")).chars().take(160).collect();
    let ident = get("id");
    let nsfw = get("nsfw");
    if !query.is_empty() {
        crate::http_read::match_names(c, &query)?;
    }
    let work = get("work");
    let mut args = vec![nsfw.to_owned()];
    let work_ids = if work.is_empty() { Vec::new() } else { crate::http_read::work_tree(c, work)? };
    let work_filter = if work_ids.is_empty() {
        String::new()
    } else {
        format!(" AND e.work_id IN ({})", vec!["?"; work_ids.len()].join(","))
    };
    if !ident.is_empty() {
        args.extend([ident.to_owned(), ident.to_owned(), ident.to_owned()]);
    }
    args.extend(work_ids);
    args.push(minimum.to_string());
    let sql = format!(
        "SELECT e.id,entity_name(e.id) AS name,e.work,e.kind,e.artist,e.label_only,e.mascot,e.mascot_origin,
           count(m.uuid) AS count,coalesce(sum(m.status='machine'),0) AS unchecked,
           min(CASE WHEN ?<>'hide'
                      OR NOT coalesce({nsfw},0)
                    THEN m.uuid END) AS cover,
           (SELECT max(reviewed_at) FROM reviews r WHERE r.entity=e.id) AS reviewed_at
         FROM {ENTITIES} e
         {matched}
         LEFT JOIN {members} m ON m.entity=e.id AND {visible}{rating}
         WHERE e.parent_id IS NULL AND e.kind<>'work' AND e.merged_into IS NULL{mascot}{only}{work_filter}
         GROUP BY e.id
         HAVING count(m.uuid)>=?
         ORDER BY {ranking}count DESC,name,e.id",
        nsfw = crate::sql_fragments::nsfw("m"),
        matched = if query.is_empty() { "" } else { "JOIN name_matches sm ON sm.entity=e.id" },
        members = if !ident.is_empty() {
            "(SELECT * FROM memberships WHERE entity=? UNION ALL SELECT * FROM tag_memberships WHERE entity=?)"
        } else if !query.is_empty() {
            "(SELECT * FROM memberships UNION ALL
                SELECT m.* FROM tag_memberships m JOIN terms t ON t.key=m.entity
                JOIN term_tree r ON r.term_id=t.id AND r.label_only=1 AND r.original=0)"
        } else {
            "memberships"
        },
        visible = visible("m"),
        rating = rating("m", nsfw)?,
        mascot = if !ident.is_empty() {
            ""
        } else if query.is_empty() {
            " AND e.mascot=0 AND NOT (e.label_only=1 AND e.original=0)"
        } else {
            " AND e.mascot=0"
        },
        only = if ident.is_empty() { "" } else { " AND e.id=?" },
        ranking = if query.is_empty() { "" } else { "sm.rank DESC," },
    );
    bindable_minimum(get("min"))?;
    let mut bound: Vec<rusqlite::types::Value> = args.into_iter().map(Into::into).collect();
    *bound.last_mut().expect("minimum argument") = minimum.into();
    crate::http_read::typed_rows(c, &sql, &bound)
}
/// 请求里的整数没有上限（沿用旧实现）；超出 SQLite 有符号 64 位绑定范围的最小值照旧按绑定错误拒绝。
/// 在 SQL 绑定处检查，位于与 Store.characters 相同的作品、分级检查之后。
fn bindable_minimum(min: &str) -> Result<()> {
    let exact_min: String = min
        .trim()
        .chars()
        .filter(|c| *c != '_')
        .map(|c| crate::compat_unicode::decimal(c).map(|n| (b'0' + n) as char).unwrap_or(c))
        .collect();
    if !exact_min.is_empty() && exact_min.parse::<i64>().is_err() {
        return Err(Error::Sql(rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "整数超出数据库支持的范围",
        )))));
    }
    Ok(())
}

/// 角色 `ids`（JSON 列表）的皮肤，按角色分组；除非只要摘要，否则附上各自的张数。
fn skins(c: &Connection, ids: &str, nsfw: &str, summary: bool) -> Result<HashMap<String, Vec<Json>>> {
    let variant_count = if summary {
        "NULL".to_owned()
    } else {
        format!(
            "(SELECT count(*) FROM {VARIANT_MEMBERSHIPS} v WHERE v.entity=e.id AND {}{})",
            visible("v"),
            rating("v", nsfw)?
        )
    };
    let mut variants: HashMap<String, Vec<Json>> = HashMap::new();
    let skins = format!(
        "SELECT e.id,e.parent_id,e.variant AS name,{variant_count} AS count
         FROM {ENTITIES} e
         WHERE e.parent_id IN (SELECT value FROM json_each(?)) AND e.kind<>'work' AND e.merged_into IS NULL
         ORDER BY e.variant,e.id"
    );
    for r in rows(c, &skins, &[ids])? {
        let parent = text(&r, "parent_id");
        variants.entry(parent).or_default().push(shown(c, r, true, !summary)?);
    }
    Ok(variants)
}
/// 给每个角色选一张封面：从它最好的十二张可见图片中挑，优先选只有它一人的图，以及还没有被图片更多的
/// 角色用作封面的图。
fn choose_covers(c: &Connection, result: &mut [Json], ids: &str, nsfw: &str) -> Result<()> {
    let ranked = format!(
        "SELECT entity,uuid
         FROM (
           SELECT m.entity,m.uuid,
             row_number() OVER (
               PARTITION BY m.entity
               ORDER BY m.status='machine',coalesce(a.width>a.height,0),a.added_at DESC,m.uuid) AS rk
           FROM memberships m
           JOIN assets a ON a.uuid=m.uuid
           WHERE m.entity IN (SELECT value FROM json_each(?)){})
         WHERE rk<=12
         ORDER BY entity,rk",
        rating("m", nsfw)?
    );
    let ranked = rows(c, &ranked, &[ids])?;
    let pool = array(ranked.iter().map(|r| text(r, "uuid")).collect::<BTreeSet<_>>().into_iter().collect()).dump();
    let vis: HashSet<String> = strings(
        c,
        &format!("SELECT a.uuid FROM assets a WHERE a.uuid IN (SELECT value FROM json_each(?)) AND {}", visible("a")),
        &[&pool],
    )?
    .into_iter()
    .collect();
    let shared: HashSet<String> = strings(
        c,
        "SELECT uuid FROM memberships WHERE uuid IN (SELECT value FROM json_each(?)) GROUP BY uuid HAVING count(*)>1",
        &[&pool],
    )?
    .into_iter()
    .collect();
    let mut options: HashMap<String, Vec<String>> = HashMap::new();
    for r in ranked {
        let u = text(&r, "uuid");
        if vis.contains(&u) {
            options.entry(text(&r, "entity")).or_default().push(u);
        }
    }
    for opts in options.values_mut() {
        opts.sort_by_key(|u| shared.contains(u));
    }
    let mut ordered: Vec<usize> = (0..result.len()).collect();
    ordered.sort_by_key(|i| -number(&result[*i], "count"));
    let mut used = HashSet::new();
    for i in ordered {
        let r = &mut result[i];
        let opts = options.get(&text(r, "id"));
        let cover = opts
            .and_then(|v| v.iter().find(|u| !used.contains(*u)).or(v.first()))
            .map(|u| Json::from(u.clone()))
            .unwrap_or_else(|| r.field("cover").clone());
        used.insert(cover.text().to_owned());
        r.set("cover", cover);
    }
    Ok(())
}
/// 摘要列表里显示的角色。
fn summarized(r: Json) -> Json {
    const FIELDS: [&str; 12] = [
        "id",
        "name",
        "tag",
        "name_source",
        "work",
        "work_id",
        "work_name",
        "kind",
        "count",
        "unchecked",
        "cover",
        "summary",
    ];
    let mut s = Json::Object(Vec::new());
    for k in FIELDS {
        if let Some(v) = r.get(k) {
            s.set(k, v.clone());
        }
    }
    let variants = if let Json::Array(v) = r.field("variants") {
        v.iter().map(|v| Json::object([("id", v.field("id").clone()), ("name", v.field("name").clone())])).collect()
    } else {
        Vec::new()
    };
    s.set("variants", Json::Array(variants));
    s
}
fn profile(c: &Connection, e: &str) -> Result<Json> {
    let id = crate::character_commands::resolve(c, e)?;
    let row = rows(
        c,
        &format!(
            "SELECT id,parent_id,kind,original,label_only,work_id
             FROM {ENTITIES}
             WHERE kind<>'work' AND id=?"
        ),
        &[&id],
    )?
    .into_iter()
    .next()
    .ok_or_else(|| Error::Invalid("角色不存在".into()))?;
    if truth(row.field("parent_id")) {
        return invalid("请编辑角色本身，子标签跟随角色");
    }
    let shown = display(c, Json::object([("id", id.clone().into())]))?;
    let user = strings(
        c,
        "SELECT value
         FROM term_names
         WHERE term_id = (SELECT id FROM terms WHERE key = ?) AND role = 'display' AND source = 'user'
         ORDER BY preferred DESC
         LIMIT 1",
        &[&id],
    )?
    .into_iter()
    .next()
    .unwrap_or_default();
    let tag = strings(
        c,
        &format!(
            "SELECT l.key
                 FROM {LINKS} l
                 WHERE l.entity_id = ? AND l.core_system = 'danbooru' AND l.current = 1
                 LIMIT 1",
            LINKS = crate::character_links::LINKS
        ),
        &[&id],
    )?
    .into_iter()
    .next()
    .unwrap_or_default();
    let other: i64 = c.query_row(
        "SELECT count(DISTINCT value)
         FROM term_names
         WHERE term_id = (SELECT id FROM terms WHERE key = ?) AND role = 'alias' AND source <> 'user'",
        [&id],
        |r| r.get(0),
    )?;
    let work = text(&row, "work_id");
    let name =
        if work.is_empty() { String::new() } else { c.query_row("SELECT entity_name(?)", [&work], |r| r.get(0))? };
    let catalog = catalog_changes(c, &id)?;
    let mut profile = Json::object([
        ("id", id.clone().into()),
        ("name", shown.field("name").clone()),
        ("user_name", user.into()),
        (
            "aliases",
            array(strings(
                c,
                "SELECT value
                 FROM term_names
                 WHERE term_id = (SELECT id FROM terms WHERE key = ?) AND role = 'alias' AND source = 'user'
                 ORDER BY position, value",
                &[&id],
            )?),
        ),
        ("other_aliases", other.into()),
        ("tag", tag.into()),
        ("work_id", row.field("work_id").clone()),
        ("work_name", name.into()),
        ("oc", (number(&row, "original") != 0).into()),
        ("label_only", (number(&row, "label_only") != 0).into()),
    ]);
    // 只在目录动过这个角色时才带这一项，其余资料的响应和以前逐字节相同。
    if !matches!(&catalog, Json::Array(items) if items.is_empty()) {
        profile.set("catalog", catalog);
    }
    Ok(profile)
}

/// 角色目录合并进这个角色、挂到它下面的角色（#837），给「编辑资料」里的「拆开」用：[{id, kind, name}]，
/// kind 是 `merged` 或 `attached`。
fn catalog_changes(c: &Connection, id: &str) -> Result<Json> {
    let state = strings(c, "SELECT value FROM meta WHERE key=?", &[illustash_core::character_import::CATALOG_META])?;
    let state: serde_json::Value =
        state.first().and_then(|s| serde_json::from_str(s).ok()).unwrap_or(serde_json::Value::Null);
    let mut out = Vec::new();
    for kind in ["merged", "attached"] {
        for pair in state[kind].as_array().into_iter().flatten() {
            let (Some(other), Some(owner)) = (pair[0].as_str(), pair[1].as_str()) else { continue };
            if owner != id {
                continue;
            }
            let name: String = c.query_row("SELECT entity_name(?)", [other], |r| r.get(0))?;
            out.push(Json::object([("id", other.into()), ("kind", kind.into()), ("name", name.into())]));
        }
    }
    Ok(Json::Array(out))
}
fn activity(c: &Connection, nsfw: &str) -> Result<Json> {
    let t = crate::platform_time::local_date(30 * 86400);
    let since = format!("{:04}-{:02}-{:02}", t.tm_year + 1900, t.tm_mon + 1, t.tm_mday);
    let mut out = Json::Object(Vec::new());
    let sql = format!(
        "SELECT m.entity,sum(a.added_at>=?) AS recent,max(substr(a.added_at,1,10)) AS last
         FROM memberships m
         JOIN assets a ON a.uuid=m.uuid
         WHERE {}{}
         GROUP BY m.entity",
        visible("m"),
        rating("m", nsfw)?
    );
    for r in rows(c, &sql, &[&since])? {
        let last = Json::object([("recent", number(&r, "recent").into()), ("last", text(&r, "last").into())]);
        out.set(text(&r, "entity"), last);
    }
    Ok(out)
}
fn stats(c: &Connection, id: &str, nsfw: &str, exclude: &str) -> Result<Json> {
    if id.is_empty() {
        return invalid("缺少角色");
    }
    let where_ = format!("m.entity=? AND {}{}", visible("m"), rating("m", nsfw)?);
    let monthly = format!(
        "SELECT substr(a.added_at,1,7) AS month,count(*) AS count
         FROM memberships m
         JOIN assets a ON a.uuid=m.uuid
         WHERE {where_}
         GROUP BY 1"
    );
    let per = rows(c, &monthly, &[id])?;
    let mut dated: Vec<String> = per.iter().map(|r| text(r, "month")).filter(|s| !s.is_empty()).collect();
    dated.sort();
    let t = crate::platform_time::local_date(0);
    let mut y = t.tm_year + 1900;
    let mut m = t.tm_mon + 1;
    let mut months = Vec::new();
    for _ in 0..12 {
        let k = format!("{y:04}-{m:02}");
        let n = per.iter().find(|r| text(r, "month") == k).map(|r| number(r, "count")).unwrap_or(0);
        months.push(Json::object([("month", k.into()), ("count", n.into())]));
        if m == 1 {
            y -= 1;
            m = 12;
        } else {
            m -= 1;
        }
    }
    months.reverse();
    let recent = format!(
        "SELECT m.uuid
         FROM memberships m
         JOIN assets a ON a.uuid=m.uuid
         WHERE {where_} AND m.uuid<>?
         ORDER BY a.added_at DESC,m.uuid
         LIMIT 4"
    );
    let recent = strings(c, &recent, &[id, exclude])?;
    Ok(Json::object([
        ("count", per.iter().map(|r| number(r, "count")).sum::<i64>().into()),
        ("first", dated.first().cloned().unwrap_or_default().into()),
        ("months", Json::Array(months)),
        ("recent", array(recent)),
    ]))
}
fn together(c: &Connection, id: &str, nsfw: &str, limit: i64) -> Result<Json> {
    // memberships 的主键是（角色，图片），每个同框角色在同一张图上只计一次。
    let sql = format!(
        "SELECT peer.entity AS id,count(*) AS count
         FROM memberships m
         JOIN assets a ON a.uuid=m.uuid
         JOIN memberships peer ON peer.uuid=m.uuid AND peer.entity<>m.entity
         JOIN {ENTITIES} subject ON subject.id=m.entity AND subject.label_only=0
         JOIN {ENTITIES} other ON other.id=peer.entity AND other.label_only=0
         WHERE m.entity=? AND {visible}{rating}
         GROUP BY peer.entity
         ORDER BY count DESC,peer.entity
         LIMIT ?",
        visible = visible("m"),
        rating = rating("m", nsfw)?,
    );
    Ok(Json::Array(crate::http_read::typed_rows(c, &sql, &[id.to_owned().into(), limit.into()])?))
}
fn people(c: &Connection) -> Result<HashMap<String, String>> {
    Ok(rows(c, "SELECT id,character_id FROM identity_nodes", &[])?
        .into_iter()
        .map(|r| (text(&r, "id"), text(&r, "character_id")))
        .collect())
}
fn page<T>(v: Vec<T>, offset: i64, limit: i64) -> Vec<T> {
    v.into_iter().skip(usize::try_from(offset).unwrap_or(usize::MAX)).take(limit as usize).collect()
}
fn pair_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.into(), b.into())
    } else {
        (b.into(), a.into())
    }
}
fn numeric(j: &Json) -> Option<f64> {
    match j {
        Json::Float(x) => Some(*x),
        Json::Scalar(v) => v.as_f64(),
        _ => None,
    }
}
/// 身份队列可能询问的身份对，按家族每对只出现一次：附上这一对的最短距离和最先找到的提示（连同其关系）。
struct Pairs {
    family: HashMap<String, String>,
    counts: HashMap<String, i64>,
    decided: HashSet<(String, String)>,
    apart: BTreeSet<(String, String)>,
    skip: HashSet<String>,
    marks: HashSet<String>,
    found: Vec<Json>,
    keys: HashMap<(String, String), usize>,
}
impl Pairs {
    /// `shown`：对计入数量的成员关系 `m` 应用的内容筛选。
    fn load(c: &Connection, shown: &str) -> Result<Self> {
        let family = people(c)?;
        let fam = |e: &str| family.get(e).cloned().unwrap_or_else(|| e.to_owned());
        let counted =
            format!("SELECT entity,count(*) AS count FROM memberships m WHERE {}{shown} GROUP BY entity", visible("m"));
        let counts = rows(c, &counted, &[])?.iter().map(|r| (text(r, "entity"), number(r, "count"))).collect();
        let decided = rows(c, &format!("SELECT r.a, r.b FROM {RELATIONS} r WHERE r.status <> 'proposed'"), &[])?
            .iter()
            .map(|r| pair_key(&fam(&text(r, "a")), &fam(&text(r, "b"))))
            .collect();
        let skipped = strings(c, "SELECT value FROM prefs WHERE key='identity_skipped'", &[])?
            .into_iter()
            .next()
            .unwrap_or_else(|| "[]".into());
        let skip = match parse_python(&skipped)? {
            Json::Array(v) => v.iter().map(|x| x.text().to_owned()).collect(),
            _ => HashSet::new(),
        };
        let marks =
            strings(c, &format!("SELECT id FROM {ENTITIES} WHERE label_only=1"), &[])?.iter().map(|e| fam(e)).collect();
        let apart = crate::character_identity::kept_apart_pairs(c)?;
        Ok(Self { family, counts, decided, apart, skip, marks, found: Vec::new(), keys: HashMap::new() })
    }
    fn family(&self, e: &str) -> String {
        self.family.get(e).cloned().unwrap_or_else(|| e.to_owned())
    }
    fn add(&mut self, a: &str, b: &str, distance: Json, hint: Json, relation: Json, catalog_kind: Option<&str>) {
        let (a, b) = (self.family(a), self.family(b));
        let key = pair_key(&a, &b);
        if a == b
            || self.marks.contains(&a)
            || self.marks.contains(&b)
            || self.decided.contains(&key)
            || self.apart.contains(&key)
            || self.skip.contains(&format!("{}|{}", key.0, key.1))
            || self.counts.get(&a).copied().unwrap_or(0) == 0
            || self.counts.get(&b).copied().unwrap_or(0) == 0
        {
            return;
        }
        let found = &mut self.found;
        let i = *self.keys.entry(key).or_insert_with(|| {
            found.push(Json::object([
                ("a", a.clone().into()),
                ("b", b.clone().into()),
                ("distance", None::<String>.into()),
                ("hint", None::<String>.into()),
                ("relation_id", None::<String>.into()),
            ]));
            found.len() - 1
        });
        let item = &mut found[i];
        if let Some(d) = numeric(&distance) {
            if numeric(item.field("distance")).is_none_or(|old| d < old) {
                item.set("distance", distance);
            }
        }
        if truth(&hint) && !truth(item.field("hint")) {
            item.set("hint", hint);
            item.set("relation_id", relation);
        }
        if let Some(kind) = catalog_kind.filter(|_| item.get("catalog_kind").is_none()) {
            item.set("catalog_kind", kind.into());
        }
    }
}
/// 识别器认为相近的身份对；以它们的 Danbooru 关系作为提示。
fn close_pairs(c: &Connection, pairs: &mut Pairs, count_only: bool) -> Result<()> {
    for r in rows(c, "SELECT a,b,original_distance,distance,evidence_json FROM identity_pairs", &[])? {
        let mut hint = None::<String>.into();
        let distance = if count_only {
            None::<String>.into()
        } else {
            let e = parse_python(r.field("evidence_json").text())?;
            if ["relation", "subject", "object"].iter().all(|k| e.get(k).is_some_and(truth)) {
                hint = Json::object([
                    ("kind", e.field("relation").clone()),
                    ("subject", e.field("subject").clone()),
                    ("object", e.field("object").clone()),
                    ("source", "Danbooru".into()),
                ]);
            }
            if numeric(r.field("distance")).is_some() {
                r.field("distance").clone()
            } else {
                r.field("original_distance").clone()
            }
        };
        pairs.add(&text(&r, "a"), &text(&r, "b"), distance, hint, None::<String>.into(), None);
    }
    Ok(())
}
/// 仍处于提议状态的关系，以它们的 Danbooru 标签作为提示。
fn proposed_pairs(c: &Connection, pairs: &mut Pairs, count_only: bool) -> Result<()> {
    // 原来的旧表按 id 扫描：同一对身份的提示取 id 最小的关系。
    let proposed = format!(
        "SELECT r.id, r.a, r.b, r.kind,
           (SELECT min(e.source) FROM {EVIDENCE} e WHERE e.relation_id = r.id) AS source
         FROM {RELATIONS} r
         WHERE r.status = 'proposed'
         ORDER BY r.id"
    );
    let danbooru = format!(
        "SELECT l.entity_id, l.key
         FROM {LINKS} l
         WHERE l.core_system = 'danbooru' AND l.entity_id IN (?, ?)",
        LINKS = crate::character_links::LINKS
    );
    for r in rows(c, &proposed, &[])? {
        let a = text(&r, "a");
        let b = text(&r, "b");
        let hint = if count_only {
            None::<String>.into()
        } else {
            let tags: HashMap<String, String> =
                rows(c, &danbooru, &[&a, &b])?.iter().map(|r| (text(r, "entity_id"), text(r, "key"))).collect();
            Json::object([
                ("kind", r.field("kind").clone()),
                ("subject", tags.get(&a).cloned().unwrap_or_default().into()),
                ("object", tags.get(&b).cloned().unwrap_or_default().into()),
                ("source", text(&r, "source").into()),
            ])
        };
        pairs.add(&a, &b, None::<String>.into(), hint, r.field("id").clone(), None);
    }
    Ok(())
}

/// 词表关系按当前图库和启用的目录生成，撤回词表或撤销判断后无需修复候选缓存。
fn catalog_pairs(c: &Connection, pairs: &mut Pairs) -> Result<()> {
    use crate::character_catalog::{Action, Relation};

    let hybrid = hybrid(c)?;
    let catalog = crate::character_catalog::current();
    let mut linked: HashMap<u32, BTreeSet<String>> = HashMap::new();
    let lookup = format!(
        "SELECT l.key,l.entity_id FROM {LINKS} l JOIN {ENTITIES} e ON e.id=l.entity_id
         WHERE l.core_system IN ('danbooru','wd_character') AND l.current=1
           AND e.kind<>'work' AND e.parent_id IS NULL AND e.merged_into IS NULL AND e.mascot=0 AND e.label_only=0"
    );
    for row in c.prepare_cached(&lookup)?.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (tag, id) = row?;
        if let Some(entry) = catalog.character(&tag).filter(|_| pairs.counts.contains_key(&id)) {
            linked.entry(entry.id).or_default().insert(id);
        }
    }
    let declined: HashSet<_> =
        crate::character_import::declined_pairs(c)?.iter().map(|(a, b)| pair_key(a, b)).collect();
    let mut add = |a: &str, b: &str, kind: &'static str| {
        if !declined.contains(&pair_key(a, b)) {
            let empty = Json::from(serde_json::Value::Null);
            pairs.add(a, b, empty.clone(), empty.clone(), empty, Some(kind));
        }
    };
    for entry in catalog.characters() {
        let Some(subjects) = linked.get(&entry.id) else { continue };
        if hybrid {
            for (i, a) in subjects.iter().enumerate() {
                for b in subjects.iter().skip(i + 1) {
                    add(a, b, "duplicate_of");
                }
            }
        }
        if !hybrid && entry.action != Some(Action::Ask) {
            continue;
        }
        let (Some(objects), Some(relation)) = (entry.parent.and_then(|p| linked.get(&p)), entry.relation) else {
            continue;
        };
        let kind = match relation {
            Relation::Appearance => "appearance",
            Relation::Form => "form",
            Relation::Unknown => "unknown",
        };
        for a in subjects {
            for b in objects {
                add(a, b, kind);
            }
        }
    }
    Ok(())
}

fn queue(c: &Connection, limit: i64, offset: i64, nsfw: &str, count_only: bool) -> Result<Json> {
    if !(1..=100).contains(&limit) || offset < 0 {
        return invalid("分页参数不正确");
    }
    let shown = rating("m", nsfw)?;
    let mut pairs = Pairs::load(c, &shown)?;
    close_pairs(c, &mut pairs, count_only)?;
    proposed_pairs(c, &mut pairs, count_only)?;
    catalog_pairs(c, &mut pairs)?;
    let Pairs { mut found, counts, .. } = pairs;
    let total = found.len();
    if count_only {
        return Ok(Json::object([("total", (total as i64).into())]));
    }
    found.sort_by(|x, y| {
        let n = |v: &Json| counts[&text(v, "a")] + counts[&text(v, "b")];
        n(y).cmp(&n(x)).then(text(x, "a").cmp(&text(y, "a"))).then(text(x, "b").cmp(&text(y, "b")))
    });
    let samples = format!(
        "SELECT m.uuid
         FROM memberships m
         WHERE m.entity=? AND {}{shown}
         ORDER BY m.status='confirmed' DESC,m.score DESC,m.uuid
         LIMIT 4",
        visible("m")
    );
    let mut items = page(found, offset, limit);
    for x in &mut items {
        for side in ["a", "b"] {
            let e = text(x, side);
            let name: String = c.query_row("SELECT entity_info(?,0)", [&e], |r| r.get(0))?;
            let info = parse_python(&name)?;
            let samples = strings(c, &samples, &[&e])?;
            x.set(
                side,
                Json::object([
                    ("id", e.clone().into()),
                    ("name", info.get("name").cloned().unwrap_or_else(|| "".into())),
                    ("tag", info.get("tag").cloned().unwrap_or_else(|| "".into())),
                    ("count", counts[&e].into()),
                    ("samples", array(samples)),
                ]),
            );
        }
    }
    Ok(Json::object([("items", Json::Array(items)), ("total", (total as i64).into())]))
}
pub(crate) fn assignments(c: &Connection, id: &str) -> Result<Json> {
    let (a, mut p) = if hybrid(c)? {
        let assigned = format!(
            "
            SELECT m.uuid,m.entity AS character_id,v.entity AS variant_id,m.status,m.origin
            FROM memberships m
            LEFT JOIN (SELECT entity,parent_id FROM {VARIANT_MEMBERSHIPS} WHERE uuid=?) v ON v.parent_id=m.entity
            WHERE m.uuid=?"
        );
        let undetermined = "
            SELECT options_json,reason FROM identity_uncertain WHERE uuid=?
            UNION ALL
            SELECT options_json,reason FROM identity_suspended WHERE uuid=?";
        (rows(c, &assigned, &[id, id])?, rows(c, undetermined, &[id, id])?)
    } else {
        (Vec::new(), Vec::new())
    };
    for r in &mut p {
        let options = remove(r, "options_json");
        r.set("options", parse_python(options.text())?);
    }
    Ok(Json::object([("assignments", Json::Array(a)), ("undetermined", Json::Array(p))]))
}
pub fn read(c: &Connection, _context: &LibraryContext, path: &str, q: &HashMap<String, String>) -> Result<(Json, u16)> {
    let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
    let int =
        |k: &str, default: &str| python_int(q.get(k).map(String::as_str).unwrap_or(default)).map_err(Error::Invalid);
    let value = match path {
        "/api/characters" => characters(c, q, int("min", "0")?)?,
        "/api/tagged" => {
            let sql = format!(
                "SELECT e.id,entity_name(e.id) AS name,
                   (SELECT count(*) FROM tag_memberships m WHERE m.entity=e.id AND {visible}{rating}) AS count
                 FROM {ENTITIES} e
                 WHERE e.mascot=1 AND e.merged_into IS NULL
                 ORDER BY count DESC,name,e.seq",
                visible = visible("m"),
                rating = rating("m", get("nsfw"))?,
            );
            Json::Array(rows(c, &sql, &[])?)
        }
        "/api/character/profile" => profile(c, get("entity"))?,
        "/api/character/activity" => activity(c, get("nsfw"))?,
        "/api/character/stats" => stats(c, get("id"), get("nsfw"), get("exclude"))?,
        TOGETHER => together(c, get("id"), get("nsfw"), int("limit", TOGETHER_LIMIT)?)?,
        "/api/identity/count" => queue(c, 20, 0, get("nsfw"), true)?,
        "/api/identity/queue" => queue(c, int("limit", "20")?, int("offset", "0")?, get("nsfw"), false)?,
        _ => return invalid("unknown character read"),
    };
    Ok((value, 200))
}

pub fn validate(path: &str, q: &HashMap<String, String>) -> Result<()> {
    let args: &[(&str, &str)] = match path {
        "/api/characters" => &[("min", "0")],
        TOGETHER => &[("limit", TOGETHER_LIMIT)],
        "/api/identity/queue" => &[("limit", "20"), ("offset", "0")],
        _ => &[],
    };
    let numbers = args
        .iter()
        .map(|(k, default)| python_int(q.get(*k).map(String::as_str).unwrap_or(default)).map_err(Error::Invalid))
        .collect::<Result<Vec<_>>>()?;
    let get = |key| q.get(key).map(String::as_str).unwrap_or("");
    match path {
        "/api/characters" if numbers[0] < 0 => return invalid("最小成员数不正确"),
        "/api/identity/queue" => {
            if !(1..=100).contains(&numbers[0]) || numbers[1] < 0 {
                return invalid("分页参数不正确");
            }
            rating("m", get("nsfw"))?;
        }
        "/api/identity/count" => {
            rating("m", get("nsfw"))?;
        }
        TOGETHER if numbers[0] <= 0 => return invalid("分页参数不正确"),
        "/api/character/stats" | TOGETHER => {
            if get("id").is_empty() {
                return invalid("缺少角色");
            }
            rating("m", get("nsfw"))?;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimum_overflow_keeps_sql_error_and_unicode_boundary() {
        assert!(bindable_minimum("９_２２３_３７２_０３６_８５４_７７５_８０７").is_ok());
        for minimum in ["9223372036854775808", "９_２２３_３７２_０３６_８５４_７７５_８０８"] {
            assert!(matches!(bindable_minimum(minimum), Err(Error::Sql(rusqlite::Error::ToSqlConversionFailure(e)))
                if e.to_string() == "整数超出数据库支持的范围"));
        }
    }
}
