//! 离线识别 v1：验证后源事务提交；应用投影与 imported 事件可独立重试。
use crate::app_imports::{Observer, Silent};
use crate::{app_library::Library, Error, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::Path,
    sync::atomic::AtomicBool,
};

const SCHEMA: &str = r#"CREATE TABLE IF NOT EXISTS recognition_imports(
 digest TEXT PRIMARY KEY, manifest TEXT NOT NULL, assets TEXT NOT NULL, status TEXT NOT NULL,
 error TEXT NOT NULL DEFAULT '', imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
 CREATE TABLE IF NOT EXISTS recognition_assets(
 asset TEXT PRIMARY KEY, digest TEXT NOT NULL, package TEXT NOT NULL);
 CREATE TABLE IF NOT EXISTS embeddings(uuid TEXT NOT NULL,model TEXT NOT NULL,vec BLOB NOT NULL,PRIMARY KEY(uuid,model));"#;
fn invalid() -> Error {
    Error::Invalid("识别结果包不合法或与目标图库不匹配".into())
}
fn sha(data: &[u8]) -> String {
    crate::strict_json::sha256_hex(data)
}
fn compact(v: &Value) -> String {
    // Python json.dumps(sort_keys=True,separators=(',',':'),ensure_ascii=True).
    match v {
        Value::Array(a) => format!("[{}]", a.iter().map(compact).collect::<Vec<_>>().join(",")),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter().map(|(k, v)| format!("{}:{}", compact(&json!(k)), compact(v))).collect::<Vec<_>>().join(",")
        ),
        _ => crate::character_json::dump(v, true),
    }
}
fn file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(invalid());
    }
    let options = crate::platform_filesystem::read_nofollow_options();
    let mut data = Vec::new();
    options.open(path)?.take(limit + 1).read_to_end(&mut data)?;
    if data.len() as u64 > limit {
        return Err(invalid());
    }
    Ok(data)
}
pub fn model_specs(root: &Path) -> Result<Value> {
    crate::recognition_compute::models::ModelFiles::resolve(root)?.specs(&|| Ok(()))
}
// NPY's header is a Python literal, never executable. Parse exactly the three permitted fields.
#[cfg(test)]
fn vectors(raw: Vec<u8>, count: usize, dimension: usize) -> Result<Vec<u8>> {
    vectors_checked(raw, count, dimension, &|| Ok(()))
}
pub(crate) fn vectors_checked(
    mut raw: Vec<u8>,
    count: usize,
    dimension: usize,
    check: &dyn Fn() -> Result<()>,
) -> Result<Vec<u8>> {
    if raw.get(..6) != Some(b"\x93NUMPY") {
        return Err(invalid());
    }
    let (start, n) = match raw.get(6..8) {
        Some([1, 0]) if raw.len() >= 10 => {
            (10, u16::from_le_bytes(raw[8..10].try_into().expect("app_recognition internal invariant")) as usize)
        }
        Some([2, 0]) if raw.len() >= 12 => {
            (12, u32::from_le_bytes(raw[8..12].try_into().expect("app_recognition internal invariant")) as usize)
        }
        _ => return Err(invalid()),
    };
    let header = raw.get(start..start + n).and_then(|b| std::str::from_utf8(b).ok()).ok_or_else(invalid)?;
    let h = header.trim();
    // Tokenization preserves strings, and rejects extra keys/expressions instead of searching substrings.
    let mut tokens = Vec::new();
    let mut it = h.chars().peekable();
    while let Some(ch) = it.next() {
        if ch.is_whitespace() {
            continue;
        }
        if ch == '\'' || ch == '"' {
            let mut s = String::new();
            loop {
                let x = it.next().ok_or_else(invalid)?;
                if x == ch {
                    break;
                }
                if x == '\\' {
                    return Err(invalid());
                }
                s.push(x);
            }
            tokens.push(format!("s:{s}"));
        } else if ch.is_ascii_alphanumeric() {
            let mut s = ch.to_string();
            while it.peek().is_some_and(|x| x.is_ascii_alphanumeric()) {
                s.push(it.next().expect("app_recognition internal invariant"));
            }
            tokens.push(s);
        } else {
            tokens.push(ch.to_string());
        }
    }
    let mut p = 0;
    fn take(t: &[String], p: &mut usize, expected: &str) -> Result<()> {
        if t.get(*p).map(String::as_str) != Some(expected) {
            return Err(invalid());
        }
        *p += 1;
        Ok(())
    }
    take(&tokens, &mut p, "{")?;
    let mut keys = HashSet::new();
    while tokens.get(p).map(String::as_str) != Some("}") {
        let key = tokens.get(p).ok_or_else(invalid)?.clone();
        p += 1;
        if !keys.insert(key.clone()) {
            return Err(invalid());
        }
        take(&tokens, &mut p, ":")?;
        match key.as_str() {
            "s:descr" => take(&tokens, &mut p, "s:<f4")?,
            "s:fortran_order" => take(&tokens, &mut p, "False")?,
            "s:shape" => {
                take(&tokens, &mut p, "(")?;
                take(&tokens, &mut p, &count.to_string())?;
                take(&tokens, &mut p, ",")?;
                take(&tokens, &mut p, &dimension.to_string())?;
                if tokens.get(p).map(String::as_str) == Some(",") {
                    p += 1;
                }
                take(&tokens, &mut p, ")")?;
            }
            _ => return Err(invalid()),
        }
        if tokens.get(p).map(String::as_str) == Some(",") {
            p += 1;
        } else if tokens.get(p).map(String::as_str) != Some("}") {
            return Err(invalid());
        }
    }
    take(&tokens, &mut p, "}")?;
    if keys.len() != 3 || p != tokens.len() || raw.len() - start - n != count * dimension * 4 {
        return Err(invalid());
    }
    for row in raw[start + n..].chunks_exact(dimension * 4) {
        check()?;
        let mut norm = 0.0f64;
        for b in row.chunks_exact(4) {
            let v = f32::from_le_bytes(b.try_into().expect("app_recognition internal invariant"));
            if !v.is_finite() {
                return Err(invalid());
            }
            norm += (v as f64).powi(2);
        }
        if norm <= 1e-24 {
            return Err(invalid());
        }
    }
    raw.drain(..start + n);
    Ok(raw)
}
struct Package {
    manifest: Value,
    digest: String,
    ids: Vec<String>,
    labels: HashMap<String, Vec<Value>>,
    wd: Vec<u8>,
    ccip: Vec<u8>,
}
fn validate(
    path: &Path,
    library: &Library,
    root: &Path,
    cancel: &AtomicBool,
    observer: &dyn Observer,
) -> Result<Package> {
    observer.stage("models");
    let specs =
        crate::recognition_compute::models::ModelFiles::resolve(root)?.specs(&|| crate::app_sync::check(cancel))?;
    validate_against(path, library, root, &specs, cancel, observer)
}
fn validate_against(
    path: &Path,
    library: &Library,
    root: &Path,
    specs: &Value,
    cancel: &AtomicBool,
    observer: &dyn Observer,
) -> Result<Package> {
    observer.stage("reading");
    crate::app_sync::check(cancel)?;
    let m = crate::strict_json::parse(
        std::str::from_utf8(&file(&path.join("manifest.json"), 4 * 1024 * 1024)?).map_err(|_| invalid())?,
    )
    .map_err(Error::Invalid)?;
    if m["format"] != "illustash-recognition/1" || m["library_id"] != library.id() || &m["models"] != specs {
        return Err(invalid());
    }
    let ids: Vec<String> = serde_json::from_value(m["assets"].clone()).map_err(|_| invalid())?;
    if ids.is_empty()
        || ids.len() > 100000
        || ids.iter().any(String::is_empty)
        || ids.iter().collect::<HashSet<_>>().len() != ids.len()
    {
        return Err(invalid());
    }
    if m["files"].as_object().map(|o| o.len()) != Some(3) {
        return Err(invalid());
    }
    let mut payload = HashMap::new();
    for (read, name) in ["labels.jsonl", "wd.npy", "ccip.npy"].into_iter().enumerate() {
        observer.count(read as u64, 3);
        crate::app_sync::check(cancel)?;
        let raw = file(&path.join(name), 1024 * 1024 * 1024)?;
        if m["files"][name] != json!({"bytes":raw.len(),"sha256":sha(&raw)}) {
            return Err(invalid());
        }
        payload.insert(name, raw);
    }
    observer.stage("verifying");
    let wd = vectors_checked(
        payload.remove("wd.npy").expect("app_recognition internal invariant"),
        ids.len(),
        1024,
        &|| crate::app_sync::check(cancel),
    )?;
    let ccip = vectors_checked(
        payload.remove("ccip.npy").expect("app_recognition internal invariant"),
        ids.len(),
        768,
        &|| crate::app_sync::check(cancel),
    )?;
    let mut vocab = HashSet::new();
    let mut csv = csv::Reader::from_path(root.join("tags/tags_v3.csv")).map_err(|_| invalid())?;
    let headers = csv.headers().map_err(|_| invalid())?;
    let cat = headers.iter().position(|s| s == "category").ok_or_else(invalid)?;
    let name = headers.iter().position(|s| s == "name").ok_or_else(invalid)?;
    for row in csv.records() {
        let row = row.map_err(|_| invalid())?;
        let dim = match row.get(cat) {
            Some("0") => "general",
            Some("4") => "character",
            Some("9") => "rating",
            _ => continue,
        };
        vocab.insert((dim.to_owned(), row.get(name).ok_or_else(invalid)?.to_owned()));
    }
    let mut labels = ids.iter().map(|u| (u.clone(), Vec::new())).collect::<HashMap<_, _>>();
    let mut keys = HashSet::new();
    let mut rated = HashSet::new();
    for line in std::str::from_utf8(&payload["labels.jsonl"]).map_err(|_| invalid())?.lines() {
        crate::app_sync::check(cancel)?;
        let row = crate::strict_json::parse(line).map_err(Error::Invalid)?;
        let u = row["asset"].as_str().ok_or_else(invalid)?;
        let dim = row["dim"].as_str().ok_or_else(invalid)?;
        let value = row["value"].as_str().ok_or_else(invalid)?;
        let conf = row["confidence"].as_f64().ok_or_else(invalid)?;
        if !labels.contains_key(u)
            || !vocab.contains(&(dim.to_owned(), value.to_owned()))
            || !(0.0..=1.0).contains(&conf)
            || !keys.insert((u.to_owned(), dim.to_owned(), value.to_owned()))
        {
            return Err(invalid());
        }
        labels.get_mut(u).expect("app_recognition internal invariant").push(json!([dim, value, conf]));
        if dim == "rating" {
            rated.insert(u.to_owned());
        }
    }
    if rated.len() != ids.len() {
        return Err(invalid());
    }
    let digest = sha(compact(&m).as_bytes());
    Ok(Package { manifest: m, digest, ids, labels, wd, ccip })
}
fn writer(path: &Path) -> Result<Connection> {
    let c = crate::sqlite::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    crate::configure(&c)?;
    c.busy_timeout(crate::WRITER_BUSY)?;
    Ok(c)
}
/// Optional research dispatcher acknowledges notification delivery, not completion of algorithms.
/// The caller must bind this Library to its immutable package/session before invoking this API.
pub fn import_package(
    library: &Library,
    path: &Path,
    root: &Path,
    cancel: &AtomicBool,
    dispatch: &dyn Fn(&Library, &str) -> Result<()>,
) -> Result<Value> {
    import_internal(library, path, root, cancel, dispatch, None, &Silent)
}
/// `import_package` that reports its stages and publishes its source transaction through `observer` (#88).
pub(crate) fn import_package_observed(
    library: &Library,
    path: &Path,
    root: &Path,
    cancel: &AtomicBool,
    dispatch: &dyn Fn(&Library, &str) -> Result<()>,
    observer: &dyn Observer,
) -> Result<Value> {
    import_internal(library, path, root, cancel, dispatch, None, observer)
}
/// Owned only by one recognition task; build before taking TaskContext::receive.
pub(crate) struct WorkerInputs {
    identity: crate::app_tasks::TaskIdentity,
    snapshot: crate::recognition_compute::anchor_snapshot::Snapshot,
    models: Value,
    root: std::path::PathBuf,
    shared: std::path::PathBuf,
}
impl WorkerInputs {
    pub(crate) fn load(
        library: &Library,
        identity: &crate::app_tasks::TaskIdentity,
        root: &Path,
        cancel: &AtomicBool,
    ) -> Result<Self> {
        if identity.library_id != library.id() || identity.package != library.instance_path() {
            return Err(invalid());
        }
        let check = || crate::app_sync::check(cancel);
        check()?;
        let models = crate::recognition_compute::models::ModelFiles::resolve(root)?.specs(&check)?;
        let shared = crate::app_resources::shared_path(root)?;
        let snapshot =
            crate::recognition_compute::anchor_snapshot::Snapshot::load_checked(&shared, &library.source(), &check)?;
        Ok(Self { identity: identity.clone(), snapshot, models, root: root.to_owned(), shared })
    }
}
/// A receipt proof of full prevalidation, never a second feature/anchor cache.
pub(crate) struct PreparedWorkerPackage {
    package: std::path::PathBuf,
    sidecar: std::path::PathBuf,
    digest: String,
    sidecar_digest: String,
}
pub(crate) fn prepare_worker_package(
    library: &Library,
    path: &Path,
    sidecar: &Path,
    inputs: &WorkerInputs,
    cancel: &AtomicBool,
) -> Result<PreparedWorkerPackage> {
    if inputs.identity.library_id != library.id() || inputs.identity.package != library.instance_path() {
        return Err(invalid());
    }
    let pkg = validate_against(path, library, &inputs.root, &inputs.models, cancel, &Silent)?;
    let bytes = file(sidecar, 4 * 1024 * 1024)?;
    let value =
        crate::strict_json::parse(std::str::from_utf8(&bytes).map_err(|_| invalid())?).map_err(Error::Invalid)?;
    let claims: crate::recognition_compute::claims::Claims = crate::worker_protocol::parse(value, sidecar)?;
    claims.validate(&inputs.identity, &pkg.ids, &inputs.models, &inputs.snapshot, &pkg.ccip, &|| {
        crate::app_sync::check(cancel)
    })?;
    Ok(PreparedWorkerPackage {
        package: path.to_owned(),
        sidecar: sidecar.to_owned(),
        digest: pkg.digest,
        sidecar_digest: sha(&bytes),
    })
}
/// Caller holds the task receipt. Only linear authoritative rechecks occur in the source transaction.
pub(crate) fn import_worker_package(
    library: &Library,
    prepared: &PreparedWorkerPackage,
    inputs: &WorkerInputs,
    cancel: &AtomicBool,
    dispatch: &dyn Fn(&Library, &str) -> Result<()>,
) -> Result<Value> {
    if inputs.identity.library_id != library.id() || inputs.identity.package != library.instance_path() {
        return Err(invalid());
    }
    import_internal(library, &prepared.package, &inputs.root, cancel, dispatch, Some((prepared, inputs)), &Silent)
}
fn import_internal(
    library: &Library,
    path: &Path,
    root: &Path,
    cancel: &AtomicBool,
    dispatch: &dyn Fn(&Library, &str) -> Result<()>,
    claims: Option<(&PreparedWorkerPackage, &WorkerInputs)>,
    observer: &dyn Observer,
) -> Result<Value> {
    if library.frozen() {
        return Err(Error::Conflict("图库只读".into()));
    }
    let mut pkg = if let Some((prepared, inputs)) = claims {
        let pkg = validate_against(path, library, root, &inputs.models, cancel, observer)?;
        if pkg.digest != prepared.digest {
            return Err(invalid());
        }
        pkg
    } else {
        validate(path, library, root, cancel, observer)?
    };
    observer.stage("writing");
    let mut c = writer(&library.source())?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Source lease freezes authoritative role metadata. Recheck bytes/identity/version,
    // never rebuild the task's O(A²) alias index or recompute batch claims here.
    let claims = if let Some((prepared, inputs)) = claims {
        let check = || crate::app_sync::check(cancel);
        check()?;
        let bytes = file(&prepared.sidecar, 4 * 1024 * 1024)?;
        if sha(&bytes) != prepared.sidecar_digest {
            return Err(invalid());
        }
        let value =
            crate::strict_json::parse(std::str::from_utf8(&bytes).map_err(|_| invalid())?).map_err(Error::Invalid)?;
        let claims: crate::recognition_compute::claims::Claims =
            crate::worker_protocol::parse(value.clone(), &prepared.sidecar)?;
        if claims.identity != inputs.identity
            || claims.anchors
                != crate::recognition_compute::anchor_snapshot::Snapshot::fingerprint(&inputs.shared, &tx, &check)?
            || crate::recognition_compute::models::ModelFiles::resolve(root)?.specs(&check)? != inputs.models
        {
            return Err(invalid());
        }
        for claim in &claims.claims {
            let known: bool =
                tx.query_row("SELECT EXISTS(SELECT 1 FROM characters WHERE char_id=?)", [&claim.character], |r| {
                    r.get(0)
                })?;
            if !known {
                return Err(invalid());
            }
        }
        if claims.anchors.vectors.is_some() {
            pkg.digest = sha(compact(&json!([pkg.manifest, value])).as_bytes());
            pkg.manifest = json!({"recognition":pkg.manifest,"character_claims":value});
            Some(claims)
        } else {
            // No-anchor sidecars are still mandatory/validated for native tasks, but
            // preserve the public v1 per-asset/package fingerprint and duplicate behavior.
            None
        }
    } else {
        None
    };
    let models =
        if claims.is_some() { pkg.manifest["recognition"]["models"].clone() } else { pkg.manifest["models"].clone() };
    tx.execute_batch(SCHEMA)?;
    for id in &pkg.ids {
        let live: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM assets WHERE uuid=? AND in_scope=1 AND coalesce(trashed,0)=0)",
            [id],
            |r| r.get(0),
        )?;
        if !live {
            return Err(invalid());
        }
    }
    let duplicate: bool =
        tx.query_row("SELECT EXISTS(SELECT 1 FROM recognition_imports WHERE digest=?)", [&pkg.digest], |r| r.get(0))?;
    if !duplicate {
        let mut added = Vec::new();
        for (i, u) in pkg.ids.iter().enumerate() {
            observer.count(i as u64, pkg.ids.len() as u64);
            crate::app_sync::check(cancel)?;
            let wd = &pkg.wd[i * 1024 * 4..(i + 1) * 1024 * 4];
            let ccip = &pkg.ccip[i * 768 * 4..(i + 1) * 768 * 4];
            let mut labels = pkg.labels[u].clone();
            labels.sort_by(|a, b| a[0].as_str().cmp(&b[0].as_str()).then(a[1].as_str().cmp(&b[1].as_str())));
            let mut bytes = crate::character_json::dump(&models, true).into_bytes();
            bytes.extend(compact(&json!(labels)).as_bytes());
            bytes.extend(wd);
            bytes.extend(ccip);
            if let Some(claims) = &claims {
                let rows = claims.claims.iter().filter(|c| &c.asset == u).collect::<Vec<_>>();
                bytes.extend(compact(&json!({"anchors":claims.anchors,"claims":rows})).as_bytes());
            }
            let fingerprint = sha(&bytes);
            let old: Option<String> =
                tx.query_row("SELECT digest FROM recognition_assets WHERE asset=?", [u], |r| r.get(0)).optional()?;
            if let Some(old) = old {
                if old != fingerprint {
                    return Err(invalid());
                }
                continue;
            }
            tx.execute(
                "DELETE FROM labels WHERE uuid=? AND source='wd_v3' AND dim IN ('general','character','rating')",
                [u],
            )?;
            for label in &pkg.labels[u] {
                tx.execute(
                    "INSERT INTO labels(uuid,dim,value,conf,source,model_ver,run_id) VALUES (?,?,?,?,'wd_v3',?,?)",
                    params![
                        u,
                        label[0].as_str(),
                        label[1].as_str(),
                        label[2].as_f64(),
                        models["wd"]["version"].as_str(),
                        format!("import:{}", pkg.digest)
                    ],
                )?;
            }
            if let Some(claims) = &claims {
                tx.execute("DELETE FROM labels WHERE uuid=? AND source='char_index' AND dim='char_ext'", [u])?;
                for claim in claims.claims.iter().filter(|c| &c.asset == u) {
                    tx.execute("INSERT INTO labels(uuid,dim,value,conf,source,model_ver,run_id) VALUES (?,'char_ext',?,?,'char_index','character_index@2025-09-25',?)",params![u,claim.character,claim.confidence,format!("import:{}",pkg.digest)])?;
                }
            }
            for (model, vec) in [("wd", wd), ("ccip", ccip)] {
                tx.execute("INSERT OR REPLACE INTO embeddings VALUES (?,?,?)", params![u, model, vec])?;
            }
            tx.execute("INSERT INTO recognition_assets VALUES (?,?,?)", params![u, fingerprint, pkg.digest])?;
            added.push(u.clone());
        }
        tx.execute(
            "INSERT INTO recognition_imports(digest,manifest,assets,status) VALUES (?,?,?,'pending')",
            params![
                pkg.digest,
                crate::character_json::dump(&pkg.manifest, true),
                crate::character_json::dump(&json!(added), true)
            ],
        )?;
    }
    observer.stage("committing");
    crate::app_sync::check(cancel)?;
    // The publication point: nothing reaches the library unless this commit runs, and it runs only if no
    // cancellation came first. A cancellation arriving during it waits; the projection below then pauses as pending.
    let mut tx = Some(tx);
    observer.publish(&mut || Ok(tx.take().expect("the source transaction is published once").commit()?))?;
    drop(tx);
    drop(c);
    observer.stage("projecting");
    let mut result = resume(library, cancel, dispatch)?;
    result["digest"] = json!(pkg.digest);
    result["duplicate"] = json!(duplicate);
    Ok(result)
}
/// Source committed records survive application/effect failure; projection marker and event commit together.
pub fn resume(
    library: &Library,
    cancel: &AtomicBool,
    dispatch: &dyn Fn(&Library, &str) -> Result<()>,
) -> Result<Value> {
    if library.frozen() {
        return Err(Error::Conflict("图库只读".into()));
    }
    let mut c = writer(&library.source())?;
    let exists: bool =
        c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='recognition_imports')", [], |r| r.get(0))?;
    if !exists {
        return Ok(json!({"status":"done"}));
    }
    // Serializes concurrent resumes for this package without a process-global or library-ID lock.
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let pending = tx
        .prepare("SELECT digest,assets FROM recognition_imports WHERE status<>'done' ORDER BY rowid")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (digest, raw) in pending {
        let process: Result<()> = (|| {
            crate::app_sync::check(cancel)?;
            let ids: Vec<String> = serde_json::from_str(&raw).map_err(|_| invalid())?;
            if ids.is_empty() {
                return Ok(());
            }
            let mut app = writer(&library.db())?;
            let marker = format!("recognition_projection:{digest}");
            let event: Option<String> =
                app.query_row("SELECT value FROM meta WHERE key=?", [&marker], |r| r.get(0)).optional()?;
            let event = if let Some(event) = event {
                event
            } else {
                crate::app_migrate::deliver_wd(&mut app, &library.source(), library.id(), Some(&ids), true)?;
                let src = crate::app_library::read_connection(&library.source())?;
                let app_tx = app.transaction_with_behavior(TransactionBehavior::Immediate)?;
                crate::schema::begin_writing(&app_tx)?;
                if !ids.is_empty() {
                    import_changes(&app_tx, &src, library, &ids)?;
                }
                app_tx.execute("INSERT INTO backend_effects(kind) VALUES ('imported')", [])?;
                let event = app_tx.last_insert_rowid().to_string();
                app_tx.execute("INSERT INTO meta VALUES (?,?)", params![marker, event])?;
                crate::app_sync::check(cancel)?;
                crate::schema::commit_writing(app_tx)?;
                event
            };
            let status: String =
                app.query_row("SELECT status FROM backend_effects WHERE id=?", [&event], |r| r.get(0))?;
            if status != "done" {
                if let Err(e) = dispatch(library, "imported") {
                    app.execute(
                        "UPDATE backend_effects SET error=? WHERE id=?",
                        params![e.to_string().chars().take(500).collect::<String>(), event],
                    )?;
                    return Err(e);
                }
                crate::app_sync::check(cancel)?;
                app.execute("UPDATE backend_effects SET status='done',error='' WHERE id=?", [event])?;
            }
            Ok(())
        })();
        match process {
            Ok(()) => {
                tx.execute("UPDATE recognition_imports SET status='done',error='' WHERE digest=?", [digest])?;
            }
            Err(e) => {
                let message = e.to_string().chars().take(500).collect::<String>();
                tx.execute(
                    "UPDATE recognition_imports SET status='pending',error=? WHERE digest=?",
                    params![message, digest],
                )?;
                tx.commit()?;
                return Ok(json!({"status":"pending","error":message}));
            }
        }
    }
    tx.commit()?;
    Ok(json!({"status":"done"}))
}
// Preserve the existing 500-ID query batches and their row order.
fn source_rows(src: &Connection, sql: &str, ids: &[String]) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for chunk in ids.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        out.extend(crate::app_migrate_data::rows_with(
            src,
            &sql.replace("{}", &marks),
            rusqlite::params_from_iter(chunk),
        )?);
    }
    Ok(out)
}
fn import_changes(app: &Connection, src: &Connection, library: &Library, ids: &[String]) -> Result<()> {
    let list = serde_json::to_string(ids).expect("internal JSON value is serializable").replace('\'', "''");
    let scope = format!("uuid IN (SELECT value FROM json_each('{list}'))");
    let rows=source_rows(src,"SELECT uuid,filename,src_path,width,height,added_at,src_kind FROM assets WHERE in_scope=1 AND coalesce(trashed,0)=0 AND uuid IN ({})",ids)?;
    let added =
        rows.iter().map(|r| r[0].as_str().expect("validated internal text field").to_owned()).collect::<Vec<_>>();
    if rows.len() != ids.len() {
        return Err(invalid());
    }
    let mut shape = crate::app_migrate_data::rows(
        app,
        &format!("SELECT uuid,filename,src_path,width,height,added_at,src_kind FROM assets WHERE NOT ({scope})"),
    )?;
    shape.extend(rows.clone());
    let album = library.source().parent().expect("internal file path must have a parent").join("album-char-map.json");
    let album = if album.exists() {
        crate::strict_json::parse(&fs::read_to_string(album)?).map_err(Error::Invalid)?["map"].clone()
    } else {
        json!({})
    };
    let stamp: String = src.query_row("SELECT datetime('now','localtime')", [], |r| r.get(0))?;
    let args = json!({"op":"import","rows":rows,"removed":[],"run_id":null,"new_ids":{},"wd_tags":crate::app_migrate_data::rows(src,"SELECT char_id,wd_tag FROM characters")?,"labels":source_rows(src,"SELECT uuid,value,conf,dim FROM labels WHERE ((dim='character' AND conf>=0.35) OR dim='char_ext') AND uuid IN ({})",&added)?,"decisions":source_rows(src,"SELECT uuid,value FROM decisions WHERE dim='character' AND uuid IN ({})",&added)?,"album_map":album,"duplicates":crate::app_migrate_data::duplicates(app,src,&shape)?,"realistic":crate::app_migrate_data::rows(src,"SELECT uuid FROM labels WHERE dim='general' AND value='realistic' AND conf>=0.15")?.iter().map(|r|r[0].clone()).collect::<Vec<_>>(),"indexed":stamp});
    crate::character_commands::command(app, &args)?;
    Ok(())
}

