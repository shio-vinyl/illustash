//! 已有的应用图库包。打开只做校验；创建、升级和切换都是显式的流程。
use crate::{http_json::Json, Error, Result};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

pub const FORMAT: i64 = 1;
pub const APPLICATION_SCHEMA: &str = "36";
const CORE_MARKERS: [&str; 4] = ["terms_artists", "terms_labels", "terms_characters", "terms_names"];

/// 打开进度：正在检查的数据库，以及两项完整性检查合计的完成比例。
/// 在打开图库的线程上频繁调用（每一百个 SQLite 步骤一次）；调用方自行节流输出。
pub type Progress = Arc<dyn Fn(&str, f64) + Send + Sync + std::panic::RefUnwindSafe>;

/// 路径在校验后不再改变。会话身份还必须包含一个新的会话 generation，因为图库包的副本可能共用同一个配置 ID。
/// 不查默认图库包。
///
/// 图库包拥有的每个文件都经打开时固定的规范路径访问：用户打开的别名之后改指别处，也不得把读写重定向到另一个图库包。
/// 保留别名只是为了显示和记住用户选的是什么。
#[derive(Clone, Debug)]
pub struct Library {
    display_path: PathBuf,
    instance_path: PathBuf,
    config: Value,
    ordered_config: Json,
}

