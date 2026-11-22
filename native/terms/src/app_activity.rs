//! 把后台任务的进行状态告诉桌面外壳（#451）：外壳据此决定关窗是隐藏还是退出、什么时候发系统通知。
//!
//! 主循环每秒调用一次 `Watcher::tick`。它读各任务自己的状态（只读内存里的快照，不读数据库），「进行中」的判断与任务
//! 面板相同（web/static/core/jobs.js 的 `*_ACTIVE`，由下面的测试对照）；有任务开始、结束，或进度变了，就经本机通道
//! `POST /platform/activity` 告诉外壳一次。外壳没开（命令行宿主、浏览器宿主）时什么都不做。
//!
//! 发给外壳的只有任务种类、状态、数字和失败原因的前 200 个字，不含路径、文件名和图库内容。
use std::sync::mpsc::{channel, Sender};
use std::thread;

use serde_json::{json, Value};

use crate::app_service::{Application, Session};
use crate::http_json::Json;

pub(crate) const ROUTE: &str = "/platform/activity";
/// 与 web/static/core/jobs.js 的同名列表相同。
pub(crate) const SYNC_ACTIVE: &[&str] = &["waiting", "scanning", "recognising", "importing"];
pub(crate) const INDEX_ACTIVE: &[&str] = &["checking", "fetching", "cleaning", "building", "scanning", "importing"];
pub(crate) const NAMES_ACTIVE: &[&str] = &["fetching", "matching", "translating", "running"];
pub(crate) const SIMILAR_ACTIVE: &[&str] = &["reading", "pairing", "comparing", "waiting"];
pub(crate) const PROV_ACTIVE: &[&str] = &["running", "stopping"];
pub(crate) const IMPORT_ACTIVE: &[&str] = &["running", "stopping"];
const RUNNING: &[&str] = &["running"];

/// 一个任务此刻的样子。`kind` 是外壳认得的名字。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Task {
    pub kind: &'static str,
    pub active: bool,
    /// 应用级资料包下载不随图库切换结束。
    across_libraries: bool,
    pub status: String,
    /// 进行中：已完成多少、一共多少（没有就是 0）。
    pub done: i64,
    pub total: i64,
    /// 结束时给通知用的数字：同步新加入的张数、找回画师新找到的、检查新角色发现的。
    pub count: i64,
    /// 同步结束时这次识别了多少张。
    pub recognised: i64,
    /// 导入的种类（`library`、`recognition`、`human_conclusions`），识别模型是下载还是导入。
    pub variant: String,
    pub reason: String,
}

fn text(value: Option<&Json>) -> String {
    value.map(Json::text).unwrap_or("").to_owned()
}

fn number(value: Option<&Json>) -> i64 {
    value.map(Json::integer).unwrap_or(0)
}

fn reason(value: &Json) -> String {
    ["message", "reason", "error"]
        .iter()
        .map(|k| text(value.get(k)))
        .find(|s| !s.is_empty())
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect()
}

fn task(kind: &'static str, active: &[&str], value: &Json) -> Task {
    let status = text(value.get("status"));
    Task {
        kind,
        across_libraries: false,
        active: active.contains(&status.as_str()),
        done: number(value.get("done")),
        total: number(value.get("total")),
        count: 0,
        recognised: 0,
        variant: text(value.get("kind")),
        reason: reason(value),
        status,
    }
}

/// 面板的一种活动及其内存快照。同一面板项可以同时有全应用的下载和当前图库的检查。
struct Source {
    key: &'static str,
    active: &'static [&'static str],
    read: fn(&Application, &Session) -> Json,
    across_libraries: bool,
}

