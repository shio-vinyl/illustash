//! 浏览读取保持 Store 的 SQL、字段覆盖顺序和只在本次请求内有效的快照。
use crate::character_entities::ENTITIES;
use crate::{
    http::LibraryContext,
    http_json::Json,
    http_read::{display, flag, python_int, rows, strings, trim, typed_rows},
    sql_fragments::{
        first_leads, hybrid, leads, learnable, proto_hints, rating, unasked, unclaimed, unfolded, visible, HINTS,
        NSFW_RATINGS,
    },
    Error, Result,
};
use illustash_core::{error::invalid, legacy_labels::page_asset_tags, schema::FAVORITE_KEY};
use rusqlite::{types::Value, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
/// 重复图 `d` 所在的组里，各份副本所属的身份不完全相同。
const MIXED: &str = "EXISTS(
  SELECT 1
  FROM duplicates y
  WHERE y.grp=d.grp AND y.uuid<>d.uuid
    AND (EXISTS(SELECT entity FROM memberships WHERE uuid=d.uuid
                EXCEPT SELECT entity FROM memberships WHERE uuid=y.uuid)
      OR EXISTS(SELECT entity FROM memberships WHERE uuid=y.uuid
                EXCEPT SELECT entity FROM memberships WHERE uuid=d.uuid)))";
/// 图片 `a` 在未匹配池里：没有身份，没有标签身份，也没有被暂缓。
fn unmatched(a: &str) -> String {
    format!(
        "NOT EXISTS(SELECT 1 FROM memberships m WHERE m.uuid={a}.uuid) \
         AND NOT EXISTS(SELECT 1 FROM tag_memberships t WHERE t.uuid={a}.uuid) \
         AND NOT EXISTS(SELECT 1 FROM deferred d WHERE d.uuid={a}.uuid)"
    )
}
/// 图的归属行和身份（名字、作品文字），按图的条件 `uuid`（如 `= ?`）。
fn members(uuid: &str) -> String {
    format!(
        "SELECT m.*,e.id,entity_name(e.id) AS name,e.work
         FROM memberships m
         JOIN {ENTITIES} e ON e.id=m.entity
         WHERE m.uuid {uuid}
         ORDER BY m.status='machine',name,e.id"
    )
}
fn audit_open(a: &str) -> String {
    format!(
        "EXISTS(SELECT 1 FROM audit_sample u WHERE u.uuid={a}.uuid)
         AND NOT EXISTS(
           SELECT 1
           FROM asked k
           JOIN audit_sample u ON u.uuid=k.uuid AND u.entity=k.entity
           WHERE k.uuid={a}.uuid AND k.answer<>'later')
         AND NOT EXISTS(
           SELECT 1
           FROM audit_sample u
           JOIN assertions s ON s.uuid=u.uuid
           JOIN {ENTITIES} e ON e.id=s.entity
           WHERE u.uuid={a}.uuid AND (coalesce(e.parent_id,e.id)=u.entity OR (s.status='confirmed' AND u.solo=1)))"
    )
}
pub(crate) fn day_bounds(day: &str) -> Result<(String, String)> {
    let bad = || Error::Invalid("日期不正确".into());
    let parts: Vec<_> = day.split('-').collect();
    if parts.len() != 3 {
        return Err(bad());
    }
    let decimal = |s: &str| -> Option<i32> {
        s.chars().try_fold(0i32, |n, c| crate::compat_unicode::decimal(c).map(|d| n * 10 + i32::from(d)))
    };
    if parts[0].chars().count() != 4 || parts[1].len() > 2 || parts[2].len() > 2 {
        return Err(bad());
    }
    if !parts[1].bytes().all(|c| c.is_ascii_digit())
        || !parts[2].strip_prefix(' ').unwrap_or(parts[2]).bytes().all(|c| c.is_ascii_digit())
    {
        return Err(bad());
    }
    let y = decimal(parts[0]).ok_or_else(bad)?;
    let m = decimal(parts[1]).ok_or_else(bad)?;
    let d = decimal(parts[2].strip_prefix(' ').unwrap_or(parts[2])).ok_or_else(bad)?;
    let leap = |y: i32| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let size = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap(y) {
                29
            } else {
                28
            }
        }
        _ => return Err(bad()),
    };
    if y == 0 || d < 1 || d > size {
        return Err(bad());
    }
    // 在旧实现 day_bounds 调用 mktime/localtime 的同一位置使用宿主系统的日历转换，包括它能表示的年份范围。
    // SAFETY: libc::tm 只含整数（Unix 上另有可为空的时区名指针），全零是合法值。
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = y - 1900;
    tm.tm_mon = m - 1;
    tm.tm_mday = d + 1;
    tm.tm_hour = 12;
    tm.tm_isdst = -1;
    let stamp = crate::platform_time::local_timestamp(&mut tm);
    if stamp == -1 {
        return Err(Error::Io(std::io::Error::other("mktime argument out of range")));
    }
    Ok((day.to_owned(), format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)))
}
/// `added_at` 直接存本地日期，与按天视图一样，读取时不再转换时区。
fn month_bounds(month: &str) -> Result<(String, String)> {
    let Some((year, number)) = month.split_once('-') else {
        return invalid("月份不正确");
    };
    if year.len() != 4 || number.len() != 2 || !year.bytes().chain(number.bytes()).all(|c| c.is_ascii_digit()) {
        return invalid("月份不正确");
    }
    let year: u32 = year.parse().map_err(|_| Error::invalid("月份不正确"))?;
    let number: u32 = number.parse().map_err(|_| Error::invalid("月份不正确"))?;
    if year == 0 || !(1..=12).contains(&number) {
        return invalid("月份不正确");
    }
    let end = if number == 12 && year < 9999 {
        format!("{:04}-01-01", year + 1)
    } else {
        // 9999-12 使用排序在所有合法月份之后的上界。
        format!("{year:04}-{:02}-01", number + 1)
    };
    Ok((format!("{month}-01"), end))
}
fn value<'a>(q: &'a HashMap<String, String>, k: &str) -> &'a str {
    q.get(k).map(String::as_str).unwrap_or("")
}
fn null() -> Json {
    None::<String>.into()
}
fn get(v: &Json, k: &str) -> Json {
    v.get(k).cloned().unwrap_or_else(null)
}
fn count(c: &Connection, sql: &str, args: &[&str]) -> Result<i64> {
    Ok(c.query_row(sql, rusqlite::params_from_iter(args), |r| r.get(0))?)
}
fn exists(c: &Connection, sql: &str, args: &[&str]) -> Result<bool> {
    Ok(count(c, &format!("SELECT EXISTS({sql})"), args)? != 0)
}
/// 参数转换在打开任何数据库之前进行，与旧实现的分派器相同。
fn preflight(path: &str, q: &HashMap<String, String>) -> Result<()> {
    let keys: Vec<(&str, &str)> = match path {
        "/api/assets" | "/api/ids" => vec![("offset", "0"), ("limit", "100")],
        "/api/home" => vec![("days", "5")],
        _ => return Ok(()),
    };
    let mut parsed = Vec::new();
    for (k, d) in keys {
        parsed.push(python_int(q.get(k).map(String::as_str).unwrap_or(d)).map_err(Error::Invalid)?);
    }
    if path == "/api/assets" && (!(0..=10000000).contains(&parsed[0]) || !(1..=200).contains(&parsed[1])) {
        return Err(Error::Invalid("分页参数不正确".into()));
    }
    Ok(())
}
fn attach_provenance(c: &Connection, context: &LibraryContext) -> Result<()> {
    if let Some(library) = &context.library {
        let path = library.join("provenance.sqlite");
        if path.exists() && !attached(c, "prov")? {
            c.execute("ATTACH DATABASE ? AS prov", [crate::http_read::sqlite_uri(&path)])?;
        }
    }
    Ok(())
}
// 数字判定沿用旧实现的 isdigit：包括 Unicode 15 中的 Digit 字符（不只是 Decimal）。
fn digit_extra(c: char) -> bool {
    const R: &[(u32, u32)] = &[
        (0xb2, 0xb3),
        (0xb9, 0xb9),
        (0x1369, 0x1371),
        (0x19da, 0x19da),
        (0x2070, 0x2070),
        (0x2074, 0x2079),
        (0x2080, 0x2089),
        (0x2460, 0x2468),
        (0x2474, 0x247c),
        (0x2488, 0x2490),
        (0x24ea, 0x24ea),
        (0x24f5, 0x24fd),
        (0x24ff, 0x24ff),
        (0x2776, 0x277e),
        (0x2780, 0x2788),
        (0x278a, 0x2792),
        (0x10a40, 0x10a43),
        (0x10e60, 0x10e68),
        (0x11052, 0x1105a),
        (0x1f100, 0x1f10a),
    ];
    R.iter().any(|&(a, b)| (a..=b).contains(&(c as u32)))
}
/// /api/assets（以及作用于浏览选区的命令）接受的筛选参数。
const FILTERS: [&str; 21] = [
    "entity", "variant", "status", "q", "tag", "utag", "pool", "order", "nsfw", "dup", "reason", "cluster", "work",
    "day", "artist", "has", "not", "from", "to", "like", "like_by",
];
fn browse_filters(q: &HashMap<String, String>) -> HashMap<String, String> {
    q.iter().filter(|(key, _)| FILTERS.contains(&key.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect()
}

const FAVORITE_TERM: &str = "(SELECT id FROM terms WHERE key=? AND kind='mark')";

/// 解析、限长和去重用于浏览与持久化，保留条件的输入顺序。
pub(crate) fn condition_lists(f: &HashMap<String, String>) -> Result<[Vec<String>; 2]> {
    let mut lists = [Vec::new(), Vec::new()];
    let mut count = 0;
    for (key, list) in ["has", "not"].into_iter().zip(&mut lists) {
        let Some(raw) = f.get(key) else { continue };
        let conditions: Vec<String> = serde_json::from_str(raw).map_err(|_| Error::invalid("筛选参数不正确"))?;
        count += conditions.len();
        if count > 20 || conditions.iter().any(|s| s.is_empty() || s.chars().count() > 160) {
            return invalid("筛选参数不正确");
        }
        let mut seen = HashSet::new();
        list.extend(conditions.into_iter().filter(|s| seen.insert(s.clone())));
    }
    Ok(lists)
}

// 只读附件每次连接挂一次；一个列表请求可在同一快照里计算多个画廊封面。
fn attached(c: &Connection, db: &str) -> Result<bool> {
    exists(c, "SELECT 1 FROM pragma_database_list WHERE name=?", &[db])
}

fn condition_filter(
    c: &Connection,
    context: &LibraryContext,
    f: &HashMap<String, String>,
    filter: &mut Filter,
) -> Result<()> {
    let [has, not] = condition_lists(f)?;
    let mut tags = Vec::new();
    let mut blocked_tags = Vec::new();
    for (negated, conditions, tags) in [(false, &has, &mut tags), (true, &not, &mut blocked_tags)] {
        for condition in conditions {
            let mut clause = Filter { clauses: Vec::new(), args: Vec::new(), numeric_index: None };
            if condition == "fav" {
                clause.add(format!(
                    "a.uuid IN (
                       WITH favorites AS MATERIALIZED (
                         SELECT x.key FROM term_projection p JOIN term_assets x ON x.id=p.asset_id
                         WHERE p.term_id={FAVORITE_TERM} AND p.state='confirmed' AND x.namespace='library')
                       SELECT key FROM favorites
                       UNION ALL
                       SELECT y.uuid FROM favorites f JOIN duplicates d ON d.uuid=f.key JOIN duplicates y ON y.grp=d.grp)"
                ), [FAVORITE_KEY.to_owned()]);
            } else {
                let Some((kind, key)) = condition.split_once(':').filter(|(_, key)| !key.is_empty()) else {
                    return invalid("筛选参数不正确");
                };
                match kind {
                    "char" => {
                        identity_filter(c, &HashMap::from([("entity".into(), key.into())]), &mut clause)?;
                    }
                    "work" => {
                        let ids = crate::http_read::work_tree(c, key)?;
                        let marks = vec!["?"; ids.len()].join(",");
                        clause.add(
                            format!(
                                "a.uuid IN (SELECT m.uuid FROM memberships m JOIN {ENTITIES} e
                                         ON e.id=m.entity WHERE e.work_id IN ({marks}))"
                            ),
                            ids,
                        );
                    }
                    "artist" => artist_filter(c, context, key, &mut clause)?,
                    "tag" => {
                        if !crate::http_read::tag_vocabulary(context)?.iter().any(|tag| tag == key) {
                            return invalid("请选择已有的通用标签");
                        }
                        tags.push(key.to_owned());
                        continue;
                    }
                    "utag" => clause.add("a.uuid IN (SELECT uuid FROM asset_tags WHERE tag=?)", [key.into()]),
                    "source" => source_filter(c, context, key, &mut clause)?,
                    "gallery" => {
                        let id = key
                            .parse::<i64>()
                            .ok()
                            .filter(|n| *n > 0 && key.bytes().all(|b| b.is_ascii_digit()))
                            .ok_or_else(|| Error::invalid("画廊编号不正确"))?
                            .to_string();
                        if !exists(c, "SELECT 1 FROM galleries WHERE id=? AND kind='manual'", &[&id])? {
                            return invalid("手选画廊不存在");
                        }
                        clause.add(
                            "a.uuid IN (
                               WITH members AS MATERIALIZED (
                                 SELECT m.uuid FROM gallery_members m JOIN assets x ON x.uuid=m.uuid
                                 WHERE m.gallery_id=?)
                               SELECT uuid FROM members
                               UNION ALL
                               SELECT y.uuid FROM members m JOIN duplicates d ON d.uuid=m.uuid
                               JOIN duplicates y ON y.grp=d.grp)",
                            [id],
                        );
                    }
                    _ => return invalid("筛选参数不正确"),
                }
            }
            let sql = clause.clauses.join(" AND ");
            filter.add(if negated { format!("NOT ({sql})") } else { sql }, clause.args);
        }
    }
    for (op, tags) in [("IN", tags), ("NOT IN", blocked_tags)] {
        if !tags.is_empty() {
            let marks = vec!["?"; tags.len()].join(",");
            let every = if op == "IN" {
                format!(" GROUP BY p.asset_id HAVING count(DISTINCT l.key)={}", tags.len())
            } else {
                String::new()
            };
            filter.add(
                format!(
                    "a.uuid {op} (
                   SELECT x.key FROM term_links l
                   JOIN terms t ON t.id=l.term_id AND t.kind='topic'
                   JOIN term_projection p ON p.term_id=l.term_id AND p.state IN ('confirmed','machine')
                   JOIN term_assets x ON x.id=p.asset_id AND x.namespace='library'
                   WHERE l.system='wd' AND l.key IN ({marks}){every})"
                ),
                tags,
            );
        }
    }
    for (key, lower) in [("from", true), ("to", false)] {
        let Some(day) = f.get(key) else { continue };
        // 组合条件用固定宽度日期，才能直接比较两个输入的先后。
        if !day.as_bytes().iter().enumerate().all(
            |(i, ch)| {
                if i == 4 || i == 7 {
                    *ch == b'-'
                } else {
                    ch.is_ascii_digit()
                }
            },
        ) || day.len() != 10
        {
            return invalid("日期不正确");
        }
        let (lo, hi) = day_bounds(day)?;
        if lower {
            filter.add("a.added_at>=?", [lo]);
        } else {
            filter.add("a.added_at<?", [hi]);
        }
    }
    if matches!((f.get("from"), f.get("to")), (Some(from), Some(to)) if from > to) {
        return invalid("日期不正确");
    }
    Ok(())
}

