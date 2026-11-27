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

#[derive(Hash, PartialEq, Eq)]
enum Work {
    Local(String),
    CatalogWork(String),
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
        let mut stmt = c.prepare_cached(&format!(
            "WITH wanted_roles(name, work) AS (
                SELECT DISTINCT n.name,e.work_id FROM json_each(?1) r JOIN {ENTITIES} e ON e.id=r.value
                JOIN term_name_display n ON n.entity=e.id AND n.skin_only=0
                WHERE {INDEPENDENT} AND n.tag<>'' AND n.name<>'' AND n.name_source<>'user' AND n.name=n.zh
             )
             SELECT e.id,e.work_id,e.work,n.name,n.name_source,n.en,n.tag,n.name=n.zh
             FROM {ENTITIES} e JOIN term_name_display n ON n.entity=e.id AND n.skin_only=0
             WHERE {INDEPENDENT} AND {SELECTABLE_ROLE}
                AND n.name IN (SELECT name FROM wanted_roles)
                AND (e.work_id IS NULL OR e.work_id IN (SELECT work FROM wanted_roles)
                     OR EXISTS(SELECT 1 FROM wanted_roles WHERE work IS NULL))"
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
                Role {
                    id: r.get(0)?,
                    automatic_zh: source != "user" && r.get::<_, Option<bool>>(7)? == Some(true),
                    tag,
                },
            ))
        })? {
            let (work, work_text, name, shown, role) = row?;
            let work = match work {
                Some(work) => Some(Work::Local(work)),
                None => catalog_scope(c, catalog, &role.tag, &work_text, &mut local_works)?,
            };
            if let Some(work) = work {
                scopes.entry((work, name, shown)).or_default().push(role);
            }
        }
        let mut out = HashMap::new();
        for roles in scopes.into_values().filter(|roles| roles.len() > 1) {
            if roles.iter().any(|r| ids.contains(r.id.as_str())) {
                disambiguate(catalog, &roles, &ids, &mut out);
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
        let known = catalog.work(work).is_some() || work_name(work).is_some();
        if work == work_text || known {
            if !local.contains_key(work) {
                local.insert(work.into(), crate::character_profile::local_work(c, &[work])?);
            }
            if let Some(id) = &local[work] {
                return Ok(Some(Work::Local(id.clone())));
            }
            if known {
                return Ok(Some(Work::CatalogWork(work.into())));
            }
        }
    }
    Ok(entry.and_then(|entry| catalog_root(catalog, entry)).map(|root| Work::CatalogFamily(root.id)))
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

fn disambiguate(catalog: &Catalog, roles: &[Role], requested: &HashSet<&str>, out: &mut HashMap<String, Json>) {
    let entries: Vec<_> = roles.iter().filter_map(|role| catalog.character(&role.tag)).collect();
    let mut labels: Vec<_> = roles
        .iter()
        .filter(|role| role.automatic_zh && !role.tag.is_empty())
        .map(|role| (role, short_form(catalog, &entries, &role.tag)))
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

fn short_form(catalog: &Catalog, entries: &[&Character], tag: &str) -> String {
    if let Some(entry) = catalog.character(tag) {
        let tail = crate::character_profile::qualifier(&entry.tag);
        if let Some(variant) = &entry.variant {
            return translated(variant, tail);
        }
        if let Some(parent) = catalog.parent(entry) {
            let shared = qualifiers(&parent.tag);
            let forms: Vec<_> = qualifiers(&entry.tag).into_iter().filter(|q| !shared.contains(q)).collect();
            if !forms.is_empty() {
                return forms.into_iter().map(|form| translated(form, tail)).collect::<Vec<_>>().join(" · ");
            }
        } else if entries
            .iter()
            .any(|other| other.id != entry.id && catalog_root(catalog, other).is_some_and(|root| root.id == entry.id))
        {
            return if roman() { "Base form" } else { "基本形态" }.into();
        }
    }
    readable(tag, false)
}
