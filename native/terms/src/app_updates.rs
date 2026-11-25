//! 检查更新 A 档（#191，在 #165 中决定）：向 GitHub 查询最新的 Release，与本构建的版本（native/terms/application.json）
//! 比较，并让页面链接到 Release 页面；不下载也不替换任何东西。联网规则与其他地方相同：自动检查在用户允许之前一直关闭
//! （新图库的首次引导问一次，可在「设置 › 网络与隐私」中更改）；允许后，App 每次启动时检查一次，一直开着时每过一天
//! 再检查一次（用户 2026-10-18：Mac 上的 App 可能一开就是几周）。手动检查就是用户的点击。
//! 请求是对公开 Release 列表的普通 HTTPS GET：不带账号，也不带图库信息，只发给 api.github.com，不跟随重定向。
//! 列表按仓库的数字 ID 查（#358），所以仓库改名或转移后，已经发出去的版本仍查得到。
//!
//! 和界面语言一样是全应用范围的：选择存在 preferences.json 里（`update_check`：`"auto"` 或 `"manual"`，
//! 问过之前没有这个键）；最近一次结果存在旁边的 updates.json。
//!
//! B 档（#279）：在桌面 App 里，查到的新版本还可以「下载并重启」。下载、校验签名、安装和重启都由外壳做
//! （desktop/src/updater.rs，Tauri 更新插件），这里只经本机桥（`app_desktop`）转达用户的点击、取回进度。下载哪个版本
//! 沿用上面的检查结果，所以通道规则与检查更新完全相同；包的地址由 `package` 按版本号拼出，在这个版本自己的 Release 下面，
//! 不依赖 GitHub 把哪个 Release 标成 Latest。
use crate::{
    app_paths::{app_support, user_home},
    http_json::Json,
    Error, Result,
};
use serde_json::{Map, Value};
use std::{
    cmp::Ordering,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering as Atomic},
    time::Duration,
};

const REPOSITORY: &str = "shio-vinyl/illustash";
/// GitHub 给这个仓库的数字 ID（`gh api repos/shio-vinyl/illustash --jq .id`）。Release 列表按它查（#358）：
/// 请求不跟随重定向，而仓库改名或转移后 ID 不变，已经发出去的版本仍查得到。仓库删掉重建后 ID 会变。
const REPOSITORY_ID: u64 = 1_407_079_068;
const API: &str = "https://api.github.com/";
/// Release 的资源文件从这里下载（#279）；测试时和 API 一样换成 ILLUSTASH_UPDATE_BASE。
const DOWNLOADS: &str = "https://github.com/";
/// 本次启动（这个宿主进程）是否已做过自动检查；每个打开的页面都会问，只有一个去检查。
static LAUNCH_CHECKED: AtomicBool = AtomicBool::new(false);
/// App 一直开着时，自动检查每隔这么多秒重复一次。
const DAY: i64 = 24 * 60 * 60;
/// Release 列表很小；更大的内容不会是 GitHub 的回复。
const LIMIT: u64 = 2_000_000;

/// 不含构建元数据的 SemVer 2.0.0 优先级（tools/version.py 写出的是同样的语法）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Version {
    core: [u64; 3],
    pre: Vec<String>,
}
impl Version {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let number = |part: &str| {
            (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()) && (part == "0" || !part.starts_with('0')))
                .then(|| part.parse().ok())
                .flatten()
        };
        let mut parts = core.split('.');
        let core = [number(parts.next()?)?, number(parts.next()?)?, number(parts.next()?)?];
        if parts.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => {
                let ids: Vec<String> = pre.split('.').map(str::to_owned).collect();
                let valid = |id: &String| {
                    !id.is_empty()
                        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                        && (!id.bytes().all(|b| b.is_ascii_digit()) || id == "0" || !id.starts_with('0'))
                };
                if !ids.iter().all(valid) {
                    return None;
                }
                ids
            }
        };
        Some(Self { core, pre })
    }
    pub(crate) fn prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
    /// 不看预发布后缀的 X.Y.Z（数据包目录的最低应用版本按它比，#602：0.3.0 的测试版也能用要求 0.3.0 的包）。
    pub(crate) fn core(&self) -> [u64; 3] {
        self.core
    }
}
impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core.cmp(&other.core).then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                for (a, b) in self.pre.iter().zip(&other.pre) {
                    let order = match (a.parse::<u64>(), b.parse::<u64>()) {
                        (Ok(a), Ok(b)) => a.cmp(&b),
                        (Ok(_), Err(_)) => Ordering::Less,
                        (Err(_), Ok(_)) => Ordering::Greater,
                        (Err(_), Err(_)) => a.cmp(b),
                    };
                    if order != Ordering::Equal {
                        return order;
                    }
                }
                self.pre.len().cmp(&other.pre.len())
            }
        })
    }
}
impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [major, minor, patch] = self.core;
        write!(f, "{major}.{minor}.{patch}")?;
        if self.prerelease() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        Ok(())
    }
}

/// 值得提供给 `current` 版本构建的最新 Release：不要草稿；预发布版（按 GitHub 的标记或 `-beta.1` 这类 tag）
/// 只提供给已经在用预发布版的人。tag 的格式是 `vX.Y.Z`；其他的忽略。
pub(crate) fn newest(releases: &Value, current: &Version) -> Option<Version> {
    releases
        .as_array()?
        .iter()
        .filter(|r| !r.get("draft").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|r| {
            let version = Version::parse(r.get("tag_name")?.as_str()?.strip_prefix('v')?)?;
            let pre = version.prerelease() || r.get("prerelease").and_then(Value::as_bool).unwrap_or(false);
            (!pre || current.prerelease()).then_some(version)
        })
        .max()
}

