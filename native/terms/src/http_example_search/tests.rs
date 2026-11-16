use super::*;
use crate::http_json::{parse_request, Json};
use std::{fs, path::PathBuf};

struct Fixture {
    root: PathBuf,
    context: LibraryContext,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("illustash-example-search-{}", crate::store::new_key().unwrap()));
        let folder = root.join("input");
        let library = root.join("synthetic.pilib");
        fs::create_dir_all(&folder).unwrap();
        crate::app_create::create_folder(&library, &folder, None, None, None).unwrap();
        let context = LibraryContext {
            db: library.join("library.sqlite"),
            source: Some(library.join("source.sqlite")),
            library: Some(library),
            generation: "synthetic".into(),
            tags: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tags/tags_v3.csv"),
            token: None,
            read_only: None,
            examples: Some(Default::default()),
        };
        Self { root, context }
    }

    fn db(&self) -> Connection {
        crate::sqlite::open(&self.context.db).unwrap()
    }

    fn source(&self) -> Connection {
        let c = crate::sqlite::open(self.context.source.as_ref().unwrap()).unwrap();
        c.execute_batch(
            "CREATE TABLE IF NOT EXISTS embeddings(uuid TEXT,model TEXT,vec BLOB,PRIMARY KEY(uuid,model));",
        )
        .unwrap();
        c
    }

    fn asset(db: &Connection, source: &Connection, id: &str, name: &str, date: &str, tags: &[(&str, f64)]) {
        db.execute(
            "INSERT INTO assets(uuid,filename,width,height,added_at) VALUES (?,?,1,1,?)",
            params![id, name, date],
        )
        .unwrap();
        source.execute("INSERT INTO assets(uuid,in_scope,trashed,src_path) VALUES (?,1,0,'synthetic')", [id]).unwrap();
        for &(label, conf) in tags {
            source
                .execute(
                    "INSERT INTO labels(uuid,dim,value,source,conf) VALUES (?,'general',?,'wd_v3',?)",
                    params![id, label, conf],
                )
                .unwrap();
        }
    }

    fn read(&self, path: &str, ids: &[&str], params: &str) -> (Json, u16) {
        let like = serde_json::to_string(ids).unwrap();
        let like: String = url::form_urlencoded::byte_serialize(like.as_bytes()).collect();
        crate::http_read::read(&self.context, &format!("{path}?like={like}&{params}")).unwrap().unwrap()
    }

    fn ids(&self, examples: &[&str], params: &str) -> Vec<String> {
        let (body, status) = self.read("/api/ids", examples, params);
        assert_eq!(status, 200, "{}", body.dump());
        serde_json::from_str(&body.dump()).unwrap()
    }

    fn base(&self, rows: &[(&str, usize)]) {
        let base = self.context.library.as_ref().unwrap().join("features");
        fs::create_dir_all(&base).unwrap();
        let mut manifest = String::from("uuid\n");
        let header = format!("{{'descr': '<f4', 'fortran_order': False, 'shape': ({}, 768), }}\n", rows.len());
        let mut file = b"\x93NUMPY\x01\x00".to_vec();
        file.extend((header.len() as u16).to_le_bytes());
        file.extend(header.bytes());
        for &(id, column) in rows {
            manifest.push_str(id);
            manifest.push('\n');
            file.extend(ccip_vector(column));
        }
        fs::write(base.join("full-manifest.csv"), manifest).unwrap();
        fs::write(base.join("full-features.npy"), file).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.context.examples.take();
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn ccip_vector(column: usize) -> Vec<u8> {
    let mut row = vec![0.0; 768];
    row[column] = 1.0;
    crate::research_compute::features::bytes(&row)
}

#[test]
fn example_search_ranks_filtered_candidates_and_reuses_the_selection_for_writes() {
    let f = Fixture::new();
    let mut db = f.db();
    let mut source = f.source();
    let app = db.transaction().unwrap();
    let input = source.transaction().unwrap();
    let red = &[("red", 0.9)];
    let lower = &[("red", 0.6), ("blue", 0.8)];
    for (id, name, date, labels) in [
        ("EX", "example", "2026-01-01", red.as_slice()),
        ("EX_COPY", "eligible", "2026-01-01", red.as_slice()),
        ("FOLDED", "eligible", "2026-01-01", red.as_slice()),
        ("EXCLUDED", "eligible", "2026-01-01", red.as_slice()),
        ("ADULT", "eligible", "2026-01-01", red.as_slice()),
        ("MISSING", "eligible", "2026-01-01", &[]),
    ] {
        Fixture::asset(&app, &input, id, name, date, labels);
    }
    for index in 0..520 {
        Fixture::asset(&app, &input, &format!("RED{index:03}"), "other", "2026-01-01", red);
        let date = if index == 7 { "2026-03-01" } else { "2026-02-01" };
        Fixture::asset(&app, &input, &format!("ELIGIBLE{index:03}"), "eligible", date, lower);
    }
    app.execute_batch(
        "INSERT INTO duplicates VALUES ('EX','exact',2,0,'2026-01-01',1,0),
                                        ('EX_COPY','exact',2,1,'2026-01-01',0,0),
                                        ('FOLDED','another',2,1,'2026-01-01',0,1);
         INSERT INTO excluded VALUES ('EXCLUDED','manual','manual');
         INSERT INTO terms(key,kind,origin) VALUES ('test-explicit','rating','synthetic');
         INSERT INTO term_links(system,key,term_id,source)
             SELECT 'wd_rating','explicit',id,'synthetic' FROM terms WHERE key='test-explicit';
         INSERT INTO term_assets(namespace,key) VALUES ('library','ADULT');
         INSERT INTO term_projection(asset_id,term_id,state)
             SELECT a.id,t.id,'machine' FROM term_assets a,terms t WHERE a.key='ADULT' AND t.key='test-explicit';",
    )
    .unwrap();
    app.commit().unwrap();
    input.commit().unwrap();
    drop(db);
    drop(source);
    let examples = ["MISSING", "EX", "EX"];
    let filter = "q=eligible&nsfw=hide";
    let expected: Vec<_> = (0..500).map(|i| format!("ELIGIBLE{i:03}")).collect();
    assert_eq!(f.ids(&examples, &format!("{filter}&limit=500")), expected);
    let (count, status) = f.read("/api/assets/count", &examples, filter);
    assert_eq!((count.dump(), status), (r#"{"total": 500}"#.into(), 200));
    let (page, status) = f.read("/api/assets", &examples, &format!("{filter}&offset=4&limit=2"));
    let page: serde_json::Value = serde_json::from_str(&page.dump()).unwrap();
    assert_eq!(status, 200);
    assert_eq!(page["total"], 500);
    assert_eq!(page["items"][0]["uuid"], "ELIGIBLE004");
    assert_eq!(page["items"][1]["uuid"], "ELIGIBLE005");
    assert!(page["items"][0].get("score").is_none());
    assert_eq!(f.ids(&examples, &format!("{filter}&order=newest&limit=1")), ["ELIGIBLE007"]);
    assert_eq!(f.ids(&examples, &format!("{filter}&order=oldest&limit=1")), ["ELIGIBLE000"]);
    assert_eq!(f.ids(&examples, &format!("{filter}&offset=500")), [] as [String; 0]);

    let write = parse_request(
        r#"{"tag":"example-test","filter":{"like":"[\"MISSING\",\"EX\"]","q":"eligible","nsfw":"hide"}}"#,
    )
    .unwrap();
    crate::app_commands::request(&f.context, "/api/tag", &write).unwrap();
    let actual: Vec<String> = f
        .db()
        .prepare("SELECT uuid FROM asset_tags WHERE tag='example-test' ORDER BY uuid")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(f.read("/api/assets", &["MISSING"], "").1, 400);
    for query in ["like=broken", "like=%5B%5D", "like_by=ccip", "order=similar", "like=%5B%22EX%22%5D&like_by=wd1024"] {
        let response = crate::http_read::read(&f.context, &format!("/api/ids?{query}")).unwrap().unwrap();
        assert_eq!(response.1, 400, "{query}");
    }
}

#[test]
fn example_search_observes_wd_and_ccip_updates_and_base_file_changes() {
    let f = Fixture::new();
    let db = f.db();
    let source = f.source();
    for (id, label) in [("A", "red"), ("B", "red"), ("C", "blue"), ("D", "blue")] {
        Fixture::asset(&db, &source, id, id, "2026-01-01", &[(label, 0.9)]);
    }
    assert_eq!(f.ids(&["A"], ""), ["B", "C", "D"]);
    // 原地改值，行数和最大 rowid 都保持不变。
    source.execute("UPDATE labels SET value='blue' WHERE uuid='A'", []).unwrap();
    assert_eq!(f.ids(&["A"], ""), ["C", "D", "B"]);
    source.execute("DELETE FROM labels WHERE uuid='A'", []).unwrap();
    assert_eq!(f.read("/api/assets", &["A"], "").1, 400);

    f.base(&[("A", 0), ("B", 0), ("C", 1)]);
    assert_eq!(f.ids(&["A"], "like_by=ccip"), ["B", "C"]);
    source.execute("INSERT INTO embeddings VALUES ('A','ccip',?)", [ccip_vector(1)]).unwrap();
    assert_eq!(f.ids(&["A"], "like_by=ccip"), ["C", "B"]);
    source.execute("UPDATE embeddings SET vec=? WHERE uuid='A'", [ccip_vector(0)]).unwrap();
    assert_eq!(f.ids(&["A"], "like_by=ccip"), ["B", "C"]);
    source.execute("DELETE FROM embeddings WHERE uuid='A'", []).unwrap();
    assert_eq!(f.ids(&["A"], "like_by=ccip"), ["B", "C"]);
    f.base(&[("A", 1), ("B", 0), ("C", 1), ("D", 1)]);
    assert_eq!(f.ids(&["A"], "like_by=ccip"), ["C", "D", "B"]);
    source.execute("ALTER TABLE labels RENAME COLUMN source TO unknown_producer", []).unwrap();
    assert_eq!(f.read("/api/assets", &["B"], "").1, 400);
    assert_eq!(f.ids(&["A"], "like_by=ccip"), ["C", "D", "B"]);
}

#[test]
fn example_search_rebuilds_stale_or_damaged_wd_disk_cache() {
    let f = Fixture::new();
    let db = f.db();
    let source = f.source();
    source.pragma_update(None, "journal_mode", "WAL").unwrap();
    for (id, label) in [("A", "red"), ("B", "red"), ("C", "blue")] {
        Fixture::asset(&db, &source, id, id, "2026-01-01", &[(label, 0.9)]);
    }
    let cache = f.context.examples.as_ref().unwrap();
    let base = f.context.library.as_ref().unwrap().join("features");
    let file = crate::example_disk::path(&base).unwrap();
    assert_eq!(f.ids(&["A"], ""), ["B", "C"]);
    let initial = fs::read(&file).unwrap();
    cache.clear();
    assert_eq!(f.ids(&["A"], ""), ["B", "C"]);

    cache.clear();
    source.execute("UPDATE labels SET value='blue' WHERE uuid='A'", []).unwrap();
    // 保持相同的行数和 rowid，通过另一个只读连接识别 WAL 的已提交更新。
    assert_eq!(f.ids(&["A"], ""), ["C", "B"]);
    let updated = fs::read(&file).unwrap();
    assert_ne!(initial, updated);

    cache.clear();
    let mut damaged = updated.clone();
    *damaged.last_mut().unwrap() ^= 0x01;
    fs::write(&file, damaged).unwrap();
    assert_eq!(f.ids(&["A"], ""), ["C", "B"]);
    assert_eq!(fs::read(&file).unwrap(), updated);
}

#[test]
fn example_search_shared_scorers_support_centroid_max_and_balanced_linear_logits() {
    let f = Fixture::new();
    let db = f.db();
    let source = f.source();
    for (id, labels) in [
        ("A", vec![("red", 0.8)]),
        ("B", vec![("blue", 0.8)]),
        ("C", vec![("red", 0.8)]),
        ("D", vec![("red", 0.8), ("blue", 0.8)]),
        ("E", vec![("red", 0.8)]),
        ("NO_CCIP", vec![("red", 0.8)]),
    ] {
        Fixture::asset(&db, &source, id, id, "2026-01-01", &labels);
    }
    f.base(&[("E", 2), ("D", 2), ("C", 0), ("B", 1), ("A", 0)]);
    let base = f.context.library.as_ref().unwrap().join("features");
    let tags = example_features::load(&source, &base, Space::Tags).unwrap();
    let rows = |ids: &[&str]| ids.iter().map(|id| tags.row(id).unwrap()).collect::<Vec<_>>();
    let positives = rows(&["A", "B"]);
    let candidates = rows(&["C", "D"]);
    let expected = |scorer| {
        example_scores::top(&tags, &Examples { positives: &positives, ..Examples::default() }, &scorer, &candidates, 1)
            .iter()
            .map(|&(row, _)| tags.uuid(row))
            .collect::<Vec<_>>()
    };
    // 质心偏向兼有两个标签的 D；最近正例偏向和 A 一样的 C。
    assert_eq!(expected(Scorer::Centroid), ["D"]);
    assert_eq!(expected(Scorer::MaxSim), ["C"]);

    let fused = example_features::load(&source, &base, Space::TagsCcip).unwrap();
    assert_eq!(fused.len(), 5);
    assert!(fused.row("NO_CCIP").is_none());
    let a = fused.row("A").unwrap();
    let b = fused.row("B").unwrap();
    let d = fused.row("D").unwrap();
    let train = Examples { positives: &[a], negatives: &[b], background: &[d], exclude: &[a, b, d] };
    let result = example_scores::score(&fused, &train, &Scorer::Linear { l2: 1.0 / 0.3 });
    assert_eq!(fused.uuid(result[0].0), "C");
    assert!(result[0].1 > 0.0 && result[0].1 > result[1].1);
}

#[test]
fn sample_rank_runs_after_composed_source_exclusion_and_date_filters() {
    let f = Fixture::new();
    let db = f.db();
    let source = f.source();
    for (id, date, preset) in [
        ("EX", "2026-01-01", "other"),
        ("A", "2026-01-02", "pixiv"),
        ("B", "2026-01-03", "pixiv"),
        ("C", "2026-01-04", "x"),
        ("D", "2025-01-04", "pixiv"),
    ] {
        Fixture::asset(&db, &source, id, id, date, &[("red", 0.9)]);
        source.execute("UPDATE assets SET preset=? WHERE uuid=?", [preset, id]).unwrap();
    }
    let write = parse_request(r#"{"tag":"reviewed","ids":["A"]}"#).unwrap();
    crate::app_commands::request(&f.context, "/api/tag", &write).unwrap();
    let params = "has=%5B%22source%3Apixiv%22%5D&not=%5B%22utag%3Areviewed%22%5D&from=2026-01-01&to=2026-12-31";
    assert_eq!(f.ids(&["EX"], params), ["B"]);
    let (count, status) = f.read("/api/assets/count", &["EX"], params);
    assert_eq!((count.dump(), status), (r#"{"total": 1}"#.into(), 200));
    let (page, status) = f.read("/api/assets", &["EX"], params);
    let page: serde_json::Value = serde_json::from_str(&page.dump()).unwrap();
    assert_eq!(status, 200);
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"][0]["uuid"], "B");
    let write = parse_request(
        r#"{"tag":"similar-result","filter":{"like":"["EX"]","has":"["source:pixiv"]","not":"["utag:reviewed"]","from":"2026-01-01","to":"2026-12-31"}}"#,
    )
    .unwrap();
    crate::app_commands::request(&f.context, "/api/tag", &write).unwrap();
    let actual = db
        .prepare("SELECT uuid FROM asset_tags WHERE tag='similar-result' ORDER BY uuid")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(actual, ["B"]);
}
