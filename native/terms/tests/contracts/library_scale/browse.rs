//! 在同一份合成图库上比较两个 release 宿主的浏览读接口。
use super::*;

const PAGE: &str = "/api/assets?limit=100&offset=2000";
const PAGE_WITHOUT_TOTAL: &str = "/api/assets?limit=100&offset=2000&count=0";
const COUNT: &str = "/api/assets/count";
const FILTERED_COUNT: &str = "/api/assets/count?tag=1girl";
const FILTERED_PAGE: &str = "/api/assets?tag=1girl&order=oldest&limit=37&offset=5&marks=fav";

fn browse(root: &Path, package: &Path, binary: &Path, size: usize, version: &str) -> Vec<Value> {
    let (_host, origin, token) = launch(root, package, "scale", false, binary);
    let get = |url: &str| {
        let (status, value, _) = request_json(&origin, url, None, &token);
        assert_eq!(status, 200, "{url} {value}");
        value
    };
    let mut results = Vec::new();
    for (name, url) in [("count", COUNT), ("page", PAGE), ("page_count_0", PAGE_WITHOUT_TOTAL)] {
        results.push(get(url));
        measured(size, &format!("{version}_{name}"), 10, || {
            for _ in 0..10 {
                get(url);
            }
        });
    }
    results.extend([get(FILTERED_COUNT), get(FILTERED_PAGE)]);
    results
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
    let (_sandbox, root, _) = materialize(&json!({"input":{}}), "scale-browse");
    let (package, _) = seed(&root, size);
    let before = browse(&root, &package, &baseline, size, "baseline");
    let after = browse(&root, &package, Path::new(BINARY), size, "candidate");
    assert_eq!(before, after);
    assert_eq!(after[0]["total"], size);
    assert_eq!(after[1]["total"], size);
    assert_eq!(after[1]["offset"], 2000);
    assert_eq!(after[2]["total"], Value::Null);
    assert_eq!(after[1]["items"], after[2]["items"]);
    let items = after[1]["items"].as_array().unwrap();
    assert_eq!(items.len(), 100);
    for (i, item) in items.iter().enumerate() {
        assert_eq!(item["uuid"], format!("IMG-{:07}", size - 2000 - i));
    }
    assert!(after[3]["total"].as_u64().is_some_and(|n| n > 42 && n < size as u64));
    assert_eq!(after[4]["total"], after[3]["total"]);
    assert_eq!(after[4]["items"].as_array().unwrap().len(), 37);
}
