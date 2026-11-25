//! 只取文本的名称来源，以及 Danbooru 角色的那一张小预览图（#293）。联网时从不持有数据库事务。
use crate::error::invalid;
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

fn network<T>(s: &str) -> Result<T> {
    Err(Error::Network(s.into()))
}
pub(crate) fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) || crate::platform_process::stopping() {
        invalid("名称任务已取消")
    } else {
        Ok(())
    }
}
pub(crate) fn db(path: &std::path::Path) -> Result<Connection> {
    let c = crate::sqlite::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    c.busy_timeout(crate::app_library::TASK_WRITE_WAIT)?;
    Ok(c)
}
#[derive(Default)]
struct Host {
    next: Option<Instant>,
    failures: u32,
}
// 在同步的选择器和后台 worker 之间共享，跨会话轮换也一样。
static HOSTS: OnceLock<Mutex<HashMap<String, Arc<Mutex<Host>>>>> = OnceLock::new();
/// 未经授权发出名称文本查询时的错误；前端按它判断。
pub(crate) const UNAUTHORIZED: &str = "显示名联网尚未授权";
pub(crate) struct Source {
    pub path: PathBuf,
    pub cancel: Arc<AtomicBool>,
    /// 本进程已获授权（`--names-network`，或索引任务的索引联网授权）。
    pub authorized: bool,
    /// 图库对名称文本查询的长期同意，每次请求时都读取，这样撤销后，正在运行的任务的下一个请求就会停下。
    grant: Option<Arc<AtomicBool>>,
    pub force: bool,
    seen: HashSet<String>,
    pub db_base: String,
    bg_base: String,
}
fn base(name: &str, default: &str) -> Result<String> {
    let value = std::env::var(name).unwrap_or_else(|_| default.into());
    if value != default {
        let u = url::Url::parse(&value).map_err(|_| Error::Invalid("名称合成地址不正确".into()))?;
        if u.scheme() != "http"
            || !matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
            || u.port().is_none()
            || !u.username().is_empty()
            || u.password().is_some()
            || u.query().is_some()
            || u.fragment().is_some()
        {
            return invalid("名称替换地址须为本机合成服务");
        }
    }
    Ok(value.trim_end_matches('/').into())
}
fn encoded(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else if b == b' ' {
                "+".into()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
pub(crate) fn url(base: &str, path: &str, fields: &[(&str, String)]) -> String {
    let query = fields.iter().map(|(k, v)| format!("{}={}", encoded(k), encoded(v))).collect::<Vec<_>>().join("&");
    format!("{base}{path}?{query}")
}
impl Source {
    pub fn new(path: PathBuf, cancel: Arc<AtomicBool>, authorized: bool, force: bool) -> Result<Self> {
        Ok(Self {
            path,
            cancel,
            authorized,
            grant: None,
            force,
            seen: HashSet::new(),
            db_base: base("ILLUSTASH_COPYRIGHT_BASE", "https://danbooru.donmai.us")?,
            bg_base: base("ILLUSTASH_BANGUMI_BASE", "https://api.bgm.tv/v0")?,
        })
    }
    pub fn cached(&mut self, url: &str, body: Option<Value>) -> Result<Value> {
        check(&self.cancel)?;
        let key = crate::character_json::dump(&json!([url, body]), false);
        let old: Option<String> =
            db(&self.path)?.query_row("SELECT value FROM name_cache WHERE key=?", [&key], |r| r.get(0)).optional()?;
        if let Some(raw) = old.filter(|_| !self.force || self.seen.contains(&key)) {
            let value = serde_json::from_str(&raw).map_err(|_| Error::Invalid("名称缓存不正确".into()))?;
            validate(&value, url)?;
            return Ok(value);
        }
        let (value, serialized) = self.fetch_serialized(url, body.as_ref(), None)?;
        validate(&value, url)?;
        check(&self.cancel)?;
        db(&self.path)?.execute("INSERT OR REPLACE INTO name_cache VALUES (?,?)", [&key, &serialized])?;
        self.seen.insert(key);
        Ok(value)
    }
    /// 测试把两个提供方都指向同一个回环服务，不碰进程环境。
    #[cfg(test)]
    pub(crate) fn with_bases(mut self, db: &str, bg: &str) -> Self {
        self.db_base = db.trim_end_matches('/').into();
        self.bg_base = bg.trim_end_matches('/').into();
        self
    }
    pub fn granted(mut self, grant: Arc<AtomicBool>) -> Self {
        self.grant = Some(grant);
        self
    }
    fn allowed(&self) -> bool {
        self.authorized || self.grant.as_ref().is_some_and(|g| g.load(std::sync::atomic::Ordering::SeqCst))
    }
    fn check_request(&self, local: bool) -> Result<()> {
        check(&self.cancel)?;
        if !local && !self.allowed() {
            return network(UNAUTHORIZED);
        }
        Ok(())
    }
    fn wait(&self, until: Instant, local: bool) -> Result<()> {
        while Instant::now() < until {
            self.check_request(local)?;
            std::thread::sleep(Duration::from_millis(20).min(until.saturating_duration_since(Instant::now())));
        }
        self.check_request(local)
    }
    pub fn fetch(&self, address: &str, body: Option<&Value>, key: Option<&str>) -> Result<Value> {
        self.fetch_serialized(address, body, key).map(|(value, _)| value)
    }
    /// 来自 Danbooru CDN 的一张小图（帖子的预览图，#293），与每次名称查询需要同样的同意。最多 `limit` 字节；
    /// 不跟随重定向，不访问其他主机（测试里是本机上的合成服务）。
    pub fn picture(&self, address: &str, limit: u64) -> Result<Vec<u8>> {
        let u = url::Url::parse(address).map_err(|_| Error::Network("预览图地址不正确".into()))?;
        let local = matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        self.check_request(local)?;
        if !u.username().is_empty()
            || u.password().is_some()
            || (local && u.scheme() != "http")
            || (!local && (u.scheme() != "https" || u.host_str() != Some("cdn.donmai.us")))
        {
            return network("预览图不在 Danbooru 来源范围");
        }
        let response = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(15))
            .redirects(0)
            .build()
            .get(address)
            .set("User-Agent", "illustash/0.1 (https://github.com/shio-vinyl/illustash; local library display names)")
            .call()
            .map_err(|_| Error::Network("预览图下载失败".into()))?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Network("预览图下载失败".into()))?;
        if bytes.len() as u64 > limit {
            return network("预览图过大");
        }
        check(&self.cancel)?;
        Ok(bytes)
    }
    fn fetch_serialized(&self, address: &str, body: Option<&Value>, key: Option<&str>) -> Result<(Value, String)> {
        let u = url::Url::parse(address).map_err(|_| Error::Invalid("名称接口地址不正确".into()))?;
        let local = matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        self.check_request(local)?;
        if u.scheme() != "https" && !(local && u.scheme() == "http") {
            return network("名称接口须使用 HTTPS");
        }
        let host_key = format!("{}:{}", u.host_str().unwrap_or(""), u.port_or_known_default().unwrap_or(0));
        let host = HOSTS
            .get_or_init(Default::default)
            .lock()
            .expect("app_name_sources mutex poisoned")
            .entry(host_key)
            .or_default()
            .clone();
        let interval = if local {
            0.0
        } else {
            match u.host_str() {
                Some("api.bgm.tv") => 0.6,
                Some("danbooru.donmai.us") => 0.35,
                _ => 1.0,
            }
        };
        for attempt in 0..3 {
            self.check_request(local)?;
            // 每个主机同时只有一个进行中的请求，在两个请求的上限之内。
            let mut h = loop {
                if let Ok(h) = host.try_lock() {
                    break h;
                }
                self.wait(Instant::now() + Duration::from_millis(20), local)?;
            };
            if let Some(next) = h.next {
                self.wait(next, local)?;
            }
            self.check_request(local)?;
            h.next = Some(Instant::now() + Duration::from_secs_f64(interval));
            let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(25)).redirects(0).build();
            let mut req = agent
                .request(if body.is_some() { "POST" } else { "GET" }, address)
                .set("Accept", "application/json")
                .set(
                    "User-Agent",
                    "illustash/0.1 (https://github.com/shio-vinyl/illustash; local library display names)",
                );
            if let Some(key) = key {
                req = req.set("Authorization", &format!("Bearer {key}"));
            }
            self.check_request(local)?;
            let response = if let Some(body) = body { req.send_json(body) } else { req.call() };
            let mut retry = 0.0;
            let (value, transient, label) = match response {
                Ok(r) => {
                    let mut bytes = Vec::new();
                    let read = r.into_reader().take(2_000_001).read_to_end(&mut bytes);
                    if read.is_err() {
                        (None, true, "名称响应读取失败".into())
                    } else {
                        if bytes.len() > 2_000_000 {
                            return network("响应超过 2 MB 限制");
                        }
                        let text =
                            std::str::from_utf8(&bytes).map_err(|_| Error::Network("名称响应格式不正确".into()))?;
                        let serialized = crate::http_json::parse_request(text)
                            .map_err(|_| Error::Network("名称响应格式不正确".into()))?
                            .dump();
                        let value = serde_json::from_str(&serialized)
                            .map_err(|_| Error::Network("名称响应格式不正确".into()))?;
                        (Some((value, serialized)), false, String::new())
                    }
                }
                Err(ureq::Error::Status(404, _)) if key.is_none() => {
                    (Some((Value::Null, "null".into())), false, String::new())
                }
                Err(ureq::Error::Status(code, r)) => {
                    retry = r
                        .header("Retry-After")
                        .and_then(|s| {
                            s.parse::<f64>().ok().or_else(|| {
                                httpdate::parse_http_date(s).ok().map(|d| {
                                    d.duration_since(std::time::SystemTime::now()).unwrap_or_default().as_secs_f64()
                                })
                            })
                        })
                        .filter(|x| x.is_finite())
                        .unwrap_or(0.0)
                        .clamp(0.0, 86400.0);
                    (None, code == 429 || (500..600).contains(&code), format!("HTTP {code}: 服务请求失败"))
                }
                Err(_) => (None, true, "网络连接失败".into()),
            };
            if let Some((value, serialized)) = value {
                h.failures = 0;
                check(&self.cancel)?;
                return Ok((value, serialized));
            }
            if transient {
                h.failures += 1;
                if h.failures >= 20 {
                    retry = retry.max(180.0);
                    h.failures = 0;
                }
                let mut jitter = [0u8; 1];
                let _ = getrandom::fill(&mut jitter);
                let delay =
                    retry.max(0.5 * 2f64.powi(attempt) + (jitter[0] as f64 / 255.0) * 0.25 * 2f64.powi(attempt));
                h.next = Some(
                    h.next
                        .expect("app_name_sources internal invariant")
                        .max(Instant::now() + Duration::from_secs_f64(delay)),
                );
            }
            if !transient || attempt == 2 {
                return network(&label);
            }
        }
        unreachable!()
    }
    pub fn wiki(&mut self, tag: &str) -> Result<Vec<String>> {
        let url = url(
            &self.db_base,
            "/wiki_pages.json",
            &[("search[title]", tag.into()), ("limit", "2".into()), ("only", "title,other_names".into())],
        );
        let data = self.cached(&url, None)?;
        let mut seen = HashSet::new();
        Ok(data
            .as_array()
            .into_iter()
            .flatten()
            .find(|v| v["title"] == tag)
            .into_iter()
            .flat_map(|p| p["other_names"].as_array().into_iter().flatten())
            .filter_map(Value::as_str)
            .map(|s| s.replace('_', " ").trim().to_string())
            .filter(|s| !s.is_empty() && seen.insert(s.clone()))
            .collect())
    }
    pub fn bg(&mut self, path: &str, body: Option<Value>) -> Result<Value> {
        self.cached(&format!("{}{path}", self.bg_base), body)
    }
    pub fn search(&mut self, query: &str, character: bool) -> Result<Vec<Value>> {
        let q = query.trim().chars().take(60).collect::<String>();
        if q.is_empty() {
            return Ok(vec![]);
        }
        let latin = q.chars().all(|c| (' '..='~').contains(&c));
        let mut fields = vec![
            ("limit", if latin { "8" } else { "20" }.into()),
            ("only", if latin { "name,post_count" } else { "title,tag[post_count]" }.into()),
        ];
        if latin {
            let pattern = if character {
                format!(
                    "*{}*",
                    Regex::new(r"[\s_:,.!?]+")
                        .expect("constant regular expression is valid")
                        .split(&q.to_lowercase())
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join("*")
                )
            } else {
                format!("*{}*", q.to_lowercase().replace(' ', "_"))
            };
            fields.extend([
                ("search[name_matches]", pattern),
                ("search[category]", if character { "4" } else { "3" }.into()),
                ("search[order]", "count".into()),
            ]);
            if character {
                fields.push(("search[hide_empty]", "yes".into()));
            }
        } else {
            fields.extend([
                ("search[other_names_match]", format!("*{q}*")),
                ("search[tag][category]", if character { "4" } else { "3" }.into()),
            ]);
        }
        let data =
            self.cached(&url(&self.db_base, if latin { "/tags.json" } else { "/wiki_pages.json" }, &fields), None)?;
        let mut rows: Vec<_> = data
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| {
                let tag = v[if latin { "name" } else { "title" }].as_str()?;
                let n = if latin { &v["post_count"] } else { &v["tag"]["post_count"] };
                let posts = n.as_i64().unwrap_or(0);
                (!tag.is_empty() && (!character || posts != 0)).then(|| json!({"tag":tag,"posts":posts}))
            })
            .collect();
        rows.sort_by_key(|r| -r["posts"].as_i64().unwrap_or(0));
        rows.truncate(8);
        Ok(rows)
    }
    pub fn tag(&mut self, tag: &str) -> Result<Option<Value>> {
        let address = url(
            &self.db_base,
            "/tags.json",
            &[
                ("search[name]", tag.into()),
                ("search[category]", "4".into()),
                ("limit", "1".into()),
                ("only", "name,post_count".into()),
            ],
        );
        let data = self.cached(&address, None)?;
        Ok(data
            .as_array()
            .into_iter()
            .flatten()
            .find(|v| v["name"] == tag)
            .map(|v| json!({"tag":tag,"posts":v["post_count"].as_i64().unwrap_or(0)})))
    }
    pub fn copyright(&mut self, tag: &str) -> Result<String> {
        let data = self.cached(
            &url(
                &self.db_base,
                "/related_tag.json",
                &[("search[query]", tag.into()), ("search[category]", "copyright".into()), ("limit", "10".into())],
            ),
            None,
        )?;
        let rows: Vec<_> = data["related_tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["tag"]["name"].as_str().is_some_and(|s| !s.is_empty()))
            .collect();
        for threshold in [0.9, 0.5] {
            if let Some(r) = rows
                .iter()
                .filter(|r| {
                    r.get("frequency").unwrap_or(&r["overlap_coefficient"]).as_f64().unwrap_or(0.0) >= threshold
                })
                .min_by(|a, b| {
                    a["tag"]["post_count"]
                        .as_f64()
                        .unwrap_or(0.0)
                        .total_cmp(&b["tag"]["post_count"].as_f64().unwrap_or(0.0))
                })
            {
                return Ok(r["tag"]["name"].as_str().expect("validated internal text field").into());
            }
        }
        Ok(String::new())
    }
}
/// 预览图大约 150 px；大得多的就不是预览图。
const PREVIEW_MAX: u64 = 512 * 1024;
/// Danbooru 角色 tag 下评分最高的全年龄（rating:g）帖子的预览图，以 `danbooru-<sha256 of the tag>.jpg`
/// 存放在图库的缩略图里（随缩略图一起清除和计数）。没有这样的帖子时为 None。查找和下载它需要名称查询的同意，
/// 与列出这个 tag 的搜索一样。
pub(crate) fn preview(source: &mut Source, thumbnails: &std::path::Path, tag: &str) -> Result<Option<Vec<u8>>> {
    use sha2::{Digest, Sha256};
    // 只有一个普通 tag：搜索里不带运算符、通配符，也不夹带第二个 tag。
    if tag.is_empty()
        || tag.len() > 200
        || tag.starts_with(['-', '~'])
        || tag.chars().any(|c| c.is_whitespace() || c.is_control() || c == '*' || c == ',')
    {
        return invalid("角色 tag 不正确");
    }
    let jpeg = |b: &[u8]| b.starts_with(&[0xff, 0xd8, 0xff]);
    let path = thumbnails.join(format!("danbooru-{:x}.jpg", Sha256::digest(tag.as_bytes())));
    if let Ok(bytes) = std::fs::read(&path) {
        if jpeg(&bytes) {
            return Ok(Some(bytes));
        }
    }
    let fields = [
        ("tags", format!("{tag} rating:g order:score")),
        ("limit", "1".into()),
        ("only", "id,preview_file_url".into()),
    ];
    let posts = source.cached(&url(&source.db_base, "/posts.json", &fields), None)?;
    let Some(address) = posts.as_array().and_then(|p| p.first()).and_then(|p| p["preview_file_url"].as_str()) else {
        return Ok(None);
    };
    let bytes = source.picture(address, PREVIEW_MAX)?;
    if !jpeg(&bytes) {
        return Ok(None);
    }
    std::fs::create_dir_all(thumbnails)?;
    // 要么完整，要么没有：读方从不会看到半张图。
    let tmp = path.with_extension(format!("{}.tmp", crate::store::new_key()?));
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(tmp, &path)?;
    Ok(Some(bytes))
}
fn validate(value: &Value, address: &str) -> Result<()> {
    let sequence = |v: &Value| v.as_array().is_some_and(|a| a.iter().all(Value::is_object));
    let empty = matches!(value, Value::Null | Value::Bool(false))
        || value.as_object().is_some_and(|o| o.is_empty())
        || value.as_str() == Some("")
        || value.as_i64() == Some(0);
    let valid = if address.contains("/wiki_pages.json")
        || address.contains("/tags.json")
        || (address.contains("/characters/") && address.ends_with("/subjects"))
    {
        empty || sequence(value)
    } else if address.contains("/search/") {
        empty || (value.is_object() && value.get("data").is_none_or(sequence))
    } else if address.contains("/related_tag.json") {
        empty || (value.is_object() && value.get("related_tags").is_none_or(sequence))
    } else {
        true
    };
    if valid {
        Ok(())
    } else {
        network("名称来源响应格式不正确")
    }
}
use regex::Regex;
use unicode_normalization::UnicodeNormalization;
pub(crate) fn normalize(s: &str) -> String {
    crate::casefold::casefold(
        &s.nfkc().filter(|c| !c.is_whitespace() && !['・', '·', '･'].contains(c)).collect::<String>(),
    )
}
fn kana(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '\u{3040}'..='\u{30ff}' | '\u{31f0}'..='\u{31ff}' | '\u{1b000}'..='\u{1b16f}'))
}
fn han(s: &str) -> bool {
    s.chars().any(han_char)
}
fn han_char(c: char) -> bool {
    matches!(c, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{20000}'..='\u{323af}')
}
fn hangul(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '\u{1100}'..='\u{11ff}' | '\u{3130}'..='\u{318f}' | '\u{ac00}'..='\u{d7ff}'))
}
pub(crate) fn group(s: &str) -> bool {
    static TEAM: OnceLock<Regex> = OnceLock::new();
    let n = normalize(s);
    n.contains("チーム")
        || n.ends_with(['队', '隊'])
        || TEAM.get_or_init(|| Regex::new(r"(?i)(?:^|[\s_])team$").expect("固定的团队名格式")).is_match(s)
}
pub(crate) fn keywords(names: &[String], character: bool) -> Vec<String> {
    let mut seen = HashSet::new();
    names
        .iter()
        .filter(|n| {
            let norm = normalize(n);
            norm.chars().count() <= 40
                && !norm.contains(['(', ')'])
                && (!character || !group(n))
                && (kana(&norm) || han(&norm))
                && seen.insert(norm)
        })
        .take(3)
        .cloned()
        .collect()
}
fn codec(s: &str, encoded: &str) -> bool {
    s.chars().all(|c| codec_char(c, encoded))
}
fn codec_char(c: char, encoded: &str) -> bool {
    static GB: OnceLock<Vec<(u32, u32)>> = OnceLock::new();
    static JP: OnceLock<Vec<(u32, u32)>> = OnceLock::new();
    let storage = if encoded == crate::name_codecs::GB2312 { &GB } else { &JP };
    let ranges = storage.get_or_init(|| {
        encoded
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| {
                let (a, b) = s.split_once('-').expect("app_name_sources internal invariant");
                (
                    u32::from_str_radix(a, 16).expect("app_name_sources internal invariant"),
                    u32::from_str_radix(b, 16).expect("app_name_sources internal invariant"),
                )
            })
            .collect()
    });
    let c = c as u32;
    let i = ranges.partition_point(|(a, _)| *a <= c);
    i > 0 && c <= ranges[i - 1].1
}
/// 比较 wiki 别名时解开 Danbooru 的空格编码和尾部作品限定词。
pub(crate) fn wiki_base(name: &str) -> String {
    static QUALIFIER: OnceLock<Regex> = OnceLock::new();
    let name = name.nfkc().collect::<String>().replace(['•', '・', '･'], "·").replace('_', " ");
    let name = name.trim();
    QUALIFIER
        .get_or_init(|| Regex::new(r"^(.+?)\s*\([^()]+\)$").expect("固定的作品限定词格式"))
        .captures(name)
        .map(|m| m[1].trim().to_owned())
        .unwrap_or_else(|| name.into())
}

