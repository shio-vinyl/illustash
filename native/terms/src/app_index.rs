//! 原生角色索引：稳定查询、补丁提交和恢复；计算 worker 仅处理隔离快照。
use crate::error::invalid;
use crate::{
    app_library::Library,
    app_tasks::{Task, TaskContext, TaskIdentity},
    diagnostics::Publication,
    http_json::Json,
    Error, Result,
};
use crate::{
    task_state::{State, Status},
    worker_protocol as protocol,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    cell::Cell,
    collections::BTreeSet,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
const DEFAULTS: [&str; 8] = [
    "blue_archive",
    "arknights:_endfield",
    "wuthering_waves",
    "meitantei_precure!",
    "genshin_impact",
    "honkai:_star_rail",
    "honkai_impact_3rd",
    "zenless_zone_zero",
];
/// 共享目录里记着这台电脑启用还是撤回了基础角色索引（#607）：{enabled, sha256, version}。没有这个文件就是还没启用过。
/// 随锚点一起在同一次索引发布里换上（index_generation）。
const BASE_MARK: &str = "char-index-base.json";
/// 图库启用基础角色索引的记录（应用库 meta）：{sha256, mode}。`mode` 为 `rebuilt` 时图库的角色表和索引候选按包重建过；
/// 为 `adopted` 时图库已有同一版本的索引，只装了共享锚点，图库内容没动。撤回时删掉。
const BASE_META: &str = "character_index_base";
fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}
fn read(p: &Path, default: Value) -> Value {
    std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(default)
}
fn write(p: &Path, v: &Value) -> Result<()> {
    std::fs::create_dir_all(p.parent().expect("index file path must have a parent"))?;
    let tmp = p.with_extension(format!("json.{}.tmp", crate::store::new_key()?));
    std::fs::write(&tmp, serde_json::to_vec(v).map_err(|e| Error::Invalid(e.to_string()))?)?;
    std::fs::rename(tmp, p)?;
    Ok(())
}
fn safe(s: &str) -> bool {
    !s.is_empty() && s.len() <= 200 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}
fn short(s: &str) -> String {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new("[^a-z0-9]+").expect("constant index tag regex is valid"))
        .replace_all(&s.to_lowercase(), "_")
        .trim_matches('_')
        .into()
}
fn writer(p: &Path) -> Result<Connection> {
    crate::app_name_sources::db(p)
}
fn meta(p: &Path, id: &str) -> Value {
    if safe(id) {
        read(&p.join("char-index").join(format!("{id}.json")), Value::Null)
    } else {
        Value::Null
    }
}
const FINGERPRINT_ROWS: [&str; 4] = [
    "SELECT * FROM characters ORDER BY char_id",
    "SELECT * FROM character_aliases ORDER BY char_id,alias",
    "SELECT * FROM labels WHERE dim IN ('char_ext','character','general') ORDER BY uuid,dim,value",
    "SELECT uuid FROM assets ORDER BY uuid",
];

