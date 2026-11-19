//! 把系统的「降低透明度」设置同步给主窗口页面（#755）。
//!
//! - macOS：WKWebView（WebKit，含 Safari 17/18 及后续版本）不支持 `@media (prefers-reduced-transparency)`，
//!   由外壳读取 `NSWorkspace.accessibilityDisplayShouldReduceTransparency`，并在
//!   `NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification` 触发时实时同步到根元素 `<html>` 的
//!   `data-reduce-transparency` 特性，不重启应用也不重新载入页面；同时更新 `WKUserContentController` 里的
//!   初始化脚本，确保导航或刷新后仍在首次绘制前生效。
//! - Windows：Edge WebView2 基于 Chromium（118+ 原生支持 `prefers-reduced-transparency`，映射系统的
//!   `EnableTransparency` /「透明效果」开关），无需外壳额外注入。
//! - 其他平台（Linux 已冻结）：不做处理。
use tauri::AppHandle;

/// 本模块写入的初始化脚本前缀：设置变更时按此标记替换旧脚本，不干扰 Tauri 与 i18n 等其他初始化脚本。
const SCRIPT_MARKER: &str = "/* illustash:reduce-transparency */\n";

/// 启动主窗口前读取当前平台是否需要由外壳同步，以及初始的「降低透明度」状态。
pub fn initial() -> Option<bool> {
    platform::initial()
}

/// 生成把「降低透明度」状态写入 `document.documentElement.dataset.reduceTransparency` 的脚本：
/// - 开启时设为 `''`（存在 `data-reduce-transparency` 特性即视为开启）；
/// - 关闭时删除该特性。
pub fn script(reduced: bool) -> String {
    let body = if reduced {
        "document.documentElement.dataset.reduceTransparency = '';"
    } else {
        "delete document.documentElement.dataset.reduceTransparency;"
    };
    format!("{SCRIPT_MARKER}if (document.documentElement) {{ {body} }}\n")
}

/// 开始监听系统「降低透明度」设置变化，并在变化时同步到主窗口（需要在主线程的 setup 阶段调用）。
pub fn observe(app: &AppHandle, initial: Option<bool>) {
    platform::observe(app, initial);
}

#[cfg(any(target_os = "macos", test))]
fn is_managed_script(source: &str) -> bool {
    source.starts_with(SCRIPT_MARKER)
}

/// 过滤掉此前由本模块写入的初始化脚本，保留其余脚本的原始顺序。
#[cfg(any(target_os = "macos", test))]
fn keep_unmanaged<T>(items: impl IntoIterator<Item = T>, source_of: impl Fn(&T) -> String) -> Vec<T> {
    items.into_iter().filter(|item| !is_managed_script(&source_of(item))).collect()
}

/// 记录最近一次已同步给页面的状态；系统发来其他辅助功能显示选项变化时，状态未变则忽略。
#[cfg(any(target_os = "macos", test))]
struct Tracker {
    reduced: bool,
}

#[cfg(any(target_os = "macos", test))]
impl Tracker {
    fn new(initial: bool) -> Self {
        Self { reduced: initial }
    }

