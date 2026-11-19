//! 从缩略图派生的颜色。不写入图库数据库，不作为同步、备份或导入的输入。
mod palette;

use crate::{app_library::Library, http::LibraryContext, http_json::Json, Result};
use image::RgbImage;
use rusqlite::{params, Connection, OpenFlags};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    time::Duration,
};

const CACHE: &str = ".illustash-colors-v1.sqlite";
const BATCH: usize = 128;
const POLL: Duration = Duration::from_secs(2);
const DATE_ORDER: &str = "a.added_at DESC,a.uuid";
const HASH: &str = "coalesce((SELECT value FROM meta WHERE key='folder_hash:' || a.uuid),'')";
pub(crate) const ASSET_FIELDS: &str =
    "a.uuid,a.src_path,coalesce((SELECT value FROM meta WHERE key='folder_hash:' || a.uuid),'')";

#[derive(Debug)]
pub(crate) struct Asset {
    pub id: String,
    pub source: PathBuf,
    hash: String,
}

impl Asset {
    pub(crate) fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let path: Option<String> = row.get(1)?;
        Ok(Self { id: row.get(0)?, source: PathBuf::from(path.unwrap_or_default()), hash: row.get(2)? })
    }

    fn stamp(&self) -> Option<String> {
        crate::app_files::cache_key(&self.id, &fs::metadata(&self.source).ok()?, "thumb")
    }
}

fn cache_path(package: &Path) -> PathBuf {
    package.join(CACHE)
}
fn writer(package: &Path) -> Result<Connection> {
    let c = crate::sqlite::open(cache_path(package))?;
    c.busy_timeout(Duration::from_millis(250))?;
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS colors(
          id TEXT PRIMARY KEY,source TEXT NOT NULL,hash TEXT NOT NULL,stamp TEXT NOT NULL,
          color TEXT NOT NULL,rank INTEGER NOT NULL,palette TEXT NOT NULL
         ) WITHOUT ROWID;",
    )?;
    Ok(c)
}

