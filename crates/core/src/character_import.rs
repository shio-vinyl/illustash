//! 同步、刷新、重扫（方案第 7、9 节）：`refresh`、`reindex_characters`、`import_changes`、`_refresh_general_hits`
//! 各是一个核心事务，逐行照 tests/reference/web/store.py（#9 已删除） 的旧顺序。宿主在锁内只读 source.sqlite，把要用的行交进来：
//! 新图行、候选标签行、相册决定与映射、索引的角色 / 别名 / tag、重复组的全量分组、realistic 列表、时间戳文字。
//! 候选写成证据批次（覆盖范围：refresh 全库、reindex 改动的图、import 新图与移除的图），兼容 `candidates`
//! 由有效证据算出；相册结论是 album 批次上的裁决，照旧只在没有结论时写。
//! 前提：标签已切到核心（`general_hits`、`asset_rating` 是视图，跟着 `assets` 走），角色切换时检查。
use crate::character_commands::{hybrid, resolve, verdict, At};
use crate::character_entities::{self, New, ENTITIES, OF};
use crate::character_evidence::{publish, Scope};
use crate::character_links::{check_free, core_system, LINKS};
use crate::character_names::TERM_OF;
use crate::{character_graph, character_json, character_names, character_projection, legacy_characters, Error, Result};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

mod catalog;
pub use catalog::{
    catalog_apply, catalog_children, catalog_declined, catalog_split, declined_pairs, user_touched, CATALOG_META,
};
pub(crate) use catalog::{
    portable_trees, restore_catalog_change, suppress_catalog_change, undo_catalog_change, undo_catalog_split,
    PortableTree, SuppressedCatalogChange,
};

const DEFAULT_TAG_KEYS: [&str; 13] = [
    "sensei_(blue_archive)",
    "doodle_sensei_(blue_archive)",
    "peroro_(blue_archive)",
    "doctor_(arknights)",
    "male_doctor_(arknights)",
    "female_doctor_(arknights)",
    "trainer_(umamusume)",
    "producer_(idolmaster)",
    "p-head_producer",
    "commander_(azur_lane)",
    "commander_(girls'_frontline)",
    "admiral_(kancolle)",
    "master_(vocaloid)",
];
const SHOT_TAGS: [&str; 4] = ["chat_log", "fake_screenshot", "fake_phone_screenshot", "user_interface"];
const TEXT_TAGS: [&str; 4] = ["text_focus", "chinese_text", "korean_text", "english_text"];
const PEOPLE_TAGS: [&str; 7] = ["1girl", "1boy", "solo", "multiple_girls", "multiple_boys", "1other", "male_focus"];
const LINK_SYSTEMS: [&str; 7] = ["danbooru", "ccip", "ccip_base", "wd", "bangumi", "photos_album", "qualifier"];

fn s(v: &Value) -> Option<&str> {
    v.as_str().filter(|x| !x.is_empty())
}

/// 这一次运行里新建身份的 id：调用方可以给定（双跑对齐），没给就由核心生成。
struct Ids<'a>(&'a Value);

impl Ids<'_> {
    fn get(&self, system: &str, key: &str) -> Result<String> {
        match self.0.get(system).and_then(|m| m.get(key)).and_then(Value::as_str) {
            Some(id) => Ok(id.to_owned()),
            None => crate::store::new_key(),
        }
    }
}

/// `Store._linked`。
fn linked(c: &Connection, system: &str, key: &str) -> Result<Option<String>> {
    if key.is_empty() {
        return Ok(None);
    }
    Ok(c.prepare_cached(&format!("SELECT l.entity_id FROM {LINKS} l WHERE l.core_system = ? AND l.key = ?"))?
        .query_row([core_system(system), key], |r| r.get(0))
        .optional()?)
}

/// `Store.link`。
fn link(c: &Connection, entity: &str, system: &str, key: &str) -> Result<()> {
    if !LINK_SYSTEMS.contains(&system) {
        return Err(Error::Invalid("未知的外部来源".into()));
    }
    check_free(c, system, key)?;
    c.prepare_cached(&format!(
        "INSERT OR IGNORE INTO term_links(system, key, term_id, source) VALUES (?, ?, {TERM_OF}, 'import')"
    ))?
    .execute([core_system(system), key, entity])?;
    Ok(())
}

/// 不是作品的作品文字（#296）：索引把角色 tag 最后一个括号当作品文字，括号里也可能是服装、形态或人物类别。
const NOT_WORKS: [&str; 17] = [
    "vtuber",
    "youtuber",
    "streamer",
    "voice_actor",
    "seiyuu",
    "real_life",
    "person",
    "artist",
    "cosplayer",
    "cosplay",
    "mascot",
    "original",
    "character",
    "male",
    "female",
    "costume",
    "outfit",
];