/// 同一作品共享的无旁证别名和常见合称只用于搜索。多个候选中优先取有旁证的。
pub(crate) fn wiki_zh(
    names: &[String],
    works: &[String],
    confirmed: impl Fn(&str) -> bool,
    shared: impl Fn(&str) -> bool,
) -> Option<(String, bool)> {
    let works: Vec<_> = works.iter().map(|w| normalize(w)).filter(|w| !w.is_empty()).collect();
    let mut fallback = None;
    let mut previous = false;
    for name in names {
        let name = name.nfkc().collect::<String>();
        let base = wiki_base(&name);
        let key = normalize(&base);
        let h = base.chars().filter(|&c| han_char(c)).collect::<String>();
        let group_name = group(&base)
            || [
                "双子",
                "雙子",
                "双胞胎",
                "雙胞胎",
                "组合",
                "組合",
                "二人组",
                "二人組",
                "コンビ",
                "兄妹",
                "兄弟",
                "姉妹",
                "姐妹",
                "姊妹",
            ]
            .iter()
            .any(|suffix| key.ends_with(suffix));
        let shaped = !kana(&name) && !hangul(&name) && !h.is_empty() && base.chars().count() <= 20;
        if shaped && confirmed(&key) {
            return Some((base, true));
        }
        // 生僻汉字允许超出 GB2312。CP932 中的专用字仍需有旁证才可用。
        let usable = shaped
            && base.chars().all(|c| {
                codec_char(c, crate::name_codecs::GB2312)
                    || c == '·'
                    || (han_char(c) && !codec_char(c, crate::name_codecs::CP932))
            })
            && !group_name
            && !shared(&key)
            && !works.iter().any(|w| key.contains(w));
        if fallback.is_none() && usable && (!previous || !codec(&h, crate::name_codecs::CP932)) {
            fallback = Some((base, false));
        }
        // 被排除的合称和共用别名不影响之后本名的文字判断。
        previous |= !h.is_empty() && !group_name && !shared(&key);
    }
    fallback
}
pub(crate) fn subject_matches(v: &Value, names: &[String]) -> bool {
    ["name", "name_cn"].iter().any(|k| names.iter().any(|n| normalize(n) == normalize(v[k].as_str().unwrap_or(""))))
}
pub(crate) fn infobox(v: &Value) -> Option<String> {
    v["infobox"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|x| x["key"] == "简体中文名" && x["value"].as_str().is_some_and(|s| !s.trim().is_empty()))
        .and_then(|x| x["value"].as_str())
        .map(|s| s.trim().into())
}
const JAPANESE_WORKS: &[&str] = &[
    "touhou",
    "fate",
    "vocaloid",
    "hololive",
    "nijisanji",
    "idolmaster",
    "kancolle",
    "pokemon",
    "blue_archive",
    "umamusume",
    "love_live!",
    "bang_dream!",
    "project_sekai",
    "princess_connect!",
    "persona_3",
    "clannad",
    "guilty_gear",
    "macross",
    "umineko",
    "senren",
    "otonari_no_tenshi-sama",
];
pub(crate) fn chinese_work(work: &str, subjects: &[Value]) -> bool {
    const ZH: &[&str] = &[
        "genshin_impact",
        "arknights",
        "azur_lane",
        "girls'_frontline",
        "honkai:_star_rail",
        "zenless_zone_zero",
        "wuthering_waves",
        "honkai_impact_3rd",
    ];
    !JAPANESE_WORKS.contains(&work)
        && (ZH.contains(&work)
            || subjects.iter().any(|s| {
                let name = s["name"].as_str().unwrap_or("");
                han(name)
                    && !kana(name)
                    && !hangul(name)
                    && normalize(name) == normalize(s["name_cn"].as_str().unwrap_or(""))
            }))
}
/// 按角色目录的取名提示，从按 ID 取到的 Bangumi 角色里挑名字（#837）。
///
/// 提示是 `''`（中文名用「简体中文名」）、`name`（国产作品：原名就是中文名，「简体中文名」常空着或写的是别名）、
/// `p0`/`p1`…（「A／B」这种一个条目写了两个名字的，取第几段，原名同样取那一段），可以写成 `name:p1`。
/// 返回的写法和 [`character_values`] 一样：（名字列表，要另记成别名的「简体中文名」）。
pub(crate) fn picked_values(v: &Value, pick: &str) -> (Vec<Value>, Option<String>) {
    let (field, part) = match pick.split_once(':') {
        Some((f, p)) => (f, Some(p)),
        None if pick.starts_with('p') => ("", Some(pick)),
        None => (pick, None),
    };
    let index = part.and_then(|p| p.strip_prefix('p')).and_then(|n| n.parse::<usize>().ok());
    if part.is_some() && index.is_none() {
        return (vec![], None);
    }
    let segment = |s: &str| -> Option<String> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        match index {
            None => Some(s.to_owned()),
            Some(i) => Regex::new(r"\s*[／/]\s*")
                .expect("constant regular expression is valid")
                .split(s)
                .nth(i)
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(str::to_owned),
        }
    };
    let name = v["name"].as_str().unwrap_or("");
    let boxed = infobox(v);
    if field == "name" {
        let zh = segment(name);
        return (
            zh.into_iter().map(|z| json!(["zh", z])).collect(),
            boxed.filter(|b| Some(b.as_str()) != segment(name).as_deref()),
        );
    }
    let mut values = vec![];
    if let Some(original) = segment(name).filter(|n| !hangul(n)) {
        values.push(json!([if kana(&original) || han(&original) { "ja" } else { "en" }, original]));
    }
    if let Some(zh) = boxed.as_deref().and_then(segment) {
        values.push(json!(["zh", zh]));
    }
    (values, None)
}
pub(crate) fn character_values(
    v: &Value,
    work: &str,
    subjects: &[Value],
    wiki: &[String],
) -> (Vec<Value>, Option<String>) {
    let name = v["name"].as_str().unwrap_or("");
    let zh = !kana(name)
        && !hangul(name)
        && han(name)
        && (chinese_work(work, subjects)
            || (subjects.is_empty()
                && !JAPANESE_WORKS.contains(&work)
                && wiki.iter().any(|s| normalize(s) == normalize(name))));
    if zh {
        return (vec![json!(["zh", name])], infobox(v));
    }
    let mut values = vec![];
    if !hangul(name) {
        values.push(json!([if kana(name) || han(name) { "ja" } else { "en" }, name]));
    }
    values.push(json!(["zh", infobox(v)]));
    (values, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_queries_need_this_run_or_the_library_consent_read_at_each_request() {
        let path = std::env::temp_dir().join("illustash-names-consent-unit.sqlite");
        let cancel = Arc::new(AtomicBool::new(false));
        let refused = |source: &Source| match source.fetch("https://danbooru.donmai.us/tags.json", None, None) {
            Err(Error::Network(m)) => m == UNAUTHORIZED,
            _ => false,
        };
        assert!(refused(&Source::new(path.clone(), cancel.clone(), false, false).unwrap()));
        let grant = Arc::new(AtomicBool::new(false));
        let source = Source::new(path.clone(), cancel.clone(), false, false).unwrap().granted(grant.clone());
        assert!(refused(&source));
        grant.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(source.allowed());
        grant.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(refused(&source), "revoking stops the next request");
        assert!(Source::new(path, cancel, true, false).unwrap().allowed());
    }
    #[test]
    fn revocation_stops_requests_waiting_for_a_host_or_retry_deadline() {
        for queued in [true, false] {
            let host_name = if queued { "consent-queue.invalid" } else { "consent-backoff.invalid" };
            // 退避期长过等待上限：撤销要是没打断等待，下面的循环会先到上限而失败，不靠计时断言（#508）。
            let backoff = crate::test_wait::limit() * 2;
            let host = Arc::new(Mutex::new(Host { next: Some(Instant::now() + backoff), failures: 0 }));
            HOSTS.get_or_init(Default::default).lock().unwrap().insert(format!("{host_name}:443"), host.clone());
            let held = queued.then(|| host.lock().unwrap());
            let grant = Arc::new(AtomicBool::new(true));
            // 不需要数据库：被撤销的请求必须在发出 HTTP 之前停下。
            let source = Source::new(PathBuf::new(), Arc::new(AtomicBool::new(false)), false, false)
                .unwrap()
                .granted(grant.clone());
            let worker =
                std::thread::spawn(move || source.fetch(&format!("https://{host_name}/tags.json"), None, None));
            let deadline = crate::test_wait::deadline();
            while Arc::strong_count(&host) < 3 || (!queued && host.try_lock().is_ok()) {
                assert!(Instant::now() < deadline, "request did not reach the host wait");
                std::thread::sleep(Duration::from_millis(1));
            }
            grant.store(false, Ordering::SeqCst);
            let deadline = crate::test_wait::deadline();
            while !worker.is_finished() {
                assert!(Instant::now() < deadline, "revocation did not interrupt the wait");
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(matches!(worker.join().unwrap(), Err(Error::Network(m)) if m == UNAUTHORIZED));
            drop(held);
            HOSTS.get().unwrap().lock().unwrap().remove(&format!("{host_name}:443"));
        }
    }
    #[test]
    fn wiki_chinese_uses_exact_codec_order_and_work_exclusions() {
        for (input, works, expected) in [
            (vec!["アリス", "爱丽丝"], vec![], Some("爱丽丝")),
            (vec!["綾", "杉山清美"], vec![], None),
            (vec!["杉山清美"], vec![], Some("杉山清美")),
            (vec!["左衛門佐", "爱丽丝（示例）"], vec![], Some("爱丽丝")),
            (vec!["兄妹", "合成队", "示例角色"], vec!["示例"], None),
            (vec!["한글", "桜", "螢", "爱丽丝"], vec![], Some("爱丽丝")),
        ] {
            assert_eq!(
                wiki_zh(
                    &input.into_iter().map(str::to_owned).collect::<Vec<_>>(),
                    &works.into_iter().map(str::to_owned).collect::<Vec<_>>(),
                    |_| false,
                    |_| false,
                ),
                expected.map(|name| (name.to_owned(), false))
            );
        }
    }
    #[test]
    fn wiki_corroboration_accepts_rare_han_and_prefers_a_personal_name() {
        for name in ["十六夜咲夜", "秋山澪"] {
            assert_eq!(
                wiki_zh(&["小绰号".into(), name.into()], &[], |key| key == name, |_| false),
                Some((name.into(), true))
            );
        }
        assert_eq!(
            wiki_zh(&["队友别名".into(), "姐妹红".into()], &[], |_| false, |name| name == "队友别名"),
            Some(("姐妹红".into(), false))
        );
        assert_eq!(
            wiki_zh(&["队友别名".into()], &[], |name| name == "队友别名", |_| true),
            Some(("队友别名".into(), true))
        );
    }

    #[test]
    fn exact_match_keywords_and_original_language_preserve_rules() {
        assert_eq!(
            keywords(
                &[
                    "ア リス".into(),
                    "アリス".into(),
                    "Team".into(),
                    "チーム".into(),
                    "爱丽丝(作品)".into(),
                    "爱丽丝".into()
                ],
                true
            ),
            vec!["ア リス", "爱丽丝"]
        );
        let row = json!({"name":"合成角色","infobox":[{"key":"简体中文名","value":"另一个名字"}]});
        assert_eq!(
            character_values(&row, "touhou", &[], &["合成角色".into()]).0,
            vec![json!(["ja", "合成角色"]), json!(["zh", "另一个名字"])]
        );
        assert_eq!(
            character_values(&row, "genshin_impact", &[], &[]),
            (vec![json!(["zh", "合成角色"])], Some("另一个名字".into()))
        );
        assert_eq!(normalize("Ａ_・ リス"), "a_リス");
    }

    #[test]
    fn catalog_pick_chooses_field_and_segment() {
        let row = |name: &str, zh: &str| json!({"name":name,"infobox":[{"key":"简体中文名","value":zh}]});
        // 默认：原名记成日文，「简体中文名」是中文。
        assert_eq!(
            picked_values(&row("初音ミク", "初音未来"), ""),
            (vec![json!(["ja", "初音ミク"]), json!(["zh", "初音未来"])], None)
        );
        // 国产作品：原名是中文名，「简体中文名」写的是别名，另记成别名。
        assert_eq!(
            picked_values(&row("雷电将军", "巴尔泽布"), "name"),
            (vec![json!(["zh", "雷电将军"])], Some("巴尔泽布".into()))
        );
        // 一个条目写两个名字：按 tag 取对应的那一段，原名同样取那一段。
        let two = row("春野はるか／キュアフローラ", "春野遥／花神天使");
        assert_eq!(picked_values(&two, "p1"), (vec![json!(["ja", "キュアフローラ"]), json!(["zh", "花神天使"])], None));
        assert_eq!(picked_values(&two, "p0").0[1], json!(["zh", "春野遥"]));
        assert_eq!(picked_values(&row("甲/乙", ""), "name:p1"), (vec![json!(["zh", "乙"])], None));
        // 段号超出、提示认不出、名字是空的：挑不出中文名。
        assert!(!picked_values(&two, "p5").0.iter().any(|v| v[0] == "zh"));
        assert!(picked_values(&two, "px").0.is_empty());
        assert!(!picked_values(&row("Alice", ""), "").0.iter().any(|v| v[0] == "zh"));
    }
}

/// 原生传输的重试、限速和缓存规则（#9：原来是 tests/test_name_network.py，测试的是旧的 Python 更新器；
/// 原生的限制不同，按现在的样子测试）。
#[cfg(test)]
mod network_tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicUsize;

    struct Reply {
        status: u16,
        retry_after: Option<String>,
        body: &'static str,
        delay: Duration,
    }

    fn reply(status: u16, body: &'static str) -> Reply {
        Reply { status, retry_after: None, body, delay: Duration::ZERO }
    }

    /// 按脚本回答的回环服务器（脚本用完后回答 200 `[]`），每个请求一个线程。
    struct Scripted {
        base: String,
        host: String,
        calls: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Scripted {
        fn start(script: Vec<Reply>) -> Self {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let port = server.server_addr().to_ip().unwrap().port();
            let script = Arc::new(Mutex::new(VecDeque::from(script)));
            let (calls, peak, active) =
                (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
            let stop = Arc::new(AtomicBool::new(false));
            let (sc, c, p, s) = (script.clone(), calls.clone(), peak.clone(), stop.clone());
            let thread = std::thread::spawn(move || {
                while !s.load(Ordering::SeqCst) {
                    let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(10)) else { continue };
                    c.fetch_add(1, Ordering::SeqCst);
                    let next = sc.lock().unwrap().pop_front().unwrap_or_else(|| reply(200, "[]"));
                    let (p, active) = (p.clone(), active.clone());
                    std::thread::spawn(move || {
                        let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                        p.fetch_max(now, Ordering::SeqCst);
                        std::thread::sleep(next.delay);
                        active.fetch_sub(1, Ordering::SeqCst);
                        let mut response = tiny_http::Response::from_string(next.body).with_status_code(next.status);
                        if let Some(value) = next.retry_after {
                            response = response.with_header(
                                tiny_http::Header::from_bytes(&b"Retry-After"[..], value.as_bytes()).unwrap(),
                            );
                        }
                        let _ = request.respond(response);
                    });
                }
            });
            Self {
                base: format!("http://127.0.0.1:{port}"),
                host: format!("127.0.0.1:{port}"),
                calls,
                peak,
                stop,
                thread: Some(thread),
            }
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
        fn host(&self) -> Arc<Mutex<Host>> {
            HOSTS.get_or_init(Default::default).lock().unwrap().entry(self.host.clone()).or_default().clone()
        }
    }

    impl Drop for Scripted {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            self.thread.take().unwrap().join().unwrap();
        }
    }

    struct Library(PathBuf);

    impl Library {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "illustash-name-net-{}-{}",
                std::process::id(),
                crate::store::new_key().unwrap()
            ));
            std::fs::create_dir_all(&root).unwrap();
            Connection::open(root.join("library.sqlite"))
                .unwrap()
                .execute_batch(
                    "CREATE TABLE name_settings(key TEXT PRIMARY KEY,value TEXT NOT NULL);
                     CREATE TABLE name_cache(key TEXT PRIMARY KEY,value TEXT NOT NULL);",
                )
                .unwrap();
            Self(root)
        }
        fn source(&self, server: &Scripted, cancel: Arc<AtomicBool>) -> Source {
            Source::new(self.0.join("library.sqlite"), cancel, true, false)
                .unwrap()
                .with_bases(&server.base, &server.base)
        }
        fn cached_rows(&self) -> i64 {
            db(&self.0.join("library.sqlite"))
                .unwrap()
                .query_row("SELECT count(*) FROM name_cache", [], |r| r.get(0))
                .unwrap()
        }
    }

    impl Drop for Library {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn quiet() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    #[test]
    fn retry_after_then_5xx_exponential_backoff() {
        let server = Scripted::start(vec![
            Reply { retry_after: Some("1".into()), ..reply(429, "limit") },
            reply(503, "unavailable"),
            reply(200, r#"{"data":[]}"#),
        ]);
        let lib = Library::new();
        let started = Instant::now();
        let value =
            lib.source(&server, quiet()).fetch(&format!("{}/search/characters", server.base), None, None).unwrap();
        assert_eq!(value, json!({"data":[]}));
        assert_eq!(server.calls(), 3);
        // Retry-After 1 s，然后是第二级指数退避（至少 1 s）。
        assert!(started.elapsed() >= Duration::from_millis(1950), "{:?}", started.elapsed());
        assert_eq!(server.host().lock().unwrap().failures, 0);
    }

    #[test]
    fn retry_after_http_date() {
        // HTTP-date 精确到整秒，所以客户端等到 floor(now + 3 s)，即取日期之后至少 2 s。从那一刻开始计时：
        // 准备时间不能算进等待，而忽略这个头只会剩下第一级退避（最多 0.625 s）。
        let started = Instant::now();
        let date = httpdate::fmt_http_date(std::time::SystemTime::now() + Duration::from_secs(3));
        let server = Scripted::start(vec![Reply { retry_after: Some(date), ..reply(503, "busy") }]);
        let lib = Library::new();
        lib.source(&server, quiet()).fetch(&format!("{}/subjects/1", server.base), None, None).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(1950), "{:?}", started.elapsed());
        assert_eq!(server.calls(), 2);
    }

    #[test]
    fn not_found_is_empty_and_other_4xx_do_not_retry() {
        for code in [400, 401, 403] {
            let server = Scripted::start(vec![reply(code, "private body")]);
            let lib = Library::new();
            let error = lib.source(&server, quiet()).fetch(&format!("{}/search/characters", server.base), None, None);
            assert!(matches!(&error, Err(Error::Network(m)) if !m.contains("private")), "{code}: {error:?}");
            assert_eq!(server.calls(), 1, "{code}");
        }
        let server = Scripted::start(vec![reply(404, "")]);
        let lib = Library::new();
        let value = lib.source(&server, quiet()).fetch(&format!("{}/characters/0", server.base), None, None).unwrap();
        assert_eq!((value, server.calls()), (Value::Null, 1));
    }

    #[test]
    fn exhausted_network_error_has_no_negative_cache() {
        let server = Scripted::start(vec![reply(503, ""), reply(503, ""), reply(503, "")]);
        let lib = Library::new();
        let address = format!("{}/wiki_pages.json?search%5Btitle%5D=test", server.base);
        assert!(lib.source(&server, quiet()).cached(&address, None).is_err());
        assert_eq!((server.calls(), lib.cached_rows()), (3, 0));
        assert_eq!(lib.source(&server, quiet()).cached(&address, None).unwrap(), json!([]));
        assert_eq!(lib.cached_rows(), 1);
    }

    #[test]
    fn twenty_consecutive_host_failures_pause_the_host() {
        let server = Scripted::start(vec![reply(503, "")]);
        server.host().lock().unwrap().failures = 19;
        let lib = Library::new();
        let cancel = quiet();
        let source = lib.source(&server, cancel.clone());
        let address = format!("{}/search/subjects", server.base);
        let started = Instant::now();
        let worker = std::thread::spawn(move || source.fetch(&address, None, None));
        while server.calls() < 1 {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(100));
        cancel.store(true, Ordering::SeqCst); // 暂停很长；停下正在等待的请求
        assert!(worker.join().unwrap().is_err());
        let host = server.host();
        let host = host.lock().unwrap();
        assert_eq!(host.failures, 0);
        assert!(host.next.unwrap() >= started + Duration::from_secs(179), "paused for 180 s");
        assert_eq!(server.calls(), 1);
    }

    #[test]
    fn one_request_in_flight_per_host() {
        let server =
            Scripted::start((0..4).map(|_| Reply { delay: Duration::from_millis(50), ..reply(200, "[]") }).collect());
        let lib = Library::new();
        let workers: Vec<_> = (0..4)
            .map(|i| {
                let source = lib.source(&server, quiet());
                let address = format!("{}/subjects/{i}", server.base);
                std::thread::spawn(move || source.fetch(&address, None, None).unwrap())
            })
            .collect();
        for w in workers {
            assert_eq!(w.join().unwrap(), json!([]));
        }
        assert_eq!((server.calls(), server.peak.load(Ordering::SeqCst)), (4, 1));
    }

    const JPEG: &[u8] = &[0xff, 0xd8, 0xff, 0xe0, 0, 16, b'J', b'F', b'I', b'F'];
    /// 供「设为…」预览使用的合成 Danbooru（#293）：posts.json 只对 `known` 回答一个帖子，
    /// 它的预览图是 `picture`（除非是绝对地址，否则在这个服务上）；其他请求都回答一张 JPEG。
    struct Previews {
        base: String,
        calls: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }
    impl Previews {
        fn start(known: &'static str, picture: &'static str) -> Self {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let base = format!("http://127.0.0.1:{}", server.server_addr().to_ip().unwrap().port());
            let (calls, stop) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
            let (c, s, b) = (calls.clone(), stop.clone(), base.clone());
            let thread = std::thread::spawn(move || {
                while !s.load(Ordering::SeqCst) {
                    let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(10)) else { continue };
                    c.fetch_add(1, Ordering::SeqCst);
                    let address = request.url().to_owned();
                    let (path, query) = address.split_once('?').unwrap_or((&address, ""));
                    let body = if path == "/posts.json" {
                        let tags = url::form_urlencoded::parse(query.as_bytes())
                            .find(|(k, _)| k == "tags")
                            .map(|(_, v)| v.into_owned())
                            .unwrap();
                        assert!(tags.ends_with(" rating:g order:score"), "{tags}");
                        let at =
                            if picture.starts_with("https://") { picture.to_owned() } else { format!("{b}{picture}") };
                        if tags == format!("{known} rating:g order:score") {
                            json!([{"id": 1, "preview_file_url": at}]).to_string().into_bytes()
                        } else {
                            b"[]".to_vec()
                        }
                    } else {
                        JPEG.to_vec()
                    };
                    let _ = request.respond(tiny_http::Response::from_data(body));
                }
            });
            Self { base, calls, stop, thread: Some(thread) }
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }
    impl Drop for Previews {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            self.thread.take().unwrap().join().unwrap();
        }
    }
    fn previewer(lib: &Library, base: &str, authorized: bool) -> Source {
        Source::new(lib.0.join("library.sqlite"), quiet(), authorized, false).unwrap().with_bases(base, base)
    }

    #[test]
    fn an_all_ages_preview_is_fetched_once_and_kept_with_the_thumbnails() {
        let (lib, server) = (Library::new(), Previews::start("rem_(re:zero)", "/preview/ab/cd/x.jpg"));
        let thumbnails = lib.0.join("thumbnails");
        let got = preview(&mut previewer(&lib, &server.base, true), &thumbnails, "rem_(re:zero)").unwrap();
        assert_eq!(got.as_deref(), Some(JPEG));
        assert_eq!(server.calls(), 2, "the post, then its preview");
        let kept: Vec<_> =
            std::fs::read_dir(&thumbnails).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert_eq!(kept.len(), 1);
        assert!(kept[0].starts_with("danbooru-") && kept[0].ends_with(".jpg"), "{kept:?}");
        // 已保存：即使没有同意也不再请求。
        let again = preview(&mut previewer(&lib, &server.base, false), &thumbnails, "rem_(re:zero)").unwrap();
        assert_eq!(again.as_deref(), Some(JPEG));
        assert_eq!(server.calls(), 2);
        // 没有全年龄帖子：什么都没有，这个答案记在名称缓存里。
        for _ in 0..2 {
            let none = preview(&mut previewer(&lib, &server.base, true), &thumbnails, "nobody_(nowhere)");
            assert!(none.unwrap().is_none());
        }
        assert_eq!(server.calls(), 3);
    }

    #[test]
    fn a_preview_is_one_plain_tag_under_the_consent_and_only_from_danbooru() {
        let (lib, server) = (Library::new(), Previews::start("rem_(re:zero)", "https://example.com/x.jpg"));
        let thumbnails = lib.0.join("thumbnails");
        for bad in ["", "-rem", "~rem", "rem ram", "rem,ram", "rem*", "rem\n"] {
            let got = preview(&mut previewer(&lib, &server.base, true), &thumbnails, bad);
            assert!(matches!(got, Err(Error::Invalid(_))), "{bad:?}");
        }
        // 没有同意时访问 Danbooru 本身：在发出任何东西之前就拒绝（本机上的合成服务不需要同意，所以这里用真实地址）。
        let refused = preview(&mut previewer(&lib, "https://danbooru.donmai.us", false), &thumbnails, "rem_(re:zero)");
        assert!(matches!(refused, Err(Error::Network(m)) if m == UNAUTHORIZED));
        assert_eq!(server.calls(), 0);
        // 预览图不在 Danbooru CDN 上的帖子不跟随。
        let elsewhere = preview(&mut previewer(&lib, &server.base, true), &thumbnails, "rem_(re:zero)");
        assert!(matches!(elsewhere, Err(Error::Network(_))));
        assert_eq!(server.calls(), 1);
        assert!(!thumbnails.exists());
    }
}
