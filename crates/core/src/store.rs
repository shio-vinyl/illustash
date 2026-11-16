//! 词条核心的最小读写规则：登记、裁决与撤销、投影、证据包导入。
//!
//! 带 `&mut Connection` 的是命令：一个 `BEGIN IMMEDIATE` 事务，失败不留部分写入。
//! 带 `&Connection` 的是在命令事务里调用的步骤，也可以由调用方自己包事务。

use crate::error::invalid;
use crate::package::{AssetRef, Package, Target, TermRef};
use crate::schema::{Exclusive, Judge, Kind, RelationKind, HUMAN_ORIGINS};
use crate::strict_json::{canonical, hex};
use crate::{Error, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use sha2::Digest;
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub type AssetId = i64;
pub type TermId = i64;
pub type RelationId = i64;
pub type ActionId = i64;

/// 新词条的 key：随机 UUID v4，永不改变。
///
/// 只供测试：设置 `ILLUSTASH_TEST_KEY_COUNTER` 为一个计数文件时，按文件里的计数依次生成可复现的键，
/// 测试与固定期望（native/terms/tests/baseline.rs 等新对照基准与 tests/fixed_expectations.py，#201）因此得到同样的键和同样的排序。
/// 计数在旁边的锁文件下递增，同一宿主的多个线程和并行的命令行进程也不会取到同一个数。产品从不设置它。
pub fn new_key() -> Result<String> {
    let mut b = [0u8; 16];
    if let Some(counter) = std::env::var_os("ILLUSTASH_TEST_KEY_COUNTER") {
        let n = next_test_key(std::path::Path::new(&counter))?;
        b.copy_from_slice(&sha2::Sha256::digest(n.to_le_bytes())[..16]);
    } else {
        getrandom::fill(&mut b).map_err(|e| Error::Invalid(format!("无法生成随机 ID：{e}")))?;
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex(&b);
    Ok(format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]))
}

fn next_test_key(counter: &std::path::Path) -> Result<u64> {
    let lock = counter.with_extension("lock");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::fs::OpenOptions::new().write(true).create_new(true).open(&lock).is_err() {
        if std::time::Instant::now() > deadline {
            return Err(Error::Invalid(format!("测试键计数器被占用：{path}", path = lock.display())));
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let n = std::fs::read_to_string(counter).ok().and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0) + 1;
    let written = std::fs::write(counter, n.to_string());
    let _ = std::fs::remove_file(&lock);
    written?;
    Ok(n)
}

// ---- 资产 ----

/// 登记一张图（已登记就返回原来的 id；给了 md5 就补上）。移除过的图重新出现时恢复。
pub fn register_asset(conn: &Connection, namespace: &str, key: &str, md5: Option<&str>) -> Result<AssetId> {
    conn.prepare_cached(
        "INSERT INTO term_assets(namespace,key,md5) VALUES (?1,?2,?3)
         ON CONFLICT(namespace,key) DO UPDATE SET md5=coalesce(excluded.md5,md5),removed_at=NULL",
    )?
    .execute(params![namespace, key, md5])?;
    Ok(conn
        .prepare_cached("SELECT id FROM term_assets WHERE namespace=?1 AND key=?2")?
        .query_row([namespace, key], |r| r.get(0))?)
}

/// 图从图库移除：证据和裁决都留着（只增不删），投影清掉。
pub fn remove_asset(conn: &Connection, asset: AssetId) -> Result<()> {
    conn.execute("UPDATE term_assets SET removed_at=CURRENT_TIMESTAMP WHERE id=?1 AND removed_at IS NULL", [asset])?;
    project(conn, &[asset])
}

fn live_asset(conn: &Connection, asset: &AssetRef, new_assets: bool) -> Result<Option<AssetId>> {
    let found = conn
        .prepare_cached("SELECT id FROM term_assets WHERE namespace=?1 AND key=?2 AND removed_at IS NULL")?
        .query_row([&asset.namespace, &asset.id], |r| r.get(0))
        .optional()?;
    match found {
        // 同步导入新图时，WD 证据先于宿主把图写进 assets 交过来（docs/TERMS_LABELS.md）：允许先登记。
        None if asset.namespace == LIBRARY && new_assets => Ok(Some(register_asset(conn, LIBRARY, &asset.id, None)?)),
        None if asset.namespace == LIBRARY => library_asset(conn, &asset.id),
        found => Ok(found),
    }
}

/// 本图库（命名空间 `library`）的一张图：同库 `assets` 里有而核心还没登记的，现在登记（同步刚导入的新图）。
/// 不在 `assets` 里的返回 None。
pub fn library_asset(conn: &Connection, uuid: &str) -> Result<Option<AssetId>> {
    const LIVE: &str = "SELECT id FROM term_assets WHERE namespace='library' AND key=?1 AND removed_at IS NULL";
    if let Some(id) = conn.prepare_cached(LIVE)?.query_row([uuid], |r| r.get(0)).optional()? {
        return Ok(Some(id));
    }
    let has_assets: bool = conn
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='assets')")?
        .query_row([], |r| r.get(0))?;
    if !has_assets
        || conn
            .prepare_cached("SELECT 1 FROM assets WHERE uuid=?1")?
            .query_row([uuid], |_| Ok(()))
            .optional()?
            .is_none()
    {
        return Ok(None);
    }
    Ok(Some(register_asset(conn, LIBRARY, uuid, None)?))
}

// ---- 词条、名字、链接、关系 ----

pub fn create_term(conn: &Connection, kind: Kind, key: Option<&str>, origin: &str) -> Result<TermId> {
    let key = match key {
        Some(k) => k.to_owned(),
        None => new_key()?,
    };
    conn.execute("INSERT INTO terms(key,kind,origin) VALUES (?1,?2,?3)", [&key, kind.as_str(), origin]).map_err(
        |e| match e {
            rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => {
                Error::Conflict(format!("词条 key 已存在：{key}"))
            }
            e => e.into(),
        },
    )?;
    Ok(conn.query_row("SELECT id FROM terms WHERE key=?1", [&key], |r| r.get(0))?)
}

