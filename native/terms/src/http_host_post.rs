//! 会话 POST 业务路由；入口安全检查由 app_http 保持。
use super::http_host_get::{ccip_status, expired, packs, reply, reply_if_current, waited};
use super::*;
use std::sync::RwLockWriteGuard;

/// 图库包损坏时会话仍会应答的请求：它们都不写包里的数据库。
const DAMAGED_ALLOWS: [&str; 7] = [
    "/api/integrity/rescue",
    "/api/libraries/open",
    "/api/libraries/new",
    "/api/libraries/import",
    "/api/libraries/forget",
    "/api/backup",
    "/api/preferences",
];
type Translate<'a> = &'a dyn Fn(&str) -> String;

pub(super) fn handle(request: Request, host: &Host, received: u64, text: &str, body: Json) {
    // 原生选择窗口的提示文字用页面的语言（#173）。
    let language = header(&request, "X-Illustash-Language").map(str::to_owned);
    let translate = |text: &str| crate::app_preferences::tr(&host.root, language.as_deref(), text);
    match request.url() {
        "/api/libraries/choose" => return choose(request, host, received, &body, &translate),
        // 全应用范围，且可能要联网往返一次（#191）：从不持有会话，所以不会拖慢切换。
        "/api/updates" => return reply(request, crate::app_updates::post(&host.root, &body), true),
        "/api/diagnostics/open-logs" => {
            return reply(request, crate::app_desktop::request("POST", "/platform/diagnostics/open-logs", 5), false);
        }
        "/api/wd-extension/package" => return wd_package(request, host, received, body, &translate),
        "/api/models" => return models(request, host, body, &translate),
        "/api/ccip-longtail/package" => return ccip_package(request, host, received, body, &translate),
        "/api/packs" => return packs_post(request, host, received, body, &translate),
        url if url.starts_with("/api/files/") => return files(request, host, received, &body, &translate),
        _ => {}
    }
    let mut body = body;
    if request.url().starts_with("/api/human-conclusions/") {
        let allowed: &[&str] = match request.url() {
            "/api/human-conclusions/export" => &[],
            "/api/human-conclusions/export/cancel" => &["id"],
            "/api/human-conclusions/import" => &["path"],
            "/api/human-conclusions/restore" => &["backup_id"],
            _ => return unavailable(request),
        };
        if !matches!(&body,Json::Object(fields) if fields.iter().all(|(k,_)|allowed.contains(&k.as_str()))) {
            return failure(request, Error::invalid("人工结论请求字段不正确"), true);
        }
    }
    if request.url() == "/api/human-conclusions/import" && body.get("path").is_none() {
        match pick_file(host, received, &translate, "human-conclusions") {
            Picked::Path(path) => body.set("path", path),
            Picked::Cancelled => return cancelled(request),
            Picked::Switched => return expired(request),
            Picked::Failed(e) => return failure(request, e, true),
        }
    }
    if request.url() == "/api/accounts/x/import" && !body.get("path").is_some_and(truthy) {
        match archive(host, received, &translate) {
            Picked::Path(path) => body.set("path", path),
            Picked::Cancelled => return cancelled(request),
            Picked::Switched => return expired(request),
            Picked::Failed(e) => return failure(request, e, true),
        }
    }
    let mut session = host.session.write().expect("http_host_post session lock poisoned");
    if !host.accepts(&session, received) || header(&request, "X-CSRF-Token") != Some(session.token.as_str()) {
        return expired(request);
    }
    // 损坏的图库包不再接受写入（#212）：只允许离开它、备份 library.sqlite 和全应用范围的选项。
    if let Some(damage) = session.damage() {
        if !DAMAGED_ALLOWS.contains(&request.url()) {
            drop(session);
            return failure(request, damage.error(), true);
        }
    }
    match request.url() {
        "/api/human-conclusions/export" => {
            let result = wd_idle(&session)
                .and_then(|()| session.human_exports.start(&session.library, session.generation, &session.token));
            drop(session);
            return reply(request, result, true);
        }
        "/api/human-conclusions/export/cancel" => {
            let result = session.human_exports.cancel(body.get("id").map(Json::text).unwrap_or(""));
            drop(session);
            return reply(request, result, true);
        }
        "/api/human-conclusions/import" | "/api/human-conclusions/restore" => {
            let result = wd_idle(&session)
                .and_then(|()| human_import(host, &session, &body, request.url().ends_with("/restore")));
            drop(session);
            return reply(request, result, true);
        }
        "/api/cache" => return cache(request, host, session, &body),
        "/api/entities" | "/api/character/profile" | "/api/local-links" => {
            return profiles(request, host, received, session, &body)
        }
        "/api/names" if body.get("action").map(Json::text) == Some("model_api_test") => {
            let result = session.names.source(&session.library);
            drop(session);
            return reply_if_current(request, host, received, result.and_then(|s| crate::app_names::test(&s)), true);
        }
        "/api/relate" => return relate(request, host, received, session, &body),
        // 导入是后台任务（#26、#88）：请求做完校验、启动任务后立即应答；页面轮询 GET /api/imports，可以取消。
        // 识别结果导入始终绑定在开始时的图库上。取消、切换图库或退出，都会让两种导入在发布点之前停下。
        "/api/ccip-longtail/enable" | "/api/ccip-longtail/withdraw" => {
            let withdraw = request.url().ends_with("/withdraw");
            // 启用带 `version` 时用那个版本，就是换包（#602）。
            let version = match &body {
                Json::Object(fields) if fields.is_empty() => None,
                Json::Object(fields) if fields.len() == 1 && !withdraw => match body.get("version") {
                    Some(Json::Scalar(serde_json::Value::String(v))) if !v.is_empty() => Some(v.clone()),
                    _ => return failure(request, Error::invalid("参考包请求字段不正确"), true),
                },
                _ => return failure(request, Error::invalid("参考包请求字段不正确"), true),
            };
            let result = longtail(host, &session, withdraw, version.as_deref())
                .and_then(|started| ccip_status(host, &session, Some(started)));
            drop(session);
            return reply(request, result, true);
        }
        "/api/ccip-longtail/cancel" => {
            let Some(id) = body.get("id").map(Json::text).filter(|id| !id.is_empty()) else {
                return failure(request, Error::invalid("缺少参考包任务编号"), true);
            };
            let generation = session.generation;
            drop(session);
            let result = host.imports.cancel_ccip(generation, id).and_then(|()| ccip_status_current(host, received));
            return reply_if_current(request, host, received, result, true);
        }
        "/api/recognition/import" => {
            let started = wd_idle(&session).and_then(|()| recognition_import(host, &session, &body));
            drop(session);
            return reply(request, started.map(started_task), true);
        }
        "/api/libraries/import" => {
            let started = wd_idle(&session).and_then(|()| library_import(host, &session, &body));
            drop(session);
            return reply(request, started.map(started_task), true);
        }
        "/api/imports/cancel" => {
            drop(session);
            return respond(request, host.imports.cancel_import(), 200, "core");
        }
        "/api/tag"
        | "/api/favorite"
        | "/api/galleries"
        | "/api/similar/seen"
        | "/api/undo"
        | "/api/artist/verdict"
        | "/api/curate"
        | "/api/fold"
        | "/api/works"
        | "/api/character/mascot"
        | "/api/kind"
        | "/api/oc/shared"
        | "/api/change"
        | "/api/audit/answer"
        | "/api/ask/answer"
        | "/api/reassign"
        | "/api/sweep"
        | "/api/identity/decision"
        | "/api/identity/skip"
        | "/api/merge" => {
            let result = crate::app_commands::request(&session.context(&host.tags), request.url(), &body);
            drop(session);
            return page_write_reply(request, result);
        }
        _ => {}
    }
    match route(request.url(), host, received, &mut session, text, &body, language.as_deref()) {
        Some(result) => {
            drop(session);
            reply(request, result, true)
        }
        None => unavailable(request),
    }
}
/// 用图动作（#450，app_asset_files、app_asset_export）。页面只传资产 ID；先核对会话和 CSRF，再弹选择窗口或叫外壳，
/// 这期间不持有会话锁。这些请求不写图库，图库包损坏时照样应答。
fn files(request: Request, host: &Host, received: u64, body: &Json, translate: Translate) {
    let (library, generation) = {
        let session = host.session.read().expect("http_host_post session lock poisoned");
        if !host.accepts(&session, received) || header(&request, "X-CSRF-Token") != Some(session.token.as_str()) {
            drop(session);
            return expired(request);
        }
        (session.library.clone(), session.generation)
    };
    let id = || match body.get("id") {
        Some(Json::Scalar(serde_json::Value::String(id))) if !id.is_empty() => Ok(id.as_str()),
        _ => Err(Error::invalid("请选择图片")),
    };
    let result: Result<Json> = match request.url() {
        "/api/files/open" => id().and_then(|id| crate::app_asset_files::open(&library, id)).map(Into::into),
        "/api/files/reveal" => id().and_then(|id| crate::app_asset_files::reveal(&library, id)).map(Into::into),
        "/api/files/drag" => {
            let ids = match body.get("ids") {
                Some(Json::Array(ids)) => ids.iter().map(|v| v.text().to_owned()).filter(|v| !v.is_empty()).collect(),
                _ => Vec::new(),
            };
            crate::app_asset_files::drag(&library, &ids, |located| {
                crate::app_files::cached_icon(&library, &located.uuid, &located.path)
            })
            .map(Into::into)
        }
        "/api/files/export" => return export(request, host, received, body, translate),
        "/api/files/export/cancel" => {
            let session = host.session.read().expect("http_host_post session lock poisoned");
            Ok(session.asset_exports.cancel())
        }
        "/api/files/export/reveal" => {
            let destination = host
                .session
                .read()
                .expect("http_host_post session lock poisoned")
                .asset_exports
                .destination(generation);
            match destination {
                Some(folder) => crate::app_asset_files::show_folder(&folder).map(Into::into),
                None => Err(Error::invalid("没有导出过")),
            }
        }
        _ => return unavailable(request),
    };
    reply_if_current(request, host, received, result, true);
}
/// 选中的图导出到文件夹：先在选择窗口里选目录（不持有会话），再按页面的选择（ID 或筛选条件）开后台任务。
fn export(request: Request, host: &Host, received: u64, body: &Json, translate: Translate) {
    let group = match crate::app_asset_export::Group::parse(body.get("group").map(Json::text).unwrap_or("")) {
        Ok(group) => group,
        Err(e) => return failure(request, e, true),
    };
    let mut selection = body.clone();
    if let Json::Object(fields) = &mut selection {
        fields.retain(|(k, _)| k == "ids" || k == "filter" || k == "exclude");
    }
    // 导出只复制、不改图库：超过 500 张也不用再确认一次。
    selection.set("confirm", true.into());
    if host.session.read().expect("http_host_post session lock poisoned").asset_exports.running() {
        return failure(request, Error::invalid("正在导出，完成后再开始下一次"), true);
    }
    let destination = match pick_file(host, received, translate, "export") {
        Picked::Path(path) => PathBuf::from(path.text()),
        Picked::Cancelled => return cancelled(request),
        Picked::Switched => return expired(request),
        Picked::Failed(e) => return failure(request, e, true),
    };
    let session = host.session.read().expect("http_host_post session lock poisoned");
    if !host.accepts(&session, received) {
        drop(session);
        return expired(request);
    }
    let context = session.context(&host.tags);
    let ids = (|| {
        let c = crate::sqlite::open_with_flags(
            &context.db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )?;
        c.busy_timeout(std::time::Duration::from_secs(10))?;
        crate::http_read::register_names(&c)?;
        // 只导出选中的这几张，不带上它们的重复副本。
        crate::app_commands::selected_scope(&c, &context, &selection, false)
    })();
    let result = ids.and_then(|ids| {
        session.asset_exports.start(
            &session.library,
            session.generation,
            ids,
            group,
            translate(group.fallback()),
            destination,
        )
    });
    drop(session);
    reply(request, result, true);
}
fn cancelled(request: Request) {
    respond(request, Json::object([("cancelled", true.into())]), 200, "core");
}
fn started_task(task: Json) -> Json {
    Json::object([("started", true.into()), ("task", task)])
}
fn choose(request: Request, host: &Host, received: u64, body: &Json, translate: Translate) {
    let result = crate::app_platform::choose(
        body.get("kind").map(Json::text).unwrap_or(""),
        body.get("multiple").is_some_and(truthy),
        &crate::platform_process::stopping,
        translate,
    )
    .map_err(management_error);
    reply_if_current(request, host, received, result, true);
}
/// 全应用范围（#168）：模型下载在自己的线程上运行，切换图库后仍继续。不带路径的导入先打开原生选择窗口，
/// 期间不持有会话。
fn models(request: Request, host: &Host, mut body: Json, translate: Translate) {
    if body.get("action").map(Json::text) == Some("import") && body.get("paths").is_none() {
        match model_paths("models", translate) {
            Ok(Some(paths)) => body.set("paths", paths),
            Ok(None) => return cancelled(request),
            Err(e) => return failure(request, e, true),
        }
    }
    // 本会话的识别或角色索引正在使用模型时，删除模型会被拒绝（`app_models::post` 提示等识别结束再删）。
    let in_use = body.get("action").map(Json::text) == Some("delete") && {
        let session = host.session.read().expect("http_host_post session lock poisoned");
        session.syncer.running() || session.index.writing()
    };
    reply(request, crate::app_models::post(&host.root, &body, in_use), true);
}
fn wd_package(request: Request, host: &Host, received: u64, mut body: Json, translate: Translate) {
    if body.get("action").map(Json::text) == Some("import") && body.get("paths").is_none() {
        if !matches!(&body, Json::Object(fields) if fields.len() == 1) {
            return failure(request, Error::invalid("未知参数"), true);
        }
        match model_paths("wd-extension", translate) {
            Ok(Some(paths)) => body.set("paths", paths),
            Ok(None) => return cancelled(request),
            Err(e) => return failure(request, e, true),
        }
    }
    // 包文件全应用共用，同一宿主里的并发命令按序交给独立下载槽。
    let session = host.session.write().expect("WD 角色扩展包会话锁中毒");
    if !host.accepts(&session, received) || header(&request, "X-CSRF-Token") != Some(&session.token) {
        return expired(request);
    }
    let result = crate::app_wd_extension::card_pack(&host.root, &session.library)
        .and_then(|current| crate::app_models::extension_post(&host.root, &body, &current))
        .and_then(|()| host.wd_status(&session))
        .map(Json::from);
    drop(session);
    reply(request, result, true);
}

