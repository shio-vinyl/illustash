use super::*;
use crate::package::tests::build;
use crate::package::{Package, FORMAT};
use serde_json::json;

fn db() -> Connection {
    let mut c = Connection::open_in_memory().unwrap();
    crate::configure(&c).unwrap();
    crate::migrate(&mut c).unwrap();
    for key in ["A-1", "A-2", "A-3"] {
        register_asset(&c, "photos", key, None).unwrap();
    }
    c
}

fn term_key(c: &Connection, term: TermId) -> Result<String> {
    Ok(c.query_row("SELECT key FROM terms WHERE id=?1", [term], |r| r.get(0))?)
}

fn manifest(assets: &[&str]) -> Value {
    json!({
        "format": FORMAT, "library_id": "lib-1",
        "producer": {"name": "provenance", "version": "0.4"},
        "created_at": "2026-09-28T10:00:00Z",
        "assets": assets.iter().map(|a| json!({"asset": {"namespace": "photos", "id": a}, "status": "completed", "error": null})).collect::<Vec<_>>(),
        "files": {}
    })
}

fn artist(asset: &str, method: &str, version: &str, score: f64) -> Value {
    json!({
        "id": format!("{asset}-{method}-{version}"),
        "asset": {"namespace": "photos", "id": asset},
        "term": {"kind": "artist", "link": {"system": "danbooru", "key": "some_artist", "url": "https://danbooru.donmai.us/artists?name=some_artist"}, "name": "Some Artist"},
        "source": "provenance", "method": method, "version": version,
        "score": {"value": score, "metric": "hamming_distance"},
        "observed_at": "2026-09-28T02:00:00Z", "data": {"post_id": "123"}
    })
}

fn wd(asset: &str, kind: &str, tag: &str, p: f64) -> Value {
    json!({
        "id": format!("{asset}-{tag}"),
        "asset": {"namespace": "photos", "id": asset},
        "term": {"kind": kind, "link": {"system": "danbooru", "key": tag}},
        "source": "wd", "method": "tagger", "version": "swinv2_448",
        "score": {"value": p, "metric": "wd_probability"},
        "observed_at": "2026-09-28T02:00:00Z", "data": {}
    })
}

fn package(assets: &[&str], rows: &[Value]) -> Package {
    let (m, e) = build(manifest(assets), rows);
    Package::from_parts(&m, &e).unwrap()
}

fn import(c: &mut Connection, assets: &[&str], rows: &[Value]) -> Result<Imported> {
    import_package(c, &package(assets, rows), "lib-1")
}

fn asset(c: &Connection, key: &str) -> AssetId {
    c.query_row("SELECT id FROM term_assets WHERE key=?1", [key], |r| r.get(0)).unwrap()
}

fn linked(c: &Connection, key: &str) -> TermId {
    term_by_link(c, "danbooru", key).unwrap().unwrap()
}

fn st(c: &Connection, a: &str, term: &str) -> Option<String> {
    state(c, asset(c, a), linked(c, term)).unwrap()
}

fn count(c: &Connection, table: &str) -> i64 {
    c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap()
}

#[test]
fn artist_takes_only_strong_evidence_and_packages_import_once() {
    let mut c = db();
    let rows = [artist("A-1", "pixel_hash", "1", 0.0), artist("A-2", "ocr", "1", 0.9)];
    let first = import(&mut c, &["A-1", "A-2"], &rows).unwrap();
    assert_eq!((first.added, first.created_terms, first.already), (2, 1, false));
    assert_eq!(st(&c, "A-1", "some_artist").as_deref(), Some("machine"));
    assert_eq!(st(&c, "A-2", "some_artist").as_deref(), Some("maybe"));
    let name: String = c.query_row("SELECT value FROM term_names", [], |r| r.get(0)).unwrap();
    assert_eq!(name, "Some Artist");
    // 同一个包：什么都不写。
    let again = import(&mut c, &["A-1", "A-2"], &rows).unwrap();
    assert!(again.already && again.package == first.package);
    // 另一个包里的相同证据：去重，不重复入库。
    let mut other = manifest(&["A-1"]);
    other["producer"]["version"] = json!("0.5");
    let (m, e) = build(other, &rows[..1]);
    let third = import_package(&mut c, &Package::from_parts(&m, &e).unwrap(), "lib-1").unwrap();
    assert_eq!((third.added, third.duplicate), (0, 1));
    assert_eq!(count(&c, "term_evidence"), 2);
    assert_eq!(count(&c, "term_packages"), 2);
}

