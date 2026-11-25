//! 角色扩展的有效证据批次，以及保留原归属的补充投影。
use crate::character_projection;
use crate::store::{self, Claim, NewEvidence, LIBRARY};
use crate::strict_json::sha256_hex;
use crate::{Error, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map};
use std::collections::{hash_map::Entry, BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Copy)]
pub(crate) enum Extension {
    Wd,
    Ccip,
}

impl Extension {
    fn evidence(self) -> (&'static str, &'static str) {
        match self {
            Self::Wd => (crate::wd_extension::SOURCE, crate::wd_extension::METHOD),
            Self::Ccip => (crate::ccip_longtail::SOURCE, crate::ccip_longtail::METHOD),
        }
    }
}

type Active = BTreeMap<i64, (String, BTreeSet<i64>)>;

fn current(c: &Connection, extension: Extension, scope: Option<&[String]>) -> Result<Active> {
    let (source, method) = extension.evidence();
    let mut found = Active::new();
    let mut stmt = c.prepare_cached(
        "SELECT a.id,a.key,r.evidence_id FROM term_active_evidence x
         JOIN term_assets a ON a.id=x.asset_id AND a.namespace='library'
         JOIN term_evidence_set_rows r ON r.set_id=x.set_id
         JOIN term_evidence e ON e.id=r.evidence_id AND e.asset_id=x.asset_id
         WHERE x.source=?1 AND x.method=?2
           AND (?3 IS NULL OR a.key IN (SELECT value FROM json_each(?3)))",
    )?;
    let scope = scope.map(|ids| json!(ids).to_string());
    let rows = stmt.query_map(params![source, method, scope], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
    })?;
    for row in rows {
        let (id, uuid, evidence) = row?;
        found.entry(id).or_insert_with(|| (uuid, BTreeSet::new())).1.insert(evidence);
    }
    Ok(found)
}

fn replace(c: &Connection, extension: Extension, producer: &str, old: &Active, new: &Active) -> Result<usize> {
    let changed: BTreeMap<i64, &String> = old
        .iter()
        .filter(|(id, rows)| new.get(id) != Some(*rows))
        .chain(new.iter().filter(|(id, rows)| old.get(id) != Some(*rows)))
        .map(|(id, (uuid, _))| (*id, uuid))
        .collect();
    if changed.is_empty() {
        return Ok(0);
    }
    let cover = changed.keys().copied().collect::<Vec<_>>();
    let rows = cover.iter().filter_map(|id| new.get(id)).flat_map(|(_, ids)| ids.iter().copied()).collect();
    store::publish_asset_batch(c, producer, extension.evidence(), &cover, &rows, None)?;
    let uuids = changed.into_values().cloned().collect::<Vec<_>>();
    character_projection::refresh_candidates(c, &uuids)?;
    character_projection::memberships(c, Some(&uuids))?;
    Ok(uuids.len())
}

