//! 每个图库会话的自建标签学习。读快照与计算在发布事务外，写入前复核原输入。
use crate::{
    app_library::Library,
    app_tasks::{Deadline, Task, TaskContext, TaskIdentity},
    example_cache::Cache,
    example_features::Space,
    example_learning::{self as learning, Asset, Label, Verdict, MIN_POSITIVES},
    Result,
};
use illustash_core::example_candidates::{self, Batch};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

const NORMAL_SETTLE: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_secs(1);
const WD_VERSION: &str = "swinv2_448";
const CCIP_VERSION: &str = "ccip-model_feat";

#[derive(Default)]
pub(crate) struct Learner(Option<Task>);

impl Learner {
    pub(crate) fn start(&mut self, library: &Library, generation: u64, token: &str, cache: Arc<Cache>) -> Result<()> {
        if library.frozen() || self.0.as_ref().is_some_and(Task::running) {
            return Ok(());
        }
        let identity = TaskIdentity::new(library, generation, token)?;
        let library = library.clone();
        self.0 = Some(Task::start_with(identity, Deadline::default(), move |context| {
            let result = run(&library, &cache, &context);
            if let Err(error) = &result {
                if context.stop_reason().is_none() {
                    eprintln!("自建标签学习停止：{error}");
                }
            }
            result
        }));
        Ok(())
    }

    pub(crate) fn stop(&mut self) {
        if let Some(mut task) = self.0.take() {
            task.context().cancel();
            task.abandon();
        }
    }
}

impl Drop for Learner {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Default)]
struct Snapshot {
    assets: Vec<Asset>,
    labels: BTreeMap<String, Label>,
}

impl Snapshot {
    fn read(db: &Connection) -> Result<Self> {
        let mut labels: BTreeMap<String, Label> = db
            .prepare("SELECT key FROM terms WHERE kind='concept' ORDER BY key")?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, Label::default())))?
            .collect::<rusqlite::Result<_>>()?;
        if labels.is_empty() {
            return Ok(Self::default());
        }
        let mut stmt = db.prepare(
            "SELECT key,uuid,verdict FROM (
               SELECT t.key,s.uuid,v.verdict,row_number() OVER (
                 PARTITION BY v.asset_id,v.term_id ORDER BY v.anchor DESC,v.batch_id IS NOT NULL DESC,v.id DESC
               ) AS rn
               FROM term_verdicts v LEFT JOIN actions x ON x.id=v.action_id
               JOIN terms t ON t.id=v.term_id AND t.kind='concept'
               JOIN term_assets a ON a.id=v.asset_id AND a.namespace='library' AND a.removed_at IS NULL
               JOIN assets s ON s.uuid=a.key WHERE v.batch_id IS NOT NULL OR x.undone=0
             ) WHERE rn=1 ORDER BY key,uuid",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            let uuid: String = row.get(1)?;
            let value: String = row.get(2)?;
            let verdict = match value.as_str() {
                "yes" => Verdict::Yes,
                "no" => Verdict::No,
                "clear" => Verdict::Clear,
                _ => return Err(crate::Error::invalid("自建标签裁决未知")),
            };
            labels.get_mut(&key).expect("verdict has a listed concept").verdicts.insert(uuid, verdict);
        }
        let assets = db
            .prepare(
                "SELECT a.uuid,d.grp,coalesce(d.hidden,0)=0 AND x.uuid IS NULL
                 FROM assets a LEFT JOIN duplicates d ON a.uuid=d.uuid LEFT JOIN excluded x ON a.uuid=x.uuid
                 ORDER BY a.uuid",
            )?
            .query_map([], |r| Ok(Asset { uuid: r.get(0)?, group: r.get(1)?, visible: r.get(2)? }))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Self { assets, labels })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Epoch {
    source: crate::example_disk::Stamp,
    base: crate::example_cache::BaseStamp,
}

fn epoch(library: &Library) -> Result<Epoch> {
    Ok(Epoch {
        source: crate::example_disk::source_stamp(&library.source())?,
        base: crate::example_cache::base_stamp(&library.features(), Space::Ccip)?,
    })
}

fn data_version(c: &Connection) -> Result<i64> {
    Ok(c.query_row("PRAGMA data_version", [], |r| r.get(0))?)
}