/// 导入时不为它建作品的作品文字：人物类别（`NOT_WORKS`）、服装（`1st_costume`、`…_outfit`）、皮肤词表能整段翻译的
/// 形态（`swimsuit`、`new_year` 等）。只看本地清单，不联网（docs/DATA_MODEL.md 第 5 节「导入」第 2 条）。
pub fn not_work(text: &str) -> bool {
    NOT_WORKS.contains(&text)
        || text.ends_with("_costume")
        || text.ends_with("_outfit")
        || crate::names_view::skin_zh(text).is_some()
}

/// 只供测试：设置 `ILLUSTASH_TEST_NO_IMPORT_WORKS` 时，导入照 #296 之前的规则走：不新建作品，WD 结果建的角色不取
/// tag 后缀当作品文字。对照基准（native/terms/tests/baseline.rs 与 tests/fixed_expectations.py，#201）录在 #296
/// 之前，重放时设上它，录下的期望不改（用户 2026-11-12 定）；导入建作品由 #296 新加的测试覆盖。产品从不设置它。
fn import_builds_works() -> bool {
    std::env::var_os("ILLUSTASH_TEST_NO_IMPORT_WORKS").is_none()
}

/// 新角色的所属作品：按作品文字找 `qualifier` 链接；找不到、文字又不在「不是作品」的清单里，就照第 25 版迁移的写法
/// 新建一个待核对的作品（英文可读名、按文字键已有的作品名与别名、`qualifier` 链接），同一个文字在新旧图库都是
/// 同一个作品。
fn work_of(c: &Connection, ids: &Ids, work: &str) -> Result<Option<String>> {
    if work.is_empty() {
        return Ok(None);
    }
    if let Some(found) = linked(c, "qualifier", work)? {
        return Ok(Some(found));
    }
    if not_work(work) || !import_builds_works() {
        return Ok(None);
    }
    let ident = ids.get("qualifier", work)?;
    character_entities::create(c, &New { id: &ident, kind: "work", ..New::default() }, legacy_characters::ORIGIN)?;
    link(c, &ident, "qualifier", work)?;
    character_names::put_name(c, &ident, "en", "index", &crate::names_view::readable(work, false), "display", 0, 0)?;
    let shown: Vec<(String, String, String)> = c
        .prepare_cached("SELECT lang, source, value FROM term_key_names WHERE kind = 'work' AND key = ?")?
        .query_map([work], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (lang, source, value) in shown {
        character_names::put_name(c, &ident, &lang, &source, &value, "display", 0, 0)?;
    }
    let aliases: Vec<(String, i64)> = c
        .prepare_cached("SELECT value, position FROM term_key_aliases WHERE kind = 'work' AND key = ?")?
        .query_map([work], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (value, position) in aliases {
        character_names::put_name(c, &ident, "", "danbooru", &value, "alias", position, 0)?;
    }
    c.prepare_cached(crate::character_review::OPEN)?.execute(params![ident, "pending", "", 0])?;
    Ok(Some(ident))
}

/// `Store._entity`：链接到 (system, key) 的身份；没有就新建一个带这个链接的人物，作品按 `work_of`。
fn entity(c: &Connection, ids: &Ids, system: &str, key: &str, name: &str, work: &str, origin: &str) -> Result<String> {
    if let Some(found) = linked(c, system, key)? {
        return Ok(found);
    }
    let ident = ids.get(system, key)?;
    let work_id = work_of(c, ids, work)?;
    // 识别证据可以在索引安装或启动之后才首次写入这个 WD 标签。
    let default_mascot = system == "wd" && DEFAULT_TAG_KEYS.contains(&key);
    let new = New {
        id: &ident,
        kind: "character",
        work,
        work_id: work_id.as_deref(),
        mascot: i64::from(default_mascot),
        mascot_origin: default_mascot.then_some("default"),
        ..New::default()
    };
    character_entities::create(c, &new, origin)?;
    link(c, &ident, system, key)?;
    if !name.is_empty() && name != key {
        character_names::alias(c, &ident, name, "index", "")?;
    }
    Ok(ident)
}

/// 只有由角色扩展新建的角色词条才使用此创建来源。
pub const EXTENSION_CHARACTER_ORIGIN: &str = "extension:character";

pub(crate) fn extension_entity(c: &Connection, tag: &str) -> Result<(String, bool)> {
    let ids = Ids(&Value::Null);
    tagged_entity(c, &ids, "danbooru", tag, crate::character_profile::qualifier(tag), EXTENSION_CHARACTER_ORIGIN)
}

/// 跨来源的同一个识别 tag 共用原始词条。目录别名保持各自的原始词条，由目录控制合并指向。
fn tagged_entity(
    c: &Connection,
    ids: &Ids,
    system: &str,
    tag: &str,
    work: &str,
    origin: &str,
) -> Result<(String, bool)> {
    if let Some(found) = linked(c, system, tag)? {
        return Ok((found, false));
    }
    if system == "wd" {
        if let Some(found) = linked(c, "danbooru", tag)? {
            return Ok((found, false));
        }
    } else if crate::character_catalog::current().character(tag).is_some() {
        if let Some(found) = linked(c, "wd", tag)? {
            link(c, &found, system, tag)?;
            return Ok((found, false));
        }
    }
    let ident = entity(c, ids, system, tag, tag, work, origin)?;
    let structure_changed = catalog::fresh(c, ids, &ident, tag)?;
    Ok((ident, structure_changed))
}

// ---- 索引同步：`_characters`（含 `_hierarchy`、`_tags`、别名、`_default_tags`） ----

type Chars = HashMap<String, String>;
type Wd = HashMap<String, BTreeSet<String>>;

fn characters(c: &Connection, index: &Value, historical_index: bool) -> Result<(Chars, Wd)> {
    let ids = Ids(&index["new_ids"]);
    let rows = index["characters"].as_array().map(Vec::as_slice).unwrap_or_default();
    let catalog = crate::character_catalog::current();
    let mut tags = BTreeMap::new();
    for t in index["tags"].as_array().into_iter().flatten() {
        if let (Some(char_id), Some(tag)) = (s(&t[0]), s(&t[1])) {
            tags.entry(char_id).or_insert(tag);
        }
    }
    let (mut chars, mut wd): (Chars, Wd) = (HashMap::new(), HashMap::new());
    let mut fresh = Vec::new();
    for r in rows {
        let char_id = s(&r["char_id"]).ok_or_else(|| Error::Invalid("索引角色缺少 char_id".into()))?;
        let tag = s(&r["danbooru_tag"]).or_else(|| tags.get(char_id).copied()).or(s(&r["wd_tag"]));
        let name = s(&r["name_zh"]).or(s(&r["name_ja"])).or(s(&r["name_en"])).unwrap_or(char_id);
        let work = r["copyright"].as_str().unwrap_or("");
        let prior = linked(c, "ccip", char_id)?;
        let ident = if let Some(tag) = tag.filter(|tag| !historical_index && catalog.character(tag).is_some()) {
            let found = match prior {
                Some(id) => Some(id),
                None => catalog::exact(c, tag)?,
            };
            let ident = match found {
                Some(id) => id,
                None => {
                    let id = entity(c, &ids, "ccip", char_id, name, work, legacy_characters::ORIGIN)?;
                    fresh.push((id.clone(), tag));
                    id
                }
            };
            link(c, &ident, "ccip", char_id)?;
            link(c, &ident, "danbooru", tag)?;
            ident
        } else if let Some(id) = prior {
            id
        } else {
            entity(c, &ids, "ccip", char_id, name, work, legacy_characters::ORIGIN)?
        };
        if historical_index && !hybrid(c)? {
            c.prepare_cached(&format!("UPDATE term_tree SET variant=?2 WHERE {OF}"))?
                .execute([&ident, s(&r["skin"]).unwrap_or("")])?;
        }
        chars.insert((*char_id).to_owned(), ident.clone());
        if let Some(tag) = s(&r["wd_tag"]) {
            wd.entry(tag.to_owned()).or_default().insert(ident.clone());
        }
        for (lang, alias) in [("", "char_id"), ("", "wd_tag"), ("zh", "name_zh"), ("ja", "name_ja"), ("en", "name_en")]
        {
            if let Some(value) = s(&r[alias]) {
                character_names::alias(c, &ident, value, "index", lang)?;
            }
        }
    }
    if historical_index {
        hierarchy(c, rows, &ids)?;
    } else {
        // 同批出现新旧标签时，先挂旧标签到本批已有的规范端点。
        fresh.sort_by_key(|(_, tag)| catalog.character(tag).is_some_and(|entry| entry.tag.as_str() == *tag));
        for (ident, tag) in fresh {
            catalog::fresh(c, &ids, &ident, tag)?;
        }
    }
    for t in index["tags"].as_array().into_iter().flatten() {
        let (Some(char_id), Some(tag)) = (t[0].as_str(), t[1].as_str()) else { continue };
        if let Some(ident) = chars.get(char_id) {
            link(c, ident, "danbooru", tag)?;
            character_names::alias(c, ident, tag, "index", "")?;
        }
    }
    finish_hybrid_import(c)?;
    for a in index["aliases"].as_array().into_iter().flatten() {
        let (Some(char_id), Some(value)) = (a[0].as_str(), a[1].as_str()) else { continue };
        if let Some(ident) = chars.get(char_id) {
            character_names::alias(c, ident, value, "index", "")?;
        }
    }
    default_tags(c)?;
    Ok((chars, wd))
}

/// `Store._default_tags`。
pub fn default_tags(c: &Connection) -> Result<usize> {
    let marks = vec!["?"; DEFAULT_TAG_KEYS.len()].join(",");
    let n = c
        .prepare_cached(&format!(
            "UPDATE term_tree SET mascot = 1, mascot_origin = 'default'
             WHERE mascot_origin IS NULL AND mascot = 0 AND parent_id IS NULL AND merged_into IS NULL
               AND term_id IN (SELECT id FROM terms WHERE kind <> 'work')
               AND term_id IN (SELECT x.term_id FROM term_links x WHERE x.key IN ({marks}))"
        ))?
        .execute(params_from_iter(DEFAULT_TAG_KEYS))?;
    Ok(n)
}

/// `Store._hierarchy`（策略未设置与 hybrid；hybrid 下只给索引里新出现的角色套名字规则，再进基线）。
fn hierarchy(c: &Connection, rows: &[Value], ids: &Ids) -> Result<()> {
    let identity = hybrid(c)?;
    let known: Option<BTreeSet<String>> = if identity {
        Some(
            c.prepare_cached("SELECT id FROM term_identity_baseline")?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?,
        )
    } else {
        None
    };
    let by_id: HashMap<&str, &Value> = rows.iter().filter_map(|r| r["char_id"].as_str().map(|k| (k, r))).collect();
    let text = |v: &Value| v.as_str().unwrap_or("").to_owned();
    // base_tag 为空时键是（没有 base_tag, copyright），与旧实现一致；这里用 Option 保留「没有 base_tag」。
    let mut plain: HashMap<String, String> = HashMap::new();
    for r in rows {
        if let Some(base) = s(&r["base_tag"]) {
            if s(&r["skin"]).is_none() && r["danbooru_tag"].as_str() == Some(base) {
                plain.insert(base.to_owned(), text(&r["char_id"]));
            }
        }
    }
    let mut parents: HashMap<(Option<String>, String), Vec<String>> = HashMap::new();
    for r in rows {
        if s(&r["skin"]).is_none() {
            parents
                .entry((s(&r["base_tag"]).map(str::to_owned), text(&r["copyright"])))
                .or_default()
                .push(text(&r["char_id"]));
        }
    }
    for r in rows {
        let Some(skin) = s(&r["skin"]) else { continue };
        let Some(child) = linked(c, "ccip", r["char_id"].as_str().unwrap_or(""))? else { continue };
        if known.as_ref().is_some_and(|k| k.contains(&child)) {
            continue;
        }
        let key = (s(&r["base_tag"]).map(str::to_owned), text(&r["copyright"]));
        let mut matches: Vec<String> =
            if key.0.is_some() { parents.get(&key).cloned().unwrap_or_default() } else { vec![] };
        if matches.len() != 1 {
            if let Some(p) = key.0.as_ref().and_then(|b| plain.get(b)) {
                matches = vec![p.clone()];
            }
        }
        let parent = if matches.len() == 1 {
            let p = by_id[matches[0].as_str()];
            let char_id = p["char_id"].as_str().unwrap_or("");
            let name = s(&p["name_zh"]).or(s(&p["name_ja"])).or(s(&p["name_en"])).unwrap_or(char_id);
            entity(c, ids, "ccip", char_id, name, &key.1, legacy_characters::ORIGIN)?
        } else {
            let char_id = text(&r["char_id"]);
            let base = match &key.0 {
                Some(b) => json!([b, key.1]),
                None => json!([char_id, key.1]),
            };
            let base = character_json::dump(&base, false);
            let name = key.0.clone().unwrap_or(char_id.clone());
            entity(c, ids, "ccip_base", &base, &name, &key.1, legacy_characters::ORIGIN)?
        };
        let parent = resolve(c, &parent)?;
        c.prepare_cached(&format!("UPDATE term_tree SET parent_id=?2,variant=?3,work_id=NULL WHERE {OF}"))?
            .execute(params![child, parent, skin])?;
        c.prepare_cached("UPDATE terms SET kind='skin' WHERE key=? AND kind IS NOT 'skin'")?.execute([&child])?;
    }
    Ok(())
}

fn finish_hybrid_import(c: &Connection) -> Result<()> {
    if hybrid(c)? {
        let before: i64 =
            c.prepare_cached("SELECT count(*) FROM term_identity_baseline")?.query_row([], |r| r.get(0))?;
        character_graph::baseline(c)?;
        let after: i64 =
            c.prepare_cached("SELECT count(*) FROM term_identity_baseline")?.query_row([], |r| r.get(0))?;
        if after != before {
            character_graph::rebuild(c)?;
        }
    }
    Ok(())
}

// ---- 候选与相册 ----

/// `Store._candidate`：一条源标签 → (图, 身份, WD / CCIP, 分数)。
fn candidate(
    c: &Connection,
    ids: &Ids,
    row: &Value,
    chars: &Chars,
    wd: &Wd,
) -> Result<(String, String, String, f64, bool)> {
    let uuid = row[0].as_str().unwrap_or("").to_owned();
    let value = row[1].as_str().unwrap_or("");
    let conf = row[2].as_f64().ok_or_else(|| Error::Invalid("候选分数不是数".into()))?;
    let dim = row[3].as_str().unwrap_or("");
    let (ident, structure_changed) = if dim == "char_ext" {
        let ident = match chars.get(value) {
            Some(i) => i.clone(),
            None => entity(c, ids, "ccip", value, value, "", legacy_characters::ORIGIN)?,
        };
        (ident, false)
    } else {
        let matches = wd.get(value);
        match matches.filter(|m| m.len() == 1) {
            Some(m) => (m.iter().next().expect("一个").clone(), false),
            None => {
                let work = if import_builds_works() { crate::character_profile::qualifier(value) } else { "" };
                tagged_entity(c, ids, "wd", value, work, legacy_characters::ORIGIN)?
            }
        }
    };
    Ok((uuid, ident, if dim == "character" { "WD".into() } else { "CCIP".into() }, conf, structure_changed))
}

type Rows = BTreeMap<(String, String, String), f64>;

fn candidates(
    c: &Connection,
    ids: &Ids,
    labels: &Value,
    chars: &Chars,
    wd: &Wd,
    only: Option<&BTreeSet<String>>,
) -> Result<Rows> {
    let mut out: Rows = BTreeMap::new();
    let hybrid = hybrid(c)?;
    let mut changed = BTreeSet::new();
    let mut resolved: HashMap<String, String> = HashMap::new();
    for row in labels.as_array().into_iter().flatten() {
        if only.is_some_and(|o| !o.contains(row[0].as_str().unwrap_or(""))) {
            continue;
        }
        let (u, mut e, src, score, structure_changed) = candidate(c, ids, row, chars, wd)?;
        if hybrid && structure_changed {
            changed.insert(e.clone());
        }
        if !hybrid {
            e = match resolved.entry(e) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.get().clone(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let id = entry.key();
                    let owner = if catalog::managed_merge(c, id)? { id.clone() } else { resolve(c, id)? };
                    entry.insert(owner).clone()
                }
            };
        }
        // INSERT_CANDIDATE：同键取最大。
        let slot = out.entry((u, e, src)).or_insert(score);
        *slot = slot.max(score);
    }
    character_graph::register_catalog(c, &changed)?;
    Ok(out)
}

/// 这一次运行的相册导入批次（没有 action；排在最近一步之后）。
struct Album<'a> {
    c: &'a Connection,
    batch: Option<(i64, i64)>,
}

