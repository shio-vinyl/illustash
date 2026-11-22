//! 关窗后在后台把任务做完（#451，方案见 Issue 评论）。
//!
//! 宿主每秒看一次各任务的状态，有变化时经本机通道 `POST /platform/activity` 告诉外壳（native/terms/src/app_activity.rs）。
//! 外壳据此：
//! - 关主窗口时，有任务在跑就隐藏窗口、任务继续；没有就照旧退出；
//! - 任务结束时，窗口隐藏或不在前台就发系统通知（notify.rs），点通知回到窗口并打开任务面板；
//! - 隐藏期间任务都结束、又安静了 `QUIET` 秒，按设置退出或留在后台；
//! - Windows 上隐藏期间在通知区域放一个托盘图标；macOS 留在 Dock，点 Dock 图标回到窗口（main.rs 的 `Reopen`）。
//!
//! 启动页上的「导入图库」由外壳自己做，也算在跑的任务。
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::i18n::{tr, tr_with};
use crate::notify;

/// 任务都结束以后等这么久，确认没有接着开始的任务（同步做完会接着找回画师之类）再按设置办。
const QUIET: Duration = Duration::from_secs(10);
/// `preferences.json` 里的键：关窗后任务做完时 `quit`（默认）或 `stay`。
pub const AFTER_TASKS: &str = "after_tasks";

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Busy {
    pub kind: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub variant: String,
    #[serde(default)]
    pub done: i64,
    #[serde(default)]
    pub total: i64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Ended {
    pub kind: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub count: i64,
    #[serde(default)]
    pub recognised: i64,
    #[serde(default)]
    pub variant: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Default, Deserialize)]
struct Message {
    #[serde(default)]
    busy: Vec<Busy>,
    #[serde(default)]
    ended: Vec<Ended>,
}

#[derive(Default)]
pub struct Background {
    busy: Mutex<Vec<Busy>>,
    /// 窗口是因为关窗转到后台而隐藏的。
    hidden: AtomicBool,
    /// 每次任务状态变化加一：安静计时结束时用它判断这段时间里有没有新的变化。
    epoch: AtomicU64,
}

/// 现在有没有任务在跑：宿主报来的，加上启动页上的导入。
pub fn busy(app: &AppHandle) -> bool {
    let reported = app.state::<Background>().busy.lock().expect("background busy lock").is_empty();
    !reported || crate::welcome::importing(app)
}

/// `POST /platform/activity` 的正文。
pub fn activity(app: &AppHandle, body: &str) -> Result<Value, String> {
    let message: Message = serde_json::from_str(body).map_err(|_| "任务状态格式不对".to_string())?;
    let state = app.state::<Background>();
    *state.busy.lock().expect("background busy lock") = message.busy.clone();
    state.epoch.fetch_add(1, Ordering::SeqCst);
    let alert = should_alert(app);
    for ended in &message.ended {
        if let Some((title, text)) = alert.then(|| describe(ended)).flatten() {
            notify::send(app, &title, &text);
        }
    }
    notify::keep_awake(app, !message.busy.is_empty());
    #[cfg(windows)]
    tray::tooltip(app, &message.busy);
    if state.hidden.load(Ordering::SeqCst) && !busy(app) {
        wait_then_finish(app);
    }
    Ok(json!({"ok": true}))
}

/// 窗口隐藏、最小化或不在前台时才发通知；在前台时页面自己会提示。
fn should_alert(app: &AppHandle) -> bool {
    if app.state::<Background>().hidden.load(Ordering::SeqCst) {
        return true;
    }
    let Some(main) = app.get_webview_window("main") else { return false };
    !main.is_visible().unwrap_or(true) || main.is_minimized().unwrap_or(false) || !main.is_focused().unwrap_or(true)
}

