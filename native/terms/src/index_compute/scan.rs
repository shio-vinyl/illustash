use super::*;
use crate::recognition_compute::anchors::{self, Metadata};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

/// 老清单可独立于 embeddings 表存在。以扫描输出使用的 UUID 查来源资产，分批避免构造整份 JSON。
fn missing_from_manifest(db: &Connection, assets: &[String]) -> Result<HashSet<String>> {
    let mut missing = HashSet::new();
    if assets.is_empty() {
        return Ok(missing);
    }
    let mut stmt = db.prepare(
        "SELECT j.value FROM json_each(?) j LEFT JOIN assets a ON a.uuid=j.value COLLATE BINARY WHERE a.uuid IS NULL",
    )?;
    for batch in assets.chunks(4096) {
        let ids: Vec<_> = batch.iter().map(|id| id.trim()).collect();
        let encoded = serde_json::to_string(&ids).map_err(|e| Error::invalid(e.to_string()))?;
        for uuid in stmt.query_map([encoded], |r| r.get(0))? {
            missing.insert(uuid?);
        }
    }
    Ok(missing)
}

pub(super) fn scan(
    review: &Path,
    characters: &[Value],
    vectors: &[Vec<f32>],
    ids: &[String],
    meta: &HashMap<String, Metadata>,
    patched: &HashSet<String>,
) -> Result<Vec<(String, String, f64)>> {
    let fp = review.join("full-features.npy");
    let manifest = review.join("full-manifest.csv");
    if !fp.try_exists()? {
        save(&fp, &[])?;
        fs::write(&manifest, "uuid\n")?;
    }
    let mut features = matrix(&fp)?;
    let mut rdr = csv::Reader::from_path(&manifest).map_err(|e| Error::invalid(e.to_string()))?;
    let uuid = rdr
        .headers()
        .map_err(|e| Error::invalid(e.to_string()))?
        .iter()
        .position(|s| s == "uuid")
        .ok_or_else(|| Error::invalid("manifest uuid缺失"))?;
    let mut assets = rdr
        .records()
        .map(|r| {
            let r = r.map_err(|e| Error::invalid(e.to_string()))?;
            Ok(r.get(uuid).ok_or_else(|| Error::invalid("manifest uuid缺失"))?.to_owned())
        })
        .collect::<Result<Vec<_>>>()?;
    if assets.len() != features.len() {
        return Err(Error::invalid("索引特征与manifest长度不匹配"));
    }
    let have: HashSet<_> = assets.iter().map(|s| s.trim().to_uppercase()).collect();
    let db = crate::sqlite::open_with_flags(review.join("pi.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let exists: bool =
        db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='embeddings')", [], |r| {
            r.get(0)
        })?;
    // 抢救后仍认识的旧图可能缺来源资产行。保持原矩阵形状，单独决定哪些认领可以发布。
    let mut missing_assets = missing_from_manifest(&db, &assets)?;
    if exists {
        let mut stmt = db.prepare(
            "SELECT e.uuid,e.vec,a.uuid IS NULL FROM embeddings e LEFT JOIN assets a ON a.uuid=e.uuid WHERE e.model='ccip'",
        )?;
        let extra =
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, bool>(2)?)))?;
        for row in extra {
            let (id, blob, missing) = row?;
            if missing {
                missing_assets.insert(id.clone());
            }
            if have.contains(&id.to_uppercase()) {
                continue;
            }
            if blob.len() != DIM * 4 {
                return Err(Error::invalid("索引CCIP向量维数不正确"));
            }
            let values: Vec<_> =
                blob.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().expect("four bytes"))).collect();
            if values.iter().any(|v| !v.is_finite()) {
                return Err(Error::invalid("索引CCIP向量必须有限"));
            }
            features.push(values);
            assets.push(id);
        }
    }
    let mut csv = csv::WriterBuilder::new()
        .terminator(csv::Terminator::CRLF)
        .from_path(review.join("char-scan.csv"))
        .map_err(|e| Error::invalid(e.to_string()))?;
    csv.write_record(["uuid", "char_id", "entity", "dist", "thresh", "tier", "rank", "within"])
        .map_err(|e| Error::invalid(e.to_string()))?;
    let known: HashSet<_> = characters.iter().filter_map(|c| c["char_id"].as_str()).collect();
    let mut old = db.prepare(
        "SELECT l.uuid,l.value,l.conf FROM labels l LEFT JOIN assets a ON a.uuid=l.uuid
         WHERE l.dim='char_ext' AND a.uuid IS NULL ORDER BY l.uuid,l.value",
    )?;
    let mut preserved = Vec::new();
    let mut rows = old.query([])?;
    while let Some(row) = rows.next()? {
        let uuid: String = row.get(0)?;
        let entity: String = row.get(1)?;
        if missing_assets.contains(&uuid) && known.contains(entity.as_str()) {
            preserved.push((uuid, entity, row.get(2)?));
        }
    }
    if vectors.is_empty() || features.is_empty() {
        csv.flush()?;
        return Ok(preserved);
    }
    let index = anchors::anchor_index_for_scan(vectors, ids, meta)?;
    let limit = anchors::claim_thresholds(ids, &index, patched)?;
    let mut labels = vec![];
    let mut positions = HashMap::new();
    // 保持既有的每块 4000 列的扫描形状：BLAS 的矩阵形状会改变边界上的判定。
    for start in (0..features.len()).step_by(4000) {
        let d = anchors::cross(vectors, &features[start..(start + 4000).min(features.len())])?;
        for j in 0..features[start..(start + 4000).min(features.len())].len() {
            let col: Vec<_> = d.iter().map(|r| r[j]).collect();
            let asset = assets[start + j].trim();
            let mut seen = HashSet::new();
            let mut rank = 0;
            for a in numeric::nearest_nine(&col) {
                let entity = &index.entities[a];
                if !seen.insert(entity) {
                    continue;
                }
                let distance = col[a];
                let ok = limit[a] > 0. && distance <= limit[a];
                let tier = if !ok {
                    "-"
                } else if distance <= index.p50[a] {
                    "p50"
                } else if distance <= index.p75[a] {
                    "p75"
                } else {
                    "p90"
                };
                let rounded = format!("{:.4}", f64::from(distance));
                csv.write_record([
                    asset,
                    &ids[a],
                    entity,
                    &rounded,
                    &format!("{:.4}", f64::from(limit[a])),
                    tier,
                    &rank.to_string(),
                    if ok { "1" } else { "0" },
                ])
                .map_err(|e| Error::invalid(e.to_string()))?;
                rank += 1;
                if ok && !missing_assets.contains(asset) {
                    let confidence = 1. - rounded.parse::<f64>().map_err(|e| Error::invalid(e.to_string()))?;
                    let row = (asset.to_owned(), entity.clone(), confidence);
                    let key = (asset.to_owned(), entity.clone());
                    if let Some(&i) = positions.get(&key) {
                        labels[i] = row;
                    } else {
                        positions.insert(key, labels.len());
                        labels.push(row);
                    }
                }
            }
        }
    }
    csv.flush()?;
    labels.extend(preserved);
    Ok(labels)
}
