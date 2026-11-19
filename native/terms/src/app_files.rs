//! 原生静态字节和现有的按内容寻址的 JPEG 缓存。不扫描来源。
use crate::{app_library::Library, Error};
use image::{metadata::Orientation, DynamicImage, ImageReader};
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{self, BufRead, BufReader, Read, Seek},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, RwLock, Weak},
    time::UNIX_EPOCH,
};
use tiny_http::{Header, Response, StatusCode};

pub const ROUTES: &[&str] =
    &["/", "/app.js", "/app.css", "/theme.js", "/favicon.svg", "/splash.js", "/i18n.js", "/image"];
/// 页面在 web/static 下按文件夹发的文件：(文件夹, 扩展名, 类型)。core/ 和 ui/ 是 ES 模块（#199）：core/ 从不碰 DOM，
/// 可以服务任何布局；ui/ 是桌面布局。css/ 是按层拆开的样式表（#705）。路径是 /<文件夹>/<名>.<扩展名>，名字由小写字母、
/// 数字、- 和 _ 组成，不含子文件夹。
const MODULE_FOLDERS: &[(&str, &str, &str)] =
    &[("/core/", ".js", "text/javascript"), ("/ui/", ".js", "text/javascript"), ("/css/", ".css", "text/css")];
/// 文件在 web/static 下的文件夹、文件名和类型。
fn module(path: &str) -> Option<(&'static str, &str, &'static str)> {
    let (folder, extension, kind) = MODULE_FOLDERS.iter().find(|(folder, ..)| path.starts_with(*folder))?;
    let file = &path[folder.len()..];
    let name = file.strip_suffix(extension)?;
    let plain = !name.is_empty()
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    plain.then_some((folder.trim_matches('/'), file, *kind))
}
/// `path`（不含查询串）是不是静态文件、模块、样式表或图片。
pub fn serves(path: &str) -> bool {
    ROUTES.contains(&path) || module(path).is_some()
}

pub enum Failure {
    Status(u16),
    Core(Error),
}
impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        Self::Core(e)
    }
}
pub enum Prepared {
    Static(PathBuf, &'static str),
    /// 按语言设置和 web/static/locales 里的各语言表生成的 web/static/i18n.js（#173、#182）。
    I18n {
        script: PathBuf,
        locales: PathBuf,
    },
    Image {
        source: PathBuf,
        target: PathBuf,
        bound: u32,
    },
}
pub struct File {
    bytes: Vec<u8>,
    kind: &'static str,
    cache: bool,
}
impl File {
    /// 存放在缩略图中的 JPEG，比如 Danbooru 角色的预览图（#293）；页面可以缓存它。
    pub fn jpeg(bytes: Vec<u8>) -> Self {
        Self { bytes, kind: "image/jpeg", cache: true }
    }
}

pub fn prepare(library: &Library, root: &Path, url: &str) -> Result<Prepared, Failure> {
    let path = url.split('?').next().unwrap_or(url);
    if path != "/image" {
        return static_file(root, path);
    }
    let query = crate::http_read::query(url);
    let size = query.get("size").map(String::as_str).unwrap_or("thumb");
    let bound = match size {
        "thumb" => 400,
        "medium" => 800,
        "preview" => 1600,
        "full" => 3000,
        _ => return Err(Failure::Status(400)),
    };
    let ident = query.get("id").map(String::as_str).unwrap_or("");
    let connection = crate::app_library::read_connection(&library.db())?;
    let source: Option<String> = connection
        .query_row("SELECT src_path FROM assets WHERE uuid=?", [ident], |r| r.get(0))
        .optional()
        .map_err(Error::from)?
        .flatten();
    let source = source.filter(|p| !p.is_empty()).ok_or(Failure::Status(404))?;
    let source = PathBuf::from(source);
    let stat = fs::metadata(&source).map_err(|_| Failure::Status(404))?;
    let key = cache_key(ident, &stat, size).ok_or(Failure::Status(404))?;
    Ok(Prepared::Image { source, target: library.thumbnails().join(format!("{key}.jpg")), bound })
}
/// 页面的静态文件：固定的几个根目录文件、按语言生成的 i18n.js，以及 core/、ui/、css/ 下的模块和样式表。不读图库。
fn static_file(root: &Path, path: &str) -> Result<Prepared, Failure> {
    if path == "/i18n.js" {
        let script = root.join("web/static/i18n.js");
        return Ok(Prepared::I18n { script, locales: crate::app_preferences::locales_folder(root) });
    }
    if let Some((folder, file, kind)) = module(path) {
        let script = root.join("web/static").join(folder).join(file);
        return if script.is_file() { Ok(Prepared::Static(script, kind)) } else { Err(Failure::Status(404)) };
    }
    let (name, kind) = match path {
        "/" => ("index.html", "text/html"),
        "/app.css" => ("app.css", "text/css"),
        "/favicon.svg" => ("favicon.svg", "image/svg+xml"),
        "/app.js" => ("app.js", "text/javascript"),
        "/theme.js" => ("theme.js", "text/javascript"),
        "/splash.js" => ("splash.js", "text/javascript"),
        _ => return Err(Failure::Status(404)),
    };
    Ok(Prepared::Static(root.join("web/static").join(name), kind))
}
/// 缓存文件名：图片 ID、原文件的修改时间和大小、尺寸档。原文件改了就是新的键。
fn cache_key(ident: &str, stat: &fs::Metadata, size: &str) -> Option<String> {
    // SystemTime 也允许纪元之前的时间戳；旧实现的 st_mtime_ns 也是有符号的。
    let modified = stat.modified().ok()?;
    let nanos = match modified.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    };
    Some(format!("{:x}", Sha256::digest(format!("{ident}:{nanos}:{}:{size}", stat.len()).as_bytes())))
}
/// 拖出时显示的小图（#450）：页面已经显示过的缩略图缓存，没有就是 None（不为拖动现解码原图）。
pub(crate) fn cached_icon(library: &Library, ident: &str, source: &Path) -> Option<PathBuf> {
    let stat = fs::metadata(source).ok()?;
    ["thumb", "medium", "preview"]
        .into_iter()
        .filter_map(|size| cache_key(ident, &stat, size))
        .map(|key| library.thumbnails().join(format!("{key}.jpg")))
        .find(|path| path.is_file())
}

