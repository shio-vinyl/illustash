//! 全应用共用的凭据与桌面登录；显式的账号操作只授权它自己的验证请求。保存凭据从不授权后台 I/O。
use crate::{app_library::Library, http_json::Json, Error, Result};
use serde_json::{json, Value};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

static ACCOUNT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub const PIXIV_EXPIRED: &str = "Pixiv 登录已过期，请到设置页「绑定的账号」重新登录";
pub const PIXIV_NO_WINDOW: &str = "Pixiv 登录在桌面 app 的登录窗口里，现在不是从桌面 app 启动的";
/// Pixiv 查询出了什么问题，供要重试或把它当作登录失效的调用方使用（#204）。
/// 调用方按这个分支，不按文字分支；文字留在错误里。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pixiv {
    /// Pixiv 不接受这个登录（HTTP 401，或响应正文这样说）。
    Refused,
    /// 桌面 app 的登录窗口里没有 Pixiv 登录。
    NoLogin,
    /// 请求在途中失败，重试可能成功。
    Failed,
    /// 从桌面 app 登录窗口发出的请求失败；窗口可能还在加载。
    WindowFailed,
    /// 其他情况，包括调用桌面 app 失败（`app_desktop::unanswered`）。
    Other,
}
#[derive(Debug)]
struct PixivError {
    kind: Pixiv,
    error: Error,
}
impl PixivError {
    fn new(kind: Pixiv, message: impl Into<String>) -> Self {
        PixivError { kind, error: Error::invalid(message) }
    }
    /// 桌面 app 没有应答或拒绝了调用。
    fn unanswered(&self) -> bool {
        matches!(&self.error, Error::Invalid(m) if crate::app_desktop::unanswered(m))
    }
}
impl From<Error> for PixivError {
    fn from(error: Error) -> Self {
        PixivError { kind: Pixiv::Other, error }
    }
}
impl From<PixivError> for Error {
    fn from(e: PixivError) -> Self {
        e.error
    }
}
type PixivResult<T> = std::result::Result<T, PixivError>;
/// 给后台使用方的非机密信息。授权由调用方的任务策略传入，从不根据有没有凭据推断。
pub fn network_state(authorized: bool) -> Result<Value> {
    let _guard = ACCOUNT_LOCK.lock().expect("app_accounts mutex poisoned");
    let m = meta()?;
    let in_app = app(&m);
    let user_id = if in_app {
        m.get("pixiv").and_then(|p| p.get("user_id")).map(|v| v.text().to_owned())
    } else {
        secret_inner("pixiv")?.map(|s| s.split('_').next().unwrap_or("").to_owned())
    };
    Ok(json!({"authorized":authorized,"desktop":desktop_available(),"pixiv_in_app":in_app,
        "pixiv_user_id":user_id,"pixiv_expired":m.get("pixiv").and_then(|p|p.get("expired")).map(Json::text),
        "saucenao_bound":secret_inner("saucenao")?.is_some()}))
}
fn support() -> Result<PathBuf> {
    crate::platform_credentials::support()
}
fn check(kind: &str) -> Result<()> {
    if !["pixiv", "saucenao"].contains(&kind) {
        return Err(Error::invalid("未知的账号类型"));
    }
    Ok(())
}
/// 带机密的内部接口，账号 HTTP 响应从不序列化它。
pub fn secret(kind: &str) -> Result<Option<String>> {
    let _guard = ACCOUNT_LOCK.lock().expect("app_accounts mutex poisoned");
    secret_inner(kind)
}
fn secret_inner(kind: &str) -> Result<Option<String>> {
    check(kind)?;
    crate::platform_credentials::read(kind)
}
fn put(kind: &str, value: &str) -> Result<()> {
    check(kind)?;
    crate::platform_credentials::write(kind, value)
}
fn remove_secret(kind: &str) -> Result<()> {
    check(kind)?;
    crate::platform_credentials::remove(kind)
}
fn meta() -> Result<Json> {
    Ok(fs::read_to_string(support()?.join("accounts.json"))
        .ok()
        .and_then(|s| crate::http_json::parse_python(&s).ok())
        .filter(|j| matches!(j, Json::Object(_)))
        .unwrap_or(Json::Object(vec![])))
}
fn save(meta: &Json) -> Result<()> {
    let dir = support()?;
    fs::create_dir_all(&dir)?;
    let tmp = dir.join("accounts.tmp");
    fs::write(&tmp, format!("{}\n", meta.dump()))?;
    fs::rename(tmp, dir.join("accounts.json"))?;
    Ok(())
}
fn app(meta: &Json) -> bool {
    meta.get("pixiv").and_then(|j| j.get("via")).is_some_and(|j| j.text() == "app")
}
fn checked(mut entry: Json) -> Json {
    entry.set("checked_at", chrono::Local::now().format("%Y-%m-%d %H:%M").to_string().into());
    entry
}
fn erase(entry: &mut Json, key: &str) {
    if let Json::Object(fields) = entry {
        fields.retain(|(k, _)| k != key);
    }
}

