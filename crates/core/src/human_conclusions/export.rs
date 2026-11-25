//! 一致读事务中的人工状态投影；只读取白名单列，不打开原图或导出机器载荷。
use super::{
    format::*,
    matching::SourceHash,
    state::{self, Domain, State},
};
use crate::{error::invalid, Error, Result};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const LATEST: &str = "WITH latest AS (
 SELECT v.*,row_number() OVER (PARTITION BY v.asset_id,v.term_id,v.relation_id
  ORDER BY v.anchor DESC,v.batch_id IS NOT NULL DESC,v.id DESC) AS rank
 FROM term_verdicts v LEFT JOIN actions a ON a.id=v.action_id
 WHERE v.batch_id IS NOT NULL OR a.undone=0)
";

/// 从调用方持有的一致读事务生成当前状态；内容摘要暂记 missing，由宿主核验后填入。
/// 图库显示名、路径、设置、整段历史和机器证据不会进入结果。
pub fn capture(c: &Connection, header: Header) -> Result<Snapshot> {
    if c.is_autocommit() {
        return invalid("人工快照需要一致读事务");
    }
    crate::schema::require_current(c)?;
    let policy = if crate::character_projection::policy(c)? { Policy::Hybrid } else { Policy::Default };
    if header.policy != policy {
        return invalid("快照策略与读取中的图库不一致");
    }
    let mut records = Vec::new();
    image_verdicts(c, &mut records)?;
    relation_verdicts(c, &mut records)?;
    simple_states(c, &mut records)?;
    contexts(c, &mut records)?;
    reviews_and_skips(c, &mut records)?;
    kept_apart(c, &mut records)?;
    anchors(c, &mut records)?;
    let mut terms: BTreeSet<String> = records.iter().flat_map(|r| r.terms().into_iter().map(str::to_owned)).collect();
    for record in &records {
        if let Record::IdentitySkip { pair, .. } = record {
            if let Some((a, b)) = super::skips::pair(pair) {
                let found: i64 =
                    c.query_row("SELECT count(*) FROM terms WHERE key IN (?1,?2)", [a, b], |r| r.get(0))?;
                if found == 2 {
                    terms.extend([a.to_owned(), b.to_owned()]);
                }
            }
        }
    }
    // 旧结构没有逐字段来源。完整保留其现状并标 legacy_unknown，不能因缺少图片而丢掉 OC 或人工作品编辑。
    for key in c
        .prepare(
            "SELECT key FROM terms WHERE (origin='user' AND kind<>'mark') OR id IN (SELECT term_id FROM term_tree)
        OR id IN (SELECT term_id FROM term_names WHERE source='user' AND (role='alias' OR preferred=1))
        OR id IN (SELECT term_id FROM term_links WHERE source='user')",
        )?
        .query_map([], |r| r.get::<_, String>(0))?
    {
        terms.insert(key?);
    }
    let aliases = merge_aliases(c)?;
    let portable = crate::character_import::portable_trees(c)?;
    terms.extend(aliases.keys().cloned());
    let mut visited = BTreeSet::new();
    while let Some(key) = terms.pop_first() {
        if !visited.insert(key.clone()) {
            continue;
        }
        let definition = entity(c, &key, policy, aliases.get(&key), portable.get(&key))?;
        let record = Record::Entity { key, definition: Box::new(definition) };
        terms.extend(record.terms().into_iter().filter(|key| !visited.contains(*key)).map(str::to_owned));
        records.push(record);
    }
    let assets: BTreeSet<String> = records
        .iter()
        .filter_map(|r| match r {
            Record::ImageVerdict { asset, .. }
            | Record::IdentityContext { asset, .. }
            | Record::Curation { asset, .. }
            | Record::Deferred { asset, .. }
            | Record::Uncertain { asset, .. } => Some(asset.clone()),
            _ => None,
        })
        .collect();
    records.extend(assets.into_iter().map(|key| Record::Asset { key, hash: SourceHash::Missing }));
    records.sort_by(|a, b| a.identity().cmp(&b.identity()));
    let snapshot = Snapshot { header, records };
    snapshot.validate()?;
    Ok(snapshot)
}