#[test]
fn a_conflicting_row_rolls_back_the_whole_package() {
    let mut c = db();
    import(&mut c, &["A-1"], &[artist("A-1", "pixel_hash", "1", 0.0)]).unwrap();
    let before = (count(&c, "terms"), count(&c, "term_evidence"), count(&c, "term_packages"), count(&c, "term_links"));
    let rows = [wd("A-2", "topic", "swimsuit", 0.9), artist("A-1", "pixel_hash", "1", 3.0)];
    let err = import(&mut c, &["A-1", "A-2"], &rows).unwrap_err();
    assert!(matches!(err, Error::Conflict(ref m) if m.contains("第 2 行") && m.contains("升版本号")), "{err}");
    let after = (count(&c, "terms"), count(&c, "term_evidence"), count(&c, "term_packages"), count(&c, "term_links"));
    assert_eq!(before, after);
    assert_eq!(count(&c, "term_projection"), 1);
    // 升版本号就是新证据，与旧的并存。
    let ok = import(&mut c, &["A-1"], &[artist("A-1", "pixel_hash", "2", 3.0)]).unwrap();
    assert_eq!(ok.added, 1);
}

#[test]
fn packages_are_bound_to_the_library_and_its_live_assets() {
    let mut c = db();
    let p = package(&["A-1"], &[artist("A-1", "pixel_hash", "1", 0.0)]);
    assert!(matches!(import_package(&mut c, &p, "lib-2"), Err(Error::Invalid(_))));
    let unknown = import(&mut c, &["A-9"], &[]).unwrap_err();
    assert!(unknown.to_string().contains("A-9"));
    let a3 = asset(&c, "A-3");
    remove_asset(&c, a3).unwrap();
    assert!(import(&mut c, &["A-3"], &[]).is_err());
    assert_eq!(count(&c, "term_packages"), 0);
}

#[test]
fn packages_cannot_create_user_concepts_or_reference_missing_keys() {
    let mut c = db();
    let mut concept = wd("A-1", "concept", "summer", 0.9);
    assert!(import(&mut c, &["A-1"], &[concept.clone()]).unwrap_err().to_string().contains("概念"));
    let id = create_term(&c, Kind::Concept, Some("summer-key"), "user").unwrap();
    concept["term"] = json!({"kind": "concept", "key": "summer-key"});
    assert_eq!(import(&mut c, &["A-1"], &[concept]).unwrap().added, 1);
    assert_eq!(state(&c, asset(&c, "A-1"), id).unwrap().as_deref(), Some("maybe"));
    let mut missing = wd("A-1", "concept", "x", 0.9);
    missing["term"] = json!({"kind": "concept", "key": "nope"});
    assert!(import(&mut c, &["A-1"], &[missing]).unwrap_err().to_string().contains("不存在"));
}

#[test]
fn a_rejection_is_permanent_against_machines_and_undoable_by_people() {
    let mut c = db();
    import(&mut c, &["A-1"], &[artist("A-1", "pixel_hash", "1", 0.0)]).unwrap();
    let (a1, t) = (asset(&c, "A-1"), linked(&c, "some_artist"));
    let action = judge(
        &mut c,
        &[VerdictTarget::Image { asset: a1, term: t }],
        Verdict::No,
        "user",
        "",
        "已标记 1 张不是 Some Artist 画的",
    )
    .unwrap();
    assert_eq!(st(&c, "A-1", "some_artist").as_deref(), Some("rejected"));
    let reviewed: String = c.query_row("SELECT reviewed FROM term_verdicts", [], |r| r.get(0)).unwrap();
    assert_eq!(reviewed, "[1]");
    // 机器重跑、新版本的证据都不能把否定翻回来。
    import(&mut c, &["A-1"], &[artist("A-1", "gallery_match", "2", 1.0)]).unwrap();
    assert_eq!(st(&c, "A-1", "some_artist").as_deref(), Some("rejected"));
    // 撤销：回到由证据决定。
    assert_eq!(undo_last(&mut c).unwrap(), Undo::Undone { action, label: "已标记 1 张不是 Some Artist 画的".into() });
    assert_eq!(st(&c, "A-1", "some_artist").as_deref(), Some("machine"));
    assert_eq!(count(&c, "term_verdicts"), 1, "撤销不删裁决，只把那一步标为 undone");
    // 确认后收回：同样回到证据。
    judge(&mut c, &[VerdictTarget::Image { asset: a1, term: t }], Verdict::Yes, "user", "", "确认").unwrap();
    assert_eq!(st(&c, "A-1", "some_artist").as_deref(), Some("confirmed"));
    judge(&mut c, &[VerdictTarget::Image { asset: a1, term: t }], Verdict::Clear, "user", "", "清除").unwrap();
    assert_eq!(st(&c, "A-1", "some_artist").as_deref(), Some("machine"));
    let effective: i64 = c.query_row("SELECT count(*) FROM term_effective_verdicts", [], |r| r.get(0)).unwrap();
    assert_eq!(effective, 0);
}

