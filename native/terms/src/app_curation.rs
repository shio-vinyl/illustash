//! 人工收藏和折叠投影，与 Store.curate/set_fold_kinds 一致。
use crate::{http::LibraryContext, http_json::Json, Error, Result};
use rusqlite::{params, params_from_iter, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

pub(crate) fn curate(c: &mut Connection, context: &LibraryContext, body: &Json) -> Result<Json> {
    let action = match body.get("action") {
        Some(Json::Scalar(Value::String(s))) if matches!(s.as_str(), "exclude" | "include" | "reset") => s.as_str(),
        _ => return Err(Error::Invalid("未知操作".into())),
    };
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    crate::schema::begin_writing(&tx)?;
    let ids = crate::app_commands::selected(&tx, context, body)?;
    // 新历史使用结构化紧凑 JSON；已有历史保持原文，撤销按解析后的字段恢复。
    let mut before = Vec::new();
    for id in &ids {
        let had: Option<(String, String, String)> = tx
            .query_row("SELECT status,origin,created_at FROM curation WHERE uuid=?", [id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?;
        let previous = match had {
            Some((status, origin, created)) => json!({"status":status,"origin":origin,"created_at":created}),
            None => Value::Null,
        };
        before.push(json!({"uuid":id,"curation":previous}));
        if action == "reset" {
            tx.execute("DELETE FROM curation WHERE uuid=?", [id])?;
        } else {
            tx.execute(
                "INSERT OR REPLACE INTO curation(uuid,status,origin) VALUES (?,?,'review')",
                params![id, format!("{action}d")],
            )?;
        }
    }
    refresh_excluded(&tx, &ids)?;
    let encoded = serde_json::to_string(&ids).map_err(|e| Error::Invalid(e.to_string()))?;
    let groups: HashMap<String, String> = tx
        .prepare("SELECT uuid,grp FROM duplicates WHERE uuid IN (SELECT value FROM json_each(?))")?
        .query_map([encoded], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let count = ids.iter().map(|id| groups.get(id).unwrap_or(id)).collect::<HashSet<_>>().len();
    let label = match action {
        "exclude" => format!("已排除 {n} 张", n = count),
        "include" => format!("已收录 {n} 张", n = count),
        _ => format!("已将 {n} 张交回自动规则", n = count),
    };
    tx.execute("INSERT INTO actions(label,before_json) VALUES (?,?)", params![label, json!(before).to_string()])?;
    let step = tx.last_insert_rowid();
    for id in &ids {
        use illustash_core::human_conclusions::state::{self, Domain};
        if action == "reset" {
            state::absent(&tx, Domain::Curation, id, step)?;
        } else {
            state::present(&tx, Domain::Curation, id, step)?;
        }
    }
    let undo = crate::app_commands::last_action(&tx)?;
    crate::schema::commit_writing(tx)?;
    Ok(Json::object([("undo", undo)]))
}
pub(crate) fn refresh_excluded(c: &Connection, ids: &[String]) -> Result<()> {
    c.execute_batch("CREATE TEMP TABLE IF NOT EXISTS touched(uuid TEXT PRIMARY KEY); DELETE FROM touched;")?;
    for id in ids {
        c.execute("INSERT OR IGNORE INTO touched VALUES (?)", [id])?;
    }
    c.execute("DELETE FROM excluded WHERE uuid IN (SELECT uuid FROM touched)", [])?;
    c.execute("INSERT INTO excluded SELECT a.uuid,coalesce(r.reason,'manual'),CASE WHEN k.status='excluded' THEN 'review' ELSE 'rule' END FROM assets a LEFT JOIN curation k ON k.uuid=a.uuid LEFT JOIN auto_rules r ON r.uuid=a.uuid AND r.kind='exclude' WHERE (k.status='excluded' OR (r.uuid IS NOT NULL AND k.uuid IS NULL)) AND a.uuid IN (SELECT uuid FROM touched)", [])?;
    Ok(())
}

pub(crate) fn fold(c: &mut Connection, context: &LibraryContext, body: &Json) -> Result<Json> {
    const ALLOWED: &[&str] = &["identical", "duplicate", "subtle", "crop", "variant", "edit"];
    let Some(Json::Array(values)) = body.get("kinds") else {
        return Err(Error::Invalid("未知关系".into()));
    };
    let mut kinds = Vec::new();
    for value in values {
        let Json::Scalar(Value::String(kind)) = value else {
            return Err(Error::Invalid("未知关系".into()));
        };
        if !ALLOWED.contains(&kind.as_str()) {
            return Err(Error::Invalid("未知关系".into()));
        }
        kinds.push(kind.clone());
    }
    kinds.sort();
    kinds.dedup();
    let path = context.source.as_ref().ok_or_else(|| Error::Io(std::io::Error::other("图库缺少源库")))?;
    let mut source = crate::sqlite::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.busy_timeout(Duration::from_secs(20))?;
    source.pragma_update(None, "query_only", true)?;
    let src = source.transaction()?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let persisted =
        crate::character_json::dump(&Value::Array(kinds.iter().cloned().map(Value::String).collect()), true);
    tx.execute("INSERT OR REPLACE INTO prefs VALUES ('fold_kinds',?)", [persisted])?;
    refresh_duplicates(&tx, &src, &kinds)?;
    crate::character_projection::fold(&tx)?;
    tx.commit()?;
    Ok(Json::object([("kinds", Json::Array(kinds.into_iter().map(Json::from).collect()))]))
}

/// 开库、发布会话前按新规则刷新曾存下的折叠结果。版本标记与派生表一起提交。
pub(crate) fn refresh_fold_on_open(library: &crate::app_library::Library) -> Result<()> {
    const NEEDS_REFRESH: &str = "SELECT EXISTS(SELECT 1 FROM duplicates)
            AND NOT EXISTS(SELECT 1 FROM meta WHERE key='fold_projection' AND value='2')";
    if library.frozen()
        || !crate::app_library::read_connection(&library.db())?.query_row(NEEDS_REFRESH, [], |r| r.get::<_, bool>(0))?
    {
        return Ok(());
    }
    let mut c = crate::sqlite::open_with_flags(library.db(), OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    crate::configure(&c)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if tx.query_row(NEEDS_REFRESH, [], |r| r.get::<_, bool>(0))? {
        crate::character_projection::fold(&tx)?;
        tx.execute("INSERT OR REPLACE INTO meta VALUES ('fold_projection','2')", [])?;
    }
    tx.commit()?;
    Ok(())
}

// 分组的插入顺序会通过 duplicates 的 rowid 体现出来（沿用旧实现）。并集运算先求右侧。
#[derive(Default)]
struct Parents {
    index: HashMap<String, usize>,
    ids: Vec<String>,
    links: Vec<usize>,
}
impl Parents {
    fn find(&mut self, id: &str) -> usize {
        let at = if let Some(at) = self.index.get(id) {
            *at
        } else {
            let at = self.ids.len();
            self.ids.push(id.into());
            self.links.push(at);
            self.index.insert(id.into(), at);
            at
        };
        let mut u = at;
        while self.links[u] != u {
            self.links[u] = self.links[self.links[u]];
            u = self.links[u];
        }
        u
    }
    fn join(&mut self, a: &str, b: &str) {
        let b = self.find(b);
        let a = self.find(a);
        self.links[a] = b;
    }
}
pub(crate) fn refresh_duplicates(c: &Connection, src: &Connection, kinds: &[String]) -> Result<()> {
    let assets: HashMap<String, (String, i128)> = c
        .prepare("SELECT uuid,coalesce(added_at,''),coalesce(width,0),coalesce(height,0) FROM assets")?
        .query_map([], |r| {
            Ok((r.get(0)?, (r.get(1)?, i128::from(r.get::<_, i64>(2)?) * i128::from(r.get::<_, i64>(3)?))))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut parent = Parents::default();
    let mut keys: HashMap<String, String> = HashMap::new();
    let has_hash = src
        .prepare("PRAGMA table_info(assets)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == "stable_hash");
    if has_hash {
        let mut first: HashMap<String, String> = HashMap::new();
        for row in src
            .prepare("SELECT uuid,stable_hash FROM assets WHERE coalesce(stable_hash,'')<>''")?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        {
            let (id, key) = row?;
            if assets.contains_key(&id) {
                let first = first.entry(key.clone()).or_insert_with(|| id.clone());
                parent.join(&id, first);
                keys.insert(id, key);
            }
        }
    }
    if !kinds.is_empty() {
        let marks = std::iter::repeat_n("?", kinds.len()).collect::<Vec<_>>().join(",");
        for row in c
            .prepare(&format!("SELECT a,b FROM relations WHERE kind IN ({marks})"))?
            .query_map(params_from_iter(kinds), |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        {
            let (a, b) = row?;
            if assets.contains_key(&a) && assets.contains_key(&b) {
                parent.join(&a, &b);
            }
        }
    }
    let mut roots = HashMap::new();
    let mut groups: Vec<Vec<String>> = Vec::new();
    for id in parent.ids.clone() {
        let root = parent.find(&id);
        let slot = *roots.entry(root).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[slot].push(id);
    }
    // 同键的组覆盖先前的值，但保留它在 dict 里的位置（沿用旧实现的行为）。
    let mut slots: HashMap<String, usize> = HashMap::new();
    let mut keyed: Vec<(String, Vec<String>)> = Vec::new();
    for group in groups {
        let key = group
            .iter()
            .filter_map(|id| keys.get(id))
            .min()
            .cloned()
            .unwrap_or_else(|| format!("sim:{}", group.iter().min().expect("nonempty")));
        if let Some(&slot) = slots.get(&key) {
            keyed[slot] = (key, group);
        } else {
            slots.insert(key.clone(), keyed.len());
            keyed.push((key, group));
        }
    }
    c.execute("DELETE FROM duplicates", [])?;
    for (key, mut members) in keyed {
        if members.len() < 2 {
            continue;
        }
        members.sort_by(|a, b| (&assets[a].0, a).cmp(&(&assets[b].0, b)));
        let best = members.iter().min_by_key(|id| (-assets[*id].1, &assets[*id].0, *id)).expect("nonempty");
        let added = &assets[members.last().expect("nonempty")].0;
        for (pos, id) in members.iter().enumerate() {
            c.execute(
                "INSERT INTO duplicates(uuid,grp,size,pos,grp_added,rep) VALUES (?,?,?,?,?,?)",
                params![id, key, members.len() as i64, pos as i64 + 1, added, id == best],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        root: PathBuf,
        c: Connection,
        context: LibraryContext,
    }
    impl Fixture {
        fn new(hash: bool) -> Self {
            let root = std::env::temp_dir().join(format!(
                "illustash-curation-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let db = root.join("library.sqlite");
            let source = root.join("source.sqlite");
            let mut c = crate::sqlite::open(&db).unwrap();
            c.execute_batch("CREATE TABLE assets(uuid TEXT PRIMARY KEY,added_at TEXT,width INTEGER,height INTEGER); INSERT INTO assets VALUES ('a','2026-01',10,10),('b','2026-02',20,20),('c','2026-02',20,20),('d','2025-01',NULL,NULL); CREATE TABLE duplicates(uuid TEXT PRIMARY KEY,grp TEXT NOT NULL,size INTEGER NOT NULL,pos INTEGER NOT NULL,grp_added TEXT NOT NULL,rep INTEGER NOT NULL,hidden INTEGER NOT NULL DEFAULT 0); CREATE INDEX duplicates_grp ON duplicates(grp,pos); CREATE TABLE curation(uuid TEXT PRIMARY KEY,status TEXT NOT NULL,origin TEXT NOT NULL,created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP); CREATE TABLE excluded(uuid TEXT PRIMARY KEY,reason TEXT,origin TEXT); CREATE TABLE auto_rules(uuid TEXT PRIMARY KEY,kind TEXT,reason TEXT); CREATE TABLE actions(id INTEGER PRIMARY KEY AUTOINCREMENT,label TEXT,before_json TEXT,ts TEXT DEFAULT CURRENT_TIMESTAMP,undone INTEGER DEFAULT 0,label_kind TEXT,label_count TEXT); CREATE TABLE prefs(key TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE relations(a TEXT,b TEXT,kind TEXT); CREATE TABLE memberships(uuid TEXT,entity TEXT); CREATE TABLE assertions(uuid TEXT,entity TEXT,status TEXT,origin TEXT,PRIMARY KEY(uuid,entity));").unwrap();
            crate::migrate(&mut c).unwrap();
            let src = crate::sqlite::open(&source).unwrap();
            if hash {
                src.execute_batch("CREATE TABLE assets(uuid TEXT PRIMARY KEY,stable_hash TEXT); INSERT INTO assets VALUES ('c','z'),('b','z'),('a','a'),('d',NULL),('not-in-library','z');").unwrap();
            } else {
                src.execute_batch(
                    "CREATE TABLE assets(uuid TEXT PRIMARY KEY); INSERT INTO assets VALUES ('a'),('b'),('c'),('d');",
                )
                .unwrap();
            }
            let context = LibraryContext {
                examples: None,
                db,
                generation: "synthetic".into(),
                library: Some(root.clone()),
                tags: root.join("unused.csv"),
                source: Some(source),
                token: Some("synthetic".into()),
                read_only: None,
            };
            Self { root, c, context }
        }
        fn body(s: &str) -> Json {
            crate::http_json::parse_request(s).unwrap()
        }
        fn count(&self, table: &str) -> i64 {
            self.c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn curate_history_is_structured_and_rules_have_human_precedence() {
        let mut f = Fixture::new(false);
        f.c.execute_batch("INSERT INTO auto_rules VALUES ('a','exclude','photo'),('b','exclude','text'); INSERT INTO curation VALUES ('a','excluded','人🙂','old'); INSERT INTO duplicates VALUES ('b','pair',2,1,'2026-02',1,0),('c','pair',2,2,'2026-02',0,1);").unwrap();
        let out = curate(&mut f.c, &f.context, &Fixture::body(r#"{"ids":["b","a"],"action":"include"}"#)).unwrap();
        assert_eq!(out.get("undo").unwrap().get("label").unwrap().text(), "已收录 2 张");
        let before: String = f.c.query_row("SELECT before_json FROM actions", [], |r| r.get(0)).unwrap();
        assert_eq!(
            before,
            r#"[{"curation":null,"uuid":"b"},{"curation":{"created_at":"old","origin":"人🙂","status":"excluded"},"uuid":"a"},{"curation":null,"uuid":"c"}]"#
        );
        assert_eq!(f.count("excluded"), 0);
        curate(&mut f.c, &f.context, &Fixture::body(r#"{"ids":["b","a"],"action":"reset"}"#)).unwrap();
        let rows: Vec<(i64, String, String, String)> =
            f.c.prepare("SELECT rowid,uuid,reason,origin FROM excluded ORDER BY rowid")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
        assert_eq!(
            rows,
            vec![(1, "a".into(), "photo".into(), "rule".into()), (2, "b".into(), "text".into(), "rule".into())]
        );
        assert_eq!(f.count("curation"), 0);
    }
    #[test]
    fn invalid_or_failed_curation_never_partially_commits() {
        let mut f = Fixture::new(false);
        assert!(
            matches!(curate(&mut f.c,&f.context,&Fixture::body(r#"{"action":true,"exclude":false}"#)),Err(Error::Invalid(m)) if m=="未知操作")
        );
        f.c.execute_batch("CREATE TRIGGER fail_history BEFORE INSERT ON actions BEGIN SELECT RAISE(ABORT,'synthetic history failure'); END;").unwrap();
        assert!(matches!(
            curate(&mut f.c, &f.context, &Fixture::body(r#"{"ids":["b","a"],"action":"exclude"}"#)),
            Err(Error::Sql(_))
        ));
        for table in ["curation", "excluded", "actions"] {
            assert_eq!(f.count(table), 0);
        }
    }
    #[test]
    fn fold_hashes_relations_representatives_mixed_visibility_and_rowids() {
        let mut f = Fixture::new(true);
        f.c.execute_batch("INSERT INTO relations VALUES ('a','b','duplicate'),('d','not-in-library','duplicate'); INSERT INTO memberships VALUES ('a','x'),('b','x'),('c','y');").unwrap();
        let bytes = fs::read(f.context.source.as_ref().unwrap()).unwrap();
        let out = fold(&mut f.c, &f.context, &Fixture::body(r#"{"kinds":["duplicate","duplicate"]}"#)).unwrap();
        assert_eq!(out.dump(), r#"{"kinds": ["duplicate"]}"#);
        let rows: Vec<(i64, String, String, i64, i64, i64)> =
            f.c.prepare("SELECT rowid,uuid,grp,pos,rep,hidden FROM duplicates ORDER BY rowid")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
        assert_eq!(
            rows,
            vec![
                (1, "a".into(), "a".into(), 1, 0, 1),
                (2, "b".into(), "a".into(), 2, 1, 0),
                (3, "c".into(), "a".into(), 3, 0, 1)
            ]
        );
        assert_eq!(bytes, fs::read(f.context.source.as_ref().unwrap()).unwrap());
        fold(&mut f.c, &f.context, &Fixture::body(r#"{"kinds":[]}"#)).unwrap();
        assert_eq!(f.count("duplicates"), 2); // 字节相同、只有机器结果不同的副本也折叠。
        assert_eq!(
            f.c.query_row("SELECT uuid FROM duplicates WHERE hidden=1", [], |r| r.get::<_, String>(0)).unwrap(),
            "c"
        );
        assert_eq!(f.count("actions"), 0);
    }
    #[test]
    fn reopens_old_folding_projection_once_without_touching_human_decisions() {
        let f = Fixture::new(false);
        let package = f.root.join("old-fold.pilib");
        crate::app_create::create_folder(&package, &f.root, None, None, None).unwrap();
        let library = crate::app_library::Library::open(&package).unwrap();
        let mut c = crate::sqlite::open(library.db()).unwrap();
        let tx = c.transaction().unwrap();
        crate::schema::begin_writing(&tx).unwrap();
        tx.execute_batch(
            "INSERT INTO assets(uuid,filename,src_path,src_kind,added_at,width,height) VALUES
                 ('manual','manual.png','manual.png','orig','2026-10-01',10,10),
                 ('large','large.png','large.png','orig','2026-10-02',20,20);
             INSERT INTO duplicates(uuid,grp,size,pos,grp_added,rep,hidden) VALUES
                 ('manual','same',2,1,'2026-10-02',0,0), ('large','same',2,2,'2026-10-02',1,0);",
        )
        .unwrap();
        crate::character_commands::command(&tx, &json!({"op":"create_entity","id":"a","name":"合成角色","lang":"zh"}))
            .unwrap();
        // 表示 0.4.0 时每份图各自的人工判断；旧缓存同时显示两份图。
        tx.execute("INSERT INTO assertions VALUES ('manual','a','confirmed','review')", []).unwrap();
        crate::schema::commit_writing(tx).unwrap();

        let claims = || {
            let mut rows = c.prepare("SELECT uuid,entity,status,origin FROM assertions ORDER BY uuid,entity").unwrap();
            rows.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
        };
        let before = claims();
        let (opened, owner) = crate::app_service::open_owned(&package, None, None).unwrap();
        assert_eq!(claims(), before);
        assert_eq!(crate::schema::version(&c).unwrap(), crate::SCHEMA_VERSION);
        assert_eq!(
            c.query_row("SELECT uuid FROM duplicates WHERE rep=1 AND hidden=0", [], |r| r.get::<_, String>(0)).unwrap(),
            "manual"
        );
        assert_eq!(
            c.query_row("SELECT uuid FROM duplicates WHERE hidden=1", [], |r| r.get::<_, String>(0)).unwrap(),
            "large"
        );
        assert_eq!(
            c.query_row("SELECT value FROM meta WHERE key='fold_projection'", [], |r| r.get::<_, String>(0)).unwrap(),
            "2"
        );
        // 用触发器确认再开时无需重写持久标记。重复打开应继续得到同样的代表图。
        c.execute_batch(
            "CREATE TRIGGER refresh_twice BEFORE INSERT ON meta WHEN new.key='fold_projection'
            BEGIN SELECT RAISE(ABORT, 'synthetic duplicate refresh'); END;",
        )
        .unwrap();
        drop((opened, owner));
        let _reopened = crate::app_service::open_owned(&package, None, None).unwrap();
        assert_eq!(claims(), before);
    }

    #[test]
    fn no_hash_relation_groups_preserve_python_rhs_insertion_order() {
        let mut f = Fixture::new(false);
        f.c.execute_batch("INSERT INTO relations VALUES ('a','b','crop'),('c','d','crop');").unwrap();
        fold(&mut f.c, &f.context, &Fixture::body(r#"{"kinds":["crop"]}"#)).unwrap();
        let rows: Vec<(String, String)> =
            f.c.prepare("SELECT uuid,grp FROM duplicates ORDER BY rowid")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
        assert_eq!(
            rows,
            vec![
                ("a".into(), "sim:a".into()),
                ("b".into(), "sim:a".into()),
                ("d".into(), "sim:c".into()),
                ("c".into(), "sim:c".into())
            ]
        );
        f.c.execute_batch("CREATE TRIGGER fail_duplicate BEFORE INSERT ON duplicates BEGIN SELECT RAISE(ABORT,'synthetic duplicate failure'); END;").unwrap();
        let before: String = f.c.query_row("SELECT value FROM prefs WHERE key='fold_kinds'", [], |r| r.get(0)).unwrap();
        assert!(matches!(
            fold(&mut f.c, &f.context, &Fixture::body(r#"{"kinds":["crop","edit"]}"#)),
            Err(Error::Sql(_))
        ));
        assert_eq!(f.count("duplicates"), 4);
        let current: String =
            f.c.query_row("SELECT value FROM prefs WHERE key='fold_kinds'", [], |r| r.get(0)).unwrap();
        assert_eq!(current, before);
        assert_eq!(before, r#"["crop"]"#);
    }
    #[test]
    fn bad_kinds_and_missing_source_do_not_write() {
        let mut f = Fixture::new(false);
        for text in [r#"{"kinds":null}"#, r#"{"kinds":[true]}"#, r#"{"kinds":["unknown"]}"#] {
            assert!(matches!(fold(&mut f.c,&f.context,&Fixture::body(text)),Err(Error::Invalid(m)) if m=="未知关系"));
        }
        fs::remove_file(f.context.source.as_ref().unwrap()).unwrap();
        assert!(matches!(fold(&mut f.c, &f.context, &Fixture::body(r#"{"kinds":[]}"#)), Err(Error::Sql(_))));
        assert_eq!(f.count("prefs"), 0);
    }
}
