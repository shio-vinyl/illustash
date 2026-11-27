//! 进程内的应用生命周期。传输分帧和安全仍在 app_http。
use crate::http_json::Json;
use crate::{app_library::Library, Error, Result};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, RwLock,
    },
};
/// 打开某个物理图库包的宿主持有它，直到切换走、关闭或退出：用的是操作系统的文件锁，进程结束时系统会释放它，
/// 所以崩溃后图库包绝不会一直被锁着。第二个宿主（或通过另一个别名打开同一个图库包）会被拒绝，而不是把当前持有者
/// 正在进行的工作当成崩溃留下的残余。
///
/// 持有者还负责图库包的完整性记录（#212）：会话开始时打开，正常结束时关闭，所以下次打开时记录仍处于打开状态，
/// 就说明上一次会话没有正常结束。
pub(crate) struct Owner {
    _lock: std::fs::File,
    pub(crate) health: Arc<crate::app_integrity::Health>,
}
impl Owner {
    pub(crate) fn claim(library: &Library) -> Result<Arc<Self>> {
        crate::app_file_lock::ensure_supported()?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(library.instance_path().join(".illustash-owner.lock"))?;
        match crate::app_file_lock::acquire_unobstructed(&file) {
            Ok(()) => Ok(Arc::new(Self {
                _lock: file,
                health: Arc::new(crate::app_integrity::Health::load(library.instance_path())),
            })),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.raw_os_error() == Some(33) => {
                Err(Error::Conflict("这个图库已在另一个 Illustash 中打开；请先关闭那边，或回到那边继续使用".into()))
            }
            Err(e) => Err(e.into()),
        }
    }
}
impl Drop for Owner {
    /// 切换、关闭或丢弃服务时释放：这是正常结束，除非正处于 panic 展开中。
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.health.closed();
        }
    }
}

/// 为会话打开 `path`（#212）。先校验图库包但不做完整性全检，然后持有它；只有上一次会话没有正常结束或记录里有损坏时
/// 才做全检；有损坏就拒绝打开。`current` 是会话自己的图库包和持有者：重新打开同一个图库包时保留其持有权。
/// 打开时通过 `progress` 报告正在做的检查。
pub(crate) fn open_owned(
    path: &Path,
    current: Option<(&Library, &Arc<Owner>)>,
    progress: Option<crate::app_library::Progress>,
) -> Result<(Library, Arc<Owner>)> {
    let opened = Library::open_deferring_checks(path);
    // SQLite 可能在读取版本时就遇到损坏；之后的全检会说明损坏在哪个数据库里。
    let corrupt = match &opened {
        Err(Error::Sql(e)) => crate::app_library::corruption(e).is_some(),
        _ => false,
    };
    let library = match opened {
        Ok(library) => library,
        Err(e) if corrupt => {
            let library = Library::locate(path)?;
            let owner = claim_or_reuse(&library, current)?;
            return Err(match crate::app_integrity::check_both(&library, progress)? {
                Some(damage) => {
                    owner.health.damaged(damage.clone())?;
                    damage.error()
                }
                None => e,
            });
        }
        // 记录过损坏、现在因格式不对打不开：多半是按提示换回了升级前的旧格式备份（#492）。完整检查认定完好就清除
        // 损坏记录，外壳随后按需要升级来处理，而不是再显示一遍损坏提示；仍然损坏就照旧记下。
        Err(e @ Error::Schema(_)) => {
            let library = Library::locate(path)?;
            if crate::app_integrity::recorded_damage(library.instance_path()).is_some() {
                let owner = claim_or_reuse(&library, current)?;
                match crate::app_integrity::check_both(&library, progress)? {
                    Some(damage) => {
                        owner.health.damaged(damage.clone())?;
                        return Err(damage.error());
                    }
                    None => owner.health.sound_again()?,
                }
            }
            return Err(e);
        }
        Err(e) => return Err(e),
    };
    let owner = claim_or_reuse(&library, current)?;
    let checked = if owner.health.needs_check() {
        if let Some(damage) = crate::app_integrity::check_both(&library, progress)? {
            owner.health.damaged(damage.clone())?;
            return Err(damage.error());
        }
        &crate::app_integrity::Database::BOTH[..]
    } else {
        &[]
    };
    finish_open(library, owner, checked)
}
/// 在打开 `path` 之前抢救它损坏的 source.sqlite（#228）：持有图库包（是会话自己的图库包时沿用会话的持有权），
/// 建好新文件并放到位。只抢救记录在案的 source.sqlite 损坏；library.sqlite 损坏时仍按它自己的提示处理。
pub(crate) fn rescue_package(
    path: &Path,
    current: Option<(&Library, &Arc<Owner>)>,
    reference: Option<&Path>,
    step: crate::app_rescue::Step,
) -> Result<crate::app_rescue::Report> {
    let library = Library::locate(path)?;
    let owner = claim_or_reuse(&library, current)?;
    match owner.health.damage() {
        Some(damage) if damage.database == crate::app_integrity::Database::Source => {}
        Some(damage) => return Err(damage.error()),
        None => return Err(Error::Invalid(crate::app_integrity::NOTHING_TO_RESCUE.into())),
    }
    crate::app_rescue::rescue(&library, &owner.health, reference, &AtomicBool::new(false), step)
}
/// 经过完整检查打开的图库包（显式升级），为会话持有。
pub(crate) fn own_checked(library: Library) -> Result<(Library, Arc<Owner>)> {
    let owner = Owner::claim(&library)?;
    finish_open(library, owner, &crate::app_integrity::Database::BOTH)
}

fn finish_open(
    library: Library,
    owner: Arc<Owner>,
    checked: &[crate::app_integrity::Database],
) -> Result<(Library, Arc<Owner>)> {
    // 副本折叠是派生状态：刷新失败（数据库忙、只读、磁盘满）不挡住打开。版本标记随刷新一起回滚，下次打开再刷新。
    // 数据库损坏照打开时的损坏处理（#212）：完整检查后记下损坏，不当成普通的打开失败。
    if let Err(error) = crate::app_curation::refresh_fold_on_open(&library) {
        if matches!(&error, Error::Sql(e) if crate::app_library::corruption(e).is_some()) {
            return Err(match crate::app_integrity::check_both(&library, None)? {
                Some(damage) => {
                    owner.health.damaged(damage.clone())?;
                    damage.error()
                }
                None => error,
            });
        }
        use crate::diagnostics::{failure, Operation, Publication, Stage};
        failure(Operation::Library, Stage::Write, &error, Publication::NotCommitted);
    }
    owner.health.opened(checked)?;
    Ok((library, owner))
}
fn claim_or_reuse(library: &Library, current: Option<(&Library, &Arc<Owner>)>) -> Result<Arc<Owner>> {
    match current {
        Some((open, owner)) if open.instance_path() == library.instance_path() => Ok(owner.clone()),
        _ => Owner::claim(library),
    }
}

