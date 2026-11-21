//! native/terms 的集成测试合成的一个测试二进制（#310）。以前 `tests/` 下每个文件各自编译、各自链接一次整个库；
//! 现在各文件是这里的子模块，改了 `src` 之后只重编、重链接这一个。`tests/contracts.rs` 和 `tests/baseline.rs`
//! 仍是单独的 target，理由写在各自开头。新的集成测试放进本目录，在下面登记为子模块。
//!
//! 测试框架在同一个进程里多线程运行这些测试。有的测试要改进程环境变量或当前目录（库按 `ILLUSTASH_APP_SUPPORT`
//! 等找目录），以前靠「一个二进制只有一个测试」保证不与别的测试竞争；现在由这里的读写锁保证：改环境的测试第一行取
//! [`exclusive`]，其余测试第一行取 [`shared`]，于是改环境时没有别的测试在跑，结束时环境和当前目录恢复原样。
//! 每个测试都必须取其中一个，由 [`every_test_takes_the_environment_lock`] 检查。
#![allow(clippy::unwrap_used, reason = "集成测试里 unwrap 就是断言")]

mod app_service;
mod baseline_projection;
mod build_script;
mod character_projection;
mod entry;
mod integrity;
mod package_dir;
mod rescue;
mod v8_upgrade;

// 与 `contracts.rs`、`baseline.rs` 共用的辅助代码，文件留在 `tests/common/`；子模块经 `crate::common::…` 使用。
#[path = "../common/baseline.rs"]
mod common_baseline;
#[path = "../common/entry.rs"]
mod common_entry;
#[path = "../common/wait.rs"]
mod common_wait;
mod common {
    pub(crate) use super::{common_baseline as baseline, common_entry as entry, common_wait as wait};
}

use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

static ENVIRONMENT: RwLock<()> = RwLock::new(());

/// 不改进程环境的测试持有它：可以与别的 [`shared`] 测试同时运行，但不会与 [`exclusive`] 测试同时运行。
pub struct Shared {
    _lock: RwLockReadGuard<'static, ()>,
}

/// 要改进程环境变量或当前目录的测试持有它：期间没有别的测试在跑；释放时把环境变量和当前目录恢复成取得时的样子。
pub struct Exclusive {
    vars: Vec<(OsString, OsString)>,
    cwd: PathBuf,
    _lock: RwLockWriteGuard<'static, ()>,
}

/// 一个测试失败（panic）时锁会中毒；忽略中毒，免得后面的测试都跟着失败、盖住真正失败的那一个。
pub fn shared() -> Shared {
    Shared { _lock: ENVIRONMENT.read().unwrap_or_else(PoisonError::into_inner) }
}

/// 见 [`Exclusive`]；忽略中毒的理由见 [`shared`]。
pub fn exclusive() -> Exclusive {
    let lock = ENVIRONMENT.write().unwrap_or_else(PoisonError::into_inner);
    Exclusive { vars: std::env::vars_os().collect(), cwd: std::env::current_dir().unwrap(), _lock: lock }
}

impl Drop for Exclusive {
    fn drop(&mut self) {
        // 先于 `_lock` 释放执行：恢复完之前别的测试还进不来。
        let before: std::collections::HashMap<_, _> = self.vars.iter().cloned().collect();
        for (key, _) in std::env::vars_os() {
            if !before.contains_key(&key) {
                std::env::remove_var(key);
            }
        }
        for (key, value) in &self.vars {
            if std::env::var_os(key).as_ref() != Some(value) {
                std::env::set_var(key, value);
            }
        }
        let _ = std::env::set_current_dir(&self.cwd);
    }
}

/// 子模块里每个 `#[test]` 都取了 [`shared`] 或 [`exclusive`]：漏取的测试可能在别的测试改环境时运行。
#[test]
fn every_test_takes_the_environment_lock() {
    let _env = shared();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it");
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let mut lines = text.lines();
        while let Some(line) = lines.next() {
            if line.trim() != "#[test]" {
                continue;
            }
            // 跳过其他属性，找到函数签名的下一行。
            let signature = lines.by_ref().find(|l| !l.trim_start().starts_with("#[")).unwrap_or_default();
            let first = lines.next().unwrap_or_default().trim();
            if first != "let _env = crate::shared();"
                && first != "let _env = crate::exclusive();"
                && first != "let _env = shared();"
            {
                missing.push(format!("{}: {}", path.file_name().unwrap().to_string_lossy(), signature.trim()));
            }
        }
    }
    assert!(missing.is_empty(), "these tests take neither crate::shared() nor crate::exclusive() first: {missing:?}");
}
