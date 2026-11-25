//! 人工快照的事务写入与独立撤销所有者；图片候选只接受宿主本次核验的完整清单。
use super::{
    definitions, export,
    format::{Entity, Header, Policy, Record, Snapshot},
    links,
    matching::{self, Match, SourceAsset, TargetAsset},
    names, relations, skips,
    state::{self, Domain, Mark},
};
use crate::{error::invalid, schema, store, Error, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// 一条源逻辑记录的处理结果；宿主持久化后按页提供全部明细。
#[derive(Clone, Debug, Serialize)]
pub struct Detail {
    /// 源记录类别。
    pub kind: String,
    /// 源文件内的稳定记录引用。
    pub record_key: String,
    /// applied、already_present 或拒绝原因。
    pub result: String,
    /// 资产未核验等结果的具体原因；不与上层结果重复计数。
    pub reason: Option<String>,
    /// 目标逻辑键各部分；未能认回时为空。
    pub target_key: Option<Vec<String>>,
    /// 已有依赖定义的独立名字组和链接结果；不与记录数相加。
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub field_counts: BTreeMap<String, usize>,
}

/// 单次导入结果；定义数和人工逻辑记录数分开计数。
#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    /// 没有有效写入时为空，不创建空历史。
    pub action_id: Option<i64>,
    /// 人工逻辑记录按唯一结果分类的数量。
    pub records: BTreeMap<String, usize>,
    /// 依赖定义按结果分类的数量。
    pub definitions: BTreeMap<String, usize>,
    /// 已有定义的名字组、链接等独立字段单位，不与依赖对象数相加。
    pub definition_fields: BTreeMap<String, usize>,
    /// 源资产按匹配结果分类的数量。
    pub assets: BTreeMap<String, usize>,
    /// 全部源记录的结果；HTTP 单页限制由宿主应用，不能截断台账。
    pub details: Vec<Detail>,
    /// 需要宿主在同一事务刷新收录投影的目标资产。
    #[serde(skip)]
    pub touched: Vec<String>,
}

impl Report {
    fn record(&mut self, record: &Record, result: &str) {
        self.record_reason(record, result, None);
    }

