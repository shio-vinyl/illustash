//! 签名包目录里的角色词表。每个进程从已下载的文件中选定一次，导入、投影和取名始终读同一份。
use super::{bad, cached, extensions_root, get, Catalog, Entry, File};
use crate::{character_catalog, Error, Result};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

pub(super) const KIND: &str = "character-catalog";
pub(super) const LIMIT: u64 = 16 << 20;

fn descriptors(catalog: &Catalog) -> impl Iterator<Item = (&Entry, &File)> {
    catalog.entries.iter().filter_map(|entry| Some((entry, entry.character_file.as_ref()?)))
}

fn available(catalog: &Catalog) -> Vec<(&Entry, &File)> {
    let app = crate::app_updates::current();
    let mut entries: Vec<_> = descriptors(catalog)
        .filter(|(e, _)| {
            e.character_supported && app.core() >= e.min_app.core() && e.revision > character_catalog::BUNDLED_REVISION
        })
        .collect();
    entries.sort_by_key(|(e, _)| std::cmp::Reverse(e.revision));
    entries
}

fn local_file(root: &Path, file: &File) -> PathBuf {
    root.join(format!("character-catalog-{}.json", file.sha256))
}

fn checked(bytes: &[u8], file: &File, revision: u32) -> Result<character_catalog::Catalog> {
    if bytes.len() as u64 != file.size || crate::strict_json::sha256_hex(bytes) != file.sha256 {
        return Err(bad("角色词表的大小或 SHA-256 和签名目录不符"));
    }
    let source = std::str::from_utf8(bytes).map_err(|_| bad("角色词表不是 UTF-8"))?;
    let catalog = character_catalog::Catalog::parse(source)?;
    if catalog.revision != revision {
        return Err(bad("角色词表的修订号和签名目录不符"));
    }
    Ok(catalog)
}

fn read(root: &Path, file: &File, revision: u32) -> Result<Option<character_catalog::Catalog>> {
    let payload = match fs::File::open(local_file(root, file)) {
        Ok(open) => open,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let mut bytes = Vec::new();
    payload.take(file.size + 1).read_to_end(&mut bytes)?;
    checked(&bytes, file, revision).map(Some)
}

/// 首次准备或检查更新时下载最新的适用修订。目录没变但上次下载失败时也会重试。
pub(super) fn fetch(resources: &Path, agent: &ureq::Agent) -> Result<()> {
    let Some(catalog) = cached(resources)? else { return Ok(()) };
    let Some((entry, file)) = available(&catalog).first().copied() else { return Ok(()) };
    let root = extensions_root(resources)?;
    if matches!(read(&root, file, entry.revision), Ok(Some(_))) {
        return Ok(());
    }
    let address = crate::app_updates::asset_url(&file.url).map_err(Error::Network)?;
    let bytes = get(agent, &address, file.size).map_err(Error::Network)?;
    install(&root, file, entry.revision, &bytes)?;
    Ok(())
}

fn install(root: &Path, file: &File, revision: u32, bytes: &[u8]) -> Result<bool> {
    checked(bytes, file, revision)?;
    if matches!(read(root, file, revision), Ok(Some(_))) {
        return Ok(false);
    }
    fs::create_dir_all(root)?;
    let path = local_file(root, file);
    let tmp = path.with_extension(format!("{}.tmp", crate::store::new_key()?));
    let write = fs::write(&tmp, bytes).and_then(|()| fs::rename(&tmp, &path));
    if write.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    write?;
    Ok(true)
}

/// 按 ZIP 内的版本路径认词表。也兼容单文件放在 ZIP 根上的旧写法；多个候选仍逐个核对完整摘要。
/// 文件必须同时属于 ZIP 的签名目录和当前采用的目录，较旧的签名目录无法撤销本机已知的限制。
pub(super) fn import_member(
    resources: &Path,
    archive: &Catalog,
    active: Option<&Catalog>,
    name: &str,
    size: u64,
    reader: &mut dyn Read,
) -> Option<&'static str> {
    let candidates: Vec<_> = descriptors(archive)
        .filter(|(entry, file)| {
            name == file.name || name.split_once('/') == Some((entry.version.as_str(), file.name.as_str()))
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let usable = active.map(available).unwrap_or_default();
    let accepted: Vec<_> = candidates
        .iter()
        .filter_map(|(entry, file)| {
            usable.iter().find(|(current, actual)| {
                current.revision == entry.revision
                    && current.version == entry.version
                    && actual.sha256 == file.sha256
                    && actual.size == file.size
            })
        })
        .collect();
    if accepted.is_empty() {
        return Some("catalog_file_unsupported");
    }
    if !accepted.iter().any(|(_, file)| file.size == size) {
        return Some("mismatch");
    }
    let mut bytes = Vec::new();
    if reader.take(size + 1).read_to_end(&mut bytes).is_err() {
        return Some("catalog_file_invalid");
    }
    let digest = crate::strict_json::sha256_hex(&bytes);
    let Some((entry, file)) =
        accepted.into_iter().find(|(_, file)| file.sha256 == digest && file.size == bytes.len() as u64)
    else {
        return Some("mismatch");
    };
    Some(match extensions_root(resources).and_then(|root| install(&root, file, entry.revision, &bytes)) {
        Ok(true) => "catalog_file_installed",
        Ok(false) => "already",
        Err(error) => {
            eprintln!("无法导入角色词表 {}：{error}", entry.version);
            "catalog_file_invalid"
        }
    })
}

pub(crate) fn load(resources: &Path) {
    character_catalog::initialize(|| match installed(resources) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("无法读取已安装的角色词表：{error}");
            None
        }
    });
}

