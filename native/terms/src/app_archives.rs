//! 图库包归档和会话持有的导出。只删除明确新建的暂存路径。
use crate::{app_library::Library, http_json::Json, Error, Result};
use rusqlite::{
    backup::{Backup, StepResult},
    Connection, OpenFlags,
};
use std::collections::HashMap;
use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};
const EXPORTED: &[&str] = &[
    "library.json",
    "source.sqlite",
    "library.sqlite",
    "features/full-features.npy",
    "features/full-manifest.csv",
    "album-char-map.json",
];
fn invalid(e: impl std::fmt::Display) -> Error {
    Error::Invalid(e.to_string())
}
// 旧版 library_action 会捕获 OSError/ValueError，但 BadZipFile 和解压失败会落到 HTTP 500。
// 保留这些状态码，不依赖异常的文字。
fn zip_failure(error: zip::result::ZipError) -> Error {
    match error {
        zip::result::ZipError::Io(error) => invalid(error),
        other => Error::Io(std::io::Error::other(other)),
    }
}
fn zip_read_failure(error: std::io::Error) -> Error {
    if matches!(error.kind(), std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof) {
        Error::Io(error)
    } else {
        invalid(error)
    }
}
fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) {
        Err(Error::Invalid("导出已取消".into()))
    } else {
        Ok(())
    }
}

fn export_filename(name: &str, date: &str) -> String {
    let suffix = format!("-{date}.pilib.zip");
    // 给文件系统的单个分量留出后缀和可能的下划线。按 UTF-8 字节限制也能满足 NTFS 的 UTF-16 长度限制。
    let max_stem_bytes = 255 - suffix.len() - 1;
    let mut stem = String::new();
    for c in name.chars().filter(|c| !c.is_control() && !"<>:\"/\\|?*".contains(*c)) {
        if stem.len() + c.len_utf8() > max_stem_bytes {
            break;
        }
        stem.push(c);
    }
    let stem = stem.trim_start_matches(' ');
    let stem = if stem.trim_matches(['.', ' ']).is_empty() { "图库" } else { stem };
    let mut filename = format!("{stem}{suffix}");
    // Win32 也把带扩展名的保留名当作设备。检查加完后缀的文件名，例如 CON.txt-日期.pilib.zip。
    let base = filename.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    if matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || base
            .strip_prefix("COM")
            .or_else(|| base.strip_prefix("LPT"))
            .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"))
    {
        filename.insert(0, '_');
    }
    filename
}

