//! 只建全新图库的文件夹/Photos 图库包工厂。不登记图库、不切换会话、不扫描，也不升级已有图库。
use crate::{Error, Result};
use rusqlite::{types::ValueRef, Connection, OpenFlags, TransactionBehavior};
use serde_json::{json, Map, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

const APPLICATION: &str = include_str!("app_create_schema.sql");
/// 内置作品名写进核心的 `term_key_names`，所以在核心迁移之后。
const BUILTIN_NAMES: &str = include_str!("app_create_names.sql");
const SOURCE: &str = include_str!("app_source_schema.sql");

pub fn create_folder(
    dest: &Path,
    folder: &Path,
    name: Option<&str>,
    preset: Option<&str>,
    reference: Option<&Path>,
) -> Result<Value> {
    create(dest, folder, name, preset, reference, |_| Ok(()))
}
/// 描述 Photos 来源，不打开、不解析、不校验平台所属的文件。
pub fn create_photos(dest: &Path, photos: &Path, name: Option<&str>, reference: Option<&Path>) -> Result<Value> {
    let dest = absolute(dest)?;
    let name = name
        .filter(|v| !v.is_empty())
        .or_else(|| dest.file_stem().and_then(|v| v.to_str()))
        .ok_or_else(|| Error::Invalid("图库名称不是 UTF-8".into()))?;
    let sources = json!([{"id":"s1","kind":"photos","path":text(photos)?,
        "apps":crate::origin::photos_defaults()}]);
    create_package(&dest, name, sources, reference, |_| Ok(()))
}
fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(if path.is_absolute() { path.to_owned() } else { std::env::current_dir()?.join(path) })
}
fn text(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| Error::Invalid("图库路径不是 UTF-8".into()))
}
fn create<F: FnOnce(&Connection) -> Result<()>>(
    dest: &Path,
    folder: &Path,
    name: Option<&str>,
    preset: Option<&str>,
    reference: Option<&Path>,
    after_birth: F,
) -> Result<Value> {
    let dest = absolute(dest)?;
    if dest.extension().and_then(|s| s.to_str()) != Some("pilib") {
        return Err(Error::Invalid("图库包的扩展名必须是 .pilib".into()));
    }
    let folder = absolute(folder)?;
    if !folder.is_dir() {
        return Err(Error::Invalid(format!("不是文件夹：{path}", path = folder.display())));
    }
    let folder = crate::app_paths::canonical(&folder).map_err(|e| crate::error::io_at(e, &folder, None))?;
    // 出处按用户说的来；没给时是「其他」，从不按文件夹名猜。
    let explicit_preset = preset.is_some();
    let preset = match preset {
        Some(value) if crate::origin::find(value).is_some() => value.to_owned(),
        Some(_) => return Err(Error::Invalid("来源预设只能是 x、pixiv、other".into())),
        None => "other".to_owned(),
    };
    let name = name
        .filter(|v| !v.is_empty())
        .or_else(|| folder.file_name().and_then(|v| v.to_str()))
        .filter(|v| !v.is_empty())
        .or_else(|| dest.file_stem().and_then(|v| v.to_str()))
        .ok_or_else(|| Error::Invalid("图库名称不是 UTF-8".into()))?;
    let sources = json!([{"id":"s1","kind":"folder","path":text(&folder)?,"preset":preset,
        "preset_origin":if explicit_preset {"user"} else {"auto"}}]);
    create_package(&dest, name, sources, reference, after_birth)
}
/// 只有可选的参考文件不存在时才选用空的内置 schema。链接失效、是目录、没有权限仍然报错，不会悄悄丢掉索引。
pub(crate) fn optional_reference(reference: &Path) -> Result<Option<&Path>> {
    match fs::symlink_metadata(reference) {
        Ok(_) => Ok(Some(reference)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(crate::error::io_at(error, reference, None)),
    }
}
/// 新建图库的一个来源：带用户所给出处的文件夹，或 Photos。
pub enum NewSource {
    Folder { path: PathBuf, preset: String },
    Photos { path: PathBuf },
}
/// 文件夹要成为来源，必须存在、出处已知，并且既不包含另一个文件夹来源，
/// 也不在另一个文件夹来源里面（同一个文件不能属于两个来源）。
pub(crate) fn folder_source(id: &str, folder: &Path, preset: &str, others: &[PathBuf]) -> Result<(PathBuf, Value)> {
    let folder = absolute(folder)?;
    if !folder.is_dir() {
        return Err(Error::Invalid(format!("不是文件夹：{path}", path = folder.display())));
    }
    let folder = crate::app_paths::canonical(&folder).map_err(|e| crate::error::io_at(e, &folder, None))?;
    if crate::origin::find(preset).is_none() {
        return Err(Error::Invalid("来源预设只能是 x、pixiv、other".into()));
    }
    if let Some(other) = others.iter().find(|other| crate::app_paths::overlaps(&folder, other)) {
        return Err(Error::Invalid(format!(
            "{path} 与已添加的文件夹 {other} 重叠：同一个文件夹不能添加两次，也不能和它的上级或下级文件夹同时添加",
            path = folder.display(),
            other = other.display()
        )));
    }
    let entry = json!({"id":id,"kind":"folder","path":text(&folder)?,"preset":preset,"preset_origin":"user"});
    Ok((folder, entry))
}
/// 有一个或多个来源的图库，id 按给定顺序为 s1、s2、…。最多一个 Photos 来源。
pub fn create_sources(dest: &Path, name: &str, sources: Vec<NewSource>, reference: Option<&Path>) -> Result<Value> {
    let dest = absolute(dest)?;
    if sources.is_empty() {
        return Err(Error::Invalid("请至少添加一个来源".into()));
    }
    let (mut folders, mut entries, mut photos) = (Vec::new(), Vec::new(), false);
    for (i, source) in sources.into_iter().enumerate() {
        let id = format!("s{}", i + 1);
        match source {
            NewSource::Folder { path, preset } => {
                let (folder, entry) = folder_source(&id, &path, &preset, &folders)?;
                folders.push(folder);
                entries.push(entry);
            }
            NewSource::Photos { path } => {
                if std::mem::replace(&mut photos, true) {
                    return Err(Error::Invalid("照片图库只能添加一次".into()));
                }
                entries
                    .push(json!({"id":id,"kind":"photos","path":text(&path)?,"apps":crate::origin::photos_defaults()}));
            }
        }
    }
    create_package(&dest, name, Value::Array(entries), reference, |_| Ok(()))
}
fn create_package<F: FnOnce(&Connection) -> Result<()>>(
    dest: &Path,
    name: &str,
    sources: Value,
    reference: Option<&Path>,
    after_birth: F,
) -> Result<Value> {
    if dest.extension().and_then(|s| s.to_str()) != Some("pilib") {
        return Err(Error::Invalid("图库包的扩展名必须是 .pilib".into()));
    }
    if let Some(reference) = reference {
        if !reference.is_file() {
            return Err(Error::Invalid(format!("缺少参考库 {path}", path = reference.display())));
        }
    }
    // 独占获取：已有的目录、文件，甚至失效的符号链接都不会进入清理。
    // 先尝试最后一级路径，与 pathlib.mkdir(parents=True) 的出错路径一致。
    // 直接递归创建会接受已经存在的目录。
    match fs::create_dir(dest) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| crate::error::io_at(e, parent, None))?;
            }
            fs::create_dir(dest).map_err(|e| crate::error::io_at(e, dest, None))?;
        }
        Err(error) => return Err(crate::error::io_at(error, dest, None)),
    }
    let result = (|| {
        for sub in ["features", "thumbnails", "backups"] {
            let path = dest.join(sub);
            fs::create_dir(&path).map_err(|e| crate::error::io_at(e, &path, None))?;
        }
        initialize_source(&dest.join("source.sqlite"), reference)?;
        if sources.as_array().is_some_and(|items| items.iter().any(|item| item["kind"] == "photos")) {
            let c = crate::sqlite::open_with_flags(dest.join("source.sqlite"), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            c.prepare("SELECT z_pk FROM assets LIMIT 0")
                .map_err(|e| Error::Invalid(format!("参考库结构不支持照片同步：{e}")))?;
        }
        initialize_application(&dest.join("library.sqlite"), &dest.join("source.sqlite"), after_birth)?;
        let c = Connection::open_in_memory()?;
        let (created, offset): (String, i64) = c.query_row(
            "SELECT strftime('%Y-%m-%dT%H:%M:%S','now','localtime'),unixepoch('now','localtime')-unixepoch('now')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let created = format!(
            "{created}{}{:02}:{:02}",
            if offset < 0 { '-' } else { '+' },
            offset.abs() / 3600,
            offset.abs() % 3600 / 60
        );
        let config = json!({"format":1,"id":crate::store::new_key()?.to_ascii_uppercase(),"name":name,"created_at":created,
            "sources":sources});
        // library.json 是让图库能被发现的标记。只在两个数据库都写完并关闭后才发布。
        let temp = dest.join("library.json.tmp");
        let sources = serde_json::to_string_pretty(&config["sources"])
            .map_err(|e| Error::Invalid(e.to_string()))?
            .replace('\n', "\n  ");
        // 保持现有的配置字段顺序，不改变 serde_json 全局的 map 表示。
        let bytes = format!("{{\n  \"format\": 1,\n  \"id\": {},\n  \"name\": {},\n  \"created_at\": {},\n  \"sources\": {sources}\n}}\n", config["id"], config["name"], config["created_at"]);
        fs::write(&temp, bytes).map_err(|e| crate::error::io_at(e, &temp, None))?;
        let config_path = dest.join("library.json");
        fs::rename(&temp, &config_path).map_err(|e| crate::error::io_at(e, &temp, Some(&config_path)))?;
        Ok(json!({"path":text(dest)?,"name":name}))
    })();
    if let Err(error) = result {
        // 上面的每个 SQLite 连接都限定在失败的闭包里，删除文件之前已经关闭。
        if let Err(cleanup) = fs::remove_dir_all(dest) {
            return Err(Error::Invalid(format!("新图库创建失败：{error}；清理本次新目录失败：{cleanup}")));
        }
        return Err(error);
    }
    result
}
pub(crate) fn quote_id(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
pub(crate) fn file_uri(path: &Path) -> Result<String> {
    let mut uri = String::from("file:");
    for byte in text(&crate::sqlite::path(path))?.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    Ok(uri + "?mode=ro")
}
pub(crate) fn initialize_source(path: &Path, reference: Option<&Path>) -> Result<()> {
    let mut c = crate::sqlite::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_URI,
    )?;
    c.busy_timeout(Duration::from_secs(20))?;
    if let Some(reference) = reference {
        let reference = crate::app_paths::canonical(reference).map_err(|e| crate::error::io_at(e, reference, None))?;
        c.execute("ATTACH DATABASE ? AS ref", [file_uri(&reference)?])?;
    }
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if reference.is_some() {
        let tables: Vec<(String, String)> = tx
            .prepare("SELECT name,sql FROM ref.sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (name, sql) in tables {
            tx.execute_batch(&sql)?;
            if ["characters", "character_aliases", "runs"].contains(&name.as_str()) {
                let name = quote_id(&name);
                tx.execute_batch(&format!("INSERT INTO main.{name} SELECT * FROM ref.{name}"))?;
            }
        }
        let indices: Vec<String> = tx
            .prepare("SELECT sql FROM ref.sqlite_master WHERE type='index' AND sql IS NOT NULL")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for sql in indices {
            tx.execute_batch(&sql.replacen("CREATE INDEX ", "CREATE INDEX IF NOT EXISTS ", 1))?;
        }
    } else {
        tx.execute_batch(include_str!("app_source_empty.sql"))?;
    }
    complete_source(&tx)?;
    // 如果给的参考文件不能支持当前的来源读取和导入，就在发布前失败。
    for sql in [
        "SELECT uuid,filename,src_path,width,height,added_at,src_kind,in_scope,trashed,hidden,stable_hash,orig_size,source_id,preset FROM assets LIMIT 0",
        "SELECT uuid,dim,value,conf,source,model_ver,run_id FROM labels LIMIT 0",
        "SELECT uuid,dim,value,note FROM decisions LIMIT 0",
        "SELECT * FROM runs LIMIT 0",
        "SELECT uuid,model,vec FROM embeddings LIMIT 0",
        "SELECT char_id,wd_tag,name_zh,name_ja,name_en FROM characters LIMIT 0",
        "SELECT char_id,alias FROM character_aliases LIMIT 0",
        "SELECT asset,system,key,source_id,current FROM asset_links LIMIT 0",
        "SELECT source_id,path,size,mtime_ns,md5,asset FROM files LIMIT 0",
    ] {
        tx.prepare(sql).map_err(|e| Error::Invalid(format!("参考库结构不支持当前读取与同步：{e}")))?;
    }
    tx.commit()?;
    if reference.is_some() {
        c.execute("DETACH DATABASE ref", [])?;
    }
    check(&c)?;
    Ok(())
}
/// 每个当前来源都有的 asset 列和文件夹记账表；抢救之后也一样（#228）。
pub(crate) fn complete_source(c: &Connection) -> Result<()> {
    let columns: Vec<String> =
        c.prepare("PRAGMA table_info(assets)")?.query_map([], |r| r.get(1))?.collect::<rusqlite::Result<_>>()?;
    for column in ["source_id", "preset"] {
        if !columns.iter().any(|v| v == column) {
            c.execute_batch(&format!("ALTER TABLE assets ADD COLUMN {column} TEXT"))?;
        }
    }
    c.execute_batch(SOURCE)?;
    Ok(())
}
pub(crate) fn names_triggers(c: &Connection) -> Result<()> {
    c.execute("INSERT OR IGNORE INTO meta VALUES ('names_version','0')", [])?;
    crate::legacy_names::version_triggers(c)
}
fn source_index(c: &Connection) -> Result<Value> {
    let mut query = c.prepare("SELECT * FROM characters")?;
    let columns: Vec<String> = query.column_names().map(str::to_owned).collect();
    let rows = query
        .query_map([], |r| {
            let mut obj = Map::new();
            for (i, key) in columns.iter().enumerate() {
                let value = match r.get_ref(i)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(v) => v.into(),
                    ValueRef::Real(v) => v.into(),
                    ValueRef::Text(v) => std::str::from_utf8(v)
                        .map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(i, rusqlite::types::Type::Text, Box::new(e))
                        })?
                        .into(),
                    ValueRef::Blob(_) => {
                        return Err(rusqlite::Error::InvalidColumnType(i, key.clone(), rusqlite::types::Type::Blob))
                    }
                };
                obj.insert(key.clone(), value);
            }
            Ok(Value::Object(obj))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let aliases = c
        .prepare("SELECT char_id,alias FROM character_aliases")?
        .query_map([], |r| Ok(json!([r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?])))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let tag = if columns.iter().any(|v| v == "danbooru_tag") {
        "coalesce(nullif(danbooru_tag,''),char_id)"
    } else {
        "char_id"
    };
    let tags = c
        .prepare(&format!("SELECT char_id,{tag} FROM characters"))?
        .query_map([], |r| Ok(json!([r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?])))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!({"characters":rows,"aliases":aliases,"tags":tags,"new_ids":{}}))
}
fn initialize_application<F: FnOnce(&Connection) -> Result<()>>(
    path: &Path,
    source: &Path,
    after_birth: F,
) -> Result<()> {
    let mut c = crate::open(path)?;
    c.pragma_update(None, "journal_mode", "WAL")?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(APPLICATION)?;
    tx.commit()?;
    crate::migrate(&mut c)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(BUILTIN_NAMES)?;
    tx.commit()?;
    crate::new_library::switch(&mut c)?;
    names_triggers(&c)?;
    after_birth(&c)?;
    drop(c);
    let src = crate::sqlite::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let index = source_index(&src)?;
    drop(src);
    let clock = Connection::open_in_memory()?;
    let stamp: String = clock.query_row("SELECT strftime('%Y-%m-%d %H:%M:%S','now','localtime')", [], |r| r.get(0))?;
    crate::app_service::character_request(
        path,
        &json!({"op":"refresh","index":index,"assets":[],"labels":[],"decisions":[],"duplicates":[],"album_map":{},"indexed":stamp,"rating_indexed":stamp,"general_indexed":stamp,"realistic":[],"finalize_labels":true}),
    )?;
    let c = crate::sqlite::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    check(&c)?;
    Ok(())
}
fn check(c: &Connection) -> Result<()> {
    let result: String = c.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    if result != "ok" {
        return Err(Error::Invalid("新图库数据库完整性检查失败".into()));
    }
    Ok(())
}

