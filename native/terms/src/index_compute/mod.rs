//! 在 App Index 准备好的任务上做索引计算。不联网，也不做正式发布。
mod build;
mod clean;
pub mod numeric;
mod scan;
use crate::recognition_compute::{backend::Acceleration, producer, session::CcipSession};
use crate::research_compute::features::{read_npy, NpyValues};
use crate::{Error, Result};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

pub(super) const DIM: usize = 768;
pub(super) fn read(path: &Path) -> Result<Value> {
    crate::strict_json::parse(&fs::read_to_string(path)?)
        .map_err(|e| Error::invalid(format!("{}: {e}", path.display())))
}
pub(super) fn write(path: &Path, value: &Value) -> Result<()> {
    fs::write(path, serde_json::to_vec(value).map_err(|e| Error::invalid(e.to_string()))?)?;
    Ok(())
}
// 元数据里并列最大的性别中哪个排在前面是可观察的，不能让 Value 的有序映射改写它。
pub(super) fn write_meta(path: &Path, value: &Value, keys: &[String]) -> Result<()> {
    use serde::ser::{Serialize, SerializeMap};
    struct Gender<'a> {
        values: &'a serde_json::Map<String, Value>,
        keys: &'a [String],
    }
    impl Serialize for Gender<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
            let mut map = serializer.serialize_map(Some(self.values.len()))?;
            for key in self.keys {
                if let Some(value) = self.values.get(key) {
                    map.serialize_entry(key, value)?;
                }
            }
            for (key, value) in self.values {
                if !self.keys.contains(key) {
                    map.serialize_entry(key, value)?;
                }
            }
            map.end()
        }
    }
    let Some(gender) = value["gender"].as_object() else { return write(path, value) };
    let mut bytes = Vec::new();
    let mut serializer = serde_json::Serializer::new(&mut bytes);
    let mut map = serde::Serializer::serialize_map(&mut serializer, None).map_err(|e| Error::invalid(e.to_string()))?;
    for (key, entry) in value.as_object().ok_or_else(|| Error::invalid("索引元数据不是对象"))? {
        if key == "gender" {
            map.serialize_entry(key, &Gender { values: gender, keys }).map_err(|e| Error::invalid(e.to_string()))?;
        } else {
            map.serialize_entry(key, entry).map_err(|e| Error::invalid(e.to_string()))?;
        }
    }
    map.end().map_err(|e| Error::invalid(e.to_string()))?;
    fs::write(path, bytes)?;
    Ok(())
}
pub(super) fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key].as_str().ok_or_else(|| Error::invalid(format!("索引字段 {key} 不正确")))
}
pub(super) fn safe(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}
pub(super) fn files(folder: &Path, extension: &str) -> Result<Vec<PathBuf>> {
    if !folder.try_exists()? {
        return Ok(vec![]);
    }
    let mut paths = fs::read_dir(folder)?.map(|v| v.map(|v| v.path())).collect::<std::io::Result<Vec<_>>>()?;
    paths.retain(|p| p.extension().is_some_and(|e| e == extension));
    paths.sort();
    Ok(paths)
}
pub(super) fn reference(path: &Path) -> Result<crate::research_compute::features::NpyMatrix> {
    checked_reference(read_npy(path)?)
}
// 上游截断的文件仍可跳过；能解析但向量无效的，让任务失败。
pub(super) fn optional_reference(path: &Path) -> Result<Option<crate::research_compute::features::NpyMatrix>> {
    match read_npy(path) {
        Ok(raw) => checked_reference(raw).map(Some),
        Err(_) => Ok(None),
    }
}
fn checked_reference(
    raw: crate::research_compute::features::NpyMatrix,
) -> Result<crate::research_compute::features::NpyMatrix> {
    if raw.dimension != DIM {
        return Err(Error::invalid("索引向量维数必须为768"));
    }
    let finite = match &raw.values {
        NpyValues::Float32(r) => r.iter().flatten().all(|x| x.is_finite()),
        NpyValues::Float64(r) => r.iter().flatten().all(|x| x.is_finite()),
    };
    if !finite {
        return Err(Error::invalid("索引向量必须有限"));
    }
    Ok(raw)
}
pub(super) fn matrix(path: &Path) -> Result<Vec<Vec<f32>>> {
    Ok(reference(path)?.raw32())
}
pub(super) fn save(path: &Path, rows: &[Vec<f32>]) -> Result<()> {
    fs::write(path, producer::npy(rows, DIM)?)?;
    Ok(())
}
fn progress(review: &Path, identity: &Value, step: &str, done: Option<usize>, total: Option<usize>) -> Result<()> {
    let mut v = json!({"identity":identity,"step":step});
    if let Some(n) = done {
        v["done"] = json!(n);
    }
    if let Some(n) = total {
        v["total"] = json!(n);
    }
    let tmp = review.join("progress.tmp");
    write(&tmp, &v)?;
    fs::rename(tmp, review.join("progress.json"))?;
    Ok(())
}
pub struct Resources<'a> {
    pub tags: &'a Path,
    pub model: &'a Path,
    pub runtime: &'a Path,
    pub acceleration: &'a Acceleration,
}
/// 遇到第一个需要计算的有效参考图时才加载 CCIP；不加载 WD，也不用 Features 缓存。
pub fn run_files(input: &Path, output: &Path, resources: &Resources<'_>) -> Result<()> {
    let req = read(input)?;
    let mut session = None;
    let mut features = |image: &image::RgbImage| -> Result<Vec<f32>> {
        if session.is_none() {
            session = Some(CcipSession::load(resources.model, resources.runtime, resources.acceleration)?);
        }
        session.as_mut().ok_or_else(|| Error::invalid("CCIP会话不可用"))?.features(image)
    };
    let response = run_with(input, output, &req, resources.tags, &mut features)?;
    let temporary = output.with_extension("tmp");
    write(&temporary, &response)?;
    fs::rename(temporary, output)?;
    Ok(())
}
fn run_with(
    input: &Path,
    output: &Path,
    req: &Value,
    tags: &Path,
    features: &mut dyn FnMut(&image::RgbImage) -> Result<Vec<f32>>,
) -> Result<Value> {
    if req["format"] != "illustash-index/1" {
        return Err(Error::invalid("unsupported index request"));
    }
    let review = Path::new(field(req, "review")?).canonicalize()?;
    if input.canonicalize()?.parent() != Some(review.as_path())
        || output.file_name().and_then(|s| s.to_str()) != Some("response.json")
        || output.parent().ok_or_else(|| Error::invalid("worker output missing parent"))?.canonicalize()? != review
    {
        return Err(Error::invalid("worker output outside isolated job"));
    }
    let rows = req["rows"].as_array().ok_or_else(|| Error::invalid("索引rows不正确"))?;
    let selected = rows
        .iter()
        .map(|r| {
            let s = field(r, "short_tag")?;
            if !safe(s) {
                return Err(Error::invalid("索引角色ID不正确"));
            }
            Ok(s.to_owned())
        })
        .collect::<Result<HashSet<_>>>()?;
    match req["action"].as_str() {
        Some("apply") => {
            for (i, row) in rows.iter().enumerate() {
                progress(&review, &req["identity"], "fetch", Some(i), Some(rows.len()))?;
                fetch(&review, row, features)?;
            }
            progress(&review, &req["identity"], "clean", None, None)?;
            clean::clean(&review, &selected)?;
        }
        Some("drop") => clean::drop(&review, &selected, field(req, "reason")?.trim())?,
        // 启用或撤回基础角色索引（#607）：不下载也不清理，只按任务目录里有没有包重建。
        Some("base") if rows.is_empty() => {}
        _ => return Err(Error::invalid("unsupported index action")),
    }
    progress(&review, &req["identity"], "build", None, None)?;
    let (characters, aliases, vectors, ids, meta, patched) = build::build(&review, tags)?;
    progress(&review, &req["identity"], "scan", None, None)?;
    let labels = scan::scan(&review, &characters, &vectors, &ids, &meta, &patched)?;
    Ok(
        json!({"format":req["format"],"identity":req["identity"],"before":req["before"],"characters":characters,"aliases":aliases,"labels":labels}),
    )
}
fn fetch(review: &Path, row: &Value, features: &mut dyn FnMut(&image::RgbImage) -> Result<Vec<f32>>) -> Result<()> {
    let cid = field(row, "short_tag")?;
    let idx = review.join("char-index");
    let patch = review.join("char-patch");
    if idx.join(format!("{cid}.npy")).try_exists()?
        || patch.join(format!("raw/{cid}.npy")).try_exists()?
        || patch.join(format!("rejected/{cid}.json")).try_exists()?
    {
        return Ok(());
    }
    let refs = read(&patch.join(format!("{cid}-refs.json")))?;
    let refs = refs.as_array().ok_or_else(|| Error::invalid("参考图清单不正确"))?;
    let mut vectors = vec![];
    let mut posts = vec![];
    let mut general = std::collections::BTreeMap::<String, usize>::new();
    for r in refs {
        let path = Path::new(field(r, "path")?);
        // 只解码 App Index 下载到隔离目录的参考图。
        let permitted = match path.canonicalize() {
            Ok(actual) => actual.starts_with(review.join("char-patch/img")),
            Err(_) => path
                .parent()
                .and_then(|p| p.canonicalize().ok())
                .is_some_and(|p| p.starts_with(review.join("char-patch/img"))),
        };
        if !permitted || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(Error::invalid("参考图路径越界"));
        }
        let image = match producer::decode(path) {
            Ok(i) => i,
            Err(_) => continue,
        };
        vectors.push(features(&image)?);
        posts.push(r["id"].clone());
        for tag in r["general"].as_str().unwrap_or("").split_whitespace().collect::<HashSet<_>>() {
            *general.entry(tag.into()).or_default() += 1;
        }
    }
    if posts.len() < 3 {
        return clean::reject(review, row, &format!("Danbooru 上可用的全年龄单人图只有 {n} 张", n = posts.len()));
    }
    let n = posts.len() as f64;
    let mut core: Vec<_> = general
        .iter()
        .filter(|(t, c)| **c as f64 / n >= 0.6 && !["solo", "1girl", "1boy", "looking_at_viewer"].contains(&t.as_str()))
        .map(|(t, c)| (t.clone(), *c))
        .collect();
    core.sort_by_key(|(t, c)| (std::cmp::Reverse(*c), t.clone()));
    core.truncate(24);
    let mut meta = row.clone();
    meta["copyrights"] = json!([row["copyright"]]);
    meta["core_tags"] = json!(core.iter().map(|(t, _)| t).collect::<Vec<_>>());
    meta["gender"] =
        json!({"girl":*general.get("1girl").unwrap_or(&0) as f64/n,"boy":*general.get("1boy").unwrap_or(&0) as f64/n});
    meta["source"] = json!("danbooru-patch");
    meta["fetched_at"] = json!(chrono::Local::now().format("%Y-%m-%d").to_string());
    meta["posts"] = json!(posts);
    fs::create_dir_all(&idx)?;
    save(&idx.join(format!("{cid}.npy")), &vectors)?;
    write_meta(&idx.join(format!("{cid}.json")), &meta, &["girl".into(), "boy".into()])
}