// 复合逻辑键编码为稳定摘要，避免分隔符碰撞，也不把源数据库整数行号当作身份。
fn key(parts: &[&str]) -> String {
    crate::strict_json::sha256_hex(serde_json::to_string(parts).expect("字符串数组可序列化").as_bytes())
}
fn strings(encoded: &str) -> Result<Vec<String>> {
    serde_json::from_value(crate::strict_json::parse(encoded).map_err(Error::invalid)?)
        .map_err(|_| Error::invalid("人工状态中的身份列表不正确"))
}
fn decision(r: &rusqlite::Row<'_>, start: usize) -> rusqlite::Result<Verdict> {
    Ok(Verdict {
        verdict: r.get(start)?,
        origin: r.get(start + 1)?,
        via: r.get(start + 2)?,
        reason: r.get(start + 3)?,
        created_at: r.get(start + 4)?,
    })
}
fn image_verdicts(c: &Connection, out: &mut Vec<Record>) -> Result<()> {
    let sql = format!(
        "{LATEST} SELECT a.key,t.key,v.verdict,v.origin,v.via,v.reason,v.created_at
        FROM latest v JOIN term_assets a ON a.id=v.asset_id JOIN terms t ON t.id=v.term_id
        WHERE v.rank=1 AND a.namespace='library' ORDER BY a.key,t.key"
    );
    for row in c.prepare(&sql)?.query_map([], |r| {
        let (asset, term): (String, String) = (r.get(0)?, r.get(1)?);
        Ok(Record::ImageVerdict { key: key(&[&asset, &term]), asset, term, decision: decision(r, 2)? })
    })? {
        out.push(row?);
    }
    Ok(())
}
fn relation_verdicts(c: &Connection, out: &mut Vec<Record>) -> Result<()> {
    let sql = format!(
        "{LATEST} SELECT r.id,a.key,b.key,r.kind,v.verdict,v.origin,v.via,v.reason,v.created_at,v.reviewed,m.legacy_id,v.id
        FROM latest v JOIN term_relations r ON r.id=v.relation_id JOIN terms a ON a.id=r.a JOIN terms b ON b.id=r.b
        LEFT JOIN term_legacy_ids m ON m.kind='relation' AND m.core_id=r.id
        WHERE v.rank=1 ORDER BY m.legacy_id IS NULL,m.legacy_id,r.id"
    );
    let mut evidence = c.prepare(
        "SELECT id,source,method,lower(hex(fingerprint)),json_extract(data,'$.legacy_fingerprint')
        FROM term_evidence WHERE relation_id=?1 ORDER BY id",
    )?;
    for (order, row) in c
        .prepare(&sql)?
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                decision(r, 4)?,
                r.get::<_, String>(9)?,
                r.get::<_, Option<i64>>(10)?,
                r.get::<_, i64>(11)?,
            ))
        })?
        .enumerate()
    {
        let (id, mut a, mut b, kind, decision, reviewed, legacy, verdict) = row?;
        if matches!(kind.as_str(), "different_from" | "excludes") && a > b {
            std::mem::swap(&mut a, &mut b);
        }
        let reviewed: BTreeSet<i64> =
            serde_json::from_str(&reviewed).map_err(|_| Error::invalid("关系审阅列表不正确"))?;
        let (mut seen, mut acknowledged) = super::relations::context(c, verdict)?;
        for row in evidence.query_map([id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })? {
            let (evidence, source, method, fingerprint, legacy) = row?;
            let fingerprint = if source == "legacy-hierarchy" {
                legacy
                    .map(|v| format!("legacy-hierarchy:{v}"))
                    .ok_or_else(|| Error::invalid("旧层级证据缺少稳定指纹"))?
            } else {
                format!("evidence:{}", key(&[&source, &method, &fingerprint]))
            };
            if reviewed.contains(&evidence) {
                acknowledged.insert(fingerprint.clone());
            }
            seen.insert(fingerprint);
        }
        out.push(Record::RelationVerdict {
            key: key(&[&a, &b, &kind]),
            a,
            b,
            kind,
            order: i64::try_from(order).map_err(|_| Error::invalid("关系顺序超出格式范围"))?,
            identity_relation: legacy.is_some(),
            decision,
            seen: seen.into_iter().collect(),
            reviewed: acknowledged.into_iter().collect(),
        });
    }
    Ok(())
}
fn simple_states(c: &Connection, out: &mut Vec<Record>) -> Result<()> {
    for (domain, table) in
        [(Domain::Curation, "curation"), (Domain::Deferred, "deferred"), (Domain::Uncertain, "identity_uncertain")]
    {
        let sql = format!(
            "SELECT uuid FROM {table} UNION SELECT asset_key FROM human_state_marks WHERE domain=?1 ORDER BY 1"
        );
        let keys: Vec<String> =
            c.prepare(&sql)?.query_map([domain.key()], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        for asset in keys {
            let value = state::read(c, domain, &asset)?;
            let record = match (domain, value) {
                (Domain::Curation, State::Positive(v)) => Record::Curation {
                    key: asset.clone(),
                    asset,
                    state: text(&v, "status")?,
                    origin: text(&v, "origin")?,
                    created_at: text(&v, "created_at")?,
                },
                (Domain::Deferred, State::Positive(_)) => Record::Deferred {
                    key: asset.clone(),
                    asset,
                    state: "deferred".into(),
                    origin: "legacy".into(),
                    created_at: None,
                },
                (Domain::Uncertain, State::Positive(v)) => Record::Uncertain {
                    key: asset.clone(),
                    asset,
                    state: "uncertain".into(),
                    options: serde_json::from_value(v["options"].clone())
                        .map_err(|_| Error::invalid("未定候选不正确"))?,
                    reason: text(&v, "reason")?,
                    origin: "legacy".into(),
                    created_at: text(&v, "created_at")?,
                },
                (Domain::Curation, State::Absent(m)) => Record::Curation {
                    key: asset.clone(),
                    asset,
                    state: "reset".into(),
                    origin: m.origin,
                    created_at: m.created_at,
                },
                (Domain::Deferred, State::Absent(m)) => Record::Deferred {
                    key: asset.clone(),
                    asset,
                    state: "resumed".into(),
                    origin: m.origin,
                    created_at: Some(m.created_at),
                },
                (Domain::Uncertain, State::Absent(m)) => Record::Uncertain {
                    key: asset.clone(),
                    asset,
                    state: "cleared".into(),
                    options: vec![],
                    reason: String::new(),
                    origin: m.origin,
                    created_at: m.created_at,
                },
                (_, State::Undecided) => continue,
            };
            out.push(record);
        }
    }
    Ok(())
}
fn text(v: &Value, field: &str) -> Result<String> {
    v[field].as_str().map(str::to_owned).ok_or_else(|| Error::invalid("人工状态字段不正确"))
}
fn contexts(c: &Connection, out: &mut Vec<Record>) -> Result<()> {
    let sql = "WITH pairs AS (SELECT uuid,entity FROM identity_scopes UNION SELECT uuid,entity FROM identity_suspended)
        SELECT p.uuid,p.entity,s.members_json,x.reason,x.options_json FROM pairs p
        LEFT JOIN identity_scopes s ON s.uuid=p.uuid AND s.entity=p.entity
        LEFT JOIN identity_suspended x ON x.uuid=p.uuid AND x.entity=p.entity ORDER BY p.uuid,p.entity";
    for row in c.prepare(sql)?.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
        ))
    })? {
        let (asset, term, scope, reason, options) = row?;
        let suspended = match (reason, options) {
            (Some(reason), Some(options)) => Some(Suspended { reason, options: strings(&options)? }),
            (None, None) => None,
            _ => return invalid("暂停状态不完整"),
        };
        out.push(Record::IdentityContext {
            key: key(&[&asset, &term]),
            asset,
            term,
            scope: scope.as_deref().map(strings).transpose()?,
            suspended,
        });
    }
    Ok(())
}
fn reviews_and_skips(c: &Connection, out: &mut Vec<Record>) -> Result<()> {
    let sql = "SELECT entity,max(reviewed_at) FROM reviews r LEFT JOIN actions a ON a.id=r.action_id
        WHERE a.undone=0 OR a.id IS NULL GROUP BY entity ORDER BY entity";
    for row in c.prepare(sql)?.query_map([], |r| {
        let term: String = r.get(0)?;
        Ok(Record::Review { key: term.clone(), term, reviewed_at: r.get(1)? })
    })? {
        out.push(row?);
    }
    let skips: Option<String> =
        c.query_row("SELECT value FROM prefs WHERE key='identity_skipped'", [], |r| r.get(0)).optional()?;
    if let Some(skips) = skips {
        for pair in strings(&skips)?.into_iter().collect::<BTreeSet<_>>() {
            out.push(Record::IdentitySkip { key: key(&[&pair]), pair });
        }
    }
    Ok(())
}
fn kept_apart(c: &Connection, out: &mut Vec<Record>) -> Result<()> {
    let mut pairs = crate::character_identity::kept_apart_pairs(c)?;
    for (a, b) in crate::character_import::declined_pairs(c)? {
        if a != b {
            pairs.insert(if a < b { (a, b) } else { (b, a) });
        }
    }
    let mut exists = c.prepare_cached("SELECT count(*)=2 FROM terms WHERE key IN (?1,?2)")?;
    for (a, b) in pairs {
        if exists.query_row([&a, &b], |r| r.get::<_, bool>(0))? {
            out.push(Record::IdentityKeepApart { key: key(&[&a, &b]), a, b });
        }
    }
    Ok(())
}