fn run(library: &Library, cache: &Cache, context: &TaskContext) -> Result<()> {
    let mut db = crate::sqlite::open_with_flags(library.db(), OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    db.busy_timeout(Duration::from_millis(100))?;
    db.pragma_update(None, "foreign_keys", true)?;
    let mut seen: Option<(i64, Epoch)> = None;
    let mut current = Snapshot::default();
    let mut pending = BTreeMap::new();
    let mut features = None;
    let mut model = None;
    while context.stop_reason().is_none() {
        let app = data_version(&db)?;
        let input = epoch(library)?;
        if seen != Some((app, input)) {
            let tx = db.transaction()?;
            let next = Snapshot::read(&tx)?;
            tx.commit()?;
            if data_version(&db)? != app || epoch(library)? != input {
                std::thread::park_timeout(POLL);
                continue;
            }
            let globally_changed = seen.is_none_or(|(_, old)| old != input) || current.assets != next.assets;
            pending.retain(|key, _| next.labels.contains_key(key));
            for (key, label) in &next.labels {
                if globally_changed || current.labels.get(key) != Some(label) {
                    pending.insert(key.clone(), Instant::now() + NORMAL_SETTLE);
                }
            }
            current = next;
            seen = Some((app, input));
        }
        let now = Instant::now();
        let due: Vec<_> =
            pending.iter().filter(|(_, deadline)| **deadline <= now).map(|(key, _)| key.clone()).collect();
        for key in due {
            context.check()?;
            let label = &current.labels[&key];
            // 少量标注从未产生候选时不打开特征缓存，也不登记空历史。
            let can_train =
                label.verdicts.values().filter(|&&v| v == Verdict::Yes).take(MIN_POSITIVES).count() == MIN_POSITIVES;
            if !can_train && !has_old_batch(&db, &key)? {
                pending.remove(&key);
                continue;
            }
            let trained = if can_train {
                let matrix = cache.get(&library.source(), &library.features(), Space::TagsCcip)?;
                let fingerprint = match features {
                    Some((old, digest)) if old == input => digest,
                    _ => {
                        let digest = matrix.fingerprint();
                        features = Some((input, digest));
                        digest
                    }
                };
                Some((matrix, fingerprint))
            } else {
                None
            };
            if model.as_ref().is_none_or(|(known, _)| *known != input) {
                model = Some((input, model_version(library)?));
            }
            let feature_version = &model.as_ref().expect("model version loaded").1;
            let plan = match trained.as_ref() {
                Some((matrix, fingerprint)) => learning::make_plan(
                    matrix,
                    &key,
                    label,
                    &current.assets,
                    &format!("{feature_version}:{fingerprint:x}"),
                ),
                None => learning::Plan {
                    inputs: input_summary(&[], label, feature_version)[..16].into(),
                    positives: label.verdicts.values().filter(|&&v| v == Verdict::Yes).count(),
                    negatives: label.verdicts.values().filter(|&&v| v == Verdict::No).count(),
                    cutoff: None,
                    candidates: vec![],
                },
            };
            let summary = input_summary(&current.assets, label, &plan.inputs);
            let version = format!("linear/1;{feature_version};inputs={}", plan.inputs);
            let data = json!({"positives":plan.positives,"negatives":plan.negatives,"cutoff":plan.cutoff});
            let batch = Batch {
                library: library.id(),
                term: &key,
                version: &version,
                input: &summary,
                data: data.as_object().expect("training summary is an object"),
                rows: &plan.candidates,
            };
            let committed = context.receive_part(context.identity(), || {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                if data_version(&tx)? != app || epoch(library)? != input {
                    return Ok(false);
                }
                example_candidates::publish(&tx, &batch)?;
                if epoch(library)? != input {
                    return Ok(false);
                }
                context.check()?;
                tx.commit()?;
                Ok(true)
            });
            let committed = match committed {
                Ok(value) => value,
                Err(crate::Error::Sql(rusqlite::Error::SqliteFailure(error, _)))
                    if matches!(
                        error.code,
                        rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                    ) =>
                {
                    false
                }
                Err(err) => return Err(err),
            };
            if !committed {
                break;
            }
            pending.remove(&key);
        }
        std::thread::park_timeout(POLL);
    }
    Ok(())
}

fn has_old_batch(c: &Connection, key: &str) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM term_evidence_sets WHERE source='example' AND method=?)",
        [key],
        |r| r.get(0),
    )?)
}

fn input_summary(assets: &[Asset], label: &Label, input: &str) -> String {
    let mut digest = Sha256::new();
    learning::hash_field(&mut digest, input.as_bytes());
    digest.update((assets.len() as u64).to_le_bytes());
    for asset in assets {
        learning::hash_field(&mut digest, asset.uuid.as_bytes());
        learning::hash_field(&mut digest, asset.group.as_deref().unwrap_or_default().as_bytes());
        digest.update([u8::from(asset.visible)]);
    }
    digest.update((label.verdicts.len() as u64).to_le_bytes());
    for (uuid, verdict) in &label.verdicts {
        learning::hash_field(&mut digest, uuid.as_bytes());
        digest.update([match verdict {
            Verdict::Yes => 1,
            Verdict::No => 2,
            Verdict::Clear => 3,
        }]);
    }
    format!("{:x}", digest.finalize())
}

fn model_version(library: &Library) -> Result<String> {
    let source = crate::app_library::read_connection(&library.source())?;
    let exists: bool = source.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='recognition_imports' AND type='table')",
        [],
        |r| r.get(0),
    )?;
    let mut wd = WD_VERSION.to_owned();
    let mut ccip = CCIP_VERSION.to_owned();
    if exists {
        let formats: Option<(Option<String>, Option<String>)> = source
            .query_row(
                "SELECT coalesce(json_extract(manifest,'$.recognition.models.wd.version'),
                                 json_extract(manifest,'$.models.wd.version')),
                        coalesce(json_extract(manifest,'$.recognition.models.ccip.version'),
                                 json_extract(manifest,'$.models.ccip.version'))
                 FROM recognition_imports
                 WHERE status<>'invalidated' ORDER BY rowid DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((new_wd, new_ccip)) = formats {
            if let Some(version) = new_wd {
                wd = version;
            }
            if let Some(version) = new_ccip {
                ccip = version;
            }
        }
    }
    Ok(Space::TagsCcip.version(&wd, &ccip))
}

#[cfg(test)]
mod tests;