    fn record_reason(&mut self, record: &Record, result: &str, reason: Option<&str>) {
        *self.records.entry(result.into()).or_default() += 1;
        self.details.push(Detail {
            kind: record.identity().0.into(),
            record_key: record.identity().1.into(),
            result: result.into(),
            reason: reason.map(str::to_owned),
            target_key: None,
            field_counts: BTreeMap::new(),
        });
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct History {
    v: u32,
    source_library: String,
    snapshot_id: String,
    // after 保护新增人工逻辑键；定义和名字组分别保存自己的精确撤销前提。
    after: Vec<Record>,
    created: Vec<definitions::Created>,
    relations: Vec<relations::Created>,
    names: Vec<names::Edit>,
    links: Vec<links::Added>,
    skip_before: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    catalog_splits: Vec<crate::character_import::SuppressedCatalogChange>,
    graph: bool,
}

enum Resolution {
    Existing(String),
    New,
    Blocked,
}

fn key(record: &Record) -> Vec<String> {
    record.logical_key().into_iter().map(str::to_owned).collect()
}

fn current(c: &Connection) -> Result<Snapshot> {
    let policy = if crate::character_projection::policy(c)? { Policy::Hybrid } else { Policy::Default };
    export::capture(c, Header::new("current".into(), "current".into(), "2000-01-01T00:00:00Z".into(), policy))
}

fn equivalent(a: &Record, b: &Record) -> Result<bool> {
    if let (Record::RelationVerdict { seen: local, .. }, Record::RelationVerdict { seen: source, .. }) = (a, b) {
        let local: BTreeSet<_> = local.iter().collect();
        if !source.iter().all(|f| local.contains(f)) {
            return Ok(false);
        }
    }
    let normalize = |record: &Record| -> Result<Value> {
        let mut value = serde_json::to_value(record).map_err(|e| Error::invalid(e.to_string()))?;
        let object = value.as_object_mut().expect("记录序列化为对象");
        object.remove("record_key");
        // 操作时间是审计信息，不让同一判断因创建时刻不同被重复导入。
        object.remove("created_at");
        // 整数顺序在目标追加分配；代表语义由方向和组内相对顺序保留。
        object.remove("order");
        if matches!(record, Record::RelationVerdict { .. }) {
            object.remove("seen");
        }
        if let Some(decision) = object.get_mut("decision").and_then(Value::as_object_mut) {
            decision.remove("created_at");
        }
        Ok(value)
    };
    Ok(normalize(a)? == normalize(b)?)
}

fn resolve_existing(
    c: &Connection,
    source_library: &str,
    same_library: bool,
    source: &str,
    definition: &Entity,
) -> Result<Resolution> {
    // 人工标记的 key 是各图库共用的；没有名字、链接或可映射的身份结构。
    if definition.kind == "mark" {
        return Ok(match store::term_by_key(c, source)? {
            Some((_, schema::Kind::Mark)) => Resolution::Existing(source.to_owned()),
            Some(_) => Resolution::Blocked,
            None => Resolution::New,
        });
    }
    let mut candidates = BTreeSet::new();
    if same_library && store::term_by_key(c, source)?.is_some() {
        candidates.insert(source.to_owned());
    }
    for target in c
        .prepare(
            "SELECT o.target_key FROM human_entity_origins o JOIN actions a ON a.id=o.owner_action_id
        JOIN terms t ON t.key=o.target_key WHERE o.source_library=?1 AND o.source_key=?2 AND a.undone=0",
        )?
        .query_map([source_library, source], |r| r.get::<_, String>(0))?
    {
        candidates.insert(target?);
    }
    for link in &definition.links {
        if let Some(target) = c
            .query_row(
                "SELECT t.key FROM term_links l JOIN terms t ON t.id=l.term_id WHERE l.system=?1 AND l.key=?2",
                [&link.system, &link.key],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            candidates.insert(target);
        }
    }
    let Some(target) = candidates.pop_first() else {
        return Ok(Resolution::New);
    };
    if !candidates.is_empty() {
        return Ok(Resolution::Blocked);
    }
    let kind: String = c.query_row("SELECT kind FROM terms WHERE key=?1", [&target], |r| r.get(0))?;
    Ok(if kind == definition.kind { Resolution::Existing(target) } else { Resolution::Blocked })
}

fn match_reason(result: &Match) -> &'static str {
    match result {
        Match::Matched { .. } => "matched",
        Match::Unmatched => "unmatched_asset",
        Match::ContentChanged => "content_changed",
        Match::Ambiguous { .. } => "ambiguous_asset",
        Match::Unverified { .. } => "unverified_asset",
    }
}

fn family_conflict(
    c: &Connection,
    asset: &str,
    term: &str,
    entity: &Entity,
    verdict: &str,
    policy: Policy,
) -> Result<bool> {
    let Some(tree) = &entity.tree else { return Ok(false) };
    if policy == Policy::Hybrid {
        // 已合并的旧断言仍保留原 key；实际归属沿现行有效断言和身份节点判断。
        // 本次才建立的别名尚不在目标图内，先沿已验证源投影的代表认回；后续图预检核对该投影。
        let identity = tree.merged_into.as_deref().unwrap_or(term);
        let canonical: String = c.query_row(
            "SELECT coalesce((SELECT canonical_id FROM identity_nodes WHERE id=?1),?1)",
            [identity],
            |r| r.get(0),
        )?;
        let family_key = tree.parent.as_deref().unwrap_or(identity);
        let family: String = c.query_row(
            "SELECT coalesce((SELECT character_id FROM identity_nodes WHERE id=?1),?1)",
            [family_key],
            |r| r.get(0),
        )?;
        let root: bool = c.query_row(
            "SELECT coalesce((SELECT variant_id IS NULL FROM identity_nodes WHERE id=?1),?2)",
            params![identity, tree.parent.is_none()],
            |r| r.get(0),
        )?;
        // 挂起按原身份键匹配；只有比较裁决时才使用代表身份和角色归属。
        return Ok(c.query_row(
            "WITH effective AS (
                SELECT n.canonical_id AS entity,n.character_id,n.variant_id,s.status
                FROM assertions s JOIN identity_nodes n ON n.id=s.entity JOIN assets a ON a.uuid=s.uuid
                WHERE s.uuid=?1
                  AND NOT EXISTS(SELECT 1 FROM identity_suspended h WHERE h.uuid=s.uuid AND h.entity=s.entity)
                  AND NOT EXISTS(SELECT 1 FROM identity_uncertain u WHERE u.uuid=s.uuid)
             )
             SELECT CASE ?5
                 WHEN 'yes' THEN EXISTS(SELECT 1 FROM effective WHERE status='rejected'
                     AND (entity=?2 OR (character_id=?3 AND variant_id IS NULL)))
                 WHEN 'no' THEN EXISTS(SELECT 1 FROM effective WHERE status='confirmed'
                     AND (entity=?2 OR (?4 AND character_id=?3)))
                     AND NOT EXISTS(SELECT 1 FROM effective WHERE status='rejected' AND entity=?2)
                 ELSE 0
             END",
            params![asset, canonical, family, root, verdict],
            |r| r.get(0),
        )?);
    }
    match verdict {
        "yes"=>Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM assertions WHERE uuid=?1 AND entity=?2 AND status='rejected')",
            params![asset,tree.parent],|r|r.get(0))?),
        "no"=>Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM assertions a JOIN term_tree t ON t.term_id=(SELECT id FROM terms WHERE key=a.entity)
            WHERE a.uuid=?1 AND a.status='confirmed' AND t.parent_id=?2)
            AND NOT EXISTS(SELECT 1 FROM assertions WHERE uuid=?1 AND entity=?2 AND status='rejected')",params![asset,term],|r|r.get(0))?),
        _=>Ok(false),
    }
}