pub(crate) struct Session {
    pub(crate) library: Library,
    /// 在会话关闭（切换或关闭）时释放，而不只是在服务被丢弃时。
    pub(crate) owner: Option<Arc<Owner>>,
    pub(crate) token: String,
    pub(crate) generation: u64,
    pub(crate) syncer: crate::app_sync::Syncer,
    pub(crate) wd_extension: crate::app_wd_extension::Tasks,
    wd_history: std::collections::HashMap<PathBuf, serde_json::Value>,
    pub(crate) names: crate::app_names::Updater,
    pub(crate) provenance: crate::app_provenance::Runner,
    pub(crate) index: crate::app_index::Updater,
    pub(crate) research: crate::app_research::Research,
    pub(crate) examples: Arc<crate::example_cache::Cache>,
    colors: crate::app_colors::Worker,
    pub(crate) onboarding: crate::app_onboarding::Onboarding,
    /// 与会话绑定的人工结论自动保护；关闭时取消，不等待整库哈希。
    pub(crate) human_backups: crate::app_human_conclusions::Backups,
    pub(crate) human_exports: crate::app_human_conclusions::Exports,
    /// 选中的图导出到文件夹（#450）；换库或关闭时停下。
    pub(crate) asset_exports: crate::app_asset_export::Exports,
    /// 打开这个图库时它为什么没能列入最近使用的图库（#176）。列入只是省得下次再选一遍，所以图库照样打开，页面提示一次。
    pub(crate) unlisted: Option<String>,
    /// 这个图库在打开时被升级过的话，升级前的备份（#193）：页面提示一次备份在哪里。
    pub(crate) upgraded: Option<String>,
    /// 后台完整性检查（#212）。
    pub(crate) checker: crate::app_integrity::Checker,
    /// 在后台抢救损坏的 source.sqlite（#228）。
    pub(crate) rescue: crate::app_rescue::Rescuer,
    /// 这个图库以抢救过的 source.sqlite 打开时，抢救做了什么：页面提示一次。
    pub(crate) rescued: Option<Json>,
    /// 发现了损坏，会话的任务已停止。
    pub(crate) halted: bool,
    /// 这次打开以后已经替等着默认启用的图库启动过角色资料包的启用（#630）。每次打开只自动启动一次：失败或取消后
    /// 卡片上照常有「重试」「启用…」，不在后台反复重试。
    pack_default_started: bool,
    /// 这次打开以后已经按角色目录补过一遍（#837），或者查过不用补。
    catalog_checked: bool,
}
impl Session {
    pub(crate) fn token() -> Result<String> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| Error::Invalid(e.to_string()))?;
        Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }
    fn new(
        (library, owner): (Library, Arc<Owner>),
        generation: u64,
        token: String,
        root: &Path,
        reference: &Path,
        startup: &SessionStartup,
    ) -> Result<Self> {
        let mut names = crate::app_names::Updater::new(
            &library,
            startup.names_enabled,
            startup.names_network_authorized,
            generation,
            &token,
        )?;
        names.wait_for_catalog();
        let provenance = crate::app_provenance::Runner::new(
            &library,
            generation,
            &token,
            startup.provenance_enabled,
            startup.provenance_network_authorized,
        )?;
        let index = crate::app_index::Updater::new(
            &library,
            root,
            generation,
            &token,
            startup.index_network_authorized,
            startup.index_auto_check,
        )?;
        let similar = crate::app_settings::startup(&library)?.get("similar").is_some_and(truthy);
        let mut research = crate::app_research::Research::new(generation, &token, similar);
        research.anchors = reference.parent().expect("内部参考文件路径必须有父目录").to_owned();
        let onboarding = crate::app_onboarding::Onboarding::new(generation, &token).with_resources(root);
        let mut session = Self {
            library,
            owner: Some(owner),
            generation,
            token,
            syncer: crate::app_sync::Syncer::default(),
            wd_extension: Default::default(),
            wd_history: Default::default(),
            names,
            provenance,
            index,
            research,
            examples: Default::default(),
            colors: Default::default(),
            onboarding,
            human_backups: Default::default(),
            human_exports: Default::default(),
            asset_exports: Default::default(),
            unlisted: None,
            upgraded: None,
            checker: Default::default(),
            rescue: Default::default(),
            rescued: None,
            halted: false,
            pack_default_started: false,
            catalog_checked: false,
        };
        session.syncer.resource_root = root.to_owned();
        session.syncer.per_run = startup.sync_limit;
        session.syncer.configure_startup(&session.library, !startup.sync_disabled)?;
        Ok(session)
    }
    /// 同步要等的后台任务正在进行（#607）：角色索引在写入，或 WD 扩展检查在跑。这时的同步请求先排着
    /// （[`crate::app_sync::Syncer::queue`]），由恢复检查在它们结束后开始。
    pub(crate) fn sync_waits(&self) -> bool {
        self.index.writing() || self.wd_extension.running()
    }
    /// 这个图库包记录在案的损坏：会话不再写入任何东西（#212）。
    pub(crate) fn damage(&self) -> Option<crate::app_integrity::Damage> {
        self.owner.as_ref().and_then(|owner| owner.health.damage())
    }
    /// 启动到期的后台检查（`delay` 让页面先加载）。
    pub(crate) fn start_checks(&mut self, delay: std::time::Duration) {
        if !self.halted {
            self.colors.start(&self.library);
        }
        if let Err(error) = self.human_backups.start(&self.library, self.generation, &self.token) {
            eprintln!("人工备份未能启动：{error}");
        }
        if let Some(owner) = &self.owner {
            let due = owner.health.due();
            self.checker.start(&self.library, owner.health.clone(), due, delay);
        }
    }
    /// `POST /api/integrity`：立即检查两个数据库，除非已有检查在运行。
    pub(crate) fn check_now(&mut self) -> Json {
        if let Some(owner) = &self.owner {
            self.checker.start(
                &self.library,
                owner.health.clone(),
                crate::app_integrity::Database::BOTH.to_vec(),
                std::time::Duration::ZERO,
            );
        }
        self.integrity()
    }
    /// `GET /api/integrity`，附带损坏的 source.sqlite 的抢救状态（#228）。
    pub(crate) fn integrity(&self) -> Json {
        let mut info = match &self.owner {
            Some(owner) => self.checker.info(&owner.health),
            None => Json::object([("running", serde_json::Value::Null.into())]),
        };
        info.set("rescue", self.rescue.info());
        info
    }
    /// `POST /api/integrity/rescue`：会话保持停止，在后台建一个新的 source.sqlite。
    pub(crate) fn start_rescue(&mut self, reference: Option<PathBuf>) -> Result<Json> {
        match self.damage() {
            Some(damage) if damage.database == crate::app_integrity::Database::Source => {}
            Some(damage) => return Err(damage.error()),
            None => return Err(Error::Invalid(crate::app_integrity::NOTHING_TO_RESCUE.into())),
        }
        self.checker.stop();
        self.rescue.start(&self.library, reference);
        Ok(self.integrity())
    }
}
pub struct Application {
    pub(crate) session: RwLock<Session>,
    pub(crate) generation: AtomicU64,
    closed: Arc<AtomicBool>,
    /// 有页面在等后台学习落定（`GET /api/learning`，#404）：主循环看到它就提前跑一次恢复检查。
    recovery_wanted: AtomicBool,
    pub(crate) tags: PathBuf,
    pub(crate) root: PathBuf,
    pub(crate) reference: PathBuf,
    pub(crate) startup: SessionStartup,
    pub(crate) images: crate::app_files::Images,
    pub(crate) exports: crate::app_archives::Exports,
    /// 识别结果导入和归档导入，一次一个，在会话锁之外进行（#26、#88）。
    pub(crate) imports: crate::app_imports::Imports,
}
pub(crate) fn truthy(value: &Json) -> bool {
    match value {
        Json::Scalar(serde_json::Value::Null) => false,
        Json::Scalar(serde_json::Value::Bool(value)) => *value,
        Json::Scalar(serde_json::Value::String(value)) => !value.is_empty(),
        Json::Scalar(serde_json::Value::Number(value)) => value.as_f64() != Some(0.0),
        Json::Integer(value) => value.trim_start_matches('-').chars().any(|c| c != '0'),
        Json::Float(value) => *value != 0.0,
        Json::Array(values) => !values.is_empty(),
        Json::Object(values) => !values.is_empty(),
        _ => true,
    }
}
/// 显式的应用配置；不需要套接字、桌面或客户端。
#[derive(Clone, Debug)]
pub struct Options {
    pub root: PathBuf,
    pub startup: SessionStartup,
}
impl Options {
    pub fn new(root: PathBuf) -> Self {
        Self { root, startup: SessionStartup::default() }
    }
}

