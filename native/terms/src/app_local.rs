//! 没有研究 worker 时宿主本地的信息。不启动任务，也不授予联网权限。
use crate::{
    http::LibraryContext,
    http_json::Json,
    http_read::{rows, strings},
    Error, Result,
};
use rusqlite::{Connection, OpenFlags};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};
pub(crate) const ROUTES: &[&str] =
    &["/api/about", "/api/sync", "/api/names", "/api/index", "/api/provenance", "/api/accounts", "/api/oc/groups"];
fn off() -> Json {
    Json::object([("status", "off".into())])
}
pub(crate) fn read(context: &LibraryContext, url: &str) -> Result<Json> {
    let q = crate::http_read::query(url);
    let path = url.split('?').next().unwrap_or(url);
    match path {
        "/api/about" => return Ok(about(context)),
        "/api/index" => return Ok(Json::object([("state", off())])),
        "/api/provenance" => return Ok(Json::object([("available", false.into())])),
        // OC 细分是按需的研究计算，从不存成缓存。
        "/api/oc/groups" => return Err(Error::Invalid("图像特征未加载，暂时无法按相似细分".into())),
        "/api/accounts" => return accounts(context),
        _ => {}
    }
    let mut c = crate::sqlite::open_with_flags(&context.db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    c.busy_timeout(std::time::Duration::from_secs(10))?;
    let tx = c.transaction()?;
    let result = match path {
        "/api/sync" => {
            let mut history = rows(&tx, "SELECT * FROM sync_runs ORDER BY id DESC LIMIT 8", &[])?;
            for r in &mut history {
                let s = r.get("sources").expect("app_local internal invariant").text();
                r.set("sources", if s.is_empty() { None::<String>.into() } else { crate::http_json::parse_python(s)? });
            }
            Json::object([("state", off()), ("history", Json::Array(history))])
        }
        "/api/names" => names(&tx, &context.db, q.get("progress").is_some_and(|p| p == "1"))?,
        _ => return Err(Error::Invalid("未迁移宿主接口".into())),
    };
    tx.commit()?;
    Ok(result)
}
fn about(context: &LibraryContext) -> Json {
    let platform = crate::platform_system::platform();
    Json::object([
        ("name", env!("ILLUSTASH_NAME").into()),
        ("version", env!("ILLUSTASH_VERSION").into()),
        ("channel", env!("ILLUSTASH_CHANNEL").into()),
        ("build", env!("ILLUSTASH_BUILD").into()),
        ("schema", 36i64.into()),
        ("python", None::<String>.into()),
        ("platform", platform.into()),
        ("libraries", components(context)),
        ("runtime", env!("ILLUSTASH_RUSTC").into()),
        ("can_open_logs", crate::app_desktop::available().into()),
    ])
}
/// worker 会用到的识别组件（#148）：识别 worker 和它加载的 ONNX Runtime，按 worker 使用的同样路径查找
/// （会话的资源根目录，同步以 `ILLUSTASH_ROOT` 交给 worker）。缺少的为 null（显示为「未安装」）。只看有没有：
/// 宿主从不加载 ONNX Runtime，也不报告任何路径。
fn components(context: &LibraryContext) -> Json {
    let worker = std::env::var_os("ILLUSTASH_RECOGNITION_WORKER")
        .is_some_and(|p| !p.is_empty() && std::path::Path::new(&p).is_file());
    // 会话的 tags 文件是 `<root>/tags/tags_v3.csv`。
    let root = context
        .tags
        .parent()
        .and_then(std::path::Path::parent)
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(crate::app_resources::default_root);
    let runtime = crate::app_resources::ort_runtime(&root).is_file();
    let state = |present: bool, label: &str| if present { Json::from(label) } else { None::<String>.into() };
    Json::object([("识别组件", state(worker, "可用")), ("ONNX Runtime", state(runtime, "已安装"))])
}
pub(crate) fn names(c: &Connection, path: &Path, progress: bool) -> Result<Json> {
    let mut out = Json::object([(
        "state",
        Json::object([("status", "off".into()), ("done", 0i64.into()), ("total", 0i64.into())]),
    )]);
    if progress {
        return Ok(out);
    }
    let counts = if crate::legacy_names::projection_current(c)? {
        let counts = projected_counts(c)?;
        if crate::legacy_names::verifying() {
            let loaded = loaded_counts(c)?;
            if counts.dump() != loaded.dump() {
                return Err(Error::Conflict(format!(
                    "按投影统计的名字覆盖与整库载入不一致：{projected} ≠ {loaded}",
                    projected = counts.dump(),
                    loaded = loaded.dump()
                )));
            }
            crate::legacy_names::record_verified("names_counts")?;
        }
        counts
    } else {
        loaded_counts(c)?
    };
    out.set("counts", counts);
    out.set("model_api", crate::app_names::model_api::status(c, path)?);
    Ok(out)
}
/// `/api/names` 统计的角色、作品和皮肤：有效、已链接、未合并的非 OC 身份的角色 tag、作品文本和皮肤变体。
const KINDS: [(&str, &str); 3] = [("characters", "character"), ("works", "work"), ("skins", "skin")];
fn count_json(total: i64, zh: i64, ja: i64, readable: i64) -> Json {
    Json::object([("total", total.into()), ("zh", zh.into()), ("ja", ja.into()), ("readable", readable.into())])
}
/// 来自显示投影的计数（#105）：身份和键在 SQL 里，每个键选用的名称从 `term_name_choice` 读；
/// 投影里没有的键（没有 tag 的角色）从它自己的行里选。只有投影按类别的合计和未投影的键会到 Rust 这边。
/// `shown` 只物化一次，而不是内联进三个键分支的每一个。
fn projected_counts(c: &Connection) -> Result<Json> {
    let mut sums: HashMap<String, (i64, i64, i64, i64, String)> = c
        .prepare(
            &format!(
            "WITH active(id) AS (SELECT entity FROM memberships UNION SELECT entity FROM candidates
               UNION SELECT parent_id FROM {ENTITIES} WHERE id IN (SELECT entity FROM memberships UNION SELECT entity FROM candidates)
                AND parent_id IS NOT NULL),
             shown AS MATERIALIZED (SELECT e.id,e.parent_id,coalesce(e.work,'') AS work,coalesce(e.variant,'') AS variant FROM {ENTITIES} e
               WHERE e.id IN active AND e.merged_into IS NULL AND e.kind NOT IN ('oc','work') AND coalesce(e.original,0)=0
                AND EXISTS(SELECT 1 FROM {LINKS} l WHERE l.entity_id=e.id AND l.current=1)),
             keys(kind,key) AS (SELECT 'character',d.tag FROM shown s JOIN term_name_display d ON d.entity=s.id AND d.skin_only=0
                WHERE s.parent_id IS NULL
               UNION SELECT 'work',work FROM shown WHERE work<>''
               UNION SELECT 'skin',variant FROM shown WHERE variant<>'')
             SELECT k.kind,count(*),coalesce(sum(coalesce(ch.zh,'')<>''),0),coalesce(sum(coalesce(ch.ja,'')<>''),0),
              coalesce(sum(ch.name_source='readable'),0),json_group_array(k.key) FILTER (WHERE ch.kind IS NULL)
             FROM keys k LEFT JOIN term_name_choice ch ON ch.kind=k.kind AND ch.key=k.key GROUP BY k.kind",
            LINKS = crate::character_links::LINKS,
            ENTITIES = crate::character_entities::ENTITIES
        ),
        )?
        .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))))?
        .collect::<rusqlite::Result<_>>()?;
    let mut counts = Json::Object(Vec::new());
    for (plural, kind) in KINDS {
        let (total, mut zh, mut ja, mut readable, missing) = sums.remove(kind).unwrap_or((0, 0, 0, 0, "[]".into()));
        let missing: Vec<String> = serde_json::from_str(&missing).map_err(|e| Error::Invalid(e.to_string()))?;
        for key in missing {
            let choice = crate::names_view::choose_key(c, kind, &key)?;
            zh += i64::from(choice.names[0].as_ref().is_some_and(|s| !s.is_empty()));
            ja += i64::from(choice.names[1].as_ref().is_some_and(|s| !s.is_empty()));
            readable += i64::from(choice.source == "readable");
        }
        counts.set(plural, count_json(total, zh, ja, readable));
    }
    Ok(counts)
}
/// 全图库一次加载得到的计数（#102）：名称还不在核心里，或投影落后于名称表。也是增量检查下投影计数的对照基准。
fn loaded_counts(c: &Connection) -> Result<Json> {
    let sql = format!(
        "SELECT entity FROM memberships
         UNION SELECT entity FROM candidates
         UNION SELECT parent_id
         FROM {ENTITIES}
         WHERE id IN (SELECT entity FROM memberships UNION SELECT entity FROM candidates) AND parent_id IS NOT NULL",
        ENTITIES = crate::character_entities::ENTITIES
    );
    let active: HashSet<_> = strings(c, &sql, &[])?.into_iter().collect();
    let view = crate::names_view::View::load_with_work(c)?;
    let mut keys: [HashSet<String>; 3] = Default::default();
    for e in view.identities() {
        if e.merged_into.is_some()
            || matches!(e.kind, "oc" | "work")
            || e.original
            || !e.linked
            || !active.contains(e.id)
        {
            continue;
        }
        if e.parent.is_none() {
            keys[0].insert(e.tag.into());
        }
        for (i, s) in [(1, e.work), (2, e.variant)] {
            if !s.is_empty() {
                keys[i].insert(s.into());
            }
        }
    }
    let mut counts = Json::Object(Vec::new());
    for (i, (plural, kind)) in KINDS.into_iter().enumerate() {
        let mut zh = 0;
        let mut ja = 0;
        let mut readable = 0;
        for key in &keys[i] {
            let choice = view.choose(kind, key)?;
            zh += i64::from(choice.names.first().and_then(|x| x.as_ref()).is_some_and(|s| !s.is_empty()));
            ja += i64::from(choice.names.get(1).and_then(|x| x.as_ref()).is_some_and(|s| !s.is_empty()));
            readable += i64::from(choice.source == "readable");
        }
        counts.set(plural, count_json(keys[i].len() as i64, zh, ja, readable));
    }
    Ok(counts)
}
fn accounts(context: &LibraryContext) -> Result<Json> {
    let mut out = crate::app_accounts::status()?;
    let mut x = Json::object([("imported", 0i64.into())]);
    if let Some(path) = context.library.as_ref().map(|p| p.join("provenance.sqlite")).filter(|p| p.exists()) {
        let c = crate::sqlite::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut resolved = 0;
        let mut pending = 0;
        let mut total = 0;
        for r in rows(&c, "SELECT status,count(*) AS n FROM leads WHERE source='x_following' GROUP BY status", &[])? {
            let n = r.get("n").expect("app_local internal invariant").integer();
            total += n;
            match r.get("status").expect("app_local internal invariant").text() {
                "resolved" => resolved = n,
                "pending" => pending = n,
                _ => {}
            }
        }
        let time = rows(&c, "SELECT max(added_at) AS t FROM leads WHERE source='x_following'", &[])?
            .remove(0)
            .get("t")
            .expect("app_local internal invariant")
            .clone();
        x = Json::object([
            ("imported", total.into()),
            ("resolved", resolved.into()),
            ("pending", pending.into()),
            ("imported_at", time),
        ]);
    }
    out.set("x", x);
    let name = context
        .library
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p.join("library.json")).ok())
        .and_then(|s| crate::http_json::parse_python(&s).ok())
        .and_then(|v| v.get("name").cloned())
        .unwrap_or_else(|| None::<String>.into());
    out.set("library", name);
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// 名称更新器的合成基础图库，加上计数会筛选的每种身份形态。
    fn library() -> (std::path::PathBuf, Connection) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/names/base.json.gz");
        let mut text = String::new();
        flate2::read::GzDecoder::new(std::fs::File::open(path).unwrap()).read_to_string(&mut text).unwrap();
        let data: serde_json::Value = serde_json::from_str(&text).unwrap();
        let root = std::env::temp_dir().join(format!(
            "illustash-local-names-{}-{}",
            std::process::id(),
            crate::store::new_key().unwrap()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut c = Connection::open(root.join("library.sqlite")).unwrap();
        c.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        c.execute_batch(&data["base"]["library"].as_str().unwrap().replace("@ROOT@", &root.to_string_lossy())).unwrap();
        crate::migrate(&mut c).unwrap();
        let tx = c.transaction().unwrap();
        crate::schema::begin_writing(&tx).unwrap();
        tx.execute_batch(
            "CREATE TEMP TABLE added(n INTEGER PRIMARY KEY,id TEXT,work TEXT,variant TEXT,parent_id TEXT,merged_into TEXT,
              kind TEXT,original INTEGER);
             INSERT INTO added(id,work,variant,parent_id,merged_into,kind,original) VALUES
              ('tagless','Example','',NULL,NULL,'character',0),('hidden','Other','',NULL,NULL,'character',0),
              ('skin1','Example','swimsuit','tagless',NULL,'skin',0),('skin2','','outfit_x','tagless',NULL,'skin',0),
              ('merged','Example','',NULL,'tagless','character',0),('own','Mine','',NULL,NULL,'character',1),
              ('loose','Loose','',NULL,NULL,'character',0),('idle','Idle','',NULL,NULL,'character',0);
             -- New identities: a term and its row in `term_tree`, as the commands make them (`character_entities::create`).
             INSERT INTO terms(key,kind,origin) SELECT id,kind,'user' FROM added ORDER BY n;
             INSERT INTO term_tree(term_id,parent_id,merged_into,work_text,variant,original,seq)
              SELECT t.id,a.parent_id,a.merged_into,a.work,a.variant,a.original,
               (SELECT coalesce(max(seq),0) FROM term_tree)+a.n
              FROM added a JOIN terms t ON t.key=a.id ORDER BY a.n;
             DROP TABLE added;
             INSERT INTO term_links(system,key,term_id,current,source,added_at) VALUES
              ('bangumi','tagless',(SELECT id FROM terms WHERE key='tagless'),1,'import','2026-10-07 06:00:00'),
              -- (No empty-key link: `term_links` keys are 1–256 characters, as the mirror required before.)
              ('danbooru','hidden_tag',(SELECT id FROM terms WHERE key='hidden'),1,'import','2026-10-07 06:00:02'),
              ('danbooru','swim_tag',(SELECT id FROM terms WHERE key='skin1'),1,'import','2026-10-07 06:00:03'),('danbooru','outfit_tag',(SELECT id FROM terms WHERE key='skin2'),1,'import','2026-10-07 06:00:08'),('danbooru','merged_tag',(SELECT id FROM terms WHERE key='merged'),1,'import','2026-10-07 06:00:04'),
              ('danbooru','own_tag',(SELECT id FROM terms WHERE key='own'),1,'import','2026-10-07 06:00:05'),('danbooru','loose_tag',(SELECT id FROM terms WHERE key='loose'),0,'import','2026-10-07 06:00:06'),
              ('danbooru','idle_tag',(SELECT id FROM terms WHERE key='idle'),1,'import','2026-10-07 06:00:07');
             INSERT INTO term_names(term_id,lang,value,source,role,preferred,position,seq) VALUES
              ((SELECT id FROM terms WHERE key='tagless'),'ja','タグナシ','index','display',0,0,
               (SELECT coalesce(max(seq),0)+1 FROM term_names));
             INSERT INTO term_names(term_id,lang,value,source,role,preferred,position,seq) VALUES
              ((SELECT id FROM terms WHERE key='hidden'),'zh','隐','user','display',1,0,
               (SELECT coalesce(max(seq),0)+1 FROM term_names));
             INSERT INTO term_key_names VALUES ('character','','en','wiki','Empty key'),('skin','outfit_x','zh','index','外套');
             INSERT INTO candidates VALUES ('a','tagless','WD',0.9),('a','hidden','WD',0.9),('a','skin1','WD',0.9),
              ('a','skin2','WD',0.9),('a','merged','WD',0.9),('a','own','WD',0.9),('a','loose','WD',0.9);",
        )
        .unwrap();
        crate::schema::commit_writing(tx).unwrap();
        (root, c)
    }

    #[test]
    fn projected_counts_match_a_whole_load() {
        let (root, c) = library();
        let tx = c.unchecked_transaction().unwrap();
        assert!(crate::legacy_names::projection_current(&tx).unwrap());
        let start = crate::names_view::whole_loads();
        let projected = projected_counts(&tx).unwrap();
        assert_eq!(crate::names_view::whole_loads(), start, "the projected counts load nothing library-wide");
        assert_eq!(projected.dump(), loaded_counts(&tx).unwrap().dump());
        // alice、bob、hidden_tag，以及无 tag 角色未投影的 ("character","") 键；OC、已合并、未链接和无效的身份不计入。
        // （从核心 v8 起，隐藏 tag 的空键链接已经存不进去：`term_links` 的键是 1–256 个字符。）
        let counts: serde_json::Value = serde_json::from_str(&projected.dump()).unwrap();
        assert_eq!(counts["characters"]["total"], 4, "{counts}");
        // 中文名是 `hidden` 的用户命名，现在在它自己的 tag 下。
        assert_eq!(
            (&counts["characters"]["zh"], &counts["characters"]["readable"]),
            (&1.into(), &2.into()),
            "{counts}"
        );
        assert_eq!(counts["skins"]["total"], 2, "{counts}");
        assert_eq!(names(&tx, false).unwrap().get("counts").unwrap().dump(), projected.dump());
        tx.commit().unwrap();
        // 绕过核心提交的写入让投影落后：计数重新计算。
        c.execute("UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='names_version'", []).unwrap();
        assert!(!crate::legacy_names::projection_current(&c).unwrap());
        let start = crate::names_view::whole_loads();
        assert_eq!(names(&c, false).unwrap().get("counts").unwrap().dump(), projected.dump());
        assert_eq!(crate::names_view::whole_loads() - start, 1);
        drop(c);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn about_reports_the_components_the_workers_would_use() {
        let root = std::env::temp_dir().join(format!(
            "illustash-about-{}-{}",
            std::process::id(),
            crate::store::new_key().unwrap()
        ));
        std::fs::create_dir_all(root.join("tags")).unwrap();
        let context = LibraryContext {
            examples: None,
            db: root.join("library.sqlite"),
            generation: String::new(),
            library: None,
            tags: root.join("tags/tags_v3.csv"),
            source: None,
            token: None,
            read_only: None,
        };
        let libraries = |context: &LibraryContext| -> serde_json::Value {
            serde_json::from_str(&about(context).get("libraries").unwrap().dump()).unwrap()
        };
        let keys: std::collections::BTreeSet<String> =
            libraries(&context).as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["ONNX Runtime", "识别组件"].map(String::from).into(), "no Python-era Pillow/NumPy entries");
        // ONNX Runtime 在 worker 查找的地方找到：在会话的资源根目录下，除非 ILLUSTASH_ORT_DYLIB 指定了一个（Windows
        // 验收运行就是这样；这时只算那个文件）。
        let runtime = crate::app_resources::ort_runtime(&root);
        if std::env::var_os("ILLUSTASH_ORT_DYLIB").is_none() {
            assert_eq!(libraries(&context)["ONNX Runtime"], serde_json::Value::Null);
            std::fs::create_dir_all(runtime.parent().unwrap()).unwrap();
            std::fs::write(&runtime, b"synthetic").unwrap();
        }
        assert_eq!(libraries(&context)["ONNX Runtime"] == "已安装", runtime.is_file());
        std::fs::remove_dir_all(root).unwrap();
    }
}
