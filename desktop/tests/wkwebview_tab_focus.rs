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

const TEST_NAME: &str = "wkwebview_tab_focus::enables_tab_focuses_links";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // 兼容 `tools/quarantine.py cargo-test` 传入的 `--list` 与 `--skip` 参数。
    if args.iter().any(|arg| arg == "--list") {
        #[cfg(target_os = "macos")]
        println!("{TEST_NAME}: test");
        return;
    }
    let mut skip_next = false;
    let mut skipped = false;
    let mut exact = false;
    let mut filters = Vec::new();
    for arg in &args {
        if skip_next {
            if arg == TEST_NAME {
                skipped = true;
            }
            skip_next = false;
            continue;
        }
        if arg == "--skip" {
            skip_next = true;
        } else if arg == "--exact" {
            exact = true;
        } else if !arg.starts_with('-') {
            filters.push(arg.as_str());
        }
    }
    if skipped
        || (!filters.is_empty() && !filters.iter().any(|f| if exact { TEST_NAME == *f } else { TEST_NAME.contains(f) }))
    {
        return;
    }

    #[cfg(target_os = "macos")]
    {
        let mtm = objc2::MainThreadMarker::new().expect("wkwebview_tab_focus 须在主线程运行");
        // SAFETY: 已持有 MainThreadMarker，仅在内存中构造 WKWebView 并读写公开的 tabFocusesLinks 偏好。
        unsafe {
            let wk = objc2_web_kit::WKWebView::new(mtm);
            assert!(
                !wk.configuration().preferences().tabFocusesLinks(),
                "WKWebView 初始默认 tabFocusesLinks 应为 false，否则无法证明显式设置生效"
            );
            webview::enable_wk_tab_focuses_links(&wk);
            assert!(
                wk.configuration().preferences().tabFocusesLinks(),
                "调用 enable_wk_tab_focuses_links 后 WKWebView 的 tabFocusesLinks 应为 true"
            );
        }
        println!("test {TEST_NAME} ... ok");
        println!("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out");
    }
}
