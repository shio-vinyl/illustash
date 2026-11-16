use super::*;
use crate::{
    schema::Kind,
    store::{Verdict, VerdictTarget},
};

#[test]
fn publishes_only_the_current_tag_batch_and_replays_identical_input() {
    let mut db = Connection::open_in_memory().unwrap();
    crate::configure(&db).unwrap();
    crate::migrate(&mut db).unwrap();
    let term = store::create_term(&db, Kind::Concept, Some("label"), "user").unwrap();
    store::create_term(&db, Kind::Concept, Some("other"), "user").unwrap();
    let a = store::register_asset(&db, "library", "a", None).unwrap();
    store::register_asset(&db, "library", "b", None).unwrap();
    let data = Map::new();
    let a_rows = [("a".to_owned(), 0.8)];
    let b_rows = [("b".to_owned(), 0.9)];
    let first = Batch { library: "fixture", term: "label", version: "one", input: "first", data: &data, rows: &a_rows };
    let other = Batch { term: "other", rows: &b_rows, ..first };
    let empty = Batch { version: "two", input: "changed", rows: &[], ..first };
    let apply = |db: &mut Connection, batch| {
        let tx = db.transaction().unwrap();
        let changed = publish(&tx, batch)?;
        tx.commit()?;
        Ok::<_, Error>(changed)
    };
    let n = |db: &Connection, sql| db.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();
    assert!(apply(&mut db, &first).unwrap());
    assert!(!apply(&mut db, &first).unwrap());
    assert!(apply(&mut db, &other).unwrap());
    assert_eq!(n(&db, "SELECT count(*) FROM actions"), 0);
    assert_eq!(n(&db, "SELECT count(*) FROM term_projection WHERE state='maybe'"), 2);
    let different = Batch { rows: &b_rows, ..first };
    assert!(matches!(apply(&mut db, &different), Err(Error::Conflict(_))));
    assert_eq!(n(&db, "SELECT count(*) FROM term_evidence_sets"), 2);

    store::judge(&mut db, &[VerdictTarget::Image { asset: a, term }], Verdict::Clear, "user", "", "clear").unwrap();
    assert_eq!(n(&db, "SELECT count(*) FROM term_projection WHERE state='maybe'"), 1);
    assert!(apply(&mut db, &empty).unwrap());
    assert_eq!(n(&db, "SELECT count(*) FROM term_current_evidence WHERE source='example'"), 1);
    assert_eq!(n(&db, "SELECT count(*) FROM term_projection WHERE state='maybe'"), 1);
    store::undo_last(&mut db).unwrap();
    assert!(apply(&mut db, &first).unwrap());
    assert_eq!(n(&db, "SELECT count(*) FROM term_projection WHERE state='maybe'"), 2);
    assert_eq!(n(&db, "SELECT count(*) FROM term_evidence WHERE source='example'"), 2);

    let missing = [("gone".to_owned(), 0.8)];
    let failed = Batch { input: "error", rows: &missing, ..first };
    let before = n(&db, "SELECT count(*) FROM term_packages");
    assert!(apply(&mut db, &failed).is_err());
    assert_eq!(n(&db, "SELECT count(*) FROM term_packages"), before);
}