fn import_cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) || crate::platform_process::stopping() {
        Err(Error::Conflict("程序正在退出，导入已取消".into()))
    } else {
        Ok(())
    }
}
/// 配置读进内存解析；真实的配置只有几 KB。
const CONFIG_LIMIT: u64 = 1 << 20;
/// 解包后留出的空闲空间，导入不会占满图库所在的磁盘。
const SPACE_MARGIN: u64 = 256 << 20;
fn random() -> Result<String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(invalid)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
struct Stage(PathBuf, bool);
impl Drop for Stage {
    fn drop(&mut self) {
        if !self.1 {
            if let Err(error) = fs::remove_dir_all(&self.0) {
                eprintln!("归档临时目录清理失败 {}: {error}", self.0.display());
            }
        }
    }
}
fn stage(parent: &Path) -> Result<Stage> {
    fs::create_dir_all(parent)?;
    let path = parent.join(format!(".illustash-export-{}", random()?));
    fs::create_dir(&path)?;
    Ok(Stage(path, false))
}
fn copy_cancelled(mut from: impl Read, mut to: impl Write, cancel: &AtomicBool) -> Result<()> {
    let mut bytes = [0u8; 65536];
    loop {
        cancelled(cancel)?;
        let n = from.read(&mut bytes)?;
        if n == 0 {
            return Ok(());
        }
        to.write_all(&bytes[..n])?;
    }
}
/// 只对导出快照或新解开的图库包调用，从不对发送方自己的数据库调用。
/// 模型连接设置（含将来的密钥）、长期联网同意和首次引导记录（回答过哪些问题、勾过哪些项：#116）属于个人数据。
fn sanitize_database(c: &mut Connection, cancel: Option<Arc<AtomicBool>>) -> Result<()> {
    // 大图库导出时压实可能占大部分时间。切换图库、关闭应用时，除了流式复制和备份，也必须能中断这些 SQLite 操作。
    c.progress_handler(1000, cancel.map(|cancel| move || cancel.load(Ordering::SeqCst)))?;
    c.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA secure_delete=ON;")?;
    let tx = c.transaction()?;
    for (table, sql) in [
        ("name_settings", "DELETE FROM name_settings"),
        ("prefs", "DELETE FROM prefs WHERE key IN ('names_network','onboarding')"),
    ] {
        if tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?)", [table], |r| {
            r.get::<_, bool>(0)
        })? {
            tx.execute(sql, [])?;
        }
    }
    // VACUUM 清掉旧写入在 secure_delete 关闭时留在空闲页里的机密。
    // SQLite 只会改写无索引表的隐式 rowid。临时索引保住这些身份（含历史表和用户表），且不改变最终的 schema。
    let tables = tx
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let mut indexes = Vec::new();
    for table in tables {
        let table = quote(&table);
        if tx.prepare(&format!("PRAGMA index_list({table})"))?.exists([])? {
            continue;
        }
        let column: String = tx.query_row(&format!("PRAGMA table_info({table})"), [], |r| r.get(1))?;
        let index = quote(&format!("archive_rowid_{}", random()?));
        tx.execute_batch(&format!("CREATE INDEX {index} ON {table}({})", quote(&column)))?;
        indexes.push(index);
    }
    tx.commit()?;
    c.execute_batch("VACUUM")?;
    let tx = c.transaction()?;
    for index in indexes {
        tx.execute_batch(&format!("DROP INDEX {index}"))?;
    }
    tx.commit()?;
    Ok(())
}
pub(crate) fn export(library: &Library, dest: &Path, cancel: &Arc<AtomicBool>) -> Result<PathBuf> {
    let work = stage(dest.parent().ok_or_else(|| invalid("无效导出路径"))?)?;
    let archive = work.0.join("archive.zip");
    // 先复制数据库，作为一致的一组；之后压缩时不持有任何数据库。
    let copies = snapshot(library, &work.0, cancel)?;
    if let Some(copy) = copies.get("library.sqlite") {
        let mut c = crate::sqlite::open_with_flags(copy, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        sanitize_database(&mut c, Some(cancel.clone()))?;
        cancelled(cancel)?;
    }
    let mut zip = ZipWriter::new(BufWriter::with_capacity(
        1 << 20,
        OpenOptions::new().write(true).create_new(true).open(&archive)?,
    ));
    for rel in EXPORTED {
        cancelled(cancel)?;
        let source = match copies.get(rel) {
            Some(copy) => copy.clone(),
            None => {
                let src = library.instance_path().join(rel);
                if rel.ends_with(".sqlite") || !src.try_exists()? {
                    continue;
                }
                src
            }
        };
        zip.start_file(
            *rel,
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(true),
        )
        .map_err(invalid)?;
        copy_cancelled(File::open(source)?, &mut zip, cancel)?;
    }
    let mut file = zip.finish().map_err(invalid)?;
    file.flush()?;
    drop(file);
    cancelled(cancel)?;
    fs::rename(&archive, dest)?;
    Ok(dest.to_owned())
}
/// 一起属于这个图库包的几份数据库副本：每份都重新复制，直到第一份和最后一份之间没有任何数据库提交过（SQLite
/// data_version），这样归档不会把跨库更新之前的 source 和更新之后的应用数据库配在一起。
fn snapshot(library: &Library, work: &Path, cancel: &AtomicBool) -> Result<HashMap<&'static str, PathBuf>> {
    let mut readers = Vec::new();
    for rel in EXPORTED.iter().filter(|rel| rel.ends_with(".sqlite")) {
        let src = library.instance_path().join(rel);
        if src.try_exists()? {
            let c = crate::app_library::read_connection(&src)?;
            c.busy_timeout(Duration::from_millis(50))?;
            readers.push((*rel, c, work.join(rel)));
        }
    }
    let versions = |readers: &[(&str, rusqlite::Connection, PathBuf)]| -> Result<Vec<i64>> {
        readers.iter().map(|(_, c, _)| Ok(c.query_row("PRAGMA data_version", [], |r| r.get(0))?)).collect()
    };
    for _ in 0..SNAPSHOT_ATTEMPTS {
        cancelled(cancel)?;
        let attempt = || -> Result<bool> {
            let before = versions(&readers)?;
            for (_, c, copy) in &readers {
                if copy.try_exists()? {
                    fs::remove_file(copy)?;
                }
                backup_into(c, copy, cancel)?;
            }
            Ok(versions(&readers)? == before)
        };
        match attempt() {
            Ok(true) => return Ok(readers.into_iter().map(|(rel, _, copy)| (rel, copy)).collect()),
            Ok(false) => {}
            // 写入者也会短暂挡住 data_version；与版本变化一样，重做整组快照。
            Err(Error::Sql(rusqlite::Error::SqliteFailure(error, _)))
                if error.code == rusqlite::ErrorCode::DatabaseBusy => {}
            Err(error) => return Err(error),
        }
    }
    Err(invalid("导出期间图库一直在写入，请稍后再导出"))
}
/// 重新做快照比压缩便宜；一直在变的图库就报告出来。
const SNAPSHOT_ATTEMPTS: usize = 5;
fn backup_into(source: &rusqlite::Connection, copy: &Path, cancel: &AtomicBool) -> Result<()> {
    let mut b = crate::sqlite::open(copy)?;
    let backup = Backup::new(source, &mut b)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        cancelled(cancel)?;
        match backup.step(256)? {
            StepResult::Done => return Ok(()),
            StepResult::More => {}
            _ if std::time::Instant::now() >= deadline => return Err(invalid("数据库备份超时")),
            _ => thread::sleep(Duration::from_millis(10)),
        }
    }
}
struct CheckedWriter<'a, W> {
    output: W,
    crc: crc32fast::Hasher,
    bytes: u64,
    cancel: &'a AtomicBool,
    written: &'a dyn Fn(u64),
}
impl<W: Write> Write for CheckedWriter<'_, W> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::SeqCst) || crate::platform_process::stopping() {
            return Err(std::io::Error::other("cancelled"));
        }
        let n = self.output.write(data)?;
        self.crc.update(&data[..n]);
        self.bytes += n as u64;
        (self.written)(n as u64);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}