/// 一个任务结束时的通知：标题和正文。不值得打扰的（没有新图的例行同步、用户自己停下的）返回 `None`。
pub fn describe(ended: &Ended) -> Option<(String, String)> {
    let n = ended.count;
    let failed = |title: String| Some((title, ended.reason.clone()));
    match (ended.kind.as_str(), ended.status.as_str()) {
        ("sync", "done") if n > 0 => Some((tr("同步完成"), tr_with("新加入 {n} 张", &[("n", &number(n))]))),
        ("sync", "done") if ended.recognised > 0 => {
            Some((tr("同步完成"), tr_with("已识别 {n} 张", &[("n", &number(ended.recognised))])))
        }
        ("sync", "needs_recognition") => Some((tr("同步完成"), tr("有图片没识别完，在任务面板里查看"))),
        ("sync", "error") => failed(tr("同步失败")),
        ("provenance", "done") if n > 0 => {
            Some((tr("找回画师完成"), tr_with("新找到 {n} 张图的画师", &[("n", &number(n))])))
        }
        ("provenance", "done") => Some((tr("找回画师完成"), tr("没有新找到画师"))),
        ("provenance", "failed") => failed(tr("找回画师失败")),
        ("index", "checked") if n > 0 => {
            Some((tr("检查新角色完成"), tr_with("发现 {n} 个可添加的角色，在任务面板里添加", &[("n", &number(n))])))
        }
        ("index", "done") => Some((tr("添加角色完成"), String::new())),
        ("index", "paused") => Some((tr("添加角色已暂停，在任务面板里继续"), ended.reason.clone())),
        ("index", "error") => failed(tr("检查新角色失败")),
        ("import", "done") => Some((tr("导入完成"), String::new())),
        ("import", "error") => failed(tr("导入失败")),
        ("export", "done") => Some((tr("导出完成"), tr("导出的文件在「下载」文件夹里"))),
        ("export", "error") => failed(tr("导出失败")),
        ("files", "done") => Some((tr("导出图片完成"), String::new())),
        ("files", "error") => failed(tr("导出图片失败")),
        ("pack", "done") if ended.variant == "withdraw" => Some((tr("已撤回角色资料包"), String::new())),
        ("pack", "done") if matches!(ended.variant.as_str(), "import" | "download") => {
            Some((tr("角色资料包已就位"), String::new()))
        }
        ("pack", "done") => Some((tr("角色资料包"), tr("完成"))),
        ("pack", "failed" | "error") => {
            let body = if ended.reason.is_empty() {
                tr("操作失败。")
            } else {
                tr_with("错误编号：{id}", &[("id", &ended.reason)])
            };
            Some((tr("角色资料包"), body))
        }
        ("models", "done") if ended.variant == "import" => Some((tr("识别模型已导入"), String::new())),
        ("models", "done") => Some((tr("识别模型已下载"), String::new())),
        ("models", "failed" | "error") => failed(tr("识别模型下载失败")),
        _ => None,
    }
}

/// 千位分隔：与页面的 `number()` 一样用逗号。
fn number(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        out.insert(0, '-');
    }
    out
}

/// 主窗口要关时：有任务在跑就隐藏、返回 `true`；没有就返回 `false`，由调用方照旧退出。
pub fn close_requested(app: &AppHandle) -> bool {
    if !busy(app) {
        return false;
    }
    let Some(main) = app.get_webview_window("main") else { return false };
    crate::window_state::save(app);
    if main.hide().is_err() {
        return false;
    }
    app.state::<Background>().hidden.store(true, Ordering::SeqCst);
    notify::prepare(app);
    #[cfg(windows)]
    tray::show(app);
    true
}

/// 宿主停了（换图库重启，或者出错退出）：它报过的任务都不算了。`reveal` 时窗口隐藏着就拿出来。
pub fn host_gone(app: &AppHandle, reveal_window: bool) {
    let state = app.state::<Background>();
    state.busy.lock().expect("background busy lock").clear();
    state.epoch.fetch_add(1, Ordering::SeqCst);
    notify::keep_awake(app, false);
    if reveal_window && state.hidden.load(Ordering::SeqCst) {
        reveal(app);
    }
}