impl Library {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_validated(path, false, Checks::Now(None), read_connection)
    }

    /// 同样的校验；几个 GiB 的图库包完整性检查要几十秒，所以报告进度。
    pub fn open_reporting(path: &Path, progress: Progress) -> Result<Self> {
        Self::open_validated(path, false, Checks::Now(Some(progress)), read_connection)
    }

    /// 同样的校验，但不做两项完整检查，给自己决定是否检查的会话使用（#212，`app_service::open_owned`）：版本、
    /// 核心标记和来源的基础 schema 仍然必须满足。
    pub(crate) fn open_deferring_checks(path: &Path) -> Result<Self> {
        Self::open_validated(path, false, Checks::Deferred, read_connection)
    }

    /// 校验刚解包的归档，接受需要显式升级的旧核心。只读检查使用归档专用连接。
    pub(crate) fn check_import(path: &Path) -> Result<()> {
        match Self::open_validated(path, false, Checks::Now(None), read_archive) {
            Ok(_) => Ok(()),
            Err(Error::Schema(_)) => Self::open_validated(path, true, Checks::Now(None), read_archive).map(|_| ()),
            Err(error) => Err(error),
        }
    }

    /// 不做两项完整性检查的 `open_for_upgrade`：要不要提供升级只根据版本、标记和 schema 决定，
    /// 升级自己在它的写锁下运行检查，只运行一次（#267）。
    pub(crate) fn open_for_upgrade_deferring_checks(path: &Path) -> Result<Self> {
        Self::open_validated(path, true, Checks::Deferred, read_connection)
    }

    fn open_validated(path: &Path, upgrade: bool, checks: Checks, open: ReadConnection) -> Result<Self> {
        let library = Self::locate(path)?;
        if let Checks::Now(progress) = checks {
            checked(library.inspect_using(progress, open)?)?;
        }
        let mut db = open(&library.db())?;
        // 一次读快照：并发的迁移不会给出混杂的版本和归属标记。
        let tx = db.transaction()?;
        validate_checked(&tx, upgrade)?;
        tx.commit()?;
        let source = open(&library.source())?;
        // 来源的布局没有独立的版本号。只要求既定的基础 schema；
        // 迁移后的读取用不到新的来源链接和出处预设，打开时也绝不能添加它们。
        source.prepare(
            "SELECT uuid,filename,src_path,width,height,added_at,src_kind,in_scope,trashed FROM assets LIMIT 0",
        )?;
        source.prepare("SELECT uuid,dim,value,conf FROM labels LIMIT 0")?;
        // provenance.sqlite 是可选的：没有时，现有的 HTTP 读取方给出空的证据。
        Ok(library)
    }

    /// 图库包和它的 library.json，不读任何一个数据库。
    pub(crate) fn locate(path: &Path) -> Result<Self> {
        if path.extension().and_then(|s| s.to_str()) != Some("pilib") {
            return Err(Error::Invalid("图库包的扩展名必须是 .pilib".into()));
        }
        // 显示路径保留显式的别名，与旧实现的 Path.absolute 一致，而不是 resolve。
        let path = if path.is_absolute() { path.to_owned() } else { std::env::current_dir()?.join(path) };
        let instance_path = crate::app_paths::canonical(&path)?;
        if !path.is_dir() {
            return Err(Error::Invalid("图库包必须是已有目录".into()));
        }
        let text = fs::read_to_string(path.join("library.json"))?;
        let config: Value =
            serde_json::from_str(&text).map_err(|e| Error::Invalid(format!("无效 library.json：{e}")))?;
        if !config.is_object() || config.get("format").and_then(Value::as_i64) != Some(FORMAT) {
            return Err(Error::Schema("图库包格式需要显式升级，当前程序只打开 format 1".into()));
        }
        for key in ["id", "name"] {
            if !config.get(key).is_some_and(Value::is_string) {
                return Err(Error::Invalid(format!("library.json 缺少字符串 {key}")));
            }
        }
        if config.get("sources").is_some_and(|v| {
            !v.is_array() || v.as_array().expect("validated internal JSON array").iter().any(|s| !s.is_object())
        }) {
            return Err(Error::Invalid("library.json 的 sources 必须是对象数组".into()));
        }
        let ordered_config =
            serde_json::from_str(&text).map_err(|e| Error::Invalid(format!("无效 library.json：{e}")))?;
        Ok(Self { display_path: path, instance_path, config, ordered_config })
    }

    /// 经规范路径再次校验同一个图库包，保留用户打开时的别名用于显示。
    /// 只有 Unix 符号链接测试使用它。
    #[cfg(all(test, unix))]
    pub(crate) fn reopen(&self) -> Result<Self> {
        self.keep_display(Self::open_validated(&self.instance_path, false, Checks::Now(None), read_connection)?)
    }

    /// 显式升级流程用的 `reopen`，接受旧的核心。不做完整检查：升级在持有写锁时自己运行它们（#267）。
    pub(crate) fn reopen_for_upgrade_deferring_checks(&self) -> Result<Self> {
        self.keep_display(Self::open_validated(&self.instance_path, true, Checks::Deferred, read_connection)?)
    }

    /// 不做完整检查的 `reopen`，给提交之后的升级使用：它只检查自己写过的数据库。
    pub(crate) fn reopen_deferring_checks(&self) -> Result<Self> {
        self.keep_display(Self::open_validated(&self.instance_path, false, Checks::Deferred, read_connection)?)
    }

    /// 只对 library.sqlite 做完整检查，`progress` 收到它的完成比例；损坏与 `check` 中一样是错误。
    pub(crate) fn check_application(&self, progress: Option<Report>) -> Result<()> {
        match inspect_file(&self.db(), progress, Watch::default())? {
            Verdict::Damaged(result) => Err(Error::Schema(format!("图库数据库损坏：{result}"))),
            _ => Ok(()),
        }
    }

    /// 两项完整性检查，先查 library.sqlite；`progress` 收到合计的完成比例，每个文件按大小加权，
    /// 让比例跟上实际读过的字节。损坏是一个错误，写明 SQLite 发现了什么。
    pub(crate) fn check(&self, progress: Option<Progress>) -> Result<()> {
        checked(self.inspect(progress)?)
    }

    /// 在第一个损坏的文件处停下并说明是哪个的 `check`，供会话自己处理（#212）。
    pub(crate) fn inspect(&self, progress: Option<Progress>) -> Result<Vec<(&'static str, Verdict)>> {
        self.inspect_using(progress, read_connection)
    }

    fn inspect_using(&self, progress: Option<Progress>, open: ReadConnection) -> Result<Vec<(&'static str, Verdict)>> {
        let files = [("library.sqlite", self.db()), ("source.sqlite", self.source())];
        let weights = match &progress {
            Some(_) => {
                let (db, source) = (database_bytes(&files[0].1)?, database_bytes(&files[1].1)?);
                let total = (db + source).max(1) as f64;
                [(0.0, db as f64 / total), (db as f64 / total, source as f64 / total)]
            }
            None => [(0.0, 0.0); 2],
        };
        let mut found = Vec::new();
        for (index, (label, path)) in files.into_iter().enumerate() {
            let report = progress.clone().map(|p| {
                let (base, span) = weights[index];
                Arc::new(move |done: f64| p(label, base + span * done)) as Report
            });
            let verdict = check_database_reporting(&mut open(&path)?, report, Watch::default())?;
            let damaged = matches!(verdict, Verdict::Damaged(_));
            found.push((label, verdict));
            if damaged {
                break;
            }
        }
        Ok(found)
    }

    fn keep_display(&self, mut library: Self) -> Result<Self> {
        library.display_path = self.display_path.clone();
        Ok(library)
    }

    /// 设置写入时更新已捕获会话的配置，不重新打开或升级数据库。
    pub(crate) fn reload_config(&mut self) -> Result<()> {
        let text = fs::read_to_string(self.instance_path.join("library.json"))?;
        self.config = serde_json::from_str(&text).map_err(|e| Error::Invalid(e.to_string()))?;
        self.ordered_config = serde_json::from_str(&text).map_err(|e| Error::Invalid(e.to_string()))?;
        Ok(())
    }

    /// 用户打开时的路径，含别名：只用于显示和最近图库登记表。
    pub fn display_path(&self) -> &Path {
        &self.display_path
    }
    /// 规范的图库包身份，也是图库包每个文件的根目录；作为身份使用时要和新的会话 generation 组合，从不只用 config.id。
    pub fn instance_path(&self) -> &Path {
        &self.instance_path
    }
    pub fn config(&self) -> &Value {
        &self.config
    }
    pub fn name(&self) -> &str {
        self.config["name"].as_str().expect("validated name")
    }
    pub fn id(&self) -> &str {
        self.config["id"].as_str().expect("validated id")
    }
    pub fn source(&self) -> PathBuf {
        self.instance_path.join("source.sqlite")
    }
    pub fn db(&self) -> PathBuf {
        self.instance_path.join("library.sqlite")
    }
    pub fn features(&self) -> PathBuf {
        self.instance_path.join("features")
    }
    pub fn thumbnails(&self) -> PathBuf {
        self.instance_path.join("thumbnails")
    }
    pub fn backups(&self) -> PathBuf {
        self.instance_path.join("backups")
    }
    pub fn frozen(&self) -> bool {
        self.config.get("test").is_some_and(truthy) || self.config.get("frozen").is_some_and(truthy)
    }

    /// 字段顺序和嵌套配置的顺序与 web.library.Library.info 相同；大小包括正在使用的 WAL 字节。
    pub(crate) fn info(&self) -> Result<Json> {
        Ok(Json::object([
            ("path", self.display_path.to_str().ok_or_else(|| Error::Invalid("图库路径不是 UTF-8".into()))?.into()),
            ("name", self.ordered_config.get("name").expect("app_library internal invariant").clone()),
            ("id", self.ordered_config.get("id").expect("app_library internal invariant").clone()),
            ("format", self.ordered_config.get("format").expect("app_library internal invariant").clone()),
            ("sources", self.ordered_config.get("sources").cloned().unwrap_or(Json::Array(Vec::new()))),
            ("test", self.ordered_config.get("test").cloned().unwrap_or_else(|| Value::Null.into())),
            ("source_bytes", Value::from(database_bytes(&self.source())?).into()),
            ("library_bytes", Value::from(database_bytes(&self.db())?).into()),
            ("origins", crate::origin::descriptions().into()),
        ]))
    }

    /// 公开的字节契约，不暴露仅供 HTTP 使用的有序 JSON 表示。
    pub fn info_json(&self) -> Result<String> {
        Ok(self.info()?.dump())
    }
}