/// 流式解出一项，写入量不超过它声明的大小（导入的空间检查依赖这一点）。
/// `written` 收到每个写出的块，用于页面轮询的进度。
fn unpack<W: Write>(
    zip: &mut ZipArchive<BufReader<File>>,
    name: &str,
    mut output: W,
    buffer: &mut [u8],
    cancel: &AtomicBool,
    written: &dyn Fn(u64),
) -> Result<()> {
    let index = zip.index_for_name(name).ok_or_else(|| invalid("缺少归档内容"))?;
    let mut raw = zip.by_index_raw(index).map_err(zip_failure)?;
    if raw.compression() == zip::CompressionMethod::LZMA {
        // ZIP-LZMA 带版本和属性长度前缀，没有 .lzma 的解压后大小头。
        // 只改写这个头；解码器直接流式写到目标，并检查大小和 CRC。
        let (size, crc) = (raw.size(), raw.crc32());
        let mut prefix = [0; 4];
        raw.read_exact(&mut prefix).map_err(zip_read_failure)?;
        if u16::from_le_bytes([prefix[2], prefix[3]]) != 5 {
            return Err(Error::Io(std::io::Error::other("无效 ZIP LZMA 属性")));
        }
        let mut header = [0; 13];
        raw.read_exact(&mut header[..5]).map_err(zip_read_failure)?;
        header[5..].copy_from_slice(&size.to_le_bytes());
        let mut input = BufReader::with_capacity(65536, std::io::Cursor::new(header).chain(raw));
        let mut checked = CheckedWriter { output, crc: crc32fast::Hasher::new(), bytes: 0, cancel, written };
        let decoded = lzma_rs::lzma_decompress(&mut input, &mut checked);
        import_cancelled(cancel)?;
        decoded.map_err(|error| Error::Io(std::io::Error::other(error)))?;
        if checked.bytes != size || checked.crc.finalize() != crc {
            return Err(Error::Io(std::io::Error::other("Invalid checksum")));
        }
    } else {
        let size = raw.size();
        drop(raw);
        let mut source = zip.by_name(name).map_err(zip_failure)?;
        let mut emitted = 0u64;
        loop {
            import_cancelled(cancel)?;
            let n = source.read(buffer).map_err(zip_read_failure)?;
            if n == 0 {
                break;
            }
            emitted += n as u64;
            if emitted > size {
                return Err(Error::Io(std::io::Error::other("归档内容比声明的大小更大")));
            }
            output.write_all(&buffer[..n]).map_err(invalid)?;
            written(n as u64);
        }
    }
    Ok(())
}
/// 作为后台任务运行（#88）：取消按钮、切换图库或退出时设置 `cancel`，在发布之前停下的导入会删掉它解出的内容。
/// `observer` 收到各阶段和已解出的字节数；新目录只有走过发布点才保留。
pub(crate) fn import(
    archive: &Path,
    cancel: &Arc<AtomicBool>,
    observer: &dyn crate::app_imports::Observer,
) -> Result<(PathBuf, String)> {
    observer.stage("reading");
    let mut zip = ZipArchive::new(BufReader::with_capacity(1 << 20, File::open(archive).map_err(invalid)?))
        .map_err(zip_failure)?;
    let mut names = std::collections::BTreeSet::new();
    let mut declared = 0u64;
    for i in 0..zip.len() {
        let file = zip.by_index_raw(i).map_err(zip_failure)?;
        let name = file.name().map_err(zip_failure)?;
        let name = name.as_ref();
        // ZIP 只用 `/` 分隔；按原始字节拒绝反斜杠，不依赖各平台对 `features\x` 的解释。
        if file.name_raw().contains(&b'\\')
            || !EXPORTED.contains(&name)
            || file.enclosed_name().as_deref() != Some(Path::new(name))
            || file.is_dir()
        {
            return Err(invalid("导出文件里有不认识的内容，已拒绝导入"));
        }
        // 归档索引按名称做键，所以条目最多就是导出的那一组。
        names.insert(name.to_owned());
        if name == "library.json" && file.size() > CONFIG_LIMIT {
            return Err(invalid("导出文件里的图库配置过大，已拒绝导入"));
        }
        declared = declared.saturating_add(file.size());
    }
    if !names.contains("library.json") || !names.contains("source.sqlite") {
        return Err(invalid("不是导出的图库（缺少 library.json 或 source.sqlite）"));
    }
    // 只把很小的配置读进缓冲；数据库和 features 的内容流式解出并校验 CRC。
    let mut buffer = vec![0u8; 1 << 20];
    let config_text = {
        let mut bytes = Vec::new();
        unpack(&mut zip, "library.json", &mut bytes, &mut buffer, cancel, &|_| ())?;
        String::from_utf8(bytes).map_err(invalid)?
    };
    let value: serde_json::Value = serde_json::from_str(&config_text).map_err(invalid)?;
    if !value.is_object() {
        return Err(Error::Io(std::io::Error::other("无效图库配置")));
    }
    let mut config = crate::app_settings::config_from_text(&config_text)?;
    let fmt = config.clone().response().get("format").cloned().ok_or_else(|| invalid("无效图库格式"))?;
    let valid = match &fmt {
        Json::Integer(v) => v.parse::<i64>().is_ok_and(|v| v <= crate::app_library::FORMAT),
        Json::Scalar(serde_json::Value::Number(v)) => v.as_i64().is_some_and(|v| v <= crate::app_library::FORMAT),
        Json::Scalar(serde_json::Value::Bool(_)) => true,
        _ => false,
    };
    if !valid {
        return Err(invalid("图库格式需要升级程序"));
    }
    let name = config
        .clone()
        .response()
        .get("name")
        .map(|v| v.text().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| archive.file_stem().unwrap_or_default().to_string_lossy().into_owned());
    let path = crate::app_libraries::unique_path(&name)?;
    fs::create_dir_all(path.parent().expect("internal file path must have a parent")).map_err(invalid)?;
    // 解包最多写入声明的大小；library.sqlite 压实可能还要同样多。
    let library_size = zip.index_for_name("library.sqlite").map_or(Ok(0), |i| zip.by_index_raw(i).map(|f| f.size()));
    let needed = declared.saturating_add(library_size.map_err(zip_failure)?).saturating_add(SPACE_MARGIN);
    let available =
        crate::platform_filesystem::available_space(path.parent().expect("internal file path must have a parent"))
            .map_err(invalid)?;
    if needed > available {
        return Err(invalid(format!(
            "磁盘空间不足：导入需要约 {needed} MiB，可用 {available} MiB",
            needed = needed.div_ceil(1 << 20),
            available = available >> 20
        )));
    }
    import_cancelled(cancel)?;
    fs::create_dir(&path).map_err(invalid)?;
    let mut cleanup = Stage(path.clone(), false);
    observer.stage("unpacking");
    observer.count(0, declared);
    let unpacked = std::cell::Cell::new(0u64);
    let written = |n: u64| {
        unpacked.set(unpacked.get() + n);
        observer.count(unpacked.get(), declared);
    };
    for name in names {
        let out = path.join(&name);
        fs::create_dir_all(out.parent().expect("internal file path must have a parent")).map_err(invalid)?;
        let mut file = BufWriter::with_capacity(
            1 << 20,
            OpenOptions::new().write(true).create_new(true).open(out).map_err(invalid)?,
        );
        unpack(&mut zip, &name, &mut file, &mut buffer, cancel, &written)?;
        file.flush().map_err(invalid)?;
    }
    // 旧的导出仍可能含有凭据和发送方的长期同意。
    // 在发布或登记这个新建的图库包之前去掉它们。
    let database = path.join("library.sqlite");
    observer.stage("cleaning");
    if database.try_exists()? {
        let sanitize = || {
            let mut c = crate::sqlite::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
            crate::sqlite::protect_archive(&c)?;
            sanitize_database(&mut c, Some(cancel.clone()))
        };
        sanitize().map_err(|e| match import_cancelled(cancel) {
            Err(stop) => stop,
            Ok(()) => e,
        })?;
    }
    for sub in ["features", "thumbnails", "backups"] {
        fs::create_dir_all(path.join(sub)).map_err(invalid)?;
    }
    config.set(
        "imported_from".into(),
        crate::app_settings::SettingValue::Scalar(
            archive.file_name().unwrap_or_default().to_string_lossy().into_owned().into(),
        ),
    );
    crate::app_settings::write_config(&path, &config).map_err(invalid)?;
    observer.stage("checking");
    // 只发布能打开的图库包：数据库损坏或配置不完整在这里就拒绝并删掉新目录，不登记一个随后打不开的图库。
    if fmt_is_current(&fmt) {
        Library::check_import(&path).map_err(|e| invalid(format!("导入的图库无法打开，已取消导入：{e}")))?;
    }
    import_cancelled(cancel)?;
    observer.stage("publishing");
    // 发布点：先到的取消让新目录不进图库；发布期间到的取消会等它完成，导入随后算作完成。
    observer.publish(&mut || {
        import_cancelled(cancel)?;
        cleanup.1 = true;
        Ok(())
    })?;
    Ok((path, name))
}
/// 归档声明的是当前图库包格式（旧格式仍走它们各自的显式升级）。
fn fmt_is_current(fmt: &Json) -> bool {
    match fmt {
        Json::Integer(v) => v.parse::<i64>().is_ok_and(|v| v == crate::app_library::FORMAT),
        Json::Scalar(serde_json::Value::Number(v)) => v.as_i64() == Some(crate::app_library::FORMAT),
        _ => false,
    }
}
struct Task {
    state: Arc<Mutex<Json>>,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    instance: PathBuf,
    generation: u64,
}
#[derive(Default)]
pub(crate) struct Exports(Mutex<Option<Task>>, AtomicBool);
impl Exports {
    pub(crate) fn running(&self) -> bool {
        self.0
            .lock()
            .expect("app_archives mutex poisoned")
            .as_ref()
            .is_some_and(|task| task.handle.as_ref().is_some_and(|handle| !handle.is_finished()))
    }