/// 某个版本的 Release 页面，在这里拼出来而不取自回复：页面只会打开这个网站。
pub(crate) fn page(version: &Version) -> String {
    format!("https://github.com/{REPOSITORY}/releases/tag/v{version}")
}

pub(crate) fn current() -> Version {
    Version::parse(env!("ILLUSTASH_VERSION")).expect("application.json version is SemVer (tools/version.py)")
}

fn support() -> Result<PathBuf> {
    Ok(app_support(&user_home()?))
}
pub(crate) fn read(file: &Path) -> Map<String, Value> {
    fs::read_to_string(file)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| match value {
            Value::Object(fields) => Some(fields),
            _ => None,
        })
        .unwrap_or_default()
}
/// 读-改-写，使用与 libraries.json 相同的锁和改名方式；其他键保留。
pub(crate) fn update(file: &Path, change: impl FnOnce(&mut Map<String, Value>)) -> Result<()> {
    crate::app_libraries::registry_locked(file, || {
        let mut fields = read(file);
        change(&mut fields);
        let text =
            serde_json::to_string_pretty(&Value::Object(fields)).map_err(|e| Error::Invalid(e.to_string()))? + "\n";
        fs::create_dir_all(file.parent().ok_or_else(|| Error::Invalid("无效的偏好设置路径".into()))?)?;
        let tmp = file.with_extension("tmp");
        fs::write(&tmp, text)?;
        fs::rename(&tmp, file).inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })?;
        Ok(())
    })
}

/// 状态存在哪里、现在几点：App 里是真实文件和时钟，测试里是夹具。
struct Place<'a> {
    preferences: PathBuf,
    state: PathBuf,
    now: chrono::DateTime<chrono::FixedOffset>,
    launch: &'a AtomicBool,
    /// 对外壳本机桥的一次请求（方法、路径）：App 里是 `app_desktop`，测试里是替身。
    desktop: &'a dyn Fn(&str, &str) -> Result<Json>,
}
impl Place<'static> {
    fn app() -> Result<Self> {
        let support = support()?;
        Ok(Self {
            preferences: support.join("preferences.json"),
            state: support.join("updates.json"),
            now: chrono::Local::now().fixed_offset(),
            launch: &LAUNCH_CHECKED,
            desktop: &desktop,
        })
    }
}

/// 经本机桥问外壳（#279）。不在桌面 App 里（命令行的 `http`、浏览器宿主）时没有外壳可问。
fn desktop(method: &str, route: &str) -> Result<Json> {
    crate::app_desktop::request(method, route, 5)
}
impl Place<'_> {
    /// `Some(true)` 自动，`Some(false)` 只手动，`None` 从没问过。
    fn auto(&self) -> Option<bool> {
        match read(&self.preferences).get("update_check").and_then(Value::as_str) {
            Some("auto") => Some(true),
            Some("manual") => Some(false),
            _ => None,
        }
    }
    /// 距上次检查（不论哪次启动做的）已过一天；时钟被调回也算，而不是永远挡住检查。
    fn stale(&self) -> bool {
        let state = read(&self.state);
        let at =
            state.get("checked_at").and_then(Value::as_str).and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok());
        at.is_none_or(|at| !(0..DAY).contains(&(self.now - at).num_seconds()))
    }
    /// 已允许，并且本次启动还没检查过或已过了一天。
    fn due(&self) -> bool {
        self.auto() == Some(true) && (!self.launch.load(Atomic::SeqCst) || self.stale())
    }
    /// 领取到期的自动检查：本次启动的第一次（只有一个调用方能拿到），或每天一次的那次。
    fn claim(&self) -> bool {
        self.auto() == Some(true)
            && (self.launch.compare_exchange(false, true, Atomic::SeqCst, Atomic::SeqCst).is_ok() || self.stale())
    }
    fn info(&self, current: &Version) -> Json {
        let state = read(&self.state);
        let latest = state.get("latest").and_then(Value::as_str).and_then(Version::parse);
        let newer = latest.as_ref().filter(|latest| *latest > current);
        let text = |key: &str| state.get(key).and_then(Value::as_str).map(str::to_owned);
        Json::object([
            ("current", current.to_string().into()),
            ("auto", self.auto().into()),
            ("checked_at", text("checked_at").into()),
            ("latest", latest.as_ref().map(Version::to_string).into()),
            ("newer", newer.is_some().into()),
            ("url", newer.map(page).into()),
            ("dismissed", (newer.is_some() && text("dismissed") == latest.as_ref().map(Version::to_string)).into()),
            ("error", text("error").into()),
            ("due", self.due().into()),
            // 检查时请求的地址；它答复 404 时的回退见 `releases`。
            ("address", by_id(API).into()),
            ("install", self.install()),
        ])
    }
    /// 外壳的「下载并重启」状态（#279，desktop/src/updater.rs 的 `status`）；不在桌面 App 里，或外壳没有回答时为 null，
    /// 页面只提供「打开下载页」。
    fn install(&self) -> Json {
        (self.desktop)("GET", "/platform/update/status").unwrap_or(Json::Scalar(Value::Null))
    }
    fn record(&self, result: std::result::Result<Option<Version>, String>) -> Result<()> {
        let at = self.now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        update(&self.state, |fields| {
            fields.insert("checked_at".into(), at.into());
            match result {
                Ok(latest) => {
                    fields.insert("latest".into(), latest.map(|v| v.to_string()).into());
                    fields.remove("error");
                }
                // 检查失败时保留上次已知的 Release。
                Err(message) => {
                    fields.insert("error".into(), message.into());
                }
            }
        })
    }
}