fn source_filter(c: &Connection, context: &LibraryContext, key: &str, filter: &mut Filter) -> Result<()> {
    if !matches!(key, "pixiv" | "x" | "other") {
        return invalid("筛选参数不正确");
    }
    let Some(path) = &context.source else {
        filter.add(if key == "other" { "1" } else { "0" }, []);
        return Ok(());
    };
    if !attached(c, "src")? {
        c.execute("ATTACH DATABASE ? AS src", [crate::http_read::sqlite_uri(path)])?;
    }
    if key == "other" {
        filter
            .add("a.uuid NOT IN (SELECT uuid FROM src.assets WHERE preset IN ('pixiv','x') AND uuid IS NOT NULL)", []);
    } else {
        filter.add("a.uuid IN (SELECT uuid FROM src.assets WHERE preset=?)", [key.into()]);
    }
    Ok(())
}

fn artist_filter(c: &Connection, context: &LibraryContext, key: &str, filter: &mut Filter) -> Result<()> {
    if context.library.is_none() {
        return invalid("这个图库还没有画师数据");
    }
    if !flag(c, "terms_artists")? {
        attach_provenance(c, context)?;
    }
    let assets = crate::http_artists::assets_of(c, true, key)?;
    let assets = Json::Array(assets.into_iter().map(Into::into).collect()).dump();
    filter.add("a.uuid IN (SELECT value FROM json_each(?))", [assets]);
    Ok(())
}
fn marks_favorite(q: &HashMap<String, String>) -> bool {
    value(q, "marks").split(',').any(|mark| mark == "fav")
}