    pub(crate) fn state(&self, library: &Library, generation: u64) -> Json {
        let tasks = self.0.lock().expect("app_archives mutex poisoned");
        tasks
            .as_ref()
            .filter(|t| t.instance == library.instance_path() && t.generation == generation)
            .map(|t| t.state.lock().expect("app_archives mutex poisoned").clone())
            .unwrap_or_else(|| Json::object([("status", "idle".into())]))
    }
    pub(crate) fn start(&self, library: &Library, generation: u64) -> Result<Json> {
        let mut tasks = self.0.lock().expect("app_archives mutex poisoned");
        if self.1.load(Ordering::SeqCst) {
            return Err(invalid("服务正在退出"));
        }
        if tasks.as_ref().is_some_and(|t| t.handle.as_ref().is_some_and(|h| !h.is_finished())) {
            return Err(invalid("正在导出"));
        }
        if let Some(mut old) = tasks.take() {
            if let Some(handle) = old.handle.take() {
                let _ = handle.join();
            }
        }
        let date: String =
            Connection::open_in_memory()?.query_row("SELECT strftime('%Y%m%d','now','localtime')", [], |r| r.get(0))?;
        let dest = crate::app_paths::user_home()?.join("Downloads").join(export_filename(library.name(), &date));
        let initial = Json::object([("status", "running".into()), ("path", dest.to_string_lossy().as_ref().into())]);
        let state = Arc::new(Mutex::new(initial.clone()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (s, c, lib) = (state.clone(), cancel.clone(), library.clone());
        let handle = thread::Builder::new().name("library-export".into()).spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| export(&lib, &dest, &c)));
            let result = result.unwrap_or_else(|_| Err(invalid("导出任务异常退出")));
            *s.lock().expect("app_archives mutex poisoned") = match result {
                Ok(path) => Json::object([
                    ("status", "done".into()),
                    ("path", path.to_string_lossy().as_ref().into()),
                    ("bytes", serde_json::Value::from(fs::metadata(&path).map(|m| m.len()).unwrap_or(0)).into()),
                ]),
                Err(e) => {
                    let d = crate::diagnostics::failure(
                        crate::diagnostics::Operation::Export,
                        crate::diagnostics::Stage::Background,
                        &e,
                        crate::diagnostics::Publication::Unknown,
                    );
                    Json::object([
                        ("status", "error".into()),
                        ("path", dest.to_string_lossy().as_ref().into()),
                        ("error", e.reason().chars().take(200).collect::<String>().into()),
                        ("diagnostic", d.value().into()),
                    ])
                }
            };
        })?;
        *tasks = Some(Task {
            state,
            cancel,
            handle: Some(handle),
            instance: library.instance_path().to_owned(),
            generation,
        });
        Ok(initial)
    }
    pub(crate) fn shutdown(&self) {
        self.1.store(true, Ordering::SeqCst);
        self.stop();
    }
    pub(crate) fn stop(&self) {
        if let Some(mut task) = self.0.lock().expect("app_archives mutex poisoned").take() {
            task.cancel.store(true, Ordering::SeqCst);
            if let Some(h) = task.handle.take() {
                let _ = h.join();
            }
        }
    }
}
impl Drop for Exports {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn export_names_are_single_portable_filename_components() {
        use std::path::Component;