/// 回到窗口：点 Dock 图标、托盘图标或通知时。
pub fn reveal(app: &AppHandle) {
    app.state::<Background>().hidden.store(false, Ordering::SeqCst);
    #[cfg(windows)]
    tray::hide(app);
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.unminimize();
        let _ = main.show();
        let _ = main.set_focus();
    }
}

/// 点通知：回到窗口并打开任务面板（图库页的 `window.illustashOpenTasks`；启动页上没有，什么都不做）。
pub fn open_tasks(app: &AppHandle) {
    reveal(app);
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.eval("window.illustashOpenTasks && window.illustashOpenTasks()");
    }
}

/// 隐藏期间任务都结束了：等 `QUIET`，这段时间里没有新变化、窗口也还隐藏着，就按设置退出或留着。
fn wait_then_finish(app: &AppHandle) {
    let epoch = app.state::<Background>().epoch.load(Ordering::SeqCst);
    let app = app.clone();
    thread::spawn(move || {
        thread::sleep(QUIET);
        let state = app.state::<Background>();
        if state.hidden.load(Ordering::SeqCst) && state.epoch.load(Ordering::SeqCst) == epoch && !busy(&app) {
            if stay() {
                eprintln!("[background] 任务做完了，按设置留在后台");
            } else {
                eprintln!("[background] 任务做完了，退出");
                app.exit(0);
            }
        }
    });
}

/// 设置「关闭窗口后，任务做完时」是不是「留在后台」。文件读不了、没有这一项或值不认识，都按默认的「退出」。
fn stay() -> bool {
    stay_in(crate::i18n::preference_text().as_deref())
}

fn stay_in(text: Option<&str>) -> bool {
    let value = text.and_then(|t| serde_json::from_str::<Value>(t).ok());
    value.as_ref().and_then(|v| v.get(AFTER_TASKS)).and_then(Value::as_str) == Some("stay")
}

/// 托盘图标的提示文字：「Illustash：同步中」，多项时「Illustash：2 项进行中」。
#[cfg(any(windows, test))]
pub fn summary(busy: &[Busy]) -> String {
    let line = match busy {
        [] => return "Illustash".into(),
        [one] => {
            let name = match one.kind.as_str() {
                "sync" => tr("同步中"),
                "index" => tr("检查新角色中"),
                "names" => tr("角色名称中"),
                "similar" => tr("相似图片中"),
                "provenance" => tr("找回画师中"),
                "import" => tr("导入中"),
                "export" => tr("导出图库中"),
                "files" => tr("导出图片中"),
                "pack" => match one.variant.as_str() {
                    "withdraw" => tr("撤回角色资料包中"),
                    "enable" => tr("检查角色资料包中"),
                    "import" => tr("导入角色资料包中"),
                    "catalog" => tr("正在读取角色资料包目录"),
                    _ => tr("下载角色资料包中"),
                },
                "models" if one.variant == "import" => tr("导入识别模型中"),
                "models" => tr("下载识别模型中"),
                _ => tr("任务进行中"),
            };
            if one.total > 0 {
                format!("{name} {} / {}", number(one.done), number(one.total))
            } else {
                name
            }
        }
        many => tr_with("{n} 项进行中", &[("n", &many.len())]),
    };
    tr_with("{name}：{line}", &[("name", &"Illustash"), ("line", &line)])
}

#[cfg(windows)]
mod tray {
    use super::{reveal, summary, Busy};
    use crate::i18n::tr;
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use tauri::{AppHandle, Manager};

    const ID: &str = "background";