fn feature_digest(c: &Connection, progress: &mut impl FnMut() -> Result<()>) -> Result<String> {
    let mut digest = Sha256::new();
    if c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='embeddings')", [], |r| r.get::<_, bool>(0))? {
        let mut q = c.prepare("SELECT uuid,vec FROM embeddings WHERE model='ccip' ORDER BY uuid")?;
        let mut rows = q.query([])?;
        let mut count = 0;
        while let Some(r) = rows.next()? {
            if count % 4096 == 0 {
                progress()?;
            }
            count += 1;
            let uuid: String = r.get(0)?;
            digest.update(uuid.as_bytes());
            digest.update(r.get_ref(1)?.as_blob().map_err(|_| Error::Invalid("CCIP 特征不是二进制".into()))?);
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// 保留旧日志的指纹字节和标签排序，每次只在 Rust 中保存一行。
fn fingerprint_rows(c: &Connection, mut progress: impl FnMut() -> Result<()>) -> Result<String> {
    let mut digest = Sha256::new();
    let mut encoded = Vec::new();
    digest.update(b"[");
    for sql in FINGERPRINT_ROWS {
        progress()?;
        digest.update(b"[");
        let mut q = c.prepare_cached(sql)?;
        let cols = q.column_count();
        let mut rows = q.query([])?;
        let mut values = Vec::with_capacity(cols);
        let mut count = 0;
        while let Some(row) = rows.next()? {
            if count > 0 {
                digest.update(b",");
            }
            if count % 4096 == 0 {
                progress()?;
            }
            count += 1;
            for i in 0..cols {
                values.push(crate::source_rows::cell(row.get_ref(i)?)?);
            }
            serde_json::to_writer(&mut encoded, &values).map_err(|e| Error::invalid(e.to_string()))?;
            digest.update(&encoded);
            values.clear();
            encoded.clear();
        }
        digest.update(b"],");
    }
    let feature = feature_digest(c, &mut progress)?;
    serde_json::to_writer(&mut digest, &feature).map_err(|e| Error::invalid(e.to_string()))?;
    digest.update(b"]");
    Ok(format!("{:x}", digest.finalize()))
}

fn fingerprint(c: &Connection, ctx: &TaskContext) -> Result<String> {
    fingerprint_rows(c, || {
        ctx.check()?;
        ctx.progressed();
        Ok(())
    })
}

fn with_index_progress<T>(
    c: &mut Connection,
    ctx: &TaskContext,
    read: impl FnOnce(&mut Connection) -> Result<T>,
) -> Result<T> {
    // SQLite 在第一行出来前可能要排序。进度回调让取消在这段期间也能中断查询。
    let stop = ctx.clone();
    c.progress_handler(16_384, Some(move || stop.check().is_err()))?;
    let result = read(c);
    c.progress_handler(0, None::<fn() -> bool>)?;
    result.or_else(|error| ctx.check().and(Err(error)))
}
// 关注列表存的是作品 ID（docs/DATA_MODEL.md），它们只在所属的图库里存在，
// 所以每个图库在自己的 prefs 里保存自己的列表和阈值（#205）。没有列表的图库从旧版本共用的 `watch.json` 开始：
// 只带过来它自己的作品和未解析的 tag，那个文件以后不再写入。
const WATCH_PREF: &str = "index_watch";
fn work_for_tag(c: &Connection, tag: &str, current: bool) -> Result<Option<String>> {
    let current = if current { " AND l.current = 1" } else { "" };
    let sql = format!(
        "SELECT l.entity_id FROM {LINKS} l JOIN {ENTITIES} e ON e.id = l.entity_id
         WHERE l.core_system = 'danbooru' AND l.key = ?{current} AND e.kind = 'work'",
        LINKS = crate::character_links::LINKS,
        ENTITIES = crate::character_entities::ENTITIES
    );
    Ok(c.query_row(&sql, [tag], |r| r.get(0)).optional()?)
}
fn legacy_watch(c: &Connection, dir: &Path) -> Result<Value> {
    let w = read(&dir.join("watch.json"), json!({"works":DEFAULTS,"min_posts":100}));
    let mut ids = vec![];
    let mut pending = vec![];
    if w["version"] == 2 {
        for id in w["works"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            let own: bool = c.query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM {ENTITIES} WHERE id=? AND kind='work')",
                    ENTITIES = crate::character_entities::ENTITIES
                ),
                [id],
                |r| r.get(0),
            )?;
            if own && !ids.contains(&json!(id)) {
                ids.push(json!(id));
            }
        }
        pending = w["pending_tags"].as_array().cloned().unwrap_or_default();
    } else {
        for tag in w["works"].as_array().into_iter().flatten() {
            match work_for_tag(c, tag.as_str().unwrap_or(""), false)? {
                Some(id) if !ids.contains(&json!(id)) => ids.push(json!(id)),
                Some(_) => {}
                None => pending.push(tag.clone()),
            }
        }
    }
    Ok(json!({"version":2,"works":ids,"pending_tags":pending,"min_posts":w["min_posts"]}))
}
fn load_watch(c: &Connection, dir: &Path) -> Result<Value> {
    let stored: Option<String> =
        c.query_row("SELECT value FROM prefs WHERE key=?", [WATCH_PREF], |r| r.get(0)).optional()?;
    let mut w = match stored {
        Some(text) => serde_json::from_str(&text).map_err(|e| Error::Invalid(format!("无效的关注列表：{e}")))?,
        None => legacy_watch(c, dir)?,
    };
    // 本图库里有作品把旧 tag 作为当前的 Danbooru 链接时，这个 tag 就解析出来了。
    let mut ids = w["works"].as_array().cloned().unwrap_or_default();
    let mut remaining = vec![];
    for tag in w["pending_tags"].as_array().into_iter().flatten() {
        match work_for_tag(c, tag.as_str().unwrap_or(""), true)? {
            Some(id) => {
                if !ids.contains(&json!(id)) {
                    ids.push(json!(id));
                }
            }
            None => remaining.push(tag.clone()),
        }
    }
    w["works"] = json!(ids);
    w["pending_tags"] = json!(remaining);
    Ok(w)
}
fn watch(lib: &Library, dir: &Path) -> Result<Value> {
    load_watch(&crate::app_library::read_connection(&lib.db())?, dir)
}
fn set_watch(lib: &Library, dir: &Path, b: &Value) -> Result<()> {
    let Some(works) = b["works"].as_array().filter(|a| a.len() <= 40 && a.iter().all(Value::is_string)) else {
        return invalid("关注列表应为最多 40 个作品 ID");
    };
    let Some(n) = b["min_posts"].as_i64().filter(|n| (20..=5000).contains(n)) else {
        return invalid("最少图片数应在 20–5000 之间");
    };
    let mut c = crate::sqlite::open_with_flags(lib.db(), rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    c.busy_timeout(Duration::from_secs(20))?;
    // 先取写锁，两次保存就不会互相丢掉对方的改动。
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    crate::schema::require_current(&tx)?;
    for w in works {
        let linked = format!(
            "SELECT EXISTS(
               SELECT 1
               FROM {ENTITIES} e
               JOIN {LINKS} l ON l.entity_id=e.id
               WHERE e.id=? AND e.kind='work' AND l.core_system='danbooru' AND l.current=1)",
            ENTITIES = crate::character_entities::ENTITIES,
            LINKS = crate::character_links::LINKS
        );
        if !tx
            .query_row(&linked, [w.as_str().expect("watch list was validated as strings")], |r| r.get::<_, bool>(0))?
        {
            return invalid("只能关注已确认 Danbooru 链接的作品");
        }
    }
    let mut w = load_watch(&tx, dir)?;
    let mut ids = vec![];
    for id in works {
        if !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    w["works"] = json!(ids);
    w["min_posts"] = n.into();
    tx.execute("INSERT OR REPLACE INTO prefs(key,value) VALUES (?1,?2)", params![WATCH_PREF, w.to_string()])?;
    tx.commit()?;
    Ok(())
}
fn candidates(
    lib: &Library,
    resources: &Path,
    shared: &Path,
    dir: &Path,
    net: &crate::app_name_sources::Source,
) -> Result<Value> {
    let w = watch(lib, dir)?;
    let app = crate::app_library::read_connection(&lib.db())?;
    let src = crate::app_library::read_connection(&lib.source())?;
    let mut known = BTreeSet::new();
    for column in ["char_id", "danbooru_tag", "wd_tag"] {
        if src
            .prepare("PRAGMA table_info(characters)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|c| c == column)
        {
            known.extend(
                src.prepare(&format!("SELECT {column} FROM characters WHERE {column} IS NOT NULL"))?
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            );
        }
    }
    let mut tags = vec![];
    for id in w["works"].as_array().into_iter().flatten() {
        tags.extend(
            app.prepare(&format!(
                "SELECT l.key FROM {LINKS} l WHERE l.entity_id = ? AND l.core_system = 'danbooru' AND l.current = 1",
                LINKS = crate::character_links::LINKS
            ))?
            .query_map([id.as_str().unwrap_or("")], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    let rejected = if dir.join("rejected").is_dir() {
        std::fs::read_dir(dir.join("rejected"))?
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().to_str().and_then(|s| s.strip_suffix(".json")).map(str::to_owned))
            .collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };
    let covered = package_covered(&app, resources)?;
    let own = |id: &str| shared.join("char-index").join(format!("{id}.json")).exists();
    let local = Local {
        known: &known,
        covered: &covered,
        rejected: &rejected,
        own: &own,
        min_posts: w["min_posts"].as_i64().unwrap_or(100),
    };
    let mut rows = vec![];
    let mut counts = serde_json::Map::new();
    let mut coverage = serde_json::Map::new();
    let mut seen = BTreeSet::new();
    let work_order = tags.clone();
    for cp in tags {
        let rel = net.fetch(
            &crate::app_name_sources::url(
                &net.db_base,
                "/related_tag.json",
                &[("query", cp.clone()), ("category", "character".into()), ("limit", "1000".into())],
            ),
            None,
            None,
        )?;
        let related = rel["related_tags"].as_array().ok_or_else(|| Error::Network("索引查询响应不正确".into()))?;
        if related.is_empty() {
            coverage.insert(cp.clone(), Value::Null);
            counts.insert(cp, Value::Null);
            continue;
        }
        let tally = tally(&cp, related, &local, &mut seen, &mut rows)?;
        counts.insert(cp.clone(), tally["missing"].clone());
        coverage.insert(cp, tally);
    }
    Ok(
        json!({"checked_at":now(),"works":counts,"coverage":coverage,"min_posts":w["min_posts"],"missing":rows,"work_order":work_order}),
    )
}
/// 检查新角色认为本地已有的角色（#561）：WD 自带和精选索引里的（来源库的 characters，也含自己补过的）、自己补过的
/// （共享目录的 char-index 里有）、本图库启用的数据包覆盖的，以及用户移除过、以后不再提示的。
struct Local<'a> {
    known: &'a BTreeSet<String>,
    covered: &'a BTreeSet<String>,
    rejected: &'a BTreeSet<String>,
    own: &'a dyn Fn(&str) -> bool,
    min_posts: i64,
}
/// 一个作品在 Danbooru 上的相关角色：还缺的追加到 `rows`，返回这个作品的计数 {packs, own, missing}。`seen` 跨作品
/// 共用，每个角色只算进第一个列出它的作品。自己补过的先算（它们也在 characters 里），然后跳过 WD 自带和精选索引里已有的
/// （不计数），再算数据包覆盖的，跳过用户移除过的；剩下的才是还缺的。
fn tally(
    cp: &str,
    related: &[Value],
    local: &Local,
    seen: &mut BTreeSet<String>,
    rows: &mut Vec<Value>,
) -> Result<Value> {
    static SUFFIX: OnceLock<regex::Regex> = OnceLock::new();
    let suffix =
        SUFFIX.get_or_init(|| regex::Regex::new(r"_\([^()]*\)$").expect("constant index suffix regex is valid"));
    let (mut missing, mut own, mut packs) = (0, 0, 0);
    for r in related {
        let Some(tag) = r["tag"]["name"].as_str() else { return invalid("索引角色 tag 不正确") };
        let id = short(tag);
        let posts = r["tag"]["post_count"].as_i64().unwrap_or(0);
        let overlap = r["overlap_coefficient"].as_f64().unwrap_or(0.0);
        if !safe(&id)
            || overlap < 0.5
            || posts < local.min_posts
            || seen.contains(tag)
            || suffix.is_match(&suffix.replace(tag, ""))
        {
            continue;
        }
        if (local.own)(&id) {
            own += 1;
        } else if local.known.contains(tag) || local.known.contains(&id) {
            continue;
        } else if local.covered.contains(tag) {
            packs += 1;
        } else if local.rejected.contains(&id) {
            continue;
        } else {
            let overlap = (overlap * 1000.0).round() / 1000.0;
            rows.push(json!({"tag":tag,"short_tag":id,"copyright":cp,"post_count":posts,"overlap":overlap}));
            missing += 1;
        }
        seen.insert(tag.to_owned());
    }
    Ok(json!({"packs":packs,"own":own,"missing":missing}))
}
/// 本图库启用的数据包能认出的角色标签（#561）：检查新角色把它们算作已有，按作品计数。现在只有 WD 角色扩展
/// （app_wd_extension 读启用状态和包里的标签表）；CCIP 长尾包（#559）合入后，在这里并进它启用的包的角色标签。
fn package_covered(app: &Connection, resources: &Path) -> Result<BTreeSet<String>> {
    let mut covered = BTreeSet::new();
    covered.extend(crate::app_wd_extension::covered_labels(app, resources)?);
    Ok(covered)
}

pub(crate) struct Updater {
    /// 资源根目录：在这里找识别模型（#168）。
    resources: PathBuf,
    shared: PathBuf,
    dir: PathBuf,
    journal: PathBuf,
    state: Arc<Mutex<State>>,
    task: Option<Task>,
    generation: u64,
    session: String,
    network: bool,
    pub automatic: bool,
    next_retry: Instant,
    /// 自动启用或撤回基础角色索引（#607）失败后，隔一段时间再试。
    base_retry: Instant,
}
impl Updater {
    pub fn new(
        lib: &Library,
        root: &Path,
        generation: u64,
        session: &str,
        network: bool,
        automatic: bool,
    ) -> Result<Self> {
        let shared = crate::app_resources::shared_path(root)?;
        Ok(Self {
            resources: root.to_owned(),
            dir: shared.join("char-patch"),
            shared,
            journal: lib.instance_path().join("index-job.json"),
            state: Arc::new(Mutex::new(State::from(json!({"status":"idle"})))),
            task: None,
            generation,
            session: session.into(),
            network,
            automatic,
            next_retry: Instant::now(),
            base_retry: Instant::now(),
        })
    }
    /// 这个图库关注新角色的作品（#96 在推荐作品里标出它们）。
    pub fn watched(&self, lib: &Library) -> Result<Vec<String>> {
        Ok(watch(lib, &self.dir)?["works"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|w| w.as_str().map(str::to_owned))
            .collect())
    }
    pub fn writing(&self) -> bool {
        self.task.as_ref().is_some_and(Task::running)
            && self.state.lock().expect("index state mutex poisoned").status() != Status::Checking
    }
    pub fn stop(&mut self) {
        if let Some(mut t) = self.task.take() {
            t.stop();
        }
    }
    pub fn progress(&self) -> Json {
        let state = self.public_state(self.state.lock().expect("index state mutex poisoned").value());
        Json::object([(
            "state",
            ordered(
                &state,
                &[
                    "status",
                    "started_at",
                    "done",
                    "total",
                    "found",
                    "kind",
                    "added",
                    "changed",
                    "message",
                    "finished_at",
                ],
            ),
        )])
    }
    /// 终态允许调用方立即发起下一次操作；与 `start` 使用同一线程退出边界，确保资源已释放。
    fn public_state(&self, mut state: Value) -> Value {
        if matches!(state["status"].as_str(), Some("checked" | "done" | "paused" | "error"))
            && self.task.as_ref().is_some_and(Task::running)
        {
            state["status"] = json!("cleaning");
            state.as_object_mut().expect("索引状态是对象").remove("finished_at");
        }
        state
    }
    /// 上次检查时每个关注作品的覆盖情况（#561，GET /api/index/coverage）；还没检查过时为 null。
    pub fn coverage(&self) -> Json {
        read_check(&self.dir.join("coverage.json"))
    }
    pub fn info(&self, lib: &Library) -> Result<Json> {
        let c = crate::app_library::read_connection(&lib.db())?;
        crate::http_read::register_names(&c)?;
        let (state, mut history) = state_and_history(&self.state, &self.dir);
        let state = self.public_state(state);
        history.truncate(6);
        for h in &mut history {
            if let Some(h) = h.as_object_mut() {
                h.remove("job_id");
            }
            for r in h["characters"].as_array_mut().into_iter().flatten() {
                let owner = format!(
                    "SELECT coalesce(e.parent_id,e.id)
                     FROM {LINKS} l
                     JOIN {ENTITIES} e ON e.id=l.entity_id
                     WHERE l.core_system='ccip' AND l.key=?",
                    LINKS = crate::character_links::LINKS,
                    ENTITIES = crate::character_entities::ENTITIES
                );
                let id: Option<String> =
                    c.query_row(&owner, [r["short_tag"].as_str().unwrap_or("")], |r| r.get(0)).optional()?;
                r["entity"] = if let Some(id) = id {
                    let sql = format!(
                        "SELECT entity_name(e.id),
                           (SELECT count(*)
                            FROM memberships m
                            WHERE m.entity=e.id AND {visible})
                         FROM {ENTITIES} e
                         WHERE e.id=?",
                        visible = crate::sql_fragments::visible("m"),
                        ENTITIES = crate::character_entities::ENTITIES
                    );
                    let e = c.query_row(&sql, [&id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
                    let mut shown = crate::http_read::display(
                        &c,
                        Json::object([("id", id.clone().into()), ("name", e.0.into()), ("count", e.1.into())]),
                    )?;
                    shown.set("id", crate::character_commands::resolve(&c, &id)?.into());
                    serde_json::from_str(&shown.dump()).map_err(|e| Error::Invalid(e.to_string()))?
                } else {
                    Value::Null
                };
            }
        }
        let works = crate::http_read::works(&c, "")?;
        Ok(Json::object([
            (
                "state",
                ordered(
                    &state,
                    &[
                        "status",
                        "started_at",
                        "done",
                        "total",
                        "found",
                        "kind",
                        "added",
                        "changed",
                        "message",
                        "finished_at",
                    ],
                ),
            ),
            ("watch", ordered(&watch(lib, &self.dir)?, &["version", "works", "pending_tags", "min_posts"])),
            ("check", read_check(&self.dir.join("check.json"))),
            ("history", Json::Array(history.iter().map(history_json).collect())),
            ("mb_per_character", Json::Float(1.1)),
            ("works", works),
        ]))
    }
    pub fn post(&mut self, lib: &Library, b: &Json, busy: bool) -> Result<Json> {
        let b: Value = serde_json::from_str(&b.dump()).map_err(|e| Error::Invalid(e.to_string()))?;
        if lib.frozen() {
            return invalid("测试图库不能更新索引");
        };
        match b["action"].as_str() {
            Some("watch") => set_watch(lib, &self.dir, &b)?,
            // 为等人确认而暂停的任务（#287）：用新的一次运行继续，或在它还没写入任何东西之前放弃。
            Some("resume") => {
                if b.get("allow_network").is_some_and(|v| !v.is_boolean()) {
                    return invalid("联网确认必须是布尔值");
                }
                if busy {
                    return invalid("正在与 Photos 同步，完成后再更新角色索引");
                }
                if !self.journal.exists() {
                    return invalid("没有等待继续的索引任务");
                }
                self.next_retry = Instant::now() + Duration::from_secs(60);
                self.start(lib, json!({"action":"resume", "allow_network":b["allow_network"]}))?;
            }
            Some("discard") => {
                if self.task.as_ref().is_some_and(Task::running) {
                    return invalid("角色索引正在更新，请稍后");
                }
                if paused(&self.journal).is_none() {
                    return invalid("没有可以放弃的添加任务");
                }
                let job: protocol::IndexJob = protocol::read(&self.journal)?;
                if job.folder.parent() == Some(lib.instance_path().join("index-jobs").as_path()) {
                    let _ = std::fs::remove_dir_all(&job.folder);
                }
                std::fs::remove_file(&self.journal)?;
                *self.state.lock().expect("index state mutex poisoned") = State::from(json!({"status":"idle"}));
            }
            Some("check" | "apply" | "drop") => {
                if b.get("allow_network").is_some_and(|v| !v.is_boolean()) {
                    return invalid("联网确认必须是布尔值");
                }
                if b["action"] == "check" && !self.network && b["allow_network"] != true {
                    return invalid("请先确认本次检查允许联网查询 Danbooru");
                }
                if paused(&self.journal).is_some() && !self.task.as_ref().is_some_and(Task::running) {
                    return invalid("上次添加角色已暂停，请先在任务面板里继续或放弃");
                }
                if b["action"] != "check" && busy {
                    return invalid("正在与 Photos 同步，完成后再更新角色索引");
                };
                if self.journal.exists() && b["action"] != "check" {
                    let job: protocol::IndexJob = protocol::read(&self.journal)?;
                    let job = protocol::value(&job)?;
                    let same = if b["action"] == "apply" {
                        b["tags"].as_array().is_some_and(|tags| {
                            tags.iter().map(|r| r.as_str().unwrap_or("")).collect::<BTreeSet<_>>()
                                == job["rows"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .map(|r| r["short_tag"].as_str().unwrap_or(""))
                                    .collect::<BTreeSet<_>>()
                        })
                    } else {
                        job["rows"][0]["short_tag"] == b["char"] && job["reason"] == b["reason"]
                    };
                    if !same || job["action"] != b["action"] {
                        return invalid("上次索引任务尚待恢复，请先重试原任务");
                    }
                    self.next_retry = Instant::now() + Duration::from_secs(60);
                    self.start(lib, json!({"action":"resume", "allow_network":b["allow_network"]}))?;
                } else {
                    self.next_retry = Instant::now() + Duration::from_secs(60);
                    self.start(lib, b)?;
                }
            }
            _ => return invalid("未知操作"),
        };
        self.info(lib)
    }
    /// 基础角色索引（#607）的状态（GET /api/index/base）；GET /api/index 的回复冻结，另开这个路由。
    pub fn base_info(&self, lib: &Library) -> Result<Json> {
        let release = crate::character_index::release();
        let meta = base_meta(lib)?;
        let mark = read(&self.shared.join(BASE_MARK), Value::Null);
        let running = self.task.as_ref().is_some_and(Task::running)
            && self.state.lock().expect("index state mutex poisoned").value()["kind"] == "base";
        Ok(Json::from(json!({
            "included": release.is_some(),
            "version": release.map(|(r, _)| r.version.clone()),
            "characters": release.map(|(r, _)| r.characters),
            "file": release.map(|(_, f)| json!({"path": f.path, "size": f.size, "sha256": f.sha256})),
            "available": crate::character_index::present(&self.resources),
            "enabled": meta.is_some(),
            "mode": meta.as_ref().map(|m| m["mode"].clone()),
            "withdrawn": mark["enabled"] == false,
            "running": running,
            "writable": !lib.frozen(),
        })))
    }
    /// POST /api/index/base `{"enable": true|false}`：用户启用或撤回基础角色索引，同一台电脑的其他图库下次打开时跟着同步。
    pub fn base_post(&mut self, lib: &Library, body: &Json, busy: bool) -> Result<Json> {
        let enable = match body {
            Json::Object(fields) if fields.len() == 1 => match body.get("enable") {
                Some(Json::Scalar(Value::Bool(enable))) => *enable,
                _ => return invalid("基础角色索引请求字段不正确"),
            },
            _ => return invalid("基础角色索引请求字段不正确"),
        };
        if lib.frozen() {
            return invalid("测试图库不能更新索引");
        }
        if busy {
            return invalid("正在与 Photos 同步，完成后再更新角色索引");
        }
        if self.journal.exists() {
            return invalid("上次索引任务尚待恢复，请先重试原任务");
        }
        self.start(lib, json!({"action":"base","enable":enable}))?;
        self.base_info(lib)
    }
    /// 让这个图库跟上这台电脑的基础角色索引（#607）：包已下载、没有撤回过、图库还没启用（或启用的是旧版本）时启用；
    /// 在别的图库撤回过、这个图库还启用着时撤回。不联网。失败后十分钟内不再自动重试。
    pub fn sync_base(&mut self, lib: &Library, busy: bool) {
        if busy
            || lib.frozen()
            || self.task.as_ref().is_some_and(Task::running)
            || self.journal.exists()
            || Instant::now() < self.base_retry
        {
            return;
        }
        let Some(enable) = self.base_wanted(lib) else { return };
        self.base_retry = Instant::now() + Duration::from_secs(600);
        if let Err(error) = self.start(lib, json!({"action":"base","enable":enable})) {
            eprintln!("基础角色索引未能自动{}：{error}", if enable { "启用" } else { "撤回" });
        }
    }
    /// 这个图库的基础角色索引还会启用（#630）：包已在本机、正随识别模型下载，或者识别模型还没到齐（不带模型的安装包首次
    /// 准备时，基础索引等识别模型就位后才启用），没有撤回过，图库还没启用当前版本，而且没有刚试过失败（`sync_base` 失败后
    /// 十分钟内不再试）。角色资料包的默认启用等它启用完，CCIP 部分记下的就是启用以后的
    /// 索引代数，不会一启用就提示重新检查。
    pub fn base_outstanding(&self, lib: &Library) -> bool {
        if std::env::var_os("ILLUSTASH_INDEX_WORKER").is_none() {
            return false;
        }
        let Some((_, file)) = crate::character_index::release() else { return false };
        if read(&self.shared.join(BASE_MARK), Value::Null)["enabled"] == false {
            return false;
        }
        if base_meta(lib).ok().flatten().is_some_and(|m| m["sha256"] == file.sha256.as_str()) {
            return false;
        }
        if self.task.as_ref().is_some_and(Task::running) {
            return true;
        }
        if Instant::now() < self.base_retry {
            return false;
        }
        crate::character_index::present(&self.resources)
            || crate::app_models::downloading()
            || crate::app_models::waiting(&self.resources)
    }
    fn base_wanted(&self, lib: &Library) -> Option<bool> {
        std::env::var_os("ILLUSTASH_INDEX_WORKER")?;
        let (_, file) = crate::character_index::release()?;
        if crate::app_models::waiting(&self.resources) {
            return None;
        }
        let meta = base_meta(lib).ok()?;
        if read(&self.shared.join(BASE_MARK), Value::Null)["enabled"] == false {
            return meta.is_some().then_some(false);
        }
        if !crate::character_index::present(&self.resources) {
            return None;
        }
        match meta {
            None => Some(true),
            Some(m) if m["sha256"] != file.sha256.as_str() => Some(true),
            Some(_) => None,
        }
    }
    pub fn recover(&mut self, lib: &Library, busy: bool) {
        if self.task.as_ref().is_some_and(Task::running) || Instant::now() < self.next_retry || lib.frozen() {
            return;
        }
        if self.journal.exists() {
            // 暂停的任务等人确认；重启之后显示它停在哪里。
            if let Some(waiting) = paused(&self.journal) {
                let mut state = self.state.lock().expect("index state mutex poisoned");
                if state.status() == Status::Idle {
                    *state = waiting.into();
                }
            } else if !busy {
                self.next_retry = Instant::now() + Duration::from_secs(60);
                let _ = self.start(lib, json!({"action":"resume"}));
            }
            return;
        }
        if !self.automatic || !self.network || lib.frozen() {
            return;
        }
        let prefs = crate::app_settings::startup(lib)
            .ok()
            .and_then(|v| serde_json::from_str::<Value>(&v.dump()).ok())
            .unwrap_or_default();
        if prefs["index_check"] == false {
            return;
        }
        let at = read(&self.dir.join("check.json"), Value::Null)["checked_at"]
            .as_str()
            .and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok());
        if at.is_none_or(|t| chrono::Local::now().naive_local().signed_duration_since(t).num_seconds() >= 7 * 86400) {
            self.next_retry = Instant::now() + Duration::from_secs(60);
            let _ = self.start(lib, json!({"action":"check"}));
        }
    }
    fn start(&mut self, lib: &Library, b: Value) -> Result<()> {
        if self.task.as_ref().is_some_and(Task::running) {
            return invalid("角色索引正在更新，请稍后");
        };
        let action = b["action"].as_str().unwrap_or("");
        let mut rows = vec![];
        if action == "apply" {
            let check = read(&self.dir.join("check.json"), json!({}));
            let offered = check["missing"].as_array().cloned().unwrap_or_default();
            let Some(tags) = b["tags"].as_array().filter(|t| !t.is_empty()) else {
                return invalid("请从最近一次检查的结果里选择角色");
            };
            for t in tags {
                let r = offered
                    .iter()
                    .find(|r| r["short_tag"] == *t)
                    .ok_or_else(|| Error::Invalid("请从最近一次检查的结果里选择角色".into()))?;
                if !rows.contains(r) {
                    rows.push(r.clone());
                }
            }
        }
        if action == "drop" {
            let id = b["char"].as_str().unwrap_or("");
            let m = meta(&self.shared, id);
            if m["source"] != "danbooru-patch" {
                return invalid("只能放弃从 Danbooru 补进来的角色");
            };
            if b["reason"].as_str().is_none_or(|r| r.trim().is_empty() || r.trim().chars().count() > 200) {
                return invalid("请写一句放弃的原因");
            };
            rows.push(m);
        }
        if action != "check" && action != "resume" && std::env::var_os("ILLUSTASH_INDEX_WORKER").is_none() {
            return invalid("角色索引计算 worker 未配置");
        };
        if action != "check" && action != "resume" && crate::app_models::waiting(&self.resources) {
            return invalid("识别模型还没下载；下载后才能更新角色索引");
        }
        if action == "base" {
            if crate::character_index::release().is_none() {
                return invalid("这个版本没有基础角色索引");
            }
            if b["enable"] == true && !crate::character_index::present(&self.resources) {
                return Err(Error::Conflict(crate::character_index::MISSING.into()));
            }
        }
        self.task.take();
        let identity = TaskIdentity::new(lib, self.generation, &self.session)?;
        let lib = lib.clone();
        let (resources, shared, dir, journal, state, network) = (
            self.resources.clone(),
            self.shared.clone(),
            self.dir.clone(),
            self.journal.clone(),
            self.state.clone(),
            self.network || b["allow_network"] == true,
        );
        // 失败时说它是什么任务：继续运行的任务就是它所接续的添加或移除。
        let kind = match (action, std::fs::read(&self.journal).ok()) {
            ("resume", Some(bytes)) => {
                match serde_json::from_slice::<Value>(&bytes).ok().map(|j| j["action"].clone()) {
                    Some(Value::String(a)) if a == "drop" => "drop",
                    Some(Value::String(a)) if a == "base" => "base",
                    _ => "add",
                }
            }
            ("check", _) => "check",
            ("drop", _) => "drop",
            ("base", _) => "base",
            _ => "add",
        };
        let status = match action {
            "check" => "checking",
            "drop" | "base" => "building",
            _ => "fetching",
        };
        let mut initial = json!({"status":status,"started_at":now()});
        // 基础角色索引的任务从一开始就带 kind，`GET /api/index/base` 才看得出它在运行（#607）。其他动作的运行中状态
        // 是冻结的 /api/index 回复，照旧不带。
        if kind == "base" {
            initial["kind"] = kind.into();
        }
        *state.lock().expect("index state mutex poisoned") = initial.into();
        self.task = Some(Task::start_with(identity, deadline(), move |ctx| {
            let progress = PatchProgress { state: &state, publication: Cell::new(Publication::NotCommitted) };
            let run = || {
                let net = crate::app_name_sources::Source::new(lib.db(), ctx.cancellation_arc(), network, true)?;

                if b["action"] == "check" {
                    let checked = candidates(&lib, &resources, &shared, &dir, &net)?;
                    let found = checked["missing"].as_array().expect("candidates always returns a missing array").len();
                    ctx.check()?;
                    write_check(&dir.join("check.json"), &checked)?;
                    write_json(&dir.join("coverage.json"), &coverage_json(&checked))?;
                    update(&state, json!({"status":"checked","found":found,"finished_at":now()}));
                    pause_after_done();
                    Ok(())
                } else {
                    patch(
                        &lib,
                        &PatchPaths { resources: &resources, shared: &shared, dir: &dir, journal: &journal },
                        &b,
                        &rows,
                        &net,
                        &ctx,
                        &progress,
                    )
                }
            };
            let result = run();
            if let Err(ref e) = result {
                // 只写原因：错误的类别（"不合法：…"）是给代码看的，不是给人看的（#287）。
                let message = ctx.stop_reason().unwrap_or_else(|| e.reason()).chars().take(300).collect::<String>();
                let d = crate::diagnostics::failure(
                    crate::diagnostics::Operation::Index,
                    crate::diagnostics::Stage::Background,
                    e,
                    progress.publication.get(),
                );
                update(
                    &state,
                    json!({"status":"error","kind":kind,"message":message,"finished_at":now(), "diagnostic":d.value()}),
                );
            }
            result
        }));
        Ok(())
    }
}
/// 索引运行在 ILLUSTASH_INDEX_TIMEOUT_MS（默认 10 分钟）内没有进展就停止；每下载一张图、worker 每报告一步都算进展，
/// 所以添加很多角色不会在开始后固定的时间被截断（#287）。ILLUSTASH_INDEX_MAX_MS（默认 24 小时）限制整次运行。
fn deadline() -> crate::app_tasks::Deadline {
    let ms = |name: &str, default: u64| {
        Duration::from_millis(
            std::env::var(name).ok().and_then(|v| v.parse::<u64>().ok()).filter(|v| *v > 0).unwrap_or(default),
        )
    };
    crate::app_tasks::Deadline {
        idle: Some(ms("ILLUSTASH_INDEX_TIMEOUT_MS", 600_000)),
        limit: Some(ms("ILLUSTASH_INDEX_MAX_MS", 86_400_000)),
    }
}
// 跨进程的共享索引锁。崩溃时操作系统会释放它；不会有残留的锁文件挡住重启。
struct SharedLock(std::fs::File);
impl SharedLock {
    fn acquire(dir: &Path, ctx: &TaskContext) -> Result<Self> {
        if crate::app_file_lock::ensure_supported().is_err() {
            return invalid("当前平台尚不支持索引计算 worker");
        }
        std::fs::create_dir_all(dir)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join("native.lock"))?;
        loop {
            ctx.check()?;
            match crate::app_file_lock::acquire(&file) {
                Ok(()) => return Ok(Self(file)),
                Err(e) if crate::app_file_lock::should_retry(&e) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => return Err(e.into()),
            }
        }
    }
}
impl Drop for SharedLock {
    fn drop(&mut self) {
        let _ = crate::app_file_lock::release(&self.0);
    }
}

fn copy_tree(src: &Path, dst: &Path, ctx: &TaskContext) -> Result<()> {
    if !src.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        ctx.check()?;
        let e = e?;
        if e.file_type()?.is_symlink() {
            return invalid("索引资源不能是符号链接");
        }
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &dst.join(e.file_name()), ctx)?;
        } else {
            std::fs::copy(e.path(), dst.join(e.file_name()))?;
        }
    }
    Ok(())
}
/// 一张参考图最多能占的大小。
const PICTURE_MAX: u64 = 2_000_000;
/// 添加等人确认时显示，并保留在任务日志里，直到对方让它继续（#287）：重启或自动重试之后，
/// 没有人为这次运行允许联网，而参考图还没下载齐。
const NETWORK_PAUSE: &str = "继续下载参考图需要你确认联网";
fn download(address: &str, path: &Path, ctx: &TaskContext, authorized: bool) -> Result<()> {
    ctx.check()?;
    let u = url::Url::parse(address).map_err(|_| Error::Invalid("参考图地址不正确".into()))?;
    let local = matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    if (!local && !authorized)
        || !u.username().is_empty()
        || u.password().is_some()
        || (!local && (u.scheme() != "https" || !matches!(u.host_str(), Some("cdn.donmai.us" | "danbooru.donmai.us"))))
        || (local && u.scheme() != "http")
    {
        return invalid("参考图来源尚未授权或不在 Danbooru 来源范围");
    };
    let mut last = None;
    for _ in 0..3 {
        ctx.check()?;
        match ureq::AgentBuilder::new().redirects(0).timeout(Duration::from_secs(25)).build().get(address).call() {
            Ok(r) => {
                let mut bytes = vec![];
                if r.into_reader().take(PICTURE_MAX + 1).read_to_end(&mut bytes).is_err() {
                    last = Some(Error::Network("参考图下载失败".into()));
                    continue;
                }
                if bytes.len() as u64 > PICTURE_MAX {
                    return Err(Error::Network("参考图超过 2 MB".into()));
                };
                ctx.check()?;
                std::fs::create_dir_all(path.parent().expect("index file path must have a parent"))?;
                // 要么完整，要么没有：留给下次运行的图从不是截断的。
                let tmp = path.with_extension(format!("img.{}.tmp", crate::store::new_key()?));
                std::fs::write(&tmp, bytes)?;
                std::fs::rename(tmp, path)?;
                ctx.progressed();
                return Ok(());
            }
            Err(_) => last = Some(Error::Network("参考图下载失败".into())),
        }
    }
    Err(last.expect("download retry loop always records a failure"))
}
/// 任务日志里的标记，表示任务在等人让它继续；自动重试不动它。
const PAUSED: &str = "paused";
/// 访问这个服务是否需要对方同意：除了本机上的合成服务，都需要；测试也可以让合成服务算作远程，以便看到添加在等待。
fn needs_consent(base: &str) -> bool {
    !url::Url::parse(base).is_ok_and(|u| matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")))
        || std::env::var_os("ILLUSTASH_TEST_INDEX_CONSENT").is_some()
}
/// 一个角色的参考图是否都已放在任务文件夹里。
fn fetched(folder: &Path, cid: &str) -> bool {
    meta(folder, cid)["source"] == "danbooru-patch"
        || folder.join("char-patch").join(format!("{cid}-refs.json")).exists()
}
/// 在任务日志里等待的任务的状态（如果有的话）。
fn paused(journal: &Path) -> Option<Value> {
    let job: protocol::IndexJob = protocol::read(journal).ok()?;
    job.extra.get(PAUSED).filter(|_| job.phase == protocol::JobPhase::Prepared).cloned()
}
struct PatchPaths<'a> {
    resources: &'a Path,
    shared: &'a Path,
    dir: &'a Path,
    journal: &'a Path,
}
struct PatchProgress<'a> {
    state: &'a Mutex<State>,
    publication: Cell<Publication>,
}

