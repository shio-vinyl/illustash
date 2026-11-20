//! 桌面外壳复用 Rust CLI 的原生 HTTP 前门，并提供原生账号使用的 Pixiv 登录桥。
//! 核心命令直接执行、不创建窗口；窗口关闭等待宿主任务收尾。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod background;
mod bridge;
#[cfg(any(windows, target_os = "macos", test))]
mod bundled_workers;
mod drag;
mod i18n;
mod notify;
mod picker;
mod pixiv;
mod server;
mod transparency;
mod updater;
mod webview;
mod welcome;
mod window_state;

#[cfg(not(windows))]
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// 主窗口的最小尺寸（逻辑像素）。
const MIN_SIZE: (f64, f64) = (720.0, 480.0);

fn main() {
    #[cfg(any(windows, target_os = "macos"))]
    let computation = bundled_workers::configure();
    #[cfg(not(any(windows, target_os = "macos")))]
    let computation: Result<(), String> = Ok(());
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| illustash_terms::cli::is_command(a)) {
        if let Err(error) = &computation {
            eprintln!("{error}");
            std::process::exit(2);
        }
        std::process::exit(illustash_terms::cli::run(&args).into());
    }
    illustash_terms::diagnostics::initialize();
    let builder = tauri::Builder::default();
    // 窗口隐藏在后台时再从开始菜单打开 Illustash：回到已在运行的那个，不另开第二个去抢同一个图库（#451）。
    // macOS 由系统负责，不会另开。
    #[cfg(windows)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| background::reveal(app)));
    let app = builder
        .plugin(tauri_plugin_dialog::init())
        // 应用内更新（#279）：只用它的 Rust 接口（updater.rs），页面没有调用它的权限。
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updater::Updates::default())
        .manage(pixiv::Pixiv::default())
        .manage(server::Child::default())
        .manage(server::ErrorPage::default())
        .manage(server::StartupPage::default())
        .manage(server::Launch::default())
        .manage(welcome::Import::default())
        .manage(welcome::Requests::default())
        .manage(window_state::Memory::default())
        .manage(background::Background::default())
        .invoke_handler(tauri::generate_handler![
            pixiv::pixiv_reply,
            welcome::welcome_pick,
            welcome::welcome_open,
            welcome::welcome_create,
            welcome::welcome_inspect,
            welcome::welcome_import,
            welcome::welcome_import_status,
            welcome::welcome_import_cancel,
            welcome::startup_choose,
            welcome::startup_retry,
            welcome::startup_upgrade,
            welcome::startup_rescue,
            welcome::startup_diagnostics
        ])
        .on_menu_event(|app, event| {
            // 只对图库页：刷新 Pixiv 窗口会打断在途的请求。
            let Some(main) = app.get_webview_window("main") else { return };
            match event.id().as_ref() {
                "reload" => {
                    let _ = main.reload();
                }
                // 后退／前进交给页面（core/route.js 的 stepHistory）：它只在图库页自己的地址之间走，不会退回启动页或
                // 上一个图库；启动页、欢迎页上没有这个函数，什么都不做（#402）。
                "back" => {
                    let _ = main.eval("window.illustashHistory && window.illustashHistory(-1)");
                }
                "forward" => {
                    let _ = main.eval("window.illustashHistory && window.illustashHistory(1)");
                }
                _ => {}
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            #[cfg(windows)]
            updater::clean_leftovers(&handle);
            #[cfg(not(windows))]
            {
                // macOS 保留原生的编辑/退出菜单和刷新命令；后退／前进用 Safari 的 ⌘[、⌘]（#402）。
                let menu = Menu::default(&handle)?;
                let reload = MenuItem::with_id(app, "reload", i18n::tr("刷新页面"), true, Some("CmdOrCtrl+R"))?;
                let back = MenuItem::with_id(app, "back", i18n::tr("后退"), true, Some("CmdOrCtrl+["))?;
                let forward = MenuItem::with_id(app, "forward", i18n::tr("前进"), true, Some("CmdOrCtrl+]"))?;
                // 默认菜单已经有 View 菜单（全屏）：这几项加进这个菜单，不另建第二个。
                let view = menu
                    .items()?
                    .into_iter()
                    .filter_map(|item| item.as_submenu().cloned())
                    .find(|submenu| submenu.text().is_ok_and(|text| text == "View"));
                match view {
                    Some(view) => {
                        view.append(&PredefinedMenuItem::separator(app)?)?;
                        view.append_items(&[&back, &forward, &PredefinedMenuItem::separator(app)?, &reload])?;
                    }
                    None => menu.append(&Submenu::with_items(
                        app,
                        i18n::tr("视图"),
                        true,
                        &[&back, &forward, &PredefinedMenuItem::separator(app)?, &reload],
                    )?)?,
                }
                app.set_menu(menu)?;
            }
            let reduced_transparency = transparency::initial();
            let mut main = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Illustash")
                .initialization_script(i18n::page_script());
            if let Some(script) = reduced_transparency.map(transparency::script) {
                main = main.initialization_script(script);
            }
            let main = webview::build(
                main.inner_size(1280.0, 840.0)
                    .min_inner_size(MIN_SIZE.0, MIN_SIZE.1)
                    // 先不显示：放回记下的位置和大小以后再显示，免得窗口先在默认位置闪一下（#451）。
                    .visible(false)
                    .on_page_load(|window, payload| {
                        if payload.event() == tauri::webview::PageLoadEvent::Finished {
                            server::page_loaded(&window);
                        }
                    })
                    .on_navigation(server::keep_inside)
                    .on_new_window(|url, _| {
                        server::open_outside(&url);
                        tauri::webview::NewWindowResponse::Deny
                    }),
            )?;
            transparency::observe(&handle, reduced_transparency);
            window_state::restore(&main, MIN_SIZE);
            main.show()?;
            if let Err(error) = &computation {
                server::blocked(&main, error);
                return Ok(());
            }
            let token = bridge::token();
            let port = match bridge::start(handle.clone(), token.clone()) {
                Ok(port) => port,
                Err(error) => {
                    server::blocked(
                        &main,
                        &i18n::tr_with("无法启动本机登录通道：{error}；请重新启动应用", &[("error", &error)]),
                    );
                    return Ok(());
                }
            };
            server::boot(handle, main, port, token);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("启动桌面外壳失败");
    app.run(|handle, event| {
        match event {
            RunEvent::WindowEvent { label, event: WindowEvent::CloseRequested { api, .. }, .. } if label == "main" => {
                api.prevent_close();
                // 有任务在跑就隐藏窗口、在后台做完（#451）；否则退出：隐藏的 Pixiv 窗口不能让服务一直活着。
                if !background::close_requested(handle) {
                    handle.exit(0);
                }
            }
            // 点 Dock 图标回到隐藏着的窗口。
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => background::reveal(handle),
            RunEvent::Exit => {
                window_state::save(handle);
                if let Some(child) = handle.try_state::<server::Child>() {
                    child.stop();
                }
                illustash_terms::diagnostics::logs::flush();
            }
            _ => {}
        }
    });
}
