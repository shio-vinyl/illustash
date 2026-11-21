//! 默认的原生图库包宿主，显式创建和升级。
use crate::{app_library::Library, http, http_json::Json, Error, Result};
use std::{
    io::Read,
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{atomic::Ordering, Arc},
    thread,
    time::{Duration, Instant},
};
use tiny_http::{Request, Server};

#[path = "http_host_get.rs"]
mod http_host_get;
#[path = "http_host_post.rs"]
mod http_host_post;

const FORCED_OFF: [&str; 2] = ["similar", "sync"];
/// 同时服务的连接数（#25）。浏览器对每个源大约保持六个连接，所以开几个窗口也远低于这个数。
const MAX_CONNECTIONS: usize = 64;
/// 同时运行的请求处理数；同一连接上流水线发来的每个请求各占一个。
const MAX_HANDLERS: usize = 128;

/// 每个连接都有限制的本机 HTTP 服务（#25）：停止发送（或停止读取）的客户端在超时后失去连接，超出上限的连接以 503 拒绝，
/// 所以慢的或卡住的客户端不能无限占用线程。`ILLUSTASH_HTTP_TIMEOUT_MS` 为测试缩短超时。
fn serve(listener: TcpListener) -> Result<Server> {
    let timeout = std::env::var("ILLUSTASH_HTTP_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map_or(Duration::from_secs(30), Duration::from_millis);
    let limits = tiny_http::Limits {
        read_timeout: Some(timeout),
        write_timeout: Some(timeout),
        max_connections: Some(MAX_CONNECTIONS),
    };
    Server::from_listener_with_limits(listener, None, limits).map_err(|e| Error::Invalid(e.to_string()))
}

use crate::app_service::{truthy, Application, Options, Session, SessionStartup};
struct Host {
    application: Application,
    origin: String,
    entry: Entry,
}
impl Host {
    fn wd_status(&self, session: &Session) -> Result<serde_json::Value> {
        crate::app_wd_extension::status(&self.root, &session.library, &session.wd_extension, self.wd_busy(session))
    }
}
impl std::ops::Deref for Host {
    type Target = Application;
    fn deref(&self) -> &Application {
        &self.application
    }
}

/// 每个请求都带一个其他本机进程读不到的密钥：127.0.0.1 对这台机器上的每个进程和用户都开放，Host 和 Origin
/// 检查只能挡住网页。就绪行打印带一次性代码的地址；第一次访问用它换会话 cookie，没有这个 cookie 的请求一律拒绝。
struct Entry {
    /// `illustash-<port>`：cookie 不区分端口，所以另一个端口上的宿主保留它自己的 cookie。
    cookie: String,
    secret: String,
    code: std::sync::Mutex<Option<String>>,
}
impl Entry {
    fn new(port: u16) -> Result<Self> {
        Ok(Self {
            cookie: format!("illustash-{port}"),
            secret: Session::token()?,
            code: std::sync::Mutex::new(Some(Session::token()?)),
        })
    }
    /// 就绪行打印的地址。桌面外壳从 stdout 读它；CLI 把它打印给人看。
    fn address(&self, origin: &str) -> String {
        let code = self.code.lock().expect("entry code lock poisoned");
        format!("{origin}/?k={}", code.as_deref().unwrap_or(""))
    }
    fn admits(&self, request: &Request) -> bool {
        request
            .headers()
            .iter()
            .filter(|h| h.field.equiv("Cookie"))
            .flat_map(|h| h.value.as_str().split(';'))
            .filter_map(|pair| pair.trim().split_once('='))
            .any(|(name, value)| name == self.cookie && same(value, &self.secret))
    }
    /// 带着未用过的代码访问 `GET /?k=<code>` 会取走这个代码，第二次使用就被拒绝，并返回接着要去的地址：
    /// 去掉 `k` 的同一个查询串。
    fn redeem(&self, request: &Request) -> Option<String> {
        let (path, query) = request.url().split_once('?')?;
        if path != "/" {
            return None;
        }
        let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
        let given = pairs.iter().find(|(k, _)| k == "k").map(|(_, v)| v.as_str())?;
        let mut code = self.code.lock().expect("entry code lock poisoned");
        if !code.as_deref().is_some_and(|code| same(given, code)) {
            return None;
        }
        *code = None;
        let rest = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs.iter().filter(|(k, _)| k != "k"))
            .finish();
        Some(if rest.is_empty() { "/".into() } else { format!("/?{rest}") })
    }
    /// 设置 cookie，并用这个页面的刷新而不是重定向继续：重定向延续的是带来代码的那次导航，从桌面启动页来时它是跨站的，
    /// SameSite=Strict 的 cookie 不会随它发送。刷新是 127.0.0.1 自己的导航。
    fn enter(&self, request: Request, target: &str) {
        let target = target.replace('&', "&amp;");
        let body = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" content=\"0;url={target}\"><title>Illustash</title><script src=\"/theme.js\"></script><style>html{{background:#f4f4f6}}html[data-theme=dark]{{background:#1d1d21}}</style></head><body></body></html>"
        );
        let cookie = format!("{}={}; HttpOnly; SameSite=Strict; Path=/", self.cookie, self.secret);
        let headers = [
            ("Content-Type", "text/html; charset=utf-8"),
            ("Cache-Control", "no-store"),
            ("X-Content-Type-Options", "nosniff"),
            ("Referrer-Policy", "no-referrer"),
            ("Content-Security-Policy", "default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"),
            ("X-Illustash-Route", "core"),
            ("Set-Cookie", cookie.as_str()),
        ];
        let mut response = tiny_http::Response::from_string(body);
        for (name, value) in headers {
            response.add_header(tiny_http::Header::from_bytes(name, value).expect("entry header is valid"));
        }
        let _ = request.respond(response);
    }
}
/// 比较时不在第一个不同处停下。
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |d, (x, y)| d | (x ^ y)) == 0
}
impl Session {
    fn context(&self, tags: &std::path::Path) -> http::LibraryContext {
        http::LibraryContext {
            db: self.library.db(),
            generation: self.token.clone(),
            library: Some(self.library.instance_path().to_owned()),
            tags: tags.to_owned(),
            source: Some(self.library.source()),
            examples: Some(self.examples.clone()),
            token: Some(self.token.clone()),
            read_only: None,
        }
    }
}
fn header<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request.headers().iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
}
fn error(message: &str) -> Json {
    Json::object([("error", message.into())])
}
fn respond(request: Request, payload: Json, status: u16, route: &str) {
    match http::json_response(&request, &payload, status, route) {
        Ok(response) => {
            let _ = request.respond(response);
        }
        Err(e) => eprintln!("HTTP response: {e}"),
    }
}
fn failure(request: Request, err: Error, write: bool) {
    use crate::diagnostics::{self, Publication, Stage};
    // 接口以外的路径请求不合法（WebView2 每次加载页面都会自己请求 /favicon.ico）不是用户操作的失败：照常应答，
    // 但不记诊断，免得每次启动都挤占记录名额。
    let recorded = request.url().starts_with("/api/") || !matches!(err, Error::Invalid(_));
    // 传输层不知道每个操作的发布边界，不能把已发布后的登记失败说成「全部回滚」。
    let diagnostic = recorded.then(|| {
        diagnostics::failure(
            diagnostics::operation(request.url()),
            if write { Stage::Write } else { Stage::Read },
            &err,
            if write { Publication::Unknown } else { Publication::NotApplicable },
        )
    });
    let (payload, status) = match err {
        Error::Invalid(message) => (error(&message), 400),
        Error::Limit(message) => (error(&message), 413),
        Error::Network(message) => (error(&message), 502),
        Error::Conflict(message) => (error(&message), 409),
        err => {
            eprintln!("HTTP native: {err}");
            (
                error(if write {
                    "保存失败，请先核对操作结果；可在设置的关于页复制诊断信息"
                } else {
                    "读取失败，请重试；可在设置的关于页复制诊断信息"
                }),
                500,
            )
        }
    };
    match http::json_response(&request, &payload, status, "core") {
        Ok(mut response) => {
            if let Some(diagnostic) = diagnostic {
                let value = diagnostic.value().to_string();
                response.add_header(
                    tiny_http::Header::from_bytes("X-Illustash-Diagnostic", value.as_bytes())
                        .expect("diagnostic header is ASCII JSON"),
                );
            }
            let _ = request.respond(response);
        }
        Err(e) => eprintln!("HTTP response: {e}"),
    }
}
fn unavailable(request: Request) {
    respond(request, error("接口或可选研究任务不可用"), 502, "unavailable");
}
fn forced_off(library: &Library, startup: &SessionStartup) -> Vec<String> {
    let mut result: Vec<String> = FORCED_OFF
        .iter()
        .filter(|key| {
            (**key != "similar" || !crate::app_research::configured())
                && (**key != "sync" || startup.sync_disabled || !crate::app_sync::Syncer::available(library))
        })
        .map(|s| s.to_string())
        .collect();
    if !startup.index_auto_check {
        result.insert(0, "index_check".into());
    }
    if !startup.provenance_enabled || std::env::var_os("ILLUSTASH_PROVENANCE_WORKER").is_none() {
        result.insert(if startup.index_auto_check { 0 } else { 1 }, "provenance".into());
    }
    if !startup.names_enabled {
        result.insert(1, "names".into());
    }
    result
}
fn handle(mut request: Request, host: &Host, received: u64) {
    let write = request.method().as_str() == "POST";
    // 页面语言下的名称（#173）：只有读请求使用它（以及只读的建议，http_host_post.rs），所以写入保存的东西从不依赖它。
    let _language = crate::http_read::PageLanguage::set(header(&request, "X-Illustash-Language").filter(|_| !write));
    // 安全检查在读请求体之前。不完整的请求体不得持有图库锁。
    let denied = if header(&request, "Host") != Some(host.origin.trim_start_matches("http://")) {
        Some("仅允许本机地址访问")
    } else if header(&request, "Origin").is_some_and(|v| !v.is_empty() && v != host.origin) {
        Some("不允许跨站访问")
    } else {
        None
    };
    if let Some(message) = denied {
        respond(request, error(message), 403, "core");
        return;
    }
    if request.method().as_str() == "GET" {
        if let Some(target) = host.entry.redeem(&request) {
            host.entry.enter(request, &target);
            return;
        }
    }
    if !host.entry.admits(&request) {
        respond(request, error("请用启动时显示的地址打开图库"), 403, "core");
        return;
    }
    {
        let session = host.session.read().expect("app_http session lock poisoned");
        if !host.accepts(&session, received)
            || (write
                && (header(&request, "Origin") != Some(host.origin.as_str())
                    || header(&request, "X-CSRF-Token") != Some(session.token.as_str())))
        {
            respond(request, error("会话已过期，请刷新页面"), 403, "core");
            return;
        }
    }
    if write {
        let length = request.body_length();
        if !matches!(length, Some(1..=100000))
            || !header(&request, "Content-Type")
                .is_some_and(|v| v.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("application/json"))
        {
            respond(request, error("请求格式不正确"), 400, "core");
            return;
        }
        let mut bytes = Vec::new();
        if request
            .as_reader()
            .take(length.expect("app_http internal invariant") as u64)
            .read_to_end(&mut bytes)
            .is_err()
        {
            respond(request, error("请求格式不正确"), 400, "core");
            return;
        }
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(_) => {
                respond(request, error("请求格式不正确"), 400, "core");
                return;
            }
        };
        let body = match crate::http_json::parse_request(text) {
            Ok(value @ Json::Object(_)) => value,
            Ok(_) => {
                respond(request, error("请求格式不正确"), 400, "core");
                return;
            }
            Err(e) => {
                failure(request, e, true);
                return;
            }
        };
        http_host_post::handle(request, host, received, text, body);
        return;
    }
    if request.method().as_str() != "GET" {
        unavailable(request);
        return;
    }
    http_host_get::handle(request, host, received);
}

