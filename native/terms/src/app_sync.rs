//! 原生来源同步与可选识别 worker 编排。任务固定包实例，切库/退出取消并等待。
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
use crate::{app_library::Library, Error, Result};
use crate::{
    task_state::{State, Status},
    worker_protocol as protocol,
};
use rusqlite::{params, Connection, OpenFlags};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

pub(crate) struct Syncer {
    state: Arc<Mutex<State>>,
    task: Option<crate::app_tasks::Task>,
    pub resource_root: std::path::PathBuf,
    pub per_run: usize,
    pub enabled: bool,
}
impl Default for Syncer {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::from(json!({"status":"idle"})))),
            task: None,
            resource_root: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
            per_run: 0,
            enabled: true,
        }
    }
}
impl Syncer {
    pub fn running(&self) -> bool {
        self.task.as_ref().is_some_and(crate::app_tasks::Task::running)
    }
    pub fn available(library: &Library) -> bool {
        !library.frozen()
            && library.config()["sources"].as_array().is_some_and(|s| {
                !s.is_empty() && s.iter().all(|s| matches!(s["kind"].as_str(), Some("folder" | "photos")))
            })
    }
    pub fn status(&self, library: &Library) -> Result<crate::http_json::Json> {
        use crate::http_json::Json;
        let c = crate::app_library::read_connection(&library.db())?;
        let mut q = c.prepare("SELECT * FROM sync_runs ORDER BY id DESC LIMIT 8")?;
        let names = q.column_names().iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let history = q
            .query_map([], |r| {
                let mut row = Vec::new();
                for (i, name) in names.iter().enumerate() {
                    let v = match r.get_ref(i)? {
                        rusqlite::types::ValueRef::Null => Value::Null,
                        rusqlite::types::ValueRef::Integer(v) => v.into(),
                        rusqlite::types::ValueRef::Real(v) => v.into(),
                        rusqlite::types::ValueRef::Text(v) => String::from_utf8_lossy(v).into_owned().into(),
                        _ => Value::Null,
                    };
                    let v: Json = if name == "sources" {
                        match v {
                            Value::String(s) if !s.is_empty() => {
                                serde_json::from_str(&s).unwrap_or_else(|_| Json::from(Value::Null))
                            }
                            _ => Json::from(Value::Null),
                        }
                    } else {
                        v.into()
                    };
                    row.push((name.clone(), v));
                }
                Ok(Json::Object(row))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let state = if self.enabled && Self::available(library) {
            let mut state = self.state.lock().expect("sync state mutex poisoned").snapshot();
            if let Some(task) = &self.task {
                if let crate::app_tasks::TaskOutcome::Failed(message) = task.outcome() {
                    state.set("status", "error".into());
                    state.set("message", message.chars().take(300).collect::<String>().into());
                }
            }
            Json::Object(
                [
                    "status",
                    "started_at",
                    "scanned",
                    "scan_total",
                    "found",
                    "removed",
                    "imported",
                    "failed",
                    "total",
                    "done",
                    "message",
                    "finished_at",
                    "last_error",
                    "ms_per_image",
                    "stage",
                    "import_done",
                    "import_total",
                    "unreadable",
                    "damaged",
                    "problem",
                ]
                .iter()
                .filter_map(|k| state.get(k).map(|v| ((*k).to_string(), v.clone())))
                .collect(),
            )
        } else {
            Json::object([("status", "off".into())])
        };
        let mut fields = vec![("state", state), ("history", Json::Array(history))];
        if let Some(listing) = damaged::listing(&c)? {
            fields.push(("damaged", listing));
        }
        Ok(Json::object(fields))
    }
    pub fn start(&mut self, library: &Library, generation: u64, session: &str) -> Result<bool> {
        if !self.enabled || !Self::available(library) {
            return Err(Error::Invalid("本次启动关闭了同步（--no-sync）".into()));
        }
        if let Some(task) = self.task.as_mut() {
            if task.running() {
                if matches!(
                    self.state.lock().expect("sync state mutex poisoned").status(),
                    Status::Scanning | Status::Recognising | Status::Importing
                ) {
                    return Ok(false);
                }
                if !task.wait(std::time::Duration::from_secs(1)) {
                    return Ok(false);
                }
            }
        }
        self.task.take();
        let identity = crate::app_tasks::TaskIdentity::new(library, generation, session)?;
        self.state = Arc::new(Mutex::new(State::from(json!({"status":"scanning","started_at":now()}))));
        let (library, state) = (library.clone(), self.state.clone());
        let root = self.resource_root.clone();
        let per_run = self.per_run;
        self.task = Some(crate::app_tasks::Task::start_with(identity, deadline(), move |ctx| {
            let cancel = ctx.cancellation();
            let result = run_with(&library, cancel, Some(&ctx), &root, per_run, &|fields| {
                if ctx.check().is_err() {
                    return;
                }
                // Every state change comes from finished work (files checked, images recognised,
                // batches imported), so the task keeps running for as long as it moves.
                ctx.progressed();
                let apply = || -> Result<()> {
                    let mut s = state.lock().expect("sync state mutex poisoned");
                    s.merge(fields);
                    Ok(())
                };
                let _ = apply();
            });
            match result {
                Ok(_) => Ok(()),
                Err(e) => {
                    let mut s = state.lock().expect("sync state mutex poisoned");
                    s.set(
                        "status",
                        (if cancel.load(Ordering::SeqCst) || crate::platform_process::stopping() {
                            "interrupted"
                        } else {
                            "error"
                        })
                        .into(),
                    );
                    s.set("message", stop_message(cancel, Some(&ctx), &e).into());
                    s.set("finished_at", now().into());
                    Err(e)
                }
            }
        }));
        Ok(true)
    }
    pub fn configure_startup(&mut self, library: &Library, allowed: bool) -> Result<()> {
        let prefs: Value = serde_json::from_str(&crate::app_settings::startup(library)?.dump())
            .map_err(|e| Error::Invalid(e.to_string()))?;
        self.enabled = allowed && prefs["sync"] != false;
        Ok(())
    }
    pub fn automatic(&mut self, library: &Library, generation: u64, session: &str) -> Result<()> {
        let prefs: Value = serde_json::from_str(&crate::app_settings::startup(library)?.dump())
            .map_err(|e| Error::Invalid(e.to_string()))?;
        if self.enabled && Self::available(library) && prefs["sync"] == true {
            self.start(library, generation, session)?;
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn install_fixture_task(&mut self, task: crate::app_tasks::Task) {
        assert!(self.task.is_none());
        self.task = Some(task);
    }
    pub fn stop(&mut self) {
        if let Some(mut task) = self.task.take() {
            task.stop();
        }
    }
}
impl Drop for Syncer {
    fn drop(&mut self) {
        self.stop();
    }
}
/// Sync runs until it stops making progress, not for a fixed time from its start.
/// ILLUSTASH_RECOGNITION_TIMEOUT_MS (default 10 minutes) is the longest stretch without any
/// progress; ILLUSTASH_RECOGNITION_MAX_MS (default 24 hours) bounds the whole run.
fn deadline() -> crate::app_tasks::Deadline {
    let ms = |name: &str, default: u64| {
        std::time::Duration::from_millis(
            std::env::var(name).ok().and_then(|v| v.parse::<u64>().ok()).filter(|v| *v > 0).unwrap_or(default),
        )
    };
    crate::app_tasks::Deadline {
        idle: Some(ms("ILLUSTASH_RECOGNITION_TIMEOUT_MS", 600_000)),
        limit: Some(ms("ILLUSTASH_RECOGNITION_MAX_MS", 86_400_000)),
    }
}
/// A cancelled or timed-out run reports why it stopped, not the step it happened to interrupt.
fn stop_message(cancel: &AtomicBool, ctx: Option<&crate::app_tasks::TaskContext>, e: &Error) -> String {
    let stopped = if cancel.load(Ordering::SeqCst) { ctx.and_then(|c| c.stop_reason()) } else { None };
    stopped.unwrap_or_else(|| e.to_string()).chars().take(300).collect()
}
fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}
fn writer(path: &std::path::Path) -> Result<Connection> {
    let c = crate::sqlite::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    crate::configure(&c)?;
    c.busy_timeout(crate::WRITER_BUSY)?;
    Ok(c)
}
/// 可供 M4 复用：文件盘点后本地刷新；imported 事件与应用变更同事务提交，保留 pending 给 M4 消费。
pub(crate) fn run(library: &Library, cancel: &AtomicBool, update: &dyn Fn(Value)) -> Result<Value> {
    run_with(library, cancel, None, &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."), 0, update)
}
fn run_with(
    library: &Library,
    cancel: &AtomicBool,
    context: Option<&crate::app_tasks::TaskContext>,
    root: &std::path::Path,
    per_run: usize,
    update: &dyn Fn(Value),
) -> Result<Value> {
    if !Syncer::available(library) {
        return Err(Error::Invalid("原生同步没有支持的来源".into()));
    }
    let terminal = std::cell::RefCell::new(None);
    let publish = update;
    let update = |fields: Value| {
        if fields.get("finished_at").is_some() {
            terminal.replace(Some(fields));
        } else {
            publish(fields);
        }
    };
    let c = writer(&library.db())?;
    c.execute(
        "UPDATE sync_runs SET status='interrupted',finished_at=datetime('now','localtime') WHERE status='running'",
        [],
    )?;
    c.execute("INSERT INTO sync_runs(status,started_at) VALUES ('running',datetime('now','localtime'))", [])?;
    let run_id = c.last_insert_rowid();
    drop(c);
    let mut counts = json!({"found":0,"removed":0,"imported":0,"failed":0});
    // What the person has to do about a failure, as a code the page words (#202); the message stays the error's own.
    let problem = std::cell::Cell::new(None);
    // Where a failure happened and which database it was writing, for the log: the page shows the error alone (#275).
    let stage = std::cell::Cell::new("恢复识别导入（source.sqlite、library.sqlite）");
    let result: Result<(&str, String, Value)> = (|| {
        let recovery = crate::app_recognition::resume(library, cancel, &crate::app_research::dispatch)?;
        if recovery["status"] != "done" {
            return Err(Error::Invalid("识别导入恢复未完成".into()));
        }
        let mut sources: Vec<(String, Value)> = Vec::new();
        let mut scanned = 0;
        let mut unreadable = 0;
        for source in library.config()["sources"].as_array().into_iter().flatten() {
            let done_in_source = std::cell::Cell::new(0);
            let progress = |done, total| {
                done_in_source.set(done);
                update(json!({"scanned":scanned+done,"scan_total":scanned+total}))
            };
            stage.set("扫描来源（source.sqlite）");
            let (scan, order) = if source["kind"] == "folder" {
                let scan = crate::app_folder::scan(source, &library.source(), &|| check(cancel), &progress)?;
                let _ = scan.changed;
                let order = scan.counts["sources"]
                    .as_object()
                    .expect("folder scan source counts must be an object")
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>();
                (scan.counts, order)
            } else {
                crate::app_photos::scan_ordered(source, &library.source(), &|| check(cancel), &progress).inspect_err(
                    |e| {
                        if crate::platform_photos::denied(e) {
                            problem.set(Some("photos_permission"));
                        }
                    },
                )?
            };
            scanned += done_in_source.get();
            unreadable += scan["unreadable"].as_u64().unwrap_or(0);
            counts["found"] = json!(
                counts["found"].as_u64().expect("internal sync counter must be unsigned")
                    + scan["new"].as_u64().expect("internal sync counter must be unsigned")
                    + scan["returned"].as_u64().expect("internal sync counter must be unsigned")
            );
            for sid in &order {
                let entry = &scan["sources"][sid];
                if let Some((_, old)) = sources.iter_mut().find(|(id, _)| id == sid) {
                    for key in ["photos", "collected"] {
                        old[key] = json!(
                            old[key].as_u64().expect("internal sync counter must be unsigned")
                                + entry[key].as_u64().expect("internal sync counter must be unsigned")
                        );
                    }
                } else {
                    sources.push((sid.clone(), entry.clone()));
                }
            }
            // The persisted source hash, rather than this in-memory list, drives retry after app failure.
        }
        stage.set("记录扫描结果（library.sqlite）");
        let c = writer(&library.db())?;
        c.execute(
            "UPDATE sync_runs SET found=?,sources=? WHERE id=?",
            params![
                counts["found"].as_u64(),
                crate::http_json::Json::Object(
                    sources
                        .iter()
                        .map(|(sid, v)| (
                            sid.clone(),
                            crate::http_json::Json::object([
                                ("name", v["name"].clone().into()),
                                ("photos", v["photos"].clone().into()),
                                ("collected", v["collected"].clone().into())
                            ])
                        ))
                        .collect()
                )
                .dump(),
                run_id
            ],
        )?;
        drop(c);
        check(cancel)?;
        update(json!({"status":"importing"}));
        stage.set("刷新图库（library.sqlite）");
        let (out, refreshed) = refresh(library, run_id, cancel)?;
        counts["imported"] = out["added"].clone();
        counts["removed"] = out["removed"].clone();
        let pending = out["pending"].as_u64().expect("internal sync counter must be unsigned");
        if pending == 0 {
            // Every picture is recognised or gone: nothing is known damaged any more.
            damaged::save(library, &damaged::load(library)?, &serde_json::Map::new())?;
        }
        if pending > 0 {
            if let Some(ctx) = context {
                let worker =
                    std::env::var_os("ILLUSTASH_RECOGNITION_WORKER").filter(|_| !crate::app_models::waiting(root));
                if let Some(worker) = worker {
                    stage.set("识别与导入（source.sqlite、library.sqlite）");
                    recognise(
                        library,
                        ctx,
                        &worker,
                        RecognitionRun { root, per_run, already_counted: &refreshed, run_id },
                        &mut counts,
                        &update,
                    )?;
                    let damaged = counts["damaged"].as_u64().unwrap_or(0);
                    let message =
                        with_unreadable(unreadable, failure_message(counts["failed"].as_u64().unwrap_or(0), damaged));
                    let mut fields = json!({"status":"done","message":message,"finished_at":now(),"found":counts["found"],"removed":counts["removed"],"imported":counts["imported"],"failed":counts["failed"]});
                    if unreadable > 0 {
                        fields["unreadable"] = json!(unreadable);
                    }
                    if damaged > 0 {
                        fields["damaged"] = json!(damaged);
                    }
                    update(fields);
                    return Ok(("done", message, out));
                }
            }
        }
        let status = if pending > 0 { "needs_recognition" } else { "done" };
        let message = with_unreadable(
            unreadable,
            if pending > 0 {
                if crate::app_models::waiting(root) {
                    format!("有 {pending} 张图等待识别；{why}", why = crate::app_models::MISSING)
                } else {
                    format!("有 {pending} 张图等待识别；本次启动没有识别 worker")
                }
            } else {
                String::new()
            },
        );
        let mut fields = json!({"status":status,"total":pending,"done":0,"message":message,"finished_at":now(),"found":counts["found"],"removed":counts["removed"],"imported":counts["imported"],"failed":0});
        if unreadable > 0 {
            fields["unreadable"] = json!(unreadable);
        }
        update(fields);
        Ok((status, message, out))
    })();
    let c = writer(&library.db())?;
    let (status, message) = match &result {
        Ok((s, m, _)) => (*s, m.clone()),
        Err(e) => (
            if cancel.load(Ordering::SeqCst) || crate::platform_process::stopping() { "interrupted" } else { "error" },
            stop_message(cancel, context, e),
        ),
    };
    c.execute("UPDATE sync_runs SET status=?,message=?,found=?,removed=?,imported=?,failed=?,finished_at=datetime('now','localtime') WHERE id=?",params![status,message,counts["found"].as_u64(),counts["removed"].as_u64(),counts["imported"].as_u64(),counts["failed"].as_u64(),run_id])?;
    if let Err(e) = &result {
        eprintln!("同步失败，{}：{e:?}", stage.get());
        let mut fields = json!({"found":counts["found"],"removed":counts["removed"],"imported":counts["imported"],"failed":counts["failed"],"message":stop_message(cancel, context, e)});
        if let Some(problem) = problem.get() {
            fields["problem"] = json!(problem);
        }
        update(fields);
    }
    if let Some(fields) = terminal.into_inner() {
        publish(fields);
    }
    result.map(|(_, _, out)| out)
}
fn refresh(library: &Library, run_id: i64, cancel: &AtomicBool) -> Result<(Value, HashSet<String>)> {
    let mut c = writer(&library.db())?;
    let mut src = crate::app_library::read_connection(&library.source())?;
    let src = src.transaction()?;
    let live=crate::app_migrate_data::rows(&src,"SELECT uuid,filename,src_path,width,height,added_at,src_kind FROM assets WHERE in_scope=1 AND coalesce(trashed,0)=0")?;
    let known =
        crate::app_migrate_data::rows(&c, "SELECT uuid,filename,src_path,width,height,added_at,src_kind FROM assets")?;
    let have = known
        .iter()
        .map(|r| r[0].as_str().expect("SQLite uuid column must be text").to_owned())
        .collect::<HashSet<_>>();
    let present =
        live.iter().map(|r| r[0].as_str().expect("SQLite uuid column must be text").to_owned()).collect::<HashSet<_>>();
    let removed = known
        .iter()
        .filter(|r| !present.contains(r[0].as_str().expect("SQLite uuid column must be text")))
        .map(|r| r[0].clone())
        .collect::<Vec<_>>();
    // Source labels are authoritative for machine freshness. Existing content metadata is persisted in meta
    // so an application failure after source commit is retried even when the next scan sees unchanged files.
    let journal_path = library.source().with_extension("scan-pending.json");
    let journal: serde_json::Map<String, Value> = if journal_path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&journal_path)?).map_err(|e| Error::Invalid(e.to_string()))?
    } else {
        serde_json::Map::new()
    };
    let mut changed = Vec::new();
    let mut hashes = Vec::new();
    let mut labels_changed = HashSet::new();
    let metadata = c
        .prepare("SELECT key,value FROM meta WHERE key LIKE 'folder_hash:%' OR key LIKE 'folder_labels:%'")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;
    let source_hashes = src
        .prepare("SELECT uuid,stable_hash FROM assets")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;

    for row in &live {
        let u = row[0].as_str().expect("SQLite uuid column must be text");
        let hash = source_hashes.get(u).cloned().unwrap_or(None);
        let label_rows =
            crate::app_migrate_data::rows_with(&src, "SELECT * FROM labels WHERE uuid=? ORDER BY dim,value,conf", [u])?;
        let label_hash = crate::strict_json::sha256_hex(
            serde_json::to_string(&label_rows).expect("SQLite rows contain serializable JSON").as_bytes(),
        );
        let label_key = format!("folder_labels:{u}");
        let old_labels = metadata.get(&label_key);
        if have.contains(u) && old_labels.map(String::as_str) != Some(label_hash.as_str()) && !label_rows.is_empty() {
            labels_changed.insert(u.to_string());
        }
        hashes.push((label_key, label_hash));
        let key = format!("folder_hash:{u}");
        let old = metadata.get(&key);
        if have.contains(u)
            && ((old.is_some() && old.map(String::as_str) != hash.as_deref())
                || (old.is_none() && journal.get(u).and_then(Value::as_str) == hash.as_deref()))
        {
            changed.push(u.to_owned());
        }
        if let Some(hash) = hash {
            hashes.push((key, hash));
        }
    }
    let pending:i64=src.query_row("SELECT count(*) FROM assets a WHERE in_scope=1 AND coalesce(trashed,0)=0 AND NOT EXISTS(SELECT 1 FROM labels l WHERE l.uuid=a.uuid AND l.dim='rating')",[],|r|r.get(0))?;
    // Photos keeps the established ready-only application import. Folder inventory remains
    // browsable before recognition, as delivered by M3; both sources share the same backlog.
    let photos_pending=src.prepare("SELECT a.uuid FROM assets a WHERE EXISTS(SELECT 1 FROM asset_links l WHERE l.asset=a.uuid AND l.system='photos' AND l.source_id=a.source_id) AND NOT EXISTS(SELECT 1 FROM labels l WHERE l.uuid=a.uuid AND l.dim='rating')")?.query_map([],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<HashSet<_>>>()?;
    let added = live
        .iter()
        .filter(|r| !photos_pending.contains(r[0].as_str().expect("SQLite uuid column must be text")))
        .filter(|r| {
            !have.contains(r[0].as_str().expect("SQLite uuid column must be text"))
                || labels_changed.contains(r[0].as_str().expect("SQLite uuid column must be text"))
                || changed.iter().any(|u| Some(u.as_str()) == r[0].as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    let uuids =
        added.iter().map(|r| r[0].as_str().expect("SQLite uuid column must be text").to_owned()).collect::<Vec<_>>();
    check(cancel)?;
    // WD delivery uses the established native evidence-package path; source snapshot remains read-only.
    crate::app_migrate::deliver_wd(&mut c, &library.source(), library.id(), Some(&uuids), true)?;
    let list = serde_json::to_string(&uuids).expect("UUID strings are serializable");
    let labels=crate::app_migrate_data::rows(&src,&format!("SELECT uuid,value,conf,dim FROM labels WHERE ((dim='character' AND conf>=0.35) OR dim='char_ext') AND uuid IN (SELECT value FROM json_each('{}'))",list.replace('\'',"''")))?;
    let decisions = crate::app_migrate_data::rows(
        &src,
        &format!(
            "SELECT uuid,value FROM decisions WHERE dim='character' AND uuid IN (SELECT value FROM json_each('{}'))",
            list.replace('\'', "''")
        ),
    )?;
    let realistic = crate::app_migrate_data::rows(
        &src,
        "SELECT uuid FROM labels WHERE dim='general' AND value='realistic' AND conf>=0.15",
    )?
    .iter()
    .map(|r| r[0].clone())
    .collect::<Vec<_>>();
    let album = library.source().parent().expect("source database path must have a parent").join("album-char-map.json");
    let album = if album.is_file() {
        serde_json::from_str::<Value>(&std::fs::read_to_string(album)?).map_err(|e| Error::Invalid(e.to_string()))?
            ["map"]
            .clone()
    } else {
        json!({})
    };
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    crate::schema::begin_writing(&tx)?;
    let mut stale_ids = Vec::new();
    for u in &changed {
        let ids=tx.prepare("SELECT e.id,e.asset_id FROM term_evidence e JOIN term_assets a ON a.id=e.asset_id JOIN terms t ON t.id=e.term_id WHERE a.namespace='library' AND a.key=? AND e.source='wd' AND t.kind IN ('topic','rating')")?.query_map([u],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, asset) in ids {
            tx.execute(
                "INSERT OR REPLACE INTO meta VALUES (?,?)",
                params![format!("folder_stale_wd:{id}"), asset.to_string()],
            )?;
            stale_ids.push(asset);
        }
    }
    stale_ids.sort_unstable();
    stale_ids.dedup();
    crate::store::project(&tx, &stale_ids)?;
    for r in &live {
        tx.execute(
            "UPDATE assets SET filename=?,src_path=? WHERE uuid=?",
            params![r[1].as_str(), r[2].as_str(), r[0].as_str()],
        )?;
    }
    let shape = known
        .iter()
        .filter(|r| !removed.contains(&r[0]) && !uuids.iter().any(|u| Some(u.as_str()) == r[0].as_str()))
        .cloned()
        .chain(added.iter().cloned())
        .collect::<Vec<_>>();
    let stamp = now();
    let args = json!({"op":"import","rows":added,"removed":removed,"run_id":run_id,"new_ids":{},"wd_tags":crate::app_migrate_data::rows(&src,"SELECT char_id,wd_tag FROM characters")?,"labels":labels,"decisions":decisions,"album_map":album,"duplicates":crate::app_migrate_data::duplicates(&tx,&src,&shape)?,"realistic":realistic,"indexed":stamp});
    let out = if !uuids.is_empty() || !removed.is_empty() {
        let out = crate::character_commands::command(&tx, &args)?;
        tx.execute("INSERT INTO backend_effects(kind) VALUES ('imported')", [])?;
        out
    } else {
        json!({"added":0,"removed":0})
    };
    // Changed pictures were imported above (an 'imported' event follows), so similar recomputes them.
    crate::app_research::forget_content(&tx, &src, &changed)?;
    for (key, hash) in hashes {
        tx.execute("INSERT INTO meta(key,value) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value<>excluded.value", params![key, hash])?;
    }
    check(cancel)?;
    crate::schema::commit_writing(tx)?;
    // App hash markers make retry idempotent even if removing the committed journal fails.
    if journal_path.exists() {
        std::fs::remove_file(&journal_path)?;
    }
    Ok((json!({"added":out["added"],"removed":out["removed"],"pending":pending}), uuids.into_iter().collect()))
}
/// Pictures given up on in this run, by what helps: wait and sync again, or fix or replace the file.
fn failure_message(failed: u64, damaged: u64) -> String {
    let mut parts = Vec::new();
    if failed > 0 {
        parts.push(format!("{n} 张暂时读不到（可能还在 iCloud），下次同步重试", n = failed));
    }
    if damaged > 0 {
        parts.push(format!("{n} 张图片已损坏或格式不支持，留在图库里但不识别；文件改动后会重新识别", n = damaged));
    }
    parts.join("；")
}
/// Folder paths the scan could not read keep their pictures as they were; say so, so the user can retry.
fn with_unreadable(unreadable: u64, message: String) -> String {
    if unreadable == 0 {
        return message;
    }
    let note = format!("{n} 处文件或文件夹暂时读不到，其中已有的图片保留不动；恢复访问后再同步", n = unreadable);
    if message.is_empty() {
        note
    } else {
        format!("{note}；{message}")
    }
}
/// Reads the recognition worker's progress file while the host waits for it. Only a value for
/// this task, within this request's size and moving forward is accepted; anything else is ignored,
/// so a stale or malformed file can neither fake progress nor fail the task.
struct ProgressWatch {
    task_id: String,
    total: usize,
    read_at: Option<std::time::Instant>,
    processed: Option<usize>,
    stage: String,
    /// When the worker reported its first recognised image, for the time estimate.
    started: Option<(std::time::Instant, usize)>,
}
impl ProgressWatch {
    fn new(task_id: &str, total: usize) -> Self {
        Self { task_id: task_id.to_owned(), total, read_at: None, processed: None, stage: String::new(), started: None }
    }
    /// The validated (stage, processed, failed) in `bytes`, if it is newer than what was shown.
    fn accept(&mut self, bytes: &[u8]) -> Option<(String, usize, usize)> {
        let v: Value = serde_json::from_slice(bytes).ok()?;
        let count = |k: &str| v[k].as_u64().and_then(|n| usize::try_from(n).ok());
        let (stage, processed, failed) = (v["stage"].as_str()?, count("processed")?, count("failed")?);
        let valid = v["format"] == "illustash-recognition-progress/1"
            && v["task_id"].as_str() == Some(self.task_id.as_str())
            && matches!(stage, "preparing" | "recognising")
            && count("total") == Some(self.total)
            && processed <= self.total
            && failed <= processed;
        let backwards = self.processed.is_some_and(|p| processed < p)
            || (stage == "preparing" && (processed > 0 || self.stage == "recognising"));
        if !valid || backwards {
            return None;
        }
        if self.processed == Some(processed) && self.stage == stage {
            return None;
        }
        (self.processed, self.stage) = (Some(processed), stage.to_owned());
        Some((stage.to_owned(), processed, failed))
    }
    /// State fields for the sync task, at most every 200 ms.
    fn poll(&mut self, path: &std::path::Path, unreadable: usize) -> Option<Value> {
        let now = std::time::Instant::now();
        if self.read_at.is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(200)) {
            return None;
        }
        self.read_at = Some(now);
        if !std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= 4096) {
            return None;
        }
        let (stage, processed, failed) = self.accept(&std::fs::read(path).ok()?)?;
        let mut fields = json!({"status":"recognising","stage":stage,"done":unreadable + processed,
            "failed":unreadable + failed});
        if stage == "recognising" {
            let (since, from) = *self.started.get_or_insert((now, processed));
            if processed > from {
                fields["ms_per_image"] =
                    json!(now.duration_since(since).as_millis() as u64 / (processed - from) as u64);
            }
        }
        Some(fields)
    }
}
struct RecognitionRun<'a> {
    root: &'a std::path::Path,
    per_run: usize,
    already_counted: &'a HashSet<String>,
    run_id: i64,
}
fn recognise(
    library: &Library,
    ctx: &crate::app_tasks::TaskContext,
    worker: &std::ffi::OsStr,
    options: RecognitionRun<'_>,
    counts: &mut Value,
    update: &dyn Fn(Value),
) -> Result<()> {
    use std::{fs, process::Command};
    let RecognitionRun { root, per_run, already_counted, run_id } = options;
    let c = crate::app_library::read_connection(&library.source())?;
    let mut assets=crate::app_migrate_data::rows(&c,"SELECT uuid,src_path,stable_hash FROM assets a WHERE in_scope=1 AND coalesce(trashed,0)=0 AND NOT EXISTS(SELECT 1 FROM labels l WHERE l.uuid=a.uuid AND l.dim='rating') ORDER BY uuid")?;
    drop(c);
    // Pictures that could be read but not decoded wait, unchanged, until their file changes (#112);
    // they never take a place within per_run.
    let marks = damaged::load(library)?;
    let (mut kept, mut held) = (serde_json::Map::new(), serde_json::Map::new());
    let mut located = HashMap::new();
    assets.retain(|row| {
        let id = row[0].as_str().expect("SQLite uuid column must be text");
        let path = row[1].as_str().unwrap_or("");
        let hash = row[2].as_str();
        located.insert(id.to_owned(), (path.to_owned(), hash.map(str::to_owned)));
        let Some(mark) = marks.get(id) else { return true };
        match damaged::fingerprint(path, hash) {
            Some(now) if &now == mark => {
                kept.insert(id.to_owned(), now);
                false
            }
            Some(_) => true,
            // Not readable right now: it is retried as such, and still known damaged if it comes back unchanged.
            None => {
                held.insert(id.to_owned(), mark.clone());
                true
            }
        }
    });
    let skipped = kept.len();
    kept.extend(held);
    damaged::save(library, &marks, &kept)?;
    counts["damaged"] = json!(skipped);
    if per_run > 0 {
        assets.truncate(per_run);
    }
    let total = assets.len();
    let mut readable = Vec::new();
    let mut failed = HashSet::new();
    for row in assets {
        ctx.check()?;
        let id = row[0].as_str().expect("SQLite uuid column must be text").to_owned();
        if std::fs::File::open(row[1].as_str().unwrap_or("")).is_err() {
            failed.insert(id);
        } else {
            readable.push(protocol::SyncAsset {
                id,
                path: row[1]
                    .as_str()
                    .ok_or_else(|| Error::Invalid("source.sqlite: assets.src_path 必须为字符串".into()))?
                    .to_owned(),
            });
        }
    }
    update(json!({"status":"recognising","stage":"preparing","total":total,"done":failed.len(),"failed":failed.len()}));
    counts["failed"] = json!(failed.len());
    if readable.is_empty() {
        return Ok(());
    }
    let folder = std::env::temp_dir().join(format!("illustash-sync-{}", ctx.identity().task_id));
    crate::platform_filesystem::create_private_dir(&folder)?;
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(folder.clone());
    let request = folder.join("request.json");
    let response = folder.join("response.json");
    fs::write(
        &request,
        serde_json::to_vec(&protocol::value(&protocol::SyncRequest {
            format: protocol::Format::Sync,
            identity: ctx.identity(),
            assets: &readable,
            output: &folder,
        })?)
        .map_err(|e| Error::Invalid(e.to_string()))?,
    )?;
    let mut command = Command::new(worker);
    command.env(crate::app_tasks::WORKER_KIND_ENV, "recognition").arg(&request).arg(&response);
    command.env("ILLUSTASH_ROOT", crate::app_paths::resolve(root)?);
    command.env("ILLUSTASH_SHARED", crate::app_resources::shared_path(root)?);
    // Inside the task folder, so it is cleaned up with the batches. Workers that do not write it
    // leave the count where it is until import, as before.
    let progress = folder.join("progress.json");
    command.env("ILLUSTASH_RECOGNITION_PROGRESS", &progress);
    let mut watch = ProgressWatch::new(&ctx.identity().task_id, readable.len());
    let unreadable = failed.len();
    // Only the platform-accepted shipped native route mandates sidecars. The
    // installed Python transition keeps its existing response contract. Explicit
    // sidecar declarations remain fully validated on every platform below.
    let native_adapter = crate::recognition_compute::production_adapter_enabled()
        && std::path::Path::new(worker).file_stem().is_some_and(|name| name == "illustash-recognition");
    let mut batches = Batches {
        library,
        require_claims: native_adapter,
        ctx,
        root,
        folder: crate::app_paths::canonical(&folder)?,
        expected: readable.iter().map(|r| r.id.clone()).collect(),
        seen: HashSet::new(),
        streamed: HashSet::new(),
        inputs: None,
        already_counted,
        run_id,
        import_done: 0,
        scanned_at: None,
    };
    // Import each finished batch while the worker computes the next, so an interruption keeps
    // every batch already imported; the next sync then recognises only what is left.
    let status = ctx.process_observed(&mut command, || {
        if let Some(fields) = watch.poll(&progress, unreadable) {
            update(fields);
        }
        batches.stream(counts, update)
    })?;
    if !status.success() {
        return Err(Error::Invalid(format!("识别进程异常退出：{status}")));
    }
    let metadata = fs::symlink_metadata(&response)?;
    if !metadata.is_file() || metadata.len() > 4 * 1024 * 1024 {
        return Err(Error::Invalid("识别响应不合法".into()));
    }
    let reply: protocol::SyncResponse = protocol::read(&response)?;
    if &reply.identity != ctx.identity() {
        return Err(Error::Conflict("任务结果已过期或重复".into()));
    }
    if native_adapter && reply.character_claims.is_none() {
        return Err(Error::Invalid("原生识别响应缺少角色 sidecar 声明".into()));
    }
    if reply.character_claims.as_ref().is_some_and(|v| v.len() != reply.packages.len()) {
        return Err(Error::Invalid("识别角色 sidecar 数量与批次不匹配".into()));
    }
    let mut packages = Vec::new();
    let mut listed = HashSet::new();
    for (index, row) in reply.packages.iter().enumerate() {
        let path = crate::app_paths::canonical(row)?;
        if !path.starts_with(&batches.folder) {
            return Err(Error::Invalid("结果包必须位于本任务输出目录".into()));
        }
        let ids = manifest_assets(&path)?;
        if !listed.insert(path.clone()) {
            return Err(Error::Conflict("结果超出本次识别范围或重复".into()));
        }
        if batches.streamed.contains(&path) {
            // Imported while the worker ran; its assets are already counted as seen.
            continue;
        }
        batches.claim(&ids)?;
        let claims = if let Some(paths) = &reply.character_claims {
            let path = crate::app_paths::canonical(&paths[index])?;
            if !path.starts_with(&batches.folder) {
                return Err(Error::Invalid("角色 sidecar 必须位于本任务输出目录".into()));
            }
            Some(path)
        } else {
            None
        };
        packages.push((path, ids, claims));
    }
    if !batches.streamed.is_subset(&listed) {
        return Err(Error::Invalid("识别响应缺少已完成的批次".into()));
    }
    let (mut marked, mut found) = (kept.clone(), 0);
    for u in &reply.failed {
        let id = &u.uuid;
        if !batches.expected.contains(id) || !batches.seen.insert(id.to_owned()) {
            return Err(Error::Conflict("失败结果超出本次范围或重复".into()));
        }
        let (path, hash) = &located[id];
        // A file that cannot even be looked at now is counted as unreadable and retried.
        match (u.reason, damaged::fingerprint(path, hash.as_deref())) {
            (protocol::FailureReason::Undecodable, Some(now)) => {
                marked.insert(id.to_owned(), now);
                found += 1;
            }
            _ => {
                failed.insert(id.to_owned());
            }
        }
    }
    if batches.seen != batches.expected {
        return Err(Error::Invalid("识别结果不完整".into()));
    }
    damaged::save(library, &kept, &marked)?;
    counts["damaged"] = json!(skipped + found);
    counts["failed"] = json!(failed.len());
    // Recognition is over: the reply's counts are final. Import has its own count, so the bar never
    // moves back from recognised to imported, and nothing recognised is shown as imported.
    let import_total = batches.import_done + packages.iter().map(|(_, ids, _)| ids.len()).sum::<usize>();
    update(json!({"status":"importing","stage":"verifying","done":total,"total":total,"failed":failed.len(),
        "import_done":batches.import_done,"import_total":import_total}));
    // The batches not yet imported: each is checked and imported on its own, as a streamed one.
    for (package, ids, claims) in packages {
        batches.import(&package, &ids, claims.as_deref(), counts, update)?;
    }
    update(
        json!({"status":"recognising","done":total,"total":total,"imported":counts["imported"],"failed":counts["failed"]}),
    );
    Ok(())
}

/// The recognition batches of one task: which assets they cover and which are already imported.
struct Batches<'a> {
    library: &'a Library,
    /// The native route must deliver character claims with every batch.
    require_claims: bool,
    ctx: &'a crate::app_tasks::TaskContext,
    root: &'a std::path::Path,
    folder: std::path::PathBuf,
    expected: HashSet<String>,
    seen: HashSet<String>,
    /// Batches imported while the worker still ran.
    streamed: HashSet<std::path::PathBuf>,
    /// Models and anchor snapshot, loaded once, at the first batch that has character claims.
    inputs: Option<crate::app_recognition::WorkerInputs>,
    already_counted: &'a HashSet<String>,
    run_id: i64,
    import_done: usize,
    scanned_at: Option<std::time::Instant>,
}
impl Batches<'_> {
    /// Each asset belongs to this run and to exactly one batch.
    fn claim(&mut self, ids: &[String]) -> Result<()> {
        for id in ids {
            if !self.expected.contains(id) || !self.seen.insert(id.to_owned()) {
                return Err(Error::Conflict("结果超出本次识别范围或重复".into()));
            }
        }
        Ok(())
    }
    /// Import the batches the worker has marked complete since the last look, at most every 500 ms.
    fn stream(&mut self, counts: &mut Value, update: &dyn Fn(Value)) -> Result<()> {
        let now = std::time::Instant::now();
        if self.scanned_at.is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(500)) {
            return Ok(());
        }
        self.scanned_at = Some(now);
        let mut ready = Vec::new();
        for entry in std::fs::read_dir(&self.folder)? {
            let path = entry?.path();
            let dir = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
            if dir && !self.streamed.contains(&path) && path.join("complete.json").is_file() {
                ready.push(path);
            }
        }
        ready.sort();
        for batch in ready {
            let (ids, claims) = self.complete(&batch)?;
            self.claim(&ids)?;
            self.streamed.insert(batch.clone());
            self.import(&batch, &ids, claims.as_deref(), counts, update)?;
        }
        Ok(())
    }
    /// The assets and claims sidecar of a batch the worker has finished, checked against its
    /// completion marker.
    fn complete(&self, batch: &std::path::Path) -> Result<(Vec<String>, Option<std::path::PathBuf>)> {
        let bad = || Error::Invalid("识别批次的完成标记不合法".into());
        let marker = batch.join("complete.json");
        if !std::fs::symlink_metadata(&marker)?.is_file() || std::fs::metadata(&marker)?.len() > 1024 * 1024 {
            return Err(bad());
        }
        let v: Value = serde_json::from_slice(&std::fs::read(&marker)?).map_err(|_| bad())?;
        let digest =
            |path: std::path::PathBuf| -> Result<String> { Ok(crate::strict_json::sha256_hex(&std::fs::read(path)?)) };
        let ids: Vec<String> = serde_json::from_value(v["assets"].clone()).map_err(|_| bad())?;
        let sidecar = batch.join("character-claims.json");
        let claims = match v["sidecar_sha256"].as_str() {
            Some(expected) if digest(sidecar.clone())? == expected => Some(sidecar),
            None if v["sidecar_sha256"].is_null() && !self.require_claims => None,
            _ => return Err(bad()),
        };
        if v["format"] != "illustash-recognition-batch/1"
            || v["task_id"].as_str() != Some(self.ctx.identity().task_id.as_str())
            || v["manifest_sha256"].as_str() != Some(digest(batch.join("manifest.json"))?.as_str())
            || ids.is_empty()
            || manifest_assets(batch)? != ids
        {
            return Err(bad());
        }
        Ok((ids, claims))
    }
    /// Check and import one batch, in its own source transaction, under this task's receipt.
    fn import(
        &mut self,
        package: &std::path::Path,
        ids: &[String],
        claims: Option<&std::path::Path>,
        counts: &mut Value,
        update: &dyn Fn(Value),
    ) -> Result<()> {
        let (library, ctx) = (self.library, self.ctx);
        ctx.check()?;
        let prepared = match claims {
            Some(claims) => {
                if self.inputs.is_none() {
                    self.inputs = Some(crate::app_recognition::WorkerInputs::load(
                        library,
                        ctx.identity(),
                        self.root,
                        ctx.cancellation(),
                    )?);
                }
                let inputs = self.inputs.as_ref().expect("claims task snapshot");
                Some(crate::app_recognition::prepare_worker_package(
                    library,
                    package,
                    claims,
                    inputs,
                    ctx.cancellation(),
                )?)
            }
            None => None,
        };
        let imported = ctx.receive_part(ctx.identity(), || match &prepared {
            Some(prepared) => crate::app_recognition::import_worker_package(
                library,
                prepared,
                self.inputs.as_ref().expect("claims task snapshot"),
                ctx.cancellation(),
                &crate::app_research::dispatch,
            ),
            None => crate::app_recognition::import_package(
                library,
                package,
                self.root,
                ctx.cancellation(),
                &crate::app_research::dispatch,
            ),
        })?;
        if imported["status"] != "done" {
            return Err(Error::Invalid("识别导入待恢复".into()));
        }
        counts["imported"] = json!(
            counts["imported"].as_u64().expect("internal sync counter must be unsigned")
                + ids.iter().filter(|u| !self.already_counted.contains(*u)).count() as u64
        );
        remember_labels(library, ids, ctx.cancellation())?;
        self.import_done += ids.len();
        writer(&library.db())?.execute(
            "UPDATE sync_runs SET imported=?,failed=? WHERE id=?",
            params![counts["imported"].as_u64(), counts["failed"].as_u64(), self.run_id],
        )?;
        update(json!({"import_done":self.import_done,"imported":counts["imported"],"failed":counts["failed"]}));
        Ok(())
    }
}
/// The asset list of a result package's manifest.
fn manifest_assets(path: &std::path::Path) -> Result<Vec<String>> {
    let manifest = path.join("manifest.json");
    if std::fs::metadata(&manifest)?.len() > 4 * 1024 * 1024 {
        return Err(Error::Invalid("结果清单过大".into()));
    }
    let m = std::fs::read(&manifest)?;
    if m.len() > 4 * 1024 * 1024 {
        return Err(Error::Invalid("结果清单过大".into()));
    }
    let m: protocol::RecognitionManifest = protocol::parse(
        serde_json::from_slice(&m).map_err(|e| Error::Invalid(format!("{}: {e}", manifest.display())))?,
        &manifest,
    )?;
    Ok(m.assets)
}

