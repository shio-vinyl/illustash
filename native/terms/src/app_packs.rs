//! 角色资料包的签名目录（#602）：WD 角色扩展和 CCIP 长尾锚点合成一个「角色资料包」，不跟软件版本发布，新版本列在一份
//! 签过名的目录里。2026-11-17 用户定：对用户只是一个包、一个版本号、一个开关；包里分成两个组成部分（WD、CCIP），
//! 各自固定依赖的基础模型。证据仍按两个来源（`WD_EXT`、`CCIP_LONGTAIL`）分别记，启用、换包和撤回按组成部分分别执行。
//!
//! - 目录是 `packs` 这个 Release 里的 `packs.json`，签名在旁边的 `packs.json.sig`。签名和应用更新一样是 Tauri 写的
//!   minisign 格式，但用另一把专签数据包的密钥；公钥在 `native/terms/pack-key.pub`，还没配置时整个目录功能不启用。
//! - 只跟着「检查更新」读（`app_updates` 的 check）：关掉检查更新、也没手动检查过，就从不联网读目录。读到的目录验过签、
//!   序号不比本机已有的小，才整份存进本机缓存（`extensions/catalog.json`）。
//! - 离线时，用包 Release 里的 `-offline.zip`（包文件加上签过名的目录）从文件导入，见 `adopt_archives`。
//! - 应用认的包 = 内置的角色资料包 0.1（测试版，就是原来写死的 `wd-extension-v1` 和 `ccip-longtail-v1`，离线也能用）加上最近一份
//!   验过签的目录里、本版本能用的那些：两个组成部分的格式都支持、基础模型都和这个构建的模型对得上、满足最低应用版本。
//!   别的条目原样留在目录里，但不提供。
//! - 组成部分的文件按 SHA-256 放：和内置文件相同的就用内置的文件夹，其余放 `extensions/<种类>-<SHA-256 前 12 位>/`。
//!   所以下一个版本只换了一个组成部分时，另一个不用重新下载。
pub(crate) mod character_catalog;

use crate::app_updates::Version;
use crate::{Error, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

/// 目录文件自己的格式。
pub(crate) const CATALOG_FORMAT: &str = "illustash-packs/1";
/// Release 里的两个文件名，也是离线包里的成员名。
pub(crate) const CATALOG_NAME: &str = "packs.json";
pub(crate) const SIGNATURE_NAME: &str = "packs.json.sig";
/// 目录所在的 Release。
const CATALOG_TAG: &str = "packs";
/// 目录里的下载地址只能指向本项目的 Release 资源；测试时前缀换成 ILLUSTASH_UPDATE_BASE（`app_updates::asset_url`）。
pub(crate) const RELEASE_PREFIX: &str = "https://github.com/shio-vinyl/illustash/releases/download/";
const CATALOG_LIMIT: u64 = 1 << 20;
const SIGNATURE_LIMIT: u64 = 4096;
const FILE_LIMIT: u64 = 512 << 20;
const ENTRY_LIMIT: usize = 64;
const SHARD_LIMIT: usize = 128;
pub(crate) const WD_COEFFICIENT_BYTES: u64 = (1024 + 1) * 4;
/// 专签数据包的公钥（Tauri 格式：minisign 公钥文本整体再 base64 一次）。`#` 开头的行是说明；只有说明时目录不启用。
const PUBKEY_FILE: &str = include_str!("../pack-key.pub");
/// 这个构建随带的 WD 标签表 `resources/tags/tags_v3.csv`（见 docs/THIRD_PARTY.md）。
pub(crate) const WD_TAGS_SHA256: &str = "298633d94d0031d2081c0893f29c82eab7f0df00b08483ba8f29d1e979441217";
/// 本机缓存的信封格式：原样保存目录字节和签名，读的时候重新验签。
const CACHE_FORMAT: &str = "illustash-packs-cache/1";
const CACHE_NAME: &str = "catalog.json";

/// 包的种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Wd,
    Ccip,
}

impl Kind {
    /// 组成部分的名字（也是按 SHA-256 取名的文件夹的前缀）。
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Wd => "wd-extension",
            Self::Ccip => "ccip-longtail",
        }
    }
    /// 页面和接口用的短名。
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Wd => "wd",
            Self::Ccip => "ccip",
        }
    }
    /// 这个版本读得了的包格式。旧格式 `illustash-wd-extension/1` 的包全部算一级；`/2` 给每个头标级别。
    fn formats(self) -> &'static [&'static str] {
        match self {
            Self::Wd => &[WD_FORMAT_V1, WD_FORMAT_V2],
            Self::Ccip => &[CCIP_FORMAT],
        }
    }
}

pub(crate) const WD_FORMAT_V1: &str = "illustash-wd-extension/1";
pub(crate) const WD_FORMAT_V2: &str = "illustash-wd-extension/2";
pub(crate) const CCIP_FORMAT: &str = "illustash-557-ccip-offline/1";

/// 包文件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct File {
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
    pub(crate) url: String,
}

/// WD 分片的有序清单。完整包的字节是头部、所有权重、所有偏置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Shards {
    pub(crate) header: File,
    pub(crate) coefficients: Vec<File>,
}

impl Shards {
    pub(crate) fn files(&self) -> impl Iterator<Item = &File> {
        std::iter::once(&self.header).chain(&self.coefficients)
    }
}

/// 包依赖的基础模型身份。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Base {
    Wd { model: String, tags: String },
    Ccip { model: String },
}