// 沿用旧实现 library_action 的分类：文件系统失败包装成 Invalid，SQLite 失败仍是 500。
fn management_error(error: Error) -> Error {
    match error {
        Error::Io(e) => Error::Invalid(e.to_string()),
        other => other,
    }
}

/// 缺省、null 和空白名称沿用默认名；其他非字符串在创建任何目录之前拒绝。
fn creation_name(body: &Json) -> Result<&str> {
    let name = match body.get("name") {
        None | Some(Json::Scalar(serde_json::Value::Null)) => "",
        Some(Json::Scalar(serde_json::Value::String(name))) => name,
        _ => return Err(Error::invalid("图库名称必须是字符串")),
    };
    let name = crate::http_read::trim(name);
    Ok(if name.is_empty() { "新图库" } else { name })
}
fn create_library(host: &Host, body: &Json) -> Result<Json> {
    let name = creation_name(body)?;
    let dest = crate::app_libraries::unique_path(name)?;
    let result = match body.get("kind").map(Json::text) {
        _ if body.get("sources").is_some() => create_sources_library(host, body, &dest, name),
        Some("photos") => {
            if name.contains('\0') {
                return Err(Error::invalid("无法创建图库：名称不能包含空字符（NUL）"));
            }
            // 与 web.sync.PHOTOS 一致；这里只是配置，从不访问 Photos 图库包。
            let photos = crate::app_libraries::photos_library()?;
            crate::app_create::create_photos(&dest, &photos, Some(name), creation_reference(host)?)
        }
        Some("folder") => create_folder_library(host, body, &dest, name),
        _ => return Err(Error::Invalid("请选择来源".into())),
    }
    .map_err(|error| creation_error(error, &host.reference))?;
    // 已发布的图库包在登记失败时仍然保留。创建从不轮换当前会话。登记失败时连同已发布的路径一起报告（#97）：
    // 再创建一次只会多出一个带编号的图库包。
    let path = result["path"].as_str().ok_or_else(|| Error::Schema("建库结果缺少路径".into()))?;
    let mut out = Json::object([("path", path.into()), ("name", name.into())]);
    if let Err(error) = crate::app_libraries::remember_created(std::path::Path::new(path), name) {
        out.set("listed", false.into());
        out.set("error", crate::app_libraries::unlisted_reason(&error).into());
    }
    Ok(out)
}
/// `sources: [{kind: "folder", path, preset} | {kind: "photos"}]`：每个文件夹都写明出处；从不猜。
fn create_sources_library(host: &Host, body: &Json, dest: &Path, name: &str) -> Result<serde_json::Value> {
    let Some(Json::Array(items)) = body.get("sources") else {
        return Err(Error::Invalid("sources 必须是数组".into()));
    };
    if name.contains('\0') {
        return Err(Error::invalid("无法创建图库：名称不能包含空字符（NUL）"));
    }
    let mut sources = Vec::new();
    for item in items {
        match item.get("kind").map(Json::text) {
            Some("folder") => {
                let path = item.get("path").map(Json::text).unwrap_or("");
                if path.is_empty() || path.contains('\0') {
                    return Err(Error::Invalid("请选择文件夹".into()));
                }
                let path = expand_path(path)?;
                let path = if path.is_absolute() { path } else { host.root.join(path) };
                sources.push(crate::app_create::NewSource::Folder {
                    path: crate::app_paths::resolve(&path)?,
                    preset: item.get("preset").map(Json::text).unwrap_or("").to_owned(),
                });
            }
            Some("photos") => {
                if !illustash_platform::capabilities::photos() {
                    return Err(Error::Invalid("Unsupported：照片图库只在 macOS 上可用".into()));
                }
                sources.push(crate::app_create::NewSource::Photos { path: crate::app_libraries::photos_library()? });
            }
            _ => return Err(Error::Invalid("请选择来源".into())),
        }
    }
    crate::app_service::build_sources(&host.root, dest, name, sources)
}
fn create_folder_library(host: &Host, body: &Json, dest: &Path, name: &str) -> Result<serde_json::Value> {
    let folder = match body.get("folder").filter(|v| truthy(v)) {
        Some(Json::Scalar(serde_json::Value::String(value))) => value.as_str(),
        None => "",
        Some(value) => {
            let kind = match value {
                Json::Integer(_) => "int",
                Json::Float(_) => "float",
                Json::Array(_) => "list",
                Json::Object(_) => "dict",
                Json::Scalar(serde_json::Value::Bool(_)) => "bool",
                _ => "object",
            };
            return Err(Error::Invalid(format!(
                "argument should be a str or an os.PathLike object where __fspath__ returns a str, not '{kind}'"
            )));
        }
    };
    if folder.contains('\0') {
        return Err(Error::Invalid("lstat: embedded null character in path".into()));
    }
    let folder = expand_path(folder)?;
    let folder = if folder.is_absolute() { folder } else { host.root.join(folder) };
    // 非严格解析在校验目录和出处预设之前，与 new_folder_library 一致。
    let folder = crate::app_paths::resolve(&folder)?;
    if !folder.is_dir() {
        return Err(Error::Invalid(format!("不是文件夹：{path}", path = folder.display())));
    }
    let preset = match body.get("preset").filter(|v| truthy(v)) {
        Some(Json::Scalar(serde_json::Value::String(value))) => Some(value.as_str()),
        None => None,
        Some(_) => return Err(Error::Invalid("来源预设只能是 x、pixiv、other".into())),
    };
    if preset.is_some_and(|value| crate::origin::find(value).is_none()) {
        return Err(Error::Invalid("来源预设只能是 x、pixiv、other".into()));
    }
    if name.contains('\0') {
        return Err(Error::invalid("无法创建图库：名称不能包含空字符（NUL）"));
    }
    crate::app_create::create_folder(dest, &folder, Some(name), preset, creation_reference(host)?)
}
fn creation_reference(host: &Host) -> Result<Option<&Path>> {
    crate::app_create::optional_reference(&host.reference)
}
use crate::app_create::creation_error;

