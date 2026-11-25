//! 用冻结的角色扩展头检查已存的 WD 向量。包可以是内置的 `wd-extension-v1`，也可以是签名目录里列出的新版本（#602，
//! `app_packs`）；同一图库从一个版本换到另一个，沿用同一个 `WD_EXT` 来源的替换流程，图库里记下用的是哪个包。
use crate::app_library::Library;
use crate::app_packs::{Base, Pack, WD_FORMAT_V1, WD_FORMAT_V2};
use crate::{Error, Result};
use illustash_core::wd_extension::{self, Hit};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

mod scoring;

const DIM: usize = 1024;

/// 包文件第一行的清单。`illustash-wd-extension/1`：统一的 `threshold`，可以另带逐头的 `thresholds`（#551 的 257 头包），
/// 全部是一级头。`/2`：必须带逐头的 `thresholds` 和 `tiers`（1 可以补主归属，2 只出候选）。
#[derive(Deserialize)]
struct Manifest {
    format: String,
    wd_model_sha256: String,
    wd_tags_sha256: String,
    #[serde(default)]
    threshold: Option<f64>,
    labels: Vec<String>,
    #[serde(default)]
    thresholds: Option<Vec<f64>>,
    #[serde(default)]
    tiers: Option<Vec<u8>>,
}

struct Package {
    wd_model_sha256: String,
    wd_tags_sha256: String,
    labels: Vec<String>,
    thresholds: Vec<f64>,
    /// 每个头是不是二级（只出候选）。
    candidate_only: Vec<bool>,
    weights: Vec<f32>,
    bias: Vec<f32>,
}

fn float32(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().expect("四字节分块"))).collect()
}

impl Package {
    /// 读一个版本的包：大小和 SHA-256 先对上这个版本，再按格式解开。
    fn load(path: &Path, pack: &Pack) -> Result<Self> {
        let input = crate::app_recognition::file(path, pack.file.size)?;
        if input.len() as u64 != pack.file.size || crate::strict_json::sha256_hex(&input) != pack.file.sha256 {
            return Err(Error::invalid("WD 扩展包与这个版本的冻结文件不符"));
        }
        Self::decode(&input, pack)
    }

    fn decode(input: &[u8], pack: &Pack) -> Result<Self> {
        let end = input.iter().position(|&byte| byte == b'\n').ok_or_else(|| Error::invalid("缺少 WD 扩展包清单"))?;
        let manifest: Manifest =
            serde_json::from_slice(&input[..end]).map_err(|e| Error::invalid(format!("WD 扩展包清单无效：{e}")))?;
        let coefficients = &input[end + 1..];
        let heads = pack.heads;
        // 目录里的包，模型和标签表都要是目录写的那两个。内置包只看模型（models.json 只写了它，和以前一样）；
        // 标签表和每次交付对不对得上，扫描时逐批核对（`compatible`）。
        let base_matches = matches!(&pack.base, Base::Wd { model, tags }
            if *model == manifest.wd_model_sha256 && (pack.builtin || *tags == manifest.wd_tags_sha256));
        if manifest.format != pack.format
            || !base_matches
            || manifest.labels.len() != heads
            || coefficients.len() != heads * (DIM + 1) * 4
        {
            return Err(Error::invalid("WD 扩展包格式不符"));
        }
        let (thresholds, tiers) = match manifest.format.as_str() {
            WD_FORMAT_V1 if manifest.tiers.is_none() => match (manifest.thresholds, manifest.threshold) {
                (Some(values), Some(_)) => (values, vec![1; heads]),
                (None, Some(value)) => (vec![value; heads], vec![1; heads]),
                _ => return Err(Error::invalid("WD 扩展包缺少阈值")),
            },
            WD_FORMAT_V2 => match (manifest.thresholds, manifest.tiers) {
                (Some(values), Some(tiers)) => (values, tiers),
                _ => return Err(Error::invalid("WD 扩展包缺少逐头阈值或级别")),
            },
            _ => return Err(Error::invalid("WD 扩展包格式不符")),
        };
        if thresholds.len() != heads || tiers.len() != heads {
            return Err(Error::invalid("WD 扩展包阈值或级别数目不符"));
        }
        if thresholds.iter().any(|value| !(0.0..=1.0).contains(value)) || tiers.iter().any(|t| !matches!(t, 1 | 2)) {
            return Err(Error::invalid("WD 扩展包阈值或级别不合法"));
        }
        let candidate_only = tiers.iter().map(|&t| t == 2).collect::<Vec<_>>();
        if candidate_only.iter().filter(|&&c| c).count() != pack.candidate_only {
            return Err(Error::invalid("WD 扩展包的二级头数和目录不符"));
        }
        let (weights, bias) = coefficients.split_at(heads * DIM * 4);
        Ok(Self {
            wd_model_sha256: manifest.wd_model_sha256,
            wd_tags_sha256: manifest.wd_tags_sha256,
            labels: manifest.labels,
            thresholds,
            candidate_only,
            weights: float32(weights),
            bias: float32(bias),
        })
    }

    fn compatible(&self, manifest: &str) -> Result<()> {
        let doc: Value =
            serde_json::from_str(manifest).map_err(|e| Error::invalid(format!("识别账本清单无效：{e}")))?;
        // 带角色认领的交付把公开清单包在 recognition 字段里，其他交付直接存公开清单。
        let manifest = if doc.get("format").is_some() { &doc } else { &doc["recognition"] };
        let wd = match manifest["format"].as_str() {
            Some("illustash-recognition/1" | "illustash-recognition/2") => &manifest["models"]["wd"],
            _ => &Value::Null,
        };
        if wd["version"] != "swinv2_448"
            || wd["dimension"].as_u64() != Some(DIM as u64)
            || wd["sha256"].as_str() != Some(&self.wd_model_sha256)
            || wd["tags_sha256"].as_str() != Some(&self.wd_tags_sha256)
        {
            return Err(Error::Conflict("WD 扩展包与图库向量的底座模型或标签表版本不匹配".into()));
        }
        Ok(())
    }

    /// 校验一行 WD 向量并追加到这一块（#582）。
    fn push_vector(bytes: &[u8], block: &mut Vec<f32>) -> Result<()> {
        if bytes.len() != DIM * 4 {
            return Err(Error::invalid("WD 特征向量维数不正确"));
        }
        let start = block.len();
        block.extend(bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().expect("四字节分块"))));
        if block[start..].iter().any(|v| !v.is_finite()) {
            return Err(Error::invalid("WD 特征向量包含非有限值"));
        }
        Ok(())
    }

    /// 给攒好的一块图片打分（`scoring`），命中按图片、再按头的顺序追加，然后清空这一块。
    fn hits(&self, heads: &scoring::Heads<'_>, uuids: &mut Vec<String>, block: &mut Vec<f32>, out: &mut Vec<Hit>) {
        heads.score(block, |image, head, probability| {
            // 级别在打分之后才起作用：命中照常记下，二级头的命中标成只出候选（#602）。
            let (tag, candidate_only) = (self.labels[head].clone(), self.candidate_only[head]);
            out.push(Hit { uuid: uuids[image].clone(), tag, probability, candidate_only });
        });
        uuids.clear();
        block.clear();
    }
}

fn writer(path: &Path) -> Result<Connection> {
    let conn = crate::sqlite::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    crate::configure(&conn)?;
    conn.busy_timeout(crate::app_library::TASK_WRITE_WAIT)?;
    Ok(conn)
}

const IN_SCOPE: &str = "v.model='wd' AND a.in_scope=1 AND coalesce(a.trashed,0)=0";
const VECTORS: &str = "FROM embeddings v JOIN assets a ON a.uuid=v.uuid";
const SCOPE: &str = "AND v.uuid IN (SELECT value FROM json_each(?1))";
const META_KEY: &str = "wd_extension";
/// 角色资料包默认启用以后（#602），本图库明确不用：提示里选了不用，或撤回过。启用时清掉。
const DECLINED_KEY: &str = "character_pack_declined";

/// 新图库第一次同步时角色资料包还不在本机（#630）：本该默认启用，等包就位后由宿主补上（`app_packs::pending_enable`）。
/// 启用、撤回或选了不用时清掉。
const PENDING_KEY: &str = "character_pack_default_pending";

fn flag(app: &Connection, key: &str) -> Result<bool> {
    Ok(app.query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE key=?1)", [key], |r| r.get(0))?)
}

/// 本图库是不是明确不用角色资料包（`app_packs::default_state`）。
pub(crate) fn declined(app: &Connection) -> Result<bool> {
    flag(app, DECLINED_KEY)
}

/// 本图库是不是等着角色资料包就位后默认启用（#630）。
pub(crate) fn default_pending(app: &Connection) -> Result<bool> {
    flag(app, PENDING_KEY)
}

/// 记下或清掉「本图库不用角色资料包」。两种情况都清掉「等包就位后默认启用」：用户已经选过了。
pub(crate) fn set_declined(library: &Library, declined: bool) -> Result<()> {
    set_flags(library, &[(DECLINED_KEY, declined), (PENDING_KEY, false)])
}

/// 记下「等角色资料包就位后默认启用」（#630）。调用方先确认本图库没选过不用（`app_packs::default_enable`）。
pub(crate) fn set_default_pending(library: &Library) -> Result<()> {
    set_flags(library, &[(PENDING_KEY, true)])
}

/// 在一个事务里记下（`true`）或清掉（`false`）几个只看有没有的 `meta` 标记。
fn set_flags(library: &Library, flags: &[(&str, bool)]) -> Result<()> {
    ensure_writable(library)?;
    let mut app = writer(&library.db())?;
    crate::schema::require_current(&app)?;
    let tx = app.transaction_with_behavior(TransactionBehavior::Immediate)?;
    crate::schema::begin_writing(&tx)?;
    for &(key, on) in flags {
        if on {
            tx.execute("INSERT OR REPLACE INTO meta(key,value) VALUES (?1,CURRENT_TIMESTAMP)", [key])?;
        } else {
            tx.execute("DELETE FROM meta WHERE key=?1", [key])?;
        }
    }
    crate::schema::commit_writing(tx)
}

fn has_table(src: &Connection, name: &str) -> Result<bool> {
    Ok(src
        .query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?)", [name], |r| r.get(0))?)
}

/// 本图库存着的 WD 向量张数（角色资料包启用时决定现在检查还是等识别结果，#602）。
pub(crate) fn stored_vectors(library: &Library) -> Result<usize> {
    vector_count(&crate::app_library::read_connection(&library.source())?, None)
}

