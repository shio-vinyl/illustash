//! Pixiv 登录窗口：用户在这里亲手登录（有人机验证就自己点过）；之后原生账号模块要的 www.pixiv.net/ajax 只读请求，
//! 在这个窗口的 www.pixiv.net 页面里用 fetch 发出，状态码和正文原样交回。不改请求头、不伪装、不处理验证页。
//! 窗口在 macOS 14+ 使用独立网站数据存储，在 Windows 使用独立 WebView2 用户数据目录。
//! 解绑只清 Pixiv 窗口的数据存储。
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

const LABEL: &str = "pixiv";
const HOME: &str = "https://www.pixiv.net/";
const LOGIN: &str = "https://accounts.pixiv.net/login?return_to=https%3A%2F%2Fwww.pixiv.net%2F";
const HOST: &str = "www.pixiv.net";
#[cfg(target_os = "macos")]
const STORE: [u8; 16] = *b"illustash-pixiv\0";
const PAGE_TIMEOUT: Duration = Duration::from_secs(45);
const REPLY_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct Pixiv {
    pending: Mutex<HashMap<String, SyncSender<Value>>>,
    page: Mutex<Page>,
    loaded: Condvar,
    busy: Mutex<()>, // 一次只发一个请求；控制节奏由原生调用方负责
    login_open: AtomicBool,
}

/// Pixiv 窗口显示的内容：到目前为止完成的加载次数，以及最后加载完成的页面的主机（导航进行中时为 None）。
/// 请求只从已加载完成的 www.pixiv.net 页面发出。
#[derive(Default)]
struct Page {
    loads: u64,
    host: Option<String>,
    warm: bool, // 这次启动里已有 pixiv.net 页面加载完成
}

/// 由 Pixiv 窗口里的页面调用，带回一次请求的结果（这是它唯一能调用的命令）。
#[tauri::command]
pub fn pixiv_reply(webview: tauri::Webview, state: tauri::State<Pixiv>, id: String, reply: Value) {
    if webview.label() != LABEL {
        return;
    }
    if let Some(tx) = state.pending.lock().expect("lock poisoned by an earlier panic").remove(&id) {
        let _ = tx.try_send(reply);
    }
}

fn window(app: &AppHandle) -> Result<WebviewWindow, String> {
    if let Some(win) = app.get_webview_window(LABEL) {
        return Ok(win);
    }
    let blank = WebviewUrl::External(Url::parse("about:blank").expect("constant URL"));
    let builder = WebviewWindowBuilder::new(app, LABEL, blank)
        .title(crate::i18n::tr("登录 Pixiv"))
        .inner_size(1000.0, 780.0)
        .visible(false)
        .on_new_window(|_, _| NewWindowResponse::Allow) // 登录页可能开第三方登录的弹窗
        .on_page_load(|win, payload| {
            let state = win.state::<Pixiv>();
            let mut page = state.page.lock().expect("lock poisoned by an earlier panic");
            if payload.event() == PageLoadEvent::Started {
                page.host = None;
                return;
            }
            page.loads += 1;
            page.host = payload.url().host_str().map(str::to_string);
            page.warm |= page.host.as_deref().is_some_and(|h| h == "pixiv.net" || h.ends_with(".pixiv.net"));
            eprintln!("[pixiv] 页面加载完成：{}", payload.url().host_str().unwrap_or(payload.url().scheme()));
            state.loaded.notify_all();
            // cookie 可能比服务器端的会话活得久。窗口一直显示到用户关掉为止；
            // 光凭页面加载完成不能证明登录成功。
        });
    let cannot_open = |e: tauri::Error| crate::i18n::tr_with("打不开 Pixiv 窗口：{error}", &[("error", &e)]);
    #[cfg(target_os = "macos")]
    let builder = builder.data_store_identifier(STORE);
    #[cfg(windows)]
    let builder = builder.data_directory(app.path().app_local_data_dir().map_err(cannot_open)?.join("pixiv-webview"));
    let win = crate::webview::build(builder).map_err(cannot_open)?;
    let hide = win.clone();
    win.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close(); // 窗口里的页面还要用来发请求，关掉只是收起
            hide.state::<Pixiv>().login_open.store(false, Ordering::SeqCst);
            let _ = hide.hide();
        }
    });
    Ok(win)
}

fn on_pixiv(win: &WebviewWindow) -> bool {
    win.state::<Pixiv>().page.lock().expect("lock poisoned by an earlier panic").host.as_deref() == Some(HOST)
}

/// 加载 `url`，等到它所在主机上的页面加载完成。在别处完成的加载（比如窗口最初的 about:blank）不算：
/// 从那里发出的请求会在 Pixiv 页面和它的 cookie 就位之前发出去。
fn open(win: &WebviewWindow, url: &str) -> Result<(), String> {
    let target = Url::parse(url).expect("callers pass a constant URL");
    let state = win.state::<Pixiv>();
    let before = state.page.lock().expect("lock poisoned by an earlier panic").loads;
    win.navigate(target.clone()).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + PAGE_TIMEOUT;
    let mut page = state.page.lock().expect("lock poisoned by an earlier panic");
    while page.loads == before || page.host.as_deref() != target.host_str() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(crate::i18n::tr("Pixiv 页面加载超时"));
        }
        page = state.loaded.wait_timeout(page, left).expect("lock poisoned by an earlier panic").0;
    }
    Ok(())
}