fn expand_path(value: &str) -> Result<PathBuf> {
    if value == "~" || value.starts_with("~/") {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .ok_or_else(|| Error::Invalid("无法定位用户目录".into()))?;
        return Ok(PathBuf::from(home).join(value.strip_prefix("~/").unwrap_or("")));
    }
    Ok(PathBuf::from(value))
}

/// 桌面启动协议：`启动进度：<per mille> <stage>` 行在就绪行之前。外壳显示这些行，
/// 并把每一行当作仍在运行的信号；CLI 不额外打印。
fn startup_progress(stage: &str, done: f64) {
    if std::env::var_os("ILLUSTASH_DESKTOP_STDIN").is_none() {
        return;
    }
    let permille = (done.clamp(0.0, 1.0) * 1000.0) as u32;
    let mut last = LAST_PROGRESS.lock().expect("startup progress lock poisoned");
    // 每次可见的变化一行：无论图库多大，最多大约一千行。
    if last.as_ref().is_some_and(|(p, s, _)| *p == permille && s == stage) {
        return;
    }
    *last = Some((permille, stage.to_owned(), Instant::now()));
    print_progress_line(permille, stage);
}
/// 最近一条启动进度行和它打印的时间。
static LAST_PROGRESS: std::sync::Mutex<Option<(u32, String, Instant)>> = std::sync::Mutex::new(None);

