//! 格式 1 图库包的显式升级：所有领域都已切换到词条核心的图库（核心 5 到本构建的前一版），在备份后用一个应用数据库
//! 事务完成升级。切换之前的图库一律拒绝；由 `pre-v8` 版本先升级它们（#201，用户 2026-10-19 决定）。
use crate::{
    app_library::{self, Library},
    Error, Result,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug)]
pub(crate) struct ValidatedUpgrade {
    library: Library,
    before: i64,
}
#[derive(Debug)]
pub(crate) struct UpgradeReport {
    pub library: Library,
    pub before: i64,
    pub after: i64,
    pub backup: PathBuf,
    pub maintenance_pending: bool,
}
#[derive(Debug)]
pub(crate) struct UpgradeFailure {
    pub error: Error,
    pub backup: Option<PathBuf>,
    pub committed: bool,
}
impl fmt::Display for UpgradeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.committed {
            match &self.backup {
                Some(backup) => write!(
                    f,
                    "升级已提交，后续处理失败：{error}；升级前备份：{backup}",
                    error = self.error,
                    backup = backup.display()
                ),
                None => write!(f, "升级已提交，后续处理失败：{error}", error = self.error),
            }
        } else {
            match &self.backup {
                Some(backup) => write!(
                    f,
                    "升级未提交：{error}；升级前备份：{backup}",
                    error = self.error,
                    backup = backup.display()
                ),
                None => write!(f, "升级未提交：{error}", error = self.error),
            }
        }
    }
}
impl std::error::Error for UpgradeFailure {}
impl From<Error> for UpgradeFailure {
    fn from(error: Error) -> Self {
        Self { error, backup: None, committed: false }
    }
}

/// 页面和桌面外壳在请求升级前显示的文字（#193）。页面靠这段文字识别它。
pub(crate) const NEEDED: &str =
    "这个图库需要升级到新格式才能打开。升级后旧版本的 Illustash 打不开它；升级前会自动备份，升级没有完成时图库保持原样。";

/// 打开失败的图库包是否能由显式升级打开，只读地根据版本、标记和 schema 判断：不做完整性全检，所以大图库也能立刻
/// 问到（#267）。
pub(crate) fn needed(path: &Path) -> Option<ValidatedUpgrade> {
    prepare(path).ok()
}

/// 打开失败的图库包这个版本升级不了、而 `pre-v8` 版本可以时（#201），说明原因：页面显示这段话，而不是内部错误。
pub(crate) fn pre_v8(path: &Path) -> Option<String> {
    match prepare(path) {
        Err(Error::Schema(why)) if why.contains(PRE_V8) => Some(why),
        _ => None,
    }
}

/// 升级正在做什么，给桌面外壳的启动页：所处阶段以及占整个升级的比例。
pub(crate) type Progress = app_library::Progress;

/// 升级一个已预检的图库包，并像 `--upgrade` 那样在终端上说明结果。
pub(crate) fn run(
    prepared: &ValidatedUpgrade,
    progress: Option<Progress>,
) -> std::result::Result<UpgradeReport, UpgradeFailure> {
    let report = upgrade(prepared, progress)?;
    // 桌面用就绪协议和图库会话报告成功，stderr 留给空间回收等真实告警。
    if !crate::desktop_shell::is_host_child() {
        eprintln!(
            "图库升级已提交：terms {} → {}；升级前备份：{}{}",
            report.before,
            report.after,
            report.backup.display(),
            if report.maintenance_pending { "；空间维护待重试" } else { "" }
        );
    }
    Ok(report)
}

/// 页面显示的备份路径：不需要时去掉 Windows 的 verbatim 前缀 `\\?\`。
pub(crate) fn shown(path: &Path) -> String {
    crate::app_paths::canonical(path).unwrap_or_else(|_| path.to_owned()).to_string_lossy().into_owned()
}

impl ValidatedUpgrade {
    pub(crate) fn name(&self) -> &str {
        self.library.name()
    }
}

/// 不写 SQLite、不备份、不登记会话、不刷新源库索引；也不做完整性全检：
/// 由 `upgrade` 在写入锁下、备份之前做一次（#267）。
pub(crate) fn prepare(path: &Path) -> Result<ValidatedUpgrade> {
    let library = Library::open_for_upgrade_deferring_checks(path)?;
    let mut c = app_library::read_connection(&library.db())?;
    let tx = c.transaction()?;
    let before = preflight_application(&tx)?;
    preflight_source(&library)?;
    tx.commit()?;
    Ok(ValidatedUpgrade { library, before })
}

/// 切换到词条核心之前的图库得到的说明：这个版本不再升级它（#201）。
pub(crate) const PRE_V8: &str =
    "这个图库还没有完全切换到词条核心，这个版本不再升级它：请先用 pre-v8 版本打开并升级，再用这个版本打开。";

/// 有某个领域从未切换到词条核心的图库（应用 schema 低于 36，或者切换标记缺失或未完成）：以 `PRE_V8` 拒绝，只读。
pub(crate) fn refuse_unswitched(c: &Connection) -> Result<()> {
    let has_meta: bool =
        c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='meta')", [], |r| r.get(0))?;
    if !has_meta {
        return Ok(());
    }
    let older = meta(c, "schema_version")?.and_then(|v| v.parse::<i64>().ok()).is_none_or(|v| v < 36);
    let mut unswitched = false;
    for key in ["terms_artists", "terms_labels", "terms_characters", "terms_names"] {
        unswitched |= meta(c, key)?.as_deref() != Some("core");
    }
    if older || unswitched {
        return Err(Error::Schema(PRE_V8.into()));
    }
    Ok(())
}