/// 展开当前页的副本再查裁决。标记显示只需查这些图，不必读全库收藏。
fn decorate_favorite(c: &Connection, items: &mut [Json]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let page = Json::Array(items.iter().map(|item| get(item, "uuid")).collect()).dump();
    let sql = format!(
        "WITH page AS MATERIALIZED (SELECT value AS uuid FROM json_each(?)),
         copies(uuid, member) AS (
           SELECT uuid,uuid FROM page
           UNION ALL
           SELECT p.uuid,y.uuid FROM page p JOIN duplicates d ON d.uuid=p.uuid
           JOIN duplicates y ON y.grp=d.grp AND y.uuid<>p.uuid)
         SELECT DISTINCT y.uuid FROM copies y
         CROSS JOIN term_assets x ON x.namespace='library' AND x.key=y.member
         CROSS JOIN term_projection p ON p.asset_id=x.id AND p.term_id={FAVORITE_TERM}
         WHERE p.state='confirmed'"
    );
    let favorites: HashSet<_> = strings(c, &sql, &[&page, FAVORITE_KEY])?.into_iter().collect();
    for item in items {
        item.set("fav", favorites.contains(get(item, "uuid").text()).into());
    }
    Ok(())
}
/// 正在构建的浏览选区：针对图片 `a` 的 SQL 子句及其参数，其中可能有一个是聚类或相似组的 id，按整数绑定。
struct Filter {
    clauses: Vec<String>,
    args: Vec<String>,
    numeric_index: Option<usize>,
}
impl Filter {
    fn add(&mut self, clause: impl Into<String>, args: impl IntoIterator<Item = String>) {
        self.clauses.push(clause.into());
        self.args.extend(args);
    }
}
/// 选区针对资产 `a` 的 SQL 条件、参数和排序。各项检查按旧实现 Store 原来的顺序拒绝。
pub(crate) fn selection(
    c: &Connection,
    context: &LibraryContext,
    f: &HashMap<String, String>,
) -> Result<(String, Vec<Value>, String)> {
    if f.keys().any(|k| !FILTERS.contains(&k.as_str())) {
        return Err(Error::Invalid("筛选参数不正确".into()));
    }
    let v = |k| value(f, k);
    let mut filter = Filter { clauses: vec!["1".to_owned()], args: Vec::new(), numeric_index: None };
    if !v("day").is_empty() {
        let (lo, hi) = day_bounds(v("day"))?;
        filter.add("a.added_at>=? AND a.added_at<?", [lo, hi]);
    }
    if !v("artist").is_empty() {
        artist_filter(c, context, v("artist"), &mut filter)?;
    }
    if !v("work").is_empty() {
        let ids = crate::http_read::work_tree(c, v("work"))?;
        let clause = format!(
            "EXISTS(
               SELECT 1
               FROM memberships m
               JOIN {ENTITIES} e ON e.id=m.entity
               WHERE m.uuid=a.uuid AND e.work_id IN ({marks}))",
            marks = vec!["?"; ids.len()].join(",")
        );
        filter.add(clause, ids);
    }
    let entity = identity_filter(c, f, &mut filter)?;
    let pool = v("pool");
    pool_filter(pool, v("cluster"), &mut filter)?;
    filter.clauses.push(match pool {
        "duplicates" => "NOT EXISTS(SELECT 1 FROM excluded x WHERE x.uuid=a.uuid)".into(),
        "excluded" => unfolded("a"),
        _ => visible("a"),
    });
    if !v("reason").is_empty() {
        if pool != "excluded" || !["photo", "screenshot", "text", "manual"].contains(&v("reason")) {
            return Err(Error::Invalid("未知排除原因".into()));
        }
        filter.add("EXISTS(SELECT 1 FROM excluded x WHERE x.uuid=a.uuid AND x.reason=?)", [v("reason").into()]);
    }
    let dup = v("dup");
    if dup == "mixed" {
        filter.add(format!("EXISTS(SELECT 1 FROM duplicates d WHERE d.uuid=a.uuid AND {MIXED})"), []);
    } else if !dup.is_empty() {
        return Err(Error::Invalid("未知重复筛选".into()));
    }
    let status = f.get("status").map(String::as_str).unwrap_or("all");
    status_filter(status, &entity, &mut filter)?;
    search_filter(c, v("q"), &mut filter)?;
    label_filter(c, context, v("tag"), v("nsfw"), &mut filter)?;
    if !v("utag").is_empty() {
        filter.add("a.uuid IN (SELECT uuid FROM asset_tags WHERE tag=?)", [v("utag").into()]);
    }
    condition_filter(c, context, f, &mut filter)?;
    let like = crate::http_example_search::Search::parse(f)?;
    let order = f.get("order").map(String::as_str).unwrap_or(if like.is_some() { "similar" } else { "newest" });
    let ordering = if order == "color" {
        crate::app_colors::ordering(c, context)
    } else if like.is_some() {
        match order {
            "similar" => "(SELECT rank FROM temp.like_rank WHERE uuid=a.uuid)".into(),
            "oldest" => "a.added_at ASC,a.uuid".into(),
            _ => "a.added_at DESC,a.uuid".into(),
        }
    } else {
        ordering(order, pool, dup, status, &entity)?
    };
    let args = bound(filter.args, filter.numeric_index, v("cluster"))?;
    let conditions = filter.clauses.join(" AND ");
    if let Some(like) = like {
        like.rank(c, context, &conditions, &args)?;
        // 本事务的全部条件已经应用于临时表，不再执行一次分级或搜索子查询。
        return Ok(("a.uuid IN (SELECT uuid FROM temp.like_rank)".into(), Vec::new(), ordering));
    }
    Ok((conditions, args, ordering))
}
/// 角色（及皮肤）筛选：已合并的角色沿合并关系找到它被并入的角色；皮肤则选中它所属的角色，并以该皮肤作为
/// variant。返回选中的角色（如果有）。
fn identity_filter(c: &Connection, f: &HashMap<String, String>, filter: &mut Filter) -> Result<String> {
    let mut entity = value(f, "entity").to_owned();
    let mut variant = value(f, "variant").to_owned();
    let parent_of = |id: &str| -> Result<Option<Option<String>>> {
        let sql = format!("SELECT parent_id FROM {ENTITIES} WHERE id=?");
        Ok(c.query_row(&sql, [id], |r| r.get(0)).optional()?)
    };
    if !entity.is_empty() {
        for _ in 0..16 {
            let next: Option<Option<String>> = c
                .query_row(&format!("SELECT merged_into FROM {ENTITIES} WHERE id=?"), [&entity], |r| r.get(0))
                .optional()?;
            match next {
                Some(Some(n)) if !n.is_empty() => entity = n,
                _ => break,
            }
        }
        match parent_of(&entity)? {
            None => return Err(Error::Invalid("角色不存在".into())),
            Some(Some(p)) if !p.is_empty() => {
                variant = entity;
                entity = p
            }
            _ => {}
        }
        if value(f, "status") == "maybe" {
            let maybe = format!(
                "EXISTS(SELECT 1 FROM {HINTS} v WHERE v.uuid=a.uuid AND v.entity=?)
                 AND NOT EXISTS(SELECT 1 FROM memberships m WHERE m.uuid=a.uuid AND m.entity=?)
                 AND NOT EXISTS(SELECT 1 FROM assertions s WHERE s.uuid=a.uuid AND s.entity=? AND s.status='rejected')"
            );
            filter.add(maybe, vec![entity.clone(); 3]);
        } else {
            let mascot = exists(c, &format!("SELECT 1 FROM {ENTITIES} WHERE id=? AND mascot<>0"), &[&entity])?;
            let members = if mascot { "tag_memberships" } else { "memberships" };
            filter
                .add(format!("EXISTS(SELECT 1 FROM {members} m WHERE m.uuid=a.uuid AND m.entity=?)"), [entity.clone()]);
        }
    }
    if !variant.is_empty() {
        let p = parent_of(&variant)?;
        if !p.flatten().is_some_and(|p| !p.is_empty() && (entity.is_empty() || p == entity)) {
            return Err(Error::Invalid("子标签不属于该角色".into()));
        }
        filter.add("EXISTS(SELECT 1 FROM variant_memberships v WHERE v.uuid=a.uuid AND v.entity=?)", [variant]);
    }
    Ok(entity)
}
/// 图片池（`pool`）；聚类池和相似池另用组 id `cluster`。
fn pool_filter(pool: &str, cluster: &str, filter: &mut Filter) -> Result<()> {
    match pool {
        "unmatched" => filter.add(unmatched("a"), []),
        "skipped" => filter.add("EXISTS(SELECT 1 FROM deferred d WHERE d.uuid=a.uuid)", []),
        "duplicates" => filter.add("EXISTS(SELECT 1 FROM duplicates d WHERE d.uuid=a.uuid)", []),
        "excluded" => filter.add("EXISTS(SELECT 1 FROM excluded x WHERE x.uuid=a.uuid)", []),
        "cluster" | "similar" => {
            if cluster.is_empty()
                || !cluster.chars().all(|c| crate::compat_unicode::decimal(c).is_some() || digit_extra(c))
            {
                return Err(Error::Invalid(if pool == "cluster" { "请指定分组" } else { "请指定相似组" }.into()));
            }
            let n = python_int(cluster).map_err(Error::Invalid)?;
            filter.numeric_index = Some(filter.args.len());
            let clause = if pool == "cluster" {
                format!("EXISTS(SELECT 1 FROM clusters k WHERE k.uuid=a.uuid AND k.cid=?) AND {}", unclaimed("a"))
            } else {
                "EXISTS(SELECT 1 FROM similar_groups g WHERE g.uuid=a.uuid AND g.gid=?)".into()
            };
            filter.add(clause, [n.to_string()]);
        }
        "recent" => filter.add(
            "EXISTS(SELECT 1 FROM sync_added s WHERE s.uuid=a.uuid AND s.run_id=(SELECT max(run_id) FROM sync_added))",
            [],
        ),
        "audit" => filter.add("EXISTS(SELECT 1 FROM audit_sample u WHERE u.uuid=a.uuid)", []),
        "pending" => filter.add(
            "EXISTS(SELECT 1 FROM auto_rules r WHERE r.uuid=a.uuid AND r.kind='pending')
             AND NOT EXISTS(SELECT 1 FROM curation k WHERE k.uuid=a.uuid)",
            [],
        ),
        "" => {}
        _ => return Err(Error::Invalid("未知图片池".into())),
    }
    Ok(())
}
/// 成员关系状态筛选（`status`）；选了角色时只针对该角色。
fn status_filter(status: &str, entity: &str, filter: &mut Filter) -> Result<()> {
    let of_entity = (!entity.is_empty()).then(|| entity.to_owned());
    match status {
        "maybe" if entity.is_empty() => return Err(Error::Invalid("「可能也是」需要指定角色".into())),
        "maybe" | "all" => {}
        "confirmed" | "machine" => {
            let only = if entity.is_empty() { "" } else { " AND m.entity=?" };
            let clause = format!("EXISTS(SELECT 1 FROM memberships m WHERE m.uuid=a.uuid AND m.status=?{only})");
            filter.add(clause, std::iter::once(status.to_owned()).chain(of_entity));
        }
        "ambiguous" => {
            let only = if entity.is_empty() { "" } else { " AND x.entity=?" };
            filter.add(format!("EXISTS(SELECT 1 FROM ambiguous_claims x WHERE x.uuid=a.uuid{only})"), of_entity);
        }
        "unknown" => filter.add("EXISTS(SELECT 1 FROM deferred d WHERE d.uuid=a.uuid)", []),
        _ => return Err(Error::Invalid("未知筛选状态".into())),
    }
    Ok(())
}
/// 搜索框（`q`）：匹配文件名，以及名字匹配的身份的图片。
fn search_filter(c: &Connection, q: &str, filter: &mut Filter) -> Result<()> {
    let query = trim(q).chars().take(160).collect::<String>();
    if query.is_empty() {
        return Ok(());
    }
    let hits = crate::names_search::hits(c, &query)?;
    let mut search = "a.filename LIKE ? ESCAPE '\\'".to_owned();
    let mut args = vec![crate::http_read::like(&query)];
    if !hits.is_empty() {
        let marks = vec!["?"; hits.len()].join(",");
        search += &format!(" OR a.uuid IN (SELECT uuid FROM memberships WHERE entity IN ({marks}))");
        args.extend(hits.into_keys());
    }
    filter.add(format!("({search})"), args);
    Ok(())
}
/// 普通标签（`tag`）和内容（`nsfw`）筛选。标签在核心里时，「只看 NSFW 图里的某个标签」是对词条投影的一次查询。
fn label_filter(c: &Connection, context: &LibraryContext, tag: &str, nsfw: &str, filter: &mut Filter) -> Result<()> {
    let combined = !tag.is_empty() && nsfw == "only" && flag(c, "terms_labels")?;
    if !tag.is_empty() {
        if !crate::http_read::tag_vocabulary(context)?.iter().any(|t| t == tag) {
            return Err(Error::Invalid("请选择已有的通用标签".into()));
        }
        let clause = if combined {
            format!(
                "a.uuid IN (
                   SELECT ta.key
                   FROM term_projection rated
                   JOIN term_assets ta ON ta.id=rated.asset_id AND ta.namespace='library'
                   WHERE rated.state IN ('machine','confirmed')
                     AND rated.term_id IN (
                       SELECT t.id
                       FROM terms t
                       JOIN term_links l ON l.term_id=t.id AND l.system='wd_rating'
                       WHERE t.kind='rating' AND l.key IN ({NSFW_RATINGS}))
                     AND EXISTS(
                       SELECT 1
                       FROM term_projection tagged
                       WHERE tagged.asset_id=rated.asset_id AND tagged.state IN ('machine','confirmed')
                         AND tagged.term_id IN (
                           SELECT t.id
                           FROM terms t
                           JOIN term_links l ON l.term_id=t.id AND l.system='wd'
                           WHERE t.kind='topic' AND l.key=?)))"
            )
        } else {
            "a.uuid IN (SELECT uuid FROM general_hits WHERE value=?)".into()
        };
        filter.add(clause, [tag.into()]);
    }
    if ["hide", "only"].contains(&nsfw) && !combined {
        let hit = crate::sql_fragments::nsfw("a");
        filter.add(if nsfw == "hide" { format!("NOT coalesce({hit},0)") } else { hit }, []);
    } else if !nsfw.is_empty() && !["hide", "only"].contains(&nsfw) {
        return Err(Error::Invalid("未知内容筛选".into()));
    }
    Ok(())
}
/// 选区的 ORDER BY（`order`）；每种排序最后都按主键排。
fn ordering(order: &str, pool: &str, dup: &str, status: &str, entity: &str) -> Result<String> {
    if !["newest", "oldest", "suspicious", "hinted"].contains(&order) {
        return Err(Error::Invalid("未知排序".into()));
    }
    let direction = if order == "oldest" { "ASC" } else { "DESC" };
    let quoted = entity.replace('\'', "''");
    Ok(if status == "maybe" && order == "suspicious" {
        format!(
            "(SELECT min(v.weak)*10-max(v.score) FROM {HINTS} v WHERE v.uuid=a.uuid AND v.entity='{quoted}'),a.uuid"
        )
    } else if order == "hinted" {
        format!(
            "coalesce((SELECT min(v.weak)*10-max(v.score) FROM {} v WHERE v.uuid=a.uuid),99),a.added_at DESC,a.uuid",
            leads()
        )
    } else if order == "suspicious" {
        let only = if entity.is_empty() { String::new() } else { format!(" AND m.entity='{quoted}'") };
        format!("(SELECT min(m.skey) FROM memberships m WHERE m.uuid=a.uuid{only}),a.uuid")
    } else if pool == "duplicates" || !dup.is_empty() {
        format!(
            "(SELECT d.grp_added FROM duplicates d WHERE d.uuid=a.uuid) {direction},\
             (SELECT d.grp FROM duplicates d WHERE d.uuid=a.uuid),\
             (SELECT d.pos FROM duplicates d WHERE d.uuid=a.uuid),a.uuid"
        )
    } else {
        format!("a.added_at {direction},a.uuid")
    })
}
/// 把参数转成 SQL 值：组 id 为整数，其余为文本。
fn bound(args: Vec<String>, numeric_index: Option<usize>, cluster: &str) -> Result<Vec<Value>> {
    if numeric_index.is_some() {
        let digits = cluster
            .chars()
            .map(|c| (b'0' + crate::compat_unicode::decimal(c).expect("http_browse internal invariant")) as char)
            .collect::<String>();
        let digits = digits.trim_start_matches('0');
        if digits.len() > 19 || (digits.len() == 19 && digits > "9223372036854775807") {
            // 所有筛选校验都在「整数超出 SQLite INTEGER 范围」的绑定错误之前完成。
            return Err(Error::Io(std::io::Error::other("整数超出数据库支持的范围")));
        }
    }
    Ok(args
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            if Some(i) == numeric_index {
                Value::Integer(v.parse().expect("http_browse internal invariant"))
            } else {
                Value::Text(v)
            }
        })
        .collect())
}
fn evidence(v: &Json, fields: &[&str]) -> Json {
    let mut sources =
        get(v, "sources").text().split(',').filter(|s| !s.is_empty()).map(String::from).collect::<Vec<_>>();
    sources.sort();
    let mut e = Json::object([
        ("sources", Json::Array(sources.into_iter().map(Into::into).collect())),
        ("max_score", get(v, "score")),
    ]);
    for k in fields {
        e.set(*k, get(v, k));
    }
    e
}
fn shown(c: &Connection, v: Json, skin: bool) -> Result<Json> {
    crate::http_read::display_skin(c, v, skin)
}
/// 图上现有角色的显示名，顺序同详情栏（导出时按角色建子文件夹用，#450）。
pub(crate) fn character_names(c: &Connection, id: &str) -> Result<Vec<String>> {
    Ok(rows(c, &members("= ?"), &[id])?.into_iter().map(|r| get(&r, "name").text().to_owned()).collect())
}
fn memberships(c: &Connection, id: &str) -> Result<Vec<Json>> {
    let mut out = Vec::new();
    for r in rows(c, &members("= ?"), &[id])? {
        let entity = get(&r, "id");
        let mut item = display(c, r.clone())?;
        item.set("evidence", evidence(&r, &[]));
        let variants = rows(
            c,
            "SELECT entity AS id,variant AS name,status
             FROM variant_memberships
             WHERE uuid=? AND parent_id=?
             ORDER BY variant",
            &[id, entity.text()],
        )?;
        item.set("variants", Json::Array(variants.into_iter().map(|v| shown(c, v, true)).collect::<Result<_>>()?));
        out.push(item);
    }
    Ok(out)
}
// 投影后的页面只需要本页的身份、作品名和 hybrid 节点。保持显示字段的覆盖顺序，但不为每张卡片准备好几条 SQL 查询。
struct PageDisplay {
    entities: HashMap<String, Json>,
    names: HashMap<(String, bool), Json>,
    nodes: HashMap<String, Json>,
}
impl PageDisplay {
    fn load(c: &Connection, ids: Vec<Json>) -> Result<Option<Self>> {
        if !flag(c, "terms_names")? {
            return Ok(None);
        }
        let mut name_ids = ids.clone();
        let ids = Json::Array(ids).dump();
        let mut entities = HashMap::new();
        let sql = format!(
            "SELECT e.id,e.kind,e.original,coalesce(p.work_id,e.work_id) AS work_id
             FROM {ENTITIES} e
             LEFT JOIN terms pt ON pt.key=e.parent_id AND e.kind='skin'
             LEFT JOIN term_tree p ON p.term_id=pt.id
             WHERE e.id IN (SELECT value FROM json_each(?))"
        );
        for r in rows(c, &sql, &[&ids])? {
            entities.insert(get(&r, "id").text().to_owned(), r);
        }
        name_ids.extend(entities.values().map(|r| get(r, "work_id")));
        let name_ids = Json::Array(name_ids).dump();
        let mut names = HashMap::new();
        let displayed = "
            SELECT entity,skin_only,name,name_source,zh,ja,en,tag
            FROM term_name_display
            WHERE entity IN (SELECT value FROM json_each(?))";
        for r in rows(c, displayed, &[&name_ids])? {
            let info = Json::object([
                ("name", get(&r, "name")),
                ("name_source", get(&r, "name_source")),
                ("names", Json::object([("zh", get(&r, "zh")), ("ja", get(&r, "ja")), ("en", get(&r, "en"))])),
                ("tag", get(&r, "tag")),
            ]);
            let key = (get(&r, "entity").text().to_owned(), get(&r, "skin_only").integer() != 0);
            names.insert(key, crate::http_read::shown_info(info));
        }
        let mut nodes = HashMap::new();
        if hybrid(c)? {
            let identities = "
                SELECT id,node_type,character_id,variant_id,canonical_id
                FROM identity_nodes
                WHERE id IN (SELECT value FROM json_each(?))";
            for mut r in rows(c, identities, &[&ids])? {
                let id = get(&r, "id").text().to_owned();
                if let Json::Object(fields) = &mut r {
                    fields.retain(|(k, _)| k != "id");
                }
                nodes.insert(id, r);
            }
        }
        Ok(Some(Self { entities, names, nodes }))
    }
    fn display(&self, mut v: Json, skin: bool) -> Json {
        let id = get(&v, "id").text().to_owned();
        if let Some(info) = self.names.get(&(id.clone(), skin)) {
            v.extend(info.clone());
        }
        let mut work = null();
        if let Some(entity) = self.entities.get(&id) {
            if v.get("kind").is_some() && get(entity, "kind").text() != "work" {
                v.set("kind", if get(entity, "original").integer() != 0 { "oc" } else { "" }.into());
            }
            work = get(entity, "work_id");
            v.set("work_id", work.clone());
        }
        if v.get("work").is_some() {
            let name = match &work {
                Json::Scalar(serde_json::Value::String(id)) => {
                    self.names.get(&(id.clone(), false)).map(|r| get(r, "name"))
                }
                _ => None,
            }
            .unwrap_or_else(|| "".into());
            v.set("work_name", name);
        }
        if let Some(node) = self.nodes.get(&id) {
            v.extend(node.clone());
        }
        v
    }
}
/// 显示一页上的身份：已投影时用本页批量取得的名字，否则逐个读取。
struct Renderer<'a> {
    c: &'a Connection,
    page: Option<PageDisplay>,
}
impl Renderer<'_> {
    fn display(&self, v: Json, skin: bool) -> Result<Json> {
        match &self.page {
            Some(page) => Ok(page.display(v, skin)),
            None => shown(self.c, v, skin),
        }
    }
}
/// 本页按图片索引的几张表：图片自己的标签、暂缓、重复组和排除。
struct PageExtras {
    tags: HashMap<String, Vec<Json>>,
    deferred: HashSet<String>,
    duplicates: HashMap<String, Json>,
    excluded: HashMap<String, Json>,
}
impl PageExtras {
    fn load(c: &Connection, ids: &str) -> Result<Self> {
        let selected = "IN (SELECT value FROM json_each(?))";
        let mut tags: HashMap<String, Vec<Json>> = HashMap::new();
        for (uuid, tag) in page_asset_tags(c, ids)? {
            tags.entry(uuid).or_default().push(tag.into());
        }
        let deferred =
            strings(c, &format!("SELECT uuid FROM deferred WHERE uuid {selected}"), &[ids])?.into_iter().collect();
        let mut duplicates = HashMap::new();
        for r in rows(c, &format!("SELECT uuid,grp,size,pos FROM duplicates WHERE uuid {selected}"), &[ids])? {
            duplicates.insert(
                get(&r, "uuid").text().to_owned(),
                Json::object([("grp", get(&r, "grp")), ("size", get(&r, "size")), ("pos", get(&r, "pos"))]),
            );
        }
        let mut excluded = HashMap::new();
        for r in rows(c, &format!("SELECT uuid,reason FROM excluded WHERE uuid {selected}"), &[ids])? {
            excluded.insert(get(&r, "uuid").text().to_owned(), get(&r, "reason"));
        }
        Ok(Self { tags, deferred, duplicates, excluded })
    }
}
/// 本页（`ids`，JSON 列表）每张未认领图片的首选线索。和组头计票、分组用同一个 `first_leads`，
/// 所以组页网格上每张图的线索和组头问的角色排序一致。
fn page_leads(c: &Connection, ids: &str) -> Result<HashMap<String, Json>> {
    let unclaimed = strings(
        c,
        "SELECT j.value FROM json_each(?) j WHERE NOT EXISTS(SELECT 1 FROM memberships m WHERE m.uuid=j.value)",
        &[ids],
    )?;
    let page = Json::Array(unclaimed.into_iter().map(Into::into).collect()).dump();
    Ok(first_leads(c, &page)?.into_iter().map(|(uuid, entity)| (uuid, entity.into())).collect())
}
/// 本页每张抽查图片被抽查的身份，以及它已有的回答。
fn page_audit(c: &Connection, ids: &str) -> Result<(HashMap<String, Json>, HashMap<String, Json>)> {
    let selected = "IN (SELECT value FROM json_each(?))";
    let mut audit = HashMap::new();
    for r in rows(c, &format!("SELECT uuid,entity FROM audit_sample WHERE uuid {selected}"), &[ids])? {
        audit.insert(get(&r, "uuid").text().to_owned(), get(&r, "entity"));
    }
    let mut answered = HashMap::new();
    let answers = format!(
        "SELECT u.uuid,coalesce(
           (SELECT k.answer FROM asked k WHERE k.uuid=u.uuid AND k.entity=u.entity AND k.answer<>'later'),
           (SELECT CASE s.status WHEN 'confirmed' THEN 'yes' ELSE 'no' END
            FROM assertions s
            JOIN {ENTITIES} e ON e.id=s.entity
            WHERE s.uuid=u.uuid AND coalesce(e.parent_id,e.id)=u.entity
            ORDER BY s.status='confirmed'
            LIMIT 1),
           (SELECT 'no' FROM assertions s WHERE s.uuid=u.uuid AND s.status='confirmed' AND u.solo=1 LIMIT 1)) AS answer
         FROM audit_sample u
         WHERE u.uuid {selected}"
    );
    for r in rows(c, &answers, &[ids])? {
        answered.insert(get(&r, "uuid").text().to_owned(), get(&r, "answer"));
    }
    Ok((audit, answered))
}
/// 每张图片的成员关系，整理成显示格式，并带上皮肤。
fn page_members(
    renderer: &Renderer,
    variant_rows: Vec<Json>,
    member_rows: Vec<Json>,
) -> Result<HashMap<String, Vec<Json>>> {
    let mut variants: HashMap<(String, String), Vec<Json>> = HashMap::new();
    for r in variant_rows {
        let item = renderer.display(
            Json::object([("id", get(&r, "id")), ("name", get(&r, "name")), ("status", get(&r, "status"))]),
            true,
        )?;
        variants.entry((get(&r, "uuid").text().into(), get(&r, "parent_id").text().into())).or_default().push(item);
    }
    let mut members: HashMap<String, Vec<Json>> = HashMap::new();
    for r in member_rows {
        let mut item = renderer.display(r.clone(), false)?;
        item.set("evidence", evidence(&r, &[]));
        let key = (get(&r, "uuid").text().into(), get(&r, "id").text().into());
        item.set("variants", Json::Array(variants.get(&key).cloned().unwrap_or_default()));
        members.entry(get(&r, "uuid").text().into()).or_default().push(item);
    }
    Ok(members)
}
/// 批量处理的一页，保持「取第一条最好线索」和同键后者覆盖前者的语义（沿用旧实现）。
fn decorate(c: &Connection, q: &HashMap<String, String>, page: &mut [Json]) -> Result<()> {
    if page.is_empty() {
        return Ok(());
    }
    let ids = Json::Array(page.iter().map(|v| get(v, "uuid")).collect()).dump();
    let selected = "IN (SELECT value FROM json_each(?))";
    let leads_by = page_leads(c, &ids)?;
    let (audit, answered) = page_audit(c, &ids)?;
    let variants = format!(
        "SELECT uuid,parent_id,entity AS id,variant AS name,status
         FROM variant_memberships
         WHERE uuid {selected}
         ORDER BY variant"
    );
    let variant_rows = rows(c, &variants, &[&ids])?;
    let member_rows = rows(c, &members(selected), &[&ids])?;
    let mut entity_ids = variant_rows.iter().chain(&member_rows).map(|r| get(r, "id")).collect::<Vec<_>>();
    entity_ids.extend(leads_by.values().chain(audit.values()).cloned());
    let renderer = Renderer { c, page: PageDisplay::load(c, entity_ids)? };
    let members = page_members(&renderer, variant_rows, member_rows)?;
    let extras = PageExtras::load(c, &ids)?;
    for row in page.iter_mut() {
        let id = get(row, "uuid").text().to_owned();
        let m = members.get(&id).cloned().unwrap_or_default();
        row.set("memberships", Json::Array(m.clone()));
        row.set("confirmed", Json::Array(m.into_iter().filter(|m| get(m, "status").text() == "confirmed").collect()));
        row.set("unknown", extras.deferred.contains(&id).into());
        row.set("user_tags", Json::Array(extras.tags.get(&id).cloned().unwrap_or_default()));
        row.set("dup", extras.duplicates.get(&id).cloned().unwrap_or_else(null));
        row.set("excluded", extras.excluded.get(&id).cloned().unwrap_or_else(null));
        for (key, map) in [("hint", &leads_by), ("audit", &audit)] {
            let mut info = null();
            if let Some(e) = map.get(&id) {
                let shown = renderer.display(Json::object([("id", e.clone())]), false)?;
                info = Json::object([("id", get(&shown, "id")), ("name", get(&shown, "name"))]);
                if key == "audit" {
                    info.set("answer", answered.get(&id).cloned().unwrap_or_else(null));
                }
            }
            row.set(key, info);
        }
    }
    if marks_favorite(q) {
        decorate_favorite(c, page)?;
    }
    Ok(())
}
pub fn validate(path: &str, q: &HashMap<String, String>) -> Result<()> {
    preflight(path, q)?;
    if path == "/api/ids" {
        let offset = python_int(q.get("offset").map(String::as_str).unwrap_or("0")).map_err(Error::Invalid)?;
        let limit = python_int(q.get("limit").map(String::as_str).unwrap_or("100")).map_err(Error::Invalid)?;
        if !(0..=10000000).contains(&offset) || !(1..=5000).contains(&limit) {
            return Err(Error::Invalid("分页参数不正确".into()));
        }
    }
    if path == "/api/home" {
        if !value(q, "before").is_empty() {
            day_bounds(value(q, "before"))?;
        }
        let n = python_int(q.get("days").map(String::as_str).unwrap_or("5")).map_err(Error::Invalid)?;
        if !(1..=20).contains(&n) {
            return Err(Error::Invalid("分页参数不正确".into()));
        }
        rating("a", value(q, "nsfw"))?;
    }
    if path == "/api/calendar" {
        month_bounds(value(q, "month"))?;
    }
    Ok(())
}
fn question(c: &Connection, id: &str) -> Result<Json> {
    if exists(c, "SELECT 1 FROM excluded WHERE uuid=?", &[id])? {
        return Ok(null());
    }
    let audit = strings(
        c,
        &format!(
            "SELECT u.entity FROM audit_sample u JOIN assets a ON a.uuid=u.uuid WHERE u.uuid=? AND {}",
            audit_open("a")
        ),
        &[id],
    )?
    .into_iter()
    .next();
    let mut ambiguous = None;
    let mut hint = None;
    if audit.is_none() {
        let closest = format!(
            "SELECT m.entity,m.rival
             FROM ambiguous_claims m
             WHERE m.uuid=? AND {} AND {}
             ORDER BY m.margin
             LIMIT 1",
            unasked("m"),
            learnable("m")
        );
        ambiguous = rows(c, &closest, &[id])?.into_iter().next();
        let hinted = format!("{} AND w.uuid=? ORDER BY w.score DESC LIMIT 1", proto_hints("w.entity"));
        hint = strings(c, &hinted, &[id])?.into_iter().next();
    }
    let confirmed = |e: &str| count(c, "SELECT count(*) FROM memberships WHERE entity=? AND status='confirmed'", &[e]);
    let (kind, entity, rival) = if let Some(e) = audit {
        ("audit", e, None)
    } else if let Some(e) = hint {
        ("hint", e, None)
    } else if let Some(r) = ambiguous {
        ("ambiguous", get(&r, "entity").text().to_owned(), Some(get(&r, "rival").text().to_owned()))
    } else {
        let mut found = None;
        let seeds = format!(
            "SELECT m.entity
             FROM memberships m
             JOIN single_assets o ON o.uuid=m.uuid
             WHERE m.uuid=? AND m.status='machine' AND {} AND {}",
            unasked("m"),
            learnable("m")
        );
        for e in strings(c, &seeds, &[id])? {
            if confirmed(&e)? < 5 {
                found = Some(e);
                break;
            }
        }
        let Some(e) = found else { return Ok(null()) };
        ("seed", e, None)
    };
    let name = |e: &str| -> Result<Json> {
        Ok(c.query_row(&format!("SELECT entity_name(e.id) FROM {ENTITIES} e WHERE id=?"), [e], |r| {
            r.get::<_, String>(0)
        })?
        .into())
    };
    let mut out = Json::object([
        ("kind", kind.into()),
        ("entity", entity.clone().into()),
        ("name", name(&entity)?),
        ("confirmed", confirmed(&entity)?.into()),
    ]);
    if let Some(rival) = rival.filter(|r| !r.is_empty()) {
        out.set("rival", rival.clone().into());
        out.set("rival_name", name(&rival)?);
    }
    if kind == "hint" {
        if let Some(current) =
            strings(c, "SELECT entity FROM memberships WHERE uuid=? AND entity<>? LIMIT 1", &[id, &entity])?
                .into_iter()
                .next()
        {
            out.set("current", current.clone().into());
            out.set("current_name", name(&current)?);
        }
    }
    Ok(out)
}
fn detail(c: &Connection, context: &LibraryContext, id: &str) -> Result<Json> {
    let Some(mut out) = rows(c, "SELECT uuid,filename,width,height,added_at,src_kind FROM assets WHERE uuid=?", &[id])?
        .into_iter()
        .next()
    else {
        return Err(Error::Invalid("图片不存在".into()));
    };
    let members = memberships(c, id)?;
    out.set("memberships", Json::Array(members.clone()));
    let tagged = format!(
        "SELECT e.id,entity_name(e.id) AS name,t.status
         FROM tag_memberships t
         JOIN {ENTITIES} e ON e.id=t.entity
         WHERE t.uuid=?
         ORDER BY name"
    );
    out.set("tag_identities", Json::Array(rows(c, &tagged, &[id])?));
    out.set("ownership", ownership(c, id, members)?);
    out.set("alternatives", alternatives(c, id)?);
    out.set("single", exists(c, "SELECT 1 FROM single_assets WHERE uuid=?", &[id])?.into());
    let assertions = format!(
        "SELECT e.id,entity_name(e.id) AS name,s.status,s.origin
         FROM assertions s
         JOIN {ENTITIES} e ON e.id=s.entity
         WHERE s.uuid=?
         ORDER BY name,e.id"
    );
    let candidates = format!(
        "SELECT e.id,entity_name(e.id) AS name,c.source,c.score
         FROM candidates c
         JOIN {ENTITIES} e ON e.id=c.entity
         WHERE c.uuid=?
         ORDER BY name,e.id"
    );
    let displayed = |sql: &str| -> Result<Json> {
        Ok(Json::Array(rows(c, sql, &[id])?.into_iter().map(|r| display(c, r)).collect::<Result<_>>()?))
    };
    out.set("assertions", displayed(&assertions)?);
    out.set("candidates", displayed(&candidates)?);
    out.set("unknown", exists(c, "SELECT 1 FROM deferred WHERE uuid=?", &[id])?.into());
    let tags = strings(c, "SELECT tag FROM asset_tags WHERE uuid=? ORDER BY tag", &[id])?;
    out.set("user_tags", Json::Array(tags.into_iter().map(Into::into).collect()));
    out.set(
        "excluded",
        rows(c, "SELECT reason,origin FROM excluded WHERE uuid=?", &[id])?.into_iter().next().unwrap_or_else(null),
    );
    let curated = rows(c, "SELECT status FROM curation WHERE uuid=?", &[id])?.into_iter().next();
    out.set("curation", curated.as_ref().map(|v| get(v, "status")).unwrap_or_else(null));
    out.set(
        "pending",
        (curated.is_none() && exists(c, "SELECT 1 FROM auto_rules WHERE uuid=? AND kind='pending'", &[id])?).into(),
    );
    out.set("duplicates", copies(c, id)?);
    let source = context.source.as_ref().ok_or_else(|| Error::Invalid("HTTP 图库状态缺少 source".into()))?;
    c.execute("ATTACH DATABASE ? AS src", [crate::http_read::sqlite_uri(source)])?;
    let tags = "
        SELECT value,conf
        FROM src.labels
        WHERE uuid=? AND dim='general' AND conf>=0.35
        ORDER BY conf DESC
        LIMIT 24";
    out.set("tags", Json::Array(rows(c, tags, &[id])?));
    out.set("identity", crate::http_characters::assignments(c, id)?);
    out.set("question", question(c, id)?);
    attach_provenance(c, context)?;
    out.extend(crate::http_artists::for_asset(c, context.library.is_some(), id, None, false)?);
    Ok(out)
}
/// 图片的成员关系，后面跟着用户在这张图上否定过的家族。
fn ownership(c: &Connection, id: &str, mut members: Vec<Json>) -> Result<Json> {
    let rejected = format!(
        "SELECT e.id,entity_name(e.id) AS name,s.status,s.origin,v.sources,v.score
         FROM assertions s
         JOIN {ENTITIES} e ON e.id=s.entity
         LEFT JOIN family_evidence v ON v.entity=e.id AND v.uuid=s.uuid
         WHERE s.uuid=? AND s.status='rejected' AND e.parent_id IS NULL"
    );
    for r in rows(c, &rejected, &[id])? {
        let mut item = display(c, r.clone())?;
        item.set("evidence", evidence(&r, &[]));
        members.push(item);
    }
    Ok(Json::Array(members))
}
/// 证据在这张图上指向的其他身份，最多十二个，强线索在前。
fn alternatives(c: &Connection, id: &str) -> Result<Json> {
    let sql = format!(
        "WITH extension AS (
           SELECT coalesce(e.parent_id,e.id) AS entity,v.source,max(v.score) AS score
           FROM candidates v JOIN {ENTITIES} e ON e.id=v.entity
           WHERE v.uuid=?1 AND (v.source='CCIP_LONGTAIL'
             OR (v.source='WD_EXT' AND EXISTS(SELECT 1 FROM meta WHERE key='wd_extension')))
           GROUP BY coalesce(e.parent_id,e.id),v.source
         )
         SELECT *
         FROM (
           SELECT e.id,entity_name(e.id) AS name,e.work,v.sources,v.score,v.wd,v.ccip,v.rank,0 AS weak,0 AS priority
           FROM family_evidence v
           JOIN {ENTITIES} e ON e.id=v.entity
           WHERE v.uuid=?1
           UNION ALL
           SELECT e.id,entity_name(e.id),e.work,v.source,v.score,NULL,
                  CASE WHEN v.source='CCIP_LONGTAIL' THEN v.score END,0,0,
                  CASE WHEN v.source='CCIP_LONGTAIL' THEN 1 ELSE 2 END
           FROM extension v JOIN {ENTITIES} e ON e.id=v.entity
           WHERE NOT EXISTS(SELECT 1 FROM family_evidence f WHERE f.uuid=?1 AND f.entity=v.entity)
             AND (v.source='CCIP_LONGTAIL' OR NOT EXISTS(
                 SELECT 1 FROM extension x WHERE x.entity=v.entity AND x.source='CCIP_LONGTAIL'))
           UNION ALL
           SELECT e.id,entity_name(e.id),e.work,group_concat(w.source),max(w.score),NULL,max(w.score),1000,1,3
           FROM weak_claims w
           JOIN {ENTITIES} e ON e.id=w.entity
           WHERE w.uuid=?1 AND NOT EXISTS(SELECT 1 FROM family_evidence f WHERE f.uuid=w.uuid AND f.entity=w.entity)
             AND NOT EXISTS(SELECT 1 FROM extension v WHERE v.entity=w.entity)
           GROUP BY e.id) x
         WHERE NOT EXISTS(SELECT 1 FROM memberships m WHERE m.uuid=?1 AND m.entity=x.id)
           AND NOT EXISTS(SELECT 1 FROM assertions s WHERE s.uuid=?1 AND s.entity=x.id AND s.status='rejected')
         ORDER BY priority,rank,score DESC
         LIMIT 12"
    );
    let mut alternatives = Vec::new();
    for r in rows(c, &sql, &[id])? {
        let mut item =
            display(c, Json::object([("id", get(&r, "id")), ("name", get(&r, "name")), ("work", get(&r, "work"))]))?;
        item.set("weak", (get(&r, "weak").integer() != 0).into());
        item.set("evidence", evidence(&r, &["wd", "ccip"]));
        alternatives.push(item);
    }
    Ok(Json::Array(alternatives))
}
/// 图片所在重复组里的其他副本，附上每份副本上的身份名字。
fn copies(c: &Connection, id: &str) -> Result<Json> {
    let copies = "
        SELECT y.uuid,y.pos,y.size,a.width,a.height,a.added_at
        FROM duplicates d
        JOIN duplicates y ON y.grp=d.grp AND y.uuid<>d.uuid
        JOIN assets a ON a.uuid=y.uuid
        WHERE d.uuid=?
        ORDER BY y.pos";
    let names = format!(
        "SELECT entity_name(e.id)
         FROM memberships m
         JOIN {ENTITIES} e ON e.id=m.entity
         WHERE m.uuid=?
         ORDER BY m.status='machine',1"
    );
    let mut copies = rows(c, copies, &[id])?;
    for copy in &mut copies {
        let uuid = get(copy, "uuid");
        copy.set("names", Json::Array(strings(c, &names, &[uuid.text()])?.into_iter().map(Into::into).collect()));
    }
    Ok(Json::Array(copies))
}
fn pools(c: &Connection, nsfw: &str) -> Result<Json> {
    let r = rating("a", nsfw)?;
    let seen = format!("{}{r}", visible("a"));
    let unmatched =
        format!("SELECT count(*) AS n,min(a.uuid) AS cover FROM assets a WHERE {seen} AND {}", unmatched("a"));
    let row = rows(c, &unmatched, &[])?.remove(0);
    let skipped = count(c, &format!("SELECT count(*) FROM deferred a JOIN assets USING(uuid) WHERE {seen}"), &[])?;
    let listed = format!("NOT EXISTS(SELECT 1 FROM excluded x WHERE x.uuid=d.uuid){}", rating("d", nsfw)?);
    let dups =
        rows(c, &format!("SELECT count(*) AS n,count(DISTINCT grp) AS groups FROM duplicates d WHERE {listed}"), &[])?
            .remove(0);
    let mixed = count(c, &format!("SELECT count(DISTINCT d.grp) FROM duplicates d WHERE {MIXED} AND {listed}"), &[])?;
    let mut reasons = Json::Object(Vec::new());
    let mut excluded = 0;
    for x in rows(
        c,
        &format!("SELECT reason,count(*) AS n FROM excluded a WHERE {}{r} GROUP BY reason", unfolded("a")),
        &[],
    )? {
        excluded += get(&x, "n").integer();
        reasons.set(get(&x, "reason").text(), get(&x, "n"));
    }
    let recent = format!(
        "SELECT count(*)
         FROM sync_added a
         WHERE run_id=(SELECT max(run_id) FROM sync_added) AND {seen}
           AND EXISTS(SELECT 1 FROM assets x WHERE x.uuid=a.uuid)"
    );
    let recent = count(c, &recent, &[])?;
    let pending = format!(
        "SELECT count(*)
         FROM auto_rules a
         WHERE kind='pending' AND {seen} AND NOT EXISTS(SELECT 1 FROM curation k WHERE k.uuid=a.uuid)"
    );
    let pending = count(c, &pending, &[])?;
    let audit = count(c, &format!("SELECT count(*) FROM assets a WHERE {} AND {seen}", audit_open("a")), &[])?;
    let basic = |kind: &str, name: &str, n: Json, cover: Json| {
        Json::object([("kind", kind.into()), ("name", name.into()), ("count", n), ("cover", cover)])
    };
    Ok(Json::Array(vec![
        basic("unmatched", "未识别", get(&row, "n"), get(&row, "cover")),
        basic("skipped", "已跳过", skipped.into(), null()),
        Json::object([
            ("kind", "duplicates".into()),
            ("name", "重复".into()),
            ("count", get(&dups, "n")),
            ("groups", get(&dups, "groups")),
            ("mixed", mixed.into()),
            ("cover", null()),
        ]),
        Json::object([
            ("kind", "excluded".into()),
            ("name", "已排除".into()),
            ("count", excluded.into()),
            ("reasons", reasons),
            ("cover", null()),
        ]),
        basic("pending", "待确认", pending.into(), null()),
        basic("recent", "新加入", recent.into(), null()),
        basic("audit", "随机抽查", audit.into(), null()),
    ]))
}
fn home(c: &Connection, context: &LibraryContext, q: &HashMap<String, String>) -> Result<Json> {
    let before = value(q, "before");
    let days = python_int(q.get("days").map(String::as_str).unwrap_or("5")).map_err(Error::Invalid)?;
    let filters = browse_filters(q);
    let (seen, filter_args, order) = if filters.keys().all(|k| k == "nsfw") {
        // 原来的首页请求仍走同一可见性、排序和验证规则。
        (format!("{}{}", visible("a"), rating("a", value(q, "nsfw"))?), Vec::new(), "a.added_at DESC,a.uuid".into())
    } else {
        selection(c, context, &filters)?
    };
    let mut args = filter_args.clone();
    if !before.is_empty() {
        args.push(Value::Text(before.to_owned()));
    }
    args.push(Value::Integer(days));
    let groups = format!(
        "SELECT substr(a.added_at,1,10) AS day,count(*) AS n
         FROM assets a
         WHERE {seen} AND a.added_at IS NOT NULL{before}
         GROUP BY 1
         ORDER BY 1 DESC
         LIMIT ?",
        before = if before.is_empty() { "" } else { " AND a.added_at<?" }
    );
    let groups = typed_rows(c, &groups, &args)?;
    let day_people = format!(
        "SELECT m.entity,count(*) AS n
         FROM memberships m
         JOIN assets a ON a.uuid=m.uuid
         WHERE {seen} AND a.added_at>=? AND a.added_at<?
         GROUP BY m.entity
         ORDER BY n DESC,m.entity"
    );
    let latest = format!(
        "SELECT a.uuid,a.width,a.height
         FROM assets a
         WHERE {seen} AND a.added_at>=? AND a.added_at<?
         ORDER BY {order}
         LIMIT ?"
    );
    let mut result = Vec::new();
    let mut all = Vec::new();
    let mut lengths = Vec::new();
    for g in groups {
        let day = get(&g, "day");
        let (lo, hi) = day_bounds(day.text())?;
        let mut args = filter_args.clone();
        args.extend([Value::Text(lo), Value::Text(hi)]);
        let people = typed_rows(c, &day_people, &args)?;
        args.push(Value::Integer(18));
        let items = typed_rows(c, &latest, &args)?;
        lengths.push(items.len());
        all.extend(items);
        let mut chars = Vec::new();
        for p in people.iter().take(8) {
            let shown = display(c, Json::object([("id", get(p, "entity"))]))?;
            chars.push(Json::object([
                ("id", get(&shown, "id")),
                ("name", get(&shown, "name")),
                ("tag", get(&shown, "tag")),
                ("count", get(p, "n")),
            ]));
        }
        result.push(Json::object([
            ("day", day),
            ("count", get(&g, "n")),
            ("people", (people.len() as i64).into()),
            ("items", Json::Array(Vec::new())),
            ("characters", Json::Array(chars)),
        ]));
    }
    decorate(c, q, &mut all)?;
    let mut at = 0;
    for (g, n) in result.iter_mut().zip(lengths) {
        let items = all[at..at + n].to_vec();
        at += n;
        let top = match g.get("characters") {
            Some(Json::Array(v)) => v.first().map(|v| get(v, "id")),
            _ => None,
        };
        let hero = top
            .and_then(|t| {
                items.iter().find(|v| match v.get("memberships") {
                    Some(Json::Array(m)) => m.iter().any(|m| get(m, "id").text() == t.text()),
                    _ => false,
                })
            })
            .or_else(|| items.first())
            .map(|v| get(v, "uuid"))
            .unwrap_or_else(null);
        g.set("items", Json::Array(items));
        g.set("hero", hero);
    }
    let next = if result.len() == days as usize {
        result.last().map(|v| get(v, "day")).unwrap_or_else(|| "".into())
    } else {
        "".into()
    };
    Ok(Json::object([("days", Json::Array(result)), ("next", next)]))
}
fn adjacent_month(
    c: &Connection,
    conditions: &str,
    filter_args: &[Value],
    edge: &Value,
    earlier: bool,
) -> Result<Json> {
    let (comparison, direction) = if earlier { ("<", "DESC") } else { (">=", "ASC") };
    let sql = format!(
        "SELECT substr(a.added_at,1,7)
         FROM assets a
         WHERE {conditions} AND a.added_at{comparison}?
         ORDER BY a.added_at {direction}
         LIMIT 1"
    );
    let month = c
        .query_row(&sql, rusqlite::params_from_iter(filter_args.iter().chain(std::iter::once(edge))), |r| {
            r.get::<_, String>(0)
        })
        .optional()?;
    Ok(month.into())
}

