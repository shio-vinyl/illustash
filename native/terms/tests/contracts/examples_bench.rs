//! #449 的 release 宿主计时，跟随固定契约基准的 loopback HTTP 口径。
use super::*;
use rusqlite::params;
use std::io::{BufWriter, Write};

const IMAGES: usize = 50_000;
const DIM: usize = 768;

fn generate(root: &Path) -> PathBuf {
    let package = root.join("search.pilib");
    illustash_terms::app_create::create_folder(&package, &root.join("folder"), None, None, None).unwrap();
    let mut db = Connection::open(package.join("library.sqlite")).unwrap();
    let mut source = Connection::open(package.join("source.sqlite")).unwrap();
    let app = db.transaction().unwrap();
    let input = source.transaction().unwrap();
    let mut insert =
        app.prepare("INSERT INTO assets(uuid,filename,width,height,added_at) VALUES (?,?,120,150,?)").unwrap();
    let mut add_source =
        input.prepare("INSERT INTO assets(uuid,in_scope,trashed,src_path) VALUES (?,1,0,'synthetic')").unwrap();
    let mut add_tag =
        input.prepare("INSERT INTO labels(uuid,dim,value,source,conf) VALUES (?,'general',?,'wd_v3',?)").unwrap();
    let vocabulary: Vec<_> = (0..8000).map(|i| format!("tag{i:04}")).collect();
    let features = package.join("features");
    fs::create_dir_all(&features).unwrap();
    let mut manifest = BufWriter::new(fs::File::create(features.join("full-manifest.csv")).unwrap());
    writeln!(manifest, "uuid").unwrap();
    let header = format!("{{'descr': '<f4', 'fortran_order': False, 'shape': ({IMAGES}, {DIM}), }}\n");
    let mut npy = BufWriter::new(fs::File::create(features.join("full-features.npy")).unwrap());
    npy.write_all(b"\x93NUMPY\x01\x00").unwrap();
    npy.write_all(&(header.len() as u16).to_le_bytes()).unwrap();
    npy.write_all(header.as_bytes()).unwrap();
    for i in 0..IMAGES {
        let id = format!("IMAGE-{i:05}");
        let group = i % 40;
        let date = if i % 3 == 0 { "2026-04-01" } else { "2026-05-01" };
        let name = if i % 7 == 0 { "cluster-a" } else { "cluster-b" };
        insert.execute(params![id, name, date]).unwrap();
        add_source.execute([&id]).unwrap();
        for tag in 0..200 {
            let (column, probability) = if tag < 15 {
                (group * 16 + tag, 0.9 - tag as f64 * 0.025)
            } else {
                (640 + (tag * 13 + i * 43) % 7360, 0.08 / (1.0 + tag as f64 / 200.0))
            };
            add_tag.execute(params![id, vocabulary[column], probability]).unwrap();
        }
        writeln!(manifest, "{id}").unwrap();
        let mut vector = [0; DIM * 4];
        vector[group * 4..group * 4 + 4].copy_from_slice(&1.0f32.to_le_bytes());
        let variation = 40 + (17 * i % (DIM - 40));
        vector[variation * 4..variation * 4 + 4].copy_from_slice(&0.2f32.to_le_bytes());
        npy.write_all(&vector).unwrap();
    }
    manifest.flush().unwrap();
    npy.flush().unwrap();
    drop((insert, add_source, add_tag));
    app.commit().unwrap();
    input.commit().unwrap();
    package
}

#[cfg(unix)]
fn rss(pid: u32) -> f64 {
    let out = Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().unwrap();
    working_set(out) / 1024.0
}

#[cfg(windows)]
fn rss(pid: u32) -> f64 {
    let expression = format!("(Get-Process -Id {pid}).WorkingSet64");
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &expression])
        .output()
        .unwrap();
    working_set(out) / 1_048_576.0
}

fn working_set(output: std::process::Output) -> f64 {
    assert!(output.status.success(), "无法读取宿主常驻内存");
    String::from_utf8(output.stdout).unwrap().trim().parse().unwrap()
}