fn patch(
    lib: &Library,
    paths: &PatchPaths<'_>,
    b: &Value,
    rows: &[Value],
    net: &crate::app_name_sources::Source,
    ctx: &TaskContext,
    progress: &PatchProgress<'_>,
) -> Result<()> {
    let (shared, dir, journal) = (paths.shared, paths.dir, paths.journal);
    let _lock = SharedLock::acquire(dir, ctx)?;
    let mut job: protocol::IndexJob =
        if journal.exists() { protocol::read(journal)? } else { prepare_job(lib, paths, b, rows, ctx)? };
    if job.version != 1 || job.package != lib.instance_path() {
        return invalid("索引恢复包实例不匹配");
    };
    let folder = job.folder.clone();
    if folder.parent() != Some(lib.instance_path().join("index-jobs").as_path())
        || folder.file_name().and_then(|n| n.to_str()) != Some(job.job_id.as_str())
    {
        return invalid("索引任务目录越界");
    }
    if job.phase == protocol::JobPhase::Prepared {
        job.identity = ctx.identity().clone();
        // 继续运行：从这里起的失败照旧重试，暂停会再次写入。等人确认的任务可能等过了几次同步；它从图库现在的样子继续，
        // 保留已有的参考图（#287）。
        if job.extra.remove(PAUSED).is_some() {
            job.before = snapshot_library(lib, &folder, ctx)?;
        }
        write(journal, &protocol::value(&job)?)?;
        if job.action == protocol::IndexAction::Apply
            && !fetch_references(&job.rows, &folder, net, ctx, progress.state)?
        {
            let done = job.rows.iter().filter(|r| fetched(&folder, &r.short_tag)).count();
            let paused =
                json!({"status":"paused","kind":"add","done":done,"total":job.rows.len(),"message":NETWORK_PAUSE});
            job.extra.insert(PAUSED.into(), paused.clone());
            write(journal, &protocol::value(&job)?)?;
            *progress.state.lock().expect("index state mutex poisoned") = paused.into();
            return Ok(());
        }
        let response = compute(&job, &folder, ctx, progress.state)?;
        validate(lib, &response, &folder.join("response.json"))?;
        job.result = Some(response);
        job.phase = protocol::JobPhase::Ready;
        write(journal, &protocol::value(&job)?)?;
    }
    ctx.receive(ctx.identity(), || commit(lib, shared, dir, journal, &mut job, ctx, progress))?;
    Ok(())
}
/// 新任务：它自己的文件夹，里面有 source.sqlite 的快照和 worker 要读的每个输入，以及它的任务日志。
fn prepare_job(
    lib: &Library,
    paths: &PatchPaths<'_>,
    b: &Value,
    rows: &[Value],
    ctx: &TaskContext,
) -> Result<protocol::IndexJob> {
    let (shared, dir, journal) = (paths.shared, paths.dir, paths.journal);
    let id = &ctx.identity().task_id;

    let folder = lib.instance_path().join("index-jobs").join(id);
    std::fs::create_dir_all(&folder)?;
    let mut prepared = PreparedStage(folder.clone(), false);
    let before = snapshot_library(lib, &folder, ctx)?;
    copy_tree(&shared.join("char-index"), &folder.join("char-index"), ctx)?;
    copy_tree(&shared.join("game-chars"), &folder.join("game-chars"), ctx)?;
    copy_tree(&dir.join("raw"), &folder.join("char-patch/raw"), ctx)?;
    copy_tree(&dir.join("rejected"), &folder.join("char-patch/rejected"), ctx)?;
    std::fs::create_dir_all(folder.join("char-patch/rejected"))?;
    let mut plan = read(&dir.join("plan.json"), json!([])).as_array().cloned().unwrap_or_default();
    for r in rows {
        if !plan.iter().any(|x| x["short_tag"] == r["short_tag"]) {
            plan.push(r.clone());
        }
    }
    write(&folder.join("char-patch/plan.json"), &json!(plan))?;
    let base = base_plan(lib, paths, b, &folder)?;
    let job = protocol::parse(
        json!({
            "version":1,"package":lib.instance_path(),"job_id":id,"identity":ctx.identity(),"action":b["action"],
            "reason":b["reason"],"rows":rows,"folder":folder,"before":before,"phase":"prepared","base":base
        }),
        journal,
    )?;

    write(journal, &protocol::value(&job)?)?;
    prepared.1 = true;
    Ok(job)
}
/// 这个任务带不带基础角色索引（#607），以及启用、撤回时要不要改图库内容。带的话把核对过的包复制进任务目录。
/// - 启用：图库已有同一版本的索引（角色表里有包里的每个角色）时只装共享锚点（`adopted`），
///   不重写证据、不动主归属；否则按包重建角色表和索引候选（`rebuilt`，版本不同时就是来源替换）。
/// - 撤回：按启用时的方式还原；`adopted` 的图库内容本来没动，只把共享锚点换回不带包的。
/// - 其他索引任务：这台电脑启用了基础索引就带上，否则不带，重建的结果与启用状态一致。
fn base_plan(lib: &Library, paths: &PatchPaths<'_>, b: &Value, folder: &Path) -> Result<Value> {
    let mark = read(&paths.shared.join(BASE_MARK), Value::Null);
    let include = if b["action"] == "base" { b["enable"] == true } else { mark["enabled"] == true };
    let mut plan = json!({"include": include});
    if include {
        plan["sha256"] = json!(crate::character_index::copy_verified(
            paths.resources,
            &folder.join(crate::character_index::JOB_FILE)
        )?);
    }
    if b["action"] == "base" {
        let rebuild = if include {
            let entries = crate::character_index::load_job(folder)?.unwrap_or_default();
            !same_version(&crate::app_library::read_connection(&folder.join("pi.db"))?, &entries)?
        } else {
            base_meta(lib)?.is_some_and(|m| m["mode"] == "rebuilt")
        };
        plan["enable"] = json!(include);
        plan["write_source"] = json!(rebuild);
    }
    Ok(plan)
}
/// 图库的角色表已经是同一版本的精选索引：包里的每个角色都在。不要求每个都有阈值：已有图库当年装载时个别角色
/// 没算出锚点，这样的角色在图库里本来就不认领，装上共享锚点后仍因阈值为空而不认领，图库不变。
fn same_version(src: &Connection, entries: &[crate::character_index::Entry]) -> Result<bool> {
    if entries.is_empty() {
        return Ok(false);
    }
    let have = src
        .prepare("SELECT char_id FROM characters")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    Ok(entries.iter().all(|e| have.contains(&e.id)))
}
/// 图库启用基础角色索引的记录；没启用时为 None。
fn base_meta(lib: &Library) -> Result<Option<Value>> {
    let c = crate::app_library::read_connection(&lib.db())?;
    let raw: Option<String> =
        c.query_row("SELECT value FROM meta WHERE key=?", [BASE_META], |r| r.get(0)).optional()?;
    Ok(raw.and_then(|v| serde_json::from_str(&v).ok()))
}
/// 启用或撤回之后记下这个图库的状态。重放同一任务时结果相同。
fn record_base(lib: &Library, job: &protocol::IndexJob) -> Result<()> {
    let base = job.extra.get("base").cloned().unwrap_or(Value::Null);
    let mut app = writer(&lib.db())?;
    let tx = app.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    crate::schema::begin_writing(&tx)?;
    if base["enable"] == true {
        let mode = if base["write_source"] == true { "rebuilt" } else { "adopted" };
        let value = json!({"sha256": base["sha256"], "mode": mode}).to_string();
        tx.execute("INSERT OR REPLACE INTO meta(key,value) VALUES (?,?)", params![BASE_META, value])?;
    } else {
        tx.execute("DELETE FROM meta WHERE key=?", [BASE_META])?;
    }
    crate::schema::commit_writing(tx)?;
    Ok(())
}
/// 任务等待期间图库可能改动的内容在任务里的副本：来源数据库、图库的 features 和相册映射。
/// 共享索引文件只在索引任务里改变，而索引任务会等这个任务。
/// 返回提交结果时用来核对的指纹。
fn snapshot_library(lib: &Library, folder: &Path, ctx: &TaskContext) -> Result<String> {
    let src = crate::app_library::read_connection(&lib.source())?;
    let _ = std::fs::remove_file(folder.join("pi.db"));
    backup_db(&src, &folder.join("pi.db"), ctx)?;
    for name in ["full-features.npy", "full-manifest.csv"] {
        let from = lib.instance_path().join("features").join(name);
        if from.exists() {
            std::fs::copy(from, folder.join(name))?;
        } else {
            let _ = std::fs::remove_file(folder.join(name));
        }
    }
    let album = lib.instance_path().join("album-char-map.json");
    if album.exists() {
        std::fs::copy(album, folder.join("album-char-map.json"))?;
    } else {
        write(&folder.join("album-char-map.json"), &json!({"map":{}}))?;
    }
    let mut snapshot = crate::app_library::read_connection(&folder.join("pi.db"))?;
    crate::app_recognition::require_published_inputs(&crate::app_library::read_connection(&lib.db())?, &snapshot)?;
    with_index_progress(&mut snapshot, ctx, |c| fingerprint(c, ctx))
}
/// 为每个新角色下载最多二十张 Danbooru 参考图，除非它的索引已经打过补丁。写下了图片列表的角色就算完成；
/// 同一任务后来的运行会跳过它，并复用停下时那个角色的图片（#287）。还要下载的角色需要联网、
/// 而没有人为这次运行允许（重启或自动重试）时返回 false：这时添加等人确认。
fn fetch_references(
    rows: &[protocol::IndexRow],
    folder: &Path,
    net: &crate::app_name_sources::Source,
    ctx: &TaskContext,
    state: &Mutex<State>,
) -> Result<bool> {
    for (i, r) in rows.iter().enumerate() {
        ctx.check()?;
        let cid = r.short_tag.as_str();
        if !safe(cid) {
            return invalid("索引角色 ID 不正确");
        };
        update(state, json!({"status":"fetching","done":i,"total":rows.len()}));
        if fetched(folder, cid) {
            continue;
        }
        if !net.authorized && needs_consent(&net.db_base) {
            return Ok(false);
        }
        let query = [
            ("tags", format!("{} solo rating:g,s", r.tag)),
            ("limit", "20".into()),
            ("only", "id,tag_string_general,media_asset".into()),
        ];
        let posts = net.fetch(&crate::app_name_sources::url(&net.db_base, "/posts.json", &query), None, None)?;
        let Some(posts) = posts.as_array().filter(|p| p.len() <= 20) else {
            return invalid("参考图帖子响应不正确");
        };
        let mut refs = vec![];
        for p in posts {
            let variant =
                p["media_asset"]["variants"].as_array().into_iter().flatten().find(|v| v["type"] == "720x720");
            if let Some(v) = variant {
                let pid = p["id"].as_u64().ok_or_else(|| Error::Invalid("参考图帖子 ID 不正确".into()))?;
                let path = folder.join("char-patch/img").join(cid).join(format!("{pid}.img"));
                if !path.exists() {
                    if let Err(e) = download(v["url"].as_str().unwrap_or(""), &path, ctx, net.authorized) {
                        ctx.check()?;
                        if matches!(e, Error::Network(_)) {
                            continue;
                        } else {
                            return Err(e);
                        }
                    }
                }
                refs.push(json!({"id":pid,"path":path,"general":p["tag_string_general"]}));
            }
        }
        write(&folder.join("char-patch").join(format!("{cid}-refs.json")), &json!(refs))?;
        ctx.progressed();
    }
    Ok(true)
}
/// 在任务文件夹上运行索引 worker，转发它的进度，并读回它经过校验的响应。
fn compute(
    job: &protocol::IndexJob,
    folder: &Path,
    ctx: &TaskContext,
    state: &Mutex<State>,
) -> Result<protocol::IndexResponse> {
    let request = protocol::value(&protocol::IndexRequest {
        format: protocol::Format::Index,
        identity: ctx.identity(),
        before: &job.before,
        action: job.action,
        rows: &job.rows,
        reason: &job.reason,
        review: folder,
    })?;
    write(&folder.join("request.json"), &request)?;
    update(state, json!({"status":"building"}));
    let executable = std::env::var_os("ILLUSTASH_INDEX_WORKER")
        .ok_or_else(|| Error::Invalid("角色索引计算 worker 未配置".into()))?;
    let process_ctx = ctx.clone();
    let worker_folder = folder.to_owned();
    let process = std::thread::spawn(move || {
        process_ctx.process(
            std::process::Command::new(executable)
                .env(crate::app_tasks::WORKER_KIND_ENV, "index")
                .arg(worker_folder.join("request.json"))
                .arg(worker_folder.join("response.json")),
        )
    });
    while !process.is_finished() {
        if ctx.check().is_err() {
            ctx.cancel();
        } else {
            report_progress(folder, ctx, state);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let status = process.join().map_err(|_| Error::Invalid("索引计算进程等待失败".into()))??;
    if !status.success() {
        return invalid("角色索引计算 worker 失败");
    };
    ctx.check()?;
    if std::fs::metadata(folder.join("response.json"))?.len() > 64_000_000 {
        return invalid("索引结果超过 64 MB");
    }
    let response: protocol::IndexResponse = protocol::read(&folder.join("response.json"))?;
    if &response.identity != ctx.identity()
        || response.format != protocol::Format::Index
        || response.before != job.before
    {
        return invalid("索引结果身份或版本不匹配");
    };
    Ok(response)
}
/// 转发 worker 最后写进 progress.json 的步骤和计数，如果它们属于这个任务。
fn report_progress(folder: &Path, ctx: &TaskContext, state: &Mutex<State>) {
    let p = read(&folder.join("progress.json"), Value::Null);
    if p["identity"] != json!(ctx.identity()) {
        return;
    }
    let step = match p["step"].as_str() {
        Some("fetch") => "fetching",
        Some("clean") => "cleaning",
        Some("build") => "building",
        Some("scan") => "scanning",
        _ => return,
    };
    let mut fields = json!({"status":step});
    for key in ["done", "total"] {
        if let Some(n) = p[key].as_u64().filter(|n| *n <= 100000) {
            fields[key] = n.into();
        }
    }
    let before = state.lock().expect("index state mutex poisoned").value();
    if ["status", "done", "total"].iter().any(|k| fields.get(*k).is_some_and(|v| before.get(*k) != Some(v))) {
        ctx.progressed();
    }
    update(state, fields);
}
fn validate(lib: &Library, v: &protocol::IndexResponse, file: &Path) -> Result<()> {
    let field_error =
        |field: &str, message: &str| Error::Invalid(format!("{file}: 字段 {field}: {message}", file = file.display()));
    let chars = &v.characters;
    if chars.len() > 100000 {
        return Err(field_error("characters", "索引角色结果不正确"));
    }
    let mut ids = BTreeSet::new();
    for (index, r) in chars.iter().enumerate() {
        let id = r.char_id.as_str();
        if !safe(id) {
            return Err(field_error(&format!("characters[{index}].char_id"), "索引角色 ID 不正确"));
        }
        if !ids.insert(id) {
            return invalid("索引角色重复");
        };
        for key in [
            "danbooru_tag",
            "wd_tag",
            "copyright",
            "base_tag",
            "skin",
            "gender",
            "core_tags",
            "name_en",
            "name_ja",
            "name_zh",
        ] {
            if !r.columns.get(key).unwrap_or(&Value::Null).is_null()
                && r.columns.get(key).unwrap_or(&Value::Null).as_str().is_none_or(|s| s.len() > 10000)
            {
                return Err(field_error(&format!("characters[{index}].{key}"), "索引角色文本字段不正确"));
            }
        }
        for key in ["post_count", "n_ref", "anchor_row"] {
            if !r.columns.get(key).unwrap_or(&Value::Null).is_null()
                && !r
                    .columns
                    .get(key)
                    .unwrap_or(&Value::Null)
                    .as_i64()
                    .is_some_and(|n| (0..=1_000_000_000).contains(&n))
            {
                return Err(field_error(&format!("characters[{index}].{key}"), "索引角色计数字段不正确"));
            }
        }
        for key in ["gender_conf", "anchor_p50", "anchor_p75", "anchor_p90"] {
            if !r.columns.get(key).unwrap_or(&Value::Null).is_null()
                && !r.columns.get(key).unwrap_or(&Value::Null).as_f64().is_some_and(|n| (0.0..=1.0).contains(&n))
            {
                return Err(field_error(&format!("characters[{index}].{key}"), "索引角色阈值字段不正确"));
            }
        }
    }
    let src = crate::app_library::read_connection(&lib.source())?;
    let assets = src
        .prepare("SELECT uuid FROM assets")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    let old_labels = crate::app_migrate_data::rows(&src, "SELECT uuid,value,conf FROM labels WHERE dim='char_ext'")?;
    let labels = &v.labels;
    let mut seen = BTreeSet::new();
    for r in labels {
        if (!assets.contains(&r.0) && !old_labels.contains(&json!(r)))
            || !ids.contains(r.1.as_str())
            || !(0.0..=1.0).contains(&r.2)
        {
            return invalid("索引认领越界");
        };
        if !seen.insert((&r.0, &r.1)) {
            return invalid("索引认领重复");
        }
    }
    let aliases = &v.aliases;
    for r in aliases {
        if !safe(&r.0) {
            return invalid("索引别名越界");
        }
    }
    Ok(())
}
fn commit(
    lib: &Library,
    shared: &Path,
    dir: &Path,
    journal: &Path,
    job: &mut protocol::IndexJob,
    ctx: &TaskContext,
    progress: &PatchProgress<'_>,
) -> Result<()> {
    ctx.check()?;
    let id = job.job_id.clone();
    let folder = job.folder.clone();
    let base = job.extra.get("base").cloned().unwrap_or(Value::Null);
    // 只装共享锚点的启用和撤回（#607 的 `adopted`）不改来源库，也就没有要投影的候选。
    let write_source = job.action != protocol::IndexAction::Base || base["write_source"] == true;
    let mut src = writer(&lib.source())?;
    if write_source {
        if let Some(stored) = stored_run(&src, &id)? {
            *job = stored;
        } else {
            commit_source(lib, shared, dir, journal, job, ctx, &mut src)?;
        }
        progress.publication.set(Publication::MayBePartial);
    }
    // 已提交源结果的重放仍可能遇到新的识别回执；在发布共享文件和应用候选之前再检查。
    let app = crate::app_library::read_connection(&lib.db())?;
    let projected: bool =
        app.query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE key=?)", [format!("index_job:{}", job.job_id)], |r| {
            r.get(0)
        })?;
    if !projected {
        crate::app_recognition::require_published_inputs(&app, &src)?;
    }
    drop(app);
    publish(shared, dir, &folder, job, &progress.publication)?;
    update(progress.state, json!({"status":"importing"}));
    let changed = if write_source { import(lib, &src, job, ctx)? } else { 0 };
    if job.action == protocol::IndexAction::Base {
        if write_source {
            prune_backups(dir)?;
        }
        record_base(lib, job)?;
        progress.publication.set(Publication::Committed);
        std::fs::remove_file(journal)?;
        let _ = std::fs::remove_dir_all(&folder);
        update(
            progress.state,
            json!({"status":"done","kind":"base","enabled":base["enable"] == true,"changed":changed,"finished_at":now()}),
        );
        pause_after_done();
        return Ok(());
    }
    progress.publication.set(Publication::Committed);
    let drop = job.action == protocol::IndexAction::Drop;
    let chars = outcomes(job, shared, dir, &src)?;
    record(dir, &id, &chars, changed, drop)?;
    prune_backups(dir)?;
    let added = chars.iter().filter(|r| r["status"] == "added").count();
    // 报告完成之前先清理：看到完成的人必须找不到残留的任务日志或任务文件夹（#123）。
    std::fs::remove_file(journal)?;
    let _ = std::fs::remove_dir_all(&folder);
    update(
        progress.state,
        json!({"status":"done","kind":if drop{"drop"}else{"add"},"added":added,"changed":changed,"finished_at":now()}),
    );
    pause_after_done();
    Ok(())
}
/// 之前的尝试提交这个任务时 source.sqlite 记下的任务。
fn stored_run(src: &Connection, id: &str) -> Result<Option<protocol::IndexJob>> {
    let has_runs: bool =
        src.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='native_index_runs')", [], |r| r.get(0))?;
    if !has_runs {
        return Ok(None);
    }
    let stored: Option<String> =
        src.query_row("SELECT result FROM native_index_runs WHERE id=?", [id], |r| r.get(0)).optional()?;
    let Some(stored) = stored else { return Ok(None) };
    Ok(Some(protocol::parse(
        serde_json::from_str(&stored)
            .map_err(|e| Error::Invalid(format!("source.sqlite native_index_runs.result: {e}")))?,
        Path::new("source.sqlite native_index_runs.result"),
    )?))
}
/// 备份之后，把校验过的结果连同任务记录写进 source.sqlite；如果准备任务之后输入变了，就拒绝（并放弃任务）。
fn commit_source(
    lib: &Library,
    shared: &Path,
    dir: &Path,
    journal: &Path,
    job: &mut protocol::IndexJob,
    ctx: &TaskContext,
    src: &mut Connection,
) -> Result<()> {
    let folder = job.folder.clone();
    let changed_inputs = |message: &str| -> Result<()> {
        std::fs::remove_file(journal)?;
        let _ = std::fs::remove_dir_all(&folder);
        invalid(message)
    };
    if with_index_progress(src, ctx, |c| fingerprint(c, ctx))? != job.before {
        return changed_inputs("索引计算输入已改变，请重试检查");
    };
    let result = job
        .result
        .as_ref()
        .ok_or_else(|| Error::Invalid(format!("{file}: 字段 result: ready job 缺少结果", file = journal.display())))?;
    validate(lib, result, journal)?;
    let changed = changed_labels(src, result)?;
    job.extra.insert("changed".into(), json!(changed));
    let backup = dir.join("backup").join(&job.job_id);
    std::fs::create_dir_all(&backup)?;
    backup_db(src, &backup.join("characters.sqlite"), ctx)?;
    for name in ["char-anchors.npy", "char-anchors.txt"] {
        if shared.join(name).exists() {
            std::fs::copy(shared.join(name), backup.join(name))?;
        }
    }
    with_index_progress(src, ctx, |src| {
        let tx = src.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if fingerprint(&tx, ctx)? != job.before {
            return changed_inputs("索引计算输入已改变");
        };
        crate::app_recognition::require_published_inputs(&crate::app_library::read_connection(&lib.db())?, &tx)?;
        let result = job.result.as_ref().ok_or_else(|| Error::Invalid("ready job 缺少结果".into()))?;
        write_result(&tx, result, &job.job_id)?;
        let result = job.result.take();
        tx.execute(
            "INSERT INTO native_index_runs VALUES (?,?)",
            params![job.job_id, protocol::value(&job)?.to_string()],
        )?;
        job.result = result;
        ctx.check()?;
        tx.commit()?;
        Ok(())
    })
}
/// 结果改变了索引标签的那些图片。
fn changed_labels(src: &Connection, result: &protocol::IndexResponse) -> Result<BTreeSet<String>> {
    let old = crate::app_migrate_data::rows(src, "SELECT uuid,value,round(conf,4) FROM labels WHERE dim='char_ext'")?;
    let old_set = old.iter().map(|r| r.to_string()).collect::<BTreeSet<_>>();
    let new_set = result
        .labels
        .iter()
        .map(|r| json!([r.0, r.1, (r.2 * 10000.0).round() / 10000.0]).to_string())
        .collect::<BTreeSet<_>>();
    old_set
        .symmetric_difference(&new_set)
        .map(|s| {
            let row: (String, String, f64) = protocol::parse(
                serde_json::from_str(s).map_err(|e| Error::Invalid(e.to_string()))?,
                Path::new("source.sqlite labels"),
            )?;
            Ok(row.0)
        })
        .collect()
}
/// source.sqlite 里 `table` 的列，不同图库版本之间有差别。
fn columns(c: &Connection, table: &str) -> Result<Vec<String>> {
    Ok(c.prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}
/// 用结果里的角色、别名和索引标签替换现有的。
fn write_result(tx: &Connection, result: &protocol::IndexResponse, id: &str) -> Result<()> {
    tx.execute_batch("CREATE TABLE IF NOT EXISTS native_index_runs(id TEXT PRIMARY KEY,result TEXT NOT NULL)")?;
    tx.execute("DELETE FROM character_aliases", [])?;
    tx.execute("DELETE FROM characters", [])?;
    let cols = columns(tx, "characters")?;
    for r in &result.characters {
        let vals = cols
            .iter()
            .map(|k| {
                if k == "char_id" {
                    rusqlite::types::Value::Text(r.char_id.clone())
                } else {
                    sql_value(r.columns.get(k).unwrap_or(&Value::Null))
                }
            })
            .collect::<Vec<_>>();
        tx.execute(
            &format!("INSERT INTO characters ({}) VALUES ({})", cols.join(","), vec!["?"; cols.len()].join(",")),
            rusqlite::params_from_iter(vals),
        )?;
    }
    let alias_cols = columns(tx, "character_aliases")?;
    for r in &result.aliases {
        if alias_cols.iter().any(|c| c == "kind") {
            let p = params![r.0, r.1, r.2];
            tx.execute("INSERT OR IGNORE INTO character_aliases(char_id,alias,kind) VALUES (?,?,?)", p)?;
        } else {
            tx.execute("INSERT OR IGNORE INTO character_aliases(char_id,alias) VALUES (?,?)", params![r.0, r.1])?;
        }
    }
    // 来源资产已丢的认领沿用抢救留下的原行（含来源和批次），明确移除了角色时才一起清掉。
    tx.execute_batch(
        "DELETE FROM labels WHERE dim='char_ext' AND
         (EXISTS (SELECT 1 FROM assets a WHERE a.uuid=labels.uuid) OR
          NOT EXISTS (SELECT 1 FROM characters c WHERE c.char_id=labels.value))",
    )?;
    let preserved = tx
        .prepare("SELECT uuid,value FROM labels WHERE dim='char_ext'")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    let label_cols = columns(tx, "labels")?;
    for r in &result.labels {
        if !preserved.is_empty() && preserved.contains(&(r.0.clone(), r.1.clone())) {
            continue;
        }
        if ["source", "model_ver", "run_id"].iter().all(|k| label_cols.iter().any(|c| c == k)) {
            sourced_label(tx, r, id)?;
            continue;
        }
        tx.execute("INSERT INTO labels(uuid,dim,value,conf) VALUES (?,'char_ext',?,?)", params![r.0, r.1, r.2])?;
    }
    Ok(())
}
/// 一条索引标签，在会记录标签来源的图库里连同来源一起写。
fn sourced_label(c: &Connection, r: &(String, String, f64), id: &str) -> Result<()> {
    let model = format!("character_index@2025-09-25+danbooru@{}", chrono::Local::now().format("%Y-%m-%d"));
    let p = params![r.0, r.1, r.2, model, format!("char-scan-{id}")];
    let sql =
        "INSERT INTO labels(uuid,dim,value,conf,source,model_ver,run_id) VALUES (?,'char_ext',?,?,'char_index',?,?)";
    c.execute(sql, p)?;
    Ok(())
}
/// 文件从一个不可变、已校验的任务复制过来。崩溃后重复这一边界是安全的。
/// 共享文件先暂存在各自目标的旁边：复制失败或磁盘写满时，已发布的那一组不受影响。
/// 之后才在同一次发布 generation 内把它们改名到位，所以读方从不会拿到新旧混杂的文件（index_generation）。
fn publish(
    shared: &Path,
    dir: &Path,
    folder: &Path,
    job: &protocol::IndexJob,
    publication: &Cell<Publication>,
) -> Result<()> {
    let allowed = job.rows.iter().map(|r| r.short_tag.as_str()).collect::<BTreeSet<_>>();
    if job.action == protocol::IndexAction::Base {
        let base = job.extra.get("base").cloned().unwrap_or(Value::Null);
        let version = crate::character_index::release().map(|(r, _)| r.version.clone());
        write(
            &folder.join(BASE_MARK),
            &json!({"enabled": base["enable"] == true, "sha256": base["sha256"], "version": version}),
        )?;
    }
    let mut staged: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut retired: Vec<PathBuf> = Vec::new();
    if let Err(e) = stage_all(shared, folder, &allowed, &mut staged, &mut retired) {
        for (tmp, _) in &staged {
            let _ = std::fs::remove_file(tmp);
        }
        return Err(e);
    }
    let generation = crate::index_generation::start(shared)?;
    publication.set(Publication::MayBePartial);
    for (tmp, to) in &staged {
        std::fs::rename(tmp, to)?;
    }
    for to in &retired {
        std::fs::remove_file(to)?;
    }
    crate::index_generation::finish(shared, generation)?;
    for cid in allowed {
        let raw = folder.join("char-patch/raw").join(format!("{cid}.npy"));
        if raw.exists() {
            std::fs::create_dir_all(dir.join("raw"))?;
            std::fs::copy(raw, dir.join("raw").join(format!("{cid}.npy")))?;
        }
        let rejected = folder.join("char-patch/rejected").join(format!("{cid}.json"));
        if rejected.exists() {
            write(&dir.join("rejected").join(format!("{cid}.json")), &read(&rejected, Value::Null))?;
        }
    }
    write(&dir.join("plan.json"), &read(&folder.join("char-patch/plan.json"), json!([])))
}
/// 把每个新的索引文件复制到目标旁边（`staged`：临时路径和最终路径），并列出任务要删除的文件（`retired`）。
fn stage_all(
    shared: &Path,
    folder: &Path,
    allowed: &BTreeSet<&str>,
    staged: &mut Vec<(PathBuf, PathBuf)>,
    retired: &mut Vec<PathBuf>,
) -> Result<()> {
    let mut stage = |from: &Path, to: PathBuf| -> Result<()> {
        std::fs::create_dir_all(to.parent().expect("index file path must have a parent"))?;
        let name = to.file_name().expect("index file path must have a name").to_string_lossy().into_owned();
        let tmp = to.with_file_name(format!(".{name}.{}.tmp", crate::store::new_key()?));
        staged.push((tmp.clone(), to));
        std::fs::copy(from, tmp)?;
        Ok(())
    };
    for cid in allowed {
        if !safe(cid) {
            return invalid("索引文件 ID 不正确");
        };
        for ext in ["json", "npy"] {
            let from = folder.join("char-index").join(format!("{cid}.{ext}"));
            let to = shared.join("char-index").join(format!("{cid}.{ext}"));
            if from.exists() {
                stage(&from, to)?;
            } else if to.exists() {
                retired.push(to);
            }
        }
    }
    for name in ["char-anchors.npy", "char-anchors.txt", BASE_MARK] {
        if folder.join(name).exists() {
            stage(&folder.join(name), shared.join(name))?;
        }
    }
    Ok(())
}
/// 根据改变的标签重新索引图库，每个任务一次：返回有多少成员关系改变。
fn import(lib: &Library, src: &Connection, job: &protocol::IndexJob, ctx: &TaskContext) -> Result<i64> {
    let src = src.unchecked_transaction()?;
    let mut app = writer(&lib.db())?;
    crate::http_read::register_names(&app)?;
    let marker = format!("index_job:{}", job.job_id);
    let tx = app.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    crate::schema::begin_writing(&tx)?;
    let done: Option<String> =
        tx.query_row("SELECT value FROM meta WHERE key=?", [&marker], |r| r.get(0)).optional()?;
    let changed = if let Some(n) = done {
        n.parse::<i64>().unwrap_or(0)
    } else {
        crate::app_recognition::require_published_inputs(&tx, &src)?;
        let uuids = job.extra.get("changed").unwrap_or(&Value::Null);
        let labels = crate::app_migrate_data::rows_with(
            &src,
            "SELECT uuid,value,conf,dim
             FROM labels
             WHERE ((dim='character' AND conf>=0.35) OR dim='char_ext') AND uuid IN (SELECT value FROM json_each(?))",
            [uuids.to_string()],
        )?;
        let realistic = crate::app_recognition::realistic(&tx, &src, &[])?;
        let index = crate::app_migrate_data::index(&src)?;
        let out = crate::character_commands::command(
            &tx,
            &json!({
                "op":"reindex","index":index,"uuids":uuids,"labels":labels,"realistic":realistic,"indexed":job.job_id
            }),
        )?;
        let n = out["changed"].as_i64().unwrap_or(0);
        tx.execute("INSERT INTO meta VALUES (?,?)", params![marker, n.to_string()])?;
        tx.execute("INSERT INTO backend_effects(kind) VALUES ('names:{\"entity\":null,\"refresh\":false}')", [])?;
        tx.execute("INSERT INTO backend_effects(kind) VALUES ('index_changed')", [])?;
        n
    };
    ctx.check()?;
    crate::schema::commit_writing(tx)?;
    Ok(changed)
}
/// 任务里每个角色的结果：已移除、已添加并带上它的认领，或被拒绝并附上原因。
fn outcomes(job: &protocol::IndexJob, shared: &Path, dir: &Path, src: &Connection) -> Result<Vec<Value>> {
    let mut chars = vec![];
    for row in &job.rows {
        let mut r = protocol::value(row)?;
        let cid = row.short_tag.clone();
        if job.action == protocol::IndexAction::Drop {
            let reason = job.reason.as_deref().unwrap_or("").trim();
            r = json!({"tag":r["tag"],"short_tag":cid,"copyright":r["copyright"],"status":"dropped","reason":reason});
        } else if meta(shared, &cid)["source"] == "danbooru-patch" {
            let n: i64 = src.query_row(
                "SELECT count(DISTINCT uuid) FROM labels WHERE dim='char_ext' AND value=?",
                [&cid],
                |r| r.get(0),
            )?;
            r["status"] = "added".into();
            r["claims"] = n.into();
        } else {
            r["status"] = "rejected".into();
            r["reason"] = read(&dir.join("rejected").join(format!("{cid}.json")), json!({}))["rejected"]
                .as_str()
                .unwrap_or("没有可用的参考图")
                .into();
        }
        chars.push(r);
    }
    Ok(chars)
}
/// 把添加的角色从上次检查的缺失列表里去掉，并把任务记进历史，只做一次。
fn record(dir: &Path, id: &str, chars: &[Value], changed: i64, drop: bool) -> Result<()> {
    let mut check = read(&dir.join("check.json"), Value::Null);
    if !drop && !check.is_null() {
        let done = chars.iter().map(|r| r["short_tag"].clone()).collect::<Vec<_>>();
        if let Some(missing) = check["missing"].as_array_mut() {
            missing.retain(|r| !done.contains(&r["short_tag"]));
        }
        write(&dir.join("check.json"), &check)?;
        // 覆盖情况跟着改（#561）：添加成功的从还缺挪到自己补过，没添加成的不再计入还缺。
        let mut coverage = read(&dir.join("coverage.json"), Value::Null);
        if coverage["coverage"].is_object() {
            for r in chars {
                let Some(work) = r["copyright"].as_str().and_then(|cp| coverage["coverage"].get_mut(cp)) else {
                    continue;
                };
                if !work.is_object() {
                    continue;
                }
                work["missing"] = json!((work["missing"].as_i64().unwrap_or(0) - 1).max(0));
                if r["status"] == "added" {
                    work["own"] = json!(work["own"].as_i64().unwrap_or(0) + 1);
                }
            }
            write(&dir.join("coverage.json"), &coverage)?;
        }
    }
    let mut ledger = read(&dir.join("native-history.json"), json!({}));
    let record = if let Some(record) = ledger.get(id) {
        record.clone()
    } else {
        let record = json!({"at":now(),"kind":if drop{"drop"}else{"add"},"characters":chars,"changed":changed});
        ledger[id] = record.clone();
        write(&dir.join("native-history.json"), &ledger)?;
        record
    };
    let mut history = read(&dir.join("history.json"), json!([])).as_array().cloned().unwrap_or_default();
    if !history.contains(&record) {
        history.truncate(30);
        history.insert(0, record);
        write(&dir.join("history.json"), &json!(history))?;
    }
    Ok(())
}
/// 保留最新的五份来源备份。
fn prune_backups(dir: &Path) -> Result<()> {
    let mut backups = std::fs::read_dir(dir.join("backup"))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.len() == 32 && n.bytes().all(|b| b.is_ascii_hexdigit())))
        .collect::<Vec<_>>();
    backups.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
    let excess = backups.len().saturating_sub(5);
    for e in backups.into_iter().take(excess) {
        let _ = std::fs::remove_dir_all(e.path());
    }
    Ok(())
}