fn calendar(c: &Connection, context: &LibraryContext, q: &HashMap<String, String>) -> Result<Json> {
    let (start, end) = month_bounds(value(q, "month"))?;
    let (conditions, mut args, order) = selection(c, context, &browse_filters(q))?;
    let (start, end) = (Value::Text(start), Value::Text(end));
    let prev = adjacent_month(c, &conditions, &args, &start, true)?;
    let next = adjacent_month(c, &conditions, &args, &end, false)?;
    args.extend([start, end]);
    // 范围过滤可用 assets_date。先数当天全部匹配图片，再给最多四张样图加载展示字段。
    let sql = format!(
        "WITH ranked AS (
           SELECT a.uuid,substr(a.added_at,1,10) AS day,
                  count(*) OVER (PARTITION BY substr(a.added_at,1,10)) AS n,
                  row_number() OVER (PARTITION BY substr(a.added_at,1,10) ORDER BY {order}) AS pos
           FROM assets a
           WHERE {conditions} AND a.added_at>=? AND a.added_at<?
         )
         SELECT r.day,r.n,a.uuid,a.width,a.height
         FROM ranked r JOIN assets a ON a.uuid=r.uuid
         WHERE r.pos<=4
         ORDER BY r.day,r.pos"
    );
    let rows = typed_rows(c, &sql, &args)?;
    let mut samples: Vec<_> = rows
        .iter()
        .map(|r| Json::object([("uuid", get(r, "uuid")), ("width", get(r, "width")), ("height", get(r, "height"))]))
        .collect();
    decorate(c, q, &mut samples)?;
    let mut samples = samples.into_iter();
    let mut days = Vec::new();
    let mut total = 0;
    for group in rows.chunk_by(|a, b| a.get("day").map(Json::text) == b.get("day").map(Json::text)) {
        let count = get(&group[0], "n");
        total += count.integer();
        days.push(Json::object([
            ("day", get(&group[0], "day")),
            ("count", count),
            ("samples", Json::Array(samples.by_ref().take(group.len()).collect())),
            ("color", null()),
        ]));
    }
    Ok(Json::object([("days", Json::Array(days)), ("total", total.into()), ("prev", prev), ("next", next)]))
}