fn wd_extension(url: &str, host: &Host, session: &mut Session, body: &Json) -> Result<Json> {
    use crate::app_wd_extension::Action;
    if url.ends_with("/cancel") {
        let Some(Json::Scalar(serde_json::Value::String(id))) = body.get("id") else {
            return Err(Error::invalid("需要 WD 角色扩展任务编号"));
        };
        if id.is_empty() || !matches!(body, Json::Object(fields) if fields.len() == 1) {
            return Err(Error::invalid("WD 角色扩展任务编号不合法"));
        }
        session.wd_extension.cancel(id)?;
        return host.wd_status(session).map(Json::from);
    }
    // 启用和重新检查用卡片上的版本；带 `version` 时用那个版本，就是换包（#602）。撤回不带字段。
    let version = match body {
        Json::Object(fields) if fields.is_empty() => None,
        Json::Object(fields) if fields.len() == 1 && url.ends_with("/enable") => match body.get("version") {
            Some(Json::Scalar(serde_json::Value::String(v))) if !v.is_empty() => Some(v.clone()),
            _ => return Err(Error::invalid("WD 角色扩展请求字段不正确")),
        },
        _ => return Err(Error::invalid("WD 角色扩展请求字段不正确")),
    };
    if session.wd_extension.running() {
        return Err(wd_running());
    }
    let mut snapshot = host.wd_status(session)?;
    match snapshot["library"]["blocked"].as_str() {
        Some("frozen") => return Err(Error::Conflict("图库只读".into())),
        Some("syncing") => return Err(Error::Conflict("正在同步或交付识别结果，请等它结束".into())),
        Some("busy") => return Err(Error::Conflict("图库正在导入、导出或更新角色索引，请等它结束".into())),
        Some("no_vectors") if url.ends_with("/enable") => {
            return Err(Error::Conflict("这个图库没有已保存的 WD 向量".into()));
        }
        _ => {}
    }
    let action = if url.ends_with("/withdraw") {
        Action::Withdraw
    } else {
        let pack = match &version {
            Some(version) => crate::app_packs::by_version(&host.root, version)?.wd,
            None => crate::app_wd_extension::card_pack(&host.root, &session.library)?,
        };
        let path = crate::app_models::extension_file(&host.root, &pack)?
            .ok_or_else(|| Error::Conflict("扩展包还没就位".into()))?;
        Action::Enable(Box::new(pack), path)
    };
    let identity = crate::app_tasks::TaskIdentity::new(&session.library, session.generation, &session.token)?;
    snapshot["task"] = session.wd_extension.start_for(&session.library, identity, action)?;
    Ok(snapshot.into())
}

