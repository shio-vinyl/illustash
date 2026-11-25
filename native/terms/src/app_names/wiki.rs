//! Danbooru 别名的中文显示候选。按作品比较已落库的别名，工作范围内的一次更新同时复核同作品角色。

use super::{choose, dump, matched, net};
use crate::Result;
use rusqlite::Connection;
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Default)]
struct Names {
    aliases: Vec<String>,
    evidence: HashSet<String>,
    wiki: Vec<(String, bool)>,
}

/// 用同一写事务内的别名和旁证重选。全库检查只在旧规则首次更新时做。
pub(super) fn refresh(c: &Connection, scope: Option<(&str, &[&str])>) -> Result<bool> {
    let filter = match scope {
        None => "",
        Some(("", _)) => "AND t.key IN (SELECT value FROM json_each(?))",
        Some(_) => "AND r.work_text = ?",
    };
    let mut stmt = c.prepare_cached(&format!(
        "SELECT r.work_text, t.key, n.source, n.value, n.preferred
         FROM term_tree r JOIN terms t ON t.id=r.term_id JOIN term_names n ON n.term_id=t.id
         WHERE r.parent_id IS NULL AND r.merged_into IS NULL AND coalesce(r.original,0)=0 AND t.kind NOT IN ('oc','work')
           AND ((n.source='danbooru' AND n.role='alias')
             OR (n.source='wiki' AND n.lang='zh' AND n.role='display')
             OR (n.source='bangumi' AND n.lang='zh')
             OR (n.source IN ('index','bangumi','model') AND n.role='display' AND n.lang IN ('ja','zh')))
           {filter}
         ORDER BY n.position,n.seq"
    ))?;
    let mut groups = BTreeMap::<String, BTreeMap<String, Names>>::new();
    let mut rows = match scope {
        Some(("", ids)) => stmt.query([dump(&json!(ids))])?,
        Some((work, _)) => stmt.query([work])?,
        None => stmt.query([])?,
    };
    while let Some(row) = rows.next()? {
        let work: String = row.get::<_, Option<String>>(0)?.unwrap_or_default();
        let names = groups.entry(work).or_default().entry(row.get(1)?).or_default();
        let source: String = row.get(2)?;
        let value: String = row.get(3)?;
        match source.as_str() {
            "danbooru" => names.aliases.push(value),
            "wiki" => names.wiki.push((value, row.get::<_, i64>(4)? != 0)),
            _ => {
                names.evidence.insert(net::normalize(&value));
            }
        }
    }
    let mut changed = false;
    for (work, characters) in groups {
        let mut seen = HashMap::<String, &str>::new();
        let mut shared = HashSet::new();
        if !work.is_empty() {
            for (id, names) in &characters {
                for alias in &names.aliases {
                    let key = net::normalize(&net::wiki_base(alias));
                    if let Some(previous) = seen.insert(key.clone(), id) {
                        if previous != id {
                            shared.insert(key);
                        }
                    }
                }
            }
        }
        let is_chinese = net::chinese_work(&work, &matched(c, &work)?);
        let works = [choose(c, "work", &work)?.names[0].clone().unwrap_or_default()];
        for (id, names) in &characters {
            let candidate =
                net::wiki_zh(&names.aliases, &works, |n| names.evidence.contains(n), |n| shared.contains(n))
                    .map(|(name, confirmed)| (name, confirmed || is_chinese));
            if !names.wiki.iter().eq(candidate.iter()) {
                crate::names_commands::set_wiki_display(c, id, candidate.as_ref().map(|(n, p)| (n.as_str(), *p)))?;
                changed = true;
            }
        }
    }
    Ok(changed)
}