/// 走日期索引分页（#48）时，隐藏 NSFW 后图库至少还要剩下的比例，单位千分之一。
/// 在合成图库上测得（debug 宿主，1 万和 5 万张资产）：计数两次会从词条投影重建两次 NSFW 列表，所以只有
/// NSFW 占少数时，日期索引分页才比筛选副本快；见 docs/TERMS_PERFORMANCE.md 第 3 节。
const DATE_PAGE_MIN_KEPT_PERMILLE: i64 = 700;
/// 只隐藏 NSFW、按日期排序的普通页面是否应跳过筛选副本。在真正计数之前，根据同一快照里两次廉价的读取决定：
/// 全部资产数，以及分级投影里的 NSFW 行数（一段索引范围，不做 asset_rating 的连接，超过阈值就停）。
/// 这个估计不考虑折叠和排除的行；它只决定走哪条路径，从不影响结果。标签不在核心里的图库继续用副本。
/// 很深的分页也继续用副本，因为沿索引走过一半以上的保留行，开销约等于再计数一次。
fn date_page_pays(c: &Connection, offset: i64, limit: i64) -> Result<bool> {
    if !flag(c, "terms_labels")? {
        return Ok(false);
    }
    let all = count(c, "SELECT count(*) FROM assets", &[])?;
    let most_nsfw = all - (all * DATE_PAGE_MIN_KEPT_PERMILLE + 999) / 1000;
    let nsfw = format!(
        "SELECT count(*)
         FROM (
           SELECT 1
           FROM term_projection
           WHERE state IN ('machine','confirmed')
             AND term_id IN (
               SELECT l.term_id
               FROM term_links l
               JOIN terms t ON t.id=l.term_id AND t.kind='rating'
               WHERE l.system='wd_rating' AND l.key IN ({NSFW_RATINGS}))
           LIMIT {})",
        most_nsfw + 1
    );
    let nsfw = count(c, &nsfw, &[])?;
    Ok(nsfw <= most_nsfw && (offset + limit) * 2 <= all - nsfw)
}
pub fn read(c: &Connection, context: &LibraryContext, path: &str, q: &HashMap<String, String>) -> Result<(Json, u16)> {
    validate(path, q)?;
    let result = match path {
        "/api/fold" => {
            let kinds = c
                .query_row("SELECT value FROM prefs WHERE key='fold_kinds'", [], |r| r.get::<_, String>(0))
                .optional()?
                .map(|s| crate::http_json::parse_python(&s))
                .transpose()?
                .unwrap_or_else(|| Json::Array(vec!["duplicate".into(), "identical".into()]));
            Json::object([("kinds", kinds)])
        }
        "/api/review-pools" => pools(c, value(q, "nsfw"))?,
        "/api/galleries" => crate::app_galleries::read(c, value(q, "nsfw"))?,
        "/api/galleries/covers" => crate::app_galleries::covers(c, context, value(q, "nsfw"), value(q, "ids"))?,
        "/api/detail" => {
            let mut item = detail(c, context, value(q, "id"))?;
            if marks_favorite(q) {
                decorate_favorite(c, std::slice::from_mut(&mut item))?;
            }
            item
        }
        "/api/home" => home(c, context, q)?,
        "/api/calendar" => calendar(c, context, q)?,
        "/api/assets" | "/api/assets/count" | "/api/ids" => assets(c, context, path, q)?,
        _ => return Err(Error::Invalid("未迁移浏览接口".into())),
    };
    Ok((result, 200))
}
/// 普通图库的总数等于资产数减去已排除的资产，再减去尚未计入排除的折叠资产。
/// 以排除表与副本表为起点，只查它们引用的资产；已移出图库的 UUID 不参与扣减。
fn visible_total(c: &Connection) -> Result<i64> {
    count(
        c,
        "SELECT (SELECT count(*) FROM assets)
             - (SELECT count(*) FROM excluded x CROSS JOIN assets a ON a.uuid=x.uuid)
             - (SELECT count(*) FROM duplicates h CROSS JOIN assets a ON a.uuid=h.uuid
                WHERE h.hidden=1 AND NOT EXISTS(SELECT 1 FROM excluded x WHERE x.uuid=h.uuid))",
        &[],
    )
}