/// 角色资料包的一个组成部分（WD 扩展头或 CCIP 长尾锚点）。WD、CCIP 两边的加载、扫描和启用都只看它。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Pack {
    pub(crate) kind: Kind,
    /// 所属角色资料包的修订号。
    pub(crate) revision: u32,
    /// 共享数据里 `extensions/` 下的文件夹名：内置的是 `wd-extension-v1`、`ccip-longtail-v1`，其余按 SHA-256 取名。
    pub(crate) version: String,
    pub(crate) format: String,
    /// WD 的头数或 CCIP 的角色数。
    pub(crate) heads: usize,
    /// 其中只出候选、永不补主归属的二级头数（只有 WD 的 `/2` 格式有）。
    pub(crate) candidate_only: usize,
    pub(crate) file: File,
    pub(crate) shards: Option<Shards>,
    pub(crate) base: Base,
    pub(crate) license: String,
    pub(crate) builtin: bool,
}

impl Pack {
    fn view(&self) -> Value {
        json!({"format":self.format,"heads":self.heads,"candidate_only":self.candidate_only,"license":self.license,
            "file":{"path":self.file.name,"size":self.file.size,"sha256":self.file.sha256}})
    }
}

/// 一个角色资料包：内置的 0.1，或目录里本版本能用的一条。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CharacterPack {
    /// 版本名，如 `0.2`、`1.0`：只用来显示和给 Release 打 tag（`character-pack-0.2`），比新旧只看 `revision`。
    pub(crate) version: String,
    /// 越大越新。
    pub(crate) revision: u32,
    /// 测试版（目录条目的 `stage: "preview"`，2026-11-18 用户定）：只是显示不同，默认启用等行为和正式版一样。
    pub(crate) preview: bool,
    pub(crate) license: String,
    /// 一行说明（中文、英文）；内置包没有。
    pub(crate) note: Option<(String, String)>,
    pub(crate) builtin: bool,
    pub(crate) wd: Pack,
    pub(crate) ccip: Pack,
}

impl CharacterPack {
    pub(crate) fn component(&self, kind: Kind) -> &Pack {
        match kind {
            Kind::Wd => &self.wd,
            Kind::Ccip => &self.ccip,
        }
    }
    /// 角色数：两个组成部分的头数相加（两边认的角色不重复：长尾锚点只收 WD 和扩展头都没有的角色）。
    pub(crate) fn heads(&self) -> usize {
        self.wd.heads + self.ccip.heads
    }
    pub(crate) fn view(&self) -> Value {
        json!({
            "version":self.version,"revision":self.revision,"stage":if self.preview {"preview"} else {"release"},
            "heads":self.heads(),
            "candidate_only":self.wd.candidate_only + self.ccip.candidate_only,"license":self.license,"builtin":self.builtin,
            "note":self.note.as_ref().map(|(zh, en)| json!({"zh-CN":zh,"en":en})),
            "size":self.wd.file.size + self.ccip.file.size,
            "components":{"wd":self.wd.view(),"ccip":self.ccip.view()},
        })
    }
}

// ---------- 目录格式 ----------

/// 目录里角色资料包的种类名。以后的种类结构照查，本版本不提供。
pub(crate) const PACK_KIND: &str = "character-pack";
/// 内置的角色资料包：0.1（测试版），就是原来写死的 `wd-extension-v1` 和 `ccip-longtail-v1`。文件名、文件夹和这两个
/// Release 的 tag 都不变，只是版本名（2026-11-18 用户定）。
pub(crate) const BUILTIN_VERSION: &str = "0.1";
/// 目录条目的 `stage`：没有或 `release` 是正式版，`preview` 是测试版。别的值是以后的标记，这个条目不提供。
const STAGE_PREVIEW: &str = "preview";
const STAGE_RELEASE: &str = "release";

#[derive(Deserialize)]
struct RawCatalog {
    format: String,
    serial: u64,
    packs: Vec<RawEntry>,
}

#[derive(Deserialize)]
struct RawEntry {
    kind: String,
    revision: u32,
    version: String,
    min_app: String,
    license: String,
    #[serde(default)]
    stage: Option<String>,
    #[serde(default)]
    note: Option<RawNote>,
    #[serde(default)]
    components: Option<RawComponents>,
    file: Option<RawFile>,
    format: Option<String>,
}

#[derive(Deserialize)]
struct RawComponents {
    wd: RawComponent,
    ccip: RawComponent,
}

#[derive(Deserialize)]
struct RawComponent {
    format: String,
    heads: usize,
    #[serde(default)]
    candidate_only: usize,
    file: RawFile,
    shards: Option<RawShards>,
    base: RawBase,
    license: String,
}

#[derive(Deserialize)]
struct RawShards {
    header: RawFile,
    coefficients: Vec<RawFile>,
}

#[derive(Deserialize)]
struct RawFile {
    name: String,
    size: u64,
    sha256: String,
    url: String,
}

impl RawFile {
    fn valid(&self) -> bool {
        let tag = self.url.strip_prefix(RELEASE_PREFIX).and_then(|rest| rest.split_once('/'));
        segment(&self.name, 128)
            && (1..=FILE_LIMIT).contains(&self.size)
            && hex64(&self.sha256)
            && tag.is_some_and(|(tag, name)| segment(tag, 64) && name == self.name)
    }

    fn into_file(self) -> File {
        File { name: self.name, size: self.size, sha256: self.sha256, url: self.url }
    }
}

