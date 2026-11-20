use super::*;
use crate::package::tests::build;
use crate::package::{Package, FORMAT};
use crate::store::{import_package_with, state, AssetId, LABELS_IN_CORE};
use serde_json::{json, Value};

/// tests/reference/web/store.py（#9 已删除） 里本模块读的表（原样，含索引）和三张图。
const LIBRARY_SQL: &str = r#"
CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE actions(id INTEGER PRIMARY KEY AUTOINCREMENT,label TEXT NOT NULL,before_json TEXT NOT NULL,ts TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,undone INTEGER NOT NULL DEFAULT 0);
CREATE TABLE assets(uuid TEXT PRIMARY KEY,filename TEXT,src_path TEXT,width INTEGER,height INTEGER,added_at TEXT,src_kind TEXT);
CREATE TABLE general_hits(uuid TEXT NOT NULL,value TEXT NOT NULL);
CREATE INDEX idx_general_value ON general_hits(value,uuid);
CREATE TABLE asset_tags(uuid TEXT NOT NULL,tag TEXT NOT NULL,origin TEXT NOT NULL,created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,PRIMARY KEY(uuid,tag));
CREATE INDEX asset_tags_tag ON asset_tags(tag,uuid);
CREATE TABLE asset_rating(uuid TEXT PRIMARY KEY,rating TEXT NOT NULL);
INSERT INTO assets(uuid) VALUES ('a'),('b'),('c');
"#;

fn library() -> Connection {
    let mut c = Connection::open_in_memory().unwrap();
    crate::configure(&c).unwrap();
    c.execute_batch(LIBRARY_SQL).unwrap();
    crate::migrate(&mut c).unwrap();
    // 三张表换成核心之上的视图，标签在核心里（新建图库由 `new_library` 这样做）。
    c.execute_batch("DROP TABLE asset_tags; DROP TABLE general_hits; DROP TABLE asset_rating;").unwrap();
    c.execute_batch(VIEWS).unwrap();
    c.execute("INSERT INTO meta VALUES (?1,'core')", [LABELS_IN_CORE]).unwrap();
    c
}

fn wd(asset: &str, dim: &str, value: &str, p: f64) -> Value {
    let (kind, system) = if dim == "rating" { ("rating", "wd_rating") } else { ("topic", "wd") };
    json!({"id": format!("{asset}:{dim}:{value}"), "asset": {"namespace": "library", "id": asset},
           "term": {"kind": kind, "link": {"system": system, "key": value}},
           "source": "wd", "method": "wd_v3", "version": "swinv2_448", "score": {"value": p, "metric": "wd_probability"},
           "observed_at": "2026-09-28T00:00:00Z", "data": {}})
}

fn deliver(c: &mut Connection, assets: &[&str], rows: &[Value], new_assets: bool) {
    let manifest = json!({"format": FORMAT, "library_id": "lib", "producer": {"name": "wd", "version": "labels"},
        "created_at": "2026-09-28T00:00:00Z", "files": {},
        "assets": assets.iter().map(|a| json!({"asset": {"namespace": "library", "id": a}, "status": "completed", "error": null})).collect::<Vec<_>>()});
    let (m, e) = build(manifest, rows);
    import_package_with(c, &Package::from_parts(&m, &e).unwrap(), "lib", new_assets).unwrap();
}

/// WD 证据：题材 ≥ 0.35 的进 general_hits，每张图最高的分级进 asset_rating。
fn deliver_all(c: &mut Connection) {
    let rows = [
        wd("a", "general", "solo", 0.9),
        wd("a", "general", "beach", 0.25),
        wd("b", "general", "1girl", 0.8),
        wd("a", "rating", "general", 0.7),
        wd("a", "rating", "sensitive", 0.2),
        wd("b", "rating", "explicit", 0.6),
        wd("b", "rating", "questionable", 0.3),
        wd("c", "rating", "sensitive", 0.9),
    ];
    deliver(c, &["a", "b", "c"], &rows, false);
}

fn rows(c: &Connection, sql: &str) -> Vec<(String, String)> {
    c.prepare(sql).unwrap().query_map([], |x| Ok((x.get(0)?, x.get(1)?))).unwrap().map(|x| x.unwrap()).collect()
}