pub fn term_by_key(conn: &Connection, key: &str) -> Result<Option<(TermId, Kind)>> {
    let row: Option<(TermId, String)> = conn
        .prepare_cached("SELECT id,kind FROM terms WHERE key=?1")?
        .query_row([key], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    Ok(row.map(|(id, k)| (id, Kind::parse(&k).expect("类型由触发器约束"))))
}

pub fn term_by_link(conn: &Connection, system: &str, key: &str) -> Result<Option<TermId>> {
    Ok(conn
        .prepare_cached("SELECT term_id FROM term_links WHERE system=?1 AND key=?2")?
        .query_row([system, key], |r| r.get(0))
        .optional()?)
}

pub fn add_name(conn: &Connection, term: TermId, lang: &str, value: &str, role: &str, source: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO term_names(term_id,lang,value,role,source) VALUES (?1,?2,?3,?4,?5)",
        params![term, lang, value, role, source],
    )?;
    Ok(())
}

/// 外部链接只属于一个词条；已属于别的词条时拒绝（合并走 same_as 关系，不搬链接）。
pub fn add_link(conn: &Connection, term: TermId, system: &str, key: &str, url: &str, source: &str) -> Result<()> {
    match term_by_link(conn, system, key)? {
        Some(owner) if owner == term => Ok(()),
        Some(_) => Err(Error::Conflict(format!("链接 {system}:{key} 已属于另一个词条"))),
        None => {
            conn.execute(
                "INSERT INTO term_links(system,key,term_id,url,source) VALUES (?1,?2,?3,?4,?5)",
                params![system, key, term, url, source],
            )?;
            Ok(())
        }
    }
}

/// 取得或建立一条关系（只建端点，不代表任何结论）。对称关系存成 a < b；same_as 两个方向算同一条。
pub fn relation(conn: &Connection, a: TermId, b: TermId, kind: RelationKind) -> Result<RelationId> {
    if a == b {
        return invalid("关系两端不能是同一个词条");
    }
    let (a, b) = if kind.symmetric() && b < a { (b, a) } else { (a, b) };
    let existing: Option<RelationId> = if kind == RelationKind::SameAs {
        conn.query_row(
            "SELECT id FROM term_relations WHERE kind='same_as' AND ((a=?1 AND b=?2) OR (a=?2 AND b=?1))",
            [a, b],
            |r| r.get(0),
        )
        .optional()?
    } else {
        conn.query_row(
            "SELECT id FROM term_relations WHERE a=?1 AND b=?2 AND kind=?3",
            params![a, b, kind.as_str()],
            |r| r.get(0),
        )
        .optional()?
    };
    if let Some(id) = existing {
        return Ok(id);
    }
    if let Some(id) = crate::human_conclusions::visibility::ordinary_relation(conn, a, b, kind.as_str())? {
        return Ok(id);
    }
    conn.execute("INSERT INTO term_relations(a,b,kind) VALUES (?1,?2,?3)", params![a, b, kind.as_str()])?;
    Ok(conn.query_row(
        "SELECT id FROM term_relations WHERE a=?1 AND b=?2 AND kind=?3",
        params![a, b, kind.as_str()],
        |r| r.get(0),
    )?)
}

// ---- 裁决与撤销 ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerdictTarget {
    Image { asset: AssetId, term: TermId },
    Relation(RelationId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Yes,
    No,
    /// 人收回自己的结论：回到由证据决定。
    Clear,
}

impl Verdict {
    fn as_str(self) -> &'static str {
        match self {
            Verdict::Yes => "yes",
            Verdict::No => "no",
            Verdict::Clear => "clear",
        }
    }
}

/// 人工裁决：一步进统一历史（`actions`），同一事务里重算受影响的图。否定永久有效，直到人撤销或收回。
pub fn judge(
    conn: &mut Connection,
    targets: &[VerdictTarget],
    verdict: Verdict,
    origin: &str,
    reason: &str,
    label: &str,
) -> Result<ActionId> {
    // 和原命令一样，先拒绝不合法的参数，再取写锁。
    judge_args(targets, origin, label)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let action = judge_in(&tx, targets, verdict, origin, reason, label)?;
    tx.commit()?;
    Ok(action)
}

fn judge_args(targets: &[VerdictTarget], origin: &str, label: &str) -> Result<()> {
    if !HUMAN_ORIGINS.contains(&origin) {
        return invalid(format!("裁决只来自人，来源不能是 {origin}"));
    }
    if targets.is_empty() || targets.len() > 5000 || label.trim().is_empty() {
        return invalid("一次裁决 1–5000 个目标，并要有历史文案");
    }
    Ok(())
}

