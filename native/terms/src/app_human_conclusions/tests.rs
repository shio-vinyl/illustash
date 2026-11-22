use super::*;
use crate::{http::LibraryContext, http_json::Json};
use illustash_core::human_conclusions::state::{self, Domain, State};
use rusqlite::params;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn hybrid(c: &Connection) {
    c.execute("INSERT OR REPLACE INTO meta VALUES ('identity_policy','hybrid')", []).unwrap();
    // 当前 v8 身份布局保留原 hybrid 归属规则，entities 已由核心同形查询取代。
    c.execute_batch(
        &include_str!("../app_migration_sql/hybrid.sql")
            .replace("entities", illustash_core::character_entities::ENTITIES),
    )
    .unwrap();
}
struct Fixture {
    root: PathBuf,
    db: PathBuf,
    id: String,
}
impl Fixture {
    fn new(id: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "illustash-human-roundtrip-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let library = root.join("synthetic.pilib");
        crate::app_create::create_folder(&library, &root, Some("合成图库"), Some("other"), None).unwrap();
        let db = library.join("library.sqlite");
        Self { root, db, id: id.into() }
    }
    fn asset(&self, key: &str, filename: &str, color: u8) -> PathBuf {
        let path = self.root.join(filename);
        image::RgbImage::from_pixel(2, 2, image::Rgb([color, 10, 20])).save(&path).unwrap();
        let c = crate::open(&self.db).unwrap();
        c.execute(
            "INSERT INTO assets(uuid,filename,src_path,src_kind,added_at) VALUES (?1,?2,?3,'orig','2026-11-15')",
            params![key, filename, path.to_string_lossy()],
        )
        .unwrap();
        path
    }
    fn term(&self, key: &str, link: &str) {
        let c = crate::open(&self.db).unwrap();
        let term = crate::store::create_term(&c, crate::Kind::Artist, Some(key), "package:synthetic").unwrap();
        crate::store::add_link(&c, term, "danbooru", link, "", "synthetic").unwrap();
        crate::store::add_name(&c, term, "zh", "同名合成画师", "display", "synthetic").unwrap();
    }
    fn source(&self) {
        for (key, color) in [("a", 1), ("b", 2), ("c", 3), ("d", 4)] {
            self.asset(key, &format!("source-{key}.png"), color);
        }
        self.term("source-artist", "synthetic_artist");
        let mut c = crate::open(&self.db).unwrap();
        let tx = c.transaction().unwrap();
        crate::schema::begin_writing(&tx).unwrap();
        tx.execute("INSERT INTO actions(id,label,before_json) VALUES (1,'合成人工操作','[]')", []).unwrap();
        let term = crate::store::term_by_key(&tx, "source-artist").unwrap().unwrap().0;
        for (key, verdict) in [("a", "yes"), ("b", "no"), ("c", "clear")] {
            let asset = crate::store::library_asset(&tx, key).unwrap().unwrap();
            tx.execute("INSERT INTO term_verdicts(action_id,anchor,asset_id,term_id,verdict,origin,via,reason,created_at) VALUES (1,1,?1,?2,?3,'user','review','合成理由','2026-11-01')",
                params![asset,term,verdict]).unwrap();
        }
        tx.execute(
            "INSERT INTO curation(uuid,status,origin,created_at) VALUES ('a','included','review','2026-11-01')",
            [],
        )
        .unwrap();
        state::absent(&tx, Domain::Curation, "b", 1).unwrap();
        tx.execute("INSERT INTO deferred(uuid) VALUES ('c')", []).unwrap();
        state::absent(&tx, Domain::Deferred, "d", 1).unwrap();
        state::absent(&tx, Domain::Uncertain, "a", 1).unwrap();
        crate::schema::commit_writing(tx).unwrap();
    }
    fn target(&self) {
        for (key, color) in [("new-a", 1), ("new-b", 2), ("new-c", 3), ("new-d", 4)] {
            self.asset(key, &format!("moved-and-renamed-{color}.png"), color);
        }
        self.term("target-artist", "synthetic_artist");
        self.term("same-name-different-artist", "other_artist");
    }
    fn structured(&self, hybrid: bool) {
        self.source();
        let mut c = crate::open(&self.db).unwrap();
        let tx = c.transaction().unwrap();
        crate::schema::begin_writing(&tx).unwrap();
        if hybrid {
            self::hybrid(&tx);
        }
        illustash_core::character_entities::create(
            &tx,
            &illustash_core::character_entities::New { id: "source-work", kind: "work", ..Default::default() },
            "user",
        )
        .unwrap();
        crate::character_commands::command(&tx, &json!({"op":"create_entity","id":"source-parent","name":"合成原创角色","lang":"zh","kind":"oc","artist":"合成画师文字"})).unwrap();
        illustash_core::character_entities::create(
            &tx,
            &illustash_core::character_entities::New {
                id: "source-child",
                kind: "skin",
                parent_id: Some("source-parent"),
                variant: "合成变体",
                work_id: Some("source-work"),
                ..Default::default()
            },
            "user",
        )
        .unwrap();
        tx.execute(
            "UPDATE term_tree SET work_id='source-work' WHERE term_id=(SELECT id FROM terms WHERE key='source-parent')",
            [],
        )
        .unwrap();
        tx.execute("UPDATE term_tree SET review_status='reviewed',review_note='合成复核',parent_locked=1 WHERE term_id=(SELECT id FROM terms WHERE key='source-work')",[]).unwrap();
        if hybrid {
            crate::character_graph::rebuild(&tx).unwrap();
        }
        tx.execute("INSERT INTO reviews(entity,reviewed_at,action_id) VALUES ('source-parent','2026-11-01',1),('source-child','2026-11-02',1)",[]).unwrap();
        tx.execute("INSERT INTO term_verdicts(action_id,anchor,asset_id,term_id,verdict,origin,via)
            SELECT 1,1,a.id,t.id,'yes','user','review' FROM term_assets a,terms t WHERE a.key='a' AND t.key='source-parent'",[]).unwrap();
        tx.execute("INSERT INTO assertions VALUES ('a','source-parent','confirmed','review')", []).unwrap();
        tx.execute("INSERT INTO identity_scopes VALUES ('a','source-parent','[\"source-parent\"]')", []).unwrap();
        tx.execute("INSERT INTO identity_uncertain(uuid,options_json,reason) VALUES ('b','[\"source-parent\",\"source-child\"]','合成未定')",[]).unwrap();
        crate::character_projection::rebuild(&tx, None).unwrap();
        crate::schema::commit_writing(tx).unwrap();
    }
    fn export(&self) -> PathBuf {
        let snapshot = capture(&self.db, &self.id, &|| Ok(())).unwrap();
        assert_eq!(snapshot.coverage().verified, 4);
        let path = self.root.join("synthetic.illustash-human.jsonl");
        snapshot.write(&mut File::create(&path).unwrap()).unwrap();
        let data = fs::read_to_string(&path).unwrap();
        assert!(!data.contains(self.root.to_string_lossy().as_ref()));
        assert!(!data.contains("source-a.png"));
        path
    }
    fn context(&self) -> LibraryContext {
        LibraryContext {
            examples: None,
            db: self.db.clone(),
            generation: "synthetic".into(),
            library: Some(self.root.clone()),
            source: None,
            tags: self.root.join("unused.csv"),
            token: Some("synthetic".into()),
            read_only: None,
        }
    }
    fn undo(&self, id: i64) -> Result<Json> {
        crate::app_commands::request(
            &self.context(),
            "/api/undo",
            &crate::http_json::parse_request(&json!({"id":id}).to_string()).unwrap(),
        )
    }
    fn count(&self, sql: &str) -> i64 {
        crate::open(&self.db).unwrap().query_row(sql, [], |r| r.get(0)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn favorite_snapshot_restores_fixed_key_and_undo_allows_reusing_it() {
    use illustash_core::schema::FAVORITE_KEY;
    let set = |f: &Fixture, ids: &[&str], add: bool| {
        crate::app_commands::request(
            &f.context(),
            "/api/favorite",
            &crate::http_json::parse_request(&json!({"ids":ids,"add":add}).to_string()).unwrap(),
        )
        .unwrap()
    };
    let confirmed = |f: &Fixture| {
        f.count("SELECT count(*) FROM term_projection p JOIN terms t ON t.id=p.term_id WHERE t.kind='mark' AND p.state='confirmed'")
    };
    let source = Fixture::new("favorite-source");
    source.asset("a", "source-a.png", 1);
    source.asset("b", "source-b.png", 2);
    set(&source, &["a", "b"], true);
    set(&source, &["b"], false);
    let path = source.root.join("favorite.illustash-human.jsonl");
    let snapshot = capture(&source.db, &source.id, &|| Ok(())).unwrap();
    assert_eq!(snapshot.coverage().verified, 2);
    snapshot.write(&mut File::create(&path).unwrap()).unwrap();

    let target = Fixture::new("favorite-target");
    target.asset("moved-a", "target-a.png", 1);
    target.asset("moved-b", "target-b.png", 2);
    let restore = || import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    let result = restore();
    let action = result.action_id.unwrap();
    let c = crate::open(&target.db).unwrap();
    let (term, kind) = crate::store::term_by_key(&c, FAVORITE_KEY).unwrap().unwrap();
    assert_eq!(kind, crate::Kind::Mark);
    let a = crate::store::library_asset(&c, "moved-a").unwrap().unwrap();
    let b = crate::store::library_asset(&c, "moved-b").unwrap().unwrap();
    assert_eq!(crate::store::state(&c, a, term).unwrap().as_deref(), Some("confirmed"));
    assert!(crate::store::state(&c, b, term).unwrap().is_none());
    assert_eq!(target.count("SELECT count(*) FROM term_verdicts WHERE verdict='clear'"), 1);
    assert_eq!(restore().action_id, None);
    target.undo(action).unwrap();
    assert_eq!(confirmed(&target), 0);
    let again = restore().action_id.unwrap();
    assert_eq!(confirmed(&target), 1);
    target.undo(again).unwrap();
    set(&target, &["moved-b"], true);
    assert_eq!(confirmed(&target), 1);
    assert_eq!(target.count("SELECT count(*) FROM term_records WHERE kind='mark'"), 1);
}

#[test]
fn real_files_export_rebuild_import_and_ordinary_undo() {
    let source = Fixture::new("source-library");
    source.source();
    let path = source.export();
    let target = Fixture::new("rebuilt-library");
    target.target();
    let report = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert_eq!(report.records, BTreeMap::from([("applied".into(), 8)]));
    assert_eq!(report.assets["matched"], 4);
    assert_eq!(report.action_id, Some(1));
    let c = crate::open(&target.db).unwrap();
    for (asset, verdict) in [("new-a", "yes"), ("new-b", "no"), ("new-c", "clear")] {
        let row:(String,String,String,String)=c.query_row("SELECT t.key,v.verdict,v.reason,v.created_at FROM term_verdicts v JOIN terms t ON t.id=v.term_id JOIN term_assets a ON a.id=v.asset_id WHERE a.key=?1",[asset],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        assert_eq!(row, ("target-artist".into(), verdict.into(), "合成理由".into(), "2026-11-01".into()));
    }
    assert!(
        matches!(state::read(&c, Domain::Curation, "new-a").unwrap(), State::Positive(v) if v["status"]=="included")
    );
    for (domain, asset) in [(Domain::Curation, "new-b"), (Domain::Deferred, "new-d"), (Domain::Uncertain, "new-a")] {
        assert!(
            matches!(state::read(&c,domain,asset).unwrap(),State::Absent(m) if m.owner_action_id==1 && m.source_ref.is_some())
        );
    }
    assert!(matches!(state::read(&c, Domain::Deferred, "new-c").unwrap(), State::Positive(_)));
    let repeated = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert_eq!(repeated.action_id, None);
    assert_eq!(repeated.records, BTreeMap::from([("already_present".into(), 8)]));
    assert_eq!(target.count("SELECT count(*) FROM actions"), 1);
    target.undo(1).unwrap();
    for (domain, asset) in [
        (Domain::Curation, "new-a"),
        (Domain::Curation, "new-b"),
        (Domain::Deferred, "new-c"),
        (Domain::Deferred, "new-d"),
        (Domain::Uncertain, "new-a"),
    ] {
        assert_eq!(state::read(&c, domain, asset).unwrap(), State::Undecided);
    }
    assert_eq!(target.count("SELECT count(*) FROM term_effective_verdicts"), 0);
    assert_eq!(target.count("SELECT count(*) FROM term_verdicts"), 3);
    assert_eq!(target.count("SELECT count(*) FROM terms"), 2);
    assert_eq!(target.count("SELECT count(*) FROM actions WHERE undone=0"), 0);
    let reapplied = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert_eq!(reapplied.action_id, Some(2));
    assert_eq!(reapplied.records["applied"], 8);
    assert_eq!(target.count("SELECT count(*) FROM term_verdicts WHERE action_id=2"), 3);
}

#[test]
fn imported_definition_disappears_from_business_reads_and_reimport_owns_new_history() {
    let source = Fixture::new("source-library");
    source.source();
    let path = source.export();
    let target = Fixture::new("rebuilt-library");
    for (key, color) in [("new-a", 1), ("new-b", 2), ("new-c", 3), ("new-d", 4)] {
        target.asset(key, &format!("moved-{color}.png"), color);
    }
    let report = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert_eq!(report.records["applied"], 8);
    assert_eq!(report.definitions["applied"], 1);
    let c = crate::open(&target.db).unwrap();
    let first: i64 = c.query_row("SELECT id FROM terms", [], |r| r.get(0)).unwrap();
    assert_eq!(import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap().action_id, None);
    target.undo(report.action_id.unwrap()).unwrap();
    for table in ["terms", "term_relations", "term_names", "term_links", "term_projection", "term_effective_verdicts"] {
        assert_eq!(target.count(&format!("SELECT count(*) FROM {table}")), 0, "{table}");
    }
    assert_eq!(target.count("SELECT count(*) FROM term_records"), 1);
    assert_eq!(target.count("SELECT count(*) FROM term_verdicts"), 3);
    assert_eq!(target.count("SELECT count(*) FROM pragma_foreign_key_check"), 0);
    let empty = capture(&target.db, &target.id, &|| Ok(())).unwrap();
    let mut encoded = Vec::new();
    empty.write(&mut encoded).unwrap();
    assert_eq!(String::from_utf8(encoded).unwrap().lines().count(), 2);
    let repeated = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert_eq!(repeated.action_id, Some(2));
    let second: i64 = c.query_row("SELECT id FROM terms", [], |r| r.get(0)).unwrap();
    assert_ne!(first, second);
    assert_eq!(target.count("SELECT count(*) FROM actions WHERE id=1 AND undone=1"), 1);
    assert_eq!(target.count("SELECT count(*) FROM term_verdicts WHERE action_id=1"), 3);
    assert_eq!(target.count("SELECT count(*) FROM term_effective_verdicts WHERE action_id=1"), 0);
    assert_eq!(target.count("SELECT count(*) FROM term_links WHERE key='synthetic_artist'"), 1);
}

#[test]
fn independent_evidence_keeps_imported_definition_and_rolls_back_entire_undo() {
    let source = Fixture::new("source-library");
    source.source();
    let path = source.export();
    let target = Fixture::new("rebuilt-library");
    for (key, color) in [("new-a", 1), ("new-b", 2), ("new-c", 3), ("new-d", 4)] {
        target.asset(key, &format!("moved-{color}.png"), color);
    }
    let report = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    let mut c = crate::open(&target.db).unwrap();
    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    tx.execute(
        "INSERT INTO term_evidence(asset_id,term_id,source,method,version,fingerprint)
        SELECT a.id,t.id,'wd','synthetic','1',zeroblob(16) FROM term_assets a,terms t WHERE a.key='new-a'",
        [],
    )
    .unwrap();
    crate::schema::commit_writing(tx).unwrap();
    let before = capture(&target.db, &target.id, &|| Ok(())).unwrap().semantic_sha256().unwrap();
    assert!(matches!(target.undo(report.action_id.unwrap()), Err(Error::Conflict(_))));
    assert_eq!(capture(&target.db, &target.id, &|| Ok(())).unwrap().semantic_sha256().unwrap(), before);
    assert_eq!(target.count("SELECT count(*) FROM actions WHERE undone=0"), 1);
    assert_eq!(target.count("SELECT count(*) FROM curation"), 1);
    assert_eq!(target.count("SELECT count(*) FROM deferred"), 1);
    assert_eq!(target.count("SELECT count(*) FROM term_links"), 1);
}

#[test]
fn withdrawn_wd_extension_keeps_history_and_allows_import_undo() {
    use illustash_core::wd_extension;
    withdrawn_extension_keeps_history_and_allows_import_undo(
        |tx, tag| {
            wd_extension::apply(
                tx,
                "synthetic",
                &[wd_extension::Hit {
                    uuid: "image".into(),
                    tag: tag.into(),
                    probability: 0.95,
                    candidate_only: false,
                }],
            )
        },
        wd_extension::withdraw,
    );
}

#[test]
fn withdrawn_ccip_longtail_keeps_history_and_allows_import_undo() {
    use illustash_core::ccip_longtail;
    withdrawn_extension_keeps_history_and_allows_import_undo(
        |tx, tag| {
            ccip_longtail::apply(
                tx,
                "synthetic",
                &[ccip_longtail::Hit { uuid: "image".into(), tag: tag.into(), score: 0.95 }],
            )
        },
        ccip_longtail::withdraw,
    );
}

fn withdrawn_extension_keeps_history_and_allows_import_undo(
    apply: impl Fn(&Connection, &str) -> crate::Result<usize>,
    withdraw: impl Fn(&Connection) -> crate::Result<usize>,
) {
    use illustash_core::character_entities;

    let source = Fixture::new("source-library");
    let tag = "synthetic_character";
    let mut c = crate::open(&source.db).unwrap();
    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    character_entities::create(
        &tx,
        &character_entities::New { id: "source-character", kind: "character", ..Default::default() },
        "user",
    )
    .unwrap();
    let source_term = crate::store::term_by_key(&tx, "source-character").unwrap().unwrap().0;
    crate::store::add_link(&tx, source_term, "danbooru", tag, "", "user").unwrap();
    crate::schema::commit_writing(tx).unwrap();
    let path = source.root.join("synthetic.illustash-human.jsonl");
    capture(&source.db, &source.id, &|| Ok(())).unwrap().write(&mut File::create(&path).unwrap()).unwrap();

    let target = Fixture::new("target-library");
    target.asset("image", "synthetic.png", 1);
    let imported = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    let action = imported.action_id.unwrap();
    let first = target.count("SELECT id FROM terms");
    let mut c = crate::open(&target.db).unwrap();
    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    assert_eq!(apply(&tx, tag).unwrap(), 1);
    crate::schema::commit_writing(tx).unwrap();
    assert_eq!(target.count("SELECT term_id FROM term_current_evidence"), first);
    assert!(matches!(target.undo(action), Err(Error::Conflict(_))));
    assert_eq!(target.count("SELECT id FROM terms"), first);

    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    assert_eq!(withdraw(&tx).unwrap(), 1);
    crate::schema::commit_writing(tx).unwrap();
    target.undo(action).unwrap();
    for table in ["terms", "term_links", "term_current_evidence", "candidates", "memberships"] {
        assert_eq!(target.count(&format!("SELECT count(*) FROM {table}")), 0, "{table}");
    }
    assert_eq!(target.count("SELECT term_id FROM term_evidence JOIN term_records t ON t.id=term_id"), first);
    assert_eq!(target.count("SELECT count(*) FROM pragma_foreign_key_check"), 0);

    let reimported = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert!(reimported.action_id.is_some());
    assert_ne!(target.count("SELECT id FROM terms"), first);
    assert_eq!(target.count("SELECT count(*) FROM term_current_evidence"), 0);
    assert_eq!(target.count("SELECT term_id FROM term_evidence"), first);
}

#[test]
fn standalone_user_definition_imports_and_undoes_without_assets() {
    let source = Fixture::new("source-library");
    let c = crate::open(&source.db).unwrap();
    let id = crate::store::create_term(&c, crate::Kind::Concept, Some("synthetic-tag"), "user").unwrap();
    crate::store::add_name(&c, id, "zh", "合成标签", "alias", "user").unwrap();
    let path = source.root.join("standalone.illustash-human.jsonl");
    capture(&source.db, &source.id, &|| Ok(())).unwrap().write(&mut File::create(&path).unwrap()).unwrap();
    let target = Fixture::new("target-library");
    let report = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert!(report.records.is_empty());
    assert_eq!(report.definitions["applied"], 1);
    assert_eq!(target.count("SELECT count(*) FROM term_names WHERE value='合成标签' AND source='user'"), 1);
    target.undo(report.action_id.unwrap()).unwrap();
    assert_eq!(target.count("SELECT count(*) FROM terms"), 0);
    assert_eq!(target.count("SELECT count(*) FROM term_records"), 1);
}

#[test]
fn existing_names_import_by_group_and_undo_preserves_other_languages() {
    let source = Fixture::new("names-source");
    source.term("source", "synthetic_names");
    let c = crate::open(&source.db).unwrap();
    illustash_core::character_names::put_name(&c, "source", "zh", "user", "人工首选名", "display", 0, 1).unwrap();
    illustash_core::character_names::alias(&c, "source", "人工别名", "user", "zh").unwrap();
    let path = source.root.join("names.illustash-human.jsonl");
    capture(&source.db, &source.id, &|| Ok(())).unwrap().write(&mut File::create(&path).unwrap()).unwrap();
    let target = Fixture::new("names-target");
    target.term("target", "synthetic_names");
    let c = crate::open(&target.db).unwrap();
    illustash_core::character_names::put_name(&c, "target", "en", "user", "Existing name", "display", 0, 1).unwrap();
    let result = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert_eq!(result.records.len(), 0);
    assert_eq!(result.definition_fields["name_group_applied"], 2);
    assert_eq!(target.count("SELECT count(*) FROM term_names WHERE source='user'"), 3);
    assert_eq!(import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap().action_id, None);
    // 不属于本次组的新增语言可以保留；同组独立改动则整步原子拒绝。
    illustash_core::character_names::put_name(&c, "target", "ja", "user", "追加名", "display", 0, 1).unwrap();
    illustash_core::character_names::alias(&c, "target", "目标新增别名", "user", "zh").unwrap();
    assert!(target.undo(result.action_id.unwrap()).is_err());
    assert_eq!(target.count("SELECT count(*) FROM actions WHERE undone=1"), 0);
    c.execute("DELETE FROM term_names WHERE value='目标新增别名'", []).unwrap();
    target.undo(result.action_id.unwrap()).unwrap();
    assert_eq!(target.count("SELECT count(*) FROM term_names WHERE source='user' AND lang IN ('en','ja')"), 2);
    assert_eq!(target.count("SELECT count(*) FROM term_names WHERE source='user' AND lang='zh'"), 0);
    assert_eq!(target.count("SELECT count(*) FROM terms"), 1);
    assert!(import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap().action_id.is_some());
}

#[test]
fn ordinary_profile_undo_can_renumber_names_before_import_undo() {
    let source = Fixture::new("profile-source");
    let mut c = crate::open(&source.db).unwrap();
    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    linked_character(&tx, "profile", None);
    illustash_core::character_names::put_name(&tx, "profile", "zh", "user", "合成用户名字", "display", 0, 1).unwrap();
    illustash_core::character_names::alias(&tx, "profile", "合成用户别名", "user", "zh").unwrap();
    crate::schema::commit_writing(tx).unwrap();
    let path = source.root.join("profile.illustash-human.jsonl");
    capture(&source.db, &source.id, &|| Ok(())).unwrap().write(&mut File::create(&path).unwrap()).unwrap();
    for existed in [true, false] {
        let target = Fixture::new("profile-target");
        let mut c = crate::open(&target.db).unwrap();
        if existed {
            let tx = c.transaction().unwrap();
            crate::schema::begin_writing(&tx).unwrap();
            linked_character(&tx, "profile", None);
            crate::schema::commit_writing(tx).unwrap();
        }
        let imported = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
        let key: String = if existed {
            "profile".into()
        } else {
            c.query_row("SELECT target_key FROM human_entity_origins WHERE source_key='profile'", [], |r| r.get(0))
                .unwrap()
        };
        let old_seq: i64 = c
            .query_row(
                "SELECT max(seq) FROM term_names WHERE term_id=(SELECT id FROM terms WHERE key=?1)",
                [&key],
                |r| r.get(0),
            )
            .unwrap();
        target.term("unrelated", "synthetic_unrelated_profile");
        illustash_core::character_names::put_name(&c, "unrelated", "zh", "bangumi", "无关新名字", "display", 0, 0)
            .unwrap();
        let tx = c.transaction().unwrap();
        crate::schema::begin_writing(&tx).unwrap();
        let profile = illustash_core::character_profile::set_profile(&tx, &json!({"entity":key,"tag":""})).unwrap();
        crate::schema::commit_writing(tx).unwrap();
        target.undo(profile["action"].as_i64().unwrap()).unwrap();
        let new_seq: i64 = c
            .query_row(
                "SELECT min(seq) FROM term_names WHERE term_id=(SELECT id FROM terms WHERE key=?1)",
                [&key],
                |r| r.get(0),
            )
            .unwrap();
        assert!(new_seq > old_seq);
        target.undo(imported.action_id.unwrap()).unwrap();
        assert_eq!(target.count("SELECT count(*) FROM terms"), if existed { 2 } else { 1 });
        assert_eq!(target.count("SELECT count(*) FROM term_names WHERE source='user'"), 0);
    }
}

#[test]
fn imported_alias_on_existing_term_keeps_legacy_merge_provenance() {
    let source = Fixture::new("alias-source");
    let mut c = crate::open(&source.db).unwrap();
    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    linked_character(&tx, "alias-source-character", None);
    linked_character(&tx, "alias-target-character", None);
    illustash_core::character_names::put_name(
        &tx,
        "alias-source-character",
        "zh",
        "bangumi",
        "合并留下的别名",
        "display",
        0,
        0,
    )
    .unwrap();
    illustash_core::names_commands::one(&tx,&json!({"op":"danbooru_aliases","kind":"character","tag":"synthetic_alias-source-character","values":["可被更新器重建的别名"]})).unwrap();
    crate::character_commands::command(
        &tx,
        &json!({"op":"merge","source":"alias-source-character","target":"alias-target-character"}),
    )
    .unwrap();
    crate::schema::commit_writing(tx).unwrap();
    let path = source.root.join("aliases.illustash-human.jsonl");
    capture(&source.db, &source.id, &|| Ok(())).unwrap().write(&mut File::create(&path).unwrap()).unwrap();
    let target = Fixture::new("alias-target");
    let mut c = crate::open(&target.db).unwrap();
    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    linked_character(&tx, "alias-target-character", None);
    crate::schema::commit_writing(tx).unwrap();
    let result = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    // 中文旧合并别名可恢复；混有后台可重建 Danbooru 别名的另一组明确不处理。
    assert_eq!(result.definition_fields["name_group_applied"], 1);
    assert_eq!(result.definition_fields["name_group_unsupported_dependency"], 1);
    assert_eq!(target.count("SELECT count(*) FROM term_names WHERE source='danbooru'"), 0);
    assert_eq!(import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap().action_id, None);
    let mut bytes = Vec::new();
    capture(&target.db, &target.id, &|| Ok(())).unwrap().write(&mut bytes).unwrap();
    let rows: Vec<serde_json::Value> =
        bytes.split(|b| *b == b'\n').filter(|b| !b.is_empty()).map(|b| serde_json::from_slice(b).unwrap()).collect();
    let entity = rows.iter().find(|v| v["type"] == "entity" && v["record_key"] == "alias-target-character").unwrap();
    assert!(entity["definition"]["names"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["value"] == "合并留下的别名" && n["provenance"] == "legacy_merge" && n["source"] == "bangumi"));
    target.undo(result.action_id.unwrap()).unwrap();
    assert_eq!(target.count("SELECT count(*) FROM term_names WHERE role='alias'"), 0);
    assert_eq!(target.count("SELECT count(*) FROM terms"), 1);
    let refreshed_import = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    illustash_core::names_commands::one(&tx,&json!({"op":"danbooru_aliases","kind":"character","tag":"synthetic_alias-target-character","values":["可被更新器重建的别名"]})).unwrap();
    crate::schema::commit_writing(tx).unwrap();
    let mut bytes = Vec::new();
    capture(&target.db, &target.id, &|| Ok(())).unwrap().write(&mut bytes).unwrap();
    let updated: serde_json::Value = bytes
        .split(|b| *b == b'\n')
        .filter(|b| !b.is_empty())
        .map(|b| serde_json::from_slice::<serde_json::Value>(b).unwrap())
        .find(|v| v["type"] == "entity" && v["record_key"] == "alias-target-character")
        .unwrap();
    // 这行来自目标后台，既没有导入贡献也没有人工合并证明，不再作为人工别名导出。
    assert!(!updated["definition"]["names"].as_array().unwrap().iter().any(|n| n["value"] == "可被更新器重建的别名"));
    target.undo(refreshed_import.action_id.unwrap()).unwrap();
    assert_eq!(target.count("SELECT count(*) FROM term_names WHERE value='可被更新器重建的别名'"), 1);
}

#[test]
fn current_profile_link_clear_protects_the_slot_while_other_public_links_can_fill() {
    let source = Fixture::new("clear-source");
    let target = Fixture::new("clear-target");
    for (fixture, tag) in [(&source, "new-tag"), (&target, "")] {
        let mut c = crate::open(&fixture.db).unwrap();
        let tx = c.transaction().unwrap();
        crate::schema::begin_writing(&tx).unwrap();
        linked_character(&tx, "same-character", None);
        crate::character_profile::set_profile(&tx, &json!({"entity":"same-character","tag":tag})).unwrap();
        crate::schema::commit_writing(tx).unwrap();
    }
    let c = crate::open(&source.db).unwrap();
    let term = crate::store::term_by_key(&c, "same-character").unwrap().unwrap().0;
    crate::store::add_link(&c, term, "pixiv_user", "synthetic-public-link", "", "user").unwrap();
    let path = source.root.join("clear.illustash-human.jsonl");
    capture(&source.db, &source.id, &|| Ok(())).unwrap().write(&mut File::create(&path).unwrap()).unwrap();
    let result = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert_eq!(result.definition_fields["link_conflict"], 1);
    assert_eq!(result.definition_fields["link_applied"], 1);
    let detail = result.details.iter().find(|d| d.kind == "entity" && d.record_key == "same-character").unwrap();
    assert_eq!(detail.field_counts["link_conflict"], 1);
    assert_eq!(detail.field_counts["link_applied"], 1);
    assert_eq!(target.count("SELECT count(*) FROM term_links WHERE system='danbooru' AND current=1"), 0);
    target.undo(result.action_id.unwrap()).unwrap();
    assert_eq!(target.count("SELECT count(*) FROM term_links WHERE system='pixiv_user'"), 0);
}

#[test]
fn hash_cache_reuses_verified_versions_keeps_offline_history_and_invalidates_changes() {
    let fixture = Fixture::new("cache");
    fixture.source();
    let first = capture_mode(&fixture.db, &fixture.id, Mode::Cached, &|| Ok(())).unwrap();
    assert_eq!(first.snapshot.coverage().missing, 4);
    let filled = capture_mode(&fixture.db, &fixture.id, Mode::Fill, &|| Ok(())).unwrap();
    assert_eq!(filled.snapshot.coverage().verified, 4);
    let cached = capture_mode(&fixture.db, &fixture.id, Mode::Cached, &|| Ok(())).unwrap();
    assert_eq!(cached.snapshot.coverage().verified, 4);
    assert_eq!(cached.revision, filled.revision);
    fs::remove_file(fixture.root.join("source-a.png")).unwrap();
    let offline = capture(&fixture.db, &fixture.id, &|| Ok(())).unwrap();
    assert_eq!(offline.coverage().last_known, 1);
    let c = crate::open(&fixture.db).unwrap();
    c.execute("INSERT INTO meta(key,value) VALUES ('folder_hash:a','observed-new-version')", []).unwrap();
    assert_eq!(capture(&fixture.db, &fixture.id, &|| Ok(())).unwrap().coverage().missing, 1);
    c.execute("DELETE FROM assets WHERE uuid='b'", []).unwrap();
    assert_eq!(capture(&fixture.db, &fixture.id, &|| Ok(())).unwrap().coverage().last_known, 1);
    c.execute("INSERT INTO meta(key,value) VALUES ('folder_hash:b','new-removed-version')", []).unwrap();
    assert_eq!(capture(&fixture.db, &fixture.id, &|| Ok(())).unwrap().coverage().last_known, 0);
    image::RgbImage::from_pixel(3, 3, image::Rgb([9u8, 8, 7])).save(fixture.root.join("source-c.png")).unwrap();
    assert_eq!(
        capture_mode(&fixture.db, &fixture.id, Mode::Cached, &|| Ok(())).unwrap().snapshot.coverage().missing,
        3
    );
    fs::remove_file(fixture.root.join("source-c.png")).unwrap();
    assert_eq!(capture(&fixture.db, &fixture.id, &|| Ok(())).unwrap().coverage().last_known, 0);
}

#[test]
fn revisions_cover_unhistoried_structure_album_verdicts_and_undo_without_machine_noise() {
    let fixture = Fixture::new("revisions");
    fixture.source();
    let mut c = crate::open(&fixture.db).unwrap();
    let before = revision::read(&c).unwrap();
    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    tx.execute("INSERT INTO term_evidence(asset_id,term_id,source,method,version,fingerprint)
        SELECT a.id,t.id,'synthetic-machine','synthetic','1',zeroblob(16) FROM term_assets a,terms t WHERE a.key='a' AND t.key='source-artist'",[]).unwrap();
    tx.execute("INSERT INTO term_names(term_id,lang,value,role,source) SELECT id,'zh','机器显示名','display','synthetic' FROM terms WHERE key='source-artist'",[]).unwrap();
    crate::schema::commit_writing(tx).unwrap();
    assert_eq!(revision::read(&c).unwrap(), before);
    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    crate::character_commands::command(
        &tx,
        &json!({"op":"edit_work","action":"create","id":"new-work","name":"无历史作品","lang":"zh"}),
    )
    .unwrap();
    assert!(revision::read(&tx).unwrap() > before);
    tx.rollback().unwrap();
    assert_eq!(revision::read(&c).unwrap(), before);
    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    tx.execute("INSERT INTO term_batches(kind,anchor) VALUES ('album',1)", []).unwrap();
    let batch = tx.last_insert_rowid();
    tx.execute("INSERT INTO term_verdicts(batch_id,anchor,asset_id,term_id,verdict,origin,via)
        SELECT ?1,1,a.id,t.id,'yes','album','album' FROM term_assets a,terms t WHERE a.key='a' AND t.key='source-artist'",[batch]).unwrap();
    crate::schema::commit_writing(tx).unwrap();
    let album = revision::read(&c).unwrap();
    assert!(album > before);
    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    tx.execute("UPDATE actions SET undone=1 WHERE id=1", []).unwrap();
    assert!(revision::read(&tx).unwrap() > album);
}

#[test]
fn missing_public_links_and_resolvable_skips_roundtrip_without_overwriting_target_state() {
    let source = Fixture::new("links-source");
    let target = Fixture::new("links-target");
    source.term("a", "synthetic_link_a");
    source.term("b", "synthetic_link_b");
    target.term("new-a", "synthetic_link_a");
    target.term("new-b", "synthetic_link_b");
    let mut c = crate::open(&source.db).unwrap();
    c.execute("UPDATE term_links SET current=0 WHERE key='synthetic_link_a'", []).unwrap();
    let a = crate::store::term_by_key(&c, "a").unwrap().unwrap().0;
    crate::store::add_link(
        &c,
        a,
        "pixiv",
        "441000001",
        "https://example.invalid/private-query-is-not-exported",
        "user",
    )
    .unwrap();
    for (a, b) in [("a", "b"), ("unknown|opaque", "unresolved")] {
        crate::app_identity::request(
            &mut c,
            "/api/identity/skip",
            &crate::http_json::parse_request(&json!({"a":a,"b":b}).to_string()).unwrap(),
        )
        .unwrap();
    }
    let path = source.root.join("links.illustash-human.jsonl");
    capture(&source.db, &source.id, &|| Ok(())).unwrap().write(&mut File::create(&path).unwrap()).unwrap();
    assert!(!fs::read_to_string(&path).unwrap().contains("private-query"));
    let c = crate::open(&target.db).unwrap();
    let original = "[ \"keep|unrelated\" ]";
    c.execute("INSERT INTO prefs(key,value) VALUES ('identity_skipped',?1)", [original]).unwrap();
    let report = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert_eq!(report.records, BTreeMap::from([("applied".into(), 1), ("unsupported_dependency".into(), 1)]));
    assert_eq!(report.definition_fields["link_applied"], 1);
    assert_eq!(report.definition_fields["link_conflict"], 1);
    assert_eq!(target.count("SELECT count(*) FROM term_links WHERE key='synthetic_link_a' AND current=1"), 1);
    assert_eq!(target.count("SELECT count(*) FROM term_links WHERE system='pixiv' AND url=''"), 1);
    let skips: String = c.query_row("SELECT value FROM prefs WHERE key='identity_skipped'", [], |r| r.get(0)).unwrap();
    assert_eq!(serde_json::from_str::<Vec<String>>(&skips).unwrap(), vec!["keep|unrelated", "new-a|new-b"]);
    assert_eq!(import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap().action_id, None);
    target.undo(report.action_id.unwrap()).unwrap();
    assert_eq!(target.count("SELECT count(*) FROM term_links"), 2);
    assert_eq!(
        c.query_row("SELECT value FROM prefs WHERE key='identity_skipped'", [], |r| r.get::<_, String>(0)).unwrap(),
        original
    );
    assert_eq!(target.count("SELECT count(*) FROM terms"), 2);
}

fn structured_roundtrip(hybrid: bool) {
    let source = Fixture::new("structured-source");
    source.structured(hybrid);
    let path = source.export();
    let target = Fixture::new("structured-target");
    target.target();
    if hybrid {
        self::hybrid(&crate::open(&target.db).unwrap());
    }
    let imported = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert_eq!(imported.records, BTreeMap::from([("applied".into(), 13)]));
    assert_eq!(imported.definitions["applied"], 3);
    assert_eq!(target.count("SELECT count(*) FROM term_tree"), 3);
    assert_eq!(target.count("SELECT count(*) FROM reviews WHERE action_id=1"), 2);
    assert_eq!(target.count("SELECT count(*) FROM memberships WHERE uuid='new-a' AND status='confirmed'"), 1);
    assert_eq!(target.count("SELECT count(*) FROM term_tree WHERE original=1 AND artist='合成画师文字'"), 1);
    assert_eq!(target.count("SELECT count(*) FROM term_tree WHERE parent_locked=1 AND review_note='合成复核'"), 1);
    assert_eq!(import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap().action_id, None);
    target.undo(imported.action_id.unwrap()).unwrap();
    for table in [
        "term_tree",
        "term_identity_baseline",
        "reviews",
        "identity_scopes",
        "identity_uncertain",
        "memberships",
        "identity_canonical",
    ] {
        assert_eq!(target.count(&format!("SELECT count(*) FROM {table}")), 0, "{table}");
    }
    assert_eq!(target.count("SELECT count(*) FROM terms"), 2);
    assert_eq!(target.count("SELECT count(*) FROM pragma_foreign_key_check"), 0);
    let repeated = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert_eq!(repeated.action_id, Some(2));
    assert_eq!(repeated.records["applied"], 13);
    assert_eq!(target.count("SELECT count(*) FROM term_effective_verdicts WHERE action_id=1"), 0);
    let other = Fixture::new("different-policy");
    other.target();
    if !hybrid {
        self::hybrid(&crate::open(&other.db).unwrap());
    }
    let rejected = import_file(&other.db, &other.id, &path, &|| Ok(())).unwrap();
    assert_eq!(rejected.records["blocked_dependency"], 5);
    assert_eq!(rejected.records["applied"], 8);
    assert_eq!(other.count("SELECT count(*) FROM term_tree"), 0);
}

#[test]
fn structured_default_import_restores_names_tree_reviews_and_scope_then_undoes() {
    structured_roundtrip(false);
}

#[test]
fn structured_hybrid_import_preserves_original_baseline_and_policy_boundary() {
    structured_roundtrip(true);
}

fn family(f: &Fixture, parent_no: bool) {
    f.asset("image", "family.png", 51);
    let mut c = crate::open(&f.db).unwrap();
    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    hybrid(&tx);
    for (key, parent, kind) in [("parent", None, "character"), ("child", Some("parent"), "skin")] {
        illustash_core::character_entities::create(
            &tx,
            &illustash_core::character_entities::New {
                id: key,
                kind,
                parent_id: parent,
                variant: if parent.is_some() { "family_variant" } else { "" },
                ..Default::default()
            },
            "user",
        )
        .unwrap();
        let id = crate::store::term_by_key(&tx, key).unwrap().unwrap().0;
        crate::store::add_link(&tx, id, "danbooru", &format!("synthetic_{key}"), "", "synthetic").unwrap();
    }
    crate::character_graph::rebuild(&tx).unwrap();
    crate::character_commands::command(
        &tx,
        &json!({"op":"change","action":if parent_no {"rejected"} else {"confirmed"},
            "entity":if parent_no {"parent"}else{"child"},"asked":["image"]}),
    )
    .unwrap();
    crate::schema::commit_writing(tx).unwrap();
}

#[test]
fn imported_family_judgments_preserve_target_parent_rejection_and_child_confirmation() {
    for parent_no in [true, false] {
        let source = Fixture::new("family-source");
        family(&source, parent_no);
        let target = Fixture::new("family-target");
        family(&target, !parent_no);
        let before = capture(&target.db, &target.id, &|| Ok(())).unwrap();
        let memberships = target.count("SELECT count(*) FROM memberships WHERE status='confirmed'");
        assert_eq!(memberships, i64::from(parent_no));
        let path = source.root.join("family.illustash-human.jsonl");
        capture(&source.db, &source.id, &|| Ok(())).unwrap().write(&mut File::create(&path).unwrap()).unwrap();
        let result = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
        assert_eq!(result.action_id, None);
        assert!(result.records["blocked_dependency"] > 0);
        assert_eq!(target.count("SELECT count(*) FROM actions WHERE undone=0"), 1);
        assert_eq!(target.count("SELECT count(*) FROM memberships WHERE status='confirmed'"), memberships);
        let after = capture(&target.db, &target.id, &|| Ok(())).unwrap();
        let mut a = Vec::new();
        let mut b = Vec::new();
        before.write(&mut a).unwrap();
        after.write(&mut b).unwrap();
        let footer = |bytes: &[u8]| {
            serde_json::from_slice::<serde_json::Value>(bytes.split(|b| *b == b'\n').rfind(|s| !s.is_empty()).unwrap())
                .unwrap()["state_sha256"]
                .clone()
        };
        assert_eq!(footer(&a), footer(&b));
    }
}

fn linked_character(c: &Connection, key: &str, parent: Option<&str>) {
    illustash_core::character_entities::create(
        c,
        &illustash_core::character_entities::New {
            id: key,
            kind: if parent.is_some() { "skin" } else { "character" },
            parent_id: parent,
            variant: if parent.is_some() { "synthetic_child" } else { "" },
            ..Default::default()
        },
        "user",
    )
    .unwrap();
    let id = crate::store::term_by_key(c, key).unwrap().unwrap().0;
    crate::store::add_link(c, id, "danbooru", &format!("synthetic_{key}"), "", "synthetic").unwrap();
}

#[test]
fn family_conflicts_follow_existing_hybrid_representative() {
    for (source_parent_no, suspended) in [(true, false), (false, false), (true, true), (false, true)] {
        let source = Fixture::new("canonical-source");
        let target = Fixture::new("canonical-target");
        for (fixture, is_target) in [(&source, false), (&target, true)] {
            fixture.asset("image", "canonical.png", 63);
            let mut c = crate::open(&fixture.db).unwrap();
            let tx = c.transaction().unwrap();
            crate::schema::begin_writing(&tx).unwrap();
            tx.execute("INSERT INTO meta VALUES ('identity_policy','hybrid')", []).unwrap();
            linked_character(&tx, "Q", None);
            if is_target {
                linked_character(&tx, "P", None);
            }
            let parent_no = if is_target { !source_parent_no } else { source_parent_no };
            let parent = if is_target { "P" } else { "Q" };
            if !parent_no {
                linked_character(&tx, "C", Some(parent));
            }
            crate::character_graph::rebuild(&tx).unwrap();
            crate::character_commands::command(
                &tx,
                &json!({"op":"change","action":if parent_no {"rejected"}else{"confirmed"},
                "entity":if parent_no {parent}else{"C"},"asked":["image"]}),
            )
            .unwrap();
            if is_target {
                illustash_core::character_identity::command(&tx,&json!({"op":"decide","a":"P","b":"Q","kind":"duplicate_of","status":"accepted","reason":"合成代表更换"})).unwrap();
                if suspended {
                    tx.execute(
                        "INSERT INTO identity_suspended VALUES ('image',?,'equivalence_split',json_array('P','Q'))",
                        [if parent_no { parent } else { "C" }],
                    )
                    .unwrap();
                }
            }
            crate::schema::commit_writing(tx).unwrap();
        }
        let c = crate::open(&target.db).unwrap();
        let assertions = || {
            c.prepare(
                "SELECT s.entity,n.canonical_id,n.character_id,s.status
                 FROM assertions s JOIN identity_nodes n ON n.id=s.entity ORDER BY s.entity",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
        };
        let before = assertions();
        let (raw, canonical, status) = if source_parent_no { ("C", "C", "confirmed") } else { ("P", "Q", "rejected") };
        assert_eq!(before, vec![(raw.into(), canonical.into(), "Q".into(), status.into())]);
        let path = source.root.join("canonical.illustash-human.jsonl");
        capture(&source.db, &source.id, &|| Ok(())).unwrap().write(&mut File::create(&path).unwrap()).unwrap();
        let result = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
        if suspended {
            let action = result.action_id.expect("挂起的目标裁决不构成角色冲突");
            assert!(result.records["applied"] > 0);
            assert_eq!(target.count("SELECT count(*) FROM identity_suspended"), 1);
            assert!(assertions().iter().all(|row| row.2 == "Q"));
            target.undo(action).unwrap();
            assert_eq!(target.count("SELECT count(*) FROM identity_suspended"), 1);
        } else {
            assert_eq!(result.action_id, None);
            assert!(result.records["blocked_dependency"] > 0);
        }
        assert_eq!(target.count("SELECT count(*) FROM actions WHERE undone=0"), 2);
        assert_eq!(assertions(), before);
    }
}

#[test]
fn new_import_alias_cannot_change_existing_family_judgments() {
    for source_no in [true, false] {
        let source = Fixture::new("new-alias-source");
        let target = Fixture::new("new-alias-target");
        for (f, src) in [(&source, true), (&target, false)] {
            f.asset("image", "new-alias.png", 71);
            let mut c = crate::open(&f.db).unwrap();
            let tx = c.transaction().unwrap();
            crate::schema::begin_writing(&tx).unwrap();
            hybrid(&tx);
            linked_character(&tx, "Q", None);
            if src {
                linked_character(&tx, "A", None);
            } else if source_no {
                linked_character(&tx, "C", Some("Q"));
            }
            crate::character_graph::rebuild(&tx).unwrap();
            let entity = if src {
                "A"
            } else if source_no {
                "C"
            } else {
                "Q"
            };
            let no = if src { source_no } else { !source_no };
            crate::character_commands::command(
                &tx,
                &json!({"op":"change","action":if no {"rejected"}else{"confirmed"},"entity":entity,"asked":["image"]}),
            )
            .unwrap();
            if src {
                illustash_core::character_identity::command(&tx,&json!({"op":"decide","a":"A","b":"Q","kind":"duplicate_of","status":"accepted","reason":"合成新别名"})).unwrap();
            }
            crate::schema::commit_writing(tx).unwrap();
        }
        let path = source.root.join("new-alias.illustash-human.jsonl");
        capture(&source.db, &source.id, &|| Ok(())).unwrap().write(&mut File::create(&path).unwrap()).unwrap();
        let before = target.count("SELECT count(*) FROM memberships WHERE status='confirmed'");
        let result = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
        assert_eq!(result.action_id, None);
        assert!(result.records["blocked_dependency"] > 0);
        assert_eq!(target.count("SELECT count(*) FROM actions WHERE undone=0"), 1);
        assert_eq!(target.count("SELECT count(*) FROM memberships WHERE status='confirmed'"), before);
    }
}

#[test]
fn ordinary_identity_decision_reuses_undone_import_relation_with_existing_endpoints() {
    let source = Fixture::new("existing-relation-source");
    let target = Fixture::new("existing-relation-target");
    for fixture in [&source, &target] {
        let mut c = crate::open(&fixture.db).unwrap();
        let tx = c.transaction().unwrap();
        crate::schema::begin_writing(&tx).unwrap();
        hybrid(&tx);
        linked_character(&tx, "a", None);
        linked_character(&tx, "b", None);
        crate::character_graph::rebuild(&tx).unwrap();
        crate::schema::commit_writing(tx).unwrap();
    }
    let mut c = crate::open(&source.db).unwrap();
    crate::app_identity::request(
        &mut c,
        "/api/identity/decision",
        &crate::http_json::parse_request(
            &json!({"a":"a","b":"b","kind":"different_from","status":"accepted","reason":"合成关系"}).to_string(),
        )
        .unwrap(),
    )
    .unwrap();
    let path = source.root.join("existing-relation.illustash-human.jsonl");
    capture(&source.db, &source.id, &|| Ok(())).unwrap().write(&mut File::create(&path).unwrap()).unwrap();
    let result = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    target.undo(result.action_id.unwrap()).unwrap();
    assert_eq!(target.count("SELECT count(*) FROM terms"), 2);
    assert_eq!(target.count("SELECT count(*) FROM term_relations"), 0);
    let mut c = crate::open(&target.db).unwrap();
    crate::app_identity::request(
        &mut c,
        "/api/identity/decision",
        &crate::http_json::parse_request(
            &json!({"a":"a","b":"b","kind":"different_from","status":"accepted","reason":"合成普通复核"}).to_string(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(target.count("SELECT count(*) FROM term_legacy_ids WHERE kind='relation'"), 1);
    assert_eq!(target.count("SELECT count(*) FROM term_relations"), 1);
    assert_eq!(target.count("SELECT count(*) FROM term_effective_verdicts WHERE action_id=1"), 0);
    assert_eq!(target.count("SELECT count(*) FROM pragma_foreign_key_check"), 0);
}

#[test]
fn identity_relation_context_roundtrips_and_respects_local_review_audit() {
    let source = Fixture::new("relation-source");
    let mut c = crate::open(&source.db).unwrap();
    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    hybrid(&tx);
    for key in ["parent", "variant", "canonical"] {
        illustash_core::character_entities::create(
            &tx,
            &illustash_core::character_entities::New {
                id: key,
                kind: "character",
                variant: if key == "variant" { "synthetic_variant" } else { "" },
                ..Default::default()
            },
            "user",
        )
        .unwrap();
    }
    let relation =
        illustash_core::character_identity::propose(&tx, "variant", "parent", "form_of", None, "synthetic").unwrap();
    tx.execute("INSERT INTO actions(id,label,before_json) VALUES (1,'合成关系','[]')", []).unwrap();
    illustash_core::character_identity::decide(&tx, relation, "accepted", "合成决定", Some(1), "user").unwrap();
    let duplicate =
        illustash_core::character_identity::propose(&tx, "parent", "canonical", "duplicate_of", None, "synthetic")
            .unwrap();
    illustash_core::character_identity::decide(&tx, duplicate, "accepted", "合成代表选择", Some(1), "user").unwrap();
    // 新出现的旧层级证据尚未复核；便携文件只保留其指纹和审阅状态。
    illustash_core::character_identity::propose(
        &tx,
        "variant",
        "parent",
        "form_of",
        Some(&json!({"synthetic":true})),
        "legacy-hierarchy",
    )
    .unwrap();
    crate::character_graph::rebuild(&tx).unwrap();
    crate::schema::commit_writing(tx).unwrap();
    let path = source.root.join("relations.illustash-human.jsonl");
    capture(&source.db, &source.id, &|| Ok(())).unwrap().write(&mut File::create(&path).unwrap()).unwrap();
    let target = Fixture::new("relation-target");
    hybrid(&crate::open(&target.db).unwrap());
    let report = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert_eq!(report.records, BTreeMap::from([("applied".into(), 2)]));
    assert_eq!(target.count("SELECT count(*) FROM identity_canonical c JOIN human_entity_origins a ON a.target_key=c.entity
        JOIN human_entity_origins b ON b.target_key=c.canonical WHERE a.source_key='parent' AND b.source_key='canonical'"),1);
    assert_eq!(target.count("SELECT count(*) FROM term_evidence"), 0);
    assert_eq!(target.count("SELECT count(*) FROM human_relation_review_context WHERE reviewed=0"), 1);
    assert_eq!(import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap().action_id, None);
    let mut c = crate::open(&target.db).unwrap();
    let version = crate::character_graph::version(&c).unwrap();
    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    let reviewed = illustash_core::character_identity::command(
        &tx,
        &json!({"op":"decide","relation_id":1,"status":"accepted","reason":"合成复核"}),
    )
    .unwrap();
    crate::schema::commit_writing(tx).unwrap();
    assert_ne!(crate::character_graph::version(&c).unwrap(), version);
    let conflict = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert_eq!(conflict.action_id, None);
    assert_eq!(conflict.records["conflict"], 1);
    let reviewed_version = crate::character_graph::version(&c).unwrap();
    target.undo(reviewed["action"].as_i64().unwrap()).unwrap();
    // 普通关系撤销沿用现行“追加已审计决议”的合同，审计批次成为独立贡献。
    assert_eq!(crate::character_graph::version(&c).unwrap(), reviewed_version);
    assert!(target.undo(report.action_id.unwrap()).is_err());
    let target = Fixture::new("relation-undo-target");
    hybrid(&crate::open(&target.db).unwrap());
    let report = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    target.undo(report.action_id.unwrap()).unwrap();
    assert_eq!(target.count("SELECT count(*) FROM terms"), 0);
    assert_eq!(target.count("SELECT count(*) FROM term_relations"), 0);
    assert_eq!(target.count("SELECT count(*) FROM pragma_foreign_key_check"), 0);
    let repeated = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert!(repeated.action_id.unwrap() > report.action_id.unwrap());
}

#[test]
fn imported_multi_entity_reviews_survive_normal_merge_and_undo_without_moving_other_reviews() {
    let source = Fixture::new("review-source");
    let mut c = crate::open(&source.db).unwrap();
    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    tx.execute("INSERT INTO actions(id,label,before_json) VALUES (1,'合成检查','[]')", []).unwrap();
    for (key, at) in [("a", "2026-11-01"), ("b", "2026-11-02"), ("c", "2026-11-03")] {
        crate::character_commands::command(
            &tx,
            &json!({"op":"create_entity","id":key,"name":format!("合成角色 {key}"),"lang":"zh"}),
        )
        .unwrap();
        tx.execute("INSERT INTO reviews(entity,reviewed_at,action_id) VALUES (?1,?2,1)", params![key, at]).unwrap();
    }
    crate::schema::commit_writing(tx).unwrap();
    let path = source.root.join("reviews.illustash-human.jsonl");
    capture(&source.db, &source.id, &|| Ok(())).unwrap().write(&mut File::create(&path).unwrap()).unwrap();
    let target = Fixture::new("review-target");
    let imported = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert_eq!(imported.records["applied"], 3);
    let mut c = crate::open(&target.db).unwrap();
    let keys: BTreeMap<String, String> = c
        .prepare("SELECT source_key,target_key FROM human_entity_origins")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    let merged =
        crate::character_commands::command(&tx, &json!({"op":"merge","source":keys["a"],"target":keys["b"]})).unwrap();
    crate::schema::commit_writing(tx).unwrap();
    assert_eq!(target.count("SELECT count(*) FROM reviews"), 2);
    target.undo(merged["action"].as_i64().unwrap()).unwrap();
    for (key, at) in [("a", "2026-11-01"), ("b", "2026-11-02"), ("c", "2026-11-03")] {
        let actual: String = c
            .query_row("SELECT reviewed_at FROM reviews WHERE action_id=1 AND entity=?1", [&keys[key]], |r| r.get(0))
            .unwrap();
        assert_eq!(actual, at);
    }
    target.undo(imported.action_id.unwrap()).unwrap();
    assert_eq!(target.count("SELECT count(*) FROM reviews"), 0);
    assert_eq!(target.count("SELECT count(*) FROM terms"), 0);
}

#[test]
fn target_human_state_wins_and_independent_change_refuses_undo_atomically() {
    let source = Fixture::new("source-library");
    source.source();
    let path = source.export();
    let target = Fixture::new("target-library");
    target.target();
    let c = crate::open(&target.db).unwrap();
    c.execute("INSERT INTO curation VALUES ('new-a','excluded','review','2026-11-10')", []).unwrap();
    let report = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert_eq!(report.records["conflict"], 1);
    assert_eq!(report.records["applied"], 7);
    assert!(matches!(state::read(&c,Domain::Curation,"new-a").unwrap(),State::Positive(v) if v["status"]=="excluded"));
    c.execute("DELETE FROM deferred WHERE uuid='new-c'", []).unwrap();
    let before = capture(&target.db, &target.id, &|| Ok(())).unwrap().semantic_sha256().unwrap();
    assert!(matches!(target.undo(1), Err(Error::Conflict(_))));
    assert_eq!(capture(&target.db, &target.id, &|| Ok(())).unwrap().semantic_sha256().unwrap(), before);
    assert_eq!(target.count("SELECT count(*) FROM actions WHERE undone=0"), 1);
    assert_eq!(target.count("SELECT count(*) FROM human_state_marks"), 3);
}

#[test]
fn same_library_replaced_or_offline_id_cannot_redirect_conclusions_to_a_copy() {
    let source = Fixture::new("same-library");
    source.source();
    let path = source.export();
    let target = Fixture::new("same-library");
    let replaced = target.asset("a", "replaced.png", 9);
    target.asset("copy-a", "old-content-copy.png", 1);
    target.term("source-artist", "synthetic_artist");
    let changed = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert_eq!(changed.action_id, None);
    assert_eq!(changed.assets["content_changed"], 1);
    assert_eq!(changed.records["content_changed"], 3);
    assert_eq!(target.count("SELECT count(*) FROM actions"), 0);
    fs::remove_file(replaced).unwrap();
    let offline = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert_eq!(offline.action_id, None);
    assert_eq!(offline.records["unverified_asset"], 8);
    assert_eq!(
        offline
            .details
            .iter()
            .filter(|d| d.kind != "asset" && d.reason.as_deref() == Some("same_id_unverified"))
            .count(),
        3
    );
    assert_eq!(
        offline
            .details
            .iter()
            .filter(|d| d.kind == "asset" && d.reason.as_deref() == Some("same_id_unverified"))
            .count(),
        1
    );
    assert_eq!(target.count("SELECT count(*) FROM actions"), 0);
    assert_eq!(target.count("SELECT count(*) FROM term_verdicts"), 0);
    // 原 ID 真正离开候选集后，才允许按唯一内容认回。
    crate::open(&target.db).unwrap().execute("DELETE FROM assets WHERE uuid='a'", []).unwrap();
    let absent = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert_eq!(absent.records["applied"], 3);
    assert_eq!(target.count("SELECT count(*) FROM curation WHERE uuid='copy-a'"), 1);
}

#[test]
fn generic_cli_refuses_human_import_without_partially_undoing_exclusion() {
    let source = Fixture::new("source-library");
    source.source();
    crate::open(&source.db).unwrap().execute("UPDATE curation SET status='excluded' WHERE uuid='a'", []).unwrap();
    let path = source.export();
    let target = Fixture::new("target-library");
    target.target();
    assert_eq!(import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap().action_id, Some(1));
    assert_eq!(target.count("SELECT count(*) FROM excluded WHERE uuid='new-a'"), 1);
    let code = crate::cli::run(&[
        "undo".into(),
        "--db".into(),
        target.db.to_string_lossy().into_owned(),
        "--action".into(),
        "1".into(),
    ]);
    assert_eq!(code, 3);
    assert_eq!(target.count("SELECT count(*) FROM actions WHERE undone=0"), 1);
    assert_eq!(target.count("SELECT count(*) FROM curation WHERE status='excluded' AND uuid='new-a'"), 1);
    assert_eq!(target.count("SELECT count(*) FROM excluded WHERE uuid='new-a'"), 1);
    target.undo(1).unwrap();
    assert_eq!(target.count("SELECT count(*) FROM curation"), 0);
    assert_eq!(target.count("SELECT count(*) FROM excluded"), 0);
}

#[test]
fn incomplete_candidates_and_cancellation_do_not_create_partial_actions() {
    let source = Fixture::new("source-library");
    source.source();
    let path = source.export();
    let target = Fixture::new("target-library");
    target.target();
    let missing = target.asset("offline", "offline.png", 1);
    fs::remove_file(missing).unwrap();
    let report = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
    assert_eq!(report.records["unverified_asset"], 8);
    assert_eq!(report.action_id, None);
    assert_eq!(target.count("SELECT count(*) FROM actions"), 0);
    let c = crate::open(&target.db).unwrap();
    c.execute("DELETE FROM assets WHERE uuid='offline'", []).unwrap();
    let cancelled = std::cell::Cell::new(false);
    let check = || {
        // 读文件阶段的取消不产生空历史。
        if cancelled.replace(true) {
            Err(Error::Conflict("合成取消".into()))
        } else {
            Ok(())
        }
    };
    assert!(import_file(&target.db, &target.id, &path, &check).is_err());
    assert_eq!(target.count("SELECT count(*) FROM actions"), 0);
    assert_eq!(target.count("SELECT count(*) FROM term_verdicts"), 0);
    c.execute_batch("CREATE TRIGGER synthetic_failure BEFORE INSERT ON term_verdicts BEGIN SELECT RAISE(ABORT,'synthetic failure'); END").unwrap();
    assert!(import_file(&target.db, &target.id, &path, &|| Ok(())).is_err());
    // 裁决写入之前已处理过收录与跳过，后续 SQL 失败必须连同历史、标记一起回滚。
    for table in ["actions", "curation", "deferred", "human_state_marks", "human_state_mark_undo"] {
        assert_eq!(target.count(&format!("SELECT count(*) FROM {table}")), 0);
    }
}

#[test]
fn publication_preserves_existing_files_and_rotation_checks_only_owned_valid_snapshots() {
    let fixture = Fixture::new("files");
    let directory = files::directory(&fixture.db);
    let snapshot = capture(&fixture.db, &fixture.id, &|| Ok(())).unwrap();
    let destination = files::path(&directory, &snapshot.header().snapshot_id).unwrap();
    files::Prepared::new(&snapshot, &destination, &|| Ok(())).unwrap().publish().unwrap();
    let original = fs::read(&destination).unwrap();
    assert!(files::Prepared::new(&snapshot, &destination, &|| Ok(())).unwrap().publish().is_err());
    assert_eq!(fs::read(&destination).unwrap(), original);
    assert!(files::Prepared::new(&snapshot, &directory.join("cancelled.jsonl"), &|| Err(Error::Conflict(
        "cancelled".into()
    )))
    .is_err());
    for _ in 0..30 {
        let snapshot = capture(&fixture.db, &fixture.id, &|| Ok(())).unwrap();
        let destination = files::path(&directory, &snapshot.header().snapshot_id).unwrap();
        files::Prepared::new(&snapshot, &destination, &|| Ok(())).unwrap().publish().unwrap();
    }
    fs::write(directory.join("library-original.sqlite"), b"unrelated").unwrap();
    fs::write(directory.join("human-ffff.illustash-human.jsonl"), b"incomplete").unwrap();
    let mut catalog = files::list(&directory, &fixture.id, &|| Ok(())).unwrap();
    assert_eq!(catalog.items.len(), 31);
    assert_eq!(catalog.unreadable, 1);
    assert!(files::prune(&directory, &mut catalog, 30, &|| Ok(())).is_none());
    assert_eq!(catalog.items.len(), 30);
    assert!(directory.join("library-original.sqlite").exists());
    assert!(directory.join("human-ffff.illustash-human.jsonl").exists());
    let old = files::path(&directory, &catalog.items.last().unwrap().backup_id).unwrap();
    fs::write(&old, b"changed independently").unwrap();
    assert!(files::prune(&directory, &mut catalog, 29, &|| Ok(())).is_some());
    assert_eq!(fs::read(old).unwrap(), b"changed independently");
    assert!(!fs::read_dir(directory).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".tmp")));
}

#[test]
fn page_curation_waits_for_human_import_publication() {
    use std::{sync::mpsc, thread, time::Duration};

    struct Pause<'a> {
        locked: mpsc::Sender<()>,
        release: &'a mpsc::Receiver<()>,
    }
    impl crate::app_imports::Observer for Pause<'_> {
        fn publish(&self, apply: &mut dyn FnMut() -> Result<()>) -> Result<()> {
            self.locked.send(()).unwrap();
            self.release.recv_timeout(crate::test_wait::limit()).unwrap();
            apply()
        }
    }

    let source = Fixture::new("busy-source");
    source.source();
    let path = source.export();
    let target = Fixture::new("busy-target");
    target.target();
    let context = target.context();

    let (locked, in_publication) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    let (result, finished) = mpsc::channel();
    thread::scope(|scope| {
        let importer = scope.spawn(move || {
            import_run(
                &target.db,
                &target.id,
                &path,
                &|| Ok(()),
                &Pause { locked, release: &resume },
                "blocked-report",
                None,
            )
            .map(|(report, _)| (target, report))
            .unwrap()
        });
        in_publication.recv_timeout(crate::test_wait::limit()).unwrap();
        let body = crate::http_json::parse_request(r#"{"ids":["new-d"],"action":"exclude"}"#).unwrap();
        let db = context.db.clone();
        scope.spawn(move || result.send(crate::app_commands::request(&context, "/api/curate", &body)).unwrap());

        // 原有五秒等待在同一把锁上确实耗尽。再留一个短观察窗口，确保页面未提前失败。
        let probe = crate::sqlite::open(db).unwrap();
        crate::configure(&probe).unwrap();
        let busy = probe.execute_batch("BEGIN IMMEDIATE; ROLLBACK;").unwrap_err();
        assert_eq!(busy.sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseBusy));
        let early = finished.recv_timeout(Duration::from_secs(1));
        release.send(()).unwrap();
        assert!(matches!(early, Err(mpsc::RecvTimeoutError::Timeout)), "{early:?}");

        let (target, report) = importer.join().unwrap();
        let reply = finished.recv_timeout(crate::test_wait::limit()).unwrap().unwrap();
        assert_eq!(report.action_id, Some(1));
        assert_eq!(reply.field("undo").field("id").integer(), 2);
        assert_eq!(target.count("SELECT count(*) FROM actions"), 2);
        assert_eq!(target.count("SELECT count(*) FROM curation WHERE uuid='new-d' AND status='excluded'"), 1);
        assert_eq!(target.count("SELECT count(*) FROM curation WHERE uuid='new-a' AND status='included'"), 1);
        target.undo(2).unwrap();
        assert_eq!(target.count("SELECT count(*) FROM actions WHERE undone=0"), 1);
        assert_eq!(target.count("SELECT count(*) FROM curation WHERE uuid='new-d'"), 0);
    });
}

#[test]
fn denied_publication_rolls_back_conclusions_cache_and_reports_together() {
    struct Cancel;
    impl crate::app_imports::Observer for Cancel {
        fn publish(&self, _apply: &mut dyn FnMut() -> Result<()>) -> Result<()> {
            Err(Error::Conflict("cancelled before publication".into()))
        }
    }
    let source = Fixture::new("cancel-source");
    source.source();
    let path = source.root.join("cancel.illustash-human.jsonl");
    capture(&source.db, &source.id, &|| Ok(())).unwrap().write(&mut File::create(&path).unwrap()).unwrap();
    let target = Fixture::new("cancel-target");
    target.target();
    assert!(import_run(&target.db, &target.id, &path, &|| Ok(()), &Cancel, "cancelled-report", None).is_err());
    for table in ["actions", "term_verdicts", "human_asset_hashes", "human_import_reports", "human_import_report_rows"]
    {
        assert_eq!(target.count(&format!("SELECT count(*) FROM {table}")), 0, "{table}");
    }
}

#[test]
fn persisted_import_report_has_every_record_after_the_first_page() {
    let source = Fixture::new("report-source");
    let mut c = crate::open(&source.db).unwrap();
    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    for n in 0..503 {
        crate::store::create_term(&tx, crate::Kind::Concept, Some(&format!("synthetic-{n}")), "user").unwrap();
    }
    crate::schema::commit_writing(tx).unwrap();
    let path = source.root.join("report.illustash-human.jsonl");
    capture(&source.db, &source.id, &|| Ok(())).unwrap().write(&mut File::create(&path).unwrap()).unwrap();
    let target = Fixture::new("report-target");
    let (report, summary) =
        import_run(&target.db, &target.id, &path, &|| Ok(()), &crate::app_imports::Silent, "full-report", None)
            .unwrap();
    assert!(report.action_id.is_some());
    assert_eq!(report.details.len(), 503);
    assert_eq!(summary["detail_count"], 503);
    let page: serde_json::Value =
        serde_json::from_str(&reports::page(&target.db, "full-report", 500, 500).unwrap().dump()).unwrap();
    assert_eq!(page["details"].as_array().unwrap().len(), 3);
    let (repeat, _) =
        import_run(&target.db, &target.id, &path, &|| Ok(()), &crate::app_imports::Silent, "repeat-report", None)
            .unwrap();
    assert!(repeat.action_id.is_none());
    assert_eq!(target.count("SELECT count(*) FROM human_import_reports"), 2);
}

#[test]
fn snapshot_does_not_protect_a_human_edit_committed_during_its_read() {
    let fixture = Fixture::new("concurrent-revision");
    fixture.source();
    let c = crate::open(&fixture.db).unwrap();
    let before = revision::read(&c).unwrap();
    let changed = std::cell::Cell::new(false);
    let captured = capture_mode(&fixture.db, &fixture.id, Mode::Fill, &|| {
        if !changed.replace(true) {
            let mut writer = crate::open(&fixture.db)?;
            let tx = writer.transaction()?;
            crate::schema::begin_writing(&tx)?;
            crate::character_commands::command(
                &tx,
                &json!({"op":"edit_work","action":"create","id":"concurrent-work","name":"并发新增作品","lang":"zh"}),
            )?;
            crate::schema::commit_writing(tx)?;
        }
        Ok(())
    })
    .unwrap();
    assert!(changed.get());
    assert_eq!(captured.revision, before);
    assert!(revision::read(&c).unwrap() > captured.revision);
    assert!(!captured.snapshot.record_keys().any(|(_, key)| key == "concurrent-work"));
    let next = capture_mode(&fixture.db, &fixture.id, Mode::Cached, &|| Ok(())).unwrap();
    assert_eq!(next.revision, revision::read(&c).unwrap());
    assert!(next.snapshot.record_keys().any(|(_, key)| key == "concurrent-work"));
}

#[test]
fn provider_refresh_reusing_max_seq_is_never_owned_by_portable_undo() {
    let source = Fixture::new("provider-source");
    let mut c = crate::open(&source.db).unwrap();
    let tx = c.transaction().unwrap();
    crate::schema::begin_writing(&tx).unwrap();
    linked_character(&tx, "provider-character", None);
    crate::character_names::put_name(&tx, "provider-character", "zh", "bangumi", "提供方显示名", "display", 0, 0)
        .unwrap();
    crate::names_commands::one(
        &tx,
        &json!({"op":"danbooru_aliases","kind":"character","tag":"synthetic_provider-character","values":["后台别名"]}),
    )
    .unwrap();
    crate::schema::commit_writing(tx).unwrap();
    let path = source.root.join("provider.illustash-human.jsonl");
    capture(&source.db, &source.id, &|| Ok(())).unwrap().write(&mut File::create(&path).unwrap()).unwrap();
    for existed in [false, true] {
        let target = Fixture::new(if existed { "provider-existing" } else { "provider-new" });
        let mut c = crate::open(&target.db).unwrap();
        if existed {
            let tx = c.transaction().unwrap();
            crate::schema::begin_writing(&tx).unwrap();
            linked_character(&tx, "provider-character", None);
            crate::schema::commit_writing(tx).unwrap();
        }
        let result = import_file(&target.db, &target.id, &path, &|| Ok(())).unwrap();
        assert_eq!(target.count("SELECT count(*) FROM term_names WHERE source IN ('bangumi','danbooru')"), 0);
        if !existed {
            assert_eq!(result.definition_fields["name_rows_unsupported_dependency"], 1);
        }
        let refresh = |c: &mut Connection| {
            let tx = c.transaction().unwrap();
            crate::schema::begin_writing(&tx).unwrap();
            crate::names_commands::one(&tx,&json!({"op":"danbooru_aliases","kind":"character","tag":"synthetic_provider-character","values":["后台别名"]})).unwrap();
            crate::schema::commit_writing(tx).unwrap();
        };
        refresh(&mut c);
        let before = target.count("SELECT seq FROM term_names WHERE source='danbooru'");
        assert_eq!(before, target.count("SELECT max(seq) FROM term_names"));
        refresh(&mut c);
        assert_eq!(before, target.count("SELECT seq FROM term_names WHERE source='danbooru'"));
        if let Some(action) = result.action_id {
            assert!(target.undo(action).is_err());
        }
        assert_eq!(target.count("SELECT count(*) FROM term_names WHERE source='danbooru'"), 1);
        assert_eq!(target.count("SELECT count(*) FROM terms"), 1);
    }
}

#[test]
fn import_reports_only_track_their_own_committed_effect() {
    let source = Fixture::new("effect-source");
    source.source();
    let path = source.export();
    let target = Fixture::new("effect-target");
    target.target();
    let c = crate::open(&target.db).unwrap();
    c.execute_batch("CREATE TRIGGER synthetic_pending BEFORE UPDATE ON backend_effects BEGIN SELECT RAISE(ABORT,'synthetic delayed effect'); END;").unwrap();
    let (_, first) =
        import_run(&target.db, &target.id, &path, &|| Ok(()), &crate::app_imports::Silent, "effect-first", None)
            .unwrap();
    assert_eq!(first["committed"], true);
    assert_eq!(first["pending_effects"], true);
    let (repeat, second) =
        import_run(&target.db, &target.id, &path, &|| Ok(()), &crate::app_imports::Silent, "effect-no-op", None)
            .unwrap();
    assert!(repeat.action_id.is_none());
    assert_eq!(second["pending_effects"], false);
    c.execute_batch("DROP TRIGGER synthetic_pending; UPDATE backend_effects SET status='done'; INSERT INTO backend_effects(kind) VALUES ('verdict');").unwrap();
    for id in ["effect-first", "effect-no-op"] {
        let page: serde_json::Value =
            serde_json::from_str(&reports::page(&target.db, id, 0, 100).unwrap().dump()).unwrap();
        assert_eq!(page["report"]["pending_effects"], false);
    }
    assert!(reports::pending(&c, Some(c.last_insert_rowid())));
}

// 启动会话后台快照，轮询状态直到 done 成立；返回那一刻的状态。
fn automatic_backups(fixture: &Fixture, done: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
    let library = crate::app_library::Library::open(fixture.db.parent().unwrap()).unwrap();
    let mut backups = Backups::default();
    backups.start(&library, 1, "synthetic").unwrap();
    let deadline = crate::test_wait::deadline();
    let value = loop {
        let value: serde_json::Value = serde_json::from_str(&backups.status().dump()).unwrap();
        if done(&value) {
            break value;
        }
        assert!(std::time::Instant::now() < deadline, "{value}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    backups.stop();
    value
}

#[test]
fn upgraded_library_without_cached_hashes_gets_a_complete_snapshot_right_after_filling() {
    // 从 v8 升级的图库没有 human_asset_hashes：第一份快照全缺摘要，补算完立刻再出一份，不等一小时（#516）。
    let fixture = Fixture::new("upgraded-without-hashes");
    fixture.source();
    assert_eq!(fixture.count("SELECT count(*) FROM human_asset_hashes"), 0);
    let status = automatic_backups(&fixture, |s| {
        s["snapshots"].as_array().unwrap().len() >= 2 && !s["running"].as_bool().unwrap()
    });
    let snapshots = status["snapshots"].as_array().unwrap();
    assert_eq!(snapshots.len(), 2, "{status}");
    assert_eq!(snapshots[0]["hash_coverage"]["missing"], 0, "{status}");
    assert_eq!(snapshots[0]["hash_coverage"]["verified"], 4, "{status}");
    assert_eq!(snapshots[1]["hash_coverage"]["missing"], 4, "{status}");
    assert_eq!(status["hash_missing"], 0, "{status}");
    assert_eq!(status["refill"], false, "{status}");
    assert!(status["pending_revision"].is_null(), "补齐后的快照已覆盖补算带来的修订：{status}");
    assert!(status["next_attempt"].is_null(), "{status}");
    assert_eq!(fixture.count("SELECT count(*) FROM human_asset_hashes"), 4);
}

#[test]
fn a_really_unreadable_picture_keeps_the_hourly_interval_and_is_reported_as_unreadable() {
    let fixture = Fixture::new("upgraded-with-deleted-file");
    fixture.source();
    fs::remove_file(fixture.root.join("source-d.png")).unwrap();
    let status = automatic_backups(&fixture, |s| !s["hash_unreadable"].is_null() && !s["running"].as_bool().unwrap());
    let snapshots = status["snapshots"].as_array().unwrap();
    assert_eq!(snapshots.len(), 1, "仍有图读不到时不提前再出快照：{status}");
    assert_eq!(status["hash_missing"], 4, "{status}");
    assert_eq!(status["hash_unreadable"], 1, "{status}");
    assert_eq!(status["refill"], false, "{status}");
    // 读得到的三张已补算，作为待保护的修订按一小时间隔进下一份快照。
    assert!(!status["pending_revision"].is_null(), "{status}");
    let next = status["next_attempt"].as_i64().unwrap();
    assert!(next >= status["last_success"].as_i64().unwrap() + 3600, "{status}");
    assert_eq!(fixture.count("SELECT count(*) FROM human_asset_hashes"), 3);
}