#[test]
fn the_views_read_the_core_and_follow_the_library() {
    let mut c = library();
    deliver_all(&mut c);
    assert_eq!(
        rows(&c, "SELECT uuid,value FROM general_hits ORDER BY 1,2"),
        [("a".into(), "solo".into()), ("b".into(), "1girl".into())]
    );
    assert_eq!(
        rows(&c, "SELECT uuid,rating FROM asset_rating ORDER BY 1"),
        [("a".into(), "general".into()), ("b".into(), "explicit".into()), ("c".into(), "sensitive".into())]
    );
    // 「加上」写核心：概念按名字找，没有就建；视图里出现。
    let a: AssetId = c.query_row("SELECT id FROM term_assets WHERE key='c'", [], |x| x.get(0)).unwrap();
    let summer = concept_by_name(&c, "summer", Some("user")).unwrap().unwrap();
    assert_eq!(concept_by_name(&c, "summer", None).unwrap(), Some(summer));
    crate::store::judge(
        &mut c,
        &[crate::store::VerdictTarget::Image { asset: a, term: summer }],
        crate::store::Verdict::Yes,
        "user",
        "",
        "加上",
    )
    .unwrap();
    assert_eq!(state(&c, a, summer).unwrap().as_deref(), Some("confirmed"));
    assert_eq!(rows(&c, "SELECT uuid,tag FROM asset_tags"), [("c".into(), "summer".into())]);
    assert_eq!(page_asset_tags(&c, r#"["b","unknown"]"#).unwrap(), []);
    assert_eq!(page_asset_tags(&c, r#"["b","c","c"]"#).unwrap(), [("c".into(), "summer".into())]);
    crate::store::judge(
        &mut c,
        &[crate::store::VerdictTarget::Image { asset: a, term: summer }],
        crate::store::Verdict::Clear,
        "user",
        "",
        "去掉",
    )
    .unwrap();
    assert_eq!(page_asset_tags(&c, r#"["c"]"#).unwrap(), []);
    crate::store::undo(&mut c, None).unwrap();
    assert_eq!(page_asset_tags(&c, r#"["c"]"#).unwrap(), [("c".into(), "summer".into())]);
    assert_eq!(concept_by_name(&c, "nope", None).unwrap(), None);
    // 应用升级 schema 时会重跑建表语句：CREATE TABLE IF NOT EXISTS 碰到同名视图不报错；给视图建索引则报错。
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS asset_tags(uuid TEXT,tag TEXT);
         CREATE TABLE IF NOT EXISTS general_hits(uuid TEXT,value TEXT); CREATE TABLE IF NOT EXISTS asset_rating(uuid TEXT,rating TEXT);",
    )
    .unwrap();
    assert!(c.execute("CREATE INDEX IF NOT EXISTS asset_tags_tag ON asset_tags(tag,uuid)", []).is_err());
    // 图从图库移除：视图里跟着消失。
    c.execute("DELETE FROM assets WHERE uuid='b'", []).unwrap();
    assert_eq!(rows(&c, "SELECT uuid,value FROM general_hits"), [("a".into(), "solo".into())]);
    c.execute("DELETE FROM assets WHERE uuid='c'", []).unwrap();
    assert_eq!(page_asset_tags(&c, r#"["c"]"#).unwrap(), []);
}

#[test]
fn new_pictures_can_be_delivered_before_they_are_listed() {
    let mut c = library();
    deliver_all(&mut c);
    // 同步导入新图：WD 证据先到（图还不在 assets 里），视图里看不到；图写进 assets 后就出现。
    deliver(&mut c, &["d"], &[wd("d", "rating", "general", 0.8), wd("d", "general", "solo", 0.5)], true);
    assert_eq!(rows(&c, "SELECT uuid,rating FROM asset_rating WHERE uuid='d'"), []);
    c.execute("INSERT INTO assets(uuid) VALUES ('d')", []).unwrap();
    assert_eq!(rows(&c, "SELECT uuid,value FROM general_hits WHERE uuid='d'"), [("d".into(), "solo".into())]);
}