struct Record {
    asset: Asset,
    stamp: String,
    color: String,
    rank: i64,
    palette: String,
}
impl Record {
    fn new(asset: Asset, stamp: String, rgb: &RgbImage) -> Option<Self> {
        let colors = palette::extract(rgb);
        let main = colors.first()?.rgb;
        let palette: Vec<_> = colors
            .into_iter()
            .map(|swatch| {
                Json::object([("color", palette::hex(swatch.rgb).into()), ("share", Json::Float(swatch.share))])
            })
            .collect();
        Some(Self {
            asset,
            stamp,
            color: palette::hex(main),
            rank: palette::rank(main),
            palette: Json::Array(palette).dump_reply(),
        })
    }
}
fn write(c: &mut Connection, records: &[Record]) -> Result<()> {
    // 解码期间原文件换过就丢掉结果。读方还会用图库快照中的路径和同步哈希判定结果是否有效。
    let ready: Vec<_> = records.iter().filter(|r| r.asset.stamp().as_deref() == Some(&r.stamp)).collect();
    if ready.is_empty() {
        return Ok(());
    }
    let tx = c.transaction()?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO colors VALUES(?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(id) DO UPDATE
             SET source=excluded.source,hash=excluded.hash,stamp=excluded.stamp,color=excluded.color,
                 rank=excluded.rank,palette=excluded.palette
             WHERE colors.source<>excluded.source OR colors.hash<>excluded.hash OR colors.stamp<>excluded.stamp",
        )?;
        for record in ready {
            insert.execute(params![
                record.asset.id,
                record.asset.source.to_string_lossy(),
                record.asset.hash,
                record.stamp,
                record.color,
                record.rank,
                record.palette,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn generated(package: &Path, asset: Asset, stamp: String, rgb: &RgbImage) {
    let Some(record) = Record::new(asset, stamp, rgb) else { return };
    if let Err(e) = writer(package).and_then(|mut c| write(&mut c, &[record])) {
        eprintln!("图片颜色缓存写入失败：{e}");
    }
}

#[derive(Default)]
pub(crate) struct Worker(Option<Sender<()>>);
impl Worker {
    pub(crate) fn start(&mut self, library: &Library) {
        if self.0.is_some() {
            return;
        }
        let (stop, receiver) = mpsc::channel();
        let library = library.clone();
        match std::thread::Builder::new().name("image-colors".into()).spawn(move || {
            if let Err(e) = maintain(&library, &receiver) {
                eprintln!("图片颜色补算已停止：{e}");
            }
        }) {
            Ok(_) => self.0 = Some(stop),
            Err(e) => eprintln!("图片颜色补算未能启动：{e}"),
        }
    }
    pub(crate) fn stop(&mut self) {
        self.0 = None;
    }
}

fn maintain(library: &Library, stop: &Receiver<()>) -> Result<()> {
    let c = crate::app_library::read_connection(&library.db())?;
    let path = cache_path(library.instance_path());
    let mut previous_version = None;
    let mut previous_input = None;
    let mut had_cache = false;
    while matches!(stop.try_recv(), Err(TryRecvError::Empty)) {
        let version: i64 = c.pragma_query_value(None, "data_version", |row| row.get(0))?;
        let removed_cache = had_cache && !path.is_file();
        if previous_version != Some(version) || removed_cache {
            // 导入事件和同步结束才影响颜色的输入。同步也可能只更新路径或让先前离线的图片恢复。
            let input: (i64, i64) = c.query_row(
                "SELECT coalesce((SELECT max(id) FROM backend_effects WHERE kind='imported'),0),
                        coalesce((SELECT max(id) FROM sync_runs WHERE finished_at IS NOT NULL),0)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            previous_version = Some(version);
            if previous_input != Some(input) || removed_cache {
                match backfill(library, stop) {
                    Ok(()) => {
                        had_cache = path.is_file();
                        previous_input = Some(input);
                    }
                    Err(e) => {
                        // 缓存写入可能正和图片请求重叠。暂时锁住时下轮重试，权限等持久错误等下一次输入变化。
                        if matches!(&e, crate::Error::Sql(sql) if
                            matches!(sql.sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)))
                        {
                            previous_version = None;
                        }
                        eprintln!("图片颜色补算失败：{e}");
                    }
                }
            }
        }
        if stop.recv_timeout(POLL) != Err(RecvTimeoutError::Timeout) {
            return Ok(());
        }
    }
    Ok(())
}

/// 只读地挂载同一图库包的派生数据。连接本身仍按现有方式访问 library.sqlite。
fn attach(c: &Connection, package: &Path) -> Result<()> {
    let attached: bool =
        c.query_row("SELECT EXISTS(SELECT 1 FROM pragma_database_list WHERE name='asset_colors')", [], |row| {
            row.get(0)
        })?;
    if !attached {
        c.execute("ATTACH DATABASE ? AS asset_colors", [crate::http_read::sqlite_uri(&cache_path(package))])?;
    }
    // 刚初始化、丢失或损坏的缓存统一降级。这里先验证表和排序列存在。
    c.prepare("SELECT id,source,hash,rank FROM asset_colors.colors LIMIT 0")?;
    Ok(())
}

fn backfill(library: &Library, stop: &Receiver<()>) -> Result<()> {
    let c =
        crate::sqlite::open_with_flags(library.db(), OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI)?;
    c.busy_timeout(Duration::from_millis(250))?;
    let package = library.instance_path();
    let thumbnails = library.thumbnails();
    // 空库和来源全部离线的库，不为了补算先创建缓存。已有缓存才用它过滤掉已算且仍有效的图片。
    let mut cache = cache_path(package).is_file().then(|| writer(package)).transpose()?;
    let missing = if cache.is_some() {
        attach(&c, package)?;
        format!(
            " AND NOT EXISTS (
                SELECT 1 FROM asset_colors.colors z WHERE z.id=a.uuid AND z.source=a.src_path AND z.hash={HASH})"
        )
    } else {
        String::new()
    };
    let mut after = String::new();
    let sql = format!(
        "SELECT {ASSET_FIELDS} FROM assets a WHERE a.uuid>?1 AND a.src_path<>''{missing} ORDER BY a.uuid LIMIT ?2"
    );
    let mut batch = c.prepare(&sql)?;
    loop {
        let assets = batch.query_map(params![after, BATCH], Asset::from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        let Some(last) = assets.last() else { break };
        after.clone_from(&last.id);
        let mut records = Vec::new();
        for asset in assets {
            if !matches!(stop.try_recv(), Err(TryRecvError::Empty)) {
                return Ok(());
            }
            let Some(stamp) = asset.stamp() else { continue };
            let thumbnail = thumbnails.join(format!("{stamp}.jpg"));
            if let Ok(rgb) = crate::app_files::color_input(&asset.source, &thumbnail) {
                records.extend(Record::new(asset, stamp, &rgb));
            }
        }
        if !matches!(stop.try_recv(), Err(TryRecvError::Empty)) {
            return Ok(());
        }
        if !records.is_empty() {
            let conn = match &mut cache {
                Some(conn) => conn,
                None => cache.insert(writer(package)?),
            };
            write(conn, &records)?;
        }
    }
    Ok(())
}

pub(crate) fn ordering(c: &Connection, context: &LibraryContext) -> String {
    if context.library.as_deref().is_some_and(|package| cache_path(package).is_file() && attach(c, package).is_ok()) {
        format!(
            "coalesce((SELECT rank FROM asset_colors.colors z WHERE z.id=a.uuid AND z.source=a.src_path AND z.hash={HASH}),
             4096),{DATE_ORDER}"
        )
    } else {
        DATE_ORDER.into()
    }
}

pub(crate) fn requested(q: &HashMap<String, String>) -> bool {
    q.get("marks").is_some_and(|marks| marks.split(',').any(|mark| mark == "color"))
}

pub(crate) fn wants_work_covers(route: &str, q: &HashMap<String, String>) -> bool {
    route == "/api/works" && requested(q)
}

fn field_array<'a>(body: &'a mut Json, key: &str) -> Option<&'a mut Vec<Json>> {
    let Json::Object(fields) = body else { return None };
    let (_, Json::Array(items)) = fields.iter_mut().find(|(name, _)| name == key)? else { return None };
    Some(items)
}

struct Color {
    color: String,
    palette: Option<Json>,
}
fn load_colors(c: &Connection, package: Option<&Path>, ids: &[String], detail: bool) -> HashMap<String, Color> {
    let cached = (|| -> Result<_> {
        let Some(package) = package.filter(|_| !ids.is_empty()) else { return Ok(HashMap::new()) };
        if !cache_path(package).is_file() {
            return Ok(HashMap::new());
        }
        attach(c, package)?;
        let ids = serde_json::to_string(&ids).expect("asset IDs are serializable");
        let palette = if detail { "z.palette" } else { "NULL" };
        let sql = format!(
            "SELECT a.uuid,z.color,{palette} FROM assets a JOIN asset_colors.colors z
             ON z.id=a.uuid AND z.source=a.src_path AND z.hash={HASH}
             WHERE a.uuid IN (SELECT value FROM json_each(?))"
        );
        let mut stmt = c.prepare(&sql)?;
        let rows = stmt.query_map([&ids], |r| {
            let palette: Option<String> = r.get(2)?;
            let color =
                Color { color: r.get(1)?, palette: palette.and_then(|s| crate::http_json::parse_python(&s).ok()) };
            Ok((r.get::<_, String>(0)?, color))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    })();
    cached.unwrap_or_default()
}

fn asset_id<'a>(row: &'a Json, key: &str) -> &'a str {
    row.get(key).map_or("", Json::text)
}

pub(crate) fn decorate_response(
    c: &Connection,
    context: &LibraryContext,
    route: &str,
    q: &HashMap<String, String>,
    body: &mut Json,
) {
    let mark = requested(q);
    if !mark && route != "/api/calendar" {
        return;
    }
    let mut groups = Vec::new();
    let mut image_rows: Vec<&mut Json> = Vec::new();
    let id_key = match route {
        "/api/detail" => {
            image_rows.push(body);
            "uuid"
        }
        "/api/assets" | "/api/artist-directory" => {
            image_rows.extend(field_array(body, "items").into_iter().flatten());
            if route == "/api/assets" {
                "uuid"
            } else {
                "cover"
            }
        }
        "/api/galleries/covers" | "/api/characters" => {
            if let Json::Array(items) = body {
                image_rows.extend(items);
            }
            "cover"
        }
        "/api/works" => {
            if let Json::Array(works) = body {
                for work in works {
                    image_rows.extend(field_array(work, "covers").into_iter().flatten());
                }
            }
            "uuid"
        }
        "/api/home" | "/api/calendar" => {
            if let Some(days) = field_array(body, "days") {
                groups = days.iter_mut().collect();
            }
            "uuid"
        }
        _ => return,
    };
    let samples = if route == "/api/calendar" { "samples" } else { "items" };
    let mut ids: Vec<String> = image_rows.iter().map(|row| asset_id(row, id_key).to_owned()).collect();
    for day in &mut groups {
        ids.extend(field_array(day, samples).into_iter().flatten().map(|row| asset_id(row, id_key).to_owned()));
    }
    ids.retain(|id| !id.is_empty());
    let colors = load_colors(c, context.library.as_deref(), &ids, route == "/api/detail");
    let color_for = |row: &Json| colors.get(asset_id(row, id_key));
    let color_value = |color: Option<&Color>| color.map(|entry| entry.color.as_str()).into();
    for row in image_rows {
        let color = color_for(row);
        row.set("color", color_value(color));
        if route == "/api/detail" {
            let palette = color.and_then(|v| v.palette.clone());
            row.set("palette", palette.unwrap_or(Json::Array(Vec::new())));
        }
    }
    for day in groups {
        let Some(items) = field_array(day, samples) else { continue };
        let day_color = (route == "/api/calendar").then(|| color_value(items.iter().find_map(&color_for)));
        if mark {
            for item in items {
                item.set("color", color_value(color_for(item)));
            }
        }
        if let Some(color) = day_color {
            day.set("color", color);
        }
    }
}
