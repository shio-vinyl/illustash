//! 自建标签例图候选的发布。每次覆盖该标签一个全库批次，机器结果不进入人工撤销栈。
use crate::{
    store::{self, Claim, NewEvidence},
    strict_json::{canonical, sha256_hex},
    Error, Result,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

const SOURCE: &str = "example";
const METRIC: &str = "example_logit";

/// 一次从固定输入计算出的自建标签候选。
pub struct Batch<'a> {
    /// 图库包标识。
    pub library: &'a str,
    /// 自建词条的永久 key，也用作可替换证据的方法。
    pub term: &'a str,
    /// 学习、特征及训练输入版本。
    pub version: &'a str,
    /// 本批次的候选范围和人工输入摘要，包括会让旧候选失效的项。
    pub input: &'a str,
    /// 门槛、有效正例和负例数等传给证据的本机统计。
    pub data: &'a Map<String, Value>,
    /// 候选图片 UUID 和未校准 logit。
    pub rows: &'a [(String, f32)],
}

/// 记下当前批次并重投发生变化的图。调用方持有写事务并已确认读取的输入仍有效。
/// 同一个当前批次已经生效时返回 false。
pub fn publish(conn: &Connection, batch: &Batch<'_>) -> Result<bool> {
    crate::schema::require_current(conn)?;
    let term: i64 = conn
        .query_row("SELECT id FROM terms WHERE kind='concept' AND key=?", [batch.term], |r| r.get(0))
        .optional()?
        .ok_or_else(|| Error::invalid("自建标签已删除"))?;
    let receipt_key = sha256_hex(canonical(&json!([batch.library, batch.term, batch.version, batch.input])).as_bytes());
    let result = sha256_hex(canonical(&json!([batch.rows, batch.data])).as_bytes());
    let receipt = canonical(&json!({"term":batch.term,"version":batch.version,"input":batch.input,"result":result}));
    let known: Option<String> = conn
        .query_row("SELECT manifest FROM term_packages WHERE fingerprint=?", [&receipt_key], |r| r.get(0))
        .optional()?;
    if known.is_some_and(|manifest| manifest != receipt) {
        return Err(Error::Conflict("相同输入的自建标签批次已有不同结果".into()));
    }
    let old: Option<(i64, Option<String>)> = conn
        .query_row(
            "SELECT s.id,p.fingerprint FROM term_evidence_sets s LEFT JOIN term_packages p ON p.id=s.package_id
             WHERE s.source='example' AND s.method=? ORDER BY s.id DESC LIMIT 1",
            [batch.term],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if old.as_ref().and_then(|(_, fp)| fp.as_deref()) == Some(&receipt_key) || (old.is_none() && batch.rows.is_empty())
    {
        return Ok(false);
    }
    let mut touched = BTreeSet::new();
    if let Some((old_set, _)) = old {
        touched.extend(
            conn.prepare(
                "SELECT e.asset_id FROM term_evidence_set_rows r JOIN term_evidence e ON e.id=r.evidence_id
                 WHERE r.set_id=?",
            )?
            .query_map([old_set], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    conn.execute(
        "INSERT OR IGNORE INTO term_packages(fingerprint,producer,producer_version,library_id,manifest,evidence_count)
         VALUES (?,'example',?,?,?,?)",
        params![receipt_key, batch.version, batch.library, receipt, batch.rows.len()],
    )?;
    let package: i64 =
        conn.query_row("SELECT id FROM term_packages WHERE fingerprint=?", [receipt_key], |r| r.get(0))?;
    let mut evidence_ids = BTreeSet::new();
    for (uuid, score) in batch.rows {
        let asset = store::library_asset(conn, uuid)?.ok_or_else(|| Error::invalid("候选图已离开图库"))?;
        let target = json!({"asset":{"namespace":"library","id":uuid},"term":{"key":batch.term}});
        let claim = Claim::Image { asset, term, target };
        store::add_evidence(
            conn,
            &NewEvidence {
                claim: &claim,
                source: SOURCE,
                method: batch.term,
                version: batch.version,
                score: Some((f64::from(*score), METRIC)),
                data: batch.data,
                observed_at: None,
                package: Some(package),
            },
        )?;
        evidence_ids.insert(conn.query_row(
            "SELECT id FROM term_evidence WHERE asset_id=? AND term_id=? AND source='example' AND method=? AND version=?",
            params![asset, term, batch.term, batch.version],
            |r| r.get::<_, i64>(0),
        )?);
        touched.insert(asset);
    }

    conn.execute(
        "INSERT INTO term_evidence_sets(producer,source,method,scope,package_id) VALUES ('example','example',?,'all',?)",
        params![batch.term, package],
    )?;
    let set = conn.last_insert_rowid();
    {
        let mut insert = conn.prepare("INSERT INTO term_evidence_set_rows(set_id,evidence_id) VALUES (?,?)")?;
        for evidence in evidence_ids {
            insert.execute([set, evidence])?;
        }
    }
    conn.execute(
        "INSERT OR REPLACE INTO term_active_evidence(asset_id,source,method,set_id)
         SELECT id,'example',?,? FROM term_assets",
        params![batch.term, set],
    )?;
    store::project(conn, &touched.into_iter().collect::<Vec<_>>())?;
    Ok(true)
}

#[cfg(test)]
mod tests;
