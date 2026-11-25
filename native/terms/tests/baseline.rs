//! 第一个公开测试版的对照基准（#201），对本次构建重放。
//!
//! `tests/fixtures/baseline/` 存放以前 Python 时代调用录制的各个场景，在词条核心第 7 版的 main 上录制（以前
//! `tests/replay.rs` 里的 `record_baseline`，PR #239）：旧表删除后词条核心仍然提供的每个调用、它的响应，以及它对
//! 该调用所用数据库的改动。只为旧表服务的调用（`sync-*`、`verify-*`、`cutover-*`，以及在尚未切换到词条核心的图库上的
//! 调用）不在其中：它们留下的数据库作为检查点，在下一个调用前放好；如果它是已切换的图库、且处于录制对照基准时的
//! 词条核心版本，就先升级。比较什么、怎么比较，见 `common/baseline.rs`。
//!
//! 每个用例在自己的目录里运行，有自己的键计数器（`ILLUSTASH_TEST_KEY_COUNTER`），在检查点处恢复，所以词条核心生成的键
//! 与录制时相同；学习戳记里哈希了时钟，所以从产生它的响应里取出再往下传（输入中的 `<stamp@N>` 是本用例的响应里
//! 出现的第 N 个戳记）。
//!
//! 基准录在导入按 tag 后缀建作品（#296）之前：每次重放的调用都设上 `ILLUSTASH_TEST_NO_IMPORT_WORKS`（`NO_IMPORT_WORKS`），
//! 导入照旧规则走，录下的期望不改（用户 2026-11-12 定）。新规则由它自己的测试覆盖（`character_import`、`app_create`、
//! `app_recognition`）。
//!
//! 不并进 `tests/it/` 的合并测试二进制（#310）：下面的 `SQLITE_CONFIG_MEMSTATUS` 必须在本进程第一次用 SQLite 之前设置，
//! 和别的测试同在一个进程里时，测试框架多线程运行、先后没有保证，谁先开了库这个设置就会失败。
#![allow(clippy::unwrap_used, reason = "集成测试里 unwrap 就是断言")]
use flate2::read::GzDecoder;
use regex::Regex;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::LazyLock;

#[path = "common/baseline.rs"]
mod baseline;

static STAMPS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"learning-v1:[0-9a-f]{64}").unwrap());
static STAMP_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<stamp@(\d+)>").unwrap());
const COMPANIONS: [&str; 4] = ["", "-wal", "-shm", "-journal"];
/// 只供测试：#296 之前的导入规则（见模块说明）。
const NO_IMPORT_WORKS: &str = "ILLUSTASH_TEST_NO_IMPORT_WORKS";

/// 一个用例的临时根目录：`@ROOT@`、`@ROOT_1@`……以及它们代表的真实目录。
struct Roots {
    /// （占位符，真实路径）。
    roots: Vec<(String, String)>,
    /// （真实路径的写法，占位符），最长的在前：`norm` 替换的就是这些。
    spellings: Vec<(String, String)>,
}

impl Roots {
    fn new(base: &str, tokens: &[String]) -> Self {
        let mut roots = Vec::new();
        let mut spellings = Vec::new();
        for token in tokens {
            let real = if token == "@ROOT@" { base.to_owned() } else { format!("{base}/{}", token.trim_matches('@')) };
            fs::create_dir_all(&real).unwrap();
            if let Some(short) = real.strip_prefix("/private") {
                spellings.push((short.to_owned(), token.clone()));
            }
            spellings.push((real.clone(), token.clone()));
            roots.push((token.clone(), real));
        }
        spellings.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        Roots { roots, spellings }
    }

    fn norm(&self, text: &str) -> String {
        // 每种写法都是绝对路径，所以不含 `/` 的文本（大多数单元格）里一个也没有。
        if !text.contains('/') {
            return text.to_owned();
        }
        let mut text = text.to_owned();
        for (real, token) in &self.spellings {
            text = text.replace(real.as_str(), token);
        }
        text
    }

    fn sub(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for (token, real) in &self.roots {
            text = text.replace(token.as_str(), real);
        }
        text
    }
}