/// 打开和切换图库共用的宿主启动配置。联网权限字段来自启动参数，仅在当前宿主生效；
/// 任务请求里的单次联网授权由各任务处理。启用功能与自动执行分别配置。
#[derive(Clone, Debug)]
pub struct SessionStartup {
    pub names_enabled: bool,
    pub names_network_authorized: bool,
    pub provenance_enabled: bool,
    pub provenance_network_authorized: bool,
    pub index_network_authorized: bool,
    pub index_auto_check: bool,
    pub auto_sync: bool,
    pub sync_disabled: bool,
    pub sync_limit: usize,
}
impl Default for SessionStartup {
    fn default() -> Self {
        Self {
            names_enabled: true,
            names_network_authorized: false,
            provenance_enabled: true,
            provenance_network_authorized: false,
            index_network_authorized: false,
            index_auto_check: true,
            auto_sync: true,
            sync_disabled: false,
            sync_limit: 0,
        }
    }
}
impl Application {
    /// 打开并登记一个显式指定的图库包。不绑定端口，也不启动后台工作。
    pub fn open(path: &Path, options: Options) -> Result<Self> {
        Self::from_library(open_owned(path, None, None)?, Session::token()?, options, |error| {
            crate::app_libraries::unlisted_reason(&error)
        })
    }
    /// 在下面任何恢复步骤读取或改写图库包的任务状态之前，`opened` 已被持有。
    pub(crate) fn from_library(
        opened: (Library, Arc<Owner>),
        token: String,
        options: Options,
        unlisted_note: impl FnOnce(Error) -> String,
    ) -> Result<Self> {
        let Options { root, startup } = options;
        crate::app_packs::character_catalog::load(&root);
        let reference = crate::app_resources::reference_path(&root)?;
        let mut session = Session::new(opened, 1, token, &root, &reference, &startup)?;
        if let Err(error) = crate::app_libraries::remember(&session.library) {
            let note = unlisted_note(error);
            eprintln!("没能记进最近图库，图库照常打开：{note}");
            session.unlisted = Some(note);
        }
        Ok(Self {
            session: RwLock::new(session),
            generation: AtomicU64::new(1),
            closed: Arc::default(),
            recovery_wanted: AtomicBool::new(false),
            tags: root.join("tags/tags_v3.csv"),
            root,
            reference,
            startup,
            images: crate::app_files::Images::default(),
            exports: crate::app_archives::Exports::default(),
            imports: crate::app_imports::Imports::default(),
        })
    }
}