#[test]
fn verdicts_come_only_from_people() {
    let mut c = db();
    let t = create_term(&c, Kind::Artist, None, "user").unwrap();
    let a1 = asset(&c, "A-1");
    let target = [VerdictTarget::Image { asset: a1, term: t }];
    assert!(judge(&mut c, &target, Verdict::Yes, "provenance", "", "x").is_err());
    assert!(judge(&mut c, &[], Verdict::Yes, "user", "", "x").is_err());
    assert!(judge(&mut c, &[VerdictTarget::Image { asset: a1, term: t + 100 }], Verdict::Yes, "user", "", "x").is_err());
    assert_eq!(count(&c, "actions"), 0, "失败的裁决不留历史");
    // 没有证据时人也可以直接贴。
    judge(&mut c, &target, Verdict::Yes, "user", "", "贴上").unwrap();
    assert_eq!(state(&c, a1, t).unwrap().as_deref(), Some("confirmed"));
}

#[test]
fn undo_hands_foreign_steps_back_without_skipping_them() {
    let mut c = db();
    let t = create_term(&c, Kind::Artist, None, "user").unwrap();
    let a1 = asset(&c, "A-1");
    judge(&mut c, &[VerdictTarget::Image { asset: a1, term: t }], Verdict::Yes, "user", "", "确认").unwrap();
    // 宿主的标签操作排在后面：最近一步不是词条核心的。
    c.execute(
        "INSERT INTO actions(label,before_json) VALUES ('添加标签','[{\"uuid\":\"A-1\",\"tag\":\"x\",\"had\":null}]')",
        [],
    )
    .unwrap();
    let foreign = c.query_row("SELECT max(id) FROM actions", [], |r| r.get(0).map(ActionId::from_raw)).unwrap();
    assert_eq!(undo_last(&mut c).unwrap(), Undo::Foreign { action: foreign });
    assert_eq!(state(&c, a1, t).unwrap().as_deref(), Some("confirmed"));
    c.execute("UPDATE actions SET undone=1 WHERE id=?1", [foreign.raw()]).unwrap();
    assert!(matches!(undo_last(&mut c).unwrap(), Undo::Undone { .. }));
    assert_eq!(state(&c, a1, t).unwrap(), None);
    assert_eq!(undo_last(&mut c).unwrap(), Undo::Nothing);
}

#[test]
fn changing_kind_is_one_undoable_step() {
    let mut c = db();
    import(&mut c, &["A-1"], &[wd("A-1", "character", "doodle_sensei_(blue_archive)", 0.95)]).unwrap();
    let t = linked(&c, "doodle_sensei_(blue_archive)");
    let label = "设为只当标签";
    set_kind(&mut c, t, Kind::Label, label).unwrap();
    assert_eq!(term_by_link(&c, "danbooru", "doodle_sensei_(blue_archive)").unwrap(), Some(t));
    assert_eq!(term_by_key(&c, &term_key(&c, t).unwrap()).unwrap().unwrap().1, Kind::Label);
    // 改了类型的词条照样接收证据，不会按旧类型再建一个。
    import(&mut c, &["A-2"], &[wd("A-2", "character", "doodle_sensei_(blue_archive)", 0.9)]).unwrap();
    assert_eq!(count(&c, "terms"), 1);
    assert!(matches!(undo_last(&mut c).unwrap(), Undo::Undone { label: ref l, .. } if l == label));
    assert_eq!(term_by_key(&c, &term_key(&c, t).unwrap()).unwrap().unwrap().1, Kind::Character);
}