fn checked(mut verdicts: Vec<(&'static str, Verdict)>) -> Result<()> {
    match verdicts.pop() {
        Some((_, Verdict::Damaged(result))) => Err(Error::Schema(format!("图库数据库损坏：{result}"))),
        _ => Ok(()),
    }
}

type ReadConnection = fn(&Path) -> Result<Connection>;

fn read_archive(path: &Path) -> Result<Connection> {
    let c = read_connection(path)?;
    crate::sqlite::protect_archive(&c)?;
    Ok(c)
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64() != Some(0.0),
        Value::String(v) => !v.is_empty(),
        Value::Array(v) => !v.is_empty(),
        Value::Object(v) => !v.is_empty(),
    }
}
/// 后台任务的写连接等待另一个写入方的时长（#275）：同步及其文件夹和 Photos 扫描、识别导入、名称查询。
/// 与 `crate::configure` 给每个连接的等待相同；这些任务以前用 250 ms（名称用 2 s），
/// 比在写锁下对输入做哈希的研究检查还短。只有这些任务按这个时长等待：页面经各自的连接写入。现在写锁只短暂持有：
/// 研究在锁外检查输入，并给来源写入方让路，名称查询也不在锁下读 source.sqlite。停止在语句之间就能察觉，
/// 桌面外壳给宿主 10 秒退出，所以一次等待放得进去。仍然遇到数据库忙的任务照旧失败，由下一次自动同步或名称重试再试。
pub(crate) const TASK_WRITE_WAIT: Duration = Duration::from_secs(5);
pub(crate) fn read_connection(path: &Path) -> Result<Connection> {
    if !path.is_file() {
        return Err(Error::Invalid(format!("图库缺少已有数据库：{path}", path = path.display())));
    }
    // 不带 CREATE 标志，不迁移，也不改日志模式。图库包路径有意不按 URI 解释，
    // 所以含 '?' 或 '#' 的名称不会悄悄指向另一个文件。
    let conn = crate::sqlite::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(Duration::from_secs(10))?;
    Ok(conn)
}
/// 打开时是先运行两项完整性检查（`Now`），还是留给会话（#212）。
enum Checks {
    Now(Option<Progress>),
    Deferred,
}