        for (name, stem) in [
            ("春日插画", "春日插画"),
            (".", "图库"),
            ("..", "图库"),
            ("../escaped", "..escaped"),
            ("category/subset", "categorysubset"),
            (r"category\subset", "categorysubset"),
            ("/absolute/path", "absolutepath"),
            (r"C:\absolute\path", "Cabsolutepath"),
            (r"C:relative", "Crelative"),
            (r"\\server\share\path", "serversharepath"),
            (r"\\?\UNC\server\share\path", "UNCserversharepath"),
            (" /\\:\0*\n", "图库"),
            ("NUL.notes", "_NUL.notes"),
            ("com¹.txt", "_com¹.txt"),
            ("CON", "CON"),
        ] {
            let filename = export_filename(name, "20261114");
            assert_eq!(filename, format!("{stem}-20261114.pilib.zip"), "{name:?}");
            let mut components = Path::new(&filename).components();
            assert!(matches!(components.next(), Some(Component::Normal(part)) if part == filename.as_str()));
            assert_eq!(components.next(), None);
        }
        let filename = export_filename(&"彩图".repeat(256), "20261114");
        assert!(filename.len() <= 255);
        assert!(filename.ends_with("-20261114.pilib.zip"));
    }

    #[test]
    fn export_keeps_the_display_name_and_publishes_inside_downloads() {
        let work = stage(&std::env::temp_dir()).unwrap();
        fs::create_dir(work.0.join("folder")).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "app_archives::tests::export_name_fixture"])
            .env("ILLUSTASH_ARCHIVE_FIXTURE", &work.0)
            .env("HOME", &work.0)
            .env("USERPROFILE", &work.0)
            .env("ILLUSTASH_APP_SUPPORT", work.0.join("support"))
            .env("ILLUSTASH_SHARED", work.0.join("absent-reference"))
            .env("ILLUSTASH_NO_KEYCHAIN", "1")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    #[ignore = "由隔离的导出路径回归测试运行"]
    fn export_name_fixture() {
        let root = PathBuf::from(std::env::var_os("ILLUSTASH_ARCHIVE_FIXTURE").expect("isolated fixture only"));
        let package = root.join("synthetic.pilib");
        let name = "../escaped";
        crate::app_create::create_folder(&package, &root.join("folder"), Some(name), None, None).unwrap();
        let library = Library::open(&package).unwrap();
        let downloads = root.join("Downloads");
        fs::create_dir(&downloads).unwrap();
        let date: String = Connection::open_in_memory()
            .unwrap()
            .query_row("SELECT strftime('%Y%m%d','now','localtime')", [], |row| row.get(0))
            .unwrap();
        let outside = root.join(format!("escaped-{date}.pilib.zip"));
        fs::write(&outside, b"outside sentinel").unwrap();
        let exports = Exports::default();
        let initial = exports.start(&library, 1).unwrap();
        exports.0.lock().unwrap().as_mut().unwrap().handle.take().unwrap().join().unwrap();
        let dest = PathBuf::from(initial.get("path").unwrap().text());
        assert_eq!(dest.parent(), Some(downloads.as_path()));
        assert_eq!(exports.state(&library, 1).get("status").unwrap().text(), "done");
        let mut archive = ZipArchive::new(File::open(&dest).unwrap()).unwrap();
        let config: serde_json::Value = serde_json::from_reader(archive.by_name("library.json").unwrap()).unwrap();
        assert_eq!(config["name"], name);
        assert_eq!(library.name(), name);
        drop(archive);

        let published = fs::read(&dest).unwrap();
        let cancel = Arc::new(AtomicBool::new(true));
        assert!(export(&library, &dest, &cancel).is_err());
        assert_eq!(fs::read(&dest).unwrap(), published);
        assert_eq!(fs::read(&outside).unwrap(), b"outside sentinel");
        assert_eq!(fs::read_dir(downloads).unwrap().count(), 1);
    }

    #[test]
    fn gallery_roundtrips_in_exported_library() {
        let work = Stage(
            std::env::temp_dir().join(format!("illustash-gallery-archive-{}", crate::store::new_key().unwrap())),
            false,
        );
        fs::create_dir_all(work.0.join("folder")).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "app_archives::tests::gallery_archive_fixture"])
            .env("ILLUSTASH_ARCHIVE_FIXTURE", &work.0)
            .env("HOME", &work.0)
            .env("USERPROFILE", &work.0)
            .env("ILLUSTASH_APP_SUPPORT", work.0.join("support"))
            .env("ILLUSTASH_SHARED", work.0.join("absent-reference"))
            .env("ILLUSTASH_NO_KEYCHAIN", "1")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    #[ignore = "只在隔离的画廊归档测试中运行"]
    fn gallery_archive_fixture() {
        let root = PathBuf::from(std::env::var_os("ILLUSTASH_ARCHIVE_FIXTURE").expect("隔离测试目录"));
        let package = root.join("synthetic.pilib");
        crate::app_create::create_folder(&package, &root.join("folder"), Some("合成原图库"), None, None).unwrap();
        let library = Library::open(&package).unwrap();
        let params = r#"{"has":"[\"fav\"]","order":"oldest"}"#;
        crate::open(&library.db())
            .unwrap()
            .execute(
                "INSERT INTO galleries(id,name,params,cover,position,created_at,updated_at)
             VALUES (12,'合成收藏',?,'synthetic-cover',2,'2026-11-01 00:00:00','2026-11-02 00:00:00')",
                [params],
            )
            .unwrap();
        crate::open(&library.db())
            .unwrap()
            .execute_batch(
                "INSERT INTO galleries VALUES (13,'合成手选','{}','synthetic-b',3,'2026-11-01','2026-11-02','manual');
                 INSERT INTO gallery_members VALUES (13,'synthetic-b'),(13,'synthetic-removed');",
            )
            .unwrap();
        // 相似组的「看过了」（#427）同样随归档走。
        crate::open(&library.db())
            .unwrap()
            .execute("INSERT INTO similar_seen VALUES (3,'synthetic-a','2026-11-03 00:00:00')", [])
            .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let archive = export(&library, &root.join("export.pilib.zip"), &cancel).unwrap();
        let (restored, _) = import(&archive, &cancel, &crate::app_imports::Silent).unwrap();
        let c = crate::open(&restored.join("library.sqlite")).unwrap();
        let result: (i64, String, String, String, i64, String, String) = c
            .query_row(
                "SELECT id,name,params,cover,position,created_at,updated_at FROM galleries WHERE kind='condition'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
            )
            .unwrap();
        assert_eq!(
            result,
            (
                12,
                "合成收藏".into(),
                params.into(),
                "synthetic-cover".into(),
                2,
                "2026-11-01 00:00:00".into(),
                "2026-11-02 00:00:00".into()
            )
        );
        let restored_manual: (String, String, i64) = c
            .query_row("SELECT name,cover,position FROM galleries WHERE id=13 AND kind='manual'", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(restored_manual, ("合成手选".into(), "synthetic-b".into(), 3));
        let members = c
            .prepare("SELECT uuid FROM gallery_members WHERE gallery_id=13 ORDER BY uuid")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(members, ["synthetic-b", "synthetic-removed"]);
        let seen: (i64, String, String) = c
            .query_row("SELECT id,uuid,seen_at FROM similar_seen", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        assert_eq!(seen, (3, "synthetic-a".into(), "2026-11-03 00:00:00".into()));
    }

    #[test]
    fn exported_databases_are_one_snapshot_while_both_keep_changing() {
        let root =
            std::env::temp_dir().join(format!("illustash-archive-snapshot-{}", crate::store::new_key().unwrap()));
        fs::create_dir_all(root.join("folder")).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "app_archives::tests::snapshot_fixture"])
            .env("ILLUSTASH_ARCHIVE_FIXTURE", &root)
            .env("ILLUSTASH_APP_SUPPORT", root.join("support"))
            .env("ILLUSTASH_SHARED", root.join("absent-reference"))
            .env("ILLUSTASH_NO_KEYCHAIN", "1")
            .status()
            .unwrap();
        fs::remove_dir_all(&root).unwrap();
        assert!(status.success());
    }
    #[test]
    #[ignore = "isolated fixture, run with synthetic support by the parent test"]
    fn snapshot_fixture() {
        // #23：写入方像同步那样，每次更新先提交 source，再提交应用数据库。
        let root = PathBuf::from(std::env::var_os("ILLUSTASH_ARCHIVE_FIXTURE").expect("isolated fixture only"));
        let package = root.join("synthetic.pilib");
        crate::app_create::create_folder(&package, &root.join("folder"), None, None, None).unwrap();
        let library = Library::open(&package).unwrap();
        for db in [library.source(), library.db()] {
            crate::sqlite::open(&db)
                .unwrap()
                .execute_batch(
                    "CREATE TABLE epoch(n INTEGER); INSERT INTO epoch VALUES (0); CREATE TABLE bulk(x BLOB);
                     WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<1500) INSERT INTO bulk SELECT randomblob(4096) FROM n;",
                )
                .unwrap();
        }
        // 用真实排他锁稳定覆盖版本读取的 BUSY，不依赖机器负载碰中竞态。
        // 两份数据库都检查：重试耗尽时不发布部分归档，解除锁后继续下面的一致性回归。
        let locked_dest = root.join("locked-export.zip");
        for db in [library.source(), library.db()] {
            let lock = crate::sqlite::open(db).unwrap();
            let mode: String = lock.query_row("PRAGMA journal_mode", [], |row| row.get(0)).unwrap();
            lock.execute_batch("PRAGMA journal_mode=DELETE; BEGIN EXCLUSIVE;").unwrap();
            let error = export(&library, &locked_dest, &Arc::new(AtomicBool::new(false))).unwrap_err();
            assert!(matches!(error, Error::Invalid(ref m) if m.contains("一直在写入")), "{error}");
            assert!(!locked_dest.exists());
            lock.execute_batch("ROLLBACK").unwrap();
            lock.pragma_update(None, "journal_mode", mode).unwrap();
        }
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let (source, app, stop) = (library.source(), library.db(), stop.clone());
            thread::spawn(move || {
                let (s, a) = (crate::sqlite::open(&source).unwrap(), crate::sqlite::open(&app).unwrap());
                let mut n = 0;
                while !stop.load(Ordering::SeqCst) {
                    n += 1;
                    s.execute("UPDATE epoch SET n=?", [n]).unwrap();
                    a.execute("UPDATE epoch SET n=?", [n]).unwrap();
                    thread::sleep(Duration::from_millis(40));
                }
                n
            })
        };
        let dest = root.join("export.zip");
        let mut outcomes = Vec::new();
        for _ in 0..3 {
            let _ = fs::remove_file(&dest);
            outcomes.push(export(&library, &dest, &Arc::new(AtomicBool::new(false))).map(|_| {
                let mut zip = ZipArchive::new(File::open(&dest).unwrap()).unwrap();
                let mut epoch = |name: &str| {
                    let copy = root.join(format!("check-{name}"));
                    std::io::copy(&mut zip.by_name(name).unwrap(), &mut File::create(&copy).unwrap()).unwrap();
                    let n: i64 =
                        crate::sqlite::open(&copy).unwrap().query_row("SELECT n FROM epoch", [], |r| r.get(0)).unwrap();
                    fs::remove_file(copy).unwrap();
                    n
                };
                (epoch("source.sqlite"), epoch("library.sqlite"))
            }));
        }
        stop.store(true, Ordering::SeqCst);
        assert!(writer.join().unwrap() > 0);
        // 两者处在同一时刻：应用数据库从不领先 source，source 也不会领先整整一次更新。
        for outcome in outcomes {
            match outcome {
                Ok((source, app)) => assert!(app <= source && source <= app + 1, "source {source}, app {app}"),
                Err(Error::Invalid(m)) => assert!(m.contains("一直在写入"), "{m}"),
                Err(e) => panic!("{e}"),
            }
        }
    }

    #[test]
    fn legacy_database_without_settings_preserves_sparse_rowids() {
        let work = stage(&std::env::temp_dir()).unwrap();
        let path = work.0.join("library.sqlite");
        let mut c = crate::sqlite::open(&path).unwrap();
        c.execute_batch("CREATE TABLE human(value BLOB); INSERT INTO human(rowid,value) VALUES (37,x'00ff');").unwrap();
        sanitize_database(&mut c, None).unwrap();
        let row: (i64, Vec<u8>) =
            c.query_row("SELECT rowid,value FROM human", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(row, (37, vec![0, 255]));
        assert_eq!(
            c.query_row("SELECT count(*) FROM sqlite_schema WHERE type='index'", [], |r| r.get::<_, i64>(0)).unwrap(),
            0
        );
    }

    #[test]
    fn sanitization_honors_export_cancellation_inside_sqlite() {
        let work = stage(&std::env::temp_dir()).unwrap();
        let path = work.0.join("library.sqlite");
        let mut c = crate::sqlite::open(&path).unwrap();
        c.execute_batch("CREATE TABLE human(value); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<20000) INSERT INTO human SELECT x FROM n;").unwrap();
        let error = sanitize_database(&mut c, Some(Arc::new(AtomicBool::new(true)))).unwrap_err();
        assert!(matches!(error, Error::Sql(_)), "{error}");
    }

    #[test]
    fn backslash_entry_names_are_rejected_on_every_platform() {
        let path = std::env::temp_dir().join(format!("illustash-archive-backslash-{}.zip", std::process::id()));
        {
            let mut zip = ZipWriter::new(File::create(&path).unwrap());
            for name in ["library.json", "source.sqlite", "features\\full-features.npy"] {
                zip.start_file(name, SimpleFileOptions::default()).unwrap();
                zip.write_all(b"synthetic").unwrap();
            }
            zip.finish().unwrap();
        }
        let raw = fs::read(&path).unwrap();
        let needle: &[u8] = b"features\\full-features.npy";
        assert!(raw.windows(needle.len()).any(|w| w == needle), "fixture must keep the backslash");
        let error = import(&path, &Arc::default(), &crate::app_imports::Silent).unwrap_err().to_string();
        fs::remove_file(&path).unwrap();
        assert!(error.contains("不认识的内容"), "{error}");
    }
}