    pub fn show(app: &AppHandle) {
        if app.tray_by_id(ID).is_some() {
            return;
        }
        let built = (|| -> tauri::Result<()> {
            let open = MenuItem::with_id(app, "tray-open", tr("打开 Illustash"), true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "tray-quit", tr("退出"), true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            let busy = app.state::<super::Background>().busy.lock().expect("background busy lock").clone();
            let mut builder = TrayIconBuilder::with_id(ID)
                .tooltip(summary(&busy))
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "tray-open" => reveal(app),
                    "tray-quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left, button_state: MouseButtonState::Up, ..
                    } = event
                    {
                        reveal(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                builder = builder.icon(icon.clone());
            }
            builder.build(app)?;
            Ok(())
        })();
        if let Err(error) = built {
            eprintln!("[background] 托盘图标：{error}");
        }
    }

    pub fn hide(app: &AppHandle) {
        let _ = app.remove_tray_by_id(ID);
    }

    pub fn tooltip(app: &AppHandle, busy: &[Busy]) {
        if let Some(tray) = app.tray_by_id(ID) {
            let _ = tray.set_tooltip(Some(summary(busy)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ended(kind: &str, status: &str, count: i64) -> Ended {
        Ended { kind: kind.into(), status: status.into(), count, ..Ended::default() }
    }

    #[test]
    fn routine_or_stopped_runs_do_not_notify() {
        assert_eq!(describe(&ended("sync", "done", 0)), None);
        assert_eq!(describe(&ended("sync", "interrupted", 0)), None);
        assert_eq!(describe(&ended("provenance", "stopped", 0)), None);
        assert_eq!(describe(&ended("index", "checked", 0)), None);
        assert_eq!(describe(&ended("names", "done", 0)), None);
        assert_eq!(describe(&ended("similar", "idle", 0)), None);
        assert_eq!(describe(&ended("import", "cancelled", 0)), None);
    }

    #[test]
    fn finished_and_failed_runs_say_what_happened() {
        assert_eq!(describe(&ended("sync", "done", 51234)), Some(("同步完成".into(), "新加入 51,234 张".into())));
        let recognised = Ended { recognised: 12, ..ended("sync", "done", 0) };
        assert_eq!(describe(&recognised), Some(("同步完成".into(), "已识别 12 张".into())));
        let failed = Ended { reason: "来源文件夹不见了".into(), ..ended("sync", "error", 0) };
        assert_eq!(describe(&failed), Some(("同步失败".into(), "来源文件夹不见了".into())));
        assert_eq!(
            describe(&ended("provenance", "done", 3)),
            Some(("找回画师完成".into(), "新找到 3 张图的画师".into()))
        );
        let model = Ended { variant: "import".into(), ..ended("models", "done", 0) };
        assert_eq!(describe(&model).map(|m| m.0), Some("识别模型已导入".into()));
    }

    #[test]
    fn numbers_and_tray_summary() {
        assert_eq!(number(0), "0");
        assert_eq!(number(999), "999");
        assert_eq!(number(1000), "1,000");
        assert_eq!(number(-1234567), "-1,234,567");
        assert_eq!(summary(&[]), "Illustash");
        let sync = Busy { kind: "sync".into(), done: 1234, total: 50000, ..Busy::default() };
        assert_eq!(summary(std::slice::from_ref(&sync)), "Illustash：同步中 1,234 / 50,000");
        let two = [sync, Busy { kind: "provenance".into(), ..Busy::default() }];
        assert_eq!(summary(&two), "Illustash：2 项进行中");
    }

    #[test]
    fn after_tasks_defaults_to_quit() {
        assert!(!stay_in(None));
        assert!(!stay_in(Some("not json")));
        assert!(!stay_in(Some(r#"{"language":"en"}"#)));
        assert!(!stay_in(Some(r#"{"after_tasks":"quit"}"#)));
        assert!(!stay_in(Some(r#"{"after_tasks":true}"#)));
        assert!(stay_in(Some(r#"{"after_tasks":"stay"}"#)));
    }
}