/// `POST /api/packs`（#602）：一个角色资料包的文件操作和开关。
/// - 文件：`download`（可带 `version`）、`import`（可带 `paths`，不带时打开选择框）、`cancel`（不带 `id`）、`discard`、
///   `delete`，见 `app_models::character_pack_post`；
/// - 开关：`enable`（可带 `version`，就是换包）用两个组成部分依次检查本图库，`withdraw` 撤回启用了的组成部分，
///   `cancel` 带 `id` 取消检查（只在扫描阶段）；`decline` 记下已有图库不要默认启用（`app_packs::decline`）。
///
/// 成功时回复完整的 `GET /api/packs`；选择框关掉时回复 `{cancelled:true}`。
fn packs_post(request: Request, host: &Host, received: u64, mut body: Json, translate: Translate) {
    let Json::Object(fields) = &body else { return failure(request, Error::invalid("未知操作"), true) };
    let action = body.get("action").map(Json::text).unwrap_or("").to_owned();
    if action == "import" && body.get("paths").is_none() {
        if fields.len() != 1 {
            return failure(request, Error::invalid("未知参数"), true);
        }
        match model_paths("wd-extension", translate) {
            Ok(Some(paths)) => body.set("paths", paths),
            Ok(None) => return cancelled(request),
            Err(e) => return failure(request, e, true),
        }
    }
    let mut session = host.session.write().expect("角色资料包会话锁中毒");
    if !host.accepts(&session, received) || header(&request, "X-CSRF-Token") != Some(&session.token) {
        return expired(request);
    }
    let result = pack_action(host, &mut session, &action, &body).and_then(|()| packs(host, &session));
    drop(session);
    reply(request, result, true);
}