fn selection_total(c: &Connection, f: &HashMap<String, String>, conditions: &str, args: &[Value]) -> Result<Json> {
    let visible_only = f.iter().all(|(key, value)| match key.as_str() {
        "order" => true,
        "status" => value == "all",
        "nsfw" => value.is_empty(),
        _ => false,
    });
    let total = if visible_only {
        visible_total(c)?
    } else {
        c.query_row(
            &format!("SELECT count(*) FROM assets a WHERE {conditions}"),
            rusqlite::params_from_iter(args),
            |r| r.get::<_, i64>(0),
        )?
    };
    Ok(total.into())
}

/// 选区的数量（/api/assets/count）、id 列表（/api/ids）或其中一页（/api/assets）。
fn assets(c: &Connection, context: &LibraryContext, path: &str, q: &HashMap<String, String>) -> Result<Json> {
    let mut f = if path == "/api/assets" { browse_filters(q) } else { q.clone() };
    let paged = |k: &str, default: &str| -> Result<i64> {
        if path == "/api/assets/count" {
            return Ok(if k == "offset" { 0 } else { 100 });
        }
        python_int(q.get(k).map(String::as_str).unwrap_or(default)).map_err(Error::Invalid)
    };
    let offset = paged("offset", "0")?;
    let limit = paged("limit", "100")?;
    if path == "/api/ids" {
        f.remove("offset");
        f.remove("limit");
    }
    let (where_, mut args, order) = selection(c, context, &f)?;
    if path == "/api/assets/count" {
        return Ok(Json::object([("total", selection_total(c, &f, &where_, &args)?)]));
    }
    if path == "/api/ids" {
        args.extend([Value::Integer(limit), Value::Integer(offset)]);
        let sql = format!("SELECT a.uuid FROM assets a WHERE {where_} ORDER BY {order} LIMIT ? OFFSET ?");
        return Ok(Json::Array(typed_rows(c, &sql, &args)?.iter().map(|r| get(r, "uuid")).collect()));
    }
    let total_needed = value(q, "count") != "0";
    let nsfw = value(q, "nsfw");
    // 带分级筛选的总数先把匹配结果复制一次，再对副本计数和排序，这样分级子查询和其他筛选只跑一次。
    // 只有按日期排序、隐藏 NSFW 的普通图库视图（#48）可以改用 assets_date 分页，条件是 date_page_pays()
    // 估计复制和排序副本的开销高于计数两次。两条路径都读同一个事务快照，而且每种排序最后都按主键排，
    // 所以返回的行和顺序完全相同。
    let date_page = total_needed
        && nsfw == "hide"
        && order.starts_with("a.added_at ")
        && f.iter().all(|(k, v)| k == "nsfw" || k == "order" || v.is_empty())
        && date_page_pays(c, offset, limit)?;
    let (total, sql) = if total_needed && !nsfw.is_empty() && !date_page && !f.contains_key("like") {
        let source = if value(&f, "order") == "color" { ",a.src_path" } else { "" };
        let copy = format!(
            "CREATE TEMP TABLE selected_assets AS
             SELECT a.uuid,a.width,a.height,a.added_at{source} FROM assets a WHERE {where_}"
        );
        c.execute(&copy, rusqlite::params_from_iter(&args))?;
        let n = count(c, "SELECT count(*) FROM selected_assets", &[])?;
        args.clear();
        (n.into(), format!("SELECT a.uuid,a.width,a.height FROM selected_assets a ORDER BY {order} LIMIT ? OFFSET ?"))
    } else {
        let total = if total_needed { selection_total(c, &f, &where_, &args)? } else { null() };
        (
            total,
            format!("SELECT a.uuid,a.width,a.height FROM assets a WHERE {where_} ORDER BY {order} LIMIT ? OFFSET ?"),
        )
    };
    args.extend([Value::Integer(limit), Value::Integer(offset)]);
    let mut items = typed_rows(c, &sql, &args)?;
    decorate(c, q, &mut items)?;
    Ok(Json::object([("items", Json::Array(items)), ("total", total), ("offset", offset.into())]))
}

