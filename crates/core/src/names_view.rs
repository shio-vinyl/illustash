//! 名字的显示投影（docs/TERMS_NAMES.md 方案 N1）：逐条移植 tests/reference/web/names.py（#9 已删除） 的 `View`（`entity_rows`、`load`、`choose`、
//! `entity`、`base`、`own_name`）和词表（`PRIORITY`、`WORK_NAMES`、`readable`、`skin_zh`）。
//! 输入是身份（`character_entities::ENTITIES`）、当前链接、名字和按文字键的显示名 `term_key_names`，输出两张投影表：
//! - `term_name_display(entity, skin_only, …)`：`View.entity(id, skin_only)`；
//! - `term_name_choice(kind, key, …)`：`View.choose(kind, key)`，键的范围是人物的 tag、作品文字（身份的 `work` 与
//!   qualifier 链接）、皮肤 variant。
//!
//! 结果曾与旧代码全量逐字对照；现由 native/terms/tests/baseline.rs 的新对照基准（#201，含 test_terms_names_projection）重放钉住。
use crate::character_entities::ENTITIES;
use crate::character_links::LINKS;
use crate::{Error, Result};
use rusqlite::{params, Connection};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::LazyLock;

/// `names.PRIORITY`：小的优先。
pub const PRIORITY: [&str; 7] = ["user", "bangumi", "index", "danbooru", "wiki", "builtin", "model"];

/// 旧有造型词，也是作品后缀筛选原先采用的词表。
const LEGACY_SKINS: [(&str, &str); 15] = [
    ("swimsuit", "泳装"),
    ("new_year", "新年"),
    ("bunny", "兔女郎"),
    ("school_uniform", "校服"),
    ("first_ascension", "第一再临"),
    ("second_ascension", "第二再临"),
    ("third_ascension", "第三再临"),
    ("casual", "便服"),
    ("dress", "礼服"),
    ("idol", "偶像"),
    ("maid", "女仆"),
    ("kimono", "和服"),
    ("halloween", "万圣节"),
    ("christmas", "圣诞"),
    ("festival_outfit", "祭典服"),
];

/// 以下词只在已知是造型的 variant 中翻译。
const COMMON_SKINS: &[(&str, &str)] = &[
    ("armed", "武装"),
    ("autumn", "秋季"),
    ("band", "乐队"),
    ("camp", "露营"),
    ("cheer_squad", "啦啦队服"),
    ("elite_i", "精英一阶段"),
    ("elite_ii", "精英二阶段"),
    ("goth", "哥特风"),
    ("gothic_lolita", "哥特洛丽塔"),
    ("hoodie", "连帽衫"),
    ("jirai_kei", "地雷系"),
    ("loungewear", "家居服"),
    ("nurse", "护士服"),
    ("pajamas", "睡衣"),
    ("racing", "赛车服"),
    ("roomwear", "家居服"),
    ("sailor", "水手服"),
    ("sleepwear", "睡衣"),
    ("sportswear", "运动服"),
    ("spring", "春季"),
    ("street", "街头风"),
    ("streetwear", "街头服饰"),
    ("suit", "西服"),
    ("summer", "夏季"),
    ("track", "田径服"),
    ("winter", "冬季"),
];

/// `names.WORK_NAMES`。
pub const WORK_NAMES: [(&str, &str); 22] = [
    ("touhou", "东方Project"),
    ("hololive", "hololive"),
    ("nijisanji", "彩虹社"),
    ("vocaloid", "VOCALOID"),
    ("fate", "Fate"),
    ("idolmaster", "偶像大师"),
    ("kancolle", "舰队Collection"),
    ("azur_lane", "碧蓝航线"),
    ("girls'_frontline", "少女前线"),
    ("pokemon", "宝可梦"),
    ("genshin_impact", "原神"),
    ("arknights", "明日方舟"),
    ("blue_archive", "蔚蓝档案"),
    ("honkai:_star_rail", "崩坏：星穹铁道"),
    ("zenless_zone_zero", "绝区零"),
    ("wuthering_waves", "鸣潮"),
    ("honkai_impact_3rd", "崩坏3"),
    ("umamusume", "赛马娘"),
    ("love_live!", "LoveLive!"),
    ("bang_dream!", "BanG Dream!"),
    ("project_sekai", "世界计划"),
    ("princess_connect!", "公主连结"),
];

fn priority(source: &str) -> Result<usize> {
    PRIORITY
        .iter()
        .position(|p| *p == source)
        .ok_or_else(|| Error::Invalid(format!("名字来源不在优先级表里：{source}")))
}

pub fn work_name(key: &str) -> Option<&'static str> {
    WORK_NAMES.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// 词首大写（沿用旧实现 `str.title()` 的规则）：前一个字符不是有大小写的字母时，本字符转成首字母大写，否则转小写。
fn title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut previous_cased = false;
    for ch in s.chars() {
        if ch.is_uppercase() {
            if previous_cased {
                out.extend(ch.to_lowercase());
            } else {
                out.push(ch);
            }
            previous_cased = true;
        } else if ch.is_lowercase() {
            if previous_cased {
                out.push(ch);
            } else {
                out.extend(ch.to_uppercase());
            }
            previous_cased = true;
        } else {
            out.push(ch);
            previous_cased = false;
        }
    }
    out
}

/// `names.readable`：人物的 tag 先去掉结尾的 `_(qualifier)`。
pub fn readable(tag: &str, character: bool) -> String {
    let mut t = tag;
    if character && t.ends_with(')') {
        // re.sub(r'_\([^()]+\)$', '', tag)
        if let Some(open) = t.rfind("_(") {
            let inner = &t[open + 2..t.len() - 1];
            if !inner.is_empty() && !inner.contains(['(', ')']) {
                t = &t[..open];
            }
        }
    }
    title(&t.replace('_', " "))
}