fn remember_labels(library: &Library, ids: &[String], cancel: &AtomicBool) -> Result<()> {
    let mut app = writer(&library.db())?;
    let src = crate::app_library::read_connection(&library.source())?;
    let tx = app.transaction()?;
    for id in ids {
        let labels = crate::app_migrate_data::rows_with(
            &src,
            "SELECT * FROM labels WHERE uuid=? ORDER BY dim,value,conf",
            [id],
        )?;
        let hash = crate::strict_json::sha256_hex(
            serde_json::to_string(&labels).expect("label values are serializable JSON").as_bytes(),
        );
        tx.execute("INSERT OR REPLACE INTO meta VALUES (?,?)", params![format!("folder_labels:{id}"), hash])?;
    }
    check(cancel)?;
    tx.commit()?;
    Ok(())
}

/// Pictures whose file could be read but not decoded (#112). Each is kept with what its file looked
/// like then: size, modification time and the scan's content hash (#27). While all three stay the
/// same the picture is skipped; any change, such as the file being replaced, makes the next sync
/// try it again. Stored as one preference, which no research input reads, so no schema change.
mod damaged {
    use super::*;
    const KEY: &str = "sync_damaged";
    pub(super) type Marks = serde_json::Map<String, Value>;
    pub(super) fn fingerprint(path: &str, hash: Option<&str>) -> Option<Value> {
        let meta = std::fs::metadata(path).ok().filter(std::fs::Metadata::is_file)?;
        let mtime = match meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH) {
            Ok(v) => i64::try_from(v.as_nanos()).ok()?,
            Err(e) => -i64::try_from(e.duration().as_nanos()).ok()?,
        };
        Some(json!({"size":meta.len(),"mtime_ns":mtime,"hash":hash}))
    }
    fn read(c: &Connection) -> Result<Marks> {
        let value: Option<String> =
            c.query_row("SELECT value FROM prefs WHERE key=?", [KEY], |r| r.get(0)).optional()?;
        // Unreadable marks only cost one more attempt at each picture.
        Ok(value.and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default())
    }
    pub(super) fn load(library: &Library) -> Result<Marks> {
        read(&crate::app_library::read_connection(&library.db())?)
    }
    pub(super) fn save(library: &Library, before: &Marks, after: &Marks) -> Result<()> {
        if before == after {
            return Ok(());
        }
        let c = writer(&library.db())?;
        if after.is_empty() {
            c.execute("DELETE FROM prefs WHERE key=?", [KEY])?;
        } else {
            let value = serde_json::to_string(after).expect("internal JSON value is serializable");
            c.execute("INSERT OR REPLACE INTO prefs(key,value) VALUES (?,?)", params![KEY, value])?;
        }
        Ok(())
    }
    /// How many pictures are known damaged and where the first of them are, for the sync status.
    pub(super) fn listing(c: &Connection) -> Result<Option<crate::http_json::Json>> {
        use crate::http_json::Json;
        let marks = read(c)?;
        if marks.is_empty() {
            return Ok(None);
        }
        let ids = serde_json::to_string(&marks.keys().collect::<Vec<_>>()).expect("UUID strings are serializable");
        let count: i64 =
            c.query_row("SELECT count(*) FROM assets WHERE uuid IN (SELECT value FROM json_each(?))", [&ids], |r| {
                r.get(0)
            })?;
        if count == 0 {
            return Ok(None);
        }
        let paths = c
            .prepare(
                "SELECT src_path FROM assets WHERE uuid IN (SELECT value FROM json_each(?)) ORDER BY src_path LIMIT ?",
            )?
            .query_map(params![ids, LISTED], |r| r.get::<_, Option<String>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Some(Json::object([
            ("count", count.into()),
            ("paths", Json::Array(paths.into_iter().map(|p| p.unwrap_or_default().into()).collect())),
        ])))
    }
    /// The status shows this many locations; the rest are counted.
    const LISTED: i64 = 50;
    use rusqlite::OptionalExtension;
}