struct Serve {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn remove(path: &Path) {
    for suffix in COMPANIONS {
        let p = PathBuf::from(format!("{}{suffix}", path.display()));
        if p.exists() {
            fs::remove_file(&p).unwrap();
        }
    }
}

fn open(path: &Path) -> Connection {
    Connection::open(path).unwrap_or_else(|e| {
        let units = path.as_os_str().to_string_lossy().encode_utf16().count();
        panic!("cannot open {} ({units} UTF-16 units; a shorter temporary directory may help): {e}", path.display())
    })
}

fn short(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

struct Replay<'a> {
    name: &'a str,
    blobs: &'a Value,
    bin: &'a Path,
    /// 录制对照基准时的词条核心版本：处于这个版本的检查点会升级到本次构建的版本。
    recorded: i64,
    roots: Roots,
    counter: PathBuf,
    verified: PathBuf,
    serve: Option<Serve>,
    historical_index: bool,
    expected: HashMap<PathBuf, baseline::State>,
    stamps: Vec<String>,
    /// 升级改动了比较内容的检查点：后面的比较失败时会列出它们。
    upgraded: Vec<String>,
}

impl Replay<'_> {
    fn blob(&self, id: &Value) -> &str {
        self.blobs[id.as_str().unwrap()].as_str().unwrap()
    }

    fn state(&self, path: &Path) -> baseline::State {
        self.state_at(path, illustash_terms::SCHEMA_VERSION)
    }

    fn historical_state(&self, path: &Path, frozen: &baseline::State) -> Result<baseline::State, String> {
        baseline::historical_projection(&self.state(path), frozen, self.recorded, illustash_terms::SCHEMA_VERSION)
    }

    /// 对照仍保留旧快照的原始差分，只在比较前按录制的事实推算新版折叠状态。
    fn compare_frozen(&self, at: &str, path: &Path, frozen: &baseline::State) -> Result<(), String> {
        let mut expected = std::borrow::Cow::Borrowed(frozen);
        if self.recorded == 7 {
            if let Some(duplicates) = baseline::current_fold(frozen) {
                expected.to_mut().insert("duplicates".into(), duplicates);
            }
        }
        let got = self.historical_state(path, &expected)?;
        if got != *expected {
            return Err(format!("{at}: state differs: {:?}", baseline::differences(&expected, &got, 5)));
        }
        Ok(())
    }

    /// 参与比较的内容；`version` 是当前的词条核心版本（按录制原样的检查点则用录制时的版本）。
    fn state_at(&self, path: &Path, version: i64) -> baseline::State {
        if !path.exists() {
            return baseline::State::new();
        }
        let norm = |t: &str| self.roots.norm(t);
        baseline::state(&open(path), &baseline::Masks { roots: &norm, version })
    }

    fn output(&self, text: &str) -> String {
        let norm = |t: &str| self.roots.norm(t);
        baseline::Masks { roots: &norm, version: illustash_terms::SCHEMA_VERSION }.output(text)
    }

