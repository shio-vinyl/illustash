//! 在主线程上验证 macOS `WKWebView` 的 `tabFocusesLinks` 偏好设置（#761）。
//!
//! Rust 默认测试运行器会在工作线程执行 `#[test]`，而 `WKWebView` / `WKPreferences` 只能在主线程构造；
//! 因此使用 `harness = false` 的独立测试入口在主线程运行，只构造内存中的 `WKWebView`，
//! 不启动 `NSApplication`，也不打开任何窗口。
#![allow(clippy::unwrap_used)]

#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/webview.rs"]
mod webview;

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use libtest_mimic::{Arguments, Trial};

    // libtest-mimic 的单线程模式在调用线程执行。
    let args = Arguments { test_threads: Some(1), ..Arguments::from_args() };
    libtest_mimic::run(
        &args,
        vec![Trial::test("wkwebview_tab_focus::enables_tab_focuses_links", || {
            enables_tab_focuses_links();
            Ok(())
        })],
    )
    .exit();
}

#[cfg(target_os = "macos")]
fn enables_tab_focuses_links() {
    let mtm = objc2::MainThreadMarker::new().expect("wkwebview_tab_focus 须在主线程运行");
    // SAFETY: 已持有 MainThreadMarker，仅在内存中构造 WKWebView 并读写公开的 tabFocusesLinks 偏好。
    unsafe {
        let wk = objc2_web_kit::WKWebView::new(mtm);
        // 显式关闭以模拟 wry 未开启该偏好的情况，避免测试依赖 WebKit 的默认值。
        wk.configuration().preferences().setTabFocusesLinks(false);
        assert!(!wk.configuration().preferences().tabFocusesLinks());
        webview::enable_wk_tab_focuses_links(&wk);
        assert!(wk.configuration().preferences().tabFocusesLinks());
    }
}