#[test]
fn topic_thresholds_and_the_keep_floor() {
    let mut c = db();
    let rows = [
        wd("A-1", "topic", "swimsuit", 0.5),
        wd("A-1", "topic", "beach", 0.25),
        wd("A-1", "topic", "watermelon", 0.05),
    ];
    let out = import(&mut c, &["A-1"], &rows).unwrap();
    assert_eq!((out.added, out.skipped), (2, 1));
    assert_eq!(st(&c, "A-1", "swimsuit").as_deref(), Some("machine"));
    assert_eq!(st(&c, "A-1", "beach").as_deref(), Some("maybe"));
    assert_eq!(term_by_link(&c, "danbooru", "watermelon").unwrap(), None, "没入库的证据不建词条");
}

#[test]
fn rating_is_exactly_one_and_a_person_overrides_the_machine() {
    let mut c = db();
    let rows = [
        wd("A-1", "rating", "general", 0.2),
        wd("A-1", "rating", "explicit", 0.7),
        wd("A-1", "rating", "questionable", 0.3),
    ];
    import(&mut c, &["A-1"], &rows).unwrap();
    assert_eq!(st(&c, "A-1", "explicit").as_deref(), Some("machine"));
    assert_eq!(st(&c, "A-1", "general").as_deref(), Some("maybe"));
    let a1 = asset(&c, "A-1");
    let q = linked(&c, "questionable");
    judge(&mut c, &[VerdictTarget::Image { asset: a1, term: q }], Verdict::Yes, "user", "", "改为 R15").unwrap();
    assert_eq!(st(&c, "A-1", "questionable").as_deref(), Some("confirmed"));
    assert_eq!(st(&c, "A-1", "explicit").as_deref(), Some("maybe"));
}

#[test]
fn relation_evidence_and_verdicts() {
    let mut c = db();
    let rel = json!({
        "id": "r1",
        "relation": {"kind": "contains",
                     "a": {"kind": "character", "link": {"system": "danbooru", "key": "mika_(blue_archive)"}},
                     "b": {"kind": "skin", "link": {"system": "danbooru", "key": "mika_(swimsuit)_(blue_archive)"}}},
        "source": "danbooru", "method": "implication", "version": "2026-09", "score": null,
        "observed_at": "2026-09-28T02:00:00Z", "data": {}
    });
    import(&mut c, &[], &[rel]).unwrap();
    let state =
        |c: &Connection| -> String { c.query_row("SELECT state FROM term_relation_states", [], |r| r.get(0)).unwrap() };
    assert_eq!(state(&c), "maybe");
    let id: RelationId = c.query_row("SELECT id FROM term_relations", [], |r| r.get(0)).unwrap();
    judge(&mut c, &[VerdictTarget::Relation(id)], Verdict::Yes, "user", "皮肤归到人物", "确认层级").unwrap();
    assert_eq!(state(&c), "confirmed");
    // 对称关系只存一条；same_as 两个方向算同一条。
    let (a, b) = (linked(&c, "mika_(blue_archive)"), linked(&c, "mika_(swimsuit)_(blue_archive)"));
    assert_eq!(
        relation(&c, a, b, RelationKind::Excludes).unwrap(),
        relation(&c, b, a, RelationKind::Excludes).unwrap()
    );
    assert_eq!(relation(&c, a, b, RelationKind::SameAs).unwrap(), relation(&c, b, a, RelationKind::SameAs).unwrap());
    assert!(relation(&c, a, a, RelationKind::Contains).is_err());
}