/// API 根地址：GitHub，或由 ILLUSTASH_UPDATE_BASE 指定的本地测试服务器（只限本机上的普通 HTTP）。
fn base() -> std::result::Result<String, String> {
    test_base().unwrap_or_else(|| Ok(API.into()))
}
/// 资源文件的根地址（#279）：GitHub 的网页站，或同一个本地测试服务器。
fn downloads() -> std::result::Result<String, String> {
    test_base().unwrap_or_else(|| Ok(DOWNLOADS.into()))
}
/// 本项目 Release 资源的下载地址（#602 的数据包目录和目录里的包）：照原样，测试时把 `https://github.com/` 换成
/// ILLUSTASH_UPDATE_BASE 指向的本机测试服务。别的地址一律拒绝。
pub(crate) fn asset_url(address: &str) -> std::result::Result<String, String> {
    let rest = address
        .strip_prefix(DOWNLOADS)
        .filter(|rest| rest.starts_with(&format!("{REPOSITORY}/releases/download/")))
        .ok_or_else(|| "数据包的下载地址不在本项目的 Release 里".to_string())?;
    Ok(format!("{}{rest}", downloads()?))
}
fn test_base() -> Option<std::result::Result<String, String>> {
    let base = std::env::var("ILLUSTASH_UPDATE_BASE").ok()?;
    Some(local(base))
}
fn local(base: String) -> std::result::Result<String, String> {
    let u = url::Url::parse(&base).map_err(|_| "合成服务地址无效".to_string())?;
    if u.scheme() != "http"
        || !matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
        || u.port().is_none()
        || u.path() != "/"
        || u.query().is_some()
        || !u.username().is_empty()
        || u.password().is_some()
    {
        return Err("检查更新的测试服务只允许本机 HTTP 端口".into());
    }
    Ok(base)
}

/// Release 列表按仓库 ID 的地址（不带查询串）。仓库改名或转移后，GitHub 把按名字的旧地址 301 到的就是它；
/// 但它没有写进 GitHub 的 REST 文档。
fn by_id(base: &str) -> String {
    format!("{base}repositories/{REPOSITORY_ID}/releases")
}
/// Release 列表按「账号/仓库名」的地址（不带查询串）：写在文档里的入口，仓库改名或转移后在这里查不到。
fn by_name(base: &str) -> String {
    format!("{base}repos/{REPOSITORY}/releases")
}

/// 一次 GET 没拿到 Release 列表。
#[derive(Debug, Clone, PartialEq)]
enum Failure {
    /// GitHub 答复了这个状态码（3xx、4xx、5xx）。
    Status(u16),
    /// 没连上、没读完，或回复不是列表；已经是给页面的一句话。
    Other(&'static str),
}
impl Failure {
    /// 给页面的一句简短说明。
    fn message(&self) -> String {
        match self {
            Self::Status(403 | 429) => "GitHub 暂时拒绝了请求，请稍后再试".into(),
            // 不跟随的重定向：和改动前一样，算作回复不是版本列表。
            Self::Status(300..=399) => "GitHub 的回复不是版本列表".into(),
            Self::Status(code) => format!("GitHub 返回了错误（HTTP {code}）"),
            Self::Other(message) => (*message).into(),
        }
    }
}

/// 对 Release 列表的一次 GET。`agent` 不跟随重定向，3xx 的回复原样回到这里，按状态码记为失败，不读回复里的
/// 跳转地址。
fn get(agent: &ureq::Agent, address: &str) -> std::result::Result<Value, Failure> {
    const NOT_A_LIST: Failure = Failure::Other("GitHub 的回复不是版本列表");
    let reply = agent
        .get(address)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", "Illustash-update-check")
        .call();
    let reply = match reply {
        Ok(reply) if (300..400).contains(&reply.status()) => return Err(Failure::Status(reply.status())),
        Ok(reply) => reply,
        Err(ureq::Error::Status(code, _)) => return Err(Failure::Status(code)),
        Err(ureq::Error::Transport(_)) => return Err(Failure::Other("连不上 GitHub，请检查网络后再试")),
    };
    let mut bytes = Vec::new();
    reply
        .into_reader()
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Failure::Other("读取 GitHub 的回复失败"))?;
    if bytes.len() as u64 > LIMIT {
        return Err(NOT_A_LIST);
    }
    let releases: Value = serde_json::from_slice(&bytes).map_err(|_| NOT_A_LIST)?;
    if !releases.is_array() {
        return Err(NOT_A_LIST);
    }
    Ok(releases)
}