impl Application {
    /// 启动既定的任务和持久事件的恢复；不涉及传输层。
    pub fn start(&self) -> Result<()> {
        self.check_open()?;
        self.start_reporting(|| {})
    }
    pub(crate) fn start_reporting(&self, before_recovery: impl FnOnce()) -> Result<()> {
        {
            let session = self.session.read().expect("app_http session lock poisoned");
            self.check_open()?;
            let lib = session.library.clone();
            if crate::app_research::configured() {
                crate::app_research::dispatch(&lib, "imported")?;
            }
        }
        before_recovery();
        self.recover();
        {
            let mut session = self.session.write().expect("app_http session lock poisoned");
            self.check_open()?;
            if self.startup.auto_sync {
                let lib = session.library.clone();
                let token = session.token.clone();
                let generation = session.generation;
                let wait = session.sync_waits();
                session.syncer.automatic(&lib, generation, &token, wait)?;
            }
            session.start_checks(crate::app_integrity::BACKGROUND_DELAY);
        }
        Ok(())
    }
    /// 请主循环尽快跑一次恢复检查，不等到下一个整秒（#404）。
    pub(crate) fn recover_soon(&self) {
        self.recovery_wanted.store(true, Ordering::SeqCst);
    }
    /// 取走 [`Self::recover_soon`] 留下的请求。
    pub(crate) fn take_recover_soon(&self) -> bool {
        self.recovery_wanted.swap(false, Ordering::SeqCst)
    }
    /// 只在持有原来那个图库实例时重放持久化的工作。
    pub fn recover(&self) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let mut session = self.session.write().expect("app_http session lock poisoned");
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        // 后台发现的损坏会停止会话的任务；不向损坏的图库包恢复任何东西。
        if session.damage().is_some() {
            if !session.halted {
                session.halted = true;
                session.halt();
                drop(session);
                self.imports.cancel(crate::app_imports::DAMAGED);
                self.exports.stop();
            }
            return;
        }
        let library = session.library.clone();
        session.provenance.recover();
        let busy = session.syncer.running() || session.wd_extension.running();
        if !busy && self.wd_busy(&session).is_none() {
            if let Err(error) = apply_catalog(&mut session) {
                eprintln!("角色目录未能补到图库里：{error}");
            }
        }
        session.index.recover(&library, busy);
        let base_busy = busy
            || self.imports.busy()
            || self.exports.running()
            || session.human_exports.running()
            || self.imports.recognition_running(session.generation);
        session.index.sync_base(&library, base_busy);
        // 排着的同步（#607）：挡着它的后台任务结束后接着开始。基础索引的启用排在前面，这次识别用的就是新的锚点。
        if session.syncer.queued()
            && !session.syncer.running()
            && !session.sync_waits()
            && !self.imports.recognition_running(session.generation)
            && session.syncer.take_queued()
        {
            let (generation, token) = (session.generation, session.token.clone());
            if let Err(error) = session.syncer.start(&library, generation, &token) {
                eprintln!("排着的同步未能开始：{error}");
            }
        }
        if let Err(error) = self.start_pack_default(&mut session) {
            eprintln!("角色资料包未能默认启用：{error}");
        }
        if session.catalog_checked {
            session.names.catalog_ready();
        }
        session.names.recover(&library);
        if let Err(error) = session.research.recover(&library, busy) {
            eprintln!("研究事件恢复失败：{error}");
        }
        if let Err(error) = crate::app_effects::drain(&session.library.db()) {
            eprintln!("native verdict事件恢复失败：{error}");
        }
    }
    pub(crate) fn wd_busy(&self, session: &Session) -> Option<&'static str> {
        if session.syncer.running()
            || self.imports.active_kind(session.generation) == Some(crate::app_imports::Kind::Recognition)
        {
            Some("syncing")
        } else if session.index.writing()
            || self.imports.busy()
            || self.exports.running()
            || session.human_exports.running()
        {
            Some("busy")
        } else {
            None
        }
    }

    /// 新图库第一次同步时角色资料包还不在本机（#630）：包就位后照默认启用，和用户点「启用」走同一个任务，检查已有的识别
    /// 结果。等同步、导入导出、角色索引和别的 WD 扩展任务都结束，基础角色索引也启用了（`Updater::base_outstanding`）
    /// 才开始；用户选过不用或撤回过的图库不在此列
    /// （`app_packs::default_state`）。
    fn start_pack_default(&self, session: &mut Session) -> Result<()> {
        if session.pack_default_started
            || session.library.frozen()
            || session.wd_extension.running()
            || self.wd_busy(session).is_some()
            || session.index.base_outstanding(&session.library)
        {
            return Ok(());
        }
        // 每秒都会走到这里：先只读一个标记，大多数图库到这里就结束。
        let app = crate::app_library::read_connection(&session.library.db())?;
        if !crate::app_wd_extension::default_pending(&app)? {
            return Ok(());
        }
        drop(app);
        let Some(pack) = crate::app_packs::pending_enable(&self.root, &session.library)? else { return Ok(()) };
        session.pack_default_started = true;
        let action = crate::app_packs::enable_action(&self.root, &session.library, &pack)?;
        let identity = crate::app_tasks::TaskIdentity::new(&session.library, session.generation, &session.token)?;
        session.wd_extension.start_for(&session.library, identity, action)?;
        crate::app_ccip_longtail::clear_auto_task(&session.library);
        Ok(())
    }

    /// 停止当前会话的任务和导出并等待它们结束；正在进行的导入有一段有限的等待时间，用来删除它解开的文件。
    pub fn shutdown(&self) {
        let mut session = self.session.write().expect("application session lock poisoned");
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.imports.cancel(crate::app_imports::QUITTING);
        session.close();
        drop(session);
        self.exports.shutdown();
        self.imports.shutdown(crate::app_imports::QUIT_WAIT);
    }
    /// 在接受工作时取得，早于读取请求体、调度或等待锁。
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
    /// 针对接受时的那个物理实例执行，等到锁之后再复核。
    /// 图库包里的文件按打开时确定的规范路径解析，所以显示用的别名无法让它们指向别处。
    pub fn with_library<T>(&self, received: u64, operation: impl FnOnce(&Library) -> Result<T>) -> Result<T> {
        let session = self.session.read().expect("application session lock poisoned");
        self.check_open()?;
        if !self.accepts(&session, received) {
            return Err(Error::Conflict("会话已过期，请刷新页面".into()));
        }
        operation(&session.library)
    }
}
impl Session {
    fn close(&mut self) {
        self.checker.stop();
        self.rescue.stop();
        // 损坏的图库包也不会再得到排队的溯源工作。
        if !self.halted {
            self.provenance.close();
        }
        self.halt();
        self.owner = None;
    }
    /// 发现损坏：停止所有任务，不在图库包里排队或记录任何东西。
    fn halt(&mut self) {
        self.colors.stop();
        self.human_backups.stop();
        self.human_exports.stop();
        self.asset_exports.stop();
        self.provenance.stop();
        self.index.stop();
        self.onboarding.stop();
        self.research.stop();
        self.examples.clear();
        self.names.stop();
        self.syncer.stop();
        self.wd_extension.stop();
    }
}