/// 同样的裁决规则，在应用持有的事务里执行（包括选择和概念创建）。
pub fn judge_in(
    conn: &Connection,
    targets: &[VerdictTarget],
    verdict: Verdict,
    origin: &str,
    reason: &str,
    label: &str,
) -> Result<ActionId> {
    judge_args(targets, origin, label)?;
    conn.execute(
        "INSERT INTO actions(label,before_json) VALUES (?1,?2)",
        [label, &canonical(&json!([{"terms": "verdicts"}]))],
    )?;
    let action = conn.last_insert_rowid();
    let mut touched = BTreeSet::new();
    for target in targets {
        match *target {
            VerdictTarget::Image { asset, term } => {
                let live: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM term_assets WHERE id=?1 AND removed_at IS NULL)
                        AND EXISTS(SELECT 1 FROM terms WHERE id=?2)",
                    [asset, term],
                    |r| r.get(0),
                )?;
                if !live {
                    return invalid("图或词条不存在");
                }
                if verdict == Verdict::No
                    && conn
                        .prepare_cached("SELECT kind='mark' FROM terms WHERE id=?")?
                        .query_row([term], |r| r.get::<_, bool>(0))?
                {
                    return invalid("人工标记只能添加或取消");
                }
                let reviewed = evidence_ids(conn, "asset_id=?1 AND term_id=?2", &[asset, term])?;
                conn.execute(
                    "INSERT INTO term_verdicts(action_id,anchor,asset_id,term_id,verdict,origin,reason,reviewed) VALUES (?1,?1,?2,?3,?4,?5,?6,?7)",
                    params![action, asset, term, verdict.as_str(), origin, reason, reviewed],
                )?;
                touched.insert(asset);
            }
            VerdictTarget::Relation(rel) => {
                let reviewed = evidence_ids(conn, "relation_id=?1", &[rel])?;
                conn.execute(
                    "INSERT INTO term_verdicts(action_id,anchor,relation_id,verdict,origin,reason,reviewed) VALUES (?1,?1,?2,?3,?4,?5,?6)",
                    params![action, rel, verdict.as_str(), origin, reason, reviewed],
                )?;
                crate::human_conclusions::relations::acknowledge(conn, conn.last_insert_rowid())?;
            }
        }
    }
    project(conn, &touched.into_iter().collect::<Vec<_>>())?;
    Ok(action)
}

fn evidence_ids(conn: &Connection, filter: &str, args: &[i64]) -> Result<String> {
    let mut stmt = conn.prepare(&format!("SELECT id FROM term_evidence WHERE {filter} ORDER BY id"))?;
    let ids: Vec<i64> =
        stmt.query_map(rusqlite::params_from_iter(args), |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(canonical(&json!(ids)))
}

/// 改词条类型（例如「只当标签」）：一步可撤销的历史。
pub fn set_kind(conn: &mut Connection, term: TermId, kind: Kind, label: &str) -> Result<ActionId> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (key, had): (String, String) = tx
        .query_row("SELECT key,kind FROM terms WHERE id=?1", [term], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .ok_or_else(|| Error::Invalid("词条不存在".into()))?;
    let before = json!([{"terms": {"kind": {"term": key, "had": had}}}]);
    tx.execute("INSERT INTO actions(label,before_json) VALUES (?1,?2)", [label, &canonical(&before)])?;
    let action = tx.last_insert_rowid();
    tx.execute("UPDATE terms SET kind=?1 WHERE id=?2", params![kind.as_str(), term])?;
    let assets = assets_of_term(&tx, term)?;
    project(&tx, &assets)?;
    tx.commit()?;
    Ok(action)
}

fn assets_of_term(conn: &Connection, term: TermId) -> Result<Vec<AssetId>> {
    let mut stmt = conn.prepare(
        "SELECT asset_id FROM term_evidence WHERE term_id=?1 UNION SELECT asset_id FROM term_verdicts WHERE term_id=?1",
    )?;
    let ids = stmt.query_map([term], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Undo {
    Undone {
        action: ActionId,
        label: String,
    },
    /// 最近一步不是词条核心的（标签、角色、作品……），调用方交给它的主人撤销。
    Foreign {
        action: ActionId,
    },
    Nothing,
}

/// 历史已变化（最近一步不是调用方看到的那一步）时的提示，与 tests/reference/web/store.py（#9 已删除） 的 undo 一致。
pub const HISTORY_CHANGED: &str = "操作历史已变化，请刷新后撤销最近一步";

/// 统一撤销入口的词条这一半：同一事务里取最近一步，是自己的就撤，不是就原样交回，不跳过它去撤更早的。
pub fn undo_last(conn: &mut Connection) -> Result<Undo> {
    undo(conn, None)
}

/// 撤销最近一步；给了 `expected` 时，最近一步必须正是它（调用方看到的那一步），否则按历史已变化拒绝。
/// 命令行的撤销入口（`cli.rs`）调用这里；HTTP 宿主在自己的事务里用 `undo_in`。
pub fn undo(conn: &mut Connection, expected: Option<ActionId>) -> Result<Undo> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (result, character_owned) = undo_in(&tx, expected)?;
    if character_owned {
        crate::schema::commit_writing(tx)?;
    } else if matches!(result, Undo::Undone { .. }) {
        tx.commit()?;
    }
    Ok(result)
}

/// 事务留给调用方，用来补上现有的角色效果，或处理外来的应用历史。
pub fn undo_in(conn: &Connection, expected: Option<ActionId>) -> Result<(Undo, bool)> {
    crate::schema::require_current(conn)?;
    let last: Option<(ActionId, String, String)> = conn
        .query_row("SELECT id,label,before_json FROM actions WHERE undone=0 ORDER BY id DESC LIMIT 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    if expected.is_some() && expected != last.as_ref().map(|l| l.0) {
        return Err(Error::Conflict(HISTORY_CHANGED.into()));
    }
    let Some((action, label, before)) = last else {
        return Ok((Undo::Nothing, false));
    };
    // 角色切到核心之后，角色的步骤（新步骤的信封，以及旧的快照、合并、关系、锚点、资料）交给角色撤销；
    // 角色形状与别的形状混在一步里、或形状不认识，拒绝（character_commands::owns）。
    let parsed = serde_json::from_str::<Value>(&before).unwrap_or(Value::Null);
    if crate::legacy_characters::characters_in_core(conn)? && crate::character_commands::owns(&parsed)?.is_some() {
        {
            crate::schema::begin_writing(conn)?;
            crate::character_commands::undo(conn, &json!({"action": action}))?;
            return Ok((Undo::Undone { action, label }, true));
        }
    }
    let handed = handover(conn)?;
    let steps: Vec<Value> = match serde_json::from_str::<Value>(&before) {
        // 切换后，旧的画师判断、用户标签步骤（收编时已挂在同一个 action 上）也由核心撤：和撤核心自己的裁决一样。
        Ok(Value::Array(steps)) if owns(&steps, handed) => steps
            .into_iter()
            .map(|mut s| if s.get("terms").is_some() { s["terms"].take() } else { Value::from("verdicts") })
            .collect(),
        _ => return Ok((Undo::Foreign { action }, false)),
    };
    let mut touched: BTreeSet<AssetId> = BTreeSet::new();
    for step in steps {
        if step == "verdicts" {
            let mut stmt = conn
                .prepare("SELECT DISTINCT asset_id FROM term_verdicts WHERE action_id=?1 AND asset_id IS NOT NULL")?;
            for id in stmt.query_map([action], |r| r.get(0))? {
                touched.insert(id?);
            }
        } else if let Some(kind) = step.get("kind") {
            let (Some(key), Some(had)) = (kind["term"].as_str(), kind["had"].as_str().and_then(Kind::parse)) else {
                return Err(Error::Conflict(format!("历史第 {action} 步的记录不完整")));
            };
            let (term, _) = term_by_key(conn, key)?.ok_or_else(|| Error::Conflict(format!("词条 {key} 已不存在")))?;
            conn.execute("UPDATE terms SET kind=?1 WHERE id=?2", params![had.as_str(), term])?;
            touched.extend(assets_of_term(conn, term)?);
        } else {
            return Err(Error::Conflict(format!("历史第 {action} 步有不认识的词条记录")));
        }
    }
    conn.execute("UPDATE actions SET undone=1 WHERE id=?1", [action])?;
    project(conn, &touched.into_iter().collect::<Vec<_>>())?;
    Ok((Undo::Undone { action, label }, false))
}

/// 哪些旧领域已经切到词条核心（meta 里的切换标记）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Handover {
    /// `terms_artists`=`core`：画师。
    pub artists: bool,
    /// `terms_labels`=`core`：用户标签、分级、WD 题材。
    pub labels: bool,
}

/// 本图库资产的命名空间：键是 library.sqlite 的 `assets.uuid`，不区分来源是 Photos 还是文件夹。
pub const LIBRARY: &str = "library";

pub const ARTISTS_IN_CORE: &str = "terms_artists";
pub const LABELS_IN_CORE: &str = "terms_labels";

fn flag(conn: &Connection, key: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get::<_, String>(0))
        .optional()?
        .is_some_and(|v| v == "core"))
}