impl RawShards {
    fn validate(self, file: &RawFile, kind: Kind, heads: usize, version: &str) -> Result<Shards> {
        let valid = kind == Kind::Wd
            && (1..=SHARD_LIMIT).contains(&self.coefficients.len())
            && std::iter::once(&self.header)
                .chain(&self.coefficients)
                .all(|f| f.valid() && f.name == format!("wd-shard-{}.bin", f.sha256))
            && self.coefficients.iter().all(|f| f.size % WD_COEFFICIENT_BYTES == 0);
        if !valid {
            return Err(bad(&format!("条目 {version} 的 WD 分片不合法")));
        }
        let coefficients_size = self.coefficients.iter().map(|f| f.size).sum::<u64>();
        if coefficients_size != heads as u64 * WD_COEFFICIENT_BYTES || coefficients_size + self.header.size != file.size
        {
            return Err(bad(&format!("条目 {version} 的 WD 分片大小不符")));
        }
        Ok(Shards {
            header: self.header.into_file(),
            coefficients: self.coefficients.into_iter().map(RawFile::into_file).collect(),
        })
    }
}

#[derive(Deserialize, Default)]
struct RawBase {
    wd_model_sha256: Option<String>,
    wd_tags_sha256: Option<String>,
    ccip_model_sha256: Option<String>,
}

#[derive(Deserialize)]
struct RawNote {
    #[serde(rename = "zh-CN")]
    zh: String,
    en: String,
}

/// 一条结构合法的目录条目（还没看本版本能不能用）。不认识的种类没有组成部分。
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    kind: String,
    pack: Option<CharacterPack>,
    character_file: Option<File>,
    character_supported: bool,
    version: String,
    revision: u32,
    min_app: Version,
}

/// 验过签、结构合法的一份目录。
#[derive(Clone, Debug)]
pub(crate) struct Catalog {
    pub(crate) serial: u64,
    entries: Vec<Entry>,
}

fn bad(why: &str) -> Error {
    Error::invalid(format!("包目录无效：{why}"))
}