impl Source {
    const fn new(key: &'static str, active: &'static [&'static str], read: fn(&Application, &Session) -> Json) -> Self {
        Self { key, active, read, across_libraries: false }
    }

    fn observe(&self, value: &Json) -> Task {
        let mut item = task(self.key, self.active, value);
        item.across_libraries = self.across_libraries;
        match self.key {
            "sync" => {
                item.count = number(value.get("imported"));
                item.recognised = number(value.get("done"));
            }
            "index" | "provenance" => item.count = number(value.get("found")),
            // 包任务的原始错误可能带文件名或路径；外壳用诊断编号指向详情。
            "pack" => item.reason = text(value.get("diagnostic").and_then(|d| d.get("id"))),
            _ => {}
        }
        item
    }
}

fn state(progress: Json) -> Json {
    progress.get("state").cloned().unwrap_or(Json::Scalar(Value::Null))
}

const SOURCES: &[Source] = &[
    Source::new("sync", SYNC_ACTIVE, |_, s| s.syncer.snapshot(&s.library)),
    Source::new("index", INDEX_ACTIVE, |_, s| state(s.index.progress())),
    Source::new("names", NAMES_ACTIVE, |_, s| state(s.names.progress())),
    Source::new("similar", SIMILAR_ACTIVE, |_, s| s.research.similar_state()),
    Source::new("provenance", PROV_ACTIVE, |_, s| s.provenance.snapshot()),
    Source::new("export", RUNNING, |app, s| app.exports.state(&s.library, s.generation)),
    Source::new("files", RUNNING, |_, s| s.asset_exports.state(s.generation)),
    Source::new("import", IMPORT_ACTIVE, |app, _| app.imports.state()),
    Source::new("models", RUNNING, |_, _| crate::app_models::job().into()),
    Source::new("pack", RUNNING, |_, s| s.wd_extension.snapshot().into()),
    Source { across_libraries: true, ..Source::new("pack", RUNNING, |_, _| crate::app_models::pack_download().into()) },
];

/// 拿不到会话锁（有请求正在改会话）时返回 `None`，这一秒跳过。只读任务内存。
pub(crate) fn snapshot(app: &Application) -> Option<(u64, Vec<Task>)> {
    let session = app.session.try_read().ok()?;
    let tasks = SOURCES.iter().map(|source| source.observe(&(source.read)(app, &session))).collect();
    Some((session.generation, tasks))
}

/// 上一次看到的样子，以及往外壳发消息的线程。
pub(crate) struct Watcher {
    last: Option<(u64, Vec<Task>)>,
    sent: Option<Value>,
    sender: Option<Sender<Value>>,
}

impl Watcher {
    pub(crate) fn new() -> Self {
        let sender = crate::app_desktop::available().then(|| {
            let (sender, receiver) = channel::<Value>();
            thread::spawn(move || {
                while let Ok(mut message) = receiver.recv() {
                    // 积压了就只发最新的进行状态，但结束事件一条不丢。
                    let mut ended: Vec<Value> = message["ended"].as_array().cloned().unwrap_or_default();
                    while let Ok(next) = receiver.try_recv() {
                        ended.extend(next["ended"].as_array().cloned().unwrap_or_default());
                        message = next;
                    }
                    message["ended"] = Value::Array(ended);
                    if let Err(error) = crate::app_desktop::post(ROUTE, &message.to_string(), 5) {
                        eprintln!("[activity] {error}");
                    }
                }
            });
            sender
        });
        Self { last: None, sent: None, sender }
    }

    pub(crate) fn tick(&mut self, app: &Application) {
        if self.sender.is_none() {
            return;
        }
        let Some(now) = snapshot(app) else { return };
        if let Some(message) = self.change(now) {
            if let Some(sender) = &self.sender {
                let _ = sender.send(message);
            }
        }
    }