impl Album<'_> {
    fn at(&mut self) -> Result<At> {
        if self.batch.is_none() {
            let anchor: i64 =
                self.c.prepare_cached("SELECT coalesce(max(id),0) FROM actions")?.query_row([], |r| r.get(0))?;
            self.c
                .prepare_cached("INSERT INTO term_batches(kind,anchor,label) VALUES ('album',?,?)")?
                .execute(rusqlite::params![anchor, "相册导入"])?;
            self.batch = Some((self.c.last_insert_rowid(), anchor));
        }
        let (b, a) = self.batch.expect("有批次");
        Ok(At::Batch(b, a))
    }
}

/// `Store._import_decision`：相册成员只在第一次导入时变成一次人工确认；之后移出相册不会撤掉它。
fn decision(c: &Connection, ids: &Ids, album: &mut Album, row: &Value, map: &Value, chars: &Chars) -> Result<()> {
    let (Some(uuid), Some(value)) = (row[0].as_str(), row[1].as_str()) else { return Ok(()) };
    if c.prepare_cached("SELECT 1 FROM imported WHERE uuid=? AND value=?")?
        .query_row([uuid, value], |_| Ok(()))
        .optional()?
        .is_some()
    {
        return Ok(());
    }
    let spec = &map[value];
    let ident = match spec["char_id"].as_str().and_then(|k| chars.get(k)) {
        Some(i) => i.clone(),
        None => entity(
            c,
            ids,
            "photos_album",
            value,
            s(&spec["album_name_ja"]).unwrap_or(value),
            "",
            legacy_characters::ORIGIN,
        )?,
    };
    let ident = resolve(c, &ident)?;
    let ident: String = c
        .prepare_cached(&format!("SELECT coalesce(parent_id,id) FROM {ENTITIES} WHERE id=?"))?
        .query_row([&ident], |r| r.get(0))?;
    character_names::alias(c, &ident, value, "album", "")?;
    if c.prepare_cached("INSERT OR IGNORE INTO assertions VALUES (?,?,'confirmed','album')")?.execute([uuid, &ident])?
        > 0
    {
        if hybrid(c)? {
            crate::character_commands::scope_on_insert(c, uuid, &ident)?;
        }
        let at = album.at()?;
        verdict(c, at, uuid, &ident, "yes", "album")?;
    }
    c.prepare_cached("INSERT INTO imported VALUES (?,?)")?.execute([uuid, value])?;
    Ok(())
}

