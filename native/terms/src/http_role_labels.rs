//! 展示用的同作品角色名限定。用完整的身份集合比较，搜索和分页只决定返回哪些行。
use crate::character_catalog::{Catalog, Character};
use crate::character_entities::ENTITIES;
use crate::http_json::Json;
use crate::http_read::{flag, roman, shown_name, SELECTABLE_ROLE};
use crate::names_view::{readable, skin_zh, work_name};
use crate::Result;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(crate) struct RoleLabels(HashMap<String, Json>);

#[derive(Clone, Hash, PartialEq, Eq)]
enum Work {
    Local(String),
    CatalogTag(String),
    CatalogFamily(u32),
}

struct Role {
    id: String,
    automatic_zh: bool,
    tag: String,
}

const INDEPENDENT: &str = "e.kind='character' AND e.original=0 AND e.label_only=0 AND e.mascot=0
    AND e.parent_id IS NULL AND e.merged_into IS NULL";

impl RoleLabels {
    fn load<'a>(c: &Connection, ids: impl IntoIterator<Item = &'a str>) -> Result<Self> {
        let ids: HashSet<_> = ids.into_iter().filter(|id| !id.is_empty()).collect();
        if ids.is_empty() || !flag(c, "terms_names")? {
            return Ok(Self::default());
        }
        let wanted = serde_json::json!(ids).to_string();
        let origin = crate::character_import::EXTENSION_CHARACTER_ORIGIN;
        // 候选只按名字取，不按作品预筛：没有作品的角色要和同名的全部身份一起归组（见 merge_families），
        // 不论请求的是哪一端，拿到的分组都一样。
        let mut stmt = c.prepare_cached(&format!(
            "WITH wanted_roles(name) AS (
                SELECT DISTINCT n.name FROM json_each(?1) r JOIN {ENTITIES} e ON e.id=r.value
                JOIN term_name_display n ON n.entity=e.id AND n.skin_only=0
                WHERE {INDEPENDENT} AND n.tag<>'' AND n.name<>'' AND n.name_source<>'user' AND n.name=n.zh
             )
             SELECT * FROM (
                SELECT e.id,e.work_id,e.work,n.name,n.name_source,n.en,n.tag,n.name=n.zh,e.term_id,
                       count(*) OVER (PARTITION BY n.name) AS name_count
                FROM {ENTITIES} e JOIN term_name_display n ON n.entity=e.id AND n.skin_only=0
                WHERE {INDEPENDENT} AND {SELECTABLE_ROLE}
                    AND n.name IN (SELECT name FROM wanted_roles)
             ) WHERE name_count>1"
        ))?;
        let catalog = crate::character_catalog::current();
        let mut local_works = HashMap::new();
        let mut scopes: HashMap<(Work, String, String), Vec<Role>> = HashMap::new();
        for row in stmt.query_map([&wanted, origin], |r| {
            let name: String = r.get(3)?;
            let source: String = r.get(4)?;
            let tag: String = r.get(6)?;
            let shown = shown_name(name.clone(), &source, r.get(5)?, &tag, true);
            Ok((
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                name,
                shown,
                r.get::<_, i64>(8)?,
                Role {
                    id: r.get(0)?,
                    automatic_zh: source != "user" && r.get::<_, Option<bool>>(7)? == Some(true),
                    tag,
                },
            ))
        })? {
            let (work, work_text, name, shown, term_id, mut role) = row?;
            if let Some(tag) = fallback_catalog_tag(c, catalog, term_id, &role.tag)? {
                role.tag = tag;
            }
            let work = match work {
                Some(work) => Some(Work::Local(work)),
                None => catalog_scope(c, catalog, &role.tag, &work_text, &mut local_works)?,
            };
            if let Some(work) = work {
                scopes.entry((work, name, shown)).or_default().push(role);
            }
        }
        merge_families(catalog, &mut scopes);
        let mut out = HashMap::new();
        for ((work, ..), roles) in scopes.into_iter().filter(|(_, roles)| roles.len() > 1) {
            if roles.iter().any(|r| ids.contains(r.id.as_str())) {
                let works = work_words(c, catalog, &work, &roles, &mut local_works)?;
                disambiguate(catalog, &roles, &works, &ids, &mut out);
            }
        }
        Ok(Self(out))
    }

    pub(crate) fn rows(c: &Connection, rows: &mut [Json]) -> Result<()> {
        let labels = Self::load(c, rows.iter().map(|row| row.field("id").text()))?;
        for row in rows {
            labels.annotate(row);
        }
        Ok(())
    }

    pub(crate) fn pairs(c: &Connection, rows: &mut [Json]) -> Result<()> {
        let labels =
            Self::load(c, rows.iter().flat_map(|row| ["a", "b"].map(|side| row.field(side).field("id").text())))?;
        for row in rows {
            if let Json::Object(fields) = row {
                for (_, endpoint) in fields.iter_mut().filter(|(key, _)| key == "a" || key == "b") {
                    labels.annotate(endpoint);
                }
            }
        }
        Ok(())
    }

    fn annotate(&self, row: &mut Json) {
        if let Some(label) = self.0.get(row.field("id").text()) {
            row.set("name_disambiguation", label.clone());
        }
    }
}