    /// 和上一次比：有变化就给出要发给外壳的消息。换了图库（会话代数变了）时上一个图库的任务不算「结束」。
    pub(crate) fn change(&mut self, (generation, now): (u64, Vec<Task>)) -> Option<Value> {
        let ended: Vec<Value> = match &self.last {
            Some((last_generation, last)) => last
                .iter()
                .filter(|before| before.active && (*last_generation == generation || before.across_libraries))
                .filter(|before| !(before.kind == "pack" && before.variant == "catalog"))
                .filter_map(|before| {
                    now.iter()
                        .find(|t| t.kind == before.kind && t.across_libraries == before.across_libraries && !t.active)
                })
                .map(|t| {
                    json!({
                        "kind": t.kind,
                        "status": t.status,
                        "count": t.count,
                        "recognised": t.recognised,
                        "variant": t.variant,
                        "reason": t.reason,
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        let busy: Vec<Value> = now
            .iter()
            .filter(|t| t.active)
            .map(
                |t| json!({"kind": t.kind, "status": t.status, "variant": t.variant, "done": t.done, "total": t.total}),
            )
            .collect();
        self.last = Some((generation, now));
        let state = json!({"busy": busy});
        if ended.is_empty() && self.sent.as_ref() == Some(&state) {
            return None;
        }
        self.sent = Some(state);
        Some(json!({"busy": busy, "ended": ended}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &'static str, status: &str, active: bool) -> Task {
        Task {
            kind,
            across_libraries: false,
            active,
            status: status.into(),
            done: 0,
            total: 0,
            count: 0,
            recognised: 0,
            variant: String::new(),
            reason: String::new(),
        }
    }

    fn watcher() -> Watcher {
        Watcher { last: None, sent: None, sender: None }
    }

    #[test]
    fn reports_start_progress_and_end_once() {
        let mut w = watcher();
        // 第一次总要告诉外壳现在的样子。
        let first = w.change((1, vec![item("sync", "idle", false)])).unwrap();
        assert_eq!(first, json!({"busy": [], "ended": []}));
        assert_eq!(w.change((1, vec![item("sync", "idle", false)])), None);
        let started = w.change((1, vec![item("sync", "recognising", true)])).unwrap();
        assert_eq!(started["busy"][0]["kind"], "sync");
        let mut progressed = item("sync", "recognising", true);
        progressed.done = 10;
        progressed.total = 100;
        assert_eq!(w.change((1, vec![progressed.clone()])).unwrap()["busy"][0]["done"], 10);
        assert_eq!(w.change((1, vec![progressed])), None);
        let mut done = item("sync", "done", false);
        done.count = 3;
        let ended = w.change((1, vec![done.clone()])).unwrap();
        assert_eq!(ended["busy"], json!([]));
        assert_eq!(ended["ended"][0]["status"], "done");
        assert_eq!(ended["ended"][0]["count"], 3);
        assert_eq!(w.change((1, vec![done])), None);
    }

    #[test]
    fn switching_library_does_not_report_the_old_tasks_as_ended() {
        let mut w = watcher();
        w.change((1, vec![item("provenance", "running", true)]));
        let switched = w.change((2, vec![item("provenance", "idle", false)])).unwrap();
        assert_eq!(switched, json!({"busy": [], "ended": []}));
    }

    #[test]
    fn active_lists_match_the_task_panel() {
        let jobs = include_str!("../../../web/static/core/jobs.js");
        for (name, list) in [
            ("SYNC_ACTIVE", SYNC_ACTIVE),
            ("INDEX_ACTIVE", INDEX_ACTIVE),
            ("NAMES_ACTIVE", NAMES_ACTIVE),
            ("SIMILAR_ACTIVE", SIMILAR_ACTIVE),
            ("PROV_ACTIVE", PROV_ACTIVE),
            ("IMPORT_ACTIVE", IMPORT_ACTIVE),
        ] {
            let quoted: Vec<String> = list.iter().map(|s| format!("'{s}'")).collect();
            let line = format!("export const {name} = [{}];", quoted.join(", "));
            assert!(jobs.contains(&line), "{name} 与 jobs.js 不一致：{line}");
        }
    }

    #[test]
    fn every_panel_task_has_an_activity_source() {
        use std::collections::BTreeSet;
        let tasks = include_str!("../../../web/static/ui/tasks.js");
        let packs = include_str!("../../../web/static/ui/packs.js");
        let entries = regex::Regex::new(r"key: '([a-z]+)',\s*name:").unwrap();
        let panel: BTreeSet<_> = [tasks, packs]
            .into_iter()
            .flat_map(|s| entries.captures_iter(s))
            .map(|c| c.get(1).unwrap().as_str())
            .collect();
        assert_eq!(SOURCES.iter().map(|s| s.key).collect::<BTreeSet<_>>(), panel);
    }

    #[test]
    fn pack_download_and_library_work_have_independent_end_events() {
        let event = |shared, kind: &str, status: &str| {
            SOURCES
                .iter()
                .find(|s| s.key == "pack" && s.across_libraries == shared)
                .unwrap()
                .observe(&json!({"kind":kind,"status":status,"message":"private/file.zip"}).into())
        };
        for operation in ["enable", "withdraw"] {
            let mut w = watcher();
            let download = event(true, "download", "running");
            let check = event(false, operation, "running");
            let both = w.change((1, vec![download.clone(), check])).unwrap();
            assert_eq!(both["busy"].as_array().unwrap().len(), 2);
            let end = w.change((1, vec![download, event(false, operation, "done")])).unwrap();
            assert_eq!(end["ended"][0]["variant"], operation);
            assert_eq!(end["busy"][0]["variant"], "download");
            let switched = w.change((2, vec![event(true, "download", "done"), event(false, "", "")])).unwrap();
            assert_eq!(switched["ended"][0]["variant"], "download");
            assert_eq!(switched["ended"][0]["reason"], "");
            assert_eq!(switched["busy"], json!([]));
        }
    }

    #[test]
    fn reason_is_trimmed_and_picks_the_first_present_field() {
        let value = Json::object([("status", "error".into()), ("error", "x".repeat(300).into())]);
        assert_eq!(task("export", RUNNING, &value).reason.chars().count(), 200);
        let value = Json::object([("status", "failed".into()), ("reason", "限流".into()), ("error", "其他".into())]);
        assert_eq!(task("provenance", PROV_ACTIVE, &value).reason, "限流");
    }
}
