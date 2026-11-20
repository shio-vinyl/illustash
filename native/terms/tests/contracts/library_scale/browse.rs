//! 在同一份合成图库上比较两个 release 宿主的浏览读接口。
use super::*;

const PAGE: &str = "/api/assets?limit=100&offset=2000";
const PAGE_WITHOUT_TOTAL: &str = "/api/assets?limit=100&offset=2000&count=0";
const COUNT: &str = "/api/assets/count";
const FILTERED_COUNT: &str = "/api/assets/count?tag=1girl";
const FILTERED_PAGE: &str = "/api/assets?tag=1girl&order=oldest&limit=37&offset=5&marks=fav";

#[derive(Debug, PartialEq)]
struct BrowseResults {
    count: Value,
    page: Value,
    page_without_total: Value,
    filtered_count: Value,
    filtered_page: Value,
}

fn browse(root: &Path, package: &Path, binary: &Path, size: usize, version: &str) -> BrowseResults {
    let (_host, origin, token) = launch(root, package, "scale", false, binary);
    let get = |url: &str| {
        let (status, value, _) = request_json(&origin, url, None, &token);
        assert_eq!(status, 200, "{url} {value}");
        value
    };
    let [count, page, page_without_total] = [("count", COUNT), ("page", PAGE), ("page_count_0", PAGE_WITHOUT_TOTAL)]
        .map(|(name, url)| {
            let result = get(url);
            measured(size, &format!("{version}_{name}"), 10, || {
                for _ in 0..10 {
                    get(url);
                }
            });
            result
        });
    BrowseResults {
        count,
        page,
        page_without_total,
        filtered_count: get(FILTERED_COUNT),
        filtered_page: get(FILTERED_PAGE),
    }
}

#[test]
#[ignore = "显式运行两个 release 宿主的大图库浏览基准"]
fn benchmark_browse() {
    if cfg!(debug_assertions) {
        panic!("需用 release 编译");
    }
    let size: usize = std::env::var("ILLUSTASH_BENCH_IMAGES").unwrap_or_else(|_| "100000".into()).parse().unwrap();
    assert!(size >= 2100);
    let baseline = PathBuf::from(std::env::var_os("ILLUSTASH_BENCH_BASELINE").expect("请指定基线 release 宿主"));
    let candidate = std::env::var_os("ILLUSTASH_BENCH_CANDIDATE").map_or_else(|| PathBuf::from(BINARY), PathBuf::from);
    let (_sandbox, root, _) = materialize(&json!({"input":{}}), "scale-browse");
    let (package, _) = seed(&root, size);
    let before = browse(&root, &package, &baseline, size, "baseline");
    let after = browse(&root, &package, &candidate, size, "candidate");
    assert_eq!(before, after);
    assert_eq!(after.count["total"], size);
    assert_eq!(after.page["total"], size);
    assert_eq!(after.page["offset"], 2000);
    assert_eq!(after.page_without_total["total"], Value::Null);
    assert_eq!(after.page["items"], after.page_without_total["items"]);
    let items = after.page["items"].as_array().unwrap();
    assert_eq!(items.len(), 100);
    for (i, item) in items.iter().enumerate() {
        assert_eq!(item["uuid"], format!("IMG-{:07}", size - 2000 - i));
    }
    let filtered_total = after.filtered_count["total"].as_u64().unwrap();
    assert!(filtered_total > 5 && filtered_total < size as u64);
    assert_eq!(after.filtered_page["total"], filtered_total);
    let filtered_items = after.filtered_page["items"].as_array().unwrap();
    assert_eq!(filtered_items.len(), (filtered_total - 5).min(37) as usize);
    // 直接从合成来源的标签行核对筛选结果，收藏每十张一条，和浏览索引无关。
    let source = Connection::open(package.join("source.sqlite")).unwrap();
    let fixture_total: u64 = source
        .query_row("SELECT count(*) FROM labels WHERE dim='general' AND value='1girl'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(filtered_total, fixture_total);
    let mut fixture_page = source
        .prepare(
            "SELECT a.uuid,a.z_pk%10=0 FROM labels l JOIN assets a USING(uuid)
             WHERE l.dim='general' AND l.value='1girl'
             ORDER BY a.added_at ASC,a.uuid LIMIT 37 OFFSET 5",
        )
        .unwrap();
    let expected: Vec<_> = fixture_page
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert_eq!(filtered_items.len(), expected.len());
    for (item, (uuid, favorite)) in filtered_items.iter().zip(expected) {
        assert_eq!(item["uuid"], uuid);
        assert_eq!(item["fav"], favorite);
    }
}