impl Application {
    /// 切换到一个显式指定的图库包，拒绝任何之前实例接受的工作。
    pub fn switch(&self, received: u64, path: &Path) -> Result<PathBuf> {
        let mut session = self.session.write().expect("application session lock poisoned");
        self.switch_locked(&mut session, received, path)?;
        Ok(session.library.display_path().to_owned())
    }
    pub(crate) fn switch_locked(&self, session: &mut Session, received: u64, path: &Path) -> Result<Json> {
        self.check_open()?;
        if !self.accepts(session, received) {
            return Err(Error::Conflict("会话已过期，请刷新页面".into()));
        }
        if !path.join("library.json").is_file() {
            return Err(Error::Invalid("不是图库包".into()));
        }
        // 重新打开同一个物理图库包时保留其持有权；其他图库包要先取得持有权。
        let opened = open_owned(path, session.owner.as_ref().map(|owner| (&session.library, owner)), None)?;
        let mut replacement = Session::new(
            opened,
            session.generation + 1,
            Session::token()?,
            &self.root,
            &self.reference,
            &self.startup,
        )?;
        if let Err(error) = crate::app_libraries::remember(&replacement.library) {
            let note = crate::app_libraries::unlisted_reason(&error);
            eprintln!("没能记进最近图库，图库照常打开：{note}");
            replacement.unlisted = Some(note);
        }
        let result = Json::object([
            ("path", replacement.library.display_path().to_string_lossy().as_ref().into()),
            ("name", replacement.library.name().into()),
        ]);
        session.close();
        let wd_state = session.wd_extension.snapshot();
        if !wd_state.is_null() {
            let previous = session.library.instance_path().to_owned();
            session.wd_history.insert(previous, wd_state);
        }
        replacement.wd_history = std::mem::take(&mut session.wd_history);
        replacement.wd_extension.restore(replacement.wd_history.remove(replacement.library.instance_path()));
        self.exports.stop();
        // 导入不会跟着切换走：它在发布之前停止，替换后的会话从干净的状态开始。
        self.imports.cancel(crate::app_imports::SWITCHED);
        *session = replacement;
        let library = session.library.clone();
        if let Err(e) = crate::app_research::dispatch(&library, "imported") {
            eprintln!("研究启动调度失败：{e}");
        }
        self.generation.store(session.generation, Ordering::SeqCst);
        self.recover_soon();
        if self.startup.auto_sync {
            let lib = session.library.clone();
            let token = session.token.clone();
            let gen = session.generation;
            let wait = session.sync_waits();
            if let Err(e) = session.syncer.automatic(&lib, gen, &token, wait) {
                eprintln!("同步自动启动失败：{e}");
            }
        }
        session.start_checks(crate::app_integrity::BACKGROUND_DELAY);
        Ok(result)
    }
}

impl Application {
    /// 用抢救好的 source.sqlite 代替损坏的那个，重新打开会话自己的图库包（#228）。
    pub(crate) fn reopen_rescued(&self, session: &mut Session, received: u64) -> Result<Json> {
        self.check_open()?;
        if !self.accepts(session, received) {
            return Err(Error::Conflict("会话已过期，请刷新页面".into()));
        }
        let package = session.library.instance_path().to_owned();
        let rebuilt =
            session.rescue.take(&package).ok_or_else(|| Error::Invalid("抢救结果已不在图库里，请重新抢救".into()))?;
        let owner = session.owner.clone().ok_or_else(|| Error::Conflict("应用服务已关闭".into()))?;
        // 替换期间，这个会话的任何部分都不得打开着 source.sqlite。
        session.checker.stop();
        session.halt();
        self.exports.stop();
        let report = crate::app_rescue::install(&session.library, &owner.health, rebuilt)?.json();
        let path = session.library.display_path().to_owned();
        let mut out = self.switch_locked(session, received, &path)?;
        session.rescued = Some(report.clone());
        out.set("rescued", report);
        Ok(out)
    }
}

impl Drop for Application {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Application {
    pub(crate) fn accepts(&self, session: &Session, received: u64) -> bool {
        !self.closed.load(Ordering::SeqCst) && session.generation == received
    }
    pub(crate) fn current(&self, received: u64) -> bool {
        !self.closed.load(Ordering::SeqCst) && self.generation() == received
    }
    fn check_open(&self) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            Err(Error::Conflict("应用服务已关闭".into()))
        } else {
            Ok(())
        }
    }
}
/// 显式创建与 HTTP 和欢迎页外壳使用同样的工厂和参考规则。
/// 已发布的图库包在之后登记失败时仍然保留。
pub fn create(root: &Path, dest: &Path, name: &str, sources: Vec<crate::app_create::NewSource>) -> Result<PathBuf> {
    let created = build_sources(root, dest, name, sources)?;
    let path = created["path"].as_str().ok_or_else(|| Error::Schema("建库结果缺少路径".into()))?;
    let path = PathBuf::from(path);
    crate::app_libraries::remember_created(&path, name)?;
    Ok(path)
}
pub(crate) fn build_sources(
    root: &Path,
    dest: &Path,
    name: &str,
    sources: Vec<crate::app_create::NewSource>,
) -> Result<serde_json::Value> {
    let reference = crate::app_resources::reference_path(root)?;
    crate::app_create::create_sources(dest, name, sources, crate::app_create::optional_reference(&reference)?)
        .map_err(|error| crate::app_create::creation_error(error, &reference))
}

/// 只打开已有的库：路径不存在时 SQLite 会新建空文件，先拒绝。
pub(crate) fn existing(db: &Path) -> crate::Result<rusqlite::Connection> {
    if !db.is_file() {
        return Err(Error::Invalid(format!("找不到数据库：{path}", path = db.display())));
    }
    crate::open(db)
}

/// CLI 与常驻进程使用同一事务边界，每条命令单独连接；空闲时不持有文件或写锁。
pub(crate) fn character_request(db: &Path, input: &serde_json::Value) -> crate::Result<serde_json::Value> {
    character_request_with(db, input, false)
}