#[cfg(test)]
pub(crate) fn check_database(conn: &mut Connection) -> Result<()> {
    match check_database_reporting(conn, None, Watch::default())? {
        Verdict::Sound => Ok(()),
        Verdict::Damaged(result) => Err(Error::Schema(format!("图库数据库损坏：{result}"))),
        Verdict::Yielded => unreachable!("a check that watches no writer never yields"),
    }
}

/// 一次完整性检查发现了什么。与文件本身无关的失败（I/O、检查被停止）是错误。
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Verdict {
    Sound,
    /// SQLite 自己对它发现的第一个问题的描述。
    Damaged(String),
    /// 另一个连接开始写这个文件：检查停下并释放了它的读锁（`Watch::writer`）。
    Yielded,
}

/// 运行中的检查除了进度还要盯着什么。`stop` 中断它，这是错误，不是裁决。`writer` 是这个文件的回滚日志（#212）：
/// source.sqlite 不是 WAL 模式，检查在大图库上持有读锁可达一分钟，会让它的写入方（同步、识别导入）
/// 在较短的忙等超时后放弃。检查轮询写入方提交之前创建的日志，一发现就立即让路。
#[derive(Clone, Default)]
pub(crate) struct Watch {
    pub(crate) stop: Option<Arc<AtomicBool>>,
    pub(crate) writer: Option<PathBuf>,
}
impl Watch {
    /// 检查查找日志的频率：远低于最短的写入方忙等超时（250 ms）。
    const POLL: Duration = Duration::from_millis(20);
}
pub(crate) type Report = Arc<dyn Fn(f64) + Send + Sync + std::panic::RefUnwindSafe>;

/// 一个文件的完整检查，用它自己的只读连接，按 `watch` 的要求盯着。
pub(crate) fn inspect_file(path: &Path, progress: Option<Report>, watch: Watch) -> Result<Verdict> {
    if watch.writer.as_ref().is_some_and(|journal| journal.exists()) {
        return Ok(Verdict::Yielded);
    }
    check_database_reporting(&mut read_connection(path)?, progress, watch)
}

