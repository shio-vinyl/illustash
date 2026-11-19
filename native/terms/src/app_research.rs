//! 可选的计算 worker 在快照上工作。原生所有者负责校验和发布。
use crate::{
    app_library::Library,
    app_tasks::{Task, TaskContext, TaskIdentity},
    http_json::Json,
    Error, Result,
};
use crate::{task_state::State, worker_protocol as protocol};
use rusqlite::{
    types::{Value as Sql, ValueRef},
    Connection, TransactionBehavior,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};

/// 显式开启时向宿主 stderr 写固定阶段名和耗时。阶段内出错或取消也记录已用时间。
struct Phase(&'static str, Option<std::time::Instant>);
impl Phase {
    fn start(name: &'static str) -> Self {
        Self(name, std::env::var_os("ILLUSTASH_RESEARCH_TIMING").is_some().then(std::time::Instant::now))
    }
}
impl Drop for Phase {
    fn drop(&mut self) {
        if let Some(start) = self.1 {
            eprintln!("research-timing {} {:.3} ms", self.0, start.elapsed().as_secs_f64() * 1000.0);
        }
    }
}

pub fn configured() -> bool {
    std::env::var_os("ILLUSTASH_RESEARCH_WORKER").is_some_and(|p| !p.is_empty())
}
fn bad() -> Error {
    Error::Invalid("研究结果格式不正确".into())
}
// 发布时的冲突：快照本身有效，但到了更新的输入。只有这几种会重试。
const INPUT_CHANGED: &str = "研究输入已改变，请重试";
const LEARNING_CHANGED: &str = "学习输入已改变";
/// 输入冲突后的自动重试次数；每次的延迟从基数起翻倍。
const CONFLICT_RETRIES: u32 = 5;
fn input_changed(e: &Error) -> bool {
    matches!(e, Error::Conflict(m) if m == INPUT_CHANGED || m == LEARNING_CHANGED)
}
/// 为源库写入方让路的运行（`Writers`）。学习和相似运行就此结束，等没有同步在运行时由恢复重新启动；查询则等写入方
/// 完成后再问一次，最多 `CONFLICT_RETRIES` 次。
const YIELDED: &str = "图库正在写入，研究稍后继续";
fn yielded(e: &Error) -> bool {
    matches!(e, Error::Conflict(m) if m == YIELDED)
}
/// 应用数据库在发布检查和取得写入锁之间一轮又一轮地变化（#275）。这不算失败：结果不发布，运行等一会儿再试。
const BUSY: &str = "图库一直在写入，研究稍后继续";
fn busy(e: &Error) -> bool {
    matches!(e, Error::Conflict(m) if m == BUSY)
}
/// 等待 `d`；期间任务停止则返回错误。
fn pause(ctx: &TaskContext, d: Duration) -> Result<()> {
    let until = std::time::Instant::now() + d;
    loop {
        ctx.check()?;
        let now = std::time::Instant::now();
        if now >= until {
            return Ok(());
        }
        std::thread::sleep((until - now).min(Duration::from_millis(50)));
    }
}
/// 为写入方让路的运行，要等写入方消失这么久之后才重新开始。
const STEP_ASIDE: Duration = Duration::from_secs(2);
fn retry_base() -> Duration {
    Duration::from_millis(
        std::env::var("ILLUSTASH_RESEARCH_RETRY_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(2000),
    )
}
const CACHES: &[(&str, usize)] = &[
    ("clusters", 2),
    ("cluster_info", 7),
    ("image_sigs", 4),
    ("relations", 6),
    ("similar_paired", 1),
    ("similar_retry", 2),
];
/// 同一 UUID 下字节变了的图片，不再由按它的像素算出的相似结果描述：删掉它的签名、它参与的每条关系和它的配对标记，
/// 让下一次相似运行重新计算，并用剩下的内容重建分组。没变的图片保留结果。这些都是机器缓存；人工决定存在别处，
/// 保持不变。
pub(crate) fn forget_content(tx: &Connection, src: &Connection, changed: &[String]) -> Result<()> {
    if changed.is_empty() {
        return Ok(());
    }
    let list = serde_json::to_string(changed).expect("UUID strings are serializable");
    tx.execute("DELETE FROM image_sigs WHERE uuid IN (SELECT value FROM json_each(?1))", [&list])?;
    tx.execute(
        "DELETE FROM relations WHERE a IN (SELECT value FROM json_each(?1)) OR b IN (SELECT value FROM json_each(?1))",
        [&list],
    )?;
    // 由第一次发布它们的相似运行创建。
    if tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='similar_paired')", [], |r| r.get(0))? {
        tx.execute("DELETE FROM similar_paired WHERE uuid IN (SELECT value FROM json_each(?1))", [&list])?;
        tx.execute(
            "DELETE FROM similar_retry WHERE a IN (SELECT value FROM json_each(?1)) OR b IN (SELECT value FROM json_each(?1))",
            [&list],
        )?;
    }
    crate::app_migrate_data::early_groups(tx, src)?;
    crate::character_projection::fold(tx)
}
/// 没有任何研究计算器、学习戳或结果检查会读的应用表（#29、#752）：偏好设置、名称来源设置（模型密钥、流量计数）、
/// 名称缓存、核心名称表与显示投影表、同步历史、人工结论备份的状态与图片摘要、研究队列，以及画廊和相似组的「看过了」。
/// 写这些表从不会让结果失效。其他所有表，包括以后新增的表，都仍是学习的输入。
const UNREAD: &[&str] = &[
    "backend_effects",
    "galleries",
    "gallery_members",
    "human_asset_hashes",
    "human_backup_state",
    "name_cache",
    "name_settings",
    "prefs",
    "research_runs",
    "similar_seen",
    "sync_added",
    "sync_runs",
    "term_key_aliases",
    "term_key_names",
    "term_name_choice",
    "term_name_display",
    "term_names",
];
/// 名称写入计数、显示投影同步版本和人工结论备份修订号。研究计算与学习戳均不读取名称及显示投影。
const UNREAD_META: &str = "'names_version','names_projected','names_display_projected','human_revision'";
/// 某一种研究的结果由什么算出（#29，docs/ARCHITECTURE_DIRECTION.md）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    /// events/regroup：学习、聚类，以及（events 带 similar 时）相似扫描。
    Learning,
    Similar,
    Oc,
    Relate,
}
impl Scope {
    fn of(kind: &str) -> Self {
        match kind {
            "similar" => Self::Similar,
            "oc" => Self::Oc,
            "relate" => Self::Relate,
            _ => Self::Learning,
        }
    }
    /// 计算器读取的应用表；`None` 表示除 `UNREAD` 以外的所有表。
    fn app(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Learning => None,
            // research_compute::similar::scan.
            Self::Similar => Some(&[
                "assets",
                "duplicates",
                "excluded",
                "image_sigs",
                "relations",
                "similar_paired",
                "similar_retry",
            ]),
            // research_compute::groups::oc.
            Self::Oc => Some(&["duplicates", "excluded", "general_hits", "memberships"]),
            // 只有请求的图片的路径，由 `app_digest_once` 计算哈希。
            Self::Relate => Some(&[]),
        }
    }
    /// CCIP 向量来自叠加层，学习还使用源库的角色元数据。
    fn source(self) -> &'static [&'static str] {
        match self {
            Self::Learning => &["characters", "embeddings"],
            Self::Similar | Self::Oc => &["embeddings"],
            Self::Relate => &[],
        }
    }
    /// 发布缓存或学习结果的种类；其他种类只回答查询。
    fn publishes(self) -> bool {
        matches!(self, Self::Learning | Self::Similar)
    }
}
/// 一次运行纳入指纹的输入：该种类的范围，加上它在数据库之外读取的路径。
struct Inputs<'a> {
    scope: Scope,
    /// relate：请求的图片，排序并去重。
    ids: Vec<String>,
    features: PathBuf,
    anchors: &'a Path,
}
impl<'a> Inputs<'a> {
    fn new(library: &Library, anchors: &'a Path, kind: &str, args: &Value) -> Self {
        let scope = Scope::of(kind);
        let mut ids: Vec<String> = if scope == Scope::Relate {
            args["ids"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect()
        } else {
            vec![]
        };
        ids.sort();
        ids.dedup();
        Self { scope, ids, features: library.instance_path().join("features"), anchors }
    }
    /// 按内容计算哈希的共享文件：基础特征（除 relate 外都用）和锚点（只有学习用）。
    fn files(&self) -> Result<Vec<PathBuf>> {
        let mut files = vec![];
        if self.scope != Scope::Relate {
            files.extend([self.features.join("full-manifest.csv"), self.features.join("full-features.npy")]);
        }
        if self.scope == Scope::Learning {
            files.extend([self.anchors.join("char-anchors.npy"), self.anchors.join("char-anchors.txt")]);
            if let Ok(entries) = fs::read_dir(self.anchors.join("char-index")) {
                for entry in entries {
                    let p = entry?.path();
                    if p.extension().is_some_and(|s| s == "json") {
                        files.push(p);
                    }
                }
            }
        }
        files.sort();
        Ok(files)
    }
    /// 共享文件没变的廉价证据：索引代号加上每个文件的大小和修改时间。在写入锁下比较，而不是再读一遍文件。
    fn stamp(&self) -> Result<Vec<u8>> {
        let mut h = Sha256::new();
        if self.scope == Scope::Learning && !self.anchors.as_os_str().is_empty() {
            h.update(crate::index_generation::current(self.anchors)?.to_le_bytes());
        }
        for p in self.files()? {
            feed(&mut h, p.to_string_lossy().as_bytes());
            file_metadata(&mut h, &p);
        }
        Ok(h.finalize().to_vec())
    }
}
fn feed(h: &mut Sha256, b: &[u8]) {
    h.update((b.len() as u64).to_le_bytes());
    h.update(b);
}
fn file_metadata(h: &mut Sha256, path: &Path) {
    if let Ok(m) = fs::metadata(path) {
        h.update(m.len().to_le_bytes());
        if let Ok(t) = m.modified().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).map_err(std::io::Error::other))
        {
            h.update(t.as_nanos().to_le_bytes());
        }
    } else {
        h.update(b"missing");
    }
}
/// 一张表的有序、带类型的行（或表不存在的标记）。
fn hash_table(c: &Connection, h: &mut Sha256, table: &str, source: bool, check: &dyn Fn() -> Result<()>) -> Result<()> {
    feed(h, table.as_bytes());
    let exists: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type IN ('table','view') AND name=?)",
        [table],
        |r| r.get(0),
    )?;
    if !exists {
        feed(h, b"absent");
        return Ok(());
    }
    let quoted = table.replace('"', "\"\"");
    let n = c.prepare(&format!("SELECT * FROM \"{quoted}\""))?.column_count();
    let order = (1..=n).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let filter = match table {
        "embeddings" if source => CCIP_ROWS.into(),
        "meta" => format!(" WHERE key NOT IN ({UNREAD_META})"),
        "actions" => " WHERE CASE WHEN json_valid(before_json)
                       THEN json_type(before_json,'$[0].gallery') IS NOT 'object'
                         AND json_type(before_json,'$[0].similar_seen') IS NOT 'object' ELSE 1 END"
            .into(),
        _ => String::new(),
    };
    let mut q = c.prepare(&format!("SELECT * FROM \"{quoted}\"{filter} ORDER BY {order}"))?;
    let mut rows = q.query([])?;
    while let Some(r) = rows.next()? {
        check()?;
        #[cfg(test)]
        tests::pause_row();
        feed(h, b"row");
        for i in 0..n {
            match r.get_ref(i)? {
                ValueRef::Null => feed(h, b"null"),
                ValueRef::Integer(v) => {
                    feed(h, b"int");
                    feed(h, &v.to_le_bytes());
                }
                ValueRef::Real(v) => {
                    feed(h, b"real");
                    feed(h, &v.to_bits().to_le_bytes());
                }
                ValueRef::Text(v) => {
                    feed(h, b"text");
                    feed(h, v);
                }
                ValueRef::Blob(v) => {
                    feed(h, b"blob");
                    feed(h, v);
                }
            }
        }
    }
    Ok(())
}
/// `only`：要计算哈希的表；`None` 计算除 `skip` 以外所有表的哈希。
fn hash_db(
    c: &Connection,
    h: &mut Sha256,
    only: Option<&[&str]>,
    skip: &[&str],
    check: &dyn Fn() -> Result<()>,
) -> Result<()> {
    let tables = match only {
        Some(tables) => tables.iter().map(|t| t.to_string()).collect(),
        None => c
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|t| !skip.contains(&t.as_str()))
            .collect::<Vec<_>>(),
    };
    for table in tables {
        check()?;
        hash_table(c, h, &table, false, check)?;
    }
    Ok(())
}
/// 一次运行的输入指纹：应用部分（`app_digest`）和源库部分（`source_digest`）。两部分分开读，这样研究持有
/// source.sqlite 的读锁时，绝不会在不留意其写入方的情况下处理应用数据库（#275）。
#[cfg(test)]
fn fingerprint(app: &Connection, src: &Connection, inputs: &Inputs<'_>, ctx: &TaskContext) -> Result<String> {
    let check = || ctx.check();
    let (_, app) = app_digest(app, inputs, None, &check)?;
    let source = source_digest(src, inputs, None, &check)?;
    Ok(combine(&app, &source.digest.clone().finalize()))
}
fn combine(app: &[u8], source: &[u8]) -> String {
    let mut h = Sha256::new();
    feed(&mut h, app);
    feed(&mut h, source);
    format!("{:x}", h.finalize())
}
const CCIP_ROWS: &str = " WHERE model='ccip'";

/// 只在一次运行的原连接上复用。版本号在读事务内取得，与该次读到的行属于同一快照。
#[derive(Clone)]
struct DbHash {
    version: i64,
    digest: Sha256,
}
impl DbHash {
    fn read(c: &Connection, previous: Option<&Self>, hash: impl FnOnce(&mut Sha256) -> Result<()>) -> Result<Self> {
        let version = data_version(c)?;
        if let Some(hit) = previous.filter(|p| p.version == version) {
            return Ok(hit.clone());
        }
        let mut digest = Sha256::new();
        hash(&mut digest)?;
        Ok(Self { version, digest })
    }
}
struct InputHashes {
    app: DbHash,
    source: DbHash,
}