fn vector_count(src: &Connection, scope: Option<&str>) -> Result<usize> {
    if !has_table(src, "embeddings")? {
        return Ok(0);
    }
    let filter = if scope.is_some() { SCOPE } else { "" };
    Ok(src.query_row(
        &format!("SELECT count(*) {VECTORS} WHERE {IN_SCOPE} {filter}"),
        rusqlite::params_from_iter(scope),
        |r| r.get(0),
    )?)
}

fn scan(
    src: &Connection,
    package: &Package,
    scope: Option<&str>,
    expected_delivery: Option<&str>,
    progress: &Progress<'_>,
) -> Result<(usize, Vec<Hit>)> {
    let total = vector_count(src, scope)?;
    progress.count(0, total)?;
    if total == 0 {
        return Ok((0, Vec::new()));
    }
    let has_ledger = has_table(src, "recognition_assets")?;
    if has_ledger != has_table(src, "recognition_imports")? {
        return Err(Error::Schema("WD 识别账本不完整".into()));
    }
    let filter = if scope.is_some() { SCOPE } else { "" };
    let sql = if has_ledger {
        format!(
            "SELECT v.uuid,v.vec,r.package,i.manifest,i.status {VECTORS}
                 LEFT JOIN recognition_assets r ON r.asset=a.uuid LEFT JOIN recognition_imports i ON i.digest=r.package
                 WHERE {IN_SCOPE} {filter} ORDER BY v.uuid"
        )
    } else {
        format!("SELECT v.uuid,v.vec,NULL,NULL,NULL {VECTORS} WHERE {IN_SCOPE} {filter} ORDER BY v.uuid")
    };
    let mut seen = std::collections::HashSet::new();
    let mut hits = Vec::new();
    let mut checked = 0;
    let heads = scoring::Heads::new(DIM, &package.weights, &package.bias, &package.thresholds);
    let rows_per_block = heads.block_rows();
    let (mut uuids, mut block) = (Vec::with_capacity(rows_per_block), Vec::with_capacity(rows_per_block * DIM));
    let mut stmt = src.prepare(&sql)?;
    let mut rows = stmt.query(rusqlite::params_from_iter(scope))?;
    while let Some(row) = rows.next()? {
        progress.check()?;
        if let Some(delivery) = row.get::<_, Option<String>>(2)? {
            let status: Option<String> = row.get(4)?;
            if status.as_deref() != Some("done")
                && !(expected_delivery == Some(delivery.as_str()) && status.as_deref() == Some("pending"))
            {
                return Err(Error::Conflict("WD 识别交付尚未完成，请先恢复这次交付".into()));
            }
            if expected_delivery != Some(delivery.as_str()) && seen.insert(delivery) {
                package.compatible(&row.get::<_, String>(3)?)?;
            }
        }
        // 旧图库无逐图交付账本时，使用已经存储的 WD 向量。
        Package::push_vector(&row.get::<_, Vec<u8>>(1)?, &mut block)?;
        uuids.push(row.get::<_, String>(0)?);
        if uuids.len() == rows_per_block {
            progress.check()?;
            checked += uuids.len();
            package.hits(&heads, &mut uuids, &mut block, &mut hits);
            progress.count(checked, total)?;
        }
    }
    if !uuids.is_empty() {
        progress.check()?;
        checked += uuids.len();
        package.hits(&heads, &mut uuids, &mut block, &mut hits);
        progress.count(checked, total)?;
    }
    Ok((checked, hits))
}

#[derive(serde::Serialize, Deserialize)]
struct Enabled {
    package_sha256: String,
    enabled_at: String,
    checked_at: String,
    scored: usize,
}

fn enabled(c: &Connection) -> Result<Option<Enabled>> {
    let raw: Option<String> = c.query_row("SELECT value FROM meta WHERE key=?", [META_KEY], |r| r.get(0)).optional()?;
    raw.map(|value| serde_json::from_str(&value).map_err(|e| Error::Schema(format!("WD 扩展状态无效：{e}"))))
        .transpose()
}

fn save_enabled(app: &Connection, enabled: &Enabled) -> Result<()> {
    let value = serde_json::to_string(enabled).expect("WD 扩展状态可序列化");
    app.execute("INSERT OR REPLACE INTO meta(key,value) VALUES (?,?)", (META_KEY, value))?;
    Ok(())
}

/// 命令行（`wd-extension --package`）：按文件的 SHA-256 找它是哪个可用版本，用它检查整个图库。
pub fn apply(library: &Library, path: &Path) -> Result<Value> {
    ensure_writable(library)?;
    let resources = crate::app_resources::default_root();
    let bytes = crate::app_recognition::file(path, 512 << 20)?;
    let sha = crate::strict_json::sha256_hex(&bytes);
    let pack = crate::app_packs::component_by_sha(&resources, crate::app_packs::Kind::Wd, &sha)?
        .ok_or_else(|| Error::invalid("WD 扩展包与可用的版本都不符"))?;
    apply_loaded(library, &Package::load(path, &pack)?, &pack.file.sha256, &Progress::silent())
}

fn apply_loaded(library: &Library, package: &Package, sha256: &str, progress: &Progress<'_>) -> Result<Value> {
    let mut source = writer(&library.source())?;
    // 先读取版本，再打开评分快照。取得写锁后再核对版本，评分无需占用来源库写锁。
    let version =
        |c: &Connection| -> rusqlite::Result<i64> { c.pragma_query_value(None, "data_version", |r| r.get(0)) };
    let before = version(&source)?;
    let snapshot = source.transaction()?;
    let (checked, hits) = scan(&snapshot, package, None, None, progress)?;
    snapshot.commit()?;
    if checked == 0 {
        return Err(Error::Conflict("这个图库没有已保存的 WD 向量".into()));
    }
    progress.check()?;
    let stable = source.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if version(&stable)? != before {
        return Err(Error::Conflict("检查期间图库来源有变化，请重新检查".into()));
    }
    progress.begin_write()?;
    let mut app = writer(&library.db())?;
    crate::schema::require_current(&app)?;
    let tx = app.transaction_with_behavior(TransactionBehavior::Immediate)?;
    crate::schema::begin_writing(&tx)?;
    let current = enabled(&tx)?;
    let now: String = tx.query_row("SELECT datetime('now')", [], |r| r.get(0))?;
    let entry = Enabled {
        package_sha256: sha256.into(),
        enabled_at: current.map_or_else(|| now.clone(), |state| state.enabled_at),
        checked_at: now,
        scored: checked,
    };
    save_enabled(&tx, &entry)?;
    let changed = wd_extension::apply(&tx, sha256, &hits)?;
    crate::schema::commit_writing(tx)?;
    Ok(json!({"package_sha256":sha256,"scored":checked,"hits":hits.len(),"changed":changed}))
}

pub fn withdraw(library: &Library) -> Result<Value> {
    ensure_writable(library)?;
    let mut app = writer(&library.db())?;
    crate::schema::require_current(&app)?;
    let tx = app.transaction_with_behavior(TransactionBehavior::Immediate)?;
    crate::schema::begin_writing(&tx)?;
    let changed = wd_extension::withdraw(&tx)?;
    tx.execute("DELETE FROM meta WHERE key=?", [META_KEY])?;
    crate::schema::commit_writing(tx)?;
    Ok(json!({"changed":changed}))
}

/// 本图库启用的、本机就位的包（自动检查和角色检查用）。启用的版本不在可用列表里、或包不在本机时为 None。
pub(crate) fn enabled_pack(resources: &Path, app: &Connection) -> Result<Option<(Pack, PathBuf)>> {
    let Some(active) = enabled(app)? else { return Ok(None) };
    let Some(pack) = crate::app_packs::component_by_sha(resources, crate::app_packs::Kind::Wd, &active.package_sha256)?
    else {
        return Ok(None);
    };
    Ok(crate::app_models::extension_file(resources, &pack)?.map(|path| (pack, path)))
}

/// 角色检查使用图库的持久启用状态和本机已校验的包（#561）。包在本机缺席时仍按未覆盖处理。二级头（#602）只出候选，
/// 不算已经认得的角色。
pub(crate) fn covered_labels(app: &Connection, resources: &Path) -> Result<Vec<String>> {
    let Some((pack, path)) = enabled_pack(resources, app)? else { return Ok(Vec::new()) };
    Ok(Package::load(&path, &pack)
        .map(|p| p.labels.into_iter().zip(p.candidate_only).filter(|(_, c)| !c).map(|(label, _)| label).collect())
        .unwrap_or_default())
}

fn ensure_writable(library: &Library) -> Result<()> {
    if library.frozen() {
        return Err(Error::Conflict("图库只读".into()));
    }
    Ok(())
}

use crate::app_tasks::{lock_ignoring_poison, Task, TaskContext, TaskIdentity, TaskOutcome};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

enum Progress<'a> {
    Silent,
    Task(&'a TaskContext, &'a Mutex<Value>),
    Delivery(&'a AtomicBool),
}