// 每个关系连通组在可回滚保存点中按真实图投影检查。旧人工归属或结构发生变化时整组跳过。
fn graph_conflicts(
    c: &Connection,
    snapshot: &Snapshot,
    existing: &Snapshot,
    terms: &BTreeMap<String, String>,
    new_definitions: &[definitions::New],
    check: &mut impl FnMut() -> Result<()>,
) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
    let mut visited = BTreeSet::new();
    let mut blocked = BTreeSet::new();
    let mut conflicts = BTreeSet::new();
    let existing_records: BTreeMap<_, _> = existing.records.iter().map(|r| (key(r), r)).collect();
    for record in &snapshot.records {
        let Record::RelationVerdict { a, b, .. } = record else { continue };
        if visited.contains(a) || !terms.contains_key(a) || !terms.contains_key(b) {
            continue;
        }
        let group = definitions::connected(snapshot, BTreeSet::from([a.clone(), b.clone()]));
        visited.extend(group.iter().cloned());
        let mut relations: Vec<_> = snapshot
            .records
            .iter()
            .filter_map(|r| match r {
                Record::RelationVerdict { a, b, .. } if group.contains(a) => {
                    let mut mapped = r.clone();
                    if let Record::RelationVerdict { a: ma, b: mb, kind, .. } = &mut mapped {
                        *ma = terms.get(a)?.clone();
                        *mb = terms.get(b)?.clone();
                        if matches!(kind.as_str(), "different_from" | "excludes") && ma > mb {
                            std::mem::swap(ma, mb);
                        }
                    }
                    Some(mapped)
                }
                _ => None,
            })
            .collect();
        let all_present = relations.iter().try_fold(true, |present, r| -> Result<bool> {
            Ok(present
                && match existing_records.get(&key(r)) {
                    Some(before) => equivalent(before, r)?,
                    None => false,
                })
        })?;
        if all_present {
            continue;
        }
        let mut direct_conflict = false;
        for record in &relations {
            if let Some(before) = existing_records.get(&key(record)) {
                if !equivalent(before, record)? {
                    conflicts.insert(record.identity().1.to_owned());
                    direct_conflict = true;
                }
            }
        }
        if direct_conflict {
            blocked.extend(group);
            continue;
        }
        relations.sort_by_key(|r| match r {
            Record::RelationVerdict { order, .. } => *order,
            _ => 0,
        });
        check()?;
        c.execute_batch("SAVEPOINT human_graph_preflight")?;
        let result = (|| -> Result<()> {
            let definitions: Vec<_> = new_definitions
                .iter()
                .filter(|d| group.contains(&d.source_key))
                .map(|d| {
                    Ok(definitions::New {
                        source_key: d.source_key.clone(),
                        target_key: d.target_key.clone(),
                        definition: definitions::mapped(&d.definition, terms)
                            .ok_or_else(|| Error::Conflict("关系依赖不完整".into()))?,
                    })
                })
                .collect::<Result<_>>()?;
            c.execute("INSERT INTO actions(label,before_json) VALUES ('人工关系预检','[]')", [])?;
            let action = c.last_insert_rowid();
            definitions::create(c, action, &snapshot.header.library_id, &definitions)?;
            for record in &relations {
                if !existing_records.contains_key(&key(record)) {
                    relations::write(c, action, record)?;
                }
            }
            if existing.header.policy == Policy::Hybrid {
                crate::character_graph::rebuild(c)?;
            }
            let after = current(c)?;
            let after: BTreeMap<_, _> = after.records.iter().map(|r| (key(r), r)).collect();
            // 图投影可能改变范围、暂停状态与现存身份；目标的这些语义都有优先权。
            for before in &existing.records {
                if matches!(before, Record::Entity { .. } | Record::IdentityContext { .. })
                    && after.get(&key(before)).copied() != Some(before)
                {
                    return Err(Error::Conflict("源关系会改变目标已有身份语义".into()));
                }
            }
            for definition in &definitions {
                let Some(Record::Entity { definition: after, .. }) =
                    after.get(&vec!["entity".into(), definition.target_key.clone()]).copied()
                else {
                    return Err(Error::Conflict("源关系投影丢失身份".into()));
                };
                if !definitions::same_structure(&definition.definition, after) {
                    return Err(Error::Conflict("源关系与目标原有关系不能保留相同结构".into()));
                }
            }
            Ok(())
        })();
        c.execute_batch("ROLLBACK TO human_graph_preflight; RELEASE human_graph_preflight")?;
        match result {
            Ok(()) => (),
            Err(Error::Conflict(_) | Error::Invalid(_)) => blocked.extend(group),
            Err(e) => return Err(e),
        }
    }
    Ok((blocked, conflicts))
}

