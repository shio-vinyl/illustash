//! 来源行和历史上的派生分组；不扫描文件系统，也不运行 worker。
use crate::source_rows::cell;
pub(crate) use crate::source_rows::{rows, rows_with};
use crate::{Error, Result};
use rusqlite::{params, Connection};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
pub(crate) fn index(c: &Connection) -> Result<Value> {
    let mut q = c.prepare("SELECT * FROM characters")?;
    let cols: Vec<String> = q.column_names().map(str::to_owned).collect();
    let chars = q
        .query_map([], |r| {
            let mut o = Map::new();
            for (i, k) in cols.iter().enumerate() {
                o.insert(k.clone(), cell(r.get_ref(i)?)?);
            }
            Ok(Value::Object(o))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let tag =
        if cols.iter().any(|s| s == "danbooru_tag") { "coalesce(nullif(danbooru_tag,''),char_id)" } else { "char_id" };
    Ok(
        json!({"characters":chars,"aliases":rows(c,"SELECT char_id,alias FROM character_aliases")?,"tags":rows(c,&format!("SELECT char_id,{tag} FROM characters"))?,"new_ids":{}}),
    )
}
#[derive(Default)]
struct Groups {
    names: Vec<String>,
    ids: HashMap<String, usize>,
    parent: Vec<usize>,
}
impl Groups {
    fn get(&mut self, u: &str) -> usize {
        if let Some(i) = self.ids.get(u) {
            *i
        } else {
            let i = self.names.len();
            self.names.push(u.into());
            self.ids.insert(u.into(), i);
            self.parent.push(i);
            i
        }
    }
    fn find(&mut self, mut i: usize) -> usize {
        while self.parent[i] != i {
            self.parent[i] = self.parent[self.parent[i]];
            i = self.parent[i];
        }
        i
    }
    fn join(&mut self, a: &str, b: &str) {
        let a = self.get(a);
        let a = self.find(a);
        let b = self.get(b);
        let b = self.find(b);
        self.parent[a] = b;
    }
    fn grouped(&mut self) -> Vec<Vec<String>> {
        let mut order = Vec::new();
        let mut groups = HashMap::<usize, Vec<String>>::new();
        for i in 0..self.names.len() {
            let root = self.find(i);
            if !groups.contains_key(&root) {
                order.push(root);
            }
            groups.entry(root).or_default().push(self.names[i].clone());
        }
        order.into_iter().map(|r| groups.remove(&r).expect("app_migrate_data internal invariant")).collect()
    }
}
pub(crate) fn duplicates(c: &Connection, src: &Connection, assets: &[Value]) -> Result<Vec<Value>> {
    let mut shape = HashMap::new();
    for r in assets {
        let id = r[0].as_str().ok_or_else(|| Error::Schema("无效资产 id".into()))?;
        shape.insert(id, (r[5].as_str().unwrap_or(""), r[3].as_i64().unwrap_or(0) * r[4].as_i64().unwrap_or(0)));
    }
    let kinds: Option<String> =
        c.query_row("SELECT value FROM prefs WHERE key='fold_kinds'", [], |r| r.get(0)).optional()?;
    let kinds: Value = match kinds {
        Some(k) => serde_json::from_str(&k).map_err(|e| Error::Invalid(e.to_string()))?,
        None => json!(["identical", "duplicate"]),
    };
    let mut linked = HashSet::new();
    let mut edges = Vec::new();
    for r in rows(c, "SELECT a,b,kind FROM relations")? {
        let (a, b) = (
            r[0].as_str().expect("validated internal text field"),
            r[1].as_str().expect("validated internal text field"),
        );
        if !kinds.as_array().is_some_and(|k| k.contains(&r[2])) {
            continue;
        }
        if let (Some((&a, _)), Some((&b, _))) = (shape.get_key_value(a), shape.get_key_value(b)) {
            edges.push((a, b));
            linked.extend([a, b]);
        }
    }
    let mut source_hashes = Vec::new();
    if src.prepare("SELECT stable_hash FROM assets LIMIT 0").is_ok() {
        let mut query = src.prepare("SELECT uuid,stable_hash FROM assets WHERE coalesce(stable_hash,'')<>''")?;
        let mut rows = query.query([])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            if let Some((&id, _)) = shape.get_key_value(id.as_str()) {
                source_hashes.push((id, row.get::<_, String>(1)?));
            }
        }
    }
    let mut first = HashMap::new();
    for (id, key) in &source_hashes {
        let (first_id, repeated) = first.entry(key.as_str()).or_insert((*id, false));
        *repeated |= *id != *first_id;
    }
    let mut groups = Groups::default();
    let mut hashes = HashMap::new();
    // 孤立且哈希唯一的图片不会形成副本组。仍按源库行顺序加入其余图片，维持分组的插入次序。
    for (id, key) in &source_hashes {
        let (first_id, repeated) = first[key.as_str()];
        if repeated || linked.contains(id) {
            groups.join(id, first_id);
            hashes.insert(*id, key.as_str());
        }
    }
    for (a, b) in edges {
        groups.join(a, b);
    }
    let mut out = Vec::new();
    for mut g in groups.grouped() {
        if g.len() < 2 {
            continue;
        }
        let key = g
            .iter()
            .filter_map(|u| hashes.get(u.as_str()))
            .min()
            .map(|key| (*key).to_owned())
            .unwrap_or_else(|| format!("sim:{}", g.iter().min().expect("app_migrate_data internal invariant")));
        g.sort_by(|a, b| (shape[a.as_str()].0, a).cmp(&(shape[b.as_str()].0, b)));
        let rep = g
            .iter()
            .min_by(|a, b| {
                let (a_stamp, a_area) = shape[a.as_str()];
                let (b_stamp, b_area) = shape[b.as_str()];
                (-a_area, a_stamp, a).cmp(&(-b_area, b_stamp, b))
            })
            .expect("app_migrate_data internal invariant");
        for (pos, u) in g.iter().enumerate() {
            out.push(json!([
                u,
                key,
                g.len(),
                pos + 1,
                shape[g.last().expect("app_migrate_data internal invariant").as_str()].0,
                u == rep
            ]));
        }
    }
    out
}
use rusqlite::OptionalExtension;
pub(crate) fn early_groups(c: &Connection, src: &Connection) -> Result<()> {
    let assets = rows(c, "SELECT uuid,filename,src_path,width,height,added_at,src_kind FROM assets")?;
    c.execute("DELETE FROM duplicates", [])?;
    for r in duplicates(c, src, &assets)? {
        c.execute(
            "INSERT INTO duplicates(uuid,grp,size,pos,grp_added,rep) VALUES (?,?,?,?,?,?)",
            params![r[0].as_str(), r[1].as_str(), r[2].as_i64(), r[3].as_i64(), r[4].as_str(), r[5].as_bool()],
        )?;
    }
    c.execute("DELETE FROM similar_groups", [])?;
    let present = assets.iter().filter_map(|r| r[0].as_str()).collect::<HashSet<_>>();
    let mut groups = Groups::default();
    for r in
        rows(c, "SELECT a,b FROM relations WHERE kind IN ('identical','duplicate','subtle','crop','variant','edit')")?
    {
        let (a, b) = (
            r[0].as_str().expect("validated internal text field"),
            r[1].as_str().expect("validated internal text field"),
        );
        if present.contains(a) && present.contains(b) {
            groups.join(a, b);
        }
    }
    let mut g = groups.grouped().into_iter().filter(|g| g.len() > 1).collect::<Vec<_>>();
    for g in &mut g {
        g.sort();
    }
    g.sort_by_key(|g| (-(g.len() as i64), g[0].clone()));
    for (i, g) in g.into_iter().enumerate() {
        for u in g {
            c.execute("INSERT INTO similar_groups VALUES (?,?)", params![u, i + 1])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_groups_keep_order_and_keys_from_hashes_and_relations() -> Result<()> {
        let app = Connection::open_in_memory()?;
        app.execute_batch(
            "CREATE TABLE prefs(key TEXT,value TEXT);
             CREATE TABLE relations(a TEXT,b TEXT,kind TEXT);
             INSERT INTO relations VALUES ('b','c','duplicate'), ('d','e','duplicate'),
                 ('f','a','crop'), ('b','absent','identical');",
        )?;
        let src = Connection::open_in_memory()?;
        src.execute_batch(
            "CREATE TABLE assets(uuid TEXT,stable_hash TEXT);
             INSERT INTO assets VALUES ('f','01'), ('c','02'), ('a','09'), ('b','09'), ('absent','00');",
        )?;
        let assets = [
            json!(["a", null, null, 4, 4, "2026-01", null]),
            json!(["b", null, null, 4, 4, "2026-02", null]),
            json!(["c", null, null, 5, 5, "2026-01", null]),
            json!(["d", null, null, 4, 4, "2026-02", null]),
            json!(["e", null, null, 4, 4, "2026-01", null]),
            json!(["f", null, null, 4, 4, "2026-01", null]),
        ];
        let only_relations =
            [json!(["e", "sim:d", 2, 1, "2026-02", true]), json!(["d", "sim:d", 2, 2, "2026-02", false])];
        assert_eq!(
            duplicates(&app, &src, &assets)?,
            [
                json!(["a", "02", 3, 1, "2026-02", false]),
                json!(["c", "02", 3, 2, "2026-02", true]),
                json!(["b", "02", 3, 3, "2026-02", false]),
                only_relations[0].clone(),
                only_relations[1].clone(),
            ]
        );
        assert_eq!(duplicates(&app, &src, &assets[2..])?, only_relations);
        Ok(())
    }
}
