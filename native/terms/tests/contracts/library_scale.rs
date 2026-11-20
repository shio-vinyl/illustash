//! 大图库的同步、名字与浏览基准。只在显式运行 ignored release 测试时生成图库。
#[path = "library_scale/browse.rs"]
mod browse;

use super::*;
use illustash_terms::{app_create, schema, store, Kind};
use rusqlite::params;

const NEW: usize = 8;
const STAMP: &str = "2026-01-01 00:00:00";
const BINARY: &str = env!("CARGO_BIN_EXE_illustash-terms");

fn cli(root: &Path, args: &[&str], path: &Path, input: Option<&Value>) -> Value {
    use std::io::Write;
    let mut child = isolated_command(root, Path::new(BINARY))
        .args(args)
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(value) = input {
        child.stdin.take().unwrap().write_all(value.to_string().as_bytes()).unwrap();
    }
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success(), "{args:?}");
    serde_json::from_slice(&result.stdout).unwrap()
}

fn add_photos(photos: &Path, start: usize, end: usize) {
    let mut c = Connection::open(photos.join("database/Photos.sqlite")).unwrap();
    let tx = c.transaction().unwrap();
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS ZASSET(Z_PK INTEGER PRIMARY KEY,ZUUID TEXT,ZKIND INTEGER,
        ZTRASHEDSTATE INTEGER,ZWIDTH INTEGER,ZHEIGHT INTEGER,ZADDEDDATE REAL,ZHIDDEN INTEGER,ZDIRECTORY TEXT,
        ZFILENAME TEXT,ZADDITIONALATTRIBUTES INTEGER);
        CREATE TABLE IF NOT EXISTS ZADDITIONALASSETATTRIBUTES(Z_PK INTEGER PRIMARY KEY,ZIMPORTEDBYBUNDLEIDENTIFIER TEXT,
        ZORIGINALSTABLEHASH TEXT,ZORIGINALFILESIZE INTEGER);",
    )
    .unwrap();
    tx.execute(
        "WITH RECURSIVE n(i) AS (VALUES(?1) UNION ALL SELECT i+1 FROM n WHERE i<?2)
        INSERT INTO ZASSET SELECT i,printf('IMG-%07d',i),0,0,120,150,700000000+i,0,'I',printf('%07d.png',i),i FROM n",
        [start, end],
    )
    .unwrap();
    tx.execute("INSERT INTO ZADDITIONALASSETATTRIBUTES SELECT Z_PK,'net.pxv.iphone',ZUUID,512 FROM ZASSET WHERE Z_PK BETWEEN ? AND ?",
        [start,end]).unwrap();
    tx.commit().unwrap();
}