    /// 仅当新状态与已同步状态不同时更新并返回 `Some(next)`。
    fn update(&mut self, next: bool) -> Option<bool> {
        if self.reduced == next {
            None
        } else {
            self.reduced = next;
            Some(next)
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::sync::{Arc, Mutex};

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{NSObjectProtocol, ProtocolObject};
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification};
    use objc2_foundation::{NSNotification, NSOperationQueue, NSString};
    use objc2_web_kit::{WKUserContentController, WKUserScript, WKUserScriptInjectionTime};
    use tauri::{AppHandle, Manager};

    use super::{keep_unmanaged, script, Tracker};

    thread_local! {
        /// 通知中心返回的观察者令牌：留在这里，应用存活期间持续接收系统通知。
        static OBSERVER: RefCell<Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>> = const { RefCell::new(None) };
    }

    pub(super) fn system_reduced() -> bool {
        NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceTransparency()
    }

    pub(super) fn initial() -> Option<bool> {
        Some(system_reduced())
    }

    pub(super) fn observe(app: &AppHandle, initial: Option<bool>) {
        let Some(initial) = initial else { return };
        let tracker = Arc::new(Mutex::new(Tracker::new(initial)));
        let registered = OBSERVER.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_some() {
                return false;
            }
            let handle = app.clone();
            let state = Arc::clone(&tracker);
            let block = RcBlock::new(move |_notification: NonNull<NSNotification>| {
                sync(&handle, &state);
            });
            let center = NSWorkspace::sharedWorkspace().notificationCenter();
            let queue = NSOperationQueue::mainQueue();
            // SAFETY: `NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification` 是 AppKit 导出的静态通知名，
            // 在进程整个生命周期内有效。
            let name = unsafe { NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification };
            // SAFETY: `name` 为有效的系统通知名，`obj` 为 `None` 表示接收任意发送者，`queue` 为主队列以保证回调
            // 在主线程执行，`block` 仅捕获可跨线程发送的 `AppHandle` 和 `Arc<Mutex<Tracker>>`。
            let token =
                unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), None, Some(&queue), &block) };
            *slot = Some(token);
            true
        });
        if registered {
            // 捕获构造窗口与注册监听之间的短暂窗口期内可能发生的切换。
            sync(app, &tracker);
        }
    }

    fn sync(app: &AppHandle, tracker: &Mutex<Tracker>) {
        let current = system_reduced();
        let next = tracker.lock().expect("transparency tracker lock").update(current);
        let Some(reduced) = next else { return };
        let Some(main) = app.get_webview_window("main") else { return };
        let js = script(reduced);
        let init_js = js.clone();
        // 先更新初始化脚本（供之后的导航和刷新在首次绘制前生效），再在当前页面即时执行，不重新载入页面。
        let _ = main.with_webview(move |webview| replace_user_script(webview.controller(), &init_js));
        let _ = main.eval(&js);
    }

    /// 用最新脚本替换本模块先前注入的 `WKUserScript`，保留 Tauri 与 i18n 等其他脚本的原始顺序。
    fn replace_user_script(controller: *mut c_void, js: &str) {
        if controller.is_null() {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        // SAFETY: `controller` 来自 Tauri `PlatformWebview::controller()`，指向主窗口的 `WKUserContentController`，
        // 且 `with_webview` 回调在主线程上运行，这里已通过 `MainThreadMarker` 确认主线程并检查非空。
        let controller: &WKUserContentController = unsafe { &*(controller as *const WKUserContentController) };
        // SAFETY: 在主线程读取 `WKUserContentController` 的 `userScripts` 快照数组。
        let existing = unsafe { controller.userScripts() }.to_vec();
        let kept = keep_unmanaged(existing, |item| {
            // SAFETY: `item` 是从 `userScripts` 强引用持有的有效 `WKUserScript`，在主线程读取只读属性 `source`。
            unsafe { item.source() }.to_string()
        });
        // SAFETY: 在主线程创建新的 `WKUserScript`，在文档开始前（首次绘制之前）仅对主框架注入，与 `initialization_script` 一致。
        let fresh = unsafe {
            WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                WKUserScript::alloc(mtm),
                &NSString::from_str(js),
                WKUserScriptInjectionTime::AtDocumentStart,
                true,
            )
        };
        // SAFETY: 在主线程重建脚本列表；`kept` 与 `fresh` 持有强引用，清空后再按原顺序添加回去是安全的。
        unsafe {
            controller.removeAllUserScripts();
            for item in &kept {
                controller.addUserScript(item);
            }
            controller.addUserScript(&fresh);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use tauri::AppHandle;

    pub(super) fn initial() -> Option<bool> {
        None
    }

    pub(super) fn observe(_app: &AppHandle, _initial: Option<bool>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_sets_or_deletes_data_reduce_transparency_with_marker() {
        let on = script(true);
        assert!(is_managed_script(&on));
        assert!(on.contains("document.documentElement.dataset.reduceTransparency = '';"));
        assert!(!on.contains("delete "));

        let off = script(false);
        assert!(is_managed_script(&off));
        assert!(off.contains("delete document.documentElement.dataset.reduceTransparency;"));
        assert!(!off.contains("= ''"));
    }

    #[test]
    fn tracker_emits_only_on_actual_state_changes() {
        let mut tracker = Tracker::new(false);
        // 其他辅助功能选项变化但「降低透明度」未变时，不重复触发同步。
        assert_eq!(tracker.update(false), None);
        assert_eq!(tracker.update(true), Some(true));
        assert_eq!(tracker.update(true), None);
        assert_eq!(tracker.update(false), Some(false));
        assert_eq!(tracker.update(false), None);
        assert_eq!(tracker.update(true), Some(true));

        let mut started_on = Tracker::new(true);
        assert_eq!(started_on.update(true), None);
        assert_eq!(started_on.update(false), Some(false));
    }

    #[test]
    fn replacing_managed_script_preserves_other_init_scripts_in_order() {
        let tauri_ipc = "window.__TAURI_INTERNALS__ = {};".to_owned();
        let i18n_script = "if (location.protocol === 'tauri:') { self.ILLUSTASH_LANGUAGE = \"zh-CN\"; }".to_owned();
        let old_script = script(false);
        let new_script = script(true);

        let mut scripts = keep_unmanaged([tauri_ipc.clone(), i18n_script.clone(), old_script], |s| s.clone());
        scripts.push(new_script.clone());

        assert_eq!(scripts, vec![tauri_ipc.clone(), i18n_script.clone(), new_script]);

        // 连续切换多次后不会累积旧脚本，且其他脚本顺序保持不变。
        let back_off = script(false);
        let mut scripts_again = keep_unmanaged(scripts, |s| s.clone());
        scripts_again.push(back_off.clone());
        assert_eq!(scripts_again, vec![tauri_ipc, i18n_script, back_off]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_can_read_system_reduce_transparency_setting() {
        // 只读冒烟：确认系统 API 可正常查询，且 `initial()` 返回与系统一致的 `Some(bool)`。
        assert_eq!(initial(), Some(platform::system_reduced()));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_platforms_do_not_inject_init_script() {
        assert_eq!(initial(), None);
    }
}