pub(crate) fn hex64(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// 用作文件夹名、文件名的一段：字母数字开头，只含字母数字和 `.`、`-`、`_`，不含 `..`。
fn segment(text: &str, limit: usize) -> bool {
    !text.is_empty()
        && text.len() <= limit
        && text.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        && !text.contains("..")
}

fn short(text: &str, limit: usize) -> bool {
    !text.trim().is_empty() && text.chars().count() <= limit && !text.chars().any(char::is_control)
}

/// 目录里的组成部分放在哪个文件夹：按 SHA-256 取名，同一个文件只放一份。
fn component_dir(kind: Kind, sha256: &str) -> String {
    format!("{}-{}", kind.id(), &sha256[..12])
}

impl Catalog {
    /// 解析目录并逐项检查结构。任何一条结构不对，整份不用；本版本用不了的条目留到 `packs` 再筛。
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        let raw: RawCatalog = serde_json::from_slice(bytes).map_err(|e| bad(&e.to_string()))?;
        if raw.format != CATALOG_FORMAT {
            return Err(bad("目录格式不是这个版本认识的"));
        }
        if raw.serial == 0 {
            return Err(bad("序号须为正整数"));
        }
        if raw.packs.len() > ENTRY_LIMIT {
            return Err(bad("条目过多"));
        }
        let mut entries: Vec<Entry> = Vec::new();
        for raw in raw.packs {
            let entry = Self::entry(raw)?;
            if entries
                .iter()
                .any(|e| e.version == entry.version || (e.kind == entry.kind && e.revision == entry.revision))
            {
                return Err(bad("版本或修订号重复"));
            }
            entries.push(entry);
        }
        Ok(Self { serial: raw.serial, entries })
    }

    fn entry(raw: RawEntry) -> Result<Entry> {
        let version = raw.version.chars().take(64).collect::<String>();
        let valid = segment(&raw.kind, 32)
            && segment(&raw.version, 64)
            && raw.revision >= 1
            && short(&raw.license, 200)
            && raw.stage.as_deref().is_none_or(|stage| segment(stage, 32))
            && raw.note.as_ref().is_none_or(|n| short(&n.zh, 200) && short(&n.en, 200));
        if !valid {
            return Err(bad(&format!("条目 {version} 的字段不合法")));
        }
        let stage = raw.stage.as_deref().unwrap_or(STAGE_RELEASE);
        let known_stage = matches!(stage, STAGE_PREVIEW | STAGE_RELEASE);
        let min_app = Version::parse(&raw.min_app).ok_or_else(|| bad("最低应用版本不是 X.Y.Z"))?;
        let character_file = if raw.kind == character_catalog::KIND {
            let file = raw.file.ok_or_else(|| bad("角色词表缺少文件"))?;
            if raw.components.is_some()
                || !raw.version.starts_with("catalog-")
                || !file.valid()
                || file.size > character_catalog::LIMIT
            {
                return Err(bad("角色词表的版本或文件不合法"));
            }
            Some(file.into_file())
        } else {
            None
        };
        let pack = match (raw.kind.as_str(), raw.components) {
            (PACK_KIND, None) => return Err(bad(&format!("条目 {version} 缺少组成部分"))),
            // 不认识的标记：结构照查，本版本不提供。
            (PACK_KIND, Some(_)) if !known_stage => None,
            (PACK_KIND, Some(c)) => {
                let wd = Self::component(Kind::Wd, c.wd, raw.revision, &version)?;
                let ccip = Self::component(Kind::Ccip, c.ccip, raw.revision, &version)?;
                Some(CharacterPack {
                    version: raw.version.clone(),
                    revision: raw.revision,
                    preview: stage == STAGE_PREVIEW,
                    license: raw.license,
                    note: raw.note.map(|n| (n.zh, n.en)),
                    builtin: false,
                    wd,
                    ccip,
                })
            }
            // 以后的种类：上面的通用字段照查，本版本不提供。
            _ => None,
        };
        let character_supported =
            character_file.is_some() && known_stage && raw.format.as_deref() == Some(crate::character_catalog::FORMAT);
        Ok(Entry {
            kind: raw.kind,
            pack,
            character_file,
            character_supported,
            version: raw.version,
            revision: raw.revision,
            min_app,
        })
    }

    fn component(kind: Kind, raw: RawComponent, revision: u32, version: &str) -> Result<Pack> {
        let valid = (1..=64).contains(&raw.format.len())
            && raw.format.bytes().all(|b| b.is_ascii_graphic())
            && (1..=1_000_000).contains(&raw.heads)
            && raw.candidate_only <= raw.heads
            && raw.file.valid()
            && short(&raw.license, 200);
        if !valid {
            return Err(bad(&format!("条目 {version} 的组成部分 {} 不合法", kind.key())));
        }
        let shards = raw.shards.map(|s| s.validate(&raw.file, kind, raw.heads, version)).transpose()?;
        let hex = |v: &Option<String>| v.as_deref().filter(|s| hex64(s)).map(str::to_owned);
        let base = match kind {
            Kind::Wd => Base::Wd {
                model: hex(&raw.base.wd_model_sha256).ok_or_else(|| bad("缺少 WD 模型的 SHA-256"))?,
                tags: hex(&raw.base.wd_tags_sha256).ok_or_else(|| bad("缺少 WD 标签表的 SHA-256"))?,
            },
            Kind::Ccip => Base::Ccip {
                model: hex(&raw.base.ccip_model_sha256).ok_or_else(|| bad("缺少 CCIP 模型的 SHA-256"))?,
            },
        };
        Ok(Pack {
            kind,
            revision,
            version: component_dir(kind, &raw.file.sha256),
            format: raw.format,
            heads: raw.heads,
            candidate_only: raw.candidate_only,
            file: raw.file.into_file(),
            shards,
            base,
            license: raw.license,
            builtin: false,
        })
    }

    fn require_same_version_files(&self, previous: &Self) -> Result<()> {
        let same_parts = |new: &Pack, old: &Pack| {
            new.file.sha256 == old.file.sha256
                && new.shards.iter().flat_map(Shards::files).map(|f| &f.sha256).eq(old
                    .shards
                    .iter()
                    .flat_map(Shards::files)
                    .map(|f| &f.sha256))
        };
        for entry in &self.entries {
            let earlier = previous.entries.iter().find(|old| old.version == entry.version);
            if let Some(old) = earlier {
                if old.kind != entry.kind
                    || old.revision != entry.revision
                    || entry
                        .character_file
                        .as_ref()
                        .zip(old.character_file.as_ref())
                        .is_some_and(|(a, b)| a.sha256 != b.sha256)
                {
                    return Err(bad(&format!("已发布的版本 {} 不能替换文件或修订号", entry.version)));
                }
            }
            if let (Some(new), Some(old)) = (entry.pack.as_ref(), earlier.and_then(|e| e.pack.as_ref())) {
                if new.revision != old.revision || !same_parts(&new.wd, &old.wd) || !same_parts(&new.ccip, &old.ccip) {
                    return Err(bad(&format!("已发布的版本 {} 不能替换文件或修订号", entry.version)));
                }
            }
        }
        Ok(())
    }

    /// 本版本能用的角色资料包：种类认识，两个组成部分的格式都支持、基础模型都和这个构建的对得上，满足最低应用版本。
    /// 组成部分的文件和内置的相同时，换成内置的那个（同一个文件夹、同一个下载地址）。和内置包同版本名或同修订号的条目
    /// 不提供。
    /// 目录里各个版本两个组成部分的文件名（不管这个版本能不能用）。导入离线包时，不在这里面的成员就是附带的文件。
    fn file_names(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter_map(|e| e.pack.as_ref())
            .flat_map(|p| [p.wd.file.name.as_str(), p.ccip.file.name.as_str()])
            .collect()
    }

    pub(crate) fn packs(&self, context: &Context) -> Vec<CharacterPack> {
        let builtin = &context.builtin;
        self.entries
            .iter()
            .filter(|e| context.app.core() >= e.min_app.core())
            .filter_map(|e| e.pack.clone())
            .filter(|p| p.version != builtin.version && p.revision != builtin.revision)
            .filter(|p| {
                [&p.wd, &p.ccip].into_iter().all(|c| {
                    let own = builtin.component(c.kind);
                    c.kind.formats().contains(&c.format.as_str())
                        && (c.format != WD_FORMAT_V1 || c.candidate_only == 0)
                        && (c.kind == Kind::Wd || c.candidate_only == 0)
                        && c.base == own.base
                })
            })
            .map(|mut p| {
                for kind in [Kind::Wd, Kind::Ccip] {
                    let own = builtin.component(kind);
                    let part = match kind {
                        Kind::Wd => &mut p.wd,
                        Kind::Ccip => &mut p.ccip,
                    };
                    if part.file.sha256 == own.file.sha256 {
                        *part = Pack { revision: p.revision, ..own.clone() };
                    }
                }
                p
            })
            .filter(|p| p.wd.file.sha256 != builtin.wd.file.sha256 || p.ccip.file.sha256 != builtin.ccip.file.sha256)
            .collect()
    }
}

/// 判断条目能不能用时对照的东西：这个构建的版本和内置包（内置包的基础模型就是这个构建的模型）。
pub(crate) struct Context {
    pub(crate) app: Version,
    pub(crate) builtin: CharacterPack,
}

impl Context {
    pub(crate) fn current() -> Result<Self> {
        Ok(Self { app: crate::app_updates::current(), builtin: builtin()? })
    }
}

/// 内置的角色资料包 v1：原来写死在程序里的两个包。
pub(crate) fn builtin() -> Result<CharacterPack> {
    Ok(CharacterPack {
        version: BUILTIN_VERSION.into(),
        revision: 1,
        preview: true,
        license: "Apache-2.0 AND MIT".into(),
        note: None,
        builtin: true,
        wd: builtin_wd()?,
        ccip: builtin_ccip()?,
    })
}

