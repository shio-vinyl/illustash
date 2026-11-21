//! 在主线程用真实 WKUserContentController 验证初始化脚本的更新。
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_foundation::NSString;
use objc2_web_kit::{WKUserContentController, WKUserScript, WKUserScriptInjectionTime};

use super::{platform::replace_user_script, script};

pub fn replaces_only_transparency_script() {
    let mtm = MainThreadMarker::new().expect("WKUserContentController 须在主线程构造");
    // SAFETY: 已持有主线程标记，只构造独立的 WebKit 脚本控制器。
    let controller = unsafe { WKUserContentController::new(mtm) };
    let unrelated = ["window.first = true;", "window.second = true;"];
    let previous = script(false);
    for source in [unrelated[0], previous.as_str(), unrelated[1]] {
        // SAFETY: 主线程上构造并注册测试脚本，控制器持有脚本。
        unsafe {
            let entry = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                WKUserScript::alloc(mtm),
                &NSString::from_str(source),
                WKUserScriptInjectionTime::AtDocumentEnd,
                false,
            );
            controller.addUserScript(&entry);
        }
    }

    for reduced in [true, false, true] {
        let expected = script(reduced);
        replace_user_script(&controller, mtm, &expected);
        // SAFETY: 只在主线程读取控制器快照和其中仍有强引用的脚本。
        unsafe {
            let scripts = controller.userScripts().to_vec();
            assert_eq!(scripts.len(), 3);
            for (entry, source) in scripts.iter().zip(unrelated) {
                assert_eq!(entry.source().to_string(), source);
                assert_eq!(entry.injectionTime(), WKUserScriptInjectionTime::AtDocumentEnd);
                assert!(!entry.isForMainFrameOnly());
            }
            let entry = &scripts[2];
            assert_eq!(entry.source().to_string(), expected);
            assert_eq!(entry.injectionTime(), WKUserScriptInjectionTime::AtDocumentStart);
            assert!(entry.isForMainFrameOnly());
        }
    }
}
