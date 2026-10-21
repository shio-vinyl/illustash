//! Optional calculation workers operate on snapshots. The native owner validates and publishes.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
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

pub fn configured() -> bool {
    std::env::var_os("ILLUSTASH_RESEARCH_WORKER").is_some_and(|p| !p.is_empty())
}
fn bad() -> Error {
    Error::Invalid("研究结果格式不正确".into())
}
// Publication-time conflicts: the snapshot was valid but newer input arrived. Only these retry.
const INPUT_CHANGED: &str = "研究输入已改变，请重试";
const LEARNING_CHANGED: &str = "学习输入已改变";
/// Automatic retries after an input conflict; the delay doubles from the base each time.
const CONFLICT_RETRIES: u32 = 5;
fn input_changed(e: &Error) -> bool {
    matches!(e, Error::Conflict(m) if m == INPUT_CHANGED || m == LEARNING_CHANGED)
}
/// A run that stepped aside for a source writer (`Writers`). Learning and similar runs end and recovery starts them
/// again once no sync runs; a query waits for the writer to finish and asks again, up to `CONFLICT_RETRIES` times.
const YIELDED: &str = "图库正在写入，研究稍后继续";
fn yielded(e: &Error) -> bool {
    matches!(e, Error::Conflict(m) if m == YIELDED)
}
/// How long the writer must have been gone before a run that stepped aside starts again.
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
/// A picture whose bytes changed under the same UUID is no longer described by the similar results computed
/// from its pixels: drop its signature, every relation it is part of and its pairing marks, so the next
/// similar run computes them again, and rebuild the groups from what remains. Unchanged pictures keep their
/// results. All of these are machine caches; human decisions are stored elsewhere and stay.
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
    // Created by the first similar run that publishes them.
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
/// Application tables that no research calculator, learning stamp or result check reads (#29):
/// preferences, name-source settings (model keys, traffic counters), name caches,
/// sync history and the research queues themselves. Writing them never invalidates a result. Every
/// other table, including one added later, stays an input of learning.
const UNREAD: &[&str] =
    &["backend_effects", "name_cache", "name_settings", "prefs", "research_runs", "sync_added", "sync_runs"];