impl Progress<'_> {
    fn silent() -> Self {
        Self::Silent
    }

    fn check(&self) -> Result<()> {
        match self {
            Self::Silent => Ok(()),
            Self::Task(ctx, _) => crate::app_sync::check(ctx.cancellation()),
            Self::Delivery(cancel) => crate::app_sync::check(cancel),
        }
    }

    fn count(&self, done: usize, total: usize) -> Result<()> {
        self.check()?;
        if let Self::Task(_, state) = self {
            let mut state = lock_ignoring_poison(state);
            state["done"] = done.into();
            state["total"] = total.into();
        }
        Ok(())
    }

    fn begin_write(&self) -> Result<()> {
        if let Self::Task(_, state) = self {
            let mut state = lock_ignoring_poison(state);
            // 此锁也保护 HTTP 取消。先进入写入的任务必须完成提交或回滚。
            self.check()?;
            state["phase"] = "write".into();
        }
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct Tasks {
    active: Option<(Task, Arc<Mutex<Value>>)>,
    previous: Option<Value>,
}

impl Tasks {
    pub(crate) fn restore(&mut self, previous: Option<Value>) {
        self.previous = previous;
    }

    pub(crate) fn running(&self) -> bool {
        self.active.as_ref().is_some_and(|(task, _)| task.running())
    }

    pub(crate) fn stop(&mut self) {
        if let Some((task, _)) = &mut self.active {
            task.stop();
        }
    }

    pub(crate) fn snapshot(&self) -> Value {
        let Some((task, state)) = &self.active else { return self.previous.clone().unwrap_or(Value::Null) };
        let mut snap = lock_ignoring_poison(state).clone();
        if snap["status"] == "running" && !task.running() {
            if let TaskOutcome::Failed(message) = task.outcome() {
                snap["status"] = "failed".into();
                snap["message"] = message.into();
            }
        }
        snap
    }

    /// 取消扫描阶段的任务。任务已经结束（取消和完成擦肩而过）时什么都不做，页面读到的是结束后的状态；只有写入阶段
    /// 回 409（#559 走查）。
    pub(crate) fn cancel(&self, id: &str) -> Result<()> {
        if let Some((task, state)) = &self.active {
            let snap = lock_ignoring_poison(state);
            if snap["id"] == id && snap["status"] == "running" {
                if snap["phase"] != "scan" {
                    return Err(Error::Conflict("正在写入图库，无法取消".into()));
                }
                task.context().cancellation().store(true, Ordering::SeqCst);
                return Ok(());
            }
        }
        Ok(())
    }

    pub(crate) fn start_for(&mut self, library: &Library, identity: TaskIdentity, operation: Action) -> Result<Value> {
        if self.running() {
            return Err(Error::Conflict("已有 WD 角色扩展任务正在进行".into()));
        }
        let (kind, phase) = match &operation {
            Action::Enable(..) | Action::Pack { .. } => ("enable", "scan"),
            Action::Withdraw | Action::PackWithdraw { .. } => ("withdraw", "write"),
        };
        // `part` 是角色资料包正在处理的组成部分（`wd`、`ccip`），只给 WD 部分的旧任务也写 `wd`。
        let initial = json!({
            "id":identity.task_id,"kind":kind,"phase":phase,"status":"running","part":"wd",
            "done":0,"total":0,"message":null,"result":null,"diagnostic":null
        });
        let state = Arc::new(Mutex::new(initial.clone()));
        let shared = state.clone();
        let library = library.clone();
        let task = Task::start(identity, None, move |ctx| {
            let progress = Progress::Task(&ctx, &shared);
            // 角色资料包分两步提交：WD 部分提交以后 CCIP 部分失败或取消时，结果里留着 WD 部分的（`result.wd`）。
            let mut done = Value::Null;
            let result = match operation {
                Action::Enable(pack, path) => check_file(&library, &pack, &path, &progress),
                Action::Withdraw => withdraw(&library),
                Action::Pack { root, wd, ccip } => pack_enable(&library, &root, wd, ccip, &progress, &mut done),
                Action::PackWithdraw { wd, ccip } => pack_withdraw(&library, wd, ccip, &progress, &mut done),
            };
            let mut state = lock_ignoring_poison(&shared);
            if !done.is_null() {
                state["result"] = done;
            }
            match result {
                Ok(result) => {
                    state["status"] = "done".into();
                    state["result"] = result;
                }
                Err(_) if state["phase"] == "scan" && progress.check().is_err() => {
                    state["status"] = "cancelled".into();
                    state["message"] = "WD 角色扩展检查已取消".into();
                }
                Err(error) => {
                    use crate::diagnostics::{self, Operation, Publication, Stage};
                    let stage = if state["phase"] == "scan" { Stage::Reading } else { Stage::Writing };
                    let d = diagnostics::failure(Operation::Models, stage, &error, Publication::NotCommitted);
                    state["status"] = "failed".into();
                    state["message"] = match error {
                        Error::Conflict(message) | Error::Invalid(message) | Error::Schema(message) => message.into(),
                        _ => "WD 角色扩展任务失败，请查看诊断信息".into(),
                    };
                    state["diagnostic"] = d.value();
                }
            }
            Ok(())
        });
        self.previous = None;
        self.active = Some((task, state));
        Ok(initial)
    }
}

pub(crate) enum Action {
    /// 用这个版本检查整个图库：启用、重新检查，或换成另一个版本（#602）。
    Enable(Box<Pack>, PathBuf),
    Withdraw,
    /// 角色资料包（#602）：先用 WD 部分、再用 CCIP 部分检查整个图库，各自提交。图库里还没有对应向量的组成部分
    /// （`Part::scan` 为假）只记下启用了哪个文件，等识别结果交付时再检查。
    Pack {
        root: PathBuf,
        wd: Option<Box<Part>>,
        ccip: Option<Box<Part>>,
    },
    /// 撤回角色资料包：两个组成部分里启用了的都撤回。
    PackWithdraw {
        wd: bool,
        ccip: bool,
    },
}

/// 角色资料包的一个组成部分：这个文件，和要不要现在检查（图库里有这一种向量时）。
pub(crate) struct Part {
    pub(crate) pack: Pack,
    pub(crate) path: PathBuf,
    pub(crate) scan: bool,
}

impl Progress<'_> {
    /// 换到角色资料包的下一个组成部分：回到扫描阶段，计数清零。
    fn part(&self, part: &str) -> Result<()> {
        self.check()?;
        if let Self::Task(_, state) = self {
            let mut state = lock_ignoring_poison(state);
            state["part"] = part.into();
            state["phase"] = "scan".into();
            state["done"] = 0.into();
            state["total"] = 0.into();
        }
        Ok(())
    }
}

/// CCIP 长尾部分（`app_ccip_longtail::apply`）的进度报告到同一个任务上。
impl crate::app_imports::Observer for Progress<'_> {
    fn stage(&self, stage: &'static str) {
        if stage == "committing" {
            // 取消在这之前到达时，apply 提交前的检查会停下；这里只把阶段改成写入。
            let _ = self.begin_write();
        }
    }
    fn count(&self, done: u64, total: u64) {
        let _ = Progress::count(self, done as usize, total as usize);
    }
}

fn changed(result: &Value) -> u64 {
    result["changed"].as_u64().unwrap_or(0)
}

fn pack_enable(
    library: &Library,
    root: &Path,
    wd: Option<Box<Part>>,
    ccip: Option<Box<Part>>,
    progress: &Progress<'_>,
    done: &mut Value,
) -> Result<Value> {
    let mut out = json!({"wd":null,"ccip":null,"changed":0,"hits":0});
    if let Some(part) = wd {
        progress.part("wd")?;
        let result = if part.scan {
            check_file(library, &part.pack, &part.path, progress)?
        } else {
            mark_enabled(library, &part.pack)?
        };
        out["changed"] = (changed(&out) + changed(&result)).into();
        out["hits"] = (out["hits"].as_u64().unwrap_or(0) + result["hits"].as_u64().unwrap_or(0)).into();
        out["wd"] = result;
        *done = out.clone();
    }
    if let Some(part) = ccip {
        progress.part("ccip")?;
        let result = if part.scan {
            crate::app_ccip_longtail::apply(library, root, &part.pack, &part.path, &|| progress.check(), progress)?
        } else {
            crate::app_ccip_longtail::mark_enabled(library, root, &part.pack)?
        };
        out["changed"] = (changed(&out) + changed(&result)).into();
        out["hits"] = (out["hits"].as_u64().unwrap_or(0) + result["hits"].as_u64().unwrap_or(0)).into();
        out["ccip"] = result;
    }
    set_declined(library, false)?;
    Ok(out)
}

fn pack_withdraw(library: &Library, wd: bool, ccip: bool, progress: &Progress<'_>, done: &mut Value) -> Result<Value> {
    let mut out = json!({"wd":null,"ccip":null,"changed":0});
    if wd {
        let result = withdraw(library)?;
        out["changed"] = changed(&result).into();
        out["wd"] = result;
        *done = out.clone();
    }
    if ccip {
        if let Progress::Task(_, state) = progress {
            lock_ignoring_poison(state)["part"] = "ccip".into();
        }
        let result = crate::app_ccip_longtail::withdraw(library, progress)?;
        out["changed"] = (changed(&out) + changed(&result)).into();
        out["ccip"] = result;
    }
    // 撤回以后不再默认启用，也不再提示。
    set_declined(library, true)?;
    Ok(out)
}

/// 还没有 WD 向量的图库启用角色资料包（新图库默认启用，#602）：只记下用哪个文件，不检查，没有证据要写。之后识别结果
/// 交付时按图检查；`scored` 为 0、`pending` 为真，表示这个文件还没在整库上检查过。
pub(crate) fn mark_enabled(library: &Library, pack: &Pack) -> Result<Value> {
    ensure_writable(library)?;
    let mut app = writer(&library.db())?;
    crate::schema::require_current(&app)?;
    let tx = app.transaction_with_behavior(TransactionBehavior::Immediate)?;
    crate::schema::begin_writing(&tx)?;
    let current = enabled(&tx)?;
    let now: String = tx.query_row("SELECT datetime('now')", [], |r| r.get(0))?;
    let changed = if current.as_ref().is_some_and(|state| state.package_sha256 != pack.file.sha256) {
        // 换了文件：旧文件给的候选不能留着冒充新文件的。
        wd_extension::withdraw(&tx)?
    } else {
        0
    };
    let entry = Enabled {
        package_sha256: pack.file.sha256.clone(),
        enabled_at: current.map_or_else(|| now.clone(), |state| state.enabled_at),
        checked_at: now,
        scored: 0,
    };
    save_enabled(&tx, &entry)?;
    crate::schema::commit_writing(tx)?;
    Ok(json!({"package_sha256":pack.file.sha256,"scored":0,"hits":0,"changed":changed}))
}

fn check_file(library: &Library, pack: &Pack, path: &Path, progress: &Progress<'_>) -> Result<Value> {
    progress.check()?;
    apply_loaded(library, &Package::load(path, pack)?, &pack.file.sha256, progress)
}

/// 卡片上的角色资料包里的 WD 部分（`app_packs::library_pack`）。
pub(crate) fn card_pack(resources: &Path, library: &Library) -> Result<Pack> {
    Ok(crate::app_packs::for_library(resources, library)?.card.wd)
}

pub(crate) fn status(resources: &Path, library: &Library, tasks: &Tasks, busy: Option<&str>) -> Result<Value> {
    // 先看任务是否已经提交，再读应用库快照。看到终态时，库里的开关和计数必须已经生效。
    let task = tasks.snapshot();
    let mut info = crate::app_models::extension_info(resources, &card_pack(resources, library)?)?;
    let app = crate::app_library::read_connection(&library.db())?;
    let snapshot = app.unchecked_transaction()?;
    let source = crate::app_library::read_connection(&library.source())?;
    let vectors = vector_count(&source, None)?;
    let active = enabled(&snapshot)?;
    let (images, assigned): (usize, usize) = snapshot.query_row(
        "SELECT (SELECT count(DISTINCT uuid) FROM candidates WHERE source='WD_EXT'),
                (SELECT count(*) FROM memberships WHERE sources='WD_EXT')",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let blocked = if library.frozen() {
        Some("frozen")
    } else if busy.is_some() {
        busy
    } else if vectors == 0 {
        Some("no_vectors")
    } else {
        None
    };
    info["library"] = json!({
        "enabled":active.is_some(),
        "package_sha256":active.as_ref().map(|state| &state.package_sha256),
        "enabled_at":active.as_ref().map(|state| &state.enabled_at),
        "checked_at":active.as_ref().map(|state| &state.checked_at),
        "scored":active.as_ref().map_or(0, |state| state.scored),
        "images":images,"assigned":assigned,"vectors":vectors,"blocked":blocked
    });
    info["task"] = task;
    Ok(info)
}