pub fn handover(conn: &Connection) -> Result<Handover> {
    Ok(Handover { artists: flag(conn, ARTISTS_IN_CORE)?, labels: flag(conn, LABELS_IN_CORE)? })
}

pub fn artists_in_core(conn: &Connection) -> Result<bool> {
    flag(conn, ARTISTS_IN_CORE)
}

pub fn labels_in_core(conn: &Connection) -> Result<bool> {
    flag(conn, LABELS_IN_CORE)
}

/// 这一步是不是归词条核心撤：`before_json` 是非空数组，每项都是下面之一：
/// - 只有一个键 `terms`（核心自己写的）；
/// - 画师已切到核心时，只有一个键 `artist_verdict` 的旧画师判断；
/// - 标签已切到核心时，恰好是 `uuid`、`tag`、`had` 三个键的旧用户标签步骤。
///
/// tests/reference/web/terms_core.py（#9 已删除） 的 `owns` 用同一条规则，两边改一处就要改另一处。
pub fn owns(steps: &[Value], handed: Handover) -> bool {
    !steps.is_empty()
        && steps.iter().all(|s| {
            s.as_object().is_some_and(|o| {
                let keys: BTreeSet<&str> = o.keys().map(String::as_str).collect();
                keys == BTreeSet::from(["terms"])
                    || (handed.artists && keys == BTreeSet::from(["artist_verdict"]))
                    || (handed.labels && keys == BTreeSet::from(["uuid", "tag", "had"]))
            })
        })
}

// ---- 投影 ----

#[derive(Default)]
struct Acc {
    verdict: Option<String>,
    cleared: bool,
    strong: bool,
    best: Option<f64>,
    other: bool,
    sources: BTreeSet<String>,
}

/// 由证据批次重建有效集：每张图、每个可替换来源取覆盖它的最新一个批次（`all` 覆盖每一张图）。
/// 增量维护的结果必须与它相等。返回有效集变了的图，调用方重投它们。
pub(crate) fn ids(conn: &Connection, sql: &str) -> Result<Vec<i64>> {
    Ok(conn.prepare(sql)?.query_map([], |x| x.get(0))?.collect::<rusqlite::Result<_>>()?)
}

pub fn rebuild_active_evidence(conn: &Connection) -> Result<Vec<AssetId>> {
    const WANTED: &str =
        "SELECT a.id AS asset_id,s.source,s.method,max(s.id) AS set_id FROM term_evidence_sets s JOIN term_assets a
         ON s.scope='all' OR EXISTS(SELECT 1 FROM term_evidence_set_assets x WHERE x.set_id=s.id AND x.asset_id=a.id)
         GROUP BY a.id,s.source,s.method";
    const HAVE: &str = "SELECT asset_id,source,method,set_id FROM term_active_evidence";
    // 两个方向的 EXCEPT 和插入用的生效集合相同。
    // 物化一次，不再重复图片/批次覆盖的连接。
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS wanted_evidence(
      asset_id INTEGER,source TEXT,method TEXT,set_id INTEGER,
      PRIMARY KEY(asset_id,source,method)) WITHOUT ROWID; DELETE FROM wanted_evidence;",
    )?;
    conn.execute(&format!("INSERT INTO wanted_evidence {WANTED}"), [])?;
    let changed = ids(
        conn,
        &format!(
            "SELECT DISTINCT asset_id FROM (SELECT * FROM (SELECT * FROM wanted_evidence EXCEPT {HAVE})
             UNION ALL SELECT * FROM ({HAVE} EXCEPT SELECT * FROM wanted_evidence)) ORDER BY asset_id"
        ),
    )?;
    conn.execute("DELETE FROM term_active_evidence", [])?;
    conn.execute("INSERT INTO term_active_evidence SELECT * FROM wanted_evidence", [])?;
    Ok(changed)
}