// ---- 规则与排除 ----

fn has(tags: &[&str]) -> String {
    format!(
        "coalesce(a.uuid IN (SELECT g.uuid FROM general_hits g WHERE g.value IN ({})),0)",
        vec!["?"; tags.len()].join(",")
    )
}

/// `Store._refresh_rules`。
pub fn rules(c: &Connection, realistic: &Value) -> Result<()> {
    c.prepare_cached("DELETE FROM auto_rules")?.execute([])?;
    let mut photo =
        c.prepare_cached("INSERT OR IGNORE INTO auto_rules SELECT uuid,'exclude','photo' FROM assets WHERE uuid=?")?;
    for u in realistic.as_array().into_iter().flatten() {
        photo.execute([u.as_str().unwrap_or("")])?;
    }
    content_rules(c, "")
}

fn content_rules(c: &Connection, scope: &str) -> Result<()> {
    let nochar = "NOT EXISTS(SELECT 1 FROM memberships m WHERE m.uuid=a.uuid)";
    let (shot, people, text) = (has(&SHOT_TAGS), has(&PEOPLE_TAGS), has(&TEXT_TAGS));
    fn join(parts: &[&[&'static str]]) -> Vec<&'static str> {
        parts.concat()
    }
    c.prepare_cached(&format!("INSERT OR IGNORE INTO auto_rules SELECT a.uuid,'exclude','screenshot' FROM assets a WHERE {shot} AND NOT {people} AND {nochar}{scope}"))?.execute(
        params_from_iter(join(&[&SHOT_TAGS, &PEOPLE_TAGS])),
    )?;
    c.prepare_cached(&format!(
        "INSERT OR IGNORE INTO auto_rules SELECT a.uuid,'exclude','text' FROM assets a
            WHERE {text} AND NOT {shot} AND NOT {people} AND {nochar}{scope}"
    ))?
    .execute(params_from_iter(join(&[&TEXT_TAGS, &SHOT_TAGS, &PEOPLE_TAGS])))?;
    c.prepare_cached(&format!("INSERT OR IGNORE INTO auto_rules SELECT a.uuid,'pending','screenshot' FROM assets a WHERE {shot} AND {people} AND {nochar}{scope}"))?.execute(
        params_from_iter(join(&[&SHOT_TAGS, &PEOPLE_TAGS])),
    )?;
    Ok(())
}

const EXCLUDED: &str = "INSERT INTO excluded SELECT a.uuid,coalesce(r.reason,'manual'),CASE WHEN k.status='excluded' THEN 'review' ELSE 'rule' END
            FROM assets a LEFT JOIN curation k ON k.uuid=a.uuid LEFT JOIN auto_rules r ON r.uuid=a.uuid AND r.kind='exclude'
            WHERE (k.status='excluded' OR (r.uuid IS NOT NULL AND k.uuid IS NULL))";

/// 刷新全部排除状态，之后重算折叠，以保留不同收录状态的副本。
pub fn excluded(c: &Connection) -> Result<()> {
    c.prepare_cached("DELETE FROM excluded")?.execute([])?;
    c.prepare_cached(EXCLUDED)?.execute([])?;
    character_projection::fold(c)
}

/// 重算受归属变更影响的自动排除及其所在副本组。照片结论由识别决定，保留已有结果。
pub(crate) fn refresh_exclusions_and_fold(c: &Connection) -> Result<()> {
    c.prepare_cached(
        "DELETE FROM auto_rules WHERE reason IN ('screenshot','text') AND uuid IN (SELECT uuid FROM touched)",
    )?
    .execute([])?;
    content_rules(c, " AND a.uuid IN (SELECT uuid FROM touched)")?;
    c.prepare_cached("DELETE FROM excluded WHERE uuid IN (SELECT uuid FROM touched)")?.execute([])?;
    c.prepare_cached(&format!("{EXCLUDED} AND a.uuid IN (SELECT uuid FROM touched)"))?.execute([])?;
    character_projection::fold_touched(c)
}

fn touch(c: &Connection, uuids: &[String]) -> Result<()> {
    c.execute_batch("CREATE TEMP TABLE IF NOT EXISTS touched(uuid TEXT PRIMARY KEY); DELETE FROM touched;")?;
    let mut put = c.prepare_cached("INSERT OR IGNORE INTO touched VALUES (?)")?;
    for u in uuids {
        put.execute([u])?;
    }
    Ok(())
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_owned)).collect()
}

fn insert_assets(c: &Connection, rows: &Value) -> Result<()> {
    let mut put = c.prepare_cached("INSERT INTO assets VALUES (?,?,?,?,?,?,?)")?;
    for r in rows.as_array().into_iter().flatten() {
        let values: Vec<rusqlite::types::Value> =
            r.as_array().into_iter().flatten().map(crate::character_commands::sql_value).collect();
        if values.len() != 7 {
            return Err(Error::Invalid("图的行应有 7 列".into()));
        }
        put.execute(params_from_iter(values))?;
    }
    Ok(())
}

fn duplicates(c: &Connection, rows: &Value) -> Result<()> {
    c.prepare_cached("DELETE FROM duplicates")?.execute([])?;
    let mut put = c.prepare_cached("INSERT INTO duplicates(uuid,grp,size,pos,grp_added,rep) VALUES (?,?,?,?,?,?)")?;
    for r in rows.as_array().into_iter().flatten() {
        let values: Vec<rusqlite::types::Value> =
            r.as_array().into_iter().flatten().map(crate::character_commands::sql_value).collect();
        put.execute(params_from_iter(values))?;
    }
    Ok(())
}

fn indexed(c: &Connection, args: &Value) -> Result<()> {
    let at = args["indexed"].as_str().ok_or_else(|| Error::Invalid("缺少同步时间".into()))?;
    c.prepare_cached("INSERT OR REPLACE INTO meta VALUES ('indexed',?)")?.execute([at])?;
    Ok(())
}

// ---- 命令 ----

/// `Store.refresh` 的事务部分。App 先交 WD，再以 finalize_labels 一次生成最终派生；旧调用方仍可单独 rederive。
fn refresh(c: &Connection, args: &Value, historical_index: bool) -> Result<Value> {
    let (chars, wd) = characters(c, &args["index"], historical_index)?;
    let ids = Ids(&args["index"]["new_ids"]);
    c.prepare_cached("DELETE FROM assets")?.execute([])?;
    insert_assets(c, &args["assets"])?;
    let valid: BTreeSet<String> =
        c.prepare_cached("SELECT uuid FROM assets")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    duplicates(c, &args["duplicates"])?;
    let rows = candidates(c, &ids, &args["labels"], &chars, &wd, Some(&valid))?;
    publish(c, &format!("refresh:{}", args["indexed"].as_str().unwrap_or("")), Scope::All, &rows)?;
    c.prepare_cached("DELETE FROM candidates")?.execute([])?;
    character_projection::refresh_candidates(c, &valid.iter().cloned().collect::<Vec<_>>())?;
    let mut album = Album { c, batch: None };
    for r in args["decisions"].as_array().into_iter().flatten() {
        if valid.contains(r[0].as_str().unwrap_or("")) {
            decision(c, &ids, &mut album, r, &args["album_map"], &chars)?;
        }
    }
    character_projection::evidence(c, None)?;
    if args["finalize_labels"].as_bool() == Some(true) {
        rederive(c, args)?;
    } else {
        character_projection::single(c, None)?;
        character_projection::memberships(c, None)?;
    }
    indexed(c, args)?;
    Ok(json!({"refreshed": valid.len()}))
}

/// `Store.reindex_characters`。
fn reindex(c: &Connection, args: &Value, historical_index: bool) -> Result<Value> {
    let (chars, wd) = characters(c, &args["index"], historical_index)?;
    let ids = Ids(&args["index"]["new_ids"]);
    let known: BTreeSet<String> =
        c.prepare_cached("SELECT uuid FROM assets")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let mut uuids: Vec<String> = Vec::new();
    for u in strings(&args["uuids"]) {
        if known.contains(&u) && !uuids.contains(&u) {
            uuids.push(u);
        }
    }
    let scope: BTreeSet<String> = uuids.iter().cloned().collect();
    let rows = candidates(c, &ids, &args["labels"], &chars, &wd, Some(&scope))?;
    publish(c, &format!("reindex:{}", args["indexed"].as_str().unwrap_or("")), Scope::Assets(&uuids), &rows)?;
    character_projection::refresh_candidates(c, &uuids)?;
    character_projection::evidence(c, Some(&uuids))?;
    character_projection::memberships_before_excluded(c, Some(&uuids))?;
    rules(c, &args["realistic"])?;
    excluded(c)?;
    indexed(c, args)?;
    Ok(json!({"changed": uuids.len()}))
}

/// `Store._import_changes`（标签已在核心：general_hits、asset_rating 是视图，不另写）。
fn import(c: &Connection, args: &Value) -> Result<Value> {
    let ids = Ids(&args["new_ids"]);
    let chars: Chars = c
        .prepare_cached(&format!("SELECT l.key, l.entity_id FROM {LINKS} l WHERE l.core_system = 'ccip'"))?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut wd: Wd = HashMap::new();
    for t in args["wd_tags"].as_array().into_iter().flatten() {
        let (Some(char_id), Some(tag)) = (t[0].as_str(), s(&t[1])) else { continue };
        if let Some(ident) = chars.get(char_id) {
            wd.entry(tag.to_owned()).or_default().insert(ident.clone());
        }
    }
    let rows = args["rows"].as_array().cloned().unwrap_or_default();
    let added: Vec<String> = rows.iter().filter_map(|r| r[0].as_str().map(str::to_owned)).collect();
    let removed = strings(&args["removed"]);
    crate::wd_extension::invalidate(c, &removed)?;
    let both: Vec<String> = added.iter().chain(&removed).cloned().collect();
    touch(c, &both)?;
    for table in
        ["memberships", "tag_memberships", "excluded", "single_assets", "lead_assets", "family_evidence", "assets"]
    {
        c.prepare_cached(&format!("DELETE FROM {table} WHERE uuid IN (SELECT uuid FROM touched)"))?.execute([])?;
    }
    insert_assets(c, &Value::Array(rows))?;
    let only: BTreeSet<String> = added.iter().cloned().collect();
    let found = candidates(c, &ids, &args["labels"], &chars, &wd, Some(&only))?;
    let mut scope: Vec<String> = both.clone();
    let mut seen = BTreeSet::new();
    scope.retain(|u| seen.insert(u.clone()));
    publish(c, &format!("import:{}", args["indexed"].as_str().unwrap_or("")), Scope::Assets(&scope), &found)?;
    character_projection::refresh_candidates(c, &scope)?;
    let mut album = Album { c, batch: None };
    for r in args["decisions"].as_array().into_iter().flatten() {
        if only.contains(r[0].as_str().unwrap_or("")) {
            decision(c, &ids, &mut album, r, &args["album_map"], &chars)?;
        }
    }
    duplicates(c, &args["duplicates"])?;
    character_projection::evidence(c, Some(&added))?;
    character_projection::single(c, Some(&added))?;
    character_projection::memberships_before_excluded(c, Some(&both))?;
    rules(c, &args["realistic"])?;
    excluded(c)?;
    if let Some(run) = args["run_id"].as_i64() {
        let mut put = c.prepare_cached("INSERT OR REPLACE INTO sync_added VALUES (?,?)")?;
        for u in &added {
            put.execute(params![u, run])?;
        }
    }
    indexed(c, args)?;
    Ok(json!({"added": added.len(), "removed": removed.len()}))
}

/// `Store._refresh_general_hits`（标签已在核心）：WD 结果重交之后重算单人、线索、归属、规则、排除。
fn rederive(c: &Connection, args: &Value) -> Result<Value> {
    character_projection::single(c, None)?;
    character_projection::memberships_before_excluded(c, None)?;
    rules(c, &args["realistic"])?;
    excluded(c)?;
    for key in ["rating_indexed", "general_indexed"] {
        let at = args[key].as_str().ok_or_else(|| Error::Invalid(format!("缺少 {key}")))?;
        c.prepare_cached("INSERT OR REPLACE INTO meta VALUES (?,?)")?.execute([key, at])?;
    }
    Ok(json!({"rederived": true}))
}

/// 启动时补默认标签（旧实现的 `_default_tags`，现由核心执行）。
fn defaults(c: &Connection) -> Result<Value> {
    let n = default_tags(c)?;
    Ok(json!({"changed": n}))
}

pub fn command(c: &Connection, op: &str, args: &Value) -> Result<Option<Value>> {
    Ok(Some(match op {
        "refresh" => refresh(c, args, false)?,
        "reindex" => reindex(c, args, false)?,
        "import" => import(c, args)?,
        "rederive" => rederive(c, args)?,
        "default_tags" => defaults(c)?,
        "catalog_apply" => catalog_apply(c)?,
        "catalog_split" => catalog_split(c, args)?,
        _ => return Ok(None),
    }))
}

/// 验证构建的固定重放入口，按录制时的层级规则解释旧索引。
pub fn replay_index(c: &Connection, args: &Value) -> Result<Option<Value>> {
    if !cfg!(feature = "verify-incremental") {
        return Err(Error::invalid("历史索引回放仅用于验证构建"));
    }
    match args["op"].as_str() {
        Some("refresh") => refresh(c, args, true).map(Some),
        Some("reindex") => reindex(c, args, true).map(Some),
        _ => Ok(None),
    }
}

/// 关系出现之前的应用迁移只用历史上的层级/标签部分。
pub fn legacy_hierarchy(c: &Connection, index: &Value) -> Result<()> {
    if character_projection::policy(c)? {
        return Err(Error::Schema("早期应用版本与 hybrid 标记不兼容".into()));
    }
    let rows = index["characters"].as_array().cloned().unwrap_or_default();
    hierarchy(c, &rows, &Ids(&json!({})))?;
    for row in index["tags"].as_array().into_iter().flatten() {
        if let (Some(key), Some(tag)) = (row[0].as_str(), row[1].as_str()) {
            if let Some(id) = linked(c, "ccip", key)? {
                link(c, &id, "danbooru", tag)?;
                character_names::alias(c, &id, tag, "index", "")?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_work_covers_costumes_forms_and_person_kinds_only() {
        for text in ["vtuber", "1st_costume", "2nd_outfit", "alternate_costume", "swimsuit", "new_year", "bunny_maid"] {
            assert!(not_work(text), "{text}");
        }
        for text in ["fate", "blue_archive", "kancolle", "honkai:_star_rail", "idolmaster", "Example", "swimsuit_anime"]
        {
            assert!(!not_work(text), "{text}");
        }
    }

    #[test]
    fn set_membership_keeps_exists_null_and_absence_semantics() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE assets(uuid TEXT);
            CREATE TABLE general_hits(uuid TEXT,value TEXT);
            INSERT INTO assets VALUES ('a'),('b'),('c'),(NULL);
            INSERT INTO general_hits VALUES ('a','chat_log'),('a','1girl'),('b','text_focus'),(NULL,'chat_log');",
        )
        .unwrap();
        for tags in [&SHOT_TAGS[..], &PEOPLE_TAGS[..], &TEXT_TAGS[..]] {
            let old = format!(
                "EXISTS(SELECT 1 FROM general_hits g WHERE g.uuid=a.uuid AND g.value IN ({}))",
                vec!["?"; tags.len()].join(",")
            );
            let sql = format!("SELECT ({old}) = ({}) FROM assets a", has(tags));
            let equal: Vec<bool> = c
                .prepare(&sql)
                .unwrap()
                .query_map(params_from_iter(tags.iter().chain(tags)), |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            assert_eq!(equal, vec![true; 4]);
        }
    }
}