fn ordinal_costume(tag: &str) -> Option<(String, &str)> {
    let (ordinal, tail) = tag.split_once("_costume")?;
    let remaining = if tail.is_empty() { "" } else { tail.strip_prefix('_').filter(|s| !s.is_empty())? };
    let (digits, suffix) = ordinal.split_at_checked(ordinal.len().checked_sub(2)?)?;
    if digits.starts_with('0') || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let number: usize = digits[digits.len().saturating_sub(2)..].parse().ok()?;
    let expected = match (number % 100, number % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    };
    if suffix != expected {
        return None;
    }
    const ZH: [&str; 10] = ["", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
    let shown = match (digits.len(), number) {
        (1, n) => ZH[n].to_owned(),
        (2, 10..=19) => format!("十{}", ZH[number % 10]),
        (2, n) => format!("{}十{}", ZH[n / 10], ZH[n % 10]),
        _ => digits.to_owned(),
    };
    Some((format!("第{shown}套衣装"), remaining))
}

fn translate_skin_words(tag: &str, words: &[(&str, &str)], ordinals: bool) -> Option<String> {
    if ordinals {
        if let Some((zh, rest)) = ordinal_costume(tag) {
            return if rest.is_empty() {
                Some(zh)
            } else {
                translate_skin_words(rest, words, ordinals).map(|rest| format!("{zh} · {rest}"))
            };
        }
    }
    for &(word, zh) in words {
        if tag == word {
            return Some(zh.to_owned());
        }
        if let Some(rest) = tag.strip_prefix(word).and_then(|r| r.strip_prefix('_')) {
            if let Some(rest) = translate_skin_words(rest, words, ordinals) {
                return Some(format!("{zh} · {rest}"));
            }
        }
    }
    None
}

/// 冻结录制重放指定旧选名规则，新图库和现行测试使用当前规则。
pub fn legacy_skin_names() -> bool {
    static LEGACY: LazyLock<bool> = LazyLock::new(|| std::env::var_os("ILLUSTASH_TEST_LEGACY_SKIN_NAMES").is_some());
    *LEGACY
}

fn legacy_skin_zh(tag: &str) -> Option<String> {
    static WORDS: LazyLock<[(&str, &str); LEGACY_SKINS.len()]> = LazyLock::new(|| {
        let mut words = LEGACY_SKINS;
        words.sort_by_key(|(word, _)| std::cmp::Reverse(word.len()));
        words
    });
    translate_skin_words(tag, WORDS.as_slice(), false)
}

/// 造型词最长优先，无法完整拆解的造型保留整个原名。
pub fn skin_zh(tag: &str) -> Option<String> {
    if legacy_skin_names() {
        return legacy_skin_zh(tag);
    }
    static WORDS: LazyLock<Vec<(&str, &str)>> = LazyLock::new(|| {
        let mut words: Vec<_> = LEGACY_SKINS.into_iter().chain(COMMON_SKINS.iter().copied()).collect();
        words.sort_by_key(|(word, _)| std::cmp::Reverse(word.len()));
        words
    });
    translate_skin_words(tag, &WORDS, true)
}

/// 作品括号中的词是否匹配原有的、明确表示造型的词表。
pub(crate) fn costume_qualifier(tag: &str) -> bool {
    legacy_skin_zh(tag).is_some()
}

/// 新增词表能翻译而旧词表不能，用于找需要重投影的造型。
pub fn expanded_skin_name(tag: &str) -> bool {
    skin_zh(tag).is_some() && !costume_qualifier(tag)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub name: String,
    pub source: String,
    pub names: [Option<String>; 3], // zh, ja, en
}

const LANGS: [&str; 3] = ["zh", "ja", "en"];

#[derive(Clone, Debug, PartialEq, Eq)]
struct NameRow {
    lang: String,
    value: String,
    source: String,
    preferred: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entity {
    id: String,
    kind: String,
    parent: Option<String>,
    variant: String,
    merged: Option<String>,
    original: bool,
    keys: BTreeMap<String, String>,
    linked: bool,
    tag: String,
    /// 身份的先后 `seq`（原来 `entities` 的 rowid）顺序（人物键下的名字按这个顺序拼接）。u32 落在已有的对齐空隙里，不增大快照。
    seq: u32,
}

/// `View` 的一个快照。
pub struct View {
    entities: BTreeMap<String, Entity>,
    merged: HashMap<String, Vec<String>>,
    names: HashMap<(String, String), Vec<NameRow>>,
    own: HashMap<String, Vec<NameRow>>,
    /// 身份的 `work`，按 `seq` 排；只有 `load_with_work` 载入，否则为空。
    works: Vec<String>,
}

/// 一个身份在快照里的样子（`/api/names` 的统计用，免得再读一遍身份和链接）。
pub struct Identity<'a> {
    pub id: &'a str,
    pub kind: &'a str,
    pub parent: Option<&'a str>,
    pub merged_into: Option<&'a str>,
    pub original: bool,
    /// 有当前链接。
    pub linked: bool,
    pub tag: &'a str,
    pub work: &'a str,
    pub variant: &'a str,
}

#[cfg(feature = "verify-incremental")]
thread_local! {
    static WHOLE_LOADS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 本线程整库载入（`View::load`、`load_with_work`）的次数；测试用来断言读路径不再整库载入（#105）。对照核对里的
/// 整库载入不计。
#[cfg(feature = "verify-incremental")]
pub fn whole_loads() -> usize {
    WHOLE_LOADS.with(|n| n.get())
}

pub fn linked_tag(keys: &BTreeMap<String, String>) -> Result<String> {
    for system in ["danbooru", "wd", "photos_album", "ccip"] {
        if let Some(k) = keys.get(system).filter(|k| !k.is_empty()) {
            return Ok(k.clone());
        }
    }
    Ok(match keys.get("ccip_base") {
        Some(base) => serde_json::from_str::<serde_json::Value>(base)
            .map_err(|e| Error::Invalid(format!("ccip_base 链接不是 JSON：{e}")))?
            .get(0)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned(),
        None => String::new(),
    })
}

impl View {
    pub fn load(c: &Connection) -> Result<View> {
        View::load_inner(c, false)
    }

    /// 同 `load`，另带上身份的 `work`（`identities` 里的 `work`）。
    pub fn load_with_work(c: &Connection) -> Result<View> {
        View::load_inner(c, true)
    }

    fn load_inner(c: &Connection, with_work: bool) -> Result<View> {
        #[cfg(feature = "verify-incremental")]
        WHOLE_LOADS.with(|n| n.set(n.get() + 1));
        View::load_uncounted(c, with_work)
    }

    fn load_uncounted(c: &Connection, with_work: bool) -> Result<View> {
        let mut entities: BTreeMap<String, Entity> = BTreeMap::new();
        let mut order: Vec<String> = Vec::new();
        let mut works: Vec<String> = Vec::new();
        let work = if with_work { ",work" } else { "" };
        let sql = format!(
            "SELECT id,kind,parent_id,variant,merged_into,coalesce(original,0){work} FROM {ENTITIES} ORDER BY seq"
        );
        for row in c.prepare(&sql)?.query_map([], |r| {
            Ok((
                Entity {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    parent: r.get(2)?,
                    variant: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    merged: r.get(4)?,
                    original: r.get::<_, i64>(5)? != 0,
                    keys: BTreeMap::new(),
                    linked: false,
                    tag: String::new(),
                    seq: 0,
                },
                if with_work { r.get::<_, Option<String>>(6)?.unwrap_or_default() } else { String::new() },
            ))
        })? {
            let (mut e, work) = row?;
            e.seq = u32::try_from(order.len()).map_err(|_| Error::Invalid("身份太多".into()))?;
            if with_work {
                works.push(work);
            }
            order.push(e.id.clone());
            entities.insert(e.id.clone(), e);
        }
        for row in c
            .prepare(&format!(
                "SELECT l.system, l.key, l.entity_id FROM {LINKS} l WHERE l.current = 1 ORDER BY l.added_at, l.key"
            ))?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
        {
            let (system, key, ident) = row?;
            if let Some(e) = entities.get_mut(&ident) {
                e.keys.entry(system).or_insert(key);
            }
        }
        for e in entities.values_mut() {
            e.linked = !e.keys.is_empty();
            e.tag = linked_tag(&e.keys)?;
        }
        let mut merged: HashMap<String, Vec<String>> = HashMap::new();
        for e in entities.values() {
            // 按 id 排序（BTreeMap）
            if let Some(m) = e.merged.as_ref().filter(|m| !m.is_empty()) {
                if e.linked {
                    merged.entry(m.clone()).or_default().push(e.id.clone());
                }
            }
        }
        let mut names: HashMap<(String, String), Vec<NameRow>> = HashMap::new();
        let has_display: bool =
            c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='term_key_names')", [], |r| r.get(0))?;
        if has_display {
            for row in c
                .prepare("SELECT kind,key,lang,source,value FROM term_key_names WHERE source<>'danbooru'")?
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        NameRow { lang: r.get(2)?, source: r.get(3)?, value: r.get(4)?, preferred: false },
                    ))
                })?
            {
                let (kind, key, n) = row?;
                names.entry((kind, key)).or_default().push(n);
            }
        }
        let mut own: HashMap<String, Vec<NameRow>> = HashMap::new();
        for row in c
            .prepare(
                "SELECT t.key, n.lang, n.value, n.source, n.preferred
                 FROM term_names n
                 JOIN term_tree r ON r.term_id = n.term_id
                 JOIN terms t ON t.id = n.term_id
                 WHERE n.role = 'display'
                 ORDER BY n.preferred DESC, n.value, n.seq",
            )?
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    NameRow {
                        lang: r.get(1)?,
                        value: r.get(2)?,
                        source: r.get(3)?,
                        preferred: r.get::<_, i64>(4)? != 0,
                    },
                ))
            })?
        {
            let (ident, n) = row?;
            own.entry(ident).or_default().push(n);
        }
        for ident in &order {
            let e = &entities[ident];
            if e.kind != "work" && e.linked && e.parent.is_none() {
                if let Some(rows) = own.get(ident) {
                    names.entry(("character".into(), e.tag.clone())).or_default().extend(rows.iter().cloned());
                }
            }
        }
        Ok(View { entities, merged, names, own, works })
    }

    pub fn ids(&self) -> impl Iterator<Item = &String> {
        self.entities.keys()
    }

    /// 全部身份，按 id 排序。
    pub fn identities(&self) -> impl Iterator<Item = Identity<'_>> {
        self.entities.values().map(|e| Identity {
            id: &e.id,
            kind: &e.kind,
            parent: e.parent.as_deref(),
            merged_into: e.merged.as_deref(),
            original: e.original,
            linked: e.linked,
            tag: &e.tag,
            work: self.works.get(e.seq as usize).map(String::as_str).unwrap_or(""),
            variant: &e.variant,
        })
    }

    /// 自己的显示名会并进人物键（`load` 里同样的条件）。
    fn spoken(e: &Entity) -> bool {
        e.kind != "work" && e.linked && e.parent.is_none()
    }

    /// `View.choose`（已装载快照的那条路径）。
    pub fn choose(&self, kind: &str, key: &str) -> Result<Choice> {
        let mut rows: Vec<&NameRow> =
            self.names.get(&(kind.to_owned(), key.to_owned())).map(|v| v.iter().collect()).unwrap_or_default();
        let mut keyed = Vec::with_capacity(rows.len());
        for r in rows.drain(..) {
            // 角色的 wiki preferred 只记录旁证状态。来源和用户的偏好继续管同语言取舍。
            let preferred = r.preferred && !(kind == "character" && r.source == "wiki");
            keyed.push(((!preferred, priority(&r.source)?, r.value.clone()), r));
        }
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        let mut best: BTreeMap<String, (String, String, bool)> = BTreeMap::new();
        for (_, r) in keyed {
            best.entry(r.lang.clone()).or_insert((r.value.clone(), r.source.clone(), r.preferred));
        }
        let reviewed = (kind == "character").then(|| crate::character_catalog::current().reviewed_name(key)).flatten();
        if let Some(label) = reviewed {
            if !best.get("zh").is_some_and(|v| matches!(v.1.as_str(), "user" | "bangumi" | "index")) {
                best.insert("zh".into(), (label.into(), "builtin".into(), false));
            }
        }
        if kind == "skin" {
            if let Some(zh) = skin_zh(key) {
                let replace = match best.get("zh") {
                    None => true,
                    Some(old) => priority(&old.1)? > priority("builtin")?,
                };
                if replace {
                    best.insert("zh".into(), (zh, "builtin".into(), false));
                }
            }
            let manual = LANGS.iter().filter_map(|lang| best.get(*lang)).find(|row| row.1 == "user");
            if let Some(manual) = manual.filter(|_| !legacy_skin_names()) {
                return Ok(Choice {
                    name: manual.0.clone(),
                    source: manual.1.clone(),
                    names: LANGS.map(|lang| best.get(lang).map(|row| row.0.clone())),
                });
            }
        }
        if kind == "work" {
            if let Some(w) = work_name(key) {
                if best.get("zh").map(|v| v.1.as_str()) != Some("user") {
                    best.insert("zh".into(), (w.to_owned(), "builtin".into(), false));
                }
            }
        }
        let langs: &[&str] = if kind == "skin" { &["zh", "en"] } else { &LANGS };
        // 同档选第一种语言。角色的 wiki 中文有旁证才提档。
        let mut chosen: Option<(usize, (String, String))> = None;
        for lang in langs.iter().filter(|l| **l != "en") {
            if let Some(v) = best.get(*lang) {
                let promoted = *lang == "zh"
                    && ((v.1 == "wiki" && (kind != "character" || v.2)) || (v.1 == "builtin" && reviewed.is_some()));
                let rank = priority(if promoted { "bangumi" } else { &v.1 })?;
                if chosen.as_ref().is_none_or(|(r, _)| rank < *r) {
                    chosen = Some((rank, (v.0.clone(), v.1.clone())));
                }
            }
        }
        let mut chosen =
            chosen.map(|(_, v)| v).unwrap_or_else(|| (readable(key, kind == "character"), "readable".into()));
        if kind == "work" {
            if let Some(w) = work_name(key) {
                if chosen.1 != "user" {
                    chosen = (w.to_owned(), "builtin".into());
                }
            }
        }
        Ok(Choice { name: chosen.0, source: chosen.1, names: LANGS.map(|l| best.get(l).map(|v| v.0.clone())) })
    }

    fn own_name(&self, e: &Entity) -> Choice {
        let rows = self.own.get(&e.id).map(Vec::as_slice).unwrap_or(&[]);
        let chosen = rows.iter().find(|r| r.source == "user").or(rows.first());
        Choice {
            name: chosen.map(|r| r.value.clone()).unwrap_or_else(|| "未命名角色".into()),
            source: chosen.map(|r| r.source.clone()).unwrap_or_else(|| "readable".into()),
            names: [None, None, None],
        }
    }

    fn base(&self, e: &Entity) -> Result<Choice> {
        let own_user = self.own.get(&e.id).is_some_and(|rows| rows.iter().any(|r| r.source == "user"));
        if !e.linked || e.original || e.kind == "oc" || own_user {
            return Ok(self.own_name(e));
        }
        let result = self.choose("character", &e.tag)?;
        if result.source == "readable" {
            for m in self.merged.get(&e.id).map(Vec::as_slice).unwrap_or(&[]) {
                let named = self.choose("character", &self.entities[m].tag)?;
                if named.source != "readable" {
                    return Ok(named);
                }
            }
        }
        Ok(result)
    }

    /// `View.entity`：返回（选名结果，tag）；身份不存在时 None。
    pub fn entity(&self, ident: &str, skin_only: bool) -> Result<Option<(Choice, String)>> {
        let Some(e) = self.entities.get(ident) else { return Ok(None) };
        if e.kind == "work" {
            let rows = self.own.get(ident).map(Vec::as_slice).unwrap_or(&[]);
            let user = rows.iter().find(|r| r.source == "user");
            let key = e.keys.get("qualifier").cloned().unwrap_or_default();
            let result = if let Some(u) = user {
                Choice {
                    name: u.value.clone(),
                    source: "user".into(),
                    names: LANGS.map(|l| rows.iter().find(|r| r.lang == l).map(|r| r.value.clone())),
                }
            } else if !key.is_empty() {
                self.choose("work", &key)?
            } else {
                self.own_name(e)
            };
            return Ok(Some((result, e.keys.get("danbooru").cloned().unwrap_or_default())));
        }
        let local = !e.linked || e.original || e.kind == "oc";
        let mut result;
        if local {
            result = self.own_name(e);
        } else if let Some(parent_id) = &e.parent {
            result = self.choose("skin", &e.variant)?;
            let parent = self.entities.get(parent_id);
            let own = self.own.get(&e.id).map_or(&[][..], Vec::as_slice);
            let manual = own.iter().find(|r| r.source == "user");
            if let Some(manual) = manual.filter(|_| !legacy_skin_names()) {
                result.name = manual.value.clone();
                result.source = manual.source.clone();
                for (i, lang) in LANGS.iter().enumerate() {
                    if let Some(row) = own.iter().find(|r| r.source == "user" && r.lang == *lang) {
                        result.names[i] = Some(row.value.clone());
                    }
                }
            } else if result.source == "readable" {
                let mut rows: Vec<&NameRow> = own.iter().filter(|r| r.lang == "zh" || r.lang == "ja").collect();
                let mut keyed = Vec::with_capacity(rows.len());
                for r in rows.drain(..) {
                    keyed.push(((r.lang != "zh", !r.preferred, priority(&r.source)?, r.value.clone()), r));
                }
                keyed.sort_by(|a, b| a.0.cmp(&b.0));
                let rows: Vec<&NameRow> = keyed.into_iter().map(|(_, r)| r).collect();
                if let (Some(first), Some(parent)) = (rows.first(), parent) {
                    if first.value != self.base(parent)?.name {
                        result = Choice {
                            name: first.value.clone(),
                            source: first.source.clone(),
                            names: LANGS.map(|l| rows.iter().find(|r| r.lang == l).map(|r| r.value.clone())),
                        };
                    }
                }
            }
            if !skin_only {
                let (base_name, base_source) = match parent {
                    Some(p) => {
                        let b = self.base(p)?;
                        (b.name, Some(b.source))
                    }
                    None => (String::new(), None),
                };
                result.name = format!("{base_name} · {}", result.name);
                if base_source.as_deref() == Some("model") && (result.source != "user" || legacy_skin_names()) {
                    result.source = "model".into();
                }
            }
        } else {
            result = self.base(e)?;
        }
        let tag = if skin_only {
            e.variant.clone()
        } else if !e.linked {
            String::new()
        } else {
            e.tag.clone()
        };
        Ok(Some((result, tag)))
    }

    /// 投影的文字键范围：人物（非皮肤、非作品）的 tag、作品文字（`entities.work` 与 qualifier 链接）、皮肤 variant。
    pub fn choice_keys(&self, c: &Connection) -> Result<BTreeSet<(String, String)>> {
        let mut keys = BTreeSet::new();
        for e in self.entities.values() {
            if !e.tag.is_empty() {
                keys.insert(("character".to_owned(), e.tag.clone()));
            }
            if !e.variant.is_empty() {
                keys.insert(("skin".to_owned(), e.variant.clone()));
            }
        }
        for row in c
            .prepare(&format!(
                "SELECT work FROM {ENTITIES} WHERE work <> ''
                 UNION SELECT l.key FROM {LINKS} l WHERE l.core_system = 'qualifier'"
            ))?
            .query_map([], |r| r.get::<_, String>(0))?
        {
            keys.insert(("work".to_owned(), row?));
        }
        Ok(keys)
    }
}