fn seed(root: &Path, size: usize) -> (PathBuf, PathBuf) {
    let photos = root.join("synthetic.photoslibrary");
    fs::create_dir_all(photos.join("database")).unwrap();
    add_photos(&photos, 1, size);
    let package = root.join("scale.pilib");
    app_create::create_sources(
        &package,
        "合成图库",
        vec![app_create::NewSource::Photos { path: photos.clone() }],
        None,
    )
    .unwrap();
    let source = package.join("source.sqlite");
    let config: Value = serde_json::from_slice(&fs::read(package.join("library.json")).unwrap()).unwrap();
    assert_eq!(cli(root, &["photos-scan", "--db"], &source, Some(&config["sources"][0]))["new"], size);

    let mut db = illustash_terms::open(&package.join("library.sqlite")).unwrap();
    db.execute("ATTACH DATABASE ? AS src", [source.to_str().unwrap()]).unwrap();
    let tx = db.transaction().unwrap();
    schema::begin_writing(&tx).unwrap();
    tx.execute_batch("INSERT INTO assets(uuid,filename,src_path,width,height,added_at,src_kind)
        SELECT uuid,filename,src_path,width,height,added_at,src_kind FROM src.assets;
        INSERT INTO term_assets(id,namespace,key) SELECT z_pk,'library',uuid FROM src.assets;
        CREATE TEMP TABLE chars AS SELECT z_pk AS i,printf('fixture-char-%05d',z_pk) AS key
        FROM src.assets WHERE z_pk<=(SELECT max(1,count(*)/50) FROM src.assets);
        INSERT INTO terms(key,kind,origin) SELECT key,'character','fixture' FROM chars;
        INSERT INTO term_tree(term_id,seq,label_only,mascot,original)
        SELECT t.id,c.i,0,0,0 FROM chars c JOIN terms t USING(key);
        INSERT INTO term_links(term_id,system,key,source) SELECT t.id,'wd',c.key,'fixture' FROM chars c JOIN terms t USING(key);
        INSERT INTO memberships(uuid,entity,status,origin,sources,score,source_count,ccip_only,skey)
        SELECT a.uuid,c.key,'machine','fixture','WD',0.9,1,0,a.uuid FROM src.assets a JOIN chars c
        ON c.i=(a.z_pk-1)%(SELECT count(*) FROM chars)+1;
        INSERT INTO meta(key,value) SELECT 'folder_hash:'||uuid,stable_hash FROM src.assets;
        INSERT INTO actions(id,label,before_json) VALUES (1,'合成收藏','[{\"terms\":\"verdicts\"}]');").unwrap();
    let fav = store::create_term(&tx, Kind::Mark, Some(schema::FAVORITE_KEY), "user").unwrap();
    tx.execute(
        "INSERT INTO term_verdicts(action_id,anchor,asset_id,term_id,verdict,origin)
        SELECT 1,1,z_pk,?,'yes','user' FROM src.assets WHERE z_pk%10=0",
        [fav],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO term_projection(asset_id,term_id,state)
        SELECT asset_id,term_id,'confirmed' FROM term_verdicts WHERE action_id=1",
        [],
    )
    .unwrap();
    // 每图四个通用标签。固定词表与编号使两档的标签选择率可直接比较。
    let tag_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tags/tags_v3.csv");
    let mut tags = csv::Reader::from_path(tag_path).unwrap();
    let names: Vec<_> = tags
        .records()
        .map(|r| r.unwrap())
        .filter(|r| r.get(2) == Some("0"))
        .take(256)
        .map(|r| r[1].to_owned())
        .collect();
    assert_eq!(names.len(), 256);
    tx.execute_batch("CREATE TEMP TABLE vocab(i INTEGER PRIMARY KEY,key TEXT)").unwrap();
    for (i, tag) in names.iter().enumerate() {
        let id = store::create_term(&tx, Kind::Topic, Some(&format!("fixture-topic:{tag}")), "fixture").unwrap();
        store::add_link(&tx, id, "wd", tag, "", "fixture").unwrap();
        tx.execute("INSERT INTO vocab VALUES (?,?)", params![i, tag]).unwrap();
    }
    let rating = store::create_term(&tx, Kind::Rating, Some("fixture-safe"), "fixture").unwrap();
    store::add_link(&tx, rating, "wd_rating", "general", "", "fixture").unwrap();
    tx.execute(
        "INSERT INTO src.labels(uuid,dim,value,conf,source,ts)
        SELECT uuid,'rating','general',0.9,'wd_v3',? FROM src.assets",
        [STAMP],
    )
    .unwrap();
    tx.execute(
        "WITH shifts(k) AS (VALUES(0),(1),(2),(3))
        INSERT INTO src.labels(uuid,dim,value,conf,source,ts)
        SELECT a.uuid,'general',v.key,0.9,'wd_v3',? FROM src.assets a CROSS JOIN shifts
        JOIN vocab v ON v.i=(a.z_pk*17+k*31)%256",
        [STAMP],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO term_evidence(asset_id,term_id,source,method,version,score,metric,fingerprint)
        SELECT a.z_pk,l.term_id,'wd','tagger','fixture',s.conf,'wd_probability',zeroblob(16)
        FROM src.labels s JOIN src.assets a USING(uuid)
        JOIN term_links l ON l.system=CASE s.dim WHEN 'rating' THEN 'wd_rating' ELSE 'wd' END AND l.key=s.value",
        [],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO term_projection(asset_id,term_id,state,score,sources)
        SELECT asset_id,term_id,'machine',score,'wd.tagger' FROM term_evidence",
        [],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO src.labels(uuid,dim,value,conf,source,ts)
        SELECT uuid,'character',entity,0.9,'wd_v3',? FROM memberships",
        [STAMP],
    )
    .unwrap();
    // 与已同步的库一样，保存实际源行的指纹，首轮计时不能变成全库首次识别导入。
    {
        let mut save = tx.prepare("INSERT INTO meta(key,value) VALUES (?,?)").unwrap();
        let mut q = tx.prepare("SELECT uuid FROM src.assets ORDER BY uuid").unwrap();
        for uuid in q.query_map([], |r| r.get::<_, String>(0)).unwrap() {
            let uuid = uuid.unwrap();
            let labels = illustash_platform::source_rows::rows_with(
                &tx,
                "SELECT * FROM src.labels WHERE uuid=? ORDER BY dim,value,conf",
                [&uuid],
            )
            .unwrap();
            let hash = illustash_terms::strict_json::sha256_hex(&serde_json::to_vec(&labels).unwrap());
            save.execute(params![format!("folder_labels:{uuid}"), hash]).unwrap();
        }
    }
    schema::commit_writing(tx).unwrap();
    (package, photos)
}

fn measured<T>(size: usize, name: &str, calls: usize, run: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let result = run();
    println!("BENCH {size} {name} {:.3} ms/op ({calls} 次)", start.elapsed().as_secs_f64() * 1000.0 / calls as f64);
    result
}

#[test]
#[ignore = "显式运行 release 宿主的大图库规模基准"]
fn benchmark() {
    if cfg!(debug_assertions) {
        panic!("需用 release 编译");
    }
    let size: usize = std::env::var("ILLUSTASH_BENCH_IMAGES").unwrap_or_else(|_| "100000".into()).parse().unwrap();
    assert!(size >= 100);
    let (_sandbox, root, _) = materialize(&json!({"input":{}}), "scale");
    let (package, photos) = seed(&root, size);
    let (host, origin, token) =
        measured(size, "open", 1, || launch(&root, &package, "scale", false, Path::new(BINARY)));
    let get = |url: &str, body: Option<&Value>| {
        let (status, v, _) = request_json(&origin, url, body, &token);
        assert_eq!(status, 200, "{url} {v}");
        v
    };
    assert_eq!(get("/api/assets/count", None)["total"], size);
    let page = get("/api/assets?limit=100", None);
    assert_eq!(page["items"].as_array().unwrap().len(), 100);
    let tagged = get("/api/assets/count?tag=1girl", None)["total"].as_u64().unwrap();
    assert!(tagged > 0 && tagged < size as u64);
    for (metric, url) in [
        ("page", "/api/assets?limit=100&offset=2000"),
        ("count", "/api/assets/count"),
        ("detail", "/api/detail?id=IMG-0000001"),
        ("tag_count", "/api/assets/count?tag=1girl"),
    ] {
        get(url, None);
        measured(size, metric, 10, || {
            for _ in 0..10 {
                get(url, None);
            }
        });
    }
    let changed = get("/api/favorite", Some(&json!({"ids":["IMG-0000001"],"confirm":true})));
    let action = changed["undo"]["id"].as_i64().unwrap();
    measured(size, "undo", 1, || get("/api/undo", Some(&json!({"id":action}))));
    assert_eq!(get("/api/detail?id=IMG-0000001&marks=fav", None)["fav"], false);
    drop(host);
    let names = measured(size, "names_project", 1, || {
        cli(&root, &["names-project", "--db"], &package.join("library.sqlite"), None)
    });
    assert!(names["shown"].as_u64().unwrap() > 0, "{names}");
    for (metric, delta) in [("sync_unchanged", 0), ("sync_added", NEW)] {
        if delta > 0 {
            add_photos(&photos, size + 1, size + NEW);
            let c = Connection::open(package.join("source.sqlite")).unwrap();
            // 模拟已有识别结果的新增照片，使同步测到接收和投影，绕开模型推理吞吐。
            for i in size + 1..=size + NEW {
                c.execute(
                    "INSERT INTO labels(uuid,dim,value,conf,source,ts) VALUES (?,'rating','general',0.9,'wd_v3',?)",
                    params![format!("IMG-{i:07}"), STAMP],
                )
                .unwrap();
            }
        }
        let result = measured(size, metric, 1, || cli(&root, &["library-scan", "--library"], &package, None));
        assert_eq!(result, json!({"added":delta,"removed":0,"pending":0}));
        let c = Connection::open(package.join("library.sqlite")).unwrap();
        let counts: (usize, usize, String) = c
            .query_row("SELECT found,imported,status FROM sync_runs ORDER BY id DESC LIMIT 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(counts, (delta, delta, "done".into()));
    }
}
