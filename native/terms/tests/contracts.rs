//! 原生宿主的固定 HTTP 契约，作为第一个公开测试版的对照基准（#201）：以前 Python 参考实现契约的四个场景，
//! 在词条核心第 7 版的 main 上录制（PR #239），对本次构建重放。这个可执行文件和它启动的宿主子进程都不需要 Python
//! （`tools/test_native_contracts.sh` 用空 PATH 运行它）。
//!
//! 每个场景先生成自己的合成图库包，在不继承任何环境变量的情况下启动宿主（与契约的做法相同），再重放请求。
//! 词条核心版本比本次构建旧的图库用 `--upgrade` 打开，与用户的图库相同（#193）：留一份升级前备份，保存图库原来的样子。
//! 响应经 `common/baseline.rs` 的遮盖（以及随机会话 token 的遮盖）后按字节比较；图库包的每个数据库在启动后和
//! 每次请求后都按那里的规则比较。
//!
//! 不并进 `tests/it/` 的合并测试二进制（#310）：`tools/check_all.sh`、`.github/workflows/ci.yml` 和
//! `tools/bench_native_contracts.sh` 按 `--test contracts` 单独构建这个可执行文件，再拿它做空 PATH 重放和计时。
#![allow(clippy::unwrap_used, reason = "集成测试里 unwrap 就是断言")]
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[path = "common/baseline.rs"]
mod baseline;
#[path = "common/entry.rs"]
mod entry;
#[path = "common/wait.rs"]
mod wait;

#[path = "contracts/calendar.rs"]
mod calendar;
#[path = "contracts/character_together.rs"]
mod character_together;
#[path = "contracts/library_50k.rs"]
mod library_50k;
#[path = "contracts/library_scale.rs"]
mod library_scale;

#[path = "contracts/colors.rs"]
mod colors;

const DATABASES: [&str; 3] = ["source.sqlite", "library.sqlite", "provenance.sqlite"];

struct Sandbox(PathBuf);
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Host {
    child: Child,
    reaped: bool,
}
impl Drop for Host {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn text_masks(root: &Path) -> impl Fn(&str) -> String + '_ {
    move |text: &str| {
        let real = root.to_str().unwrap();
        // 原样的写法，以及在 JSON 文本里转义后的写法（Windows 路径）。
        let escaped = json!(real).to_string();
        text.replace(&escaped[1..escaped.len() - 1], "@ROOT@").replace(real, "@ROOT@")
    }
}

fn states(package: &Path, root: &Path) -> BTreeMap<String, baseline::State> {
    states_at(package, root, illustash_terms::SCHEMA_VERSION)
}