/// 内置的 WD 角色扩展包（models.json 的 `wd_extension`，走查时可换成合成包，见 `app_models::wd_extension`）。
pub(crate) fn builtin_wd() -> Result<Pack> {
    let spec = crate::app_models::wd_extension()?;
    let (name, size, sha256, url) = crate::app_models::entry_parts(&spec.file);
    Ok(Pack {
        kind: Kind::Wd,
        revision: 1,
        version: format!("wd-extension-v{}", spec.revision),
        format: spec.format.clone(),
        heads: spec.heads,
        candidate_only: 0,
        file: File { name, size, sha256, url },
        shards: None,
        base: Base::Wd {
            model: spec.base_sha256.clone(),
            tags: spec.tags_sha256.clone().unwrap_or_else(|| WD_TAGS_SHA256.into()),
        },
        license: spec.license.clone(),
        builtin: true,
    })
}

/// 内置的 CCIP 长尾包（app_ccip_longtail 的冻结常量，走查时可换成合成包）。
pub(crate) fn builtin_ccip() -> Result<Pack> {
    let release = crate::app_ccip_longtail::release()?;
    Ok(Pack {
        kind: Kind::Ccip,
        revision: 1,
        version: crate::app_ccip_longtail::PACKAGE_VERSION.into(),
        format: CCIP_FORMAT.into(),
        heads: release.roles,
        candidate_only: 0,
        file: File {
            name: crate::app_ccip_longtail::PACKAGE_NAME.into(),
            size: release.bytes,
            sha256: release.sha256.clone(),
            url: release.url.clone(),
        },
        shards: None,
        base: Base::Ccip { model: release.ccip_sha256.clone() },
        license: "MIT".into(),
        builtin: true,
    })
}

// ---------- 签名 ----------

/// 专签数据包的公钥。仓库里的 `pack-key.pub` 还是占位时为 None，目录功能不启用。
/// ILLUSTASH_UPDATE_BASE 指向本机测试服务时（测试和走查），只认 `ILLUSTASH_TEST_PACKS_PUBKEY` 给的测试公钥，
/// 没给就不启用：本机测试服务拿不出用正式密钥签的目录，检查更新的测试也不会因为配了正式公钥而多出请求。
pub(crate) fn pubkey() -> Option<String> {
    if std::env::var_os("ILLUSTASH_UPDATE_BASE").is_some() {
        return std::env::var("ILLUSTASH_TEST_PACKS_PUBKEY")
            .ok()
            .map(|k| k.trim().to_owned())
            .filter(|k| !k.is_empty());
    }
    configured(PUBKEY_FILE)
}

fn configured(text: &str) -> Option<String> {
    text.lines().map(str::trim).find(|line| !line.is_empty() && !line.starts_with('#')).map(str::to_owned)
}

/// 用 Tauri 格式的公钥核对 Tauri 格式的签名（`cargo tauri signer sign` 写的 `.sig`）。只认预哈希的签名（minisign 的
/// `ED`），和 `cargo tauri signer` 写出的一样。
pub(crate) fn verify(bytes: &[u8], signature: &[u8], pubkey: &str) -> Result<()> {
    let decode = |text: &[u8]| -> Option<String> {
        let raw =
            STANDARD.decode(text.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect::<Vec<_>>()).ok()?;
        String::from_utf8(raw).ok()
    };
    let key = decode(pubkey.as_bytes())
        .and_then(|text| minisign_verify::PublicKey::decode(&text).ok())
        .ok_or_else(|| Error::invalid("包目录的公钥无效"))?;
    let signature = decode(signature)
        .and_then(|text| minisign_verify::Signature::decode(&text).ok())
        .ok_or_else(|| Error::invalid("包目录的签名无效"))?;
    key.verify(bytes, &signature, false).map_err(|error| match error {
        minisign_verify::Error::UnexpectedKeyId => Error::invalid("包目录不是用数据包的密钥签的"),
        _ => Error::invalid("包目录的签名对不上"),
    })
}

// ---------- 本机缓存 ----------

/// 共享数据里放数据包的文件夹（和 WD 角色扩展包同一处）。
pub(crate) fn extensions_root(resources: &Path) -> Result<PathBuf> {
    crate::app_models::extensions_root(resources)
}

fn cache_path(resources: &Path) -> Result<PathBuf> {
    Ok(extensions_root(resources)?.join(CACHE_NAME))
}

#[derive(Deserialize)]
struct Cache {
    format: String,
    catalog: String,
    signature: String,
}

/// 本机最近一份验过签的目录。没有公钥、没有缓存，或缓存验不过签（被改过、换了密钥）时为 None。
pub(crate) fn cached(resources: &Path) -> Result<Option<Catalog>> {
    let Some(key) = pubkey() else { return Ok(None) };
    Ok(read_cache(&cache_path(resources)?, &key).map(|(catalog, _)| catalog))
}

fn read_cache(path: &Path, key: &str) -> Option<(Catalog, Vec<u8>)> {
    let text = fs::read(path).ok()?;
    let cache: Cache = serde_json::from_slice(&text).ok()?;
    let bytes = STANDARD.decode(cache.catalog).ok()?;
    let signature = STANDARD.decode(cache.signature).ok()?;
    if cache.format != CACHE_FORMAT || verify(&bytes, &signature, key).is_err() {
        return None;
    }
    Some((Catalog::parse(&bytes).ok()?, bytes))
}

/// 采用一份目录的结果。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Adopted {
    /// 比本机的新（或本机还没有），已存下。
    Newer,
    /// 和本机的是同一份。
    Same,
    /// 序号比本机的小，没有采用。
    Older,
}