fn anchors(c: &Connection, out: &mut Vec<Record>) -> Result<()> {
    // 旧库只保存 reference，没有当时参考库的版本证明。保留原决定，但不得伪造可自动恢复的参考版本。
    for row in c.prepare("SELECT entity,reference,decision,reason,created_at FROM identity_anchor_decisions ORDER BY entity,reference")?
        .query_map([],|r| {let (term,reference):(String,String)=(r.get(0)?,r.get(1)?); Ok(Record::AnchorDecision {key:key(&[&term,&reference]),term,reference,reference_version:None,decision:r.get(2)?,reason:r.get(3)?,created_at:r.get(4)?})})? {out.push(row?);}
    Ok(())
}

// 完整的默认策略合并载荷才证明非 user 来源的别名来自人工合并；片段或未来形状不升格为人工来源。
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyMerge {
    source: String,
    target: String,
    moved: Vec<(String, String, String)>,
    kept: Vec<(String, String, String)>,
    candidates: Vec<(String, String, f64)>,
    target_prev: Vec<(String, String, f64)>,
    variants: Vec<String>,
    aliases: Vec<LegacyAlias>,
    pins: Option<String>,
    reviews: Vec<i64>,
    #[serde(default)]
    review_conflicts: Vec<(i64, String, String)>,
    uuids: Vec<String>,
}
impl LegacyMerge {
    fn valid(&self) -> bool {
        let key = |s: &str| !s.is_empty() && s.chars().count() <= 256;
        self.source != self.target
            && key(&self.source)
            && key(&self.target)
            && self
                .moved
                .iter()
                .chain(&self.kept)
                .all(|(uuid, status, _)| key(uuid) && matches!(status.as_str(), "confirmed" | "rejected"))
            && self
                .candidates
                .iter()
                .chain(&self.target_prev)
                .all(|(uuid, source, score)| key(uuid) && !source.is_empty() && score.is_finite())
            && self.variants.iter().chain(&self.uuids).all(|s| key(s))
            && self.aliases.iter().all(|alias| {
                let (lang, value, source) = alias.parts();
                matches!(lang, "" | "zh" | "ja" | "en") && !value.is_empty() && !source.is_empty()
            })
            && self.reviews.iter().all(|id| *id > 0)
            && self
                .review_conflicts
                .iter()
                .all(|(id, a, b)| self.reviews.contains(id) && !a.is_empty() && !b.is_empty())
            && self.pins.as_ref().is_none_or(|s| strings(s).is_ok())
    }
}
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum LegacyAlias {
    Fields((String, String, String)),
    Text(String),
}
impl LegacyAlias {
    fn parts(&self) -> (&str, &str, &str) {
        match self {
            Self::Fields((lang, value, source)) => (lang, value, source),
            Self::Text(value) => ("", value, "legacy"),
        }
    }
}
type AliasNames = BTreeMap<(String, String, String), String>;
type Aliases = BTreeMap<String, AliasNames>;
fn record_alias(out: &mut Aliases, key: &str, lang: &str, value: &str, source: &str, provenance: &str) {
    // Danbooru 更新器整批删建同来源别名，行号/seq 可复用，历史同字面值不足以证明当前行仍属那次合并。
    let provenance =
        if source == "danbooru" || provenance == "legacy_unknown" { "legacy_unknown" } else { "legacy_merge" };
    out.entry(key.into())
        .or_default()
        .entry((lang.into(), value.into(), source.into()))
        .and_modify(|old| {
            if provenance == "legacy_unknown" {
                *old = provenance.into();
            }
        })
        .or_insert_with(|| provenance.into());
}
fn merge_aliases(c: &Connection) -> Result<Aliases> {
    let mut result = Aliases::new();
    // 便携导入声明由有效 action 持有；来源不明的别名继续保持来源不明。
    for row in c
        .prepare(
            "SELECT t.key,o.definition_json FROM human_object_contributions o
        JOIN actions a ON a.id=o.owner_action_id JOIN terms t ON t.id=o.object_id
        WHERE o.kind='term' AND a.undone=0",
        )?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
    {
        let (key, encoded) = row?;
        let definition: Entity =
            serde_json::from_str(&encoded).map_err(|_| Error::invalid("导入定义的来源记录不完整"))?;
        for name in definition
            .names
            .into_iter()
            .filter(|n| matches!(n.provenance.as_str(), "legacy_merge" | "legacy_unknown") && n.role == "alias")
        {
            record_alias(&mut result, &key, &name.lang, &name.value, &name.source, &name.provenance);
        }
    }
    for row in c
        .prepare("SELECT before_json FROM actions WHERE undone=0 ORDER BY id")?
        .query_map([], |r| r.get::<_, String>(0))?
    {
        let Ok(body) = crate::strict_json::parse(&row?) else { continue };
        for (term, name) in super::import::name_aliases(&body)? {
            if let Some(key) =
                c.query_row("SELECT key FROM terms WHERE id=?1", [term], |r| r.get::<_, String>(0)).optional()?
            {
                record_alias(&mut result, &key, &name.lang, &name.value, &name.source, &name.provenance);
            }
        }
        let Ok(Some(items)) = crate::character_commands::owns(&body) else { continue };
        if items.len() != 1
            || body[0].get("terms").is_some_and(|v| v.as_object().is_none_or(|o| o.len() != 3) || v["type"] != "merge")
        {
            continue;
        }
        for item in items {
            if item.as_object().is_none_or(|o| o.len() != 1) {
                continue;
            }
            let Ok(m) = serde_json::from_value::<LegacyMerge>(item["merge"].clone()) else { continue };
            if !m.valid() {
                continue;
            }
            let (source, target, aliases) = (m.source.as_str(), m.target.as_str(), &m.aliases);
            let effective:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM term_tree r JOIN terms t ON t.id=r.term_id WHERE t.key=?1 AND r.merged_into=?2)",[source,target],|r|r.get(0))?;
            if !effective {
                continue;
            }
            for alias in aliases {
                let (lang, value, source) = alias.parts();
                record_alias(&mut result, target, lang, value, source, "legacy_merge");
            }
        }
    }
    Ok(result)
}
fn entity(
    c: &Connection,
    key: &str,
    policy: Policy,
    aliases: Option<&AliasNames>,
    portable: Option<&crate::character_import::PortableTree>,
) -> Result<Entity> {
    let (id, mut kind, origin): (i64, String, String) =
        c.query_row("SELECT id,kind,origin FROM terms WHERE key=?1", [key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    let mut tree=c.query_row("SELECT parent_id,merged_into,work_id,work_text,variant,artist,label_only,mascot,mascot_origin,original,parent_locked,review_status,review_note,seq FROM term_tree WHERE term_id=?1",[id],|r|Ok(Tree {
        parent:r.get(0)?,merged_into:r.get(1)?,work:r.get(2)?,work_text:r.get(3)?,variant:r.get(4)?,artist:r.get(5)?,label_only:r.get(6)?,mascot:r.get(7)?,mascot_origin:r.get(8)?,original:r.get(9)?,parent_locked:r.get(10)?,review_status:r.get(11)?,review_note:r.get(12)?,order:r.get(13)?,
    })).optional()?;
    let mut baseline = if policy == Policy::Hybrid {
        c.query_row(
            "SELECT parent_id,variant,kind,merged_into,seq FROM term_identity_baseline WHERE id=?1",
            [key],
            |r| {
                Ok(Baseline {
                    parent: r.get(0)?,
                    variant: r.get(1)?,
                    kind: r.get(2)?,
                    merged_into: r.get(3)?,
                    order: r.get(4)?,
                })
            },
        )
        .optional()?
    } else {
        None
    };
    if let Some(portable) = portable {
        if let Some(tree) = &mut tree {
            if portable.merged {
                tree.merged_into = None;
            }
            if portable.attached {
                kind = portable.kind.clone().unwrap_or_else(|| "character".into());
                tree.variant = portable.variant.clone().unwrap_or_default();
                tree.work = portable.work.clone();
                tree.parent = None;
            }
            if portable.group {
                tree.label_only = false;
            }
        }
        if let Some(baseline) = &mut baseline {
            if portable.merged {
                baseline.merged_into = None;
            }
            if portable.attached {
                baseline.parent = None;
                baseline.variant = portable.variant.clone();
                baseline.kind = Some(kind.clone());
            }
        }
    }
    let mut names = Vec::new();
    let mut description = None;
    for row in c.prepare("SELECT lang,value,role,preferred,position,source FROM term_names WHERE term_id=?1 ORDER BY seq,lang,role,source,value")?
        .query_map([id],|r|Ok(Name {lang:r.get(0)?,value:r.get(1)?,role:r.get(2)?,preferred:r.get(3)?,position:r.get(4)?,source:r.get(5)?,provenance:String::new()}))? {
        let mut name=row?;
        let proof=aliases.and_then(|a|a.get(&(name.lang.clone(),name.value.clone(),name.source.clone()))).filter(|_|name.role=="alias");
        if proof.is_some_and(|p|p=="legacy_unknown") {
            name.provenance="legacy_unknown".into();names.push(name);
        } else if name.source=="user" {
            // 导入包也能声明 source=user；只有人工入口写出的别名或首选显示名能证明人工来源。
            name.provenance=if name.role=="alias" || name.preferred {"user"} else {"legacy_unknown"}.into();
            names.push(name);
        }
        else if let Some(proof)=proof {
            name.provenance=proof.clone();names.push(name);
        } else if description.is_none() && name.role=="display" {
            name.provenance="description".into();name.preferred=false;description=Some(name);
        }
    }
    if names.is_empty() {
        names.extend(description);
    }
    let links = c
        .prepare("SELECT system,key,current,added_at,source FROM term_links WHERE term_id=?1 ORDER BY system,key")?
        .query_map([id], |r| {
            Ok(Link { system: r.get(0)?, key: r.get(1)?, current: r.get(2)?, added_at: r.get(3)?, source: r.get(4)? })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let provenance = if tree.is_some() {
        "legacy_unknown"
    } else if origin == "user" {
        "user"
    } else {
        "dependency"
    }
    .into();
    Ok(Entity { kind, origin, provenance, tree, baseline, names, links })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;
    fn fixture() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        crate::configure(&c).unwrap();
        c.execute_batch("CREATE TABLE curation(uuid TEXT PRIMARY KEY,status TEXT,origin TEXT,created_at TEXT);
            CREATE TABLE deferred(uuid TEXT PRIMARY KEY);
            CREATE TABLE identity_uncertain(uuid TEXT PRIMARY KEY,options_json TEXT,reason TEXT,created_at TEXT);
            CREATE TABLE identity_scopes(uuid TEXT,entity TEXT,members_json TEXT,PRIMARY KEY(uuid,entity));
            CREATE TABLE identity_suspended(uuid TEXT,entity TEXT,reason TEXT,options_json TEXT,PRIMARY KEY(uuid,entity));
            CREATE TABLE reviews(entity TEXT,reviewed_at TEXT,action_id INTEGER);
            CREATE TABLE prefs(key TEXT PRIMARY KEY,value TEXT);
            CREATE TABLE identity_anchor_decisions(entity TEXT,reference TEXT,decision TEXT,reason TEXT,created_at TEXT);").unwrap();
        crate::schema_rules::migrate_to(&mut c, schema::SCHEMA_VERSION).unwrap();
        c
    }
    fn header(policy: Policy) -> Header {
        Header::new("synthetic-snapshot".into(), "synthetic-library".into(), "2026-11-15T00:00:00Z".into(), policy)
    }
    fn wire(snapshot: &Snapshot) -> String {
        let mut bytes = Vec::new();
        snapshot.write(&mut bytes).unwrap();
        let parsed = read(&mut std::io::Cursor::new(&bytes)).unwrap();
        assert_eq!(parsed.semantic_sha256().unwrap(), snapshot.semantic_sha256().unwrap());
        String::from_utf8(bytes).unwrap()
    }
    #[test]
    fn latest_includes_clear_batch_precedence_undo_and_removed_assets_without_private_data() {
        let mut c = fixture();
        let tx = c.transaction().unwrap();
        schema::begin_writing(&tx).unwrap();
        tx.execute_batch("INSERT INTO terms(id,key,kind,origin) VALUES (1,'t','concept','user');
            INSERT INTO term_assets(id,namespace,key,removed_at) VALUES (1,'library','removed','2026-11-01');
            INSERT INTO actions(id,label,before_json,undone) VALUES(1,'synthetic','[]',0),(2,'undone','[]',1);
            INSERT INTO term_verdicts(action_id,asset_id,term_id,verdict,origin,anchor) VALUES(1,1,1,'yes','user',1),(2,1,1,'no','user',2);
            INSERT INTO term_batches(id,kind,anchor) VALUES(1,'album',1);
            INSERT INTO term_verdicts(batch_id,asset_id,term_id,verdict,origin,anchor,via,reason,created_at) VALUES(1,1,1,'clear','album',1,'album','source reason','2026-10-01');
            INSERT INTO prefs VALUES('pins','private-setting'),('secret','never-export');
            INSERT INTO term_evidence(asset_id,term_id,source,method,version,data,fingerprint) VALUES(1,1,'wd','synthetic','1','{\"private\":\"machine-payload\"}',zeroblob(16));").unwrap();
        schema::commit_writing(tx).unwrap();
        assert!(capture(&c, header(Policy::Default)).is_err());
        let tx = c.transaction().unwrap();
        let snapshot = capture(&tx, header(Policy::Default)).unwrap();
        assert_eq!(snapshot.asset_keys().collect::<Vec<_>>(), vec!["removed"]);
        let verdicts: Vec<_> = snapshot
            .records
            .iter()
            .filter_map(|r| match r {
                Record::ImageVerdict { decision, .. } => Some(decision),
                _ => None,
            })
            .collect();
        assert_eq!(verdicts.len(), 1);
        assert_eq!(verdicts[0].verdict, "clear");
        assert_eq!(verdicts[0].origin, "album");
        assert_eq!(verdicts[0].created_at, "2026-10-01");
        let encoded = wire(&snapshot);
        for forbidden in ["private-setting", "never-export", "machine-payload", "before_json", "action_id"] {
            assert!(!encoded.contains(forbidden));
        }
        assert_eq!(snapshot.coverage().missing, 1);
    }
    #[test]
    fn independent_definitions_context_absence_reviews_and_opaque_skips_round_trip() {
        let mut c = fixture();
        let tx = c.transaction().unwrap();
        schema::begin_writing(&tx).unwrap();
        tx.execute("INSERT INTO meta VALUES ('identity_policy','hybrid')", []).unwrap();
        for (key, origin) in [("oc", "user"), ("parent", "machine")] {
            crate::character_entities::create(
                &tx,
                &crate::character_entities::New { id: key, kind: "character", original: 1, ..Default::default() },
                origin,
            )
            .unwrap();
        }
        tx.execute_batch("INSERT INTO term_identity_baseline(seq,id,parent_id) VALUES(9,'oc','parent'),(10,'parent',NULL);
            UPDATE term_tree SET parent_id='oc' WHERE term_id=(SELECT id FROM terms WHERE key='parent');
            INSERT INTO term_names(term_id,lang,value,role,source,preferred,seq) SELECT id,'zh','人工名字','display','user',1,1 FROM terms WHERE key='oc';
            INSERT INTO term_names(term_id,lang,value,role,source,preferred,seq) SELECT id,'zh','依赖描述','display','provider',1,2 FROM terms WHERE key='parent';
            INSERT INTO term_links(system,key,term_id,source,current) SELECT 'public','synthetic-link',id,'user',0 FROM terms WHERE key='oc';
            INSERT INTO actions(id,label,before_json) VALUES(1,'reset','[]'),(2,'sweep','[]');
            INSERT INTO reviews VALUES('oc','2026-10-01',1),('oc','2026-11-01',2);
            INSERT INTO identity_suspended VALUES('missing-source','oc','split','[\"oc\",\"parent\"]');
            INSERT INTO identity_uncertain VALUES('uncertain','[\"oc\",\"parent\"]','需要核实','2026-10-02');
            INSERT INTO prefs VALUES('identity_skipped','[\"old|unresolved|key\"]'),('pins','private');
            INSERT INTO identity_anchor_decisions VALUES('oc','synthetic-reference','exclude','old reason','2026-10-03');").unwrap();
        state::absent(&tx, Domain::Curation, "reset", 1).unwrap();
        state::absent(&tx, Domain::Deferred, "resumed", 1).unwrap();
        state::absent(&tx, Domain::Uncertain, "cleared", 1).unwrap();
        schema::commit_writing(tx).unwrap();
        let tx = c.transaction().unwrap();
        let snapshot = capture(&tx, header(Policy::Hybrid)).unwrap();
        assert!(snapshot
            .records
            .iter()
            .any(|r| matches!(r, Record::IdentityContext { scope: None, suspended: Some(_), .. })));
        assert!(snapshot
            .records
            .iter()
            .any(|r| matches!(r,Record::Review {reviewed_at,..} if reviewed_at=="2026-11-01")));
        assert!(snapshot.records.iter().any(|r| matches!(r, Record::AnchorDecision { reference_version: None, .. })));
        assert!(snapshot
            .records
            .iter()
            .any(|r| matches!(r,Record::IdentitySkip {pair,..} if pair=="old|unresolved|key")));
        let definitions: Vec<_> = snapshot
            .records
            .iter()
            .filter_map(|r| match r {
                Record::Entity { key, definition } => Some((key, definition)),
                _ => None,
            })
            .collect();
        assert_eq!(definitions.len(), 2);
        let oc = definitions.iter().find(|(k, _)| k.as_str() == "oc").unwrap().1;
        assert_eq!(oc.provenance, "legacy_unknown");
        assert_eq!(oc.baseline.as_ref().unwrap().order, 9);
        assert!(!oc.links[0].current);
        assert_eq!(oc.names[0].provenance, "user");
        let parent = definitions.iter().find(|(k, _)| k.as_str() == "parent").unwrap().1;
        assert_eq!(parent.names[0].provenance, "description");
        assert!(!parent.names[0].preferred);
        let encoded = wire(&snapshot);
        assert!(!encoded.contains("private"));
        for state in ["reset", "resumed", "cleared", "uncertain"] {
            assert!(encoded.contains(&format!("\"state\":\"{state}\"")));
        }
    }
    #[test]
    fn relation_review_context_uses_stable_fingerprints_and_no_evidence_payload() {
        let mut c = fixture();
        let tx = c.transaction().unwrap();
        schema::begin_writing(&tx).unwrap();
        tx.execute_batch("INSERT INTO terms(id,key,kind,origin) VALUES (1,'a','character','legacy'),(2,'b','character','legacy');
            INSERT INTO term_relations(id,a,b,kind) VALUES(1,1,2,'contains');
            INSERT INTO actions(id,label,before_json) VALUES(1,'decision','[]');
            INSERT INTO term_evidence(id,relation_id,source,method,version,fingerprint,data) VALUES
            (1,1,'legacy-hierarchy','identity_evidence','v1',zeroblob(16),'{\"legacy_fingerprint\":\"old-fingerprint\",\"payload\":\"private-evidence\"}'),
            (2,1,'provider','other','v2',zeroblob(16),'{}');
            INSERT INTO term_verdicts(action_id,relation_id,verdict,origin,anchor,reviewed) VALUES(1,1,'yes','user',1,'[1]');").unwrap();
        schema::commit_writing(tx).unwrap();
        let tx = c.transaction().unwrap();
        let snapshot = capture(&tx, header(Policy::Default)).unwrap();
        let (seen, reviewed) = snapshot
            .records
            .iter()
            .find_map(|r| match r {
                Record::RelationVerdict { seen, reviewed, .. } => Some((seen, reviewed)),
                _ => None,
            })
            .unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(reviewed, &vec!["legacy-hierarchy:old-fingerprint".to_owned()]);
        assert!(!wire(&snapshot).contains("private-evidence"));
    }
    #[test]
    fn merge_aliases_require_a_complete_active_payload_and_current_name() {
        let mut c = fixture();
        let tx = c.transaction().unwrap();
        schema::begin_writing(&tx).unwrap();
        for key in ["a", "b", "c"] {
            crate::character_entities::create(
                &tx,
                &crate::character_entities::New { id: key, kind: "character", ..Default::default() },
                "legacy",
            )
            .unwrap();
        }
        tx.execute_batch("UPDATE term_tree SET merged_into='b' WHERE term_id IN (SELECT id FROM terms WHERE key IN ('a','c'));
            INSERT INTO term_names(term_id,lang,value,role,source,seq) SELECT id,'','更早的合并别名','alias','legacy',4 FROM terms WHERE key='b';
            INSERT INTO term_names(term_id,lang,value,role,source,seq) SELECT id,'zh','真实合并别名','alias','bangumi',1 FROM terms WHERE key='b';
            INSERT INTO term_names(term_id,lang,value,role,source,seq) SELECT id,'zh','片段伪装别名','alias','bangumi',2 FROM terms WHERE key='b';
            INSERT INTO term_names(term_id,lang,value,role,source,seq) SELECT id,'zh','未生效别名','alias','bangumi',3 FROM terms WHERE key='b';").unwrap();
        let merge = serde_json::json!({"source":"a","target":"b","moved":[],"kept":[],"candidates":[],"target_prev":[],"variants":[],"aliases":[["zh","真实合并别名","bangumi"],"更早的合并别名"],"pins":null,"reviews":[],"uuids":[]});
        for (id, m) in [
            (1, merge.clone()),
            (2, serde_json::json!({"source":"c","target":"b","aliases":[["zh","片段伪装别名","bangumi"]]})),
            (3, merge),
        ] {
            let mut m = m;
            if id == 3 {
                m["aliases"] = serde_json::json!([["zh", "未生效别名", "bangumi"]]);
            }
            let body = serde_json::json!([{"terms":{"v":2,"type":"merge","before":[{"merge":m}]}}]).to_string();
            tx.execute(
                "INSERT INTO actions(id,label,before_json,undone) VALUES (?1,'synthetic',?2,?3)",
                rusqlite::params![id, body, id == 3],
            )
            .unwrap();
        }
        schema::commit_writing(tx).unwrap();
        let tx = c.transaction().unwrap();
        let snapshot = capture(&tx, header(Policy::Default)).unwrap();
        let names = snapshot
            .records
            .iter()
            .find_map(|r| match r {
                Record::Entity { key, definition } if key == "b" => Some(&definition.names),
                _ => None,
            })
            .unwrap();
        assert_eq!(names.len(), 2);
        assert_eq!(names.iter().map(|n| n.value.as_str()).collect::<Vec<_>>(), vec!["真实合并别名", "更早的合并别名"]);
        assert!(names.iter().all(|n| n.provenance == "legacy_merge"));
    }
    #[test]
    fn relation_order_uses_legacy_graph_order_and_distinguishes_generic_relations() {
        let mut c = fixture();
        let tx = c.transaction().unwrap();
        schema::begin_writing(&tx).unwrap();
        tx.execute_batch("INSERT INTO terms(id,key,kind,origin) VALUES (1,'a','character','legacy'),(2,'b','character','legacy'),(3,'c','character','legacy');
            INSERT INTO term_relations(id,a,b,kind) VALUES (10,1,2,'same_as'),(20,2,3,'same_as'),(30,1,3,'same_as');
            INSERT INTO term_legacy_ids(kind,legacy_id,core_id) VALUES('relation',2,10),('relation',1,20);
            INSERT INTO actions(id,label,before_json) VALUES(1,'synthetic','[]');
            INSERT INTO term_verdicts(action_id,anchor,relation_id,verdict,origin) VALUES(1,1,10,'yes','user'),(1,1,20,'yes','user'),(1,1,30,'yes','user');").unwrap();
        schema::commit_writing(tx).unwrap();
        let tx = c.transaction().unwrap();
        let snapshot = capture(&tx, header(Policy::Default)).unwrap();
        let mut relations: Vec<_> = snapshot
            .records
            .iter()
            .filter_map(|r| match r {
                Record::RelationVerdict { a, b, order, identity_relation, .. } => {
                    Some((*order, a.as_str(), b.as_str(), *identity_relation))
                }
                _ => None,
            })
            .collect();
        relations.sort();
        assert_eq!(relations, vec![(0, "b", "c", true), (1, "a", "b", true), (2, "a", "c", false)]);
    }
    #[test]
    fn capture_keeps_one_read_snapshot_across_concurrent_commits() {
        struct Temp(std::path::PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let root =
            Temp(std::env::temp_dir().join(format!("illustash-human-snapshot-{}", crate::store::new_key().unwrap())));
        std::fs::create_dir(&root.0).unwrap();
        let source = fixture();
        let mut writer = Connection::open(root.0.join("library.sqlite")).unwrap();
        crate::configure(&writer).unwrap();
        {
            let backup = rusqlite::backup::Backup::new(&source, &mut writer).unwrap();
            backup.run_to_completion(128, std::time::Duration::from_millis(1), None).unwrap();
        }
        writer.execute_batch("PRAGMA journal_mode=WAL").unwrap();
        let mut reader = Connection::open(root.0.join("library.sqlite")).unwrap();
        crate::configure(&reader).unwrap();
        let tx = reader.transaction().unwrap();
        let before = capture(&tx, header(Policy::Default)).unwrap();
        let write = writer.transaction().unwrap();
        schema::begin_writing(&write).unwrap();
        write.execute("INSERT INTO curation VALUES ('later','included','review','2026-11-15')", []).unwrap();
        schema::commit_writing(write).unwrap();
        assert_eq!(
            capture(&tx, header(Policy::Default)).unwrap().semantic_sha256().unwrap(),
            before.semantic_sha256().unwrap()
        );
        tx.commit().unwrap();
        let tx = reader.transaction().unwrap();
        let after = capture(&tx, header(Policy::Default)).unwrap();
        assert_eq!(after.asset_keys().collect::<Vec<_>>(), vec!["later"]);
        assert_ne!(after.semantic_sha256().unwrap(), before.semantic_sha256().unwrap());
    }
}