/// 参与比较的内容；`version` 是当前的词条核心版本（录制的输入则用录制时的版本）。
fn states_at(package: &Path, root: &Path, version: i64) -> BTreeMap<String, baseline::State> {
    let roots = text_masks(root);
    let masks = baseline::Masks { roots: &roots, version };
    DATABASES
        .iter()
        .filter(|name| package.join(name).exists())
        .map(|name| {
            let c =
                Connection::open_with_flags(package.join(name), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
            (name.to_string(), baseline::state(&c, &masks))
        })
        .collect()
}

fn apply(
    before: &BTreeMap<String, baseline::State>,
    change: Option<&Value>,
) -> Result<BTreeMap<String, baseline::State>, String> {
    let mut out = before.clone();
    for (name, delta) in change.and_then(Value::as_object).into_iter().flatten() {
        let state = baseline::apply(out.get(name).unwrap_or(&baseline::State::new()), delta)?;
        out.insert(name.clone(), state);
    }
    Ok(out)
}

fn compare(
    want: &BTreeMap<String, baseline::State>,
    got: &BTreeMap<String, baseline::State>,
    at: &str,
    historical: Option<i64>,
) {
    let mut problems = Vec::new();
    for name in DATABASES {
        let (w, original) = (want.get(name), got.get(name));
        let projected = if let ("library.sqlite", Some(recorded), Some(got)) = (name, historical, original) {
            Some(
                baseline::historical_projection(
                    got,
                    w.unwrap_or(&baseline::State::new()),
                    recorded,
                    illustash_terms::SCHEMA_VERSION,
                )
                .unwrap_or_else(|error| panic!("{at}: {error}")),
            )
        } else {
            None
        };
        let g = projected.as_ref().or(original);
        if w != g {
            let empty = baseline::State::new();
            problems.extend(
                baseline::differences(w.unwrap_or(&empty), g.unwrap_or(&empty), 5)
                    .into_iter()
                    .map(|d| format!("{name}: {d}")),
            );
        }
    }
    assert!(problems.is_empty(), "{at}: databases differ from the baseline:\n{}", problems.join("\n"));
}

fn session_token(text: &str) -> String {
    regex::Regex::new(r#"("token":\s*)"[0-9a-f]{32,}""#)
        .unwrap()
        .replace_all(text, r#"$1"<session-token>""#)
        .into_owned()
}

fn request(origin: &str, url: &str, body: Option<&Value>, token: &str) -> (u16, String, Map<String, Value>) {
    let req = if body.is_some() { ureq::post(&format!("{origin}{url}")) } else { ureq::get(&format!("{origin}{url}")) }
        .set("Accept-Encoding", "identity")
        .set("Origin", origin)
        .set("Cookie", &entry::cookie(origin))
        .set("X-CSRF-Token", token);
    let result = if let Some(body) = body { req.send_json(body) } else { req.call() };
    let response = match result {
        Ok(r) | Err(ureq::Error::Status(_, r)) => r,
        Err(e) => panic!("HTTP {url}: {e}"),
    };
    let status = response.status();
    let mut headers = Map::new();
    for k in ["Content-Type", "Cache-Control", "X-Content-Type-Options", "Referrer-Policy"] {
        if let Some(v) = response.header(k) {
            headers.insert(k.into(), json!(v));
        }
    }
    (status, response.into_string().unwrap(), headers)
}

fn materialize(fixture: &Value, category: &str) -> (Sandbox, PathBuf, PathBuf) {
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce).unwrap();
    let scratch = Sandbox(std::env::temp_dir().join(format!(
        "illustash-contract-{}-{category}-{:x}",
        std::process::id(),
        u64::from_ne_bytes(nonce)
    )));
    fs::create_dir_all(&scratch.0).unwrap();
    let root = scratch.0.canonicalize().unwrap();
    // Windows 规范化路径带有 `\\?\` 原样前缀，而宿主从不产生这种前缀。
    #[cfg(windows)]
    let root = PathBuf::from(root.to_str().unwrap().strip_prefix(r"\\?\").unwrap());
    let package = materialize_at(fixture, &root);
    (scratch, root, package)
}

fn materialize_at(fixture: &Value, root: &Path) -> PathBuf {
    let package = root.join("fixture.pilib");
    for dir in ["fixture.pilib", "fixture.pilib/thumbnails", "folder", "bin", "home", "support", "shared"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    for (name, input) in fixture["input"].as_object().unwrap() {
        let path = package.join(name);
        if let Some(sql) = input["sql"].as_str() {
            let c = Connection::open(path).unwrap();
            c.pragma_update(None, "foreign_keys", false).unwrap();
            c.execute_batch(&sql.replace("@ROOT@", root.to_str().unwrap())).unwrap();
        } else {
            // 文本输入是 JSON 文档：根目录按 JSON 字符串的内容写进去。
            let encoded = json!(root.to_str().unwrap()).to_string();
            fs::write(path, input["text"].as_str().unwrap().replace("@ROOT@", &encoded[1..encoded.len() - 1])).unwrap();
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in ["python", "python3", "python3.12"] {
            let path = root.join("bin").join(name);
            fs::write(&path, "#!/bin/sh\n/bin/echo forbidden > \"$ILLUSTASH_PYTHON_SENTINEL\"\nexit 97\n").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    package
}

#[cfg(unix)]
fn benchmark_root() -> PathBuf {
    if cfg!(debug_assertions) {
        panic!("benchmark must be compiled --release");
    }
    std::env::var_os("ILLUSTASH_BENCH_ROOT").expect("benchmark fixture directory required").into()
}

#[cfg(unix)]
#[test]
#[ignore = "explicit release performance runner only"]
fn prepare_benchmark_fixture() {
    let root = benchmark_root();
    let binary = std::env::var_os("ILLUSTASH_BENCH_BINARY").expect("explicit baseline release binary");
    let fixture = load("http_api");
    fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let package = materialize_at(&fixture, &root);
    // 固定 4096 个合成的应用侧和来源侧资产，两个参考角色。
    // 原有的三个资产保留各自的认领；其余未分类，不解码任何文件。
    for name in ["library.sqlite", "source.sqlite"] {
        let mut c = Connection::open(package.join(name)).unwrap();
        let tx = c.transaction().unwrap();
        for i in 3..4096 {
            let id = format!("synthetic-{i:05}");
            let filename = format!("{id}.png");
            if name == "library.sqlite" {
                tx.execute("INSERT INTO assets(uuid,filename,src_path,width,height,added_at,src_kind) VALUES (?,?,?,120,150,'2026-09-22','orig')",
                    rusqlite::params![id, filename, root.join("folder").join(&filename).to_str()]).unwrap();
            } else {
                tx.execute("INSERT INTO assets(uuid,filename,src_path,width,height,added_at,src_kind,in_scope,trashed) VALUES (?,?,?,120,150,'2026-09-22','orig',1,0)",
                    rusqlite::params![id, filename, root.join("folder").join(&filename).to_str()]).unwrap();
            }
        }
        tx.commit().unwrap();
    }
    // 基线宿主决定要准备的版本；它和候选都用不带 --upgrade 的普通打开来测量。
    let done = Command::new(&binary).args(["migrate", "--db"]).arg(package.join("library.sqlite")).output().unwrap();
    assert!(done.status.success(), "benchmark fixture: upgrade: {}", String::from_utf8_lossy(&done.stderr));
    let backups = package.join("library-backups");
    if backups.exists() {
        fs::remove_dir_all(backups).unwrap();
    }
}

fn isolated_command(root: &Path, binary: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .env_clear()
        .env("PATH", root.join("bin"))
        .env("HOME", root.join("home"))
        .env("USERPROFILE", root.join("home"))
        .env("ILLUSTASH_APP_SUPPORT", root.join("support"))
        .env("ILLUSTASH_NO_KEYCHAIN", "1")
        .env("ILLUSTASH_SHARED", root.join("shared"))
        .env("ILLUSTASH_PYTHON_SENTINEL", root.join("python-launched"));
    #[cfg(windows)]
    command.env("SystemRoot", std::env::var_os("SystemRoot").expect("SystemRoot"));
    command
}

fn launch(root: &Path, package: &Path, category: &str, upgrade: bool, binary: &Path) -> (Host, String, String) {
    launch_inner(root, package, category, upgrade, binary, false)
}

fn launch_inner(
    root: &Path,
    package: &Path,
    category: &str,
    upgrade: bool,
    binary: &Path,
    desktop_pipe: bool,
) -> (Host, String, String) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let origin = format!("http://127.0.0.1:{port}");
    let mut command = isolated_command(root, binary);
    if category != "scale" && category != "scale_sync" {
        command
            .env("ILLUSTASH_TEST_VERIFY_INCREMENTAL", root.join("verified"))
            .env("ILLUSTASH_TEST_KEY_COUNTER", root.join("key-counter"));
    }
    command
        .args(["http", "--no-auto-sync", "--no-names", "--no-index-check", "--no-provenance", "--library"])
        .arg(package)
        .arg("--root")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .args(["--port", &port.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if category == "scale_sync" {
        command.env("ILLUSTASH_SYNC_TIMING", "1");
    } else if category != "task_recovery" {
        command.arg("--no-sync");
    }
    if upgrade {
        command.arg("--upgrade");
    }
    if desktop_pipe {
        command.env("ILLUSTASH_DESKTOP_STDIN", "1").stdin(Stdio::piped());
    }
    let mut host = Host { child: command.spawn().unwrap(), reaped: false };
    if let Some(stdin) = host.child.stdin.as_mut() {
        std::io::Write::write_all(stdin, &[1]).unwrap();
    }
    let (ready, lines) = std::sync::mpsc::channel();
    let stdout = host.child.stdout.take().unwrap();
    thread::spawn(move || {
        for line in std::io::BufRead::lines(std::io::BufReader::new(stdout)).map_while(Result::ok) {
            if entry::address(&line).is_some() {
                let _ = ready.send(line);
            }
        }
    });
    let start = Instant::now();
    let line = loop {
        assert!(host.child.try_wait().unwrap().is_none(), "host exited at startup");
        if let Ok(line) = lines.recv_timeout(Duration::from_millis(20)) {
            break line;
        }
        assert!(start.elapsed() < wait::limit(), "host startup timeout");
    };
    entry::enter(&line);
    let (status, bootstrap, _) = request(&origin, "/api/bootstrap", None, "");
    assert_eq!(status, 200);
    let bootstrap: Value = serde_json::from_str(&bootstrap).unwrap();
    (host, origin, bootstrap["token"].as_str().unwrap().to_string())
}

fn load(category: &str) -> Value {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/baseline/contracts/{category}.json.gz"));
    let mut text = String::new();
    let file = fs::File::open(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    flate2::read::GzDecoder::new(file).read_to_string(&mut text).unwrap();
    let fixture: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(fixture["format"], baseline::FORMAT);
    fixture
}

/// 恰好一份升级前备份，保存图库原来的样子。
fn check_backup(
    package: &Path,
    root: &Path,
    before: &BTreeMap<String, baseline::State>,
    recorded: i64,
    category: &str,
) {
    let backups: Vec<PathBuf> = fs::read_dir(package.join("library-backups"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "sqlite"))
        .collect();
    assert_eq!(backups.len(), 1, "one pre-upgrade backup");
    let check = root.join("backup-check");
    fs::create_dir(&check).unwrap();
    fs::copy(&backups[0], check.join("library.sqlite")).unwrap();
    compare(
        &BTreeMap::from([("library.sqlite".to_owned(), before["library.sqlite"].clone())]),
        &states_at(&check, root, recorded),
        &format!("{category}/pre-upgrade backup"),
        None,
    );
    fs::remove_dir_all(&check).unwrap();
}

/// 固定夹具的来源库布局照旧，只把词条库升到当前版本。
fn migrate_fixture_terms(package: &Path, category: &str) {
    let done = Command::new(env!("CARGO_BIN_EXE_illustash-terms"))
        .args(["migrate", "--db"])
        .arg(package.join("library.sqlite"))
        .output()
        .unwrap();
    assert!(done.status.success(), "{category}: upgrade: {}", String::from_utf8_lossy(&done.stderr));
}

/// 宿主报告就绪后继续恢复后台任务；等角色目录事务提交再取固定启动快照。
fn wait_for_catalog(package: &Path, category: &str) {
    let c = Connection::open_with_flags(package.join("library.sqlite"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap();
    let start = Instant::now();
    while !c
        .query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE key='character_catalog')", [], |r| r.get::<_, bool>(0))
        .unwrap()
    {
        assert!(start.elapsed() < wait::limit(), "{category}: startup character catalog timeout");
        thread::sleep(Duration::from_millis(10));
    }
}

fn replay(category: &str) {
    let fixture = load(category);
    let (_scratch, root, package) = materialize(&fixture, category);
    let recorded = fixture["recorded"]["terms_version"].as_i64().unwrap();
    let before = states_at(&package, &root, recorded);
    let version = {
        let c = Connection::open_with_flags(package.join("library.sqlite"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
        baseline::terms_version(&c)
    };
    // 用 `--upgrade` 录制的（upgrade_v6）：与录制时一样由宿主升级。其余场景录制时就在当时的词条核心版本；
    // 遇到更新的词条核心时，先像 `baseline.rs` 那样用词条核心自己的命令升级（宿主的 `--upgrade` 还会把这些合成图库包
    // 较旧的 source.sqlite 布局一并更新，而这不在录制范围内），再检查并删除升级前备份。
    let upgrade = version < recorded;
    if version == recorded && recorded < illustash_terms::SCHEMA_VERSION {
        migrate_fixture_terms(&package, category);
        check_backup(&package, &root, &before, recorded, category);
        fs::remove_dir_all(package.join("library-backups")).unwrap();
    }
    let (_host, origin, token) =
        launch(&root, &package, category, upgrade, Path::new(env!("CARGO_BIN_EXE_illustash-terms")));
    wait_for_catalog(&package, category);
    let mut want = apply(&before, fixture.get("startup")).unwrap();
    compare(&want, &states(&package, &root), &format!("{category}/startup"), Some(recorded));
    if upgrade {
        check_backup(&package, &root, &before, recorded, category);
    }
    let roots = text_masks(&root);
    let masks = baseline::Masks { roots: &roots, version: illustash_terms::SCHEMA_VERSION };
    for (i, step) in fixture["steps"].as_array().unwrap().iter().enumerate() {
        let at = format!("{category}/{i} {}", step["url"]);
        let url = step["url"].as_str().unwrap();
        let body = (!step["body"].is_null()).then_some(&step["body"]);
        let previous_runs: i64 = if category == "task_recovery" && body.is_some() {
            Connection::open(package.join("library.sqlite"))
                .unwrap()
                .query_row("SELECT count(*) FROM sync_runs", [], |r| r.get(0))
                .unwrap()
        } else {
            0
        };
        let (status, raw, headers) =
            request(&origin, url, body, if step["bad_token"] == true { "bad" } else { &token });
        assert_eq!(json!(status), step["status"], "{at}: status");
        if category == "task_recovery" && body.is_some() {
            let start = Instant::now();
            loop {
                let (_, progress, _) = request(&origin, "/api/sync", None, &token);
                let progress: Value = serde_json::from_str(&progress).unwrap();
                if progress["history"].as_array().unwrap().len() as i64 > previous_runs
                    && progress["history"][0]["status"].as_str().is_some_and(|s| s != "running")
                {
                    break;
                }
                assert!(start.elapsed() < wait::limit(), "recovery timeout");
                thread::sleep(Duration::from_millis(10));
            }
        }
        let reply = match step["projection"].as_str() {
            Some(key) => serde_json::from_str::<Value>(&raw).unwrap()[key].to_string(),
            None => raw,
        };
        assert_eq!(Value::Object(headers), step["headers"], "{at}: headers");
        assert_eq!(json!(session_token(&masks.output(&reply))), step["reply"], "{at}: reply");
        want = apply(&want, step.get("change")).unwrap_or_else(|e| panic!("{at}: {e}"));
        compare(&want, &states(&package, &root), &at, Some(recorded));
    }
    assert!(!root.join("python-launched").exists());
}

#[test]
fn favorite_http_contract() {
    let (_scratch, root, _) = materialize(&json!({"input": {}}), "favorites");
    let package = root.join("favorites.pilib");
    illustash_terms::app_create::create_folder(&package, &root.join("folder"), Some("收藏夹具"), None, None).unwrap();
    let db = Connection::open(package.join("library.sqlite")).unwrap();
    db.execute_batch(
        "INSERT INTO assets(uuid,added_at) VALUES ('a','2026-09-22'),('copy','2026-09-22'),('b','2026-09-21');
         INSERT INTO duplicates VALUES ('a','g',2,0,'2026-09-22',1,0),('copy','g',2,1,'2026-09-22',0,1);",
    )
    .unwrap();
    let (_host, origin, token) =
        launch(&root, &package, "favorites", false, Path::new(env!("CARGO_BIN_EXE_illustash-terms")));
    let call = |url: &str, data: Option<&Value>| {
        let (status, result, _) = request_json(&origin, url, data, &token);
        assert_eq!(status, 200, "{url}: {result}");
        result
    };
    let verdicts = || db.query_row("SELECT count(*) FROM term_verdicts", [], |r| r.get::<_, i64>(0)).unwrap();
    let fav = "has=%5B%22fav%22%5D";
    let not_fav = "not=%5B%22fav%22%5D";
    let ids = || call(&format!("/api/ids?{fav}"), None);
    assert_eq!(call("/api/favorite", Some(&json!({"ids":["a"],"add":false}))), json!({"undo":null}));
    assert_eq!(verdicts(), 0);
    assert_eq!(ids(), json!([]));
    let (status, _, _) = request_json(&origin, "/api/favorite", Some(&json!({"ids":vec!["a"; 501]})), &token);
    assert_eq!(status, 400);
    let first = call("/api/favorite", Some(&json!({"ids":["a"]})));
    assert_eq!(first["undo"]["label"], "已收藏 1 张");
    assert_eq!(verdicts(), 2);
    let batch = call("/api/favorite", Some(&json!({"ids":["a","b"]})));
    assert_eq!(batch["undo"]["label"], "已收藏 2 张");
    assert_eq!(verdicts(), 5);
    assert_eq!(call("/api/user-tags", None), json!([]));
    db.execute_batch(
        "INSERT INTO assets(uuid,added_at) VALUES ('late','2026-09-23'),('plain','2026-09-20');
         UPDATE duplicates SET rep=0,hidden=1,size=3 WHERE grp='g';
         INSERT INTO duplicates VALUES ('late','g',3,2,'2026-09-23',1,0);",
    )
    .unwrap();
    assert_eq!(ids(), json!(["late", "b"]));
    let page = call(&format!("/api/assets?{fav}&marks=fav"), None);
    assert_eq!(page["total"], 2);
    assert_eq!(page["items"][0]["uuid"], "late");
    assert_eq!(page["items"][0]["fav"], true);
    assert_eq!(call(&format!("/api/assets/count?{fav}"), None)["total"], 2);
    assert_eq!(call(&format!("/api/ids?{not_fav}"), None), json!(["plain"]));
    assert_eq!(call(&format!("/api/ids?{fav}&{not_fav}"), None), json!([]));
    assert_eq!(call(&format!("/api/ids?{fav}&day=2026-09-21"), None), json!(["b"]));
    assert_eq!(call("/api/home?marks=fav", None)["days"][0]["items"][0]["fav"], true);
    assert_eq!(call("/api/detail?id=late&marks=fav", None)["fav"], true);
    assert_eq!(call("/api/detail?id=plain&marks=other,fav", None)["fav"], false);
    assert!(call("/api/detail?id=late", None).get("fav").is_none());

    for query in ["has=fav", "has=%5B1%5D", "not=%5B%22fav%3Ax%22%5D"] {
        assert_eq!(request(&origin, &format!("/api/ids?{query}"), None, &token).0, 400);
    }
    let too_many = format!("has=%5B{}%5D&{not_fav}", vec!["%22fav%22"; 20].join(","));
    assert_eq!(request(&origin, &format!("/api/assets/count?{too_many}"), None, &token).0, 400);

    let clear = call("/api/favorite", Some(&json!({"filter":{"has":"[\"fav\"]"},"exclude":["b"],"add":false})));
    assert_eq!(clear["undo"]["label"], "已取消收藏 1 张");
    assert_eq!(verdicts(), 8);
    assert_eq!(ids(), json!(["b"]));
    assert_eq!(call("/api/detail?id=late&marks=fav", None)["fav"], false);
    call("/api/undo", Some(&json!({"id":clear["undo"]["id"]})));
    assert_eq!(ids(), json!(["late", "b"]));
}

#[test]
fn combined_gallery_http_contract() {
    use illustash_terms::{schema, store, Kind};
    use rusqlite::params;

    let (_scratch, root, _) = materialize(&json!({"input": {}}), "galleries");
    let package = root.join("galleries.pilib");
    illustash_terms::app_create::create_folder(&package, &root.join("folder"), Some("画廊夹具"), None, None).unwrap();
    let mut db = illustash_terms::open(&package.join("library.sqlite")).unwrap();
    let tx = db.transaction().unwrap();
    schema::begin_writing(&tx).unwrap();
    tx.execute_batch(
        "INSERT INTO assets(uuid,filename,added_at) VALUES
         ('a','one.png','2025-01-01'),('b','two.png','2025-12-31'),('c','three.png','2025-06-01'),
         ('d','four.png','2025-05-01'),('e','five.png','2025-07-01'),('new','six.png','2026-01-01');",
    )
    .unwrap();
    for (tag, ids) in [("1girl", &["a", "b", "c", "d", "e", "new"][..]), ("solo", &["a", "b", "d", "e", "new"][..])] {
        let term = store::create_term(&tx, Kind::Topic, None, "test").unwrap();
        store::add_link(&tx, term, "wd", tag, "", "test").unwrap();
        for id in ids {
            let asset = store::library_asset(&tx, id).unwrap().unwrap();
            tx.execute(
                "INSERT INTO term_evidence(asset_id,term_id,source,method,version,score,metric,fingerprint)
                 VALUES (?,?,'wd','tagger','fixture',0.9,'wd_probability',zeroblob(16))",
                params![asset, term],
            )
            .unwrap();
        }
    }
    let rating = store::create_term(&tx, Kind::Rating, None, "test").unwrap();
    store::add_link(&tx, rating, "wd_rating", "explicit", "", "test").unwrap();
    let b = store::library_asset(&tx, "b").unwrap().unwrap();
    tx.execute(
        "INSERT INTO term_evidence(asset_id,term_id,source,method,version,score,metric,fingerprint)
                 VALUES (?,?,'wd','tagger','fixture',0.9,'wd_probability',zeroblob(16))",
        params![b, rating],
    )
    .unwrap();
    let registered = tx
        .prepare("SELECT id FROM term_assets")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    store::project(&tx, &registered).unwrap();
    let artist = store::create_term(&tx, Kind::Artist, Some("synthetic-artist"), "test").unwrap();
    let a = store::library_asset(&tx, "a").unwrap().unwrap();
    let d = store::library_asset(&tx, "d").unwrap().unwrap();
    for (asset, state) in [(a, "confirmed"), (b, "maybe"), (d, "rejected")] {
        tx.execute("INSERT INTO term_projection(asset_id,term_id,state) VALUES (?,?,?)", params![asset, artist, state])
            .unwrap();
    }
    use illustash_terms::character_entities::{self, New};
    for entity in [
        New { id: "fixture-root-work", kind: "work", ..New::default() },
        New { id: "fixture-child-work", kind: "work", parent_id: Some("fixture-root-work"), ..New::default() },
        New { id: "fixture-character", kind: "character", work_id: Some("fixture-child-work"), ..New::default() },
    ] {
        character_entities::create(&tx, &entity, "user").unwrap();
    }
    tx.execute_batch(
        "INSERT INTO memberships(uuid,entity,status,origin,sources,score,source_count,ccip_only,skey)
         VALUES ('a','fixture-character','machine','fixture','WD',0.9,1,0,'a');",
    )
    .unwrap();
    schema::commit_writing(tx).unwrap();
    Connection::open(package.join("source.sqlite"))
        .unwrap()
        .execute_batch(
            "INSERT INTO assets(uuid,preset,in_scope,trashed) VALUES
         ('a','pixiv',1,0),('b','pixiv',1,0),('c','pixiv',1,0),('d','x',1,0),('new','pixiv',1,0);",
        )
        .unwrap();

    let (_host, origin, token) =
        launch(&root, &package, "galleries", false, Path::new(env!("CARGO_BIN_EXE_illustash-terms")));
    let call = |url: &str, data: Option<&Value>| {
        let (status, result, _) = request_json(&origin, url, data, &token);
        assert_eq!(status, 200, "{url}: {result}");
        result
    };
    let url = |path: &str, filter: &[(&str, &str)]| {
        format!(
            "{path}?{}",
            url::form_urlencoded::Serializer::new(String::new()).extend_pairs(filter.iter().copied()).finish()
        )
    };
    let has = r#"["tag:1girl","tag:solo","source:pixiv","tag:1girl"]"#;
    let filter = [("has", has), ("from", "2025-01-01"), ("to", "2025-12-31")];
    assert_eq!(call(&url("/api/ids", &filter), None), json!(["b", "a"]));
    assert_eq!(call(&url("/api/assets/count", &filter), None)["total"], 2);
    let page = call(&url("/api/assets", &filter), None);
    assert_eq!(page["total"], 2);
    assert_eq!(page["items"][0]["uuid"], "b");
    assert_eq!(call(&url("/api/ids", &[("has", r#"["source:other"]"#)]), None), json!(["e"]));
    assert_eq!(call(&url("/api/ids", &[("has", r#"["artist:synthetic-artist"]"#)]), None), json!(["a"]));
    assert_eq!(call(&url("/api/ids", &[("not", r#"["tag:solo"]"#)]), None), json!(["c"]));
    assert_eq!(call(&url("/api/ids", &[("has", r#"["char:fixture-character"]"#)]), None), json!(["a"]));
    assert_eq!(call(&url("/api/ids", &[("has", r#"["work:fixture-root-work"]"#)]), None), json!(["a"]));
    assert_eq!(call(&url("/api/ids", &[("work", "fixture-root-work")]), None), json!(["a"]));
    let params = json!({"to":"2025-12-31","has":has,"not":"[\"utag:审完:合成\"]","from":"2025-01-01"});
    let created = call("/api/galleries", Some(&json!({"action":"create","params":params})));
    let gallery = &created["gallery"];
    let id = &gallery["id"];
    assert_eq!(gallery["name"], "1girl · solo · Pixiv · 不含 审完:合成 · 2025");
    assert_eq!(gallery["params"]["has"], r#"["tag:1girl","tag:solo","source:pixiv"]"#);
    assert_eq!(gallery["cover"], "a");
    assert!(gallery["position"].is_i64());
    let covers = |ids: &Value, nsfw: &str| url("/api/galleries/covers", &[("ids", &ids.to_string()), ("nsfw", nsfw)]);
    let first_cover = |nsfw| call(&covers(&json!([id]), nsfw), None)[0]["cover"].clone();
    assert_eq!(
        call("/api/galleries?nsfw=hide", None),
        json!([{
            "id":id,"name":gallery["name"],"params":gallery["params"],"position":gallery["position"],"cover_manual":false
        }])
    );
    assert_eq!(first_cover(""), "b");
    let fixed =
        call("/api/galleries", Some(&json!({"action":"update","id":id,"name":"手动封面","cover":"b","position":8})));
    assert_eq!(fixed["gallery"]["cover"], "a");
    assert_eq!(fixed["gallery"]["cover_manual"], true);
    assert_eq!(fixed["gallery"]["position"], 8);
    assert_eq!(first_cover("only"), "b");
    assert_eq!(first_cover("hide"), "a");
    let listed = call("/api/galleries?nsfw=hide", None);
    assert_eq!(listed[0]["cover_manual"], true);
    assert_eq!(listed[0]["position"], 8);
    assert!(listed[0].get("cover").is_none());
    let cleared = call("/api/galleries", Some(&json!({"action":"update","id":id,"cover":null})));
    assert_eq!(cleared["gallery"]["cover_manual"], false);
    call("/api/undo", Some(&json!({"id":cleared["undo"]["id"]})));
    assert_eq!(call("/api/galleries", None)[0]["cover_manual"], true);
    let deleted = call("/api/galleries", Some(&json!({"action":"delete","id":id})));
    assert_eq!(call("/api/galleries", None), json!([]));
    assert_eq!(call(&covers(&json!([id]), "hide"), None), json!([]));
    call("/api/undo", Some(&json!({"id":deleted["undo"]["id"]})));
    assert_eq!(first_cover("hide"), "a");
    let position: i64 = db.query_row("SELECT position FROM galleries WHERE id=1", [], |r| r.get(0)).unwrap();
    assert_eq!(position, 8);

    call("/api/tag", Some(&json!({"filter":params,"exclude":["a"],"tag":"审完:合成"})));
    assert_eq!(call("/api/user-tags?limit=1000", None)[0]["count"], 1);
    let exclude = ("not", r#"["utag:审完:合成"]"#);
    let filtered: Vec<_> = filter.into_iter().chain([exclude]).collect();
    assert_eq!(call(&url("/api/ids", &filtered), None), json!(["a"]));
    assert_eq!(first_cover(""), "a");

    // 模拟一个已保存的作品被删除，另存一个合法但空的选区。
    db.execute(
        "INSERT INTO galleries(name,params,position,created_at,updated_at)
         VALUES ('失效画廊',?,2,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)",
        [json!({"work":"fixture-removed-work"}).to_string()],
    )
    .unwrap();
    let invalid_id = db.last_insert_rowid();
    let empty_params = json!({"has":r#"["source:pixiv"]"#,"not":r#"["source:pixiv"]"#});
    let empty = call("/api/galleries", Some(&json!({"action":"create","params":empty_params})));
    let empty_id = &empty["gallery"]["id"];
    let listed = call("/api/galleries", None);
    assert_eq!(listed[0]["id"], invalid_id);
    assert_eq!(listed[0]["params"]["work"], "fixture-removed-work");
    assert_eq!(listed[0]["position"], 2);
    assert_eq!(listed[1]["id"], *id);
    assert_eq!(listed[2]["id"], *empty_id);
    assert_eq!(
        call(&covers(&json!([empty_id, id, invalid_id, id, -999]), "hide"), None),
        json!([{"id":invalid_id,"cover":null},{"id":id,"cover":"a"},{"id":empty_id,"cover":null}])
    );
    assert_eq!(call(&covers(&json!([]), "hide"), None), json!([]));
    assert_eq!(request(&origin, "/api/galleries/covers", None, &token).0, 400);
    for (path, filters) in [
        ("/api/ids", vec![("from", "2025-02-29")]),
        ("/api/ids", vec![("from", "2025-12-31"), ("to", "2025-01-01")]),
        ("/api/ids", vec![("has", r#"["tag:synthetic-missing"]"#)]),
        ("/api/ids", vec![("has", r#"["source:invalid"]"#)]),
        ("/api/user-tags", vec![("limit", "1001")]),
        ("/api/galleries", vec![("nsfw", "invalid")]),
        ("/api/galleries/covers", vec![("ids", "[1]"), ("nsfw", "invalid")]),
        ("/api/galleries/covers", vec![("ids", r#"[1,"2"]"#)]),
        ("/api/galleries/covers", vec![("ids", "1")]),
        ("/api/galleries/covers", vec![("ids", "[1.5]")]),
        ("/api/galleries/covers", vec![("ids", r#"{"id":1}"#)]),
    ] {
        assert_eq!(request(&origin, &url(path, &filters), None, &token).0, 400);
    }
    assert_eq!(
        request_json(&origin, "/api/galleries", Some(&json!({"action":"create","params":{"nsfw":"hide"}})), &token).0,
        400
    );
}

#[test]
fn manual_gallery_http_contract() {
    let (_scratch, root, _) = materialize(&json!({"input": {}}), "manual-gallery");
    let package = root.join("manual.pilib");
    illustash_terms::app_create::create_folder(&package, &root.join("folder"), Some("手选夹具"), None, None).unwrap();
    let db = Connection::open(package.join("library.sqlite")).unwrap();
    db.execute_batch(
        "INSERT INTO assets(uuid,added_at) VALUES ('a','2026-09-22'),('copy','2026-09-22'),('b','2026-09-21'),
                                                ('other','2026-09-20');
         INSERT INTO duplicates VALUES ('a','g',2,0,'2026-09-22',1,0),('copy','g',2,1,'2026-09-22',0,1);",
    )
    .unwrap();
    let (_host, origin, token) =
        launch(&root, &package, "manual-gallery", false, Path::new(env!("CARGO_BIN_EXE_illustash-terms")));
    let call = |url: &str, data: Option<&Value>| {
        let (status, result, _) = request_json(&origin, url, data, &token);
        assert_eq!(status, 200, "{url}: {result}");
        result
    };
    let post = |data: Value| call("/api/galleries", Some(&data));
    let undo = |result: &Value| call("/api/undo", Some(&json!({"id":result["undo"]["id"]})));
    let create = post(json!({"action":"create","kind":"manual","name":"合成手选","ids":["a","a"]}));
    let id = &create["gallery"]["id"];
    let has = format!(r#"["gallery:{id}"]"#);
    let path = |route: &str, op: &str, expr: &str| {
        format!("{route}?{}", url::form_urlencoded::Serializer::new(String::new()).append_pair(op, expr).finish())
    };
    let ids = || call(&path("/api/ids", "has", &has), None);
    let cover = || call(&path("/api/galleries/covers", "ids", &json!([id]).to_string()), None)[0]["cover"].clone();
    let members = || {
        db.prepare("SELECT uuid FROM gallery_members WHERE gallery_id=? ORDER BY uuid")
            .unwrap()
            .query_map([id.as_i64().unwrap()], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert_eq!(create["gallery"]["kind"], "manual");
    assert_eq!(create["gallery"]["params"], json!({"has":has}));
    assert_eq!(call("/api/galleries", None)[0]["kind"], "manual");
    assert_eq!(ids(), json!(["a"]));
    assert_eq!(cover(), "a");
    let empty = post(json!({"action":"create","kind":"manual","name":"合成空画廊"}));
    assert_eq!(empty["gallery"]["cover"], Value::Null);
    undo(&empty);

    let add = post(json!({"action":"add","id":id,"filter":{"not":has},"exclude":["other"]}));
    assert_eq!(ids(), json!(["a", "b"]));
    assert_eq!(call(&path("/api/assets", "has", &has), None)["total"], 2);
    assert_eq!(call(&path("/api/assets/count", "has", &has), None)["total"], 2);
    assert_eq!(call(&path("/api/ids", "not", &has), None), json!(["other"]));
    let duplicate = post(json!({"action":"add","id":id,"ids":["a","a","b"]}));
    assert!(duplicate["undo"]["id"].as_i64().unwrap() > add["undo"]["id"].as_i64().unwrap());
    assert_eq!(members(), ["a", "b", "copy"]);
    undo(&duplicate);
    assert_eq!(ids(), json!(["a", "b"]));
    undo(&add);
    assert_eq!(ids(), json!(["a"]));
    post(json!({"action":"add","id":id,"ids":["b"]}));

    // 新代表本身从未放入画廊，原成员仍赋予重复组画廊归属。
    db.execute_batch(
        "INSERT INTO assets(uuid,added_at) VALUES ('new-rep','2026-09-23');
         UPDATE duplicates SET rep=0,hidden=1,size=3 WHERE grp='g';
         INSERT INTO duplicates VALUES ('new-rep','g',3,2,'2026-09-23',1,0);",
    )
    .unwrap();
    assert_eq!(ids(), json!(["new-rep", "b"]));
    let updated = post(json!({"action":"update","id":id,"position":7,"name":"新名字","cover":"b"}));
    assert_eq!(updated["gallery"]["position"], 7);
    assert_eq!(updated["gallery"]["name"], "新名字");
    assert_eq!(cover(), "b");
    let remove = post(json!({"action":"remove","id":id,"filter":{"has":has},"exclude":["b"]}));
    assert_eq!(ids(), json!(["b"]));
    assert_eq!(members(), ["b"]);
    undo(&remove);
    assert_eq!(ids(), json!(["new-rep", "b"]));
    let cleared = post(json!({"action":"update","id":id,"cover":null}));
    assert_eq!(cover(), "new-rep");
    undo(&cleared);
    assert_eq!(cover(), "b");

    db.execute("INSERT INTO excluded(uuid,reason,origin) VALUES ('b','manual','user')", []).unwrap();
    assert_eq!(ids(), json!(["new-rep"]));
    assert_eq!(cover(), "new-rep");
    db.execute("DELETE FROM excluded WHERE uuid='b'", []).unwrap();
    assert_eq!(cover(), "b");
    db.execute("DELETE FROM assets WHERE uuid='b'", []).unwrap();
    assert_eq!(ids(), json!(["new-rep"]));
    assert_eq!(cover(), "new-rep");
    assert_eq!(members(), ["a", "b", "copy"]);
    let deleted = post(json!({"action":"delete","id":id}));
    assert_eq!(call("/api/galleries", None), json!([]));
    assert!(members().is_empty());
    assert_eq!(request(&origin, &path("/api/ids", "has", &has), None, &token).0, 400);
    undo(&deleted);
    assert_eq!(ids(), json!(["new-rep"]));
    assert_eq!(members(), ["a", "b", "copy"]);

    let conditional = post(json!({"action":"create","params":{"has":has}}));
    let cid = &conditional["gallery"]["id"];
    assert!(conditional["gallery"].get("kind").is_none());
    for data in [
        json!({"action":"add","id":cid,"ids":["a"]}),
        json!({"action":"add","id":id,"ids":["a"],"filter":{}}),
        json!({"action":"update","id":id,"params":{}}),
        json!({"action":"create","kind":"invalid","name":"合成"}),
    ] {
        assert_eq!(request_json(&origin, "/api/galleries", Some(&data), &token).0, 400);
    }
    undo(&conditional);
    let deleted = post(json!({"action":"delete","id":id}));
    let new = post(json!({"action":"create","kind":"manual","name":"另一个手选画廊"}));
    assert_ne!(new["gallery"]["id"], *id);
    assert_eq!(request(&origin, &path("/api/ids", "has", &has), None, &token).0, 400);
    undo(&new);
    undo(&deleted);
    assert_eq!(ids(), json!(["new-rep"]));
}

#[test]
fn http_api_contract() {
    replay("http_api");
}
#[test]
fn genuine_v6_upgrade_contract() {
    replay("upgrade_v6");
}
#[test]
fn undo_and_history_contract() {
    replay("undo_history");
}
#[test]
fn interrupted_task_recovery_contract() {
    replay("task_recovery");
}

/// 把响应解析成 JSON，供按字段断言和计时使用。
fn request_json(origin: &str, url: &str, body: Option<&Value>, token: &str) -> (u16, Value, Map<String, Value>) {
    let (status, raw, headers) = request(origin, url, body, token);
    (status, serde_json::from_str(&raw).unwrap_or(Value::Null), headers)
}

#[test]
fn artist_directory_contract() {
    let category = "artist-directory";
    let (_scratch, root, package) = materialize(&load("http_api"), category);
    migrate_fixture_terms(&package, category);
    let binary = Path::new(env!("CARGO_BIN_EXE_illustash-terms"));
    let (_host, origin, token) = launch(&root, &package, category, false, binary);
    let c = Connection::open(package.join("library.sqlite")).unwrap();
    c.execute_batch(
        "INSERT INTO terms(key,kind,origin) VALUES
             ('dir-a','artist','test'),('dir-b','artist','test'),('dir-c','artist','test'),('dir-d','artist','test'),
             ('dir-explicit','rating','test');
         INSERT INTO term_names(term_id,value,role,source,preferred)
             SELECT id,'Straße','display','test',1 FROM terms WHERE key='dir-a';
         INSERT INTO term_names(term_id,value,role,source)
             SELECT id,'Unpreferred','display','test' FROM terms WHERE key='dir-a';
         INSERT INTO term_links(system,key,term_id,source)
             SELECT 'x','Maple_Artist',id,'test' FROM terms WHERE key='dir-a'
             UNION ALL SELECT 'pixiv','987654',id,'test' FROM terms WHERE key='dir-a'
             UNION ALL SELECT 'danbooru','Aoi_Illustrator',id,'test' FROM terms WHERE key='dir-b'
             UNION ALL SELECT 'wd_rating','explicit',id,'test' FROM terms WHERE key='dir-explicit';
         INSERT INTO assets(uuid,filename,width,height,added_at) VALUES
             ('dir-safe-a','a.png',10,10,'2026-10-01'),('dir-safe-z','z.png',10,10,'2026-10-01'),
             ('dir-safe-b','b.png',10,10,'2026-10-03'),
             ('dir-adult','r.png',10,10,'2026-10-02'),('dir-hidden','h.png',10,10,'2026-10-03'),
             ('dir-excluded','e.png',10,10,'2026-10-04'),('dir-maybe','m.png',10,10,'2026-10-05'),
             ('dir-rejected','n.png',10,10,'2026-10-06'),('dir-removed','x.png',10,10,'2026-10-07');
         INSERT INTO duplicates(uuid,grp,size,pos,grp_added,rep,hidden)
             VALUES ('dir-hidden','dir-dup',2,2,'2026-10-03',0,1);
         INSERT INTO excluded(uuid,reason,origin) VALUES ('dir-excluded','manual','test');
         INSERT INTO term_assets(namespace,key,removed_at)
             SELECT 'library',uuid,CASE WHEN uuid='dir-removed' THEN '2026-10-08' END
             FROM assets WHERE uuid LIKE 'dir-%';
         INSERT INTO term_projection(asset_id,term_id,state)
             SELECT a.id,t.id,CASE a.key WHEN 'dir-maybe' THEN 'maybe'
                                         WHEN 'dir-rejected' THEN 'rejected'
                                         WHEN 'dir-safe-a' THEN 'confirmed' ELSE 'machine' END
             FROM term_assets a JOIN terms t ON t.key='dir-a' WHERE a.key LIKE 'dir-%' AND a.key<>'dir-safe-b';
         INSERT INTO term_projection(asset_id,term_id,state)
             SELECT a.id,t.id,'machine' FROM term_assets a JOIN terms t
                 ON (t.key='dir-b' AND a.key IN ('dir-safe-b','dir-adult'))
                 OR (t.key IN ('dir-c','dir-explicit') AND a.key='dir-adult');
         INSERT INTO term_projection(asset_id,term_id,state)
             SELECT a.id,t.id,'maybe' FROM term_assets a JOIN terms t ON t.key='dir-d' WHERE a.key='dir-maybe';",
    )
    .unwrap();

    let get = |url: &str| {
        let (status, payload, _) = request_json(&origin, url, None, &token);
        assert_eq!(status, 200, "{url}: {payload}");
        payload
    };
    let page_ids = |page: &Value| {
        page["items"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap().to_owned()).collect::<Vec<_>>()
    };
    for (nsfw, counts, cover, last) in [
        ("", &[3, 2, 1][..], "dir-adult", "2026-10-02"),
        ("hide", &[2, 1], "dir-safe-a", "2026-10-01"),
        ("only", &[1, 1, 1], "dir-adult", "2026-10-02"),
    ] {
        let page = get(&format!("/api/artist-directory?nsfw={nsfw}"));
        assert_eq!(page["total"], counts.len());
        let items = page["items"].as_array().unwrap();
        assert_eq!(items.len(), counts.len());
        for (item, (id, expected)) in items.iter().zip(["dir-a", "dir-b", "dir-c"].into_iter().zip(counts)) {
            let filter = get(&format!("/api/assets/count?artist={id}&nsfw={nsfw}"));
            assert_eq!(item["id"], id);
            assert_eq!(item["count"], *expected, "{id} {nsfw}");
            assert_eq!(item["count"], filter["total"], "{id} {nsfw}");
        }
        for id in ["dir-a", "dir-b", "dir-c", "dir-d"].into_iter().skip(counts.len()) {
            assert_eq!(get(&format!("/api/assets/count?artist={id}&nsfw={nsfw}"))["total"], 0);
        }
        assert_eq!(items[0]["cover"], cover);
        assert_eq!(items[0]["last"], last);
        assert_eq!(items[0]["name"], get("/api/artist?id=dir-a")["name"]);
        let detail = get(&format!("/api/detail?id={cover}"));
        let artist = detail["artists"].as_array().unwrap().iter().find(|x| x["id"] == "dir-a").unwrap();
        assert_eq!(items[0]["homes"], artist["homes"]);
        assert_eq!(items[0]["homes"].as_array().unwrap().len(), 2);
    }
    for (order, expected) in [
        ("count", ["dir-a", "dir-b", "dir-c"]),
        ("recent", ["dir-b", "dir-a", "dir-c"]),
        ("name", ["dir-b", "dir-a", "dir-c"]),
    ] {
        assert_eq!(page_ids(&get(&format!("/api/artist-directory?order={order}"))), expected);
        for (offset, expected) in expected.iter().enumerate() {
            let page = get(&format!("/api/artist-directory?order={order}&offset={offset}&limit=1"));
            assert_eq!(page_ids(&page), [*expected]);
            assert_eq!(page["total"], 3);
            assert_eq!(page["offset"], offset);
        }
        assert_eq!(
            get(&format!("/api/artist-directory?order={order}&offset=3&limit=1")),
            json!({"items":[], "total":3, "offset":3})
        );
    }
    for (query, expected) in [
        ("STRASSE", &["dir-a"][..]),
        ("maple_", &["dir-a"]),
        ("987654", &["dir-a"]),
        ("AOI_ILL", &["dir-b"]),
        ("aoi%20ill", &["dir-b"]),
        ("unpreferred", &[]),
        ("%25", &[]),
    ] {
        assert_eq!(page_ids(&get(&format!("/api/artist-directory?q={query}"))), expected);
    }
    for query in ["order=bad", "nsfw=bad", "limit=0", "limit=201", "offset=-1", "offset=no"] {
        assert_eq!(request_json(&origin, &format!("/api/artist-directory?{query}"), None, &token).0, 400);
    }
}

/// #448 专用合成数据。独立运行本用例三次，每次报告 50 个 HTTP 请求的平均耗时。
#[test]
#[ignore = "release HTTP benchmark; run with collab_run.py timing"]
fn benchmark_artist_directory() {
    if cfg!(debug_assertions) {
        panic!("benchmark must be compiled --release");
    }
    let category = "artist-directory-benchmark";
    let (_scratch, root, package) = materialize(&load("http_api"), category);
    migrate_fixture_terms(&package, category);
    let binary = Path::new(env!("CARGO_BIN_EXE_illustash-terms"));
    let c = Connection::open(package.join("library.sqlite")).unwrap();
    c.execute_batch(
        "BEGIN;
         CREATE TEMP TABLE seq(i INTEGER PRIMARY KEY);
         WITH RECURSIVE ints(i) AS (VALUES(0) UNION ALL SELECT i+1 FROM ints WHERE i<49999)
             INSERT INTO seq SELECT i FROM ints;
         INSERT INTO assets(uuid,width,height,added_at,filename)
             SELECT printf('directory-%05d',i),120,150,date('2023-01-01',printf('+%d days',i%1095)),
                    printf('directory-%05d.png',i)
             FROM seq;
         INSERT INTO term_assets(namespace,key) SELECT 'library',printf('directory-%05d',i) FROM seq;
         CREATE TEMP TABLE synthetic AS
             SELECT i,a.id,(i*7919)%50000 AS permuted FROM seq JOIN term_assets a ON a.namespace='library' AND a.key=printf('directory-%05d',i);
         INSERT INTO terms(key,kind,origin)
             SELECT printf('directory-artist-%04d',i),'artist','synthetic' FROM seq WHERE i<4000;
         INSERT INTO term_names(term_id,value,role,source)
             SELECT id,CASE WHEN id%5=0 THEN 'Maple ' ELSE 'Aoi ' END || key,'display','synthetic'
             FROM terms WHERE key LIKE 'directory-artist-%';
         INSERT INTO term_links(system,key,term_id,source)
             SELECT 'pixiv',printf('%d',810000000+id),id,'synthetic' FROM terms WHERE key LIKE 'directory-artist-%';
         INSERT INTO term_projection(asset_id,term_id,state)
             SELECT s.id,t.id,CASE WHEN permuted%9=0 THEN 'confirmed' ELSE 'machine' END
             FROM synthetic s JOIN terms t ON t.key=printf('directory-artist-%04d',
                  CASE WHEN permuted>=4000 AND permuted%3=0 THEN permuted%79 ELSE permuted%4000 END)
             WHERE permuted<12000;
         INSERT INTO terms(key,kind,origin) VALUES ('directory-explicit','rating','synthetic');
         INSERT INTO term_links(system,key,term_id,source)
             SELECT 'wd_rating','explicit',id,'synthetic' FROM terms WHERE key='directory-explicit';
         INSERT INTO term_projection(asset_id,term_id,state)
             SELECT s.id,t.id,'machine' FROM synthetic s JOIN terms t ON t.key='directory-explicit'
             WHERE (i*31337)%50000 < 20000;
         INSERT INTO duplicates(uuid,grp,size,pos,grp_added,rep,hidden)
             SELECT printf('directory-%05d',i),printf('d-%04d',i/2),2,i%2+1,'2025-01-01',1-i%2,i%2
             FROM seq WHERE i<2500;
         INSERT INTO terms(key,kind,origin)
             SELECT printf('directory-topic-%04d',i),'topic','synthetic' FROM seq WHERE i<3000;
         INSERT INTO term_projection(asset_id,term_id,state)
             SELECT s.id,t.id,'machine'
             FROM synthetic s JOIN seq n ON n.i<30
             JOIN terms t ON t.key=printf('directory-topic-%04d',(s.permuted+n.i*97)%3000);
         COMMIT;",
    )
    .unwrap();
    drop(c);
    let (_host, origin, token) = launch(&root, &package, category, false, binary);
    let page = request_json(&origin, "/api/artist-directory", None, &token);
    assert_eq!(page.0, 200, "{}", page.1);
    let total = page.1["total"].as_u64().unwrap();
    assert!(total > 3800);
    let last_page = format!("/api/artist-directory?offset={}", (total - 1) / 60 * 60);
    for (name, url) in [
        ("count", "/api/artist-directory"),
        ("recent", "/api/artist-directory?order=recent"),
        ("name", "/api/artist-directory?order=name"),
        ("search", "/api/artist-directory?q=maple"),
        ("hide", "/api/artist-directory?nsfw=hide"),
        ("last_page", &last_page),
    ] {
        for _ in 0..5 {
            let (status, body, _) = request_json(&origin, url, None, &token);
            assert_eq!(status, 200, "{url}: {body}");
            assert!(!body["items"].as_array().unwrap().is_empty());
        }
        let start = Instant::now();
        for _ in 0..50 {
            assert_eq!(request_json(&origin, url, None, &token).0, 200);
        }
        println!("BENCH {name} {:.3}", start.elapsed().as_secs_f64() * 1000.0 / 50.0);
    }
}

// 每次调用这个可执行文件做一次独立试验。shell 运行脚本对每个显式指定的 release 宿主恰好做三次试验，报告中位数。
#[cfg(unix)]
#[test]
#[ignore = "explicit release performance runner only"]
fn benchmark_release_host() {
    let root = benchmark_root().canonicalize().expect("prepared benchmark fixture directory missing");
    let package = root.join("fixture.pilib");
    let replacement = root.join("replacement.pilib");
    for input in [&package, &replacement] {
        for name in ["library.sqlite", "source.sqlite", "library.json"] {
            let path = input.join(name);
            assert!(path.is_file(), "prepared benchmark input missing: {}", path.display());
        }
    }
    let binary = std::env::var_os("ILLUSTASH_BENCH_BINARY").expect("explicit release binary");
    // 测量完整过程：启动进程 -> 校验和恢复图库包 -> bootstrap 就绪。
    let mut opens = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        let (mut opened, _, _) = launch(&root, &package, "benchmark", false, Path::new(&binary));
        opens.push(start.elapsed().as_secs_f64() * 1000.0);
        // SAFETY: kill 只接收整数；子进程尚未回收，PID 不会被复用。
        unsafe {
            libc::kill(opened.child.id() as i32, libc::SIGTERM);
        }
        assert!(opened.child.wait().unwrap().success());
        opened.reaped = true;
    }
    opens.sort_by(f64::total_cmp);
    let (mut host, origin, mut token) = launch(&root, &package, "benchmark", false, Path::new(&binary));
    assert_eq!(request_json(&origin, "/api/assets/count", None, &token).1["total"], 4096);
    let paths = [
        ("assets", "/api/assets?limit=100&offset=2000"),
        ("count", "/api/assets/count"),
        ("detail", "/api/detail?id=a"),
    ];
    let mut samples = Map::new();
    samples.insert("open".into(), json!(opens[3]));
    for (name, url) in paths {
        for _ in 0..5 {
            assert_eq!(request_json(&origin, url, None, &token).0, 200);
        }
        let start = Instant::now();
        for _ in 0..50 {
            assert_eq!(request_json(&origin, url, None, &token).0, 200);
        }
        samples.insert(name.into(), json!(start.elapsed().as_secs_f64() * 1000.0 / 50.0));
    }
    let mut tag_ms = 0.0;
    let mut undo_ms = 0.0;
    for _ in 0..20 {
        let start = Instant::now();
        let (status, result, _) =
            request_json(&origin, "/api/tag", Some(&json!({"ids":["a","b"],"tag":"benchmark synthetic"})), &token);
        tag_ms += start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(status, 200);
        let start = Instant::now();
        assert_eq!(request_json(&origin, "/api/undo", Some(&json!({"id":result["undo"]["id"]})), &token).0, 200);
        undo_ms += start.elapsed().as_secs_f64() * 1000.0;
    }
    samples.insert("tag".into(), json!(tag_ms / 20.0));
    samples.insert("undo".into(), json!(undo_ms / 20.0));
    let mut switches = Vec::new();
    for i in 0..21 {
        let path = if i % 2 == 0 { &replacement } else { &package };
        let start = Instant::now();
        assert_eq!(request_json(&origin, "/api/libraries/open", Some(&json!({"path":path})), &token).0, 200);
        let (status, bootstrap, _) = request_json(&origin, "/api/bootstrap", None, &token);
        assert_eq!(status, 200);
        token = bootstrap["token"].as_str().unwrap().to_owned();
        if i > 0 {
            switches.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    switches.sort_by(f64::total_cmp);
    samples.insert("switch".into(), json!((switches[9] + switches[10]) / 2.0));
    host.child.kill().unwrap();
    // SAFETY: libc::rusage 只含整数和 timeval，全零是合法值。
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    let mut status = 0;
    // SAFETY: PID 属于尚未回收的子进程，`status`、`usage` 是本地出参。
    assert_eq!(unsafe { libc::wait4(host.child.id() as i32, &mut status, 0, &mut usage) }, host.child.id() as i32);
    // wait4 已经完成回收；Drop 绝不能向可能已被复用的 PID 发信号。
    host.reaped = true;
    let bytes = if cfg!(target_os = "macos") { usage.ru_maxrss as f64 } else { usage.ru_maxrss as f64 * 1024.0 };
    samples.insert("rss_mib".into(), json!(bytes / 1048576.0));
    for (key, value) in samples {
        println!("BENCH {key} {}", value.as_f64().unwrap());
    }
    assert!(!root.join("python-launched").exists());
}

#[path = "contracts/examples_bench.rs"]
mod examples_bench;