// 索引更新会保留旧词条的当前链接，再补充目录中认识的标签。首选标签仍用于显示名；只有其余当前链接
// 指向同一个目录角色时，才用该角色的资料给重名补限定。
fn fallback_catalog_tag(c: &Connection, catalog: &Catalog, term_id: i64, primary: &str) -> Result<Option<String>> {
    if catalog.character(primary).is_some() {
        return Ok(None);
    }
    let mut stmt = c.prepare_cached(
        "SELECT key FROM term_links WHERE term_id=? AND current=1 AND system IN ('danbooru','wd_character')
         ORDER BY system='wd_character',added_at,key",
    )?;
    let mut found: Option<(u32, String)> = None;
    for row in stmt.query_map([term_id], |r| r.get::<_, String>(0))? {
        let tag = row?;
        let Some(entry) = catalog.character(&tag) else { continue };
        if let Some((id, _)) = &found {
            if *id != entry.id {
                return Ok(None);
            }
        } else {
            found = Some((entry.id, tag));
        }
    }
    Ok(found.map(|(_, tag)| tag))
}

fn catalog_scope(
    c: &Connection,
    catalog: &Catalog,
    tag: &str,
    work_text: &str,
    local: &mut HashMap<String, Option<String>>,
) -> Result<Option<Work>> {
    let entry = catalog.character(tag);
    let tail = entry.map(|entry| crate::character_profile::qualifier(&entry.tag)).unwrap_or_default();
    for work in [work_text, tail].into_iter().filter(|work| !work.is_empty()) {
        let known = known_work(catalog, work);
        if work == work_text || known {
            if !local.contains_key(work) {
                local.insert(work.into(), crate::character_profile::local_work(c, &[work])?);
            }
            if let Some(id) = &local[work] {
                return Ok(Some(Work::Local(id.clone())));
            }
            if known {
                return Ok(Some(Work::CatalogTag(work.into())));
            }
        }
    }
    Ok(entry.and_then(|entry| catalog_root(catalog, entry)).map(|root| Work::CatalogFamily(root.id)))
}

fn known_work(catalog: &Catalog, word: &str) -> bool {
    catalog.work(word).is_some() || work_name(word).is_some()
}

fn catalog_root<'a>(catalog: &'a Catalog, mut entry: &'a Character) -> Option<&'a Character> {
    let mut visited = HashSet::new();
    while let Some(parent) = catalog.parent(entry) {
        if !visited.insert(entry.id) {
            return None;
        }
        entry = parent;
    }
    Some(entry)
}

// 没有作品归属、tag 也不带作品后缀的角色（只由 WD 认出的 `darth_vader` 这类）只能按词表家族归组。同名的同家族
// 角色里有作品的只落在一个本地作品时，把它并进那一组一起比较；落在几个作品时不猜，仍各算各的。
fn merge_families(catalog: &Catalog, scopes: &mut HashMap<(Work, String, String), Vec<Role>>) {
    let root_of = |tag: &str| catalog.character(tag).and_then(|entry| catalog_root(catalog, entry)).map(|root| root.id);
    let mut locals: HashMap<(u32, String, String), HashSet<String>> = HashMap::new();
    for ((work, name, shown), roles) in scopes.iter() {
        let Work::Local(id) = work else { continue };
        for root in roles.iter().filter_map(|role| root_of(&role.tag)) {
            locals.entry((root, name.clone(), shown.clone())).or_default().insert(id.clone());
        }
    }
    let families: Vec<_> = scopes.keys().filter(|(work, ..)| matches!(work, Work::CatalogFamily(_))).cloned().collect();
    for key in families {
        let (Work::CatalogFamily(root), name, shown) = &key else { continue };
        let Some(works) = locals.get(&(*root, name.clone(), shown.clone())) else { continue };
        let mut works = works.iter();
        let (Some(work), None) = (works.next(), works.next()) else { continue };
        let target = (Work::Local(work.clone()), name.clone(), shown.clone());
        if let Some(roles) = scopes.remove(&key) {
            scopes.entry(target).or_default().extend(roles);
        }
    }
}