/// 存活期间，只要过了 `every` 还没有新的进度行，就再打印一次最近一条启动进度行（#267）。外壳会停掉
/// 30 秒不出声的服务；升级的备份、迁移和空间回收在 SQLite 内部运行，不报告进度，大图库上可能要几分钟。
/// 只为本进程里正在运行的步骤保活：守卫随这些步骤结束。
struct Keepalive {
    stop: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Keepalive {
    const EVERY: Duration = Duration::from_secs(5);
    fn start(every: Duration, print: fn(u32, &str)) -> Self {
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let thread = thread::spawn(move || {
            while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(every / 5) {
                let mut last = LAST_PROGRESS.lock().expect("startup progress lock poisoned");
                if let Some((permille, stage, at)) = last.as_mut().filter(|(_, _, at)| at.elapsed() >= every) {
                    *at = Instant::now();
                    print(*permille, stage);
                }
            }
        });
        Self { stop: Some(stop), thread: Some(thread) }
    }
}
impl Drop for Keepalive {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
fn print_progress_line(permille: u32, stage: &str) {
    println!("启动进度：{permille} {stage}");
    if std::io::Write::flush(&mut std::io::stdout()).is_ok() {
        trace_startup_output(format_args!("{permille} {stage}"));
    }
}

/// 仅供升级测试：记录宿主写出进度的单调时刻，避免把测试读线程的调度延迟算入间隔。
fn trace_startup_output(event: std::fmt::Arguments<'_>) {
    static START: std::sync::LazyLock<Option<Instant>> =
        std::sync::LazyLock::new(|| std::env::var_os("ILLUSTASH_TEST_TRACE_STARTUP_OUTPUT").map(|_| Instant::now()));
    if let Some(start) = *START {
        let _ = std::io::Write::write_fmt(
            &mut std::io::stderr().lock(),
            format_args!("ILLUSTASH_TEST_STARTUP_OUTPUT_NS {} {event}\n", start.elapsed().as_nanos()),
        );
    }
}

/// 从桌面启动页发起的 `--upgrade`：把升级的各阶段作为启动进度，在无法报告的步骤中保活（#267）。
fn upgrade_reporting(path: &Path) -> Result<crate::app_upgrade::UpgradeReport> {
    startup_progress("正在打开图库", 0.0);
    let prepared = crate::app_upgrade::prepare(path)?;
    let _alive = std::env::var_os("ILLUSTASH_DESKTOP_STDIN")
        .is_some()
        .then(|| Keepalive::start(Keepalive::EVERY, print_progress_line));
    let progress: crate::app_upgrade::Progress = std::sync::Arc::new(startup_progress);
    crate::app_upgrade::run(&prepared, Some(progress)).map_err(|failure| Error::Schema(failure.to_string()))
}

/// 完整性检查会完整读两个数据库。只有上次会话没有正常结束时打开图库才运行它们（#212）；
/// 这时它们占大图库启动的大部分时间，所以页面说明为什么要这么久。
fn open_reporting(path: &Path) -> Result<(Library, std::sync::Arc<crate::app_service::Owner>)> {
    startup_progress("正在打开图库", 0.0);
    let progress: crate::app_library::Progress = std::sync::Arc::new(|label, done| {
        startup_progress(&format!("上次没有正常关闭，正在检查图库数据（{label}）"), done * 0.95)
    });
    crate::app_service::open_owned(path, None, Some(progress))
}

/// `--rescue-source`（#228）：检查发现 source.sqlite 损坏后，桌面外壳的「抢救重建」。
/// 抢救像检查那样报告各阶段；之后图库照常打开。
fn rescue_reporting(path: &Path, root: &Path) -> Result<crate::app_rescue::Report> {
    let reference = crate::app_resources::reference_path(root)?;
    let reference = crate::app_create::optional_reference(&reference)?;
    let step = |stage: &str, done: f64| startup_progress(stage, done * 0.9);
    crate::app_service::rescue_package(path, None, reference, &step)
}

pub(crate) fn run(args: &[String]) -> Result<u8> {
    use crate::diagnostics::{self, Operation, Publication, Stage};
    if crate::desktop_shell::is_host_child() {
        trace_startup_output(format_args!("<host start>"));
    }
    diagnostics::initialize();
    let result = run_inner(args);
    if let Err(error) = &result {
        diagnostics::failure(Operation::Host, Stage::Startup, error, Publication::Unknown);
    }
    result
}
fn run_inner(args: &[String]) -> Result<u8> {
    // 管道归属在初始化之前就记下，所以即使父进程很早崩溃，也会走正常的停止和 join 路径。
    // 除非明确要求，从不读 CLI 的 stdin。
    let desktop_closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let desktop_child = crate::desktop_shell::is_host_child();
    if desktop_child {
        // 外壳在放开这道启动闸之前分配好它的 Windows Job。
        // 如果它在分配之前就退出，EOF 会在任何任务启动之前退出。
        let mut gate = [0u8; 1];
        if std::io::stdin().read_exact(&mut gate).is_err() {
            return Ok(0);
        }
        let closed = desktop_closed.clone();
        thread::spawn(move || {
            crate::platform_process::wait_stdin_closed();
            closed.store(true, Ordering::SeqCst);
        });
    }
    let mut path = None;
    let mut options = Options::new(crate::app_resources::default_root());
    let mut port = 8765;
    let mut backup = false;
    let mut upgrade = false;
    let mut rescue = false;
    let mut open = false;
    let mut expanded = Vec::new();
    for arg in args {
        if let Some((key, value)) = arg.split_once('=') {
            if ["--library", "--root", "--port", "--sync-limit"].contains(&key) {
                expanded.extend([key.to_owned(), value.to_owned()]);
                continue;
            }
        }
        expanded.push(arg.clone());
    }
    let mut iter = expanded.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" => {
                println!("illustash-terms http --library PACKAGE [--root ROOT] [--port PORT] [--backup | --upgrade | --rescue-source] [--open | --no-open] [--no-sync | --no-auto-sync] [--sync-limit N] [--no-names] [--names-network] [--no-provenance] [--allow-provenance-network] [--no-index-check | --index-check] [--index-network]");
                return Ok(0);
            }
            "--native" => {}
            "--open" => open = true,
            "--no-open" => open = false,
            arg if [
                "--python",
                "--upstream",
                "--route",
                "--",
                "--refresh",
                "--source",
                "--db",
                "--cache",
                "--identity",
                "--no-similar",
            ]
            .iter()
            .any(|key| arg == *key || arg.starts_with(&format!("{key}="))) =>
            {
                return Err(Error::Invalid(format!(
                    "兼容宿主参数已退役：{arg}；HTTP 默认使用原生宿主，升级请使用 --upgrade"
                )));
            }
            "--no-names" => options.startup.names_enabled = false,
            "--no-provenance" => options.startup.provenance_enabled = false,
            "--allow-provenance-network" => options.startup.provenance_network_authorized = true,
            "--names-network" => options.startup.names_network_authorized = true,
            "--index-network" => options.startup.index_network_authorized = true,
            "--no-index-check" => options.startup.index_auto_check = false,
            "--index-check" => options.startup.index_auto_check = true,
            "--auto-sync" => options.startup.auto_sync = true,
            "--no-auto-sync" => options.startup.auto_sync = false,
            "--no-sync" => {
                options.startup.auto_sync = false;
                options.startup.sync_disabled = true;
            }
            "--backup" => backup = true,
            "--upgrade" => upgrade = true,
            "--rescue-source" => rescue = true,
            "--library" | "--root" | "--port" | "--route" | "--sync-limit" => {
                let value = iter.next().ok_or_else(|| Error::Invalid(format!("{arg} 缺少值")))?;
                match arg.as_str() {
                    "--library" => path = Some(expand_path(value)?),
                    "--root" => options.root = PathBuf::from(value),
                    "--sync-limit" => {
                        options.startup.sync_limit =
                            value.parse().map_err(|_| Error::Invalid("sync-limit 必须是非负整数".into()))?;
                    }
                    _ => {
                        port = value
                            .parse::<u16>()
                            .ok()
                            .filter(|n| *n > 0)
                            .ok_or_else(|| Error::Invalid("port 必须在 1–65535 之间".into()))?
                    }
                }
            }
            _ => return Err(Error::Invalid(format!("原生 HTTP 不支持参数：{arg}"))),
        }
    }
    if options.root.is_relative() {
        options.root = std::env::current_dir()?.join(&options.root);
    }
    let path = match path {
        Some(path) => path,
        None => crate::app_libraries::startup()?,
    };
    if upgrade && backup {
        return Err(Error::Invalid("--upgrade 不可与 --backup 同用".into()));
    }
    if rescue && (upgrade || backup) {
        return Err(Error::Invalid("--rescue-source 不可与 --upgrade 或 --backup 同用".into()));
    }
    if backup {
        let library = Library::open(&path)?;
        println!("{}", crate::app_settings::backup(&library)?.display());
        return Ok(0);
    }
    // 在任何显式升级提交之前先占住端口和会话熵。
    let server = if upgrade {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        Some(serve(listener)?)
    } else {
        None
    };
    let token = Session::token()?;
    let mut rescued = None;
    // 显式升级在它的写锁下运行自己的完整检查，只运行一次（#212、#267）。
    // 已经是当前格式的图库包（例如之前一次尝试已提交了升级）照常打开。
    let (library, upgrade_backup) = if upgrade {
        match Library::open_deferring_checks(&path) {
            Ok(_) => (open_reporting(&path)?, None),
            Err(_) => {
                let report = upgrade_reporting(&path)?;
                (crate::app_service::own_checked(report.library)?, Some(report.backup))
            }
        }
    } else {
        if rescue {
            rescued = Some(rescue_reporting(&path, &options.root)?);
        }
        match open_reporting(&path) {
            Err(Error::Schema(_)) if desktop_child && !rescue && crate::app_upgrade::needed(&path).is_some() => {
                return Ok(crate::desktop_shell::UPGRADE_NEEDED_EXIT);
            }
            opened => (opened?, None),
        }
    };
    let server = match server {
        Some(server) => server,
        None => {
            let listener = TcpListener::bind(("127.0.0.1", port))?;
            serve(listener)?
        }
    };
    let application = Application::from_library(library, token, options, |error| match &upgrade_backup {
        Some(backup) => format!(
            "升级已提交，没能记进最近图库：{reason}；升级前备份：{backup}",
            reason = crate::app_libraries::unlisted_reason(&error),
            backup = backup.display()
        ),
        None => crate::app_libraries::unlisted_reason(&error),
    })?;
    {
        let mut session = application.session.write().expect("http session lock poisoned");
        session.upgraded = upgrade_backup.map(|backup| crate::app_upgrade::shown(&backup));
        session.rescued = rescued.map(|report: crate::app_rescue::Report| report.json());
    }
    let host = Arc::new(Host { application, origin: format!("http://127.0.0.1:{port}"), entry: Entry::new(port)? });
    http::install_signals();
    let desktop_parent = crate::platform_process::DesktopParent::capture();
    let mut handlers: Vec<thread::JoinHandle<()>> = Vec::new();
    // 恢复持久化的研究工作，或既有的无 learner 裁决通知。
    // 恢复期间持有这个会话；它的数据库里的事件不会重放进替换后的图库。
    host.start_reporting(|| startup_progress("正在恢复后台任务", 0.97))?;
    let address = host.entry.address(&host.origin);
    println!("图库已启动：{address}");
    std::io::Write::flush(&mut std::io::stdout())?;
    if desktop_child {
        trace_startup_output(format_args!("<ready>"));
    }
    // `start-web.command` 在默认浏览器里打开页面；其他情况下手动打开打印出的地址。
    #[cfg(target_os = "macos")]
    if open {
        let _ = std::process::Command::new("/usr/bin/open").arg(&address).spawn();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = open;
    let mut next_recovery = Instant::now() + Duration::from_secs(1);
    // 桌面外壳要知道有没有任务在跑（#451）：和恢复检查一样每秒看一次。
    let mut activity = crate::app_activity::Watcher::new();
    while !http::stopping() && !desktop_closed.load(Ordering::SeqCst) {
        if desktop_parent.lost() {
            break;
        }
        if let Some(request) = server.recv_timeout(Duration::from_millis(100))? {
            // 在派生线程、加锁、读请求体之前记下：排队的请求属于接收它们的会话。
            let received = host.generation.load(Ordering::SeqCst);
            if handlers.iter().filter(|h| !h.is_finished()).count() >= MAX_HANDLERS {
                // 线程有上限：更多请求立即拒绝，不堆积。
                respond(request, error("服务繁忙，请稍后重试"), 503, "core");
            } else {
                let host = host.clone();
                handlers.push(thread::spawn(move || handle(request, &host, received)));
            }
        }
        if host.take_recover_soon() {
            // 有页面在等后台学习落定（#404）：现在就检查，免得平均多等半秒；整秒的检查照常进行。
            host.recover();
        }
        if Instant::now() >= next_recovery {
            host.recover();
            activity.tick(&host);
            next_recovery = Instant::now() + Duration::from_secs(1);
        }
        let mut i = 0;
        while i < handlers.len() {
            if handlers[i].is_finished() {
                let _ = handlers.swap_remove(i).join();
            } else {
                i += 1;
            }
        }
    }
    // EOF 和父进程丢失时，除了主循环，也必须取消选择器和处理线程。
    http::request_stop();
    host.shutdown();
    let deadline = Instant::now() + Duration::from_secs(8);
    while handlers.iter().any(|h| !h.is_finished()) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
    }
    // 未读完的套接字请求体不会让可执行文件继续存活。中断写入的恢复由 SQLite 负责。
    Ok(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn keepalive_repeats_the_last_progress_line_only_while_it_lives() {
        use super::{Keepalive, LAST_PROGRESS};
        use std::{
            thread,
            time::{Duration, Instant},
        };
        static PRINTED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        fn print(permille: u32, stage: &str) {
            PRINTED.lock().unwrap().push(format!("{permille} {stage}"));
        }
        *LAST_PROGRESS.lock().unwrap() = Some((600, "正在备份图库".into(), Instant::now()));
        let alive = Keepalive::start(Duration::from_millis(40), print);
        // 等到至少两次心跳；只防挂死，不限定多快打出来（#508）。
        let deadline = crate::test_wait::deadline();
        while PRINTED.lock().unwrap().len() < 2 {
            assert!(Instant::now() < deadline, "keepalive did not print twice");
            thread::sleep(Duration::from_millis(5));
        }
        drop(alive);
        let printed = PRINTED.lock().unwrap().clone();
        assert!(printed.len() >= 2, "{printed:?}");
        assert!(printed.iter().all(|line| line == "600 正在备份图库"), "{printed:?}");
        thread::sleep(Duration::from_millis(120));
        assert_eq!(PRINTED.lock().unwrap().len(), printed.len(), "nothing after the guard ends");
        *LAST_PROGRESS.lock().unwrap() = None;
    }
    #[test]
    fn creation_name_preserves_defaults_and_rejects_non_strings() {
        use crate::http_json::Json;
        let missing = Json::object(std::iter::empty::<(&str, Json)>());
        assert_eq!(super::creation_name(&missing).unwrap(), "新图库");
        for (input, expected) in [
            ("null", "新图库"),
            (r#""""#, "新图库"),
            (r#"" ""#, "新图库"),
            (r#""\u001c 名字 \u001f""#, "名字"),
            (r#""a/b:c""#, "a/b:c"),
            (r#""bad\u0000name""#, "bad\0name"),
        ] {
            let body = crate::http_json::parse_request(&format!("{{\"name\":{input}}}")).unwrap();
            assert_eq!(super::creation_name(&body).unwrap(), expected, "{input}");
        }
        for input in [
            "false",
            "true",
            "0",
            "-0",
            "-0.0",
            "123",
            "1e400",
            "NaN",
            "Infinity",
            "-Infinity",
            "[]",
            "{}",
            r#"["x", true, null]"#,
            r#"{"a":"b"}"#,
        ] {
            let body = crate::http_json::parse_request(&format!("{{\"name\":{input}}}")).unwrap();
            assert!(
                matches!(super::creation_name(&body), Err(crate::Error::Invalid(message))
                if message == "图库名称必须是字符串"),
                "{input}"
            );
        }
    }
}
