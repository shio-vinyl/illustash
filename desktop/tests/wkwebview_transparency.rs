//! WebKit 集成测试必须在主线程运行，且无需启动 App 或创建窗口。
#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use libtest_mimic::{Arguments, Trial};

    let args = Arguments::from_args();
    // libtest-mimic 的单线程模式在调用线程执行。
    let args = Arguments { test_threads: Some(1), ..args };
    let result = libtest_mimic::run(
        &args,
        vec![Trial::test("wkwebview_transparency::replaces_only_transparency_script", || {
            transparency::webkit_test::replaces_only_transparency_script();
            Ok(())
        })],
    );
    result.exit();
}

#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/transparency.rs"]
mod transparency;
