//! Photos 平台适配：显式配置的数据库只读，差异只写本包源库。
use crate::{Error, Result};
use rusqlite::{params, OpenFlags};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

pub(crate) fn scan(
    source: &Value,
    db: &Path,
    check: &dyn Fn() -> Result<()>,
    progress: &dyn Fn(usize, usize),
) -> Result<Value> {
    scan_ordered(source, db, check, progress).map(|(counts, _)| counts)
}
pub(crate) fn scan_ordered(
    source: &Value,
    db: &Path,
    check: &dyn Fn() -> Result<()>,
    progress: &dyn Fn(usize, usize),
) -> Result<(Value, Vec<String>)> {
    let sid = source["id"].as_str().ok_or_else(|| Error::Invalid("Photos 缺少来源 ID".into()))?;
    let raw = source["path"].as_str().ok_or_else(|| Error::Invalid("Photos 缺少来源路径".into()))?;
    let (root, rows) = crate::platform_photos::read(raw)?;
    let mut c = crate::sqlite::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    c.busy_timeout(crate::app_library::TASK_WRITE_WAIT)?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute_batch(include_str!("app_source_schema.sql"))?;
    let known = tx
        .prepare("SELECT uuid,coalesce(trashed,0) FROM assets WHERE in_scope=1 AND source_id=?")?
        .query_map([sid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;
    let mut live = HashSet::new();
    let mut totals = serde_json::Map::new();
    let mut order = Vec::new();
    let (mut new, mut back) = (0, 0);
    let defaults = crate::origin::photos_defaults();
    let apps = source.get("apps").unwrap_or(&defaults);
    for (i, r) in rows.iter().enumerate() {
        check()?;
        let uuid = r[0].as_str().ok_or_else(|| Error::Invalid("Photos 标识不合法".into()))?.to_uppercase();
        live.insert(uuid.clone());
        let app = r[8].as_str().unwrap_or("");
        let entry = totals.entry(app.to_owned()).or_insert_with(|| {
            order.push(app.to_owned());
            let name = crate::origin::photos_app(app).map(|o| o.label()).unwrap_or_else(|| match app {
                "com.apple.springboard" => "截屏",
                "com.apple.camera" => "相机",
                "com.apple.mobileslideshow" => "照片",
                "com.apple.mobilesafari" => "Safari",
                "tv.danmaku.bilianime" => "哔哩哔哩",
                "ai.x.GrokApp" => "Grok",
                "com.tencent.xin" => "微信",
                "com.baidu.netdisk" => "百度网盘",
                "app.nicegram" => "Nicegram",
                "" => "未知来源",
                _ => app,
            });
            json!({"name":name,"photos":0,"collected":0})
        });
        entry["photos"] = json!(entry["photos"].as_u64().expect("validated internal numeric field") + 1);
        let Some(preset) = apps.get(app).and_then(Value::as_str) else {
            progress(i + 1, rows.len());
            continue;
        };
        entry["collected"] = json!(entry["collected"].as_u64().expect("validated internal numeric field") + 1);
        let (kind, p) =
            crate::platform_photos::path(&root, &uuid, r[6].as_str().unwrap_or(""), r[7].as_str().unwrap_or(""))?;
        // 盘点开始时已归本来源的图片有明确归属。其余仍查事务内的现状，防止覆盖不同来源的图片。
        let exists = if known.contains_key(&uuid) {
            true
        } else {
            let owner: Option<String> = tx
                .prepare_cached("SELECT coalesce(source_id,'') FROM assets WHERE uuid=?")?
                .query_row([&uuid], |r| r.get(0))
                .optional()?;
            if owner.as_deref().is_some_and(|v| !v.is_empty() && v != sid) {
                return Err(Error::Conflict("Photos 资产属于其他来源".into()));
            }
            owner.is_some()
        };
        if !exists {
            let when = r[4]
                .as_f64()
                .and_then(|v| chrono::DateTime::from_timestamp((v + 978307200.0) as i64, 0))
                .map(|v| v.with_timezone(&chrono::Local).format("%Y-%m-%dT%H:%M:%S").to_string())
                .unwrap_or_default();
            tx.execute("INSERT INTO assets(uuid,filename,z_pk,width,height,added_at,hidden,trashed,stable_hash,orig_size,src_kind,src_path,in_scope,source_id,preset) VALUES (?,?,?,?,?,?,?,0,?,?,?,?,1,?,?)",params![uuid,r[7].as_str(),r[1].as_i64(),r[2].as_i64(),r[3].as_i64(),when,r[5].as_i64(),r[9].as_str(),r[10].as_i64(),kind,p.to_string_lossy(),sid,preset])?;
            new += 1;
        } else {
            if known.get(&uuid) == Some(&1) {
                back += 1;
            }
            tx.prepare_cached(
                "UPDATE assets SET trashed=0,src_kind=?1,src_path=?2,source_id=?3,preset=?4 WHERE uuid=?5
                 AND (trashed IS NOT 0 OR src_kind IS NOT ?1 OR src_path IS NOT ?2
                      OR source_id IS NOT ?3 OR preset IS NOT ?4)",
            )?
            .execute(params![kind, p.to_string_lossy(), sid, preset, uuid])?;
        }
        tx.prepare_cached("INSERT OR IGNORE INTO asset_links(asset,system,key,source_id) VALUES (?1,'photos',?1,?2)")?
            .execute(params![uuid, sid])?;
        progress(i + 1, rows.len());
    }
    let mut gone = 0;
    for (uuid, trashed) in known {
        if trashed == 0 && !live.contains(&uuid) {
            tx.execute("UPDATE assets SET trashed=1 WHERE uuid=?", [uuid])?;
            gone += 1;
        }
    }
    check()?;
    tx.commit()?;
    Ok((json!({"new":new,"returned":back,"gone":gone,"sources":totals}), order))
}
use rusqlite::OptionalExtension;

#[cfg(test)]
#[path = "../tests/support/origin_photos.rs"]
mod origin_tests;
