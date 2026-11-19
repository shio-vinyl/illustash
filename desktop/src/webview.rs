//! 桌面外壳创建 WebView 窗口的统一入口（#761）。
//!
//! macOS 上显式打开 WKWebView 的 `tabFocusesLinks` 偏好，不依赖 wry 的默认值：
//! 系统关闭「键盘导航」时，WKWebView 默认按 Tab 只会经过输入框和按钮，走不到活页条和页面链接。
//! 统一在 `build()` 之后显式打开，也能避免升级或替换 wry 后默认值变化导致键盘导航悄悄失效。
use tauri::{Manager, WebviewWindow, WebviewWindowBuilder};

/// 创建桌面 WebView 窗口并统一应用外壳偏好。
pub(crate) fn build<M: Manager<tauri::Wry>>(
    builder: WebviewWindowBuilder<'_, tauri::Wry, M>,
) -> tauri::Result<WebviewWindow> {
    let window = builder.build()?;
    enable_tab_focuses_links(&window)?;
    Ok(window)
}

#[cfg(target_os = "macos")]
fn enable_tab_focuses_links(window: &WebviewWindow) -> tauri::Result<()> {
    window.with_webview(|webview| {
        // SAFETY: `with_webview` 在主线程上执行，且 `webview.inner()` 是 Tauri 持有的有效 `WKWebView` 指针。
        if let Some(wk) = unsafe { webview.inner().cast::<objc2_web_kit::WKWebView>().as_ref() } {
            enable_wk_tab_focuses_links(wk);
        }
    })
}

/// 打开单个 `WKWebView` 的 `tabFocusesLinks` 偏好（macOS 10.12.4+ 公开 API）。
/// `WKWebView.configuration` 返回浅拷贝，但仍指向该视图正在使用的同一个 `WKPreferences` 实例。
#[cfg(target_os = "macos")]
pub(crate) fn enable_wk_tab_focuses_links(wk: &objc2_web_kit::WKWebView) {
    // SAFETY: 调用方已确保位于主线程且 `wk` 有效；`configuration`、`preferences`、`setTabFocusesLinks:`
    // 均为 macOS 10.12.4+ 公开方法（外壳最低系统版本为 macOS 14.0）。
    unsafe {
        wk.configuration().preferences().setTabFocusesLinks(true);
    }
}

#[cfg(not(target_os = "macos"))]
fn enable_tab_focuses_links(_window: &WebviewWindow) -> tauri::Result<()> {
    // Windows 的 WebView2 基于 Chromium，默认允许 Tab 聚焦链接，无需额外设置（#761）。
    Ok(())
}

#[cfg(test)]
mod tests {
    /// 去掉 `#[cfg(test)]` 之后的内容与行注释，用于检查生产代码中的 WebView 创建入口。
    fn production_code(source: &str) -> String {
        let head = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        head.lines().map(|line| line.split("//").next().unwrap_or(line)).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn all_desktop_webviews_enable_tab_focuses_links() {
        use std::fs;
        use std::path::Path;

        let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut entries: Vec<_> = fs::read_dir(&src_dir)
            .expect("读取 desktop/src 目录失败")
            .map(|entry| entry.expect("读取目录项失败").path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("rs"))
            .collect();
        entries.sort();

        let mut total_builders = 0usize;
        let mut saw_webview_mod = false;
        for path in entries {
            let text = fs::read_to_string(&path).expect("读取源文件失败");
            let code = production_code(&text);
            let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();

            let created = code.matches("WebviewWindowBuilder::new(").count();
            let routed = code.matches("webview::build(").count();
            assert_eq!(
                created, routed,
                "{file_name} 创建了 {created} 个 WebviewWindowBuilder，但经 webview::build 构建了 {routed} 次；\
                 桌面外壳的 WebView 须统一经 webview::build 创建以显式启用 macOS 的 tabFocusesLinks（#761）"
            );
            total_builders += created;

            if file_name == "webview.rs" {
                saw_webview_mod = true;
                assert!(
                    code.contains("enable_tab_focuses_links(&window)"),
                    "webview::build 须调用 enable_tab_focuses_links"
                );
                assert!(
                    code.contains("setTabFocusesLinks(true)"),
                    "macOS 外壳须显式调用 WKPreferences::setTabFocusesLinks(true)（#761）"
                );
            }
        }
        assert!(saw_webview_mod, "未在 desktop/src 中找到 webview.rs");
        assert!(total_builders >= 2, "预期至少覆盖主窗口与 Pixiv 窗口两处 WebView 创建");
    }
}