/// 显式升级可以打开的图库包要过的只读检查：已切换、词条核心版本较旧的图库。
/// 完整性全检不在其中：由打开的一方做一次（`Library::check` 或 `upgrade`）。
pub(crate) fn validate(c: &Connection) -> Result<()> {
    if meta(c, "schema_version")?.is_some_and(|v| v.parse::<i64>().map_or(true, |v| v > 36)) {
        return Err(Error::Schema("应用库版本较新或无效，已停止迁移".into()));
    }
    refuse_unswitched(c)?;
    if !(0..=crate::SCHEMA_VERSION).contains(&crate::schema::version(c)?) {
        return Err(Error::Schema("词条版本较新，已停止迁移".into()));
    }
    if !matches!(meta(c, "identity_policy")?.as_deref(), None | Some("") | Some("hybrid") | Some("v2")) {
        return Err(Error::Schema("未知身份策略，已停止迁移".into()));
    }
    Ok(())
}

/// 升级的各个阶段，以及每个阶段起始时占整个升级的比例。备份前的检查要完整读两个数据库，占大部分时间；
/// 备份、迁移和空间回收运行期间不报告任何进度（这期间由桌面宿主维持启动页）；提交后的检查再读一遍 library.sqlite。
const CHECKED: f64 = 0.6;
const BACKUP: f64 = 0.6;
const MIGRATE: f64 = 0.7;
const COMPACT: f64 = 0.75;
const VERIFY: (f64, f64) = (0.8, 0.95);