/// Release 列表：先按仓库 ID 查；GitHub 明确答复这个地址查不到时（3xx 或 4xx，限流的 403、429 除外），再按
/// 「账号/仓库名」查一次，以这一次为准。按 ID 的地址没有文档承诺，它哪天不再提供列表（404、410、跳到别处……），
/// 只要仓库还叫原来的名字就仍查得到。换地址不是跟随重定向：回复里的跳转地址从不使用。其他失败不换地址：
/// 连不上、被限流或 GitHub 自己出错（5xx）时，换个地址结果也一样。两个地址都在 `base`（api.github.com）下面。
fn releases(
    base: &str,
    get: impl Fn(&str) -> std::result::Result<Value, Failure>,
) -> std::result::Result<Value, Failure> {
    let ask = |address: String| get(&format!("{address}?per_page=30"));
    match ask(by_id(base)) {
        Err(Failure::Status(code)) if (300..500).contains(&code) && !matches!(code, 403 | 429) => ask(by_name(base)),
        reply => reply,
    }
}

/// 向 GitHub 查一次 Release 列表（至多两次 GET，见 `releases`）。失败变成给页面的一句简短说明，绝不弹错误对话框。
fn fetch(current: &Version) -> std::result::Result<Option<Version>, String> {
    let base = base()?;
    let agent = ureq::AgentBuilder::new().redirects(0).timeout(Duration::from_secs(15)).build();
    let releases = releases(&base, |address| get(&agent, address)).map_err(|failure| failure.message())?;
    Ok(newest(&releases, current))
}

/// 外壳下载 `version` 的更新包时用的两个地址（#279）：`manifest` 是这个版本的 Release 里的 latest.json，
/// latest.json 里给本平台的包必须在 `prefix` 下面，也就是同一个 Release 里。地址在这里按版本号拼出，不取自任何回复。
#[derive(Debug, PartialEq, Eq)]
pub struct Package {
    pub manifest: String,
    pub prefix: String,
}

/// 外壳收到的版本号要能安装才给地址：合法的 SemVer、比本构建新、通道规则与 `newest` 相同（预发布版只给预发布版本的
/// 安装）。页面只能请求检查查到的那个版本（`post`），这里再核对一遍，不信任经过本机桥的参数。
pub fn package(version: &str) -> std::result::Result<Package, String> {
    let version = installable(version, &current())?;
    Ok(package_at(&downloads()?, &version))
}
fn installable(version: &str, current: &Version) -> std::result::Result<Version, String> {
    Version::parse(version)
        .filter(|v| v > current && (!v.prerelease() || current.prerelease()))
        .ok_or_else(|| "这个版本不能安装".to_owned())
}
fn package_at(base: &str, version: &Version) -> Package {
    let prefix = format!("{base}{REPOSITORY}/releases/download/v{version}/");
    Package { manifest: format!("{prefix}latest.json"), prefix }
}

/// GET /api/updates.
pub(crate) fn info() -> Result<Json> {
    Ok(Place::app()?.info(&current()))
}

/// POST /api/updates：
/// - `{"action": "auto", "allow": true|false}` 记下选择（首次引导的提问或设置里的开关）；
/// - `{"action": "check"}` 立即检查（用户的点击）；
/// - `{"action": "check", "auto": true}` 只在已允许且本次启动还没检查过时检查，否则只回复；
/// - `{"action": "dismiss"}` 不再提示已知的最新 Release（有更新的版本时会再次提示）；
/// - `{"action": "download"}` 让外壳下载并校验检查查到的那个新版本（#279），`{"action": "cancel"}` 取消下载，
///   `{"action": "install"}` 安装下好的版本并重启。外壳不在时这三个返回错误。
///
/// 真的去查了 GitHub 时（手动检查，或领到了自动检查），顺带读一次数据包目录（#602，`app_packs::refresh`）：
/// 读到的结果记在 updates.json 的 `packs_checked_at`、`packs_error` 里，不进这里的回复。
pub(crate) fn post(resources: &Path, body: &Json) -> Result<Json> {
    let place = Place::app()?;
    let mut checked = false;
    let reply = post_at(&place, body, &current(), |current| {
        checked = true;
        fetch(current)
    })?;
    if checked {
        if let Some(result) = crate::app_packs::refresh(resources) {
            record_packs(&place, result)?;
        }
    }
    Ok(reply)
}

/// 首次准备时顺带读一次包目录（2026-11-18 用户定，#630，`app_models::post`）：和检查更新同一套读取、签名校验、缓存，
/// 结果同样记在 updates.json。没有公钥时什么都不做；失败只记下原因。
pub(crate) fn refresh_packs(resources: &Path) {
    let Some(result) = crate::app_packs::refresh(resources) else { return };
    if let Err(error) = Place::app().and_then(|place| record_packs(&place, result)) {
        eprintln!("没能记下读取角色资料包目录的结果：{error}");
    }
}

fn record_packs(place: &Place, result: crate::app_packs::Refresh) -> Result<()> {
    let at = place.now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    update(&place.state, |fields| {
        fields.insert("packs_checked_at".into(), at.into());
        match result {
            crate::app_packs::Refresh::ManifestFailed(message) => {
                fields.insert("packs_error".into(), message.into());
            }
            crate::app_packs::Refresh::Adopted { character_error } => {
                fields.remove("packs_error");
                if let Some(message) = character_error {
                    fields.insert("character_catalog_error".into(), message.into());
                } else {
                    fields.remove("character_catalog_error");
                }
            }
        }
    })
}

pub(crate) struct PacksState {
    pub checked_at: Option<String>,
    pub error: Option<String>,
    pub character_error: Option<String>,
}

