//! 在对照基准里的第 7 版图库上做词条核心第 8 版升级（#201）：只有旧表与 `term_*` 一致、且每个领域都已切换到词条核心时
//! 才升级；升级时把别名和显示名移进词条核心，并为带数量的历史步骤补上种类和数量。拒绝升级时图库保持原样，一点不变。
//! （`baseline.rs` 的每个检查点都以同样方式升级；本文件覆盖拒绝的情况和对照没有涵盖的部分。）
#![allow(clippy::unwrap_used, reason = "集成测试里 unwrap 就是断言")]
use rusqlite::Connection;
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::common::baseline;

/// 已切换的第 7 版图库，含角色、链接、名称和历史步骤（golden 名称写入场景的第一个检查点）。
const CASE: &str = "tests.test_terms_golden.GoldenNameWrites.test_core_name_writes_match_the_snapshot";

struct Library {
    dir: PathBuf,
    db: PathBuf,
}

impl Drop for Library {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn library(name: &str) -> Library {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/baseline/test_terms_golden.json.gz");
    let mut text = String::new();
    flate2::read::GzDecoder::new(fs::File::open(path).unwrap()).read_to_string(&mut text).unwrap();
    let data: Value = serde_json::from_str(&text).unwrap();
    let entry = data["cases"][CASE]["calls"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|call| call["env"].as_array().cloned().unwrap_or_default())
        .find(|e| e["path"] == "@ROOT@/app.sqlite" && e.get("db").is_some())
        .expect("a checkpoint of the library");
    assert_eq!(entry["terms"], 7);
    let dir = std::env::temp_dir().join(format!("illustash-v8-upgrade-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let root = dir.to_string_lossy().replace('\\', "/");
    let part = |i: usize| data["blobs"][entry["db"][i].as_str().unwrap()].as_str().unwrap().replace("@ROOT@", &root);
    let db = dir.join("app.sqlite");
    baseline::restore(&Connection::open(&db).unwrap(), &part(0), &part(1), &part(2));
    Library { dir, db }
}

/// 运行词条核心的升级；成功返回 Ok(stdout)，失败返回 Err(stderr)。
fn upgrade(db: &Path) -> Result<String, String> {
    let done = Command::new(env!("CARGO_BIN_EXE_illustash-terms")).args(["migrate", "--db"]).arg(db).output().unwrap();
    let (out, err) =
        (String::from_utf8_lossy(&done.stdout).into_owned(), String::from_utf8_lossy(&done.stderr).into_owned());
    if done.status.success() {
        Ok(out)
    } else {
        Err(err)
    }
}

/// 按早期版本（或某个 bug）可能留下的样子写入受保护的表。
fn write(c: &Connection, sql: &str) {
    c.execute_batch(&format!(
        "INSERT INTO meta VALUES ('terms_writer','1'); {sql}; DELETE FROM meta WHERE key='terms_writer';"
    ))
    .unwrap();
}

fn exact(db: &Path) -> std::collections::BTreeMap<String, Vec<String>> {
    baseline::exact(&Connection::open(db).unwrap(), &|t| t.to_owned())
}

/// 查询的每一行，转成文本（足以比较两张表的内容）。
fn rows(c: &Connection, sql: &str) -> Vec<String> {
    let mut st = c.prepare(sql).unwrap();
    let n = st.column_count();
    st.query_map([], |r| Ok(format!("{:?}", (0..n).map(|i| r.get::<_, rusqlite::types::Value>(i)).collect::<Vec<_>>())))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn exists(c: &Connection, name: &str) -> bool {
    c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = ?1)", [name], |r| r.get(0)).unwrap()
}

/// 第 7 版图库里的前两个角色，记作 `x` 和 `y`（`terms` 的行）。
const PAIR: &str = "(SELECT t.id, t.key FROM terms t JOIN entities e ON e.id = t.key WHERE e.kind = 'character'
                     ORDER BY e.rowid LIMIT 1) x,
                    (SELECT t.id, t.key FROM terms t JOIN entities e ON e.id = t.key WHERE e.kind = 'character'
                     ORDER BY e.rowid LIMIT 1 OFFSET 1) y";

const RELATION_TABLES: [(&str, &str, &str); 4] = illustash_terms::character_relations::TABLES;

fn names_version(c: &Connection) -> i64 {
    c.query_row("SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'names_version'", [], |r| r.get(0)).unwrap()
}

#[test]
fn a_consistent_library_upgrades_retires_the_key_tables_and_fills_in_counted_steps() {
    let _env = crate::shared();
    let lib = library("consistent");
    {
        let c = Connection::open(&lib.db).unwrap();
        c.execute_batch(
            "INSERT INTO actions(label,before_json) VALUES ('已将 2 张设为 合成角色','[]'),
              ('已将 合成角色 标为已检查（3 张）','[]'),('已将 1 张从 甲 换成 乙','[]'),('已更新 合成角色 的资料','[]');",
        )
        .unwrap();
    }
    // 词条核心的副本里还没有的行：到第 7 版为止，旧表是权威数据。
    write(
        &Connection::open(&lib.db).unwrap(),
        "INSERT INTO name_aliases VALUES ('work','synthetic_work','合成别名',0);
         INSERT INTO display_names VALUES ('work','synthetic_work','zh','user','合成作品名')",
    );
    // 一条作品复核和一行基线，两边都已经有了，就像词条核心的命令镜像写入的那样。
    write(
        &Connection::open(&lib.db).unwrap(),
        "INSERT INTO work_review SELECT id, 'confirmed', '合成备注', 1 FROM entities ORDER BY rowid LIMIT 1;
         UPDATE term_tree SET review_status = 'confirmed', review_note = '合成备注', parent_locked = 1
          WHERE term_id = (SELECT t.id FROM terms t JOIN entities e ON e.id = t.key ORDER BY e.rowid LIMIT 1);
         INSERT INTO term_identity_baseline(id, parent_id, variant, kind, merged_into)
          VALUES ('synthetic_baseline', NULL, '', 'character', NULL);
         INSERT INTO identity_baseline(rowid, id, parent_id, variant, kind, merged_into)
          SELECT seq, id, parent_id, variant, kind, merged_into FROM term_identity_baseline WHERE id = 'synthetic_baseline'",
    );
    // 一条已接受的重复关系及其证据和决定，写在词条核心里，也像词条核心的命令那样镜像写进四张旧关系表
    // （涉及的身份是前两个角色）。
    write(
        &Connection::open(&lib.db).unwrap(),
        &format!(
            "INSERT INTO term_relations(a, b, kind) SELECT x.id, y.id, 'same_as' FROM {PAIR};
             INSERT INTO term_legacy_ids VALUES ('relation', 9001, (SELECT max(id) FROM term_relations));
             INSERT INTO term_evidence(relation_id, source, method, version, data, fingerprint)
              VALUES ((SELECT max(id) FROM term_relations), 'manual', 'identity_evidence', 'synthetic',
               '{{\"legacy_fingerprint\":\"synthetic\",\"payload\":\"{{}}\",\"created_at\":\"2026-01-01 00:00:00\"}}',
               zeroblob(16));
             INSERT INTO term_batches(kind, anchor, label) VALUES ('migration', 0, '合成理由');
             INSERT INTO term_verdicts(batch_id, anchor, relation_id, verdict, origin, reason, reviewed, via, created_at)
              VALUES ((SELECT max(id) FROM term_batches), 0, (SELECT max(id) FROM term_relations), 'yes', 'legacy',
               '合成理由', json_array((SELECT max(id) FROM term_evidence)), 'migration', '2026-01-02 00:00:00');
             INSERT INTO term_legacy_ids VALUES ('decision', 9001, (SELECT max(id) FROM term_verdicts));
             INSERT INTO identity_relations SELECT 9001, x.key, y.key, 'duplicate_of', 'accepted' FROM {PAIR};
             INSERT INTO identity_evidence VALUES (9001, 'synthetic', 'manual', '{{}}', '2026-01-01 00:00:00');
             INSERT INTO identity_decisions VALUES (9001, 9001, 'proposed', 'accepted', '合成理由', '2026-01-02 00:00:00');
             INSERT INTO identity_decision_evidence VALUES (9001, 'synthetic');"
        ),
    );
    let aliases = "SELECT kind, key, value, position FROM {} ORDER BY kind, key, value";
    let display = "SELECT kind, key, lang, source, value FROM {} ORDER BY kind, key, lang, source, value";
    let before = Connection::open(&lib.db).unwrap();
    let old_aliases = rows(&before, &aliases.replace("{}", "name_aliases"));
    let old_display = rows(&before, &display.replace("{}", "display_names"));
    let old_names =
        rows(&before, "SELECT entity_id, lang, value, source, role, preferred, position FROM names ORDER BY rowid");
    assert!(old_names.len() > 1);
    let old_links =
        rows(&before, "SELECT system, key, entity_id, current, source, added_at FROM links ORDER BY system, key");
    assert!(old_links.iter().any(|l| l.contains("Text(\"wd\")")), "{old_links:?}");
    let old_reviews =
        rows(&before, "SELECT entity_id, status, note, parent_locked FROM work_review ORDER BY entity_id");
    let old_baseline =
        rows(&before, "SELECT rowid, id, parent_id, variant, kind, merged_into FROM identity_baseline ORDER BY rowid");
    assert!(!old_reviews.is_empty() && !old_baseline.is_empty());
    let identity =
        "id, kind, work, variant, parent_id, merged_into, artist, label_only, mascot, mascot_origin, work_id, original";
    let old_identities = rows(&before, &format!("SELECT rowid, {identity} FROM entities ORDER BY rowid"));
    assert!(old_identities.len() > 1);
    let old_relations: Vec<Vec<String>> = RELATION_TABLES
        .iter()
        .map(|(table, columns, _)| rows(&before, &format!("SELECT {columns} FROM {table} ORDER BY 1, 2")))
        .collect();
    assert!(old_relations.iter().all(|r| !r.is_empty()));
    let old_views: Vec<String> =
        rows(&before, "SELECT name FROM sqlite_master WHERE type = 'view' AND sql LIKE '%entities%' ORDER BY name");
    assert!(old_views.len() >= 5, "{old_views:?}");
    assert!(old_aliases.iter().any(|a| a.contains("合成别名")) && old_display.iter().any(|a| a.contains("合成作品名")));
    drop(before);
    let current = illustash_terms::SCHEMA_VERSION;
    assert_eq!(upgrade(&lib.db).unwrap(), format!("{{\"from\":7,\"to\":{current}}}\n"));
    let c = Connection::open(&lib.db).unwrap();
    assert_eq!(rows(&c, &aliases.replace("{}", "term_key_aliases")), old_aliases, "aliases move to the core unchanged");
    assert_eq!(rows(&c, &display.replace("{}", "term_key_names")), old_display, "display names move unchanged");
    assert!(!exists(&c, "name_aliases") && !exists(&c, "display_names"), "the legacy tables are dropped");
    // 名称保持原有顺序：`seq` 取原来的 rowid。
    let names = "SELECT t.key, n.lang, n.value, n.source, n.role, n.preferred, n.position
                 FROM term_names n JOIN terms t ON t.id = n.term_id JOIN term_tree r ON r.term_id = n.term_id ORDER BY n.seq";
    assert_eq!(rows(&c, names), old_names, "names move to the core in their order");
    assert!(!exists(&c, "names"));
    // 链接也一样，WD 在词条核心里写作 `wd_character`。
    let links =
        "SELECT CASE x.system WHEN 'wd_character' THEN 'wd' ELSE x.system END, x.key, t.key, x.current, x.source,
                   x.added_at
                 FROM term_links x JOIN terms t ON t.id = x.term_id JOIN term_tree r ON r.term_id = x.term_id
                 ORDER BY 1, 2";
    assert_eq!(rows(&c, links), old_links, "links move to the core unchanged");
    assert!(!exists(&c, "links"));
    // 作品复核和身份基线也一样（基线的 `seq` 是原来的 rowid）。
    let reviews = "SELECT t.key, r.review_status, r.review_note, r.parent_locked
                   FROM term_tree r JOIN terms t ON t.id = r.term_id WHERE r.review_status IS NOT NULL ORDER BY t.key";
    assert_eq!(rows(&c, reviews), old_reviews, "work reviews move to the core unchanged");
    let baseline = "SELECT seq, id, parent_id, variant, kind, merged_into FROM term_identity_baseline ORDER BY seq";
    assert_eq!(rows(&c, baseline), old_baseline, "the identity baseline is the core's already");
    assert!(!exists(&c, "work_review") && !exists(&c, "identity_baseline"));
    // 身份：词条及其 `term_tree` 行，保持原有顺序（`seq` 取原来的 rowid），属性改成列，`terms.attrs` 里不再留东西。
    let identities = "SELECT r.seq, t.key, t.kind, r.work_text, r.variant, r.parent_id, r.merged_into, r.artist,
                        r.label_only, r.mascot, r.mascot_origin, r.work_id, r.original
                      FROM term_tree r JOIN terms t ON t.id = r.term_id ORDER BY r.seq";
    assert_eq!(rows(&c, identities), old_identities, "identities move to the core unchanged");
    let attrs: i64 = c
        .query_row(
            "SELECT count(*) FROM terms WHERE attrs <> '{}' AND id IN (SELECT term_id FROM term_tree)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(attrs, 0);
    assert!(!exists(&c, "entities"));
    // 视图读取词条核心的表。
    let views: Vec<String> = rows(&c, "SELECT name FROM sqlite_master WHERE type = 'view' ORDER BY name");
    for view in &old_views {
        assert!(views.contains(view), "{view} is kept");
    }
    let left: Vec<String> = rows(&c, "SELECT name FROM sqlite_master WHERE sql LIKE '%entities%' ORDER BY name");
    assert!(left.is_empty(), "{left:?}");
    for view in ["family_confirmed", "membership_rules", "variant_memberships", "ambiguous_claims", "identity_nodes"] {
        c.prepare(&format!("SELECT * FROM {view}")).unwrap().query([]).unwrap().next().unwrap();
    }
    // 关系及其证据和决定都归词条核心：旧表已删除，按旧表的形状读词条核心，得到相同的行。
    for ((table, columns, fragment), old) in RELATION_TABLES.iter().zip(&old_relations) {
        assert!(!exists(&c, table), "{table} is dropped");
        assert_eq!(&rows(&c, &format!("SELECT {columns} FROM {fragment} ORDER BY 1, 2")), old, "{table}");
    }
    let broken: i64 = c.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get(0)).unwrap();
    assert_eq!(broken, 0);
    // 现在名称版本号统计的是对词条核心显示名的写入，就像以前统计旧表的写入一样。
    let version = names_version(&c);
    c.execute("INSERT INTO term_key_names VALUES ('skin','synthetic_skin','zh','user','合成皮肤名')", []).unwrap();
    assert_eq!(names_version(&c), version + 1);
    // ……也统计对词条核心里角色、皮肤和作品的名称与链接的写入，不统计画师的。
    c.execute(
        "INSERT INTO term_names(term_id, lang, value, source, role, seq)
         SELECT term_id, 'zh', '合成名', 'user', 'alias', 0 FROM term_tree LIMIT 1",
        [],
    )
    .unwrap();
    assert_eq!(names_version(&c), version + 2);
    c.execute(
        "INSERT INTO term_links(system, key, term_id, source) SELECT 'danbooru', 'synthetic_tag', term_id, 'user' FROM term_tree LIMIT 1",
        [],
    )
    .unwrap();
    assert_eq!(names_version(&c), version + 3);
    c.execute_batch(
        "INSERT INTO terms(key, kind, origin) VALUES ('synthetic_artist', 'artist', 'user');
         INSERT INTO term_names(term_id, lang, value, source, role)
         SELECT id, 'zh', '合成画师', 'user', 'display' FROM terms WHERE key = 'synthetic_artist';
         INSERT INTO term_links(system, key, term_id, source)
         SELECT 'pixiv', '42', id, 'user' FROM terms WHERE key = 'synthetic_artist';",
    )
    .unwrap();
    assert_eq!(names_version(&c), version + 3);
    // ……也统计对身份自身的列和种类的写入，不统计作品复核。
    write(
        &c,
        "UPDATE term_tree SET variant = 'synthetic_variant' WHERE seq = (SELECT min(seq) FROM term_tree);
         UPDATE term_tree SET review_note = '只改复核' WHERE review_status IS NOT NULL;",
    );
    assert_eq!(names_version(&c), version + 4);
    write(
        &c,
        "UPDATE terms SET kind = CASE kind WHEN 'skin' THEN 'character' ELSE 'skin' END
         WHERE id = (SELECT term_id FROM term_tree ORDER BY seq LIMIT 1)",
    );
    assert_eq!(names_version(&c), version + 5);
    let filled: Vec<(String, Option<String>, Option<String>)> = c
        .prepare("SELECT label,label_kind,label_count FROM actions ORDER BY id DESC LIMIT 4")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let kinds: Vec<(Option<&str>, Option<&str>)> =
        filled.iter().map(|(_, k, n)| (k.as_deref(), n.as_deref())).collect();
    assert_eq!(kinds, [(None, None), (Some("swap"), Some("1")), (Some("sweep"), Some("3")), (Some("set"), Some("2"))]);
    let version: String = c.query_row("SELECT value FROM meta WHERE key='terms_schema'", [], |r| r.get(0)).unwrap();
    assert_eq!(version, current.to_string());
}

#[test]
fn legacy_tables_that_disagree_with_the_core_refuse_the_upgrade_and_change_nothing() {
    let _env = crate::shared();
    for (name, sql, refusal) in [
        (
            "disagree-entities",
            "UPDATE entities SET work='改过' WHERE rowid=(SELECT min(rowid) FROM entities)",
            "旧身份表与词条核心不一致",
        ),
        (
            "disagree-links",
            "UPDATE links SET current=1-current WHERE rowid=(SELECT min(rowid) FROM links)",
            "旧链接表与词条核心不一致",
        ),
        (
            "disagree-relations",
            "INSERT INTO identity_relations SELECT 9002, x.key, y.key, 'different_from', 'proposed' FROM {PAIR}",
            "旧身份关系表与词条核心不一致",
        ),
        (
            "disagree-review",
            "INSERT INTO work_review SELECT id, 'confirmed', '只在旧表里', 0 FROM entities ORDER BY rowid LIMIT 1",
            "旧作品复核表与词条核心不一致",
        ),
    ] {
        let lib = library(name);
        write(&Connection::open(&lib.db).unwrap(), &sql.replace("{PAIR}", PAIR));
        let before = exact(&lib.db);
        let refused = upgrade(&lib.db).unwrap_err();
        assert!(refused.contains(refusal), "{name}: {refused}");
        assert_eq!(exact(&lib.db), before);
    }
}

#[test]
fn a_library_not_switched_in_every_domain_is_refused_and_unchanged() {
    let _env = crate::shared();
    let lib = library("unswitched");
    Connection::open(&lib.db).unwrap().execute("DELETE FROM meta WHERE key='terms_names'", []).unwrap();
    let before = exact(&lib.db);
    let refused = upgrade(&lib.db).unwrap_err();
    assert!(refused.contains("pre-v8"), "{refused}");
    assert_eq!(exact(&lib.db), before);
}

#[test]
fn an_outdated_names_mirror_is_brought_up_to_date_first_as_every_core_write_does() {
    let _env = crate::shared();
    let lib = library("mirror");
    let entity: String = Connection::open(&lib.db)
        .unwrap()
        .query_row("SELECT entity_id FROM names ORDER BY rowid LIMIT 1", [], |r| r.get(0))
        .unwrap();
    write(
        &Connection::open(&lib.db).unwrap(),
        &format!("INSERT INTO names(entity_id,lang,value,source,role,preferred,position) VALUES ('{entity}','zh','待同步','user','display',0,0)"),
    );
    upgrade(&lib.db).unwrap();
    let mirrored: i64 = Connection::open(&lib.db)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM term_names n JOIN terms t ON t.id=n.term_id WHERE t.key=?1 AND n.value='待同步'",
            [&entity],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(mirrored, 1);
}