/// 一次全量快照（整个来源按批次生效，schema::REPLACEABLE_SOURCES）：`covered` 这些图上，这个来源当前有效的证据
/// 就是 `rows`；覆盖到而没有行 = 查无结果，旧证据随之失效。证据行本身只增不删。
pub fn publish_snapshot(
    conn: &Connection,
    source: &str,
    covered: &[AssetId],
    rows: &BTreeSet<i64>,
    package: Option<i64>,
) -> Result<i64> {
    publish_asset_batch(conn, source, (source, "*"), covered, rows, package)
}

pub(crate) fn publish_asset_batch(
    conn: &Connection,
    producer: &str,
    (source, method): (&str, &str),
    covered: &[AssetId],
    rows: &BTreeSet<i64>,
    package: Option<i64>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO term_evidence_sets(producer,source,method,scope,package_id) VALUES (?1,?2,?3,'assets',?4)",
        params![producer, source, method, package],
    )?;
    let set = conn.last_insert_rowid();
    let mut put =
        conn.prepare_cached("INSERT OR IGNORE INTO term_evidence_set_assets(set_id,asset_id) VALUES (?1,?2)")?;
    let mut active = conn.prepare_cached(
        "INSERT OR REPLACE INTO term_active_evidence(asset_id,source,method,set_id) VALUES (?1,?2,?3,?4)",
    )?;
    for &a in covered {
        put.execute([set, a])?;
        active.execute(params![a, source, method, set])?;
    }
    let covered: BTreeSet<AssetId> = covered.iter().copied().collect();
    let mut add =
        conn.prepare_cached("INSERT OR IGNORE INTO term_evidence_set_rows(set_id,evidence_id) VALUES (?1,?2)")?;
    for &id in rows {
        let asset: AssetId = conn.query_row("SELECT asset_id FROM term_evidence WHERE id=?1", [id], |r| r.get(0))?;
        if covered.contains(&asset) {
            add.execute([set, id])?;
        }
    }
    Ok(set)
}

/// 角色切到核心之后，角色树里的词条（人物、皮肤、作品、当作标签的）的投影由 `character_projection` 从兼容表
/// 维护；这里的通用重算只管别的词条，不删也不写那一片。条件与 `character_projection::mirror` 相同。
fn not_characters(conn: &Connection, column: &str) -> Result<String> {
    Ok(if crate::legacy_characters::characters_in_core(conn)? {
        format!(" AND NOT EXISTS(SELECT 1 FROM term_tree r WHERE r.term_id={column})")
    } else {
        String::new()
    })
}

