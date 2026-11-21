//! HTTP 读取和研究分组共用的 SQL 片段（#205）：每个条件只在这里写一次。接受别名 `a` 的片段读取图片列
//! `a.uuid`（或 `a.entity`）；文本与各读取方原先用的完全相同。`first_leads` 自己执行查询，
//! 研究分组（`research_compute/groups.rs`）和 HTTP 读取都调用它（#790）。
use crate::{Error, Result};
use std::collections::HashSet;

/// 算作 NSFW（R18）的分级：供 `IN (…)` 使用的 SQL 列表。
pub(crate) const NSFW_RATINGS: &str = "'questionable', 'explicit'";

/// 按 `asset_rating` 视图，图片 `a` 的分级是 NSFW。
pub(crate) fn nsfw(a: &str) -> String {
    format!("{a}.uuid IN (SELECT uuid FROM asset_rating WHERE rating IN ({NSFW_RATINGS}))")
}
/// 对图片 `a` 应用内容筛选 `nsfw`（`""`、`"hide"` 或 `"only"`），结果是以 ` AND ` 开头追加的子句；
/// 不筛选时为空。
pub(crate) fn rating(a: &str, nsfw: &str) -> Result<String> {
    match nsfw {
        "" => Ok(String::new()),
        "hide" => Ok(format!(" AND NOT coalesce({},0)", self::nsfw(a))),
        "only" => Ok(format!(" AND coalesce({},0)", self::nsfw(a))),
        _ => Err(Error::Invalid("未知内容筛选".into())),
    }
}
/// 图片 `a` 不是被隐藏（折叠）的重复图。
pub(crate) fn unfolded(a: &str) -> String {
    format!("NOT EXISTS(SELECT 1 FROM duplicates h WHERE h.uuid={a}.uuid AND h.hidden=1)")
}
/// 图片 `a` 既不是被隐藏的重复图，也没有被排除。
pub(crate) fn visible(a: &str) -> String {
    format!("{} AND NOT EXISTS(SELECT 1 FROM excluded x WHERE x.uuid={a}.uuid)", unfolded(a))
}
/// 图片 `a` 不属于任何身份，也没有被暂缓：即未匹配池，不含标签身份。
pub(crate) fn unclaimed(a: &str) -> String {
    format!(
        "NOT EXISTS(SELECT 1 FROM memberships m WHERE m.uuid={a}.uuid) \
         AND NOT EXISTS(SELECT 1 FROM deferred d WHERE d.uuid={a}.uuid)"
    )
}
/// 图片上的全部线索（`uuid`、`entity`、`score`、`weak`）：先是家族证据，再是弱认领。
pub(crate) const HINTS: &str =
    "(SELECT uuid,entity,score,0 AS weak FROM family_evidence UNION ALL SELECT uuid,entity,score,1 FROM weak_claims)";
/// （`a.uuid`，`a.entity`）这一对没有被人否定。
pub(crate) fn unrejected(a: &str) -> String {
    format!(
        "NOT EXISTS(SELECT 1 FROM assertions s WHERE s.uuid={a}.uuid AND s.entity={a}.entity AND s.status='rejected')"
    )
}
/// `HINTS` 中没有被人否定的线索。
pub(crate) fn leads() -> String {
    format!("(SELECT v.* FROM {HINTS} v WHERE {})", unrejected("v"))
}
/// `asked` 里仍在七天暂缓期内的「以后再说」。
pub(crate) const ASK_DEFERRED: &str = "answer='later' AND asked_at>=datetime('now','-7 days')";