fn source_digest(
    src: &Connection,
    inputs: &Inputs<'_>,
    previous: Option<&DbHash>,
    check: &dyn Fn() -> Result<()>,
) -> Result<DbHash> {
    let _phase = Phase::start("hash_source");
    DbHash::read(src, previous, |h| {
        for table in inputs.scope.source() {
            check()?;
            hash_table(src, h, table, true, check)?;
        }
        Ok(())
    })
}
/// 数据库版本只能证明 SQL 输入没变。共享文件仍按内容、原图仍按路径与文件元数据逐项校验。
fn app_digest(
    app: &Connection,
    inputs: &Inputs<'_>,
    previous: Option<&DbHash>,
    check: &dyn Fn() -> Result<()>,
) -> Result<(DbHash, Vec<u8>)> {
    let _phase = Phase::start("hash_app");
    let db = DbHash::read(app, previous, |h| {
        feed(h, format!("{:?}", inputs.scope).as_bytes());
        hash_db(app, h, inputs.scope.app(), UNREAD, check)
    })?;
    let files = || file_digest(db.digest.clone(), app, inputs, check);
    let digest = if inputs.scope == Scope::Learning {
        crate::index_generation::stable(inputs.anchors, files)?
    } else {
        files()?
    };
    Ok((db, digest))
}
fn file_digest(
    mut h: Sha256,
    app: &Connection,
    inputs: &Inputs<'_>,
    check: &dyn Fn() -> Result<()>,
) -> Result<Vec<u8>> {
    // 基础特征和锚点文件也是输入。向量变了会让进行中的结果失效。
    for p in inputs.files()? {
        check()?;
        let name = p.to_string_lossy();
        h.update(name.as_bytes());
        if let Ok(mut f) = fs::File::open(p) {
            let mut bytes = [0; 65536];
            loop {
                check()?;
                let n = f.read(&mut bytes)?;
                if n == 0 {
                    break;
                }
                h.update(&bytes[..n]);
            }
        }
    }
    // 原图：学习和相似用全部图片，relate 用请求的那些；OC 一张也不读。
    match inputs.scope {
        Scope::Learning | Scope::Similar => {
            let mut q = app.prepare("SELECT src_path FROM assets ORDER BY uuid")?;
            for path in q.query_map([], |r| r.get::<_, Option<String>>(0))? {
                check()?;
                file_metadata(&mut h, Path::new(&path?.unwrap_or_default()));
            }
        }
        Scope::Relate => {
            let ids = serde_json::to_string(&inputs.ids).map_err(|_| bad())?;
            let mut q = app.prepare(
                "SELECT uuid,src_path FROM assets WHERE uuid IN (SELECT value FROM json_each(?)) ORDER BY uuid",
            )?;
            for row in q.query_map([ids], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))? {
                check()?;
                let (uuid, path) = row?;
                let path = path.unwrap_or_default();
                feed(&mut h, uuid.as_bytes());
                feed(&mut h, path.as_bytes());
                file_metadata(&mut h, Path::new(&path));
            }
        }
        Scope::Oc => {}
    }
    Ok(h.finalize().to_vec())
}
/// 一次接受检查的轮数；每轮都在写入锁之外重新计算指纹。
const OUTSIDE_ROUNDS: u32 = 3;
/// 一轮等待应用写入锁多久后，就算作输入有变动的一轮：与 `crate::configure` 允许连接等待的时长相同（测试中更短）。
const LOCK_WAIT: Duration = Duration::from_millis(if cfg!(test) { 300 } else { 5000 });
/// 对每个连接做一次一致的读取：`read` 所做的一切都看到首次读取时已提交的状态。什么都不写，所以每个事务都以
/// ROLLBACK 结束。
fn read_snapshot<T>(dbs: &[&Connection], read: impl FnOnce() -> Result<T>) -> Result<T> {
    let mut open = vec![];
    let value = (|| {
        for c in dbs {
            c.execute_batch("BEGIN")?;
            open.push(*c);
        }
        read()
    })();
    for c in open {
        let end = c.execute_batch("ROLLBACK");
        if value.is_ok() {
            end?;
        }
    }
    value
}
/// source.sqlite 用回滚日志，所以在它上面的读事务会持有一把锁，让它的写入方（同步、识别导入）在 busy timeout 之后
/// 放弃（#226，`app_library::TASK_WRITE_WAIT`）。研究读取并复制 CCIP 向量时，在大图库上可能持锁
/// 很久。因此它对源库的读取会留意写入方提交前创建的日志文件，出现就让路，和后台完整性检查一样（#212）。
struct Writers {
    journal: PathBuf,
    seen: Arc<AtomicBool>,
}
impl Writers {
    /// 读取时多久查看一次日志文件：远在写入方的 busy timeout 之内。
    const POLL: Duration = Duration::from_millis(20);
    fn of(source: &Path) -> Self {
        let mut journal = source.as_os_str().to_owned();
        journal.push("-journal");
        Self { journal: journal.into(), seen: Arc::default() }
    }
    fn writing(&self) -> bool {
        self.journal.exists()
    }
    /// 在 SQL 进度回调以外的步骤检查源库写入方。
    fn check(&self) -> Result<()> {
        if self.seen.load(Ordering::SeqCst) || self.writing() {
            self.seen.store(true, Ordering::SeqCst);
            return Err(Error::Conflict(YIELDED.into()));
        }
        Ok(())
    }
    /// 运行 `read`，一旦出现写入方的日志文件就中断 `src`；此后出现的任何错误都当作让路。
    fn read<T>(&self, src: &mut Connection, read: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        self.check()?;
        let (journal, seen) = (self.journal.clone(), self.seen.clone());
        let mut polled = std::time::Instant::now();
        src.progress_handler(
            100,
            Some(move || {
                if polled.elapsed() >= Self::POLL {
                    polled = std::time::Instant::now();
                    if journal.exists() {
                        seen.store(true, Ordering::SeqCst);
                    }
                }
                seen.load(Ordering::SeqCst)
            }),
        )?;
        let result = read(src);
        src.progress_handler(0, None::<fn() -> bool>)?;
        match result {
            Err(_) if self.seen.load(Ordering::SeqCst) => Err(Error::Conflict(YIELDED.into())),
            result => result,
        }
    }
    /// 一直等到有一小段时间没看到写入方的日志文件；期间任务停止则返回错误。
    fn wait(&self, ctx: &TaskContext) -> Result<()> {
        let mut quiet = std::time::Instant::now();
        while quiet.elapsed() < STEP_ASIDE {
            ctx.check()?;
            if self.writing() {
                quiet = std::time::Instant::now();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }
}
/// SQL 进度回调以外的行、文件块和副本写入也要让路；避免每一行都查询文件系统。
fn poll<'a>(ctx: &'a TaskContext, writers: Option<&'a Writers>) -> impl Fn() -> Result<()> + 'a {
    let last = std::cell::Cell::new(std::time::Instant::now());
    move || {
        ctx.check()?;
        if let Some(writers) = writers {
            if last.get().elapsed() >= Writers::POLL {
                last.set(std::time::Instant::now());
                writers.check()?;
            }
        }
        Ok(())
    }
}
fn data_version(c: &Connection) -> Result<i64> {
    Ok(c.query_row("PRAGMA data_version", [], |r| r.get(0))?)
}
/// 发布类种类在接受时的输入检查（#29）。指纹在读事务里重新计算，不持有写入锁。在 BEGIN IMMEDIATE 下只比较
/// 本连接的 `PRAGMA data_version`（其他连接的任何提交都会改变它）和共享文件的戳；任一个变了，就放开锁重来一轮。
/// 指纹从不在锁下计算（#275）：在大图库上这要花很久，久到其他写入方都会放弃。连续 `OUTSIDE_ROUNDS` 轮都看到
/// 有提交时，检查以 `BUSY` 结束，什么都不发布。指纹不一致时总是 `INPUT_CHANGED`。`src` 在整个检查中是同一个读事务，
/// 所以指纹的源库部分只读一次。带 `writers` 时，读应用库、读共享文件和等锁都像读源库一样为源库写入方让路。
/// `between` 在每次取锁前运行（测试钩子）。
#[allow(clippy::too_many_arguments)]
fn confirm<T>(
    app: &mut Connection,
    src: &Connection,
    inputs: &Inputs<'_>,
    version: &str,
    ctx: &TaskContext,
    writers: Option<&Writers>,
    previous: Option<&InputHashes>,
    mut between: impl FnMut(),
    publish: impl FnOnce(rusqlite::Transaction<'_>) -> Result<T>,
) -> Result<T> {
    // 在行与文件块之间检查，不只是在 SQL 步骤之间：对共享文件计算哈希时不执行任何语句。
    let check = poll(ctx, writers);
    let source = source_digest(src, inputs, previous.map(|p| &p.source), &check)?.digest.finalize();
    for _ in 0..OUTSIDE_ROUNDS {
        let files = inputs.stamp()?;
        let read = |app: &mut Connection| {
            read_snapshot(&[app], || {
                let (db, digest) = app_digest(app, inputs, previous.map(|p| &p.app), &check)?;
                Ok((combine(&digest, &source), db.version))
            })
        };
        let (fresh, seen) = match writers {
            Some(writers) => writers.read(app, read)?,
            None => read(app)?,
        };
        if fresh != version {
            return Err(Error::Conflict(INPUT_CHANGED.into()));
        }
        between();
        let Some(tx) = lock(app, ctx, writers)? else {
            continue;
        };
        if data_version(&tx)? == seen && inputs.stamp()? == files {
            return publish(tx);
        }
    }
    Err(Error::Conflict(BUSY.into()))
}
/// 在 `LOCK_WAIT` 之内对应用数据库执行 BEGIN IMMEDIATE；其他写入方一直占着锁时返回 `None`。这期间检查持有源库读锁，
/// 而 SQLite 等锁时不运行进度回调，所以这里分成短步等待，在步与步之间发现源库写入方就让路。
fn lock<'c>(
    app: &'c Connection,
    ctx: &TaskContext,
    writers: Option<&Writers>,
) -> Result<Option<rusqlite::Transaction<'c>>> {
    let started = std::time::Instant::now();
    app.busy_timeout(Writers::POLL)?;
    let locked = loop {
        if let Err(e) = ctx.check().and_then(|_| writers.map_or(Ok(()), Writers::check)) {
            break Err(e);
        }
        match rusqlite::Transaction::new_unchecked(app, TransactionBehavior::Immediate) {
            Ok(tx) => break Ok(Some(tx)),
            Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::DatabaseBusy => {
                if started.elapsed() >= LOCK_WAIT {
                    break Ok(None);
                }
            }
            Err(e) => break Err(e.into()),
        }
    };
    // `crate::configure` 设置的等待时长，用于发布自己的语句。
    app.busy_timeout(Duration::from_secs(5))?;
    locked
}
/// 快照目录 `illustash-research-<task>` 在其运行结束时删除，不论结果如何（`Scratch`）。只有被杀掉的进程会留下这种
/// 目录；它比一天和两倍任务截止时长都旧时才删除，所以正在运行的任务绝不会丢掉自己的目录（#31）。
const SCRATCH_AGE: Duration = Duration::from_secs(24 * 3600);
fn sweep_scratch(root: &Path, age: Duration) -> usize {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(task) = name.to_str().and_then(|n| n.strip_prefix("illustash-research-")) else {
            continue;
        };
        if task.len() != 32 || !task.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let expired = fs::symlink_metadata(entry.path())
            .ok()
            .filter(|m| m.is_dir())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|elapsed| elapsed > age);
        if expired && fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
/// 在已固定的读事务上逐行复制。保留主键和 rowid，避免相似缓存的变更列表、学习戳和分组顺序发生变化。
fn copy_rows(
    from: &Connection,
    to: &Connection,
    table: &str,
    columns: &str,
    filter: &str,
    args: &[&dyn rusqlite::ToSql],
    check: &dyn Fn() -> Result<()>,
) -> Result<()> {
    use crate::app_create::quote_id;
    let name = quote_id(table);
    let mut read = from.prepare(&format!("SELECT {columns} FROM {name}{filter}"))?;
    let fields = read.column_names().map(quote_id).collect::<Vec<_>>();
    let marks = vec!["?"; fields.len()].join(",");
    let mut insert = to.prepare(&format!("INSERT INTO {name} ({}) VALUES ({marks})", fields.join(",")))?;
    let mut rows = read.query(args)?;
    while let Some(row) = rows.next()? {
        check()?;
        for i in 0..fields.len() {
            insert.raw_bind_parameter(i + 1, rusqlite::types::ToSqlOutput::Borrowed(row.get_ref(i)?))?;
        }
        insert.raw_execute()?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum SnapshotDb {
    App { regroup: bool },
    Source,
}
impl SnapshotDb {
    /// 核心学习发布在副本中按需重建角色投影。只有随后还要分组时才从 general_hits 读取原有的主题投影。
    fn copies_rows(self, scope: Scope, name: &str) -> bool {
        let Self::App { regroup } = self else { return true };
        if scope != Scope::Learning {
            return true;
        }
        if name == "term_projection" {
            return regroup;
        }
        if UNREAD.contains(&name) {
            return false;
        }
        // 学习戳、核心发布与分组共享术语和身份数据。用户扩展、导入报告、备份及任务历史无需交给 worker。
        name.starts_with("term_")
            || name.starts_with("identity_")
            || matches!(
                name,
                "terms"
                    | "meta"
                    | "actions"
                    | "human_object_contributions"
                    | "assets"
                    | "assertions"
                    | "candidates"
                    | "memberships"
                    | "tag_memberships"
                    | "single_assets"
                    | "lead_assets"
                    | "duplicates"
                    | "excluded"
                    | "deferred"
                    | "general_hits"
                    | "family_evidence"
                    | "learn_state"
                    | "prototypes"
                    | "proto_claims"
                    | "weak_claims"
                    | "relations"
                    | "image_sigs"
                    | "similar_paired"
                    | "similar_retry"
                    | "clusters"
                    | "cluster_info"
            )
    }
}
/// 应用学习副本还要在本地运行核心发布、重算投影，因而保留其视图、约束和触发器。其他计算只读自己的表；
/// OC 单独物化 general_hits 视图，relate 只取点名图片的路径。源库始终只复制 worker 会读取的输入。
fn copy_inputs(
    c: &Connection,
    path: &Path,
    inputs: &Inputs<'_>,
    part: SnapshotDb,
    check: &dyn Fn() -> Result<()>,
) -> Result<()> {
    use crate::app_create::quote_id;
    check()?;
    #[cfg(test)]
    tests::backup_step(path);
    let source = matches!(part, SnapshotDb::Source);
    let mut out = crate::sqlite::open(path)?;
    // 任务私有文件在副本事务完成后才交给 worker，进程中断后整份丢弃。
    out.pragma_update(None, "synchronous", "OFF")?;
    out.pragma_update(None, "foreign_keys", false)?;
    let tx = out.transaction()?;
    if !source && inputs.scope == Scope::Relate {
        tx.execute("CREATE TABLE assets(uuid TEXT PRIMARY KEY,src_path TEXT)", [])?;
        let ids = serde_json::to_string(&inputs.ids).map_err(|_| bad())?;
        copy_rows(
            c,
            &tx,
            "assets",
            "uuid,src_path",
            " WHERE uuid IN (SELECT value FROM json_each(?))",
            &[&ids],
            check,
        )?;
    } else {
        let only = if source { Some(inputs.scope.source()) } else { inputs.scope.app() };
        let mut schema = c.prepare(
            "SELECT s.type,s.name,s.tbl_name,s.sql,coalesce(t.wr,0)
             FROM sqlite_schema s LEFT JOIN pragma_table_list t ON t.schema='main' AND t.name=s.tbl_name
             WHERE t.type IN ('table','view') AND s.name NOT GLOB 'sqlite_*' AND s.sql IS NOT NULL
             ORDER BY CASE s.type WHEN 'table' THEN 0 WHEN 'index' THEN 1 WHEN 'view' THEN 2 ELSE 3 END,s.rowid",
        )?;
        let mut objects = schema.query([])?;
        while let Some(row) = objects.next()? {
            check()?;
            let kind: String = row.get(0)?;
            let name: String = row.get(1)?;
            let owner: String = row.get(2)?;
            if only.is_some_and(|list| !list.contains(&owner.as_str())) {
                continue;
            }
            if kind == "view" && only.is_some() {
                // 只复制这个视图的可见值，不把它所连接的术语库交给只读 worker。
                let query = c.prepare(&format!("SELECT * FROM {}", quote_id(&name)))?;
                let cols = query.column_names().map(quote_id).collect::<Vec<_>>();
                tx.execute(&format!("CREATE TABLE {} ({})", quote_id(&name), cols.join(",")), [])?;
                copy_rows(c, &tx, &name, "*", "", &[], check)?;
                continue;
            }
            if (source && kind == "index") || (only.is_some() && kind == "trigger") {
                continue;
            }
            let sql: String = row.get(3)?;
            tx.execute(&sql, [])?;
            if kind == "table" && part.copies_rows(inputs.scope, &name) {
                let without_rowid: bool = row.get(4)?;
                let cols = if without_rowid { "*" } else { "rowid AS rowid,*" };
                let filter = if source && name == "embeddings" { CCIP_ROWS } else { "" };
                copy_rows(c, &tx, &name, cols, filter, &[], check)?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}
fn queue_schema(c: &Connection) -> Result<()> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS research_runs(id TEXT PRIMARY KEY,kind TEXT NOT NULL,status TEXT NOT NULL,error TEXT NOT NULL DEFAULT '',created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);")?;
    Ok(())
}
/// 识别确认的是一个持久化的调度意图；计算本身另外保持待处理。
pub fn dispatch(library: &Library, kind: &str) -> Result<()> {
    if !configured() || library.frozen() {
        return Ok(());
    }
    let c = crate::sqlite::open(library.db())?;
    crate::configure(&c)?;
    queue_schema(&c)?;
    let events = c
        .prepare("SELECT id FROM backend_effects WHERE kind=? AND status='pending' ORDER BY id")?
        .query_map([kind], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if events.is_empty() {
        c.execute("INSERT INTO research_runs(id,kind,status) VALUES ('startup',?,'pending') ON CONFLICT(id) DO UPDATE SET status='pending',error='' WHERE status='done'",[kind])?;
    } else {
        for id in events {
            c.execute(
                "INSERT OR IGNORE INTO research_runs(id,kind,status) VALUES (?,?,'pending')",
                rusqlite::params![format!("effect:{kind}:{id}"), kind],
            )?;
        }
    }
    Ok(())
}
pub struct Research {
    task: Option<Task>,
    state: Arc<Mutex<State>>,
    generation: u64,
    session: String,
    pub similar_enabled: bool,
    pub anchors: PathBuf,
    paused: bool,
    retry_at: Option<std::time::Instant>,
    /// 自动冲突重试用完时设置；之后恢复要等显式的启动。
    exhausted: Arc<AtomicBool>,
    /// 上一次运行为源库写入方让路（`YIELDED`）时设置；恢复会很快重新启动它。
    stepped_aside: Arc<AtomicBool>,
}
impl Research {
    pub fn new(generation: u64, session: &str, similar_enabled: bool) -> Self {
        Self {
            generation,
            session: session.into(),
            similar_enabled,
            state: Arc::new(Mutex::new(json!({"status":if configured(){"idle"}else{"off"}}).into())),
            task: None,
            anchors: PathBuf::new(),
            paused: false,
            retry_at: None,
            exhausted: Arc::new(AtomicBool::new(false)),
            stepped_aside: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn state(&self) -> Json {
        self.state.lock().expect("research state mutex poisoned").snapshot()
    }
    pub fn similar_state(&self) -> Json {
        if !self.similar_enabled {
            return Json::object([("status", "off".into())]);
        }
        let raw = self.state();
        let keys = if raw.get("status").map(Json::text) == Some("idle") {
            vec!["status", "compared", "found", "seconds", "at"]
        } else {
            vec!["status", "done", "total", "found", "message", "diagnostic"]
        };
        Json::Object(keys.into_iter().filter_map(|k| raw.get(k).cloned().map(|v| (k.to_owned(), v))).collect())
    }
    pub fn learner_state(&self) -> Json {
        let raw = self.state();
        let mut value = Json::object([("status", raw.get("status").cloned().unwrap_or_else(|| "off".into()))]);
        if matches!(value.get("status").map(Json::text), Some("reading" | "pairing" | "comparing")) {
            value.set("status", "learning".into());
        }
        if value.get("status").map(Json::text) == Some("idle") {
            if let Some(at) = raw.get("learn_at") {
                value.set("at", at.clone());
            }
            for k in ["relearned", "changed", "clusters"] {
                if let Some(v) = raw.get(k) {
                    value.set(k, v.clone());
                }
            }
        } else {
            for key in ["message", "diagnostic"] {
                if let Some(v) = raw.get(key) {
                    value.set(key, v.clone());
                }
            }
        }
        value
    }
    /// 本会话里不会再自己启动（`GET /api/learning` 的 `stopped`，#404）：用户取消过，或者自动重试用完后
    /// 恢复检查已经把它停下。要等显式的启动或重新打开图库。
    pub fn halted(&self) -> bool {
        self.paused
    }
    /// 下一次恢复检查会马上启动待处理的工作：没有在运行、不在重试等待、没有停下（#404 的提前恢复检查）。
    pub fn ready(&self) -> bool {
        !self.paused
            && !self.task.as_ref().is_some_and(Task::running)
            && self.retry_at.is_none_or(|t| std::time::Instant::now() >= t)
    }
    pub fn stop(&mut self) {
        if let Some(mut t) = self.task.take() {
            t.stop();
        }
        *self.state.lock().expect("research state mutex poisoned") =
            json!({"status":if configured(){"idle"}else{"off"}}).into();
    }
    pub fn cancel(&mut self) {
        self.stop();
        self.paused = true;
    }
    pub fn start(
        &mut self,
        library: &Library,
        kind: &str,
        args: Value,
        reply: Option<mpsc::Sender<Result<Json>>>,
    ) -> Result<bool> {
        if library.frozen() && !["oc", "relate"].contains(&kind) {
            return Err(Error::Invalid("验收图库是只读的".into()));
        }
        let worker = std::env::var_os("ILLUSTASH_RESEARCH_WORKER")
            .filter(|p| !p.is_empty())
            .ok_or_else(|| Error::Invalid("研究 worker 未配置，功能不可用".into()))?;
        if self.task.as_ref().is_some_and(Task::running) {
            return Ok(false);
        }
        self.task.take();
        self.paused = false;
        self.retry_at = None;
        self.exhausted.store(false, Ordering::SeqCst);
        self.stepped_aside.store(false, Ordering::SeqCst);
        if ["similar", "regroup"].contains(&kind) {
            let c = crate::sqlite::open(library.db())?;
            crate::configure(&c)?;
            queue_schema(&c)?;
            c.execute(
                "INSERT INTO research_runs(id,kind,status) VALUES (lower(hex(randomblob(16))),?,'pending')",
                [kind],
            )?;
        }
        let library = library.clone();
        let identity = TaskIdentity::new(&library, self.generation, &self.session)?;
        let kind = kind.to_string();
        let state = self.state.clone();
        let anchors = self.anchors.clone();
        let exhausted = self.exhausted.clone();
        let stepped_aside = self.stepped_aside.clone();
        let timeout: u64 =
            std::env::var("ILLUSTASH_RESEARCH_TIMEOUT_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(1800000);
        sweep_scratch(&std::env::temp_dir(), Duration::from_millis(timeout.saturating_mul(2)).max(SCRATCH_AGE));
        let running = json!({"status": match kind.as_str(){"similar"=>"pairing","relate"=>"comparing","oc"=>"grouping",_=>"learning"}});
        *state.lock().expect("research state mutex poisoned") = running.clone().into();
        self.task = Some(Task::start(identity, Some(Duration::from_millis(timeout)), move |ctx| {
            let (mut attempt, mut stepped) = (0, 0);
            let result = loop {
                // 每次尝试都取新的快照；过期的结果已被拒绝，从未发布。
                match run(&library, &ctx, &worker, &kind, args.clone(), &state, &anchors) {
                    Err(e) if yielded(&e) && ["oc", "relate"].contains(&kind.as_str()) => {
                        if stepped == CONFLICT_RETRIES {
                            break Err(Error::Conflict("图库一直在写入，查询未完成，请稍后重试".into()));
                        }
                        stepped += 1;
                        if let Err(e) = Writers::of(&library.source()).wait(&ctx) {
                            break Err(e);
                        }
                    }
                    Err(e) if input_changed(&e) && !["oc", "relate"].contains(&kind.as_str()) => {
                        if attempt == CONFLICT_RETRIES {
                            exhausted.store(true, Ordering::SeqCst);
                            break Err(Error::Conflict(format!(
                                "研究输入持续变化，自动重试 {CONFLICT_RETRIES} 次后仍未完成，请稍后手动重试"
                            )));
                        }
                        attempt += 1;
                        *state.lock().expect("research state mutex poisoned") = json!({
                            "status": "waiting",
                            "message": format!("输入有变化，稍后自动重试（{attempt}/{CONFLICT_RETRIES}）"),
                        })
                        .into();
                        if let Err(e) = pause(&ctx, retry_base() * 2u32.pow(attempt - 1)) {
                            break Err(e);
                        }
                        *state.lock().expect("research state mutex poisoned") = running.clone().into();
                    }
                    other => break other,
                }
            };
            match result {
                Ok(v) => {
                    let mut public = Json::object([("status", "idle".into())]);
                    if !["oc", "relate"].contains(&kind.as_str()) {
                        // 日期也要：任务面板据此显示「MM-DD hh:mm」，与其他任务一样（#287）。
                        let at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                        public.set("at", at.clone().into());
                        public.extend(v.clone());
                        public.set("learn_at", at.into());
                        public.set("status", "idle".into());
                    }
                    *state.lock().expect("research state mutex poisoned") = public.into();
                    if let Some(tx) = reply {
                        let _ = tx.send(Ok(v));
                    }
                    Ok(())
                }
                Err(e) if yielded(&e) || busy(&e) => {
                    // 这不算失败：待处理的工作保持原样，不带错误，等恢复重新启动：为源库写入方让路的很快就重启，
                    // 图库一直忙的则照常过一分钟（#275）。
                    stepped_aside.store(yielded(&e), Ordering::SeqCst);
                    let message = if yielded(&e) { YIELDED } else { BUSY };
                    *state.lock().expect("research state mutex poisoned") =
                        Json::object([("status", "waiting".into()), ("message", message.into())]).into();
                    if let Some(tx) = reply {
                        let _ = tx.send(Err(Error::Invalid(e.to_string())));
                    }
                    Err(e)
                }
                Err(e) if ctx.cancellation().load(Ordering::SeqCst) && !ctx.timed_out() => {
                    if let Some(tx) = reply {
                        let _ = tx.send(Err(Error::Invalid(e.to_string())));
                    }
                    Err(e)
                }
                Err(e) => {
                    if !["oc", "relate"].contains(&kind.as_str()) {
                        if let Ok(c) = crate::sqlite::open(library.db()) {
                            let _ = c.busy_timeout(Duration::from_millis(100));
                            let message = e.to_string().chars().take(500).collect::<String>();
                            let _=c.execute("UPDATE backend_effects SET error=? WHERE status='pending' AND kind IN ('verdict','imported','index_changed')",[&message]);
                            let _ = c.execute("UPDATE research_runs SET error=? WHERE status='pending'", [&message]);
                        }
                    }
                    let d = crate::diagnostics::failure(
                        crate::diagnostics::Operation::Research,
                        crate::diagnostics::Stage::Background,
                        &e,
                        crate::diagnostics::Publication::Unknown,
                    );
                    *state.lock().expect("research state mutex poisoned") = Json::object([
                        ("status", "error".into()),
                        ("message", e.reason().into()),
                        ("diagnostic", d.value().into()),
                    ])
                    .into();
                    if let Some(tx) = reply {
                        let _ = tx.send(Err(Error::Invalid(e.to_string())));
                    }
                    Err(e)
                }
            }
        }));
        Ok(true)
    }
    pub fn query(&mut self, library: &Library, kind: &str, args: Value) -> Result<mpsc::Receiver<Result<Json>>> {
        let (tx, rx) = mpsc::channel();
        if !self.start(library, kind, args, Some(tx))? {
            return Err(Error::Conflict("研究任务正在运行".into()));
        }
        Ok(rx)
    }
    /// 启动待处理的学习和相似工作。同步运行期间不启动：同步的批次导入会留下 `imported` 事件，由同步之后的一次运行
    /// 合并处理，免得学习在批次之间和识别抢 CPU。为源库写入方让路的运行（#226）稍后按同样的条件重新启动。
    pub fn recover(&mut self, library: &Library, sync_busy: bool) -> Result<()> {
        if !configured()
            || library.frozen()
            || sync_busy
            || self.paused
            || self.task.as_ref().is_some_and(Task::running)
            || self.retry_at.is_some_and(|t| std::time::Instant::now() < t)
        {
            return Ok(());
        }
        if self.task.as_ref().is_some_and(|t| {
            matches!(t.outcome(), crate::app_tasks::TaskOutcome::Failed(_) | crate::app_tasks::TaskOutcome::Cancelled)
        }) {
            self.task.take();
            if self.exhausted.load(Ordering::SeqCst) {
                // 持续的写入比重试撑得更久：保留错误，直到显式启动或重新打开。
                self.paused = true;
                return Ok(());
            }
            let wait =
                if self.stepped_aside.swap(false, Ordering::SeqCst) { STEP_ASIDE } else { Duration::from_secs(60) };
            self.retry_at = Some(std::time::Instant::now() + wait);
            return Ok(());
        }
        let c = crate::sqlite::open(library.db())?;
        crate::configure(&c)?;
        queue_schema(&c)?;
        // 被打断的运行没有回执：它原来的事件和意图仍待处理。
        c.execute("UPDATE research_runs SET status='pending' WHERE status='running'", [])?;
        let pending:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM backend_effects WHERE status='pending' AND kind IN ('verdict','imported','index_changed')) OR EXISTS(SELECT 1 FROM research_runs WHERE status='pending')",[],|r|r.get(0))?;
        if pending {
            self.start(library, "events", json!({"similar":self.similar_enabled}), None)?;
        }
        Ok(())
    }
}
/// 一次运行据以计算的内容：输入版本和 `dir` 里的副本，以及它们覆盖的待处理工作。
struct Snapshot {
    version: String,
    hashes: InputHashes,
    events: Vec<i64>,
    intents: Vec<String>,
    regroup: bool,
}
/// 副本及其版本，每个数据库都在一个读事务里读取。先读取并复制应用数据库，再碰 source.sqlite：这样研究只在读源库
/// 本身时才持有源库读锁，而读源库时会为源库写入方让路（`Writers`）。以前在持有这把锁时复制应用数据库，会让写入方
/// 等完整个复制（#275）。
fn snapshot(
    c: &Connection,
    src: &mut Connection,
    writers: &Writers,
    inputs: &Inputs<'_>,
    kind: &str,
    dir: &Path,
    ctx: &TaskContext,
) -> Result<Snapshot> {
    writers.read(src, |src| {
        read_snapshot(&[c, src], || {
            let (app_hash, app) = app_digest(c, inputs, None, &|| ctx.check())?;
            let events: Vec<i64> = c
                .prepare(
                    "SELECT id FROM backend_effects WHERE status='pending' AND kind IN ('verdict','imported','index_changed') ORDER BY id",
                )?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let intents: Vec<String> =
                if c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='research_runs')", [], |r| {
                    r.get::<_, bool>(0)
                })? {
                    c.prepare("SELECT id FROM research_runs WHERE status='pending' AND (?='events' OR kind=?)")?
                        .query_map(rusqlite::params![kind, kind], |r| r.get(0))?
                        .collect::<rusqlite::Result<_>>()?
                } else {
                    vec![]
                };
            let regroup = kind == "regroup"
                || kind == "events"
                    && (c.query_row(
                        "SELECT EXISTS(SELECT 1 FROM backend_effects WHERE status='pending' AND kind IN ('imported','index_changed'))",
                        [],
                        |r| r.get::<_, bool>(0),
                    )? || !intents.is_empty());
            {
                let _phase = Phase::start("copy_app");
                copy_inputs(c, &dir.join("library.sqlite"), inputs, SnapshotDb::App { regroup }, &|| ctx.check())?;
            }
            // 源库放最后：从第一次读起，研究就持有它的读锁。
            let check = poll(ctx, Some(writers));
            let source = source_digest(src, inputs, None, &check)?;
            let _phase = Phase::start("copy_source");
            copy_inputs(src, &dir.join("source.sqlite"), inputs, SnapshotDb::Source, &check)?;
            let version = combine(&app, &source.digest.clone().finalize());
            Ok(Snapshot { version, hashes: InputHashes { app: app_hash, source }, events, intents, regroup })
        })
    })
}
fn run(
    library: &Library,
    ctx: &TaskContext,
    worker: &std::ffi::OsStr,
    kind: &str,
    args: Value,
    state: &Mutex<State>,
    anchors: &Path,
) -> Result<Json> {
    let scratch = Scratch(std::env::temp_dir().join(format!("illustash-research-{}", ctx.identity().task_id)));
    fs::create_dir(&scratch.0)?;
    let inputs = Inputs::new(library, anchors, kind, &args);
    let mut c = if inputs.scope.publishes() {
        let c = crate::sqlite::open_with_flags(library.db(), rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        crate::configure(&c)?;
        c
    } else {
        crate::app_library::read_connection(&library.db())?
    };
    let mut src = crate::app_library::read_connection(&library.source())?;
    let writers = Writers::of(&library.source());
    let Snapshot { version, hashes, events, intents, regroup } =
        snapshot(&c, &mut src, &writers, &inputs, kind, &scratch.0, ctx)?;
    // 只有学习类的种类发布学习批次；其他种类跳过它完整的输入戳。
    let learning = if inputs.scope == Scope::Learning {
        let _phase = Phase::start("learning_token");
        crate::character_learning::snapshot(&mut crate::sqlite::open(scratch.0.join("library.sqlite"))?)?["token"]
            .clone()
    } else {
        Value::Null
    };
    let request = protocol::value(&protocol::ResearchRequest {
        format: protocol::Format::Research,
        identity: ctx.identity(),
        kind,
        args: &args,
        regroup,
        snapshot: &scratch.0,
        features: library.instance_path().join("features"),
        version: &version,
        learning_token: &learning,
        anchors,
        core: std::env::current_exe()?,
        delta_tables: true,
    })?;
    let input = scratch.0.join("request.json");
    let output = scratch.0.join("response.json");
    let progress = scratch.0.join("progress.json");
    fs::write(&input, serde_json::to_vec(&request).map_err(|_| bad())?)?;
    // 不需要单独的观察者：进度是一个大小有限的文件，由进程 API 的回调轮询。
    let mut cmd = Command::new(worker);
    cmd.env(crate::app_tasks::WORKER_KIND_ENV, "research")
        .arg(&input)
        .arg(&output)
        .env("ILLUSTASH_RESEARCH_PROGRESS", &progress);
    let mut last = std::time::Instant::now();
    let worker_phase = Phase::start("worker");
    let status = ctx.process_observed(&mut cmd, || {
        if last.elapsed() < Duration::from_millis(100) {
            return Ok(());
        }
        last = std::time::Instant::now();
        if fs::metadata(&progress).is_ok_and(|m| m.len() < 16384) {
            if let Ok(bytes) = fs::read(&progress) {
                if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                    if ["learning", "grouping", "reading", "pairing", "comparing"]
                        .contains(&v["status"].as_str().unwrap_or(""))
                    {
                        *state.lock().expect("research state mutex poisoned") =
                            crate::http_json::parse_python(std::str::from_utf8(&bytes).unwrap_or("{}"))
                                .and_then(|v| State::parse(v, &progress))
                                .unwrap_or_else(|_| State::from(json!({"status":"running"})));
                    }
                }
            }
        }
        Ok(())
    })?;
    drop(worker_phase);
    ctx.check()?;
    if !status.success() {
        return Err(Error::Invalid("研究 worker 崩溃或依赖不可用".into()));
    }
    if fs::metadata(&output)?.len() > 128 * 1024 * 1024 {
        return Err(bad());
    }
    let bytes = fs::read(&output)?;
    let ordered = crate::http_json::parse_python(std::str::from_utf8(&bytes).map_err(|_| bad())?)?;
    let response: protocol::ResearchResponse = protocol::parse(
        crate::strict_json::parse(
            std::str::from_utf8(&bytes).map_err(|e| Error::Invalid(format!("{}: {e}", output.display())))?,
        )
        .map_err(|e| Error::Invalid(format!("{}: {e}", output.display())))?,
        &output,
    )?;
    if response.format != protocol::Format::Research || response.version != version {
        return Err(bad());
    }
    let importing = || *state.lock().expect("research state mutex poisoned") = json!({"status":"importing"}).into();
    importing();
    // 图库忙得没法检查结果时，结果先保留（#275）：暂停后再检查，最多 `CONFLICT_RETRIES` 次，然后运行以 `BUSY` 结束，
    // 之后由恢复重新启动。
    let mut publish = || {
        ctx.receive(&response.identity, || {
            writers.read(&mut src, |src| {
                let src = src.transaction()?;
                if !inputs.scope.publishes() {
                    // 查询什么都不发布：一个读事务同时检查它的输入和答案。
                    let check = poll(ctx, Some(&writers));
                    return writers.read(&mut c, |c| {
                        read_snapshot(&[c], || {
                            let (_, app) = app_digest(c, &inputs, Some(&hashes.app), &check)?;
                            let source = source_digest(&src, &inputs, Some(&hashes.source), &check)?;
                            if combine(&app, &source.digest.finalize()) != version {
                                return Err(Error::Conflict(INPUT_CHANGED.into()));
                            }
                            validate_query(c, kind, &Value::Object(response.result.clone()), &args)?;
                            ordered.get("result").cloned().ok_or_else(bad)
                        })
                    });
                }
                confirm(
                    &mut c,
                    &src,
                    &inputs,
                    &version,
                    ctx,
                    Some(&writers),
                    Some(&hashes),
                    || {},
                    |tx| {
                        let _phase = Phase::start("publish");
                        crate::schema::begin_writing(&tx)?;
                        if kind != "similar" {
                            if let Some(batch) = response.learning.as_ref().filter(|v| !v.is_null()) {
                                if batch["token"] != learning {
                                    return Err(bad());
                                }
                                if crate::character_learning::publish_tx(&tx, batch)?["status"] == "stale" {
                                    return Err(Error::Conflict(LEARNING_CHANGED.into()));
                                }
                            }
                        }
                        let caches = response.tables.as_ref().ok_or_else(|| {
                            Error::Invalid(format!("{file}: 字段 tables: 缺少结果表", file = output.display()))
                        })?;
                        let expected: Vec<&str> = if kind == "similar" {
                            vec!["image_sigs", "relations", "similar_paired", "similar_retry"]
                        } else if regroup {
                            let mut v = vec!["clusters", "cluster_info"];
                            if kind == "events" && args["similar"] == true {
                                v.extend(["image_sigs", "relations", "similar_paired", "similar_retry"]);
                            }
                            v
                        } else {
                            vec![]
                        };
                        if caches.keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>()
                            != expected.iter().copied().collect()
                        {
                            return Err(bad());
                        }
                        let mut published = Published::default();
                        for table in expected.iter() {
                            publish_table(&tx, table, &caches[*table], ctx, &mut published)?;
                        }
                        if expected.contains(&"relations") {
                            crate::app_migrate_data::early_groups(&tx, &src)?;
                            crate::character_projection::fold(&tx)?;
                        }
                        validate_references(&tx, &expected, &published)?;
                        if kind == "events" {
                            for id in &events {
                                tx.execute("UPDATE backend_effects SET status='done',error='' WHERE id=?", [id])?;
                            }
                        }
                        for id in &intents {
                            tx.execute("UPDATE research_runs SET status='done',error='' WHERE id=?", [id])?;
                        }
                        ctx.check()?;
                        crate::schema::commit_writing(tx)?;
                        ordered.get("result").cloned().ok_or_else(bad)
                    },
                )
            })
        })
    };
    let mut waits = 0;
    loop {
        match publish() {
            Err(e) if busy(&e) && waits < CONFLICT_RETRIES => {
                waits += 1;
                *state.lock().expect("research state mutex poisoned") =
                    Json::object([("status", "waiting".into()), ("message", YIELDED.into())]).into();
                pause(ctx, retry_base() * 2u32.pow(waits - 1))?;
                importing();
            }
            published => return published,
        }
    }
}
/// 一次发布写入 relations 和签名的内容（#31）：插入或改写的行的 rowid，以及每个被删除或改写的行的旧版本，
/// 供 `validate_references` 使用。
#[derive(Default)]
struct Published {
    touched: std::collections::HashMap<&'static str, Vec<i64>>,
    old: std::collections::HashMap<&'static str, std::collections::HashSet<Vec<crate::table_delta::Cell>>>,
}
/// 结果里的一张缓存表（#31）。完整数组（较旧的 worker、clusters）与已存的行比较；worker 给出的变更列表
/// `{delete, append}` 应用到快照里的那些行上，接受时的指纹保证这些行仍然存着。两种方式都只写入不同的行，
/// 表最终与整表替换后的结果相同：同样的行，同样的 rowid 顺序。
fn publish_table(
    c: &Connection,
    table: &str,
    value: &Value,
    ctx: &TaskContext,
    published: &mut Published,
) -> Result<usize> {
    let n = CACHES.iter().find(|(t, _)| *t == table).ok_or_else(bad)?.1;
    if ["similar_retry", "similar_paired"].contains(&table) {
        c.execute_batch("CREATE TABLE IF NOT EXISTS similar_paired(uuid TEXT PRIMARY KEY);CREATE TABLE IF NOT EXISTS similar_retry(a TEXT NOT NULL,b TEXT NOT NULL,PRIMARY KEY(a,b));")?;
    }
    let checked = match table {
        "relations" => Some("relations"),
        "image_sigs" => Some("image_sigs"),
        _ => None,
    };
    let check = || ctx.check();
    let rows = |rows: &[Value]| rows.iter().map(|r| cache_row(table, n, r)).collect::<Result<Vec<_>>>();
    let written = match value {
        Value::Array(full) if full.len() <= 1000000 => {
            crate::table_delta::sync(c, table, rows(full)?, checked.is_some(), &check)?
        }
        Value::Object(change) if change.len() == 2 => {
            let delete = change.get("delete").and_then(Value::as_array).ok_or_else(bad)?;
            let append = change.get("append").and_then(Value::as_array).ok_or_else(bad)?;
            if delete.len() + append.len() > 1000000 {
                return Err(bad());
            }
            let keys = delete
                .iter()
                .map(|k| {
                    let k = k.as_array().ok_or_else(bad)?;
                    k.iter()
                        .map(|v| {
                            v.as_str().map(|s| crate::table_delta::Cell::Text(s.as_bytes().to_vec())).ok_or_else(bad)
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .collect::<Result<Vec<_>>>()?;
            crate::table_delta::apply(c, table, &keys, rows(append)?, checked.is_some(), &check)?
        }
        _ => return Err(bad()),
    };
    if let Some(table) = checked {
        let rows = written.rows();
        published.touched.entry(table).or_default().extend(written.touched);
        published.old.entry(table).or_default().extend(written.old);
        return Ok(rows);
    }
    Ok(written.rows())
}
/// 一行校验过的结果，表示为整表替换时插入的值。
fn cache_row(table: &str, n: usize, row: &Value) -> Result<Vec<crate::table_delta::Cell>> {
    let r = row.as_array().filter(|r| r.len() == n).ok_or_else(bad)?;
    validate_row(table, r)?;
    let mut values = vec![];
    for (i, v) in r.iter().enumerate() {
        values.push(if table == "image_sigs" && i == 2 && !v.is_null() {
            let hex = v.as_str().filter(|s| s.len() == 512 && s.is_ascii()).ok_or_else(bad)?;
            Sql::Blob(
                (0..hex.len())
                    .step_by(2)
                    .map(|j| u8::from_str_radix(&hex[j..j + 2], 16).map_err(|_| bad()))
                    .collect::<Result<Vec<_>>>()?,
            )
        } else {
            match v {
                Value::Null => Sql::Null,
                Value::String(s) => Sql::Text(s.clone()),
                Value::Number(v) => {
                    if let Some(i) = v.as_i64() {
                        Sql::Integer(i)
                    } else {
                        Sql::Real(v.as_f64().filter(|f| f.is_finite()).ok_or_else(bad)?)
                    }
                }
                _ => return Err(bad()),
            }
        });
    }
    Ok(values.into_iter().map(crate::table_delta::Cell::from).collect())
}
fn validate_row(table: &str, r: &[Value]) -> Result<()> {
    let text = |i: usize| r[i].is_string();
    let int = |i: usize| r[i].as_i64().is_some_and(|v| v >= 0);
    let optional_num = |i: usize| r[i].is_null() || r[i].as_f64().is_some_and(|f| f.is_finite());
    let valid = match table {
        "clusters" => text(0) && int(1),
        "cluster_info" => {
            int(0)
                && int(1)
                && (r[2].is_null() || int(2))
                && optional_num(3)
                && text(4)
                && (r[5].is_null() || text(5))
                && text(6)
        }
        "image_sigs" => {
            text(0) && optional_num(1) && (r[2].is_null() || text(2)) && r[3].as_i64().is_some_and(|i| i == 0 || i == 1)
        }
        "relations" => text(0) && text(1) && text(2) && optional_num(3) && text(4) && text(5),
        "similar_paired" => text(0),
        "similar_retry" => text(0) && text(1),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(bad())
    }
}
fn validate_references(c: &Connection, expected: &[&str], published: &Published) -> Result<()> {
    // 身份 id：`term_tree` 里的词条。
    let ids = "SELECT t.key FROM term_tree r JOIN terms t ON t.id = r.term_id";
    for (index,sql) in [
        "SELECT EXISTS(SELECT 1 FROM clusters WHERE uuid NOT IN (SELECT uuid FROM assets) OR cid NOT IN (SELECT cid FROM cluster_info))".to_owned(),
        "SELECT EXISTS(SELECT 1 FROM cluster_info WHERE size<2 OR (near IS NOT NULL AND near NOT IN (SELECT cid FROM cluster_info)) OR (near_kind IS NOT NULL AND near_kind NOT IN ('look','trait')) OR NOT json_valid(traits))".to_owned(),
        format!(
            "SELECT EXISTS(
               SELECT 1 FROM proto_claims
               WHERE uuid NOT IN (SELECT uuid FROM assets) OR entity NOT IN ({ids}) OR score<0 OR score>1)"
        ),
        format!(
            "SELECT EXISTS(
               SELECT 1 FROM weak_claims
               WHERE uuid NOT IN (SELECT uuid FROM assets) OR entity NOT IN ({ids})
                 OR source NOT IN ('PROTO','CCIP') OR score<0 OR score>1)"
        ),
        format!(
            "SELECT EXISTS(
               SELECT 1 FROM prototypes
               WHERE entity NOT IN ({ids}) OR threshold<0 OR threshold>1 OR hint<threshold OR hint>1)"
        ),
    ].into_iter().enumerate() {
        if index<2 && !expected.contains(&"clusters") {continue;}
        if c.query_row(&sql,[],|r|r.get::<_,bool>(0))? {return Err(bad());}
    }
    // relations 和签名（#31）：只检查本次发布写入的行；其他行都是原来存着的。写入的行如果指向已不在图库里的图片，
    // 只有与原来那行完全相同时才允许（计算器保留的历史）。
    for (table, shape, missing) in [
        (
            "relations",
            "a>=b OR kind NOT IN ('identical','duplicate','subtle','crop','variant','edit','similar','unrelated') OR changed<0 OR changed>1",
            "a NOT IN (SELECT uuid FROM assets) OR b NOT IN (SELECT uuid FROM assets)",
        ),
        ("image_sigs", "ok NOT IN (0,1) OR (ok=1 AND (length(grid)!=256 OR ratio<=0))", "uuid NOT IN (SELECT uuid FROM assets)"),
    ] {
        if !expected.contains(&table) {
            continue;
        }
        let touched = serde_json::to_string(published.touched.get(table).map(Vec::as_slice).unwrap_or_default())
            .map_err(|_| bad())?;
        let written = format!("FROM {table} WHERE rowid IN (SELECT value FROM json_each(?1))");
        if c.query_row(&format!("SELECT EXISTS(SELECT 1 {written} AND ({shape}))"), [&touched], |r| r.get::<_, bool>(0))? {
            return Err(bad());
        }
        let mut q = c.prepare(&format!("SELECT * {written} AND ({missing})"))?;
        let width = q.column_count();
        let mut rows = q.query([&touched])?;
        while let Some(r) = rows.next()? {
            let row = (0..width).map(|i| r.get_ref(i).map(crate::table_delta::Cell::from)).collect::<rusqlite::Result<Vec<_>>>()?;
            if !published.old.get(table).is_some_and(|old| old.contains(&row)) {
                return Err(bad());
            }
        }
    }
    Ok(())
}
fn validate_query(c: &Connection, kind: &str, v: &Value, args: &Value) -> Result<()> {
    if !v.is_object() {
        return Err(bad());
    }
    if kind == "oc" {
        for group in v["groups"].as_array().ok_or_else(bad)? {
            for id in group.as_array().ok_or_else(bad)? {
                let id = id.as_str().ok_or_else(bad)?;
                if !c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM memberships WHERE uuid=? AND entity=?)",
                    rusqlite::params![id, args["entity"].as_str().ok_or_else(bad)?],
                    |r| r.get::<_, bool>(0),
                )? {
                    return Err(bad());
                }
            }
        }
    } else {
        let ids = args["ids"].as_array().ok_or_else(bad)?;
        for p in v["pairs"].as_array().ok_or_else(bad)? {
            if !ids.contains(&p["a"]) || !ids.contains(&p["b"]) {
                return Err(bad());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! #29：只用合成的数据库和文件。
    use super::*;
    use crate::app_tasks::{Task, TaskIdentity};
    use std::collections::BTreeSet;

    #[test]
    fn diagnostic_receipt_reaches_similar_and_learner_public_states() {
        let research = Research::new(1, "synthetic-session", true);
        *research.state.lock().unwrap() = json!({"status":"error", "message":"synthetic failure",
            "diagnostic":{"id":"D-synthetic", "publication":"unknown", "retry":"check_state"}})
        .into();
        for state in [research.similar_state(), research.learner_state()] {
            assert_eq!(state.get("diagnostic").unwrap().get("id").unwrap().text(), "D-synthetic");
            assert_eq!(state.get("message").unwrap().text(), "synthetic failure");
        }
    }

    const ALL: [Scope; 4] = [Scope::Learning, Scope::Similar, Scope::Oc, Scope::Relate];
    struct Fixture {
        root: PathBuf,
        anchors: PathBuf,
        _task: Task,
        ctx: TaskContext,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("illustash-research-scope-{}", crate::store::new_key().unwrap()));
            let anchors = root.join("shared");
            fs::create_dir_all(root.join("features")).unwrap();
            fs::create_dir_all(anchors.join("char-index")).unwrap();
            let app = crate::sqlite::open(root.join("library.sqlite")).unwrap();
            app.execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT);
                 INSERT INTO meta VALUES ('names_version','0'),('names_projected','0'),('names_display_projected','0'),('identity_policy','hybrid');
                 CREATE TABLE assets(uuid TEXT PRIMARY KEY,src_path TEXT);
                 CREATE TABLE prefs(key TEXT PRIMARY KEY,value TEXT NOT NULL);
                 CREATE TABLE name_cache(key TEXT PRIMARY KEY,value TEXT NOT NULL);
                 CREATE TABLE name_settings(key TEXT PRIMARY KEY,value TEXT NOT NULL);
                 CREATE TABLE term_names(term_id INTEGER,value TEXT);
                 CREATE TABLE term_key_names(kind TEXT,key TEXT,value TEXT);
                 CREATE TABLE term_key_aliases(kind TEXT,key TEXT,value TEXT,position INTEGER);
                 CREATE TABLE term_name_display(entity TEXT,skin_only INTEGER,name TEXT);
                 CREATE TABLE term_name_choice(kind TEXT,key TEXT,name TEXT);
                 CREATE TABLE sync_runs(id INTEGER PRIMARY KEY,status TEXT);
                 CREATE TABLE backend_effects(id INTEGER PRIMARY KEY,kind TEXT,status TEXT DEFAULT 'pending');
                 CREATE TABLE terms(id INTEGER PRIMARY KEY,key TEXT UNIQUE);
                 CREATE TABLE term_tree(term_id INTEGER PRIMARY KEY);
                 CREATE TRIGGER names_version_term_names AFTER INSERT ON term_names
                   BEGIN UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='names_version'; END;
                 CREATE TABLE assertions(uuid TEXT,entity TEXT,status TEXT);
                 CREATE TABLE memberships(uuid TEXT,entity TEXT);
                 CREATE TABLE duplicates(uuid TEXT,grp TEXT,hidden INTEGER);
                 CREATE TABLE excluded(uuid TEXT);
                 CREATE TABLE general_hits(uuid TEXT,value TEXT);
                 CREATE TABLE image_sigs(uuid TEXT PRIMARY KEY,ratio REAL,grid BLOB,ok INTEGER);
                 CREATE TABLE relations(a TEXT,b TEXT,kind TEXT,changed INTEGER,note TEXT,checked_at TEXT);",
            )
            .unwrap();
            for id in ["a", "b", "c"] {
                let path = root.join(format!("{id}.png"));
                fs::write(&path, id).unwrap();
                app.execute("INSERT INTO assets VALUES (?,?)", [id, path.to_str().unwrap()]).unwrap();
            }
            let src = crate::sqlite::open(root.join("source.sqlite")).unwrap();
            src.execute_batch(
                "CREATE TABLE embeddings(uuid TEXT,model TEXT,vec BLOB,PRIMARY KEY(uuid,model));
                 CREATE TABLE characters(char_id TEXT PRIMARY KEY,post_count INTEGER);",
            )
            .unwrap();
            let task =
                Task::start(TaskIdentity::at(&root, "synthetic", 1, "research-scope").unwrap(), None, |_| Ok(()));
            let ctx = task.context();
            Self { root, anchors, _task: task, ctx }
        }
        fn inputs(&self, scope: Scope) -> Inputs<'_> {
            Inputs {
                scope,
                ids: vec!["a".into(), "b".into()],
                features: self.root.join("features"),
                anchors: &self.anchors,
            }
        }
        fn app(&self) -> Connection {
            crate::sqlite::open(self.root.join("library.sqlite")).unwrap()
        }
        fn source(&self) -> Connection {
            crate::sqlite::open(self.root.join("source.sqlite")).unwrap()
        }
        fn version(&self, scope: Scope) -> String {
            fingerprint(&self.app(), &self.source(), &self.inputs(scope), &self.ctx).unwrap()
        }
        fn versions(&self) -> Vec<String> {
            ALL.iter().map(|s| self.version(*s)).collect()
        }
        /// 对 `scope` 运行接受检查；返回是否发布了，以及运行了多少轮。
        fn confirm(&self, scope: Scope, version: &str, mut between: impl FnMut(u32)) -> (Result<()>, u32) {
            let mut app = self.app();
            let mut src = self.source();
            let src = src.transaction().unwrap();
            let mut rounds = 0;
            let result = confirm(
                &mut app,
                &src,
                &self.inputs(scope),
                version,
                &self.ctx,
                None,
                None,
                || {
                    rounds += 1;
                    between(rounds)
                },
                |tx| {
                    tx.execute("INSERT INTO image_sigs VALUES ('published',1.0,NULL,0)", [])?;
                    tx.commit()?;
                    Ok(())
                },
            );
            (result, rounds)
        }
        fn published(&self) -> bool {
            self.app()
                .query_row("SELECT EXISTS(SELECT 1 FROM image_sigs WHERE uuid='published')", [], |r| r.get(0))
                .unwrap()
        }
    }
    fn conflict(r: &Result<()>) -> bool {
        matches!(r, Err(Error::Conflict(m)) if m == INPUT_CHANGED)
    }

    #[test]
    fn gallery_table_and_history_are_ignored_by_learning_fingerprint() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE galleries(id INTEGER,name TEXT);
             CREATE TABLE gallery_members(gallery_id INTEGER,uuid TEXT);
             CREATE TABLE actions(id INTEGER,undone INTEGER,before_json TEXT);",
        )
        .unwrap();
        let digest = || {
            let mut h = Sha256::new();
            hash_db(&c, &mut h, Scope::Learning.app(), UNREAD, &|| Ok(())).unwrap();
            h.finalize()
        };
        let before = digest();
        c.execute_batch(
            "INSERT INTO galleries VALUES (1,'合成画廊');
             INSERT INTO gallery_members VALUES (1,'synthetic-a');
             INSERT INTO actions VALUES (1,0,'[{\"gallery\":{\"id\":1,\"row\":null}}]');",
        )
        .unwrap();
        assert_eq!(digest(), before);
        c.execute_batch("DELETE FROM gallery_members; DELETE FROM galleries; UPDATE actions SET undone=1 WHERE id=1;")
            .unwrap();
        assert_eq!(digest(), before);
        c.execute_batch("INSERT INTO actions VALUES (2,0,'[]');").unwrap();
        assert_ne!(digest(), before);
    }