pub(crate) fn character_request_with(
    db: &Path,
    input: &serde_json::Value,
    historical_index: bool,
) -> crate::Result<serde_json::Value> {
    let mut c = existing(db)?;
    crate::schema::require_current(&c)?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if matches!(input["op"].as_str(), Some("change" | "sweep" | "answer")) {
        tx.execute("INSERT OR REPLACE INTO meta(key,value) VALUES (?1,'1')", [crate::schema::WRITER_KEY])?;
    } else {
        crate::schema::begin_writing(&tx)?;
    }
    let replayed = if historical_index { illustash_core::character_import::replay_index(&tx, input)? } else { None };
    let result = match replayed {
        Some(result) => result,
        None => crate::character_commands::command(&tx, input)?,
    };
    if let Some(effect) = input.get("effect").and_then(serde_json::Value::as_str) {
        tx.execute("INSERT INTO backend_effects(kind) VALUES (?)", [effect])?;
    }
    crate::schema::commit_writing(tx)?;
    Ok(result)
}

const CATALOG_DISPLAY_POLICY: &str = "[\"catalog-display\", 1]";

fn catalog_display_needed(
    c: &rusqlite::Connection,
    catalog: &crate::character_catalog::Catalog,
    stamp: &str,
) -> Result<bool> {
    use rusqlite::OptionalExtension;
    let previous: Option<String> =
        c.query_row("SELECT value FROM name_cache WHERE key=?", [CATALOG_DISPLAY_POLICY], |r| r.get(0)).optional()?;
    if let Some(previous) = previous {
        return Ok(previous != stamp);
    }
    // 新旧图库都可能还没有标记（例如先开空库，随后才识别人物）。文字键投影还包含
    // 本地造型和被并入的角色。只要现有键用到新规则，整表一起重建，包含其父子显示名。
    let mut stmt =
        c.prepare("SELECT kind, key, name_source, name, zh FROM term_name_choice WHERE kind IN ('character','skin')")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let kind: String = row.get(0)?;
        let key: String = row.get(1)?;
        if (kind == "skin" && crate::names_view::expanded_skin_name(&key))
            || (kind == "character" && catalog.reviewed_name(&key).is_some())
        {
            return Ok(true);
        }
        // 上一次空库开过后导入的离线中文名可能已被新修订撤下。另一个语言的用户名称
        // 可以盖过主显示名，仍需检查物化的中文栏。
        let source: String = row.get(2)?;
        if kind == "character" && matches!(source.as_str(), "builtin" | "user") {
            let old_name: String = row.get(3)?;
            let old_zh: Option<String> = row.get(4)?;
            if old_zh.as_ref().is_some_and(|zh| source == "builtin" || old_name != *zh) {
                let current = crate::names_view::choose_key(c, &kind, &key)?;
                if current.name != old_name || current.source != source || current.names[0] != old_zh {
                    return Ok(true);
                }
            }
        }
    }
    // 所有造型都采用新的手动名称优先顺序，包括没有内置译名的造型。
    let manual: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM term_tree r JOIN term_names n ON r.term_id=n.term_id
                       WHERE r.parent_id IS NOT NULL AND n.role='display' AND n.source='user')
         OR EXISTS(SELECT 1 FROM term_key_names k JOIN term_name_choice d ON d.kind=k.kind AND d.key=k.key
                   WHERE k.kind='skin' AND k.source='user')",
        [],
        |r| r.get(0),
    )?;
    Ok(manual)
}