/// 验签、检查结构，序号不比本机的小才存下（同一个序号只认同一份）。存的是原样的字节和签名，写临时文件再改名。
pub(crate) fn adopt(resources: &Path, bytes: &[u8], signature: &[u8]) -> Result<Adopted> {
    let key = pubkey().ok_or_else(|| Error::Conflict("这个版本还没有配置数据包目录的公钥".into()))?;
    adopt_at(&cache_path(resources)?, bytes, signature, &key)
}

fn adopt_at(path: &Path, bytes: &[u8], signature: &[u8], key: &str) -> Result<Adopted> {
    verify(bytes, signature, key)?;
    let catalog = Catalog::parse(bytes)?;
    if let Some((current, existing)) = read_cache(path, key) {
        if existing == bytes {
            return Ok(Adopted::Same);
        }
        if catalog.serial < current.serial {
            return Ok(Adopted::Older);
        }
        if catalog.serial == current.serial {
            return Err(bad("和本机已有的目录序号相同、内容不同"));
        }
        catalog.require_same_version_files(&current)?;
    }
    let envelope =
        json!({"format":CACHE_FORMAT,"catalog":STANDARD.encode(bytes),"signature":STANDARD.encode(signature)});
    fs::create_dir_all(path.parent().expect("目录缓存有上级文件夹"))?;
    let tmp = path.with_extension(format!("{}.tmp", crate::store::new_key()?));
    let written = fs::write(&tmp, envelope.to_string() + "\n").and_then(|()| fs::rename(&tmp, path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written?;
    Ok(Adopted::Newer)
}

// ---------- 联网读取 ----------

/// 读一个目录文件。连接或握手时被断开这类暂时性的错误再试两次（#642，和下载包文件同一个原因）：首次准备时读不到目录
/// 就只能下内置的 0.1。
fn get(agent: &ureq::Agent, address: &str, limit: u64) -> std::result::Result<Vec<u8>, String> {
    let mut retries = 0;
    let reply = loop {
        match agent.get(address).set("User-Agent", "Illustash-pack-catalog").call() {
            Ok(reply) => break reply,
            Err(ureq::Error::Transport(t)) if retries < 2 && crate::app_models::transient(&t) => {
                retries += 1;
                eprintln!("pack catalog: {t}; retrying");
                std::thread::sleep(crate::app_models::backoff(retries));
            }
            Err(ureq::Error::Status(code, _)) => return Err(format!("包目录的地址返回了错误（HTTP {code}）")),
            Err(ureq::Error::Transport(_)) => return Err("连不上 GitHub，没能读取包目录".into()),
        }
    };
    let mut bytes = Vec::new();
    reply.into_reader().take(limit + 1).read_to_end(&mut bytes).map_err(|_| "读取包目录失败".to_string())?;
    if bytes.len() as u64 > limit {
        return Err("包目录过大".into());
    }
    Ok(bytes)
}

/// 签名清单采用成功后，单独报告角色词表文件的下载结果。
pub(crate) enum Refresh {
    ManifestFailed(String),
    Adopted { character_error: Option<String> },
}

pub(crate) fn refresh(resources: &Path) -> Option<Refresh> {
    pubkey()?;
    let manifest = (|| {
        let prefix = crate::app_updates::asset_url(&format!("{RELEASE_PREFIX}{CATALOG_TAG}/"))?;
        let agent = crate::app_models::download_agent(Duration::from_secs(15), &prefix);
        let bytes = get(&agent, &format!("{prefix}{CATALOG_NAME}"), CATALOG_LIMIT)?;
        let signature = get(&agent, &format!("{prefix}{SIGNATURE_NAME}"), SIGNATURE_LIMIT)?;
        adopt(resources, &bytes, &signature).map_err(|e| e.reason())?;
        Ok(agent)
    })();
    Some(match manifest {
        Err(error) => Refresh::ManifestFailed(error),
        Ok(agent) => {
            Refresh::Adopted { character_error: character_catalog::fetch(resources, &agent).err().map(|e| e.reason()) }
        }
    })
}

// ---------- 离线导入 ----------

/// 所选文件里的离线包（ZIP，根上有 `packs.json` 和 `packs.json.sig`）：先采用其中的目录，再按它认包文件。
/// 角色词表在验签后独立校验和存放，其它组成部分由原来的包导入任务处理。
/// 返回目录成员和附带文件的导入结果（「名字 → 结果」，名字的写法和导入结果列表一致：`包.zip/packs.json`）。
pub(crate) fn adopt_archives(resources: &Path, paths: &[PathBuf]) -> Vec<(String, &'static str)> {
    let mut results = Vec::new();
    for path in paths {
        if !path.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")) {
            continue;
        }
        let Ok(file) = fs::File::open(path) else { continue };
        let Ok(mut archive) = zip::ZipArchive::new(file) else { continue };
        let mut member = |name: &str, limit: u64| -> Option<Vec<u8>> {
            let item = archive.by_name(name).ok()?;
            if !item.is_file() || item.size() > limit {
                return None;
            }
            let mut bytes = Vec::new();
            item.take(limit + 1).read_to_end(&mut bytes).ok()?;
            (bytes.len() as u64 <= limit).then_some(bytes)
        };
        let (Some(bytes), Some(signature)) =
            (member(CATALOG_NAME, CATALOG_LIMIT), member(SIGNATURE_NAME, SIGNATURE_LIMIT))
        else {
            continue;
        };
        let result = match adopt(resources, &bytes, &signature) {
            Ok(Adopted::Newer | Adopted::Same) => "catalog",
            Ok(Adopted::Older) => "catalog_old",
            Err(_) => "catalog_invalid",
        };
        let zip = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        results.push((format!("{zip}/{CATALOG_NAME}"), result));
        results.push((format!("{zip}/{SIGNATURE_NAME}"), result));
        // 验过签的目录没有列出的成员（许可、模型卡这些随包附带的说明，`tools/pack_catalog.py offline --extra`）是 `extra`：
        // 不是要导入的文件，也不算问题（#643）。目录签名不对时一个都不算，照旧逐个报认不出。
        if result == "catalog_invalid" {
            continue;
        }
        let Ok(catalog) = Catalog::parse(&bytes) else { continue };
        let latest = if result == "catalog_old" { cached(resources).ok().flatten() } else { None };
        let active = if result == "catalog_old" { latest.as_ref() } else { Some(&catalog) };
        let listed = catalog.file_names();
        for i in 0..archive.len() {
            let Ok(mut item) = archive.by_index(i) else { continue };
            let Some(name) = item.is_file().then(|| item.name().ok().map(|n| n.into_owned())).flatten() else {
                continue;
            };
            if name == CATALOG_NAME || name == SIGNATURE_NAME {
                continue;
            }
            let size = item.size();
            let installed = character_catalog::import_member(resources, &catalog, active, &name, size, &mut item);
            let base = name.rsplit('/').next().unwrap_or(&name);
            if let Some(result) = installed.or_else(|| (!listed.contains(&base)).then_some("extra")) {
                results.push((format!("{zip}/{name}"), result));
            }
        }
    }
    results
}

// ---------- 可用的包 ----------

/// 可用的角色资料包，内置的在前，其余按修订号从旧到新。
pub(crate) fn known(resources: &Path) -> Result<Vec<CharacterPack>> {
    let context = Context::current()?;
    let mut packs = vec![context.builtin.clone()];
    if let Some(catalog) = cached(resources)? {
        packs.extend(catalog.packs(&context));
    }
    packs.sort_by_key(|p| (!p.builtin, p.revision));
    Ok(packs)
}

/// 按组成部分的 SHA-256 找它（图库里按组成部分记用的是哪个文件）；同一个文件在几个版本里都有时，取最新的那个。
pub(crate) fn component_by_sha(resources: &Path, kind: Kind, sha256: &str) -> Result<Option<Pack>> {
    Ok(known(resources)?.into_iter().rev().map(|p| p.component(kind).clone()).find(|c| c.file.sha256 == sha256))
}

/// 按版本名找可用的角色资料包（页面请求下载、启用或换包时带的）。
pub(crate) fn by_version(resources: &Path, version: &str) -> Result<CharacterPack> {
    known(resources)?
        .into_iter()
        .find(|p| p.version == version)
        .ok_or_else(|| Error::Conflict("这个版本的角色资料包不在可用的列表里".into()))
}

/// 图库启用的是哪个版本：两个组成部分各记了一个文件，找两个都对得上的。对不上时（只启用了一个组成部分，或换包时
/// 一边没换成）先按 WD 部分、再按 CCIP 部分找含它的最新版本。
pub(crate) fn enabled_pack<'a>(
    packs: &'a [CharacterPack],
    wd: Option<&str>,
    ccip: Option<&str>,
) -> Option<&'a CharacterPack> {
    let has =
        |p: &CharacterPack, kind: Kind, sha: Option<&str>| sha.is_some_and(|s| p.component(kind).file.sha256 == s);
    packs
        .iter()
        .rev()
        .find(|p| has(p, Kind::Wd, wd) && has(p, Kind::Ccip, ccip))
        .or_else(|| packs.iter().rev().find(|p| has(p, Kind::Wd, wd)))
        .or_else(|| packs.iter().rev().find(|p| has(p, Kind::Ccip, ccip)))
}

