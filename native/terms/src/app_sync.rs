//! 原生来源同步与可选识别 worker 编排。任务固定包实例，切库/退出取消并等待。
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
    /// 有后台任务（角色索引写入、WD 扩展检查）挡着时收到的同步请求（#607）：记下来，挡着的任务结束后由恢复检查
    /// （[`crate::app_service::Application::recover`]）接着开始，不报错。页面上显示为 `waiting`。
    queued: bool,
}
impl Default for Syncer {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::from(json!({"status":"idle"})))),
            task: None,
            resource_root: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
            per_run: 0,
            enabled: true,
            queued: false,
        }
    }
}
impl Syncer {
    pub fn running(&self) -> bool {
        self.task.as_ref().is_some_and(crate::app_tasks::Task::running)
    }
    /// 先记下这次同步请求，等挡着的后台任务结束（#607）。同步关着时和 [`Self::start`] 一样拒绝。
    pub(crate) fn queue(&mut self, library: &Library) -> Result<()> {
        self.allowed(library)?;
        self.queued = true;
        Ok(())
    }
    fn allowed(&self, library: &Library) -> Result<()> {
        if !self.enabled || !Self::available(library) {
            return Err(Error::Invalid("本次启动关闭了同步（--no-sync）".into()));
        }
        Ok(())
    }
    /// 有没有排着的同步请求。
    pub(crate) fn queued(&self) -> bool {
        self.queued
    }
    /// 取走排着的同步请求。
    pub(crate) fn take_queued(&mut self) -> bool {
        std::mem::take(&mut self.queued)
    }
    pub fn available(library: &Library) -> bool {
        !library.frozen()
            && library.config()["sources"].as_array().is_some_and(|s| {
                !s.is_empty() && s.iter().all(|s| matches!(s["kind"].as_str(), Some("folder" | "photos")))
            })
    }
    /// 页面看到的同步状态（`GET /api/sync` 的 `state`），只读内存，不读数据库；任务面板和桌面外壳（#451）都用它。
    pub(crate) fn snapshot(&self, library: &Library) -> crate::http_json::Json {
        use crate::http_json::Json;
        if self.enabled && Self::available(library) {
            let mut state = self.state.lock().expect("sync state mutex poisoned").snapshot();
            if let Some(task) = &self.task {
                if let crate::app_tasks::TaskOutcome::Failed(message) = task.outcome() {
                    state.set("status", "error".into());
                    state.set("message", message.chars().take(300).collect::<String>().into());
                }
            }
            if self.queued && !self.running() {
                state = Json::object([("status", "waiting".into())]);
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
                    "input_done",
                    "input_total",
                    "unreadable",
                    "damaged",
                    "problem",
                    "diagnostic",
                ]
                .iter()
                .filter_map(|k| state.get(k).map(|v| ((*k).to_string(), v.clone())))
                .collect(),
            )
        } else {
            Json::object([("status", "off".into())])
        }
    }
    pub fn status(&self, library: &Library) -> Result<crate::http_json::Json> {
        use crate::http_json::Json;
        let c = crate::app_library::read_connection(&library.db())?;
        let mut q = c.prepare("SELECT * FROM sync_runs ORDER BY id DESC LIMIT 8")?;
        let names: Vec<String> = q.column_names().map(str::to_owned).collect();
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
        let state = self.snapshot(library);
        let mut fields = vec![("state", state), ("history", Json::Array(history))];
        if let Some(listing) = damaged::listing(&c)? {
            fields.push(("damaged", listing));
        }
        Ok(Json::object(fields))
    }
    pub fn start(&mut self, library: &Library, generation: u64, session: &str) -> Result<bool> {
        self.allowed(library)?;
        // 开始（或已在进行）的这次同步就满足了排着的请求。
        self.queued = false;
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
            // 新图库默认启用角色资料包（#602）：还没有任何识别向量时，在这次同步之前记成启用。
            if let Err(error) = crate::app_packs::default_enable(&root, &library) {
                eprintln!("无法默认启用角色资料包：{error}");
            }
            let result = run_with(&library, cancel, Some(&ctx), &root, per_run, &|fields| {
                if ctx.check().is_err() {
                    return;
                }
                // 每次状态变化都来自完成的工作（检查了文件、识别了图片、导入了批次），所以只要任务在推进就一直运行。
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
                    let stage = crate::diagnostics::stage(
                        s.get("stage").or_else(|| s.get("status")).map(crate::http_json::Json::text).unwrap_or(""),
                    );
                    s.set(
                        "status",
                        (if cancel.load(Ordering::SeqCst) || crate::platform_process::stopping() {
                            "interrupted"
                        } else {
                            "error"
                        })
                        .into(),
                    );
                    if s.status() == Status::Error || ctx.timed_out() {
                        let d = crate::diagnostics::failure(
                            crate::diagnostics::Operation::Sync,
                            stage,
                            &e,
                            crate::diagnostics::Publication::MayBePartial,
                        );
                        s.set("diagnostic", d.value().into());
                    }
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
    /// 打开图库时的自动同步：按启动设置决定要不要同步。`wait` 为真（有后台任务挡着）时只记下请求，等它结束（#607）。
    pub fn automatic(&mut self, library: &Library, generation: u64, session: &str, wait: bool) -> Result<()> {
        let prefs: Value = serde_json::from_str(&crate::app_settings::startup(library)?.dump())
            .map_err(|e| Error::Invalid(e.to_string()))?;
        if self.enabled && Self::available(library) && prefs["sync"] == true {
            if wait {
                self.queue(library)?;
            } else {
                self.start(library, generation, session)?;
            }
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
/// 同步一直运行到不再有进展为止，而不是从开始起跑固定的时长。
/// ILLUSTASH_RECOGNITION_TIMEOUT_MS（默认 10 分钟）是最长允许的无进展时长；
/// ILLUSTASH_RECOGNITION_MAX_MS（默认 24 小时）限定整次运行。
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
/// 被取消或超时的运行报告它为什么停下，而不是碰巧被打断的那一步。
fn stop_message(cancel: &AtomicBool, ctx: Option<&crate::app_tasks::TaskContext>, e: &Error) -> String {
    let stopped = if cancel.load(Ordering::SeqCst) { ctx.and_then(|c| c.stop_reason()) } else { None };
    stopped.unwrap_or_else(|| e.reason()).chars().take(300).collect()
}
fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}
fn writer(path: &std::path::Path) -> Result<Connection> {
    let c = crate::sqlite::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    crate::configure(&c)?;
    c.busy_timeout(crate::app_library::TASK_WRITE_WAIT)?;
    Ok(c)
}
/// 可供 M4 复用：文件盘点后本地刷新；imported 事件与应用变更同事务提交，保留 pending 给 M4 消费。
pub(crate) fn run(library: &Library, cancel: &AtomicBool, update: &dyn Fn(Value)) -> Result<Value> {
    run_with(library, cancel, None, &crate::app_resources::default_root(), 0, update)
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
    let wd = context.and_then(|_| {
        crate::app_wd_extension::Delivery::load(root, library).unwrap_or_else(|error| {
            eprintln!("无法加载本次同步的 WD 扩展包：{error}");
            None
        })
    });
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
    // 用户对失败要做什么，用一个代码表示，由页面配上文字（#202）；message 仍是错误自己的。
    let problem = std::cell::Cell::new(None);
    // 当前步骤和它写入的数据库，失败时写进日志（#275）：只看 message 看不出是哪个数据库忙。
    let step = std::cell::Cell::new("recognition resume, source.sqlite and library.sqlite");
    let result: Result<(&str, String, Value)> = (|| {
        // 仅撤回没有应用投影、且已确定不能验证输入的 pending；随后扫描可重新识别这些图片。
        let active = || context.map_or_else(|| check(cancel), crate::app_tasks::TaskContext::check);
        retry_recognition_write(active, || crate::app_recognition::retire_unverifiable(library, cancel))?;
        let mut sources: Vec<(String, Value)> = Vec::new();
        let mut scanned = 0;
        let mut unreadable = 0;
        step.set("scan, source.sqlite");
        for source in library.config()["sources"].as_array().into_iter().flatten() {
            let done_in_source = std::cell::Cell::new(0);
            let progress = |done, total| {
                done_in_source.set(done);
                update(json!({"scanned":scanned+done,"scan_total":scanned+total}))
            };
            crate::app_relocation::resume_undo(source, library)?;
            if source["kind"] == "folder"
                && source["path"].as_str().is_some_and(|path| std::fs::metadata(path).is_err())
            {
                unreadable += 1;
                continue;
            }
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
            // 应用库失败后靠持久化的来源哈希驱动重试，而不是这份内存里的列表。
        }
        // 先扫描，避免已删除/换内容的图片被旧 pending 永久挡住；仍有效的结果再恢复。
        step.set("recognition resume, source.sqlite and library.sqlite");
        let deferred = retry_recognition_write(active, || {
            crate::app_recognition::resume_sync(library, root, cancel, wd.as_ref())?.unavailable_inputs()
        })?;
        unreadable += deferred.len() as u64;
        step.set("sync record, library.sqlite");
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
        step.set("library refresh, library.sqlite");
        let (out, refreshed) = refresh(library, run_id, cancel, &deferred)?;
        counts["imported"] = out["added"].clone();
        counts["removed"] = out["removed"].clone();
        let pending = out["pending"].as_u64().expect("internal sync counter must be unsigned");
        if pending == 0 {
            // 每张图都已识别或已消失：不再有已知损坏的图。
            damaged::save(library, &damaged::load(library)?, &serde_json::Map::new())?;
        }
        if pending > 0 {
            if let Some(ctx) = context {
                let worker =
                    std::env::var_os("ILLUSTASH_RECOGNITION_WORKER").filter(|_| !crate::app_models::waiting(root));
                if let Some(worker) = worker {
                    step.set("recognition, source.sqlite and library.sqlite");
                    recognise(
                        library,
                        ctx,
                        &worker,
                        RecognitionRun { root, per_run, already_counted: &refreshed, run_id, wd: wd.as_ref() },
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
    if let Err(e) = &result {
        if !cancel.load(Ordering::SeqCst) {
            eprintln!("同步失败（{step}）：{e:?}", step = step.get());
        }
    }
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
fn refresh(
    library: &Library,
    run_id: i64,
    cancel: &AtomicBool,
    deferred: &HashSet<String>,
) -> Result<(Value, HashSet<String>)> {
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
    // 机器结果是否最新以源库标签为准。已有的内容元数据持久化在 meta 里，这样源库提交后应用库失败时，
    // 即使下次扫描看到文件没变也会重试。
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
        // 保留其应用状态和哈希基线；输入回来后仍须经过 resume 校验，不能由普通刷新发布。
        if deferred.contains(u) {
            continue;
        }
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
    // Photos 保持既定的做法：只有就绪的图片才导入应用库。文件夹盘点结果在识别之前就能浏览，这是 M3 交付的行为；
    // 两种来源共用同一份待办。
    let photos_pending=src.prepare("SELECT a.uuid FROM assets a WHERE EXISTS(SELECT 1 FROM asset_links l WHERE l.asset=a.uuid AND l.system='photos' AND l.source_id=a.source_id) AND NOT EXISTS(SELECT 1 FROM labels l WHERE l.uuid=a.uuid AND l.dim='rating')")?.query_map([],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<HashSet<_>>>()?;
    let added = live
        .iter()
        .filter(|r| !deferred.contains(r[0].as_str().expect("SQLite uuid column must be text")))
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
    // WD 结果走既定的原生证据包路径交付；源库快照保持只读。
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
    let realistic = crate::app_recognition::realistic(&c, &src, &uuids)?;
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
    crate::app_migrate::invalidate_wd(&tx, &changed)?;
    for r in &live {
        if deferred.contains(r[0].as_str().expect("SQLite uuid column must be text")) {
            continue;
        }
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
    // 改变的图片已在上面导入（随后有 'imported' 事件），所以 similar 会重新计算它们。
    crate::app_research::forget_content(&tx, &src, &changed)?;
    for (key, hash) in hashes {
        tx.execute("INSERT INTO meta(key,value) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value<>excluded.value", params![key, hash])?;
    }
    check(cancel)?;
    crate::schema::commit_writing(tx)?;
    // 应用库里的哈希标记让重试是幂等的，即使删除已提交的日志失败也一样。
    if journal_path.exists() {
        std::fs::remove_file(&journal_path)?;
    }
    Ok((json!({"added":out["added"],"removed":out["removed"],"pending":pending}), uuids.into_iter().collect()))
}
/// 本次运行放弃的图片，按怎样处理有用来分：等一会儿再同步，或者修复、替换文件。
fn failure_message(failed: u64, damaged: u64) -> String {
    let mut parts = Vec::new();
    if failed > 0 {
        parts.push(format!("{n} 张暂时无法识别，下次同步重试", n = failed));
    }
    if damaged > 0 {
        parts.push(format!("{n} 张图片已损坏或格式不支持，留在图库里但不识别；文件改动后会重新识别", n = damaged));
    }
    parts.join("；")
}
/// 扫描读不到的文件夹路径里的图片保持原样；说明这一点，让用户可以重试。
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
/// 在宿主等待识别 worker 时读取它的进度文件。只接受属于本任务、不超出本次请求规模并且向前推进的值；
/// 其他都忽略，所以过期或格式不对的文件既不能伪造进度，也不能让任务失败。
struct ProgressWatch {
    task_id: String,
    total: usize,
    read_at: Option<std::time::Instant>,
    processed: Option<usize>,
    stage: String,
    /// worker 报告第一张识别完的图片的时间，用于估算剩余时间。
    started: Option<(std::time::Instant, usize)>,
}
impl ProgressWatch {
    fn new(task_id: &str, total: usize) -> Self {
        Self { task_id: task_id.to_owned(), total, read_at: None, processed: None, stage: String::new(), started: None }
    }
    /// `bytes` 里校验过的 (stage, processed, failed)，前提是比已显示的更新。
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
    /// 同步任务的状态字段，最多每 200 毫秒一次。
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
    wd: Option<&'a crate::app_wd_extension::Delivery>,
}
/// 在打开首张输入前报告准备阶段，随后按完成文件推进；失败输入单独留待下次同步。
fn prepare_inputs(
    assets: Vec<Value>,
    check: &dyn Fn() -> Result<()>,
    update: &dyn Fn(Value),
) -> Result<(Vec<protocol::SyncAsset>, HashSet<String>)> {
    let total = assets.len();
    let mut readable = Vec::new();
    let mut failed = HashSet::new();
    update(json!({"status":"recognising","stage":"hashing","total":total,"done":0,"failed":0,
        "input_done":0,"input_total":total}));
    for (index, row) in assets.into_iter().enumerate() {
        check()?;
        let id = row[0].as_str().expect("SQLite uuid column must be text").to_owned();
        let path = row[1].as_str().ok_or_else(|| Error::invalid("source.sqlite: assets.src_path 必须为字符串"))?;
        match crate::recognition_compute::producer::input_hash(std::path::Path::new(path), check) {
            Ok(input_sha256) => readable.push(protocol::SyncAsset { id, path: path.to_owned(), input_sha256 }),
            Err(Error::Io(_)) => {
                failed.insert(id);
            }
            Err(e) => return Err(e),
        }
        update(json!({"input_done":index+1,"failed":failed.len()}));
    }
    Ok((readable, failed))
}
/// 等待识别的图片（uuid、路径、内容哈希），先认最近保存的（#360）：保存时间从新到旧，同一时间按资产 ID，
/// 与图库默认的浏览顺序相同（http_browse.rs 的 `ordering`），所以识别到一半时最先看到的几屏已经认完。
/// 没有保存时间的排在最后。
fn to_recognise(source: &Connection) -> Result<Vec<Value>> {
    crate::app_migrate_data::rows(
        source,
        "SELECT uuid,src_path,stable_hash FROM assets a WHERE in_scope=1 AND coalesce(trashed,0)=0 \
         AND NOT EXISTS(SELECT 1 FROM labels l WHERE l.uuid=a.uuid AND l.dim='rating') ORDER BY added_at DESC,uuid",
    )
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
    let RecognitionRun { root, per_run, already_counted, run_id, wd } = options;
    let c = crate::app_library::read_connection(&library.source())?;
    let mut assets = to_recognise(&c)?;
    drop(c);
    // 能读出但解码不了的图片保持不变地等着，直到文件改变（#112）；它们从不占用 per_run 的名额。
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
            // 现在读不到：按读不到重试；如果回来时没变，仍算已知损坏。
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
    // 每轮上限留下的是最近保存的那几张，其余的等下一轮。
    if per_run > 0 {
        assets.truncate(per_run);
    }
    let total = assets.len();
    let (readable, mut failed) = prepare_inputs(assets, &|| ctx.check(), update)?;
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
    // 放在任务文件夹里，这样会随批次一起清理。不写这个文件的 worker 和以前一样，计数停在原处直到导入。
    let progress = folder.join("progress.json");
    command.env("ILLUSTASH_RECOGNITION_PROGRESS", &progress);
    let mut watch = ProgressWatch::new(&ctx.identity().task_id, readable.len());
    let unreadable = failed.len();
    // 只有平台认可的、随安装包发布的原生路线才强制要求 sidecar。
    // 其他 worker（没有原生路线的平台、测试替身）不强制，保持现有的响应契约。显式声明的 sidecar 在下面各平台上都完整校验。
    let native_adapter = crate::recognition_compute::production_adapter_enabled()
        && std::path::Path::new(worker).file_stem().is_some_and(|name| name == "illustash-recognition");
    let mut batches = Batches {
        library,
        require_claims: native_adapter,
        ctx,
        root,
        wd,
        folder: crate::app_paths::canonical(&folder)?,
        expected: readable.iter().map(|r| r.id.clone()).collect(),
        expected_inputs: readable.iter().map(|r| (r.id.clone(), r.input_sha256.clone())).collect(),
        seen: HashSet::new(),
        streamed: HashSet::new(),
        inputs: None,
        already_counted,
        run_id,
        import_done: 0,
        deferred: HashSet::new(),
        scanned_at: None,
    };
    // worker 计算下一批的同时导入每个已完成的批次，这样中途被打断时已导入的批次都保留；
    // 下一次同步只识别剩下的部分。
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
            // 已在 worker 运行期间导入；它的图片已算作看过。
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
        // 现在连查看都做不到的文件算作读不到，之后重试。
        match (u.reason, damaged::fingerprint(path, hash.as_deref())) {
            (protocol::FailureReason::InputChanged, _) => {
                marked.remove(id);
                failed.insert(id.to_owned());
            }
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
    failed.extend(batches.deferred.iter().cloned());
    damaged::save(library, &kept, &marked)?;
    counts["damaged"] = json!(skipped + found);
    counts["failed"] = json!(failed.len());
    // 识别结束了：回复里的计数是最终值。导入有自己的计数，所以进度条从不会从「已识别」退回到「已导入」，
    // 也不会把识别完的显示成已导入。
    let import_total = batches.import_done + packages.iter().map(|(_, ids, _)| ids.len()).sum::<usize>();
    update(json!({"status":"importing","stage":"verifying","done":total,"total":total,"failed":failed.len(),
        "import_done":batches.import_done,"import_total":import_total}));
    // 还没导入的批次：每批单独检查和导入，与流式导入的批次一样。
    for (package, ids, claims) in packages {
        batches.import(&package, &ids, claims.as_deref(), counts, update)?;
    }
    update(
        json!({"status":"recognising","done":total,"total":total,"imported":counts["imported"],"failed":counts["failed"]}),
    );
    Ok(())
}

/// 一个任务的识别批次：覆盖哪些图片，哪些已经导入。
struct Batches<'a> {
    library: &'a Library,
    /// 原生路线必须在每个批次里给出角色认领。
    require_claims: bool,
    ctx: &'a crate::app_tasks::TaskContext,
    root: &'a std::path::Path,
    wd: Option<&'a crate::app_wd_extension::Delivery>,
    folder: std::path::PathBuf,
    expected: HashSet<String>,
    expected_inputs: HashMap<String, String>,
    seen: HashSet<String>,
    /// worker 还在运行时就已导入的批次。
    streamed: HashSet<std::path::PathBuf>,
    /// 模型和锚点快照，在第一个带角色认领的批次时加载一次。
    inputs: Option<crate::app_recognition::WorkerInputs>,
    already_counted: &'a HashSet<String>,
    run_id: i64,
    import_done: usize,
    deferred: HashSet<String>,
    scanned_at: Option<std::time::Instant>,
}
impl Batches<'_> {
    /// 每张图片都属于本次运行，并且恰好属于一个批次。
    fn claim(&mut self, ids: &[String]) -> Result<()> {
        for id in ids {
            if !self.expected.contains(id) || !self.seen.insert(id.to_owned()) {
                return Err(Error::Conflict("结果超出本次识别范围或重复".into()));
            }
        }
        Ok(())
    }
    /// 导入 worker 自上次查看以来标记为完成的批次，最多每 500 毫秒一次。
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
    /// worker 已完成的批次的图片列表和认领 sidecar，按它的完成标记核对过。
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
    /// 在本任务的回执下，用单独的源库事务检查并导入一个批次。
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
        let manifest = read_recognition_manifest(package)?;
        if manifest.assets != ids {
            return Err(Error::Conflict("识别结果的资产列表已改变".into()));
        }
        manifest.check_inputs(&self.expected_inputs)?;
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
        let deferred = retry_recognition_write(
            || ctx.check(),
            || {
                ctx.receive_part(ctx.identity(), || {
                    match &prepared {
                        Some(prepared) => crate::app_recognition::import_worker_package(
                            library,
                            prepared,
                            self.inputs.as_ref().expect("claims task snapshot"),
                            ctx.cancellation(),
                            &crate::app_research::dispatch,
                            self.wd,
                        ),
                        None => crate::app_recognition::import_sync_package(
                            library,
                            package,
                            self.root,
                            ctx.cancellation(),
                            self.wd,
                        ),
                    }?
                    .unavailable_inputs()
                })
            },
        )?;
        if ids.iter().any(|id| deferred.contains(id)) {
            self.deferred.extend(ids.iter().cloned());
            counts["failed"] = json!(counts["failed"].as_u64().unwrap_or(0) + ids.len() as u64);
            return Ok(());
        }
        counts["imported"] = json!(
            counts["imported"].as_u64().expect("internal sync counter must be unsigned")
                + ids.iter().filter(|u| !self.already_counted.contains(*u)).count() as u64
        );
        retry_recognition_write(|| ctx.check(), || remember_labels(library, ids, ctx.cancellation()))?;
        self.import_done += ids.len();
        retry_recognition_write(
            || ctx.check(),
            || {
                writer(&library.db())?.execute(
                    "UPDATE sync_runs SET imported=?,failed=? WHERE id=?",
                    params![counts["imported"].as_u64(), counts["failed"].as_u64(), self.run_id],
                )?;
                Ok(())
            },
        )?;
        update(json!({"import_done":self.import_done,"imported":counts["imported"],"failed":counts["failed"]}));
        Ok(())
    }
}
/// 识别导入及它的回执都是幂等的。短暂写锁释放后重试，退让前已释放事务和任务接收边界。
fn retry_recognition_write<T>(check: impl Fn() -> Result<()>, mut write: impl FnMut() -> Result<T>) -> Result<T> {
    const RETRIES: usize = 3;
    for attempt in 0..=RETRIES {
        check()?;
        match write() {
            Err(error) if attempt < RETRIES && crate::app_recognition::database_busy(&error) => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            result => return result,
        }
    }
    unreachable!("重试上限返回最后一次写入结果")
}

/// 结果包 manifest 里的图片列表。
fn manifest_assets(path: &std::path::Path) -> Result<Vec<String>> {
    Ok(read_recognition_manifest(path)?.assets)
}
fn read_recognition_manifest(path: &std::path::Path) -> Result<protocol::RecognitionManifest> {
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
    Ok(m)
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

/// 文件能读出但解码不了的图片（#112）。每张都连同当时文件的样子一起记下：大小、修改时间和扫描得到的
/// 内容哈希（#27）。三者都不变时跳过这张图；任何变化，比如文件被替换，都会让下一次同步再试一次。
/// 存成一个偏好项，没有研究输入会读它，所以不用改 schema。
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
        // 标记读不出时，代价只是每张图多试一次。
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
    /// 已知损坏的图片有多少张、前几张在哪里，给同步状态用。
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
    /// 状态里显示这么多个位置；其余的只计数。
    const LISTED: i64 = 50;
    use rusqlite::OptionalExtension;
}

/// 现有的宿主取消判断；读取适配器在原来的检查点收到它。
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
    fn filesystem_failure_uses_new_history_message_and_keeps_length_limit() {
        let cancel = AtomicBool::new(false);
        let path = std::path::Path::new("synthetic/input");
        let error = crate::error::io_at(std::io::Error::other("合成失败"), path, None);
        assert_eq!(stop_message(&cancel, None, &error), "文件操作失败：合成失败（路径：'synthetic/input'）");
        let error = crate::error::io_at(std::io::Error::other("长".repeat(400)), path, None);
        assert_eq!(stop_message(&cancel, None, &error), error.reason().chars().take(300).collect::<String>());
    }
    #[test]
    fn input_preparation_reports_progress_and_keeps_unavailable_inputs_retryable() {
        let root = std::env::temp_dir().join(format!("illustash-prepare-{}", crate::store::new_key().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let file = root.join("input.png");
        std::fs::write(&file, b"synthetic input").unwrap();
        let events = std::cell::RefCell::new(Vec::new());
        let rows = vec![json!(["good", file]), json!(["directory", root]), json!(["missing", root.join("missing")])];
        let (readable, failed) = prepare_inputs(rows.clone(), &|| Ok(()), &|v| events.borrow_mut().push(v)).unwrap();
        assert_eq!(readable.len(), 1);
        assert_eq!(readable[0].id, "good");
        assert_eq!(readable[0].input_sha256, crate::strict_json::sha256_hex(b"synthetic input"));
        assert_eq!(failed, HashSet::from(["directory".into(), "missing".into()]));
        let events = events.borrow();
        assert_eq!(events[0]["stage"], "hashing");
        assert_eq!(events[0]["done"], 0);
        assert_eq!(events.iter().map(|v| v["input_done"].as_u64().unwrap()).collect::<Vec<_>>(), [0, 1, 2, 3]);
        assert_eq!(events.last().unwrap()["failed"], 2);
        assert!(prepare_inputs(rows, &|| Err(Error::Conflict("synthetic cancellation".into())), &|_| {}).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unreadable_and_damaged_pictures_are_reported_apart() {
        assert_eq!(failure_message(0, 0), "");
        assert_eq!(failure_message(2, 0), "2 张暂时无法识别，下次同步重试");
        assert_eq!(failure_message(0, 1), "1 张图片已损坏或格式不支持，留在图库里但不识别；文件改动后会重新识别");
        assert_eq!(
            failure_message(2, 1),
            "2 张暂时无法识别，下次同步重试；1 张图片已损坏或格式不支持，留在图库里但不识别；文件改动后会重新识别"
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
            progress("other", "recognising", 30, 28, 1), // 另一个（过期的）任务
            progress("task", "recognising", 31, 28, 1),  // 请求规模不同
            progress("task", "recognising", 30, 31, 1),  // 超过请求的数量
            progress("task", "recognising", 30, 28, 29), // 失败数多于尝试数
            progress("task", "recognising", 30, 25, 1),  // 倒退
            progress("task", "preparing", 30, 26, 1),    // 退回加载模型阶段
            progress("task", "importing", 30, 28, 1),    // 不是 worker 的阶段
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
    #[test]
    fn pictures_waiting_for_recognition_come_newest_saved_first() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("app_source_empty.sql")).unwrap();
        c.execute_batch(
            "INSERT INTO assets(uuid,added_at,src_path,in_scope,trashed) VALUES
               ('B','2026-11-10T08:00:00','b.png',1,0),
               ('A','2026-11-12T09:30:00','a.png',1,NULL),
               ('D','2026-11-10T08:00:00','d.png',1,0),
               ('C','','c.png',1,0),
               ('E',NULL,'e.png',1,0),
               ('F','2026-11-13T00:00:00','f.png',1,0),
               ('G','2026-11-13T00:00:00','g.png',1,1),
               ('H','2026-11-13T00:00:00','h.png',0,0);
             INSERT INTO labels(uuid,dim,value,conf,source) VALUES ('F','rating','general',1,'wd_v3');",
        )
        .unwrap();
        let waiting = to_recognise(&c).unwrap();
        let order: Vec<&str> = waiting.iter().map(|row| row[0].as_str().unwrap()).collect();
        // 最新的在前，同一时间按资产 ID；没有保存时间的排在最后（空字符串先于 NULL）。
        // 已识别的 F、已移除的 G 和范围外的 H 不在其中。
        assert_eq!(order, ["A", "B", "D", "C", "E"]);
        assert_eq!(waiting[0], json!(["A", "a.png", null]));
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
                let columns = q.column_names().map(str::to_owned).collect();
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
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "app_sync::tests::scan_boundary_fixture", "--nocapture"])
            .env("ILLUSTASH_SYNC_BOUNDARY_FIXTURE", &root)
            .env("ILLUSTASH_APP_SUPPORT", root.join("support"))
            .env("ILLUSTASH_SHARED", root.join("absent-reference"))
            .env("ILLUSTASH_NO_KEYCHAIN", "1")
            .output()
            .unwrap();
        // 失败证据（#501）：报告写明夹具最后进入的阶段，并附上子进程的全部输出。先判断夹具是否成功，
        // 再判断清理，免得 Windows 上清理失败遮住夹具自己的失败原因。断言与时限不变。
        let cleanup = std::fs::remove_dir_all(root);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stage = stdout.lines().rfind(|line| line.starts_with("SYNC_BOUNDARY ")).unwrap_or("（尚未进入任何阶段）");
        assert!(
            output.status.success(),
            "同步边界夹具失败（{}），最后进入的阶段：{stage}\n--- 夹具 stdout ---\n{stdout}\n--- 夹具 stderr ---\n{stderr}",
            output.status
        );
        cleanup.unwrap();
    }

    // 夹具子进程先报告首个失败，再给真实析构路径一个回收任务的机会。
    // 析构卡住时这里仍会失败退出，父测试在子进程退出后清理目录。
    fn fail_scan_wait(service: Application, reason: String, limit: Duration) -> ! {
        eprintln!("{reason}");
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::spawn(move || {
            drop(service);
            let _ = done_tx.send(());
        });
        let cleanup = match done_rx.recv_timeout(limit) {
            Ok(()) => "析构已完成",
            Err(mpsc::RecvTimeoutError::Timeout) => "析构也超时",
            Err(mpsc::RecvTimeoutError::Disconnected) => "析构线程异常退出",
        };
        panic!("{reason}，{cleanup}");
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
        // 切换会启动本图库自己的自动备份。先让首份快照和哈希补算落地，
        // 再以完整数据库为基线，避免把合法后台初始化误算成旧同步任务越界写入。
        let mut backups = crate::app_human_conclusions::Backups::default();
        backups.start(&replacement, 1, "synthetic-backup-session").unwrap();
        let deadline = crate::test_wait::deadline();
        loop {
            let status = backups.status();
            if matches!(status.get("snapshots"), Some(crate::http_json::Json::Array(rows)) if !rows.is_empty())
                && status.get("running").is_some_and(|v| v.dump() == "false")
            {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "initial backup did not settle: {}", status.dump());
            std::thread::sleep(Duration::from_millis(10));
        }
        backups.stop();
        let replacement_source = snapshot(&replacement.source());
        let replacement_app = snapshot(&replacement.db());
        let mut options = Options::new(root.clone());
        options.startup.names_enabled = false;
        options.startup.provenance_enabled = false;
        options.startup.index_auto_check = false;
        options.startup.auto_sync = false;
        // 下面每个任务都跑真实的同步流程，用的是会话持有的真实槽位。
        for finish in ["switch", "shutdown", "drop"] {
            for stage in ["before_source_commit", "before_application_commit", "completed"] {
                let cancelled = stage != "completed";
                println!("SYNC_BOUNDARY {finish} stage={stage}");
                let first = root.join(format!("{finish}-{stage}.pilib"));
                crate::app_create::create_folder(&first, &folder, None, None, None).unwrap();
                // 逻辑 ID 相同也不能让旧任务写入另一个物理图库包。
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
                let mut service = Application::open(&first, options.clone()).unwrap();
                let generation = service.generation();
                let (ready_tx, ready_rx) = mpsc::channel();
                let (cancel_tx, cancel_rx) = mpsc::channel();
                let (release_tx, release_rx) = mpsc::channel();
                let context;
                {
                    let session = service.session.get_mut().unwrap();
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
                                // 现有的回调正好包住源库提交和应用库刷新。
                                // 精确地停在这个边界，直到观察到取消。
                                ready_tx.send(()).unwrap();
                                let deadline = crate::test_wait::deadline();
                                while ctx.check().is_ok() {
                                    assert!(std::time::Instant::now() < deadline, "lifecycle did not cancel scan");
                                    std::thread::yield_now();
                                }
                                cancel_tx.send(()).unwrap();
                                release_rx.recv_timeout(crate::test_wait::limit()).unwrap();
                            }
                        })
                        .map(|_| ())
                    });
                    context = task.context();
                    session.syncer.state = Arc::new(Mutex::new(State::from(json!({"status":"scanning"}))));
                    session.syncer.install_fixture_task(task);
                }
                let limit = crate::test_wait::limit();
                if cancelled {
                    if let Err(error) = ready_rx.recv_timeout(limit) {
                        let _ = release_tx.send(());
                        fail_scan_wait(
                            service,
                            format!("{finish}/{stage}：等待同步到达提交边界失败（{error}）"),
                            limit,
                        );
                    }
                    let session = service.session.get_mut().unwrap();
                    let library = session.library.clone();
                    let token = session.token.clone();
                    assert!(!session.syncer.start(&library, generation, &token).unwrap());
                    if stage == "before_source_commit" {
                        assert_eq!(snapshot(&lib.source()), source_before);
                    }
                } else {
                    let task = service.session.get_mut().unwrap().syncer.task.as_mut().unwrap();
                    if !task.wait(limit) {
                        fail_scan_wait(service, format!("{finish}/{stage}：同步线程在 {limit:?} 内未退出"), limit);
                    }
                    assert_eq!(task.outcome(), crate::app_tasks::TaskOutcome::Done);
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
                let service = Arc::new(service);
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
                    cancel_rx.recv_timeout(crate::test_wait::limit()).unwrap();
                    assert!(matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
                    if finish == "switch" {
                        assert_eq!(observer.as_ref().unwrap().generation(), generation);
                    }
                    release_tx.send(()).unwrap();
                }
                done_rx.recv_timeout(crate::test_wait::limit()).unwrap();
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
