//! 合成的客户端：没有 HTTP 服务器、Tauri 运行时、Python 或研究进程。
#![allow(clippy::unwrap_used, reason = "集成测试里 unwrap 就是断言")]
use illustash_terms::{
    app_create::NewSource,
    app_service::{self, Application, Options},
    app_tasks::{Task, TaskIdentity},
    store, Error, Kind,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    time::Duration,
};

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "illustash-service-{}-{}",
            std::process::id(),
            store::new_key().unwrap()
        ));
        fs::create_dir_all(root.join("source")).unwrap();
        // 测试持有 `crate::exclusive()`：修改环境变量不会与其他测试竞争。
        std::env::set_var("ILLUSTASH_APP_SUPPORT", root.join("support"));
        std::env::set_var("ILLUSTASH_SHARED", root.join("absent-reference"));
        std::env::set_var("ILLUSTASH_NO_KEYCHAIN", "1");
        Self { root: illustash_platform::app_paths::canonical(&root).unwrap() }
    }
    fn create(&self, name: &str) -> PathBuf {
        app_service::create(
            &self.root,
            &self.root.join(format!("{name}.pilib")),
            name,
            vec![NewSource::Folder { path: self.root.join("source"), preset: "other".into() }],
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}
fn options(root: &Path) -> Options {
    let mut options = Options::new(root.to_owned());
    options.startup.names_enabled = false;
    options.startup.provenance_enabled = false;
    options.startup.index_auto_check = false;
    options.startup.auto_sync = false;
    options.startup.sync_disabled = true;
    options
}

#[test]
fn creation_open_same_id_switch_late_work_and_exit_without_transport() {
    let _env = crate::exclusive();
    let f = Fixture::new();
    let a = f.create("first");
    let b = f.create("second");
    let config = fs::read(a.join("library.json")).unwrap();
    // 故意让两个不同的图库包使用同一个用户可见的 ID。
    let original: serde_json::Value = serde_json::from_slice(&config).unwrap();
    let mut second: serde_json::Value = serde_json::from_slice(&fs::read(b.join("library.json")).unwrap()).unwrap();
    second["id"] = original["id"].clone();
    fs::write(b.join("library.json"), serde_json::to_vec(&second).unwrap()).unwrap();
    #[cfg(unix)]
    let active = {
        let alias = f.root.join("active.pilib");
        std::os::unix::fs::symlink(&a, &alias).unwrap();
        alias
    };
    #[cfg(not(unix))]
    let active = a.clone();
    let service = Arc::new(Application::open(&active, options(&f.root)).unwrap());
    #[cfg(unix)]
    {
        // 打开被接受之后，把显示用的别名改指向同 ID 的另一个图库包。
        fs::remove_file(&active).unwrap();
        std::os::unix::fs::symlink(&b, &active).unwrap();
    }
    service.start().unwrap();
    let generation = service.generation();
    let identity =
        service.with_library(generation, |lib| TaskIdentity::new(lib, generation, "synthetic-session")).unwrap();
    let (go_tx, go_rx) = mpsc::channel();
    let task_service = service.clone();
    let (done_tx, done_rx) = mpsc::channel();
    let mut worker = Task::start(identity.clone(), None, move |context| {
        loop {
            context.check()?;
            match go_rx.recv_timeout(Duration::from_millis(10)) {
                Ok(()) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Ok(()),
            }
        }
        let result = context
            .receive(&identity, || task_service.with_library::<()>(generation, |_| panic!("stale worker applied")));
        done_tx.send(result.is_err()).unwrap();
        Ok(())
    });
    // 现有的 Core 写入和撤销在已接受实例的服务守卫下执行。
    service
        .with_library(generation, |lib| {
            assert_eq!(lib.db(), lib.instance_path().join("library.sqlite"));
            assert_eq!(lib.instance_path(), a);
            let mut c = illustash_terms::open(&lib.db())?;
            let asset = store::register_asset(&c, "synthetic", "service-image", None)?;
            let term = store::create_term(&c, Kind::Label, Some("service-label"), "user")?;
            let action = store::judge(
                &mut c,
                &[store::VerdictTarget::Image { asset, term }],
                store::Verdict::Yes,
                "user",
                "",
                "test",
            )?;
            assert_eq!(store::state(&c, asset, term)?.as_deref(), Some("confirmed"));
            store::undo(&mut c, Some(action))?;
            assert_eq!(store::state(&c, asset, term)?, None);
            Ok(())
        })
        .unwrap();
    assert!(service.switch(generation, &f.root.join("missing.pilib")).is_err());
    assert_eq!(service.generation(), generation);
    service.switch(generation, &b).unwrap();
    assert_eq!(service.generation(), generation + 1);
    assert!(matches!(
        service.with_library::<()>(generation, |_| panic!("stale request applied")),
        Err(Error::Conflict(_))
    ));
    assert!(service.switch(generation, &a).is_err());
    service
        .with_library(service.generation(), |lib| {
            assert_eq!(lib.id(), original["id"].as_str().unwrap());
            assert_ne!(lib.instance_path(), a);
            assert_eq!(lib.instance_path(), b);
            let c = illustash_terms::open(&lib.db())?;
            assert_eq!(
                c.query_row("SELECT COUNT(*) FROM terms WHERE key=?", ["service-label"], |row| row.get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
    go_tx.send(()).unwrap();
    assert!(done_rx.recv_timeout(crate::common::wait::limit()).unwrap());
    assert!(worker.wait(crate::common::wait::limit()));
    service.recover();
    service.shutdown();
    service.shutdown();
    assert!(service.start().is_err());
    assert!(service.with_library(service.generation(), |_| Ok(())).is_err());
    assert!(service.switch(service.generation(), &a).is_err());
    drop(worker);
    drop(service);
    // 创建失败时不能删掉已存在的已发布图库包或它的元数据。
    assert!(app_service::create(
        &f.root,
        &a,
        "first",
        vec![NewSource::Folder { path: f.root.join("source"), preset: "other".into() }]
    )
    .is_err());
    assert_eq!(config, fs::read(a.join("library.json")).unwrap());
    // 相对路径的目标登记的是创建时实际的绝对路径，当前目录改变后仍然有效。
    struct Cwd(std::path::PathBuf);
    impl Drop for Cwd {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.0).unwrap();
        }
    }
    let _cwd = Cwd(std::env::current_dir().unwrap());
    std::env::set_current_dir(&f.root).unwrap();
    let actual = app_service::create(
        &f.root,
        Path::new("relative.pilib"),
        "relative",
        vec![NewSource::Folder { path: f.root.join("source"), preset: "other".into() }],
    )
    .unwrap();
    assert_eq!(actual, f.root.join("relative.pilib"));
    fs::create_dir(f.root.join("different-cwd")).unwrap();
    std::env::set_current_dir(f.root.join("different-cwd")).unwrap();
    let entries = illustash_terms::desktop_shell::recent();
    assert!(entries.iter().any(|entry| entry["path"].as_str() == actual.to_str()));
    Application::open(&actual, options(&f.root)).unwrap().shutdown();
}