#[derive(Default)]
pub struct Images {
    keys: Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>,
    caches: Mutex<HashMap<PathBuf, Weak<RwLock<()>>>>,
    active: Mutex<usize>,
    available: Condvar,
    #[cfg(test)]
    generated: std::sync::atomic::AtomicUsize,
}
struct Slot<'a>(&'a Images);
impl Drop for Slot<'_> {
    fn drop(&mut self) {
        *self.0.active.lock().expect("app_files mutex poisoned") -= 1;
        self.0.available.notify_one();
    }
}
impl Images {
    fn cache_lock(&self, cache: &Path) -> io::Result<Arc<RwLock<()>>> {
        // 同一个图库包的不同别名共用一道闸，即使 thumbnails/ 还不存在。
        let key = crate::app_paths::canonical(cache).or_else(|_| {
            Ok::<_, io::Error>(
                crate::app_paths::canonical(cache.parent().expect("internal file path must have a parent"))?
                    .join(cache.file_name().expect("app_files internal invariant")),
            )
        })?;
        let mut caches = self.caches.lock().expect("app_files mutex poisoned");
        caches.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = caches.get(&key).and_then(Weak::upgrade) {
            return Ok(lock);
        }
        let lock = Arc::new(RwLock::new(()));
        caches.insert(key, Arc::downgrade(&lock));
        Ok(lock)
    }
    pub(crate) fn clear(&self, library: &Library, forced_off: &[String]) -> crate::Result<crate::http_json::Json> {
        let cache = library.thumbnails();
        let lock = self.cache_lock(&cache)?;
        // 读方包括读缓存字节和整个原子生成过程。套接字 IO 既不持有这道闸，也不占解码槽。其他图库包各有独立的闸。
        let _guard = lock.write().expect("app_files session lock poisoned");
        let _slot = self.slot();
        let removed = Self::remove_jpegs(&cache)?;
        let crate::http_json::Json::Object(mut fields) = crate::app_settings::info(library, forced_off)? else {
            unreachable!()
        };
        fields.insert(0, ("removed".into(), serde_json::Value::from(removed).into()));
        Ok(crate::http_json::Json::Object(fields))
    }
    fn remove_jpegs(cache: &Path) -> io::Result<u64> {
        let entries = match fs::read_dir(cache) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        let mut removed = 0;
        for entry in entries {
            let entry = entry?;
            // 与旧实现的 glob('*.jpg') 相同：直接子项，含隐藏名；Windows 用它默认的不区分大小写匹配，POSIX 区分大小写。
            let name = entry.file_name();
            let name = name.as_encoded_bytes();
            let jpeg = name
                .get(name.len().saturating_sub(4)..)
                .is_some_and(|suffix| crate::platform_filesystem::extension_matches(suffix, b".jpg"));
            if jpeg {
                match fs::remove_file(entry.path()) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
                removed += 1;
            }
        }
        Ok(removed)
    }
    fn key_lock(&self, key: &Path) -> Arc<Mutex<()>> {
        let mut keys = self.keys.lock().expect("app_files mutex poisoned");
        // 不留常驻的逐 asset 锁；只保留进行中的持有者和等待者。
        keys.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = keys.get(key).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(Mutex::new(()));
        keys.insert(key.to_owned(), Arc::downgrade(&lock));
        lock
    }
    fn slot(&self) -> Slot<'_> {
        let mut active = self.active.lock().expect("app_files mutex poisoned");
        while *active >= 4 {
            active = self.available.wait(active).expect("app_files internal invariant");
        }
        *active += 1;
        Slot(self)
    }
    pub fn load(&self, file: Prepared) -> Result<File, Failure> {
        match file {
            Prepared::Static(path, kind) => {
                Ok(File { bytes: fs::read(path).map_err(Error::from)?, kind, cache: false })
            }
            Prepared::I18n { script, locales } => {
                let locales = crate::app_preferences::locales(&locales);
                let language = crate::app_preferences::language(&locales);
                let bytes = i18n_script(&language, &fs::read(script).map_err(Error::from)?, &locales);
                Ok(File { bytes, kind: "text/javascript", cache: false })
            }
            Prepared::Image { source, target, bound } => {
                let bytes = self.cached(&source, &target, bound).map_err(|e| {
                    eprintln!("HTTP image: {e}");
                    Failure::Status(404)
                })?;
                Ok(File { bytes, kind: "image/jpeg", cache: true })
            }
        }
    }
    fn cached(&self, source: &Path, target: &Path, bound: u32) -> io::Result<Vec<u8>> {
        let cache = self.cache_lock(target.parent().expect("internal file path must have a parent"))?;
        let _cache_guard = cache.read().expect("app_files session lock poisoned");
        match fs::read(target) {
            Ok(bytes) => return Ok(bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let lock = self.key_lock(target);
        let _guard = lock.lock().expect("app_files mutex poisoned");
        match fs::read(target) {
            Ok(bytes) => return Ok(bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let _slot = self.slot();
        #[cfg(test)]
        self.generated.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let bytes = generate(source, bound)?;
        fs::create_dir_all(target.parent().expect("internal file path must have a parent"))?;
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut nonce).map_err(|e| io::Error::other(e.to_string()))?;
        let nonce: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
        let temp = target.with_extension(format!("{nonce}.tmp"));
        let result = (|| {
            use std::io::Write;
            let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
            file.write_all(&bytes)?;
            drop(file);
            fs::rename(&temp, target)
        })();
        let _ = fs::remove_file(temp);
        result?;
        Ok(bytes)
    }
}
/// 页面在 app.js 运行之前同步读取这项设置，所以首次绘制就已经是对应的语言。可选的每种语言都按名称列出，供设置页使用。
/// 只发送所设语言的表：`system` 要按浏览器的语言来定，而只有页面知道浏览器的语言，所以这时发送全部的表，
/// 由 i18n.js 选一张；事后再取会让 app.js 等待，并先闪一下中文（#182）。各表解析后再写回为 JSON：
/// 损坏或缺失的文件不放进去，它的文字仍显示中文。
fn i18n_script(language: &str, script: &[u8], locales: &[crate::app_preferences::Locale]) -> Vec<u8> {
    use serde_json::{Map, Value};
    let names: Map<String, Value> = locales.iter().map(|l| (l.code.clone(), l.name.as_str().into())).collect();
    let tables: Map<String, Value> = locales
        .iter()
        .filter(|l| language == "system" || l.code == language)
        .map(|l| (l.code.clone(), Value::Object(l.table.clone())))
        .collect();
    let mut out = format!(
        "self.ILLUSTASH_LANGUAGE = {};\nself.ILLUSTASH_LANGUAGES = {};\nself.ILLUSTASH_TABLES = {};\n",
        Value::from(language),
        Value::Object(names),
        Value::Object(tables)
    )
    .into_bytes();
    out.extend_from_slice(script);
    out
}
fn generate(source: &Path, bound: u32) -> io::Result<Vec<u8>> {
    let mut reader = BufReader::new(fs::File::open(source)?);
    // PIL 也容忍缺失或格式错误的 EXIF。按容器解析可以覆盖 PNG（image 0.25.6 的解码器不提供），以及 JPEG/TIFF/WebP。
    let orientation = exif::Reader::new()
        .read_from_container(&mut reader)
        .ok()
        .and_then(|exif| exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY).and_then(|f| f.value.get_uint(0)))
        .and_then(|n| u8::try_from(n).ok())
        .and_then(Orientation::from_exif)
        .unwrap_or(Orientation::NoTransforms);
    reader.rewind()?;
    let header = reader.fill_buf()?;
    // PIL 对调色板和二值的 PNG/BMP 用最近邻模式缩放。
    let png = header.starts_with(b"\x89PNG\r\n\x1a\n");
    let nearest = header.starts_with(b"GIF")
        || (png && (header.get(25) == Some(&3) || (header.get(24) == Some(&1) && header.get(25) == Some(&0))))
        || (header.starts_with(b"BM") && header.get(28) == Some(&1) && header.get(29) == Some(&0));
    let mut reader = ImageReader::new(reader).with_guessed_format()?;
    reader.no_limits();
    let mut im = if reader.format() == Some(image::ImageFormat::Jpeg) {
        // mozjpeg 的错误管理器遇到损坏的 JPEG 会展开栈；在进入 HTTP worker 和键锁之前兜住它，
        // 这样失败返回 404，从不毒化共享状态。
        std::panic::catch_unwind(|| -> io::Result<DynamicImage> {
            let mut decoder = mozjpeg::Decompress::new_reader(reader.into_inner())?;
            pixel_limit(decoder.width() as u32, decoder.height() as u32)?;
            let ratio = decoder.width().min(decoder.height()) / bound as usize;
            decoder.scale(if ratio >= 8 {
                1
            } else if ratio >= 4 {
                2
            } else if ratio >= 2 {
                4
            } else {
                8
            });
            let cmyk = matches!(decoder.color_space(), mozjpeg::ColorSpace::JCS_CMYK | mozjpeg::ColorSpace::JCS_YCCK);
            let mut decoder =
                if cmyk { decoder.to_colorspace(mozjpeg::ColorSpace::JCS_CMYK)? } else { decoder.rgb()? };
            let (width, height) = (decoder.width() as u32, decoder.height() as u32);
            let pixels = decoder.read_scanlines::<u8>()?;
            let pixels = if cmyk {
                // Pillow 的 JPEG 插件假定 Adobe（反相 CMYK）极性。
                pixels
                    .chunks_exact(4)
                    .flat_map(|p| [0, 1, 2].map(|i| ((u32::from(p[i]) * u32::from(p[3]) + 127) / 255) as u8))
                    .collect()
            } else {
                pixels
            };
            decoder.finish()?;
            let rgb = image::RgbImage::from_raw(width, height, pixels)
                .ok_or_else(|| io::Error::other("invalid JPEG buffer"))?;
            Ok(DynamicImage::ImageRgb8(rgb))
        })
        .map_err(|_| io::Error::other("JPEG decoder failed"))??
    } else {
        let decoder = reader.into_decoder().map_err(io::Error::other)?;
        let (width, height) = image::ImageDecoder::dimensions(&decoder);
        pixel_limit(width, height)?;
        DynamicImage::from_decoder(decoder).map_err(io::Error::other)?
    };
    im.apply_orientation(orientation);
    if im.width() > bound || im.height() > bound {
        im = resize(im, bound, nearest)?;
    }
    let rgb = if im.color().has_alpha() {
        let rgba = im.into_rgba8();
        image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
            let pixel = rgba.get_pixel(x, y);
            let alpha = u32::from(pixel[3]);
            image::Rgb(
                [0, 1, 2]
                    .map(|i| ((u32::from(pixel[i]) * alpha + [244u32, 244, 242][i] * (255 - alpha) + 127) / 255) as u8),
            )
        })
    } else {
        im.into_rgb8()
    };
    std::panic::catch_unwind(|| -> io::Result<Vec<u8>> {
        let mut encoder = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
        encoder.set_fastest_defaults();
        encoder.set_size(rgb.width() as usize, rgb.height() as usize);
        encoder.set_quality(84.0);
        let mut encoder = encoder.start_compress(Vec::new())?;
        encoder.write_scanlines(rgb.as_raw())?;
        encoder.finish()
    })
    .map_err(|_| io::Error::other("JPEG encoder failed"))?
}

