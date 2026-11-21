//! 会话 GET 业务路由；入口安全检查由 app_http 保持。GET 与 POST 共用的应答小工具也在这里。
use super::*;
use std::sync::{mpsc::Receiver, RwLockReadGuard};

/// 这个请求所属的会话已经不在（或从未存在），页面必须重新加载。
pub(super) fn expired(request: Request) {
    respond(request, error("会话已过期，请刷新页面"), 403, "core");
}
/// 用 `result` 应答；`write` 决定失败是否按保存失败报告。
pub(super) fn reply(request: Request, result: Result<Json>, write: bool) {
    match result {
        Ok(v) => respond(request, v, 200, "core"),
        Err(e) => failure(request, e, write),
    }
}
/// 用不持会话锁算出的结果应答，除非这期间会话已经切换。
pub(super) fn reply_if_current(request: Request, host: &Host, received: u64, result: Result<Json>, write: bool) {
    if !host.current(received) {
        expired(request);
        return;
    }
    reply(request, result, write)
}
/// 研究查询的答复，等任务发来后返回。
pub(super) fn waited(query: Result<Receiver<Result<Json>>>) -> Result<Json> {
    query.and_then(|rx| rx.recv().map_err(|_| Error::Invalid("研究任务已结束".into()))?)
}

fn ccip_task_and_library(
    snapshot_task: impl FnOnce() -> Option<Json>,
    read_library: impl FnOnce() -> Result<serde_json::Value>,
) -> Result<(Option<Json>, serde_json::Value)> {
    // 任务在 SQLite 提交后才发布终态。先取任务快照，避免把终态和提交前的图库状态一起发给页面。
    let task = snapshot_task();
    Ok((task, read_library()?))
}