fn pack_action(host: &Host, session: &mut Session, action: &str, body: &Json) -> Result<()> {
    use crate::app_wd_extension::Action;
    let Json::Object(fields) = body else { return Err(Error::invalid("未知操作")) };
    let only = |allowed: &[&str]| fields.iter().all(|(k, _)| allowed.contains(&k.as_str()));
    let lib = crate::app_packs::for_library(&host.root, &session.library)?;
    match action {
        "cancel" if body.get("id").is_some() => {
            let Some(Json::Scalar(serde_json::Value::String(id))) = body.get("id") else {
                return Err(Error::invalid("角色资料包任务编号不合法"));
            };
            if id.is_empty() || !only(&["action", "id"]) {
                return Err(Error::invalid("角色资料包任务编号不合法"));
            }
            return session.wd_extension.cancel(id);
        }
        "download" | "import" | "cancel" | "discard" | "delete" => {
            if action == "delete"
                && (session.wd_extension.running() || host.imports.recognition_running(session.generation))
            {
                return Err(Error::Conflict("正在检查角色，请等任务结束后再删除角色资料包".into()));
            }
            return crate::app_models::character_pack_post(&host.root, body, &lib.card);
        }
        "decline" if only(&["action"]) => return crate::app_packs::decline(&host.root, &session.library),
        "enable" if only(&["action", "version"]) => {}
        "withdraw" if only(&["action"]) => {}
        "decline" | "enable" | "withdraw" => return Err(Error::invalid("角色资料包请求字段不正确")),
        _ => return Err(Error::invalid("未知的角色资料包操作")),
    }
    if session.library.frozen() || session.damage().is_some() {
        return Err(Error::Conflict("图库只读".into()));
    }
    if session.wd_extension.running() {
        return Err(wd_running());
    }
    match host.wd_busy(session) {
        Some("syncing") => return Err(Error::Conflict("正在同步或交付识别结果，请等它结束".into())),
        Some(_) => return Err(Error::Conflict("图库正在导入、导出或更新角色索引，请等它结束".into())),
        None => {}
    }
    let action = if action == "withdraw" {
        Action::PackWithdraw { wd: lib.wd.is_some() || lib.ccip.is_none(), ccip: lib.ccip.is_some() }
    } else {
        let pack = match body.get("version") {
            None => lib.card.clone(),
            Some(Json::Scalar(serde_json::Value::String(v))) if !v.is_empty() => {
                crate::app_packs::by_version(&host.root, v)?
            }
            Some(_) => return Err(Error::invalid("角色资料包请求字段不正确")),
        };
        crate::app_packs::enable_action(&host.root, &session.library, &pack)?
    };
    let identity = crate::app_tasks::TaskIdentity::new(&session.library, session.generation, &session.token)?;
    session.wd_extension.start_for(&session.library, identity, action)?;
    crate::app_ccip_longtail::clear_auto_task(&session.library);
    Ok(())
}

fn wd_idle(session: &Session) -> Result<()> {
    if session.wd_extension.running() {
        return Err(wd_running());
    }
    Ok(())
}

fn wd_running() -> Error {
    Error::Conflict("WD 角色扩展任务正在进行，请等它结束".into())
}

