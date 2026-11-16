use super::*;
use crate::{schema::Kind, store, store::VerdictTarget};
use rusqlite::params;
use std::{fs, path::PathBuf};

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn example_learning_recomputes_after_clear_and_undo() {
    let root = Fixture(std::env::temp_dir().join(format!("illustash-concept-test-{}", store::new_key().unwrap())));
    let folder = root.0.join("input");
    let dest = root.0.join("test.pilib");
    fs::create_dir_all(&folder).unwrap();
    crate::app_create::create_folder(&dest, &folder, None, None, None).unwrap();
    let library = Library::open(&dest).unwrap();
    let mut db = crate::open(&library.db()).unwrap();
    let mut src = crate::sqlite::open(library.source()).unwrap();
    let app = db.transaction().unwrap();
    let input = src.transaction().unwrap();
    let key = "example-test";
    let term = store::create_term(&app, Kind::Concept, Some(key), "user").unwrap();
    let mut p = Vec::new();
    let mut unjudged: Vec<_> = (0..2010).map(|n| format!("U{n:04}")).collect();
    unjudged.sort_by_key(|id| {
        let mut hash = Sha256::new();
        learning::hash_field(&mut hash, key.as_bytes());
        learning::hash_field(&mut hash, id.as_bytes());
        hash.finalize()
    });
    let candidate = unjudged.last().unwrap().clone();
    for (id, positive) in (0..5).map(|n| (format!("E{n}"), true)).chain(unjudged.into_iter().map(|id| {
        let is_candidate = id == candidate;
        (id, is_candidate)
    })) {
        app.execute("INSERT INTO assets(uuid,filename,added_at) VALUES (?1,?1,'2026-09-01')", [&id]).unwrap();
        input.execute("INSERT INTO assets(uuid,in_scope,trashed,src_path) VALUES (?,1,0,'synthetic')", [&id]).unwrap();
        let label = if positive { "red" } else { "blue" };
        input
            .execute(
                "INSERT INTO labels(uuid,dim,value,source,conf) VALUES (?,'general',?,'wd_v3',0.9)",
                params![id, label],
            )
            .unwrap();
        let mut feature = vec![0.0f32; 768];
        feature[usize::from(!positive)] = 1.0;
        input
            .execute(
                "INSERT INTO embeddings VALUES (?,'ccip',?)",
                params![id, crate::research_compute::features::bytes(&feature)],
            )
            .unwrap();
        let asset = store::register_asset(&app, "library", &id, None).unwrap();
        if id.starts_with('E') {
            p.push(VerdictTarget::Image { asset, term });
        }
    }
    app.commit().unwrap();
    input.commit().unwrap();
    drop(src);
    let verdict = |db: &mut Connection, targets, value| {
        store::judge(db, targets, value, "user", "", "test example verdict").unwrap();
    };
    verdict(&mut db, &p, store::Verdict::Yes);
    let mut learner = Learner::default();
    learner.start(&library, 1, "session", Arc::new(Cache::default())).unwrap();
    let maybe = |db: &Connection| {
        db.query_row(
            "SELECT count(*) FROM term_projection p JOIN term_assets a ON a.id=p.asset_id
             WHERE p.term_id=? AND p.state='maybe' AND a.key=?",
            params![term, candidate],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    let wait = |db: &Connection, expected| {
        let deadline = crate::test_wait::deadline();
        while maybe(db) != expected {
            assert!(
                Instant::now() < deadline,
                "没有等到预期候选，任务状态 {:?}",
                learner.0.as_ref().unwrap().outcome()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    wait(&db, 1);
    verdict(&mut db, &p[..1], store::Verdict::Clear);
    assert_eq!(Snapshot::read(&db).unwrap().labels[key].verdicts["E0"], Verdict::Clear);
    wait(&db, 0);
    store::undo_last(&mut db).unwrap();
    wait(&db, 1);
    let context = learner.0.as_ref().unwrap().context();
    learner.stop();
    assert!(context.stop_reason().is_some());
    assert!(context.receive_part::<()>(context.identity(), || panic!("取消后进入了发布事务")).is_err());
}