/// 同步任务固定的一份 WD 扩展包。批次沿用手动检查的评分与证据写入，投影提交前复核启用版本。
pub(crate) struct Delivery {
    sha256: String,
    package: Package,
}

impl Delivery {
    pub(crate) fn sha256(&self) -> &str {
        &self.sha256
    }

    pub(crate) fn load(resources: &Path, library: &Library) -> Result<Option<Self>> {
        let app = crate::app_library::read_connection(&library.db())?;
        let Some((pack, path)) = enabled_pack(resources, &app)? else { return Ok(None) };
        let package = Package::load(&path, &pack)?;
        Ok(Some(Self { sha256: pack.file.sha256, package }))
    }

    pub(crate) fn score(
        &self,
        src: &Connection,
        digest: &str,
        manifest: &str,
        ids: &[String],
        cancel: &AtomicBool,
    ) -> Result<Option<(usize, Vec<Hit>)>> {
        if let Err(error) = self.package.compatible(manifest) {
            eprintln!("WD 扩展跳过不兼容的识别交付：{error}");
            return Ok(None);
        }
        let scope = json!(ids).to_string();
        scan(src, &self.package, Some(&scope), Some(digest), &Progress::Delivery(cancel)).map(Some)
    }

    pub(crate) fn publish(&self, app: &Connection, ids: &[String], (checked, hits): (usize, Vec<Hit>)) -> Result<()> {
        let Some(mut active) = enabled(app)?.filter(|active| active.package_sha256 == self.sha256) else {
            return Ok(());
        };
        wd_extension::apply_scoped(app, &self.sha256, ids, &hits)?;
        // 每次识别投影只会提交一次，已发布的回执恢复通知时不再累加。
        active.scored += checked;
        save_enabled(app, &active)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
        library: Library,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "illustash-wd-extension-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let images = root.join("images");
            fs::create_dir_all(&images).unwrap();
            let dest = root.join("test.pilib");
            crate::app_create::create_folder(&dest, &images, None, None, None).unwrap();
            let library = Library::open(&dest).unwrap();
            Self { root, library }
        }

        fn write(&self, f: impl FnOnce(&Connection) -> Result<()>) -> Result<()> {
            let mut app = writer(&self.library.db())?;
            let tx = app.transaction_with_behavior(TransactionBehavior::Immediate)?;
            crate::schema::begin_writing(&tx)?;
            f(&tx)?;
            crate::schema::commit_writing(tx)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }

    fn snapshot(c: &Connection, sql: &str) -> Vec<Value> {
        crate::app_migrate_data::rows(c, sql).unwrap()
    }

    /// 合成包：每个头只看向量的第 `axis` 维（权重 2），阈值和级别照给的。
    fn synthetic(heads: &[(&str, usize, f64, bool)]) -> Package {
        let mut weights = vec![0.; DIM * heads.len()];
        for (i, (_, axis, _, _)) in heads.iter().enumerate() {
            weights[i * DIM + axis] = 2.;
        }
        Package {
            wd_model_sha256: "model".into(),
            wd_tags_sha256: "tags".into(),
            labels: heads.iter().map(|h| h.0.to_owned()).collect(),
            thresholds: heads.iter().map(|h| h.2).collect(),
            candidate_only: heads.iter().map(|h| h.3).collect(),
            weights,
            bias: vec![0.; heads.len()],
        }
    }

    fn pack(format: &str, heads: usize, candidate_only: usize, bytes: &[u8]) -> Pack {
        Pack {
            kind: crate::app_packs::Kind::Wd,
            revision: 2,
            version: "wd-extension-v2".into(),
            format: format.into(),
            heads,
            candidate_only,
            file: crate::app_packs::File {
                name: "synthetic.bin".into(),
                size: bytes.len() as u64,
                sha256: crate::strict_json::sha256_hex(bytes),
                url: String::new(),
            },
            shards: None,
            base: Base::Wd { model: "model".into(), tags: "tags".into() },
            license: "Apache-2.0".into(),
            builtin: false,
        }
    }

    fn encode(manifest: Value, heads: usize) -> Vec<u8> {
        let mut input = manifest.to_string().into_bytes();
        input.push(b'\n');
        input.resize(input.len() + heads * (DIM + 1) * 4, 0);
        input
    }

    #[test]
    fn package_formats_threshold_layouts_and_tiers() {
        let legacy = 0.8008725047111511;
        let high = f64::from(0.99_f32);
        let labels = |n: usize| (0..n).map(|i| format!("test_{i}")).collect::<Vec<_>>();
        let base = |format: &str, n: usize| json!({"format":format,"wd_model_sha256":"model","wd_tags_sha256":"tags","labels":labels(n)});
        // 内置 v1：统一阈值，全部一级。
        let mut v1 = base(WD_FORMAT_V1, 3);
        v1["threshold"] = legacy.into();
        let bytes = encode(v1.clone(), 3);
        let decoded = Package::decode(&bytes, &pack(WD_FORMAT_V1, 3, 0, &bytes)).unwrap();
        assert_eq!((decoded.thresholds, decoded.candidate_only), (vec![legacy; 3], vec![false; 3]));
        // #551 的 257 头包：旧格式另带逐头阈值，仍然全部一级。
        let mut v257 = v1.clone();
        v257["thresholds"] = json!([legacy, legacy, high]);
        let bytes = encode(v257.clone(), 3);
        let decoded = Package::decode(&bytes, &pack(WD_FORMAT_V1, 3, 0, &bytes)).unwrap();
        assert_eq!(decoded.thresholds, vec![legacy, legacy, high]);
        assert_eq!(decoded.candidate_only, vec![false; 3]);
        // 新格式：逐头阈值和级别都必须有，二级头数要和目录一致。
        let mut v2 = base(WD_FORMAT_V2, 3);
        v2["thresholds"] = json!([legacy, high, 0.5]);
        v2["tiers"] = json!([1, 2, 2]);
        let bytes = encode(v2.clone(), 3);
        let decoded = Package::decode(&bytes, &pack(WD_FORMAT_V2, 3, 2, &bytes)).unwrap();
        assert_eq!(decoded.candidate_only, vec![false, true, true]);
        assert!(Package::decode(&bytes, &pack(WD_FORMAT_V2, 3, 1, &bytes)).is_err());
        assert!(Package::decode(&bytes, &pack(WD_FORMAT_V1, 3, 2, &bytes)).is_err());

        let rejected = |manifest: Value, format: &str, candidate_only: usize| {
            let bytes = encode(manifest, 3);
            Package::decode(&bytes, &pack(format, 3, candidate_only, &bytes)).is_err()
        };
        let mut tiered_v1 = v257.clone();
        tiered_v1["tiers"] = json!([1, 1, 1]);
        assert!(rejected(tiered_v1, WD_FORMAT_V1, 0), "旧格式不能带级别");
        let mut untiered = v2.clone();
        untiered.as_object_mut().unwrap().remove("tiers");
        assert!(rejected(untiered, WD_FORMAT_V2, 2));
        let mut bad_tier = v2.clone();
        bad_tier["tiers"] = json!([1, 3, 2]);
        assert!(rejected(bad_tier, WD_FORMAT_V2, 2));
        let mut short = v2.clone();
        short["thresholds"] = json!([legacy]);
        assert!(rejected(short, WD_FORMAT_V2, 2));
        let mut out_of_range = v257.clone();
        out_of_range["thresholds"] = json!([legacy, 1.5, legacy]);
        assert!(rejected(out_of_range, WD_FORMAT_V1, 0));
        let mut other_base = v1.clone();
        other_base["wd_tags_sha256"] = "other".into();
        assert!(rejected(other_base, WD_FORMAT_V1, 0), "基础标签表不符");
        let bytes = encode(v1, 3);
        assert!(Package::decode(&bytes[..bytes.len() - 1], &pack(WD_FORMAT_V1, 3, 0, &bytes)).is_err());
        assert!(Package::decode(&bytes, &pack(WD_FORMAT_V1, 2, 0, &bytes)).is_err(), "头数不符");

        // 文件本身要和这个版本的大小、SHA-256 对上。
        let f = Fixture::new();
        let path = f.root.join("package.bin");
        fs::write(&path, &bytes).unwrap();
        let expected = pack(WD_FORMAT_V1, 3, 0, &bytes);
        assert!(Package::load(&path, &expected).is_ok());
        let mut tampered = bytes.clone();
        *tampered.last_mut().unwrap() ^= 1;
        fs::write(&path, &tampered).unwrap();
        assert!(Package::load(&path, &expected).is_err());
    }