pub fn desktop_available() -> bool {
    crate::app_desktop::available()
}
pub fn desktop(method: &str, route: &str, timeout: u64) -> Result<Json> {
    if !desktop_available() {
        return Err(Error::invalid("不是从 Illustash 桌面 app 启动的，用不了 Pixiv 登录窗口"));
    }
    crate::app_desktop::request(method, route, timeout)
}
fn read_json(response: ureq::Response) -> Result<Json> {
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(5 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::invalid("账号服务响应读取失败"))?;
    if bytes.len() > 5 * 1024 * 1024 {
        return Err(Error::invalid("账号服务响应超过限额"));
    }
    let s = std::str::from_utf8(&bytes).map_err(|_| Error::invalid("账号服务响应编码不正确"))?;
    crate::http_json::parse_python(s).map_err(|_| Error::invalid("账号服务响应不正确"))
}
/// #51 的已授权查询通道；调用方必须显式提供联网授权。
/// 保存过的登录不代表授权；经桌面 app 发出的请求从不退回到 cookie。
fn pixiv_get(path: &str, authorized: bool, session: Option<&str>, in_app: bool) -> PixivResult<Json> {
    if !authorized {
        return Err(Error::invalid("尚未授权联网").into());
    }
    if !path.starts_with("/ajax/") || path.contains(['\r', '\n']) {
        return Err(Error::invalid("只允许 Pixiv ajax 查询").into());
    }
    if in_app {
        let route =
            format!("/pixiv/fetch?path={}", url::form_urlencoded::byte_serialize(path.as_bytes()).collect::<String>());
        let reply = desktop("GET", &route, 120)?;
        let status = reply.get("status").map(Json::integer).unwrap_or(0);
        if status == 401 {
            return Err(PixivError::new(Pixiv::Refused, "Pixiv 没有接受这个登录状态"));
        }
        if status == 404 {
            return Ok(Json::Scalar(Value::Null));
        }
        if status == 403
            || status == 429
            || (status == 503 && reply.get("body").map(Json::text).unwrap_or("").to_lowercase().contains("maintenance"))
        {
            return Err(Error::invalid("Pixiv 服务拒绝请求").into());
        }
        if !(200..300).contains(&status) {
            return Err(PixivError::new(Pixiv::WindowFailed, "Pixiv 窗口里的请求失败"));
        }
        return crate::http_json::parse_python(reply.get("body").map(Json::text).unwrap_or(""))
            .map_err(|_| Error::invalid("Pixiv 响应不正确").into());
    }
    let base = match std::env::var("ILLUSTASH_PIXIV_BASE") {
        Ok(base) => {
            let u = url::Url::parse(&base).map_err(|_| Error::invalid("合成 Pixiv 服务地址不正确"))?;
            if u.scheme() != "http"
                || u.host_str() != Some("127.0.0.1")
                || u.port().is_none()
                || !u.username().is_empty()
                || u.password().is_some()
                || u.path() != "/"
                || u.query().is_some()
                || u.fragment().is_some()
            {
                return Err(Error::invalid("Pixiv 替身只允许本机带端口 HTTP").into());
            }
            base.trim_end_matches('/').to_owned()
        }
        Err(_) => "https://www.pixiv.net".into(),
    };
    let agent = ureq::AgentBuilder::new().redirects(0).timeout(Duration::from_secs(25)).build();
    let mut req = agent
        .get(&format!("{base}{path}"))
        .set("Referer", "https://www.pixiv.net/")
        .set("User-Agent", "Illustash/0.1 (+local library; provenance lookups)");
    if let Some(s) = session {
        req = req.set("Cookie", &format!("PHPSESSID={s}"));
    }
    match req.call() {
        Ok(r) => Ok(read_json(r)?),
        Err(ureq::Error::Status(401, _)) => Err(PixivError::new(Pixiv::Refused, "Pixiv 没有接受这个登录状态")),
        Err(ureq::Error::Status(404, _)) => Ok(Json::Scalar(Value::Null)),
        Err(ureq::Error::Status(403 | 429, _)) => Err(Error::invalid("Pixiv 服务拒绝请求").into()),
        Err(ureq::Error::Status(503, response)) => {
            let mut reason = String::new();
            let _ = response.into_reader().take(4096).read_to_string(&mut reason);
            Err(if reason.to_lowercase().contains("maintenance") {
                Error::invalid("Pixiv 服务拒绝请求").into()
            } else {
                PixivError::new(Pixiv::Failed, "Pixiv 请求失败")
            })
        }
        Err(_) => Err(PixivError::new(Pixiv::Failed, "Pixiv 请求失败")),
    }
}
// 账号验证的两次查询共用一套等同于 Net 的策略。
// pixiv_get 仍是单次请求的通道，由任务使用方自行协调。
fn account_query(
    path: &str,
    session: Option<&str>,
    in_app: bool,
    last: &mut Option<std::time::Instant>,
) -> PixivResult<Json> {
    for attempt in 0..3 {
        if let Some(started) = last {
            let elapsed = started.elapsed();
            if elapsed < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_secs(2) - elapsed);
            }
        }
        *last = Some(std::time::Instant::now());
        match pixiv_get(path, true, session, in_app) {
            Err(e) if attempt < 2 && (matches!(e.kind, Pixiv::Failed | Pixiv::WindowFailed) || e.unanswered()) => {
                std::thread::sleep(Duration::from_secs(2 * (attempt + 1)));
            }
            result => return result,
        }
    }
    unreachable!()
}
fn who(uid: &str, session: Option<&str>, in_app: bool) -> PixivResult<Json> {
    let mut last = None;
    let extra = match account_query("/ajax/user/extra?lang=ja", session, in_app, &mut last) {
        Err(PixivError { kind: Pixiv::Refused, .. }) => {
            return Err(PixivError::new(
                Pixiv::Refused,
                "Pixiv 没有接受这个登录状态（可能已退出或过期）（Pixiv 返回 HTTP 401）",
            ));
        }
        result => result?,
    };
    if matches!(extra, Json::Scalar(Value::Null)) || extra.get("error").is_some_and(crate::app_service::truthy) {
        let why = if matches!(extra, Json::Scalar(Value::Null)) {
            "HTTP 404".to_string()
        } else {
            format!(
                "error：{}",
                extra
                    .get("message")
                    .map(Json::text)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("无说明")
                    .chars()
                    .take(80)
                    .collect::<String>()
            )
        };
        return Err(PixivError::new(
            Pixiv::Refused,
            format!("Pixiv 没有接受这个登录状态（可能已退出或过期）（Pixiv 返回 {why}）"),
        ));
    }
    let user = account_query(&format!("/ajax/user/{uid}?lang=ja"), session, in_app, &mut last)?;
    Ok(Json::object([
        ("user_id", uid.into()),
        (
            "name",
            user.get("body")
                .and_then(|b| b.get("name"))
                .filter(|v| crate::app_service::truthy(v))
                .cloned()
                .unwrap_or("".into()),
        ),
        ("following", extra.get("body").and_then(|b| b.get("following")).cloned().unwrap_or(Value::Null.into())),
    ]))
}
fn clean(value: &str) -> Result<String> {
    let re = regex::Regex::new(r"PHPSESSID=([^;\s]+)").expect("constant regular expression is valid");
    let raw = re.captures(value).map(|c| c[1].to_owned()).unwrap_or_else(|| value.trim().to_owned());
    if !regex::Regex::new(r"^\d+_[A-Za-z0-9]+$").expect("constant regular expression is valid").is_match(&raw) {
        return Err(Error::invalid("PHPSESSID 的格式不对"));
    }
    Ok(raw)
}
pub fn status() -> Result<Json> {
    let _guard = ACCOUNT_LOCK.lock().expect("app_accounts mutex poisoned");
    let meta = meta()?;
    let mut out = Json::Object(vec![]);
    for kind in ["pixiv", "saucenao"] {
        let bound = secret_inner(kind)?.is_some();
        let mut entry = Json::object([("bound", bound.into())]);
        if let Some(Json::Object(fields)) = meta.get(kind) {
            for (k, v) in fields {
                entry.set(k, v.clone());
            }
        }
        if kind == "pixiv" {
            entry.set("bound", (bound || app(&meta)).into());
        }
        out.set(kind, entry);
    }
    let available = desktop_available();
    out.set("desktop", available.into());
    if available && out.get("pixiv").and_then(|p| p.get("expired")).is_some_and(crate::app_service::truthy) {
        if let Ok(s) = desktop("GET", "/pixiv/session", 30) {
            let mut p = out.get("pixiv").expect("app_accounts internal invariant").clone();
            p.set("window_logged_in", s.get("loggedIn").is_some_and(crate::app_service::truthy).into());
            out.set("pixiv", p);
        }
    }
    Ok(out)
}
/// 只读元数据：不联网、不调用桌面，也不查 SauceNAO 或钥匙串。
pub fn pixiv_binding() -> Result<Value> {
    let _guard = ACCOUNT_LOCK.lock().expect("app_accounts mutex poisoned");
    serde_json::from_str(&meta()?.get("pixiv").cloned().unwrap_or(Json::Object(vec![])).dump())
        .map_err(|_| Error::invalid("账号元数据格式不正确"))
}
fn updated_pixiv(mut meta: Json, who: Option<&Value>, problem: Option<&str>, detail: Option<&str>) -> Json {
    let mut entry = meta.get("pixiv").cloned().unwrap_or(Json::Object(vec![]));
    if let Some(Value::Object(fields)) = who {
        // _who 既定的插入顺序对新建的条目也要成立。
        for key in ["user_id", "name", "following"] {
            if let Some(value) = fields.get(key) {
                entry.set(key, value.clone().into());
            }
        }
        for (key, value) in fields {
            if !["user_id", "name", "following"].contains(&key.as_str()) {
                entry.set(key, value.clone().into());
            }
        }
    }
    entry = checked(entry);
    if let Some(problem) = problem.filter(|p| !p.is_empty()) {
        entry.set("expired", problem.into());
        entry.set("expired_detail", detail.unwrap_or("").into());
    } else {
        erase(&mut entry, "expired");
        erase(&mut entry, "expired_detail");
    }
    meta.set("pixiv", entry);
    meta
}
/// 状态与字节、过期判断和重试策略由调用方已授权的 Net 负责；这里只记录与旧版 mark_pixiv 相同的非机密信息。
pub fn mark_pixiv(who: Option<&Value>, problem: Option<&str>, detail: Option<&str>) -> Result<Value> {
    let _guard = ACCOUNT_LOCK.lock().expect("app_accounts mutex poisoned");
    let m = updated_pixiv(meta()?, who, problem, detail);
    save(&m)?;
    serde_json::from_str(&m.get("pixiv").expect("app_accounts internal invariant").dump())
        .map_err(|_| Error::invalid("账号元数据格式不正确"))
}
/// 原生宿主让 HTTP 修改与当前会话串行。请求结束后不留后台任务，切换会等待有界的 I/O。
pub(crate) fn action(library: &Library, action: &str, body: &Json) -> Result<Json> {
    // 把 I/O、SQLite、归档失败转成旧版的 Invalid（400）契约。
    // X 只修改它捕获的图库包；取它的 SQLite 写锁时不要持有账号存储锁。
    let _guard =
        if action == "x/import" { None } else { Some(ACCOUNT_LOCK.lock().expect("app_accounts mutex poisoned")) };
    action_inner(library, action, body).map_err(|e| match e {
        Error::Invalid(_) => e,
        _ => Error::invalid("账号操作失败"),
    })
}
fn action_inner(library: &Library, action: &str, body: &Json) -> Result<Json> {
    let value = body.get("value").map(Json::text).unwrap_or("");
    let mut meta = meta()?;
    match action {
        "saucenao" => {
            let key = value.trim();
            if key.len() != 40 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(Error::invalid("SauceNAO 的 API key 应该是 40 位十六进制字符"));
            }
            put("saucenao", key)?;
            let entry = checked(Json::Object(vec![]));
            meta.set("saucenao", entry.clone());
            save(&meta)?;
            Ok(entry)
        }
        "pixiv" => {
            let s = clean(value)?;
            let entry = checked(who(s.split('_').next().expect("app_accounts internal invariant"), Some(&s), false)?);
            put("pixiv", &s)?;
            meta.set("pixiv", entry.clone());
            save(&meta)?;
            Ok(entry)
        }
        "pixiv/login" => {
            let force = meta.get("pixiv").and_then(|p| p.get("expired")).is_some_and(crate::app_service::truthy);
            desktop("POST", if force { "/pixiv/login?force=1" } else { "/pixiv/login" }, 30)?;
            Ok(json!({"ok":true}).into())
        }
        "pixiv/connect" => {
            let s = desktop("GET", "/pixiv/session", 30)?;
            let mut who_entry = None;
            if s.get("loggedIn").is_some_and(crate::app_service::truthy) {
                let uid = s.get("userId").map(Json::text).unwrap_or("");
                let w = who(uid, None, true);
                match w {
                    Err(e)
                        if (e.kind == Pixiv::WindowFailed || e.unanswered())
                            && s.get("loginOpen").is_some_and(crate::app_service::truthy) => {}
                    Err(e) => return Err(e.into()),
                    Ok(mut w) => {
                        w.set("via", "app".into());
                        let entry = checked(w);
                        remove_secret("pixiv")?;
                        meta.set("pixiv", entry.clone());
                        save(&meta)?;
                        who_entry = Some(entry);
                    }
                }
            }
            let mut out = Json::object([("bound", who_entry.is_some().into())]);
            if let Some(Json::Object(fields)) = who_entry {
                for (k, v) in fields {
                    out.set(&k, v);
                }
            }
            out.set(
                "login_open",
                desktop("GET", "/pixiv/session", 30)?.get("loginOpen").is_some_and(crate::app_service::truthy).into(),
            );
            Ok(out)
        }
        "pixiv/check" => {
            let in_app = app(&meta);
            let s = if in_app { None } else { secret_inner("pixiv")? };
            if !in_app && s.is_none() {
                return Err(Error::invalid("还没有绑定 Pixiv"));
            }
            if in_app && !desktop_available() {
                return Err(Error::invalid(PIXIV_NO_WINDOW));
            }
            let mut result = None;
            let mut expired_detail = String::new();
            for attempt in 0..if in_app { 2 } else { 1 } {
                let w = if in_app {
                    let state = desktop("GET", "/pixiv/session", 30)?;
                    if !state.get("loggedIn").is_some_and(crate::app_service::truthy) {
                        Err(PixivError::new(Pixiv::NoLogin, "登录窗口里没有 Pixiv 登录"))
                    } else {
                        who(state.get("userId").map(Json::text).unwrap_or(""), None, true)
                    }
                } else {
                    who(
                        s.as_ref()
                            .expect("app_accounts internal invariant")
                            .split('_')
                            .next()
                            .expect("app_accounts internal invariant"),
                        s.as_deref(),
                        false,
                    )
                };
                match w {
                    Ok(w) => {
                        result = Some(w);
                        break;
                    }
                    Err(PixivError { kind: Pixiv::Refused | Pixiv::NoLogin, error: Error::Invalid(msg) }) => {
                        expired_detail = msg;
                        if in_app && attempt == 0 {
                            std::thread::sleep(Duration::from_secs(5));
                        }
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            let ok = result.is_some();
            let who_value = result
                .as_ref()
                .map(|v| serde_json::from_str::<Value>(&v.dump()).expect("app_accounts internal invariant"));
            meta = updated_pixiv(
                meta,
                who_value.as_ref(),
                if ok { None } else { Some(PIXIV_EXPIRED) },
                if in_app { Some(&expired_detail) } else { None },
            );
            let entry = meta.get("pixiv").expect("app_accounts internal invariant").clone();
            save(&meta)?;
            let mut out = Json::object([("ok", ok.into()), ("bound", true.into())]);
            if let Json::Object(fields) = entry {
                for (k, v) in fields {
                    out.set(&k, v);
                }
            }
            Ok(out)
        }
        "pixiv/unbind" | "saucenao/unbind" => {
            let kind = action.split('/').next().expect("app_accounts internal invariant");
            remove_secret(kind)?;
            if kind == "pixiv" && app(&meta) && desktop_available() {
                desktop("POST", "/pixiv/logout", 30)?;
            }
            erase(&mut meta, kind);
            save(&meta)?;
            Ok(json!({"ok":true}).into())
        }
        "x/import" => {
            let path = body.get("path").map(Json::text).unwrap_or("");
            let expanded = if let Some(suffix) = path.strip_prefix("~/") {
                crate::app_paths::user_home()?.join(suffix)
            } else {
                PathBuf::from(path)
            };
            let n = import_archive(library, &expanded)?;
            Ok(Json::object([("new", n.into()), ("path", path.into())]))
        }
        _ => Err(Error::invalid("未知操作")),
    }
}
fn following_file(path: &Path) -> Result<PathBuf> {
    let mut found = Vec::new();
    let mut todo = vec![path.to_owned()];
    while let Some(dir) = todo.pop() {
        for e in fs::read_dir(&dir)? {
            let e = e?;
            let ty = e.file_type()?;
            if ty.is_dir() {
                todo.push(e.path());
            } else if ty.is_file()
                && e.file_name() == "following.js"
                && e.path().parent().and_then(Path::file_name).is_some_and(|n| n == "data")
            {
                found.push(e.path());
            }
        }
    }
    found.sort();
    found.into_iter().next().ok_or_else(|| Error::invalid("没有找到 data/following.js"))
}
pub fn import_archive(library: &Library, path: &Path) -> Result<i64> {
    const LIMIT: u64 = 64 * 1024 * 1024;
    let mut text = String::new();
    if path.is_dir() {
        fs::File::open(following_file(path)?)?.take(LIMIT + 1).read_to_string(&mut text)?;
    } else {
        let mut zip = zip::ZipArchive::new(fs::File::open(path)?).map_err(|_| Error::invalid("归档格式不正确"))?;
        let index = (0..zip.len())
            .find(|i| {
                zip.by_index(*i)
                    .and_then(|file| file.name().map(|name| name.ends_with("data/following.js")))
                    .unwrap_or(false)
            })
            .ok_or_else(|| Error::invalid("归档里没有 data/following.js"))?;
        zip.by_index(index).map_err(|_| Error::invalid("归档读取失败"))?.take(LIMIT + 1).read_to_string(&mut text)?;
    }
    if text.len() as u64 > LIMIT {
        return Err(Error::invalid("关注列表超过限额"));
    }
    let (_, data) = text.split_once('=').ok_or_else(|| Error::invalid("关注列表格式不正确"))?;
    let records: Vec<Value> = serde_json::from_str(data).map_err(|_| Error::invalid("关注列表格式不正确"))?;
    let mut ids = Vec::new();
    for item in records {
        if let Some(id) = item.get("following").and_then(|v| v.get("accountId")) {
            let s = match id {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Null => continue,
                _ => return Err(Error::invalid("关注账号 ID 不正确")),
            };
            if s.is_empty() {
                continue;
            }
            if !s.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Error::invalid("关注账号 ID 不正确"));
            }
            ids.push(s);
        }
    }
    let mut c = crate::sqlite::open(library.instance_path().join("provenance.sqlite"))?;
    c.busy_timeout(Duration::from_secs(30))?;
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > 5 {
        return Err(Error::invalid("provenance.sqlite 版本比程序新"));
    }
    let tx = c.transaction()?;
    if version < 5 {
        tx.execute_batch(include_str!("app_migration_sql/provenance.sql"))?;
        let colour = tx
            .prepare("PRAGMA table_info(ref_sigs)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|s| s == "colour");
        if !colour {
            tx.execute_batch("ALTER TABLE ref_sigs ADD COLUMN colour BLOB")?;
        }
        tx.execute_batch("PRAGMA user_version=5")?;
    }
    // 一条预编译的批量语句保留输入顺序和 OR IGNORE 语义，
    // 也避免为已有的归档 ID 反复进出 SQLite。
    let data = serde_json::to_string(&ids).map_err(|_| Error::invalid("关注列表格式不正确"))?;
    let count = tx.execute(
        "INSERT OR IGNORE INTO leads(source,system,key) SELECT 'x_following','x_id',value FROM json_each(?)",
        [data],
    )? as i64;
    tx.commit()?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unauthorized_transport_stops_before_reading_any_credentials_or_bridge() {
        assert!(
            matches!(pixiv_get("/ajax/user/extra", false, Some("synthetic"), true), Err(PixivError { kind: Pixiv::Other, error: Error::Invalid(message) }) if message=="尚未授权联网")
        );
    }
    #[test]
    fn mark_merges_identity_and_preserves_other_account_and_field_order() {
        let original=crate::http_json::parse_python(r#"{"pixiv":{"user_id":"123","name":"old","following":1,"via":"app","expired":"old","expired_detail":"old"},"saucenao":{"checked_at":"synthetic"}}"#).unwrap();
        let m = updated_pixiv(original, Some(&json!({"user_id":"123","name":"new","following":7})), None, None);
        assert_eq!(m.get("saucenao").unwrap().dump(), r#"{"checked_at": "synthetic"}"#);
        let p = m.get("pixiv").unwrap();
        assert_eq!(p.get("name").unwrap().text(), "new");
        assert_eq!(p.get("via").unwrap().text(), "app");
        assert!(p.get("expired").is_none());
        assert!(p.dump().starts_with(r#"{"user_id": "123", "name": "new", "following": 7, "via": "app""#));
        let m = updated_pixiv(m, None, Some(PIXIV_EXPIRED), Some("HTTP 401"));
        assert_eq!(m.get("pixiv").unwrap().get("expired_detail").unwrap().text(), "HTTP 401");
    }
}