#[cfg(test)]
#[path = "app_recognition/tests.rs"]
mod claims_tests;
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    #[test]
    fn source_queries_keep_python_500_id_batch_row_order() {
        let src = Connection::open_in_memory().unwrap();
        src.execute_batch("CREATE TABLE assets(uuid TEXT PRIMARY KEY)").unwrap();
        let ids = (0..503).rev().map(|i| format!("u{i:03}")).collect::<Vec<_>>();
        for id in &ids {
            src.execute("INSERT INTO assets VALUES (?)", [id]).unwrap();
        }
        let rows = source_rows(&src, "SELECT uuid FROM assets WHERE uuid IN ({})", &ids).unwrap();
        assert_eq!(rows[0], json!(["u003"]));
        assert_eq!(rows[499], json!(["u502"]));
        assert_eq!(rows[500], json!(["u000"]));
        assert_eq!(rows[502], json!(["u002"]));
    }
    #[test]
    fn numpy_literal_parser_does_not_accept_hidden_tokens() {
        fn npy(header: &str) -> Vec<u8> {
            let mut b = b"\x93NUMPY\x01\x00".to_vec();
            b.extend((header.len() as u16).to_le_bytes());
            b.extend(header.as_bytes());
            b.extend(1.0f32.to_le_bytes());
            b
        }
        assert_eq!(
            vectors(npy("{'descr': '<f4', 'fortran_order': False, 'shape': (1, 1), }\n"), 1, 1).unwrap(),
            1.0f32.to_le_bytes()
        );
        for h in [
            "{'descr':'<f4','fortran_order':False,'shape':(1,1),'extra':'<f4'}",
            "{'descr':'<f4','descr':'<f4','fortran_order':False,'shape':(1,1)}",
            "{'descr':'<f4','fortran_order':False,'shape':(1,1)} junk",
            "{'descr':'O','fortran_order':False,'shape':(1,1)}",
            "{'descr':'<f4','fortran_order':False,'shape':(1,1+0)}",
        ] {
            assert!(vectors(npy(h), 1, 1).is_err());
        }
    }
    #[test]
    fn persisted_python_fingerprints_keep_ascii_and_float_layout() {
        assert_eq!(
            compact(&json!({"z":"人物🙂","a":[0.9,1e-7,-0.0]})),
            r#"{"a":[0.9,1e-07,-0.0],"z":"\u4eba\u7269\ud83d\ude42"}"#
        );
    }
    #[test]
    fn dispatch_failure_recovery_and_cancel_preserve_committed_projection() {
        let root = std::env::temp_dir().join(format!(
            "illustash-recognition-{}-{}.pilib",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let run = || -> Result<()> {
            fs::write(root.join("library.json"), r#"{"format":1,"id":"synthetic","name":"synthetic","sources":[]}"#)?;
            let app = crate::sqlite::open(root.join("library.sqlite"))?;
            app.execute_batch(&format!("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT); INSERT INTO meta VALUES ('schema_version','36'),('terms_schema','{v}'),('terms_artists','core'),('terms_labels','core'),('terms_characters','core'),('terms_names','core'),('recognition_projection:d','1'); CREATE TABLE backend_effects(id INTEGER PRIMARY KEY,kind TEXT,status TEXT,error TEXT); INSERT INTO backend_effects VALUES (1,'imported','pending','');", v = crate::SCHEMA_VERSION))?;
            let source = crate::sqlite::open(root.join("source.sqlite"))?;
            source.execute_batch("CREATE TABLE assets(uuid TEXT,filename TEXT,src_path TEXT,width INTEGER,height INTEGER,added_at TEXT,src_kind TEXT,in_scope INTEGER,trashed INTEGER); CREATE TABLE labels(uuid TEXT,dim TEXT,value TEXT,conf REAL);")?;
            source.execute_batch(SCHEMA)?;
            source.execute(
                "INSERT INTO recognition_imports(digest,manifest,assets,status) VALUES ('d','{}','[\"a\"]','pending')",
                [],
            )?;
            let library = Library::open(&root)?;
            let cancel = AtomicBool::new(false);
            assert_eq!(
                resume(&library, &cancel, &|_, _| Err(Error::Invalid("synthetic dispatch failure".into())))?["status"],
                "pending"
            );
            assert!(app
                .query_row("SELECT error FROM backend_effects", [], |r| r.get::<_, String>(0))?
                .contains("synthetic dispatch failure"));
            cancel.store(true, Ordering::SeqCst);
            assert_eq!(resume(&library, &cancel, &|_, _| panic!("cancel must precede dispatch"))?["status"], "pending");
            cancel.store(false, Ordering::SeqCst);
            assert_eq!(
                resume(&library, &cancel, &|lib, kind| {
                    assert_eq!(lib.instance_path(), crate::app_paths::canonical(&root)?);
                    assert_eq!(kind, "imported");
                    Ok(())
                })?["status"],
                "done"
            );
            assert_eq!(resume(&library, &cancel, &|_, _| panic!("done must not redispatch"))?["status"], "done");
            assert_eq!(app.query_row("SELECT count(*) FROM backend_effects", [], |r| r.get::<_, i64>(0))?, 1);
            Ok(())
        };
        let result = run();
        fs::remove_dir_all(&root).unwrap();
        result.unwrap();
    }
}