pub(crate) fn apply<'a>(
    c: &Connection,
    extension: Extension,
    package: &str,
    scope: Option<&[String]>,
    hits: impl IntoIterator<Item = (&'a str, &'a str, f64, bool)>,
) -> Result<usize> {
    let (source, method) = extension.evidence();
    let metric = match extension {
        Extension::Wd => "wd_extension_probability",
        Extension::Ccip => "ccip_score",
    };
    let before = current(c, extension, scope)?;
    let in_scope = scope.map(|ids| ids.iter().map(String::as_str).collect::<BTreeSet<_>>());
    let mut after = Active::new();
    let mut terms = HashMap::new();
    let mut registered = false;
    for (uuid, tag, score, candidate_only) in hits {
        if in_scope.as_ref().is_some_and(|ids| !ids.contains(uuid)) {
            return Err(Error::invalid("角色扩展候选超出扫描范围"));
        }
        if !score.is_finite() || !(0.0..=1.0).contains(&score) {
            return Err(Error::invalid("角色扩展候选分数不合法"));
        }
        let live: bool =
            c.prepare_cached("SELECT EXISTS(SELECT 1 FROM assets WHERE uuid=?1)")?.query_row([uuid], |r| r.get(0))?;
        if !live {
            continue;
        }
        let asset = c
            .prepare_cached("SELECT id FROM term_assets WHERE namespace=?1 AND key=?2 AND removed_at IS NULL")?
            .query_row(params![LIBRARY, uuid], |r| r.get(0))
            .optional()?;
        let asset = match asset {
            Some(id) => id,
            None => {
                registered = true;
                store::register_asset(c, LIBRARY, uuid, None)?
            }
        };
        let (key, term) = &*match terms.entry(tag) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let key = crate::character_import::extension_entity(c, tag)?;
                let term: i64 =
                    c.prepare_cached("SELECT id FROM terms WHERE key=?")?.query_row([&key], |r| r.get(0))?;
                entry.insert((key, term))
            }
        };
        // 同一个包重跑同一输入复用证据行。底座重算后分数变化时记录新行。二级头（#602）的证据带 `tier: 2`，
        // 版本里也带上级别；一级头的证据和以前逐字节相同。
        let tier = if candidate_only { ":2" } else { "" };
        let fingerprint = sha256_hex(format!("{}:{:016x}{tier}", tag, score.to_bits()).as_bytes());
        let version = format!("{package}/{}", &fingerprint[..16]);
        let claim = Claim::Image { asset, term: *term, target: json!({"asset": [LIBRARY, uuid], "term": key}) };
        let mut data = Map::new();
        data.insert("tag".into(), json!(tag));
        if candidate_only {
            data.insert("tier".into(), json!(2));
        }
        store::add_evidence(
            c,
            &NewEvidence {
                claim: &claim,
                source,
                method,
                version: &version,
                score: Some((score, metric)),
                data: &data,
                observed_at: None,
                package: None,
            },
        )?;
        let evidence = c.prepare_cached(
            "SELECT id FROM term_evidence WHERE asset_id=?1 AND term_id=?2 AND source=?3 AND method=?4 AND version=?5",
        )?.query_row(params![asset, term, source, method, version], |r| r.get(0))?;
        after.entry(asset).or_insert_with(|| (uuid.to_owned(), BTreeSet::new())).1.insert(evidence);
    }
    if registered {
        store::rebuild_active_evidence(c)?;
    }
    if !terms.is_empty() && character_projection::policy(c)? {
        crate::character_graph::baseline(c)?;
    }
    replace(c, extension, &format!("extension:{package}"), &before, &after)
}

pub(crate) fn withdraw(c: &Connection, extension: Extension) -> Result<usize> {
    replace(c, extension, "extension:withdraw", &current(c, extension, None)?, &Active::new())
}

pub(crate) fn invalidate(c: &Connection, extension: Extension, uuids: &[String]) -> Result<()> {
    if !uuids.is_empty() {
        let old = current(c, extension, Some(uuids))?;
        let producer = match extension {
            Extension::Wd => "extension:stale_wd",
            Extension::Ccip => "extension:stale_ccip",
        };
        replace(c, extension, producer, &old, &Active::new())?;
    }
    Ok(())
}

// 撤销旧合并时还要考虑合并之后接入的扩展证据。按原身份读有效证据，避免用旧快照重新激活已撤回的候选。
pub(crate) fn images_of<'a>(c: &Connection, terms: impl Iterator<Item = &'a str>) -> Result<Vec<String>> {
    let mut images = BTreeSet::new();
    let mut stmt = c.prepare_cached(
        "SELECT a.key FROM terms t JOIN term_current_evidence e ON e.term_id=t.id
         JOIN term_assets a ON a.id=e.asset_id AND a.namespace='library'
         WHERE t.key=?1 AND (
            (e.source=?2 AND e.method=?3) OR (e.source=?4 AND e.method=?5))",
    )?;
    let wd = Extension::Wd.evidence();
    let ccip = Extension::Ccip.evidence();
    for term in terms {
        for row in stmt.query_map([term, wd.0, wd.1, ccip.0, ccip.1], |r| r.get::<_, String>(0))? {
            images.insert(row?);
        }
    }
    Ok(images.into_iter().collect())
}

