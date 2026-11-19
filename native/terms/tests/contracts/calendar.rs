use super::{json, launch, materialize, request, request_json, Path, Value};
use illustash_terms::{character_entities, schema, store, Kind};
use rusqlite::params;

#[test]
fn calendar_and_home_filter_http_contract() {
    let category = "calendar";
    let (_scratch, root, _) = materialize(&json!({"input": {}}), category);
    let package = root.join("calendar.pilib");
    illustash_terms::app_create::create_folder(&package, &root.join("folder"), None, None, None).unwrap();
    let mut db = illustash_terms::open(&package.join("library.sqlite")).unwrap();
    let tx = db.transaction().unwrap();
    schema::begin_writing(&tx).unwrap();
    tx.execute_batch(
        "INSERT INTO assets(uuid,filename,width,height,added_at) VALUES
         ('dec','keep-dec.png',80,120,'2023-12-31 23:59:59'),
         ('jan','other-jan.png',80,120,'2024-01-31 23:59:59'),
         ('first','other-first.png',80,120,'2024-02-01 00:00:00'),
         ('a','keep-a.png',80,120,'2024-02-29 04:00:00'),
         ('b','keep-b.png',80,120,'2024-02-29 04:00:00'),
         ('c','keep-c.png',80,120,'2024-02-29 12:00:00'),
         ('d','keep-d.png',80,120,'2024-02-29 18:00:00'),
         ('e','keep-e.png',80,120,'2024-02-29 23:59:59'),
         ('copy','keep-copy.png',80,120,'2024-02-29 23:59:59'),
         ('excluded','keep-excluded.png',80,120,'2024-03-01 00:00:00'),
         ('april','keep-april.png',80,120,'2024-04-01 00:00:00'),
         ('undated','keep-undated.png',80,120,NULL);
         INSERT INTO duplicates VALUES
          ('a','g',2,0,'2024-02-29',1,0),('copy','g',2,1,'2024-02-29',0,1);
         INSERT INTO excluded VALUES ('excluded','manual','test');",
    )
    .unwrap();
    for (id, image) in [("present", "a"), ("filtered", "d")] {
        character_entities::create(
            &tx,
            &character_entities::New { id, kind: "character", ..Default::default() },
            "test",
        )
        .unwrap();
        tx.execute(
            "INSERT INTO memberships(uuid,entity,status,origin,sources,score,source_count,ccip_only,skey)
             VALUES (?,?,'machine','fixture','WD',0.9,1,0,?)",
            params![image, id, image],
        )
        .unwrap();
    }
    let rating = store::create_term(&tx, Kind::Rating, None, "test").unwrap();
    store::add_link(&tx, rating, "wd_rating", "explicit", "", "test").unwrap();
    let e = store::library_asset(&tx, "e").unwrap().unwrap();
    tx.execute("INSERT INTO term_projection(asset_id,term_id,state) VALUES (?,?,'confirmed')", params![e, rating])
        .unwrap();
    schema::commit_writing(tx).unwrap();
    let (_host, origin, token) =
        launch(&root, &package, category, false, Path::new(env!("CARGO_BIN_EXE_illustash-terms")));
    let get = |url: &str| {
        let (status, body, _) = request_json(&origin, url, None, &token);
        assert_eq!(status, 200, "{url}: {body}");
        body
    };
    let uuids = |items: &Value| items.as_array().unwrap().iter().map(|item| item["uuid"].clone()).collect::<Vec<_>>();

    let month = get("/api/calendar?month=2024-02");
    assert_eq!(month["total"], 6);
    assert_eq!(month["prev"], "2024-01");
    assert_eq!(month["next"], "2024-04");
    let days = month["days"].as_array().unwrap();
    assert_eq!(days.len(), 2);
    assert_eq!(days[0]["day"], "2024-02-01");
    assert_eq!(days[0]["count"], 1);
    assert_eq!(days[1]["day"], "2024-02-29");
    assert_eq!(days[1]["count"], 5);
    assert_eq!(uuids(&days[1]["samples"]), json!(["e", "d", "c", "a"]).as_array().unwrap().clone());
    assert_eq!(days[1].get("color"), Some(&Value::Null));
    assert!(days[1]["samples"][0].get("fav").is_none());
    assert_eq!(get("/api/calendar?month=2024-03"), json!({"days":[],"total":0,"prev":"2024-02","next":"2024-04"}));
    assert_eq!(
        get("/api/calendar?month=2024-02&nsfw=only")["days"][0]["samples"],
        get("/api/assets?nsfw=only&day=2024-02-29")["items"]
    );

    let (status, _, _) = request_json(&origin, "/api/tag", Some(&json!({"ids":["d"],"tag":"skip"})), &token);
    assert_eq!(status, 200);
    let (status, _, _) = request_json(&origin, "/api/favorite", Some(&json!({"ids":["a","d","jan"]})), &token);
    assert_eq!(status, 200);
    let filters = "has=%5B%22fav%22%5D&not=%5B%22utag%3Askip%22%5D&q=keep&from=2024-02-01&to=2024-02-29&marks=fav";
    let grid = get(&format!("/api/assets?{filters}"));
    assert_eq!(grid["total"], 1);
    let calendar = get(&format!("/api/calendar?month=2024-02&{filters}"));
    assert_eq!(
        calendar,
        json!({"days":[{"day":"2024-02-29","count":1,"samples":grid["items"],"color":null}],
               "total":1,"prev":null,"next":null})
    );
    let home = get(&format!("/api/home?days=1&before=2024-03-01&{filters}"));
    assert_eq!(home["next"], "2024-02-29");
    let day = &home["days"][0];
    assert_eq!(day["count"], 1);
    assert_eq!(day["items"], grid["items"]);
    assert_eq!(day["items"][0]["fav"], true);
    assert_eq!(day["people"], 1);
    assert_eq!(day["characters"][0]["id"], "present");
    assert_eq!(day["characters"][0]["count"], 1);
    assert!(get(&format!("/api/home?before=2024-02-29&{filters}"))["days"].as_array().unwrap().is_empty());

    let old_first = "day=2024-02-29&utag=skip&order=oldest";
    let filtered = get(&format!("/api/assets?{old_first}"));
    assert_eq!(filtered["total"], 1);
    assert_eq!(get(&format!("/api/home?{old_first}"))["days"][0]["items"], filtered["items"]);
    assert_eq!(get(&format!("/api/calendar?month=2024-02&{old_first}"))["days"][0]["samples"], filtered["items"]);
    let date_order = get("/api/calendar?month=2024-02&order=oldest");
    assert_eq!(uuids(&date_order["days"][1]["samples"]), json!(["a", "b", "c", "d"]).as_array().unwrap().clone());
    let searched = get("/api/calendar?month=2024-02&q=keep");
    assert_eq!(searched["prev"], "2023-12");
    assert_eq!(searched["next"], "2024-04");
    assert_eq!(searched["total"], 5);
    for query in ["", "?month=2024-2", "?month=2024-00", "?month=2024-13", "?month=2024-02-29", "?month=0000-01"] {
        assert_eq!(request(&origin, &format!("/api/calendar{query}"), None, &token).0, 400);
    }
    for query in ["has=bad", "from=2024-02-30", "to=2024-01-30&from=2024-02-01", "status=bad", "nsfw=bad"] {
        let result = request(&origin, &format!("/api/assets?{query}"), None, &token);
        assert_eq!(result.0, 400);
        for path in ["/api/calendar?month=2024-02", "/api/home?days=2"] {
            let peer = request(&origin, &format!("{path}&{query}"), None, &token);
            assert_eq!((peer.0, peer.1), (result.0, result.1.clone()));
        }
    }
}