pub(crate) fn creation_error(error: Error, reference: &Path) -> Error {
    match error {
        Error::Invalid(message) if message == format!("缺少参考库 {path}", path = reference.display()) => {
            Error::Invalid(format!("{message}（python -m web.library export-reference）"))
        }
        Error::IoAt { source, path, destination } => {
            Error::Invalid(crate::error::filesystem_message(&source, &path, destination.as_deref()))
        }
        Error::Io(source) => Error::Invalid(source.to_string()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        root: PathBuf,
        folder: PathBuf,
        reference: PathBuf,
        dest: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "illustash-create-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let folder = root.join("pixiv-downloads");
            fs::create_dir(&folder).unwrap();
            // Windows 文件名不允许 `?`；那里仍保留引号与 `#` 两个 URI 敏感字符。
            #[cfg(unix)]
            let reference = root.join("reference '?#.sqlite");
            #[cfg(windows)]
            let reference = root.join("reference '#.sqlite");
            let c = crate::sqlite::open(&reference).unwrap();
            c.execute_batch("CREATE TABLE assets(uuid TEXT PRIMARY KEY,filename TEXT,src_path TEXT,width INTEGER,height INTEGER,added_at TEXT,src_kind TEXT,in_scope INTEGER DEFAULT 1,trashed INTEGER,hidden INTEGER,stable_hash TEXT,orig_size INTEGER,z_pk INTEGER); CREATE TABLE embeddings(uuid TEXT,model TEXT,vec BLOB,PRIMARY KEY(uuid,model)); CREATE TABLE labels(uuid TEXT,dim TEXT,value TEXT,conf REAL,source TEXT DEFAULT 'legacy',model_ver TEXT,run_id TEXT); CREATE TABLE decisions(uuid TEXT,dim TEXT,value TEXT,note TEXT); CREATE TABLE runs(run_id TEXT PRIMARY KEY,dim TEXT); CREATE TABLE characters(char_id TEXT PRIMARY KEY,danbooru_tag TEXT,wd_tag TEXT,copyright TEXT,base_tag TEXT,skin TEXT,name_zh TEXT,name_ja TEXT,name_en TEXT); CREATE TABLE character_aliases(char_id TEXT,alias TEXT,kind TEXT); CREATE INDEX reference_alias_idx ON character_aliases(alias); CREATE TABLE reference_extra(value TEXT); CREATE INDEX reference_extra_idx ON reference_extra(value);
             INSERT INTO assets VALUES ('not-imported','fixture.png','/synthetic-only',10,20,'2026-10-01','orig',1,0,0,NULL,NULL,NULL);
             INSERT INTO labels(uuid,dim,value,conf) VALUES ('not-imported','general','blue_hair',0.9);
             INSERT INTO decisions VALUES ('not-imported','character','alice','album:synthetic pk=1');
             INSERT INTO reference_extra VALUES ('not-copied'); INSERT INTO runs VALUES ('reference-run','character');
             INSERT INTO characters VALUES ('alice','alice_tag','alice_tag','Example','alice',NULL,'爱丽丝',NULL,'Alice'),('alice_swim','alice_swim_tag','alice_swim_tag','Example','alice','swimsuit',NULL,NULL,'Alice swim'),('doodle_sensei_(blue_archive)','doodle_sensei_(blue_archive)',NULL,'blue_archive',NULL,NULL,NULL,NULL,'Doodle Sensei'),('carol_(vtuber)','carol_(vtuber)',NULL,'vtuber','carol',NULL,NULL,NULL,'Carol');
             INSERT INTO character_aliases VALUES ('alice','alias alice','index');").unwrap();
            drop(c);
            let dest = root.join("synthetic.pilib");
            Self { root, folder, reference, dest }
        }
        fn create(&self) -> Result<Value> {
            create_folder(&self.dest, &self.folder, None, None, Some(&self.reference))
        }
        fn db(&self) -> Connection {
            crate::sqlite::open(self.dest.join("library.sqlite")).unwrap()
        }
        fn source(&self) -> Connection {
            crate::sqlite::open(self.dest.join("source.sqlite")).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
    fn count(c: &Connection, sql: &str) -> i64 {
        c.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn built_in_source_is_empty_and_supports_folder_scan() {
        let f = Fixture::new();
        fs::remove_file(&f.reference).unwrap();
        create_folder(&f.dest, &f.folder, None, None, None).unwrap();
        let source = f.source();
        for table in ["assets", "labels", "decisions", "runs", "embeddings", "characters", "character_aliases"] {
            assert_eq!(count(&source, &format!("SELECT count(*) FROM {table}")), 0);
        }
        assert!(!f.reference.exists());
        // 工厂不扫描；扫描器无需测试时改表就能接受它的 schema。
        fs::write(f.folder.join("synthetic.png"), b"synthetic invalid image").unwrap();
        let source_config = json!({"id":"s1","path":f.folder,"preset":"other"});
        let scan = crate::app_folder::scan(
            &source_config,
            &f.dest.join("source.sqlite"),
            &|| crate::app_sync::check(&std::sync::atomic::AtomicBool::new(false)),
            &|_, _| {},
        )
        .unwrap();
        assert_eq!(scan.counts["new"], 1);
    }

    #[test]
    fn photos_source_is_configuration_only_and_preserves_explicit_path() {
        let f = Fixture::new();
        // 失效的链接证明这个工厂不需要、不读取、不解析 Photos 来源。
        let photos = f.root.join("Photos Library.photoslibrary");
        #[cfg(unix)]
        std::os::unix::fs::symlink(f.root.join("missing-photos"), &photos).unwrap();
        let before = fs::read(&f.reference).unwrap();
        assert_eq!(
            create_photos(&f.dest, &photos, None, Some(&f.reference)).unwrap(),
            json!({"path":f.dest,"name":"synthetic"})
        );
        let config: Value = serde_json::from_slice(&fs::read(f.dest.join("library.json")).unwrap()).unwrap();
        assert_eq!(
            config["sources"],
            json!([{"id":"s1","kind":"photos","path":photos,
            "apps":{"com.atebits.Tweetie2":"x","net.pxv.iphone":"pixiv"}}])
        );
        assert_eq!(count(&f.source(), "SELECT count(*) FROM assets"), 0);
        assert_eq!(count(&f.db(), "SELECT count(*) FROM assets"), 0);
        assert_eq!(count(&f.db(), "SELECT count(*) FROM meta WHERE key LIKE 'terms_%' AND value='core'"), 4);
        assert_eq!(before, fs::read(&f.reference).unwrap());
        assert!(!f.root.join("missing-photos").exists());
    }
    #[test]
    fn photos_failure_never_removes_preexisting_destination_or_source() {
        let f = Fixture::new();
        fs::create_dir(&f.dest).unwrap();
        fs::write(f.dest.join("keep"), b"existing package").unwrap();
        assert!(create_photos(&f.dest, &f.folder, None, Some(&f.reference)).is_err());
        assert_eq!(fs::read(f.dest.join("keep")).unwrap(), b"existing package");
        let dest = f.root.join("failed.pilib");
        let c = crate::sqlite::open(&f.reference).unwrap();
        c.execute_batch("DROP TABLE characters").unwrap();
        drop(c);
        assert!(create_photos(&dest, &f.folder, None, Some(&f.reference)).is_err());
        assert!(!dest.exists());
        assert!(f.dest.join("keep").is_file());
        assert!(f.folder.is_dir());
        assert!(f.reference.is_file());
    }

    #[test]
    fn fresh_package_is_complete_and_core_verified_without_importing_reference_assets() {
        let f = Fixture::new();
        let before = fs::read(&f.reference).unwrap();
        let result = f.create().unwrap();
        assert_eq!(result, json!({"path":f.dest,"name":"pixiv-downloads"}));
        let config: Value = serde_json::from_slice(&fs::read(f.dest.join("library.json")).unwrap()).unwrap();
        assert_eq!(config["format"], 1);
        assert_eq!(
            config["sources"][0],
            json!({"id":"s1","kind":"folder","path":crate::app_paths::canonical(&f.folder).unwrap(),"preset":"other","preset_origin":"auto"})
        );
        let id = config["id"].as_str().unwrap();
        assert_eq!(id.len(), 36);
        assert_eq!(id, id.to_ascii_uppercase());
        assert_eq!(id.as_bytes()[14], b'4');
        for sub in ["features", "thumbnails", "backups"] {
            assert!(f.dest.join(sub).is_dir());
        }
        assert!(!f.dest.join("library.json.tmp").exists());
        let c = f.db();
        assert_eq!(count(&c,"SELECT count(*) FROM meta WHERE key IN ('terms_artists','terms_labels','terms_characters','terms_names') AND value='core'"),4);
        assert_eq!(
            count(
                &c,
                &format!(
                    "SELECT count(*) FROM meta WHERE key IN ('schema_version','terms_schema') AND value IN ('36','{}')",
                    crate::SCHEMA_VERSION
                )
            ),
            2
        );
        assert_eq!(
            count(
                &c,
                "SELECT count(*) FROM meta WHERE key IN ('indexed','rating_indexed','general_indexed','names_version')"
            ),
            4
        );
        assert_eq!(count(&c, "SELECT count(*) FROM meta WHERE key='identity_policy'"), 0);
        assert_eq!(count(&c, "SELECT count(*) FROM actions"), 0);
        assert_eq!(count(&c, "SELECT count(*) FROM assets"), 0);
        assert_eq!(count(&c, "SELECT count(*) FROM prefs"), 0);
        assert_eq!(count(&c, "SELECT count(*) FROM backend_effects"), 0);
        assert_eq!(
            count(&c, "SELECT count(*) FROM term_key_names WHERE kind='work' AND lang='zh' AND source='builtin'"),
            22
        );
        assert_eq!(
            count(&c, "SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name LIKE 'names_version_%'"),
            13
        );
        assert_eq!(count(&c, "SELECT count(*) FROM term_links WHERE system='ccip'"), 4);
        // 新图库导入角色时按作品文字建作品（#296）：一个文字一个待核对的作品，挂 qualifier 链接，内置的中文名一并
        // 写成作品的名字。合成的 alice_swim 没有审核词表关系，保持独立并保留所属作品。vtuber 不建作品。
        let works = |sql: &str| -> Vec<String> {
            c.prepare(sql).unwrap().query_map([], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap()
        };
        assert_eq!(
            works(
                "SELECT x.key FROM term_links x JOIN term_tree r ON r.term_id=x.term_id JOIN terms t ON t.id=x.term_id
                 WHERE x.system='qualifier' AND x.source='import' AND t.kind='work' AND r.review_status='pending'
                 ORDER BY x.key"
            ),
            ["Example", "blue_archive"]
        );
        assert_eq!(
            works(
                "SELECT l.key||'→'||coalesce(q.key,'-') FROM term_links l JOIN term_tree r ON r.term_id=l.term_id
                 LEFT JOIN terms w ON w.key=r.work_id LEFT JOIN term_links q ON q.term_id=w.id AND q.system='qualifier'
                 WHERE l.system='ccip' ORDER BY l.key"
            ),
            ["alice→Example", "alice_swim→Example", "carol_(vtuber)→-", "doodle_sensei_(blue_archive)→blue_archive"]
        );
        assert_eq!(
            works(
                "SELECT n.lang||':'||n.source||':'||n.value FROM term_names n JOIN term_links q ON q.term_id=n.term_id
                 WHERE q.system='qualifier' AND q.key='blue_archive' AND n.role='display' ORDER BY n.lang"
            ),
            ["en:index:Blue Archive", "zh:builtin:蔚蓝档案"]
        );
        assert_eq!(count(&c, "SELECT count(*) FROM term_tree WHERE parent_id IS NOT NULL AND variant='swimsuit'"), 0);
        assert_eq!(count(&c, "SELECT count(*) FROM term_tree WHERE mascot=1 AND mascot_origin='default'"), 1);
        assert_eq!(count(&c, "SELECT count(*) FROM sqlite_master WHERE name='entities'"), 0);
        assert!(crate::legacy_characters::characters_in_core(&c).unwrap());
        assert!(crate::legacy_names::names_in_core(&c).unwrap());
        // 四个域从核心开始（`new_library`）：标签表是核心之上的视图，兼容表和仅核心表有守卫，派生表已压实，
        // 以前的切换留下的三组空的全图库证据集都在。
        assert_eq!(
            count(&c, "SELECT count(*) FROM sqlite_master WHERE type='view' AND name IN ('asset_tags','general_hits','asset_rating')"),
            3
        );
        assert_eq!(count(&c, "SELECT count(*) FROM sqlite_master WHERE name='artist_verdicts'"), 0);
        assert_eq!(
            count(&c, "SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name='assertions_writer_insert'"),
            1
        );
        assert_eq!(
            count(&c, "SELECT count(*) FROM sqlite_master WHERE name='candidates' AND sql LIKE '%WITHOUT ROWID%'"),
            1
        );
        assert_eq!(
            count(&c, "SELECT count(*) FROM term_evidence_sets WHERE producer='legacy:provenance' OR producer LIKE 'migration:%'"),
            3
        );
        let src = f.source();
        for table in ["assets", "labels", "decisions", "reference_extra"] {
            assert_eq!(count(&src, &format!("SELECT count(*) FROM {table}")), 0);
        }
        assert_eq!(count(&src, "SELECT count(*) FROM characters"), 4);
        assert_eq!(count(&src, "SELECT count(*) FROM runs"), 1);
        assert_eq!(count(&src,"SELECT count(*) FROM sqlite_master WHERE type='index' AND name IN ('reference_alias_idx','reference_extra_idx')"),2);
        src.prepare("SELECT source_id,preset FROM assets").unwrap();
        src.prepare("SELECT * FROM asset_links").unwrap();
        src.prepare("SELECT * FROM files").unwrap();
        assert_eq!(before, fs::read(&f.reference).unwrap());
    }
    #[test]
    fn all_core_reads_work_on_fresh_empty_package() {
        let f = Fixture::new();
        f.create().unwrap();
        let context = crate::http::LibraryContext {
            examples: None,
            db: f.dest.join("library.sqlite"),
            generation: "synthetic".into(),
            library: Some(f.dest.clone()),
            source: Some(f.dest.join("source.sqlite")),
            tags: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tags/tags_v3.csv"),
            token: Some("synthetic-token".into()),
            read_only: None,
        };
        let id: String = f
            .db()
            .query_row(
                "SELECT t.key FROM term_links x JOIN terms t ON t.id=x.term_id WHERE x.system='ccip' AND x.key='alice'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        for route in crate::http_read::CORE_ROUTES {
            let target = match *route {
                "/api/character/profile" => format!("{route}?entity={id}"),
                "/api/character/stats" | crate::http_characters::TOGETHER => format!("{route}?id={id}"),
                "/api/galleries/covers" => format!("{route}?ids=%5B%5D"),
                "/api/calendar" => format!("{route}?month=2026-01"),
                _ => (*route).to_owned(),
            };
            let out = crate::http_read::read(&context, &target).unwrap_or_else(|e| panic!("{route}: {e}"));
            assert!(out.is_some(), "{route}");
            let expected = if *route == "/api/detail" { 400 } else { 200 };
            assert_eq!(out.unwrap().1, expected, "{route}");
        }
    }
    #[test]
    fn explicit_preset_and_name_are_retained() {
        let f = Fixture::new();
        create_folder(&f.dest, &f.folder, Some("合成新图库"), Some("x"), Some(&f.reference)).unwrap();
        let config: Value = serde_json::from_slice(&fs::read(f.dest.join("library.json")).unwrap()).unwrap();
        assert_eq!(config["name"], "合成新图库");
        assert_eq!(config["sources"][0]["preset"], "x");
        assert_eq!(config["sources"][0]["preset_origin"], "user");
    }
    #[test]
    fn unicode_preset_nullable_reference_and_nested_destination_match_existing_contract() {
        let mut f = Fixture::new();
        f.dest = f.root.join("new/parent/nested.pilib");
        let c = crate::sqlite::open(&f.reference).unwrap();
        c.execute("INSERT INTO character_aliases VALUES (NULL,NULL,NULL)", []).unwrap();
        c.execute("UPDATE characters SET danbooru_tag=NULL WHERE char_id='alice'", []).unwrap();
        drop(c);
        f.create().unwrap();
        let bytes = fs::read_to_string(f.dest.join("library.json")).unwrap();
        assert!(bytes.starts_with("{\n  \"format\": 1,\n  \"id\":"));
        assert!(f.dest.join("library.sqlite").is_file());
    }
    #[test]
    fn existing_directory_and_file_are_never_cleaned_or_overwritten() {
        for directory in [true, false] {
            let f = Fixture::new();
            if directory {
                fs::create_dir(&f.dest).unwrap();
                fs::write(f.dest.join("keep"), b"existing").unwrap();
            } else {
                fs::write(&f.dest, b"existing").unwrap();
            }
            assert!(f.create().is_err());
            assert_eq!(fs::read(if directory { f.dest.join("keep") } else { f.dest.clone() }).unwrap(), b"existing");
        }
    }
    #[cfg(unix)]
    #[test]
    fn dangling_or_existing_symlink_destination_is_never_touched() {
        for dangling in [true, false] {
            let f = Fixture::new();
            let target = f.root.join("symlink-target");
            if !dangling {
                fs::create_dir(&target).unwrap();
                fs::write(target.join("keep"), b"existing").unwrap();
            }
            std::os::unix::fs::symlink(&target, &f.dest).unwrap();
            assert!(f.create().is_err());
            assert_eq!(fs::read_link(&f.dest).unwrap(), target);
            if !dangling {
                assert_eq!(fs::read(target.join("keep")).unwrap(), b"existing");
            }
        }
    }
    #[test]
    fn bad_input_or_reference_failure_preserves_source_folder_and_removes_only_new_package() {
        let f = Fixture::new();
        fs::write(f.folder.join("keep"), b"source").unwrap();
        assert!(create_folder(&f.dest, &f.folder, None, Some("bad"), Some(&f.reference)).is_err());
        assert!(!f.dest.exists());
        assert!(create_folder(&f.dest, &f.folder, None, None, Some(&f.root.join("missing.sqlite"))).is_err());
        assert!(!f.dest.exists());
        let invalid = f.root.join("invalid.sqlite");
        fs::write(&invalid, b"not sqlite").unwrap();
        assert!(create_folder(&f.dest, &f.folder, None, None, Some(&invalid)).is_err());
        assert!(!f.dest.exists());
        assert_eq!(fs::read(invalid).unwrap(), b"not sqlite");
        let incomplete = f.root.join("incomplete.sqlite");
        let c = crate::sqlite::open(&incomplete).unwrap();
        c.execute_batch("CREATE TABLE assets(uuid TEXT);").unwrap();
        drop(c);
        let before = fs::read(&incomplete).unwrap();
        assert!(create_folder(&f.dest, &f.folder, None, None, Some(&incomplete)).is_err());
        assert!(!f.dest.exists());
        assert_eq!(fs::read(incomplete).unwrap(), before);
        assert_eq!(fs::read(f.folder.join("keep")).unwrap(), b"source");
    }
    #[test]
    fn failure_after_the_core_switch_does_not_publish_config_and_cleans_owned_databases() {
        let f = Fixture::new();
        let before = fs::read(&f.reference).unwrap();
        let result = create(&f.dest, &f.folder, None, None, Some(&f.reference), |c| {
            assert!(!f.dest.join("library.json").exists());
            assert_eq!(count(c,"SELECT count(*) FROM meta WHERE key IN ('terms_artists','terms_labels','terms_characters','terms_names') AND value='core'"),4);
            // 新图库从核心开始：切换前没有要备份的东西。
            assert!(!f.dest.join("library-backups").exists());
            c.execute_batch(
                "CREATE TRIGGER injected BEFORE INSERT ON terms BEGIN SELECT RAISE(ABORT,'injected'); END;",
            )?;
            Ok(())
        });
        assert!(result.is_err());
        assert!(!f.dest.exists());
        assert_eq!(before, fs::read(&f.reference).unwrap());
        assert!(f.folder.is_dir());
    }
    #[test]
    fn active_wal_reference_is_read_without_altering_committed_input() {
        let f = Fixture::new();
        let c = crate::sqlite::open(&f.reference).unwrap();
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; INSERT INTO character_aliases VALUES ('alice','wal alias','index');").unwrap();
        let wal = PathBuf::from(format!("{}-wal", f.reference.display()));
        let before = fs::read(&wal).unwrap();
        f.create().unwrap();
        assert_eq!(count(&f.source(), "SELECT count(*) FROM character_aliases"), 2);
        assert_eq!(before, fs::read(wal).unwrap());
    }
}