pub(crate) fn fill_unassigned(c: &Connection, scoped: bool) -> Result<()> {
    let hybrid = character_projection::policy(c)?;
    let (entities, family, restrictions) = if hybrid {
        (
            "identity_nodes n ON n.id=v.entity",
            "n.character_id",
            // 图库里没有 effective_assertions 视图（它只在旧 Python 迁移 SQL 里），这里按它的定义直接查：
            // 人工否定落在这个角色本身（不是造型），且这条否定没有被挂起。
            "NOT EXISTS(SELECT 1 FROM assertions s JOIN identity_nodes x ON x.id=s.entity
                WHERE s.uuid=v.uuid AND x.character_id=n.character_id AND x.variant_id IS NULL AND s.status='rejected'
                  AND NOT EXISTS(SELECT 1 FROM identity_suspended h WHERE h.uuid=s.uuid AND h.entity=s.entity))
             AND NOT EXISTS(SELECT 1 FROM identity_uncertain u WHERE u.uuid=v.uuid)
             AND NOT EXISTS(SELECT 1 FROM identity_suspended h WHERE h.uuid=v.uuid)",
        )
    } else {
        (
            "terms t JOIN term_tree e ON t.id=e.term_id AND t.key=v.entity",
            "coalesce(e.parent_id,t.key)",
            "NOT EXISTS(SELECT 1 FROM assertions s WHERE s.uuid=v.uuid AND s.entity=coalesce(e.parent_id,t.key)
                         AND s.status='rejected')",
        )
    };
    let scope = if scoped { "AND v.uuid IN (SELECT uuid FROM touched)" } else { "" };
    // 二级头（#602）的候选不补归属：只有一级证据支持的 WD 扩展候选才能补。
    character_projection::assignable_extension(
        c,
        &format!(
            "SELECT DISTINCT v.uuid FROM candidates v WHERE v.source='WD_EXT' {scope}
               AND NOT EXISTS(SELECT 1 FROM memberships m WHERE m.uuid=v.uuid)"
        ),
    )?;
    // 基础候选与人工结论已投影完成。已有扩展来源优先，余下的按固定来源顺序取候选。
    // 分数只在同一来源的候选之间比较。
    c.execute(
        &format!(
            "WITH eligible AS (
               SELECT v.uuid,{family} AS entity,v.score,v.source,
                      row_number() OVER(PARTITION BY v.uuid
                        ORDER BY p.source IS v.source DESC,v.source='WD_EXT' DESC,v.score DESC,{family},v.entity) AS n
               FROM candidates v JOIN assets a ON a.uuid=v.uuid JOIN {entities}
               JOIN terms f ON f.key={family} JOIN term_tree r ON r.term_id=f.id AND r.mascot=0
               LEFT JOIN previous_extension_sources p ON p.uuid=v.uuid
               WHERE v.source IN ('WD_EXT','CCIP_LONGTAIL') {scope} AND {restrictions}
                 AND (v.source<>'WD_EXT' OR EXISTS(SELECT 1 FROM temp.assignable_extension x
                       WHERE x.uuid=v.uuid AND x.entity=v.entity))
                 AND NOT EXISTS(SELECT 1 FROM memberships m WHERE m.uuid=v.uuid)
                 AND NOT EXISTS(SELECT 1 FROM family_confirmed m WHERE m.uuid=v.uuid))
             INSERT INTO memberships
             SELECT uuid,entity,'machine','machine',source,score,1,0,printf('001%020.12f1',score)
             FROM eligible WHERE n=1"
        ),
        [],
    )?;
    Ok(())
}

/// 带上仅由扩展识别补主归属的造型。基础识别和人工归属继续使用既有的造型规则。
/// 查询和 `fill_unassigned` 用同一个已选来源、分数和并列顺序，且 WD 二级头不能补归属。
pub(crate) const VARIANT_MEMBERSHIPS: &str = "(SELECT uuid,entity,parent_id,variant,status FROM variant_memberships
    UNION ALL
    SELECT m.uuid,t.key,r.parent_id,r.variant,'machine'
    FROM memberships m
    JOIN terms t ON t.key=(
        SELECT v.entity FROM candidates v JOIN identity_nodes n ON n.id=v.entity
        WHERE v.uuid=m.uuid AND v.source=m.sources AND v.score=m.score AND n.character_id=m.entity
          AND (v.source<>'WD_EXT' OR EXISTS(
            SELECT 1 FROM term_assets a JOIN term_current_evidence e ON e.asset_id=a.id
            JOIN terms x ON x.id=e.term_id
            WHERE a.namespace='library' AND a.key=v.uuid AND e.source='wd_extension' AND e.method='linear_head'
              AND json_extract(e.data,'$.tier') IS NULL
              AND (x.key=v.entity OR EXISTS(SELECT 1 FROM identity_nodes original
                 WHERE original.id=x.key AND original.canonical_id=v.entity))))
        ORDER BY v.entity LIMIT 1
    )
    JOIN term_tree r ON r.term_id=t.id AND r.parent_id=m.entity
    WHERE m.sources IN ('WD_EXT','CCIP_LONGTAIL')
      AND NOT EXISTS(SELECT 1 FROM variant_memberships v WHERE v.uuid=m.uuid AND v.entity=t.key)
      AND NOT EXISTS(SELECT 1 FROM assertions s WHERE s.uuid=m.uuid AND s.entity=t.key AND s.status='rejected'))";