/// 在宿主持有的 BEGIN IMMEDIATE 和核心写钥匙中应用快照。
/// 宿主须提供全部当前资产的本次内容核验结果，并在提交前复核文件版本、会话及取消信号。
/// 返回错误时必须回滚整个事务。未实现的依赖或记录明确报告，不算成功导入。
pub fn apply_in(
    c: &Connection,
    snapshot: &Snapshot,
    target_library: &str,
    candidates: &[TargetAsset],
    check: &mut impl FnMut() -> Result<()>,
) -> Result<Report> {
    if c.is_autocommit() {
        return invalid("导入人工结论需要写事务");
    }
    schema::require_current(c)?;
    snapshot.validate()?;
    check()?;
    let live: BTreeSet<String> =
        c.prepare("SELECT uuid FROM assets")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    if live != candidates.iter().map(|a| a.key.clone()).collect() || live.len() != candidates.len() {
        return Err(Error::Conflict("目标资产清单已经变化，须重新核验图片内容".into()));
    }
    let same_library = snapshot.header.library_id == target_library;
    let source: Vec<SourceAsset> = snapshot
        .records
        .iter()
        .filter_map(|r| match r {
            Record::Asset { key, hash } => Some(SourceAsset { key: key.clone(), hash: hash.clone() }),
            _ => None,
        })
        .collect();
    let matched = matching::match_assets(same_library, &source, candidates)?;
    let assets: BTreeMap<_, _> = source.iter().map(|a| a.key.as_str()).zip(&matched).collect();
    let mut report = Report::default();
    for (source, result) in source.iter().zip(&matched) {
        *report.assets.entry(match_reason(result).into()).or_default() += 1;
        report.details.push(Detail {
            kind: "asset".into(),
            record_key: source.key.clone(),
            result: match_reason(result).into(),
            reason: match result {
                Match::Ambiguous { reason } | Match::Unverified { reason } => Some(reason.clone()),
                _ => None,
            },
            target_key: match result {
                Match::Matched { target, .. } => Some(vec![target.clone()]),
                _ => None,
            },
            field_counts: BTreeMap::new(),
        });
    }
    let mut terms = BTreeMap::new();
    let mut new_definitions = Vec::new();
    for record in &snapshot.records {
        if let Record::Entity { key, definition } = record {
            match resolve_existing(c, &snapshot.header.library_id, same_library, key, definition)? {
                Resolution::Existing(target) => {
                    terms.insert(key.clone(), target);
                }
                Resolution::New => {
                    let target = if definition.kind == "mark" { key.clone() } else { store::new_key()? };
                    terms.insert(key.clone(), target.clone());
                    new_definitions.push(definitions::New {
                        source_key: key.clone(),
                        target_key: target,
                        definition: definition.as_ref().clone(),
                    });
                }
                Resolution::Blocked => (),
            }
        }
    }
    // 两个源定义同时认回一个目标会让各自的结论占用同一个逻辑键；禁止靠记录顺序覆盖。
    let mut claims = BTreeMap::<String, usize>::new();
    for target in terms.values() {
        *claims.entry(target.clone()).or_default() += 1;
    }
    terms.retain(|_, target| claims[target] == 1);
    let existing = current(c)?;
    let existing_entities: BTreeMap<_, _> = existing
        .records
        .iter()
        .filter_map(|r| match r {
            Record::Entity { key, definition } => Some((key, definition.as_ref())),
            _ => None,
        })
        .collect();
    let fresh: BTreeSet<_> = new_definitions.iter().map(|new| new.source_key.as_str()).collect();
    let source_entities: BTreeMap<_, _> = snapshot
        .records
        .iter()
        .filter_map(|r| match r {
            Record::Entity { key, definition } => Some((key.as_str(), definition.as_ref())),
            _ => None,
        })
        .collect();
    let mut blocked = BTreeSet::new();
    for record in &snapshot.records {
        if let Record::Entity { key, definition } = record {
            let mapped = definitions::mapped(definition, &terms);
            let compatible = terms.get(key).is_some_and(|target| {
                mapped.as_ref().is_some_and(|mapped| {
                    if fresh.contains(key.as_str()) {
                        true
                    } else if let Some(before) = existing_entities.get(target) {
                        definitions::same_structure(before, mapped)
                    } else {
                        mapped.tree.is_none() && mapped.baseline.is_none()
                    }
                })
            });
            if !compatible || (snapshot.header.policy != existing.header.policy && definition.tree.is_some()) {
                blocked.insert(key.clone());
            }
        }
        if matches!(record, Record::RelationVerdict { identity_relation: true, .. })
            && snapshot.header.policy != existing.header.policy
        {
            blocked.extend(record.terms().into_iter().map(str::to_owned));
        }
        if let Record::ImageVerdict { asset, term, decision, .. } = record {
            if let (Some(Match::Matched { target: asset, .. }), Some(target), Some(entity)) = (
                assets.get(asset.as_str()).copied(),
                terms.get(term),
                source_entities.get(term.as_str()).and_then(|e| definitions::mapped(e, &terms)),
            ) {
                if family_conflict(c, asset, target, &entity, &decision.verdict, existing.header.policy)? {
                    blocked.insert(term.clone());
                }
            }
        }
    }
    let blocked = definitions::connected(snapshot, blocked);
    terms.retain(|source, _| !blocked.contains(source));
    let (blocked, relation_conflicts) = graph_conflicts(c, snapshot, &existing, &terms, &new_definitions, check)?;
    terms.retain(|source, _| !blocked.contains(source));
    let mut name_edits = Vec::new();
    let mut link_edits = Vec::new();
    let mut field_counts = BTreeMap::new();
    for (source, target) in &terms {
        if !fresh.contains(source.as_str()) {
            let mut fields = BTreeMap::new();
            link_edits.extend(links::plan(c, target, source_entities[source.as_str()], &mut fields)?);
            name_edits.extend(names::plan(
                c,
                target,
                source_entities[source.as_str()],
                existing_entities.get(target).copied(),
                &mut fields,
            )?);
            for (result, count) in &fields {
                *report.definition_fields.entry(result.clone()).or_default() += count;
            }
            field_counts.insert(source, fields);
        }
    }
    for record in &snapshot.records {
        if let Record::Entity { key, .. } = record {
            if !terms.contains_key(key) {
                *report.definitions.entry("blocked_dependency".into()).or_default() += 1;
                report.details.push(Detail {
                    kind: "entity".into(),
                    record_key: key.clone(),
                    result: "blocked_dependency".into(),
                    reason: None,
                    target_key: None,
                    field_counts: BTreeMap::new(),
                });
            } else if !fresh.contains(key.as_str()) {
                *report.definitions.entry("matched_dependency".into()).or_default() += 1;
                report.details.push(Detail {
                    kind: "entity".into(),
                    record_key: key.clone(),
                    result: "matched_dependency".into(),
                    reason: None,
                    target_key: Some(vec![terms[key].clone()]),
                    field_counts: field_counts.remove(key).unwrap_or_default(),
                });
            }
        }
    }
    new_definitions.retain(|new| terms.contains_key(&new.source_key));
    for new in &mut new_definitions {
        new.definition = definitions::mapped(&new.definition, &terms).expect("依赖已核对并排除受阻组");
    }
    let existing: BTreeMap<_, _> = existing.records.iter().map(|r| (key(r), r)).collect();
    let mut planned = Vec::new();
    let mut mapped_records = Vec::new();
    for source in &snapshot.records {
        check()?;
        if matches!(source, Record::Asset { .. } | Record::Entity { .. }) {
            continue;
        }
        if let Some((result, reason)) = source.assets().into_iter().find_map(|a| match assets[a] {
            Match::Matched { .. } => None,
            result => Some((
                match_reason(result),
                match result {
                    Match::Ambiguous { reason } | Match::Unverified { reason } => Some(reason.as_str()),
                    _ => None,
                },
            )),
        }) {
            report.record_reason(source, result, reason);
            continue;
        }
        if source.terms().iter().any(|t| !terms.contains_key(*t)) {
            report.record(
                source,
                if relation_conflicts.contains(source.identity().1) { "conflict" } else { "blocked_dependency" },
            );
            continue;
        }
        let mut target = source.clone();
        let asset_key = |asset: &str| match assets[asset] {
            Match::Matched { target, .. } => target.clone(),
            _ => unreachable!("所有依赖资产已经匹配"),
        };
        match &mut target {
            Record::ImageVerdict { asset, term, .. } => {
                *asset = asset_key(asset);
                *term = terms[term].clone();
            }
            Record::Curation { asset, .. } | Record::Deferred { asset, .. } => *asset = asset_key(asset),
            Record::Uncertain { asset, options, .. } => {
                *asset = asset_key(asset);
                *options = options.iter().map(|t| terms[t].clone()).collect();
            }
            Record::Review { term, .. } => *term = terms[term].clone(),
            Record::RelationVerdict { a, b, kind, .. } => {
                *a = terms[a].clone();
                *b = terms[b].clone();
                if matches!(kind.as_str(), "different_from" | "excludes") && a > b {
                    std::mem::swap(a, b);
                }
            }
            Record::IdentitySkip { pair, .. } => {
                let Some((a, b)) = skips::pair(pair)
                    .filter(|(a, b)| source_entities.contains_key(a) && source_entities.contains_key(b))
                else {
                    report.record_reason(source, "unsupported_dependency", Some("opaque_or_dangling_pair"));
                    continue;
                };
                let (Some(a), Some(b)) = (terms.get(a), terms.get(b)) else {
                    report.record(source, "blocked_dependency");
                    continue;
                };
                *pair = if a < b { format!("{a}|{b}") } else { format!("{b}|{a}") };
            }
            Record::IdentityKeepApart { a, b, .. } => {
                *a = terms[a].clone();
                *b = terms[b].clone();
                if a == b {
                    report.record(source, "conflict");
                    continue;
                }
                if a > b {
                    std::mem::swap(a, b);
                }
            }
            Record::AnchorDecision { .. } => {
                report.record_reason(source, "unsupported_dependency", Some("reference_version_unverified"));
                continue;
            }
            Record::IdentityContext { asset, term, scope, suspended, .. } => {
                *asset = asset_key(asset);
                *term = terms[term].clone();
                if let Some(scope) = scope {
                    *scope = scope.iter().map(|t| terms[t].clone()).collect();
                }
                if let Some(suspended) = suspended {
                    suspended.options = suspended.options.iter().map(|t| terms[t].clone()).collect();
                }
            }
            _ => {
                report.record(source, "unsupported_record");
                continue;
            }
        }
        mapped_records.push((source, target));
    }
    let pair = |record: &Record| match record {
        Record::ImageVerdict { asset, term, .. } | Record::IdentityContext { asset, term, .. } => {
            Some((asset.clone(), term.clone()))
        }
        _ => None,
    };
    let mut pair_conflicts = BTreeSet::new();
    let mapped_keys: BTreeSet<_> = mapped_records.iter().map(|(_, record)| key(record)).collect();
    for (_, target) in &mapped_records {
        if let Some(pair) = pair(target) {
            // 已有图片判断的附件缺省也属于目标语义；不能因裁决文字相同就给它追加源范围。
            if matches!(target, Record::IdentityContext { .. })
                && existing.contains_key(&vec!["image_verdict".into(), pair.0.clone(), pair.1.clone()])
                && !existing.contains_key(&key(target))
            {
                pair_conflicts.insert(pair.clone());
            }
            if let Some(before) = existing.get(&key(target)) {
                if !equivalent(before, target)? {
                    pair_conflicts.insert(pair);
                }
            } else {
                let other =
                    if matches!(target, Record::ImageVerdict { .. }) { "identity_context" } else { "image_verdict" };
                let other = vec![other.to_owned(), pair.0.clone(), pair.1.clone()];
                if existing.contains_key(&other) && !mapped_keys.contains(&other) {
                    pair_conflicts.insert(pair);
                }
            }
        }
    }
    for (source, target) in mapped_records {
        let target_key = key(&target);
        if pair(&target).is_some_and(|pair| pair_conflicts.contains(&pair)) {
            let conflict = match existing.get(&key(&target)) {
                Some(before) => !equivalent(before, &target)?,
                None => false,
            };
            report.record(source, if conflict { "conflict" } else { "blocked_dependency" });
        } else if let Some(before) = existing.get(&key(&target)) {
            report.record(source, if equivalent(before, &target)? { "already_present" } else { "conflict" });
        } else {
            report.record(source, "applied");
            planned.push(target);
        }
        report.details.last_mut().expect("刚记录处理结果").target_key = Some(target_key);
    }
    let mut needed: BTreeSet<_> = planned.iter().flat_map(|r| r.terms().into_iter().map(str::to_owned)).collect();
    for record in &planned {
        if let Record::IdentitySkip { pair, .. } = record {
            if let Some((a, b)) = skips::pair(pair) {
                needed.extend([a.to_owned(), b.to_owned()]);
            }
        }
    }
    new_definitions.retain(|new| {
        let human = new.definition.tree.is_some()
            || new.definition.provenance == "user"
            || new.definition.names.iter().any(|n| matches!(n.provenance.as_str(), "user" | "legacy_merge"));
        let keep = (human && new.definition.kind != "mark") || needed.contains(&new.target_key);
        *report.definitions.entry(if keep { "applied" } else { "unneeded_dependency" }.into()).or_default() += 1;
        let unsupported =
            if keep { new.definition.names.iter().filter(|name| !names::restorable(name)).count() } else { 0 };
        let mut fields = BTreeMap::new();
        if unsupported > 0 {
            fields.insert("name_rows_unsupported_dependency".into(), unsupported);
            *report.definition_fields.entry("name_rows_unsupported_dependency".into()).or_default() += unsupported;
        }
        report.details.push(Detail {
            kind: "entity".into(),
            record_key: new.source_key.clone(),
            result: if keep { "applied" } else { "unneeded_dependency" }.into(),
            reason: None,
            target_key: keep.then(|| vec![new.target_key.clone()]),
            field_counts: fields,
        });
        keep
    });
    for new in &mut new_definitions {
        new.definition.names.retain(names::restorable);
    }
    if planned.is_empty() && new_definitions.is_empty() && name_edits.is_empty() && link_edits.is_empty() {
        return Ok(report);
    }
    c.execute("INSERT INTO actions(label,before_json) VALUES ('已导入人工结论','[]')", [])?;
    let action = c.last_insert_rowid();
    let skip_before =
        if planned.iter().any(|r| matches!(r, Record::IdentitySkip { .. })) { skips::stored(c)? } else { None };
    let name_edits = names::apply(c, name_edits)?;
    links::apply(c, &link_edits)?;
    let mut created = definitions::create(c, action, &snapshot.header.library_id, &new_definitions)?;
    let graph = crate::character_projection::policy(c)?
        && (new_definitions.iter().any(|new| new.definition.tree.is_some())
            || planned.iter().any(|r| matches!(r, Record::RelationVerdict { identity_relation: true, .. })));
    let mut relation_records: Vec<_> = planned.iter().filter(|r| matches!(r, Record::RelationVerdict { .. })).collect();
    relation_records.sort_by_key(|r| match r {
        Record::RelationVerdict { order, .. } => *order,
        _ => 0,
    });
    let mut created_relations = Vec::new();
    for record in relation_records {
        if let Some(created) = relations::write(c, action, record)? {
            created_relations.push(created);
        }
    }
    if graph {
        crate::character_graph::rebuild(c)?;
    }
    definitions::refresh_signatures(c, &mut created)?;
    let mut touched = BTreeSet::new();
    let mut catalog_splits = Vec::new();
    for record in &planned {
        check()?;
        if !matches!(record, Record::RelationVerdict { .. }) {
            write(c, action, &snapshot.header.snapshot_id, record)?;
        }
        if let Record::IdentityKeepApart { a, b, .. } = record {
            if let Some(split) = crate::character_import::suppress_catalog_change(c, a, b)? {
                catalog_splits.push(split);
            }
        }
        touched.extend(record.assets().into_iter().map(str::to_owned));
    }
    project(c, &touched)?;
    let mut applied: BTreeSet<_> = planned.iter().map(key).collect();
    applied.extend(created.iter().map(|object| vec!["entity".into(), object.key.clone()]));
    let after = current(c)?.records.into_iter().filter(|r| applied.contains(&key(r))).collect::<Vec<_>>();
    if after.len() != planned.len() + created.len() {
        return Err(Error::Conflict("导入后的有效记录与本次计划不一致".into()));
    }
    let after_by_key: BTreeMap<_, _> = after.iter().map(|record| (key(record), record)).collect();
    for expected in &planned {
        let valid = match after_by_key.get(&key(expected)) {
            Some(record) => equivalent(record, expected)?,
            None => false,
        };
        if !valid {
            return Err(Error::Conflict("导入后的判断语义与源记录不一致".into()));
        }
    }
    let history = History {
        v: 1,
        source_library: snapshot.header.library_id.clone(),
        snapshot_id: snapshot.header.snapshot_id.clone(),
        after,
        created,
        relations: created_relations,
        names: name_edits,
        links: link_edits,
        skip_before,
        catalog_splits,
        graph,
    };
    c.execute(
        "UPDATE actions SET before_json=?1 WHERE id=?2",
        params![json!([{"human_conclusions":history}]).to_string(), action],
    )?;
    report.action_id = Some(action);
    report.touched = touched.into_iter().collect();
    check()?;
    Ok(report)
}

