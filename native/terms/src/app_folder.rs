//! 文件夹来源盘点：只读来源文件；source 提交与应用刷新是两个可重试边界。
use crate::{Error, Result};
use rusqlite::{params, OpenFlags, OptionalExtension};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

use crate::platform_folder::{describe, hash, walk};
#[derive(Debug)]
pub(crate) struct Scan {
    pub counts: Value,
    pub changed: Vec<String>,
}
/// The IDs pictures had before source.sqlite was lost (#228): by full path, then by content. Rescuing a damaged
/// source.sqlite scans with them, so a picture the rescue could not read back keeps its ID and its curation.
#[derive(Default)]
pub(crate) struct Earlier {
    pub by_path: HashMap<String, String>,
    /// Several pictures can share content: each ID is given to one of them only.
    pub by_hash: HashMap<String, Vec<String>>,
    /// The content each ID had, to tell whether its recognition results still describe the picture.
    pub hash_of: HashMap<String, String>,
    /// IDs already given out, in this scan or an earlier one of the same rescue.
    pub given: HashSet<String>,
}
impl Earlier {
    fn claim(&mut self, path: &str, hash: &str, taken: &HashSet<String>) -> Option<String> {
        let free = |u: &String, given: &HashSet<String>| !taken.contains(u) && !given.contains(u);
        let found =
            self.by_path.get(path).filter(|u| free(u, &self.given)).cloned().or_else(|| {
                self.by_hash.get(hash).and_then(|ids| ids.iter().find(|u| free(u, &self.given)).cloned())
            })?;
        self.given.insert(found.clone());
        Some(found)
    }
}
pub(crate) fn scan(
    source: &Value,
    db: &Path,
    check: &dyn Fn() -> Result<()>,
    progress: &dyn Fn(usize, usize),
) -> Result<Scan> {
    scan_with(source, db, check, progress, None)
}
/// `scan`, giving a new picture its earlier ID when `earlier` knows it (a rescue, #228).
pub(crate) fn scan_with(
    source: &Value,
    db: &Path,
    check: &dyn Fn() -> Result<()>,
    progress: &dyn Fn(usize, usize),
    mut earlier: Option<&mut Earlier>,
) -> Result<Scan> {
    let root = Path::new(source["path"].as_str().ok_or_else(|| Error::Invalid("来源缺少路径".into()))?);
    if !root.is_dir() {
        return Err(Error::Invalid(format!("文件夹不存在或无法访问：{path}", path = root.display())));
    }
    let sid = source["id"].as_str().ok_or_else(|| Error::Invalid("来源缺少 id".into()))?;
    let preset = source["preset"].as_str().unwrap_or("other");
    let mut c = crate::sqlite::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    c.busy_timeout(crate::WRITER_BUSY)?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute_batch(include_str!("app_source_schema.sql"))?;
    type Old = (i64, i64, String, String);
    let known_order = tx
        .prepare("SELECT path,size,mtime_ns,md5,asset FROM files WHERE source_id=?")?
        .query_map([sid], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?),
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let known: HashMap<String, Old> = known_order.iter().cloned().collect();
    // Pictures some source still has on file keep the ID they have there; only the others can take an earlier one.
    let taken = match earlier {
        Some(_) => tx
            .prepare("SELECT asset FROM files")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<HashSet<_>>>()?,
        None => HashSet::new(),
    };
    let trashed = tx
        .prepare("SELECT uuid FROM assets WHERE source_id=? AND coalesce(trashed,0)=1")?
        .query_map([sid], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    let (mut files, mut unreadable) = (Vec::new(), Vec::new());
    walk(root, root, &mut files, &mut unreadable, check)?;
    let mut seen = Vec::new();
    let mut hashed = Vec::new();
    let total = files.len();
    for (i, mut f) in files.into_iter().enumerate() {
        check()?;
        if let Some(old) = known.get(&f.rel).filter(|old| old.0 == f.size && old.1 == f.mtime) {
            seen.push((f.rel, old.3.clone()));
        } else {
            match hash(&f.path, check) {
                Ok(h) => {
                    f.hash = h;
                    hashed.push(f);
                }
                Err(Error::Io(_)) => unreadable.push(f.rel.clone()),
                Err(e) => return Err(e),
            }
        }
        progress(i + 1, total);
    }
    let present = seen.iter().map(|r| r.0.clone()).chain(hashed.iter().map(|f| f.rel.clone())).collect::<HashSet<_>>();
    // A path that could not be read is not known to be gone: its asset stays as it is until a scan reads it.
    let unknown = |p: &str| unreadable.iter().any(|u| u.is_empty() || p == u || p.starts_with(&format!("{u}/")));
    let mut vanished = known_order
        .iter()
        .filter(|(p, _)| !present.contains(p) && !unknown(p))
        .map(|(p, _)| p.clone())
        .collect::<HashSet<_>>();
    let mut movable: HashMap<String, std::collections::VecDeque<(String, String)>> = HashMap::new();
    for (p, old) in &known_order {
        if vanished.contains(p) {
            movable.entry(old.2.clone()).or_default().push_back((p.clone(), old.3.clone()));
        }
    }
    let (mut new, mut moved, mut kept, mut changed) = (0, 0, 0, Vec::new());
    let journal_path = db.with_extension("scan-pending.json");
    let mut journal: serde_json::Map<String, Value> = if journal_path.exists() {
        serde_json::from_str(&fs::read_to_string(&journal_path)?).map_err(|e| Error::Invalid(e.to_string()))?
    } else {
        serde_json::Map::new()
    };
    let mut back = seen.iter().map(|r| r.1.clone()).filter(|u| trashed.contains(u)).collect::<Vec<_>>();
    let mut planned = Vec::new();
    for f in hashed {
        let movement =
            if known.contains_key(&f.rel) { None } else { movable.get_mut(&f.hash).and_then(|v| v.pop_front()) };
        let kind = if known.contains_key(&f.rel) {
            2
        } else if movement.is_some() {
            1
        } else {
            0
        };
        planned.push((kind, f, movement));
    }
    planned.sort_by_key(|p| p.0);
    for (_, f, movement) in planned {
        check()?;
        let asset;
        if let Some(old) = known.get(&f.rel) {
            asset = old.3.clone();
            changed.push(asset.clone());
            journal.insert(asset.clone(), json!(f.hash));
            let (w, h, _) = describe(&f);
            tx.execute(
                "UPDATE assets SET width=?,height=?,stable_hash=?,orig_size=? WHERE uuid=?",
                params![w, h, f.hash, f.size, asset],
            )?;
            tx.execute("DELETE FROM labels WHERE uuid=?", [&asset])?;
            tx.execute("DELETE FROM embeddings WHERE uuid=?", [&asset])?;
            tx.execute(
                "UPDATE files SET size=?,mtime_ns=?,md5=? WHERE source_id=? AND path=?",
                params![f.size, f.mtime, f.hash, sid, f.rel],
            )?;
            // A file written back to the path it was removed from returns in this scan, not the next one.
            if trashed.contains(&asset) {
                back.push(asset.clone());
            }
            continue;
        } else if let Some((old_path, u)) = movement {
            asset = u;
            moved += 1;
            vanished.remove(&old_path);
            tx.execute(
                "UPDATE assets SET filename=?,src_path=? WHERE uuid=?",
                params![
                    f.path.file_name().expect("app_folder internal invariant").to_string_lossy(),
                    f.path.to_string_lossy(),
                    asset
                ],
            )?;
            tx.execute(
                "DELETE FROM asset_links WHERE system='file' AND key=? AND source_id=?",
                params![old_path, sid],
            )?;
            tx.execute("DELETE FROM files WHERE source_id=? AND path=?", params![sid, old_path])?;
            if trashed.contains(&asset) {
                back.push(asset.clone());
            }
        } else if let Some(u) = earlier.as_deref_mut().and_then(|e| e.claim(&f.path.to_string_lossy(), &f.hash, &taken))
        {
            asset = u;
            kept += 1;
            let name = f.path.file_name().expect("app_folder internal invariant").to_string_lossy().into_owned();
            let (w, h, added) = describe(&f);
            let had: Option<Option<String>> =
                tx.query_row("SELECT stable_hash FROM assets WHERE uuid=?", [&asset], |r| r.get(0)).optional()?;
            let before = match had {
                Some(hash) => {
                    tx.execute("UPDATE assets SET filename=?,width=?,height=?,trashed=0,stable_hash=?,orig_size=?,src_kind='orig',src_path=?,in_scope=1,source_id=?,preset=? WHERE uuid=?",params![name,w,h,f.hash,f.size,f.path.to_string_lossy(),sid,preset,asset])?;
                    hash
                }
                None => {
                    tx.execute("INSERT INTO assets(uuid,filename,width,height,added_at,hidden,trashed,stable_hash,orig_size,src_kind,src_path,in_scope,source_id,preset) VALUES (?,?,?,?,?,0,0,?,?,'orig',?,1,?,?)",params![asset,name,w,h,added,f.hash,f.size,f.path.to_string_lossy(),sid,preset])?;
                    earlier.as_deref().and_then(|e| e.hash_of.get(&asset).cloned())
                }
            };
            // Results for other content no longer describe this picture: it is recognized again.
            if before.as_deref() != Some(f.hash.as_str()) {
                tx.execute("DELETE FROM labels WHERE uuid=?", [&asset])?;
                tx.execute("DELETE FROM embeddings WHERE uuid=?", [&asset])?;
            }
        } else {
            asset = crate::store::new_key()?.to_uppercase();
            new += 1;
            let (w, h, added) = describe(&f);
            tx.execute("INSERT INTO assets(uuid,filename,width,height,added_at,hidden,trashed,stable_hash,orig_size,src_kind,src_path,in_scope,source_id,preset) VALUES (?,?,?,?,?,0,0,?,?,'orig',?,1,?,?)",params![asset,f.path.file_name().expect("app_folder internal invariant").to_string_lossy(),w,h,added,f.hash,f.size,f.path.to_string_lossy(),sid,preset])?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO asset_links(asset,system,key,source_id) VALUES (?,'file',?,?)",
            params![asset, f.rel, sid],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO files VALUES (?,?,?,?,?,?)",
            params![sid, f.rel, f.size, f.mtime, f.hash, asset],
        )?;
    }
    for p in &vanished {
        tx.execute("UPDATE assets SET trashed=1 WHERE uuid=?", [&known[p].3])?;
    }
    for u in &back {
        tx.execute("UPDATE assets SET trashed=0 WHERE uuid=?", [u])?;
    }
    let total: i64 = tx.query_row("SELECT count(*) FROM files WHERE source_id=?", [sid], |r| r.get(0))?;
    check()?;
    if !journal.is_empty() {
        use std::io::Write;
        let temp = db.with_extension(format!("scan-{}.tmp", crate::store::new_key()?));
        let publish = (|| -> Result<()> {
            let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
            file.write_all(serde_json::to_string(&journal).expect("internal JSON value is serializable").as_bytes())?;
            file.sync_all()?;
            fs::rename(&temp, &journal_path)?;
            Ok(())
        })();
        if publish.is_err() {
            let _ = fs::remove_file(&temp);
        }
        publish?;
    }
    check()?;
    tx.commit()?;
    let name = crate::origin::get(preset).label();
    let mut counts = json!({"new":new,"returned":back.len(),"gone":vanished.len(),"moved":moved,"changed":changed.len(),"sources":{sid:{"name":format!("{}（{}）",name,root.file_name().unwrap_or_default().to_string_lossy()),"photos":total-vanished.len() as i64,"collected":total-vanished.len() as i64}}});
    if !unreadable.is_empty() {
        counts["unreadable"] = json!(unreadable.len());
    }
    if earlier.is_some() {
        counts["kept"] = json!(kept);
    }
    Ok(Scan { counts, changed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    #[test]
    fn interrupted_hashing_rolls_back_source_and_retry_can_commit() {
        let root = std::env::temp_dir().join(format!("illustash-folder-cancel-{}", crate::store::new_key().unwrap()));
        fs::create_dir(&root).unwrap();
        let images = root.join("images");
        fs::create_dir(&images).unwrap();
        image::RgbImage::new(4, 3).save(images.join("a.png")).unwrap();
        let db = root.join("source.sqlite");
        let c = crate::sqlite::open(&db).unwrap();
        c.execute_batch("CREATE TABLE assets(uuid TEXT PRIMARY KEY,filename TEXT,width INTEGER,height INTEGER,added_at TEXT,hidden INTEGER,trashed INTEGER,stable_hash TEXT,orig_size INTEGER,src_kind TEXT,src_path TEXT,in_scope INTEGER,source_id TEXT,preset TEXT); CREATE TABLE labels(uuid TEXT); CREATE TABLE embeddings(uuid TEXT);").unwrap();
        c.execute_batch(include_str!("app_source_schema.sql")).unwrap();
        let source = json!({"id":"s1","path":images,"preset":"other"});
        let cancel = AtomicBool::new(false);
        let result =
            scan(&source, &db, &|| crate::app_sync::check(&cancel), &|_, _| cancel.store(true, Ordering::SeqCst));
        assert!(matches!(result, Err(Error::Conflict(_))));
        assert_eq!(c.query_row("SELECT count(*) FROM assets", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        assert_eq!(c.query_row("SELECT count(*) FROM files", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        cancel.store(false, Ordering::SeqCst);
        assert_eq!(scan(&source, &db, &|| crate::app_sync::check(&cancel), &|_, _| {}).unwrap().counts["new"], 1);
        assert_eq!(scan(&source, &db, &|| crate::app_sync::check(&cancel), &|_, _| {}).unwrap().counts["new"], 0);
        drop(c);
        fs::remove_dir_all(root).unwrap();
    }
}