/// 仅供测试：拉长内部完成状态已写下、线程尚未退出的窗口（#123、#505）；生产环境不设置。
fn pause_after_done() {
    if let Some(ms) = std::env::var("ILLUSTASH_TEST_INDEX_DONE_PAUSE_MS").ok().and_then(|v| v.parse().ok()) {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

fn sql_value(v: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as S;
    match v {
        Value::Null => S::Null,
        Value::String(s) => S::Text(s.clone()),
        Value::Number(n) => n
            .as_i64()
            .map(S::Integer)
            .unwrap_or_else(|| S::Real(n.as_f64().expect("JSON number has a numeric representation"))),
        Value::Bool(b) => S::Integer(i64::from(*b)),
        _ => S::Text(v.to_string()),
    }
}

fn ordered(v: &Value, keys: &[&str]) -> Json {
    if !v.is_object() {
        return Json::from(v.clone());
    }
    let mut fields =
        keys.iter().filter_map(|k| v.get(*k).map(|x| ((*k).to_string(), Json::from(x.clone())))).collect::<Vec<_>>();
    fields.extend(
        v.as_object()
            .expect("ordered checked object type")
            .iter()
            .filter(|(k, _)| !keys.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), Json::from(v.clone()))),
    );
    Json::Object(fields)
}
fn character_json(v: &Value) -> Json {
    let mut out =
        ordered(v, &["tag", "short_tag", "copyright", "post_count", "overlap", "status", "claims", "reason", "entity"]);
    if let Some(e) = v.get("entity") {
        let mut shown = ordered(
            e,
            &[
                "id",
                "name",
                "count",
                "name_source",
                "names",
                "work_id",
                "node_type",
                "character_id",
                "variant_id",
                "canonical_id",
            ],
        );
        if let Some(names) = e.get("names") {
            shown.set("names", ordered(names, &["zh", "ja", "en"]));
        }
        out.set("entity", shown);
    }
    out
}
fn check_json(v: &Value) -> Json {
    let mut value = v.clone();
    // 按作品的覆盖情况（#561）另存在 coverage.json，GET /api/index 的回复保持原样。
    value.as_object_mut().map(|o| (o.remove("work_order"), o.remove("coverage")));
    let mut out = ordered(&value, &["checked_at", "works", "min_posts", "missing"]);
    if let Some(order) = v["work_order"].as_array() {
        out.set("works", by_work(order, &v["works"], |x| Json::from(x.clone())));
    }
    if let Some(rows) = v["missing"].as_array() {
        out.set("missing", Json::Array(rows.iter().map(character_json).collect()));
    }
    out
}
/// 按关注作品的顺序排一个以作品 tag 为键的对象。
fn by_work(order: &[Value], map: &Value, item: impl Fn(&Value) -> Json) -> Json {
    Json::Object(
        order.iter().filter_map(|key| key.as_str().and_then(|k| map.get(k).map(|x| (k.to_owned(), item(x))))).collect(),
    )
}
/// 上次检查时每个关注作品的覆盖情况（#561）：{checked_at, coverage: {作品 tag: {packs, own, missing} 或 null}}，
/// 按关注作品的顺序。GET /api/index/coverage 原样返回。
fn coverage_json(v: &Value) -> Json {
    let mut out = Json::object([("checked_at", Json::from(v["checked_at"].clone()))]);
    let order = v["work_order"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| v["coverage"].as_object().map(|m| m.keys().map(|k| json!(k)).collect()).unwrap_or_default());
    out.set("coverage", by_work(&order, &v["coverage"], |x| ordered(x, &["packs", "own", "missing"])));
    out
}
fn history_json(v: &Value) -> Json {
    let mut out = ordered(v, &["at", "kind", "characters", "changed"]);
    if let Some(rows) = v["characters"].as_array() {
        out.set("characters", Json::Array(rows.iter().map(character_json).collect()));
    }
    out
}