fn model_paths(kind: &str, translate: Translate) -> Result<Option<Json>> {
    let picked = crate::app_platform::choose(kind, true, &crate::platform_process::stopping, translate)
        .map_err(management_error)?;
    Ok(picked.get("paths").filter(|v| matches!(v, Json::Array(list) if !list.is_empty())).cloned())
}
fn ccip_status_current(host: &Host, received: u64) -> Result<Json> {
    let session = host.session.read().expect("http_host_post session lock poisoned");
    if !host.accepts(&session, received) {
        return Err(Error::Conflict("图库已切换，请重新加载页面".into()));
    }
    ccip_status(host, &session, None)
}
fn ccip_card(host: &Host, received: u64) -> Result<crate::app_packs::Pack> {
    let session = host.session.read().expect("http_host_post session lock poisoned");
    if !host.accepts(&session, received) {
        return Err(Error::Conflict("图库已切换，请重新加载页面".into()));
    }
    crate::app_ccip_longtail::card_pack(&host.root, &session.library)
}
fn ccip_package(request: Request, host: &Host, received: u64, mut body: Json, translate: Translate) {
    if body.get("action").map(Json::text) == Some("import") && body.get("paths").is_none() {
        // 参考包是一个 ZIP：单选，只列 .zip（#559 走查：原来借用识别模型的选择器，能多选 .onnx、.json）。
        let picked = crate::app_platform::choose("ccip-longtail", false, &crate::platform_process::stopping, translate)
            .map_err(management_error);
        match picked.map(|p| p.get("path").filter(|p| !p.text().is_empty()).cloned()) {
            Ok(Some(path)) => body.set("paths", Json::Array(vec![path])),
            Ok(None) => return cancelled(request),
            Err(e) => return failure(request, e, true),
        }
    }
    let in_use = {
        let session = host.session.read().expect("http_host_post session lock poisoned");
        session.syncer.running() || host.imports.recognition_running(session.generation)
    };
    let result = ccip_card(host, received)
        .and_then(|current| crate::app_models::longtail_package_post(&host.root, &body, in_use, &current))
        .and_then(|()| ccip_status_current(host, received));
    reply_if_current(request, host, received, result, true);
}
enum Picked {
    Path(Json),
    Cancelled,
    /// 选择窗口打开期间会话已切换。
    Switched,
    Failed(Error),
}
/// 用户为 X 导入选的归档文件。原生选择窗口运行时不持有会话；取消或切换之后，选中的文件绝不能交给替换后的图库包。
fn archive(host: &Host, received: u64, translate: Translate) -> Picked {
    pick_file(host, received, translate, "archive")
}
fn pick_file(host: &Host, received: u64, translate: Translate, kind: &str) -> Picked {
    let picked = match crate::app_platform::choose(kind, false, &crate::platform_process::stopping, translate) {
        Ok(picked) => picked,
        Err(e) => return Picked::Failed(management_error(e)),
    };
    if !host.current(received) {
        return Picked::Switched;
    }
    match picked.get("path").filter(|path| truthy(path)) {
        Some(path) => Picked::Path(path.clone()),
        None => Picked::Cancelled,
    }
}
fn human_import(host: &Host, session: &Session, body: &Json, restore: bool) -> Result<Json> {
    let (path, expected) = if restore {
        let id = body.get("backup_id").map(Json::text).unwrap_or("");
        let (path, hash) = session.human_backups.restore_path(&session.library.db(), id)?;
        (path, Some((id.to_owned(), hash)))
    } else {
        let Some(Json::Scalar(serde_json::Value::String(path))) = body.get("path") else {
            return Err(Error::invalid("需要本机文件 path"));
        };
        if path.is_empty() {
            return Err(Error::invalid("需要本机文件 path"));
        }
        (expand_path(path)?, None)
    };
    let identity = crate::app_tasks::TaskIdentity::new(&session.library, session.generation, &session.token)?;
    let library = session.library.clone();
    host.imports.start(identity, crate::app_imports::Kind::HumanConclusions, "", move |progress| {
        crate::app_human_conclusions::import_observed(&library.db(), library.id(), &path, progress, expected)
    })
}
fn cache(request: Request, host: &Host, session: RwLockWriteGuard<Session>, body: &Json) {
    if !matches!(body.get("action"), Some(Json::Scalar(serde_json::Value::String(action))) if action == "clear") {
        drop(session);
        return failure(request, Error::Invalid("未知操作".into()), true);
    }
    // 在请求解析和会话校验之后才绑定图库；等待图片处理时不能持有会话锁，切换之后也不能让这个操作改指别的图库。
    let library = session.library.clone();
    drop(session);
    reply(request, host.images.clear(&library, &host.library_forced_off(&library)), true);
}
/// 角色资料的写入：先在不持会话锁时查询它们的作品，再在锁内写入。
fn profiles(request: Request, host: &Host, received: u64, session: RwLockWriteGuard<Session>, body: &Json) {
    let library = session.library.clone();
    let mut context = session.context(&host.tags);
    context.db = library.instance_path().join("library.sqlite");
    context.source = Some(library.instance_path().join("source.sqlite"));
    let source = session.names.source(&library);
    drop(session);
    let prepared = source.and_then(|mut source| {
        let c = crate::app_library::read_connection(&context.db)?;
        crate::app_profiles::prepare(&c, body, request.url(), &mut source)
    });
    let session = host.session.write().expect("http_host_post session lock poisoned");
    if !host.accepts(&session, received) {
        return expired(request);
    }
    let result = prepared.and_then(|p| crate::app_commands::request_prepared(&context, request.url(), body, Some(&p)));
    drop(session);
    page_write_reply(request, result);
}