/// 按当前有效的证据（`term_current_evidence`）、有效裁决和类型策略重算这些图的 term_projection。
pub fn project(conn: &Connection, assets: &[AssetId]) -> Result<()> {
    let (e_scope, v_scope, p_scope) = (
        not_characters(conn, "e.term_id")?,
        not_characters(conn, "v.term_id")?,
        not_characters(conn, "term_projection.term_id")?,
    );
    let mut evidence = conn.prepare_cached(&format!(
        "SELECT e.term_id,t.kind,e.source,e.method,e.score,e.metric FROM term_current_evidence e
         JOIN terms t ON t.id=e.term_id WHERE e.asset_id=?1 AND NOT EXISTS(SELECT 1 FROM meta WHERE key='folder_stale_wd:'||e.id){e_scope}"
    ))?;
    let mut verdicts = conn.prepare_cached(&format!(
        "SELECT v.term_id,t.kind,v.verdict FROM term_verdicts v LEFT JOIN actions a ON a.id=v.action_id
         JOIN terms t ON t.id=v.term_id WHERE v.asset_id=?1 AND (v.batch_id IS NOT NULL OR a.undone=0){v_scope}
         ORDER BY v.anchor,v.batch_id IS NOT NULL,v.id"
    ))?;
    for &asset in assets {
        conn.prepare_cached(&format!("DELETE FROM term_projection WHERE asset_id=?1{p_scope}"))?.execute([asset])?;
        let removed: bool = conn
            .prepare_cached("SELECT removed_at IS NOT NULL FROM term_assets WHERE id=?1")?
            .query_row([asset], |r| r.get(0))?;
        if removed {
            continue;
        }
        let mut acc: BTreeMap<(Kind, TermId), Acc> = BTreeMap::new();
        let rows = evidence.query_map([asset], |r| {
            Ok((
                r.get::<_, TermId>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<f64>>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })?;
        for row in rows {
            let (term, kind, source, method, score, metric) = row?;
            let kind = Kind::parse(&kind).expect("类型由触发器约束");
            if kind == Kind::Mark {
                continue;
            }
            let a = acc.entry((kind, term)).or_default();
            a.sources.insert(format!("{source}.{method}"));
            match kind.policy().judge {
                Judge::Strong(methods) => a.strong |= methods.contains(&method.as_str()),
                Judge::Scores { metric: m, .. } | Judge::Top { metric: m } if metric.as_deref() == Some(m) => {
                    let s = score.expect("有量纲就有分数");
                    a.best = Some(a.best.map_or(s, |b: f64| b.max(s)));
                }
                _ => a.other = true,
            }
        }
        for row in verdicts
            .query_map([asset], |r| Ok((r.get::<_, TermId>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
        {
            let (term, kind, verdict) = row?;
            let kind = Kind::parse(&kind).expect("类型由触发器约束");
            let state = acc.entry((kind, term)).or_default();
            state.cleared = verdict == "clear";
            state.verdict = (!state.cleared).then_some(verdict);
        }
        let mut states: Vec<(Kind, TermId, &'static str, Option<f64>, String)> = Vec::new();
        for ((kind, term), a) in &mut acc {
            let state = match a.verdict.as_deref() {
                Some("yes") => "confirmed",
                Some(_) => "rejected",
                None if kind == Kind::Concept && a.cleared => continue,
                None => match kind.policy().judge {
                    Judge::Human => continue,
                    Judge::Pending => "maybe",
                    Judge::System => "machine",
                    Judge::Strong(_) if a.strong => "machine",
                    Judge::Strong(_) => "maybe",
                    Judge::Top { .. } if a.best.is_some() => "machine",
                    Judge::Top { .. } => "maybe",
                    Judge::Scores { machine, maybe, .. } => match a.best {
                        Some(s) if s >= machine => "machine",
                        Some(s) if s >= maybe => "maybe",
                        _ if a.other => "maybe",
                        _ => continue,
                    },
                },
            };
            states.push((*kind, *term, state, a.best, a.sources.iter().cloned().collect::<Vec<_>>().join(",")));
        }
        // 至多一个 / 恰好一个：有人工确认时机器结果全部降为「也可能是」，否则只留分数最高的一个。
        let mut by_kind: HashMap<Kind, Vec<usize>> = HashMap::new();
        for (i, s) in states.iter().enumerate() {
            if matches!(s.0.policy().exclusive, Exclusive::ExactlyOne | Exclusive::AtMostOne) {
                by_kind.entry(s.0).or_default().push(i);
            }
        }
        for idx in by_kind.values() {
            let confirmed = idx.iter().any(|&i| states[i].2 == "confirmed");
            let mut machines: Vec<usize> = idx.iter().copied().filter(|&i| states[i].2 == "machine").collect();
            machines.sort_by(|&x, &y| {
                let (sx, sy) = (states[x].3.unwrap_or(f64::NEG_INFINITY), states[y].3.unwrap_or(f64::NEG_INFINITY));
                sy.total_cmp(&sx).then(states[x].1.cmp(&states[y].1))
            });
            for (rank, i) in machines.into_iter().enumerate() {
                if confirmed || rank > 0 {
                    states[i].2 = "maybe";
                }
            }
        }
        for (_, term, state, score, sources) in states {
            conn.prepare_cached(
                "INSERT INTO term_projection(asset_id,term_id,state,score,sources) VALUES (?1,?2,?3,?4,?5)",
            )?
            .execute(params![asset, term, state, score, sources])?;
        }
    }
    Ok(())
}

/// 整表重建投影（投影随时可以从证据和裁决算回来）。
pub fn project_all(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let scope = not_characters(&tx, "term_projection.term_id")?;
    tx.execute(&format!("DELETE FROM term_projection WHERE 1{scope}"), [])?;
    let ids: Vec<AssetId> = tx
        .prepare("SELECT id FROM term_assets WHERE removed_at IS NULL")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    project(&tx, &ids)?;
    tx.commit()?;
    Ok(())
}

// ---- 证据包导入 ----

#[derive(Debug, PartialEq, Eq)]
pub struct Imported {
    pub package: i64,
    /// 本次新增的证据。
    pub added: usize,
    /// 与已有证据完全相同、未重复入库的。
    pub duplicate: usize,
    /// 低于类型策略的入库分数、没有入库的。
    pub skipped: usize,
    pub created_terms: usize,
    /// 同一个包以前导入过：什么都没写。
    pub already: bool,
}

/// 导入一个已校验的证据包：一个事务，任何一行出错整包回滚。
/// 同一个包重复导入是空操作；与已有证据完全相同的行去重；同一目标、来源、方法、版本却内容不同时拒绝整包。
pub fn import_package(conn: &mut Connection, pkg: &Package, library_id: &str) -> Result<Imported> {
    import_package_with(conn, pkg, library_id, false)
}

/// 同上；`new_assets`：本库（`library` 命名空间）还没进 `assets` 的图也先登记，供同步导入新图时先交 WD 证据。
pub fn import_package_with(
    conn: &mut Connection,
    pkg: &Package,
    library_id: &str,
    new_assets: bool,
) -> Result<Imported> {
    let m = &pkg.manifest;
    if m.library_id != library_id {
        return invalid(format!("证据包属于图库 {owner}，不是 {library_id}", owner = m.library_id));
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(id) = tx
        .query_row("SELECT id FROM term_packages WHERE fingerprint=?1", [&pkg.fingerprint], |r| r.get(0))
        .optional()?
    {
        let stale = tx.prepare("SELECT e.id,e.asset_id FROM term_evidence e JOIN meta m ON m.key='folder_stale_wd:'||e.id WHERE e.package_id=?")?.query_map([id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        if !stale.is_empty() {
            for (e, _) in &stale {
                tx.execute("DELETE FROM meta WHERE key=?", [format!("folder_stale_wd:{e}")])?;
            }
            project(&tx, &stale.iter().map(|(_, a)| *a).collect::<Vec<_>>())?;
            tx.commit()?;
        }
        return Ok(Imported { package: id, added: 0, duplicate: 0, skipped: 0, created_terms: 0, already: true });
    }
    let mut assets: HashMap<&AssetRef, AssetId> = HashMap::new();
    for outcome in &m.assets {
        let Some(id) = live_asset(&tx, &outcome.asset, new_assets)? else {
            return invalid(format!(
                "图 {namespace}:{id} 不在这个图库里或已移除",
                namespace = outcome.asset.namespace,
                id = outcome.asset.id
            ));
        };
        assets.insert(&outcome.asset, id);
    }
    tx.execute(
        "INSERT INTO term_packages(fingerprint,producer,producer_version,library_id,manifest,evidence_count) VALUES (?1,?2,?3,?4,?5,0)",
        params![pkg.fingerprint, m.producer, m.producer_version, m.library_id, canonical(&m.raw)],
    )?;
    let package = tx.last_insert_rowid();
    let origin = format!("package:{}", m.producer);
    let mut terms: HashMap<&TermRef, (TermId, String)> = HashMap::new();
    let mut out = Imported { package, added: 0, duplicate: 0, skipped: 0, created_terms: 0, already: false };
    let mut touched = BTreeSet::new();
    // 整个来源按快照替换的包（溯源）：记下这个来源每条图证据的 id，最后按「已完成」的图发布一个批次。
    let snapshot = crate::schema::REPLACEABLE_SOURCES.contains(&m.producer.as_str());
    let mut snapshot_rows: BTreeSet<i64> = BTreeSet::new();
    for info in &pkg.terms {
        let at = |e: Error| match e {
            Error::Invalid(msg) => Error::Invalid(format!("terms.jsonl 第 {n} 行：{msg}", n = info.line)),
            Error::Conflict(msg) => Error::Conflict(format!("terms.jsonl 第 {n} 行：{msg}", n = info.line)),
            e => e,
        };
        let (term, _) =
            resolve(&tx, &info.term, &m.producer, &origin, &mut terms, &mut out.created_terms).map_err(at)?;
        for (system, key, url) in &info.links {
            declare_link(&tx, term, system, key, url, &m.producer).map_err(at)?;
        }
        for (lang, value, source) in &info.names {
            let source = if source.is_empty() { m.producer.as_str() } else { source.as_str() };
            tx.execute(
                "DELETE FROM term_names WHERE term_id=?1 AND role='display' AND lang=?2 AND source=?3 AND value<>?4",
                params![term, lang, source, value],
            )?;
            add_name(&tx, term, lang, value, "display", source)?;
        }
    }
    for row in &pkg.rows {
        let at = |e: Error| match e {
            Error::Invalid(msg) => Error::Invalid(format!("evidence.jsonl 第 {n} 行：{msg}", n = row.line)),
            Error::Conflict(msg) => Error::Conflict(format!("evidence.jsonl 第 {n} 行：{msg}", n = row.line)),
            e => e,
        };
        let score = row.score.as_ref().map(|s| (s.value, s.metric.as_str()));
        let claim = match &row.target {
            Target::Image { asset, term } => {
                if let (Some((metric, min)), Some((value, m))) = (term.kind().policy().keep_min, score) {
                    if m == metric && value < min {
                        out.skipped += 1;
                        continue;
                    }
                }
                let (term, key) =
                    resolve(&tx, term, &m.producer, &origin, &mut terms, &mut out.created_terms).map_err(at)?;
                Claim::Image {
                    asset: assets[asset],
                    term,
                    target: json!({"asset": [asset.namespace, asset.id], "term": key}),
                }
            }
            Target::Relation { kind, a, b } => {
                let (a, ka) = resolve(&tx, a, &m.producer, &origin, &mut terms, &mut out.created_terms).map_err(at)?;
                let (b, kb) = resolve(&tx, b, &m.producer, &origin, &mut terms, &mut out.created_terms).map_err(at)?;
                let rel = relation(&tx, a, b, *kind).map_err(at)?;
                let (ka, kb) = if kind.symmetric() && kb < ka { (kb, ka) } else { (ka, kb) };
                Claim::Relation { relation: rel, target: json!({"relation": [kind.as_str(), ka, kb]}) }
            }
        };
        let evidence = NewEvidence {
            claim: &claim,
            source: &row.source,
            method: &row.method,
            version: &row.version,
            score,
            data: &row.data,
            observed_at: Some(&row.observed_at),
            package: Some(package),
        };
        if add_evidence(&tx, &evidence).map_err(at)? {
            out.added += 1;
            if let Claim::Image { asset, .. } = claim {
                touched.insert(asset);
            }
        } else {
            out.duplicate += 1;
            if let Claim::Image { asset, term, .. } = claim {
                let id:i64=tx.query_row("SELECT id FROM term_evidence WHERE asset_id=? AND term_id=? AND source=? AND method=? AND version=?",params![asset,term,row.source,row.method,row.version],|r|r.get(0))?;
                if tx.execute("DELETE FROM meta WHERE key=?", [format!("folder_stale_wd:{id}")])? > 0 {
                    touched.insert(asset);
                }
            }
        }
        if let Claim::Image { asset, term, .. } = claim {
            if snapshot && row.source == m.producer {
                snapshot_rows.insert(tx.query_row(
                    "SELECT id FROM term_evidence WHERE asset_id=?1 AND term_id=?2 AND source=?3 AND method=?4 AND version=?5",
                    params![asset, term, row.source, row.method, row.version],
                    |r| r.get(0),
                )?);
            }
        }
    }
    if snapshot {
        let covered: Vec<AssetId> = m
            .assets
            .iter()
            .filter(|o| o.status == crate::package::Status::Completed)
            .map(|o| assets[&o.asset])
            .collect();
        publish_snapshot(&tx, &m.producer, &covered, &snapshot_rows, Some(package))?;
        touched.extend(covered);
    }
    tx.execute(
        "UPDATE term_packages SET evidence_count=?1,skipped_count=?2 WHERE id=?3",
        params![out.added as i64, out.skipped as i64, package],
    )?;
    project(&tx, &touched.into_iter().collect::<Vec<_>>())?;
    tx.commit()?;
    Ok(out)
}

/// 证据的目标。`target` 是指纹里用的规范化描述：用词条 key 和资产引用，不用内部整数 id，换库也不变。
pub enum Claim {
    Image { asset: AssetId, term: TermId, target: Value },
    Relation { relation: RelationId, target: Value },
}

pub struct NewEvidence<'a> {
    pub claim: &'a Claim,
    pub source: &'a str,
    pub method: &'a str,
    pub version: &'a str,
    pub score: Option<(f64, &'a str)>,
    pub data: &'a serde_json::Map<String, Value>,
    pub observed_at: Option<&'a str>,
    pub package: Option<i64>,
}

/// 内容指纹供重复交付预检共用；预检不可以另造较宽松的判等规则。
pub fn evidence_fingerprint(e: &NewEvidence) -> Vec<u8> {
    let target = match e.claim {
        Claim::Image { target, .. } | Claim::Relation { target, .. } => target,
    };
    let digest = sha2::Sha256::digest(
        canonical(&json!({
            "target": target, "source": e.source, "method": e.method, "version": e.version,
            "score": e.score.map(|(v, m)| json!([v, m])), "data": e.data,
        }))
        .as_bytes(),
    );
    digest[..16].to_vec()
}

/// 追加一条证据；与已有证据完全相同时不写、返回 false。同一目标、来源、方法、版本已有不同内容时报冲突。
pub(crate) fn add_evidence(conn: &Connection, e: &NewEvidence) -> Result<bool> {
    let fingerprint = evidence_fingerprint(e);
    let existing: Option<Vec<u8>> = match *e.claim {
        Claim::Image { asset, term, .. } => conn.prepare_cached("SELECT fingerprint FROM term_evidence WHERE asset_id=?1 AND term_id=?2 AND source=?3 AND method=?4 AND version=?5")?.query_row(
            params![asset, term, e.source, e.method, e.version],
            |r| r.get(0),
        ),
        Claim::Relation { relation, .. } => conn.prepare_cached("SELECT fingerprint FROM term_evidence WHERE relation_id=?1 AND source=?2 AND method=?3 AND version=?4")?.query_row(
            params![relation, e.source, e.method, e.version],
            |r| r.get(0),
        ),
    }
    .optional()?;
    match existing {
        Some(fp) if fp == fingerprint => return Ok(false),
        Some(_) => {
            return Err(Error::Conflict(format!(
                "{source} {method} 第 {version} 版对这个目标已有不同的结果；算法或模型变了请升版本号",
                source = e.source,
                method = e.method,
                version = e.version
            )))
        }
        None => {}
    }
    let (asset, term, relation) = match *e.claim {
        Claim::Image { asset, term, .. } => (Some(asset), Some(term), None),
        Claim::Relation { relation, .. } => (None, None, Some(relation)),
    };
    conn.prepare_cached("INSERT INTO term_evidence(asset_id,term_id,relation_id,source,method,version,score,metric,data,fingerprint,package_id,observed_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)")?.execute(
        params![
            asset,
            term,
            relation,
            e.source,
            e.method,
            e.version,
            e.score.map(|s| s.0),
            e.score.map(|s| s.1),
            canonical(&Value::Object(e.data.clone())),
            fingerprint,
            e.package,
            e.observed_at
        ],
    )?;
    Ok(true)
}

/// 产出者声明的链接：没有就加，url 变了就改；属于另一个同由这个产出者建的词条（`package:<产出者>`、`legacy:<产出者>`）
/// 就移过来（产出者自己改了归属）；属于别人建的词条则冲突，不替人改。
fn declare_link(conn: &Connection, term: TermId, system: &str, key: &str, url: &str, producer: &str) -> Result<()> {
    let owner: Option<(TermId, String, String)> = conn.prepare_cached("SELECT l.term_id,l.url,t.origin FROM term_links l JOIN terms t ON t.id=l.term_id WHERE l.system=?1 AND l.key=?2")?.query_row(
            [system, key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    match owner {
        None => add_link(conn, term, system, key, url, producer),
        Some((id, old, _)) if id == term => {
            if old != url {
                conn.prepare_cached("UPDATE term_links SET url=?1 WHERE system=?2 AND key=?3")?
                    .execute(params![url, system, key])?;
            }
            Ok(())
        }
        Some((_, _, origin)) if origin == format!("package:{producer}") || origin == format!("legacy:{producer}") => {
            conn.prepare_cached("UPDATE term_links SET term_id=?1,url=?2 WHERE system=?3 AND key=?4")?
                .execute(params![term, url, system, key])?;
            Ok(())
        }
        Some(_) => Err(Error::Conflict(format!("链接 {system}:{key} 属于别处建的词条，不能由 {producer} 改归属"))),
    }
}

fn resolve<'p>(
    conn: &Connection,
    r: &'p TermRef,
    producer: &str,
    origin: &str,
    cache: &mut HashMap<&'p TermRef, (TermId, String)>,
    created: &mut usize,
) -> Result<(TermId, String)> {
    if let Some(hit) = cache.get(r) {
        return Ok(hit.clone());
    }
    // 词条已存在时沿用它现在的类型：人把角色改成「只当标签」后，机器证据照样挂在同一个词条上。
    let id = match r {
        TermRef::Key { key, .. } => {
            term_by_key(conn, key)?.ok_or_else(|| Error::Invalid(format!("词条 {key} 不存在")))?.0
        }
        TermRef::Link { kind, system, key, url, name } => match term_by_link(conn, system, key)? {
            Some(id) => id,
            None if kind.policy().package_may_create => {
                // 链接系统就是产出者自己（provenance:<画师 id>）：这是它给这个词条的身份，用作 key，界面链接不变。
                let own = (system == producer && term_by_key(conn, key)?.is_none()).then_some(key.as_str());
                let id = create_term(conn, *kind, own, origin)?;
                add_link(conn, id, system, key, url, producer)?;
                if let Some(name) = name {
                    add_name(conn, id, "", name, "display", producer)?;
                }
                *created += 1;
                id
            }
            None => {
                return invalid(format!("证据包不能新建「{kind}」词条（{system}:{key}）", kind = kind.policy().name))
            }
        },
    };
    let (key, mark): (String, bool) = conn
        .prepare_cached("SELECT key,kind='mark' FROM terms WHERE id=?")?
        .query_row([id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if mark {
        return invalid("证据包不能使用人工标记词条");
    }
    let hit = (id, key);
    cache.insert(r, hit.clone());
    Ok(hit)
}

// ---- 读 ----

/// 一张图上一个词条的投影状态。
pub fn state(conn: &Connection, asset: AssetId, term: TermId) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT state FROM term_projection WHERE asset_id=?1 AND term_id=?2", [asset, term], |r| r.get(0))
        .optional()?)
}

#[cfg(test)]
mod tests;