fn write(c: &Connection, action: i64, snapshot: &str, record: &Record) -> Result<()> {
    let absent = |domain, asset: &str, origin: &str, at: &str| {
        state::set(
            c,
            domain,
            asset,
            action,
            Some(Mark {
                owner_action_id: action,
                origin: origin.into(),
                source_ref: Some(format!("{snapshot}:{}", record.identity().1)),
                created_at: at.into(),
            }),
        )
    };
    match record {
        Record::ImageVerdict { asset, term, decision, .. } => {
            let (uuid, entity) = (asset, term);
            let asset = store::library_asset(c, asset)?.ok_or_else(|| Error::Conflict("目标图片已移除".into()))?;
            let (term, _) = store::term_by_key(c, term)?.ok_or_else(|| Error::Conflict("目标词条已移除".into()))?;
            c.execute(
                "INSERT INTO term_verdicts(action_id,anchor,asset_id,term_id,verdict,origin,via,reason,created_at)
                 VALUES (?1,?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    action,
                    asset,
                    term,
                    decision.verdict,
                    decision.origin,
                    decision.via,
                    decision.reason,
                    decision.created_at
                ],
            )?;
            if c.query_row("SELECT EXISTS(SELECT 1 FROM term_tree WHERE term_id=?1)", [term], |r| r.get::<_, bool>(0))?
            {
                // refresh_assertion 会重建范围；便携导入保留源附件，只同步这一行派生值。
                c.execute("DELETE FROM assertions WHERE uuid=?1 AND entity=?2", params![uuid, entity])?;
                if decision.verdict != "clear" {
                    c.execute(
                        "INSERT INTO assertions(uuid,entity,status,origin) VALUES (?1,?2,?3,?4)",
                        params![
                            uuid,
                            entity,
                            if decision.verdict == "yes" { "confirmed" } else { "rejected" },
                            decision.via
                        ],
                    )?;
                }
            }
        }
        Record::Curation { asset, state: value, origin, created_at, .. } => {
            if value == "reset" {
                absent(Domain::Curation, asset, origin, created_at)?;
            } else {
                c.execute(
                    "INSERT INTO curation(uuid,status,origin,created_at) VALUES (?1,?2,?3,?4)",
                    params![asset, value, origin, created_at],
                )?;
            }
        }
        Record::Deferred { asset, state: value, origin, created_at, .. } => {
            if value == "resumed" {
                absent(Domain::Deferred, asset, origin, created_at.as_deref().expect("格式已检查恢复时间"))?;
            } else {
                c.execute("INSERT INTO deferred(uuid) VALUES (?1)", [asset])?;
            }
        }
        Record::Uncertain { asset, state: value, options, reason, origin, created_at, .. } => {
            if value == "cleared" {
                absent(Domain::Uncertain, asset, origin, created_at)?;
            } else {
                c.execute(
                    "INSERT INTO identity_uncertain(uuid,options_json,reason,created_at) VALUES (?1,?2,?3,?4)",
                    params![asset, json!(options).to_string(), reason, created_at],
                )?;
            }
        }
        Record::Review { term, reviewed_at, .. } => {
            c.execute(
                "INSERT INTO reviews(entity,reviewed_at,action_id) VALUES (?1,?2,?3)",
                params![term, reviewed_at, action],
            )?;
        }
        Record::IdentitySkip { pair, .. } => skips::add(c, pair)?,
        Record::IdentityKeepApart { a, b, .. } => {
            crate::character_identity::set_kept_apart(c, a, b, true)?;
        }
        Record::IdentityContext { asset, term, scope, suspended, .. } => {
            if let Some(scope) = scope {
                c.execute(
                    "INSERT INTO identity_scopes(uuid,entity,members_json) VALUES (?1,?2,?3)",
                    params![asset, term, json!(scope).to_string()],
                )?;
            }
            if let Some(suspended) = suspended {
                c.execute(
                    "INSERT INTO identity_suspended(uuid,entity,reason,options_json) VALUES (?1,?2,?3,?4)",
                    params![asset, term, suspended.reason, json!(suspended.options).to_string()],
                )?;
            }
        }
        _ => return invalid("尚未接入此类人工记录的事务写入"),
    }
    Ok(())
}

