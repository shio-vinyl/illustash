use super::{json, launch, materialize, request_json, Path};
use illustash_terms::{character_entities, schema, store, Kind};
use rusqlite::params;

#[test]
fn character_together_http_contract() {
    let category = "character_together";
    let (_scratch, root, _) = materialize(&json!({"input": {}}), category);
    let package = root.join("together.pilib");
    illustash_terms::app_create::create_folder(&package, &root.join("folder"), None, None, None).unwrap();
    let mut db = illustash_terms::open(&package.join("library.sqlite")).unwrap();
    let tx = db.transaction().unwrap();
    schema::begin_writing(&tx).unwrap();
    tx.execute_batch(
        "INSERT INTO assets(uuid,filename,added_at) VALUES
         ('safe','safe.png','2026-01-01'),('explicit','explicit.png','2026-01-02'),
         ('questionable','questionable.png','2026-01-03'),
         ('copy','copy.png','2026-01-02'),('excluded','excluded.png','2026-01-03');
         INSERT INTO duplicates VALUES
         ('safe','g',2,0,'2026-01-02',1,0),('copy','g',2,1,'2026-01-02',0,1);
         INSERT INTO excluded VALUES ('excluded','manual','test');",
    )
    .unwrap();
    for (id, label_only, mascot) in [
        ("subject", 0, 0),
        ("z", 0, 0),
        ("a", 0, 0),
        ("b", 0, 0),
        ("c", 0, 0),
        ("d", 0, 0),
        ("e", 0, 0),
        ("f", 0, 0),
        ("label", 1, 0),
        ("mascot", 1, 1),
        ("possible", 0, 0),
    ] {
        character_entities::create(
            &tx,
            &character_entities::New { id, kind: "character", label_only, mascot, ..Default::default() },
            "test",
        )
        .unwrap();
    }
    for (uuid, subject_status, peers) in [
        ("safe", "confirmed", &["z", "a", "b", "c", "d", "e", "f", "label"][..]),
        ("explicit", "machine", &["z"][..]),
        ("questionable", "confirmed", &["z"][..]),
        ("copy", "confirmed", &["a"][..]),
        ("excluded", "confirmed", &["a"][..]),
    ] {
        for (id, status) in std::iter::once(("subject", subject_status))
            .chain(peers.iter().map(|&id| (id, if id == "z" { "confirmed" } else { "machine" })))
        {
            tx.execute(
                "INSERT INTO memberships(uuid,entity,status,origin,sources,source_count,ccip_only,skey)
                 VALUES (?,?,?,'test','WD',1,0,'')",
                params![uuid, id, status],
            )
            .unwrap();
        }
    }
    tx.execute_batch(
        "INSERT INTO weak_claims VALUES ('safe','possible','PROTO',0.9);
         INSERT INTO tag_memberships(uuid,entity,status,origin,sources,source_count,ccip_only,skey)
         VALUES ('safe','mascot','confirmed','test','WD',1,0,'');",
    )
    .unwrap();
    for key in ["explicit", "questionable"] {
        let rating = store::create_term(&tx, Kind::Rating, None, "test").unwrap();
        store::add_link(&tx, rating, "wd_rating", key, "", "test").unwrap();
        let asset = store::library_asset(&tx, key).unwrap().unwrap();
        tx.execute(
            "INSERT INTO term_projection(asset_id,term_id,state) VALUES (?,?,'confirmed')",
            params![asset, rating],
        )
        .unwrap();
    }
    schema::commit_writing(tx).unwrap();

    let (_host, origin, token) =
        launch(&root, &package, category, false, Path::new(env!("CARGO_BIN_EXE_illustash-terms")));
    let request = |query: &str| request_json(&origin, &format!("/api/character/together{query}"), None, &token);
    let get = |query: &str| {
        let (status, body, _) = request(query);
        assert_eq!(status, 200, "{query}: {body}");
        body
    };
    let peer = |id, count| json!({"id":id,"count":count});
    assert_eq!(
        get("?id=subject"),
        json!([peer("z", 3), peer("a", 1), peer("b", 1), peer("c", 1), peer("d", 1), peer("e", 1)])
    );
    assert_eq!(get("?id=subject&nsfw=hide&limit=2"), json!([peer("a", 1), peer("b", 1)]));
    assert_eq!(get("?id=subject&nsfw=only"), json!([peer("z", 2)]));
    assert_eq!(
        get("?id=subject&limit=20"),
        json!([peer("z", 3), peer("a", 1), peer("b", 1), peer("c", 1), peer("d", 1), peer("e", 1), peer("f", 1)])
    );
    for id in ["missing", "possible", "label", "mascot"] {
        assert_eq!(get(&format!("?id={id}")), json!([]));
    }
    for query in ["", "?id=subject&nsfw=bad", "?id=subject&limit=0", "?id=subject&limit=-1", "?id=subject&limit=bad"] {
        assert_eq!(request(query).0, 400, "{query}");
    }
}