/// quick_check 读每一页，然后（对表）再读一遍每一行来检查约束；SQLite 3.43+ 在两遍中都调用进度处理函数。
/// pager 的页面请求（缓存命中加未命中）跟踪到目前为止访问过的页；在真实图库上，2 MiB 的默认缓存让几乎每次请求都未命中，
/// 合计是页数的 1.4–1.6×（2026-10-02），所以 `READS` 是预期的比例。估低了进度条会停在接近结尾处，
/// 估高了会在结尾短暂跳一下，后者问题较小。
///
/// 页面命中也算访问；检查第二遍可能直接命中第一遍留下的页。SQLite 3.54 的内置构建不再启用
/// SQLITE_ENABLE_MEMORY_MANAGEMENT，页面请求统计仍同时适用于有、无共享页缓存组的构建。
///
/// SQLite 报告为损坏或根本不是数据库的文件也算损坏，不论是哪条语句发现的。
fn check_database_reporting(conn: &mut Connection, progress: Option<Report>, watch: Watch) -> Result<Verdict> {
    const READS: f64 = 1.5;
    let Watch { stop, writer } = watch;
    let watched = progress.is_some() || stop.is_some() || writer.is_some();
    let yielded = Arc::new(AtomicBool::new(false));
    if watched {
        let pages: i64 = match conn.query_row("PRAGMA page_count", [], |row| row.get(0)) {
            Ok(pages) => pages,
            Err(e) => return damage(e),
        };
        let expected = pages.max(1) as f64 * READS;
        // SAFETY: 只取连接的原始句柄，不转移所有权；句柄只在这个连接自己的进度回调里使用，连接在整个检查期间存活。
        let handle = Handle(unsafe { conn.handle() });
        let start = page_requests(handle);
        let report = progress.clone();
        if let Some(progress) = &progress {
            progress(0.0);
        }
        let (flag, mut polled) = (yielded.clone(), std::time::Instant::now());
        conn.progress_handler(
            100,
            Some(move || {
                if let Some(report) = &report {
                    report(((page_requests(handle) - start).max(0) as f64 / expected).min(0.99));
                }
                if let Some(journal) = writer.as_ref().filter(|_| polled.elapsed() >= Watch::POLL) {
                    polled = std::time::Instant::now();
                    if journal.exists() {
                        flag.store(true, Ordering::SeqCst);
                        return true;
                    }
                }
                stop.as_ref().is_some_and(|stop| stop.load(Ordering::SeqCst))
            }),
        )?;
    }
    let result: rusqlite::Result<String> = conn.query_row("PRAGMA quick_check(1)", [], |row| row.get(0));
    if watched {
        conn.progress_handler(0, None::<fn() -> bool>)?;
    }
    if yielded.load(Ordering::SeqCst) {
        return Ok(Verdict::Yielded);
    }
    match result {
        Ok(text) if text == "ok" => {
            if let Some(progress) = &progress {
                progress(1.0);
            }
            Ok(Verdict::Sound)
        }
        Ok(text) => Ok(Verdict::Damaged(text)),
        Err(e) => damage(e),
    }
}

/// SQLite 的损坏和非数据库结果是文件本身的状态；其他情况仍是错误。
pub(crate) fn damage(error: rusqlite::Error) -> Result<Verdict> {
    match corruption(&error) {
        Some(detail) => Ok(Verdict::Damaged(detail)),
        None => Err(error.into()),
    }
}
pub(crate) fn corruption(error: &rusqlite::Error) -> Option<String> {
    match error {
        rusqlite::Error::SqliteFailure(e, message)
            if matches!(e.code, rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase) =>
        {
            Some(message.clone().unwrap_or_else(|| e.to_string()))
        }
        _ => None,
    }
}

/// 连接自己的句柄，只在它的进度回调里读取（同一线程，在它的 step 内）。
#[derive(Clone, Copy)]
struct Handle(*mut rusqlite::ffi::sqlite3);
// SAFETY: 句柄只在所属连接的进度回调里读取，与连接同一线程、同一次 step 内；实现 Send 只为满足回调闭包的约束，
// 不会跨线程使用。
unsafe impl Send for Handle {}
impl std::panic::RefUnwindSafe for Handle {}