pub const TABLES: &str = "
CREATE TABLE IF NOT EXISTS term_name_display(
 entity TEXT NOT NULL, skin_only INTEGER NOT NULL CHECK(skin_only IN (0,1)),
 name TEXT NOT NULL, name_source TEXT NOT NULL, zh TEXT, ja TEXT, en TEXT, tag TEXT NOT NULL,
 PRIMARY KEY(entity, skin_only)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS term_name_choice(
 kind TEXT NOT NULL, key TEXT NOT NULL,
 name TEXT NOT NULL, name_source TEXT NOT NULL, zh TEXT, ja TEXT, en TEXT,
 PRIMARY KEY(kind, key)) WITHOUT ROWID;
";

/// 整表重建两张投影表（影子阶段：只读名字的兼容表，写核心自己的两张表）。
pub fn rebuild(c: &Connection) -> Result<(usize, usize)> {
    let view = View::load(c)?;
    c.execute_batch("DELETE FROM term_name_display; DELETE FROM term_name_choice;")?;
    let mut put = c.prepare(
        "INSERT INTO term_name_display(entity,skin_only,name,name_source,zh,ja,en,tag) VALUES (?,?,?,?,?,?,?,?)",
    )?;
    let mut shown = 0;
    for id in view.ids() {
        for skin_only in [false, true] {
            if let Some((ch, tag)) = view.entity(id, skin_only)? {
                let [zh, ja, en] = &ch.names;
                put.execute(params![id, skin_only as i64, ch.name, ch.source, zh, ja, en, tag])?;
                shown += 1;
            }
        }
    }
    let mut put =
        c.prepare("INSERT INTO term_name_choice(kind,key,name,name_source,zh,ja,en) VALUES (?,?,?,?,?,?,?)")?;
    let keys = view.choice_keys(c)?;
    for (kind, key) in &keys {
        let ch = view.choose(kind, key)?;
        let [zh, ja, en] = &ch.names;
        put.execute(params![kind, key, ch.name, ch.source, zh, ja, en])?;
    }
    Ok((shown, keys.len()))
}

/// 名字、身份或链接变化时只写依赖闭包（#103 A3、#104 A4，见 `Scope`）：只载入闭包和选名要读的行，重写受影响的
/// 投影行，删掉已经不存在的身份和库里不再出现的键。闭包碰到空 tag 或空 variant 的键、或者大到全库的八分之一以上时
/// 退回整表重建。
pub fn rebuild_changed(c: &Connection) -> Result<Maintained> {
    match Scope::collect(c)? {
        Some(scope) => {
            let scoped = scope.load(c)?;
            write_changed(c, &scoped, &scope)?;
            // 事务后面若再刷新一次（中途的历史文案之后又有改动），这次写过的行也要按那时的状态重算。
            let mut id = c.prepare_cached(
                "INSERT INTO names_scope_ids SELECT ?1 WHERE NOT EXISTS(SELECT 1 FROM names_scope_ids WHERE id=?1)",
            )?;
            for i in &scope.ids {
                id.execute([i])?;
            }
            let mut key = c.prepare_cached(
                "INSERT INTO names_scope_keys SELECT ?1,?2
                 WHERE NOT EXISTS(SELECT 1 FROM names_scope_keys WHERE kind=?1 AND key=?2)",
            )?;
            for (kind, k) in &scope.keys {
                key.execute([kind, k])?;
            }
            Ok(if scope.structure { Maintained::Structure } else { Maintained::Names })
        }
        None => rebuild(c).map(|_| Maintained::Whole),
    }
}

/// `rebuild_changed` 走了哪条路（对照核对按它分别计数）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Maintained {
    /// 局部维护，本事务只改了名字行。
    Names,
    /// 局部维护，本事务改过身份或链接。
    Structure,
    /// 退回整表重建。
    Whole,
}

/// 把闭包里的身份和键按 `view` 重写进两张投影表：身份不存在了就删掉它的两行，键在库里不再出现就删掉它的行。
fn write_changed(c: &Connection, view: &View, scope: &Scope) -> Result<(usize, usize)> {
    let mut put = c.prepare(
        "INSERT OR REPLACE INTO term_name_display(entity,skin_only,name,name_source,zh,ja,en,tag)
        VALUES (?,?,?,?,?,?,?,?)",
    )?;
    let mut gone = c.prepare("DELETE FROM term_name_display WHERE entity=?")?;
    let mut shown = 0;
    for id in &scope.ids {
        if !view.entities.contains_key(id) {
            gone.execute([id])?;
            continue;
        }
        for skin in [false, true] {
            if let Some((ch, tag)) = view.entity(id, skin)? {
                let [zh, ja, en] = &ch.names;
                put.execute(params![id, skin as i64, ch.name, ch.source, zh, ja, en, tag])?;
                shown += 1;
            }
        }
    }
    let mut put = c.prepare(
        "INSERT OR REPLACE INTO term_name_choice(kind,key,name,name_source,zh,ja,en) VALUES (?,?,?,?,?,?,?)",
    )?;
    let mut gone = c.prepare("DELETE FROM term_name_choice WHERE kind=? AND key=?")?;
    let mut chosen = 0;
    for (kind, key) in &scope.keys {
        if !scope.existing.contains(&(kind.clone(), key.clone())) {
            gone.execute([kind, key])?;
            continue;
        }
        let ch = view.choose(kind, key)?;
        let [zh, ja, en] = &ch.names;
        put.execute(params![kind, key, ch.name, ch.source, zh, ja, en])?;
        chosen += 1;
    }
    Ok((shown, chosen))
}