/// 最近一次检查签名清单和下载词表的独立结果。
pub(crate) fn packs_state() -> Result<PacksState> {
    let state = read(&Place::app()?.state);
    let text = |key: &str| state.get(key).and_then(Value::as_str).map(str::to_owned);
    Ok(PacksState {
        checked_at: text("packs_checked_at"),
        error: text("packs_error"),
        character_error: text("character_catalog_error"),
    })
}

fn post_at(
    place: &Place,
    body: &Json,
    current: &Version,
    fetch: impl FnOnce(&Version) -> std::result::Result<Option<Version>, String>,
) -> Result<Json> {
    let Json::Object(fields) = body else { return Err(Error::Invalid("未知操作".into())) };
    let get = |key: &str| fields.iter().find(|(k, _)| k == key).map(|(_, v)| v);
    let flag = |key: &str| match get(key) {
        None => Ok(None),
        Some(Json::Scalar(Value::Bool(value))) => Ok(Some(*value)),
        Some(_) => Err(Error::Invalid("参数取值不正确".into())),
    };
    let allowed: &[&str] = match get("action").map(Json::text) {
        Some("auto") => &["action", "allow"],
        Some("check") => &["action", "auto"],
        Some("dismiss" | "download" | "cancel" | "install") => &["action"],
        _ => return Err(Error::Invalid("未知操作".into())),
    };
    if fields.iter().any(|(key, _)| !allowed.contains(&key.as_str())) {
        return Err(Error::Invalid("未知参数".into()));
    }
    match get("action").map(Json::text) {
        Some("auto") => {
            let allow = flag("allow")?.ok_or_else(|| Error::Invalid("参数取值不正确".into()))?;
            update(&place.preferences, |fields| {
                fields.insert("update_check".into(), if allow { "auto" } else { "manual" }.into());
            })?;
        }
        Some("check") => {
            if flag("auto")? != Some(true) || place.claim() {
                place.record(fetch(current))?;
            }
        }
        Some("download") => {
            // 只下载检查查到、比本构建新的那个版本；外壳收到后按 `package` 再核对一遍。
            let state = read(&place.state);
            let latest = state.get("latest").and_then(Value::as_str).and_then(Version::parse);
            let Some(latest) = latest.filter(|latest| latest > current) else {
                return Err(Error::invalid("没有可以下载的新版本"));
            };
            (place.desktop)("POST", &format!("/platform/update/download?version={latest}"))?;
        }
        Some("cancel") => {
            (place.desktop)("POST", "/platform/update/cancel")?;
        }
        Some("install") => {
            (place.desktop)("POST", "/platform/update/install")?;
        }
        _ => {
            let latest = read(&place.state).get("latest").cloned().unwrap_or(Value::Null);
            update(&place.state, |fields| {
                fields.insert("dismissed".into(), latest);
            })?;
        }
    }
    Ok(place.info(current))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::{Cell, RefCell};
    use std::sync::atomic::{AtomicU64, Ordering as Atomic};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|| panic!("{text}"))
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "illustash-updates-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Atomic::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        /// `now` 时刻的 App，处在自动检查状态为 `launch` 的那次启动里。
        fn at<'a>(&self, now: &str, launch: &'a AtomicBool) -> Place<'a> {
            self.with(now, launch, &no_desktop)
        }
        /// 同上，外壳由 `desktop` 回答。
        fn with<'a>(
            &self,
            now: &str,
            launch: &'a AtomicBool,
            desktop: &'a dyn Fn(&str, &str) -> Result<Json>,
        ) -> Place<'a> {
            Place {
                preferences: self.0.join("preferences.json"),
                state: self.0.join("updates.json"),
                now: chrono::DateTime::parse_from_rfc3339(now).unwrap(),
                launch,
                desktop,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    /// 不在桌面 App 里：没有外壳可问。
    fn no_desktop(_: &str, _: &str) -> Result<Json> {
        Err(Error::invalid("桌面平台服务不可用"))
    }
    fn body(value: Value) -> Json {
        crate::http_json::parse_request(&value.to_string()).unwrap()
    }
    fn field(info: &Json, key: &str) -> Value {
        serde_json::from_str(&info.get(key).unwrap().dump()).unwrap()
    }

    #[test]
    fn versions_follow_semver_precedence() {
        let order = [
            "0.1.0",
            "0.1.1",
            "0.2.0-alpha",
            "0.2.0-alpha.1",
            "0.2.0-alpha.beta",
            "0.2.0-beta",
            "0.2.0-beta.2",
            "0.2.0-beta.11",
            "0.2.0-rc.1",
            "0.2.0",
            "0.10.0",
            "1.0.0",
        ];
        for pair in order.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        }
        for text in order {
            assert_eq!(v(text).to_string(), text);
        }
        for bad in ["", "0.1", "0.1.0.0", "v0.1.0", "01.0.0", "0.1.0-", "0.1.0-01", "0.1.0-a..b", "0.1.0+b", "0.1.x"] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        assert!(current().to_string() == env!("ILLUSTASH_VERSION"));
    }

    #[test]
    fn newest_skips_drafts_odd_tags_and_prereleases_for_stable_builds() {
        let releases = json!([
            {"tag_name": "v0.3.0", "draft": true, "prerelease": false},
            {"tag_name": "v0.2.1-beta.1", "draft": false, "prerelease": true},
            {"tag_name": "v0.2.2", "draft": false, "prerelease": true},
            {"tag_name": "v0.2.0", "draft": false, "prerelease": false},
            {"tag_name": "release-9", "draft": false, "prerelease": false},
            {"tag_name": "v9.0", "draft": false, "prerelease": false},
            {"name": "no tag"},
        ]);
        assert_eq!(newest(&releases, &v("0.1.0")), Some(v("0.2.0")));
        assert_eq!(newest(&releases, &v("0.2.0-beta.1")), Some(v("0.2.2")));
        assert_eq!(newest(&json!([]), &v("0.1.0")), None);
        assert_eq!(newest(&json!({"message": "Not Found"}), &v("0.1.0")), None);
        assert_eq!(page(&v("0.2.0-beta.1")), "https://github.com/shio-vinyl/illustash/releases/tag/v0.2.0-beta.1");
    }

    #[test]
    fn releases_are_asked_by_repository_id_and_by_name_only_if_that_address_is_refused() {
        const BY_ID: &str = "https://api.github.com/repositories/1407079068/releases?per_page=30";
        const BY_NAME: &str = "https://api.github.com/repos/shio-vinyl/illustash/releases?per_page=30";
        // 按顺序答复每次请求，记下问过的地址；多问一次或少问一次都算错。
        let ask = |replies: Vec<std::result::Result<Value, Failure>>| {
            let (seen, replies) = (RefCell::new(Vec::new()), RefCell::new(replies.into_iter()));
            let reply = releases(API, |address| {
                seen.borrow_mut().push(address.to_owned());
                replies.borrow_mut().next().expect("多发了一次请求")
            });
            assert_eq!(replies.borrow_mut().next(), None, "准备的答复没有用完");
            (reply, seen.into_inner())
        };
        let list = json!([{"tag_name": "v0.2.0"}]);
        let (only_id, both) = (vec![BY_ID.to_owned()], vec![BY_ID.to_owned(), BY_NAME.to_owned()]);
        // 平时只有一次请求，按仓库 ID。
        assert_eq!(ask(vec![Ok(list.clone())]), (Ok(list.clone()), only_id.clone()));
        // 按 ID 的地址答复 3xx 或 4xx（限流除外）：再按「账号/仓库名」查一次，以这一次为准，不论成败；没有第三次。
        let missing = Failure::Status(404);
        for refused in [301, 302, 308, 400, 401, 404, 410, 451] {
            let refused = Failure::Status(refused);
            assert_eq!(
                ask(vec![Err(refused.clone()), Ok(list.clone())]),
                (Ok(list.clone()), both.clone()),
                "{refused:?}"
            );
        }
        for second in
            [missing.clone(), Failure::Status(301), Failure::Status(403), Failure::Other("GitHub 的回复不是版本列表")]
        {
            assert_eq!(ask(vec![Err(missing.clone()), Err(second.clone())]), (Err(second), both.clone()));
        }
        // 其他失败不换地址：限流、GitHub 出错、连不上、回复不是列表。
        for failure in [
            Failure::Status(403),
            Failure::Status(429),
            Failure::Status(500),
            Failure::Status(503),
            Failure::Other("连不上 GitHub，请检查网络后再试"),
            Failure::Other("GitHub 的回复不是版本列表"),
        ] {
            assert_eq!(ask(vec![Err(failure.clone())]), (Err(failure), only_id.clone()));
        }
        // 给页面的说明和按名字查的时候一样。
        assert_eq!(missing.message(), "GitHub 返回了错误（HTTP 404）");
        assert_eq!(Failure::Status(500).message(), "GitHub 返回了错误（HTTP 500）");
        assert_eq!(Failure::Status(301).message(), "GitHub 的回复不是版本列表");
        for limited in [403, 429] {
            assert_eq!(Failure::Status(limited).message(), "GitHub 暂时拒绝了请求，请稍后再试");
        }
        assert_eq!(Failure::Other("读取 GitHub 的回复失败").message(), "读取 GitHub 的回复失败");
    }

    #[test]
    fn nothing_is_fetched_until_allowed_then_at_each_launch_and_daily_while_open() {
        let fixture = Fixture::new();
        let launch = AtomicBool::new(false);
        let calls = Cell::new(0);
        let fetch = |_: &Version| {
            calls.set(calls.get() + 1);
            Ok(Some(v("0.2.0")))
        };
        let auto = || body(json!({"action": "check", "auto": true}));
        let morning = fixture.at("2026-10-18T09:00:00+08:00", &launch);
        let info = morning.info(&v("0.1.0"));
        assert_eq!(field(&info, "auto"), Value::Null);
        assert_eq!(field(&info, "due"), json!(false));
        // 页面拿到的是检查时请求的地址：按仓库 ID 的那个（#358）。
        assert_eq!(field(&info, "address"), json!("https://api.github.com/repositories/1407079068/releases"));
        // 从没问过，然后被拒绝：自动检查不联网就回复，也不消耗任何东西。
        post_at(&morning, &auto(), &v("0.1.0"), fetch).unwrap();
        post_at(&morning, &body(json!({"action": "auto", "allow": false})), &v("0.1.0"), fetch).unwrap();
        post_at(&morning, &auto(), &v("0.1.0"), fetch).unwrap();
        assert_eq!(calls.get(), 0);
        assert!(!fixture.0.join("updates.json").exists());
        assert!(!launch.load(Atomic::SeqCst));
        // 已允许：本次启动只检查一次，不管有多少页面来问。
        let info = post_at(&morning, &body(json!({"action": "auto", "allow": true})), &v("0.1.0"), fetch).unwrap();
        assert_eq!((field(&info, "auto"), field(&info, "due")), (json!(true), json!(true)));
        let info = post_at(&morning, &auto(), &v("0.1.0"), fetch).unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(field(&info, "latest"), json!("0.2.0"));
        assert_eq!(field(&info, "newer"), json!(true));
        assert_eq!(field(&info, "url"), json!("https://github.com/shio-vinyl/illustash/releases/tag/v0.2.0"));
        assert_eq!(field(&info, "due"), json!(false));
        let evening = fixture.at("2026-10-18T23:00:00+08:00", &launch);
        post_at(&evening, &auto(), &v("0.1.0"), fetch).unwrap();
        assert_eq!(calls.get(), 1);
        // 手动检查就是用户的点击：总会检查。
        post_at(&evening, &body(json!({"action": "check"})), &v("0.1.0"), fetch).unwrap();
        assert_eq!(calls.get(), 2);
        // 下一次启动会再检查，即使只过了一小时。
        let relaunch = AtomicBool::new(false);
        let next = fixture.at("2026-10-19T00:00:00+08:00", &relaunch);
        assert_eq!(field(&next.info(&v("0.1.0")), "due"), json!(true));
        post_at(&next, &auto(), &v("0.1.0"), fetch).unwrap();
        post_at(&next, &auto(), &v("0.1.0"), fetch).unwrap();
        assert_eq!(calls.get(), 3);
        // 一直开着：距上次检查过了一天时检查。
        let later = fixture.at("2026-10-19T23:59:59+08:00", &relaunch);
        assert_eq!(field(&later.info(&v("0.1.0")), "due"), json!(false));
        let day = fixture.at("2026-10-20T00:00:00+08:00", &relaunch);
        assert_eq!(field(&day.info(&v("0.1.0")), "due"), json!(true));
        post_at(&day, &auto(), &v("0.1.0"), fetch).unwrap();
        post_at(&day, &auto(), &v("0.1.0"), fetch).unwrap();
        assert_eq!(calls.get(), 4);
        // 时钟被调回算作到期，而不是永远挡住检查。
        assert_eq!(field(&fixture.at("2026-10-01T00:00:00+08:00", &relaunch).info(&v("0.1.0")), "due"), json!(true));
        // 又关掉了：不再检查，不论是新启动还是过了一天。
        post_at(&day, &body(json!({"action": "auto", "allow": false})), &v("0.1.0"), fetch).unwrap();
        let fresh = AtomicBool::new(false);
        post_at(&fixture.at("2026-10-25T00:00:00+08:00", &fresh), &auto(), &v("0.1.0"), fetch).unwrap();
        assert_eq!(calls.get(), 4);
        // 选择存在 preferences.json 里，和语言放在一起。
        let preferences: Value =
            serde_json::from_str(&fs::read_to_string(fixture.0.join("preferences.json")).unwrap()).unwrap();
        assert_eq!(preferences, json!({"update_check": "manual"}));
    }

    #[test]
    fn other_preferences_are_kept() {
        let fixture = Fixture::new();
        let launch = AtomicBool::new(false);
        fs::write(fixture.0.join("preferences.json"), r#"{"language": "en", "future": 1}"#).unwrap();
        let place = fixture.at("2026-10-18T09:00:00+08:00", &launch);
        post_at(&place, &body(json!({"action": "auto", "allow": true})), &v("0.1.0"), |_| unreachable!()).unwrap();
        let preferences: Value = serde_json::from_str(&fs::read_to_string(&place.preferences).unwrap()).unwrap();
        assert_eq!(preferences, json!({"language": "en", "future": 1, "update_check": "auto"}));
    }

    #[test]
    fn failures_keep_the_last_release_and_dismissing_lasts_until_a_newer_one() {
        let fixture = Fixture::new();
        let launch = AtomicBool::new(false);
        let place = fixture.at("2026-10-18T09:00:00+08:00", &launch);
        let check = body(json!({"action": "check"}));
        post_at(&place, &check, &v("0.1.0"), |_| Ok(Some(v("0.2.0")))).unwrap();
        let info = post_at(&place, &check, &v("0.1.0"), |_| Err("连不上 GitHub，请检查网络后再试".into())).unwrap();
        assert_eq!(field(&info, "latest"), json!("0.2.0"));
        assert_eq!(field(&info, "error"), json!("连不上 GitHub，请检查网络后再试"));
        let info = post_at(&place, &body(json!({"action": "dismiss"})), &v("0.1.0"), |_| unreachable!()).unwrap();
        assert_eq!((field(&info, "newer"), field(&info, "dismissed")), (json!(true), json!(true)));
        let info = post_at(&place, &check, &v("0.1.0"), |_| Ok(Some(v("0.2.1")))).unwrap();
        assert_eq!((field(&info, "error"), field(&info, "dismissed")), (Value::Null, json!(false)));
        // 安装之后，同一个 Release 就不再算更新。
        let info = place.info(&v("0.2.1"));
        assert_eq!((field(&info, "newer"), field(&info, "url")), (json!(false), Value::Null));
        // 一个 Release 都没有是成功的回复，不是失败。
        let info = post_at(&place, &check, &v("0.1.0"), |_| Ok(None)).unwrap();
        assert_eq!((field(&info, "latest"), field(&info, "newer")), (Value::Null, json!(false)));
    }

    #[test]
    fn requests_are_checked_strictly() {
        let fixture = Fixture::new();
        let launch = AtomicBool::new(false);
        let place = fixture.at("2026-10-18T09:00:00+08:00", &launch);
        for bad in [
            json!({}),
            json!({"action": "upgrade"}),
            json!({"action": "install", "version": "0.2.0"}),
            json!({"action": "download", "url": "https://example.com/"}),
            json!({"action": "auto"}),
            json!({"action": "auto", "allow": "yes"}),
            json!({"action": "check", "auto": 1}),
            json!({"action": "check", "url": "https://example.com/"}),
            json!({"action": "dismiss", "version": "0.2.0"}),
            json!(["check"]),
        ] {
            let Ok(request) = crate::http_json::parse_request(&bad.to_string()) else { continue };
            let result = post_at(&place, &request, &v("0.1.0"), |_| unreachable!());
            assert!(matches!(result, Err(Error::Invalid(_))), "{bad}");
        }
        assert!(!place.preferences.exists() && !place.state.exists());
    }

    #[test]
    fn packages_are_looked_up_in_the_release_of_that_version() {
        let package = package_at(DOWNLOADS, &v("0.2.0-beta.1"));
        assert_eq!(
            package,
            Package {
                manifest: "https://github.com/shio-vinyl/illustash/releases/download/v0.2.0-beta.1/latest.json".into(),
                prefix: "https://github.com/shio-vinyl/illustash/releases/download/v0.2.0-beta.1/".into(),
            }
        );
        assert_eq!(
            package_at("http://127.0.0.1:8807/", &v("0.2.0")).manifest,
            "http://127.0.0.1:8807/shio-vinyl/illustash/releases/download/v0.2.0/latest.json"
        );
        // 只装更新的版本；正式版不装预发布版，和检查更新的通道规则一样。
        assert_eq!(installable("0.2.0", &v("0.1.0")), Ok(v("0.2.0")));
        assert_eq!(installable("0.2.0-beta.1", &v("0.2.0-alpha")), Ok(v("0.2.0-beta.1")));
        for (bad, current) in [
            ("0.1.0", "0.1.0"),
            ("0.0.9", "0.1.0"),
            ("0.2.0-beta.1", "0.1.0"),
            ("v0.2.0", "0.1.0"),
            ("0.2.0/../../evil", "0.1.0"),
            ("", "0.1.0"),
        ] {
            assert!(installable(bad, &v(current)).is_err(), "{bad} over {current}");
        }
        // 测试服务器只能是本机的普通 HTTP 端口。
        assert!(local("http://127.0.0.1:8807/".into()).is_ok());
        for bad in ["https://127.0.0.1:8807/", "http://example.com:80/", "http://127.0.0.1/", "http://127.0.0.1:1/x/"] {
            assert!(local(bad.into()).is_err(), "{bad}");
        }
    }

    #[test]
    fn download_install_and_cancel_go_to_the_shell_and_only_for_the_newer_release() {
        let fixture = Fixture::new();
        let launch = AtomicBool::new(false);
        let asked = RefCell::new(Vec::<(String, String)>::new());
        let desktop = |method: &str, route: &str| {
            asked.borrow_mut().push((method.into(), route.into()));
            Ok(crate::http_json::parse_request(r#"{"available": true, "stage": "idle"}"#).unwrap())
        };
        let place = fixture.with("2026-11-14T09:00:00+08:00", &launch, &desktop);
        // 还没检查过：没有可下载的版本，也不去问外壳要不要下载。
        let result = post_at(&place, &body(json!({"action": "download"})), &v("0.1.0"), |_| unreachable!());
        assert!(matches!(result, Err(Error::Invalid(_))));
        assert!(!asked.borrow().iter().any(|(method, _)| method == "POST"));
        post_at(&place, &body(json!({"action": "check"})), &v("0.1.0"), |_| Ok(Some(v("0.2.0")))).unwrap();
        let info = post_at(&place, &body(json!({"action": "download"})), &v("0.1.0"), |_| unreachable!()).unwrap();
        // 状态里带着外壳的回答。
        assert_eq!(field(&info, "install"), json!({"available": true, "stage": "idle"}));
        post_at(&place, &body(json!({"action": "cancel"})), &v("0.1.0"), |_| unreachable!()).unwrap();
        post_at(&place, &body(json!({"action": "install"})), &v("0.1.0"), |_| unreachable!()).unwrap();
        let posts: Vec<_> = asked.borrow().iter().filter(|(m, _)| m == "POST").map(|(_, r)| r.clone()).collect();
        assert_eq!(
            posts,
            ["/platform/update/download?version=0.2.0", "/platform/update/cancel", "/platform/update/install"]
        );
        // 已经装上了这个版本：不再下载。
        let result = post_at(&place, &body(json!({"action": "download"})), &v("0.2.0"), |_| unreachable!());
        assert!(matches!(result, Err(Error::Invalid(_))));
        // 不在桌面 App 里：状态里没有 install，三个操作都报错。
        let alone = fixture.at("2026-11-14T09:00:00+08:00", &launch);
        assert_eq!(field(&alone.info(&v("0.1.0")), "install"), Value::Null);
        for action in ["download", "cancel", "install"] {
            let result = post_at(&alone, &body(json!({"action": action})), &v("0.1.0"), |_| unreachable!());
            assert!(matches!(result, Err(Error::Invalid(_))), "{action}");
        }
    }
}