/// 卡片上显示的版本：本图库启用的那个；没启用时是本机已有的最新的一个，都没有就是最新的一个。
pub(crate) fn current(
    packs: &[CharacterPack],
    enabled: Option<&CharacterPack>,
    present: impl Fn(&CharacterPack) -> bool,
) -> CharacterPack {
    if let Some(pack) = enabled {
        return pack.clone();
    }
    let newest = |list: Vec<&CharacterPack>| list.into_iter().max_by_key(|p| p.revision).cloned();
    newest(packs.iter().filter(|p| present(p)).collect())
        .or_else(|| newest(packs.iter().collect()))
        .expect("至少有内置的包")
}

/// 比卡片上的版本新的最新一个（「有新版本」）。
pub(crate) fn offer(packs: &[CharacterPack], current: &CharacterPack) -> Option<CharacterPack> {
    packs.iter().filter(|p| p.revision > current.revision).max_by_key(|p| p.revision).cloned()
}

/// 本图库的角色资料包：两个组成部分各自启用的文件（图库应用库的 `meta`），和据此找到的版本。
pub(crate) struct LibraryPack {
    pub(crate) wd: Option<String>,
    pub(crate) ccip: Option<String>,
    pub(crate) enabled: Option<CharacterPack>,
    pub(crate) card: CharacterPack,
    pub(crate) known: Vec<CharacterPack>,
}

/// 这个版本的两个文件都在本机（只看大小，读状态时用；用的时候再核对 SHA-256）。
pub(crate) fn present(resources: &Path, pack: &CharacterPack) -> bool {
    let shared = crate::app_resources::shared_path(resources);
    crate::app_models::extension_present(resources, &pack.wd).unwrap_or(false)
        && shared.is_ok_and(|shared| crate::app_ccip_longtail::package_path(&shared, &pack.ccip).is_file())
}

/// 打开图库读一次 `library_pack`。
pub(crate) fn for_library(resources: &Path, library: &crate::app_library::Library) -> Result<LibraryPack> {
    let app = crate::app_library::read_connection(&library.db())?;
    library_pack(resources, &app, |pack| present(resources, pack))
}