// 没有研究 worker 时也能读到缓存的分组。`seen` 时每组多一个 `seen`：看得到的成员是否都在某一次「看过了」里（#427）。
pub(crate) fn similar_groups(c: &Connection, kind: &str, nsfw: &str, seen: bool) -> Result<Json> {
    const KINDS: &[&str] = &["identical", "duplicate", "subtle", "crop", "variant", "edit"];
    if !kind.is_empty() && !KINDS.contains(&kind) {
        return Err(Error::Invalid("未知关系".into()));
    }
    let a = format!("{}{}", visible("a"), rating("a", nsfw)?);
    let b = format!("{}{}", visible("b"), rating("b", nsfw)?);
    let mut members: Vec<(Json, Vec<Json>)> = Vec::new();
    let mut group_positions = HashMap::new();
    let grouped = format!(
        "SELECT g.gid,g.uuid
         FROM similar_groups g
         JOIN assets a ON a.uuid=g.uuid
         WHERE {a}
         ORDER BY g.gid,a.added_at DESC"
    );
    for r in rows(c, &grouped, &[])? {
        let gid = get(&r, "gid");
        let pos = *group_positions.entry(gid.integer()).or_insert_with(|| {
            members.push((gid, Vec::new()));
            members.len() - 1
        });
        members[pos].1.push(get(&r, "uuid"));
    }
    let related = format!(
        "SELECT g.gid,r.kind,count(*) AS n
         FROM relations r
         JOIN similar_groups g ON g.uuid=r.a
         JOIN similar_groups h ON h.uuid=r.b AND h.gid=g.gid
         JOIN assets a ON a.uuid=r.a
         JOIN assets b ON b.uuid=r.b
         WHERE {a} AND {b}
         GROUP BY g.gid,r.kind"
    );
    let mut kinds: HashMap<i64, Json> = HashMap::new();
    for r in rows(c, &related, &[])? {
        if KINDS.contains(&get(&r, "kind").text()) {
            let group = kinds.entry(get(&r, "gid").integer()).or_insert_with(|| Json::Object(Vec::new()));
            group.set(get(&r, "kind").text(), get(&r, "n"));
        }
    }
    let marks = if seen { Some(crate::app_similar_seen::Seen::load(c)?) } else { None };
    Ok(Json::Array(
        members
            .into_iter()
            .filter_map(|(gid, uuids)| {
                let k = kinds.remove(&gid.integer()).unwrap_or_else(|| Json::Object(Vec::new()));
                if uuids.len() <= 1 || (!kind.is_empty() && k.get(kind).is_none()) {
                    return None;
                }
                let seen = marks.as_ref().map(|marks| {
                    let texts: Vec<&str> = uuids.iter().map(Json::text).collect();
                    !marks.covering(&texts).is_empty()
                });
                let mut group = Json::object([
                    ("gid", gid),
                    ("size", (uuids.len() as i64).into()),
                    ("covers", Json::Array(uuids.into_iter().take(4).collect())),
                    ("kinds", k),
                ]);
                if let Some(seen) = seen {
                    group.set("seen", seen.into());
                }
                Some(group)
            })
            .collect(),
    ))
}
pub(crate) fn clusters(c: &Connection, nsfw: &str) -> Result<Json> {
    let clustered = format!(
        "SELECT i.cid,i.near,i.near_dist,i.near_kind,i.traits,i.built_at,
           count(k.uuid) AS size,group_concat(k.uuid) AS members
         FROM cluster_info i
         JOIN clusters k ON k.cid=i.cid
         JOIN assets a ON a.uuid=k.uuid
         WHERE {}{} AND {}
         GROUP BY i.cid
         HAVING count(k.uuid)>=2
         ORDER BY size DESC,i.cid",
        visible("a"),
        rating("a", nsfw)?,
        unclaimed("k")
    );
    let mut groups = rows(c, &clustered, &[])?;
    let mut members = HashMap::new();
    let mut ordered = Vec::new();
    for r in &groups {
        for u in get(r, "members").text().split(',') {
            if !members.contains_key(u) {
                ordered.push(u.to_owned());
            }
            members.insert(u.to_owned(), get(r, "cid").integer());
        }
    }
    let page = Json::Array(ordered.into_iter().map(Into::into).collect()).dump();
    let mut votes: HashMap<i64, Vec<(String, i64)>> = HashMap::new();
    let mut vote_positions = HashMap::new();
    for (uuid, entity) in first_leads(c, &page)? {
        let cid = members[&uuid];
        let group = votes.entry(cid).or_default();
        let pos = *vote_positions.entry((cid, entity.clone())).or_insert_with(|| {
            group.push((entity, 0));
            group.len() - 1
        });
        group[pos].1 += 1;
    }
    for r in &mut groups {
        let members = get(r, "members").text().split(',').map(|s| s.into()).collect::<Vec<Json>>();
        r.set("members", Json::Array(members.clone()));
        r.set("covers", Json::Array(members.into_iter().take(4).collect()));
        let traits = get(r, "traits");
        r.set(
            "traits",
            if traits.text().is_empty() {
                Json::Array(Vec::new())
            } else {
                crate::http_json::parse_python(traits.text())?
            },
        );
        let mut lead = null();
        if let Some(v) = votes.get(&get(r, "cid").integer()) {
            let top = v.iter().reduce(|a, b| if b.1 > a.1 { b } else { a });
            if let Some((id, n)) = top.filter(|(_, n)| *n >= 2.max((get(r, "size").integer() + 1) / 2)) {
                let info = display(c, Json::object([("id", id.clone().into())]))?;
                lead = Json::object([("id", id.clone().into()), ("name", get(&info, "name")), ("count", (*n).into())]);
            }
        }
        r.set("lead", lead);
    }
    Ok(Json::Array(groups))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cluster_overflow_keeps_io_error_and_i64_boundary() {
        assert_eq!(
            bound(vec![i64::MAX.to_string()], Some(0), "９２２３３７２０３６８５４７７５８０７").unwrap(),
            vec![Value::Integer(i64::MAX)]
        );
        for cluster in ["9223372036854775808", "９２２３３７２０３６８５４７７５８０８"] {
            assert!(matches!(bound(vec![cluster.into()], Some(0), cluster), Err(Error::Io(e))
                if e.to_string() == "整数超出数据库支持的范围"));
        }
    }
    /// 核心的身份树，按这里的夹具所需的样子建。
    const TERM_TREE: &str = "CREATE TABLE term_tree(term_id INTEGER PRIMARY KEY,parent_id TEXT,merged_into TEXT,
        work_id TEXT,work_text TEXT NOT NULL DEFAULT '',variant TEXT NOT NULL DEFAULT '',
        artist TEXT NOT NULL DEFAULT '',
        label_only INTEGER NOT NULL DEFAULT 0,mascot INTEGER NOT NULL DEFAULT 0,mascot_origin TEXT,
        original INTEGER NOT NULL DEFAULT 0,seq INTEGER);";
    #[test]
    fn page_display_matches_single_row_renderer_for_projected_names() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(TERM_TREE).unwrap();
        c.execute_batch(
            "CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT); INSERT INTO meta VALUES ('terms_names','core');
             CREATE TABLE terms(id INTEGER PRIMARY KEY,key TEXT UNIQUE,kind TEXT);
             INSERT INTO terms VALUES
               (1,'root','character'),(2,'skin','skin'),(3,'oc','character'),(4,'unshown','character'),
               (5,'work','work');
             INSERT INTO term_tree(term_id,original,work_id,parent_id,seq) VALUES
               (1,0,'work',NULL,1),(2,0,'unused','root',2),(3,1,NULL,NULL,3),(4,0,NULL,NULL,4),(5,0,NULL,NULL,5);
             CREATE TABLE identity_nodes(id TEXT PRIMARY KEY,node_type TEXT,character_id TEXT,variant_id TEXT,
               canonical_id TEXT);
             INSERT INTO identity_nodes VALUES
               ('root','character','root',NULL,'root'),('skin','form','root','skin','skin');",
        )
        .unwrap();
        c.execute_batch(crate::names_view::TABLES).unwrap();
        for (id, name) in [("root", "人物🙂"), ("skin", "人物 · 泳装"), ("oc", "OC"), ("work", "作品")] {
            for skin in [0, 1] {
                c.execute(
                    "INSERT INTO term_name_display VALUES (?,?,?,'user','中',NULL,'en','tag')",
                    rusqlite::params![id, skin, if skin == 0 { name } else { "短名" }],
                )
                .unwrap();
            }
        }
        for hybrid in [false, true] {
            c.execute("INSERT OR REPLACE INTO meta VALUES ('identity_policy',?)", [if hybrid { "hybrid" } else { "" }])
                .unwrap();
            crate::http_read::register_names(&c).unwrap();
            let ids = ["root", "skin", "oc", "unshown", "absent"];
            let page = PageDisplay::load(&c, ids.iter().map(|id| (*id).into()).collect()).unwrap().unwrap();
            for id in ids {
                for skin in [false, true] {
                    for work in [false, true] {
                        for kind in [false, true] {
                            let mut row = Json::object([
                                ("id", id.into()),
                                ("name", "旧名".into()),
                                ("status", "confirmed".into()),
                            ]);
                            if work {
                                row.set("work", "旧作品".into());
                            }
                            if kind {
                                row.set("kind", "旧类型".into());
                            }
                            assert_eq!(
                                page.display(row.clone(), skin).dump(),
                                shown(&c, row, skin).unwrap().dump(),
                                "{id} skin={skin} work={work} kind={kind} hybrid={hybrid}"
                            );
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn query_only_groups_match_recorded_python_store_raw_json() {
        let path = std::env::temp_dir().join(format!(
            "illustash-browse-groups-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let mut c = crate::sqlite::open(&path).unwrap();
        c.execute_batch(TERM_TREE).unwrap();
        c.execute_batch(
            "CREATE TABLE meta(key TEXT,value TEXT);
             CREATE TABLE terms(id INTEGER PRIMARY KEY,key TEXT UNIQUE,kind TEXT);
             INSERT INTO terms VALUES (1,'alice','character'),(2,'bob','character');
             INSERT INTO term_tree(term_id,seq) VALUES (1,1),(2,2);
             CREATE TABLE assets(uuid TEXT PRIMARY KEY,added_at TEXT);
             INSERT INTO assets VALUES ('a','2026-09-30'),('b','2026-09-30'),('c','2026-09-29'),('d','2026-09-28'),
               ('e','2026-09-27'),('f','2026-09-26');
             CREATE TABLE duplicates(uuid TEXT,hidden INTEGER);INSERT INTO duplicates VALUES ('e',1);
             CREATE TABLE excluded(uuid TEXT);INSERT INTO excluded VALUES ('f');
             CREATE TABLE asset_rating(uuid TEXT,rating TEXT);INSERT INTO asset_rating VALUES ('c','explicit');
             CREATE TABLE memberships(uuid TEXT,entity TEXT);CREATE TABLE deferred(uuid TEXT);
             CREATE TABLE assertions(uuid TEXT,entity TEXT,status TEXT);
             CREATE TABLE family_evidence(uuid TEXT,entity TEXT,score REAL);
             INSERT INTO family_evidence VALUES ('a','bob',.92),('b','alice',.92),('c','alice',.91),('d','bob',.9);
             CREATE TABLE weak_claims(uuid TEXT,entity TEXT,score REAL);
             CREATE TABLE similar_groups(uuid TEXT,gid INTEGER);
             INSERT INTO similar_groups VALUES ('a',1),('b',1),('c',1),('d',1),('e',1),('f',1);
             CREATE TABLE relations(a TEXT,b TEXT,kind TEXT);
             INSERT INTO relations VALUES
               ('a','b','identical'),('b','c','subtle'),('c','d','edit'),('e','f','duplicate');
             CREATE TABLE clusters(uuid TEXT,cid INTEGER);
             INSERT INTO clusters VALUES ('a',7),('b',7),('c',7),('d',7),('e',7),('f',7);
             CREATE TABLE cluster_info(cid INTEGER,near TEXT,near_dist REAL,near_kind TEXT,traits TEXT,built_at TEXT);
             INSERT INTO cluster_info VALUES (7,'near',.125,'character','{\"z\":1,\"a\":2}','2026-09-30');
             CREATE INDEX weak_uuid ON weak_claims(uuid,entity);CREATE INDEX cluster_cid ON clusters(cid,uuid);",
        )
        .unwrap();
        let f = rusqlite::functions::FunctionFlags::SQLITE_UTF8;
        c.create_scalar_function("entity_info", 2, f, |ctx| {
            let id: String = ctx.get(0)?;
            Ok(Json::object([("name", id.into())]).dump())
        })
        .unwrap();
        // 原 Python Store 的 clusters()/similar_groups() 在这个数据库上输出的字节，录制一次（#9）。
        let expected = include_bytes!("../tests/fixtures/json/browse_groups.json");
        let tx = c.transaction().unwrap();
        let mut actual = Vec::new();
        for nsfw in ["", "hide", "only"] {
            actual.push(clusters(&tx, nsfw).unwrap());
            for kind in ["", "identical", "subtle", "edit"] {
                actual.push(similar_groups(&tx, kind, nsfw, false).unwrap());
            }
        }
        assert_eq!(Json::Array(actual).dump().as_bytes(), expected.as_slice());
        tx.commit().unwrap();
        drop(c);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn rating_filtered_pages_keep_order_total_and_bounds() {
        // #48：带分级筛选的页面在两条路径上对并列、NULL 日期、折叠和排除的行、最后不满的一页、越过末尾的
        // offset 以及空结果，都保持固定期望。不加额外的行时图库一半是 NSFW，所以每一页都退回筛选副本；
        // 加上十二个被排除的 general 行后，保留比例超过阈值，于是较浅的只隐藏 NSFW 的普通页面用 assets_date，
        // 较深的仍然退回。
        for extra in [0usize, 12] {
            rating_filtered_pages(extra);
        }
    }
    /// 分级页面用的合成图库：八张带分级的图片，外加 `extra` 张被排除的 general 图片（`z00`…）。
    fn rating_library(extra: usize) -> (std::path::PathBuf, std::path::PathBuf, Vec<String>) {
        let root = std::env::temp_dir().join(format!(
            "illustash-browse-rating-pages-{}-{extra}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(root.join("input")).unwrap();
        let dest = root.join("synthetic.pilib");
        crate::app_create::create_folder(&dest, &root.join("input"), None, None, None).unwrap();
        let c = crate::sqlite::open(dest.join("library.sqlite")).unwrap();
        let extras = (0..extra).map(|i| format!("z{i:02}")).collect::<Vec<_>>();
        for z in &extras {
            c.execute(
                "INSERT INTO assets(uuid,filename,width,height,added_at)
                 VALUES (?1,?1||'.png',1,1,'2026-09-01 10:00:00')",
                [z],
            )
            .unwrap();
            c.execute("INSERT INTO excluded VALUES (?,'photo','manual')", [z]).unwrap();
        }
        // 插入顺序与每种期望顺序都不同，所以按 rowid 顺序不可能碰巧通过。
        c.execute_batch(
            "INSERT INTO assets(uuid,filename,width,height,added_at) VALUES
               ('e1','e1.png',1,1,'2026-09-28 10:00:00'),('a3','a3.png',1,1,'2026-09-30 10:00:00'),
               ('c1','c1.png',1,1,NULL),('b2','b2.png',1,1,'2026-09-29 10:00:00'),
               ('a2','a2.png',1,1,'2026-09-30 10:00:00'),('d1','d1.png',1,1,'2026-09-28 10:00:00'),
               ('b1','b1.png',1,1,'2026-09-29 10:00:00'),('a1','a1.png',1,1,'2026-09-30 10:00:00');
             INSERT INTO duplicates VALUES
               ('d1','g','3',1,'2026-09-28 10:00:00',0,1),('b2','g','3',0,'2026-09-28 10:00:00',1,0),
               ('removed-copy','g','3',2,'2026-09-28 10:00:00',0,1);
             INSERT INTO excluded VALUES ('e1','photo','manual');
             INSERT INTO terms(key,kind,origin) VALUES
               ('synthetic-general','rating','synthetic'),('synthetic-sensitive','rating','synthetic'),
               ('synthetic-questionable','rating','synthetic'),('synthetic-explicit','rating','synthetic');
             INSERT INTO term_links(system,key,term_id,source)
               SELECT 'wd_rating',substr(key,11),id,'synthetic' FROM terms WHERE key LIKE 'synthetic-%';
             INSERT INTO term_assets(namespace,key) SELECT 'library',uuid FROM assets;
             INSERT INTO term_projection(asset_id,term_id,state)
               SELECT ta.id,t.id,'machine'
               FROM (VALUES ('a1','general'),('a2','explicit'),('b1','questionable'),('b2','sensitive'),
                            ('c1','general'),('d1','explicit'),('e1','explicit')) r
               JOIN term_assets ta ON ta.namespace='library' AND ta.key=r.column1
               JOIN terms t ON t.key='synthetic-'||r.column2;
             INSERT INTO term_projection(asset_id,term_id,state)
               SELECT ta.id,t.id,'machine'
               FROM term_assets ta
               JOIN terms t ON t.key='synthetic-general'
               WHERE ta.key LIKE 'z%';",
        )
        .unwrap();
        (root, dest, extras)
    }
    fn rating_filtered_pages(extra: usize) {
        let (root, dest, extras) = rating_library(extra);
        let c = crate::sqlite::open(dest.join("library.sqlite")).unwrap();
        // 两个图库都有四张 NSFW 资产：保留 4/8 低于阈值，16/20 高于阈值。
        let pays = |offset, limit| date_page_pays(&c, offset, limit).unwrap();
        if extra == 0 {
            assert!(!pays(0, 1));
        } else {
            assert!(pays(0, 2) && pays(3, 2) && pays(0, 8));
            assert!(!pays(7, 2) && !pays(0, 100));
        }
        drop(c);
        let context = crate::http::LibraryContext {
            examples: None,
            db: dest.join("library.sqlite"),
            generation: "synthetic".into(),
            library: Some(dest.clone()),
            source: Some(dest.join("source.sqlite")),
            tags: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tags/tags_v3.csv"),
            token: Some("synthetic-token".into()),
            read_only: None,
        };
        let page = |query: &str| {
            let (body, status) = crate::http_read::read(&context, &format!("/api/assets?{query}")).unwrap().unwrap();
            assert_eq!(status, 200, "{query}");
            let ids = match body.get("items") {
                Some(Json::Array(items)) => items.iter().map(|v| get(v, "uuid").text().to_owned()).collect::<Vec<_>>(),
                _ => panic!("{query}"),
            };
            (ids, get(&body, "total").dump(), body)
        };
        let excluded = extras.iter().map(String::as_str).collect::<Vec<_>>();
        let excluded_total = extra.to_string();
        let cases: &[(&str, &[&str], &str)] = &[
            ("nsfw=hide", &["a1", "a3", "b2", "c1"], "4"),
            ("nsfw=hide&limit=2", &["a1", "a3"], "4"),
            ("nsfw=hide&limit=1&offset=1", &["a3"], "4"),
            ("nsfw=hide&limit=2&offset=2", &["b2", "c1"], "4"),
            ("nsfw=hide&limit=2&offset=3", &["c1"], "4"),
            ("nsfw=hide&limit=2&offset=4", &[], "4"),
            ("nsfw=hide&limit=2&offset=7", &[], "4"),
            ("nsfw=hide&offset=99", &[], "4"),
            ("nsfw=hide&order=oldest", &["c1", "b2", "a1", "a3"], "4"),
            ("nsfw=hide&order=oldest&limit=2", &["c1", "b2"], "4"),
            ("nsfw=hide&order=oldest&limit=1&offset=3", &["a3"], "4"),
            ("nsfw=hide&q=", &["a1", "a3", "b2", "c1"], "4"),
            ("nsfw=hide&q=&limit=2", &["a1", "a3"], "4"),
            ("nsfw=hide&order=hinted", &["a1", "a3", "b2", "c1"], "4"),
            ("nsfw=hide&order=suspicious&limit=3&offset=1", &["a3", "b2", "c1"], "4"),
            ("nsfw=hide&q=a", &["a1", "a3"], "2"),
            ("nsfw=hide&pool=duplicates", &["b2"], "1"),
            ("nsfw=hide&pool=excluded", &excluded, &excluded_total),
            ("nsfw=only", &["a2", "b1"], "2"),
            ("nsfw=only&order=oldest", &["b1", "a2"], "2"),
            ("nsfw=only&pool=duplicates", &["d1"], "1"),
            ("nsfw=only&q=nomatch", &[], "0"),
            ("nsfw=only&pool=excluded", &["e1"], "1"),
            ("", &["a1", "a2", "a3", "b1", "b2", "c1"], "6"),
            ("nsfw=", &["a1", "a2", "a3", "b1", "b2", "c1"], "6"),
            ("status=all&order=oldest", &["c1", "b1", "b2", "a1", "a2", "a3"], "6"),
        ];
        for (query, expected, total) in cases {
            let (ids, shown_total, body) = page(query);
            assert_eq!(
                (ids.iter().map(String::as_str).collect::<Vec<_>>(), shown_total.as_str()),
                (expected.to_vec(), *total),
                "{query} extra={extra}"
            );
            // 不带总数的页面是同样的行，逐字节相同。
            let (_, untotaled, unpaged) = page(&format!("{query}&count=0"));
            assert_eq!(untotaled, "null", "{query}");
            assert_eq!(get(&body, "items").dump(), get(&unpaged, "items").dump(), "{query}");
            let filters = query.split('&').filter(|p| !p.starts_with("limit=") && !p.starts_with("offset="));
            let url = format!("/api/assets/count?{}", filters.collect::<Vec<_>>().join("&"));
            let counted = crate::http_read::read(&context, &url).unwrap().unwrap();
            assert_eq!(get(&counted.0, "total").dump(), *total, "{query}");
        }
        // 同时被折叠和排除的图片只扣一次，排除表里已移出的 UUID 不扣。
        let c = crate::sqlite::open(&context.db).unwrap();
        c.execute_batch("INSERT INTO excluded VALUES ('d1','photo','manual'),('removed','photo','manual');").unwrap();
        drop(c);
        let (ids, total, _) = page("limit=2&offset=4");
        assert_eq!(ids, ["b2", "c1"]);
        assert_eq!(total, "6");
        let (body, status) = crate::http_read::read(&context, "/api/assets/count").unwrap().unwrap();
        assert_eq!(status, 200);
        assert_eq!(get(&body, "total").integer(), 6);
        std::fs::remove_dir_all(root).unwrap();
    }
}