    #[test]
    fn similar_seen_and_its_history_are_ignored_by_learning_fingerprint() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE similar_seen(id INTEGER,uuid TEXT,seen_at TEXT);
             CREATE TABLE actions(id INTEGER,undone INTEGER,before_json TEXT);",
        )
        .unwrap();
        let digest = || {
            let mut h = Sha256::new();
            hash_db(&c, &mut h, Scope::Learning.app(), UNREAD, &|| Ok(())).unwrap();
            h.finalize()
        };
        let before = digest();
        c.execute_batch(
            "INSERT INTO similar_seen VALUES (1,'a','t'),(1,'b','t');
             INSERT INTO actions VALUES (1,0,'[{\"similar_seen\":{\"id\":1,\"rows\":null}}]');",
        )
        .unwrap();
        assert_eq!(digest(), before);
        c.execute_batch("DELETE FROM similar_seen; UPDATE actions SET undone=1 WHERE id=1;").unwrap();
        assert_eq!(digest(), before);
    }

    #[test]
    fn snapshot_copies_only_the_worker_inputs() {
        let f = Fixture::new();
        f.app()
            .execute_batch(
                "INSERT INTO name_settings VALUES ('model_key','synthetic-key');
             INSERT INTO term_names VALUES (1,'合成');
             INSERT INTO term_name_display VALUES ('e1',0,'合成');
             INSERT INTO memberships VALUES ('a','e1');
             CREATE TABLE local_archive(v,display AS (v+1)); INSERT INTO local_archive(v) VALUES (5);
             CREATE TABLE term_projection(id INTEGER PRIMARY KEY); INSERT INTO term_projection VALUES (1);
             DROP TABLE general_hits;
             CREATE TABLE tag_input(uuid TEXT,value TEXT);
             INSERT INTO tag_input VALUES ('a','blue_hair');
             CREATE VIEW general_hits AS SELECT * FROM tag_input;",
            )
            .unwrap();
        f.source()
            .execute_batch(
                "INSERT INTO embeddings VALUES ('a','ccip',X'01'),('a','wd',X'02');
             INSERT INTO characters VALUES ('c',4);
             CREATE TABLE labels(uuid TEXT); INSERT INTO labels VALUES ('a');",
            )
            .unwrap();
        for (scope, kind) in
            [(Scope::Learning, "events"), (Scope::Similar, "similar"), (Scope::Oc, "oc"), (Scope::Relate, "relate")]
        {
            let dir = f.root.join(kind);
            fs::create_dir(&dir).unwrap();
            let taken = snapshot(
                &f.app(),
                &mut f.source(),
                &Writers::of(&f.root.join("source.sqlite")),
                &f.inputs(scope),
                kind,
                &dir,
                &f.ctx,
            )
            .unwrap();
            assert_eq!(taken.version, f.version(scope));
            let app = crate::sqlite::open(dir.join("library.sqlite")).unwrap();
            let source = crate::sqlite::open(dir.join("source.sqlite")).unwrap();
            let tables = |c: &Connection, table: &str| {
                c.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE name=?", [table], |row| row.get::<_, i64>(0))
                    .unwrap()
            };
            assert_eq!(tables(&source, "labels"), 0);
            if scope == Scope::Learning {
                for table in ["local_archive", "term_projection", "term_names", "term_name_display"] {
                    assert_eq!(
                        app.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get::<_, i64>(0)).unwrap(),
                        0
                    );
                }
                assert_eq!(app.query_row("SELECT count(*) FROM name_settings", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
            }
            assert_eq!(tables(&source, "characters"), i64::from(scope == Scope::Learning));
            if scope == Scope::Relate {
                assert_eq!(tables(&source, "embeddings"), 0);
                assert_eq!(
                    app.query_row(
                        "SELECT group_concat(uuid,',') FROM (SELECT uuid FROM assets ORDER BY uuid)",
                        [],
                        |r| r.get::<_, String>(0)
                    )
                    .unwrap(),
                    "a,b"
                );
            } else {
                assert_eq!(
                    source
                        .query_row("SELECT group_concat(model) FROM embeddings", [], |r| r.get::<_, String>(0))
                        .unwrap(),
                    "ccip"
                );
            }
            if scope == Scope::Oc {
                assert_eq!(
                    app.query_row("SELECT value FROM general_hits WHERE uuid='a'", [], |r| r.get::<_, String>(0))
                        .unwrap(),
                    "blue_hair"
                );
                assert_eq!(tables(&app, "tag_input"), 0);
            }
        }
    }
    #[test]
    fn cached_database_hashes_still_guard_source_commits_and_external_files() {
        let f = Fixture::new();
        let inputs = f.inputs(Scope::Learning);
        let mut app = f.app();
        let mut src = f.source();
        let dir = f.root.join("snapshot");
        fs::create_dir(&dir).unwrap();
        let taken =
            snapshot(&app, &mut src, &Writers::of(&f.root.join("source.sqlite")), &inputs, "events", &dir, &f.ctx)
                .unwrap();
        let mut accept = || {
            let pinned = src.transaction().unwrap();
            confirm(
                &mut app,
                &pinned,
                &inputs,
                &taken.version,
                &f.ctx,
                None,
                Some(&taken.hashes),
                || {},
                |tx| {
                    let writer = f.source();
                    writer.busy_timeout(Duration::ZERO)?;
                    let error = writer.execute("INSERT INTO characters VALUES ('late',1)", []).unwrap_err();
                    assert_eq!(error.sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseBusy));
                    Ok(tx.commit()?)
                },
            )
        };
        // 同连接命中缓存仍持有源库读锁，直到应用事务提交。
        assert!(accept().is_ok());
        f.source().execute("INSERT INTO embeddings VALUES ('a','wd',X'00')", []).unwrap();
        assert!(accept().is_ok());
        fs::write(f.root.join("a.png"), "new external content").unwrap();
        assert!(conflict(&accept()));
        // 独立核对源库的版本失效分支。
        let old =
            read_snapshot(&[&src], || source_digest(&src, &inputs, Some(&taken.hashes.source), &|| Ok(()))).unwrap();
        f.source().execute("INSERT INTO embeddings VALUES ('a','ccip',X'01')", []).unwrap();
        let fresh = read_snapshot(&[&src], || source_digest(&src, &inputs, Some(&old), &|| Ok(()))).unwrap();
        assert_ne!(fresh.digest.finalize(), old.digest.finalize());
    }

    #[test]
    fn oc_rechecks_the_visible_tags_and_only_ccip_vectors() {
        let f = Fixture::new();
        f.app()
            .execute_batch(
                "DROP TABLE general_hits;
             CREATE TABLE tag_input(uuid TEXT,value TEXT);
             CREATE VIEW general_hits AS SELECT * FROM tag_input;",
            )
            .unwrap();
        let before = f.version(Scope::Oc);
        f.source().execute("INSERT INTO embeddings VALUES ('a','wd',X'01')", []).unwrap();
        assert_eq!(f.version(Scope::Oc), before);
        f.app().execute("INSERT INTO tag_input VALUES ('a','blue_hair')", []).unwrap();
        assert_ne!(f.version(Scope::Oc), before);
    }

    #[test]
    fn each_kind_fingerprints_only_its_inputs() {
        let f = Fixture::new();
        let app = |sql: &str| f.app().execute_batch(sql).unwrap();
        let src = |sql: &str| f.source().execute_batch(sql).unwrap();
        type Change<'a> = Box<dyn Fn() + 'a>;
        use Scope::*;
        let changes: Vec<(&str, Change, &[Scope])> = vec![
            ("prefs", Box::new(|| app("INSERT INTO prefs VALUES ('synthetic_view','grid')")), &[]),
            ("name cache", Box::new(|| app("INSERT INTO name_cache VALUES ('synthetic','{}')")), &[]),
            ("name settings", Box::new(|| app("INSERT INTO name_settings VALUES ('synthetic_setting','42')")), &[]),
            ("model key", Box::new(|| app("INSERT INTO name_settings VALUES ('model_key','synthetic')")), &[]),
            // 核心名称表、显示投影表与名称版本元数据都不被任何研究计算器或学习戳读取（#752）。
            ("names + names_version", Box::new(|| app("INSERT INTO term_names VALUES (1,'合成')")), &[]),
            ("display names", Box::new(|| app("INSERT INTO term_key_names VALUES ('character','e1','合成')")), &[]),
            ("key aliases", Box::new(|| app("INSERT INTO term_key_aliases VALUES ('work','w1','合成',0)")), &[]),
            (
                "name projection and meta",
                Box::new(|| {
                    app("INSERT INTO term_name_display VALUES ('e1',0,'合成');
                    INSERT INTO term_name_choice VALUES ('character','e1','合成');
                    UPDATE meta SET value='1' WHERE key IN ('names_projected','names_display_projected');")
                }),
                &[],
            ),
            ("sync history", Box::new(|| app("INSERT INTO sync_runs(status) VALUES ('done')")), &[]),
            ("pending event", Box::new(|| app("INSERT INTO backend_effects(kind) VALUES ('verdict')")), &[]),
            (
                "backup digest, state and revision",
                Box::new(|| {
                    app("CREATE TABLE human_asset_hashes(asset_key TEXT,digest TEXT);
                    CREATE TABLE human_backup_state(id INTEGER,state_json TEXT);
                    INSERT INTO human_asset_hashes VALUES ('a','synthetic');
                    INSERT INTO human_backup_state VALUES (1,'{}');
                    INSERT INTO meta VALUES ('human_revision','1');")
                }),
                &[],
            ),
            ("human verdict", Box::new(|| app("INSERT INTO assertions VALUES ('a','e1','confirmed')")), &[Learning]),
            (
                "identity policy",
                Box::new(|| app("UPDATE meta SET value='default' WHERE key='identity_policy'")),
                &[Learning],
            ),
            (
                "entity",
                Box::new(|| app("INSERT INTO terms VALUES (2,'e2'); INSERT INTO term_tree VALUES (2)")),
                &[Learning],
            ),
            ("membership", Box::new(|| app("INSERT INTO memberships VALUES ('a','e1')")), &[Learning, Oc]),
            ("general tag", Box::new(|| app("INSERT INTO general_hits VALUES ('a','blue_hair')")), &[Learning, Oc]),
            ("excluded", Box::new(|| app("INSERT INTO excluded VALUES ('c')")), &[Learning, Similar, Oc]),
            ("duplicate", Box::new(|| app("INSERT INTO duplicates VALUES ('a','g',0)")), &[Learning, Similar, Oc]),
            ("signature", Box::new(|| app("INSERT INTO image_sigs VALUES ('c',1.0,NULL,0)")), &[Learning, Similar]),
            (
                "relation",
                Box::new(|| app("INSERT INTO relations VALUES ('a','b','similar',0,'','t')")),
                &[Learning, Similar],
            ),
            ("other asset", Box::new(|| app("INSERT INTO assets VALUES ('z','')")), &[Learning, Similar]),
            (
                "requested path",
                Box::new(|| app("UPDATE assets SET src_path=src_path||'.moved' WHERE uuid='a'")),
                &[Learning, Similar, Relate],
            ),
            (
                "ccip vector",
                Box::new(|| src("INSERT INTO embeddings VALUES ('a','ccip',x'00')")),
                &[Learning, Similar, Oc],
            ),
            ("character metadata", Box::new(|| src("INSERT INTO characters VALUES ('c1',3)")), &[Learning]),
            (
                "feature file",
                Box::new(|| fs::write(f.root.join("features/full-manifest.csv"), "uuid\na\n").unwrap()),
                &[Learning, Similar, Oc],
            ),
            ("anchor file", Box::new(|| fs::write(f.anchors.join("char-index/c1.json"), "{}").unwrap()), &[Learning]),
            (
                "requested image",
                Box::new(|| fs::write(f.root.join("b.png"), "changed bytes").unwrap()),
                &[Learning, Similar, Relate],
            ),
        ];
        for (name, change, expected) in changes {
            let before = f.versions();
            change();
            let after = f.versions();
            let changed: BTreeSet<_> = ALL
                .iter()
                .zip(before.iter().zip(&after))
                .filter(|(_, (b, a))| b != a)
                .map(|(s, _)| format!("{s:?}"))
                .collect();
            let expected: BTreeSet<_> = expected.iter().map(|s| format!("{s:?}")).collect();
            assert_eq!(changed, expected, "{name}");
        }
    }

    #[test]
    fn unread_commit_between_check_and_lock_is_accepted() {
        let f = Fixture::new();
        for scope in [Scope::Learning, Scope::Similar] {
            let version = f.version(scope);
            let (result, rounds) = f.confirm(scope, &version, |round| {
                if round == 1 {
                    f.app()
                        .execute("INSERT OR REPLACE INTO prefs VALUES ('synthetic',?)", [format!("{scope:?}")])
                        .unwrap();
                }
            });
            // 这次提交改变了 data_version，所以第二轮确认了输入没变。
            assert!(result.is_ok(), "{scope:?}: {result:?}");
            assert_eq!(rounds, 2, "{scope:?}");
            assert!(f.published());
            f.app().execute("DELETE FROM image_sigs WHERE uuid='published'", []).unwrap();
        }
        // 中间什么都没发生：只有一轮，取锁后只比较 data_version 和文件戳。
        let version = f.version(Scope::Learning);
        assert_eq!(f.confirm(Scope::Learning, &version, |_| {}).1, 1);
    }

    #[test]
    fn relevant_commit_between_check_and_lock_is_rejected() {
        let f = Fixture::new();
        // 在外部检查之后、BEGIN IMMEDIATE 之前提交：由 data_version 发现。
        let version = f.version(Scope::Learning);
        let (result, rounds) = f.confirm(Scope::Learning, &version, |round| {
            if round == 1 {
                f.app().execute("INSERT INTO assertions VALUES ('b','e1','rejected')", []).unwrap();
            }
        });
        assert!(conflict(&result), "{result:?}");
        assert_eq!(rounds, 1);
        assert!(!f.published());
        let version = f.version(Scope::Similar);
        let (result, _) = f.confirm(Scope::Similar, &version, |round| {
            if round == 1 {
                f.app().execute("INSERT INTO excluded VALUES ('a')", []).unwrap();
            }
        });
        assert!(conflict(&result), "{result:?}");
        assert!(!f.published());
        // 同一时间窗内替换了一个共享文件：由文件戳发现。
        let version = f.version(Scope::Learning);
        let (result, _) = f.confirm(Scope::Learning, &version, |round| {
            if round == 1 {
                fs::write(f.anchors.join("char-anchors.txt"), "c1\n").unwrap();
            }
        });
        assert!(conflict(&result), "{result:?}");
        assert!(!f.published());
    }

    #[test]
    fn rounds_end_without_publishing_while_commits_continue() {
        let f = Fixture::new();
        // 每次取锁前都有不读的表的提交：没有一轮能等到输入稳定，也没有一轮在锁下计算哈希。
        let version = f.version(Scope::Learning);
        let (result, rounds) = f.confirm(Scope::Learning, &version, |round| {
            f.app().execute("INSERT OR REPLACE INTO prefs VALUES ('synthetic',?)", [round]).unwrap();
        });
        assert!(matches!(&result, Err(e) if busy(e)), "{result:?}");
        assert_eq!(rounds, OUTSIDE_ROUNDS);
        assert!(!f.published());
        // 在最后一次取锁前提交的真实输入，会被下一次检查的外部轮发现。
        let (result, rounds) = f.confirm(Scope::Learning, &version, |round| {
            let sql = if round == OUTSIDE_ROUNDS {
                "INSERT INTO memberships VALUES ('c','e1')"
            } else {
                "INSERT OR REPLACE INTO prefs VALUES ('synthetic','x')"
            };
            f.app().execute(sql, []).unwrap();
        });
        assert!(matches!(&result, Err(e) if busy(e)), "{result:?}");
        assert_eq!(rounds, OUTSIDE_ROUNDS);
        let (result, rounds) = f.confirm(Scope::Learning, &version, |_| {});
        assert!(conflict(&result), "{result:?}");
        assert_eq!(rounds, 0);
        assert!(!f.published());
    }

    /// 在 `LOCK_WAIT` 之内拿不到写入锁的一轮，算作输入有变动的一轮。
    #[test]
    fn a_held_write_lock_ends_the_round() {
        let f = Fixture::new();
        let version = f.version(Scope::Similar);
        let holder = f.app();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        let started = std::time::Instant::now();
        let (result, rounds) = f.confirm(Scope::Similar, &version, |_| {});
        assert!(started.elapsed() >= LOCK_WAIT * OUTSIDE_ROUNDS);
        assert!(matches!(&result, Err(e) if busy(e)), "{result:?}");
        assert_eq!(rounds, OUTSIDE_ROUNDS);
        holder.execute_batch("ROLLBACK").unwrap();
        assert!(f.confirm(Scope::Similar, &version, |_| {}).0.is_ok());
        assert!(f.published());
    }

    /// #31 之前的发布方式：先完整地临时复制 relations 和签名，再把每张结果表按结果顺序删掉重插，最后做整表检查。
    /// 它用的行转换是 `cache_row`（原样移到了那里）。
    fn legacy_publish(tx: &Connection, tables: &serde_json::Map<String, Value>, ctx: &TaskContext) -> Result<()> {
        tx.execute_batch("CREATE TEMP TABLE research_old_relations AS SELECT * FROM relations;CREATE TEMP TABLE research_old_sigs AS SELECT * FROM image_sigs;")?;
        for (table, value) in tables {
            legacy_replace(tx, table, value, ctx)?;
        }
        for sql in [
            "SELECT EXISTS(SELECT 1 FROM relations r WHERE a>=b OR ((a NOT IN (SELECT uuid FROM assets) OR b NOT IN (SELECT uuid FROM assets)) AND NOT EXISTS(SELECT 1 FROM research_old_relations o WHERE o.a=r.a AND o.b=r.b AND o.kind=r.kind AND o.changed IS r.changed AND o.note=r.note AND o.checked_at=r.checked_at)) OR kind NOT IN ('identical','duplicate','subtle','crop','variant','edit','similar','unrelated') OR changed<0 OR changed>1)",
            "SELECT EXISTS(SELECT 1 FROM image_sigs r WHERE (uuid NOT IN (SELECT uuid FROM assets) AND NOT EXISTS(SELECT 1 FROM research_old_sigs o WHERE o.uuid=r.uuid AND o.ratio IS r.ratio AND o.grid IS r.grid AND o.ok=r.ok)) OR ok NOT IN (0,1) OR (ok=1 AND (length(grid)!=256 OR ratio<=0)))",
        ] {
            if tx.query_row(sql, [], |r| r.get::<_, bool>(0))? {
                return Err(bad());
            }
        }
        Ok(())
    }
    fn legacy_replace(c: &Connection, table: &str, value: &Value, ctx: &TaskContext) -> Result<()> {
        let n = CACHES.iter().find(|(t, _)| *t == table).ok_or_else(bad)?.1;
        let rows = value.as_array().ok_or_else(bad)?;
        if rows.len() > 1000000 {
            return Err(bad());
        }
        if ["similar_retry", "similar_paired"].contains(&table) {
            c.execute_batch("CREATE TABLE IF NOT EXISTS similar_paired(uuid TEXT PRIMARY KEY);CREATE TABLE IF NOT EXISTS similar_retry(a TEXT NOT NULL,b TEXT NOT NULL,PRIMARY KEY(a,b));")?;
        }
        c.execute(&format!("DELETE FROM {table}"), [])?;
        let mut q = c.prepare(&format!("INSERT INTO {table} VALUES ({})", vec!["?"; n].join(",")))?;
        for row in rows {
            ctx.check()?;
            let values = cache_row(table, n, row)?;
            q.execute(rusqlite::params_from_iter(values.iter()))?;
        }
        Ok(())
    }
    /// 相似表现在的发布方式，与 `run` 的做法相同。
    fn publish(tx: &Connection, tables: &serde_json::Map<String, Value>, ctx: &TaskContext) -> Result<usize> {
        let mut published = Published::default();
        let mut written = 0;
        for (table, value) in tables {
            written += publish_table(tx, table, value, ctx, &mut published)?;
        }
        validate_references(tx, &tables.keys().map(String::as_str).collect::<Vec<_>>(), &published)?;
        Ok(written)
    }
    const SIMILAR: [&str; 4] = ["image_sigs", "relations", "similar_paired", "similar_retry"];
    fn similar_library() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("app_create_schema.sql")).unwrap();
        // 身份放在核心表里（`terms`、`term_tree`），和每个图库一样。
        crate::migrate(&mut c).unwrap();
        c.execute_batch("CREATE TABLE IF NOT EXISTS similar_paired(uuid TEXT PRIMARY KEY);CREATE TABLE IF NOT EXISTS similar_retry(a TEXT NOT NULL,b TEXT NOT NULL,PRIMARY KEY(a,b));").unwrap();
        for i in 0..20 {
            c.execute("INSERT INTO assets(uuid,src_path) VALUES (?,'')", [format!("a{i:02}")]).unwrap();
        }
        for i in 0..18 {
            c.execute(
                "INSERT INTO image_sigs VALUES (?,?,?,1)",
                rusqlite::params![format!("a{i:02}"), 1.0 + f64::from(i) / 8.0, vec![i as u8; 256]],
            )
            .unwrap();
            c.execute("INSERT INTO similar_paired VALUES (?)", [format!("a{i:02}")]).unwrap();
        }
        c.execute_batch(
            "INSERT INTO image_sigs VALUES ('a18',NULL,NULL,0);
             INSERT INTO relations VALUES ('a00','a01','duplicate',0,'','2026-10-01 00:00:00'),('a02','a03','similar',NULL,'','2026-10-01 00:00:00'),
               ('gone-a','gone-b','duplicate',0,'historic','2000-01-01 00:00:00');
             INSERT INTO similar_retry VALUES ('a04','a05');",
        )
        .unwrap();
        c
    }
    fn copy(c: &Connection) -> Connection {
        let mut out = Connection::open_in_memory().unwrap();
        rusqlite::backup::Backup::new(c, &mut out).unwrap().run_to_completion(256, Duration::ZERO, None).unwrap();
        out
    }
    fn cache_rows(c: &Connection, table: &str, order: &str) -> Vec<Vec<crate::table_delta::Cell>> {
        let mut q = c.prepare(&format!("SELECT * FROM {table}{order}")).unwrap();
        let n = q.column_count();
        q.query_map([], |r| {
            (0..n).map(|i| r.get_ref(i).map(crate::table_delta::Cell::from)).collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    }
    fn count_writes(c: &Connection) {
        c.execute_batch("CREATE TEMP TABLE writes(tbl TEXT,op TEXT);").unwrap();
        for table in SIMILAR {
            for op in ["INSERT", "UPDATE", "DELETE"] {
                c.execute_batch(&format!("CREATE TEMP TRIGGER w_{table}_{op} AFTER {op} ON main.{table} BEGIN INSERT INTO writes VALUES ('{table}','{op}'); END;")).unwrap();
            }
        }
    }
    fn writes(c: &Connection) -> i64 {
        c.query_row("SELECT count(*) FROM temp.writes", [], |r| r.get(0)).unwrap()
    }
    type Change = fn(&mut [Vec<Value>; 4]);
    /// 写入的行数（完整结果，变更列表）。
    /// 相似运行对它读到的数组做的事（`similar::scan`）：被替换的行移到末尾。
    fn worker_changes() -> Vec<(&'static str, Change, [i64; 2])> {
        fn replace(rows: &mut Vec<Value>, row: Value) {
            rows.retain(|r| r[0] != row[0] || (r.as_array().unwrap().len() == 2 && r[1] != row[1]));
            rows.push(row);
        }
        fn grid(n: u8) -> Value {
            Value::String(format!("{n:02x}").repeat(256))
        }
        vec![
            ("unchanged", |_| {}, [0, 0]),
            ("one signature", |t| replace(&mut t[0], json!(["a05", 3.5, grid(5), 1])), [2, 2]),
            // 读不出的图片再试了一次；最后一行已存的行留在原处。
            ("failed again, last row", |t| replace(&mut t[0], json!(["a18", null, null, 0])), [0, 0]),
            (
                "failed again, earlier row",
                |t| {
                    replace(&mut t[0], json!(["a18", null, null, 0]));
                    replace(&mut t[0], json!(["a17", 3.0, grid(17), 1]));
                    replace(&mut t[0], json!(["a18", null, null, 0]));
                },
                // 完整结果原地改写 a17；变更列表则把它和 a18 再移动一次。
                [1, 4],
            ),
            (
                "new picture",
                |t| {
                    t[0].push(json!(["a19", 1.5, grid(19), 1]));
                    replace(&mut t[0], json!(["a18", null, null, 0]));
                    t[1].retain(|r| !(r[0] == "a00" && r[1] == "a01"));
                    t[1].push(json!(["a00", "a01", "identical", 0.0, "", "2026-10-12 00:00:00"]));
                    t[1].push(json!(["a00", "a19", "crop", 1.0, "", "2026-10-12 00:00:00"]));
                    t[2].push(json!(["a19"]));
                    t[3].retain(|r| r[0] != "a04");
                    t[3].push(json!(["a06", "a07"]));
                },
                [9, 9],
            ),
            (
                "every signature",
                |t| {
                    for i in 0..18u8 {
                        replace(&mut t[0], json!([format!("a{i:02}"), 2.0 + f64::from(i), grid(i), 1]));
                    }
                },
                [36, 36],
            ),
        ]
    }

    #[test]
    fn change_lists_and_full_results_publish_the_full_replacement_rows_writing_only_changes() {
        let f = Fixture::new();
        let base = similar_library();
        for (name, change, expected) in worker_changes() {
            let mut read: [Vec<Value>; 4] = SIMILAR.map(|t| crate::research_compute::similar::table(&base, t).unwrap());
            let before = read.clone();
            change(&mut read);
            let full: serde_json::Map<_, _> =
                SIMILAR.iter().zip(read.iter()).map(|(t, rows)| (t.to_string(), Value::Array(rows.clone()))).collect();
            let delta: serde_json::Map<_, _> = SIMILAR
                .iter()
                .zip(read.iter().zip(&before))
                .map(|(t, (rows, old))| {
                    (t.to_string(), crate::research_compute::similar::changes(&base, t, old, rows.clone()).unwrap())
                })
                .collect();
            let mut legacy = copy(&base);
            let tx = legacy.transaction().unwrap();
            legacy_publish(&tx, &full, &f.ctx).unwrap();
            tx.commit().unwrap();
            for (mode, tables, expected) in [("full", &full, expected[0]), ("delta", &delta, expected[1])] {
                let mut c = copy(&base);
                count_writes(&c);
                let tx = c.transaction().unwrap();
                let written = publish(&tx, tables, &f.ctx).unwrap();
                tx.commit().unwrap();
                for table in SIMILAR {
                    for order in ["", " ORDER BY rowid"] {
                        assert_eq!(
                            cache_rows(&c, table, order),
                            cache_rows(&legacy, table, order),
                            "{name} {mode} {table}"
                        );
                    }
                }
                assert_eq!(writes(&c), written as i64, "{name} {mode}");
                assert_eq!(written as i64, expected, "{name} {mode}");
            }
            if name == "unchanged" {
                assert!(delta.values().all(|v| v["delete"] == json!([]) && v["append"] == json!([])));
            }
        }
    }

    #[test]
    fn written_rows_are_checked_like_the_full_replacement() {
        let f = Fixture::new();
        let base = similar_library();
        let historic = json!(["gone-a", "gone-b", "duplicate", 0.0, "historic", "2000-01-01 00:00:00"]);
        type Rows = fn(&mut Vec<Value>);
        let cases: [(&str, Rows, bool); 4] = [
            ("reversed pair", |r| r.push(json!(["a09", "a08", "similar", 0.0, "", "t"])), false),
            ("unknown picture", |r| r.push(json!(["a00", "zz", "similar", 0.0, "", "t"])), false),
            (
                "historic row rewritten",
                |r| {
                    r.iter_mut().for_each(|x| {
                        if x[0] == "gone-a" {
                            x[4] = json!("changed")
                        }
                    })
                },
                false,
            ),
            ("historic row kept", |_| {}, true),
        ];
        for (name, change, ok) in cases {
            let old = crate::research_compute::similar::table(&base, "relations").unwrap();
            assert!(old.contains(&historic));
            let mut rows = old.clone();
            change(&mut rows);
            let full = serde_json::Map::from_iter([("relations".to_string(), Value::Array(rows.clone()))]);
            let delta = serde_json::Map::from_iter([(
                "relations".to_string(),
                crate::research_compute::similar::changes(&base, "relations", &old, rows).unwrap(),
            )]);
            let mut legacy = copy(&base);
            let tx = legacy.transaction().unwrap();
            assert_eq!(legacy_publish(&tx, &full, &f.ctx).is_ok(), ok, "{name} legacy");
            for tables in [&full, &delta] {
                let mut c = copy(&base);
                let tx = c.transaction().unwrap();
                assert_eq!(publish(&tx, tables, &f.ctx).is_ok(), ok, "{name}");
            }
        }
        // 针对没存着的行、或形状不同的行的变更列表会被拒绝。
        for bad in [
            json!({"delete":[["a98","a99"]],"append":[]}),
            json!({"delete":[["a00"]],"append":[]}),
            json!({"delete":[],"append":[["a00","a01","duplicate",0,"","t"]]}),
            json!({"delete":[],"append":[],"extra":[]}),
            json!({"append":[]}),
        ] {
            let mut c = copy(&base);
            let tx = c.transaction().unwrap();
            let tables = serde_json::Map::from_iter([("relations".to_string(), bad.clone())]);
            assert!(publish(&tx, &tables, &f.ctx).is_err(), "{bad}");
        }
    }

    type BackupHook = dyn FnMut(&Path);
    thread_local! {
        /// 给本线程的指纹计算模拟一块慢磁盘：每读一行后的暂停。
        static ROW_PAUSE: std::cell::Cell<Duration> = const { std::cell::Cell::new(Duration::ZERO) };
        /// 本线程的指纹计算每读一行后运行：模拟另一个任务在那一刻做的事。
        static ROW_HOOK: std::cell::RefCell<Option<Box<dyn FnMut()>>> = const { std::cell::RefCell::new(None) };
        /// 本线程的快照复制每一步之前运行，参数是副本的路径。
        static BACKUP_HOOK: std::cell::RefCell<Option<Box<BackupHook>>> = const { std::cell::RefCell::new(None) };
    }
    pub(super) fn backup_step(path: &Path) {
        BACKUP_HOOK.with(|hook| {
            if let Some(hook) = hook.borrow_mut().as_mut() {
                hook(path);
            }
        });
    }

    /// #275：一个源库写入方（识别导入、文件夹或 Photos 扫描；同步以前的 250 毫秒 busy timeout）在运行为快照复制应用
    /// 数据库时提交。这时运行不得持有源库读锁：复制不在源库上执行任何语句，所以看不到写入方，复制时间一旦超过写入方的
    /// 等待时长，写入方就会以「database is locked」失败。它从复制的第一步起就在本线程里提交，所以研究是否持有这把锁
    /// 不取决于时机。
    #[test]
    fn copying_the_application_database_does_not_hold_source() {
        let f = Fixture::new();
        let path = f.root.join("source.sqlite");
        let outcomes = std::rc::Rc::new(std::cell::RefCell::new(Vec::<std::result::Result<(), String>>::new()));
        let seen = outcomes.clone();
        let commit = path.clone();
        BACKUP_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |copy: &Path| {
                if copy.ends_with("library.sqlite") {
                    let c = crate::sqlite::open(&commit).unwrap();
                    c.busy_timeout(Duration::from_millis(250)).unwrap();
                    let n = seen.borrow().len();
                    let done = c
                        .execute_batch(&format!(
                            "BEGIN IMMEDIATE; INSERT INTO characters VALUES ('synthetic-{n}',1); COMMIT;"
                        ))
                        .map_err(|e| e.to_string());
                    seen.borrow_mut().push(done);
                }
            }))
        });
        let dir = f.root.join("snapshot");
        fs::create_dir_all(&dir).unwrap();
        let (app, mut src) = (f.app(), f.source());
        let taken = snapshot(&app, &mut src, &Writers::of(&path), &f.inputs(Scope::Learning), "events", &dir, &f.ctx);
        BACKUP_HOOK.with(|hook| hook.borrow_mut().take());
        let outcomes = outcomes.borrow();
        assert!(!outcomes.is_empty());
        assert!(outcomes.iter().all(std::result::Result::is_ok), "{outcomes:?}");
        // 副本和版本一致：源库副本里有应用库复制期间提交的行。
        let taken = taken.unwrap();
        assert_eq!(taken.version, f.version(Scope::Learning));
        let copied: i64 = crate::sqlite::open(dir.join("source.sqlite"))
            .unwrap()
            .query_row("SELECT count(*) FROM characters WHERE char_id LIKE 'synthetic-%'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(copied, outcomes.len() as i64);
    }
    pub(super) fn pause_row() {
        let pause = ROW_PAUSE.with(std::cell::Cell::get);
        if !pause.is_zero() {
            std::thread::sleep(pause);
        }
        ROW_HOOK.with(|hook| {
            if let Some(hook) = hook.borrow_mut().as_mut() {
                hook();
            }
        });
    }

    /// #275：学习结果在发布前检查期间，另一个任务持续向应用数据库提交（名称更新把每次查询标为完成，没有研究输入读它）。
    /// 每计算 50 行的哈希，它就用同步以前的写入设置（250 毫秒 busy timeout）提交一次。检查持有写入锁时绝不能计算
    /// 输入的哈希：在大图库上这曾让写入方以「database is locked」失败。
    #[test]
    fn steady_unread_writer_is_not_locked_out_by_the_publication_check() {
        let f = Fixture::new();
        f.app()
            .execute_batch(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<200)
                 INSERT INTO assertions SELECT 'a','synthetic-'||i,'confirmed' FROM n;",
            )
            .unwrap();
        let version = f.version(Scope::Learning);
        let path = f.root.join("library.sqlite");
        let outcomes = std::rc::Rc::new(std::cell::RefCell::new(Vec::<std::result::Result<(), String>>::new()));
        let (seen, mut rows) = (outcomes.clone(), 0);
        ROW_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                rows += 1;
                if rows % 50 == 0 {
                    let c = crate::sqlite::open(&path).unwrap();
                    c.busy_timeout(Duration::from_millis(250)).unwrap();
                    let done = c
                        .execute_batch(&format!(
                            "BEGIN IMMEDIATE; INSERT OR REPLACE INTO name_cache VALUES ('synthetic-done','{rows}'); COMMIT;"
                        ))
                        .map_err(|e| e.to_string());
                    seen.borrow_mut().push(done);
                }
            }))
        });
        let (result, rounds) = f.confirm(Scope::Learning, &version, |_| {});
        ROW_HOOK.with(|hook| hook.borrow_mut().take());
        let outcomes = outcomes.borrow();
        assert!(outcomes.len() >= OUTSIDE_ROUNDS as usize, "{outcomes:?}");
        assert!(outcomes.iter().all(std::result::Result::is_ok), "{outcomes:?}");
        // 在检查和取锁之间输入始终没稳定：什么都不发布，运行被告知等待。
        assert!(matches!(&result, Err(e) if busy(e)), "{result:?}");
        assert_eq!(rounds, OUTSIDE_ROUNDS);
        assert!(!f.published());
        // 写入方一停，同样的检查在第一轮就发布。
        let (result, rounds) = f.confirm(Scope::Learning, &version, |_| {});
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(rounds, 1);
        assert!(f.published());
    }

    /// #226：研究读源库期间，一个用同步的 250 毫秒 busy timeout 的源库写入方提交，运行取快照时和发布前检查结果时
    /// 都是如此。如果不让路，读取方在整个读取期间（这里约两秒）都持有锁，提交就会以「database is locked」失败。
    #[test]
    fn source_reads_step_aside_for_a_writer() {
        let f = Fixture::new();
        f.source()
            .execute_batch(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<2000)
                 INSERT INTO embeddings SELECT i,'ccip',randomblob(64) FROM n;",
            )
            .unwrap();
        let path = f.root.join("source.sqlite");
        // 等下面的读取进行到一定程度后提交；返回提交的结果。
        let writer = |key: &'static str| {
            let (started, wait) = mpsc::channel::<()>();
            let path = path.clone();
            let handle = std::thread::spawn(move || {
                wait.recv().unwrap();
                std::thread::sleep(Duration::from_millis(200));
                let c = crate::sqlite::open(&path).unwrap();
                c.busy_timeout(Duration::from_millis(250)).unwrap();
                c.execute_batch(&format!("BEGIN IMMEDIATE; INSERT INTO characters VALUES ('{key}',1); COMMIT;"))
            });
            (started, handle)
        };
        ROW_PAUSE.with(|p| p.set(Duration::from_millis(1)));

        let (started, handle) = writer("synthetic-a");
        let (app, mut src) = (f.app(), f.source());
        let writers = Writers::of(&path);
        started.send(()).unwrap();
        let read = writers.read(&mut src, |src| {
            read_snapshot(&[&app, src], || {
                fingerprint(&app, src, &f.inputs(Scope::Learning), &f.ctx)?;
                copy_inputs(
                    src,
                    &f.root.join("snapshot.sqlite"),
                    &f.inputs(Scope::Learning),
                    SnapshotDb::Source,
                    &poll(&f.ctx, Some(&writers)),
                )
            })
        });
        assert_eq!(handle.join().unwrap().map_err(|e| e.to_string()), Ok(()));
        assert!(matches!(&read, Err(e) if yielded(e)), "{read:?}");

        ROW_PAUSE.with(|p| p.set(Duration::ZERO));
        let version = f.version(Scope::Learning);
        ROW_PAUSE.with(|p| p.set(Duration::from_millis(1)));
        let (started, handle) = writer("synthetic-b");
        let (mut app, mut src) = (f.app(), f.source());
        let writers = Writers::of(&path);
        started.send(()).unwrap();
        // 和 `run` 里一样，源库事务随读取它的发布步骤一起结束。
        let checked = writers.read(&mut src, |src| {
            let src = src.transaction()?;
            confirm(
                &mut app,
                &src,
                &f.inputs(Scope::Learning),
                &version,
                &f.ctx,
                Some(&writers),
                None,
                || {},
                |tx| Ok(tx.commit()?),
            )
        });
        assert_eq!(handle.join().unwrap().map_err(|e| e.to_string()), Ok(()));
        assert!(matches!(&checked, Err(e) if yielded(e)), "{checked:?}");

        // 已经有写入方在工作：读取不会开始。
        ROW_PAUSE.with(|p| p.set(Duration::ZERO));
        let c = crate::sqlite::open(&path).unwrap();
        c.execute_batch("BEGIN IMMEDIATE; INSERT INTO characters VALUES ('synthetic-c',1);").unwrap();
        let mut src = f.source();
        assert!(matches!(Writers::of(&path).read(&mut src, |_| Ok(())), Err(e) if yielded(&e)));
        c.execute_batch("COMMIT").unwrap();
        assert!(Writers::of(&path).read(&mut src, |_| Ok(())).is_ok());
    }

    #[test]
    fn only_old_task_snapshots_are_swept() {
        let root = std::env::temp_dir().join(format!("illustash-sweep-{}", crate::store::new_key().unwrap()));
        let task = |n: u8| root.join(format!("illustash-research-{}", format!("{n:02x}").repeat(16)));
        for n in 0..2 {
            fs::create_dir_all(task(n)).unwrap();
            fs::write(task(n).join("library.sqlite"), "synthetic").unwrap();
        }
        fs::create_dir_all(root.join("illustash-research-scope-other")).unwrap();
        fs::create_dir_all(root.join("unrelated")).unwrap();
        // 没有哪个比一小时更旧。
        assert_eq!(sweep_scratch(&root, Duration::from_secs(3600)), 0);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(sweep_scratch(&root, Duration::from_millis(10)), 2);
        assert!(!task(0).exists() && !task(1).exists());
        assert!(root.join("illustash-research-scope-other").exists() && root.join("unrelated").exists());
        fs::remove_dir_all(root).unwrap();
    }

    /// #31 测量用的合成图库：n 张图片，带签名和配对标记，n-1 条关系连成一串，没有重试。
    fn bench_library(base: &Path, n: usize) {
        let c = crate::sqlite::open(base).unwrap();
        c.execute_batch(include_str!("app_create_schema.sql")).unwrap();
        c.execute_batch("CREATE TABLE IF NOT EXISTS similar_paired(uuid TEXT PRIMARY KEY);CREATE TABLE IF NOT EXISTS similar_retry(a TEXT NOT NULL,b TEXT NOT NULL,PRIMARY KEY(a,b));BEGIN;").unwrap();
        for i in 0..n {
            let id = format!("{i:08x}-synthetic");
            c.execute("INSERT INTO assets(uuid,src_path) VALUES (?,'')", [&id]).unwrap();
            let grid: Vec<u8> = (0..256).map(|j| ((i * 31 + j * 7) % 256) as u8).collect();
            c.execute("INSERT INTO image_sigs VALUES (?,?,?,1)", rusqlite::params![id, 1.25, grid]).unwrap();
            c.execute("INSERT INTO similar_paired VALUES (?)", [&id]).unwrap();
            if i > 0 {
                c.execute(
                    "INSERT INTO relations VALUES (?,?,'similar',0,'','2026-10-12 00:00:00')",
                    [format!("{:08x}-synthetic", i - 1), id],
                )
                .unwrap();
            }
        }
        c.execute_batch("COMMIT;VACUUM;").unwrap();
    }
    type BenchMode = fn(&mut [Vec<Value>; 4], &dyn Fn(usize) -> Value);
    /// 不改变；替换一个签名（像计算器那样移到末尾）；替换每一个签名。
    fn bench_modes() -> [(&'static str, BenchMode); 3] {
        [
            ("nochange", |_, _| {}),
            ("single", |t, grid| {
                let row = t[0].remove(t[0].len() / 2);
                t[0].push(json!([row[0], 1.5, grid(7), 1]));
            }),
            ("full", |t, grid| {
                let rows = std::mem::take(&mut t[0]);
                t[0] = rows.into_iter().enumerate().map(|(i, r)| json!([r[0], 1.5, grid(i), 1])).collect();
            }),
        ]
    }
    /// 本进程到目前为止的峰值常驻内存，单位字节（macOS 报告字节，Linux 报告 KiB）。`getrusage` 只有 POSIX 有，
    /// 所以这项测量和用它的测试不在 Windows 上构建。
    #[cfg(unix)]
    fn peak_rss() -> u64 {
        // SAFETY: libc::rusage 只含整数和 timeval，全零是合法值。
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: `usage` 是本地结构，getrusage 只向其中写入。
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
        let peak = usage.ru_maxrss as u64;
        if cfg!(target_os = "macos") {
            peak
        } else {
            peak * 1024
        }
    }
    /// #31 所有者内存（合成数据，release；仅 POSIX），每个进程一个结果：先用
    /// `ILLUSTASH_RESEARCH_RSS_DIR=<dir> cargo test --release research_publish_rss -- --ignored --nocapture` 写出
    /// 图库和响应，然后每次带 `ILLUSTASH_RESEARCH_RSS_RUN=<n>-<mode>-<main|delta>` 运行，像 App 那样解析一份响应并
    /// 发布，报告发布前后的进程峰值。
    #[cfg(unix)]
    #[test]
    #[ignore]
    fn research_publish_rss() {
        let Some(dir) = std::env::var_os("ILLUSTASH_RESEARCH_RSS_DIR").map(PathBuf::from) else {
            return;
        };
        let f = Fixture::new();
        let Ok(run) = std::env::var("ILLUSTASH_RESEARCH_RSS_RUN") else {
            let sizes = std::env::var("ILLUSTASH_RESEARCH_BENCH_SIZES").unwrap_or_else(|_| "1000,10000,50000".into());
            for n in sizes.split(',').map(|s| s.trim().parse::<usize>().unwrap()) {
                let base = dir.join(format!("{n}.sqlite"));
                bench_library(&base, n);
                let snapshot = crate::sqlite::open(&base).unwrap();
                let old: [Vec<Value>; 4] =
                    SIMILAR.map(|t| crate::research_compute::similar::table(&snapshot, t).unwrap());
                let grid = |i: usize| Value::String(format!("{:02x}", i % 256).repeat(256));
                for (mode, change) in bench_modes() {
                    let mut new = old.clone();
                    change(&mut new, &grid);
                    let full: serde_json::Map<_, _> =
                        SIMILAR.iter().zip(&new).map(|(t, rows)| (t.to_string(), Value::Array(rows.clone()))).collect();
                    let delta: serde_json::Map<_, _> = SIMILAR
                        .iter()
                        .zip(new.iter().zip(&old))
                        .map(|(t, (rows, before))| {
                            let change = crate::research_compute::similar::changes(&snapshot, t, before, rows.clone());
                            (t.to_string(), change.unwrap())
                        })
                        .collect();
                    for (code, tables) in [("main", full), ("delta", delta)] {
                        let bytes =
                            serde_json::to_vec(&json!({"format":"illustash-research/1","tables":tables})).unwrap();
                        fs::write(dir.join(format!("{n}-{mode}-{code}.json")), bytes).unwrap();
                    }
                }
            }
            return;
        };
        let n = run.split('-').next().unwrap();
        let before = peak_rss();
        let bytes = fs::read(dir.join(format!("{run}.json"))).unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        let ordered = crate::http_json::parse_python(text).unwrap();
        let parsed = crate::strict_json::parse(text).unwrap();
        let path = dir.join(format!("{run}.sqlite"));
        fs::copy(dir.join(format!("{n}.sqlite")), &path).unwrap();
        let mut c = crate::sqlite::open(&path).unwrap();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
        let tables = parsed["tables"].as_object().unwrap();
        if run.ends_with("-main") {
            legacy_publish(&tx, tables, &f.ctx).unwrap();
        } else {
            publish(&tx, tables, &f.ctx).unwrap();
        }
        tx.commit().unwrap();
        drop((ordered, parsed, bytes));
        println!("{}", json!({"run": run, "peak_before": before, "peak_after": peak_rss()}));
        drop(c);
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::remove_file(dir.join(format!("{run}.sqlite{suffix}")));
        }
    }

    /// #31 测量（合成数据，release）：`cargo test --release research_delta_bench -- --ignored --nocapture`。
    /// 只测四张相似表的发布（early_groups/fold 和指纹两边相同，不计入）。ILLUSTASH_RESEARCH_BENCH_SIZES 覆盖默认的
    /// 1000,10000,50000；三次运行取中位数。
    #[test]
    #[ignore]
    fn research_delta_bench() {
        use std::time::Instant;
        let sizes = std::env::var("ILLUSTASH_RESEARCH_BENCH_SIZES").unwrap_or_else(|_| "1000,10000,50000".into());
        let f = Fixture::new();
        let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
        let median = |mut v: Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        for n in sizes.split(',').map(|s| s.trim().parse::<usize>().unwrap()) {
            let dir = f.root.join(format!("delta-{n}"));
            fs::create_dir_all(&dir).unwrap();
            let base = dir.join("base.sqlite");
            bench_library(&base, n);
            let snapshot = crate::sqlite::open(&base).unwrap();
            let old: [Vec<Value>; 4] = SIMILAR.map(|t| crate::research_compute::similar::table(&snapshot, t).unwrap());
            let grid = |i: usize| Value::String(format!("{:02x}", i % 256).repeat(256));
            for (mode, change) in bench_modes() {
                let mut new = old.clone();
                change(&mut new, &grid);
                let full = Value::Object(
                    SIMILAR.iter().zip(&new).map(|(t, rows)| (t.to_string(), Value::Array(rows.clone()))).collect(),
                );
                let mut delta_ms = vec![];
                let mut delta = Value::Null;
                for _ in 0..3 {
                    let started = Instant::now();
                    delta = Value::Object(
                        SIMILAR
                            .iter()
                            .zip(new.iter().zip(&old))
                            .map(|(t, (rows, before))| {
                                let change =
                                    crate::research_compute::similar::changes(&snapshot, t, before, rows.clone());
                                (t.to_string(), change.unwrap())
                            })
                            .collect(),
                    );
                    delta_ms.push(ms(started.elapsed()));
                }
                for (code, tables) in [("main", &full), ("#31", &delta)] {
                    let bytes = serde_json::to_vec(&json!({"format":"illustash-research/1","tables":tables})).unwrap();
                    let (mut parse, mut apply, mut wal, mut written, mut scratch) = (vec![], vec![], vec![], 0, 0);
                    for run in 0..3 {
                        let started = Instant::now();
                        let text = std::str::from_utf8(&bytes).unwrap();
                        crate::http_json::parse_python(text).unwrap();
                        let parsed = crate::strict_json::parse(text).unwrap();
                        parse.push(ms(started.elapsed()));
                        let path = dir.join(format!("{mode}-{run}.sqlite"));
                        fs::copy(&base, &path).unwrap();
                        let mut c = crate::sqlite::open(&path).unwrap();
                        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;").unwrap();
                        let tables = parsed["tables"].as_object().unwrap();
                        let started = Instant::now();
                        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
                        if code == "main" {
                            legacy_publish(&tx, tables, &f.ctx).unwrap();
                            written = old.iter().chain(&new).map(Vec::len).sum::<usize>();
                            scratch = old[0].len() + old[1].len();
                        } else {
                            written = publish(&tx, tables, &f.ctx).unwrap();
                            scratch = 0;
                        }
                        tx.commit().unwrap();
                        apply.push(ms(started.elapsed()));
                        wal.push(
                            fs::metadata(dir.join(format!("{mode}-{run}.sqlite-wal"))).map(|m| m.len()).unwrap_or(0)
                                as f64,
                        );
                        drop(c);
                        for suffix in ["", "-wal", "-shm"] {
                            let _ = fs::remove_file(dir.join(format!("{mode}-{run}.sqlite{suffix}")));
                        }
                    }
                    println!(
                        "{}",
                        json!({"n": n, "mode": mode, "code": code, "response_bytes": bytes.len(),
                            "worker_delta_ms": if code == "#31" { median(delta_ms.clone()) } else { 0.0 },
                            "parse_ms": median(parse), "publish_ms": median(apply), "rows_written": written,
                            "temp_rows": scratch, "wal_bytes": median(wal)})
                    );
                }
            }
        }
    }

    /// #29 之前 main 的指纹，原样复制，只是用 features 目录代替 `Library`：两个数据库里除研究队列外的每张表，
    /// 在调用方的锁内计算。
    fn main_fingerprint(
        app: &Connection,
        src: &Connection,
        features: &Path,
        anchors: &Path,
        ctx: &TaskContext,
    ) -> (String, [f64; 3]) {
        let started = std::time::Instant::now();
        let mut parts = [0.0; 3];
        let mut h = Sha256::new();
        hash_db(app, &mut h, None, &["backend_effects", "research_runs"], &|| ctx.check()).unwrap();
        hash_db(src, &mut h, None, &["backend_effects", "research_runs"], &|| ctx.check()).unwrap();
        parts[0] = started.elapsed().as_secs_f64() * 1000.0;
        let mut files = vec![
            features.join("full-manifest.csv"),
            features.join("full-features.npy"),
            anchors.join("char-anchors.npy"),
            anchors.join("char-anchors.txt"),
        ];
        files.sort();
        for p in files {
            h.update(p.to_string_lossy().as_bytes());
            if let Ok(bytes) = fs::read(&p) {
                h.update(&bytes);
            }
        }
        parts[1] = started.elapsed().as_secs_f64() * 1000.0 - parts[0];
        let mut q = app.prepare("SELECT src_path FROM assets ORDER BY uuid").unwrap();
        for path in q.query_map([], |r| r.get::<_, Option<String>>(0)).unwrap() {
            file_metadata(&mut h, Path::new(&path.unwrap().unwrap_or_default()));
        }
        parts[2] = started.elapsed().as_secs_f64() * 1000.0 - parts[0] - parts[1];
        (format!("{:x}", h.finalize()), parts)
    }

    /// #29 测量（合成数据，release）：
    /// `cargo test --release research_fingerprint_bench -- --ignored --nocapture`。
    /// ILLUSTASH_RESEARCH_BENCH_SIZES 覆盖默认的 1000,10000,50000。
    #[test]
    #[ignore]
    fn research_fingerprint_bench() {
        use std::time::Instant;
        let sizes = std::env::var("ILLUSTASH_RESEARCH_BENCH_SIZES").unwrap_or_else(|_| "1000,10000,50000".into());
        let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
        let median = |mut v: Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        for n in sizes.split(',').map(|s| s.trim().parse::<usize>().unwrap()) {
            let f = Fixture::new();
            let app = f.app();
            app.execute_batch("DELETE FROM assets; BEGIN;").unwrap();
            fs::create_dir_all(f.root.join("images")).unwrap();
            let grid = vec![7u8; 256];
            for i in 0..n {
                let id = format!("{i:08x}-synthetic");
                let path = f.root.join("images").join(format!("{i}.png"));
                fs::write(&path, [i as u8]).unwrap();
                app.execute("INSERT INTO assets VALUES (?,?)", [&id, path.to_str().unwrap()]).unwrap();
                app.execute("INSERT INTO image_sigs VALUES (?,?,?,1)", rusqlite::params![id, 1.25, grid]).unwrap();
                app.execute("INSERT INTO assertions VALUES (?,'e1','confirmed')", [&id]).unwrap();
                app.execute("INSERT INTO memberships VALUES (?,'e1')", [&id]).unwrap();
                app.execute("INSERT INTO general_hits VALUES (?,'blue_hair')", [&id]).unwrap();
                app.execute("INSERT INTO name_cache VALUES (?,'{}')", [&id]).unwrap();
                if i > 0 {
                    app.execute(
                        "INSERT INTO relations VALUES (?,?,'similar',0,'','2026-10-12')",
                        [format!("{:08x}-synthetic", i - 1), id.clone()],
                    )
                    .unwrap();
                }
            }
            app.execute_batch("COMMIT;").unwrap();
            let src = f.source();
            src.execute_batch("BEGIN").unwrap();
            for i in 0..n / 100 {
                src.execute(
                    "INSERT INTO embeddings VALUES (?,'ccip',?)",
                    rusqlite::params![format!("{i:08x}-synthetic"), vec![1u8; 3072]],
                )
                .unwrap();
            }
            src.execute_batch("COMMIT").unwrap();
            let manifest: String =
                std::iter::once("uuid\n".to_string()).chain((0..n).map(|i| format!("{i:08x}-synthetic\n"))).collect();
            fs::write(f.root.join("features/full-manifest.csv"), manifest).unwrap();
            fs::write(f.root.join("features/full-features.npy"), vec![3u8; n * 768 * 4]).unwrap();
            let features = f.root.join("features");
            let (mut before_prepare, mut before_lock, mut before_parts) = (vec![], vec![], vec![]);
            for _ in 0..3 {
                let started = Instant::now();
                let src = f.source();
                let mut c = f.app();
                let read = c.transaction().unwrap();
                before_parts.push(main_fingerprint(&read, &src, &features, &f.anchors, &f.ctx).1);
                read.finish().unwrap();
                before_prepare.push(ms(started.elapsed()));
                let started = Instant::now();
                let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
                main_fingerprint(&tx, &src, &features, &f.anchors, &f.ctx);
                tx.commit().unwrap();
                before_lock.push(ms(started.elapsed()));
            }
            println!(
                "{}",
                json!({"n": n, "code": "main", "kind": "all", "prepare_ms": median(before_prepare), "accept_outside_ms": 0.0, "accept_lock_ms": median(before_lock),
                    "prepare_parts_ms": {"tables": median(before_parts.iter().map(|p| p[0]).collect()), "shared_files": median(before_parts.iter().map(|p| p[1]).collect()), "images": median(before_parts.iter().map(|p| p[2]).collect())}})
            );
            for scope in ALL {
                let (mut prepare, mut outside, mut lock) = (vec![], vec![], vec![]);
                for _ in 0..3 {
                    let started = Instant::now();
                    let version = f.version(scope);
                    prepare.push(ms(started.elapsed()));
                    let mut app = f.app();
                    let mut src = f.source();
                    let src = src.transaction().unwrap();
                    let started = Instant::now();
                    if !scope.publishes() {
                        // 查询在一个读事务里检查，不取写入锁。
                        let read = rusqlite::Transaction::new_unchecked(&app, TransactionBehavior::Deferred).unwrap();
                        assert_eq!(fingerprint(&read, &src, &f.inputs(scope), &f.ctx).unwrap(), version);
                        outside.push(ms(started.elapsed()));
                        lock.push(0.0);
                        continue;
                    }
                    let locked = std::cell::Cell::new(started);
                    confirm(
                        &mut app,
                        &src,
                        &f.inputs(scope),
                        &version,
                        &f.ctx,
                        None,
                        None,
                        || locked.set(Instant::now()),
                        |tx| {
                            tx.commit()?;
                            Ok(())
                        },
                    )
                    .unwrap();
                    let done = Instant::now();
                    outside.push(ms(locked.get() - started));
                    lock.push(ms(done - locked.get()));
                }
                println!(
                    "{}",
                    json!({"n": n, "code": "#29", "kind": format!("{scope:?}"), "prepare_ms": median(prepare), "accept_outside_ms": median(outside), "accept_lock_ms": median(lock)})
                );
            }
        }
    }
}