fn search(origin: &str, token: &str, uri: &str) {
    let (status, result, _) = request_json(origin, uri, None, token);
    assert_eq!(status, 200, "{uri}: {result}");
    assert!(!result.is_null());
}

#[test]
#[ignore = "只在空闲验收机上运行 release 宿主合成图库计时"]
fn benchmark_example_search_http() {
    if cfg!(debug_assertions) {
        panic!("bench requires a release binary");
    }
    let binary = Path::new(env!("CARGO_BIN_EXE_illustash-terms"));
    let (_scratch, root, _) = materialize(&load("http_api"), "example-search-bench");
    let package = generate(&root);
    let one = "%5B%22IMAGE-00000%22%5D";
    let five =
        "%5B%22IMAGE-00000%22%2C%22IMAGE-00040%22%2C%22IMAGE-00080%22%2C%22IMAGE-00120%22%2C%22IMAGE-00160%22%5D";
    let paths = [
        ("wd_first", format!("/api/assets?like={one}")),
        ("wd_third", format!("/api/assets?like={one}&offset=200")),
        ("wd_count", format!("/api/assets/count?like={one}")),
        ("wd_ids", format!("/api/ids?like={one}&limit=500")),
        ("wd_three_conditions", format!("/api/assets?like={one}&day=2026-04-01&nsfw=hide&q=cluster-a")),
        ("wd_five_examples", format!("/api/assets?like={five}")),
        ("ccip_first", format!("/api/assets?like={one}&like_by=ccip")),
    ];
    // 先量第一次从源库建缓存，再用三个独立宿主量从已有派生文件加载。
    let (host, origin, token) = launch_inner(&root, &package, "benchmark", false, binary, true);
    let start = Instant::now();
    search(&origin, &token, &paths[0].1);
    println!("FIRST_BUILD wd_ms {:.3}", start.elapsed().as_secs_f64() * 1000.0);
    stop(host);
    let cache = package.join("cache/wd-tags-idf-v1.bin");
    let written = fs::metadata(&cache).unwrap().modified().unwrap();
    let mut samples: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for trial in 1..=3 {
        let (host, origin, token) = launch_inner(&root, &package, "benchmark", false, binary, true);
        let pid = host.child.id();
        let run = |uri: &str| search(&origin, &token, uri);
        for (prefix, uri) in [("wd", &paths[0].1), ("ccip", &paths[6].1)] {
            let before = rss(pid);
            let start = Instant::now();
            run(uri);
            let (cold, memory) =
                if prefix == "wd" { ("wd_cold_ms", "wd_rss_mib") } else { ("ccip_cold_ms", "ccip_rss_mib") };
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            println!("TRIAL {trial} {cold} {elapsed:.3}");
            samples.entry(cold).or_default().push(elapsed);
            samples.entry(memory).or_default().push(rss(pid) - before);
        }
        for (name, url) in &paths {
            for _ in 0..5 {
                run(url);
            }
            let start = Instant::now();
            for _ in 0..50 {
                run(url);
            }
            samples.entry(name).or_default().push(start.elapsed().as_secs_f64() * 1000.0 / 50.0);
        }
        stop(host);
        assert_eq!(fs::metadata(&cache).unwrap().modified().unwrap(), written, "派生缓存被重建");
    }
    for (name, mut trials) in samples {
        trials.sort_by(f64::total_cmp);
        println!("BENCH {name} {:.3}", trials[1]);
    }
}

fn stop(mut host: Host) {
    // 和桌面外壳一样通过 EOF 关闭，让下一轮正常打开同一份合成图库。
    drop(host.child.stdin.take());
    let deadline = wait::deadline();
    loop {
        if let Some(status) = host.child.try_wait().unwrap() {
            host.reaped = true;
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "宿主未在等待上限内退出");
        thread::sleep(Duration::from_millis(25));
    }
}
