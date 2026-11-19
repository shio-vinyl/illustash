use super::*;
use image::{Rgb, RgbImage};
use rusqlite::params;
use serde_json::Value;

const CACHE: &str = ".illustash-colors-v1.sqlite";

fn wait_for(mut predicate: impl FnMut() -> bool) {
    let deadline = wait::deadline();
    while !predicate() {
        assert!(Instant::now() < deadline, "图片颜色补算没有完成");
        thread::sleep(Duration::from_millis(20));
    }
}

fn color_count(package: &Path) -> usize {
    Connection::open_with_flags(package.join(CACHE), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .and_then(|db| db.query_row("SELECT count(*) FROM colors", [], |row| row.get(0)))
        .unwrap_or(0)
}

fn raw_get(origin: &str, token: &str, url: &str) -> String {
    let (status, raw, _) = request(origin, url, None, token);
    assert_eq!(status, 200, "{url}: {raw}");
    raw
}

fn get(origin: &str, token: &str, url: &str) -> Value {
    serde_json::from_str(&raw_get(origin, token, url)).unwrap()
}

#[test]
fn image_colors_http_contract() {
    let category = "image-colors";
    let (_scratch, root, package) = materialize(&load("http_api"), category);
    migrate_fixture_terms(&package, category);
    let mut db = illustash_terms::open(&package.join("library.sqlite")).unwrap();
    let assets = [
        ("color-muted", [125, 125, 125], "20"),
        ("color-yellow", [255, 255, 0], "19"),
        ("color-red-a", [235, 0, 0], "18"),
        ("color-red-b", [235, 0, 0], "18"),
        ("color-dark", [110, 0, 0], "17"),
    ];
    for (id, rgb, date) in assets {
        let file = root.join("folder").join(format!("{id}.png"));
        RgbImage::from_pixel(18, 24, Rgb(rgb)).save(&file).unwrap();
        db.execute(
            "INSERT INTO assets(uuid,filename,src_path,width,height,added_at,src_kind)
             VALUES (?1,?1,?2,18,24,?3,'orig')",
            params![id, file.to_str().unwrap(), format!("2026-11-{date}")],
        )
        .unwrap();
    }
    let tx = db.transaction().unwrap();
    illustash_terms::schema::begin_writing(&tx).unwrap();
    tx.execute_batch(
        "INSERT INTO assets(uuid,filename,added_at) VALUES ('color-unavailable','color-unavailable','2026-11-21');
         INSERT INTO terms(key,kind,origin) VALUES ('tone-artist','artist','test'),('tone-character','character','test'),
                                                    ('tone-work','work','test'),('tone-child','work','test');
         INSERT INTO term_tree(term_id,seq,work_id,parent_id)
              SELECT id,id,CASE WHEN key='tone-character' THEN 'tone-child' END,
                     CASE WHEN key='tone-child' THEN 'tone-work' END
              FROM terms WHERE key IN ('tone-character','tone-work','tone-child');
         INSERT INTO memberships(uuid,entity,status,origin,sources,score,source_count,ccip_only,skey)
              VALUES ('color-red-a','tone-character','machine','fixture','WD',0.9,1,0,'color-red-a');
         INSERT INTO term_assets(namespace,key) VALUES ('library','color-red-a');
         INSERT INTO term_evidence(asset_id,term_id,source,method,version,fingerprint)
              SELECT a.id,t.id,'synthetic','pixel_hash','fixture',zeroblob(16)
              FROM terms t JOIN term_assets a ON a.key='color-red-a' WHERE t.key='tone-artist';",
    )
    .unwrap();
    let red = illustash_terms::store::library_asset(&tx, "color-red-a").unwrap().unwrap();
    illustash_terms::store::project(&tx, &[red]).unwrap();
    illustash_terms::schema::commit_writing(tx).unwrap();

    let binary = Path::new(env!("CARGO_BIN_EXE_illustash-terms"));
    let (_host, origin, token) = launch(&root, &package, category, false, binary);
    wait_for(|| color_count(&package) == assets.len());
    let get = |url: &str| get(&origin, &token, url);
    let raw = |url: &str| raw_get(&origin, &token, url);

    let (status, gallery, _) = request_json(
        &origin,
        "/api/galleries",
        Some(&json!({"action":"create","name":"合成颜色","params":{"q":"color-"}})),
        &token,
    );
    assert_eq!(status, 200, "{gallery}");
    let gallery_id = gallery["gallery"]["id"].as_i64().unwrap();
    let (status, result, _) = request_json(&origin, "/api/favorite", Some(&json!({"ids":["color-red-a"]})), &token);
    assert_eq!(status, 200, "{result}");

    let ordered = "/api/assets?from=2026-11-01&order=color&nsfw=hide";
    let expected = ["color-dark", "color-red-a", "color-red-b", "color-yellow", "color-muted", "color-unavailable"];
    let page = get(ordered);
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.iter().map(|row| row["uuid"].as_str().unwrap()).collect::<Vec<_>>(), expected);
    assert_eq!(page["total"], expected.len());
    assert_eq!(get(&format!("{ordered}&offset=1&limit=2"))["items"], json!(&items[1..3]));
    assert_eq!(get("/api/ids?from=2026-11-01&order=color"), json!(expected));
    assert_eq!(get(&format!("{ordered}&marks=color"))["items"][5]["color"], Value::Null);
    assert!(items.iter().all(|row| row.get("color").is_none()));

    let covers = format!("/api/galleries/covers?ids=%5B{gallery_id}%5D&marks=color");
    assert_eq!(get(&covers)[0], json!({"id":gallery_id,"cover":"color-unavailable","color":null}));
    let (status, result, _) = request_json(
        &origin,
        "/api/galleries",
        Some(&json!({"action":"update","id":gallery_id,"cover":"color-red-a"})),
        &token,
    );
    assert_eq!(status, 200, "{result}");
    assert_eq!(get(&covers)[0], json!({"id":gallery_id,"cover":"color-red-a","color":"#eb0000"}));

    let character = get("/api/characters?id=tone-character&summary=1&marks=color");
    assert_eq!(character[0]["cover"], "color-red-a");
    assert_eq!(character[0]["color"], "#eb0000");
    let artists = get("/api/artist-directory?marks=color");
    let artist = artists["items"].as_array().unwrap().iter().find(|a| a["id"] == "tone-artist").unwrap();
    assert_eq!(artist["cover"], "color-red-a");
    assert_eq!(artist["color"], "#eb0000");
    assert_eq!(request(&origin, "/api/works?marks=color&nsfw=bad", None, &token).0, 400);
    assert_eq!(raw("/api/works?nsfw=bad"), raw("/api/works"));
    let works = get("/api/works?marks=color");
    for id in ["tone-work", "tone-child"] {
        let row = works.as_array().unwrap().iter().find(|w| w["id"] == id).unwrap();
        assert_eq!(row["covers"], json!([{"uuid":"color-red-a","color":"#eb0000"}]));
    }
    assert!(get("/api/works").as_array().unwrap().iter().all(|row| row.get("covers").is_none()));

    let detail = "/api/detail?id=color-red-a&marks=fav";
    let both = format!("{detail},color");
    let plain_detail = raw(detail);
    let marked_detail = raw(&both);
    assert!(marked_detail.starts_with(plain_detail.strip_suffix('}').unwrap()));
    assert!(marked_detail.ends_with(r##", "color": "#eb0000", "palette": [{"color": "#eb0000", "share": 1.0}]}"##));
    let home = get("/api/home?days=5&marks=fav,color");
    let red_day = home["days"].as_array().unwrap().iter().find(|day| day["day"] == "2026-11-18").unwrap();
    assert_eq!(red_day["items"][0]["fav"], true);
    assert_eq!(red_day["items"][0]["color"], "#eb0000");

    // 没有导入事件，后台补算没有这张图。真实 /image 缺缓存时生成 JPEG，并立即发布颜色。
    let fresh = root.join("folder/new-thumbnail.png");
    RgbImage::from_pixel(18, 24, Rgb([0, 155, 0])).save(&fresh).unwrap();
    db.execute(
        "INSERT INTO assets(uuid,filename,src_path,width,height,added_at,src_kind)
         VALUES ('fresh-thumb','fresh-thumb.png',?1,18,24,'2026-11-22','orig')",
        [fresh.to_str().unwrap()],
    )
    .unwrap();
    let fresh_detail = "/api/detail?id=fresh-thumb&marks=color";
    assert_eq!(get(fresh_detail)["color"], Value::Null);
    let jpeg = ureq::get(&format!("{origin}/image?id=fresh-thumb&size=thumb"))
        .set("Cookie", &entry::cookie(&origin))
        .call()
        .unwrap();
    assert_eq!(jpeg.header("Content-Type"), Some("image/jpeg"));
    let mut magic = [0; 2];
    jpeg.into_reader().read_exact(&mut magic).unwrap();
    assert_eq!(magic, [0xff, 0xd8]);
    assert_eq!(get(fresh_detail)["color"], "#009b00");

    let (status, cleared, _) = request_json(&origin, "/api/cache", Some(&json!({"action":"clear"})), &token);
    assert_eq!(status, 200, "{cleared}");
    assert_eq!(cleared["removed"], 1);
    fs::remove_dir(package.join("thumbnails")).unwrap();
    assert!(package.join(CACHE).is_file());
    assert_eq!(get(fresh_detail)["color"], "#009b00");

    // 同步用已发布的内容哈希和路径做一致性检查。源文件替换后，恢复任务会重新补算。
    let old = root.join("folder/color-red-a.png");
    let new = root.join("folder/color-new.png");
    RgbImage::from_pixel(18, 24, Rgb([0, 111, 255])).save(&new).unwrap();
    db.execute("UPDATE assets SET src_path=? WHERE uuid='color-red-a'", [new.to_str().unwrap()]).unwrap();
    fs::remove_file(old).unwrap();
    assert_eq!(get(&both)["color"], Value::Null);
    db.execute("INSERT INTO backend_effects(kind) VALUES ('imported')", []).unwrap();
    wait_for(|| get(&both)["color"] == "#006fff");

    // 关闭原图后，当前图库快照里的既有颜色仍可读。
    fs::remove_file(new).unwrap();
    assert_eq!(get(&both)["color"], "#006fff");
}

#[cfg(unix)]
fn jpeg_fixture(sample: u8) -> Vec<u8> {
    let image = RgbImage::from_fn(400, 300, |x, y| {
        let panel = (x / 100 + 2 * (y / 100)) as u8;
        let stripes = ((x + y) % 53) as u8;
        Rgb([sample.wrapping_mul(panel + 1), 230 - 20 * panel, panel * 28 + stripes])
    });
    let mut enc = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
    enc.set_fastest_defaults();
    enc.set_size(400, 300);
    enc.set_quality(84.0);
    let mut enc = enc.start_compress(Vec::new()).unwrap();
    enc.write_scanlines(image.as_raw()).unwrap();
    enc.finish().unwrap()
}

#[cfg(unix)]
#[test]
#[ignore = "显式准备补算计时夹具"]
fn prepare_color_backfill_fixture() {
    let root = benchmark_root();
    assert!(root.file_name().is_some_and(|name| name.to_string_lossy().ends_with(".noindex")));
    fs::create_dir(&root).unwrap();
    let dir = materialize_at(&json!({"input":{}}), &root);
    let package = dir.with_file_name("color-bench.pilib");
    illustash_terms::app_create::create_folder(&package, &root.join("folder"), None, None, None).unwrap();
    fs::create_dir_all(package.join("thumbnails")).unwrap();
    let mut db = illustash_terms::open(&package.join("library.sqlite")).unwrap();
    let trans = db.transaction().unwrap();
    let samples: Vec<_> = (0..=127).map(jpeg_fixture).collect();
    for i in 0..50_000 {
        let id = format!("color-{i:05}");
        let path = root.join("folder").join(format!("{id}.jpg"));
        fs::write(&path, &samples[i % samples.len()]).unwrap();
        trans
            .execute(
                "INSERT INTO assets(uuid,filename,src_path,width,height,added_at,src_kind)
                 VALUES (?1,?1,?2,400,300,'2026-11-19','orig')",
                params![id, path.to_str().unwrap()],
            )
            .unwrap();
    }
    trans.commit().unwrap();
}

#[cfg(unix)]
#[test]
#[ignore = "release 计时，使用 collab_run.py timing"]
fn benchmark_color_backfill() {
    let root = benchmark_root();
    let package = root.join("color-bench.pilib");
    assert!(!package.join(CACHE).exists());
    assert_eq!(fs::read_dir(package.join("thumbnails")).unwrap().count(), 0);
    let binary = Path::new(env!("CARGO_BIN_EXE_illustash-terms"));
    let start = Instant::now();
    let (_host, origin, token) = launch(&root, &package, "scale", false, binary);
    let deadline = start + wait::limit();
    let completed = loop {
        let count = color_count(&package);
        if count == 50_000 {
            break start.elapsed();
        }
        assert!(Instant::now() < deadline, "仅有 {count} 张算出了颜色");
        thread::sleep(Duration::from_millis(100));
    };
    let page = get(&origin, &token, "/api/assets?order=color&marks=color&limit=100");
    assert!(page["items"].as_array().unwrap().iter().all(|row| row["color"].is_string()));
    println!("颜色补算：50,000 张 JPEG，从宿主启动到全部记录落盘 {:.3} s", completed.as_secs_f64());
}