/// 这些页面命令返回的 SQLITE_BUSY 都在提交前出现，后台跟进和撤销完成后的读取各自报告错误。
fn page_write_reply(request: Request, result: Result<Json>) {
    use crate::diagnostics::{self, Cause, Publication, Retry, Stage};
    if let Err(Error::Sql(rusqlite::Error::SqliteFailure(code, _))) = &result {
        if code.code == rusqlite::ErrorCode::DatabaseBusy {
            let diagnostic = diagnostics::record(
                diagnostics::operation(request.url()),
                Stage::Write,
                Cause::Database,
                Some(code.extended_code),
                Publication::NotCommitted,
                Retry::Retry,
            );
            return respond_failure(request, error("服务繁忙，请稍后重试"), 503, Some(diagnostic));
        }
    }
    reply(request, result, true);
}
fn relate(request: Request, host: &Host, received: u64, mut session: RwLockWriteGuard<Session>, body: &Json) {
    let value = serde_json::from_str::<serde_json::Value>(&body.dump()).unwrap_or_default();
    let ids = value["ids"].as_array();
    if ids.is_none_or(|ids| {
        ids.iter().any(|v| !v.is_string())
            || !(2..=12).contains(
                &ids.iter()
                    .map(|v| v.as_str().expect("validated internal text field"))
                    .collect::<std::collections::BTreeSet<_>>()
                    .len(),
            )
    }) {
        drop(session);
        return failure(request, Error::Invalid("请选择 2–12 张图".into()), true);
    }
    let lib = session.library.clone();
    let result = session.research.query(&lib, "relate", value);
    drop(session);
    reply_if_current(request, host, received, waited(result), true);
}
fn longtail(host: &Host, session: &Session, withdraw: bool, version: Option<&str>) -> Result<Json> {
    use crate::app_ccip_longtail;
    if session.library.frozen() {
        return Err(Error::Conflict("图库只读".into()));
    }
    wd_idle(session)?;
    if session.syncer.running() || session.index.writing() {
        return Err(Error::Conflict("正在同步或更新角色索引，请稍后重试".into()));
    }
    let (kind, path) = if withdraw {
        (crate::app_imports::Kind::CcipWithdraw, None)
    } else {
        let pack = match version {
            Some(version) => crate::app_packs::by_version(&host.root, version)?.ccip,
            None => app_ccip_longtail::card_pack(&host.root, &session.library)?,
        };
        let shared = crate::app_resources::shared_path(&host.root)?;
        let path = app_ccip_longtail::package_path(&shared, &pack);
        if !path.try_exists()? {
            return Err(Error::Conflict("参考包还没就位，请先从文件导入".into()));
        }
        if app_ccip_longtail::stored_vectors(&session.library)? == 0 {
            return Err(Error::Conflict("这个图库还没有 CCIP 向量，请先识别图片".into()));
        }
        (crate::app_imports::Kind::CcipEnable, Some((pack, path)))
    };
    let identity = crate::app_tasks::TaskIdentity::new(&session.library, session.generation, &session.token)?;
    let (library, root) = (session.library.clone(), host.root.clone());
    let started = host.imports.start(identity, kind, "", move |progress| {
        let value = match path {
            Some((pack, path)) => app_ccip_longtail::apply(
                &library,
                &root,
                &pack,
                &path,
                &|| crate::app_sync::check(progress.cancellation()),
                progress,
            )?,
            None => app_ccip_longtail::withdraw(&library, progress)?,
        };
        Ok(value.into())
    })?;
    app_ccip_longtail::clear_auto_task(&session.library);
    Ok(started)
}