/// 角色资料包默认启用（2026-11-17 用户定，#602）时本图库的情况，给卡片用：
/// - `null`：已经启用了（至少一个组成部分），或图库只读；
/// - `"auto"`：还没有任何识别向量，等于新图库：第一次同步之前记成启用（`default_enable`），之后的识别结果照常检查；
/// - `"pending"`：新图库第一次同步时包还不在本机（#630）：包就位后宿主照默认启用，像用户点了启用一样检查已有的识别
///   结果（`pending_enable`）；
/// - `"ask"`：已有识别向量、还没决定过的图库（升级上来的）：卡片上提示一次，用户点了才启用，因为启用会给还没有归属的图
///   补上机器归属；
/// - `"declined"`：用户在提示里选了不用，或撤回过：不再提示，也不自动启用。
pub(crate) fn default_state(
    _resources: &Path,
    library: &crate::app_library::Library,
    lib: &LibraryPack,
) -> Result<Value> {
    if lib.wd.is_some() || lib.ccip.is_some() || library.frozen() {
        return Ok(Value::Null);
    }
    let app = crate::app_library::read_connection(&library.db())?;
    if crate::app_wd_extension::declined(&app)? {
        return Ok("declined".into());
    }
    if crate::app_wd_extension::default_pending(&app)? {
        return Ok("pending".into());
    }
    let vectors =
        crate::app_wd_extension::stored_vectors(library)? > 0 || crate::app_ccip_longtail::stored_vectors(library)? > 0;
    Ok(if vectors { "ask" } else { "auto" }.into())
}

/// 新图库默认启用（#602）：同步开始前，图库还没有任何识别向量、也没决定过时，把卡片上的版本记成启用（不检查，没有
/// 证据要写），这次同步交付的识别结果就照常检查。包还不在本机时记下「等包就位后默认启用」（#630），由宿主在包到了以后
/// 启用（`pending_enable`）：这次同步交付的识别结果到那时一起检查。
pub(crate) fn default_enable(resources: &Path, library: &crate::app_library::Library) -> Result<bool> {
    let lib = for_library(resources, library)?;
    if default_state(resources, library, &lib)? != "auto" {
        return Ok(false);
    }
    if !present(resources, &lib.card) {
        crate::app_wd_extension::set_default_pending(library)?;
        return Ok(false);
    }
    crate::app_wd_extension::mark_enabled(library, &lib.card.wd)?;
    crate::app_ccip_longtail::mark_enabled(library, resources, &lib.card.ccip)?;
    Ok(true)
}

/// 等着默认启用的图库（`"pending"`）在包就位后要启用的版本：卡片上的版本，两个文件都在本机时。不是这种情况时为 `None`。
pub(crate) fn pending_enable(resources: &Path, library: &crate::app_library::Library) -> Result<Option<CharacterPack>> {
    let lib = for_library(resources, library)?;
    if default_state(resources, library, &lib)? != "pending" || !present(resources, &lib.card) {
        return Ok(None);
    }
    Ok(Some(lib.card))
}

/// 用这个版本启用（或换包）的任务：两个组成部分依次处理，已有识别向量的那部分检查整库，没有的只记下启用了哪个文件。
/// 用户点「启用」和等着默认启用的图库在包就位后（`pending_enable`）都走这里。
pub(crate) fn enable_action(
    resources: &Path,
    library: &crate::app_library::Library,
    pack: &CharacterPack,
) -> Result<crate::app_wd_extension::Action> {
    use crate::app_wd_extension::{Action, Part};
    let missing = || Error::Conflict("角色资料包还没就位".into());
    let wd_path = crate::app_models::extension_file(resources, &pack.wd)?.ok_or_else(missing)?;
    let shared = crate::app_resources::shared_path(resources)?;
    let ccip_path = crate::app_ccip_longtail::package_path(&shared, &pack.ccip);
    if !ccip_path.try_exists()? {
        return Err(missing());
    }
    let wd_scan = crate::app_wd_extension::stored_vectors(library)? > 0;
    let ccip_scan = crate::app_ccip_longtail::stored_vectors(library)? > 0;
    Ok(Action::Pack {
        root: resources.to_path_buf(),
        wd: Some(Box::new(Part { pack: pack.wd.clone(), path: wd_path, scan: wd_scan })),
        ccip: Some(Box::new(Part { pack: pack.ccip.clone(), path: ccip_path, scan: ccip_scan })),
    })
}

/// 已有图库在提示里选了不用（`POST /api/packs {"action":"decline"}`）。
pub(crate) fn decline(_resources: &Path, library: &crate::app_library::Library) -> Result<()> {
    crate::app_wd_extension::set_declined(library, true)
}

/// 读本图库启用了哪个版本，算出卡片上的版本。`present` 看一个版本的两个文件是不是都在本机。
pub(crate) fn library_pack(
    resources: &Path,
    app: &rusqlite::Connection,
    present: impl Fn(&CharacterPack) -> bool,
) -> Result<LibraryPack> {
    use rusqlite::OptionalExtension;
    let wd: Option<String> = app
        .query_row("SELECT json_extract(value,'$.package_sha256') FROM meta WHERE key='wd_extension'", [], |r| r.get(0))
        .optional()?
        .flatten();
    let ccip = illustash_core::ccip_longtail::enabled_package(app)?;
    let known = known(resources)?;
    let enabled = enabled_pack(&known, wd.as_deref(), ccip.as_deref()).cloned();
    let card = current(&known, enabled.as_ref(), present);
    Ok(LibraryPack { wd, ccip, enabled, card, known })
}

#[cfg(test)]
mod tests;
