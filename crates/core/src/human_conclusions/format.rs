//! JSONL v1 的封闭字段、完整性摘要与资源上限；解析成功前不接触目标数据库。
use super::matching::SourceHash;
use crate::{error::invalid, strict_json, Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{BufRead, Read, Write};

/// 单个快照最多 256 MiB，含头尾记录。
pub const MAX_BYTES: u64 = 256 * 1024 * 1024;
/// 单行最多 4 MiB，含换行符。
pub const MAX_LINE: u64 = 4 * 1024 * 1024;
/// 头尾以外最多二百万条记录。
pub const MAX_RECORDS: usize = 2_000_000;

/// 源图库使用的身份解释策略。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    /// 普通父子结构及合并指向。
    Default,
    /// 含关系基线、等价组和判断范围的身份策略。
    Hybrid,
}

/// 快照头；随机编号和导出时间不参与语义摘要。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    #[serde(rename = "type")]
    record: String,
    format: String,
    version: u32,
    #[serde(rename = "mode")]
    scope: String,
    /// 快照唯一编号。
    pub snapshot_id: String,
    /// 源图库编号，不能单独作为图片相同的证明。
    pub library_id: String,
    /// UTC 导出时间。
    #[serde(rename = "created_at")]
    pub exported_at: String,
    /// 源图库的身份解释策略。
    #[serde(rename = "identity_policy")]
    pub policy: Policy,
}

impl Header {
    /// 创建 JSONL v1 的固定格式头。
    pub fn new(snapshot_id: String, library_id: String, exported_at: String, policy: Policy) -> Self {
        Self {
            record: "header".into(),
            format: "illustash-human-conclusions".into(),
            version: 1,
            scope: "effective-state".into(),
            snapshot_id,
            library_id,
            exported_at,
            policy,
        }
    }