/// 窗口的数据存储要等这次启动里有 pixiv.net 页面加载过，才交出保存的 cookie：在那之前读起来是未登录
/// （2026-09-28：一次有效的登录在启动后立刻被判为过期）。所以先加载一次首页，除非用户正在用这个窗口登录。
/// 调用方持有 `busy`。
fn warm(win: &WebviewWindow) {
    let state = win.state::<Pixiv>();
    if !state.page.lock().expect("lock poisoned by an earlier panic").warm && !state.login_open.load(Ordering::SeqCst) {
        if let Err(error) = open(win, HOME) {
            eprintln!("[pixiv] 预热失败：{error}");
        }
    }
}

/// 已登录的 Pixiv 用户：PHPSESSID 形如 `<user id>_<random>`。只有 id 会离开这个函数。
pub fn user_id(app: &AppHandle) -> Option<String> {
    let win = window(app).ok()?;
    {
        let state = app.state::<Pixiv>();
        let _one = state.busy.lock().expect("lock poisoned by an earlier panic");
        warm(&win);
    }
    // cookies_for_url 只匹配完全相同的主机，而 PHPSESSID 在 .pixiv.net 上；这个存储里只有 Pixiv 的东西。
    let cookies = win.cookies().ok()?;
    let sid = cookies
        .iter()
        .find(|c| c.name() == "PHPSESSID" && c.domain().is_some_and(|d| d.trim_start_matches('.') == "pixiv.net"))?
        .value()
        .to_string();
    let uid = sid.split('_').next()?;
    (sid.contains('_') && !uid.is_empty() && uid.bytes().all(|b| b.is_ascii_digit())).then(|| uid.to_string())
}

pub fn session(app: &AppHandle) -> Value {
    let uid = user_id(app);
    json!({"loggedIn": uid.is_some(), "userId": uid,
           "loginOpen": app.state::<Pixiv>().login_open.load(Ordering::SeqCst)})
}

/// 从 www.pixiv.net 页面对 `path`（必须以 /ajax/ 开头）发一次只读 GET：{status, body, cfMitigated}。
pub fn fetch(app: &AppHandle, path: &str) -> Result<Value, String> {
    if !path.starts_with("/ajax/") || path.contains("..") || path.contains("//") {
        return Err("只允许 www.pixiv.net/ajax/ 下的路径".into());
    }
    let state = app.state::<Pixiv>();
    let _one = state.busy.lock().expect("lock poisoned by an earlier panic");
    let win = window(app)?;
    if !on_pixiv(&win) {
        if state.login_open.load(Ordering::SeqCst) {
            return Err(crate::i18n::tr("用户正在 Pixiv 窗口里登录")); // 不把用户正在用的页面换走
        }
        open(&win, HOME)?;
    }
    let mut raw = [0u8; 12];
    getrandom::fill(&mut raw).map_err(|e| e.to_string())?;
    let id: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let (tx, rx) = sync_channel(1);
    state.pending.lock().expect("lock poisoned by an earlier panic").insert(id.clone(), tx);
    let js = format!(
        "(async () => {{ let r; try {{ const res = await fetch({path}, {{credentials: 'same-origin', headers: {{Accept: 'application/json'}}}}); \
         r = {{status: res.status, cfMitigated: res.headers.get('cf-mitigated'), body: await res.text()}}; }} \
         catch (e) {{ r = {{status: 0, error: String(e)}}; }} \
         window.__TAURI_INTERNALS__.invoke('pixiv_reply', {{id: {id}, reply: r}}); }})();",
        path = serde_json::to_string(path).expect("strings always serialize"),
        id = serde_json::to_string(&id).expect("strings always serialize")
    );
    let sent = win.eval(&js).map_err(|e| e.to_string());
    let reply =
        sent.and_then(|_| rx.recv_timeout(REPLY_TIMEOUT).map_err(|_| crate::i18n::tr("Pixiv 窗口没有返回结果")));
    state.pending.lock().expect("lock poisoned by an earlier panic").remove(&id);
    reply
}

/// 显示窗口让用户登录（已登录时则让用户查看自己的 Pixiv）。
pub fn login(app: &AppHandle, force: bool) -> Result<(), String> {
    if force {
        logout(app)?; // 显示登录表单之前，先清掉被拒绝的会话。
    }
    let win = window(app)?;
    let state = app.state::<Pixiv>();
    state.login_open.store(true, Ordering::SeqCst);
    let target = if !force && user_id(app).is_some() { HOME } else { LOGIN };
    win.navigate(Url::parse(target).expect("constant URL")).map_err(|e| e.to_string())?;
    win.show().map_err(|e| e.to_string())?;
    let _ = win.set_focus();
    Ok(())
}

/// 忘掉登录：清空这个窗口自己的数据存储（含 cookie），页面留空白。
pub fn logout(app: &AppHandle) -> Result<(), String> {
    let win = window(app)?;
    let state = app.state::<Pixiv>();
    let _one = state.busy.lock().expect("lock poisoned by an earlier panic");
    warm(&win); // 否则保存的 cookie 还没载入，清空后仍会留下
    win.clear_all_browsing_data().map_err(|e| e.to_string())?;
    state.login_open.store(false, Ordering::SeqCst);
    let _ = win.navigate(Url::parse("about:blank").expect("constant URL"));
    let _ = win.hide();
    Ok(())
}