/// 一次应用 SQLite 提交。提交前失败会回滚；恢复要显式进行。
///
/// 完整性全检各做一次，都在写入事务内，这样检查读到的就是要备份和迁移的内容：备份前检查两个数据库，提交后检查
/// 唯一被写入的 library.sqlite。#267 之前，同一次升级要完整读 library.sqlite 十次、source.sqlite 六次
/// （打开、预检和重新校验各自又检查一遍），而且都不报告进度。
pub(crate) fn upgrade(
    prepared: &ValidatedUpgrade,
    progress: Option<Progress>,
) -> std::result::Result<UpgradeReport, UpgradeFailure> {
    upgrade_with(prepared, progress, |_| Ok(()), |_| Ok(()))
}
fn upgrade_with<B, A>(
    prepared: &ValidatedUpgrade,
    progress: Option<Progress>,
    before_commit: B,
    after_commit: A,
) -> std::result::Result<UpgradeReport, UpgradeFailure>
where
    B: FnOnce(&Connection) -> Result<()>,
    A: FnOnce(&Library) -> Result<()>,
{
    let stage = |text: &str, done: f64| {
        if let Some(progress) = &progress {
            progress(text, done)
        }
    };
    // 重新校验图库包：已预检的句柄不等于允许升级一个已经改变的图库包。
    let current = prepared.library.reopen_for_upgrade_deferring_checks()?;
    if current.instance_path() != prepared.library.instance_path() || current.config() != prepared.library.config() {
        return Err(Error::Conflict("升级预检后图库包已改变，请重新预检".into()).into());
    }
    let mut c = crate::sqlite::open_with_flags(current.db(), OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(Error::from)?;
    crate::configure(&c)?;
    let mut backup = None;
    let mut committed = false;
    let result = (|| -> Result<UpgradeReport> {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let before = preflight_application(&tx)?;
        if before != prepared.before {
            return Err(Error::Conflict("升级预检后词条版本已改变，请重新预检".into()));
        }
        preflight_source(&current)?;
        // 有损坏时在任何备份之前停止升级。从这里到提交，其他写入方都被排除在外。
        current.check(progress.clone().map(|p| {
            Arc::new(move |label: &str, done: f64| p(&format!("正在检查图库数据（{label}）"), done * CHECKED))
                as Progress
        }))?;
        stage("正在备份图库", BACKUP);
        pause_for_tests();
        // 这个事务排除其他写入方期间，独立的读连接看到的是已提交的状态。
        // SQLite backup 保留隐式 rowid；VACUUM INTO 会在恢复副本里重写它们。
        let snapshot = app_library::read_connection(&current.db())?;
        let prepared_backup =
            crate::schema_rules::prepare_upgrade_backup(&snapshot, before, &crate::platform_backup::capability())?;
        backup = prepared_backup.path().map(std::path::Path::to_owned);
        drop(snapshot);
        let backup_path = backup.clone().ok_or_else(|| Error::Invalid("升级前备份没有生成".into()))?;
        stage("正在升级图库", MIGRATE);
        crate::schema_rules::migrate_prepared_in(&tx, crate::SCHEMA_VERSION, &prepared_backup)?;
        before_commit(&tx)?;
        tx.commit()?;
        committed = true;
        // 提交后的维护与现有的核心 CLI 相同。忙或磁盘失败时保留它的重试标记。
        if meta(&c, "terms_compact_pending")?.is_some() {
            stage("正在回收图库空间", COMPACT);
        }
        crate::schema::compact_pending(&c);
        let maintenance_pending = meta(&c, "terms_compact_pending")?.is_some();
        drop(c);
        after_commit(&current)?;
        // source.sqlite 没有被写入，备份前已检查过；library.sqlite 再检查一次。
        let library = current.reopen_deferring_checks()?;
        library.check_application(progress.clone().map(|p| {
            Arc::new(move |done: f64| p("正在核对升级后的图库数据", VERIFY.0 + (VERIFY.1 - VERIFY.0) * done))
                as app_library::Report
        }))?;
        Ok(UpgradeReport { library, before, after: crate::SCHEMA_VERSION, backup: backup_path, maintenance_pending })
    })();
    result.map_err(|error| UpgradeFailure { error, backup, committed })
}
fn meta(c: &Connection, key: &str) -> Result<Option<String>> {
    Ok(c.query_row("SELECT value FROM meta WHERE key=?", [key], |r| r.get(0)).optional()?)
}
/// 仅供测试：`ILLUSTASH_TEST_UPGRADE_PAUSE_MS` 让检查之后的那一步花这么长时间，就像大图库的备份和迁移那样，
/// 期间不报告任何进度（#267）。
fn pause_for_tests() {
    if let Some(ms) = std::env::var("ILLUSTASH_TEST_UPGRADE_PAUSE_MS").ok().and_then(|v| v.parse().ok()) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}
/// 升级后的图库要读的源库 schema；它的全检由 `upgrade` 做。
fn preflight_source(library: &Library) -> Result<()> {
    let mut c = app_library::read_connection(&library.source())?;
    let tx = c.transaction()?;
    for sql in [
        "SELECT uuid,filename,src_path,width,height,added_at,src_kind,in_scope,trashed,source_id,preset FROM assets LIMIT 0",
        "SELECT uuid,dim,value,conf FROM labels LIMIT 0",
        "SELECT uuid,dim,value,note FROM decisions LIMIT 0",
        "SELECT asset,system,key,source_id,current FROM asset_links LIMIT 0",
        "SELECT source_id,path,size,mtime_ns,md5,asset FROM files LIMIT 0",
        "SELECT * FROM characters LIMIT 0", "SELECT * FROM character_aliases LIMIT 0", "SELECT * FROM runs LIMIT 0",
    ] {
        tx.prepare(sql).map_err(|e| Error::Schema(format!("source schema 是更早的布局（{e}）。{why}", why = PRE_V8)))?;
    }
    tx.commit()?;
    Ok(())
}
fn preflight_application(c: &Connection) -> Result<i64> {
    validate(c)?;
    let app = meta(c, "schema_version")?;
    let version = crate::schema::version(c)?;
    if app.as_deref() != Some("36") || !(5..crate::SCHEMA_VERSION).contains(&version) {
        return Err(Error::Schema("需要应用升级".into()));
    }
    for key in ["terms_artists", "terms_labels", "terms_characters", "terms_names"] {
        if meta(c, key)?.as_deref() != Some("core") {
            return Err(Error::Schema("需要模块切换".into()));
        }
    }
    if meta(c, "identity_policy")?.as_deref() == Some("v2") {
        return Err(Error::Schema("身份策略 v2 需要应用层转换，本批词条升级不支持".into()));
    }
    crate::character_projection::policy(c)?;
    // 打开时不能悄悄补上未建好的索引，也不能走 Store.initialize 的写入路径。
    for key in ["indexed", "general_indexed", "rating_indexed", "names_version"] {
        if meta(c, key)?.is_none() {
            return Err(Error::Schema(format!("应用初始化或索引未完成：缺少 {key}。{why}", why = PRE_V8)));
        }
    }
    // 所有版本共同使用的应用读取对象和核心存储都必须已经存在。
    // 预处理视图会展开它们的依赖，而不会改动投影或建临时表。
    const OBJECTS: &str = "assets candidates general_hits assertions deferred imported asset_tags asset_rating actions image_sigs relations asked audit_sample name_cache name_settings reviews prefs family_evidence single_assets lead_assets duplicates curation sync_runs sync_added similar_groups clusters cluster_info prototypes weak_claims proto_claims learn_state auto_rules excluded family_confirmed membership_rules memberships tag_memberships variant_memberships ambiguous_claims identity_canonical identity_scopes identity_suspended identity_uncertain identity_anchor_decisions identity_anchor_history identity_pairs identity_nodes backend_effects term_assets terms term_names term_links term_relations term_packages term_evidence term_projection term_batches term_verdicts term_evidence_sets term_evidence_set_assets term_evidence_set_rows term_active_evidence term_identity_baseline term_tree term_legacy_ids term_key_names term_key_aliases term_name_display term_name_choice term_effective_verdicts term_relation_states term_relation_latest term_current_evidence";
    for object in OBJECTS.split_whitespace() {
        c.prepare(&format!("SELECT * FROM {object} LIMIT 0"))
            .map_err(|e| Error::Schema(format!("应用/core 结构未完成：{object}：{e}")))?;
    }
    // v8 已逐行核对并删除旧兼容表。只对 v5–v7 要求这些对象；v8 检查替代它们的权威列。
    if version < 8 {
        for object in
            ["entities", "identity_relations", "identity_evidence", "identity_decisions", "identity_decision_evidence"]
        {
            c.prepare(&format!("SELECT * FROM {object} LIMIT 0"))?;
        }
        c.prepare("SELECT id,name,work,external_key,variant,parent_id,tag,merged_into,kind,artist,label_only,mascot,mascot_origin,work_id,original FROM entities LIMIT 0")?;
    } else {
        for sql in [
            "SELECT id,key,kind FROM terms LIMIT 0",
            "SELECT term_id,parent_id,merged_into,work_id,work_text,review_status,review_note,parent_locked,variant,artist,label_only,mascot,mascot_origin,original,seq FROM term_tree LIMIT 0",
            "SELECT seq FROM term_names LIMIT 0",
            "SELECT label_kind,label_count FROM actions LIMIT 0",
        ] { c.prepare(sql)?; }
    }
    for sql in [
        "SELECT uuid,filename,src_path,width,height,added_at,src_kind FROM assets LIMIT 0",
        "SELECT uuid,entity,source,score FROM candidates LIMIT 0",
        "SELECT uuid,entity,sources,score FROM family_evidence LIMIT 0",
        "SELECT id,label,before_json,ts,undone FROM actions LIMIT 0",
        "SELECT entity,skin_only,name,name_source,zh,ja,en,tag FROM term_name_display LIMIT 0",
        "SELECT kind,key,name,name_source,zh,ja,en FROM term_name_choice LIMIT 0",
        "SELECT key,value FROM prefs LIMIT 0",
        "SELECT uuid,entity,status,origin FROM memberships LIMIT 0",
    ] {
        c.prepare(sql)?;
    }
    for table in crate::schema::PRIVATE.iter().chain(crate::legacy_characters::GUARDED.iter()) {
        for event in ["insert", "update", "delete"] {
            let trigger = format!("{table}_writer_{event}");
            let exists: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='trigger' AND name=? AND tbl_name=?)",
                [trigger.as_str(), *table],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(Error::Schema(format!("核心写保护未完成：缺少 {trigger}")));
            }
        }
    }
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        // 用真正的 migrate_to 生成旧核心 DDL，而不是把 schema 7 数据库的版本标记改低。
        // 历史产出方和数据的完全等价另由应用集成测试覆盖。
        fn new(version: i64) -> Self {
            let p = std::env::temp_dir().join(format!(
                "illustash-upgrade-{}-{}.pilib",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&p).unwrap();
            fs::write(p.join("library.json"), r#"{"format":1,"id":"synthetic","name":"升级合成","sources":[]}"#)
                .unwrap();
            let mut c = crate::open(&p.join("library.sqlite")).unwrap();
            c.execute_batch(include_str!("app_create_schema.sql")).unwrap();
            crate::schema::migrate_to(&mut c, 5).unwrap();
            c.execute_batch("DROP TABLE asset_tags; DROP TABLE general_hits; DROP TABLE asset_rating;").unwrap();
            c.execute_batch(crate::legacy_labels::VIEWS).unwrap();
            c.execute_batch("INSERT INTO meta VALUES ('terms_artists','core'),('terms_labels','core'),('terms_characters','core'),('terms_names','core'),('indexed','synthetic'),('general_indexed','synthetic'),('rating_indexed','synthetic'),('names_version','0');").unwrap();
            for table in crate::legacy_characters::GUARDED.iter() {
                crate::schema::guard(&c, table).unwrap();
            }
            if version > 5 {
                crate::schema::migrate_to(&mut c, version).unwrap();
                crate::schema::compact_pending(&c);
                // 夹具的部分构建现在用公开的、备份安全的核心 API。
                // 只删掉这个合成夹具建立时产生的备份，再测量真正的升级。
                fs::remove_dir_all(p.join("library-backups")).unwrap();
            }
            c.execute_batch("CREATE TABLE implicit_rows(value TEXT); INSERT INTO implicit_rows(rowid,value) VALUES(27,'retain rowid');").unwrap();
            drop(c);
            let s = crate::sqlite::open(p.join("source.sqlite")).unwrap();
            s.execute_batch("CREATE TABLE assets(uuid TEXT,filename TEXT,src_path TEXT,width INTEGER,height INTEGER,added_at TEXT,src_kind TEXT,in_scope INTEGER,trashed INTEGER,source_id TEXT,preset TEXT); CREATE TABLE labels(uuid TEXT,dim TEXT,value TEXT,conf REAL); CREATE TABLE decisions(uuid TEXT,dim TEXT,value TEXT,note TEXT); CREATE TABLE characters(char_id TEXT); CREATE TABLE character_aliases(char_id TEXT,alias TEXT); CREATE TABLE runs(id INTEGER);").unwrap();
            s.execute_batch(include_str!("app_source_schema.sql")).unwrap();
            Self(p)
        }
        // 从历史核心生成的固定行建立测试库；DDL 仍由真正 migrate_to(version) 创建。
        fn recorded(input: &serde_json::Value) -> Self {
            let f = Fixture::new(input["terms_schema"].as_i64().unwrap());
            let mut c = f.connection();
            let tx = c.transaction().unwrap();
            tx.execute_batch("PRAGMA defer_foreign_keys=ON").unwrap();
            // 装入固定旧状态时只借写钥匙，不调用当前版本的投影器改写旧行。
            tx.execute("INSERT OR REPLACE INTO meta VALUES (?1,'1')", [crate::schema::WRITER_KEY]).unwrap();
            for (table, data) in input["tables"].as_object().unwrap() {
                assert!([
                    "actions",
                    "assertions",
                    "assets",
                    "memberships",
                    "meta",
                    "term_assets",
                    "term_name_display",
                    "term_names",
                    "term_projection",
                    "term_tree",
                    "term_verdicts",
                    "terms"
                ]
                .contains(&table.as_str()));
                let columns: Vec<_> = data["columns"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
                assert!(columns.iter().all(|v| v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')));
                let values: Vec<_> = (0..columns.len()).map(|i| format!("json_extract(?1,'$[{i}]')")).collect();
                let verb = if table == "meta" { "INSERT OR REPLACE" } else { "INSERT" };
                let sql = format!("{verb} INTO {table} ({}) SELECT {}", columns.join(","), values.join(","));
                for row in data["rows"].as_array().unwrap() {
                    tx.execute(&sql, [row.to_string()]).unwrap();
                }
            }
            tx.execute("DELETE FROM meta WHERE key=?1", [crate::schema::WRITER_KEY]).unwrap();
            tx.commit().unwrap();
            drop(c);
            f
        }
        fn connection(&self) -> Connection {
            crate::open(&self.0.join("library.sqlite")).unwrap()
        }
        fn backups(&self) -> usize {
            fs::read_dir(self.0.join("library-backups")).map(|it| it.count()).unwrap_or(0)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn old_layouts_upgrade_once_with_one_rowid_faithful_backup_and_unchanged_source() {
        for version in [5, 6, 7, 8] {
            let f = Fixture::new(version);
            assert!(Library::open(&f.0).is_err());
            let source = fs::read(f.0.join("source.sqlite")).unwrap();
            let db_before = fs::read(f.0.join("library.sqlite")).unwrap();
            let prepared = needed(&f.0).expect("桌面打开失败后应提出升级");
            assert_eq!(db_before, fs::read(f.0.join("library.sqlite")).unwrap());
            assert_eq!(f.backups(), 0);
            let report = run(&prepared, None).unwrap();
            assert_eq!(
                (report.before, report.after, report.maintenance_pending),
                (version, crate::SCHEMA_VERSION, false)
            );
            assert_eq!(report.library.display_path(), f.0);
            assert_eq!(f.backups(), 1);
            let backup = app_library::read_connection(&report.backup).unwrap();
            assert_eq!(crate::schema::version(&backup).unwrap(), version);
            assert_eq!(backup.query_row("SELECT rowid FROM implicit_rows", [], |r| r.get::<_, i64>(0)).unwrap(), 27);
            assert_eq!(source, fs::read(f.0.join("source.sqlite")).unwrap());
            assert!(prepare(&f.0).is_err());
            assert_eq!(f.backups(), 1);
        }
    }
    #[test]
    fn v10_gallery_and_favorite_survive_upgrade_and_legacy_undo() {
        use crate::{http_json::parse_request, store};
        use rusqlite::params;
        use serde_json::json;

        let f = Fixture::new(10);
        let mut c = f.connection();
        let tx = c.transaction().unwrap();
        crate::schema::begin_writing(&tx).unwrap();
        tx.execute("INSERT INTO assets(uuid) VALUES ('a'),('b')", []).unwrap();
        let term = store::create_term(&tx, crate::Kind::Mark, Some(crate::schema::FAVORITE_KEY), "user").unwrap();
        let asset = store::library_asset(&tx, "a").unwrap().unwrap();
        store::judge_in(
            &tx,
            &[store::VerdictTarget::Image { asset, term }],
            store::Verdict::Yes,
            "user",
            "",
            "已收藏 1 张",
        )
        .unwrap();
        let filters = json!({"has":"[\"fav\"]","order":"oldest"});
        let ts = "2026-11-10 00:00:00";
        tx.execute("INSERT INTO galleries VALUES (12,'条件画廊',?,'a',4,?,?)", params![filters.to_string(), ts, ts])
            .unwrap();
        // 保留 v10 的历史数据形状，新 schema 继续能撤销此前的改名。
        let before = json!([{"gallery":{"id":12,"row":{"id":12,"name":"原来名字","params":filters.to_string(),
            "cover":"a","position":4,"created_at":ts,"updated_at":ts}}}]);
        tx.execute("INSERT INTO actions(label,before_json) VALUES (?,?)", ["已更新画廊", &before.to_string()]).unwrap();
        let action = tx.last_insert_rowid();
        crate::schema::commit_writing(tx).unwrap();
        drop(c);

        let report = run(&needed(&f.0).unwrap(), None).unwrap();
        assert_eq!((report.before, report.after, f.backups()), (10, 11, 1));
        let backup = app_library::read_connection(&report.backup).unwrap();
        assert_eq!(crate::schema::version(&backup).unwrap(), 10);
        let c = f.connection();
        assert_eq!(c.query_row("SELECT count(*) FROM gallery_members", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        let context = crate::http::LibraryContext {
            examples: None,
            db: f.0.join("library.sqlite"),
            generation: "synthetic".into(),
            library: Some(f.0.clone()),
            tags: f.0.join("unused.csv"),
            source: Some(f.0.join("source.sqlite")),
            token: Some("synthetic".into()),
            read_only: None,
        };
        let expect_gallery = |name: &str| {
            let rows = crate::app_galleries::read(&c, "").unwrap();
            let crate::http_json::Json::Array(rows) = rows else {
                panic!("画廊列表应为数组");
            };
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].field("name").text(), name);
            assert_eq!(rows[0].field("id").integer(), 12);
            assert_eq!(rows[0].field("params").field("has").text(), filters["has"].as_str().unwrap());
            assert_eq!(rows[0].field("position").integer(), 4);
            assert!(rows[0].get("kind").is_none());
            let covers = crate::app_galleries::covers(&c, &context, "", "[12]").unwrap();
            let crate::http_json::Json::Array(covers) = covers else {
                panic!("封面列表应为数组");
            };
            assert_eq!(covers[0].field("cover").text(), "a");
        };
        expect_gallery("条件画廊");
        let filters = std::collections::HashMap::from([("has".into(), "[\"fav\"]".into())]);
        let (where_, args, _) = crate::http_browse::selection(&c, &context, &filters).unwrap();
        let ids = c
            .prepare(&format!("SELECT a.uuid FROM assets a WHERE {where_}"))
            .unwrap()
            .query_map(rusqlite::params_from_iter(args), |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(ids, ["a"]);
        let req = |path, data: &serde_json::Value| {
            crate::app_commands::request(&context, path, &parse_request(&data.to_string()).unwrap()).unwrap()
        };
        req("/api/undo", &json!({"id":action}));
        expect_gallery("原来名字");
    }
    #[test]
    fn v8_curation_history_survives_normal_upgrade_and_application_undo() {
        use illustash_core::human_conclusions::state::{self, Domain, State};
        use serde_json::json;
        let f = Fixture::new(8);
        let c = f.connection();
        c.execute("INSERT INTO assets(uuid) VALUES ('a')", []).unwrap();
        // v8 的收录操作没有 kind/count，不从展示文案补出 reset；原业务状态和撤销仍然保留。
        for (label, before) in [
            ("已将 1 张交回自动规则", json!([{"uuid":"a","curation":null}])),
            ("已收录 1 张", json!([{"uuid":"a","curation":null}])),
            (
                "已将 1 张交回自动规则",
                json!([{"uuid":"a","curation":{"status":"included","origin":"review","created_at":"2026-10-01 00:00:00"}}]),
            ),
        ] {
            c.execute("INSERT INTO actions(label,before_json) VALUES (?1,?2)", [label, &before.to_string()]).unwrap();
        }
        drop(c);
        let report = run(&needed(&f.0).unwrap(), None).unwrap();
        assert_eq!((report.before, report.after, f.backups()), (8, crate::SCHEMA_VERSION, 1));
        let context = crate::http::LibraryContext {
            examples: None,
            db: f.0.join("library.sqlite"),
            generation: "synthetic".into(),
            library: Some(f.0.clone()),
            tags: f.0.join("unused.csv"),
            source: Some(f.0.join("source.sqlite")),
            token: Some("synthetic".into()),
            read_only: None,
        };
        let read = || state::read(&f.connection(), Domain::Curation, "a").unwrap();
        assert_eq!(read(), State::Undecided);
        for id in [3, 2, 1] {
            crate::app_commands::request(
                &context,
                "/api/undo",
                &crate::http_json::parse_request(&json!({"id":id}).to_string()).unwrap(),
            )
            .unwrap();
            match id {
                3 => assert!(matches!(read(), State::Positive(v) if v["status"] == "included")),
                2 => assert_eq!(read(), State::Undecided),
                _ => assert_eq!(read(), State::Undecided),
            }
        }
    }
    #[test]
    fn genuine_v8_counted_operation_survives_name_collision_upgrade_and_undo() {
        use illustash_core::human_conclusions::state::{self, Domain, State};
        use serde_json::{json, Value};
        // 固定数据由未改动的 02b6437d/v8 核心实际执行夹具 commands 生成；构建时未运行新状态代码。
        // Fixture::new(8) 用真正迁移建立同版 DDL，再装入产出方的非空行；没有降版本号或手写动作载荷。
        let input: Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/library_upgrade/human_v8_name_collision.json"))
                .unwrap();
        assert_eq!(input["producer_commit"], "02b6437d");
        assert_eq!(input["terms_schema"], 8);
        let f = Fixture::recorded(&input);
        let c = f.connection();
        let (label, kind, count): (String, String, String) = c
            .query_row("SELECT label,label_kind,label_count FROM actions WHERE id=2", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!((kind.as_str(), count.as_str()), ("set", "1"));
        assert_eq!(crate::undo_labels::UndoLabel::recognize(&label).unwrap().0, crate::undo_labels::UndoLabel::Sweep);
        assert_eq!(c.query_row("SELECT count(*) FROM deferred", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        drop(c);
        run(&needed(&f.0).unwrap(), None).unwrap();
        let read = || state::read(&f.connection(), Domain::Deferred, "synthetic-a").unwrap();
        assert!(matches!(read(),State::Absent(m) if m.owner_action_id==2));
        let context = crate::http::LibraryContext {
            examples: None,
            db: f.0.join("library.sqlite"),
            generation: "synthetic".into(),
            library: Some(f.0.clone()),
            tags: f.0.join("unused.csv"),
            source: Some(f.0.join("source.sqlite")),
            token: Some("synthetic".into()),
            read_only: None,
        };
        for id in [2, 1] {
            crate::app_commands::request(
                &context,
                "/api/undo",
                &crate::http_json::parse_request(&json!({"id":id}).to_string()).unwrap(),
            )
            .unwrap();
            if id == 2 {
                assert_eq!(read(), State::Positive(json!(true)));
            } else {
                assert_eq!(read(), State::Undecided);
            }
        }
    }
    #[test]
    fn failure_before_commit_rolls_back_all_core_changes_and_retains_backup() {
        let f = Fixture::new(5);
        let p = prepare(&f.0).unwrap();
        let failure = upgrade_with(
            &p,
            None,
            |tx| {
                tx.execute("INSERT INTO prefs VALUES ('injected','must roll back')", [])?;
                Err(Error::Invalid("synthetic failure".into()))
            },
            |_| Ok(()),
        )
        .unwrap_err();
        assert!(!failure.committed);
        assert!(failure.backup.as_ref().unwrap().is_file());
        let c = f.connection();
        assert_eq!(crate::schema::version(&c).unwrap(), 5);
        assert!(!c
            .query_row("SELECT EXISTS(SELECT 1 FROM prefs WHERE key='injected')", [], |r| r.get::<_, bool>(0))
            .unwrap());
        let ddl: String =
            c.query_row("SELECT sql FROM sqlite_master WHERE name='candidates'", [], |r| r.get(0)).unwrap();
        assert!(!ddl.contains("WITHOUT ROWID"));
        assert!(failure.to_string().contains("升级未提交"));
    }
    #[test]
    fn post_commit_failure_never_claims_rollback() {
        let f = Fixture::new(6);
        let p = prepare(&f.0).unwrap();
        let failure = upgrade_with(&p, None, |_| Ok(()), |_| Err(Error::Invalid("synthetic startup failure".into())))
            .unwrap_err();
        assert!(failure.committed);
        assert!(failure.to_string().contains("升级已提交"));
        assert!(failure.backup.as_ref().unwrap().is_file());
        assert_eq!(crate::schema::version(&f.connection()).unwrap(), crate::SCHEMA_VERSION);
        assert!(Library::open(&f.0).is_ok());
    }
    #[test]
    fn invalid_policy_ownership_and_old_core_application_refuse_without_backup() {
        for (version, fault) in [
            (5, "DROP TRIGGER candidates_writer_insert"),
            (5, "UPDATE meta SET value='35' WHERE key='schema_version'"),
            (8, "DROP TRIGGER candidates_writer_insert"),
            (8, "ALTER TABLE actions DROP COLUMN label_count"),
            (8, "DROP INDEX term_names_seq; ALTER TABLE term_names DROP COLUMN seq"),
        ] {
            let f = Fixture::new(version);
            f.connection().execute_batch(fault).unwrap();
            let before = fs::read(f.0.join("library.sqlite")).unwrap();
            assert!(prepare(&f.0).is_err(), "{fault}");
            assert_eq!(before, fs::read(f.0.join("library.sqlite")).unwrap());
            assert_eq!(f.backups(), 0);
        }
        // 核心 5 之前的布局：由 pre-v8 版本升级（#201）。
        let f = Fixture::new(5);
        crate::sqlite::open(f.0.join("source.sqlite")).unwrap().execute_batch("DROP TABLE files").unwrap();
        assert!(prepare(&f.0).unwrap_err().to_string().contains("pre-v8"));
        assert_eq!(f.backups(), 0);
        let f = Fixture::new(5);
        f.connection().execute("DELETE FROM meta WHERE key='indexed'", []).unwrap();
        assert!(prepare(&f.0).unwrap_err().to_string().contains("pre-v8"));
        assert_eq!(f.backups(), 0);
        let f = Fixture::new(5);
        f.connection().execute("DELETE FROM meta WHERE key='terms_names'", []).unwrap();
        assert!(prepare(&f.0).unwrap_err().to_string().contains("pre-v8"));
        assert!(Library::open(&f.0).unwrap_err().to_string().contains("pre-v8"));
        assert!(pre_v8(&f.0).is_some_and(|why| why.contains("pre-v8")));
        assert_eq!(f.backups(), 0);
    }
    #[test]
    fn changed_package_after_prepare_refuses_before_any_backup() {
        let f = Fixture::new(5);
        let p = prepare(&f.0).unwrap();
        fs::write(f.0.join("library.json"), r#"{"format":1,"id":"different","name":"changed"}"#).unwrap();
        let failure = upgrade(&p, None).unwrap_err();
        assert!(!failure.committed);
        assert!(failure.backup.is_none());
        assert_eq!(f.backups(), 0);
        assert_eq!(crate::schema::version(&f.connection()).unwrap(), 5);
    }
    /// 在一张单独的表里写几页合成行，再用垃圾数据覆盖其中一页：只有完整性全检才能发现。
    fn damage(db: &Path) {
        let c = crate::sqlite::open(db).unwrap();
        c.execute_batch(
            "CREATE TABLE synthetic_bulk(id INTEGER PRIMARY KEY, body TEXT); CREATE INDEX synthetic_bulk_body ON synthetic_bulk(body);
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<4000)
             INSERT INTO synthetic_bulk SELECT i, printf('%0100d', i) FROM n;",
        )
        .unwrap();
        c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())).unwrap();
        let size: usize = c.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
        let count: usize = c.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
        drop(c);
        let mut bytes = fs::read(db).unwrap();
        let start = (count - 3) * size;
        for (i, byte) in bytes[start..start + size].iter_mut().enumerate() {
            *byte = (i as u8).wrapping_mul(37) ^ 0xA5;
        }
        fs::write(db, bytes).unwrap();
    }
    #[test]
    fn asking_reads_no_data_and_the_upgrade_checks_before_its_backup() {
        // 是否提出升级只看版本和 schema（#267）：只有全检才能发现的损坏不会拖慢或改变这个提问。
        // 升级自己的检查会在任何备份或写入之前发现它。
        let f = Fixture::new(5);
        damage(&f.0.join("source.sqlite"));
        let before = fs::read(f.0.join("library.sqlite")).unwrap();
        let p = prepare(&f.0).unwrap();
        assert!(needed(&f.0).is_some());
        let failure = upgrade(&p, None).unwrap_err();
        assert!(!failure.committed && failure.backup.is_none(), "{failure}");
        assert!(failure.to_string().contains("图库数据库损坏"), "{failure}");
        assert_eq!(f.backups(), 0);
        assert_eq!(before, fs::read(f.0.join("library.sqlite")).unwrap());
    }
    #[test]
    fn the_upgrade_reports_its_stages_in_order() {
        for version in [5, 6, 7, 8] {
            let f = Fixture::new(version);
            let seen = Arc::new(std::sync::Mutex::new(Vec::<(String, f64)>::new()));
            let record = seen.clone();
            let progress: Progress = Arc::new(move |stage: &str, done: f64| {
                record.lock().unwrap().push((stage.to_owned(), done));
            });
            upgrade(&prepare(&f.0).unwrap(), Some(progress)).unwrap();
            let seen = seen.lock().unwrap();
            let stages: Vec<&str> = seen.iter().map(|(stage, _)| stage.as_str()).collect();
            let first =
                |stage: &str| stages.iter().position(|s| *s == stage).unwrap_or_else(|| panic!("{stage}: {stages:?}"));
            let order = [
                first("正在检查图库数据（library.sqlite）"),
                first("正在检查图库数据（source.sqlite）"),
                first("正在备份图库"),
                first("正在升级图库"),
                first("正在核对升级后的图库数据"),
            ];
            assert!(order.windows(2).all(|w| w[0] < w[1]), "{stages:?}");
            assert!(seen.windows(2).all(|w| w[0].1 <= w[1].1), "{seen:?}");
            assert!(seen.iter().all(|(_, done)| (0.0..=VERIFY.1).contains(done)), "{seen:?}");
        }
    }

    /// #492：照着 library.sqlite 的损坏提示换回两处备份。`backups/` 里「立即备份」留下的 library- 文件直接能打开；
    /// `library-backups/` 里升级前的备份要再升级一次，之前记下的损坏不能挡在升级提问前面；抢救留存的
    /// source-damaged 文件不是整理记录，换回去也不会被当成图库打开。每一份备份之后的整理都丢失。
    #[test]
    fn the_damage_guidance_matches_the_backups_that_can_be_restored() {
        let guidance = crate::app_integrity::LIBRARY_DAMAGED;
        for needle in [
            "退出 Illustash",
            "library.sqlite-wal",
            "library.sqlite-shm",
            "backups 文件夹",
            "library- 开头",
            "source-damaged",
            "library-backups 文件夹",
            "再升级一次",
            "pre-v8",
            "之后的整理会丢失",
            "「设置 › 数据」的「人工结论快照」",
            "换回之前最近的一份自动快照点「恢复…」",
            "只补这个图库还没决定的项",
        ] {
            assert!(guidance.contains(needle), "{needle}");
        }
        let f = Fixture::new(5);
        let report = upgrade(&prepare(&f.0).unwrap(), None).unwrap();
        let required: Vec<PathBuf> =
            fs::read_dir(f.0.join("library-backups")).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(required.len(), 1);
        let name = required[0].file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with(&format!("terms-v5-before-v{}-", crate::SCHEMA_VERSION)), "{name}");
        let manual = crate::app_settings::backup(&report.library).unwrap();
        assert!(manual.file_name().unwrap().to_string_lossy().starts_with("library-"), "{manual:?}");
        drop(report);
        // 抢救识别结果时留在同一个文件夹里的损坏文件（命名同 app_rescue::install）。
        let rescued = f.0.join("backups/source-damaged-20261115-120000.sqlite");
        fs::copy(f.0.join("source.sqlite"), &rescued).unwrap();
        // 两份备份之后才做的整理。
        f.connection().execute("INSERT INTO implicit_rows(rowid,value) VALUES(28,'after both backups')", []).unwrap();

        let db = f.0.join("library.sqlite");
        let aside = std::cell::Cell::new(0);
        // 损坏被发现、记下，打开时给出这段提示。
        let broken = || {
            damage(&db);
            crate::app_integrity::Health::load(&f.0)
                .damaged(crate::app_integrity::Damage::new(crate::app_integrity::Database::Library, "合成".into()))
                .unwrap();
            let error = crate::app_service::open_owned(&f.0, None, None).map(|_| ()).unwrap_err();
            assert!(matches!(&error, Error::Conflict(text) if text == guidance), "{error}");
            assert_eq!(crate::desktop_shell::damage_note(&f.0), Some(guidance));
        };
        // 照提示：损坏的数据库连同 WAL、SHM 移走，选中的备份复制回来改名为 library.sqlite。
        let restore = |from: &Path| {
            aside.set(aside.get() + 1);
            let moved = f.0.join(format!("moved-aside-{}", aside.get()));
            fs::create_dir(&moved).unwrap();
            for name in ["library.sqlite", "library.sqlite-wal", "library.sqlite-shm"] {
                if f.0.join(name).exists() {
                    fs::rename(f.0.join(name), moved.join(name)).unwrap();
                }
            }
            assert!(moved.join("library.sqlite").is_file());
            fs::copy(from, &db).unwrap();
        };
        let kept = |library: &Library| {
            let c = app_library::read_connection(&library.db()).unwrap();
            let rows: rusqlite::Result<Vec<i64>> = c
                .prepare("SELECT rowid FROM implicit_rows ORDER BY rowid")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect();
            rows.unwrap()
        };

        // 手动备份：直接打开，记下的损坏随打开前的完整检查清除。
        broken();
        restore(&manual);
        let (library, owner) = crate::app_service::open_owned(&f.0, None, None).unwrap();
        assert!(crate::app_integrity::recorded_damage(&f.0).is_none());
        assert_eq!(
            crate::schema::version(&app_library::read_connection(&library.db()).unwrap()).unwrap(),
            crate::SCHEMA_VERSION
        );
        assert_eq!(kept(&library), [27]);
        drop((library, owner));

        // 升级前的必需备份：外壳不再显示损坏提示，而是问要不要升级；升级时又留下一份备份。
        broken();
        restore(&required[0]);
        let error = crate::app_service::open_owned(&f.0, None, None).map(|_| ()).unwrap_err();
        assert!(matches!(error, Error::Schema(_)), "{error}");
        assert!(crate::app_integrity::recorded_damage(&f.0).is_none());
        assert_eq!(crate::desktop_shell::damage_note(&f.0), None);
        assert!(crate::desktop_shell::upgrade_needed(&f.0).is_some());
        let report = upgrade(&prepare(&f.0).unwrap(), None).unwrap();
        assert_eq!((report.before, report.after), (5, crate::SCHEMA_VERSION));
        assert_eq!(f.backups(), 2);
        drop(report);
        let (library, owner) = crate::app_service::open_owned(&f.0, None, None).unwrap();
        assert_eq!(kept(&library), [27]);
        drop((library, owner));

        // 抢救留存的识别结果文件：不能当整理记录打开，也不提出升级；外壳照旧显示损坏提示，请用户换一份。
        broken();
        restore(&rescued);
        assert!(crate::app_service::open_owned(&f.0, None, None).is_err());
        assert!(crate::desktop_shell::upgrade_needed(&f.0).is_none());
        assert_eq!(crate::desktop_shell::damage_note(&f.0), Some(guidance));
    }
}