#[test]
fn groups_are_at_most_one_and_links_have_one_owner() {
    let mut c = db();
    let group = |key: &str, score: f64| {
        json!({"id": key, "asset": {"namespace": "photos", "id": "A-1"},
               "term": {"kind": "duplicate", "link": {"system": "illustash_dup", "key": key}},
               "source": "duplicates", "method": "pixel", "version": "1", "score": {"value": score, "metric": "similarity"},
               "observed_at": "2026-09-28T02:00:00Z", "data": {}})
    };
    import(&mut c, &["A-1"], &[group("g1", 0.9), group("g2", 0.99)]).unwrap();
    let g = |key: &str| state(&c, asset(&c, "A-1"), term_by_link(&c, "illustash_dup", key).unwrap().unwrap()).unwrap();
    // System 判定器不看量纲：同分时按 id；这里两者都是机器结果，只留一个。
    let states = [g("g1"), g("g2")];
    assert_eq!(states.iter().filter(|s| s.as_deref() == Some("machine")).count(), 1);
    let t = create_term(&c, Kind::Artist, None, "user").unwrap();
    let other = create_term(&c, Kind::Artist, None, "user").unwrap();
    add_link(&c, t, "pixiv", "123", "", "user").unwrap();
    add_link(&c, t, "pixiv", "123", "", "user").unwrap();
    assert!(matches!(add_link(&c, other, "pixiv", "123", "", "user"), Err(Error::Conflict(_))));
    assert!(matches!(create_term(&c, Kind::Artist, Some(&term_key(&c, t).unwrap()), "user"), Err(Error::Conflict(_))));
}