fn recognition_import(host: &Host, session: &Session, body: &Json) -> Result<Json> {
    let resume = matches!(body.get("resume"), Some(Json::Scalar(serde_json::Value::Bool(true))));
    let path = match body.get("path") {
        Some(Json::Scalar(serde_json::Value::String(path))) => Some(PathBuf::from(path)),
        _ => None,
    };
    if session.index.writing() {
        return Err(Error::Invalid("角色索引正在更新，请稍后".into()));
    }
    if !resume && path.is_none() {
        return Err(Error::Invalid("需要结果包目录 path".into()));
    }
    let identity = crate::app_tasks::TaskIdentity::new(&session.library, session.generation, &session.token)?;
    let file = path
        .as_deref()
        .filter(|_| !resume)
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (library, root) = (session.library.clone(), host.root.clone());
    host.imports.start(identity, crate::app_imports::Kind::Recognition, &file, move |progress| {
        use crate::app_imports::Observer;
        let cancel = progress.cancellation();
        let out = match path.filter(|_| !resume) {
            Some(path) => crate::app_recognition::import_package_observed(
                &library,
                &path,
                &root,
                cancel,
                &crate::app_research::dispatch,
                progress,
            )?,
            None => {
                progress.stage("projecting");
                crate::app_recognition::resume_at(&library, &root, cancel, &crate::app_research::dispatch)?
            }
        };
        // 保持旧版响应的插入顺序（digest、duplicate、status、error）。
        Ok(Json::Object(
            ["digest", "duplicate", "status", "error"]
                .iter()
                .filter_map(|k| out.get(*k).map(|v| ((*k).to_string(), v.clone().into())))
                .collect(),
        ))
    })
}
fn library_import(host: &Host, session: &Session, body: &Json) -> Result<Json> {
    let archive = expand_path(body.get("path").map(Json::text).unwrap_or(""))?;
    let identity = crate::app_tasks::TaskIdentity::new(&session.library, session.generation, &session.token)?;
    let file = archive.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    host.imports.start(identity, crate::app_imports::Kind::Archive, &file, move |progress| {
        let (path, name) = crate::app_archives::import(&archive, &progress.cancellation_arc(), progress)?;
        // 已发布：登记失败时保留完整的图库包，并随结果一起报告，与新建相同（#97、#111）；
        // 再导入一次只会多出第二个带编号的包。
        let listed = crate::app_libraries::remember_created(&path, &name);
        let mut out = Json::object([("path", path.to_string_lossy().as_ref().into()), ("name", name.into())]);
        if let Err(error) = listed {
            out.set("listed", false.into());
            out.set("error", crate::app_libraries::unlisted_reason(&error).into());
        }
        Ok(out)
    })
}
/// 释放会话锁之后才应答的路由；未知路由返回 None。
fn route(
    url: &str,
    host: &Host,
    received: u64,
    session: &mut Session,
    text: &str,
    body: &Json,
    language: Option<&str>,
) -> Option<Result<Json>> {
    Some(match url {
        "/api/provenance" => session.provenance.post(body),
        "/api/onboarding" => {
            let library = session.library.clone();
            let works = body.get("works").is_some_and(truthy);
            session.onboarding.post(&library, body).and_then(|()| {
                crate::app_onboarding::Onboarding::read(session, &host.root, &session.context(&host.tags), works)
            })
        }
        route if route.starts_with("/api/accounts/") => {
            crate::app_accounts::action(&session.library, &route["/api/accounts/".len()..], body)
        }
        "/api/index" => {
            let library = session.library.clone();
            let busy = session.syncer.running() || session.wd_extension.running();
            if host.imports.recognition_running(session.generation)
                && matches!(body.get("action").map(Json::text), Some("apply" | "drop"))
            {
                Err(Error::Invalid("识别结果正在导入，完成后再更新角色索引".into()))
            } else {
                session.index.post(&library, body, busy)
            }
        }
        "/api/index/base" => {
            let library = session.library.clone();
            let busy = session.syncer.running() || session.wd_extension.running();
            if host.imports.recognition_running(session.generation) {
                Err(Error::Invalid("识别结果正在导入，完成后再更新角色索引".into()))
            } else {
                session.index.base_post(&library, body, busy)
            }
        }
        "/api/names" => {
            let library = session.library.clone();
            session.names.post(&library, body)
        }
        "/api/wd-extension/enable" | "/api/wd-extension/withdraw" | "/api/wd-extension/cancel" => {
            wd_extension(url, host, session, body)
        }
        "/api/sync" => sync(host, received, session),
        "/api/similar" | "/api/regroup" => research(url, session, body),
        // 只读，所以其中的名字和其他读取一样跟随页面语言（app_http.rs）。
        "/api/suggest" => crate::http_read::PageLanguage::set(language)
            .with(|| crate::http_local::suggest(&session.context(&host.tags), body)),
        "/api/startup" => crate::app_settings::set_startup_json(&session.library, text)
            .and_then(|_| host.library_settings(&session.library)),
        // 全应用范围，所以不属于 /api/settings（其应答保持录制时的样子）；页面从 /i18n.js 读取。
        "/api/preferences" => {
            let locales = crate::app_preferences::locales(&crate::app_preferences::locales_folder(&host.root));
            crate::app_preferences::set_json(text, &locales)
                .map(|_| Json::object([("preferences", crate::app_preferences::info(&locales))]))
        }
        "/api/backup" => crate::app_settings::backup(&session.library)
            .map(|path| Json::object([("path", path.to_string_lossy().as_ref().into())])),
        "/api/libraries/new" => create_library(host, body),
        "/api/libraries/sources/add"
        | "/api/libraries/sources/remove"
        | "/api/libraries/sources/move"
        | "/api/libraries/sources/recognize"
        | "/api/libraries/sources/undo" => sources(url, received, session, body).map_err(management_error),
        "/api/libraries/preset" => crate::app_settings::preset(
            &mut session.library,
            body.get("source").map(Json::text).unwrap_or(""),
            body.get("preset").map(Json::text).unwrap_or(""),
        )
        .map_err(management_error),
        "/api/libraries/forget" => forget(session, body),
        "/api/libraries/export" => {
            wd_idle(session).and_then(|()| host.exports.start(&session.library, session.generation))
        }
        "/api/integrity" => Ok(session.check_now()),
        "/api/integrity/rescue" => crate::app_create::optional_reference(&host.reference)
            .and_then(|reference| session.start_rescue(reference.map(Path::to_owned))),
        "/api/libraries/open" => open(host, received, session, body),
        _ => return None,
    })
}
fn sync(host: &Host, received: u64, session: &mut Session) -> Result<Json> {
    // 角色索引在写入、WD 扩展检查在跑：这次同步排在它们后面，不报错（#607）。首次准备时模型一到齐，宿主就为图库启用
    // 基础角色索引，页面同时发起的首次同步正好撞上。
    if session.sync_waits() {
        let library = session.library.clone();
        session.syncer.queue(&library)?;
        return Ok(Json::object([("started", false.into()), ("queued", true.into())]));
    }
    if host.imports.recognition_running(session.generation) {
        return Err(Error::Invalid("识别结果正在导入，请稍后".into()));
    }
    let library = session.library.clone();
    let token = session.token.clone();
    session.syncer.start(&library, received, &token).map(|started| Json::object([("started", started.into())]))
}
/// 相似扫描（/api/similar）或重新分组（/api/regroup）：启动或取消。
fn research(url: &str, session: &mut Session, body: &Json) -> Result<Json> {
    if url == "/api/similar" && !session.research.similar_enabled {
        return Err(Error::Invalid("相似扫描已关闭".into()));
    }
    let lib = session.library.clone();
    if body.get("action").map(Json::text) == Some("cancel") {
        session.research.cancel();
        return Ok(Json::object([("cancelled", true.into())]));
    }
    let kind = if url == "/api/similar" { "similar" } else { "regroup" };
    session.research.start(&lib, kind, serde_json::json!({}), None).map(|_| Json::object([("started", true.into())]))
}
fn sources(url: &str, received: u64, session: &mut Session, body: &Json) -> Result<Json> {
    wd_idle(session)?;
    // 进行中的同步持有一份固定的来源副本；只在两次同步之间修改来源。
    if session.syncer.running() || session.index.writing() {
        return Err(Error::Invalid("正在同步或更新角色索引，请稍后再修改来源".into()));
    }
    let mut result = if url == "/api/libraries/sources/add" {
        add_sources(received, session, body)?
    } else if url != "/api/libraries/sources/remove" {
        let action = url.rsplit('/').next().expect("来源操作有末段");
        let field = match action {
            "move" => "path",
            "undo" => "token",
            _ => "digest",
        };
        let argument = body.get(field).map(Json::text).unwrap_or("");
        let result = crate::app_sources::relocation(
            &mut session.library,
            action,
            body.get("source").map(Json::text).unwrap_or(""),
            argument,
        )?;
        if action == "recognize" && argument.is_empty() {
            return Ok(result);
        }
        result
    } else {
        let confirm = body.get("confirm").is_some_and(truthy);
        let result = crate::app_sources::remove(
            &mut session.library,
            body.get("source").map(Json::text).unwrap_or(""),
            confirm,
        )?;
        if !confirm {
            return Ok(result);
        }
        result
    };
    // 新来源立即扫描；移除的来源的图片随这次同步离开浏览数据库。
    let (library, token) = (session.library.clone(), session.token.clone());
    result.set("sync", session.syncer.start(&library, received, &token).unwrap_or(false).into());
    Ok(result)
}
/// 一次选中的多个文件夹共用一个出处和一次同步：用 `paths`，或单个 `path`。
fn add_sources(received: u64, session: &mut Session, body: &Json) -> Result<Json> {
    let raw: Vec<&str> = match body.get("paths") {
        Some(Json::Array(items)) => items.iter().map(Json::text).collect(),
        _ => vec![body.get("path").map(Json::text).unwrap_or("")],
    };
    let (kind, preset) =
        (body.get("kind").map(Json::text).unwrap_or(""), body.get("preset").map(Json::text).unwrap_or(""));
    let mut added = Vec::new();
    for path in raw {
        let path = if path.is_empty() { PathBuf::new() } else { crate::app_paths::resolve(&expand_path(path)?)? };
        let path = path.to_str().ok_or_else(|| Error::Invalid("来源路径不是 UTF-8".into()))?;
        match crate::app_sources::add(&mut session.library, kind, path, preset) {
            Ok(source) => added.push(source),
            Err(error) => {
                // 被拒绝的文件夹之前已加上的保持加上，并立即扫描。
                if !added.is_empty() {
                    let (library, token) = (session.library.clone(), session.token.clone());
                    let _ = session.syncer.start(&library, received, &token);
                }
                return Err(error);
            }
        }
    }
    Ok(if added.len() == 1 {
        added.pop().expect("one added source")
    } else {
        Json::object([("added", Json::Array(added))])
    })
}
fn forget(session: &Session, body: &Json) -> Result<Json> {
    let raw =
        body.get("path").map(Json::text).filter(|s| !s.is_empty()).ok_or_else(|| Error::Invalid("无效路径".into()))?;
    if Path::new(raw) == session.library.display_path() {
        return Err(Error::Invalid("不能移出当前打开的图库".into()));
    }
    crate::app_libraries::forget(&expand_path(raw)?).map_err(management_error)?;
    Ok(Json::object([("ok", true.into())]))
}
fn open(host: &Host, received: u64, session: &mut Session, body: &Json) -> Result<Json> {
    let path =
        body.get("path")
            .and_then(|value| {
                if let Json::Scalar(serde_json::Value::String(path)) = value {
                    Some(path.as_str())
                } else {
                    None
                }
            })
            .filter(|v| !v.is_empty())
            .ok_or_else(|| Error::Invalid("不是图库包".into()))?;
    let path = expand_path(path)?;
    if !path.join("library.json").is_file() {
        return Err(Error::Invalid("不是图库包".into()));
    }
    // 会话自己的图库包，抢救出的 source.sqlite 已就绪（#228）：在重新打开时换上。
    let own = crate::app_paths::canonical(&path).is_ok_and(|p| p == session.library.instance_path());
    if own && session.rescue.ready() {
        return host.reopen_rescued(session, received);
    }
    // 另一个经检查发现 source.sqlite 损坏的图库包：用户同意后先抢救。
    if !own && body.get("rescue").is_some_and(truthy) {
        let reference = crate::app_create::optional_reference(&host.reference)?;
        let report = crate::app_service::rescue_package(&path, None, reference, &|_, _| {})?.json();
        let mut out = host.switch_locked(session, received, &path)?;
        session.rescued = Some(report.clone());
        out.set("rescued", report);
        return Ok(out);
    }
    // 旧格式的图库包要等用户同意升级后才打开（#193）；只在打开失败后才去检查它，当前格式的图库因此没有额外开销。
    match host.switch_locked(session, received, &path) {
        Err(Error::Schema(why)) => match crate::app_upgrade::needed(&path) {
            None => Err(crate::app_upgrade::pre_v8(&path).map_or(Error::Schema(why), Error::Invalid)),
            Some(_) if !body.get("upgrade").is_some_and(truthy) => {
                Err(Error::Conflict(crate::app_upgrade::NEEDED.into()))
            }
            Some(prepared) => {
                // 按实际发生的情况说明：未提交（图库包保持原样），或已提交并附上备份位置。
                let report =
                    crate::app_upgrade::run(&prepared, None).map_err(|failure| Error::Invalid(failure.to_string()))?;
                let backup = crate::app_upgrade::shown(&report.backup);
                drop(report);
                let mut out = host.switch_locked(session, received, &path)?;
                session.upgraded = Some(backup.clone());
                out.set("upgraded", Json::object([("backup", backup.as_str().into())]));
                Ok(out)
            }
        },
        other => other,
    }
}