    /// 带级别的 `/2` 包经过 `scan` 的批量打分（#582），逐图和改前的逐图逐头标量实现比：命中、概率的每一位、级别和顺序
    /// 都相同。1,300 张图跨两块；偏置调成让多数头的 z 落在各自阈值的 logit 附近。
    #[test]
    fn tiered_package_scan_matches_scalar_hits() {
        let (images, heads) = (1300usize, 37usize);
        let mut state = 0x582_602_u64;
        let mut unit = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 40) as f32 / (1u64 << 23) as f32 - 1.0
        };
        let base: Vec<f32> = (0..DIM).map(|_| unit() * 2.0).collect();
        let vectors: Vec<Vec<f32>> =
            (0..images).map(|i| base.iter().map(|&v| v + unit() * [0.0, 1e-6, 1e-3, 0.3][i % 4]).collect()).collect();
        let weights: Vec<f32> = (0..heads * DIM).map(|_| unit() * 0.5).collect();
        let thresholds: Vec<f64> = (0..heads).map(|h| [0.8008725047111511, f64::from(0.99_f32), 0.5][h % 3]).collect();
        let bias: Vec<f32> = (0..heads)
            .map(|h| {
                let s: f64 = base.iter().zip(&weights[h * DIM..]).map(|(&x, &w)| f64::from(x) * f64::from(w)).sum();
                let logit = (thresholds[h] / (1.0 - thresholds[h])).ln();
                (logit - s + f64::from(unit()) * [0.0, 1e-6, 1e-3][h % 3]) as f32
            })
            .collect();
        let tiers: Vec<u8> = (0..heads).map(|h| if h % 4 == 1 { 2 } else { 1 }).collect();
        let labels: Vec<String> = (0..heads).map(|h| format!("role_{h:02}")).collect();
        let manifest = json!({"format":WD_FORMAT_V2,"wd_model_sha256":"model","wd_tags_sha256":"tags",
            "labels":labels,"thresholds":thresholds,"tiers":tiers});
        let mut bytes = manifest.to_string().into_bytes();
        bytes.push(b'\n');
        bytes.extend(weights.iter().chain(&bias).flat_map(|v| v.to_le_bytes()));
        let second = tiers.iter().filter(|&&t| t == 2).count();
        let package = Package::decode(&bytes, &pack(WD_FORMAT_V2, heads, second, &bytes)).unwrap();

        let f = Fixture::new();
        let source = writer(&f.library.source()).unwrap();
        source
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS embeddings(uuid TEXT,model TEXT,vec BLOB,PRIMARY KEY(uuid,model))",
            )
            .unwrap();
        let ids: Vec<String> = (0..images).map(|i| format!("image-{i:04}")).collect();
        for (uuid, vector) in ids.iter().zip(&vectors) {
            source
                .execute(
                    "INSERT INTO assets(uuid,filename,src_path,width,height,added_at,src_kind,in_scope,trashed)
                     VALUES (?1,'synthetic.png','synthetic.png',8,8,'2026-11-18','folder',1,0)",
                    [uuid],
                )
                .unwrap();
            let blob = vector.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>();
            source.execute("INSERT INTO embeddings VALUES (?1,'wd',?2)", rusqlite::params![uuid, blob]).unwrap();
        }
        let (checked, hits) = scan(&source, &package, None, None, &Progress::silent()).unwrap();
        assert_eq!(checked, images);

        // 改前的逐图逐头实现（#614 的 `Package::hits`），按 `scan` 的 uuid 顺序。
        let mut expected = Vec::new();
        for (uuid, vector) in ids.iter().zip(&vectors) {
            for h in 0..heads {
                let w = &weights[h * DIM..(h + 1) * DIM];
                let sum: f64 = vector.iter().zip(w).map(|(&x, &w)| f64::from(x) * f64::from(w)).sum();
                let probability = (1.0 / (1.0 + (-(sum + f64::from(bias[h]))).exp())) as f32;
                if f64::from(probability) >= thresholds[h] {
                    expected.push((uuid.clone(), labels[h].clone(), f64::from(probability).to_bits(), tiers[h] == 2));
                }
            }
        }
        let actual: Vec<_> =
            hits.iter().map(|h| (h.uuid.clone(), h.tag.clone(), h.probability.to_bits(), h.candidate_only)).collect();
        assert!(expected.iter().any(|e| e.3) && expected.iter().any(|e| !e.3), "两级都要有命中");
        assert!(expected.len() < images * heads, "要有不命中的头");
        assert_eq!(actual, expected);
    }

    /// v1 → 新版本 → v1 → 撤回：换包沿用同一个 WD_EXT 来源的替换，图库记下用的是哪个包；二级头只出候选。
    #[test]
    fn switching_versions_round_trips_and_candidate_only_heads_never_assign() {
        let f = Fixture::new();
        let assets = ["one", "two", "three"];
        let source = writer(&f.library.source()).unwrap();
        source
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS embeddings(uuid TEXT,model TEXT,vec BLOB,PRIMARY KEY(uuid,model))",
            )
            .unwrap();
        for (i, uuid) in assets.iter().enumerate() {
            let mut wd = vec![0f32; DIM];
            wd[i] = 1.;
            let vector = wd.into_iter().flat_map(f32::to_le_bytes).collect::<Vec<_>>();
            source
                .execute(
                    "INSERT INTO assets(uuid,filename,src_path,width,height,added_at,src_kind,in_scope,trashed)
                     VALUES (?1,'synthetic.png','synthetic.png',8,8,'2026-11-17','folder',1,0)",
                    [uuid],
                )
                .unwrap();
            source.execute("INSERT INTO embeddings VALUES (?1,'wd',?2)", rusqlite::params![uuid, vector]).unwrap();
        }
        f.write(|tx| {
            let rows = assets
                .iter()
                .map(|uuid| json!([uuid, "synthetic.png", "synthetic.png", 8, 8, "2026-11-17", "folder"]))
                .collect::<Vec<_>>();
            crate::character_commands::command(
                tx,
                &json!({"op":"refresh","assets":rows,"indexed":"2026-11-17","index":{},"labels":[],"decisions":[],
                    "duplicates":[]}),
            )
            .map(|_| ())
        })
        .unwrap();
        let app = writer(&f.library.db()).unwrap();
        let members = "SELECT uuid,entity,sources FROM memberships ORDER BY uuid";
        let candidates = "SELECT uuid,entity,source FROM candidates ORDER BY uuid,entity";
        let active = "SELECT id,asset_id,term_id,version,score FROM term_current_evidence ORDER BY id";
        let baseline = (snapshot(&app, members), snapshot(&app, candidates), snapshot(&app, active));
        let state = || enabled(&app).unwrap().map(|e| e.package_sha256);
        let key = |uuid: &str| -> String {
            app.query_row("SELECT entity FROM candidates WHERE uuid=?1 AND source='WD_EXT'", [uuid], |r| r.get(0))
                .unwrap()
        };

        // v1：一个头，只认第一张图。
        let v1 = synthetic(&[("role_one", 0, 0.8, false)]);
        assert_eq!(apply_loaded(&f.library, &v1, "v1", &Progress::silent()).unwrap()["changed"], 1);
        assert_eq!(state().as_deref(), Some("v1"));
        let after_v1 = (snapshot(&app, members), snapshot(&app, candidates), snapshot(&app, active));
        let one = key("one");
        assert_eq!(after_v1.0, vec![json!(["one", one, "WD_EXT"])]);

        // 新版本：原来的头加一个一级头（第二张图）和一个二级头（第三张图）。二级头只进候选，不补归属。
        let v2 = synthetic(&[("role_one", 0, 0.8, false), ("role_two", 1, 0.8, false), ("role_three", 2, 0.8, true)]);
        assert_eq!(apply_loaded(&f.library, &v2, "v2", &Progress::silent()).unwrap()["changed"], 3);
        assert_eq!(state().as_deref(), Some("v2"));
        let (two, three) = (key("two"), key("three"));
        assert_eq!(snapshot(&app, members), vec![json!(["one", one, "WD_EXT"]), json!(["two", two, "WD_EXT"])]);
        assert_eq!(
            snapshot(&app, candidates),
            vec![json!(["one", one, "WD_EXT"]), json!(["three", three, "WD_EXT"]), json!(["two", two, "WD_EXT"])]
        );
        let tier: Vec<Value> = snapshot(
            &app,
            "SELECT json_extract(data,'$.tag'),json_extract(data,'$.tier') FROM term_current_evidence
             WHERE source='wd_extension' ORDER BY 1",
        );
        assert_eq!(tier, vec![json!(["role_one", null]), json!(["role_three", 2]), json!(["role_two", null])]);
        // 同一个一级头在两个版本里分数相同，但证据行按包分开记。
        assert_eq!(
            snapshot(&app, "SELECT DISTINCT version FROM term_current_evidence WHERE source='wd_extension'").len(),
            3
        );

        // 之后别的操作按图或整库重算归属时，二级候选照样不补归属。
        f.write(|tx| {
            illustash_core::character_projection::memberships(tx, Some(&["three".to_owned()]))?;
            illustash_core::character_projection::memberships(tx, None)
        })
        .unwrap();
        assert_eq!(snapshot(&app, members).len(), 2);

        // 换回 v1：恢复成 v1 那时的样子（证据行复用，没有新行）。
        let rows = snapshot(&app, "SELECT count(*) FROM term_evidence").pop();
        assert_eq!(apply_loaded(&f.library, &v1, "v1", &Progress::silent()).unwrap()["changed"], 3);
        assert_eq!(state().as_deref(), Some("v1"));
        assert_eq!((snapshot(&app, members), snapshot(&app, candidates), snapshot(&app, active)), after_v1);
        assert_eq!(snapshot(&app, "SELECT count(*) FROM term_evidence").pop(), rows);

        // 撤回：回到启用之前，历史证据保留。
        assert_eq!(withdraw(&f.library).unwrap()["changed"], 1);
        assert_eq!(state(), None);
        assert_eq!((snapshot(&app, members), snapshot(&app, candidates), snapshot(&app, active)), baseline);
        assert_eq!(snapshot(&app, "SELECT id FROM term_evidence WHERE source='wd_extension'").len(), 4);
    }

    #[test]
    fn cancel_stops_scan_and_write_boundary_refuses_it() {
        let f = Fixture::new();
        for in_write in [false, true] {
            let identity = TaskIdentity::new(&f.library, 1, "synthetic").unwrap();
            let id = identity.task_id.clone();
            let state = Arc::new(Mutex::new(json!({"id":id,"status":"running","phase":"scan"})));
            let shared = state.clone();
            let (ready, entered) = std::sync::mpsc::channel();
            let (release, resume) = std::sync::mpsc::channel();
            let task = Task::start(identity, None, move |ctx| {
                let progress = Progress::Task(&ctx, &shared);
                if in_write {
                    progress.begin_write()?;
                }
                ready.send(()).unwrap();
                resume.recv_timeout(crate::test_wait::limit()).unwrap();
                progress.check()
            });
            let mut tasks = Tasks { active: Some((task, state)), previous: None };
            entered.recv_timeout(crate::test_wait::limit()).unwrap();
            let cancelled = tasks.cancel(&id);
            release.send(()).unwrap();
            let task = &mut tasks.active.as_mut().unwrap().0;
            assert!(task.wait(crate::test_wait::limit()));
            if in_write {
                assert!(matches!(cancelled, Err(Error::Conflict(_))));
                assert!(matches!(task.outcome(), TaskOutcome::Done));
            } else {
                assert!(cancelled.is_ok());
                assert!(matches!(task.outcome(), TaskOutcome::Cancelled));
            }
        }
    }

    #[test]
    fn shared_projection_keeps_existing_extension_until_it_is_withdrawn() {
        use illustash_core::ccip_longtail;
        let f = Fixture::new();
        let asset = "overlap".to_owned();
        f.write(|tx| {
            crate::character_commands::command(
                tx,
                &json!({"op":"refresh","assets":[[asset,"sample.png","sample.png",8,8,"2026-11-16","folder"]],
                    "indexed":"2026-11-16","index":{},"labels":[],"decisions":[],"duplicates":[]}),
            )?;
            let a = [ccip_longtail::Hit { uuid: asset.clone(), tag: "longtail_role".into(), score: 0.85 }];
            let b =
                [Hit { uuid: asset.clone(), tag: "extension_role".into(), probability: 0.99, candidate_only: false }];
            let scope = std::slice::from_ref(&asset);
            let sources = || snapshot(tx, "SELECT sources FROM memberships WHERE uuid='overlap'");
            let candidates = || snapshot(tx, "SELECT source FROM candidates WHERE uuid='overlap' ORDER BY source");

            assert_eq!(ccip_longtail::apply(tx, "synthetic-a", &a)?, 1);
            assert_eq!(wd_extension::apply_scoped(tx, "synthetic-b", scope, &b)?, 1);
            assert_eq!(sources(), vec![json!(["CCIP_LONGTAIL"])]);
            assert_eq!(candidates(), vec![json!(["CCIP_LONGTAIL"]), json!(["WD_EXT"])]);

            assert_eq!(ccip_longtail::withdraw(tx)?, 1);
            assert_eq!(sources(), vec![json!(["WD_EXT"])]);
            assert_eq!(ccip_longtail::apply(tx, "synthetic-a", &a)?, 1);
            assert_eq!(sources(), vec![json!(["WD_EXT"])]);
            assert_eq!(wd_extension::withdraw(tx)?, 1);
            assert_eq!(sources(), vec![json!(["CCIP_LONGTAIL"])]);
            assert_eq!(candidates(), vec![json!(["CCIP_LONGTAIL"])]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn catalog_joins_renamed_tags_and_attaches_costumes_on_import() {
        let columns = r#"["id","tag","tags","parent","relation","action","variant","bangumi","pick","wikidata"]"#;
        let rows = r#"[1,"base_(w)",["base_old"],null,null,null,null,null,"",null],
            [2,"base_(swimsuit)_(w)",[],1,"appearance","auto","swimsuit",null,"",null],
            [3,"base_(alt)_(w)",[],1,"form","ask",null,null,"",null],
            [4,"lonely_(swimsuit)_(w)",[],5,"appearance","auto","swimsuit",null,"",null],
            [5,"lonely_(w)",[],null,null,null,null,null,"",null]"#;
        let doc = format!(
            r#"{{"format":"{}","revision":1,"columns":{columns},"characters":[{rows}],"works":[]}}"#,
            illustash_core::character_catalog::FORMAT
        );
        let catalog = Box::leak(Box::new(illustash_core::character_catalog::Catalog::parse(&doc).unwrap()));
        illustash_core::character_catalog::override_for_thread(Some(catalog));
        let f = Fixture::new();
        let assets = ["old", "new", "swim", "alt", "lonely"];
        f.write(|tx| {
            let rows: Vec<Value> =
                assets.iter().map(|u| json!([u, "s.png", "s.png", 8, 8, "2026-11-23", "folder"])).collect();
            crate::character_commands::command(
                tx,
                &json!({"op":"refresh","assets":rows,"indexed":"2026-11-23","index":{},
                    "labels":[["old","base_old",0.9,"character"]],"decisions":[],"duplicates":[]}),
            )?;
            let hits: Vec<Hit> = [
                ("new", "base_(w)"),
                ("swim", "base_(swimsuit)_(w)"),
                ("alt", "base_(alt)_(w)"),
                ("lonely", "lonely_(swimsuit)_(w)"),
            ]
            .iter()
            .map(|(u, t)| Hit { uuid: (*u).into(), tag: (*t).into(), probability: 0.99, candidate_only: false })
            .collect();
            let scope: Vec<String> = assets.iter().map(|s| (*s).to_owned()).collect();
            wd_extension::apply_scoped(tx, "synthetic-catalog", &scope, &hits)?;
            let owner = |key: &str| -> rusqlite::Result<String> {
                tx.query_row(
                    &format!("SELECT l.entity_id FROM {} l WHERE l.key=?", illustash_core::character_links::LINKS),
                    [key],
                    |r| r.get(0),
                )
            };
            let tree = |id: &str| -> rusqlite::Result<(Option<String>, Option<String>)> {
                tx.query_row(
                    &format!(
                        "SELECT parent_id,variant FROM {} e WHERE e.id=?",
                        illustash_core::character_entities::ENTITIES
                    ),
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
            };
            let base = owner("base_old")?;
            // 各标签保留原始证据端点，但界面归到同一角色。
            let canonical = owner("base_(w)")?;
            assert_ne!(canonical, base);
            assert_eq!(crate::character_commands::resolve(tx, &base)?, canonical);
            // 泳装自动挂到本体下；形态照旧是顶层角色。
            assert_eq!(tree(&owner("base_(swimsuit)_(w)")?)?, (Some(canonical), Some("swimsuit".into())));
            assert_eq!(tree(&owner("base_(alt)_(w)")?)?.0, None);
            // 本体还没有时，先建一个本体再挂上去。
            let lonely = owner("lonely_(w)")?;
            assert_eq!(tree(&owner("lonely_(swimsuit)_(w)")?)?.0, Some(lonely));
            let record: String =
                tx.query_row("SELECT value FROM meta WHERE key='character_catalog'", [], |r| r.get(0))?;
            assert_eq!(serde_json::from_str::<Value>(&record).unwrap()["attached"].as_array().unwrap().len(), 2);
            Ok(())
        })
        .unwrap();
        illustash_core::character_catalog::override_for_thread(None);
    }

    fn catalog_with(rows: &str) -> &'static illustash_core::character_catalog::Catalog {
        catalog_revision(rows, 1)
    }

    fn catalog_revision(rows: &str, revision: u32) -> &'static illustash_core::character_catalog::Catalog {
        let columns = r#"["id","tag","tags","parent","relation","action","variant","bangumi","pick","wikidata"]"#;
        let doc = format!(
            r#"{{"format":"{}","revision":{revision},"columns":{columns},"characters":[{rows}],"works":[]}}"#,
            illustash_core::character_catalog::FORMAT
        );
        Box::leak(Box::new(illustash_core::character_catalog::Catalog::parse(&doc).unwrap()))
    }

    #[test]
    fn catalog_apply_merges_renamed_duplicates_and_attaches_costumes_once() {
        // 先在没有目录的情况下导入：旧 tag、新 tag、泳装各建一个顶层角色，和 0.4.0 的图库一样。
        illustash_core::character_catalog::override_for_thread(Some(catalog_with("")));
        let f = Fixture::new();
        let assets = ["old", "new", "swim", "named_old", "named_new", "missing_base"];
        f.write(|tx| {
            let rows: Vec<Value> =
                assets.iter().map(|u| json!([u, "s.png", "s.png", 8, 8, "2026-11-23", "folder"])).collect();
            crate::character_commands::command(
                tx,
                &json!({"op":"refresh","assets":rows,"indexed":"2026-11-23","index":{},
                    "labels":[["old","base_old",0.9,"character"],["named_old","named_old",0.9,"character"]],
                    "decisions":[],"duplicates":[]}),
            )?;
            let hits: Vec<Hit> = [
                ("new", "base_(w)"),
                ("swim", "base_(swimsuit)_(w)"),
                ("named_new", "named_(w)"),
                ("missing_base", "missing_(swimsuit)_(w)"),
            ]
            .iter()
            .map(|(u, t)| Hit { uuid: (*u).into(), tag: (*t).into(), probability: 0.99, candidate_only: false })
            .collect();
            let scope: Vec<String> = assets.iter().map(|s| (*s).to_owned()).collect();
            wd_extension::apply_scoped(tx, "synthetic-catalog", &scope, &hits)?;
            Ok(())
        })
        .unwrap();
        let owner = |tx: &Connection, key: &str| -> rusqlite::Result<String> {
            tx.query_row(
                &format!("SELECT l.entity_id FROM {} l WHERE l.key=?", illustash_core::character_links::LINKS),
                [key],
                |r| r.get(0),
            )
        };
        let tree = |tx: &Connection, id: &str| -> rusqlite::Result<(Option<String>, Option<String>, Option<String>)> {
            tx.query_row(
                &format!(
                    "SELECT parent_id,variant,merged_into FROM {} e WHERE e.id=?",
                    illustash_core::character_entities::ENTITIES
                ),
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
        };
        // 用户给其中一对起过名字：这一对不合并。
        f.write(|tx| {
            let id = owner(tx, "named_old")?;
            illustash_core::character_names::put_name(tx, &id, "zh", "user", "用户起的名字", "display", 0, 1)?;
            Ok(())
        })
        .unwrap();
        illustash_core::character_catalog::override_for_thread(Some(catalog_with(
            r#"[1,"base_(w)",["base_old"],null,null,null,null,null,"",null],
            [2,"base_(swimsuit)_(w)",[],1,"appearance","auto","swimsuit",null,"",null],
            [3,"named_(w)",["named_old"],null,null,null,null,null,"",null],
            [4,"missing_(swimsuit)_(w)",[],5,"appearance","auto","swimsuit",null,"",null],
            [5,"missing_(w)",[],null,null,null,null,null,"",null]"#,
        )));
        f.write(|tx| {
            let (old, new, swim) = (owner(tx, "base_old")?, owner(tx, "base_(w)")?, owner(tx, "base_(swimsuit)_(w)")?);
            assert_ne!(old, new);
            let done = crate::character_commands::command(tx, &json!({"op":"catalog_apply"}))?;
            assert_eq!(done, json!({"merged": 1, "attached": 2}));
            assert_eq!(tree(tx, &owner(tx, "missing_(swimsuit)_(w)")?)?.0, Some(owner(tx, "missing_(w)")?));
            // 由规范标签决定合并目标，输入顺序或随机 ID 不改变结果。
            let (survivor, gone) = (new, old);
            assert_eq!(tree(tx, &gone)?.2, Some(survivor.clone()));
            assert_eq!(tree(tx, &swim)?.0, Some(survivor));
            let label: String = tx.query_row("SELECT label FROM actions ORDER BY id DESC LIMIT 1", [], |r| r.get(0))?;
            assert!(label.starts_with("角色词表："), "{label}");
            // 用户起过名字的那一对照旧是两个角色。
            assert_eq!(tree(tx, &owner(tx, "named_old")?)?.2, None);
            assert_ne!(owner(tx, "named_old")?, owner(tx, "named_(w)")?);
            // 同一修订只跑一次。
            assert_eq!(
                crate::character_commands::command(tx, &json!({"op":"catalog_apply"}))?,
                json!({"skipped": true})
            );
            // 拆开：合并的拆回来，挂上的造型改回顶层；拆过的不能再拆。
            let split = |id: &str| crate::character_commands::command(tx, &json!({"op":"catalog_split","entity":id}));
            assert_eq!(split(&gone)?, json!({"split": "merged"}));
            assert_eq!(tree(tx, &gone)?.2, None);
            assert_eq!(split(&swim)?, json!({"split": "attached"}));
            assert_eq!(tree(tx, &swim)?.0, None);
            assert!(split(&swim).is_err());
            Ok(())
        })
        .unwrap();
        // 新的目录修订：拆开过的两对不再合并、不再挂。
        illustash_core::character_catalog::override_for_thread(Some(catalog_revision(
            r#"[1,"base_(w)",["base_old"],null,null,null,null,null,"",null],
            [2,"base_(swimsuit)_(w)",[],1,"appearance","auto","swimsuit",null,"",null]"#,
            2,
        )));
        f.write(|tx| {
            assert_eq!(
                crate::character_commands::command(tx, &json!({"op":"catalog_apply"}))?,
                json!({"merged": 0, "attached": 0})
            );
            Ok(())
        })
        .unwrap();
        illustash_core::character_catalog::override_for_thread(None);
    }

    #[test]
    fn catalog_base_and_longtail_keep_raw_evidence_when_directory_withdraws_a_relation() {
        illustash_core::character_catalog::override_for_thread(Some(catalog_with(
            r#"[1,"hero_(w)",["old_hero"],null,null,null,null,null,"",null],
            [2,"hero_(swimsuit)_(w)",[],1,"appearance","auto","swimsuit",null,"",null],
            [3,"hero_(alt)_(w)",[],1,"form","ask",null,null,"",null]"#,
        )));
        let f = Fixture::new();
        f.write(|tx| {
            let assets = ["ccip_new", "ccip_old", "raw_old", "ccip_swim", "ccip_alt", "outside", "longtail"];
            let rows: Vec<_> =
                assets.iter().map(|u| json!([u,"test.png","test.png",8,8,"2026-11-23","folder"])).collect();
            let characters = json!([
                {"char_id":"old","danbooru_tag":"old_hero","copyright":"w"},
                {"char_id":"new","danbooru_tag":"hero_(w)","copyright":"w"},
                {"char_id":"swim","danbooru_tag":"hero_(swimsuit)_(w)","copyright":"w","skin":"old label"},
                {"char_id":"alt","danbooru_tag":"hero_(alt)_(w)","copyright":"w","skin":"unreviewed"},
                {"char_id":"outside","danbooru_tag":"hero_(guild)_(w)","copyright":"w","skin":"guild"}
            ]);
            crate::character_commands::command(tx,&json!({"op":"refresh","assets":rows,"indexed":"2026-11-23",
                "index":{"characters":characters},"labels":[
                    ["ccip_new","new",0.99,"char_ext"],["ccip_old","old",0.99,"char_ext"],["raw_old","old_hero",0.99,"character"],
                    ["ccip_swim","swim",0.99,"char_ext"],["ccip_alt","alt",0.99,"char_ext"],["outside","outside",0.99,"char_ext"]
                ]}))?;
            illustash_core::ccip_longtail::apply(tx,"catalog-longtail",&[
                illustash_core::ccip_longtail::Hit{uuid:"longtail".into(),tag:"old_hero".into(),score:0.92}
            ])?;
            let id = |key: &str| tx.query_row(
                "SELECT t.key FROM term_links l JOIN terms t ON t.id=l.term_id WHERE l.system='ccip' AND l.key=?",
                [key], |r| r.get::<_, String>(0),
            );
            let owner = |uuid: &str| tx.query_row("SELECT entity FROM memberships WHERE uuid=?", [uuid], |r| r.get::<_, String>(0));
            let (old, new, swim, alt, outside) = (id("old")?, id("new")?, id("swim")?, id("alt")?, id("outside")?);
            assert_ne!(old,new);
            for image in ["ccip_new","ccip_old","raw_old","ccip_swim","longtail"] {
                assert_eq!(owner(image)?,new);
            }
            assert_eq!(owner("ccip_alt")?,alt);
            assert_eq!(owner("outside")?,outside);
            assert_eq!(
                tx.query_row(
                    "SELECT r.variant FROM term_tree r JOIN terms t ON t.id=r.term_id WHERE t.key=?",
                    [&swim], |r| r.get::<_, String>(0)
                )?, "swimsuit"
            );
            let raw = snapshot(tx,"SELECT a.key,t.key FROM term_current_evidence e JOIN term_assets a ON a.id=e.asset_id JOIN terms t ON t.id=e.term_id WHERE a.key IN ('ccip_old','raw_old','longtail') ORDER BY a.key");
            assert_eq!(raw, vec![json!(["ccip_old",old]),json!(["longtail",old]),json!(["raw_old",old])]);
            illustash_core::character_names::put_name(tx,&new,"zh","user","用户保留的名字","display",0,1)?;
            crate::character_commands::command(tx,&json!({"op":"change","entity":new,"asked":["ccip_new"],"action":"confirmed"}))?;
            illustash_core::character_catalog::override_for_thread(Some(catalog_revision("",2)));
            crate::character_commands::command(tx,&json!({"op":"catalog_apply"}))?;
            assert_eq!(owner("ccip_new")?,new);
            assert_eq!(snapshot(tx,"SELECT value FROM term_names WHERE source='user'"),vec![json!(["用户保留的名字"])]);
            assert_eq!(snapshot(tx,"SELECT status FROM assertions WHERE uuid='ccip_new'"),vec![json!(["confirmed"])]);
            for image in ["ccip_old","raw_old","longtail"] {
                assert_eq!(owner(image)?,old);
            }
            assert_eq!(owner("ccip_swim")?,swim);
            Ok(())
        }).unwrap();
        illustash_core::character_catalog::override_for_thread(None);
    }

    #[test]
    fn catalog_existing_hybrid_roles_stay_unmodified_and_groups_remain_searchable_as_tags() {
        illustash_core::character_catalog::override_for_thread(Some(catalog_with("")));
        let f = Fixture::new();
        f.write(|tx| {
            tx.execute("INSERT OR REPLACE INTO meta VALUES ('identity_policy','hybrid')", [])?;
            tx.execute_batch(&include_str!("app_migration_sql/hybrid.sql")
                .replace("entities", illustash_core::character_entities::ENTITIES))?;
            let assets = ["base","old","costume","group"];
            let rows: Vec<_> = assets.iter().map(|u| json!([u,"t.png","t.png",8,8,"2026-11-23","folder"])).collect();
            crate::character_commands::command(tx,&json!({"op":"refresh","assets":rows,"indexed":"2026-11-23","index":{},
                "labels":[["base","hero",0.99,"character"],["old","old_hero",0.99,"character"],["costume","costume",0.99,"character"]]}))?;
            let group_catalog = json!({
                "format": illustash_core::character_catalog::FORMAT,
                "revision": 1,
                "columns": ["id","tag","tags","parent","relation","action","variant","bangumi","pick","wikidata","group"],
                "characters": [
                    [1,"hero",["old_hero"],null,null,null,null,null,"",null,false],
                    [2,"costume",[],1,"appearance","auto","costume",null,"",null,false],
                    [3,"group",[],null,null,null,null,null,"",null,true],
                ],
                "works": [],
            });
            let group_catalog = illustash_core::character_catalog::Catalog::parse(&group_catalog.to_string()).unwrap();
            illustash_core::character_catalog::override_for_thread(Some(Box::leak(Box::new(group_catalog))));
            let before = snapshot(tx, "SELECT id,parent_id,merged_into FROM term_identity_baseline ORDER BY id");
            crate::character_commands::command(tx,&json!({"op":"catalog_apply"}))?;
            assert_eq!(snapshot(tx, "SELECT id,parent_id,merged_into FROM term_identity_baseline ORDER BY id"), before);
            assert_eq!(snapshot(tx,"SELECT count(*) FROM term_verdicts"),vec![json!([0])]);
            let hits = [illustash_core::ccip_longtail::Hit{uuid:"group".into(),tag:"group".into(),score:0.95}];
            illustash_core::ccip_longtail::apply(tx,"catalog-group",&hits)?;
            assert_eq!(snapshot(tx,"SELECT count(*) FROM memberships WHERE uuid='group'"),vec![json!([0])]);
            assert_eq!(snapshot(tx,"SELECT l.key FROM tag_memberships m JOIN terms t ON t.key=m.entity JOIN term_links l ON l.term_id=t.id WHERE m.uuid='group'"),vec![json!(["group"])]);
            illustash_core::character_catalog::override_for_thread(Some(catalog_revision("",2)));
            crate::character_commands::command(tx,&json!({"op":"catalog_apply"}))?;
            assert_eq!(snapshot(tx,"SELECT count(*) FROM memberships WHERE uuid='group'"),vec![json!([1])]);
            Ok(())
        }).unwrap();
        illustash_core::character_catalog::override_for_thread(None);
    }

    #[test]
    fn catalog_split_restores_in_a_portable_snapshot_and_undo_restores_the_catalog_edge() {
        use illustash_core::human_conclusions::{export, format, import};
        illustash_core::character_catalog::override_for_thread(Some(catalog_with(
            r#"[1,"hero",["old_hero"],null,null,null,null,null,"",null],
            [2,"costume",[],1,"appearance","auto","costume",null,"",null]"#,
        )));
        let init = |tx: &Connection| -> Result<Vec<String>> {
            let characters = json!([
                {"char_id":"old","danbooru_tag":"old_hero"},
                {"char_id":"new","danbooru_tag":"hero"},
                {"char_id":"skin","danbooru_tag":"costume"}
            ]);
            crate::character_commands::command(
                tx,
                &json!({"op":"refresh","assets":[],"indexed":"2026-11-23",
                "index":{"characters":characters},"labels":[]}),
            )?;
            Ok(tx
                .prepare(
                    "SELECT t.key FROM term_links l JOIN terms t ON t.id=l.term_id
                     WHERE l.system='ccip' ORDER BY l.key",
                )?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?)
        };
        let f = Fixture::new();
        let mut wire = Vec::new();
        f.write(|tx| {
            let ids = init(tx)?;
            for id in &ids[1..] {
                crate::character_commands::command(tx, &json!({"op":"catalog_split","entity":id}))?;
            }
            export::capture(
                tx,
                format::Header::new(
                    "sample".into(),
                    "source".into(),
                    "2026-11-23T00:00:00Z".into(),
                    format::Policy::Default,
                ),
            )?
            .write(&mut wire)?;
            Ok(())
        })
        .unwrap();
        let snapshot = format::read(&mut std::io::Cursor::new(wire)).unwrap();
        let target = Fixture::new();
        target
            .write(|tx| {
                let ids = init(tx)?;
                let edge_count = || {
                    tx.query_row(
                        "SELECT count(*) FROM term_tree WHERE parent_id IS NOT NULL OR merged_into IS NOT NULL",
                        [],
                        |r| r.get::<_, i64>(0),
                    )
                };
                assert_eq!(edge_count()?, 2);
                let applied = import::apply_in(tx, &snapshot, "other-library", &[], &mut || Ok(()))?;
                assert_eq!(applied.records["applied"], 2);
                assert_eq!(edge_count()?, 0);
                assert!(illustash_core::character_identity::kept_apart(tx, &ids[0], &ids[1])?);
                assert!(illustash_core::character_identity::kept_apart(tx, &ids[0], &ids[2])?);
                let replay = import::apply_in(tx, &snapshot, "other-library", &[], &mut || Ok(()))?;
                assert_eq!(replay.records["already_present"], 2);
                import::undo_in(tx, applied.action_id.unwrap())?;
                assert_eq!(edge_count()?, 2);
                assert!(!illustash_core::character_identity::kept_apart(tx, &ids[0], &ids[1])?);
                assert!(!illustash_core::character_identity::kept_apart(tx, &ids[0], &ids[2])?);
                Ok(())
            })
            .unwrap();
        illustash_core::character_catalog::override_for_thread(None);
    }

    #[test]
    fn catalog_protects_manual_form_endpoints_until_the_action_is_undone() {
        illustash_core::character_catalog::override_for_thread(Some(catalog_with("")));
        let f = Fixture::new();
        f.write(|tx| {
            let index = json!({"characters":[
                {"char_id":"a","danbooru_tag":"role_a"},
                {"char_id":"b","danbooru_tag":"role_b"}
            ]});
            crate::character_commands::command(
                tx,
                &json!({"op":"refresh","assets":[],"indexed":"2026-11-23","index":index}),
            )?;
            let by_tag = |tag: &str| {
                tx.query_row(
                    "SELECT t.key FROM terms t JOIN term_links l ON l.term_id=t.id WHERE l.key=?1 AND l.system='ccip'",
                    [tag],
                    |r| r.get::<_, String>(0),
                )
            };
            let (a, b) = (by_tag("a")?, by_tag("b")?);
            illustash_core::character_identity::attach_form(tx, &a, &b)?;
            assert!(illustash_core::character_import::user_touched(tx, &a)?);
            assert!(illustash_core::character_import::user_touched(tx, &b)?);
            let action = tx.query_row("SELECT max(id) FROM actions", [], |r| r.get::<_, i64>(0))?;
            crate::character_commands::command(tx, &json!({"op":"undo","action":action}))?;
            assert!(!illustash_core::character_import::user_touched(tx, &a)?);
            assert!(!illustash_core::character_import::user_touched(tx, &b)?);
            Ok(())
        })
        .unwrap();
        illustash_core::character_catalog::override_for_thread(None);
    }

    #[test]
    fn evidence_replaces_and_restores_without_rewriting_old_sources() {
        round_trip(false);
        round_trip(true);
    }

    fn round_trip(hybrid: bool) {
        let f = Fixture::new();
        let assets = ["existing", "weak", "empty", "human", "rejected"];
        let app_rows = assets
            .iter()
            .map(|uuid| json!([uuid, "synthetic.png", "synthetic.png", 8, 8, "2026-11-16", "folder"]))
            .collect::<Vec<_>>();
        let package = synthetic(&[("from_b", 0, 0.8008725047111511, false)]);
        let source = writer(&f.library.source()).unwrap();
        source
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS embeddings(uuid TEXT,model TEXT,vec BLOB,PRIMARY KEY(uuid,model))",
            )
            .unwrap();
        let mut wd = vec![0f32; DIM];
        wd[0] = 1.;
        let vector = wd.into_iter().flat_map(f32::to_le_bytes).collect::<Vec<_>>();
        for uuid in assets {
            source
                .execute(
                    "INSERT INTO assets(uuid,filename,src_path,width,height,added_at,src_kind,in_scope,trashed)
                 VALUES (?1,'synthetic.png','synthetic.png',8,8,'2026-11-16','folder',1,0)",
                    [uuid],
                )
                .unwrap();
            source.execute("INSERT INTO embeddings VALUES (?1,'wd',?2)", rusqlite::params![uuid, vector]).unwrap();
        }
        f.write(|tx| {
            if hybrid {
                tx.execute("INSERT OR REPLACE INTO meta VALUES ('identity_policy','hybrid')", [])?;
                tx.execute_batch(
                    &include_str!("app_migration_sql/hybrid.sql")
                        .replace("entities", illustash_core::character_entities::ENTITIES),
                )?;
            }
            crate::character_commands::command(
                tx,
                &json!({
                    "op":"refresh","assets":app_rows,"indexed":"2026-11-16","index":{},
                    "labels":[["existing","base",0.89,"character"],["weak","near",0.90,"char_ext"],
                              ["human","manually_chosen",0.40,"character"],["rejected","from_b",0.50,"character"]],
                    "decisions":[],"duplicates":[]
                }),
            )?;
            for (uuid, action) in [("human", "confirmed"), ("rejected", "rejected")] {
                let entity: String =
                    tx.query_row("SELECT entity FROM candidates WHERE uuid=?1", [uuid], |r| r.get(0))?;
                if uuid == "rejected" {
                    // 同一个公开键即使此前由人工操作或原来源创建，也不能绕过否定。
                    let term: i64 = tx.query_row("SELECT id FROM terms WHERE key=?1", [&entity], |r| r.get(0))?;
                    tx.execute(
                        "INSERT INTO term_links(system,key,term_id,source) VALUES ('danbooru','from_b',?1,'import')",
                        [term],
                    )?;
                }
                crate::character_commands::command(
                    tx,
                    &json!({"op":"change","asked":[uuid],"entity":entity,"action":action}),
                )?;
            }
            Ok(())
        })
        .unwrap();
        let app = writer(&f.library.db()).unwrap();
        let candidates = "SELECT * FROM candidates ORDER BY uuid,source,entity";
        let members = "SELECT * FROM memberships ORDER BY uuid,entity";
        let old_candidates = snapshot(&app, candidates);
        let old_members = snapshot(&app, members);
        let active_rows = "SELECT id,asset_id,term_id,version,score FROM term_current_evidence ORDER BY id";
        let old_evidence = snapshot(&app, active_rows);
        let verdicts = "SELECT * FROM term_verdicts ORDER BY id";
        let old_verdicts = snapshot(&app, verdicts);
        let projection = "SELECT * FROM term_projection ORDER BY asset_id,term_id";
        let old_projection = snapshot(&app, projection);
        assert_eq!(old_members.len(), 2);
        assert_eq!(withdraw(&f.library).unwrap()["changed"], 0);
        let applied = apply_loaded(&f.library, &package, "test", &Progress::silent()).unwrap();
        assert_eq!(applied["changed"], 5);
        assert_eq!(applied["hits"], 5);
        assert_eq!(
            snapshot(&app, "SELECT uuid FROM memberships WHERE sources='WD_EXT' ORDER BY uuid"),
            vec![json!(["empty"]), json!(["weak"])]
        );
        assert_eq!(
            snapshot(&app, "SELECT * FROM candidates WHERE source<>'WD_EXT' ORDER BY uuid,source,entity"),
            old_candidates
        );
        let evidence =
            snapshot(&app, "SELECT id,asset_id,term_id,version,score FROM term_current_evidence ORDER BY id");
        let sets = snapshot(&app, "SELECT * FROM term_evidence_sets ORDER BY id");
        assert_eq!(apply_loaded(&f.library, &package, "test", &Progress::silent()).unwrap()["changed"], 0);
        assert_eq!(snapshot(&app, "SELECT * FROM term_evidence_sets ORDER BY id"), sets);

        // 模拟批次生效途中数据库写入失败。上一个有效集和投影不动，重试仍能完成。
        app.execute_batch(
            "CREATE TRIGGER stop_extension BEFORE INSERT ON term_evidence_sets
             WHEN new.source='wd_extension' BEGIN SELECT RAISE(ABORT,'simulated interruption'); END;",
        )
        .unwrap();
        assert!(withdraw(&f.library).is_err());
        assert_eq!(
            snapshot(&app, "SELECT id,asset_id,term_id,version,score FROM term_current_evidence ORDER BY id"),
            evidence
        );
        app.execute_batch("DROP TRIGGER stop_extension").unwrap();
        assert_eq!(withdraw(&f.library).unwrap()["changed"], 5);
        assert_eq!(withdraw(&f.library).unwrap()["changed"], 0);
        assert_eq!(snapshot(&app, candidates), old_candidates);
        assert_eq!(snapshot(&app, members), old_members);
        assert_eq!(snapshot(&app, active_rows), old_evidence);
        assert_eq!(snapshot(&app, verdicts), old_verdicts);
        assert_eq!(snapshot(&app, projection), old_projection);
        assert!(snapshot(&app, "SELECT id FROM term_current_evidence WHERE source='wd_extension'").is_empty());
        assert_eq!(snapshot(&app, "SELECT id FROM term_evidence WHERE source='wd_extension'").len(), 5);
        let reopened = Library::open(&f.root.join("test.pilib")).unwrap();
        assert_eq!(snapshot(&writer(&reopened.db()).unwrap(), members), old_members);
        assert_eq!(apply_loaded(&reopened, &package, "test", &Progress::silent()).unwrap()["changed"], 5);
        assert_eq!(
            snapshot(&app, "SELECT id,asset_id,term_id,version,score FROM term_current_evidence ORDER BY id"),
            evidence
        );

        // 装错底座和向量损坏时，已有的有效集不被空结果覆盖。
        source.execute("UPDATE embeddings SET vec=X'00' WHERE uuid='weak'", []).unwrap();
        assert!(apply_loaded(&reopened, &package, "test", &Progress::silent()).is_err());
        assert_eq!(
            snapshot(&app, "SELECT id,asset_id,term_id,version,score FROM term_current_evidence ORDER BY id"),
            evidence
        );
        let mut manifest = json!({"format":"illustash-recognition/2","models":{"wd":{
            "version":"swinv2_448","dimension":1024,"sha256":"model","tags_sha256":"tags"
        }}});
        package.compatible(&manifest.to_string()).unwrap();
        package.compatible(&json!({"recognition": manifest,"character_claims": {}}).to_string()).unwrap();
        manifest["models"]["wd"]["sha256"] = "unrelated".into();
        assert!(package.compatible(&manifest.to_string()).is_err());

        f.write(|tx| wd_extension::invalidate(tx, &["weak".into()])).unwrap();
        assert_eq!(
            snapshot(&app, "SELECT uuid FROM memberships WHERE sources='WD_EXT' ORDER BY uuid"),
            vec![json!(["empty"])]
        );
    }
}