fn installed(resources: &Path) -> Result<Option<character_catalog::Catalog>> {
    let Some(catalog) = cached(resources)? else { return Ok(None) };
    let root = extensions_root(resources)?;
    for (entry, file) in available(&catalog) {
        match read(&root, file, entry.revision) {
            Ok(Some(data)) => return Ok(Some(data)),
            Ok(None) => (),
            Err(error) => eprintln!("已下载的角色词表 {} 无法使用：{error}", entry.version),
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_packs::{CATALOG_FORMAT, RELEASE_PREFIX};
    use serde_json::json;

    #[test]
    fn only_signed_descriptor_matching_file_and_revision_is_usable() {
        let data = json!({"format":character_catalog::FORMAT, "revision":2,
            "columns":["id","tag","tags","parent","relation","action","variant","bangumi","pick","wikidata"],
            "characters":[],"works":[]})
        .to_string();
        let descriptor = json!({"kind":KIND, "version":"catalog-2", "revision":2, "min_app":"0.4.0",
            "format":character_catalog::FORMAT, "license":"test","file":{"name":"roles.json",
            "url":format!("{RELEASE_PREFIX}catalog-2/roles.json"),"size":data.len(),
            "sha256":crate::strict_json::sha256_hex(data.as_bytes())}});
        let catalog = |entry| {
            Catalog::parse(&serde_json::to_vec(&json!({"format":CATALOG_FORMAT,"serial":1,"packs":[entry]})).unwrap())
        };
        let parsed = catalog(descriptor.clone()).unwrap();
        let (_, file) = available(&parsed)[0];
        assert_eq!(checked(data.as_bytes(), file, 2).unwrap().revision, 2);
        assert!(checked(data.as_bytes(), file, 3).is_err());
        assert!(checked(b"{}", file, 2).is_err());
        let mut future = descriptor.clone();
        future["min_app"] = json!("99.0.0");
        assert!(available(&catalog(future).unwrap()).is_empty());
        let mut future = descriptor.clone();
        future["format"] = json!("illustash-character-catalog/9");
        assert!(available(&catalog(future).unwrap()).is_empty());
        let mut bundled = descriptor.clone();
        bundled["revision"] = json!(character_catalog::BUNDLED_REVISION);
        assert!(available(&catalog(bundled).unwrap()).is_empty());
        let mut replaced = descriptor;
        replaced["file"]["sha256"] = json!("1".repeat(64));
        assert!(catalog(replaced.clone()).unwrap().require_same_version_files(&parsed).is_err());
        replaced["file"]["size"] = json!(LIMIT + 1);
        assert!(catalog(replaced).is_err());
    }
}