/// `GET /api/packs`（#602）：本图库的角色资料包。
/// `{catalog:{configured,serial,checked_at,error}, current, offer, download, library, task, wd, ccip}`：
/// `current` 是卡片上的版本（本图库启用的；没启用时是本机已有的最新的，都没有就是最新的），`offer` 是比它新的最新一个
/// （没有时为 null），两个都带两个文件在本机下载了多少（`status`、`have`），`offer` 另带 `downloading`。`download` 是
/// 两个组成部分的下载或导入合成的一个状态，`task` 是启用、换包、撤回的任务（`part` 是正在处理的组成部分）。
/// `library` 是本图库启用了哪个版本、两个组成部分各自启用没有，以及默认启用的提示（`prompt`，见 `app_packs::default_state`）。
/// `location` 是共享数据里放角色资料包的文件夹。`wd`、`ccip` 是两个组成部分的旧路由的完整回复。只读，不联网：目录只跟着检查更新读（`app_updates::post`）。
pub(super) fn packs(host: &Host, session: &Session) -> Result<Json> {
    use serde_json::{json, Value};
    let (checked_at, error) = crate::app_updates::packs_state()?;
    let catalog = crate::app_packs::cached(&host.root)?;
    let lib = crate::app_packs::for_library(&host.root, &session.library)?;
    let download = crate::app_models::pack_download();
    let view = |pack: &crate::app_packs::CharacterPack| -> Result<Value> {
        let mut view = pack.view();
        let progress = crate::app_models::pack_progress(&host.root, pack)?;
        view["status"] = progress["status"].clone();
        view["have"] = progress["have"].clone();
        Ok(view)
    };
    let offer = match crate::app_packs::offer(&lib.known, &lib.card) {
        Some(pack) => {
            let mut v = view(&pack)?;
            v["downloading"] = (download["version"] == pack.version.as_str()).into();
            v
        }
        None => Value::Null,
    };
    let wd = host.wd_status(session)?;
    let ccip: Value =
        serde_json::from_str(&ccip_status(host, session, None)?.dump()).map_err(|e| Error::invalid(e.to_string()))?;
    let prompt = crate::app_packs::default_state(&host.root, &session.library, &lib)?;
    // 两个组成部分合起来的数：有扩展候选的图按图去重，补上的归属按来源算。
    let app = crate::app_library::read_connection(&session.library.db())?;
    let (images, assigned): (i64, i64) = app.query_row(
        "SELECT (SELECT count(DISTINCT uuid) FROM candidates WHERE source IN ('WD_EXT','CCIP_LONGTAIL')),
                (SELECT count(*) FROM memberships WHERE sources IN ('WD_EXT','CCIP_LONGTAIL'))",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let checked = [&wd["library"]["checked_at"], &ccip["library"]["checked_at"]]
        .into_iter()
        .filter_map(Value::as_str)
        .max()
        .map(str::to_owned);
    let vectors = wd["library"]["vectors"].as_u64().unwrap_or(0).max(ccip["library"]["vectors"].as_u64().unwrap_or(0));
    Ok(json!({
        "catalog":{"configured":crate::app_packs::pubkey().is_some(),"serial":catalog.map(|c| c.serial),
            "checked_at":checked_at,"error":error},
        "current":view(&lib.card)?,"offer":offer,"download":download,
        "location":crate::app_packs::extensions_root(&host.root)?,
        "library":{"enabled":lib.wd.is_some() || lib.ccip.is_some(),"version":lib.enabled.as_ref().map(|p| &p.version),
            "wd":lib.wd.is_some(),"ccip":lib.ccip.is_some(),"prompt":prompt,"images":images,"assigned":assigned,
            "vectors":vectors,"checked_at":checked,"blocked":wd["library"]["blocked"].clone()},
        "task":wd["task"].clone(),"wd":wd,"ccip":ccip
    })
    .into())
}

pub(super) fn ccip_status(host: &Host, session: &Session, started: Option<Json>) -> Result<Json> {
    use serde_json::{json, Value};
    let pack = crate::app_ccip_longtail::card_pack(&host.root, &session.library)?;
    let model = crate::app_models::longtail_package(&host.root, &pack)?;
    let (task, local) = ccip_task_and_library(
        || {
            started
                .or_else(|| crate::app_ccip_longtail::auto_task(&session.library).map(Json::from))
                .or_else(|| host.imports.ccip_state(&session.library))
        },
        || crate::app_ccip_longtail::status(&session.library, &host.root),
    )?;
    let model: Value = serde_json::from_str(&model.dump()).map_err(|e| Error::invalid(e.to_string()))?;
    let task = task.map(|mut task| {
        if task.get("phase").is_none() {
            let phase = if task.get("kind").map(Json::text) == Some("withdraw")
                || task.get("stage").map(Json::text) == Some("committing")
            {
                "write"
            } else {
                "scan"
            };
            task.set("phase", phase.into());
        }
        if task.get("status").map(Json::text) == Some("error") {
            task.set("status", "failed".into());
        }
        if task.get("status").map(Json::text) == Some("stopping") {
            task.set("status", "running".into());
            task.set("message", "正在取消参考包检查".into());
        }
        let message = task.get("error").or_else(|| task.get("reason")).cloned();
        if let Some(message) = message {
            task.set("message", message);
        }
        for key in ["done", "total", "message", "result", "diagnostic"] {
            if task.get(key).is_none() {
                task.set(key, Json::from(Value::Null));
            }
        }
        task
    });
    let task = task
        .map(|task| serde_json::from_str::<Value>(&task.dump()))
        .transpose()
        .map_err(|e| Error::invalid(e.to_string()))?;
    let blocked = if session.library.frozen() || session.damage().is_some() {
        Some("frozen")
    } else if session.syncer.running() || session.index.writing() {
        Some("syncing")
    } else if session.wd_extension.running() || host.imports.busy() {
        Some("busy")
    } else if local["vectors"] == 0 {
        Some("no_vectors")
    } else {
        None
    };
    Ok(json!({
        "package": {
            "version": model["pack"], "heads": local["roles"], "license": "见来源与许可",
            "file": {"path": model["files"][0]["path"], "size": model["files"][0]["size"], "sha256": model["files"][0]["sha256"]},
            "status": if model["status"] == "ready" { "ready" } else if model["partial"] == true { "partial" } else { "missing" },
            "have": model["have"], "partial": model["partial"], "location": model["location"],
            "sources": model["sources"], "manual": model["manual"],
        },
        "download": model["job"],
        "library": {
            "enabled": local["enabled"].is_string(), "package_sha256": local["enabled"],
            "enabled_at": local["enabled_at"], "checked_at": local["checked_at"],
            "scored": local["scored"], "vectors": local["vectors"], "images": local["candidates"],
            "assigned": local["assigned"], "needs_scan": local["needs_scan"], "blocked": blocked, "error": local["error"],
        },
        "task": task,
    }).into())
}

impl Host {
    /// 图库的设置，连同本宿主强制关闭的项目。
    pub(super) fn library_settings(&self, library: &Library) -> Result<Json> {
        crate::app_settings::info(library, &self.library_forced_off(library))
    }
    /// 本宿主为该图库强制关闭的任务。
    pub(super) fn library_forced_off(&self, library: &Library) -> Vec<String> {
        forced_off(library, &self.startup)
    }
}

pub(super) fn handle(request: Request, host: &Host, received: u64) {
    let session = host.session.read().expect("http_host_get session lock poisoned");
    if !host.accepts(&session, received) {
        expired(request);
        return;
    }
    let path = request.url().split('?').next().unwrap_or(request.url());
    if crate::app_files::serves(path) {
        file(request, host, session);
        return;
    }
    if crate::http_read::remote(request.url()) || path == "/api/local-links" {
        remote_names(request, host, received, session);
        return;
    }
    if path == "/api/entity-preview" {
        preview(request, host, received, session);
        return;
    }
    if path == "/api/oc/groups" {
        drop(session);
        oc_groups(request, host, received);
        return;
    }
    if path == "/api/learning" {
        learning(request, host, received, session);
        return;
    }
    let result = read(&request, host, &session, path);
    // 请求持有自己的旧快照；套接字的背压不会阻塞切换或数据库命令。
    // 绝不为了发出这个响应而针对替换后的会话重新打开。
    drop(session);
    match result {
        Ok(Some((payload, status))) => respond(request, payload, status, "core"),
        Ok(None) => unavailable(request),
        Err(e) => failure(request, e, false),
    }
}
/// 在锁内解析旧会话的资产；解码和套接字写入不能拖慢切换。之后的请求处理从不重新打开替换后的图库。
fn file(request: Request, host: &Host, session: RwLockReadGuard<Session>) {
    let result = crate::app_files::prepare(&session.library, &host.root, request.url());
    drop(session);
    match result.and_then(|file| host.images.load(file)) {
        Ok(file) => {
            let _ = request.respond(crate::app_files::response(file));
        }
        Err(crate::app_files::Failure::Status(status)) => {
            respond(request, error("图片不存在或原图暂不可读"), status, "core")
        }
        Err(crate::app_files::Failure::Core(e)) => failure(request, e, false),
    }
}
/// 联网查询的名字，查询时不持有会话。
fn remote_names(request: Request, host: &Host, received: u64, session: RwLockReadGuard<Session>) {
    let mut source = session.names.source(&session.library);
    drop(session);
    let result = source
        .as_mut()
        .map_err(|_| Error::Invalid("名称服务配置不正确".into()))
        .and_then(|source| crate::app_names::remote(source, request.url()));
    reply_if_current(request, host, received, result, false);
}
/// Danbooru 角色的预览图（#293），查询和下载时不持有会话。Danbooru 上没有该角色的全年龄帖子时返回 404，
/// 页面随后保留占位图。
fn preview(request: Request, host: &Host, received: u64, session: RwLockReadGuard<Session>) {
    let mut source = session.names.source(&session.library);
    let thumbnails = session.library.thumbnails();
    drop(session);
    let tag = crate::http_read::query(request.url()).remove("tag").unwrap_or_default();
    let result = source
        .as_mut()
        .map_err(|_| Error::Invalid("名称服务配置不正确".into()))
        .and_then(|source| crate::app_name_sources::preview(source, &thumbnails, &tag));
    if !host.current(received) {
        expired(request);
        return;
    }
    match result {
        Ok(Some(bytes)) => {
            let _ = request.respond(crate::app_files::response(crate::app_files::File::jpeg(bytes)));
        }
        Ok(None) => respond(request, error("Danbooru 上没有这个角色的全年龄图"), 404, "core"),
        Err(e) => failure(request, e, false),
    }
}
fn oc_groups(request: Request, host: &Host, received: u64) {
    let mut session = host.session.write().expect("http_host_get session lock poisoned");
    if !host.accepts(&session, received) {
        expired(request);
        return;
    }
    let q = crate::http_read::query(request.url());
    let entity = q.get("entity").map(String::as_str).unwrap_or("");
    let c = crate::app_library::read_connection(&session.library.db());
    let entity = c.and_then(|c| crate::character_commands::resolve(&c, entity));
    let lib = session.library.clone();
    let result = entity.and_then(|entity| session.research.query(&lib, "oc", serde_json::json!({"entity":entity})));
    drop(session);
    reply_if_current(request, host, received, waited(result), false);
}
/// 撤销、确认角色这类写入引起的后台学习是否已经落定（#404）。会话锁下只取研究和同步的标志，放锁后再读数据库，
/// 从不在服务端等待。还没落定、研究也没在忙时，让主循环提前跑一次恢复检查，不用等到下一个整秒。
fn learning(request: Request, host: &Host, received: u64, session: RwLockReadGuard<Session>) {
    let learner_json = session.research.learner_state();
    let learner = crate::app_effects::Learner {
        sync: session.syncer.running(),
        error: learner_json.get("status").map(Json::text) == Some("error"),
        stopped: session.research.halted() || session.library.frozen(),
    };
    let ready = session.research.ready();
    let db = session.library.db();
    drop(session);
    let upto = crate::http_read::query(request.url()).remove("upto");
    let result = crate::app_effects::settled(&db, upto.as_deref()).map(|(upto, done)| {
        let state = crate::app_effects::learning_state(done, learner);
        if state == "pending" && ready {
            host.recover_soon();
        }
        Json::object([("upto", upto.into()), ("state", state.into()), ("learner", learner_json)])
    });
    reply_if_current(request, host, received, result, false);
}
/// URL 要的是进度（`progress=1`）而不是状态。
fn progress(request: &Request) -> bool {
    crate::http_read::query(request.url()).get("progress").is_some_and(|v| v == "1")
}
fn read(request: &Request, host: &Host, session: &Session, path: &str) -> Result<Option<(Json, u16)>> {
    match path {
        "/api/diagnostics" => Ok(Some((crate::diagnostics::report().into(), 200))),
        "/api/provenance" => session.provenance.info().map(|v| Some((v, 200))),
        // 上次检查时每个关注作品的覆盖情况（#561）；/api/index 的回复冻结，另开这个路由。
        "/api/index/coverage" => Ok(Some((session.index.coverage(), 200))),
        // 基础角色索引（#607）的状态，同样不并进 /api/index。
        "/api/index/base" => session.index.base_info(&session.library).map(|v| Some((v, 200))),
        "/api/index" => {
            if progress(request) {
                Ok(Some((session.index.progress(), 200)))
            } else {
                session.index.info(&session.library).map(|v| Some((v, 200)))
            }
        }
        "/api/similar" | "/api/clusters" => {
            crate::http_read::read(&session.context(&host.tags), request.url()).map(|value| {
                value.map(|(mut value, status)| {
                    if path == "/api/similar" {
                        value.set("state", session.research.similar_state());
                    } else {
                        value.set("learner", session.research.learner_state());
                    }
                    (value, status)
                })
            })
        }
        "/api/names" => {
            if progress(request) {
                Ok(Some((session.names.progress(), 200)))
            } else {
                session.names.status(&session.library).map(|v| Some((v, 200)))
            }
        }
        "/api/wd-extension" => host.wd_status(session).map(|v| Some((v.into(), 200))),
        "/api/sync" => session.syncer.status(&session.library).map(|p| Some((p, 200))),
        "/api/integrity" => Ok(Some((session.integrity(), 200))),
        // 全应用范围（#191）：检查更新的选项和上次结果，不属于图库。
        "/api/updates" => crate::app_updates::info().map(|p| Some((p, 200))),
        // 全应用范围（#168）：识别模型是否就位，以及正在进行的下载或导入。
        "/api/ccip-longtail" => ccip_status(host, session, None).map(|value| Some((value, 200))),
        // 两种数据包的签名目录（#602）：卡片上的版本、目录里有没有更新的版本、最近一次读目录的结果。
        "/api/packs" => packs(host, session).map(|value| Some((value, 200))),
        "/api/models" => crate::app_models::info(&host.root).map(|p| Some((p, 200))),
        "/api/onboarding" => {
            let works = crate::http_read::query(request.url()).get("works").is_some_and(|v| v == "1");
            crate::app_onboarding::Onboarding::read(session, &host.root, &session.context(&host.tags), works)
                .map(|p| Some((p, 200)))
        }
        // 用图动作（#450）：能不能拖出（只在桌面 App 里），以及最近一次导出到文件夹的状态。
        "/api/files" => Ok(Some((
            Json::object([
                ("drag", crate::app_desktop::available().into()),
                ("export", session.asset_exports.state(session.generation)),
            ]),
            200,
        ))),
        // 导入槽位属于应用：切换图库后，页面仍能读到上一次导入为什么停下。
        "/api/imports" => Ok(Some((host.imports.state(), 200))),
        "/api/human-conclusions" => Ok(Some((
            Json::object([("automatic", session.human_backups.status()), ("export", session.human_exports.state())]),
            200,
        ))),
        "/api/human-conclusions/report" => {
            let q = crate::http_read::query(request.url());
            let id = q.get("id").filter(|s| !s.is_empty()).ok_or_else(|| Error::invalid("需要报告编号"))?;
            let parse = |key: &str, default: usize| -> Result<usize> {
                q.get(key)
                    .map(|v| v.parse::<usize>().map_err(|_| Error::invalid("分页参数不正确")))
                    .unwrap_or(Ok(default))
            };
            let (offset, limit) = (parse("offset", 0)?, parse("limit", 100)?);
            if limit == 0 || limit > 500 || offset > illustash_core::human_conclusions::format::MAX_RECORDS {
                return Err(Error::invalid("每页须为 1–500 条，偏移不能超过格式记录上限"));
            }
            Ok(Some((session.human_exports.page(&session.library.db(), id, offset, limit)?, 200)))
        }
        path if crate::app_local::ROUTES.contains(&path) => {
            crate::app_local::read(&session.context(&host.tags), request.url()).map(|p| Some((p, 200)))
        }
        "/api/settings" => host.library_settings(&session.library).map(|mut p| {
            p.set("tasks", Json::object([("similar", session.research.similar_state())]));
            // 只有桌面外壳读的全应用设置（#451）：浏览器里打开的图库页没有这一节。
            if crate::app_desktop::available() {
                let after = crate::app_preferences::after_tasks();
                p.set("desktop", Json::object([(crate::app_preferences::AFTER_TASKS, after.into())]));
            }
            Some((p, 200))
        }),
        "/api/libraries" => libraries(host, session).map(|p| Some((p, 200))),
        _ if !crate::http_read::remote(request.url()) => {
            crate::http_read::read(&session.context(&host.tags), request.url())
        }
        _ => Ok(None),
    }
}
fn libraries(host: &Host, session: &Session) -> Result<Json> {
    let mut p = crate::app_libraries::info(&session.library)?;
    if session.damage().is_none() {
        let changes = crate::app_relocation::reports_if_free(&session.library);
        if !changes.is_empty() {
            p.set("folder_changes", serde_json::Value::Array(changes).into());
        }
    }
    p.set("export", host.exports.state(&session.library, session.generation));
    // 只在把它登记进图库列表失败时（#176）：页面提示一次；平常的应答不变。
    if let Some(why) = &session.unlisted {
        p.set("unlisted", why.as_str().into());
    }
    // 只在打开时做过升级之后（#193）：升级前的备份在哪里。
    if let Some(backup) = &session.upgraded {
        p.set("upgraded", Json::object([("backup", backup.as_str().into())]));
    }
    // 只在打开时抢救过 source.sqlite 之后（#228）：做了什么，损坏的文件移到了哪里。
    if let Some(report) = &session.rescued {
        p.set("rescued", report.clone());
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::{ccip_task_and_library, Json};
    use serde_json::json;
    use std::cell::Cell;

    #[test]
    fn ccip_terminal_task_never_pairs_with_precommit_library_state() {
        for (kind, before, after) in [("enable", 0, 2), ("rescan", 2, 3), ("withdraw", 3, 0), ("auto", 2, 3)] {
            let committed = Cell::new(false);
            let snapshot_task = || {
                if committed.get() {
                    Some(Json::object([("status", "done".into())]))
                } else if kind == "auto" {
                    None
                } else {
                    Some(Json::object([("status", "running".into())]))
                }
            };
            let (task, library) = ccip_task_and_library(snapshot_task, || {
                let images = before;
                // 模拟图库读到旧值后，worker 随即提交并发布终态。
                committed.set(true);
                Ok(json!({"images": images}))
            })
            .unwrap();
            let pending = if kind == "auto" { None } else { Some("running") };
            assert_eq!(task.as_ref().and_then(|t| t.get("status")).map(Json::text), pending, "{kind}");
            assert_eq!(library["images"], before, "{kind}");

            let (task, library) = ccip_task_and_library(snapshot_task, || Ok(json!({"images": after}))).unwrap();
            assert_eq!(task.as_ref().and_then(|t| t.get("status")).map(Json::text), Some("done"), "{kind}");
            assert_eq!(library["images"], after, "{kind}");
        }
    }
}