/// 名字变化时局部维护投影要读的范围（#103 A3）。`collect` 从本事务记下的身份和键出发，用按索引的定向查询求依赖
/// 闭包（身份 `ids`、文字键 `keys`），再补上选名要读的身份：父身份、并入者、与它们同 tag 的身份。`load` 只为这些
/// 身份建快照，`entity` / `choose` 照旧。`term_tree` 上没有索引的三列（皮肤 `variant`、`merged_into`、作品文字
/// `work_text`）各用一条扫描语句，只在闭包里有对应的键或身份时执行（#38 2026-10-12 用户决定第 2 条；variant 同样只
/// 记录、不加索引，2026-10-13）。
struct Scope {
    ids: BTreeSet<String>,
    keys: BTreeSet<(String, String)>,
    /// `keys` 里库中出现过的（`choice_keys` 的范围）。
    existing: BTreeSet<(String, String)>,
    /// 要载入的身份与它们的 `seq`。
    read: BTreeMap<String, (i64, Entity)>,
    /// 本事务改过身份或链接。
    structure: bool,
}

/// 按 id 取过的身份（含当前链接）；None = 库里没有。
#[derive(Default)]
struct Fetched(HashMap<String, Option<(i64, Entity)>>);

impl Fetched {
    fn get(&mut self, c: &Connection, id: &str) -> Result<Option<&Entity>> {
        if !self.0.contains_key(id) {
            let found = fetch(c, id)?;
            self.0.insert(id.to_owned(), found);
        }
        Ok(self.0[id].as_ref().map(|(_, e)| e))
    }

    /// 当前 tag 恰好是 `tag` 的身份。tag 只可能来自这几种链接（`linked_tag`），按 `links` 主键找候选，再按优先级复核。
    fn tagged(&mut self, c: &Connection, tag: &str) -> Result<Vec<String>> {
        let candidates: Vec<String> = c
            .prepare_cached(&format!(
                "SELECT l.entity_id FROM {LINKS} l
                 WHERE l.core_system IN ('danbooru', 'wd_character', 'photos_album', 'ccip') AND l.key = ?1 AND l.current = 1
                 UNION
                 SELECT l.entity_id FROM {LINKS} l
                 WHERE l.core_system = 'ccip_base' AND l.current = 1
                   AND CASE WHEN json_valid(l.key) THEN json_extract(l.key, '$[0]') END = ?1"
            ))?
            .query_map([tag], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut found = Vec::new();
        for id in candidates {
            if self.get(c, &id)?.is_some_and(|e| e.tag == tag) {
                found.push(id);
            }
        }
        Ok(found)
    }

    /// 当前 qualifier 链接是 `key` 的身份。
    fn qualified(&mut self, c: &Connection, key: &str) -> Result<Vec<String>> {
        let candidates: Vec<String> = c
            .prepare_cached(&format!(
                "SELECT l.entity_id FROM {LINKS} l WHERE l.core_system = 'qualifier' AND l.key = ? AND l.current = 1"
            ))?
            .query_map([key], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut found = Vec::new();
        for id in candidates {
            if self.get(c, &id)?.is_some_and(|e| e.keys.get("qualifier").is_some_and(|k| k == key)) {
                found.push(id);
            }
        }
        Ok(found)
    }
}

/// 按 `links_sql`（参数是身份 id，返回当前链接的 system,key，按 `added_at,key` 排）补上链接、tag。
fn link_up(c: &Connection, e: &mut Entity, links_sql: &str) -> Result<()> {
    for row in
        c.prepare_cached(links_sql)?.query_map([&e.id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
    {
        let (system, key) = row?;
        e.keys.entry(system).or_insert(key);
    }
    e.linked = !e.keys.is_empty();
    e.tag = linked_tag(&e.keys)?;
    Ok(())
}

/// 一个身份的当前链接，按 `linked_tag` 要的先后。
static LINKS_SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!("SELECT l.system, l.key FROM {LINKS} l WHERE l.entity_id = ? AND l.current = 1 ORDER BY l.added_at, l.key")
});

fn current_work(c: &Connection, id: &str) -> Result<String> {
    Ok(c.prepare_cached(&format!("SELECT coalesce(work,'') FROM {ENTITIES} WHERE id=?"))?
        .query_row([id], |r| r.get(0))?)
}

/// 事务开始时的身份与它的 `work`（`legacy_names::track` 存下的；没存就是没改过，取现在的）；那时没有这个
/// 身份时 None。
fn old_state(c: &Connection, id: &str) -> Result<Option<(String, Entity)>> {
    use rusqlite::OptionalExtension;
    let captured = c
        .prepare_cached(
            "SELECT present,kind,parent_id,variant,merged_into,coalesce(original,0),coalesce(work,'')
             FROM names_old_entities WHERE id=?",
        )?
        .query_row([id], |r| {
            Ok((
                r.get::<_, i64>(0)? != 0,
                Entity {
                    id: id.to_owned(),
                    kind: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    parent: r.get(2)?,
                    variant: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    merged: r.get(4)?,
                    original: r.get::<_, i64>(5)? != 0,
                    keys: BTreeMap::new(),
                    linked: false,
                    tag: String::new(),
                    seq: 0,
                },
                r.get::<_, String>(6)?,
            ))
        })
        .optional()?;
    let (mut e, work) = match captured {
        Some((false, ..)) => return Ok(None),
        Some((true, e, work)) => (e, work),
        None => match fetch(c, id)? {
            // 行没改过：只有链接改过时才会走到这里，链接下面按存下的重新补。
            Some((_, mut e)) => {
                e.keys.clear();
                (e, current_work(c, id)?)
            }
            None => return Ok(None),
        },
    };
    let seen: bool = c
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM names_old_links_seen WHERE id=?)")?
        .query_row([id], |r| r.get(0))?;
    let links = if seen {
        "SELECT system,key FROM names_old_links WHERE entity_id=? AND current=1 ORDER BY added_at,key"
    } else {
        LINKS_SQL.as_str()
    };
    link_up(c, &mut e, links)?;
    Ok(Some((work, e)))
}

/// 一个身份和它的当前链接，与 `View::load` 里同一个身份逐项相同（`seq` 由 `Scope::load` 重新编）。
fn fetch(c: &Connection, id: &str) -> Result<Option<(i64, Entity)>> {
    use rusqlite::OptionalExtension;
    let found = c
        .prepare_cached(&format!(
            "SELECT seq,id,kind,parent_id,variant,merged_into,coalesce(original,0) FROM {ENTITIES} WHERE id=?"
        ))?
        .query_row([id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                Entity {
                    id: r.get(1)?,
                    kind: r.get(2)?,
                    parent: r.get(3)?,
                    variant: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    merged: r.get(5)?,
                    original: r.get::<_, i64>(6)? != 0,
                    keys: BTreeMap::new(),
                    linked: false,
                    tag: String::new(),
                    seq: 0,
                },
            ))
        })
        .optional()?;
    let Some((rowid, mut e)) = found else { return Ok(None) };
    link_up(c, &mut e, &LINKS_SQL)?;
    Ok(Some((rowid, e)))
}

fn json_list<'a>(items: impl IntoIterator<Item = &'a str>) -> Result<String> {
    serde_json::to_string(&items.into_iter().collect::<Vec<_>>()).map_err(|e| Error::Invalid(e.to_string()))
}