    fn validate(&self) -> Result<()> {
        if self.record != "header"
            || self.format != "illustash-human-conclusions"
            || self.version != 1
            || self.scope != "effective-state"
        {
            return invalid("不支持的人工结论文件格式或版本");
        }
        key(&self.snapshot_id)?;
        key(&self.library_id)?;
        if !self.exported_at.ends_with('Z') || self.exported_at.len() > 40 || !self.exported_at.contains('T') {
            return invalid("快照导出时间须为 UTC");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Name {
    pub lang: String,
    pub value: String,
    pub role: String,
    pub preferred: bool,
    pub position: i64,
    pub source: String,
    pub provenance: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Link {
    pub system: String,
    pub key: String,
    pub current: bool,
    pub added_at: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Tree {
    pub parent: Option<String>,
    pub merged_into: Option<String>,
    pub work: Option<String>,
    pub work_text: String,
    pub variant: String,
    pub artist: String,
    pub label_only: bool,
    pub mascot: bool,
    pub mascot_origin: Option<String>,
    pub original: bool,
    pub parent_locked: Option<bool>,
    pub review_status: Option<String>,
    pub review_note: Option<String>,
    pub order: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Baseline {
    pub parent: Option<String>,
    pub variant: Option<String>,
    pub kind: Option<String>,
    pub merged_into: Option<String>,
    pub order: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entity {
    pub kind: String,
    pub origin: String,
    pub provenance: String,
    pub tree: Option<Tree>,
    pub baseline: Option<Baseline>,
    pub names: Vec<Name>,
    pub links: Vec<Link>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Verdict {
    pub verdict: String,
    pub origin: String,
    pub via: String,
    pub reason: String,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Suspended {
    pub reason: String,
    pub options: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Record {
    Asset {
        #[serde(rename = "record_key")]
        key: String,
        hash: SourceHash,
    },
    Entity {
        #[serde(rename = "record_key")]
        key: String,
        definition: Box<Entity>,
    },
    ImageVerdict {
        #[serde(rename = "record_key")]
        key: String,
        asset: String,
        term: String,
        decision: Verdict,
    },
    RelationVerdict {
        #[serde(rename = "record_key")]
        key: String,
        a: String,
        b: String,
        kind: String,
        order: i64,
        identity_relation: bool,
        decision: Verdict,
        seen: Vec<String>,
        reviewed: Vec<String>,
    },
    IdentityContext {
        #[serde(rename = "record_key")]
        key: String,
        asset: String,
        term: String,
        scope: Option<Vec<String>>,
        suspended: Option<Suspended>,
    },
    Curation {
        #[serde(rename = "record_key")]
        key: String,
        asset: String,
        state: String,
        origin: String,
        created_at: String,
    },
    Deferred {
        #[serde(rename = "record_key")]
        key: String,
        asset: String,
        state: String,
        origin: String,
        created_at: Option<String>,
    },
    Uncertain {
        #[serde(rename = "record_key")]
        key: String,
        asset: String,
        state: String,
        options: Vec<String>,
        reason: String,
        origin: String,
        created_at: String,
    },
    Review {
        #[serde(rename = "record_key")]
        key: String,
        term: String,
        reviewed_at: String,
    },
    IdentitySkip {
        #[serde(rename = "record_key")]
        key: String,
        pair: String,
    },
    IdentityKeepApart {
        #[serde(rename = "record_key")]
        key: String,
        a: String,
        b: String,
    },
    AnchorDecision {
        #[serde(rename = "record_key")]
        key: String,
        term: String,
        reference: String,
        reference_version: Option<String>,
        decision: String,
        reason: String,
        created_at: String,
    },
}

impl Record {
    pub(crate) fn identity(&self) -> (&'static str, &str) {
        match self {
            Self::Asset { key, .. } => ("asset", key),
            Self::Entity { key, .. } => ("entity", key),
            Self::ImageVerdict { key, .. } => ("image_verdict", key),
            Self::RelationVerdict { key, .. } => ("relation_verdict", key),
            Self::IdentityContext { key, .. } => ("identity_context", key),
            Self::Curation { key, .. } => ("curation", key),
            Self::Deferred { key, .. } => ("deferred", key),
            Self::Uncertain { key, .. } => ("uncertain", key),
            Self::Review { key, .. } => ("review", key),
            Self::IdentitySkip { key, .. } => ("identity_skip", key),
            Self::IdentityKeepApart { key, .. } => ("identity_keep_apart", key),
            Self::AnchorDecision { key, .. } => ("anchor_decision", key),
        }
    }

    pub(crate) fn assets(&self) -> Vec<&str> {
        match self {
            Self::ImageVerdict { asset, .. }
            | Self::IdentityContext { asset, .. }
            | Self::Curation { asset, .. }
            | Self::Deferred { asset, .. }
            | Self::Uncertain { asset, .. } => vec![asset],
            _ => Vec::new(),
        }
    }

    pub(crate) fn terms(&self) -> Vec<&str> {
        match self {
            Self::Entity { definition, .. } => {
                let mut out = Vec::new();
                if let Some(tree) = &definition.tree {
                    out.extend(
                        [tree.parent.as_deref(), tree.merged_into.as_deref(), tree.work.as_deref()]
                            .into_iter()
                            .flatten(),
                    );
                }
                if let Some(baseline) = &definition.baseline {
                    out.extend([baseline.parent.as_deref(), baseline.merged_into.as_deref()].into_iter().flatten());
                }
                out
            }
            Self::ImageVerdict { term, .. } | Self::Review { term, .. } | Self::AnchorDecision { term, .. } => {
                vec![term]
            }
            Self::RelationVerdict { a, b, .. } | Self::IdentityKeepApart { a, b, .. } => vec![a, b],
            Self::IdentityContext { term, scope, suspended, .. } => {
                let mut out = vec![term.as_str()];
                out.extend(scope.iter().flatten().map(String::as_str));
                if let Some(s) = suspended {
                    out.extend(s.options.iter().map(String::as_str));
                }
                out
            }
            Self::Uncertain { options, .. } => options.iter().map(String::as_str).collect(),
            _ => Vec::new(),
        }
    }

    pub(crate) fn logical_key(&self) -> Vec<&str> {
        let (kind, key) = self.identity();
        match self {
            Self::ImageVerdict { asset, term, .. } | Self::IdentityContext { asset, term, .. } => {
                vec![kind, asset, term]
            }
            // same_as 的方向决定代表，记录本身保留原方向；去重时两个方向占用同一逻辑关系。
            Self::RelationVerdict { a, b, kind: relation, .. } if relation == "same_as" && a > b => {
                vec![kind, b, a, relation]
            }
            Self::RelationVerdict { a, b, kind: relation, .. } => vec![kind, a, b, relation],
            Self::IdentitySkip { pair, .. } => vec![kind, pair],
            Self::IdentityKeepApart { a, b, .. } => vec![kind, a, b],
            Self::Curation { asset, .. } | Self::Deferred { asset, .. } | Self::Uncertain { asset, .. } => {
                vec![kind, asset]
            }
            Self::Review { term, .. } => vec![kind, term],
            Self::AnchorDecision { term, reference, .. } => vec![kind, term, reference],
            _ => vec![kind, key],
        }
    }

    fn validate(&self) -> Result<()> {
        key(self.identity().1)?;
        for value in self.terms().into_iter().chain(self.assets()) {
            key(value)?;
        }
        match self {
            Self::Asset { hash, .. } => {
                hash.content()?;
            }
            Self::Entity { definition: e, .. } => {
                if crate::schema::Kind::parse(&e.kind).is_none()
                    || !matches!(e.provenance.as_str(), "user" | "legacy_unknown" | "dependency")
                {
                    return invalid("词条定义类型或来源不正确");
                }
                if e.kind == "mark"
                    && (e.origin != "user"
                        || e.tree.is_some()
                        || e.baseline.is_some()
                        || !e.names.is_empty()
                        || !e.links.is_empty())
                {
                    return invalid("人工标记定义不能携带身份、名字或链接");
                }
                bounded(self.identity().1, 1, 128)?;
                bounded(&e.origin, 1, 80)?;
                if let Some(t) = &e.tree {
                    if t.review_status.is_some() != t.review_note.is_some()
                        || t.review_status.is_some() != t.parent_locked.is_some()
                    {
                        return invalid("作品复核三字段必须同时存在或同时为空");
                    }
                }
                if e.baseline
                    .as_ref()
                    .and_then(|b| b.kind.as_deref())
                    .is_some_and(|k| crate::schema::Kind::parse(k).is_none())
                {
                    return invalid("身份基线类型不正确");
                }
                let mut names = BTreeSet::new();
                let mut preferred = BTreeSet::new();
                for n in &e.names {
                    if !matches!(n.lang.as_str(), "" | "zh" | "ja" | "en")
                        || !matches!(n.role.as_str(), "display" | "alias")
                        || !names.insert((&n.lang, &n.role, &n.source, &n.value))
                        || (n.preferred && !preferred.insert((&n.lang, &n.role)))
                        || !matches!(n.provenance.as_str(), "user" | "legacy_merge" | "legacy_unknown" | "description")
                        || (n.provenance == "description" && n.preferred)
                        || (n.provenance == "user" && (n.source != "user" || (n.role == "display" && !n.preferred)))
                        || (n.provenance == "legacy_merge" && n.role != "alias")
                    {
                        return invalid("名字定义重复或不正确");
                    }
                    bounded(&n.value, 1, 200)?;
                    bounded(&n.source, 1, 80)?;
                }
                let mut links = BTreeSet::new();
                for l in &e.links {
                    bounded(&l.system, 1, 32)?;
                    bounded(&l.key, 1, 256)?;
                    bounded(&l.source, 1, 80)?;
                    if !links.insert((&l.system, &l.key)) {
                        return invalid("外部链接定义重复");
                    }
                }
            }
            Self::ImageVerdict { decision, .. } | Self::RelationVerdict { decision, .. } => {
                one_of(&decision.verdict, &["yes", "no", "clear"])?;
                one_of(&decision.origin, &["user", "album", "legacy"])?;
                bounded(&decision.via, 0, 40)?;
            }
            Self::Curation { state, origin, .. } => {
                one_of(state, &["included", "excluded", "reset"])?;
                bounded(origin, 1, 80)?;
            }
            Self::Deferred { state, origin, created_at, .. } => {
                one_of(state, &["deferred", "resumed"])?;
                bounded(origin, 1, 80)?;
                if state == "resumed" && created_at.is_none() {
                    return invalid("恢复标记缺少原操作时间");
                }
            }
            Self::Uncertain { state, options, reason, origin, .. } => {
                one_of(state, &["uncertain", "cleared"])?;
                bounded(origin, 1, 80)?;
                if (state == "cleared" && (!options.is_empty() || !reason.is_empty()))
                    || (state == "uncertain" && (!(2..=20).contains(&options.len()) || reason.trim().is_empty()))
                {
                    return invalid("未定状态的候选或理由不正确");
                }
                unique(options)?;
            }
            Self::IdentityContext { scope, suspended, .. } => {
                if scope.is_none() && suspended.is_none() {
                    return invalid("身份上下文不能同时缺少范围和暂停状态");
                }
                if let Some(scope) = scope {
                    unique(scope)?;
                }
                if let Some(s) = suspended {
                    unique(&s.options)?;
                }
            }
            _ => {}
        }
        if let Self::RelationVerdict { a, b, kind, seen, reviewed, .. } = self {
            one_of(kind, &["contains", "same_as", "different_from", "excludes"])?;
            if a == b || (matches!(kind.as_str(), "different_from" | "excludes") && a > b) {
                return invalid("身份关系端点不正确");
            }
            unique(seen)?;
            unique(reviewed)?;
            let seen: BTreeSet<_> = seen.iter().collect();
            if reviewed.iter().any(|v| !seen.contains(v)) {
                return invalid("已审证据必须属于所见证据");
            }
        }
        if matches!(self, Self::IdentityKeepApart { a, b, .. } if a >= b) {
            return invalid("保持分开需要两个不同的身份");
        }
        if let Self::IdentitySkip { pair, .. } = self {
            // 现行入口允许任意键，且以竖线拼接。保持原串，不把无法唯一解析的旧键捏造成两个身份。
            bounded(pair, 1, MAX_LINE as usize)?;
        }
        if let Self::AnchorDecision { reference, reference_version, decision, .. } = self {
            key(reference)?;
            if let Some(version) = reference_version {
                key(version)?;
            }
            one_of(decision, &["exclude", "restore"])?;
        }
        Ok(())
    }
}

fn bounded(value: &str, min: usize, max: usize) -> Result<()> {
    if !(min..=max).contains(&value.chars().count()) || value.contains('\0') {
        return invalid("人工结论字段长度或内容不正确");
    }
    Ok(())
}
fn key(value: &str) -> Result<()> {
    bounded(value, 1, 256)
}
fn one_of(value: &str, choices: &[&str]) -> Result<()> {
    if !choices.contains(&value) {
        return invalid("不支持的人工状态值");
    }
    Ok(())
}
fn unique(values: &[String]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for v in values {
        key(v)?;
        if !seen.insert(v) {
            return invalid("人工状态包含重复引用");
        }
    }
    Ok(())
}

/// 哈希覆盖按源资产数计；last_known 保留历史内容证明，不承诺源文件当前在线。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coverage {
    /// 快照内的资产总数。
    pub total: u64,
    /// 导出时在线核验成功的资产数。
    pub verified: u64,
    /// 有有效历史摘要的离线资产数。
    pub last_known: u64,
    /// 尚无有效摘要的资产数。
    pub missing: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Footer {
    #[serde(rename = "type")]
    record: String,
    counts: BTreeMap<String, u64>,
    hash_coverage: Coverage,
    asset_identity_complete: bool,
    sha256: String,
    #[serde(rename = "state_sha256")]
    semantic_sha256: String,
}

/// 已通过字段、引用、计数和完整性校验的人工状态快照。
pub struct Snapshot {
    pub(crate) header: Header,
    pub(crate) records: Vec<Record>,
}

impl Snapshot {
    /// 读取已验证的格式头。
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// 每条白名单记录的类别与源文件键；供宿主生成可分页的导出报告。
    pub fn record_keys(&self) -> impl Iterator<Item = (&'static str, &str)> + '_ {
        self.records.iter().map(Record::identity)
    }

    /// 需要宿主核验完整内容的源资产键，包括已离开当前来源的资产。
    pub fn asset_keys(&self) -> impl Iterator<Item = &str> {
        self.records.iter().filter_map(|r| match r {
            Record::Asset { key, .. } => Some(key.as_str()),
            _ => None,
        })
    }

    /// 填入宿主已经核验、且与这份读快照的内容版本绑定的摘要。未提供的资产继续报告 missing。
    pub fn with_hashes(mut self, hashes: &BTreeMap<String, SourceHash>) -> Result<Self> {
        let known: BTreeSet<_> = self.asset_keys().collect();
        if hashes.keys().any(|key| !known.contains(key.as_str())) {
            return invalid("摘要包含快照以外的资产");
        }
        for record in &mut self.records {
            if let Record::Asset { key, hash } = record {
                if let Some(value) = hashes.get(key) {
                    value.content()?;
                    *hash = value.clone();
                }
            }
        }
        self.validate()?;
        Ok(self)
    }

    /// 计算资产摘要的覆盖，不受调用方传来的 footer 数字影响。
    pub fn coverage(&self) -> Coverage {
        let mut out = Coverage::default();
        for r in &self.records {
            if let Record::Asset { hash, .. } = r {
                out.total += 1;
                match hash {
                    SourceHash::Verified { .. } => out.verified += 1,
                    SourceHash::LastKnown { .. } => out.last_known += 1,
                    SourceHash::Missing => out.missing += 1,
                }
            }
        }
        out
    }

    /// 稳定语义摘要；忽略文件记录次序、快照随机编号和导出时间，保留人工操作的来源时间。
    pub fn semantic_sha256(&self) -> Result<String> {
        let mut hash = Sha256::new();
        hash.update(encode(&serde_json::json!({"format":"illustash-human-conclusions","version":1,"mode":"effective-state","library_id":self.header.library_id,"identity_policy":self.header.policy}))?);
        let mut records: Vec<_> = self.records.iter().collect();
        records.sort_by_key(|r| r.identity());
        for record in records {
            hash.update(encode(record)?);
        }
        Ok(strict_json::hex(&hash.finalize()))
    }

    fn counts(&self) -> BTreeMap<String, u64> {
        let mut out = BTreeMap::new();
        for record in &self.records {
            *out.entry(record.identity().0.into()).or_default() += 1;
        }
        out
    }

    pub(crate) fn validate(&self) -> Result<()> {
        self.header.validate()?;
        if self.records.len() > MAX_RECORDS {
            return Err(Error::Limit("人工结论超过格式 v1 的记录数量上限".into()));
        }
        let mut ids = BTreeSet::new();
        let mut logical = BTreeSet::new();
        let mut assets = BTreeSet::new();
        let mut entities = BTreeMap::new();
        let mut external = BTreeSet::new();
        let mut relation_order = BTreeSet::new();
        let mut baseline_order = BTreeSet::new();
        let mut tree_order = BTreeSet::new();
        for record in &self.records {
            record.validate()?;
            if !ids.insert(record.identity()) || !logical.insert(record.logical_key()) {
                return invalid("人工结论记录重复");
            }
            match record {
                Record::Asset { key, .. } => {
                    assets.insert(key.as_str());
                }
                Record::RelationVerdict { order, .. } if !relation_order.insert(order) => {
                    return invalid("关系相对顺序重复")
                }
                Record::Entity { key, definition } => {
                    entities.insert(key.as_str(), record);
                    if definition.baseline.as_ref().is_some_and(|b| !baseline_order.insert(b.order))
                        || definition.tree.as_ref().is_some_and(|t| !tree_order.insert(t.order))
                    {
                        return invalid("身份结构相对顺序重复");
                    }
                    for link in &definition.links {
                        if !external.insert((&link.system, &link.key)) {
                            return invalid("多个定义占用了同一外部链接");
                        }
                    }
                    if self.header.policy == Policy::Default && definition.baseline.is_some() {
                        return invalid("默认策略不能携带 hybrid 身份基线");
                    }
                }
                _ => {}
            }
        }
        for record in &self.records {
            if record.assets().iter().any(|k| !assets.contains(k))
                || record.terms().iter().any(|k| !entities.contains_key(k))
            {
                return invalid("人工结论缺少引用的资产或词条定义");
            }
            if let Record::ImageVerdict { term, decision, .. } = record {
                if decision.verdict == "no"
                    && matches!(entities[term.as_str()], Record::Entity { definition, .. } if definition.kind == "mark")
                {
                    return invalid("人工标记只能添加或取消");
                }
            }
        }
        // 当前结构与 hybrid 原始基线属于不同时间的图，分别查环；跨时间相反的边不能拼成假环。
        // 无递归检查让长依赖链也不会耗尽调用栈。
        for baseline in [false, true] {
            let mut degrees = BTreeMap::new();
            let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
            for (&key, record) in &entities {
                let Record::Entity { definition, .. } = record else { unreachable!("只登记实体") };
                let refs: BTreeSet<_> = if baseline {
                    definition
                        .baseline
                        .as_ref()
                        .map(|b| vec![b.parent.as_deref(), b.merged_into.as_deref()])
                        .unwrap_or_default()
                } else {
                    definition
                        .tree
                        .as_ref()
                        .map(|t| vec![t.parent.as_deref(), t.merged_into.as_deref(), t.work.as_deref()])
                        .unwrap_or_default()
                }
                .into_iter()
                .flatten()
                .collect();
                degrees.insert(key, refs.len());
                for dependency in refs {
                    dependents.entry(dependency).or_default().push(key);
                }
            }
            let mut queue: VecDeque<_> =
                degrees.iter().filter(|(_, count)| **count == 0).map(|(&key, _)| key).collect();
            let mut visited = 0;
            while let Some(key) = queue.pop_front() {
                visited += 1;
                for child in dependents.get(key).into_iter().flatten() {
                    let count = degrees.get_mut(child).expect("依赖图的端点已经校验");
                    *count -= 1;
                    if *count == 0 {
                        queue.push_back(child);
                    }
                }
            }
            if visited != entities.len() {
                return invalid("人工定义存在依赖环");
            }
        }
        super::format_graph::validate(self)?;
        Ok(())
    }

    /// 写出经过自校验的 JSONL；调用方负责私有临时文件、刷盘和原子发布。
    pub fn write(&self, output: &mut impl Write) -> Result<()> {
        self.write_checked(output, &|| Ok(()))
    }

    /// 按行响应宿主取消，完整写入后仍由宿主决定是否发布文件。
    pub fn write_checked(&self, output: &mut impl Write, check: &dyn Fn() -> Result<()>) -> Result<()> {
        check()?;
        self.validate()?;
        let mut digest = Sha256::new();
        let mut bytes = 0;
        let header = encode(&self.header)?;
        write_line(output, &header, &mut bytes)?;
        digest.update(&header);
        for record in &self.records {
            check()?;
            let line = encode(record)?;
            write_line(output, &line, &mut bytes)?;
            digest.update(line);
        }
        let coverage = self.coverage();
        let footer = Footer {
            record: "footer".into(),
            counts: self.counts(),
            asset_identity_complete: coverage.missing == 0,
            hash_coverage: coverage,
            sha256: strict_json::hex(&digest.finalize()),
            semantic_sha256: self.semantic_sha256()?,
        };
        write_line(output, &encode(&footer)?, &mut bytes)
    }
}

fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut line = serde_json::to_vec(value).map_err(|e| Error::invalid(e.to_string()))?;
    line.push(b'\n');
    Ok(line)
}
fn write_line(out: &mut impl Write, line: &[u8], bytes: &mut u64) -> Result<()> {
    *bytes += line.len() as u64;
    if line.len() as u64 > MAX_LINE || *bytes > MAX_BYTES {
        return Err(Error::Limit("人工结论超过格式 v1 的文件或单行大小上限".into()));
    }
    out.write_all(line)?;
    Ok(())
}
fn decode<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Result<T> {
    serde_json::from_value(value).map_err(|e| Error::invalid(format!("人工结论记录不正确：{e}")))
}

/// 严格读取一份完整快照；限制逐行读取长度，校验原始字节摘要、语义摘要及所有引用。
pub fn read(input: &mut impl BufRead) -> Result<Snapshot> {
    read_checked(input, &|| Ok(()))
}

/// 严格读取并在每行检查取消，解析成功前不打开目标写事务。
pub fn read_checked(input: &mut impl BufRead, check: &dyn Fn() -> Result<()>) -> Result<Snapshot> {
    let mut digest = Sha256::new();
    let mut bytes = 0;
    let mut header = None;
    let mut records = Vec::new();
    let mut footer = None;
    loop {
        check()?;
        let mut line = Vec::new();
        let n = (&mut *input).take(MAX_LINE + 1).read_until(b'\n', &mut line)?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        if n as u64 > MAX_LINE || bytes > MAX_BYTES {
            return Err(Error::Limit("人工结论超过格式 v1 的文件或单行大小上限".into()));
        }
        if footer.is_some() {
            return invalid("人工结论文件尾部还有内容");
        }
        if line.last() != Some(&b'\n') {
            return invalid("人工结论文件被截断");
        }
        let text = std::str::from_utf8(&line).map_err(|_| Error::invalid("人工结论文件须为 UTF-8"))?;
        let value = strict_json::parse(text).map_err(Error::invalid)?;
        if header.is_none() {
            let parsed: Header = decode(value)?;
            parsed.validate()?;
            header = Some(parsed);
        } else if value["type"] == "footer" {
            footer = Some(decode::<Footer>(value)?);
            continue;
        } else {
            if records.len() == MAX_RECORDS {
                return Err(Error::Limit("人工结论超过格式 v1 的记录数量上限".into()));
            }
            records.push(decode::<Record>(value)?);
        }
        digest.update(&line);
    }
    let (Some(header), Some(footer)) = (header, footer) else { return invalid("人工结论文件缺少完整头尾") };
    let snapshot = Snapshot { header, records };
    snapshot.validate()?;
    let coverage = snapshot.coverage();
    if footer.record != "footer"
        || footer.sha256 != strict_json::hex(&digest.finalize())
        || footer.semantic_sha256 != snapshot.semantic_sha256()?
        || footer.counts != snapshot.counts()
        || footer.hash_coverage != coverage
        || footer.asset_identity_complete != (coverage.missing == 0)
    {
        return invalid("人工结论文件摘要、计数或哈希覆盖不一致");
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sample() -> Snapshot {
        Snapshot {
            header: Header::new(
                "snapshot-a".into(),
                "library-a".into(),
                "2026-11-15T00:00:00Z".into(),
                Policy::Default,
            ),
            records: vec![
                Record::Asset { key: "a".into(), hash: SourceHash::Missing },
                Record::Curation {
                    key: "curation:a".into(),
                    asset: "a".into(),
                    state: "reset".into(),
                    origin: "user".into(),
                    created_at: "2026-01-01 00:00:00".into(),
                },
            ],
        }
    }

    fn bytes(snapshot: &Snapshot) -> Vec<u8> {
        let mut out = Vec::new();
        snapshot.write(&mut out).unwrap();
        out
    }

    #[test]
    fn round_trip_keeps_explicit_reset_and_reports_incomplete_hash_coverage() {
        let original = sample();
        let wire = bytes(&original);
        let got = read(&mut Cursor::new(&wire)).unwrap();
        assert_eq!(got.header, original.header);
        assert_eq!(got.records, original.records);
        assert_eq!(got.coverage(), Coverage { total: 1, missing: 1, ..Coverage::default() });
        assert_eq!(wire, bytes(&got));
    }

    #[test]
    fn semantic_digest_ignores_only_export_identity_time_and_record_order() {
        let mut other = sample();
        let expected = other.semantic_sha256().unwrap();
        other.header.snapshot_id = "snapshot-b".into();
        other.header.exported_at = "2026-11-16T12:00:00Z".into();
        other.records.reverse();
        assert_eq!(other.semantic_sha256().unwrap(), expected);
        if let Record::Curation { created_at, .. } = &mut other.records[0] {
            *created_at = "changed-human-time".into();
        }
        assert_ne!(other.semantic_sha256().unwrap(), expected);
    }

    #[test]
    fn truncation_trailing_data_duplicate_fields_unknown_fields_and_tampered_counts_fail() {
        let wire = String::from_utf8(bytes(&sample())).unwrap();
        let failures = [
            wire[..wire.len() - 1].to_owned(),
            format!("{wire}\n"),
            wire.replace("\"state\":\"reset\"", "\"state\":\"reset\",\"state\":\"included\""),
            wire.replace("\"state\":\"reset\"", "\"state\":\"reset\",\"filename\":\"private.jpg\""),
            wire.replace("\"total\":1", "\"total\":2"),
            wire.replace("\"reset\"", "\"included\""),
            wire.replace("\"version\":1", "\"version\":2"),
        ];
        for invalid in failures {
            assert!(read(&mut Cursor::new(invalid)).is_err());
        }
    }

    #[test]
    fn duplicate_semantics_and_missing_dependencies_fail_even_with_different_record_keys() {
        let mut snapshot = sample();
        let mut duplicate = snapshot.records[1].clone();
        if let Record::Curation { key, .. } = &mut duplicate {
            *key = "other-key".into();
        }
        snapshot.records.push(duplicate);
        assert!(snapshot.validate().is_err());
        snapshot.records.pop();
        snapshot.records.remove(0);
        assert!(snapshot.validate().is_err());
    }

    fn entity(key: &str, parent: Option<&str>) -> Record {
        Record::Entity {
            key: key.into(),
            definition: Box::new(Entity {
                kind: "character".into(),
                origin: "user".into(),
                provenance: "user".into(),
                names: vec![],
                links: vec![],
                baseline: Some(Baseline {
                    parent: parent.map(str::to_owned),
                    variant: None,
                    kind: None,
                    merged_into: None,
                    order: i64::from(key.as_bytes()[0]),
                }),
                tree: None,
            }),
        }
    }

    #[test]
    fn dependency_cycles_are_rejected_without_recursing() {
        let mut snapshot = sample();
        snapshot.header.policy = Policy::Hybrid;
        snapshot.records.extend([entity("x", Some("y")), entity("y", Some("x"))]);
        assert!(snapshot.validate().is_err());
        snapshot.records.pop();
        snapshot.records.push(entity("y", None));
        snapshot.validate().unwrap();
    }

    #[test]
    fn legacy_skipped_pair_remains_opaque_when_its_keys_no_longer_resolve() {
        let mut snapshot = sample();
        snapshot.records.push(Record::IdentitySkip { key: "skip:a".into(), pair: "old|key|missing".into() });
        let result = read(&mut Cursor::new(bytes(&snapshot))).unwrap();
        assert_eq!(result.records, snapshot.records);
    }

    #[test]
    fn long_unterminated_line_stops_at_the_bounded_read() {
        let wire = vec![b' '; MAX_LINE as usize + 100];
        let mut reader = Cursor::new(wire);
        assert!(read(&mut reader).is_err());
        assert_eq!(reader.position(), MAX_LINE + 1);
    }
    fn relation(order: i64, a: &str, b: &str, kind: &str) -> Record {
        Record::RelationVerdict {
            key: format!("relation-{order}"),
            a: a.into(),
            b: b.into(),
            kind: kind.into(),
            order,
            identity_relation: true,
            decision: Verdict {
                verdict: "yes".into(),
                origin: "user".into(),
                via: "identity".into(),
                reason: String::new(),
                created_at: "old".into(),
            },
            seen: vec![],
            reviewed: vec![],
        }
    }
    #[test]
    fn hybrid_relation_cycles_multiple_parents_and_different_equivalence_fail() {
        for relations in [
            vec![relation(0, "a", "b", "contains"), relation(1, "b", "a", "contains")],
            vec![relation(0, "a", "b", "contains"), relation(1, "c", "b", "contains")],
            vec![relation(0, "a", "b", "same_as"), relation(1, "a", "b", "different_from")],
            vec![relation(0, "a", "b", "contains"), relation(1, "a", "b", "different_from")],
            vec![relation(0, "a", "b", "same_as"), relation(1, "a", "b", "contains")],
        ] {
            let mut snapshot = sample();
            snapshot.header.policy = Policy::Hybrid;
            snapshot.records.extend([entity("a", None), entity("b", None), entity("c", None)]);
            snapshot.records.extend(relations);
            assert!(snapshot.validate().is_err());
        }
        let mut snapshot = sample();
        snapshot.header.policy = Policy::Hybrid;
        snapshot.records.extend([
            entity("a", None),
            entity("b", None),
            entity("c", None),
            relation(0, "a", "b", "contains"),
            relation(1, "a", "c", "different_from"),
        ]);
        snapshot.validate().unwrap();
    }
    #[test]
    fn same_as_reverse_duplicates_and_conflicts_share_one_key_without_changing_direction() {
        for verdict in ["yes", "no", "clear"] {
            let mut snapshot = sample();
            snapshot.header.policy = Policy::Hybrid;
            snapshot.records.extend([entity("a", None), entity("b", None), relation(0, "b", "a", "same_as")]);
            let encoded = bytes(&snapshot);
            let restored = read(&mut Cursor::new(encoded)).unwrap();
            assert!(matches!(restored.records.last(),Some(Record::RelationVerdict{a,b,..}) if a=="b" && b=="a"));
            let mut reverse = relation(1, "a", "b", "same_as");
            if let Record::RelationVerdict { decision, .. } = &mut reverse {
                decision.verdict = verdict.into();
            }
            snapshot.records.push(reverse);
            assert!(snapshot.validate().is_err(), "{verdict}");
        }
    }
    #[test]
    fn hash_attachment_requires_snapshot_membership_and_round_trips_states() {
        use super::super::matching::Content;
        let content = Content { sha256: "a".repeat(64), byte_length: 5, representation: "original".into() };
        assert!(sample().with_hashes(&BTreeMap::from([("outside".into(), SourceHash::Missing)])).is_err());
        for hash in [
            SourceHash::Verified { content: content.clone(), version: "version".into() },
            SourceHash::LastKnown { content: content.clone(), version: "version".into() },
        ] {
            let snapshot = sample().with_hashes(&BTreeMap::from([("a".into(), hash)])).unwrap();
            let wire = bytes(&snapshot);
            let encoded = String::from_utf8(wire.clone()).unwrap();
            assert!(encoded.contains("\"type\":\"header\""));
            assert!(encoded.contains("\"mode\":\"effective-state\""));
            assert!(encoded.contains("\"state_sha256\":"));
            assert!(!encoded.contains("portable_complete"));
            assert_eq!(read(&mut Cursor::new(wire)).unwrap().records, snapshot.records);
            assert_eq!(snapshot.coverage().missing, 0);
        }
    }
}