fn project(c: &Connection, touched: &BTreeSet<String>) -> Result<()> {
    let mut assets = Vec::new();
    for uuid in touched {
        if let Some(asset) = store::library_asset(c, uuid)? {
            assets.push(asset);
        }
    }
    store::project(c, &assets)?;
    crate::character_projection::rebuild(c, Some(&touched.iter().cloned().collect::<Vec<_>>()))
}

fn history(value: &Value) -> Result<Option<History>> {
    let Some(items) = value.as_array() else { return Ok(None) };
    if !items.iter().any(|v| v.get("human_conclusions").is_some()) {
        return Ok(None);
    }
    if items.len() != 1 || items[0].as_object().is_none_or(|o| o.len() != 1) {
        return Err(Error::Conflict("人工结论撤销信封不完整".into()));
    }
    let decoded: History = serde_json::from_value(items[0]["human_conclusions"].clone())
        .map_err(|_| Error::Conflict("人工结论撤销信封不完整".into()))?;
    if decoded.v != 1 || (decoded.after.is_empty() && decoded.names.is_empty() && decoded.links.is_empty()) {
        return Err(Error::Conflict("人工结论撤销版本不受支持".into()));
    }
    Ok(Some(decoded))
}

pub(crate) fn name_aliases(value: &Value) -> Result<Vec<(i64, super::format::Name)>> {
    Ok(history(value)?.map(|h| names::aliases(&h.names)).unwrap_or_default())
}