/// 从 UUID 的 JSON 数组取每张图的首选线索：没有被否定的线索里，先强证据再弱认领，同类按分数从高到低取第一条。
/// 分组按它拆组，组头按返回顺序计票（同票时取先出现的角色），网格上每张图的线索也取它（#790）。
/// 这几处共用这一个排序，改排序会同时改变分组和组头。
pub(crate) fn first_leads(c: &rusqlite::Connection, page: &str) -> Result<Vec<(String, String)>> {
    let mut query = c.prepare(&format!(
        "SELECT v.uuid,v.entity
         FROM {} v
         WHERE v.uuid IN (SELECT value FROM json_each(?))
         ORDER BY v.weak,v.score DESC",
        leads()
    ))?;
    let mut seen = HashSet::new();
    let mut first = Vec::new();
    for row in query.query_map([page], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))? {
        let (uuid, entity) = row?;
        if seen.insert(uuid.clone()) {
            first.push((uuid, entity));
        }
    }
    Ok(first)
}
/// （`a.uuid`，`a.entity`）这一对还没有问过用户。
pub(crate) fn unasked(a: &str) -> String {
    format!("NOT EXISTS(SELECT 1 FROM asked k WHERE k.uuid={a}.uuid AND k.entity={a}.entity)")
}
/// 身份 `a.entity` 可以学习：它不是只作标签用的标记。
pub(crate) fn learnable(a: &str) -> String {
    format!(
        "NOT EXISTS(
  SELECT 1
  FROM terms t
  JOIN term_tree l ON l.term_id=t.id
  WHERE t.key={a}.entity AND l.label_only=1)"
    )
}
/// 单人图上仍值得询问的原型提示，选取 `w`（`weak_claims`）的 `columns` 列。
pub(crate) fn proto_hints(columns: &str) -> String {
    format!(
        "SELECT {columns}
FROM weak_claims w
JOIN single_assets o ON o.uuid=w.uuid
WHERE w.source='PROTO'
  AND {unasked}
  AND {learnable}
  AND NOT EXISTS(SELECT 1 FROM memberships m WHERE m.uuid=w.uuid AND m.entity=w.entity)
  AND NOT EXISTS(SELECT 1 FROM assertions s WHERE s.uuid=w.uuid AND (s.status='confirmed' OR s.entity=w.entity))
  AND NOT EXISTS(SELECT 1 FROM excluded x WHERE x.uuid=w.uuid)",
        unasked = unasked("w"),
        learnable = learnable("w")
    )
}
/// 四个模块是否都已在核心里：返回一行，真或假。
pub(crate) const SWITCHED: &str = "
    SELECT count(*)=4
    FROM meta
    WHERE key IN ('terms_artists','terms_labels','terms_characters','terms_names') AND value='core'";
/// 图库采用 hybrid 身份策略（此时使用 `identity_nodes` 和身份读取）。
pub(crate) fn hybrid(c: &rusqlite::Connection) -> Result<bool> {
    use rusqlite::OptionalExtension;
    let policy = "SELECT value FROM meta WHERE key='identity_policy'";
    Ok(c.query_row(policy, [], |r| r.get::<_, String>(0)).optional()?.as_deref() == Some("hybrid"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragments_keep_the_text_each_reader_used() {
        assert_eq!(
            rating("m", "hide").unwrap(),
            " AND NOT coalesce(m.uuid IN (SELECT uuid FROM asset_rating \
             WHERE rating IN ('questionable', 'explicit')),0)"
        );
        assert_eq!(
            rating("a", "only").unwrap(),
            " AND coalesce(a.uuid IN (SELECT uuid FROM asset_rating WHERE rating IN ('questionable', 'explicit')),0)"
        );
        assert_eq!(rating("a", "").unwrap(), "");
        assert!(rating("a", "x").is_err());
        assert_eq!(
            visible("m"),
            "NOT EXISTS(SELECT 1 FROM duplicates h WHERE h.uuid=m.uuid AND h.hidden=1) \
             AND NOT EXISTS(SELECT 1 FROM excluded x WHERE x.uuid=m.uuid)"
        );
        assert_eq!(
            leads(),
            "(SELECT v.* FROM (SELECT uuid,entity,score,0 AS weak FROM family_evidence UNION ALL SELECT uuid,entity,\
             score,1 FROM weak_claims) v WHERE NOT EXISTS(SELECT 1 FROM assertions s WHERE s.uuid=v.uuid AND \
             s.entity=v.entity AND s.status='rejected'))"
        );
    }

    #[test]
    fn first_leads_prefer_strong_evidence_then_score_and_skip_rejections() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE family_evidence(uuid TEXT,entity TEXT,score REAL);
             CREATE TABLE weak_claims(uuid TEXT,entity TEXT,score REAL);
             CREATE TABLE assertions(uuid TEXT,entity TEXT,status TEXT);
             INSERT INTO family_evidence VALUES
                ('a','X',0.8),('a','Y',0.7),('b','X',0.95),('c','Y',0.98),('other','X',1.0);
             INSERT INTO weak_claims VALUES ('a','Y',0.99),('b','Y',0.9),('b','Z',0.8);
             INSERT INTO assertions VALUES ('b','X','rejected'),('c','Y','rejected');",
        )
        .unwrap();
        assert_eq!(
            first_leads(&c, r#"["a","b","c"]"#).unwrap(),
            vec![("a".into(), "X".into()), ("b".into(), "Y".into())]
        );
    }
}
