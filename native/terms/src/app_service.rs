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
    owner.health.opened(checked)?;
    Ok((library, owner))
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
    owner.health.opened(&crate::app_integrity::Database::BOTH)?;
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
        let names = crate::app_names::Updater::new(
            &library,
            startup.names_enabled,
            startup.names_network_authorized,
            generation,
            &token,
        )?;
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
            let mut session = self.session.write().expect("app_http session lock poisoned");
            self.check_open()?;
            let library = session.library.clone();
            if self.startup.names_enabled {
                let _ = session.names.start(&library, None, false);
            }
        }
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
        if self.startup.names_enabled {
            let _ = session.names.start(&library, None, false);
        }
        self.generation.store(session.generation, Ordering::SeqCst);
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
    let mut c = existing(db)?;
    crate::schema::require_current(&c)?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if matches!(input["op"].as_str(), Some("change" | "sweep" | "answer")) {
        tx.execute("INSERT OR REPLACE INTO meta(key,value) VALUES (?1,'1')", [crate::schema::WRITER_KEY])?;
    } else {
        crate::schema::begin_writing(&tx)?;
    }
    let result = crate::character_commands::command(&tx, input)?;
    if let Some(effect) = input.get("effect").and_then(serde_json::Value::as_str) {
        tx.execute("INSERT INTO backend_effects(kind) VALUES (?)", [effect])?;
    }
    crate::schema::commit_writing(tx)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
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
