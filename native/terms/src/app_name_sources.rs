//! Text-only name sources. Network never holds a database transaction.
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
fn fail<T>(s: &str) -> Result<T> {
    Err(Error::Invalid(s.into()))
}
pub(crate) fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) || crate::platform_process::stopping() {
        fail("名称任务已取消")
    } else {
        Ok(())
    }
}
pub(crate) fn db(path: &std::path::Path) -> Result<Connection> {
    let c = crate::sqlite::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    c.busy_timeout(crate::WRITER_BUSY)?;
    Ok(c)
}
#[derive(Default)]
struct Host {
    next: Option<Instant>,
    failures: u32,
}
// Shared across synchronous pickers and background workers, including session rotations.
static HOSTS: OnceLock<Mutex<HashMap<String, Arc<Mutex<Host>>>>> = OnceLock::new();
/// The error of a name text query made without authorization; the front end keys on it.
pub(crate) const UNAUTHORIZED: &str = "显示名联网尚未授权";
pub(crate) struct Source {
    pub path: PathBuf,
    pub cancel: Arc<AtomicBool>,
    /// Authorized for this process (`--names-network`, or index network for the index task).
    pub authorized: bool,
    /// The library's lasting consent to name text queries, read at every request so that
    /// revoking it stops the next request of a running task.
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
            return fail("名称替换地址须为本机合成服务");
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
    /// Tests point both providers at one loopback service without touching the process environment.
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
            // One in-flight request per host is within the two-request maximum.
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
            {
                let mut c = db(&self.path)?;
                let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let reserved: i64 = tx
                    .query_row(
                        "SELECT CAST(value AS INTEGER) FROM name_settings WHERE key='traffic_reserved'",
                        [],
                        |r| r.get(0),
                    )
                    .optional()?
                    .unwrap_or(0);
                if reserved + 2_000_000 > 500_000_000 {
                    return network("已达到 500 MB 流量预算；继续前需要用户批准");
                }
                tx.execute(
                    "INSERT OR REPLACE INTO name_settings VALUES ('traffic_reserved',?)",
                    [(reserved + 2_000_000).to_string()],
                )?;
                tx.commit()?;
            }
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
                let actual = serialized.len() + address.len() + body.map(|v| v.to_string().len()).unwrap_or(4) + 2048;
                db(&self.path)?.execute(
                    "UPDATE name_settings SET value=CAST(value AS INTEGER)-? WHERE key='traffic_reserved'",
                    [(2_000_000usize.saturating_sub(actual)) as i64],
                )?;
                h.failures = 0;
                check(&self.cancel)?;
                return Ok((value, serialized));
            }
            if transient {
                db(&self.path)?.execute(
                    "UPDATE name_settings SET value=CAST(value AS INTEGER)-1934464 WHERE key='traffic_reserved'",
                    [],
                )?;
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
    s.chars().any(|c| ('\u{3040}'..='\u{30ff}').contains(&c))
}
fn han(s: &str) -> bool {
    s.chars().any(|c| ('\u{3400}'..='\u{9fff}').contains(&c))
}
fn hangul(s: &str) -> bool {
    s.chars().any(|c| ('\u{ac00}'..='\u{d7a3}').contains(&c))
}
pub(crate) fn group(s: &str) -> bool {
    let n = normalize(s);
    n.contains("チーム")
        || n.ends_with(['队', '隊'])
        || Regex::new(r"(?i)(?:^|[\s_])team$").expect("constant regular expression is valid").is_match(s)
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
    s.chars().all(|c| {
        let c = c as u32;
        let i = ranges.partition_point(|(a, _)| *a <= c);
        i > 0 && c <= ranges[i - 1].1
    })
}
pub(crate) fn wiki_zh(names: &[String], works: &[String]) -> Option<String> {
    let mut previous = false;
    let re = Regex::new(r"^(.+?)\s*[(（][^()（）]+[)）]$").expect("constant regular expression is valid");
    for name in names {
        let name = name.nfkc().collect::<String>().replace(['•', '・'], "·");
        let name = name.trim();
        let base = re.captures(name).map(|m| m[1].trim().to_owned()).unwrap_or_else(|| name.into());
        let h = base.chars().filter(|c| ('\u{3400}'..='\u{9fff}').contains(c)).collect::<String>();
        let usable = !kana(name)
            && !hangul(name)
            && !h.is_empty()
            && base.chars().count() <= 20
            && codec(&base.replace('·', ""), crate::name_codecs::GB2312)
            && !group(&base)
            && !["兄妹", "姐妹", "兄弟"].iter().any(|s| base.contains(s))
            && !works.iter().any(|w| !w.is_empty() && normalize(&base).contains(&normalize(w)));
        if usable && (!previous || !codec(&h, crate::name_codecs::CP932)) {
            return Some(base);
        }
        previous |= !h.is_empty();
    }
    None
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
fn chinese(name: &str, work: &str, subjects: &[Value], wiki: &[String]) -> bool {
    const JA: &[&str] = &[
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
    !kana(name)
        && !hangul(name)
        && han(name)
        && !JA.contains(&work)
        && (ZH.contains(&work)
            || subjects.iter().any(|s| {
                !s["name"].as_str().unwrap_or("").is_empty()
                    && normalize(s["name"].as_str().expect("validated internal text field"))
                        == normalize(s["name_cn"].as_str().unwrap_or(""))
            })
            || (subjects.is_empty() && wiki.iter().any(|s| normalize(s) == normalize(name))))
}
pub(crate) fn character_values(
    v: &Value,
    work: &str,
    subjects: &[Value],
    wiki: &[String],
) -> (Vec<Value>, Option<String>) {
    let name = v["name"].as_str().unwrap_or("");
    let zh = chinese(name, work, subjects, wiki);
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
            let host = Arc::new(Mutex::new(Host { next: Some(Instant::now() + Duration::from_secs(10)), failures: 0 }));
            HOSTS.get_or_init(Default::default).lock().unwrap().insert(format!("{host_name}:443"), host.clone());
            let held = queued.then(|| host.lock().unwrap());
            let grant = Arc::new(AtomicBool::new(true));
            // No database is needed: a revoked request must stop before reserving traffic or sending HTTP.
            let source = Source::new(PathBuf::new(), Arc::new(AtomicBool::new(false)), false, false)
                .unwrap()
                .granted(grant.clone());
            let worker =
                std::thread::spawn(move || source.fetch(&format!("https://{host_name}/tags.json"), None, None));
            let deadline = Instant::now() + Duration::from_secs(2);
            while Arc::strong_count(&host) < 3 || (!queued && host.try_lock().is_ok()) {
                assert!(Instant::now() < deadline, "request did not reach the host wait");
                std::thread::sleep(Duration::from_millis(1));
            }
            grant.store(false, Ordering::SeqCst);
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
                    &works.into_iter().map(str::to_owned).collect::<Vec<_>>()
                ),
                expected.map(str::to_owned)
            );
        }
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
}

/// Retry, pacing and cache rules of the native transport (#9: formerly tests/test_name_network.py,
/// which exercised the old Python updater; native limits differ and are tested as they are).
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

    /// Loopback server answering from a script (then 200 `[]`), one thread per request.
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
        // Retry-After 1 s, then the second exponential step (at least 1 s).
        assert!(started.elapsed() >= Duration::from_millis(1950), "{:?}", started.elapsed());
        assert_eq!(server.host().lock().unwrap().failures, 0);
    }

    #[test]
    fn retry_after_http_date() {
        // HTTP-date has whole-second precision, so the client waits until floor(now + 3 s), at least 2 s
        // after the date is taken. Measure from that moment: setup time must not count against the wait,
        // while ignoring the header would leave only the first backoff step (at most 0.625 s).
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
        cancel.store(true, Ordering::SeqCst); // the pause is long; stop the waiting request
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
}