    /// 把一个调用要用的文件和检查点放好。
    fn environment(&mut self, call: &Value) -> Result<(), String> {
        for entry in call.get("env").and_then(Value::as_array).into_iter().flatten() {
            let path = PathBuf::from(self.roots.sub(entry["path"].as_str().unwrap()));
            remove(&path);
            if entry.get("delete").is_some() {
                self.expected.remove(&path);
                continue;
            }
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            if let Some(from) = entry.get("copy").and_then(Value::as_str) {
                let from = PathBuf::from(self.roots.sub(from));
                let source = open(&from);
                let mut target = open(&path);
                rusqlite::backup::Backup::new(&source, &mut target)
                    .unwrap()
                    .run_to_completion(1024, Default::default(), None)
                    .unwrap();
                match self.expected.get(&from).cloned() {
                    Some(state) => self.expected.insert(path, state),
                    None => self.expected.remove(&path),
                };
            } else if let Some(parts) = entry.get("db").and_then(Value::as_array) {
                let [tables, rows, objects] = [&parts[0], &parts[1], &parts[2]].map(|p| self.roots.sub(self.blob(p)));
                baseline::restore(&open(&path), &tables, &rows, &objects);
                let base = self.state_at(&path, self.recorded);
                // 对照基准里后续调用都不再用到的检查点可能是尚未切换的图库：无法升级。
                if entry["terms"].as_i64() == Some(self.recorded) && !baseline::before_cutover(&open(&path)) {
                    if let Some(change) = self.upgrade(&path, &base)? {
                        self.upgraded.push(change);
                    }
                }
                // 旧检查点保留历史分组和裁决，用当前应用打开时的投影作为此次重放的起点。
                if !baseline::before_cutover(&open(&path)) && baseline::current_fold(&base).is_some() {
                    illustash_terms::character_projection::fold(&open(&path)).map_err(|e| e.to_string())?;
                }
                self.expected.insert(path, base);
            } else {
                let text = self.blob(&entry["file"]);
                let bytes = match text.strip_prefix("x:") {
                    Some(h) => (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect(),
                    None => self.roots.sub(text).into_bytes(),
                };
                fs::write(&path, bytes).unwrap();
                self.expected.remove(&path);
            }
        }
        Ok(())
    }

    /// 处于录制时词条核心版本的检查点，在下一个调用前升级到本次构建的版本，与用户的图库相同。升级产生的备份会删掉，
    /// 让目录与录制时一样。如果升级改动了比较内容，返回改动了什么。
    fn upgrade(&self, path: &Path, base: &baseline::State) -> Result<Option<String>, String> {
        let backups = path.parent().unwrap().join("library-backups");
        let existing: Vec<PathBuf> =
            fs::read_dir(&backups).map(|d| d.map(|e| e.unwrap().path()).collect()).unwrap_or_default();
        let done = Command::new(self.bin)
            .args(["migrate", "--db"])
            .arg(path)
            .env("ILLUSTASH_TEST_KEY_COUNTER", &self.counter)
            .env(NO_IMPORT_WORKS, "1")
            .output()
            .unwrap();
        if !done.status.success() {
            return Err(format!(
                "upgrading checkpoint {}: {}",
                self.roots.norm(&path.to_string_lossy()),
                String::from_utf8_lossy(&done.stderr)
            ));
        }
        if let Ok(entries) = fs::read_dir(&backups) {
            for entry in entries {
                let made = entry.unwrap().path();
                if !existing.contains(&made) {
                    fs::remove_file(&made).unwrap();
                }
            }
            if existing.is_empty() {
                let _ = fs::remove_dir(&backups);
            }
        }
        // 升级带来的改动（例如修复过期的镜像）本身不算失败：后面每个调用都会与对照基准比较，改动一旦显现就在那里失败，
        // 届时也会列出这项改动。
        let after = self.historical_state(path, base)?;
        Ok((after != *base).then(|| {
            format!(
                "upgrading checkpoint {} changed {:?}",
                self.roots.norm(&path.to_string_lossy()),
                baseline::differences(base, &after, 5)
            )
        }))
    }

    fn input(&self, text: &str) -> Result<String, String> {
        let text = self.roots.sub(text);
        let mut missing = None;
        let out = STAMP_REF
            .replace_all(&text, |c: &regex::Captures| {
                let n: usize = c[1].parse().unwrap();
                self.stamps.get(n).cloned().unwrap_or_else(|| {
                    missing = Some(n);
                    String::new()
                })
            })
            .into_owned();
        match missing {
            Some(n) => Err(format!("input refers to stamp {n}, only {} seen", self.stamps.len())),
            None => Ok(out),
        }
    }

    fn call(&mut self, at: &str, call: &Value) -> Result<(), String> {
        self.environment(call)?;
        if let Some(n) = call.get("counter").and_then(Value::as_u64) {
            fs::write(&self.counter, n.to_string()).unwrap();
        }
        let db = call.get("db").and_then(Value::as_str).map(|d| PathBuf::from(self.roots.sub(d)));
        let mut before = db.as_ref().and_then(|d| self.expected.get(d).cloned()).unwrap_or_default();
        if let (Some(d), Some(change)) = (&db, call.get("before")) {
            before = baseline::apply(&before, change).map_err(|e| format!("{at}: before: {e}"))?;
            self.compare_frozen(&format!("{at}: before"), d, &before)?;
        }
        if call["mode"] == "cli" {
            let args = call["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| self.input(a.as_str().unwrap()))
                .collect::<Result<Vec<_>, _>>()?;
            let stdin = call.get("stdin").and_then(Value::as_str).map(|i| self.input(i)).transpose()?;
            let mut child = Command::new(self.bin)
                .args(&args)
                .env("ILLUSTASH_TEST_KEY_COUNTER", &self.counter)
                .env(NO_IMPORT_WORKS, "1")
                .env(VERIFY_INCREMENTAL, &self.verified)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            if let Some(input) = stdin {
                child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
            }
            drop(child.stdin.take());
            let done = child.wait_with_output().unwrap();
            let code = done.status.code().unwrap_or(-1);
            if Some(i64::from(code)) != call["code"].as_i64() {
                return Err(format!("{at}: exit {code}, baseline {}", call["code"]));
            }
            for (key, bytes) in [("stdout", &done.stdout), ("stderr", &done.stderr)] {
                let text = String::from_utf8_lossy(bytes);
                self.stamps.extend(STAMPS.find_iter(&text).map(|m| m.as_str().to_owned()));
                let got = self.output(&text);
                if Some(got.as_str()) != call[key].as_str() {
                    return Err(format!("{at}: {key} {got:?}, baseline {:?}", call[key]));
                }
            }
        } else {
            let input: Value = serde_json::from_str(&self.input(&call["input"].to_string())?).unwrap();
            let line = serde_json::json!({"db": db.as_ref().unwrap().to_string_lossy(), "input": input});
            let (bin, counter, verified) = (self.bin, &self.counter, &self.verified);
            let s = self.serve.get_or_insert_with(|| {
                let mut child = Command::new(bin)
                    .arg("serve")
                    .args(self.historical_index.then_some("--historical-index"))
                    .env("ILLUSTASH_TEST_KEY_COUNTER", counter)
                    .env(NO_IMPORT_WORKS, "1")
                    .env(VERIFY_INCREMENTAL, verified)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .spawn()
                    .unwrap();
                let (stdin, stdout) = (child.stdin.take().unwrap(), BufReader::new(child.stdout.take().unwrap()));
                Serve { child, stdin, stdout }
            });
            writeln!(s.stdin, "{line}").unwrap();
            s.stdin.flush().unwrap();
            let mut reply = String::new();
            s.stdout.read_line(&mut reply).unwrap();
            self.stamps.extend(STAMPS.find_iter(&reply).map(|m| m.as_str().to_owned()));
            let got = self.output(reply.trim_end_matches(['\r', '\n']));
            if Some(got.as_str()) != call["reply"].as_str() {
                return Err(format!("{at}: reply {got}, baseline {}", call["reply"]));
            }
        }
        if let Some(d) = db {
            let want = match call.get("change") {
                Some(change) => baseline::apply(&before, change).map_err(|e| format!("{at}: {e}"))?,
                None => before,
            };
            self.compare_frozen(at, &d, &want)?;
            self.expected.insert(d, want);
        }
        Ok(())
    }
}

/// 词条核心的名称增量与全量对比检查（#102；只在测试构建中），对每个调用都开启：出现分歧时，发生分歧的那个调用失败。
const VERIFY_INCREMENTAL: &str = "ILLUSTASH_TEST_VERIFY_INCREMENTAL";

fn case(name: &str, data: &Value, blobs: &Value, recorded: i64, bin: &Path, verified: &Path) -> Option<String> {
    let temp = std::env::temp_dir().join(format!("illustash-baseline-{}-{}", std::process::id(), short(name)));
    let _ = fs::remove_dir_all(&temp);
    fs::create_dir_all(temp.join("root")).unwrap();
    // 对照基准里的路径用 `/`，经常出现在 JSON 文本里；在 Windows 上根目录也写成 `/`。
    let base = if cfg!(windows) {
        temp.join("root").to_string_lossy().replace('\\', "/")
    } else {
        fs::canonicalize(temp.join("root")).unwrap().to_string_lossy().into_owned()
    };
    let tokens: Vec<String> =
        data["roots"].as_array().unwrap().iter().map(|t| t.as_str().unwrap().to_owned()).collect();
    let counter = temp.join("key-counter");
    fs::write(&counter, "0").unwrap();
    let calls = data["calls"].as_array().unwrap();
    // 旧索引录有造型结构时，整个回放会话沿用录制时的解释，包括同一会话后续的索引更新。
    let historical_index = calls.iter().any(|call| {
        let input = &call["input"];
        matches!(input["op"].as_str(), Some("refresh" | "reindex"))
            && input["index"]["characters"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|row| row["skin"].as_str().is_some_and(|variant| !variant.is_empty()))
    });
    let mut replay = Replay {
        name,
        blobs,
        bin,
        recorded,
        roots: Roots::new(&base, &tokens),
        counter,
        verified: verified.to_path_buf(),
        serve: None,
        historical_index,
        expected: HashMap::new(),
        stamps: Vec::new(),
        upgraded: Vec::new(),
    };
    let mut problem = None;
    for (i, call) in calls.iter().enumerate() {
        let op = call.get("args").and_then(|a| a[0].as_str()).or_else(|| call["input"]["op"].as_str()).unwrap_or("");
        let at = format!("{} #{i} {op}", replay.name);
        if let Err(e) = replay.call(&at, call) {
            problem =
                Some(if replay.upgraded.is_empty() { e } else { format!("{e}\n  ({})", replay.upgraded.join("; ")) });
            break; // 后面的调用只会重复这个分歧
        }
    }
    drop(replay);
    let _ = fs::remove_dir_all(&temp);
    problem
}

/// 除非明确关闭（`SQLITE_CONFIG_MEMSTATUS`），SQLite 会在一个全进程共用的互斥锁下统计每次分配。每次读取状态都要重新
/// 打开数据库、解析表结构，于是所有模块的重放线程在重放的大部分时间里都在排队等这个锁。这里没有代码读这些统计。
static SQLITE_CONFIG: std::sync::Once = std::sync::Once::new();

fn replay(module: &str) {
    SQLITE_CONFIG.call_once(|| {
        // SAFETY: 在本进程建立任何连接之前调用（每个测试都从这里开始）；只接收一个 int 参数。
        let done = unsafe { rusqlite::ffi::sqlite3_config(rusqlite::ffi::SQLITE_CONFIG_MEMSTATUS, 0) };
        assert_eq!(done, rusqlite::ffi::SQLITE_OK, "SQLITE_CONFIG_MEMSTATUS");
    });
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/baseline/{module}.json.gz"));
    let mut text = String::new();
    GzDecoder::new(fs::File::open(&path).unwrap()).read_to_string(&mut text).unwrap();
    let data: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(data["format"], baseline::FORMAT, "{module}: baseline format");
    let recorded = data["recorded"]["terms_version"].as_i64().unwrap();
    let bin = Path::new(env!("CARGO_BIN_EXE_illustash-terms"));
    let work: Vec<(&String, &Value)> = data["cases"].as_object().unwrap().iter().collect();
    assert!(!work.is_empty(), "{module}: empty baseline");
    // 各用例相互独立（有自己的目录、键计数器和 serve 进程）：分到几个线程上运行。
    let verified = std::env::temp_dir().join(format!("illustash-baseline-{}-{module}.verified", std::process::id()));
    let _ = fs::remove_file(&verified);
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get().min(4));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let problems: Vec<String> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut found = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let Some((name, case_data)) = work.get(i) else { break found };
                        found.extend(case(name, case_data, &data["blobs"], recorded, bin, &verified));
                    }
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().unwrap()).collect()
    });
    let checks = fs::read_to_string(&verified).unwrap_or_default().lines().count();
    let _ = fs::remove_file(&verified);
    eprintln!("{module}: {checks} incremental names checks");
    assert!(
        UNVERIFIED.contains(&module) || checks > 0,
        "{module}: the incremental names check never ran (built without illustash-core/verify-incremental?)"
    );
    assert!(
        problems.is_empty(),
        "{module}: {} of {} cases differ:\n{}",
        problems.len(),
        work.len(),
        problems.join("\n")
    );
}

/// 不在增量路径上提交名称改动的模块（只有搜索、整体重建，或者只有别名，而别名自 #201 起由词条核心直接写入）：
/// 其他每个模块都必须至少运行过一次词条核心的增量与全量对比检查。
const UNVERIFIED: [&str; 3] = ["test_terms_core", "test_terms_names_contract", "test_terms_names_projection"];

macro_rules! modules {
    ($($name:ident),* $(,)?) => {$(
        #[test]
        fn $name() {
            replay(stringify!($name));
        }
    )*};
}

modules!(
    test_terms_core,
    test_terms_labels,
    test_terms_artists,
    test_terms_character_commands,
    test_terms_character_history,
    test_terms_character_identity,
    test_terms_character_import,
    test_terms_character_learning,
    test_terms_character_refusals,
    test_terms_character_structure,
    test_terms_characters_contract,
    test_terms_characters_cutover,
    test_terms_golden,
    test_terms_names_contract,
    test_terms_names_projection,
    test_terms_names_writes,
    test_identity,
    test_identity_hybrid,
    test_lead,
    test_web_v2,
    test_works,
    test_terms_artists_provenance,
    test_web,
);