fn backup_db(src: &Connection, path: &Path, ctx: &TaskContext) -> Result<()> {
    use rusqlite::backup::{Backup, StepResult};
    src.busy_timeout(Duration::from_millis(50))?;
    let mut out = crate::sqlite::open(path)?;
    let copy = Backup::new(src, &mut out)?;
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        ctx.check()?;
        match copy.step(256)? {
            StepResult::Done => return Ok(()),
            StepResult::More => ctx.progressed(),
            _ if Instant::now() >= end => return invalid("索引备份超时"),
            _ => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn read_check(path: &Path) -> Json {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Json::from(Value::Null))
}
fn write_check(path: &Path, v: &Value) -> Result<()> {
    write_json(path, &check_json(v))
}
fn write_json(path: &Path, v: &Json) -> Result<()> {
    let out = v.dump();
    std::fs::create_dir_all(path.parent().expect("index file path must have a parent"))?;
    let tmp = path.with_extension(format!("{}.tmp", crate::store::new_key()?));
    std::fs::write(&tmp, out)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

struct PreparedStage(PathBuf, bool);
impl Drop for PreparedStage {
    fn drop(&mut self) {
        if !self.1 {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn update(state: &Mutex<State>, fields: Value) {
    let mut s = state.lock().expect("index state mutex poisoned");
    s.merge(fields);
}

/// 给 `info` 用的任务状态和历史，先读状态。任务在报告完成之前写下它的历史条目，
/// 所以先读到的完成状态总是带着包含它的历史；先读历史可能会把上一份历史和新的完成状态配在一起（#85）。
fn state_and_history(state: &Mutex<State>, dir: &Path) -> (Value, Vec<Value>) {
    let state = state.lock().expect("index state mutex poisoned").value();
    let history = read(&dir.join("history.json"), json!([])).as_array().cloned().unwrap_or_default();
    (state, history)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn related(tag: &str, posts: i64, overlap: f64) -> Value {
        json!({"tag": {"name": tag, "post_count": posts}, "overlap_coefficient": overlap})
    }

    /// 按作品计数（#561）：自己补过、数据包覆盖、还缺三类，WD 自带的和移除过的不计；每个角色只算进第一个作品；
    /// 数据包覆盖的不再列为还缺。没启用数据包时，还缺的列表和以前一样。
    #[test]
    fn check_counts_packs_own_and_missing_per_work() {
        let set = |tags: &[&str]| tags.iter().map(|t| t.to_string()).collect::<BTreeSet<_>>();
        let known = set(&["base_(work_a)", "mine_(work_a)"]);
        let rejected = set(&["dropped_work_a"]);
        let own = |id: &str| id == "mine_work_a";
        let work_a = [
            related("base_(work_a)", 500, 0.9),
            related("mine_(work_a)", 500, 0.9),
            related("ext_one_(work_a)", 500, 0.9),
            related("ext_two_(work_a)", 500, 0.9),
            related("dropped_(work_a)", 500, 0.9),
            related("new_(work_a)", 500, 0.9),
            related("few_(work_a)", 10, 0.9),
            related("loose_(work_a)", 500, 0.2),
            related("shared_(work_a)", 500, 0.9),
        ];
        let work_b = [
            related("shared_(work_a)", 500, 0.9),
            related("ext_one_(work_a)", 500, 0.9),
            related("other_(work_b)", 300, 0.7),
        ];
        let run = |covered: &BTreeSet<String>| {
            let local = Local { known: &known, covered, rejected: &rejected, own: &own, min_posts: 100 };
            let (mut seen, mut rows) = (BTreeSet::new(), vec![]);
            let a = tally("work_a", &work_a, &local, &mut seen, &mut rows).unwrap();
            let b = tally("work_b", &work_b, &local, &mut seen, &mut rows).unwrap();
            (a, b, rows.iter().map(|r| r["tag"].as_str().unwrap().to_owned()).collect::<Vec<_>>())
        };
        let (a, b, missing) = run(&set(&["ext_one_(work_a)", "ext_two_(work_a)", "not_listed"]));
        assert_eq!(a, json!({"packs": 2, "own": 1, "missing": 2}));
        assert_eq!(b, json!({"packs": 0, "own": 0, "missing": 1}));
        assert_eq!(missing, ["new_(work_a)", "shared_(work_a)", "other_(work_b)"]);
        let (a, b, without) = run(&BTreeSet::new());
        assert_eq!(a, json!({"packs": 0, "own": 1, "missing": 4}));
        assert_eq!(b, json!({"packs": 0, "own": 0, "missing": 1}));
        assert_eq!(
            without,
            ["ext_one_(work_a)", "ext_two_(work_a)", "new_(work_a)", "shared_(work_a)", "other_(work_b)"]
        );
    }

    /// 检查结果按关注作品的顺序存；覆盖情况另存，check.json 的格式不变（GET /api/index 的回复冻结）。
    #[test]
    fn stored_check_keeps_work_order_and_coverage_apart() {
        let v = json!({"checked_at": "t", "works": {"b": 1, "a": null}, "coverage": {"a": null, "b": {"missing": 1, "own": 0, "packs": 2}},
            "min_posts": 100, "missing": [], "work_order": ["b", "a"]});
        assert_eq!(
            check_json(&v).dump(),
            r#"{"checked_at": "t", "works": {"b": 1, "a": null}, "min_posts": 100, "missing": []}"#
        );
        assert_eq!(
            coverage_json(&v).dump(),
            r#"{"checked_at": "t", "coverage": {"b": {"packs": 2, "own": 0, "missing": 1}, "a": null}}"#
        );
    }

    #[test]
    fn streamed_fingerprint_preserves_existing_index_job_digest() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE characters(char_id TEXT, note TEXT);
             CREATE TABLE character_aliases(char_id TEXT, alias TEXT);
             CREATE TABLE labels(uuid TEXT, dim TEXT, value TEXT, conf REAL, source TEXT);
             CREATE INDEX idx_labels_dim_val ON labels(dim,value);
             CREATE TABLE assets(uuid TEXT);
             CREATE TABLE embeddings(uuid TEXT, model TEXT, vec BLOB);
             INSERT INTO characters VALUES ('two','引号\"'), ('one',NULL);
             INSERT INTO character_aliases VALUES ('two','é'), ('two','\n');
             INSERT INTO labels VALUES ('a','general','applicable',0.0000000123,'wd'),
                 ('a','general','applicable',0.25,'legacy'), ('b','char_ext','x',1,'index'),
                 ('a','rating','excluded',1,'wd'), ('a','character','t',NULL,'wd');
             INSERT INTO assets VALUES ('b'), ('a');
             INSERT INTO embeddings VALUES ('a','wd',X'01'), ('a','ccip',X'04030201');",
        )
        .unwrap();
        let mut fields: Vec<Value> =
            FINGERPRINT_ROWS.iter().map(|sql| json!(crate::app_migrate_data::rows(&c, sql).unwrap())).collect();
        fields.push(json!(crate::strict_json::sha256_hex(b"a\x04\x03\x02\x01")));
        let old = crate::strict_json::sha256_hex(serde_json::to_string(&fields).unwrap().as_bytes());
        assert_eq!(fingerprint_rows(&c, || Ok(())).unwrap(), old);
    }

    #[test]
    fn recognition_publication_gates_index_snapshot_source_commit_and_replay() {
        let root = std::env::temp_dir().join(format!("illustash-index-pending-{}", crate::store::new_key().unwrap()));
        let images = root.join("images");
        std::fs::create_dir_all(&images).unwrap();
        let package = root.join("synthetic.pilib");
        crate::app_create::create_folder(&package, &images, None, None, None).unwrap();
        let lib = Library::open(&package).unwrap();
        let identity = crate::app_tasks::TaskIdentity::new(&lib, 1, "synthetic").unwrap();
        let mut task = crate::app_tasks::Task::start(identity, None, move |ctx| {
            let mut src = writer(&lib.source())?;
            src.execute_batch(
                "INSERT INTO assets(uuid,filename,src_path,in_scope,trashed) VALUES ('a','a.png','offline.png',1,0);
                CREATE TABLE recognition_imports(digest TEXT PRIMARY KEY,assets TEXT,status TEXT);
                CREATE TABLE recognition_assets(asset TEXT PRIMARY KEY,digest TEXT,package TEXT);
                INSERT INTO recognition_imports VALUES ('receipt','[\"a\"]','pending');
                INSERT INTO recognition_assets VALUES ('a','fingerprint','receipt');",
            )?;
            let app = writer(&lib.db())?;
            let folder = lib.instance_path().join("index-jobs/test");
            std::fs::create_dir_all(&folder)?;
            assert!(snapshot_library(&lib, &folder, &ctx).unwrap_err().to_string().contains("尚未完成发布"));
            // 已不拥有任何资产的旧回执不阻塞；已投影但通知 pending 也不阻塞。
            src.execute("DELETE FROM recognition_assets", [])?;
            snapshot_library(&lib, &folder, &ctx)?;
            src.execute("INSERT INTO recognition_assets VALUES ('a','fingerprint','receipt')", [])?;
            app.execute("INSERT INTO meta VALUES ('recognition_projection:receipt','1')", [])?;
            snapshot_library(&lib, &folder, &ctx)?;
            src.execute("DELETE FROM recognition_assets", [])?;
            app.execute("DELETE FROM meta WHERE key='recognition_projection:receipt'", [])?;
            let before = snapshot_library(&lib, &folder, &ctx)?;
            let journal = lib.instance_path().join("index-job.json");
            let shared = lib.instance_path().join("synthetic-shared");
            let dir = shared.join("char-patch");
            std::fs::create_dir_all(&dir)?;
            let mut job: protocol::IndexJob = protocol::parse(
                json!({
                    "version":1,"package":lib.instance_path(),"job_id":"test","identity":ctx.identity(),
                    "action":"apply","reason":null,"rows":[],"folder":folder,"before":before,"phase":"ready",
                    "result":{"format":"illustash-index/1","identity":ctx.identity(),"before":before,
                        "characters":[],"aliases":[],"labels":[]},"changed":[]
                }),
                &journal,
            )?;
            write(&journal, &protocol::value(&job)?)?;
            // 快照之后才出现 pending，且索引指纹本身未变：源提交门禁仍必须拒绝。
            src.execute("INSERT INTO recognition_assets VALUES ('a','fingerprint','receipt')", [])?;
            assert_eq!(fingerprint(&src, &ctx)?, before);
            assert!(commit_source(&lib, &shared, &dir, &journal, &mut job, &ctx, &mut src)
                .unwrap_err()
                .to_string()
                .contains("尚未完成发布"));
            assert!(!src.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='native_index_runs')",
                [],
                |r| r.get::<_, bool>(0)
            )?);
            // 旧索引已提交源结果，但应用尚未发布；重放也不能绕过新 pending 的门禁。
            src.execute_batch("CREATE TABLE native_index_runs(id TEXT PRIMARY KEY,result TEXT NOT NULL)")?;
            src.execute("INSERT INTO native_index_runs VALUES ('test',?)", [protocol::value(&job)?.to_string()])?;
            let state = Mutex::new(State::from(json!({"status":"importing"})));
            let progress = PatchProgress { state: &state, publication: Cell::new(Publication::NotCommitted) };
            assert!(commit(&lib, &shared, &dir, &journal, &mut job, &ctx, &progress)
                .unwrap_err()
                .to_string()
                .contains("尚未完成发布"));
            assert!(matches!(progress.publication.get(), Publication::MayBePartial));
            assert!(import(&lib, &src, &job, &ctx).unwrap_err().to_string().contains("尚未完成发布"));
            assert!(!app.query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE key='index_job:test')", [], |r| r
                .get::<_, bool>(0))?);
            app.execute("INSERT INTO meta VALUES ('recognition_projection:receipt','1')", [])?;
            assert_eq!(import(&lib, &src, &job, &ctx)?, 0);
            Ok(())
        });
        assert!(task.wait(crate::test_wait::limit()));
        assert_eq!(task.outcome(), crate::app_tasks::TaskOutcome::Done);
        drop(task);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn info_never_pairs_a_done_state_with_the_previous_history() {
        // #85：完成的任务先写历史条目再报告完成。读方启动时持有状态锁，按这个顺序完成任务，然后才让读方取状态。
        let dir = std::env::temp_dir().join(format!("illustash-index-info-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir.join("history.json"), &json!([{"kind":"add"}])).unwrap();
        let state = Arc::new(Mutex::new(State::from(json!({"status":"building"}))));
        let mut held = state.lock().unwrap();
        let reader = {
            let (state, dir) = (state.clone(), dir.clone());
            std::thread::spawn(move || state_and_history(&state, &dir))
        };
        // 让读方在任务完成前尽量往前走。无论它走到哪，完成状态都必须带着新的历史；等待只决定错误的顺序能不能被抓到。
        std::thread::sleep(std::time::Duration::from_millis(200));
        write(&dir.join("history.json"), &json!([{"kind":"drop"},{"kind":"add"}])).unwrap();
        *held = State::from(json!({"status":"done"}));
        drop(held);
        let (seen, history) = reader.join().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(seen["status"], "done");
        assert_eq!(history[0]["kind"], "drop", "{history:?}");
    }
}