/// `meta` rows that are not inputs: the names version counts writes to name tables (triggers).
const UNREAD_META: &str = "'names_version'";
/// What one research kind's result is computed from (#29, docs/ARCHITECTURE_DIRECTION.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    /// events/regroup: learning, clustering and (events with similar) the similar scan.
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
    /// Application tables the calculator reads; `None` means every table except `UNREAD`.
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
            // Only the requested assets' paths, hashed by `fingerprint_once`.
            Self::Relate => Some(&[]),
        }
    }
    /// Source tables the calculator reads; `None` means every table. CCIP vectors are the `embeddings` overlay.
    fn source(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Learning => None,
            Self::Similar | Self::Oc => Some(&["embeddings"]),
            Self::Relate => Some(&[]),
        }
    }
    /// Kinds that publish caches or learning; the others only answer a query.
    fn publishes(self) -> bool {
        matches!(self, Self::Learning | Self::Similar)
    }
}
/// The fingerprinted inputs of one run: the kind's scope plus the paths it reads outside the databases.
struct Inputs<'a> {
    scope: Scope,
    /// relate: the requested assets, sorted and unique.
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
    /// Shared files hashed by content: base features (all but relate) and anchors (learning only).
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
    /// Cheap evidence that the shared files are unchanged: index generation plus each file's size and
    /// modification time. Compared under the write lock instead of reading the files again.
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
/// Ordered typed rows of one table (or a missing-table marker).
fn hash_table(c: &Connection, h: &mut Sha256, table: &str, ctx: &TaskContext) -> Result<()> {
    feed(h, table.as_bytes());
    let exists: bool =
        c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)", [table], |r| r.get(0))?;
    if !exists {
        feed(h, b"absent");
        return Ok(());
    }
    let quoted = table.replace('"', "\"\"");
    let n = c.prepare(&format!("SELECT * FROM \"{quoted}\""))?.column_count();
    let order = (1..=n).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let filter = if table == "meta" { format!(" WHERE key NOT IN ({UNREAD_META})") } else { String::new() };
    let mut q = c.prepare(&format!("SELECT * FROM \"{quoted}\"{filter} ORDER BY {order}"))?;
    let mut rows = q.query([])?;
    while let Some(r) = rows.next()? {
        ctx.check()?;
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
/// `only`: the tables to hash; `None` hashes every table except `skip`.
fn hash_db(c: &Connection, h: &mut Sha256, only: Option<&[&str]>, skip: &[&str], ctx: &TaskContext) -> Result<()> {
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
        ctx.check()?;
        hash_table(c, h, &table, ctx)?;
    }
    Ok(())
}
fn fingerprint(app: &Connection, src: &Connection, inputs: &Inputs<'_>, ctx: &TaskContext) -> Result<String> {
    let source = source_digest(src, inputs, ctx)?;
    fingerprint_with(app, &source, inputs, ctx)
}
/// The source tables' part of a fingerprint, on its own: a publication check holds source still with its read
/// transaction, so it computes this once however many rounds it takes (#275).
fn source_digest(src: &Connection, inputs: &Inputs<'_>, ctx: &TaskContext) -> Result<Vec<u8>> {
    let mut h = Sha256::new();
    hash_db(src, &mut h, inputs.scope.source(), &["backend_effects", "research_runs"], ctx)?;
    Ok(h.finalize().to_vec())
}
/// Shared anchor files are read within one complete index publication (index_generation).
fn fingerprint_with(app: &Connection, source: &[u8], inputs: &Inputs<'_>, ctx: &TaskContext) -> Result<String> {
    if inputs.scope == Scope::Learning {
        crate::index_generation::stable(inputs.anchors, || fingerprint_once(app, source, inputs, ctx))
    } else {
        fingerprint_once(app, source, inputs, ctx)
    }
}
fn fingerprint_once(app: &Connection, source: &[u8], inputs: &Inputs<'_>, ctx: &TaskContext) -> Result<String> {
    let mut h = Sha256::new();
    feed(&mut h, format!("{:?}", inputs.scope).as_bytes());
    hash_db(app, &mut h, inputs.scope.app(), UNREAD, ctx)?;
    feed(&mut h, source);
    // Base feature/anchor files are inputs too. Changed vectors invalidate an in-flight result.
    for p in inputs.files()? {
        ctx.check()?;
        let name = p.to_string_lossy();
        h.update(name.as_bytes());
        if let Ok(mut f) = fs::File::open(p) {
            let mut bytes = [0; 65536];
            loop {
                ctx.check()?;
                let n = f.read(&mut bytes)?;
                if n == 0 {
                    break;
                }
                h.update(&bytes[..n]);
            }
        }
    }
    // Original images: every asset for learning/similar, the requested ones for relate; OC reads none.
    match inputs.scope {
        Scope::Learning | Scope::Similar => {
            let mut q = app.prepare("SELECT src_path FROM assets ORDER BY uuid")?;
            for path in q.query_map([], |r| r.get::<_, Option<String>>(0))? {
                ctx.check()?;
                file_metadata(&mut h, Path::new(&path?.unwrap_or_default()));
            }
        }
        Scope::Relate => {
            let ids = serde_json::to_string(&inputs.ids).map_err(|_| bad())?;
            let mut q = app.prepare(
                "SELECT uuid,src_path FROM assets WHERE uuid IN (SELECT value FROM json_each(?)) ORDER BY uuid",
            )?;
            for row in q.query_map([ids], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))? {
                ctx.check()?;
                let (uuid, path) = row?;
                let path = path.unwrap_or_default();
                feed(&mut h, uuid.as_bytes());
                feed(&mut h, path.as_bytes());
                file_metadata(&mut h, Path::new(&path));
            }
        }
        Scope::Oc => {}
    }
    Ok(format!("{:x}", h.finalize()))
}
/// Rounds of the accept check before a publication that keeps meeting other writers steps aside (#275).
const PUBLISH_ROUNDS: u32 = 5;
/// The pause after the first round that met a writer; it doubles each round.
const PUBLISH_PAUSE: Duration = Duration::from_millis(250);
/// A publication whose accept check kept meeting other writers' commits: it ends without an error and recovery
/// starts it again later (`Research::recover`).
const PUBLISH_BUSY: &str = "图库一直在写入，研究稍后继续";
/// How long recovery waits before starting a run that stepped aside, by why it did.
fn step_aside_wait(e: &Error) -> Option<Duration> {
    match e {
        Error::Conflict(m) if m == YIELDED => Some(STEP_ASIDE),
        Error::Conflict(m) if m == PUBLISH_BUSY => Some(Duration::from_secs(60)),
        _ => None,
    }
}
fn conflict_text(e: &Error) -> String {
    match e {
        Error::Conflict(m) => m.clone(),
        other => other.to_string(),
    }
}
/// Publications checking their inputs right now. Name fetching writes only tables no research result reads, but one
/// commit per entry, thousands in a row: each moves `data_version` and would keep a check from ever finding a quiet
/// moment. It holds its commits while this is non-zero (`hold_unread_writes`); the check is bounded, so the wait is too.
static CHECKING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct Checking;
impl Checking {
    fn hold() -> Self {
        CHECKING.fetch_add(1, Ordering::SeqCst);
        Self
    }
}
impl Drop for Checking {
    fn drop(&mut self) {
        CHECKING.fetch_sub(1, Ordering::SeqCst);
    }
}
/// For writers of unread tables only: waits while a research publication checks its inputs. `check` ends the wait
/// with its error (cancellation).
pub(crate) fn hold_unread_writes(check: impl Fn() -> Result<()>) -> Result<()> {
    while CHECKING.load(Ordering::SeqCst) > 0 {
        check()?;
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}
/// One consistent read of each connection: everything `read` does sees the state committed when it
/// first reads. Nothing is written, so each transaction ends with ROLLBACK.
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
/// source.sqlite keeps a rollback journal, so a read transaction on it holds a lock that makes its writers (sync,
/// recognition imports) give up after their busy timeout (#226). Research reads source for as long as hashing
/// and copying the whole file takes, which on a large library is far longer. Its source reads watch for the journal a
/// writer creates before it commits and step aside at once, like the background integrity check (#212).
struct Writers {
    journal: PathBuf,
    seen: Arc<AtomicBool>,
}
impl Writers {
    /// How often a read looks for the journal: well within the writers' busy timeout.
    const POLL: Duration = Duration::from_millis(20);
    fn of(source: &Path) -> Self {
        let mut journal = source.as_os_str().to_owned();
        journal.push("-journal");
        Self { journal: journal.into(), seen: Arc::default() }
    }
    fn writing(&self) -> bool {
        self.journal.exists()
    }
    /// Between steps that are not SQL statements (backup steps).
    fn check(&self) -> Result<()> {
        if self.seen.load(Ordering::SeqCst) || self.writing() {
            self.seen.store(true, Ordering::SeqCst);
            return Err(Error::Conflict(YIELDED.into()));
        }
        Ok(())
    }
    /// Runs `read` with `src` interrupted once a writer's journal appears; any error then is the yield.
    fn read<T>(&self, src: &Connection, read: impl FnOnce() -> Result<T>) -> Result<T> {
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
        );
        let result = read();
        src.progress_handler(0, None::<fn() -> bool>);
        match result {
            Err(_) if self.seen.load(Ordering::SeqCst) => Err(Error::Conflict(YIELDED.into())),
            result => result,
        }
    }
    /// Until no writer's journal has been seen for a moment; an error when the task stops meanwhile.
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
fn data_version(c: &Connection) -> Result<i64> {
    Ok(c.query_row("PRAGMA data_version", [], |r| r.get(0))?)
}
/// `data_version` inside a read transaction that has taken its lock: the version of what the transaction reads.
fn read_version(c: &Connection) -> Result<i64> {
    c.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))?;
    data_version(c)
}
/// Accept-time input check for publishing kinds (#29). The fingerprint is recomputed in a read
/// transaction, without the write lock. Under BEGIN IMMEDIATE only this connection's
/// `PRAGMA data_version` (changed by any commit from another connection) and the shared files' stamp
/// are compared; if either moved, the lock is let go and the round repeated after a pause. The fingerprint is never
/// recomputed under the lock: on a large library that held every other writer of the application database past its
/// busy timeout (#275). After `PUBLISH_ROUNDS` the check gives up with `PUBLISH_BUSY`. A mismatch is always
/// `INPUT_CHANGED`. The source part is hashed in its own short read and again only when source's `data_version`
/// moved: an idle read transaction on source would hold its lock for the whole check, and with no statement running
/// it would never notice a writer to step aside for (#226). Source is read again, under the write lock, only to
/// confirm that version and for the publication itself. `between` runs before each lock (test hook).
fn confirm<T>(
    app: &mut Connection,
    src: &Connection,
    inputs: &Inputs<'_>,
    version: &str,
    ctx: &TaskContext,
    mut between: impl FnMut(),
    publish: impl FnOnce(rusqlite::Transaction<'_>) -> Result<T>,
) -> Result<T> {
    let _quiet = Checking::hold();
    let mut publish = Some(publish);
    let mut source: Option<(i64, Vec<u8>)> = None;
    for round in 1..=PUBLISH_ROUNDS {
        if source.as_ref().is_none_or(|(seen, _)| data_version(src).ok() != Some(*seen)) {
            source = Some(read_snapshot(&[src], || Ok((read_version(src)?, source_digest(src, inputs, ctx)?)))?);
        }
        let (source_seen, digest) = source.as_ref().expect("hashed above");
        let files = inputs.stamp()?;
        let reader: &Connection = app;
        let (fresh, seen) =
            read_snapshot(&[reader], || Ok((fingerprint_with(reader, digest, inputs, ctx)?, data_version(reader)?)))?;
        if fresh != version {
            return Err(Error::Conflict(INPUT_CHANGED.into()));
        }
        between();
        let tx = app.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if data_version(&tx)? == seen && inputs.stamp()? == files {
            let published = read_snapshot(&[src], || {
                if read_version(src)? != *source_seen {
                    return Ok(None);
                }
                publish.take().expect("published once")(tx).map(Some)
            })?;
            if let Some(value) = published {
                return Ok(value);
            }
        } else {
            drop(tx);
        }
        if round < PUBLISH_ROUNDS {
            let until = std::time::Instant::now() + PUBLISH_PAUSE * 2u32.pow(round - 1);
            while std::time::Instant::now() < until {
                ctx.check()?;
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
    Err(Error::Conflict(PUBLISH_BUSY.into()))
}
/// Snapshot directories `illustash-research-<task>` are removed when their run ends, whatever the outcome
/// (`Scratch`). Only a killed process leaves one; such a directory is removed once it is older than a day and
/// than twice the task deadline, so a running task never loses its own (#31).
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
fn backup(c: &Connection, path: &Path, ctx: &TaskContext, writers: Option<&Writers>) -> Result<()> {
    let mut out = crate::sqlite::open(path)?;
    let copy = rusqlite::backup::Backup::new(c, &mut out)?;
    loop {
        ctx.check()?;
        if let Some(writers) = writers {
            writers.check()?;
        }
        match copy.step(256)? {
            rusqlite::backup::StepResult::Done => break,
            rusqlite::backup::StepResult::More => {}
            _ => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    Ok(())
}
fn queue_schema(c: &Connection) -> Result<()> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS research_runs(id TEXT PRIMARY KEY,kind TEXT NOT NULL,status TEXT NOT NULL,error TEXT NOT NULL DEFAULT '',created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);")?;
    Ok(())
}
/// Recognition acknowledges a durable scheduling intent; computation remains separately pending.
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
    /// Set when automatic conflict retries ran out; recovery then waits for an explicit start.
    exhausted: Arc<AtomicBool>,
    /// Milliseconds recovery waits when the last run stepped aside for other writers (`step_aside_wait`); 0 when it
    /// did not.
    stepped_aside: Arc<std::sync::atomic::AtomicU64>,
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
            stepped_aside: Arc::default(),
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
            vec!["status", "done", "total", "found", "message"]
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
        } else if let Some(v) = raw.get("message") {
            value.set("message", v.clone());
        }
        value
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
        self.stepped_aside.store(0, Ordering::SeqCst);
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
                // Each attempt takes a fresh snapshot; a stale result was rejected and never published.
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
                        let until = std::time::Instant::now() + retry_base() * 2u32.pow(attempt - 1);
                        let waited = loop {
                            if let Err(e) = ctx.check() {
                                break Err(e);
                            }
                            if std::time::Instant::now() >= until {
                                break Ok(());
                            }
                            std::thread::sleep(Duration::from_millis(50));
                        };
                        if let Err(e) = waited {
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
                        public.set("at", chrono::Local::now().format("%H:%M:%S").to_string().into());
                        public.extend(v.clone());
                        public.set("learn_at", chrono::Local::now().format("%H:%M:%S").to_string().into());
                        public.set("status", "idle".into());
                    }
                    *state.lock().expect("research state mutex poisoned") = public.into();
                    if let Some(tx) = reply {
                        let _ = tx.send(Ok(v));
                    }
                    Ok(())
                }
                Err(e) if step_aside_wait(&e).is_some() => {
                    // Not a failure: the pending work stays as it is, without an error, for recovery to start again.
                    let wait = step_aside_wait(&e).expect("guarded by the match arm");
                    stepped_aside.store(wait.as_millis() as u64, Ordering::SeqCst);
                    *state.lock().expect("research state mutex poisoned") =
                        Json::object([("status", "waiting".into()), ("message", conflict_text(&e).into())]).into();
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
                    *state.lock().expect("research state mutex poisoned") =
                        Json::object([("status", "error".into()), ("message", e.to_string().into())]).into();
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
    /// Start pending learning/similarity work. Not while a sync runs: its batch imports leave
    /// `imported` events that one run after the sync coalesces, instead of learning competing
    /// with recognition for the CPU between batches. A run that stepped aside for a source writer
    /// (#226) starts again shortly after, one whose publication kept meeting other writers (#275) a minute
    /// after, on the same terms.
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
                // Ongoing writes outlasted the retries: keep the error until an explicit start or reopen.
                self.paused = true;
                return Ok(());
            }
            let wait = match self.stepped_aside.swap(0, Ordering::SeqCst) {
                0 => Duration::from_secs(60),
                ms => Duration::from_millis(ms),
            };
            self.retry_at = Some(std::time::Instant::now() + wait);
            return Ok(());
        }
        let c = crate::sqlite::open(library.db())?;
        crate::configure(&c)?;
        queue_schema(&c)?;
        // An interrupted run has no receipt: its original events/intent remain pending.
        c.execute("UPDATE research_runs SET status='pending' WHERE status='running'", [])?;
        let pending:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM backend_effects WHERE status='pending' AND kind IN ('verdict','imported','index_changed')) OR EXISTS(SELECT 1 FROM research_runs WHERE status='pending')",[],|r|r.get(0))?;
        if pending {
            self.start(library, "events", json!({"similar":self.similar_enabled}), None)?;
        }
        Ok(())
    }
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
    let c = crate::app_library::read_connection(&library.db())?;
    let src = crate::app_library::read_connection(&library.source())?;
    let inputs = Inputs::new(library, anchors, kind, &args);
    let writers = Writers::of(&library.source());
    // Snapshot/version from the same read transactions. Copying the application database is no statement on source,
    // so its steps look for a source writer too; the work on the copies happens after source is let go (#275).
    let (version, events, intents, regroup) = writers.read(&src, || read_snapshot(&[&c, &src], || {
        let version = fingerprint(&c, &src, &inputs, ctx)?;
        backup(&c, &scratch.0.join("library.sqlite"), ctx, Some(&writers))?;
        backup(&src, &scratch.0.join("source.sqlite"), ctx, Some(&writers))?;
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
        Ok((version, events, intents, regroup))
    }))?;
    crate::sqlite::open(scratch.0.join("library.sqlite"))?
        .execute("DELETE FROM name_settings WHERE key IN ('model_key','deepseek_key')", [])?;
    // Only learning kinds publish a learning batch; the others skip its full input stamp.
    let learning = if inputs.scope == Scope::Learning {
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
    // A separate observer is unnecessary: progress is a bounded file polled by the process API callback.
    let mut cmd = Command::new(worker);
    cmd.env(crate::app_tasks::WORKER_KIND_ENV, "research")
        .arg(&input)
        .arg(&output)
        .env("ILLUSTASH_RESEARCH_PROGRESS", &progress);
    let mut last = std::time::Instant::now();
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
    *state.lock().expect("research state mutex poisoned") = json!({"status":"importing"}).into();
    ctx.receive(&response.identity, || {
        let src = crate::app_library::read_connection(&library.source())?;
        writers.read(&src, || {
            if !inputs.scope.publishes() {
                // A query publishes nothing: one read transaction checks its inputs and its answer.
                let c = crate::app_library::read_connection(&library.db())?;
                return read_snapshot(&[&c, &src], || {
                    if fingerprint(&c, &src, &inputs, ctx)? != version {
                        return Err(Error::Conflict(INPUT_CHANGED.into()));
                    }
                    validate_query(&c, kind, &Value::Object(response.result.clone()), &args)?;
                    ordered.get("result").cloned().ok_or_else(bad)
                });
            }
            let mut c = crate::sqlite::open(library.db())?;
            crate::configure(&c)?;
            confirm(
                &mut c,
                &src,
                &inputs,
                &version,
                ctx,
                || {},
                |tx| {
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
                        for id in events {
                            tx.execute("UPDATE backend_effects SET status='done',error='' WHERE id=?", [id])?;
                        }
                    }
                    for id in intents {
                        tx.execute("UPDATE research_runs SET status='done',error='' WHERE id=?", [id])?;
                    }
                    ctx.check()?;
                    crate::schema::commit_writing(tx)?;
                    ordered.get("result").cloned().ok_or_else(bad)
                },
            )
        })
    })
}
/// What a publication wrote to relations and signatures (#31): rowids of the rows inserted or rewritten, and
/// the previous version of each row deleted or rewritten, for `validate_references`.
#[derive(Default)]
struct Published {
    touched: std::collections::HashMap<&'static str, Vec<i64>>,
    old: std::collections::HashMap<&'static str, std::collections::HashSet<Vec<crate::table_delta::Cell>>>,
}
/// One cache table of a result (#31). A full array (an older worker, clusters) is compared with the stored
/// rows; a change list `{delete, append}` from the worker is applied to the rows the snapshot held, which the
/// accepted fingerprint guarantees are still stored. Either way only differing rows are written, and the table
/// ends as the full replacement left it: the same rows in the same rowid order.
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
/// One validated result row as the values the full replacement inserted.
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
    // Relations and signatures (#31): only the rows this publication wrote are checked; every other row is the
    // stored one. A written row naming a picture no longer in the library is allowed only as the identical
    // previous row (history the calculator kept).
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
    //! #29: synthetic databases and files only.
    use super::*;
    use crate::app_tasks::{Task, TaskIdentity};
    use std::collections::BTreeSet;

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
                 INSERT INTO meta VALUES ('names_version','0'),('identity_policy','hybrid');
                 CREATE TABLE assets(uuid TEXT PRIMARY KEY,src_path TEXT);
                 CREATE TABLE prefs(key TEXT PRIMARY KEY,value TEXT NOT NULL);
                 CREATE TABLE name_cache(key TEXT PRIMARY KEY,value TEXT NOT NULL);
                 CREATE TABLE name_settings(key TEXT PRIMARY KEY,value TEXT NOT NULL);
                 CREATE TABLE term_names(term_id INTEGER,value TEXT);
                 CREATE TABLE term_key_names(kind TEXT,key TEXT,value TEXT);
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
        /// Run the accept check for `scope`; returns whether it published and how many rounds ran.
        fn confirm(&self, scope: Scope, version: &str, mut between: impl FnMut(u32)) -> (Result<()>, u32) {
            let mut app = self.app();
            let src = self.source();
            let mut rounds = 0;
            let result = confirm(
                &mut app,
                &src,
                &self.inputs(scope),
                version,
                &self.ctx,
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
    fn each_kind_fingerprints_only_its_inputs() {
        let f = Fixture::new();
        let app = |sql: &str| f.app().execute_batch(sql).unwrap();
        let src = |sql: &str| f.source().execute_batch(sql).unwrap();
        type Change<'a> = Box<dyn Fn() + 'a>;
        use Scope::*;
        let changes: Vec<(&str, Change, &[Scope])> = vec![
            ("prefs", Box::new(|| app("INSERT INTO prefs VALUES ('synthetic_view','grid')")), &[]),
            ("name cache", Box::new(|| app("INSERT INTO name_cache VALUES ('synthetic','{}')")), &[]),
            ("name traffic", Box::new(|| app("INSERT INTO name_settings VALUES ('traffic_reserved','42')")), &[]),
            ("model key", Box::new(|| app("INSERT INTO name_settings VALUES ('model_key','synthetic')")), &[]),
            // Names live in the core tables since core v8 (#201); core name tables are learning inputs, as the mirror
            // made them before. The names version itself is not an input (`UNREAD_META`).
            ("names + names_version", Box::new(|| app("INSERT INTO term_names VALUES (1,'合成')")), &[Learning]),
            (
                "display names",
                Box::new(|| app("INSERT INTO term_key_names VALUES ('character','e1','合成')")),
                &[Learning],
            ),
            ("sync history", Box::new(|| app("INSERT INTO sync_runs(status) VALUES ('done')")), &[]),
            ("pending event", Box::new(|| app("INSERT INTO backend_effects(kind) VALUES ('verdict')")), &[]),
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
            // The commit moved data_version, so a second round confirmed the unchanged inputs.
            assert!(result.is_ok(), "{scope:?}: {result:?}");
            assert_eq!(rounds, 2, "{scope:?}");
            assert!(f.published());
            f.app().execute("DELETE FROM image_sigs WHERE uuid='published'", []).unwrap();
        }
        // Nothing in between: one round, and the lock only compares data_version and file stamps.
        let version = f.version(Scope::Learning);
        assert_eq!(f.confirm(Scope::Learning, &version, |_| {}).1, 1);
    }

    #[test]
    fn relevant_commit_between_check_and_lock_is_rejected() {
        let f = Fixture::new();
        // Committed after the outside check, before BEGIN IMMEDIATE: caught by data_version.
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
        // A shared file replaced in the same window: caught by the file stamp.
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
    fn commits_before_every_lock_end_in_stepping_aside() {
        let f = Fixture::new();
        // Unread commits before every lock: no round finds a quiet moment, and none recomputes under the lock (#275).
        let version = f.version(Scope::Learning);
        let (result, rounds) = f.confirm(Scope::Learning, &version, |round| {
            f.app().execute("INSERT OR REPLACE INTO prefs VALUES ('synthetic',?)", [round]).unwrap();
        });
        assert!(matches!(&result, Err(e) if step_aside_wait(e) == Some(Duration::from_secs(60))), "{result:?}");
        assert_eq!(rounds, PUBLISH_ROUNDS);
        assert!(!f.published());
        // A real input committed before a lock is found by the next round's check outside the lock.
        let (result, rounds) = f.confirm(Scope::Learning, &version, |round| {
            let sql = if round == 2 {
                "INSERT INTO memberships VALUES ('c','e1')"
            } else {
                "INSERT OR REPLACE INTO prefs VALUES ('synthetic','x')"
            };
            f.app().execute(sql, []).unwrap();
        });
        assert!(conflict(&result), "{result:?}");
        assert_eq!(rounds, 2);
        assert!(!f.published());
    }

    /// #275: between the rounds of a publication check source is not held. A source writer with the former 250 ms busy
    /// timeout commits in the pause after the first round; its table is one no fingerprint reads, so the check goes on.
    #[test]
    fn source_writers_commit_between_publication_rounds() {
        let f = Fixture::new();
        f.source().execute_batch("CREATE TABLE backend_effects(id INTEGER PRIMARY KEY,kind TEXT)").unwrap();
        let version = f.version(Scope::Learning);
        let path = f.root.join("source.sqlite");
        let (result, rounds) = f.confirm(Scope::Learning, &version, |round| {
            f.app().execute("INSERT OR REPLACE INTO prefs VALUES ('synthetic',?)", [round]).unwrap();
            if round == 1 {
                let path = path.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(100));
                    let c = crate::sqlite::open(&path).unwrap();
                    c.busy_timeout(Duration::from_millis(250)).unwrap();
                    c.execute("INSERT INTO backend_effects(kind) VALUES ('synthetic')", []).map(|_| ())
                })
                .join()
                .unwrap()
                .unwrap();
            }
        });
        assert!(matches!(&result, Err(e) if step_aside_wait(e).is_some()), "{result:?}");
        assert_eq!(rounds, PUBLISH_ROUNDS);
    }

    /// #275: other writers of the application database, with the sync's former 250 ms busy timeout, commit all through
    /// a publication check that keeps meeting them; the check reads a library that takes at least 0.3 s to hash.
    /// Before, its last round hashed the library under the write lock and those writers failed with "database is
    /// locked". A writer that holds its commits while a check runs (`hold_unread_writes`, name fetching) lets it publish.
    #[test]
    fn publication_checks_never_hold_the_write_lock_for_a_fingerprint() {
        let f = Fixture::new();
        f.app()
            .execute_batch(
                "CREATE TABLE synthetic_bulk(id INTEGER PRIMARY KEY,value BLOB);
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<300)
                 INSERT INTO synthetic_bulk SELECT i,randomblob(64) FROM n;",
            )
            .unwrap();
        let version = f.version(Scope::Learning);
        let path = f.root.join("library.sqlite");
        // Commits to an unread table every 20 ms until told to stop; returns the failed commits.
        let writer = |polite: bool| {
            let stop = Arc::new(AtomicBool::new(false));
            let (path, flag) = (path.clone(), stop.clone());
            let handle = std::thread::spawn(move || {
                let c = crate::sqlite::open(&path).unwrap();
                c.busy_timeout(Duration::from_millis(250)).unwrap();
                let mut failed = vec![];
                for i in 0.. {
                    if flag.load(Ordering::SeqCst) {
                        break;
                    }
                    if polite {
                        hold_unread_writes(|| Ok(())).unwrap();
                    }
                    if let Err(e) = c.execute("INSERT OR REPLACE INTO prefs VALUES ('synthetic_writer',?)", [i]) {
                        failed.push(e.to_string());
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                failed
            });
            (stop, handle)
        };
        ROW_PAUSE.with(|p| p.set(Duration::from_millis(1)));
        let (stop, handle) = writer(false);
        let (result, rounds) = f.confirm(Scope::Learning, &version, |_| {});
        stop.store(true, Ordering::SeqCst);
        assert_eq!(handle.join().unwrap(), Vec::<String>::new());
        assert!(matches!(&result, Err(e) if step_aside_wait(e).is_some()), "{result:?}");
        assert_eq!(rounds, PUBLISH_ROUNDS);
        assert!(!f.published());

        let (stop, handle) = writer(true);
        let (result, _) = f.confirm(Scope::Learning, &version, |_| {});
        stop.store(true, Ordering::SeqCst);
        ROW_PAUSE.with(|p| p.set(Duration::ZERO));
        assert_eq!(handle.join().unwrap(), Vec::<String>::new());
        assert!(result.is_ok(), "{result:?}");
        assert!(f.published());
    }

    /// The publication before #31: full temporary copies of relations and signatures, then every result table
    /// deleted and inserted again in result order, then the whole-table checks. The row conversion it used is
    /// `cache_row` (moved there unchanged).
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
    /// The current publication of the similar tables, as `run` does it.
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
        // The identities live in the core tables (`terms`, `term_tree`), as in every library.
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
    /// Rows written (full result, change list).
    /// What a similar run does to the arrays it read (`similar::scan`): replaced rows move to the end.
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
            // An unreadable picture tried again; the last stored row stays where it is.
            ("failed again, last row", |t| replace(&mut t[0], json!(["a18", null, null, 0])), [0, 0]),
            (
                "failed again, earlier row",
                |t| {
                    replace(&mut t[0], json!(["a18", null, null, 0]));
                    replace(&mut t[0], json!(["a17", 3.0, grid(17), 1]));
                    replace(&mut t[0], json!(["a18", null, null, 0]));
                },
                // A full result rewrites a17 in place; a change list moves it and a18 again.
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
        // A change list against rows that are not stored, or of another shape, is refused.
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

    thread_local! {
        /// A slow disk for this thread's fingerprints: the pause after each row read.
        static ROW_PAUSE: std::cell::Cell<Duration> = const { std::cell::Cell::new(Duration::ZERO) };
    }
    /// Spins rather than sleeps: at background priority macOS coalesces short sleeps into tens of milliseconds.
    pub(super) fn pause_row() {
        let pause = ROW_PAUSE.with(std::cell::Cell::get);
        let until = std::time::Instant::now() + pause;
        while std::time::Instant::now() < until {
            std::hint::spin_loop();
        }
    }

    /// #226: a source writer with the sync's 250 ms busy timeout commits while research reads source, both while a
    /// run takes its snapshot and while its result is checked before publication. Without stepping aside the reader
    /// holds its lock for the whole read (about two seconds here) and the commit fails with "database is locked".
    #[test]
    fn source_reads_step_aside_for_a_writer() {
        let f = Fixture::new();
        f.source()
            .execute_batch(
                "CREATE TABLE synthetic_bulk(id INTEGER PRIMARY KEY,value BLOB);
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<2000)
                 INSERT INTO synthetic_bulk SELECT i,randomblob(64) FROM n;",
            )
            .unwrap();
        let path = f.root.join("source.sqlite");
        // Commits once the read below is well under way; returns how the commit went.
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
        let (app, src) = (f.app(), f.source());
        let writers = Writers::of(&path);
        started.send(()).unwrap();
        let read = writers.read(&src, || {
            read_snapshot(&[&app, &src], || {
                fingerprint(&app, &src, &f.inputs(Scope::Learning), &f.ctx)?;
                backup(&src, &f.root.join("snapshot.sqlite"), &f.ctx, Some(&writers))
            })
        });
        assert_eq!(handle.join().unwrap().map_err(|e| e.to_string()), Ok(()));
        assert!(matches!(&read, Err(e) if yielded(e)), "{read:?}");

        ROW_PAUSE.with(|p| p.set(Duration::ZERO));
        let version = f.version(Scope::Learning);
        ROW_PAUSE.with(|p| p.set(Duration::from_millis(1)));
        let (started, handle) = writer("synthetic-b");
        let (mut app, src) = (f.app(), f.source());
        let writers = Writers::of(&path);
        started.send(()).unwrap();
        let checked = writers.read(&src, || {
            confirm(&mut app, &src, &f.inputs(Scope::Learning), &version, &f.ctx, || {}, |tx| Ok(tx.commit()?))
        });
        assert_eq!(handle.join().unwrap().map_err(|e| e.to_string()), Ok(()));
        assert!(matches!(&checked, Err(e) if yielded(e)), "{checked:?}");

        // A writer already at work: the read does not start.
        ROW_PAUSE.with(|p| p.set(Duration::ZERO));
        let c = crate::sqlite::open(&path).unwrap();
        c.execute_batch("BEGIN IMMEDIATE; INSERT INTO characters VALUES ('synthetic-c',1);").unwrap();
        let src = f.source();
        assert!(matches!(Writers::of(&path).read(&src, || Ok(())), Err(e) if yielded(&e)));
        c.execute_batch("COMMIT").unwrap();
        assert!(Writers::of(&path).read(&src, || Ok(())).is_ok());
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
        // Nothing is older than an hour.
        assert_eq!(sweep_scratch(&root, Duration::from_secs(3600)), 0);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(sweep_scratch(&root, Duration::from_millis(10)), 2);
        assert!(!task(0).exists() && !task(1).exists());
        assert!(root.join("illustash-research-scope-other").exists() && root.join("unrelated").exists());
        fs::remove_dir_all(root).unwrap();
    }

    /// Synthetic library of the #31 measurements: n pictures with signatures and pairing marks, a chain of
    /// n-1 relations, no retries.
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
    /// No change; one signature replaced (moved to the end, as the calculator does); every signature replaced.
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
    /// Peak resident memory of this process so far, in bytes (macOS reports bytes, Linux KiB). `getrusage` is
    /// POSIX only, so this measurement and the test using it are not built on Windows.
    #[cfg(unix)]
    fn peak_rss() -> u64 {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
        let peak = usage.ru_maxrss as u64;
        if cfg!(target_os = "macos") {
            peak
        } else {
            peak * 1024
        }
    }
    /// #31 owner memory (synthetic, release; POSIX only), one result per process: first
    /// `ILLUSTASH_RESEARCH_RSS_DIR=<dir> cargo test --release research_publish_rss -- --ignored --nocapture` writes
    /// the libraries and responses, then each run with `ILLUSTASH_RESEARCH_RSS_RUN=<n>-<mode>-<main|delta>` parses
    /// one response as App does and publishes it, reporting the process peak before and after.
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

    /// #31 measurement (synthetic, release): `cargo test --release research_delta_bench -- --ignored --nocapture`.
    /// Publication of the four similar tables only (early_groups/fold and the fingerprint are the same in both and
    /// left out). ILLUSTASH_RESEARCH_BENCH_SIZES overrides the default 1000,10000,50000; three runs, median.
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

    /// main's fingerprint before #29, copied unchanged except that the features directory replaces
    /// `Library`: every table of both databases except the research queues, inside the caller's lock.
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
        hash_db(app, &mut h, None, &["backend_effects", "research_runs"], ctx).unwrap();
        hash_db(src, &mut h, None, &["backend_effects", "research_runs"], ctx).unwrap();
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

    /// #29 measurement (synthetic, release): `cargo test --release research_fingerprint_bench -- --ignored --nocapture`.
    /// ILLUSTASH_RESEARCH_BENCH_SIZES overrides the default 1000,10000,50000.
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
                    let src = f.source();
                    let started = Instant::now();
                    if !scope.publishes() {
                        // Queries check in one read transaction and take no write lock.
                        let read = app.transaction().unwrap();
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