fn resize(im: DynamicImage, bound: u32, nearest: bool) -> io::Result<DynamicImage> {
    use fast_image_resize::{images::Image, FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
    let (width, height) = (im.width(), im.height());
    let aspect = f64::from(width) / f64::from(height);
    // PIL 按宽高比误差选向下或向上取整（相等时取向下），从不放大。
    let (out_width, out_height) = if width <= height {
        let value = f64::from(bound) * aspect;
        let rounded =
            if (aspect - value.floor() / f64::from(bound)).abs() <= (aspect - value.ceil() / f64::from(bound)).abs() {
                value.floor()
            } else {
                value.ceil()
            };
        (rounded.max(1.) as u32, bound)
    } else {
        let value = f64::from(bound) / aspect;
        let error = |n: f64| if n == 0. { 0. } else { (aspect - f64::from(bound) / n).abs() };
        let rounded = if error(value.floor()) <= error(value.ceil()) { value.floor() } else { value.ceil() };
        (bound, rounded.max(1.) as u32)
    };
    let alpha = im.color().has_alpha();
    let (pixels, kind) = if alpha {
        (im.into_rgba8().into_raw(), PixelType::U8x4)
    } else {
        (im.into_rgb8().into_raw(), PixelType::U8x3)
    };
    let source = Image::from_vec_u8(width, height, pixels, kind).map_err(io::Error::other)?;
    let mut target = Image::new(out_width, out_height, kind);
    let algorithm = if nearest { ResizeAlg::Nearest } else { ResizeAlg::Convolution(FilterType::CatmullRom) };
    Resizer::new()
        .resize(&source, &mut target, &ResizeOptions::new().resize_alg(algorithm))
        .map_err(io::Error::other)?;
    if alpha {
        Ok(DynamicImage::ImageRgba8(
            image::RgbaImage::from_raw(out_width, out_height, target.into_vec())
                .expect("internal JSON value is serializable"),
        ))
    } else {
        Ok(DynamicImage::ImageRgb8(
            image::RgbImage::from_raw(out_width, out_height, target.into_vec())
                .expect("internal JSON value is serializable"),
        ))
    }
}

fn pixel_limit(width: u32, height: u32) -> io::Result<()> {
    // 超过两倍 MAX_IMAGE_PIXELS 时 PIL 会抛出解压炸弹错误。
    if u64::from(width) * u64::from(height) > 178_956_970 {
        Err(io::Error::other("image exceeds pixel limit"))
    } else {
        Ok(())
    }
}

pub fn response(file: File) -> Response<Box<dyn Read + Send>> {
    let length = file.bytes.len();
    let headers = [
        ("Content-Type", file.kind),
        ("Cache-Control", if file.cache { "private, max-age=86400" } else { "no-store" }),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
        ("Content-Security-Policy", "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"),
        ("X-Illustash-Route", "core"),
    ].into_iter().map(|(k,v)| Header::from_bytes(k,v).expect("app_files internal invariant")).collect();
    Response::new(
        StatusCode(200),
        headers,
        Box::new(io::Cursor::new(file.bytes)) as Box<dyn Read + Send>,
        Some(length),
        None,
    )
    .with_chunked_threshold(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    #[test]
    fn i18n_script_prefixes_setting_names_and_tables() {
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut nonce).unwrap();
        let root = std::env::temp_dir().join(format!("illustash-i18n-{:x}", Sha256::digest(nonce)));
        let folder = crate::app_preferences::locales_folder(&root);
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join("en.json"),
            "{\"@name\": \"English\", \"设置\": \"Settings\", \"共 {n} 张\": \"{n} pictures\"}",
        )
        .unwrap();
        fs::write(folder.join("ja.json"), "{\"@name\": \"日本語\", \"设置\": \"設定\"}").unwrap();
        fs::write(folder.join("fr.json"), "{broken").unwrap();
        let locales = crate::app_preferences::locales(&folder);
        let script = |language| String::from_utf8(i18n_script(language, b"/* script */", &locales)).unwrap();
        const NAMES: &str = "self.ILLUSTASH_LANGUAGES = {\"en\":\"English\",\"ja\":\"日本語\"};\n";
        // 设了语言：只发它的表。损坏的 fr.json 既不列出也不发送。
        assert_eq!(
            script("en"),
            format!("self.ILLUSTASH_LANGUAGE = \"en\";\n{NAMES}self.ILLUSTASH_TABLES = {{\"en\":{{\"@name\":\"English\",\"共 {{n}} 张\":\"{{n}} pictures\",\"设置\":\"Settings\"}}}};\n/* script */")
        );
        assert!(script("ja").ends_with(&format!(
            "{NAMES}self.ILLUSTASH_TABLES = {{\"ja\":{{\"@name\":\"日本語\",\"设置\":\"設定\"}}}};\n/* script */"
        )));
        // 中文不需要表；跟随系统时发送全部的表，由页面选。
        assert!(script("zh-CN").ends_with("self.ILLUSTASH_TABLES = {};\n/* script */"));
        let system = script("system");
        let tables = system.lines().nth(2).unwrap().strip_prefix("self.ILLUSTASH_TABLES = ").unwrap();
        let tables: serde_json::Value = serde_json::from_str(tables.strip_suffix(';').unwrap()).unwrap();
        assert_eq!(tables.as_object().unwrap().keys().collect::<Vec<_>>(), ["en", "ja"]);
        // 一张表都没有时页面保持中文，而不是出错。
        assert_eq!(
            String::from_utf8(i18n_script("system", b"", &[])).unwrap(),
            "self.ILLUSTASH_LANGUAGE = \"system\";\nself.ILLUSTASH_LANGUAGES = {};\nself.ILLUSTASH_TABLES = {};\n"
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn modules_are_plain_names_in_their_folders() {
        for path in [
            "/core/session.js",
            "/ui/dom.js",
            "/core/a_b-2.js",
            "/css/tokens.css",
            "/css/a_b-2.css",
            "/app.js",
            "/image",
        ] {
            assert!(serves(path), "{path}");
        }
        for path in [
            "/core/../app.js",
            "/core/a/b.js",
            "/core/Session.js",
            "/core/.js",
            "/core/a.mjs",
            "/core/a.css",
            "/other/a.js",
            "/core",
            "/core/",
            "/js/a.js",
            "/css/a.js",
            "/css/../app.css",
            "/css/a/b.css",
            "/css/Tokens.css",
            "/css/.css",
            "/css/",
            "/ui/a.css",
        ] {
            assert!(!serves(path), "{path}");
        }
        // 页面有的每个模块和样式表都能提供。
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/static");
        for folder in MODULE_FOLDERS.iter().map(|(folder, ..)| folder.trim_matches('/')) {
            for entry in fs::read_dir(root.join(folder)).unwrap() {
                let name = entry.unwrap().file_name().into_string().unwrap();
                assert!(serves(&format!("/{folder}/{name}")), "{folder}/{name}");
            }
        }
    }
    #[test]
    fn stylesheets_are_css_with_the_same_headers_as_app_css() {
        // css/ 下的文件按 text/css 发，缓存和安全头与 /app.css 相同（#705）；不存在的文件和越出文件夹的路径是 404。
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let headers = |file: Prepared| {
            let response = response(Images::default().load(file).ok().expect("static file loads"));
            let mut found: Vec<(String, String)> = response
                .headers()
                .iter()
                .map(|h| (h.field.as_str().as_str().to_owned(), h.value.as_str().to_owned()))
                .collect();
            found.sort();
            found
        };
        let sheet = |path| match static_file(&root, path) {
            Ok(Prepared::Static(path, kind)) => (path, kind),
            _ => panic!("{path} is not a static file"),
        };
        let (path, kind) = sheet("/css/tokens.css");
        assert_eq!((path.as_path(), kind), (root.join("web/static/css/tokens.css").as_path(), "text/css"));
        let (app, app_kind) = sheet("/app.css");
        assert_eq!(headers(Prepared::Static(path, kind)), headers(Prepared::Static(app, app_kind)));
        assert!(matches!(static_file(&root, "/css/missing.css"), Err(Failure::Status(404))));
        assert!(matches!(static_file(&root, "/css/../app.css"), Err(Failure::Status(404))));
    }
    #[test]
    fn clear_waits_for_generation_and_aliases_share_gate() {
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut nonce).unwrap();
        let root = std::env::temp_dir().join(format!("illustash-clear-{:x}", Sha256::digest(nonce)));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("cache")).unwrap();
        let source = root.join("source.png");
        let target = root.join("cache/target.jpg");
        image::RgbImage::from_pixel(12, 8, image::Rgb([40, 120, 80])).save(&source).unwrap();
        let images = Images::default();
        let gate = images.cache_lock(&root.join("cache")).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("cache"), root.join("alias")).unwrap();
            assert!(Arc::ptr_eq(&gate, &images.cache_lock(&root.join("alias")).unwrap()));
        }
        let key = images.key_lock(&target);
        let key_guard = key.lock().unwrap();
        std::thread::scope(|scope| {
            let generator = scope.spawn(|| images.cached(&source, &target, 400).unwrap());
            // 键被阻塞，证明 cached() 已进入它的目录读闸。
            let deadline = crate::test_wait::deadline();
            while gate.try_write().is_ok() {
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
            let (send, receive) = std::sync::mpsc::channel();
            let (gate, images, root) = (&gate, &images, &root);
            let cleaner = scope.spawn(move || {
                let _guard = gate.write().unwrap();
                let _slot = images.slot();
                send.send(Images::remove_jpegs(&root.join("cache")).unwrap()).unwrap();
            });
            assert!(receive.recv_timeout(std::time::Duration::from_millis(30)).is_err());
            // 等待期间无关的缓存仍然可用。
            fs::create_dir(root.join("other")).unwrap();
            assert!(images.cache_lock(&root.join("other")).unwrap().try_write().is_ok());
            drop(key_guard);
            assert!(!generator.join().unwrap().is_empty());
            assert_eq!(receive.recv_timeout(crate::test_wait::limit()).unwrap(), 1);
            cleaner.join().unwrap();
        });
        assert!(!target.exists());
        assert_eq!(fs::read_dir(root.join("cache")).unwrap().count(), 0);
        assert!(!images.cached(&source, &target, 400).unwrap().is_empty());
        assert_eq!(images.generated.load(Ordering::SeqCst), 2);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn concurrent_cache_miss_generates_once_and_drops_key_owners() {
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut nonce).unwrap();
        let root = std::env::temp_dir().join(format!("illustash-files-{:x}", Sha256::digest(nonce)));
        fs::create_dir(&root).unwrap();
        let source = root.join("source.png");
        let target = root.join("cache/target.jpg");
        image::RgbImage::from_pixel(1201, 803, image::Rgb([40, 120, 80])).save(&source).unwrap();
        let images = Images::default();
        let barrier = std::sync::Barrier::new(12);
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..12)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        images.cached(&source, &target, 400).unwrap()
                    })
                })
                .collect();
            let bytes = workers.into_iter().map(|w| w.join().unwrap()).collect::<Vec<_>>();
            assert!(bytes.iter().all(|b| b == &bytes[0]));
        });
        assert_eq!(images.generated.load(Ordering::SeqCst), 1);
        assert!(images.keys.lock().unwrap().values().all(|w| w.strong_count() == 0));
        assert_eq!(fs::read_dir(root.join("cache")).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