/// Existing host cancellation predicate; read adapters receive it at their original checkpoints.
pub(crate) fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) || crate::platform_process::stopping() {
        Err(Error::Conflict("扫描已中断".into()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unreadable_and_damaged_pictures_are_reported_apart() {
        assert_eq!(failure_message(0, 0), "");
        assert_eq!(failure_message(2, 0), "2 张暂时读不到（可能还在 iCloud），下次同步重试");
        assert_eq!(failure_message(0, 1), "1 张图片已损坏或格式不支持，留在图库里但不识别；文件改动后会重新识别");
        assert_eq!(
            failure_message(2, 1),
            "2 张暂时读不到（可能还在 iCloud），下次同步重试；1 张图片已损坏或格式不支持，留在图库里但不识别；文件改动后会重新识别"
        );
    }
    #[test]
    fn damaged_fingerprint_follows_size_time_and_content_hash() {
        let root = std::env::temp_dir().join(format!("illustash-damaged-{}", crate::store::new_key().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let file = root.join("bad.png");
        std::fs::write(&file, b"\x89PNG truncated").unwrap();
        let path = file.to_str().unwrap();
        let first = damaged::fingerprint(path, Some("h1")).unwrap();
        assert_eq!(damaged::fingerprint(path, Some("h1")), Some(first.clone()));
        assert_ne!(damaged::fingerprint(path, Some("h2")), Some(first.clone()), "content hash");
        std::fs::write(&file, b"\x89PNG truncated, longer").unwrap();
        assert_ne!(damaged::fingerprint(path, Some("h1")), Some(first.clone()), "size");
        assert_eq!(damaged::fingerprint(root.to_str().unwrap(), None), None, "not a file");
        assert_eq!(damaged::fingerprint(root.join("missing").to_str().unwrap(), None), None);
        std::fs::remove_dir_all(root).unwrap();
    }
    use crate::app_service::{Application, Options};
    use std::{collections::BTreeMap, path::Path, sync::mpsc, time::Duration};

    fn progress(task: &str, stage: &str, total: usize, processed: usize, failed: usize) -> Vec<u8> {
        json!({"format":"illustash-recognition-progress/1","task_id":task,"stage":stage,"total":total,
            "processed":processed,"failed":failed})
        .to_string()
        .into_bytes()
    }
    #[test]
    fn recognition_progress_accepts_only_this_task_moving_forward() {
        let mut watch = ProgressWatch::new("task", 30);
        assert_eq!(watch.accept(&progress("task", "preparing", 30, 0, 0)), Some(("preparing".into(), 0, 0)));
        assert_eq!(watch.accept(&progress("task", "preparing", 30, 0, 0)), None, "unchanged");
        assert_eq!(watch.accept(&progress("task", "recognising", 30, 0, 0)), Some(("recognising".into(), 0, 0)));
        assert_eq!(watch.accept(&progress("task", "recognising", 30, 26, 1)), Some(("recognising".into(), 26, 1)));
        for refused in [
            progress("other", "recognising", 30, 28, 1), // a different (stale) task
            progress("task", "recognising", 31, 28, 1),  // a different request size
            progress("task", "recognising", 30, 31, 1),  // more than requested
            progress("task", "recognising", 30, 28, 29), // more failures than attempts
            progress("task", "recognising", 30, 25, 1),  // backwards
            progress("task", "preparing", 30, 26, 1),    // back to model loading
            progress("task", "importing", 30, 28, 1),    // not a worker stage
            br#"{"task_id":"task","stage":"recognising","total":30,"processed":28,"failed":1}"#.to_vec(),
            b"{\"format\":".to_vec(),
        ] {
            assert_eq!(watch.accept(&refused), None, "{}", String::from_utf8_lossy(&refused));
        }
        assert_eq!(watch.accept(&progress("task", "recognising", 30, 30, 2)), Some(("recognising".into(), 30, 2)));
    }
    #[test]
    fn recognition_progress_poll_reports_counts_with_unreadable_files() {
        let root = std::env::temp_dir().join(format!("illustash-sync-progress-unit-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("progress.json");
        let mut watch = ProgressWatch::new("task", 4);
        assert_eq!(watch.poll(&path, 2), None, "no file yet");
        std::fs::write(&path, progress("task", "recognising", 4, 3, 1)).unwrap();
        watch.read_at = None;
        let fields = watch.poll(&path, 2).unwrap();
        assert_eq!(
            (fields["status"].as_str(), fields["stage"].as_str(), fields["done"].as_u64(), fields["failed"].as_u64()),
            (Some("recognising"), Some("recognising"), Some(5), Some(3))
        );
        std::fs::write(&path, progress("task", "recognising", 4, 4, 1)).unwrap();
        assert_eq!(watch.poll(&path, 2), None, "read at most every 200 ms");
        watch.read_at = None;
        assert!(watch.poll(&path, 2).unwrap()["ms_per_image"].is_u64());
        std::fs::write(&path, vec![b' '; 4097]).unwrap();
        watch.read_at = None;
        assert_eq!(watch.poll(&path, 2), None, "oversized file is ignored");
        std::fs::remove_dir_all(root).unwrap();
    }

    type Snapshot = BTreeMap<String, (Vec<String>, Vec<Vec<rusqlite::types::Value>>)>;
    fn snapshot(path: &Path) -> Snapshot {
        let c = crate::app_library::read_connection(path).unwrap();
        if c.prepare("SELECT * FROM meta LIMIT 0").is_ok() {
            crate::http_read::register_names(&c).unwrap();
        }
        c.execute_batch("BEGIN").unwrap();
        let schema: Vec<(String, String, Option<String>)> = c
            .prepare("SELECT type,name,sql FROM sqlite_schema ORDER BY type,name")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let mut queries = vec![
            ("schema".into(), "SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name".into()),
            ("user_version".into(), "PRAGMA user_version".into()),
        ];
        for (kind, name, sql) in schema {
            if matches!(kind.as_str(), "table" | "view") {
                let columns = if kind == "view" || sql.unwrap_or_default().to_uppercase().contains("WITHOUT ROWID") {
                    "*"
                } else {
                    "rowid,*"
                };
                queries.push((name.clone(), format!("SELECT {columns} FROM \"{}\"", name.replace('"', "\"\""))));
            }
        }
        queries
            .into_iter()
            .map(|(name, sql)| {
                let mut q = c.prepare(&sql).unwrap();
                let columns = q.column_names().into_iter().map(str::to_owned).collect();
                let count = q.column_count();
                let mut rows = q
                    .query_map([], |r| (0..count).map(|i| r.get(i)).collect::<rusqlite::Result<Vec<_>>>())
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                rows.sort_by_cached_key(|r| format!("{r:?}"));
                (name, (columns, rows))
            })
            .collect()
    }

    #[test]
    fn scan_cancellation_and_completed_lifecycle_have_explicit_boundaries() {
        let root = std::env::temp_dir().join(format!("illustash-sync-boundary-{}", crate::store::new_key().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "app_sync::tests::scan_boundary_fixture", "--nocapture"])
            .env("ILLUSTASH_SYNC_BOUNDARY_FIXTURE", &root)
            .env("ILLUSTASH_APP_SUPPORT", root.join("support"))
            .env("ILLUSTASH_SHARED", root.join("absent-reference"))
            .env("ILLUSTASH_NO_KEYCHAIN", "1")
            .status()
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
        assert!(status.success());
    }

    #[test]
    #[ignore = "isolated synthetic sync fixture, invoked by parent test"]
    fn scan_boundary_fixture() {
        let root = std::path::PathBuf::from(std::env::var_os("ILLUSTASH_SYNC_BOUNDARY_FIXTURE").unwrap());
        let folder = root.join("images");
        std::fs::create_dir(&folder).unwrap();
        image::RgbImage::new(4, 3).save(folder.join("a.png")).unwrap();
        let second = root.join("replacement.pilib");
        crate::app_create::create_folder(&second, &folder, None, None, None).unwrap();
        let replacement = Library::open(&second).unwrap();
        let replacement_source = snapshot(&replacement.source());
        let replacement_app = snapshot(&replacement.db());
        let mut options = Options::new(root.clone());
        options.names_enabled = false;
        options.provenance_enabled = false;
        options.index_automatic = false;
        options.auto_sync = false;
        // Every task below executes the actual sync pipeline, in the actual session-owned slot.
        for finish in ["switch", "shutdown", "drop"] {
            for stage in ["before_source_commit", "before_application_commit", "completed"] {
                let cancelled = stage != "completed";
                println!("SYNC_BOUNDARY {finish} stage={stage}");
                let first = root.join(format!("{finish}-{stage}.pilib"));
                crate::app_create::create_folder(&first, &folder, None, None, None).unwrap();
                // Same logical ID cannot let an old task write a different physical package.
                let config_path = first.join("library.json");
                let mut config: Value = serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
                config["id"] = replacement.id().into();
                std::fs::write(config_path, serde_json::to_vec(&config).unwrap()).unwrap();
                let lib = Library::open(&first).unwrap();
                assert_eq!(lib.id(), replacement.id());
                assert_ne!(lib.instance_path(), replacement.instance_path());
                let mut c = crate::open(&lib.db()).unwrap();
                let asset = crate::store::register_asset(&c, "synthetic", "manual", None).unwrap();
                let term = crate::store::create_term(&c, crate::Kind::Label, Some("manual-label"), "user").unwrap();
                crate::store::judge(
                    &mut c,
                    &[crate::store::VerdictTarget::Image { asset, term }],
                    crate::store::Verdict::Yes,
                    "user",
                    "",
                    "test",
                )
                .unwrap();
                c.execute("INSERT INTO backend_effects(kind) VALUES ('synthetic-pending')", []).unwrap();
                drop(c);
                let source_before = snapshot(&lib.source());
                let service = Arc::new(Application::open(&first, options.clone()).unwrap());
                let generation = service.generation();
                let (ready_tx, ready_rx) = mpsc::channel();
                let (cancel_tx, cancel_rx) = mpsc::channel();
                let (release_tx, release_rx) = mpsc::channel();
                let context;
                {
                    let mut session = service.session.write().unwrap();
                    let identity =
                        crate::app_tasks::TaskIdentity::new(&session.library, generation, &session.token).unwrap();
                    let task_lib = session.library.clone();
                    let task_root = root.clone();
                    let task = crate::app_tasks::Task::start(identity, None, move |ctx| {
                        let gated = AtomicBool::new(false);
                        run_with(&task_lib, ctx.cancellation(), Some(&ctx), &task_root, 0, &|fields| {
                            if ((stage == "before_source_commit" && fields.get("scanned").is_some())
                                || (stage == "before_application_commit" && fields["status"] == "importing"))
                                && !gated.swap(true, Ordering::SeqCst)
                            {
                                // Existing callbacks bracket source commit and application refresh.
                                // Hold precisely this boundary until cancellation has been observed.
                                ready_tx.send(()).unwrap();
                                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                                while ctx.check().is_ok() {
                                    assert!(std::time::Instant::now() < deadline, "lifecycle did not cancel scan");
                                    std::thread::yield_now();
                                }
                                cancel_tx.send(()).unwrap();
                                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                            }
                        })
                        .map(|_| ())
                    });
                    context = task.context();
                    session.syncer.state = Arc::new(Mutex::new(State::from(json!({"status":"scanning"}))));
                    session.syncer.install_fixture_task(task);
                    if !cancelled {
                        assert!(session.syncer.task.as_mut().unwrap().wait(Duration::from_secs(5)));
                        assert_eq!(
                            session.syncer.task.as_ref().unwrap().outcome(),
                            crate::app_tasks::TaskOutcome::Done
                        );
                    }
                }
                if cancelled {
                    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    let mut session = service.session.write().unwrap();
                    let library = session.library.clone();
                    let token = session.token.clone();
                    assert!(!session.syncer.start(&library, generation, &token).unwrap());
                    if stage == "before_source_commit" {
                        assert_eq!(snapshot(&lib.source()), source_before);
                    }
                }
                let source_at_finish = snapshot(&lib.source());
                let src = crate::app_library::read_connection(&lib.source()).unwrap();
                let files: i64 = src.query_row("SELECT count(*) FROM files", [], |r| r.get(0)).unwrap();
                assert_eq!(files, if stage == "before_source_commit" { 0 } else { 1 });
                drop(src);
                let run = crate::app_library::read_connection(&lib.db()).unwrap();
                let status: String = run.query_row("SELECT status FROM sync_runs", [], |r| r.get(0)).unwrap();
                assert_eq!(status, if cancelled { "running" } else { "needs_recognition" });
                drop(run);
                let mut app_at_finish = snapshot(&lib.db());
                app_at_finish.remove("sync_runs");
                let completed_run = (!cancelled).then(|| snapshot(&lib.db())["sync_runs"].clone());
                let observer = (finish != "drop").then(|| service.clone());
                let target = second.clone();
                let (done_tx, done_rx) = mpsc::channel();
                let thread = std::thread::spawn(move || {
                    match finish {
                        "switch" => {
                            service.switch(generation, &target).unwrap();
                        }
                        "shutdown" => service.shutdown(),
                        "drop" => drop(service),
                        _ => unreachable!(),
                    }
                    done_tx.send(()).unwrap();
                });
                if cancelled {
                    cancel_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    assert!(matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
                    if finish == "switch" {
                        assert_eq!(observer.as_ref().unwrap().generation(), generation);
                    }
                    release_tx.send(()).unwrap();
                }
                done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                thread.join().unwrap();
                if let Some(observer) = observer {
                    assert!(observer.with_library::<()>(generation, |_| panic!("old generation published")).is_err());
                }
                assert!(context.receive::<()>(context.identity(), || panic!("old result published")).is_err());
                assert_eq!(snapshot(&lib.source()), source_at_finish);
                let mut app_after = snapshot(&lib.db());
                let runs = app_after.remove("sync_runs").unwrap();
                assert_eq!(app_after, app_at_finish);
                let c = crate::app_library::read_connection(&lib.db()).unwrap();
                let status: String = c.query_row("SELECT status FROM sync_runs", [], |r| r.get(0)).unwrap();
                assert_eq!(status, if cancelled { "interrupted" } else { "needs_recognition" });
                if let Some(completed_run) = completed_run {
                    assert_eq!(runs, completed_run);
                }
                assert_eq!(snapshot(&replacement.source()), replacement_source);
                assert_eq!(snapshot(&replacement.db()), replacement_app);
            }
        }
    }
}