/// 识别专用历史，混合或未知版本返回错误，不落入普通应用撤销。
pub fn owns(value: &Value) -> Result<bool> {
    Ok(history(value)?.is_some())
}

/// 撤销最后一个有效导入；检查全部 after 后再写入，独立变更导致冲突。
/// 调用方持有写事务和写钥匙，并在同一事务刷新返回资产的收录投影。
pub fn undo_in(c: &Connection, action: i64) -> Result<Vec<String>> {
    if c.is_autocommit() {
        return invalid("撤销人工结论需要写事务");
    }
    let last: Option<i64> = c.query_row("SELECT max(id) FROM actions WHERE undone=0", [], |r| r.get(0))?;
    if last != Some(action) {
        return Err(Error::Conflict(store::HISTORY_CHANGED.into()));
    }
    let encoded: String = c.query_row("SELECT before_json FROM actions WHERE id=?1", [action], |r| r.get(0))?;
    let value = crate::strict_json::parse(&encoded).map_err(Error::invalid)?;
    let history = history(&value)?.ok_or_else(|| Error::Conflict("这一步不属于人工结论导入".into()))?;
    let now = current(c)?;
    let now: BTreeMap<_, _> = now.records.iter().map(|r| (key(r), r)).collect();
    for expected in &history.after {
        let actual = now.get(&key(expected)).copied();
        let same = match (expected, actual) {
            (Record::Entity { definition: a, .. }, Some(Record::Entity { definition: b, .. })) => {
                definitions::same_after(a, b)
            }
            _ => actual == Some(expected),
        };
        if !same {
            return Err(Error::Conflict("导入后的人工状态已被独立修改，无法安全撤销".into()));
        }
    }
    let mut touched = BTreeSet::new();
    for record in &history.after {
        if let Record::ImageVerdict { asset, term, .. } = record {
            c.execute("DELETE FROM assertions WHERE uuid=?1 AND entity=?2", params![asset, term])?;
        }
        if let Record::Review { term, .. } = record {
            c.execute("DELETE FROM reviews WHERE action_id=?1 AND entity=?2", params![action, term])?;
        }
        if let Record::IdentityContext { asset, term, .. } = record {
            c.execute("DELETE FROM identity_scopes WHERE uuid=?1 AND entity=?2", params![asset, term])?;
            c.execute("DELETE FROM identity_suspended WHERE uuid=?1 AND entity=?2", params![asset, term])?;
        }
        let table = match record {
            Record::ImageVerdict { .. }
            | Record::Entity { .. }
            | Record::Review { .. }
            | Record::RelationVerdict { .. }
            | Record::IdentitySkip { .. }
            | Record::IdentityKeepApart { .. }
            | Record::IdentityContext { .. } => None,
            Record::Curation { .. } => Some("curation"),
            Record::Deferred { .. } => Some("deferred"),
            Record::Uncertain { .. } => Some("identity_uncertain"),
            _ => return Err(Error::Conflict("人工结论撤销包含不支持的记录".into())),
        };
        for asset in record.assets() {
            if let Some(table) = table {
                c.execute(&format!("DELETE FROM {table} WHERE uuid=?1"), [asset])?;
            }
            touched.insert(asset.to_owned());
        }
    }
    // 先撤去已经精确核对的本次附属状态，再检查其它依赖；发生冲突时宿主回滚整个事务。
    names::undo(c, &history.names)?;
    links::undo(c, &history.links)?;
    let skipped: Vec<_> = history
        .after
        .iter()
        .filter_map(|r| match r {
            Record::IdentitySkip { pair, .. } => Some(pair.as_str()),
            _ => None,
        })
        .collect();
    skips::undo(c, &skipped, history.skip_before.as_deref())?;
    for record in &history.after {
        if let Record::IdentityKeepApart { a, b, .. } = record {
            crate::character_identity::set_kept_apart(c, a, b, false)?;
        }
    }
    for split in &history.catalog_splits {
        crate::character_import::restore_catalog_change(c, split)?;
    }
    definitions::check_undo(c, action, &history.created)?;
    relations::check_undo(c, action, &history.relations)?;
    definitions::remove(c, &history.created)?;
    c.execute("UPDATE actions SET undone=1 WHERE id=?1", [action])?;
    if history.graph {
        crate::character_graph::rebuild(c)?;
    }
    state::undo(c, action)?;
    project(c, &touched)?;
    Ok(touched.into_iter().collect())
}