fn page_requests(handle: Handle) -> i64 {
    [rusqlite::ffi::SQLITE_DBSTATUS_CACHE_HIT, rusqlite::ffi::SQLITE_DBSTATUS_CACHE_MISS]
        .into_iter()
        .map(|op| {
            let (mut current, mut high) = (0, 0);
            // SAFETY: `handle.0` 是正在执行 step 的存活连接，出参是本地变量，reset 标志为 0 不修改统计。
            unsafe {
                rusqlite::ffi::sqlite3_db_status(handle.0, op, &mut current, &mut high, 0);
            }
            current as i64
        })
        .sum()
}
pub(crate) fn require_meta(conn: &Connection, key: &str, expected: &str) -> Result<()> {
    let mut query = conn.prepare("SELECT value FROM meta WHERE key=?1")?;
    let mut rows = query.query([key])?;
    let value = rows.next()?.map(|r| r.get::<_, String>(0)).transpose()?;
    if value.as_deref() != Some(expected) || rows.next()?.is_some() {
        return Err(Error::Schema(format!("图库需要显式初始化、升级或切换：{key} 应为 {expected}")));
    }
    Ok(())
}
/// 打开和加锁的升级快照共用同样的只读版本和归属检查。
fn validate_checked(conn: &Connection, upgrade: bool) -> Result<()> {
    if upgrade {
        return crate::app_upgrade::validate(conn);
    }
    crate::app_upgrade::refuse_unswitched(conn)?;
    require_meta(conn, "schema_version", APPLICATION_SCHEMA)?;
    crate::schema::require_current(conn)?;
    for marker in CORE_MARKERS {
        require_meta(conn, marker, "core")?;
    }
    Ok(())
}
fn database_bytes(path: &Path) -> Result<u64> {
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    let mut total = 0u64;
    for file in [path, Path::new(&wal)] {
        match fs::metadata(file) {
            Ok(metadata) => {
                total = total.checked_add(metadata.len()).ok_or_else(|| Error::Invalid("数据库尺寸溢出".into()))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        root: PathBuf,
        package: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "illustash-app-library-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let package = root.join("synthetic.pilib");
            fs::create_dir_all(&package).unwrap();
            fs::write(package.join("library.json"), r#"{"format":1,"id":"synthetic","name":"合成图库","sources":[{"kind":"folder","id":"s1","path":"/synthetic-only"}],"test":{"name":"合成","id":"persona"},"extra":"retain"}"#).unwrap();
            let db = crate::sqlite::open(package.join("library.sqlite")).unwrap();
            db.execute_batch(&format!("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT); INSERT INTO meta VALUES ('schema_version','36'),('terms_schema','{v}'),('terms_artists','core'),('terms_labels','core'),('terms_characters','core'),('terms_names','core');", v = crate::SCHEMA_VERSION)).unwrap();
            let src = crate::sqlite::open(package.join("source.sqlite")).unwrap();
            src.execute_batch("CREATE TABLE assets(uuid TEXT,filename TEXT,src_path TEXT,width INTEGER,height INTEGER,added_at TEXT,src_kind TEXT,in_scope INTEGER,trashed INTEGER); CREATE TABLE labels(uuid TEXT,dim TEXT,value TEXT,conf REAL);").unwrap();
            Self { root, package }
        }
        fn bytes(&self) -> Vec<(String, Vec<u8>)> {
            let mut result = fs::read_dir(&self.package)
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    (entry.file_name().to_str().unwrap().to_string(), fs::read(entry.path()).unwrap())
                })
                .collect::<Vec<_>>();
            result.sort_by(|a, b| a.0.cmp(&b.0));
            result
        }
        fn refuses_unchanged(&self) {
            let before = self.bytes();
            assert!(Library::open(&self.package).is_err());
            assert_eq!(before, self.bytes());
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn opens_existing_without_side_effects_and_keeps_info_order() {
        let f = Fixture::new();
        let before = f.bytes();
        let lib = Library::open(&f.package).unwrap();
        assert_eq!(lib.display_path(), f.package);
        assert_eq!(lib.instance_path(), crate::app_paths::canonical(&f.package).unwrap());
        assert_eq!(lib.id(), "synthetic");
        assert_eq!(lib.name(), "合成图库");
        assert!(lib.frozen());
        assert_eq!(lib.config()["extra"], "retain");
        assert!(!lib.features().exists());
        assert!(!lib.backups().exists());
        assert!(!lib.thumbnails().exists());
        let info = lib.info_json().unwrap();
        let source_bytes = fs::metadata(lib.source()).unwrap().len();
        let library_bytes = fs::metadata(lib.db()).unwrap().len();
        assert_eq!(
            info,
            format!(
                r#"{{"path": {}, "name": "合成图库", "id": "synthetic", "format": 1, "sources": [{{"kind": "folder", "id": "s1", "path": "/synthetic-only"}}], "test": {{"name": "合成", "id": "persona"}}, "source_bytes": {source_bytes}, "library_bytes": {library_bytes}, "origins": [{{"id": "pixiv", "label": "Pixiv"}}, {{"id": "x", "label": "X"}}, {{"id": "other", "label": "其他"}}]}}"#,
                serde_json::to_string(lib.display_path().to_str().unwrap()).unwrap()
            )
        );
        assert_eq!(before, f.bytes());
    }
    #[test]
    fn rejects_each_version_and_core_marker_without_upgrading() {
        let (older, newer) = ((crate::SCHEMA_VERSION - 1).to_string(), (crate::SCHEMA_VERSION + 1).to_string());
        for (key, value) in [
            ("schema_version", "35"),
            ("schema_version", "37"),
            ("terms_schema", older.as_str()),
            ("terms_schema", newer.as_str()),
            ("terms_schema", "bad"),
            ("terms_artists", "legacy"),
            ("terms_labels", "legacy"),
            ("terms_characters", "legacy"),
            ("terms_names", "legacy"),
        ] {
            let f = Fixture::new();
            let db = crate::sqlite::open(f.package.join("library.sqlite")).unwrap();
            db.execute("UPDATE meta SET value=? WHERE key=?", [value, key]).unwrap();
            drop(db);
            f.refuses_unchanged();
        }
        for key in
            ["schema_version", "terms_schema", "terms_artists", "terms_labels", "terms_characters", "terms_names"]
        {
            let f = Fixture::new();
            let db = crate::sqlite::open(f.package.join("library.sqlite")).unwrap();
            db.execute("DELETE FROM meta WHERE key=?", [key]).unwrap();
            drop(db);
            f.refuses_unchanged();
        }
    }
    #[test]
    fn rejects_bad_config_and_format_without_creating_anything() {
        for text in [
            "broken",
            "[]",
            "{}",
            r#"{"format":2,"id":"x","name":"x"}"#,
            r#"{"format":1,"id":null,"name":"x"}"#,
            r#"{"format":1,"id":"x"}"#,
            r#"{"format":1,"id":"x","name":"x","sources":null}"#,
            r#"{"format":1,"id":"x","name":"x","sources":[3]}"#,
        ] {
            let f = Fixture::new();
            fs::write(f.package.join("library.json"), text).unwrap();
            f.refuses_unchanged();
        }
    }
    #[test]
    fn rejects_missing_or_corrupt_required_files_without_recreating() {
        for file in ["library.json", "source.sqlite", "library.sqlite"] {
            let f = Fixture::new();
            fs::remove_file(f.package.join(file)).unwrap();
            f.refuses_unchanged();
        }
        for file in ["source.sqlite", "library.sqlite"] {
            let f = Fixture::new();
            fs::write(f.package.join(file), b"not SQLite").unwrap();
            f.refuses_unchanged();
        }
        let f = Fixture::new();
        let absent = f.root.join("absent.pilib");
        assert!(Library::open(&absent).is_err());
        assert!(!absent.exists());
    }
    #[test]
    fn rejects_source_schema_needing_upgrade() {
        for sql in
            ["DROP TABLE labels", "ALTER TABLE labels DROP COLUMN conf", "ALTER TABLE assets DROP COLUMN in_scope"]
        {
            let f = Fixture::new();
            let source = crate::sqlite::open(f.package.join("source.sqlite")).unwrap();
            source.execute_batch(sql).unwrap();
            drop(source);
            f.refuses_unchanged();
        }
    }
    #[test]
    fn optional_config_fields_and_provenance_are_not_required() {
        let f = Fixture::new();
        fs::write(f.package.join("library.json"), r#"{"name":"x","id":"x","format":1}"#).unwrap();
        let lib = Library::open(&f.package).unwrap();
        assert!(!lib.frozen());
        assert!(!lib.instance_path().join("provenance.sqlite").exists());
        let info = lib.info_json().unwrap();
        assert!(info.contains("\"sources\": [], \"test\": null"));
    }
    #[test]
    fn reports_check_progress_monotonically_by_file_size() {
        let f = Fixture::new();
        // 只有合成的大块数据：source.sqlite 里带索引约 8 MiB，library.sqlite 里 0.1 MiB。
        let src = crate::sqlite::open(f.package.join("source.sqlite")).unwrap();
        src.execute_batch(
            "CREATE TABLE bulk(id INTEGER PRIMARY KEY, body TEXT); CREATE INDEX bulk_body ON bulk(body);
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<40000)
             INSERT INTO bulk SELECT i, printf('%0100d', i) FROM n;",
        )
        .unwrap();
        drop(src);
        // 保持其他连接打开并读热其缓存，再确认被检查的连接仍报告单调进度。
        let other = f.root.join("other.sqlite");
        fs::copy(f.package.join("source.sqlite"), &other).unwrap();
        for idle in [0, 8] {
            let held: Vec<Connection> = (0..idle)
                .map(|_| {
                    let c = read_connection(&other).unwrap();
                    c.query_row("SELECT sum(length(body)) FROM bulk", [], |row| row.get::<_, i64>(0)).unwrap();
                    c
                })
                .collect();
            let seen = Arc::new(std::sync::Mutex::new(Vec::<(String, f64)>::new()));
            let sink = seen.clone();
            let progress: Progress = Arc::new(move |label, done| sink.lock().unwrap().push((label.into(), done)));
            let before = f.bytes();
            Library::open_reporting(&f.package, progress).unwrap();
            drop(held);
            assert_eq!(f.bytes(), before);
            let seen = seen.lock().unwrap();
            assert!(seen.windows(2).all(|w| w[0].1 <= w[1].1), "{seen:?}");
            assert_eq!(seen.first().unwrap(), &("library.sqlite".to_string(), 0.0));
            assert_eq!(seen.last().unwrap(), &("source.sqlite".to_string(), 1.0));
            let source: Vec<f64> = seen.iter().filter(|(l, _)| l == "source.sqlite").map(|(_, d)| *d).collect();
            // 扫描本身进行中的中间报告，在完成之前就走完大部分。
            assert!(source.len() > 10, "{seen:?}");
            assert!(source[source.len() - 2] > 0.8, "idle connections {idle}: {:?}", &source[source.len() - 3..]);
        }
    }
    #[test]
    fn wal_size_is_counted_without_opening_or_changing_it() {
        let f = Fixture::new();
        let lib = Library::open(&f.package).unwrap();
        let base = database_bytes(&lib.db()).unwrap();
        fs::write(f.package.join("library.sqlite-wal"), b"synthetic-size-only").unwrap();
        assert_eq!(database_bytes(&lib.db()).unwrap(), base + 19);
        assert!(lib.info_json().unwrap().contains(&format!("\"library_bytes\": {}", base + 19)));
    }
    #[cfg(unix)]
    #[test]
    fn canonical_identity_distinguishes_copies_not_aliases() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let alias = f.root.join("alias.pilib");
        symlink(&f.package, &alias).unwrap();
        let a = Library::open(&f.package).unwrap();
        let b = Library::open(&alias).unwrap();
        assert_eq!(a.instance_path(), b.instance_path());
        assert_ne!(a.display_path(), b.display_path());
        assert_eq!(b.display_path(), alias);
        let alias_text = serde_json::to_string(alias.to_str().unwrap()).unwrap();
        assert!(b.info_json().unwrap().starts_with(&format!("{{\"path\": {alias_text},")));
        assert_eq!(b.source(), a.instance_path().join("source.sqlite"));
        assert_eq!(b.db(), a.instance_path().join("library.sqlite"));
        fs::remove_file(&alias).unwrap();
        assert!(b.db().is_file());
        let other = Fixture::new();
        let c = Library::open(&other.package).unwrap();
        assert_eq!(a.id(), c.id());
        assert_ne!(a.instance_path(), c.instance_path());
    }
    #[cfg(unix)]
    #[test]
    fn retargeted_alias_keeps_every_package_file_on_the_opened_package() {
        use std::os::unix::fs::symlink;
        // 两个图库包配置 ID 相同；别名先指向 A，再指向 B。
        let (a, b) = (Fixture::new(), Fixture::new());
        fs::write(b.package.join("library.json"), r#"{"format":1,"id":"synthetic","name":"另一个合成图库"}"#).unwrap();
        let alias = a.root.join("alias.pilib");
        symlink(&a.package, &alias).unwrap();
        let mut opened = Library::open(&alias).unwrap();
        fs::remove_file(&alias).unwrap();
        symlink(&b.package, &alias).unwrap();
        let home = crate::app_paths::canonical(&a.package).unwrap();
        let files = [opened.db(), opened.source(), opened.features(), opened.thumbnails(), opened.backups()];
        assert_eq!(files.map(|p| p.parent().unwrap().to_owned()), [(); 5].map(|_| home.clone()));
        opened.reload_config().unwrap();
        assert_eq!(opened.name(), "合成图库");
        let again = opened.reopen().unwrap();
        assert_eq!(
            (again.instance_path(), again.display_path(), again.name()),
            (home.as_path(), alias.as_path(), "合成图库")
        );
        // 重新打开别名就是有意选了 B。
        assert_eq!(Library::open(&alias).unwrap().name(), "另一个合成图库");
    }
}