/// 已安装适配器的接入点。分派器只在通过验收的平台上启用它。
pub fn worker(arguments: &[String]) -> Result<()> {
    if !cfg!(any(target_os = "macos", target_os = "windows")) {
        return Err(Error::invalid("原生索引平台数值验收尚未完成"));
    }
    let resources = crate::app_resources::default_root();
    let files = crate::recognition_compute::models::ModelFiles::resolve(&resources)?;
    let model = files.root.join("ccip/model_feat.onnx");
    let runtime = crate::app_resources::ort_runtime(&resources);
    let tags = resources.join("tags/tags_v3.csv");
    let shared = crate::app_resources::shared_path(&resources).ok();
    let acceleration = Acceleration::from_env(shared.as_deref());
    // Windows 上 scan 和锚点的矩阵乘积需要固定的 OpenBLAS 后端；在做任何工作之前就失败。
    #[cfg(target_os = "windows")]
    illustash_platform::platform_research_math::windows_backend_identity().map_err(Error::Invalid)?;
    if arguments == ["--check"] {
        let _ = CcipSession::load(&model, &runtime, &acceleration)?;
        return Ok(());
    }
    if arguments.len() != 2 {
        return Err(Error::invalid("需要 request.json response.json 或 --check"));
    }
    let request = read(Path::new(&arguments[0]))?;
    let _: crate::app_tasks::TaskIdentity = serde_json::from_value(request["identity"].clone())
        .map_err(|e| Error::invalid(format!("索引 identity: {e}")))?;
    run_files(
        Path::new(&arguments[0]),
        Path::new(&arguments[1]),
        &Resources { tags: &tags, model: &model, runtime: &runtime, acceleration: &acceleration },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> PathBuf {
        let p = std::env::temp_dir().join(format!("illustash-index-{}", crate::store::new_key().unwrap()));
        fs::create_dir_all(p.join("char-index")).unwrap();
        fs::create_dir_all(p.join("char-patch")).unwrap();
        write(&p.join("album-char-map.json"), &json!({"map":{}})).unwrap();
        write(&p.join("char-patch/plan.json"), &json!([])).unwrap();
        fs::write(p.join("tags.csv"), "name,category\nalice,4\n").unwrap();
        crate::sqlite::open(p.join("pi.db"))
            .unwrap()
            .execute_batch("CREATE TABLE assets(uuid); CREATE TABLE embeddings(uuid,model,vec); CREATE TABLE labels(uuid,dim,value,conf);")
            .unwrap();
        p
    }
    fn req(p: &Path, action: &str, rows: Value) -> Value {
        json!({"format":"illustash-index/1","identity":{"task_id":"synthetic"},"before":"fixed","review":p,"action":action,"rows":rows,"reason":"合成放弃"})
    }
    fn run(p: &Path, v: &Value, f: &mut dyn FnMut(&image::RgbImage) -> Result<Vec<f32>>) -> Result<Value> {
        write(&p.join("request.json"), v)?;
        run_with(&p.join("request.json"), &p.join("response.json"), v, &p.join("tags.csv"), f)
    }
    #[test]
    fn fresh_empty_and_last_drop_clear_stale_anchors_without_model() {
        let p = root();
        let mut no = |_: &image::RgbImage| -> Result<Vec<f32>> { panic!("empty/drop must not infer") };
        let out = run(&p, &req(&p, "apply", json!([])), &mut no).unwrap();
        assert_eq!(out["characters"], json!([]));
        assert_eq!(out["labels"], json!([]));
        assert_eq!(matrix(&p.join("char-anchors.npy")).unwrap().len(), 0);
        let mut a = vec![0.; DIM];
        a[0] = 1.;
        save(&p.join("char-index/a.npy"), &vec![a; 6]).unwrap();
        write(&p.join("char-index/a.json"), &json!({"short_tag":"a","tag":"a","source":"danbooru-patch"})).unwrap();
        let out = run(&p, &req(&p, "drop", json!([{"short_tag":"a"}])), &mut no).unwrap();
        assert_eq!(out["characters"], json!([]));
        assert_eq!(read(&p.join("char-patch/rejected/a.json")).unwrap()["rejected"], "人工看图后放弃：合成放弃");
        assert!(fs::read_to_string(p.join("char-anchors.txt")).unwrap().is_empty());
        fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn bad_reference_skipped_model_failure_propagates_and_no_rejection() {
        let p = root();
        fs::create_dir_all(p.join("char-patch/img/a")).unwrap();
        let bad = p.join("char-patch/img/a/1.img");
        fs::write(&bad, b"bad image").unwrap();
        let good = p.join("char-patch/img/a/2.img");
        image::RgbImage::new(8, 8).save_with_format(&good, image::ImageFormat::Png).unwrap();
        write(&p.join("char-patch/a-refs.json"), &json!([{"path":bad,"id":1},{"path":good,"id":2}])).unwrap();
        let mut count = 0;
        let mut fail = |_: &image::RgbImage| {
            count += 1;
            Err(Error::invalid("model unavailable"))
        };
        let v = req(&p, "apply", json!([{"short_tag":"a"}]));
        assert!(run(&p, &v, &mut fail).unwrap_err().to_string().contains("model unavailable"));
        assert_eq!(count, 1);
        assert!(!p.join("response.json").exists());
        assert!(!p.join("char-patch/rejected/a.json").exists());
        fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn fetch_and_reclean_preserve_girl_first_gender_tie() {
        let p = root();
        fs::create_dir_all(p.join("char-patch/img/a")).unwrap();
        let image = p.join("char-patch/img/a/1.png");
        image::RgbImage::new(8, 8).save(&image).unwrap();
        let refs: Vec<_> = (0..6)
            .map(|i| json!({"path":image,"id":i,"general":if i % 2 == 0 {"solo 1girl"} else {"solo 1boy"}}))
            .collect();
        write(&p.join("char-patch/a-refs.json"), &json!(refs)).unwrap();
        let row = json!({"short_tag":"a","tag":"a","copyright":"synthetic","post_count":10});
        write(&p.join("char-patch/plan.json"), &json!([row])).unwrap();
        let mut a = vec![0.; DIM];
        a[0] = 1.;
        let mut features = |_: &image::RgbImage| Ok(a.clone());
        let request = req(&p, "apply", json!([row]));
        for _ in 0..2 {
            let output = run(&p, &request, &mut features).unwrap();
            assert_eq!(output["characters"][0]["gender"], "girl");
            assert_eq!(output["characters"][0]["gender_conf"], 0.5);
            assert_eq!(
                build::ordered_gender(&p.join("char-index/a.json")).unwrap(),
                vec![("girl".into(), 0.5), ("boy".into(), 0.5)]
            );
        }
        fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn truncated_upstream_skipped_but_parsed_invalid_vectors_rejected() {
        let p = root();
        let fp = p.join("char-index/a.npy");
        fs::write(&fp, b"truncated").unwrap();
        assert!(optional_reference(&fp).unwrap().is_none());
        fs::write(&fp, producer::npy(&vec![vec![1.; 2]; 3], 2).unwrap()).unwrap();
        assert!(optional_reference(&fp).is_err());
        let mut bytes = producer::npy(&vec![vec![1.; DIM]; 3], DIM).unwrap();
        let offset = 10 + u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
        bytes[offset..offset + 4].copy_from_slice(&f32::NAN.to_le_bytes());
        fs::write(&fp, bytes).unwrap();
        assert!(optional_reference(&fp).is_err());
        fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn unsafe_id_and_output_rejected_before_computation() {
        let p = root();
        let mut no = |_: &image::RgbImage| -> Result<Vec<f32>> { panic!("invalid request must not infer") };
        assert!(run(&p, &req(&p, "apply", json!([{"short_tag":"../other"}])), &mut no).is_err());
        let v = req(&p, "apply", json!([]));
        write(&p.join("request.json"), &v).unwrap();
        assert!(run_with(&p.join("request.json"), &p.join("elsewhere.json"), &v, &p.join("tags.csv"), &mut no).is_err());
        fs::remove_dir_all(p).unwrap();
    }
}