/// 在启用名称任务前修复目录中的身份关系，并重算依赖本机词表的显示名。
fn apply_catalog(session: &mut Session) -> Result<()> {
    use rusqlite::OptionalExtension;
    if session.catalog_checked {
        return Ok(());
    }
    if session.library.frozen() {
        session.catalog_checked = true;
        return Ok(());
    }
    let catalog = crate::character_catalog::current();
    let revision = i64::from(catalog.revision);
    let display_stamp = revision.to_string();
    let needs_roles = |c: &rusqlite::Connection| -> Result<bool> {
        let applied: Option<i64> = c
            .query_row(
                "SELECT json_extract(value,'$.revision') FROM meta WHERE key=?",
                [illustash_core::character_import::CATALOG_META],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(applied.is_none_or(|applied| applied < revision))
    };
    let app = crate::app_library::read_connection(&session.library.db())?;
    let needed = needs_roles(&app)? || catalog_display_needed(&app, catalog, &display_stamp)?;
    drop(app);
    if !needed {
        session.catalog_checked = true;
        return Ok(());
    }
    let mut app = crate::sqlite::open_with_flags(session.library.db(), rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    crate::configure(&app)?;
    app.busy_timeout(crate::app_library::TASK_WRITE_WAIT)?;
    crate::schema::require_current(&app)?;
    let tx = app.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if catalog_display_needed(&tx, catalog, &display_stamp)? {
        crate::legacy_names::invalidate_display(&tx)?;
        tx.execute("INSERT OR REPLACE INTO name_cache VALUES (?,?)", [CATALOG_DISPLAY_POLICY, display_stamp.as_str()])?;
    }
    crate::schema::begin_writing(&tx)?;
    if needs_roles(&tx)? {
        crate::character_commands::command(&tx, &serde_json::json!({"op": "catalog_apply"}))?;
    }
    crate::schema::commit_writing(tx)?;
    session.catalog_checked = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_precedes_initial_names_on_open_switch_and_retry() {
        let root = std::env::temp_dir().join(format!("illustash-catalog-session-{}", crate::store::new_key().unwrap()));
        std::fs::create_dir_all(root.join("source")).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "app_service::tests::catalog_names_fixture"])
            .env("ILLUSTASH_SERVICE_FIXTURE", &root)
            .env("ILLUSTASH_APP_SUPPORT", root.join("support"))
            .env("ILLUSTASH_SHARED", root.join("absent-reference"))
            .env("ILLUSTASH_NO_KEYCHAIN", "1")
            .status()
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
        assert!(status.success());
    }

    #[test]
    #[ignore = "isolated in-process fixture, invoked with synthetic support by parent test"]
    fn catalog_names_fixture() {
        use serde_json::{json, Value};
        const GUARD: &str = "CREATE TRIGGER names_need_catalog BEFORE INSERT ON backend_effects
            WHEN NEW.kind LIKE 'names:%' AND NOT EXISTS(
                SELECT 1 FROM term_links l JOIN terms t ON t.id=l.term_id
                JOIN term_tree r ON r.parent_id=t.key WHERE l.system='danbooru' AND l.key='fixture_parent')
            BEGIN SELECT RAISE(ABORT,'names saw the old catalog'); END";
        let root = PathBuf::from(std::env::var_os("ILLUSTASH_SERVICE_FIXTURE").expect("isolated fixture only"));
        let catalog = |revision: u32, roles: Value, names: Value| {
            let doc = json!({"format":crate::character_catalog::FORMAT,"revision":revision,
                "columns":["id","tag","tags","parent","relation","action","variant","bangumi","pick","wikidata"],
                "characters":roles,"works":[],"wikidata_names":names});
            let catalog = crate::character_catalog::Catalog::parse(&doc.to_string()).unwrap();
            crate::character_catalog::override_for_thread(Some(Box::leak(Box::new(catalog))));
        };
        catalog(2, json!([]), json!({}));
        let create = |name: &str| {
            let path = root.join(format!("{name}.pilib"));
            crate::app_create::create_folder(&path, &root.join("source"), None, None, None).unwrap();
            let db = path.join("library.sqlite");
            character_request(
                &db,
                &json!({"op":"refresh","assets":[["coat","coat.png","coat.png",8,8,"2026-11-23","folder"]],
                    "indexed":"2026-11-23","index":{},"labels":[["coat","fixture_costume",0.99,"character"]]}),
            )
            .unwrap();
            crate::sqlite::open(&db).unwrap().execute_batch(GUARD).unwrap();
            path
        };
        let (first, second) = (create("first"), create("second"));
        let first_db = first.join("library.sqlite");
        {
            use illustash_core::character_entities::{create, New};
            let mut c = existing(&first_db).unwrap();
            let tx = c.transaction().unwrap();
            crate::schema::begin_writing(&tx).unwrap();
            create(&tx, &New { id: "old_parent", kind: "character", ..New::default() }, "import").unwrap();
            create(
                &tx,
                &New {
                    id: "old_skin",
                    kind: "skin",
                    parent_id: Some("old_parent"),
                    variant: "winter_casual",
                    ..New::default()
                },
                "import",
            )
            .unwrap();
            tx.execute(
                "INSERT INTO term_links(system,key,term_id,source)
                 SELECT 'danbooru',key,id,'import' FROM terms WHERE key IN ('old_parent','old_skin')",
                [],
            )
            .unwrap();
            crate::schema::commit_writing(tx).unwrap();
        }
        let stale_skin_projection = |c: &rusqlite::Connection| {
            c.execute_batch(
                "UPDATE term_name_display SET name='Winter Casual', name_source='readable', zh=NULL
                 WHERE entity='old_skin' AND skin_only=1;
                 UPDATE term_name_choice SET name='Winter Casual', name_source='readable', zh=NULL
                 WHERE kind='skin' AND key='winter_casual';",
            )
            .unwrap();
        };
        let check_skin_projection = |db: &Path| {
            let c = existing(db).unwrap();
            for query in [
                "SELECT name FROM term_name_display WHERE entity='old_skin' AND skin_only=1",
                "SELECT name FROM term_name_choice WHERE kind='skin' AND key='winter_casual'",
            ] {
                assert_eq!(c.query_row(query, [], |r| r.get::<_, String>(0)).unwrap(), "冬季 · 便服");
            }
        };
        stale_skin_projection(&existing(&first_db).unwrap());
        catalog(
            2,
            json!([
                [1, "fixture_parent", [], null, null, null, null, null, "", null],
                [2, "fixture_costume", [], 1, "appearance", "auto", "coat", null, "", null]
            ]),
            json!({}),
        );
        let mut options = Options::new(root.clone());
        options.startup.provenance_enabled = false;
        options.startup.index_auto_check = false;
        options.startup.auto_sync = false;
        options.startup.sync_disabled = true;
        let service = Application::open(&first, options.clone()).unwrap();
        let name_runs = |db: &Path| {
            crate::sqlite::open(db)
                .unwrap()
                .query_row("SELECT count(*) FROM backend_effects WHERE kind LIKE 'names:%'", [], |r| r.get::<_, i64>(0))
                .unwrap()
        };
        service.start().unwrap();
        check_skin_projection(&first_db);
        assert_eq!(name_runs(&first.join("library.sqlite")), 1);

        let db = crate::sqlite::open(second.join("library.sqlite")).unwrap();
        db.execute_batch(
            "CREATE TRIGGER delay_catalog BEFORE INSERT ON meta WHEN NEW.key='character_catalog'
            BEGIN SELECT RAISE(ABORT,'temporary catalog write failure'); END;",
        )
        .unwrap();
        service.switch(service.generation(), &second).unwrap();
        service.recover();
        assert_eq!(name_runs(&second.join("library.sqlite")), 0);
        assert_eq!(name_runs(&first.join("library.sqlite")), 1);
        db.execute_batch("DROP TRIGGER delay_catalog").unwrap();
        service.recover();
        assert_eq!(name_runs(&second.join("library.sqlite")), 1);
        service.recover();
        assert_eq!(name_runs(&second.join("library.sqlite")), 1);
        assert!(!db
            .query_row("SELECT EXISTS(SELECT 1 FROM name_cache WHERE key=?)", [CATALOG_DISPLAY_POLICY], |r| r
                .get::<_, bool>(0))
            .unwrap());
        // 结构修订已经就绪时，换回较低的词表版本也要重建显示名。
        let c = existing(&first_db).unwrap();
        c.execute("UPDATE name_cache SET value='99' WHERE key=?", [CATALOG_DISPLAY_POLICY]).unwrap();
        stale_skin_projection(&c);
        service.switch(service.generation(), &first).unwrap();
        service.recover();
        check_skin_projection(&first_db);
        let revision = |c: &rusqlite::Connection| {
            c.query_row("SELECT value FROM name_cache WHERE key=?", [CATALOG_DISPLAY_POLICY], |r| r.get::<_, String>(0))
                .unwrap()
        };
        assert_eq!(revision(&c), "2");
        service.recover();
        assert_eq!(revision(&c), "2");

        let late = root.join("late.pilib");
        crate::app_create::create_folder(&late, &root.join("source"), None, None, None).unwrap();
        let late_db = late.join("library.sqlite");
        let roles = json!([[3, "late_hero", [], null, null, null, null, null, "", "Q123"]]);
        catalog(3, roles.clone(), json!({"Q123":"迟来的角色"}));
        service.switch(service.generation(), &late).unwrap();
        service.recover();
        character_request(
            &late_db,
            &json!({"op":"refresh","assets":[["late","late.png","late.png",8,8,"2026-11-23","folder"]],
                "indexed":"2026-11-23","index":{},"labels":[["late","late_hero",0.99,"character"]]}),
        )
        .unwrap();
        let chosen = |c: &rusqlite::Connection| {
            for query in [
                "SELECT name FROM term_name_display WHERE tag='late_hero' AND skin_only=0",
                "SELECT name FROM term_name_choice WHERE key='late_hero' AND kind='character'",
            ] {
                assert_eq!(
                    c.query_row(query, [], |r| r.get::<_, String>(0)).unwrap(),
                    crate::names_view::choose_key(c, "character", "late_hero").unwrap().name,
                );
            }
        };
        let c = existing(&late_db).unwrap();
        chosen(&c);
        assert_eq!(crate::names_view::choose_key(&c, "character", "late_hero").unwrap().name, "迟来的角色");
        assert!(!c
            .query_row("SELECT EXISTS(SELECT 1 FROM name_cache WHERE key=?)", [CATALOG_DISPLAY_POLICY], |r| r
                .get::<_, bool>(0),)
            .unwrap());
        service.shutdown();
        catalog(4, roles, json!({}));
        let service = Application::open(&late, options).unwrap();
        service.start().unwrap();
        chosen(&c);
        assert_eq!(crate::names_view::choose_key(&c, "character", "late_hero").unwrap().name, "Late Hero");
        assert_eq!(revision(&c), "4");
        service.shutdown();
    }

    #[test]
    fn lock_wait_rechecks_accepted_generation() {
        let root = std::env::temp_dir().join(format!(
            "illustash-service-lock-{}-{}",
            std::process::id(),
            crate::store::new_key().unwrap()
        ));
        std::fs::create_dir_all(root.join("source")).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "app_service::tests::lock_wait_fixture"])
            .env("ILLUSTASH_SERVICE_FIXTURE", &root)
            .env("ILLUSTASH_APP_SUPPORT", root.join("support"))
            .env("ILLUSTASH_SHARED", root.join("absent-reference"))
            .env("ILLUSTASH_NO_KEYCHAIN", "1")
            .status()
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
        assert!(status.success());
    }
    #[test]
    #[ignore = "isolated in-process fixture, invoked with synthetic support by parent test"]
    fn lock_wait_fixture() {
        let root = PathBuf::from(std::env::var_os("ILLUSTASH_SERVICE_FIXTURE").expect("isolated fixture only"));
        let create = |name: &str| {
            create(
                &root,
                &root.join(format!("{name}.pilib")),
                name,
                vec![crate::app_create::NewSource::Folder { path: root.join("source"), preset: "other".into() }],
            )
            .unwrap()
        };
        let (first, second) = (create("first"), create("second"));
        let mut options = Options::new(root);
        options.startup.names_enabled = false;
        options.startup.provenance_enabled = false;
        options.startup.index_auto_check = false;
        options.startup.auto_sync = false;
        options.startup.sync_disabled = true;
        let service = std::sync::Arc::new(Application::open(&first, options.clone()).unwrap());
        let received = service.generation();
        let mut session = service.session.write().unwrap();
        let waiter = service.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            tx.send(()).unwrap();
            waiter.with_library::<()>(received, |_| panic!("old waiting request applied to replacement"))
        });
        rx.recv().unwrap();
        service.switch_locked(&mut session, received, &second).unwrap();
        drop(session);
        assert!(matches!(thread.join().unwrap(), Err(Error::Conflict(_))));
        // 这些任务占用真实的 Session Syncer 槽位。它现有的停止路径必须取消并等待它们结束；
        // 持有它们的关口期间，服务不能报告正在切换或关闭。
        for finish in ["switch", "shutdown", "drop"] {
            let target_service = if finish == "drop" {
                std::sync::Arc::new(Application::open(&first, options.clone()).unwrap())
            } else {
                service.clone()
            };
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let (cancel_tx, cancel_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let generation = target_service.generation();
            {
                let mut session = target_service.session.write().unwrap();
                let identity =
                    crate::app_tasks::TaskIdentity::new(&session.library, generation, &session.token).unwrap();
                let task = crate::app_tasks::Task::start(identity, None, move |ctx| {
                    ready_tx.send(ctx.clone()).unwrap();
                    let deadline = crate::test_wait::deadline();
                    while ctx.check().is_ok() {
                        assert!(std::time::Instant::now() < deadline, "service did not cancel its task");
                        std::thread::yield_now();
                    }
                    cancel_tx.send(()).unwrap();
                    release_rx.recv_timeout(crate::test_wait::limit()).unwrap();
                    assert!(ctx.receive::<()>(ctx.identity(), || panic!("cancelled owned task published")).is_err());
                    Ok(())
                });
                session.syncer.install_fixture_task(task);
            }
            let context = ready_rx.recv_timeout(crate::test_wait::limit()).unwrap();
            let observe = (finish != "drop").then(|| target_service.clone());
            let finishing = target_service;
            let target = first.clone();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let thread = std::thread::spawn(move || {
                if finish == "shutdown" {
                    finishing.shutdown();
                } else if finish == "drop" {
                    drop(finishing);
                } else {
                    finishing.switch(generation, &target).unwrap();
                }
                done_tx.send(()).unwrap();
            });
            cancel_rx.recv_timeout(crate::test_wait::limit()).unwrap();
            assert!(matches!(done_rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));
            if let Some(observe) = observe {
                if finish == "shutdown" {
                    assert!(!observe.current(generation));
                } else {
                    assert_eq!(observe.generation(), generation);
                }
            }
            release_tx.send(()).unwrap();
            done_rx.recv_timeout(crate::test_wait::limit()).unwrap();
            thread.join().unwrap();
            assert!(context.receive::<()>(context.identity(), || panic!("joined owned task published")).is_err());
        }
        assert!(service.with_library(service.generation(), |_| Ok(())).is_err());
    }
}