// 这一组里哪些括号词是作品：词表和内置作品名认识的，与本组本地作品是同一个作品的，或本组按词表认出的作品 tag。
// 比较形态时去掉它们，「hoshino_ai_(oshi_no_ko)」相对「hoshino_ai」就不算多了一种形态。
fn work_words(
    c: &Connection,
    catalog: &Catalog,
    work: &Work,
    roles: &[Role],
    local: &mut HashMap<String, Option<String>>,
) -> Result<HashSet<String>> {
    let mut words = HashSet::new();
    for entry in roles.iter().filter_map(|role| catalog.character(&role.tag)) {
        for word in qualifiers(&entry.tag) {
            if words.contains(word) {
                continue;
            }
            let same = match work {
                Work::Local(id) => {
                    if !local.contains_key(word) {
                        local.insert(word.into(), crate::character_profile::local_work(c, &[word])?);
                    }
                    local[word].as_deref() == Some(id.as_str())
                }
                Work::CatalogTag(tag) => tag == word,
                Work::CatalogFamily(_) => false,
            };
            if same || known_work(catalog, word) {
                words.insert(word.to_owned());
            }
        }
    }
    Ok(words)
}

fn disambiguate(
    catalog: &Catalog,
    roles: &[Role],
    works: &HashSet<String>,
    requested: &HashSet<&str>,
    out: &mut HashMap<String, Json>,
) {
    let entries: Vec<_> = roles.iter().filter_map(|role| catalog.character(&role.tag)).collect();
    let mut labels: Vec<_> = roles
        .iter()
        .filter(|role| role.automatic_zh && !role.tag.is_empty())
        .map(|role| (role, short_form(catalog, &entries, works, &role.tag)))
        .collect();
    let mut counts = HashMap::new();
    for (_, label) in &labels {
        *counts.entry(label.clone()).or_insert(0) += 1;
    }
    for (role, label) in &mut labels {
        if counts[label.as_str()] > 1 {
            *label = readable(&role.tag, false);
        }
    }
    // 两个身份可能通过不同来源共用同一个 tag。编号只用于短词与可读 tag 都区分不开的情况。
    labels.sort_unstable_by(|(a, _), (b, _)| a.id.cmp(&b.id));
    let reserved: HashSet<_> = labels.iter().map(|(_, label)| label.clone()).collect();
    let mut used = HashSet::new();
    for (role, mut label) in labels {
        if !used.insert(label.clone()) {
            for index in 2.. {
                let candidate = format!("{label} {index}");
                if !reserved.contains(&candidate) && used.insert(candidate.clone()) {
                    label = candidate;
                    break;
                }
            }
        }
        if requested.contains(role.id.as_str()) {
            out.insert(role.id.clone(), Json::object([("qualifier", label.into()), ("tag", role.tag.clone().into())]));
        }
    }
}

fn translated(value: &str, work: &str) -> String {
    if !roman() {
        match (work, value) {
            ("arknights", "guard") => return "近卫".into(),
            ("arknights", "medic") => return "医疗".into(),
            _ => {}
        }
        if let Some(zh) = skin_zh(value) {
            return zh;
        }
    }
    readable(value, false)
}

fn qualifiers(mut tag: &str) -> Vec<&str> {
    let mut out = Vec::new();
    loop {
        let part = crate::character_profile::qualifier(tag);
        if part.is_empty() {
            break;
        }
        out.push(part);
        tag = &tag[..tag.len() - part.len() - 3];
    }
    out.reverse();
    out
}

// 词表条目比上级多出来的括号词，不算作品词：「近卫」这类形态。没有上级、或只差作品词时为空。
fn own_forms<'a>(catalog: &Catalog, entry: &'a Character, works: &HashSet<String>) -> Vec<&'a str> {
    let Some(parent) = catalog.parent(entry) else { return Vec::new() };
    let shared = qualifiers(&parent.tag);
    qualifiers(&entry.tag).into_iter().filter(|q| !shared.contains(q) && !works.contains(*q)).collect()
}

fn short_form(catalog: &Catalog, entries: &[&Character], works: &HashSet<String>, tag: &str) -> String {
    let Some(entry) = catalog.character(tag) else { return readable(tag, false) };
    let tail = crate::character_profile::qualifier(&entry.tag);
    if let Some(variant) = &entry.variant {
        return translated(variant, tail);
    }
    let forms = own_forms(catalog, entry, works);
    if !forms.is_empty() {
        return forms.into_iter().map(|form| translated(form, tail)).collect::<Vec<_>>().join(" · ");
    }
    // 同组里有它名下带形态词或造型的条目时，本体叫「基本形态」。旧标签改名这类只差作品词、或整个换了写法的，
    // 两边都用去掉作品后缀的可读 tag（「Shorekeeper」「The Shorekeeper」），一样时 disambiguate 再换成完整的 tag。
    let base = entries.iter().any(|other| {
        other.id != entry.id
            && catalog_root(catalog, other).is_some_and(|root| root.id == entry.id)
            && (other.variant.is_some() || !own_forms(catalog, other, works).is_empty())
    });
    if base {
        return if roman() { "Base form" } else { "基本形态" }.into();
    }
    readable(tag, true)
}