impl Scope {
    /// 本事务记下的身份和键的依赖闭包与读取范围。闭包碰到空 tag 的人物键或空 variant 的皮肤键时返回 None：库里
    /// 所有没有 tag（没有 variant）的身份都依赖它们，调用方改为整表重建。
    fn collect(c: &Connection) -> Result<Option<Scope>> {
        // 种子：改过名字的身份和键、改过行或链接的身份（`legacy_names::track`），以及本事务里先前局部维护时写过的
        // 身份和键（事务中途为历史文案刷新过投影，那时写下的行也要按最终状态重算或删除）。
        let mut new_ids: Vec<String> = c
            .prepare(
                "SELECT id FROM names_dirty_entities UNION SELECT id FROM names_old_entities
                 UNION SELECT id FROM names_old_links_seen UNION SELECT id FROM names_scope_ids",
            )?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut new_keys: Vec<(String, String)> = c
            .prepare("SELECT kind,key FROM names_dirty_keys UNION SELECT kind,key FROM names_scope_keys")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        // 改过行或链接的身份（#104 A4）：变化前、变化后两侧用到的键都要重算（键可能新出现，也可能不再出现），变化前
        // 它并入的身份也要重算（那个身份可能借过它的人物键）。
        let moved: Vec<String> = c
            .prepare("SELECT id FROM names_old_entities UNION SELECT id FROM names_old_links_seen")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for id in &moved {
            let now = match fetch(c, id)? {
                Some((_, e)) => Some((current_work(c, id)?, e)),
                None => None,
            };
            for (work, e) in [old_state(c, id)?, now].into_iter().flatten() {
                if View::spoken(&e) && e.tag.is_empty() {
                    return Ok(None);
                }
                if !e.tag.is_empty() {
                    new_keys.push(("character".into(), e.tag.clone()));
                }
                if !e.variant.is_empty() {
                    new_keys.push(("skin".into(), e.variant.clone()));
                }
                if let Some(q) = e.keys.get("qualifier") {
                    new_keys.push(("work".into(), q.clone()));
                }
                if !work.is_empty() {
                    new_keys.push(("work".into(), work));
                }
                if let Some(m) = e.merged.as_ref().filter(|m| e.linked && !m.is_empty()) {
                    new_ids.push(m.clone());
                }
            }
        }
        let mut f = Fetched::default();
        let mut ids: BTreeSet<String> = BTreeSet::new();
        let mut keys: BTreeSet<(String, String)> = BTreeSet::new();
        let mut existing: BTreeSet<(String, String)> = BTreeSet::new();
        let mut children = c.prepare_cached(&format!("SELECT id FROM {ENTITIES} WHERE parent_id=?"))?;
        // 不动点：身份 → 它的人物键（会把显示名并进人物键的身份）、子身份、它并入的身份（有链接时）；键 → tag、
        // variant、qualifier 是这个键的身份。
        while !new_ids.is_empty() || !new_keys.is_empty() {
            let mut next_ids = Vec::new();
            for id in std::mem::take(&mut new_ids) {
                if !ids.insert(id.clone()) {
                    continue;
                }
                let Some(e) = f.get(c, &id)? else { continue };
                if View::spoken(e) {
                    if e.tag.is_empty() {
                        return Ok(None);
                    }
                    new_keys.push(("character".into(), e.tag.clone()));
                }
                if let Some(m) = e.merged.as_ref().filter(|m| e.linked && !m.is_empty()) {
                    next_ids.push(m.clone());
                }
                for child in children.query_map([&id], |r| r.get::<_, String>(0))? {
                    next_ids.push(child?);
                }
            }
            let mut skins: Vec<String> = Vec::new();
            for key in std::mem::take(&mut new_keys) {
                if keys.contains(&key) {
                    continue;
                }
                match key.0.as_str() {
                    "character" => {
                        if key.1.is_empty() {
                            return Ok(None);
                        }
                        let found = f.tagged(c, &key.1)?;
                        if !found.is_empty() {
                            existing.insert(key.clone());
                        }
                        next_ids.extend(found);
                    }
                    "skin" => {
                        if key.1.is_empty() {
                            return Ok(None);
                        }
                        skins.push(key.1.clone());
                    }
                    "work" => next_ids.extend(f.qualified(c, &key.1)?),
                    _ => {}
                }
                keys.insert(key);
            }
            if !skins.is_empty() {
                // 扫描 `term_tree`（variant 没有索引）。
                let list = json_list(skins.iter().map(String::as_str))?;
                for row in c
                    .prepare_cached(&format!(
                        "SELECT id,variant FROM {ENTITIES} WHERE variant IN (SELECT value FROM json_each(?1))"
                    ))?
                    .query_map([&list], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                {
                    let (id, variant) = row?;
                    existing.insert(("skin".into(), variant));
                    next_ids.push(id);
                }
            }
            new_ids = next_ids;
        }
        let works: Vec<&str> = keys.iter().filter(|(k, _)| k == "work").map(|(_, v)| v.as_str()).collect();
        if !works.is_empty() {
            // 作品文字的范围与 `choice_keys` 相同：身份的 `work`（没有索引，扫描）与任意 qualifier 链接。
            let list = json_list(works.iter().copied())?;
            for row in c
                .prepare_cached(&format!(
                    "SELECT DISTINCT work FROM {ENTITIES} WHERE work<>'' AND work IN (SELECT value FROM json_each(?1))"
                ))?
                .query_map([&list], |r| r.get::<_, String>(0))?
            {
                existing.insert(("work".into(), row?));
            }
            let mut linked = c.prepare_cached(&format!(
                "SELECT EXISTS(SELECT 1 FROM {LINKS} l WHERE l.core_system = 'qualifier' AND l.key = ?)"
            ))?;
            for key in works {
                if linked.query_row([key], |r| r.get::<_, bool>(0))? {
                    existing.insert(("work".into(), key.to_owned()));
                }
            }
        }
        // 闭包占到全库的八分之一以上（例如一次导入很多身份）时，逐个查询不如整表重建。
        if ids.len() > 256 {
            let total: i64 = c.query_row("SELECT count(*) FROM term_tree", [], |r| r.get(0))?;
            if ids.len() as i64 * 8 > total {
                return Ok(None);
            }
        }
        // 选名还要读的身份：皮肤的父身份（`base`），以及这些身份的并入者（`base` 在没有名字时借并入者的人物键）。
        let mut want: BTreeSet<String> = BTreeSet::new();
        let mut bases: BTreeSet<String> = BTreeSet::new();
        for id in &ids {
            let Some(e) = f.get(c, id)? else { continue };
            let parent = e.parent.clone();
            want.insert(id.clone());
            bases.insert(id.clone());
            if let Some(p) = parent {
                if let Some(pe) = f.get(c, &p)? {
                    if pe.linked && pe.tag.is_empty() {
                        return Ok(None);
                    }
                    want.insert(p.clone());
                    bases.insert(p);
                }
            }
        }
        let mut tags: BTreeSet<String> = BTreeSet::new();
        if !bases.is_empty() {
            // 扫描 `term_tree`（merged_into 没有索引）。
            let list = json_list(bases.iter().map(String::as_str))?;
            let members: Vec<String> = c
                .prepare_cached(&format!(
                    "SELECT id FROM {ENTITIES} WHERE merged_into IN (SELECT value FROM json_each(?1))"
                ))?
                .query_map([&list], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for m in members {
                let Some(e) = f.get(c, &m)? else { continue };
                if e.linked {
                    if e.tag.is_empty() {
                        return Ok(None);
                    }
                    tags.insert(e.tag.clone());
                    want.insert(m);
                }
            }
        }
        for id in &bases {
            if let Some(e) = f.get(c, id)?.filter(|e| !e.tag.is_empty()) {
                tags.insert(e.tag.clone());
            }
        }
        // 人物键下的名字由同 tag 的全部身份拼成；闭包里的人物键已经找过一遍。
        for tag in tags {
            if !keys.contains(&("character".to_owned(), tag.clone())) {
                want.extend(f.tagged(c, &tag)?);
            }
        }
        let mut read = BTreeMap::new();
        for id in want {
            if let Some(Some(x)) = f.0.remove(&id) {
                read.insert(id, x);
            }
        }
        Ok(Some(Scope { ids, keys, existing, read, structure: !moved.is_empty() }))
    }

    /// 只含 `read` 的快照：每个身份、它的名字和链接与整库 `View::load` 里逐项相同；人物键、皮肤键、作品键下的名字
    /// 只装这些身份用得到的键，每个键装全（同 tag 的身份都在 `read` 里）。
    fn load(&self, c: &Connection) -> Result<View> {
        let mut order: Vec<&(i64, Entity)> = self.read.values().collect();
        order.sort_by_key(|(rowid, _)| *rowid);
        let mut entities: BTreeMap<String, Entity> = BTreeMap::new();
        for (seq, (_, e)) in order.iter().enumerate() {
            let mut e = e.clone();
            e.seq = u32::try_from(seq).map_err(|_| Error::Invalid("身份太多".into()))?;
            entities.insert(e.id.clone(), e);
        }
        let mut merged: HashMap<String, Vec<String>> = HashMap::new();
        for e in entities.values() {
            if let Some(m) = e.merged.as_ref().filter(|m| e.linked && !m.is_empty()) {
                merged.entry(m.clone()).or_default().push(e.id.clone());
            }
        }
        let mut own: HashMap<String, Vec<NameRow>> = HashMap::new();
        let mut rows = c.prepare_cached(
            "SELECT lang, value, source, preferred
             FROM term_names
             WHERE term_id = (SELECT id FROM terms WHERE key = ?) AND role = 'display'
             ORDER BY preferred DESC, value, seq",
        )?;
        for id in entities.keys() {
            let found: Vec<NameRow> = rows
                .query_map([id], |r| {
                    Ok(NameRow {
                        lang: r.get(0)?,
                        value: r.get(1)?,
                        source: r.get(2)?,
                        preferred: r.get::<_, i64>(3)? != 0,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            if !found.is_empty() {
                own.insert(id.clone(), found);
            }
        }
        let mut wanted: BTreeSet<(String, String)> = self.keys.clone();
        for e in entities.values() {
            if !e.tag.is_empty() {
                wanted.insert(("character".into(), e.tag.clone()));
            }
            if !e.variant.is_empty() {
                wanted.insert(("skin".into(), e.variant.clone()));
            }
            if let Some(q) = e.keys.get("qualifier") {
                wanted.insert(("work".into(), q.clone()));
            }
        }
        let mut names: HashMap<(String, String), Vec<NameRow>> = HashMap::new();
        let has_display: bool =
            c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='term_key_names')", [], |r| r.get(0))?;
        if has_display {
            let mut display = c.prepare_cached(
                "SELECT lang,source,value FROM term_key_names WHERE kind=? AND key=? AND source<>'danbooru'",
            )?;
            for (kind, key) in wanted {
                let found: Vec<NameRow> = display
                    .query_map([&kind, &key], |r| {
                        Ok(NameRow { lang: r.get(0)?, source: r.get(1)?, value: r.get(2)?, preferred: false })
                    })?
                    .collect::<rusqlite::Result<_>>()?;
                if !found.is_empty() {
                    names.insert((kind, key), found);
                }
            }
        }
        for (_, e) in &order {
            if View::spoken(e) {
                if let Some(rows) = own.get(&e.id) {
                    names.entry(("character".into(), e.tag.clone())).or_default().extend(rows.iter().cloned());
                }
            }
        }
        Ok(View { entities, merged, names, own, works: Vec::new() })
    }
}

/// 一个文字键的选名（#105 A5b），与 `View::load(c)?.choose(kind, key)` 逐项相同，只读这个键用得到的行：它在
/// `term_key_names` 里的行；人物键另加 tag 是它、会把显示名并进人物键的身份（`View::spoken`）自己的显示名。`choose`
/// 的结果与行的先后无关（排序键含来源和值，并列的行给出同一个名字），所以不必按整库载入的顺序读。
///
/// 读的是选名的输入，不是显示投影：在事务里调用时读到的就是本事务待提交的名字（名字更新器的模型预演），也覆盖投影
/// 不收录的键（库里没出现过的作品文字、空 tag 的人物键）。空 tag 的人物键要找所有没有 tag 的身份，扫描 `links`
/// （只记录，不加索引，同 #38 2026-10-12 用户决定第 2 条）。
pub fn choose_key(c: &Connection, kind: &str, key: &str) -> Result<Choice> {
    let mut rows: Vec<NameRow> = Vec::new();
    let has_display: bool =
        c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='term_key_names')", [], |r| r.get(0))?;
    if has_display {
        let mut display = c.prepare_cached(
            "SELECT lang,source,value FROM term_key_names WHERE kind=? AND key=? AND source<>'danbooru'",
        )?;
        for row in display.query_map([kind, key], |r| {
            Ok(NameRow { lang: r.get(0)?, source: r.get(1)?, value: r.get(2)?, preferred: false })
        })? {
            rows.push(row?);
        }
    }
    if kind == "character" {
        let mut f = Fetched::default();
        let candidates = if key.is_empty() { untagged(c)? } else { f.tagged(c, key)? };
        let mut own = c.prepare_cached(
            "SELECT lang, value, source, preferred
             FROM term_names
             WHERE term_id = (SELECT id FROM terms WHERE key = ?) AND role = 'display'
             ORDER BY preferred DESC, value, seq",
        )?;
        for id in candidates {
            if !f.get(c, &id)?.is_some_and(|e| View::spoken(e) && e.tag == key) {
                continue;
            }
            for row in own.query_map([&id], |r| {
                Ok(NameRow {
                    lang: r.get(0)?,
                    value: r.get(1)?,
                    source: r.get(2)?,
                    preferred: r.get::<_, i64>(3)? != 0,
                })
            })? {
                rows.push(row?);
            }
        }
    }
    let mut names = HashMap::new();
    if !rows.is_empty() {
        names.insert((kind.to_owned(), key.to_owned()), rows);
    }
    let view =
        View { entities: BTreeMap::new(), merged: HashMap::new(), names, own: HashMap::new(), works: Vec::new() };
    let choice = view.choose(kind, key)?;
    if crate::legacy_names::verifying() {
        verify_choice(c, kind, key, &choice)?;
        crate::legacy_names::record_verified("choice")?;
    }
    Ok(choice)
}

/// 对照核对：`choice` 与同一连接上整库载入后的选名逐项相同（不计入 `whole_loads`）。
pub fn verify_choice(c: &Connection, kind: &str, key: &str, choice: &Choice) -> Result<()> {
    let full = View::load_uncounted(c, false)?.choose(kind, key)?;
    if *choice != full {
        return Err(Error::Conflict(format!(
            "按键选名与整库载入不一致（{kind} {key}）：{choice} ≠ {full}",
            key = format_args!("{key:?}"),
            choice = format_args!("{choice:?}"),
            full = format_args!("{full:?}")
        )));
    }
    Ok(())
}

/// 可能没有 tag 的人物身份（`choose_key` 的空 tag 人物键）：有当前链接，且没有 tag 来源（`linked_tag` 的前四种
/// 链接）里的非空键，或者持有其中某种链接的空键（同一种链接按 `added_at,key` 排在前面的空键会遮住后面的非空键；
/// 每种链接的空键全库至多一条）。调用方再逐个按 `linked_tag` 复核。
fn untagged(c: &Connection) -> Result<Vec<String>> {
    let tag_systems = "('danbooru', 'wd_character', 'photos_album', 'ccip')";
    Ok(c.prepare_cached(&format!(
        "SELECT id FROM {ENTITIES} WHERE kind <> 'work' AND parent_id IS NULL AND id IN (
           SELECT l.entity_id FROM {LINKS} l WHERE l.current = 1
           EXCEPT
           SELECT l.entity_id FROM {LINKS} l
           WHERE l.core_system IN {tag_systems} AND l.key <> '' AND l.current = 1
           UNION
           SELECT l.entity_id FROM {LINKS} l
           WHERE l.core_system IN {tag_systems} AND l.key = '' AND l.current = 1)
         ORDER BY id"
    ))?
    .query_map([], |r| r.get(0))?
    .collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 按键给身份写显示名，按这里的顺序（`seq` 随之递增，与 `character_names::put_name` 的分配方式相同）。
    fn named(rows: &[(&str, &str, &str, &str, i64)]) -> String {
        rows.iter()
            .map(|(key, lang, value, source, preferred)| {
                format!(
                    "INSERT INTO term_names(term_id,lang,value,source,role,preferred,position,seq) VALUES
                     ((SELECT id FROM terms WHERE key='{key}'),'{lang}','{value}','{source}','display',{preferred},0,
                      (SELECT coalesce(max(seq),0)+1 FROM term_names));"
                )
            })
            .collect()
    }

    /// 只有名称的输入，名称已在核心里，`legacy_names::track` 才会记录变化。
    fn library() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
             INSERT INTO meta VALUES ('terms_names','core'),('names_version','0'),('names_projected','0');
             CREATE TABLE term_links(system TEXT,key TEXT,term_id INTEGER,url TEXT NOT NULL DEFAULT '',
              source TEXT NOT NULL DEFAULT '',current INTEGER NOT NULL DEFAULT 1,added_at TEXT,PRIMARY KEY(system,key));
             CREATE TABLE terms(id INTEGER PRIMARY KEY,key TEXT NOT NULL UNIQUE,kind TEXT NOT NULL);
             CREATE TABLE term_tree(term_id INTEGER PRIMARY KEY,parent_id TEXT,merged_into TEXT,work_id TEXT,
              work_text TEXT NOT NULL DEFAULT '',variant TEXT NOT NULL DEFAULT '',artist TEXT NOT NULL DEFAULT '',
              label_only INTEGER NOT NULL DEFAULT 0,mascot INTEGER NOT NULL DEFAULT 0,mascot_origin TEXT,
              original INTEGER NOT NULL DEFAULT 0,seq INTEGER);
             -- The tests write identities through this view: a term and its row in `term_tree`, as the commands do
             -- (`character_entities::create`).
             CREATE VIEW identities(id,kind,parent_id,variant,merged_into,original,work) AS
              SELECT t.key,t.kind,r.parent_id,r.variant,r.merged_into,r.original,r.work_text
              FROM term_tree r JOIN terms t ON t.id=r.term_id;
             CREATE TRIGGER identities_insert INSTEAD OF INSERT ON identities BEGIN
               INSERT INTO terms(key,kind) VALUES (NEW.id,NEW.kind);
               INSERT INTO term_tree(term_id,parent_id,variant,merged_into,original,work_text,seq)
                SELECT id,NEW.parent_id,NEW.variant,NEW.merged_into,NEW.original,NEW.work,
                 (SELECT coalesce(max(seq),0)+1 FROM term_tree)
                FROM terms WHERE key=NEW.id;
             END;
             CREATE TRIGGER identities_update INSTEAD OF UPDATE ON identities BEGIN
               UPDATE terms SET kind=NEW.kind WHERE key=OLD.id AND kind IS NOT NEW.kind;
               UPDATE term_tree SET parent_id=NEW.parent_id,variant=NEW.variant,merged_into=NEW.merged_into,
                original=NEW.original,work_text=NEW.work
               WHERE term_id=(SELECT id FROM terms WHERE key=OLD.id);
             END;
             CREATE TRIGGER identities_delete INSTEAD OF DELETE ON identities BEGIN
               DELETE FROM term_tree WHERE term_id=(SELECT id FROM terms WHERE key=OLD.id);
             END;
             CREATE TABLE term_names(term_id INTEGER,lang TEXT,value TEXT,source TEXT,role TEXT,preferred INTEGER NOT NULL DEFAULT 0,
              position INTEGER NOT NULL DEFAULT 0,seq INTEGER,PRIMARY KEY(term_id,role,lang,source,value));
             CREATE TABLE term_key_names(kind TEXT,key TEXT,lang TEXT,source TEXT,value TEXT,PRIMARY KEY(kind,key,lang,source,value));
             CREATE TABLE term_key_aliases(kind TEXT,key TEXT,value TEXT);
             INSERT INTO identities VALUES ('e1','character',NULL,'',NULL,0,'series_w'),('e2','character',NULL,'',NULL,0,''),
              ('s1','skin','e1','swimsuit_x',NULL,0,''),('w1','work',NULL,'',NULL,0,''),('m1','character',NULL,'','e1',0,'');
             INSERT INTO term_links(system,key,term_id,current,added_at) VALUES ('danbooru','synth_(series_w)',(SELECT id FROM terms WHERE key='e1'),1,'2026-01-01'),('wd_character','synth_(series_w)',(SELECT id FROM terms WHERE key='e2'),1,'2026-01-02'),
              ('danbooru','synth_swimsuit',(SELECT id FROM terms WHERE key='s1'),1,'2026-01-03'),('qualifier','series_w',(SELECT id FROM terms WHERE key='w1'),1,'2026-01-04'),
              ('danbooru','merged_tag',(SELECT id FROM terms WHERE key='m1'),1,'2026-01-05');
             INSERT INTO term_key_names VALUES ('skin','swimsuit_x','zh','index','皮肤名'),('work','series_w','zh','bangumi','作品名');",
        )
        .unwrap();
        // 在 (preferred,value) 上并列、`seq` 顺序与主键顺序不同的行。
        c.execute_batch(&named(&[
            ("e1", "zh", "同名", "index", 0),
            ("e1", "ja", "同名", "wiki", 0),
            ("e2", "en", "Other", "wiki", 0),
            ("m1", "zh", "并入名", "index", 0),
        ]))
        .unwrap();
        c
    }

    #[test]
    fn reviewed_wikidata_uses_old_tag_offline_and_preserves_user_names() {
        let c = library();
        let catalog = serde_json::json!({
            "format":crate::character_catalog::FORMAT, "revision":2,
            "columns":["id","tag","tags","parent","relation","action","variant","bangumi","pick","wikidata"],
            "characters":[[1,"current_name",["old_name"],null,null,null,null,null,null,"Q123"]],
            "works":[], "wikidata_names":{"Q123":"示例椛"}
        });
        let catalog = crate::character_catalog::Catalog::parse(&catalog.to_string()).unwrap();
        crate::character_catalog::override_for_thread(Some(Box::leak(Box::new(catalog))));
        c.execute_batch(
            "INSERT INTO term_key_names VALUES ('character','old_name','ja','index','日本語'),
            ('character','old_name','zh','wiki','误选别名');",
        )
        .unwrap();
        let choice = choose_key(&c, "character", "old_name").unwrap();
        assert_eq!((choice.name.as_str(), choice.source.as_str()), ("示例椛", "builtin"));
        assert_eq!(choice.names[1].as_deref(), Some("日本語"));
        c.execute_batch("INSERT INTO term_key_names VALUES ('character','old_name','zh','user','我改的名');").unwrap();
        let choice = choose_key(&c, "character", "old_name").unwrap();
        assert_eq!((choice.name.as_str(), choice.source.as_str()), ("我改的名", "user"));
        c.execute("UPDATE term_links SET key='old_name' WHERE system='danbooru' AND key='synth_(series_w)'", [])
            .unwrap();
        assert_eq!(crate::names_search::hits(&c, "示例椛").unwrap().get("e1"), Some(&2));
        crate::character_catalog::override_for_thread(None);
    }

    fn projection(c: &Connection) -> Vec<Vec<rusqlite::types::Value>> {
        let mut all = Vec::new();
        for sql in [
            "SELECT * FROM term_name_display ORDER BY entity,skin_only",
            "SELECT * FROM term_name_choice ORDER BY kind,key",
        ] {
            let mut st = c.prepare(sql).unwrap();
            let n = st.column_count();
            all.extend(
                st.query_map([], |r| (0..n).map(|i| r.get(i)).collect::<rusqlite::Result<Vec<_>>>())
                    .unwrap()
                    .map(|r| r.unwrap()),
            );
        }
        all
    }

    /// `library` 再加上：共用一个 variant 的皮肤、借用成员名称的无名角色、一个 ccip_base 标签，
    /// 以及两个没有标签的已链接角色；投影已完整构建。
    fn projected() -> Connection {
        let c = library();
        c.execute_batch(TABLES).unwrap();
        c.execute_batch(
            "INSERT INTO identities VALUES ('s2','skin','e2','swimsuit_x',NULL,0,''),('e3','character',NULL,'',NULL,0,''),
              ('m2','character',NULL,'','e3',0,''),('e4','character',NULL,'',NULL,0,''),
              ('b1','character',NULL,'',NULL,0,''),('b2','character',NULL,'',NULL,0,''),('w2','work',NULL,'',NULL,0,'series_w'),
              ('s3','skin','e3','',NULL,0,'');
             INSERT INTO term_links(system,key,term_id,current,added_at) VALUES ('danbooru','synth_swimsuit_2',(SELECT id FROM terms WHERE key='s2'),1,'2026-01-06'),('danbooru','lonely',(SELECT id FROM terms WHERE key='e3'),1,'2026-01-07'),
              ('danbooru','member_tag',(SELECT id FROM terms WHERE key='m2'),1,'2026-01-08'),('ccip_base','[\"ccip_tag\",1]',(SELECT id FROM terms WHERE key='e4'),1,'2026-01-09'),
              ('bangumi','b1',(SELECT id FROM terms WHERE key='b1'),1,'2026-01-10'),('danbooru','s3_tag',(SELECT id FROM terms WHERE key='s3'),1,'2026-01-13'),
              ('bangumi','b2',(SELECT id FROM terms WHERE key='b2'),1,'2026-01-11'),('danbooru','old_tag',(SELECT id FROM terms WHERE key='e4'),0,'2026-01-12');
             INSERT INTO term_key_names VALUES ('character','member_tag','ja','wiki','メンバー');",
        )
        .unwrap();
        c.execute_batch(&named(&[
            ("m2", "zh", "成员名", "index", 0),
            ("e4", "zh", "卡", "index", 0),
            ("b1", "zh", "无标签一", "index", 0),
            ("b2", "ja", "無標籤二", "index", 0),
        ]))
        .unwrap();
        rebuild(&c).unwrap();
        c
    }

    #[test]
    fn scoped_projection_matches_a_whole_rebuild() {
        let mut c = projected();
        let before = projection(&c);
        let cases: [(&str, Option<&[&str]>); 9] = [
            // 角色自己的名称：同标签的身份（经 wd 链到的 e2）、它的皮肤和角色键。
            (
                "UPDATE term_names SET value='改名' WHERE term_id=(SELECT id FROM terms WHERE key='e1') AND lang='zh'",
                Some(&["e1", "e2", "m1", "s1", "s2"]),
            ),
            // 成员的名称：它并入的无名角色借用这个名称。
            (
                "UPDATE term_names SET value='成员改名' WHERE term_id=(SELECT id FROM terms WHERE key='m2')",
                Some(&["e3", "m2", "s3"]),
            ),
            ("DELETE FROM term_names WHERE term_id=(SELECT id FROM terms WHERE key='m2')", Some(&["e3", "m2", "s3"])),
            // 共用的皮肤 variant：两个皮肤及其上级。
            (
                "INSERT INTO term_key_names VALUES ('skin','swimsuit_x','ja','wiki','ミズギ')",
                Some(&["e1", "e2", "m1", "s1", "s2"]),
            ),
            // 作品文字：带这个限定词的作品；身份的 `work` 让这个键留在范围内。
            ("DELETE FROM term_key_names WHERE kind='work' AND key='series_w'", Some(&["w1"])),
            ("INSERT INTO term_key_names VALUES ('character','ccip_tag','zh','bangumi','新卡')", Some(&["e4"])),
            (
                "INSERT INTO term_key_names VALUES ('character','member_tag','zh','bangumi','成员键')",
                Some(&["e3", "m2", "s3"]),
            ),
            // 已链接但没有标签：所有无标签的身份共用 ("character","") 这个键 → 整体重建。
            ("UPDATE term_names SET value='无标签改' WHERE term_id=(SELECT id FROM terms WHERE key='b1')", None),
            ("INSERT INTO term_key_names VALUES ('skin','','zh','index','空')", None),
        ];
        for (sql, read) in cases {
            let tx = c.transaction().unwrap();
            crate::legacy_names::track(&tx).unwrap();
            tx.execute_batch(sql).unwrap();
            let scope = Scope::collect(&tx).unwrap();
            assert_eq!(
                scope.as_ref().map(|s| s.read.keys().map(String::as_str).collect::<Vec<_>>()),
                read.map(<[&str]>::to_vec),
                "{sql}"
            );
            rebuild_changed(&tx).unwrap();
            let changed = projection(&tx);
            rebuild(&tx).unwrap();
            assert_eq!(changed, projection(&tx), "{sql}");
            assert_ne!(changed, before, "{sql}");
        }
    }

    #[test]
    fn structure_changes_are_projected_on_both_sides() {
        let mut c = projected();
        let cases: [&str; 10] = [
            // 旧的 variant 键还在（s2），又出现一个新的。
            "UPDATE identities SET variant='other_v' WHERE id='s1'",
            // 删除的身份失去它的行，它的标签键从可选项里移除。
            "DELETE FROM term_links WHERE term_id=(SELECT id FROM terms WHERE key='s2'); DELETE FROM identities WHERE id='s2'",
            // 链接易手：e1 变成未链接，e3 拿到较早的标签，'lonely' 从可选项里移除。
            "UPDATE term_links SET term_id=(SELECT id FROM terms WHERE key='e3') WHERE system='danbooru' AND key='synth_(series_w)'",
            // 解除合并：无名的 e3 不再借用成员的名称。
            "UPDATE identities SET merged_into=NULL WHERE id='m2'",
            // 并入无名的 e3，它现在借用第一个有名称的成员（e4 在 m2 之前）。
            "UPDATE identities SET merged_into='e3' WHERE id='e4'",
            "INSERT INTO identities VALUES ('n1','character',NULL,'',NULL,0,'');
             INSERT INTO term_links(system,key,term_id,current,added_at) VALUES ('danbooru','new_tag',(SELECT id FROM terms WHERE key='n1'),1,'2026-01-20');
             INSERT INTO term_names(term_id,lang,value,source,role,preferred,position,seq) VALUES
              ((SELECT id FROM terms WHERE key='n1'),'zh','新人','index','display',0,0,
               (SELECT coalesce(max(seq),0)+1 FROM term_names))",
            // REPLACE 拿走 m2 的链接，不触发 DELETE 触发器。
            "INSERT OR REPLACE INTO term_links(system,key,term_id,current,added_at) VALUES ('danbooru','member_tag',(SELECT id FROM terms WHERE key='e4'),1,'2026-01-14')",
            "UPDATE identities SET work='series_new' WHERE id='e2'",
            "UPDATE identities SET kind='work' WHERE id='e2'",
            "UPDATE term_links SET current=0 WHERE term_id=(SELECT id FROM terms WHERE key='e4') AND system='ccip_base'",
        ];
        let before = projection(&c);
        for sql in cases {
            let tx = c.transaction().unwrap();
            crate::legacy_names::track(&tx).unwrap();
            tx.execute_batch(sql).unwrap();
            assert_eq!(rebuild_changed(&tx).unwrap(), Maintained::Structure, "{sql}");
            let changed = projection(&tx);
            rebuild(&tx).unwrap();
            assert_eq!(changed, projection(&tx), "{sql}");
            assert_ne!(changed, before, "{sql}");
        }
        // 事务中途投影过（历史文案），之后又改了：为中间状态写下的行也要去掉。
        let tx = c.transaction().unwrap();
        crate::legacy_names::track(&tx).unwrap();
        tx.execute_batch(
            "INSERT INTO identities VALUES ('n2','character',NULL,'',NULL,0,'');
             INSERT INTO term_links(system,key,term_id,current,added_at) VALUES ('danbooru','passing_tag',(SELECT id FROM terms WHERE key='n2'),1,'2026-01-21');",
        )
        .unwrap();
        rebuild_changed(&tx).unwrap();
        assert!(projection(&tx).iter().any(|r| r[0] == rusqlite::types::Value::Text("n2".into())));
        tx.execute_batch("DELETE FROM term_links WHERE term_id=(SELECT id FROM terms WHERE key='n2'); DELETE FROM identities WHERE id='n2';").unwrap();
        rebuild_changed(&tx).unwrap();
        let changed = projection(&tx);
        rebuild(&tx).unwrap();
        assert_eq!(changed, projection(&tx));
        assert_eq!(changed, before);
        // 闭包覆盖图库的八分之一（大量导入）时退回整体重建。
        drop(tx);
        let tx = c.transaction().unwrap();
        crate::legacy_names::track(&tx).unwrap();
        for i in 0..300 {
            tx.execute(&format!("INSERT INTO identities VALUES ('bulk{i}','character',NULL,'',NULL,0,'')"), [])
                .unwrap();
            tx.execute(&format!("INSERT INTO term_links(system,key,term_id,current,added_at) VALUES ('danbooru','bulk_{i}',(SELECT id FROM terms WHERE key='bulk{i}'),1,'2026-01-22')"), [])
                .unwrap();
        }
        assert_eq!(rebuild_changed(&tx).unwrap(), Maintained::Whole);
        let changed = projection(&tx);
        rebuild(&tx).unwrap();
        assert_eq!(changed, projection(&tx));
        // 只改投影不读的列：不记为结构变化。
        drop(tx);
        let tx = c.transaction().unwrap();
        crate::legacy_names::track(&tx).unwrap();
        tx.execute_batch("UPDATE term_links SET added_at=added_at WHERE term_id=(SELECT id FROM terms WHERE key='e1')")
            .unwrap();
        let moved: i64 = tx
            .query_row(
                "SELECT (SELECT count(*) FROM names_old_links_seen)+(SELECT count(*) FROM names_old_entities)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(moved, 0);
    }

    #[test]
    fn one_key_choice_matches_a_whole_load() {
        let mut c = projected();
        // e5 的第一个 danbooru 链接键为空，遮住了第二个：e5 也没有标签。皮肤 s1 自己的名称从不并入它标签的角色键。
        c.execute_batch(
            "INSERT INTO identities VALUES ('e5','character',NULL,'',NULL,0,'');
             INSERT INTO term_links(system,key,term_id,current,added_at) VALUES ('danbooru','',(SELECT id FROM terms WHERE key='e5'),1,'2026-01-14'),('danbooru','shadowed',(SELECT id FROM terms WHERE key='e5'),1,'2026-01-15');
             INSERT INTO term_key_names VALUES ('character','','zh','wiki','空键'),('work','unlisted','zh','bangumi','未收录'),
              ('character','nobody','ja','index','ダレ'),('character','synth_(series_w)','en','danbooru','Ignored'),
              ('work','touhou','zh','bangumi','东方');",
        )
        .unwrap();
        c.execute_batch(&named(&[
            ("e5", "en", "Shadow", "wiki", 0),
            ("e5", "zh", "影", "user", 1),
            ("s1", "zh", "皮肤自名", "index", 0),
        ]))
        .unwrap();
        let keys = |c: &Connection| -> BTreeSet<(String, String)> {
            let mut keys: BTreeSet<(String, String)> = c
                .prepare("SELECT kind,key FROM term_key_names UNION SELECT kind,key FROM term_name_choice")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            for (kind, key) in [
                ("character", ""),
                ("character", "absent"),
                ("character", "shadowed"),
                ("character", "old_tag"),
                ("skin", ""),
                ("skin", "new_year_kimono"),
                ("work", ""),
                ("work", "touhou"),
            ] {
                keys.insert((kind.into(), key.into()));
            }
            keys
        };
        let same = |c: &Connection| {
            let view = View::load(c).unwrap();
            for (kind, key) in keys(c) {
                assert_eq!(choose_key(c, &kind, &key).unwrap(), view.choose(&kind, &key).unwrap(), "{kind} {key:?}");
            }
        };
        same(&c);
        // 未结束事务里尚未提交的写入（模型预览）：在任何投影刷新之前，按写入的样子读出。
        let tx = c.transaction().unwrap();
        crate::legacy_names::track(&tx).unwrap();
        tx.execute_batch(&named(&[("e2", "zh", "另一个", "bangumi", 1), ("b1", "ja", "ムヒョウ", "index", 0)]))
            .unwrap();
        tx.execute_batch(
            "UPDATE term_names SET value='改名' WHERE term_id=(SELECT id FROM terms WHERE key='e1') AND lang='zh';
             DELETE FROM term_names WHERE term_id=(SELECT id FROM terms WHERE key='e5') AND source='user';
             INSERT INTO term_key_names VALUES ('skin','swimsuit_x','zh','user','用户皮肤'),('work','series_w','zh','user','用户作品');
             UPDATE term_links SET current=0 WHERE term_id=(SELECT id FROM terms WHERE key='e2');",
        )
        .unwrap();
        same(&tx);
        assert_eq!(choose_key(&tx, "character", "synth_(series_w)").unwrap().names[0].as_deref(), Some("改名"));
    }

    #[test]
    fn vocabulary_helpers_follow_python() {
        assert_eq!(readable("en_only_(series_q)", true), "En Only");
        assert_eq!(readable("series_none", false), "Series None");
        assert_eq!(readable("2nd_form", false), "2Nd Form");
        assert_eq!(readable("a_(b)_(c)", true), "A (B)");
        assert_eq!(readable("x_()", true), "X ()");
        assert_eq!(readable("mika_(swimsuit)_(blue_archive)", true), "Mika (Swimsuit)");
        assert_eq!(readable("o'neill", false), "O'Neill");
        assert_eq!(skin_zh("new_year_kimono").as_deref(), Some("新年 · 和服"));
        assert_eq!(skin_zh("swimsuit_x"), None);
        assert_eq!(skin_zh("festival_outfit"), Some("祭典服".to_owned()));
        assert_eq!(skin_zh(""), None);
    }

    #[test]
    fn appearance_names_and_search_share_the_chinese_vocabulary() {
        let c = library();
        c.execute_batch(TABLES).unwrap();
        for (variant, name) in [
            ("1st_costume", "第一套衣装"),
            ("2nd_costume", "第二套衣装"),
            ("3rd_costume", "第三套衣装"),
            ("4th_costume", "第四套衣装"),
            ("9th_costume", "第九套衣装"),
            ("11th_costume", "第十一套衣装"),
            ("22nd_costume", "第二十二套衣装"),
            ("100th_costume", "第100套衣装"),
            ("101st_costume", "第101套衣装"),
            ("4294967296th_costume", "第4294967296套衣装"),
            ("winter_casual", "冬季 · 便服"),
            ("summer", "夏季"),
            ("winter", "冬季"),
            ("sleepwear", "睡衣"),
            ("racing", "赛车服"),
            ("elite_ii", "精英二阶段"),
            ("gothic_lolita", "哥特洛丽塔"),
            ("winter_3rd_costume", "冬季 · 第三套衣装"),
        ] {
            c.execute("UPDATE identities SET variant=? WHERE id='s1'", [variant]).unwrap();
            rebuild(&c).unwrap();
            let shown = c
                .query_row("SELECT name FROM term_name_display WHERE entity='s1' AND skin_only=1", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap();
            assert_eq!(shown, name, "{variant}");
            assert!(crate::names_search::hits(&c, name).unwrap().contains_key("e1"), "{variant}");
        }
        for query in ["冬季", "第三套衣装", "winter 3rd costume", "同名 · 冬季 · 第三套衣装"] {
            assert!(crate::names_search::hits(&c, query).unwrap().contains_key("e1"), "{query}");
        }
        for variant in ["0th_costume", "01st_costume", "11st_costume", "winter_summer_flower", "2nd_costume_"] {
            assert_eq!(skin_zh(variant), None, "{variant}");
        }
    }

    #[test]
    fn manual_appearance_names_win_and_unknown_names_keep_their_fallbacks() {
        let c = library();
        c.execute_batch(
            "UPDATE identities SET variant='winter_casual' WHERE id='s1';
             INSERT INTO term_key_names VALUES ('skin','winter_casual','en','user','Cold Weather'),
               ('skin','winter_casual','zh','model','模型便服');
             INSERT INTO term_key_names VALUES ('character','synth_(series_w)','zh','model','模型角色');
             DELETE FROM term_names WHERE term_id = (SELECT id FROM terms WHERE key='e1');",
        )
        .unwrap();
        let view = View::load(&c).unwrap();
        let short = view.entity("s1", true).unwrap().unwrap().0;
        assert_eq!((short.name.as_str(), short.source.as_str()), ("Cold Weather", "user"));
        assert_eq!(short.names[0].as_deref(), Some("冬季 · 便服"));

        c.execute_batch(&named(&[("s1", "ja", "冬の名前", "user", 1)])).unwrap();
        let view = View::load(&c).unwrap();
        let full = view.entity("s1", false).unwrap().unwrap().0;
        assert_eq!((full.name.as_str(), full.source.as_str()), ("模型角色 · 冬の名前", "user"));
        assert!(crate::names_search::hits(&c, "冬の名前").unwrap().contains_key("e1"));

        let variant = "shoal_beat";
        let choice = choose_key(&c, "skin", variant).unwrap();
        assert_eq!((choice.name.as_str(), choice.source.as_str()), ("Shoal Beat", "readable"));
        c.execute("INSERT INTO term_key_names VALUES ('skin',?,'zh','model','浅滩节拍')", [variant]).unwrap();
        let choice = choose_key(&c, "skin", variant).unwrap();
        assert_eq!((choice.name.as_str(), choice.source.as_str()), ("浅滩节拍", "model"));
    }

    #[test]
    fn builtin_appearance_name_coverage() {
        use crate::character_catalog::{bundled, Action, Relation};
        let variants: Vec<_> = bundled()
            .characters()
            .iter()
            .filter(|entry| entry.relation == Some(Relation::Appearance) && entry.action == Some(Action::Auto))
            .map(|entry| entry.variant.as_deref().unwrap_or(entry.tag.as_str()))
            .collect();
        let before = variants.iter().filter(|tag| costume_qualifier(tag)).count();
        let after = variants.iter().filter(|tag| skin_zh(tag).is_some()).count();
        eprintln!("内置造型译名覆盖共 {} 条，改前 {before}，改后 {after}", variants.len());
        assert!(after * 2 > variants.len());
    }
}