#[test]
fn removed_assets_leave_the_projection_and_rebuild_matches() {
    let mut c = db();
    import(&mut c, &["A-1", "A-2"], &[artist("A-1", "pixel_hash", "1", 0.0), wd("A-2", "topic", "solo", 0.9)]).unwrap();
    let snapshot = |c: &Connection| -> Vec<(i64, i64, String)> {
        c.prepare("SELECT asset_id,term_id,state FROM term_projection ORDER BY 1,2")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    let incremental = snapshot(&c);
    project_all(&mut c).unwrap();
    assert_eq!(snapshot(&c), incremental);
    remove_asset(&c, asset(&c, "A-1")).unwrap();
    assert_eq!(snapshot(&c).len(), 1);
    assert_eq!(count(&c, "term_evidence"), 2, "移除图不删证据");
    register_asset(&c, "photos", "A-1", None).unwrap();
    project_all(&mut c).unwrap();
    assert_eq!(snapshot(&c), incremental);
}

#[test]
fn new_keys_are_uuid_v4() {
    let k = new_key().unwrap();
    assert_eq!(k.len(), 36);
    assert_eq!(&k[14..15], "4");
    assert_ne!(k, new_key().unwrap());
}

#[test]
fn undo_checks_the_step_the_caller_saw() {
    let mut c = db();
    let t = create_term(&c, Kind::Artist, None, "user").unwrap();
    let a1 = asset(&c, "A-1");
    let first =
        judge(&mut c, &[VerdictTarget::Image { asset: a1, term: t }], Verdict::Yes, "user", "", "确认").unwrap();
    let second =
        judge(&mut c, &[VerdictTarget::Image { asset: a1, term: t }], Verdict::No, "user", "", "否定").unwrap();
    // 调用方看到的是上一步：历史已变化，什么都不撤。
    assert!(matches!(undo(&mut c, Some(first)), Err(Error::Conflict(ref m)) if m == HISTORY_CHANGED));
    assert_eq!(state(&c, a1, t).unwrap().as_deref(), Some("rejected"));
    assert!(matches!(undo(&mut c, Some(second)).unwrap(), Undo::Undone { action, .. } if action == second));
    assert_eq!(state(&c, a1, t).unwrap().as_deref(), Some("confirmed"));
    let none = Handover::default();
    let all = Handover { artists: true, labels: true };
    assert!(owns(&[json!({"terms": "verdicts"})], none));
    assert!(
        !owns(&[], all)
            && !owns(&[json!({"terms": "verdicts", "uuid": "a"})], all)
            && !owns(&[json!({"tag": "x"})], all)
    );
    // 旧的画师判断、用户标签步骤只在各自切到核心之后才归核心撤。
    let artist = [json!({"artist_verdict": {"uuid": "a", "artist": "x", "had": null}})];
    let tag = [json!({"uuid": "a", "tag": "x", "had": null})];
    assert!(!owns(&artist, none) && owns(&artist, Handover { artists: true, labels: false }));
    assert!(
        !owns(&tag, Handover { artists: true, labels: false }) && owns(&tag, Handover { artists: false, labels: true })
    );
    assert!(!owns(&[json!({"uuid": "a", "tag": "x", "had": null, "entity": "e"})], all));
}

#[test]
fn undo_refuses_a_library_without_the_current_schema() {
    let mut c = Connection::open_in_memory().unwrap();
    assert!(matches!(undo(&mut c, None), Err(Error::Schema(_))));
    let mut c = db();
    c.execute("UPDATE meta SET value='99' WHERE key='terms_schema'", []).unwrap();
    assert!(matches!(undo_last(&mut c), Err(Error::Schema(_))));
}

fn package_with_terms(assets: &[&str], terms: &[Value], rows: &[Value]) -> Package {
    use crate::package::tests::seal;
    let evidence: String = rows.iter().map(|r| canonical(r) + "\n").collect();
    let described: String = terms.iter().map(|r| canonical(r) + "\n").collect();
    let mut m = manifest(assets);
    m["files"]["terms.jsonl"] =
        json!({"bytes": described.len(), "sha256": crate::strict_json::sha256_hex(described.as_bytes())});
    Package::from_texts(&seal(m, &evidence), &evidence, Some(&described)).unwrap()
}

fn own(id: &str) -> Value {
    json!({"kind": "artist", "link": {"system": "provenance", "key": id}})
}

#[test]
fn producers_describe_their_terms_links_and_names() {
    let mut c = db();
    let describe = |links: Value, name: &str| json!({"term": own("ART-1"), "links": links, "names": [{"value": name, "source": "pixiv"}]});
    let row = json!({"id": "e", "asset": {"namespace": "photos", "id": "A-1"}, "term": own("ART-1"),
                     "source": "provenance", "method": "pixel_hash", "version": "1", "score": null,
                     "observed_at": "2026-09-28T00:00:00Z", "data": {}});
    let p = package_with_terms(
        &["A-1"],
        &[describe(json!([{"system": "x", "key": "foo", "url": "https://x.com/foo"}]), "フー")],
        &[row],
    );
    let out = import_package(&mut c, &p, "lib-1").unwrap();
    assert_eq!(out.created_terms, 1);
    // 产出者自己的链接系统给出 key：界面上的画师 id 不变。
    let (t, kind) = term_by_key(&c, "ART-1").unwrap().unwrap();
    assert_eq!((kind, term_by_link(&c, "x", "foo").unwrap()), (Kind::Artist, Some(t)));
    assert_eq!(state(&c, asset(&c, "A-1"), t).unwrap().as_deref(), Some("machine"));
    // 下一次：昵称改了、链接移到同一产出者的另一位画师。
    let other = json!({"term": own("ART-2"), "links": [{"system": "x", "key": "foo"}]});
    let p = package_with_terms(&["A-1"], &[describe(json!([]), "Foo"), other], &[]);
    import_package(&mut c, &p, "lib-1").unwrap();
    let names: Vec<String> = c
        .prepare("SELECT value FROM term_names")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(names, ["Foo"]);
    assert_eq!(term_by_link(&c, "x", "foo").unwrap(), Some(term_by_key(&c, "ART-2").unwrap().unwrap().0));
    // 别处（用户）建的词条上的链接，产出者不能拿走；整包回滚。
    let user = create_term(&c, Kind::Artist, None, "user").unwrap();
    add_link(&c, user, "pixiv", "9", "", "user").unwrap();
    let p =
        package_with_terms(&["A-1"], &[json!({"term": own("ART-1"), "links": [{"system": "pixiv", "key": "9"}]})], &[]);
    assert!(
        matches!(import_package(&mut c, &p, "lib-1"), Err(Error::Conflict(ref m)) if m.contains("terms.jsonl 第 1 行"))
    );
    assert_eq!(term_by_link(&c, "pixiv", "9").unwrap(), Some(user));
}

#[test]
fn library_assets_register_on_first_use() {
    let mut c = db();
    c.execute_batch("CREATE TABLE assets(uuid TEXT PRIMARY KEY); INSERT INTO assets VALUES ('NEW-1');").unwrap();
    assert!(library_asset(&c, "NOPE").unwrap().is_none());
    let id = library_asset(&c, "NEW-1").unwrap().unwrap();
    assert_eq!(library_asset(&c, "NEW-1").unwrap(), Some(id));
    let mut m = manifest(&[]);
    m["assets"] = json!([{"asset": {"namespace": "library", "id": "NEW-1"}, "status": "completed", "error": null}]);
    let row = json!({"id": "e", "asset": {"namespace": "library", "id": "NEW-1"}, "term": own("ART-9"),
                     "source": "provenance", "method": "pixel_hash", "version": "1", "score": null,
                     "observed_at": "2026-09-28T00:00:00Z", "data": {}});
    let (mt, e) = build(m, &[row]);
    assert_eq!(import_package(&mut c, &Package::from_parts(&mt, &e).unwrap(), "lib-1").unwrap().added, 1);
}

#[test]
fn replaceable_evidence_counts_only_through_its_active_set() {
    let c = db();
    let (a1, a2) = (asset(&c, "A-1"), asset(&c, "A-2"));
    let p = create_term(&c, Kind::Character, Some("p"), "legacy").unwrap();
    let topic = create_term(&c, Kind::Topic, Some("solo"), "legacy").unwrap();
    let evidence = |a: AssetId, term: TermId, source: &str, method: &str, version: &str, score: f64, metric: &str| {
        c.execute(
            "INSERT INTO term_evidence(asset_id,term_id,source,method,version,score,metric,fingerprint) VALUES (?1,?2,?3,?4,?5,?6,?7,zeroblob(16))",
            params![a, term, source, method, version, score, metric],
        )
        .unwrap();
        c.last_insert_rowid()
    };
    let set = |scope: &str, assets: &[AssetId], rows: &[i64]| {
        c.execute(
            "INSERT INTO term_evidence_sets(producer,source,method,scope) VALUES ('test','wd','wd_character',?1)",
            [scope],
        )
        .unwrap();
        let s = c.last_insert_rowid();
        for a in assets {
            c.execute("INSERT INTO term_evidence_set_assets VALUES (?1,?2)", [s, *a]).unwrap();
        }
        for r in rows {
            c.execute("INSERT INTO term_evidence_set_rows VALUES (?1,?2)", [s, *r]).unwrap();
        }
    };
    let shown = |a: AssetId, t: TermId| state(&c, a, t).unwrap();
    let old = evidence(a1, p, "wd", "wd_character", "v1", 0.8, "wd_probability");
    evidence(a1, topic, "wd", "tagger", "v1", 0.9, "wd_probability");
    project(&c, &[a1, a2]).unwrap();
    assert_eq!(shown(a1, p), None, "可替换来源没有批次就不生效");
    assert_eq!(shown(a1, topic), Some("machine".into()), "其余来源照旧全部有效");
    // 迁移批次覆盖全库：A-1 有这条证据，A-2 没有。
    set("all", &[], &[old]);
    assert_eq!(rebuild_active_evidence(&c).unwrap(), vec![a1, a2, asset(&c, "A-3")]);
    project(&c, &[a1, a2]).unwrap();
    assert_eq!(shown(a1, p), Some("maybe".into()));
    // 之后只重扫 A-2：A-1 仍按迁移批次。
    let fresh = evidence(a2, p, "wd", "wd_character", "v2", 0.7, "wd_probability");
    set("assets", &[a2], &[fresh]);
    assert_eq!(rebuild_active_evidence(&c).unwrap(), vec![a2]);
    project(&c, &[a1, a2]).unwrap();
    assert_eq!((shown(a1, p), shown(a2, p)), (Some("maybe".into()), Some("maybe".into())));
    // 重扫 A-1 成功但没有结果：旧证据失效（证据行仍在）。
    set("assets", &[a1], &[]);
    assert_eq!(rebuild_active_evidence(&c).unwrap(), vec![a1]);
    project(&c, &[a1]).unwrap();
    assert_eq!(shown(a1, p), None);
    assert_eq!(count(&c, "term_evidence"), 3);
    assert_eq!(rebuild_active_evidence(&c).unwrap(), Vec::<AssetId>::new(), "再重建一次没有变化");
}

fn provenance(asset: &str, method: &str, key: &str) -> Value {
    json!({
        "id": format!("{asset}-{method}-{key}"),
        "asset": {"namespace": "photos", "id": asset},
        "term": {"kind": "artist", "link": {"system": "danbooru", "key": key}},
        "source": "provenance", "method": method, "version": format!("v4/{method}"),
        "score": null, "observed_at": "2026-09-28T02:00:00Z", "data": {}
    })
}

#[test]
fn provenance_packages_are_snapshots_of_their_completed_pictures() {
    let mut c = db();
    // 第一次：A-1 由分组推荐给了候选，A-2 有 IQDB 认定，A-3 有同一张图的认定。
    let first =
        [provenance("A-1", "group_oc", "x"), provenance("A-2", "iqdb", "x"), provenance("A-3", "same_picture", "y")];
    import(&mut c, &["A-1", "A-2", "A-3"], &first).unwrap();
    assert_eq!(st(&c, "A-1", "x"), Some("maybe".into()));
    assert_eq!(st(&c, "A-2", "x"), Some("machine".into()), "IQDB 是强证据");
    assert_eq!(st(&c, "A-3", "y"), Some("machine".into()), "同一张图是强证据");
    // 同样的快照再交一次：什么都不变（包去重）。
    assert!(import(&mut c, &["A-1", "A-2", "A-3"], &first).unwrap().already);
    // 分组推荐重算后 A-1 不再有候选：A-1 完成且没有行 = 查无结果，旧候选失效；A-3 这次没完成（不在包里），不动。
    import(&mut c, &["A-1", "A-2"], &[provenance("A-2", "iqdb", "x")]).unwrap();
    assert_eq!(st(&c, "A-1", "x"), None);
    assert_eq!(st(&c, "A-2", "x"), Some("machine".into()));
    assert_eq!(st(&c, "A-3", "y"), Some("machine".into()));
    // 人的裁决不受机器快照影响。
    let (a1, x) = (asset(&c, "A-1"), linked(&c, "x"));
    judge(&mut c, &[VerdictTarget::Image { asset: a1, term: x }], Verdict::Yes, "user", "", "是 TA").unwrap();
    import(&mut c, &["A-1"], &[]).unwrap();
    assert_eq!(st(&c, "A-1", "x"), Some("confirmed".into()));
    // 证据只增不删：失效的行还在。
    assert_eq!(count(&c, "term_evidence"), 3);
    project_all(&mut c).unwrap();
    assert_eq!(st(&c, "A-1", "x"), Some("confirmed".into()));
    assert_eq!(st(&c, "A-3", "y"), Some("machine".into()));
}

/// 角色切到核心之后，角色树里的词条的投影归 `character_projection` 维护：别的词条的裁决、整表重投影都不改写那一片
/// （2026-09-30 真实副本上发现：对一张有角色候选的图判一次画师，角色行被通用规则改成了没有分数的 maybe）。
#[test]
fn verdicts_and_reprojection_leave_the_character_slice_alone() {
    let mut c = db();
    let asset = 1;
    let character = create_term(&c, Kind::Character, None, "user").unwrap();
    let artist = create_term(&c, Kind::Artist, None, "user").unwrap();
    // 布置：角色切换留下的样子（角色树、切换标记、角色投影一行）。
    crate::schema::begin_writing(&c).unwrap();
    c.execute("INSERT INTO term_tree(term_id) VALUES (?1)", [character]).unwrap();
    c.execute("INSERT INTO meta(key,value) VALUES ('terms_characters','core')", []).unwrap();
    c.execute(
        "INSERT INTO term_projection(asset_id,term_id,state,score,sources) VALUES (?1,?2,'machine',0.9,'CCIP')",
        [asset, character],
    )
    .unwrap();
    c.execute("DELETE FROM meta WHERE key=?1", [crate::schema::WRITER_KEY]).unwrap();
    let slice = |c: &Connection| -> Vec<(String, Option<f64>, String)> {
        c.prepare("SELECT state,score,sources FROM term_projection WHERE term_id=?1")
            .unwrap()
            .query_map([character], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    let before = slice(&c);
    judge(&mut c, &[VerdictTarget::Image { asset, term: artist }], Verdict::No, "user", "", "否定画师").unwrap();
    assert_eq!(slice(&c), before);
    assert_eq!(state(&c, asset, artist).unwrap().as_deref(), Some("rejected"));
    project_all(&mut c).unwrap();
    assert_eq!(slice(&c), before);
    assert_eq!(state(&c, asset, artist).unwrap().as_deref(), Some("rejected"));
}
