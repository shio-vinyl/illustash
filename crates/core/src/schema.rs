//! 词条核心的表、类型策略和迁移。
//!
//! 表放在 library.sqlite 里，全部以 `term_` 开头，和图库原有的 `entities`、`names`、`links`、`assets`
//! 等并存；只有撤销历史 `actions` 和版本表 `meta` 是共用的，定义与 tests/reference/web/store.py（#9 已删除） 逐字一致。
//! 版本号记在 `meta` 的 `terms_schema`，与图库原有表的 schema 版本各自独立。

#[cfg(test)]
use crate::schema::migrate;
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

/// 词条类型。类型只是词条的一个属性：「只当标签」就是把角色改成 `Label`。
/// 新增类型要同时改 `ALL`、`policy` 和一次迁移里的类型触发器。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Character,
    Skin,
    Work,
    Artist,
    Rating,
    Topic,
    Concept,
    Label,
    Pair,
    Duplicate,
    Similar,
    Cluster,
    Session,
    /// 人工标记，例如收藏。
    Mark,
}

/// 机器证据怎样变成「机器认定」或「也可能是」。人的裁决永远先于判定器。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Judge {
    /// 只投影人工确认，不使用机器证据。
    Human,
    /// 判定器还没收编（后续步骤实现）：机器证据只到「也可能是」，不进逛的界面。
    Pending,
    /// 只收强证据：方法在列表里的算机器认定，其他只到「也可能是」。
    Strong(&'static [&'static str]),
    /// 按分数：同一量纲的最高分 ≥ machine 为机器认定，≥ maybe 为也可能是，更低不投影。
    Scores { metric: &'static str, machine: f64, maybe: f64 },
    /// 同类型里取最高分的一个（配合互斥）。
    Top { metric: &'static str },
    /// 系统算法生成的组：有证据就算机器认定。
    System,
}

/// 同一张图上同类型词条之间的约束。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exclusive {
    None,
    /// 单人图只能有一个（角色判定器负责，依赖图上的 solo 证据）。
    OnSolo,
    /// 恰好一个（分级）。
    ExactlyOne,
    /// 至多一个（一张图只在一个重复组、一个保存段里）。
    AtMostOne,
}

#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub name: &'static str,
    pub judge: Judge,
    pub exclusive: Exclusive,
    /// 证据包能否按外部链接新建这种词条。用户自建的概念、CP 只能由核心或人建。
    pub package_may_create: bool,
    /// 带分数的证据低于这个分数不入证据表（原始分数留在 worker 的研究库，例如 WD 的 2000 万条原始标签）。
    pub keep_min: Option<(&'static str, f64)>,
    /// 是否参与提问。
    pub asks: bool,
}

const WD: &str = "wd_probability";

impl Kind {
    pub const ALL: [Kind; 14] = [
        Kind::Character,
        Kind::Skin,
        Kind::Work,
        Kind::Artist,
        Kind::Rating,
        Kind::Topic,
        Kind::Concept,
        Kind::Label,
        Kind::Pair,
        Kind::Duplicate,
        Kind::Similar,
        Kind::Cluster,
        Kind::Session,
        Kind::Mark,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Character => "character",
            Kind::Skin => "skin",
            Kind::Work => "work",
            Kind::Artist => "artist",
            Kind::Rating => "rating",
            Kind::Topic => "topic",
            Kind::Concept => "concept",
            Kind::Label => "label",
            Kind::Pair => "pair",
            Kind::Duplicate => "duplicate",
            Kind::Similar => "similar",
            Kind::Cluster => "cluster",
            Kind::Session => "session",
            Kind::Mark => "mark",
        }
    }

    pub fn parse(value: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == value)
    }

    pub fn policy(self) -> Policy {
        let p = |name, judge, exclusive, package_may_create, keep_min, asks| Policy {
            name,
            judge,
            exclusive,
            package_may_create,
            keep_min,
            asks,
        };
        match self {
            // 角色：d1/d2 校准加单人规则，第 5 步收编；hybrid 与 v2 成为两种策略。
            Kind::Character => p("角色", Judge::Pending, Exclusive::OnSolo, true, None, true),
            Kind::Skin => p("皮肤", Judge::Pending, Exclusive::None, true, None, true),
            // 作品主要经「包含」关系从角色继承，第 4 步收编。
            Kind::Work => p("作品", Judge::Pending, Exclusive::None, true, None, false),
            // 画师宁缺毋滥：像素哈希、本地比对、IQDB、同一张图（重复 / 相似组）才算；OCR、关注、分组推荐只进「也许是 TA」。
            // 与溯源写 image_artists.status 的规则一致（auto / candidate 由方法决定，provenance/sources/*）。
            Kind::Artist => p(
                "画师",
                Judge::Strong(&["pixel_hash", "gallery_match", "iqdb", "same_picture"]),
                Exclusive::None,
                true,
                None,
                true,
            ),
            // 与 tests/reference/web/store.py（#9 已删除） refresh_ratings 一致：每张图取 WD 最高的一个分级。
            Kind::Rating => p("分级", Judge::Top { metric: WD }, Exclusive::ExactlyOne, true, None, false),
            // 与 general_hits 一致：WD ≥ 0.35 算命中；0.2–0.35 只给搜索放宽用。
            Kind::Topic => p(
                "题材",
                Judge::Scores { metric: WD, machine: 0.35, maybe: 0.2 },
                Exclusive::None,
                true,
                Some((WD, 0.2)),
                false,
            ),
            // 规则或例子学出的判定器在第 3 步接入；包只能给已有概念加证据。
            Kind::Concept => p("概念", Judge::Pending, Exclusive::None, false, None, true),
            Kind::Label => p("只当标签", Judge::Pending, Exclusive::None, true, None, false),
            Kind::Pair => p("CP", Judge::Pending, Exclusive::None, false, None, false),
            Kind::Duplicate => p("重复组", Judge::System, Exclusive::AtMostOne, true, None, false),
            Kind::Similar => p("相似组", Judge::System, Exclusive::None, true, None, false),
            Kind::Cluster => p("聚类", Judge::System, Exclusive::None, true, None, false),
            Kind::Session => p("保存段", Judge::System, Exclusive::AtMostOne, true, None, false),
            Kind::Mark => p("人工标记", Judge::Human, Exclusive::None, false, None, false),
        }
    }
}

/// 词条—词条关系。`contains` 与 `same_as` 有方向（a 包含 b；b 是 a 的代表），其余两种对称、存成 a < b。
/// 共现不存关系行，由投影算出。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RelationKind {
    Contains,
    SameAs,
    DifferentFrom,
    Excludes,
}

impl RelationKind {
    pub const ALL: [RelationKind; 4] =
        [RelationKind::Contains, RelationKind::SameAs, RelationKind::DifferentFrom, RelationKind::Excludes];

    pub fn as_str(self) -> &'static str {
        match self {
            RelationKind::Contains => "contains",
            RelationKind::SameAs => "same_as",
            RelationKind::DifferentFrom => "different_from",
            RelationKind::Excludes => "excludes",
        }
    }

    pub fn parse(value: &str) -> Option<RelationKind> {
        RelationKind::ALL.into_iter().find(|k| k.as_str() == value)
    }

    pub fn symmetric(self) -> bool {
        matches!(self, RelationKind::DifferentFrom | RelationKind::Excludes)
    }
}

/// 裁决只来自人。`legacy` 是旧表里人工结论的收编（沿用原来的 action_id），`album` 是用户按相册批量认定。
pub const HUMAN_ORIGINS: [&str; 3] = ["user", "album", "legacy"];

/// 证据来源不能冒充人。
pub const RESERVED_SOURCES: [&str; 5] = ["user", "human", "album", "legacy", "verdict"];

pub const SCHEMA_VERSION: i64 = 11;

/// 收藏在各图库中共用的人工标记词条键。
pub const FAVORITE_KEY: &str = "mark:favorite";

/// 可替换的机器证据（`source`, `method`）：按有效批次生效（`term_active_evidence`），成功的空结果让旧证据失效。
/// 其余来源的证据全部有效。与当前 `term_evidence_sets` 的 CHECK 一致，加一种要一次迁移。
pub const REPLACEABLE: [(&str, &str); 4] = [
    ("wd", "wd_character"),
    ("ccip_index", "char_ext"),
    (crate::wd_extension::SOURCE, crate::wd_extension::METHOD),
    (crate::ccip_longtail::SOURCE, crate::ccip_longtail::METHOD),
];

/// 整个来源一起替换的机器证据（第 3 版）：一次交付是这个来源的全量快照，覆盖到的图上它所有方法的证据都按批次生效，
/// 批次的方法记为 `*`。溯源每次交的包就是 provenance.sqlite 的全量快照（tests/reference/provenance/export.py（#9 已删除））：快照里没有了的
/// 候选（分组推荐每次重算）随之失效。与 V3 的 CHECK 和 term_current_evidence 一致。
pub const REPLACEABLE_SOURCES: [&str; 1] = ["provenance"];

/// 这条证据是否只经证据批次生效。
pub fn replaceable(source: &str, method: &str) -> bool {
    source == "example" || REPLACEABLE_SOURCES.contains(&source) || REPLACEABLE.contains(&(source, method))
}

/// 第 6 版只压缩派生表的物理布局，证据、历史和各列值不变。
fn v6(conn: &Connection) -> Result<()> {
    if crate::legacy_characters::characters_in_core(conn)? {
        compact_characters(conn)?;
    }
    Ok(())
}

/// 第 7 版：身份策略 v2 已放弃（2026-09-30）。删掉只有 v2 用的 `identity_claims`、`identity_calibration`
/// 和 meta 的 `identity_calibration_active`、`identity_bank_error`；hybrid 下它们一直是空的，删掉不改变任何结果。
fn v7(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS identity_claims;
         DROP TABLE IF EXISTS identity_calibration;
         DELETE FROM meta WHERE key IN ('identity_calibration_active','identity_bank_error');",
    )?;
    Ok(())
}

/// 第 8 版（#201）：旧兼容表退役。
/// - 已有核心表的库（升级前版本 ≥ 1）里有旧表（`entities`）时，四个领域必须都已切到核心，否则拒绝升级、整体回滚：
///   没切换完的旧图库先用 `pre-v8` 版本升级。还没有核心表的库（新建的库，以及应用层已经拒绝打开的更早的旧图库）不在
///   这里拦。各张旧表在退役时与核心逐行核对（见下）。`assertions`、`candidates` 是核心维护的派生表，不在这里比。
/// - 链接、作品复核、身份基线：`term_links`、`term_tree` 的复核三列、`term_identity_baseline` 成为唯一权威，旧表
///   `links`、`work_review`、`identity_baseline` 逐行核对后删除（`legacy_characters::retire_*`，不一致就拒绝升级）。
/// - 别名、显示名、名字：`term_key_aliases`、`term_key_names`、`term_names` 成为唯一权威，旧表 `name_aliases`、
///   `display_names`、`names` 照旧表同步一次、逐行核对后删除（`legacy_names::retire_*`）；`term_names` 加名字的先后
///   `seq`，从 `names` 的 rowid 填入。
/// - 身份：`terms`、`term_tree` 成为唯一权威。`term_tree` 加属性列（从 `terms.attrs` 提升）和身份的先后 `seq`（从
///   `entities` 的 rowid 填入）；旧表 `entities` 逐行核对后删除，引用它的视图按核心表重写
///   （`legacy_characters::retire_entities`）。
/// - 身份关系：`term_relations`、关系证据、关系裁决和 `term_legacy_ids` 成为唯一权威，旧表 `identity_relations`、
///   `identity_evidence`、`identity_decisions`、`identity_decision_evidence` 逐表核对后删除
///   （`legacy_characters::retire_relations`），在身份之前。
/// - 名字版本触发器随旧表删除，改装到 `term_tree`、`terms`（类型）、`term_links`、`term_names`、`term_key_names` 上。
/// - `actions` 加 `label_kind`、`label_count`：按张数写提示的几种步骤的种类和张数（`undo_labels`）。之前的步骤按
///   已存的提示回填一次，撤销载荷 `before_json` 不动。
fn v8(conn: &Connection) -> Result<()> {
    let has = |table: &str| -> Result<bool> {
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table],
            |r| r.get(0),
        )?)
    };
    // 迁移结束才写新版本号，这里读到的还是升级前的版本。
    if version(conn)? > 0 && has("entities")? {
        let flag = |key: &str| -> Result<Option<String>> {
            Ok(conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0)).optional()?)
        };
        let flags = ["terms_artists", "terms_labels", "terms_characters", "terms_names"]
            .map(flag)
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        let switched = flags.iter().all(|v| v.as_deref() == Some("core"));
        let untouched = flags.iter().all(Option::is_none)
            && !conn.query_row("SELECT EXISTS(SELECT 1 FROM entities)", [], |r| r.get::<_, bool>(0))?;
        if !switched && !untouched {
            return Err(Error::Schema(
                "这个图库还没有完全切换到词条核心，新版本不再直接升级它：请先用 pre-v8 版本打开并升级，再用新版本打开"
                    .into(),
            ));
        }
    }
    // 旧表退役之后才把名字版本触发器装到 `term_*` 上，退役时的同步不计入 `names_version`。
    let versioned = crate::legacy_names::versioned(conn)?;
    crate::legacy_characters::retire_links(conn)?;
    crate::legacy_characters::retire_work_review(conn)?;
    crate::legacy_characters::retire_identity_baseline(conn)?;
    crate::legacy_characters::retire_relations(conn)?;
    crate::legacy_names::retire_aliases(conn)?;
    crate::legacy_names::retire_display_names(conn)?;
    let names = crate::legacy_names::retire_names(conn)?;
    if !names.clean() {
        return Err(Error::Schema(format!("旧名字表与词条核心不一致，没有升级：{report}", report = names.to_json())));
    }
    crate::legacy_characters::retire_entities(conn)?;
    if versioned {
        crate::legacy_names::version_triggers(conn)?;
    }
    let columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('actions')")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for column in ["label_kind", "label_count"] {
        if !columns.iter().any(|c| c == column) {
            conn.execute_batch(&format!("ALTER TABLE actions ADD COLUMN {column} TEXT"))?;
        }
    }
    let labels: Vec<(i64, String)> = conn
        .prepare("SELECT id,label FROM actions WHERE label_kind IS NULL ORDER BY id")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut fill = conn.prepare("UPDATE actions SET label_kind=?1,label_count=?2 WHERE id=?3")?;
    for (id, label) in labels {
        if let Some((kind, n)) = crate::undo_labels::UndoLabel::recognize(&label) {
            fill.execute(rusqlite::params![kind.name(), n, id])?;
        }
    }
    Ok(())
}

/// 迁移与首次角色切换共用；调用方持有写事务，失败时表、索引、触发器整体回滚。
pub(crate) fn compact_characters(conn: &Connection) -> Result<()> {
    // 早期收编核对留在库里的持久影子表（`shadow_*`）：没有读者，不把校验副本永久留在图库里。
    let shadows: Vec<String> = conn
        .prepare("SELECT name FROM main.sqlite_master WHERE type='table' AND name GLOB 'shadow_*'")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for table in shadows {
        conn.execute_batch(&format!("DROP TABLE \"{table}\""))?;
    }
    let legacy: bool = conn.pragma_query_value(None, "legacy_alter_table", |r| r.get(0))?;
    conn.pragma_update(None, "legacy_alter_table", true)?;
    let result = (|| -> Result<()> {
        for table in ["candidates", "family_evidence"] {
            let sql: Option<String> = conn
                .query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name=?", [table], |r| r.get(0))
                .optional()?;
            let Some(sql) = sql.filter(|s| !s.to_ascii_uppercase().contains("WITHOUT ROWID")) else { continue };
            let depend: Vec<String> = conn.prepare(
                "SELECT sql FROM sqlite_master WHERE tbl_name=? AND type IN ('index','trigger') AND sql IS NOT NULL")?
                .query_map([table], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            let body = sql.find('(').ok_or_else(|| Error::Schema(format!("看不懂 {table} 的建表语句")))?;
            conn.execute_batch(&format!(
                "CREATE TABLE terms_compact_new {} WITHOUT ROWID;
                INSERT INTO terms_compact_new SELECT * FROM {table};
                DROP TABLE {table};
                ALTER TABLE terms_compact_new RENAME TO {table};",
                &sql[body..]
            ))?;
            for definition in depend {
                conn.execute_batch(&definition)?;
            }
        }
        conn.execute("INSERT OR REPLACE INTO meta VALUES ('terms_compact_pending','1')", [])?;
        Ok(())
    })();
    conn.pragma_update(None, "legacy_alter_table", legacy)?;
    result
}

/// VACUUM 是提交后的空间维护；忙或磁盘不足时保留标记，下次启动重试，不冒充迁移失败。
pub fn compact_pending(conn: &Connection) {
    let result = (|| -> rusqlite::Result<()> {
        let pending = conn
            .query_row("SELECT value FROM meta WHERE key='terms_compact_pending'", [], |r| r.get::<_, String>(0))
            .optional()?;
        if pending.is_some() {
            // SQLite 可能给没有索引的表重新编号隐式 rowid。用户的研究/历史扩展即使核心从不读取也要保留。
            // 不要为了可有可无的文件空间回收而牺牲它们的物理身份。
            if vacuum_preserves_rowids(conn)? {
                conn.execute_batch("VACUUM")?;
            } else {
                eprintln!("词条表升级已提交；存在无索引的隐式 rowid 表，保留物理行，未回收文件空间");
            }
            // 出于安全的拒绝算维护已结束，不是可重试的忙碌或磁盘错误。
            conn.execute("DELETE FROM meta WHERE key='terms_compact_pending'", [])?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("词条表升级已提交，空间回收留待下次启动：{error}");
    }
}

/// VACUUM 会保留有索引的表和 INTEGER PRIMARY KEY 别名的 rowid。
/// 对没有索引的 rowid 表，不管当前密度如何都拒绝维护：物理身份不能取决于偶然的插入布局。
fn vacuum_preserves_rowids(conn: &Connection) -> rusqlite::Result<bool> {
    let tables = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for table in tables {
        let quoted = format!("\"{}\"", table.replace('"', "\"\""));
        if conn.prepare(&format!("SELECT rowid FROM {quoted} LIMIT 0")).is_err() {
            continue; // WITHOUT ROWID 没有隐式的物理身份。
        }
        let indexed = conn.prepare(&format!("PRAGMA index_list({quoted})"))?.exists([])?;
        // INTEGER PRIMARY KEY 别名没有单独的主键索引。历史上的 `INTEGER PRIMARY KEY DESC` 例外则确实有索引。
        let integer_primary = conn
            .prepare(&format!("PRAGMA table_info({quoted})"))?
            .query_map([], |r| Ok((r.get::<_, String>(2)?, r.get::<_, i64>(5)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|(kind, pk)| kind.eq_ignore_ascii_case("INTEGER") && *pk == 1);
        if !indexed && !integer_primary {
            return Ok(false);
        }
    }
    Ok(true)
}

/// 升级必须提供备份能力；平台相关和默认的接线留在 API 门面里。
pub fn backup_before_upgrade(
    conn: &Connection,
    before: i64,
    backup: &crate::backup::RequiredBackup,
) -> Result<Option<std::path::PathBuf>> {
    crate::backup::required(conn, &format!("v{before}-before-v{SCHEMA_VERSION}"), backup)
}

/// 核心写事务持有的「钥匙」：兼容表和私有表的写保护触发器在 `meta` 里看不到它时拒绝写入。
/// 别的连接看不到未提交的行，所以核心写事务之外对这些表的写入都会被拒绝。
pub const WRITER_KEY: &str = "terms_writer";

/// 与 tests/reference/web/store.py（#9 已删除） 的定义逐字一致；已存在时是空操作。
const SHARED: &str = "
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS actions(id INTEGER PRIMARY KEY AUTOINCREMENT,label TEXT NOT NULL,before_json TEXT NOT NULL,ts TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,undone INTEGER NOT NULL DEFAULT 0);
";

/// 每个版本一段，按顺序执行；已发布的段不再修改。
const MIGRATIONS: [fn(&Connection) -> Result<()>; 11] =
    [v1, v2, v3, v4, v5, v6, v7, v8, crate::human_conclusions::migrate, v10, v11];

fn v1(conn: &Connection) -> Result<()> {
    Ok(conn.execute_batch(V1)?)
}

const V1: &str = "
-- 资产引用：平台命名空间 + 平台内 ID。md5 只是证据，不凭它替换资产身份或传播人工裁决。
CREATE TABLE term_assets(
 id INTEGER PRIMARY KEY,
 namespace TEXT NOT NULL CHECK(length(namespace) BETWEEN 1 AND 32),
 key TEXT NOT NULL CHECK(length(key) BETWEEN 1 AND 256),
 md5 TEXT CHECK(md5 IS NULL OR length(md5)=32),
 added_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
 removed_at TEXT,
 UNIQUE(namespace,key));
CREATE INDEX term_assets_md5 ON term_assets(md5) WHERE md5 IS NOT NULL;

-- 词条：key 永不改变（界面链接、证据指纹都用它）；kind 只是属性。
CREATE TABLE terms(
 id INTEGER PRIMARY KEY,
 key TEXT NOT NULL UNIQUE CHECK(length(key) BETWEEN 1 AND 128),
 kind TEXT NOT NULL,
 origin TEXT NOT NULL CHECK(length(origin) BETWEEN 1 AND 80),
 attrs TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(attrs) AND json_type(attrs)='object'),
 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
CREATE INDEX terms_kind ON terms(kind,id);
CREATE TRIGGER terms_kind_insert BEFORE INSERT ON terms
 WHEN new.kind NOT IN ('character','skin','work','artist','rating','topic','concept','label','pair','duplicate','similar','cluster','session')
 BEGIN SELECT RAISE(ABORT,'unknown term kind'); END;
CREATE TRIGGER terms_kind_update BEFORE UPDATE OF kind ON terms
 WHEN new.kind NOT IN ('character','skin','work','artist','rating','topic','concept','label','pair','duplicate','similar','cluster','session')
 BEGIN SELECT RAISE(ABORT,'unknown term kind'); END;
CREATE TRIGGER terms_key_permanent BEFORE UPDATE OF key ON terms WHEN new.key IS NOT old.key
 BEGIN SELECT RAISE(ABORT,'term key is permanent'); END;

-- 名字（沿用 names 层）与外部链接（沿用 links 层）。同一个 (system, key) 只属于一个词条。
CREATE TABLE term_names(
 term_id INTEGER NOT NULL REFERENCES terms(id),
 lang TEXT NOT NULL DEFAULT '' CHECK(lang IN ('','zh','ja','en')),
 value TEXT NOT NULL CHECK(length(value) BETWEEN 1 AND 200),
 role TEXT NOT NULL CHECK(role IN ('display','alias')),
 source TEXT NOT NULL,
 preferred INTEGER NOT NULL DEFAULT 0 CHECK(preferred IN (0,1)),
 position INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(term_id,role,lang,source,value));
CREATE INDEX term_names_value ON term_names(value COLLATE NOCASE);
CREATE TABLE term_links(
 system TEXT NOT NULL CHECK(length(system) BETWEEN 1 AND 32),
 key TEXT NOT NULL CHECK(length(key) BETWEEN 1 AND 256),
 term_id INTEGER NOT NULL REFERENCES terms(id),
 url TEXT NOT NULL DEFAULT '',
 source TEXT NOT NULL,
 current INTEGER NOT NULL DEFAULT 1 CHECK(current IN (0,1)),
 added_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
 PRIMARY KEY(system,key));
CREATE INDEX term_links_term ON term_links(term_id,system);

-- 词条—词条：只存端点和种类。状态由证据和裁决算出（term_relation_states）。
CREATE TABLE term_relations(
 id INTEGER PRIMARY KEY,
 a INTEGER NOT NULL REFERENCES terms(id),
 b INTEGER NOT NULL REFERENCES terms(id),
 kind TEXT NOT NULL CHECK(kind IN ('contains','same_as','different_from','excludes')),
 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
 UNIQUE(a,b,kind),
 CHECK(a<>b),
 CHECK(kind IN ('contains','same_as') OR a<b));
CREATE INDEX term_relations_b ON term_relations(b,kind);

-- 证据包导入账本：同一个包只导入一次。
CREATE TABLE term_packages(
 id INTEGER PRIMARY KEY,
 fingerprint TEXT NOT NULL UNIQUE CHECK(length(fingerprint)=64),
 producer TEXT NOT NULL,
 producer_version TEXT NOT NULL,
 library_id TEXT NOT NULL,
 manifest TEXT NOT NULL CHECK(json_valid(manifest)),
 evidence_count INTEGER NOT NULL,
 skipped_count INTEGER NOT NULL DEFAULT 0,
 imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);

-- 证据：只增不删，一个来源不覆盖另一个来源。目标是 (图, 词条) 或一条词条关系，二选一。
-- 同一目标、同一来源方法和版本只能有一条；新版本另起一条。
CREATE TABLE term_evidence(
 id INTEGER PRIMARY KEY,
 asset_id INTEGER REFERENCES term_assets(id),
 term_id INTEGER REFERENCES terms(id),
 relation_id INTEGER REFERENCES term_relations(id),
 source TEXT NOT NULL CHECK(source NOT IN ('user','human','album','legacy','verdict')),
 method TEXT NOT NULL,
 version TEXT NOT NULL,
 score REAL,
 metric TEXT,
 data TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(data) AND json_type(data)='object'),
 fingerprint BLOB NOT NULL CHECK(length(fingerprint)=16),
 package_id INTEGER REFERENCES term_packages(id),
 observed_at TEXT,
 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
 CHECK((asset_id IS NOT NULL AND term_id IS NOT NULL AND relation_id IS NULL)
    OR (asset_id IS NULL AND term_id IS NULL AND relation_id IS NOT NULL)),
 CHECK((score IS NULL) = (metric IS NULL)));
CREATE UNIQUE INDEX term_evidence_claim ON term_evidence(asset_id,term_id,source,method,version) WHERE asset_id IS NOT NULL;
CREATE UNIQUE INDEX term_evidence_relation ON term_evidence(relation_id,source,method,version) WHERE relation_id IS NOT NULL;
CREATE INDEX term_evidence_term ON term_evidence(term_id,asset_id) WHERE term_id IS NOT NULL;
CREATE TRIGGER term_evidence_append_only_update BEFORE UPDATE ON term_evidence
 BEGIN SELECT RAISE(ABORT,'evidence is append-only'); END;
CREATE TRIGGER term_evidence_append_only_delete BEFORE DELETE ON term_evidence
 BEGIN SELECT RAISE(ABORT,'evidence is append-only'); END;

-- 裁决：只来自人，追加写；每条属于一个 actions 步骤，撤销就是把那一步标为 undone。
-- 同一目标取历史上最新（action_id 最大）的未撤销一条；clear 表示人收回了结论。reviewed 是裁决时看到的证据 id。
CREATE TABLE term_verdicts(
 id INTEGER PRIMARY KEY,
 action_id INTEGER NOT NULL REFERENCES actions(id),
 asset_id INTEGER REFERENCES term_assets(id),
 term_id INTEGER REFERENCES terms(id),
 relation_id INTEGER REFERENCES term_relations(id),
 verdict TEXT NOT NULL CHECK(verdict IN ('yes','no','clear')),
 origin TEXT NOT NULL CHECK(origin IN ('user','album','legacy')),
 reason TEXT NOT NULL DEFAULT '',
 reviewed TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(reviewed) AND json_type(reviewed)='array'),
 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
 CHECK((asset_id IS NOT NULL AND term_id IS NOT NULL AND relation_id IS NULL)
    OR (asset_id IS NULL AND term_id IS NULL AND relation_id IS NOT NULL)));
CREATE INDEX term_verdicts_image ON term_verdicts(asset_id,term_id,id) WHERE asset_id IS NOT NULL;
CREATE INDEX term_verdicts_relation ON term_verdicts(relation_id,id) WHERE relation_id IS NOT NULL;
CREATE INDEX term_verdicts_action ON term_verdicts(action_id);
CREATE TRIGGER term_verdicts_append_only_update BEFORE UPDATE ON term_verdicts
 BEGIN SELECT RAISE(ABORT,'verdicts are append-only; undo through actions'); END;
CREATE TRIGGER term_verdicts_append_only_delete BEFORE DELETE ON term_verdicts
 BEGIN SELECT RAISE(ABORT,'verdicts are append-only; undo through actions'); END;

-- 投影：图上显示什么，由证据、裁决和类型策略算出；可随时整表重建。
CREATE TABLE term_projection(
 asset_id INTEGER NOT NULL REFERENCES term_assets(id),
 term_id INTEGER NOT NULL REFERENCES terms(id),
 state TEXT NOT NULL CHECK(state IN ('confirmed','machine','maybe','rejected')),
 score REAL,
 sources TEXT NOT NULL DEFAULT '',
 PRIMARY KEY(asset_id,term_id)) WITHOUT ROWID;
CREATE INDEX term_projection_term ON term_projection(term_id,state,asset_id);

CREATE VIEW term_effective_verdicts AS
 SELECT id,action_id,asset_id,term_id,relation_id,verdict,origin,reason,reviewed,created_at FROM (
  SELECT v.*,row_number() OVER (PARTITION BY v.asset_id,v.term_id,v.relation_id ORDER BY v.action_id DESC,v.id DESC) AS rn
  FROM term_verdicts v JOIN actions a ON a.id=v.action_id WHERE a.undone=0)
 WHERE rn=1 AND verdict<>'clear';

CREATE VIEW term_relation_states AS
 SELECT r.id,r.a,r.b,r.kind,
  CASE v.verdict WHEN 'yes' THEN 'confirmed' WHEN 'no' THEN 'rejected' ELSE 'maybe' END AS state
 FROM term_relations r LEFT JOIN term_effective_verdicts v ON v.relation_id=r.id;
";

/// 第 4、5 步（角色、皮肤、作品）要的 schema，docs/TERMS_CHARACTERS.md 方案第 2 节。
fn v2(conn: &Connection) -> Result<()> {
    // term_verdicts 的 action_id 要变成可空：只能重建表。画师、标签切换时建的视图（asset_tags……）引用
    // term_effective_verdicts；重命名时用旧式 ALTER，不去检查、改写这些视图，它们按名字引用，重建后照常可用。
    conn.pragma_update(None, "legacy_alter_table", true)?;
    let rebuilt = conn.execute_batch(V2_VERDICTS);
    conn.pragma_update(None, "legacy_alter_table", false)?;
    rebuilt?;
    conn.execute_batch(V2)?;
    for table in PRIVATE {
        guard(conn, table)?;
    }
    Ok(())
}

/// 第 3 版：溯源证据整个来源按批次生效（`REPLACEABLE_SOURCES`）。term_evidence_sets 的 CHECK 要放宽，只能重建表；
/// 它有三张子表（批次的图、批次的行、有效集），一起重建：新子表的外键指向新父表，先删掉旧表和依赖它们的视图、
/// 触发器，再用新式 ALTER 改名（顺带改写子表的外键引用）。已有的溯源证据全部收进一个迁移批次（全库覆盖），
/// 迁移前后有效的证据不变；之后每个溯源包按快照替换。
/// 第 4 版（拆脚手架阶段 A，2026-09-29 用户决定）：切换时冻结的旧人工表不再保留。按各自的切换标记删：
/// 画师切过的删 `artist_verdicts`，标签切过的删 `legacy_asset_tags`，角色切过的删 `legacy_*` 冻结副本。
/// 没切过的库这些表还是活的（或不存在），不动。之后的切换本身也不再留冻结副本；回退靠切换前的自动备份。
fn v4(conn: &Connection) -> Result<()> {
    let switched = |key: &str| -> Result<bool> {
        Ok(conn
            .query_row("SELECT value='core' FROM meta WHERE key=?1", [key], |r| r.get(0))
            .optional()?
            .unwrap_or(false))
    };
    if switched(crate::store::ARTISTS_IN_CORE)? {
        conn.execute_batch("DROP TABLE IF EXISTS artist_verdicts")?;
    }
    if switched(crate::store::LABELS_IN_CORE)? {
        conn.execute_batch("DROP TABLE IF EXISTS legacy_asset_tags")?;
    }
    if switched(crate::legacy_characters::CHARACTERS_IN_CORE)? {
        for table in crate::legacy_characters::FROZEN_COPIES {
            conn.execute_batch(&format!("DROP TABLE IF EXISTS {table}"))?;
        }
    }
    Ok(())
}

/// 第 5 版（名字投影，docs/TERMS_NAMES.md「方案」）：名字的显示投影两张表（N1，影子阶段由 `names-project` 整表重建），
/// 按文字键的名字与别名两张主数据表（N2，`sync-names` 收编）。
fn v5(conn: &Connection) -> Result<()> {
    conn.execute_batch(crate::names_view::TABLES)?;
    conn.execute_batch(crate::legacy_names::TABLES)?;
    Ok(())
}

fn v3(conn: &Connection) -> Result<()> {
    conn.execute_batch(V3_SETS)?;
    conn.execute_batch(V3)?;
    let has: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM term_evidence WHERE source='provenance' AND asset_id IS NOT NULL)",
        [],
        |r| r.get(0),
    )?;
    if has {
        conn.execute(
            "INSERT INTO term_evidence_sets(producer,source,method,scope) VALUES ('migration:v3','provenance','*','all')",
            [],
        )?;
        let set = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO term_evidence_set_rows(set_id,evidence_id) SELECT ?1,id FROM term_evidence WHERE source='provenance' AND asset_id IS NOT NULL",
            [set],
        )?;
        crate::store::rebuild_active_evidence(conn)?;
    }
    Ok(())
}

/// 第 10 版增加人工标记，以及 WD 扩展、CCIP 长尾和按标签替换的例图候选批次。复制原批次与有效集。
fn v10(conn: &Connection) -> Result<()> {
    const V3_SOURCES: &str = "'wd.wd_character','ccip_index.char_ext'";
    const V10_SOURCES: &str =
        "'wd.wd_character','ccip_index.char_ext','wd_extension.linear_head','ccip_longtail.anchor_p75'";
    conn.execute_batch(
        &V3_SETS
            .replace(V3_SOURCES, V10_SOURCES)
            .replace("CHECK((source='provenance'", "CHECK((source='example' AND method<>'*') OR (source='provenance'"),
    )?;
    conn.execute_batch(
        &V3.replace(V3_SOURCES, V10_SOURCES)
            .replace("NOT (e.source='provenance' OR", "NOT (e.source IN ('provenance','example') OR"),
    )?;
    let kinds = Kind::ALL.map(|k| format!("'{}'", k.as_str())).join(",");
    conn.execute_batch(&format!(
        "DROP TRIGGER terms_kind_insert;
         DROP TRIGGER terms_kind_update;
         CREATE TRIGGER terms_kind_insert BEFORE INSERT ON term_records
          WHEN new.kind NOT IN ({kinds}) BEGIN SELECT RAISE(ABORT,'unknown term kind'); END;
         CREATE TRIGGER terms_kind_update BEFORE UPDATE OF kind ON term_records
          WHEN new.kind NOT IN ({kinds}) BEGIN SELECT RAISE(ABORT,'unknown term kind'); END;"
    ))?;
    let view: Option<String> = conn
        .query_row("SELECT sql FROM sqlite_schema WHERE type='view' AND name='variant_memberships'", [], |r| r.get(0))
        .optional()?;
    if let Some(view) = view.filter(|sql| !sql.contains("CCIP_LONGTAIL")) {
        // 扩展候选不能给已有主归属追加皮肤。核心独立测试库可以没有应用视图。
        let sql = crate::character_projection::scoped(
            &view,
            "v.uuid=m.uuid AND",
            "v.uuid=m.uuid AND v.source NOT IN ('WD_EXT','CCIP_LONGTAIL') AND",
        )?;
        conn.execute("DROP VIEW variant_memberships", [])?;
        conn.execute_batch(&sql)?;
    }
    conn.execute_batch(
        "CREATE TABLE galleries(
           id INTEGER PRIMARY KEY,
           name TEXT NOT NULL,
           params TEXT NOT NULL,
           cover TEXT,
           position INTEGER NOT NULL,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL);",
    )?;
    // 相似组的「看过了」（#427）：同一个 id 是一次标记，行是当时这组里看得到的图。
    conn.execute_batch(
        "CREATE TABLE similar_seen(
           id INTEGER NOT NULL,
           uuid TEXT NOT NULL,
           seen_at TEXT NOT NULL,
           PRIMARY KEY(id,uuid)) WITHOUT ROWID;
         CREATE INDEX similar_seen_uuid ON similar_seen(uuid);",
    )?;
    Ok(())
}

/// 第 11 版增加手选画廊。已删除的画廊编号不再分配给新画廊，避免旧筛选链接指到新画廊。
fn v11(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "ALTER TABLE galleries RENAME TO galleries_v10;
         CREATE TABLE galleries(
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           name TEXT NOT NULL,
           params TEXT NOT NULL,
           cover TEXT,
           position INTEGER NOT NULL,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           kind TEXT NOT NULL DEFAULT 'condition' CHECK(kind IN ('condition','manual')));
         INSERT INTO galleries(id,name,params,cover,position,created_at,updated_at)
           SELECT id,name,params,cover,position,created_at,updated_at FROM galleries_v10;
         DROP TABLE galleries_v10;
         CREATE TABLE gallery_members(
           gallery_id INTEGER NOT NULL REFERENCES galleries(id) ON DELETE CASCADE,
           uuid TEXT NOT NULL,
           PRIMARY KEY(gallery_id,uuid)) WITHOUT ROWID;",
    )?;
    Ok(())
}

const V3_SETS: &str = "
CREATE TABLE term_evidence_sets_v3(
 id INTEGER PRIMARY KEY,
 producer TEXT NOT NULL CHECK(length(producer) BETWEEN 1 AND 80),
 source TEXT NOT NULL,
 method TEXT NOT NULL,
 scope TEXT NOT NULL CHECK(scope IN ('all','assets')),
 package_id INTEGER REFERENCES term_packages(id),
 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
 CHECK((source='provenance' AND method='*') OR source||'.'||method IN ('wd.wd_character','ccip_index.char_ext')));
CREATE TABLE term_evidence_set_assets_v3(
 set_id INTEGER NOT NULL REFERENCES term_evidence_sets_v3(id),
 asset_id INTEGER NOT NULL REFERENCES term_assets(id),
 PRIMARY KEY(set_id,asset_id)) WITHOUT ROWID;
CREATE TABLE term_evidence_set_rows_v3(
 set_id INTEGER NOT NULL REFERENCES term_evidence_sets_v3(id),
 evidence_id INTEGER NOT NULL REFERENCES term_evidence(id),
 PRIMARY KEY(set_id,evidence_id)) WITHOUT ROWID;
CREATE TABLE term_active_evidence_v3(
 asset_id INTEGER NOT NULL REFERENCES term_assets(id),
 source TEXT NOT NULL,
 method TEXT NOT NULL,
 set_id INTEGER NOT NULL REFERENCES term_evidence_sets_v3(id),
 PRIMARY KEY(asset_id,source,method)) WITHOUT ROWID;
INSERT INTO term_evidence_sets_v3 SELECT id,producer,source,method,scope,package_id,created_at FROM term_evidence_sets;
INSERT INTO term_evidence_set_assets_v3 SELECT * FROM term_evidence_set_assets;
INSERT INTO term_evidence_set_rows_v3 SELECT * FROM term_evidence_set_rows;
INSERT INTO term_active_evidence_v3 SELECT * FROM term_active_evidence;
DROP VIEW term_current_evidence;
DROP TABLE term_active_evidence;
DROP TABLE term_evidence_set_rows;
DROP TABLE term_evidence_set_assets;
DROP TABLE term_evidence_sets;
ALTER TABLE term_evidence_sets_v3 RENAME TO term_evidence_sets;
ALTER TABLE term_evidence_set_assets_v3 RENAME TO term_evidence_set_assets;
ALTER TABLE term_evidence_set_rows_v3 RENAME TO term_evidence_set_rows;
ALTER TABLE term_active_evidence_v3 RENAME TO term_active_evidence;
";

const V3: &str = "
CREATE INDEX term_evidence_set_rows_evidence ON term_evidence_set_rows(evidence_id);
CREATE TRIGGER term_evidence_set_rows_match BEFORE INSERT ON term_evidence_set_rows
 WHEN NOT EXISTS(SELECT 1 FROM term_evidence e JOIN term_evidence_sets s ON s.id=new.set_id
  WHERE e.id=new.evidence_id AND e.asset_id IS NOT NULL AND e.source=s.source AND (e.method=s.method OR s.method='*')
  AND (s.scope='all' OR EXISTS(SELECT 1 FROM term_evidence_set_assets x WHERE x.set_id=s.id AND x.asset_id=e.asset_id)))
 BEGIN SELECT RAISE(ABORT,'set rows must be image evidence of the set''s source and scope'); END;
CREATE TRIGGER term_evidence_sets_append_only_update BEFORE UPDATE ON term_evidence_sets
 BEGIN SELECT RAISE(ABORT,'evidence sets are append-only'); END;
CREATE TRIGGER term_evidence_sets_append_only_delete BEFORE DELETE ON term_evidence_sets
 BEGIN SELECT RAISE(ABORT,'evidence sets are append-only'); END;
CREATE TRIGGER term_evidence_set_assets_append_only_update BEFORE UPDATE ON term_evidence_set_assets
 BEGIN SELECT RAISE(ABORT,'evidence sets are append-only'); END;
CREATE TRIGGER term_evidence_set_assets_append_only_delete BEFORE DELETE ON term_evidence_set_assets
 BEGIN SELECT RAISE(ABORT,'evidence sets are append-only'); END;
CREATE TRIGGER term_evidence_set_rows_append_only_update BEFORE UPDATE ON term_evidence_set_rows
 BEGIN SELECT RAISE(ABORT,'evidence sets are append-only'); END;
CREATE TRIGGER term_evidence_set_rows_append_only_delete BEFORE DELETE ON term_evidence_set_rows
 BEGIN SELECT RAISE(ABORT,'evidence sets are append-only'); END;
CREATE VIEW term_current_evidence AS
 SELECT e.* FROM term_evidence e
 WHERE e.asset_id IS NULL OR NOT (e.source='provenance' OR e.source||'.'||e.method IN ('wd.wd_character','ccip_index.char_ext'))
  OR EXISTS(SELECT 1 FROM term_active_evidence x JOIN term_evidence_set_rows r ON r.set_id=x.set_id AND r.evidence_id=e.id
   WHERE x.asset_id=e.asset_id AND x.source=e.source AND x.method IN (e.method,'*'));
";

/// 核心的私有表：只有核心在写事务里（持有 `WRITER_KEY`）才能写。
pub const PRIVATE: [&str; 3] = ["term_identity_baseline", "term_tree", "term_legacy_ids"];

/// 给一张表加写保护：没有 `WRITER_KEY` 时 INSERT / UPDATE / DELETE 一律拒绝。兼容表建出来时也用它。
pub fn guard(conn: &Connection, table: &str) -> Result<()> {
    if !table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(Error::Invalid(format!("表名不合法：{table}")));
    }
    for event in ["INSERT", "UPDATE", "DELETE"] {
        let low = event.to_ascii_lowercase();
        conn.execute_batch(&format!(
            "CREATE TRIGGER {table}_writer_{low} BEFORE {event} ON {table}
              WHEN NOT EXISTS(SELECT 1 FROM meta WHERE key='{WRITER_KEY}')
              BEGIN SELECT RAISE(ABORT,'{table} is written by the terms core'); END;"
        ))?;
    }
    Ok(())
}

/// 在核心的写事务里拿到钥匙。提交前必须 `commit_writing`（它先交还钥匙再提交）；回滚时钥匙随之消失。
pub fn begin_writing(tx: &Connection) -> Result<()> {
    crate::legacy_names::track(tx)?;
    tx.execute("INSERT OR REPLACE INTO meta(key,value) VALUES (?1,'1')", [WRITER_KEY])?;
    Ok(())
}

pub fn commit_writing(tx: rusqlite::Transaction<'_>) -> Result<()> {
    // 名字已在核心时，这个事务改到名字相关的数据就同事务补齐名字主数据、重建显示投影（docs/TERMS_NAMES.md「方案」）。
    crate::legacy_names::after_write(&tx)?;
    tx.execute("DELETE FROM meta WHERE key=?1", [WRITER_KEY])?;
    tx.commit()?;
    Ok(())
}

const V2_VERDICTS: &str = "
-- 非用户批次：没有 action 的人工状态（收编基线、相册导入、撤销审计、迁移）。不进撤销栈，不能撤。
-- anchor 是它在时间线上的位置：排在 id = anchor 的那一步之后、下一步之前。
CREATE TABLE term_batches(
 id INTEGER PRIMARY KEY,
 kind TEXT NOT NULL CHECK(kind IN ('baseline','album','undo_audit','migration')),
 anchor INTEGER NOT NULL CHECK(anchor>=0),
 label TEXT NOT NULL DEFAULT '',
 data TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(data) AND json_type(data)='object'),
 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
CREATE TRIGGER term_batches_append_only_update BEFORE UPDATE ON term_batches
 BEGIN SELECT RAISE(ABORT,'batches are append-only'); END;
CREATE TRIGGER term_batches_append_only_delete BEFORE DELETE ON term_batches
 BEGIN SELECT RAISE(ABORT,'batches are append-only'); END;

-- 裁决挂在一步 action 或一个批次上（恰好一个）。anchor：action 裁决 = action_id，批次裁决 = 批次的 anchor；
-- via 是原 origin 的细分（review / sweep / album / merge / migration / undo……），兼容表 assertions 的 origin 读它。
CREATE TABLE term_verdicts_v2(
 id INTEGER PRIMARY KEY,
 action_id INTEGER REFERENCES actions(id),
 asset_id INTEGER REFERENCES term_assets(id),
 term_id INTEGER REFERENCES terms(id),
 relation_id INTEGER REFERENCES term_relations(id),
 verdict TEXT NOT NULL CHECK(verdict IN ('yes','no','clear')),
 origin TEXT NOT NULL CHECK(origin IN ('user','album','legacy')),
 reason TEXT NOT NULL DEFAULT '',
 reviewed TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(reviewed) AND json_type(reviewed)='array'),
 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
 batch_id INTEGER REFERENCES term_batches(id),
 anchor INTEGER NOT NULL,
 via TEXT NOT NULL DEFAULT '' CHECK(length(via)<=40),
 CHECK((action_id IS NULL)<>(batch_id IS NULL)),
 CHECK(action_id IS NULL OR anchor=action_id),
 CHECK((asset_id IS NOT NULL AND term_id IS NOT NULL AND relation_id IS NULL)
    OR (asset_id IS NULL AND term_id IS NULL AND relation_id IS NOT NULL)));
INSERT INTO term_verdicts_v2(id,action_id,asset_id,term_id,relation_id,verdict,origin,reason,reviewed,created_at,batch_id,anchor,via)
 SELECT id,action_id,asset_id,term_id,relation_id,verdict,origin,reason,reviewed,created_at,NULL,action_id,'' FROM term_verdicts;
DROP VIEW term_relation_states;
DROP VIEW term_effective_verdicts;
DROP TABLE term_verdicts;
ALTER TABLE term_verdicts_v2 RENAME TO term_verdicts;
";

const V2: &str = "
CREATE INDEX term_verdicts_image ON term_verdicts(asset_id,term_id,id) WHERE asset_id IS NOT NULL;
CREATE INDEX term_verdicts_relation ON term_verdicts(relation_id,id) WHERE relation_id IS NOT NULL;
CREATE INDEX term_verdicts_action ON term_verdicts(action_id) WHERE action_id IS NOT NULL;
CREATE INDEX term_verdicts_batch ON term_verdicts(batch_id) WHERE batch_id IS NOT NULL;
CREATE TRIGGER term_verdicts_append_only_update BEFORE UPDATE ON term_verdicts
 BEGIN SELECT RAISE(ABORT,'verdicts are append-only; undo through actions'); END;
CREATE TRIGGER term_verdicts_append_only_delete BEFORE DELETE ON term_verdicts
 BEGIN SELECT RAISE(ABORT,'verdicts are append-only; undo through actions'); END;
CREATE TRIGGER term_verdicts_batch_anchor BEFORE INSERT ON term_verdicts
 WHEN new.batch_id IS NOT NULL AND new.anchor IS NOT (SELECT anchor FROM term_batches WHERE id=new.batch_id)
 BEGIN SELECT RAISE(ABORT,'a batch verdict sits at its batch anchor'); END;

-- 每个目标的有效裁决：批次裁决或未撤销的 action 裁决里，按时间线最新的一条；最新的是 clear 就没有。
-- 同一锚点上批次排在那一步之后。
CREATE VIEW term_effective_verdicts AS
 SELECT id,action_id,asset_id,term_id,relation_id,verdict,origin,reason,reviewed,created_at,batch_id,anchor,via FROM (
  SELECT v.*,row_number() OVER (PARTITION BY v.asset_id,v.term_id,v.relation_id
   ORDER BY v.anchor DESC,v.batch_id IS NOT NULL DESC,v.id DESC) AS rn
  FROM term_verdicts v LEFT JOIN actions a ON a.id=v.action_id WHERE v.batch_id IS NOT NULL OR a.undone=0)
 WHERE rn=1 AND verdict<>'clear';

CREATE VIEW term_relation_states AS
 SELECT r.id,r.a,r.b,r.kind,
  CASE v.verdict WHEN 'yes' THEN 'confirmed' WHEN 'no' THEN 'rejected' ELSE 'maybe' END AS state
 FROM term_relations r LEFT JOIN term_effective_verdicts v ON v.relation_id=r.id;

-- 每条关系最新的一条裁决，包括 clear（人工退回 proposed）：「未复核继承层级」看它的 reviewed。
CREATE VIEW term_relation_latest AS
 SELECT id,action_id,relation_id,verdict,origin,reason,reviewed,created_at,batch_id,anchor,via FROM (
  SELECT v.*,row_number() OVER (PARTITION BY v.relation_id
   ORDER BY v.anchor DESC,v.batch_id IS NOT NULL DESC,v.id DESC) AS rn
  FROM term_verdicts v LEFT JOIN actions a ON a.id=v.action_id
  WHERE v.relation_id IS NOT NULL AND (v.batch_id IS NOT NULL OR a.undone=0))
 WHERE rn=1;

-- 证据批次：可替换来源（REPLACEABLE）的一次成功运行。覆盖全库（all）或列出的图（assets）；
-- 覆盖到的图上，这个来源当前有效的证据就是本批次的行，没有行 = 成功的空结果。失败的运行不留批次。
CREATE TABLE term_evidence_sets(
 id INTEGER PRIMARY KEY,
 producer TEXT NOT NULL CHECK(length(producer) BETWEEN 1 AND 80),
 source TEXT NOT NULL,
 method TEXT NOT NULL,
 scope TEXT NOT NULL CHECK(scope IN ('all','assets')),
 package_id INTEGER REFERENCES term_packages(id),
 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
 CHECK(source||'.'||method IN ('wd.wd_character','ccip_index.char_ext')));
CREATE TABLE term_evidence_set_assets(
 set_id INTEGER NOT NULL REFERENCES term_evidence_sets(id),
 asset_id INTEGER NOT NULL REFERENCES term_assets(id),
 PRIMARY KEY(set_id,asset_id)) WITHOUT ROWID;
CREATE TABLE term_evidence_set_rows(
 set_id INTEGER NOT NULL REFERENCES term_evidence_sets(id),
 evidence_id INTEGER NOT NULL REFERENCES term_evidence(id),
 PRIMARY KEY(set_id,evidence_id)) WITHOUT ROWID;
CREATE INDEX term_evidence_set_rows_evidence ON term_evidence_set_rows(evidence_id);
CREATE TRIGGER term_evidence_set_rows_match BEFORE INSERT ON term_evidence_set_rows
 WHEN NOT EXISTS(SELECT 1 FROM term_evidence e JOIN term_evidence_sets s ON s.id=new.set_id
  WHERE e.id=new.evidence_id AND e.asset_id IS NOT NULL AND e.source=s.source AND e.method=s.method
  AND (s.scope='all' OR EXISTS(SELECT 1 FROM term_evidence_set_assets x WHERE x.set_id=s.id AND x.asset_id=e.asset_id)))
 BEGIN SELECT RAISE(ABORT,'set rows must be image evidence of the set''s source and scope'); END;
CREATE TRIGGER term_evidence_sets_append_only_update BEFORE UPDATE ON term_evidence_sets
 BEGIN SELECT RAISE(ABORT,'evidence sets are append-only'); END;
CREATE TRIGGER term_evidence_sets_append_only_delete BEFORE DELETE ON term_evidence_sets
 BEGIN SELECT RAISE(ABORT,'evidence sets are append-only'); END;
CREATE TRIGGER term_evidence_set_assets_append_only_update BEFORE UPDATE ON term_evidence_set_assets
 BEGIN SELECT RAISE(ABORT,'evidence sets are append-only'); END;
CREATE TRIGGER term_evidence_set_assets_append_only_delete BEFORE DELETE ON term_evidence_set_assets
 BEGIN SELECT RAISE(ABORT,'evidence sets are append-only'); END;
CREATE TRIGGER term_evidence_set_rows_append_only_update BEFORE UPDATE ON term_evidence_set_rows
 BEGIN SELECT RAISE(ABORT,'evidence sets are append-only'); END;
CREATE TRIGGER term_evidence_set_rows_append_only_delete BEFORE DELETE ON term_evidence_set_rows
 BEGIN SELECT RAISE(ABORT,'evidence sets are append-only'); END;

-- 有效集：每张图、每个可替换来源当前生效的批次（覆盖它的最新一个）。可由上面三张表重建（store::rebuild_active_evidence）。
CREATE TABLE term_active_evidence(
 asset_id INTEGER NOT NULL REFERENCES term_assets(id),
 source TEXT NOT NULL,
 method TEXT NOT NULL,
 set_id INTEGER NOT NULL REFERENCES term_evidence_sets(id),
 PRIMARY KEY(asset_id,source,method)) WITHOUT ROWID;

-- 当前有效的证据：可替换来源只取有效集里的行，其余来源全部有效。投影读它。
CREATE VIEW term_current_evidence AS
 SELECT e.* FROM term_evidence e
 WHERE e.source||'.'||e.method NOT IN ('wd.wd_character','ccip_index.char_ext')
  OR EXISTS(SELECT 1 FROM term_active_evidence x JOIN term_evidence_set_rows r ON r.set_id=x.set_id AND r.evidence_id=e.id
   WHERE x.asset_id=e.asset_id AND x.source=e.source AND x.method=e.method);

-- 角色判定器的私有状态（写保护，见 guard）。

-- = identity_baseline（活表：新身份照旧追加）。seq 保留原 rowid 顺序，_graph 按它遍历。
CREATE TABLE term_identity_baseline(
 seq INTEGER PRIMARY KEY,
 id TEXT NOT NULL UNIQUE,
 parent_id TEXT,
 variant TEXT,
 kind TEXT,
 merged_into TEXT);

-- entities 的结构状态与 work_review。parent_id、merged_into：作品是状态；人物、皮肤在策略未设置时是直接状态，
-- hybrid 下是身份图的投影结果。work_text 是 entities.work 原样。review_* 三列是 work_review 的原值，
-- 没有 work_review 行时三列都为空。键都是词条 key（= 原 entities.id），原样保存。
CREATE TABLE term_tree(
 term_id INTEGER PRIMARY KEY REFERENCES terms(id),
 parent_id TEXT,
 merged_into TEXT,
 work_id TEXT,
 work_text TEXT NOT NULL DEFAULT '',
 review_status TEXT,
 review_note TEXT,
 parent_locked INTEGER,
 CHECK((review_status IS NULL)=(review_note IS NULL) AND (review_status IS NULL)=(parent_locked IS NULL)));
CREATE INDEX term_tree_parent ON term_tree(parent_id) WHERE parent_id IS NOT NULL;
CREATE INDEX term_tree_work ON term_tree(work_id) WHERE work_id IS NOT NULL;

-- 旧 id ↔ 核心 id：identity_relations.id ↔ term_relations.id，identity_decisions.id ↔ term_verdicts.id。
-- 旧步骤 before_json 里的 relation_id、decision_id 经它对应。
CREATE TABLE term_legacy_ids(
 kind TEXT NOT NULL CHECK(kind IN ('relation','decision')),
 legacy_id INTEGER NOT NULL,
 core_id INTEGER NOT NULL,
 PRIMARY KEY(kind,legacy_id),
 UNIQUE(kind,core_id)) WITHOUT ROWID;
";

/// 当前库里的词条 schema 版本；没有建过为 0。
pub fn version(conn: &Connection) -> Result<i64> {
    let has_meta: bool =
        conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='meta')", [], |r| {
            r.get(0)
        })?;
    if !has_meta {
        return Ok(0);
    }
    let value: Option<String> =
        conn.query_row("SELECT value FROM meta WHERE key='terms_schema'", [], |r| r.get(0)).optional()?;
    match value {
        None => Ok(0),
        Some(v) => v.parse().map_err(|_| Error::Schema(format!("terms_schema 不是整数：{v}"))),
    }
}

/// 命令执行前核对版本：没有建过或版本不同都拒绝，不在撤销等命令里顺手迁移。
pub fn require_current(conn: &Connection) -> Result<()> {
    match version(conn)? {
        SCHEMA_VERSION => Ok(()),
        0 => Err(Error::Schema("这个库还没有词条表".into())),
        v => Err(Error::Schema(format!("库里的词条表是第 {v} 版，程序是第 {SCHEMA_VERSION} 版"))),
    }
}

/// 建库或升级到 `SCHEMA_VERSION`，整个过程一个事务：失败不留下半套表。返回升级前的版本。
/// 库的版本比程序新时拒绝，不降级。
pub fn migrate_with_backup(conn: &mut Connection, backup: &crate::backup::RequiredBackup) -> Result<i64> {
    let before = version(conn)?;
    if before > 0 && before < SCHEMA_VERSION {
        backup_before_upgrade(conn, before, backup)?;
    }
    let before = migrate_to(conn, SCHEMA_VERSION)?;
    compact_pending(conn);
    Ok(before)
}

/// 升级到指定版本（测试用它造旧版本的库）。
pub(crate) fn migrate_to(conn: &mut Connection, target: i64) -> Result<i64> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let before = migrate_in(&tx, target)?;
    tx.commit()?;
    Ok(before)
}

/// 宿主的夹具迁移和部分迁移同样要求这项必需的备份能力。
pub fn migrate_to_with_backup(
    conn: &mut Connection,
    target: i64,
    backup: &crate::backup::RequiredBackup,
) -> Result<i64> {
    if !(0..=SCHEMA_VERSION).contains(&target) {
        return Err(Error::Invalid("不支持的词条目标版本".into()));
    }
    let before = version(conn)?;
    if before > 0 && before < target {
        backup_before_upgrade(conn, before, backup)?;
    }
    migrate_to(conn, target)
}
/// 在宿主已持有的写事务之前，对已提交快照完成的备份。
/// 字段私有，不执行核心的 SQLite 复制就无法构造。
pub struct PreparedUpgrade {
    source: String,
    before: i64,
    path: Option<std::path::PathBuf>,
}
impl PreparedUpgrade {
    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }
}
pub fn prepare_upgrade_backup(
    snapshot: &Connection,
    before: i64,
    backup: &crate::backup::RequiredBackup,
) -> Result<PreparedUpgrade> {
    Ok(PreparedUpgrade {
        source: snapshot.path().unwrap_or("").to_owned(),
        before,
        path: backup_before_upgrade(snapshot, before, backup)?,
    })
}
pub fn migrate_prepared_in(tx: &rusqlite::Transaction<'_>, target: i64, prepared: &PreparedUpgrade) -> Result<i64> {
    if !(0..=SCHEMA_VERSION).contains(&target) {
        return Err(Error::Invalid("不支持的词条目标版本".into()));
    }
    if tx.path().unwrap_or("") != prepared.source || version(tx)? != prepared.before {
        return Err(Error::Conflict("升级预检后词条版本已改变，请重新预检".into()));
    }
    migrate_in(tx, target)
}

/// 在必需的备份之后，于宿主现有的事务里停用过时的身份表。
/// 这个破坏性的版本步骤本身特意设为私有。
/// ```compile_fail
/// let c = rusqlite::Connection::open_in_memory().unwrap();
/// illustash_core::schema::v7(&c).unwrap();
/// ```
pub fn retire_identity_calibration_in(tx: &rusqlite::Transaction<'_>, prepared: &PreparedUpgrade) -> Result<()> {
    if tx.path().unwrap_or("") != prepared.source || version(tx)? != prepared.before {
        return Err(Error::Conflict("升级预检后词条版本已改变，请重新预检".into()));
    }
    v7(tx)
}

/// 在调用方已持有的写事务里执行原样不变的迁移步骤。
/// 备份、提交和提交后的维护由调用方负责；这里从不开启嵌套事务。
pub(crate) fn migrate_in(tx: &rusqlite::Transaction<'_>, target: i64) -> Result<i64> {
    tx.execute_batch(SHARED)?;
    let before = version(tx)?;
    if before > SCHEMA_VERSION {
        return Err(Error::Schema(format!("库里的词条表是第 {before} 版，比程序支持的第 {SCHEMA_VERSION} 版新")));
    }
    if before >= target {
        tx.execute("DELETE FROM meta WHERE key=?1", [WRITER_KEY])?;
        return Ok(before);
    }
    for step in &MIGRATIONS[before as usize..target as usize] {
        step(tx)?;
    }
    tx.execute("INSERT OR REPLACE INTO meta(key,value) VALUES ('terms_schema',?1)", [target.to_string()])?;
    // 提交过的钥匙只可能是某条写路径漏删留下的：留着就等于关掉了写保护。
    tx.execute("DELETE FROM meta WHERE key=?1", [WRITER_KEY])?;
    Ok(before)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn fresh() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        crate::configure(&c).unwrap();
        migrate(&mut c).unwrap();
        c
    }

    /// 第 4 版：按切换标记删掉冻结的旧人工表；没切换的库里 `artist_verdicts` 是活表，不动。
    fn compaction_fixture(c: &mut Connection) {
        migrate_to(c, 5).unwrap();
        c.execute_batch("CREATE TABLE candidates(uuid TEXT NOT NULL,entity TEXT NOT NULL,source TEXT NOT NULL,score REAL NOT NULL,PRIMARY KEY(uuid,entity,source));
            CREATE TABLE family_evidence(uuid TEXT NOT NULL,entity TEXT NOT NULL,score REAL,PRIMARY KEY(entity,uuid));
            CREATE INDEX candidates_entity ON candidates(entity);
            CREATE VIEW compact_view AS SELECT * FROM candidates;
            INSERT INTO candidates VALUES ('图片','角色','WD',0.91);
            INSERT INTO family_evidence VALUES ('图片','角色',NULL);
            CREATE TABLE shadow_candidates AS SELECT * FROM candidates;
            INSERT INTO meta VALUES ('terms_characters','core');").unwrap();
        guard(c, "candidates").unwrap();
        guard(c, "family_evidence").unwrap();
    }

    #[test]
    fn compact_upgrade_keeps_rows_guards_views_and_a_wal_backup() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let folder = std::env::temp_dir().join(format!("terms-compact-{stamp}"));
        std::fs::create_dir(&folder).unwrap();
        let mut c = crate::open(&folder.join("library.sqlite")).unwrap();
        c.pragma_update(None, "journal_mode", "WAL").unwrap();
        compaction_fixture(&mut c);
        assert_eq!(migrate(&mut c).unwrap(), 5);
        let backups: Vec<_> =
            std::fs::read_dir(folder.join("library-backups")).unwrap().map(|p| p.unwrap().path()).collect();
        assert_eq!(backups.len(), 1);
        let old = crate::sqlite::open(&backups[0]).unwrap();
        assert_eq!(version(&old).unwrap(), 5);
        for table in ["candidates", "family_evidence"] {
            let sql: String = c.query_row("SELECT sql FROM sqlite_master WHERE name=?", [table], |r| r.get(0)).unwrap();
            assert!(sql.contains("WITHOUT ROWID"));
            assert!(c.execute(&format!("DELETE FROM {table}"), []).is_err());
        }
        let row = |db: &Connection| {
            db.query_row("SELECT uuid,entity,source,score FROM compact_view", [], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, f64>(3)?))
            })
            .unwrap()
        };
        assert_eq!(row(&c), row(&old));
        assert!(c.query_row("SELECT score IS NULL FROM family_evidence", [], |r| r.get::<_, bool>(0)).unwrap());
        assert_eq!(c.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0)).unwrap(), "ok");
        assert_eq!(
            c.query_row("SELECT count(*) FROM sqlite_master WHERE name='shadow_candidates'", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(migrate(&mut c).unwrap(), SCHEMA_VERSION);
        assert_eq!(std::fs::read_dir(folder.join("library-backups")).unwrap().count(), 1);
        drop(old);
        drop(c);
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn compact_safety_accepts_indexed_and_integer_key_tables() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE keyed(id INTEGER PRIMARY KEY,value BLOB);
            CREATE TABLE indexed(value BLOB); CREATE INDEX indexed_value ON indexed(value);
            CREATE TABLE composite(a TEXT,b TEXT,PRIMARY KEY(a,b)) WITHOUT ROWID;
            INSERT INTO indexed(rowid,value) VALUES (27,x'00ff');",
        )
        .unwrap();
        assert!(vacuum_preserves_rowids(&c).unwrap());
        c.execute_batch("VACUUM").unwrap();
        assert_eq!(c.query_row("SELECT rowid FROM indexed", [], |r| r.get::<_, i64>(0)).unwrap(), 27);
        c.execute_batch("CREATE TABLE unindexed(value BLOB)").unwrap();
        assert!(!vacuum_preserves_rowids(&c).unwrap());
    }

    #[test]
    fn compact_maintenance_keeps_unindexed_extension_rowids() {
        let c = fresh();
        c.execute_batch(
            "CREATE TABLE research_notes(value BLOB);
            INSERT INTO research_notes(rowid,value) VALUES (27,x'00ff');
            INSERT INTO meta VALUES ('terms_compact_pending','1');",
        )
        .unwrap();
        compact_pending(&c);
        assert_eq!(
            c.query_row("SELECT rowid,value FROM research_notes", [], |r| Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Vec<u8>>(1)?
            )))
            .unwrap(),
            (27, vec![0, 255])
        );
        assert!(c
            .query_row("SELECT value FROM meta WHERE key='terms_compact_pending'", [], |r| r.get::<_, String>(0))
            .optional()
            .unwrap()
            .is_none());
        // 这个安全判断不阻止普通命令，也不要求迁移。
        assert_eq!(version(&c).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn compact_upgrade_failure_rolls_back_every_schema_change() {
        let mut c = Connection::open_in_memory().unwrap();
        compaction_fixture(&mut c);
        c.execute_batch(&format!(
            "CREATE TRIGGER fail_upgrade BEFORE INSERT ON meta WHEN new.key='terms_schema' AND new.value='{SCHEMA_VERSION}'
            BEGIN SELECT RAISE(ABORT,'模拟迁移失败'); END;"
        ))
        .unwrap();
        let schema = |c: &Connection| {
            c.prepare("SELECT type,name,coalesce(sql,'') FROM sqlite_master ORDER BY type,name")
                .unwrap()
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let before = schema(&c);
        assert!(migrate(&mut c).is_err());
        assert_eq!(version(&c).unwrap(), 5);
        assert_eq!(schema(&c), before);
        assert_eq!(c.query_row("SELECT score FROM candidates", [], |r| r.get::<_, f64>(0)).unwrap(), 0.91);
        assert!(!c.pragma_query_value(None, "legacy_alter_table", |r| r.get::<_, bool>(0)).unwrap());
    }

    #[test]
    fn v3_to_v4_drops_only_the_frozen_copies_of_switched_parts() {
        const TABLES: [&str; 8] = [
            "artist_verdicts",
            "legacy_asset_tags",
            "legacy_assertions",
            "legacy_identity_relations",
            "legacy_identity_evidence",
            "legacy_identity_decisions",
            "legacy_identity_decision_evidence",
            "legacy_identity_baseline",
        ];
        let library = |flags: &[&str]| {
            let mut c = Connection::open_in_memory().unwrap();
            crate::configure(&c).unwrap();
            migrate_to(&mut c, 3).unwrap();
            for t in TABLES {
                c.execute_batch(&format!("CREATE TABLE {t}(x); INSERT INTO {t} VALUES (1);")).unwrap();
            }
            for f in flags {
                c.execute("INSERT INTO meta VALUES (?1,'core')", [f]).unwrap();
            }
            c
        };
        let left = |c: &Connection| -> Vec<String> {
            TABLES
                .into_iter()
                .filter(|t| {
                    c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name=?1)", [t], |r| r.get(0)).unwrap()
                })
                .map(String::from)
                .collect()
        };
        let mut none = library(&[]);
        assert_eq!(migrate(&mut none).unwrap(), 3);
        assert_eq!(left(&none).len(), TABLES.len(), "没切换的库什么都不删");
        let mut artists = library(&[crate::store::ARTISTS_IN_CORE]);
        migrate(&mut artists).unwrap();
        assert!(!left(&artists).contains(&"artist_verdicts".to_string()));
        assert_eq!(left(&artists).len(), TABLES.len() - 1);
        let mut all = library(&[
            crate::store::ARTISTS_IN_CORE,
            crate::store::LABELS_IN_CORE,
            crate::legacy_characters::CHARACTERS_IN_CORE,
        ]);
        migrate(&mut all).unwrap();
        assert!(left(&all).is_empty());
        assert_eq!(version(&all).unwrap(), SCHEMA_VERSION);
        assert_eq!(migrate(&mut all).unwrap(), SCHEMA_VERSION, "再跑一次什么都不做");
    }

    #[test]
    fn kinds_round_trip_and_are_accepted_by_the_schema() {
        let c = fresh();
        for (i, kind) in Kind::ALL.into_iter().enumerate() {
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
            c.execute("INSERT INTO terms(key,kind,origin) VALUES (?1,?2,'test')", (format!("k{i}"), kind.as_str()))
                .unwrap();
        }
        assert!(c.execute("INSERT INTO terms(key,kind,origin) VALUES ('x','tag','test')", []).is_err());
        assert!(c.execute("UPDATE terms SET kind='tag' WHERE key='k0'", []).is_err());
        assert!(c.execute("UPDATE terms SET key='k0b' WHERE key='k0'", []).is_err());
        c.execute("UPDATE terms SET kind='label' WHERE key='k0'", []).unwrap();
        for kind in RelationKind::ALL {
            assert_eq!(RelationKind::parse(kind.as_str()), Some(kind));
        }
    }

    /// 第 7 版：删掉只有 v2 用的表和 meta；别的身份表、别的 meta 不动。再跑一次什么都不做。
    #[test]
    fn v6_to_v7_drops_what_only_v2_used() {
        let mut c = Connection::open_in_memory().unwrap();
        crate::configure(&c).unwrap();
        migrate_to(&mut c, 6).unwrap();
        c.execute_batch(
            "CREATE TABLE identity_claims(uuid TEXT PRIMARY KEY);
             CREATE TABLE identity_calibration(id TEXT PRIMARY KEY);
             CREATE TABLE identity_scopes(uuid TEXT,entity TEXT);
             INSERT INTO identity_claims VALUES ('a');
             INSERT INTO meta VALUES ('identity_calibration_active','x'),('identity_bank_error','y'),
               ('identity_bank','/bank'),('identity_policy','hybrid');",
        )
        .unwrap();
        assert_eq!(migrate(&mut c).unwrap(), 6);
        let tables: Vec<String> = c
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'identity_%' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(tables, ["identity_scopes"]);
        let keys: Vec<String> = c
            .prepare("SELECT key FROM meta WHERE key LIKE 'identity_%' ORDER BY key")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(keys, ["identity_bank", "identity_policy"]);
        assert_eq!(migrate(&mut c).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn migrate_is_idempotent_and_records_the_version() {
        let mut c = fresh();
        assert_eq!(version(&c).unwrap(), SCHEMA_VERSION);
        assert_eq!(migrate(&mut c).unwrap(), SCHEMA_VERSION);
        let tables: i64 = c
            .query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name LIKE 'term%'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tables, 21); // 第 5 版加了名字投影与按文字键名字的四张表
    }

    #[test]
    fn coexists_with_the_python_library_tables() {
        // 与 tests/reference/web/store.py（#9 已删除） 相同的 meta/actions 定义，以及一段已有历史。
        let mut c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS actions(id INTEGER PRIMARY KEY AUTOINCREMENT,label TEXT NOT NULL,before_json TEXT NOT NULL,ts TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,undone INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE IF NOT EXISTS entities(id TEXT PRIMARY KEY,name TEXT NOT NULL,work TEXT NOT NULL DEFAULT '',external_key TEXT UNIQUE);
             INSERT INTO meta VALUES ('schema','36');
             INSERT INTO actions(label,before_json) VALUES ('已确认 1 张','[]');",
        )
        .unwrap();
        assert_eq!(migrate(&mut c).unwrap(), 0);
        let schema: String = c.query_row("SELECT value FROM meta WHERE key='schema'", [], |r| r.get(0)).unwrap();
        let actions: i64 = c.query_row("SELECT count(*) FROM actions", [], |r| r.get(0)).unwrap();
        assert_eq!((schema.as_str(), actions), ("36", 1));
    }

    #[test]
    fn refuses_a_newer_schema_and_leaves_nothing_behind() {
        let mut c = Connection::open_in_memory().unwrap();
        c.execute_batch(SHARED).unwrap();
        c.execute("INSERT INTO meta VALUES ('terms_schema','99')", []).unwrap();
        assert!(matches!(migrate(&mut c), Err(Error::Schema(_))));
        let tables: i64 =
            c.query_row("SELECT count(*) FROM sqlite_master WHERE name='terms'", [], |r| r.get(0)).unwrap();
        assert_eq!(tables, 0);
    }

    #[test]
    fn a_failed_migration_rolls_back() {
        let mut c = Connection::open_in_memory().unwrap();
        // 已有同名对象让 V1 中途失败：不能留下前半截的表，也不能写版本号。
        c.execute_batch("CREATE TABLE term_projection(x);").unwrap();
        assert!(migrate(&mut c).is_err());
        let n: i64 = c
            .query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('terms','term_assets','meta')", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn evidence_and_verdicts_are_append_only() {
        let c = fresh();
        c.execute_batch(
            "INSERT INTO term_assets(namespace,key) VALUES ('photos','a');
             INSERT INTO terms(key,kind,origin) VALUES ('t','artist','test');
             INSERT INTO actions(label,before_json) VALUES ('x','[]');
             INSERT INTO term_evidence(asset_id,term_id,source,method,version,fingerprint)
               VALUES (1,1,'provenance','pixel_hash','1',zeroblob(16));
             INSERT INTO term_verdicts(action_id,anchor,asset_id,term_id,verdict,origin) VALUES (1,1,1,1,'no','user');",
        )
        .unwrap();
        assert!(c.execute("UPDATE term_evidence SET score=1", []).is_err());
        assert!(c.execute("DELETE FROM term_evidence", []).is_err());
        assert!(c.execute("UPDATE term_verdicts SET verdict='yes'", []).is_err());
        assert!(c.execute("DELETE FROM term_verdicts", []).is_err());
        // 证据不能冒充人，裁决不能来自机器来源。
        assert!(c
            .execute(
                "INSERT INTO term_evidence(asset_id,term_id,source,method,version,fingerprint) VALUES (1,1,'user','x','1',zeroblob(16))",
                []
            )
            .is_err());
        assert!(c
            .execute(
                "INSERT INTO term_verdicts(action_id,anchor,asset_id,term_id,verdict,origin) VALUES (1,1,1,1,'no','wd')",
                []
            )
            .is_err());
        // 目标二选一。
        assert!(c
            .execute(
                "INSERT INTO term_evidence(asset_id,source,method,version,fingerprint) VALUES (1,'wd','x','1',zeroblob(16))",
                []
            )
            .is_err());
    }

    /// v1 的库：画师、标签已切换（asset_tags 等视图引用 term_effective_verdicts），有撤销过的步骤。
    fn v1_library() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        crate::configure(&c).unwrap();
        migrate_to(&mut c, 1).unwrap();
        c.execute_batch(
            "CREATE TABLE assets(uuid TEXT PRIMARY KEY);
             INSERT INTO assets VALUES ('a'),('b');
             INSERT INTO term_assets(namespace,key) VALUES ('library','a'),('library','b');
             INSERT INTO terms(key,kind,origin) VALUES ('blue','concept','user'),('some-artist','artist','user');
             INSERT INTO term_names(term_id,lang,value,role,source) VALUES (1,'','blue','display','user');
             INSERT INTO actions(label,before_json) VALUES ('1','[]'),('2','[]'),('3','[]');
             UPDATE actions SET undone=1 WHERE id=3;
             INSERT INTO term_verdicts(action_id,asset_id,term_id,verdict,origin,reason) VALUES
              (1,1,1,'yes','legacy','asset_tags'),(1,2,1,'yes','legacy','asset_tags'),(2,2,1,'clear','user',''),
              (2,1,2,'no','user',''),(3,1,2,'yes','user','');",
        )
        .unwrap();
        c.execute_batch(crate::legacy_labels::VIEWS).unwrap();
        c
    }

    fn rows(c: &Connection, sql: &str) -> Vec<String> {
        let mut stmt = c.prepare(sql).unwrap();
        let n = stmt.column_count();
        stmt.query_map([], |r| {
            (0..n)
                .map(|i| r.get_ref(i).map(|v| format!("{v:?}")))
                .collect::<rusqlite::Result<Vec<_>>>()
                .map(|v| v.join("|"))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    }

    const V1_EFFECTIVE: &str =
        "SELECT id,action_id,asset_id,term_id,relation_id,verdict,origin,reason,reviewed,created_at FROM term_effective_verdicts ORDER BY id";

    #[test]
    fn v1_to_v2_keeps_verdicts_and_the_views_built_on_them() {
        let mut c = v1_library();
        let effective = rows(&c, V1_EFFECTIVE);
        let tags = rows(&c, "SELECT * FROM asset_tags ORDER BY uuid");
        let states = rows(&c, "SELECT * FROM term_relation_states");
        assert_eq!(tags.len(), 1, "b 的标签已被收回");
        assert_eq!(migrate(&mut c).unwrap(), 1);
        assert_eq!(version(&c).unwrap(), SCHEMA_VERSION);
        assert_eq!(rows(&c, V1_EFFECTIVE), effective);
        assert_eq!(rows(&c, "SELECT * FROM asset_tags ORDER BY uuid"), tags);
        assert_eq!(rows(&c, "SELECT * FROM term_relation_states"), states);
        // 原有的裁决都是 action 裁决：锚点就是 action_id。
        assert_eq!(
            rows(&c, "SELECT count(*) FROM term_verdicts WHERE anchor=action_id AND batch_id IS NULL AND via=''"),
            ["Integer(5)"]
        );
        let legacy: bool = c.pragma_query_value(None, "legacy_alter_table", |r| r.get(0)).unwrap();
        assert!(!legacy, "迁移后恢复新式 ALTER");
        // 追加写仍然成立。
        assert!(c.execute("DELETE FROM term_verdicts", []).is_err());
        assert!(c.execute("UPDATE term_verdicts SET verdict='yes'", []).is_err());
    }

    #[test]
    fn a_failed_v2_leaves_the_v1_library_as_it_was() {
        for blocker in ["CREATE TABLE term_tree(x);", "CREATE TABLE term_batches(x);"] {
            let mut c = v1_library();
            let effective = rows(&c, V1_EFFECTIVE);
            let schema = rows(&c, "SELECT type,name,sql FROM sqlite_master ORDER BY name");
            c.execute_batch(blocker).unwrap();
            assert!(migrate(&mut c).is_err(), "{blocker}");
            c.execute_batch(&blocker.replace("CREATE TABLE", "DROP TABLE").replace("(x)", "")).unwrap();
            assert_eq!(version(&c).unwrap(), 1);
            assert_eq!(rows(&c, "SELECT type,name,sql FROM sqlite_master ORDER BY name"), schema);
            assert_eq!(rows(&c, V1_EFFECTIVE), effective);
            let legacy: bool = c.pragma_query_value(None, "legacy_alter_table", |r| r.get(0)).unwrap();
            assert!(!legacy);
            // 挡住的对象移走后再迁移就成功。
            assert_eq!(migrate(&mut c).unwrap(), 1);
        }
    }

    fn verdicts_db() -> Connection {
        let c = fresh();
        c.execute_batch(
            "INSERT INTO term_assets(namespace,key) VALUES ('library','a');
             INSERT INTO terms(key,kind,origin) VALUES ('p','character','legacy'),('q','character','legacy');
             INSERT INTO term_relations(a,b,kind) VALUES (1,2,'same_as');
             INSERT INTO actions(label,before_json) VALUES ('4','[]'),('5','[]'),('6','[]');",
        )
        .unwrap();
        c
    }

    fn effective(c: &Connection) -> Option<(String, Option<i64>, Option<i64>)> {
        c.query_row(
            "SELECT verdict,action_id,batch_id FROM term_effective_verdicts WHERE asset_id=1 AND term_id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .unwrap()
    }

    #[test]
    fn batch_verdicts_sit_on_the_timeline_after_their_anchor() {
        let c = verdicts_db();
        let batch = |kind: &str, anchor: i64| {
            c.execute("INSERT INTO term_batches(kind,anchor) VALUES (?1,?2)", params![kind, anchor]).unwrap();
            c.last_insert_rowid()
        };
        let by_batch = |b: i64, anchor: i64, verdict: &str| {
            c.execute(
                "INSERT INTO term_verdicts(batch_id,anchor,asset_id,term_id,verdict,origin,via) VALUES (?1,?2,1,1,?3,'album','album')",
                params![b, anchor, verdict],
            )
        };
        let by_action = |a: i64, verdict: &str| {
            c.execute(
                "INSERT INTO term_verdicts(action_id,anchor,asset_id,term_id,verdict,origin,via) VALUES (?1,?1,1,1,?2,'user','review')",
                params![a, verdict],
            )
        };
        // 基线（anchor 0）→ 第 2 步否定 → 撤销第 2 步回到基线。
        let base = batch("baseline", 0);
        by_batch(base, 0, "yes").unwrap();
        assert_eq!(effective(&c), Some(("yes".into(), None, Some(base))));
        by_action(2, "no").unwrap();
        assert_eq!(effective(&c), Some(("no".into(), Some(2), None)));
        c.execute("UPDATE actions SET undone=1 WHERE id=2", []).unwrap();
        assert_eq!(effective(&c), Some(("yes".into(), None, Some(base))));
        // 同一锚点上批次排在那一步之后；锚点更早的批次让位给后来的步骤。
        by_action(1, "no").unwrap();
        let album = batch("album", 1);
        by_batch(album, 1, "yes").unwrap();
        assert_eq!(effective(&c), Some(("yes".into(), None, Some(album))));
        by_action(3, "clear").unwrap();
        assert_eq!(effective(&c), None, "最新的是 clear 就没有有效裁决");
        // 约束：恰好挂一个；action 裁决的锚点是自己；批次裁决的锚点是批次的。
        assert!(by_batch(album, 2, "yes").is_err());
        assert!(c
            .execute("INSERT INTO term_verdicts(action_id,anchor,asset_id,term_id,verdict,origin) VALUES (1,2,1,1,'yes','user')", [])
            .is_err());
        assert!(c
            .execute(
                "INSERT INTO term_verdicts(action_id,batch_id,anchor,asset_id,term_id,verdict,origin) VALUES (1,1,1,1,1,'yes','user')",
                []
            )
            .is_err());
        assert!(c
            .execute(
                "INSERT INTO term_verdicts(anchor,asset_id,term_id,verdict,origin) VALUES (1,1,1,'yes','user')",
                []
            )
            .is_err());
        assert!(c.execute("UPDATE term_batches SET anchor=9", []).is_err());
        assert!(c.execute("DELETE FROM term_batches", []).is_err());
        assert!(c.execute("INSERT INTO term_batches(kind,anchor) VALUES ('user',1)", []).is_err());
    }

    #[test]
    fn the_latest_relation_verdict_includes_clear() {
        let c = verdicts_db();
        c.execute_batch(
            "INSERT INTO term_batches(kind,anchor) VALUES ('migration',0);
             INSERT INTO term_verdicts(batch_id,anchor,relation_id,verdict,origin,via) VALUES (1,0,1,'yes','legacy','migration');
             INSERT INTO term_verdicts(action_id,anchor,relation_id,verdict,origin,reviewed) VALUES (1,1,1,'clear','user','[7]');",
        )
        .unwrap();
        let latest = |c: &Connection| -> (String, String) {
            c.query_row("SELECT verdict,reviewed FROM term_relation_latest WHERE relation_id=1", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap()
        };
        assert_eq!(latest(&c), ("clear".into(), "[7]".into()));
        let state: String = c.query_row("SELECT state FROM term_relation_states WHERE id=1", [], |r| r.get(0)).unwrap();
        assert_eq!(state, "maybe", "退回 proposed：没有有效裁决");
        c.execute("UPDATE actions SET undone=1 WHERE id=1", []).unwrap();
        assert_eq!(latest(&c), ("yes".into(), "[]".into()));
    }

    #[test]
    fn private_tables_are_written_only_by_the_core() {
        let mut c = fresh();
        c.execute("INSERT INTO terms(key,kind,origin) VALUES ('w','work','legacy')", []).unwrap();
        let writes = [
            "INSERT INTO term_tree(term_id,work_text) VALUES (1,'Example')",
            "INSERT INTO term_identity_baseline(id,kind) VALUES ('w','character')",
            "INSERT INTO term_legacy_ids VALUES ('relation',3,1)",
        ];
        for sql in writes {
            let err = c.execute(sql, []).unwrap_err().to_string();
            assert!(err.contains("is written by the terms core"), "{sql}: {err}");
        }
        let tx = c.transaction().unwrap();
        begin_writing(&tx).unwrap();
        for sql in writes {
            tx.execute(sql, []).unwrap();
        }
        commit_writing(tx).unwrap();
        let key: bool =
            c.query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE key=?1)", [WRITER_KEY], |r| r.get(0)).unwrap();
        assert!(!key, "提交后钥匙已交还");
        for sql in [
            "UPDATE term_tree SET work_text='x'",
            "DELETE FROM term_identity_baseline",
            "UPDATE term_legacy_ids SET core_id=2",
            "DELETE FROM term_tree",
        ] {
            assert!(c.execute(sql, []).is_err(), "{sql}");
        }
        // 回滚的事务不留钥匙。
        {
            let tx = c.transaction().unwrap();
            begin_writing(&tx).unwrap();
        }
        assert!(c.execute("DELETE FROM term_tree", []).is_err());
        // 残留的钥匙（某条写路径漏删）在下次启动迁移时清掉。
        c.execute("INSERT INTO meta VALUES (?1,'1')", [WRITER_KEY]).unwrap();
        migrate(&mut c).unwrap();
        assert!(c.execute("DELETE FROM term_tree", []).is_err());
        // work_review 的三列同有同无。
        let tx = c.transaction().unwrap();
        begin_writing(&tx).unwrap();
        assert!(tx.execute("UPDATE term_tree SET review_status='pending' WHERE term_id=1", []).is_err());
        tx.execute("UPDATE term_tree SET review_status='pending',review_note='',parent_locked=1 WHERE term_id=1", [])
            .unwrap();
        commit_writing(tx).unwrap();
        assert!(guard(&c, "x; DROP TABLE terms").is_err());
    }

    #[test]
    fn v9_to_v10_preserves_old_batches_and_keeps_extensions_independently_replaceable() {
        let mut c = Connection::open_in_memory().unwrap();
        crate::configure(&c).unwrap();
        migrate_to(&mut c, 9).unwrap();
        c.execute_batch(
            "INSERT INTO term_assets(namespace,key) VALUES ('library','a');
             INSERT INTO terms(key,kind,origin) VALUES ('p','character','legacy');
             INSERT INTO term_evidence(asset_id,term_id,source,method,version,fingerprint) VALUES
              (1,1,'wd','wd_character','base',zeroblob(16)),
              (1,1,'ccip_index','char_ext','base',zeroblob(16)),
              (1,1,'provenance','ocr','base',zeroblob(16));
             INSERT INTO term_evidence_sets(producer,source,method,scope) VALUES
              ('t','wd','wd_character','all'),('t','ccip_index','char_ext','all'),('t','provenance','*','all');
             INSERT INTO term_evidence_set_rows VALUES (1,1),(2,2),(3,3);",
        )
        .unwrap();
        crate::store::rebuild_active_evidence(&c).unwrap();
        let ids = |c: &Connection| rows(c, "SELECT id FROM term_current_evidence ORDER BY id");
        let old_ids = ids(&c);
        let active = rows(&c, "SELECT * FROM term_active_evidence ORDER BY asset_id,source");
        assert_eq!(migrate_to(&mut c, 10).unwrap(), 9);
        assert_eq!(ids(&c), old_ids);
        assert_eq!(rows(&c, "SELECT * FROM term_active_evidence ORDER BY asset_id,source"), active);

        c.execute_batch(
            "INSERT INTO term_evidence(asset_id,term_id,source,method,version,fingerprint)
                VALUES (1,1,'wd_extension','linear_head','test',zeroblob(16)),
                       (1,1,'ccip_longtail','anchor_p75','test',zeroblob(16));",
        )
        .unwrap();
        assert_eq!(ids(&c), old_ids);
        crate::store::publish_asset_batch(
            &c,
            "wd_extension",
            ("wd_extension", "linear_head"),
            &[1],
            &[4].into_iter().collect(),
            None,
        )
        .unwrap();
        let with_wd_extension = ids(&c);
        assert_eq!(with_wd_extension.len(), old_ids.len() + 1);
        let longtail = (crate::ccip_longtail::SOURCE, crate::ccip_longtail::METHOD);
        crate::store::publish_asset_batch(&c, "enable", longtail, &[1], &[5].into_iter().collect(), None).unwrap();
        assert_eq!(ids(&c).len(), with_wd_extension.len() + 1);
        crate::store::publish_asset_batch(&c, "withdraw", longtail, &[1], &Default::default(), None).unwrap();
        assert_eq!(ids(&c), with_wd_extension);

        c.execute_batch(
            "INSERT INTO terms(key,kind,origin) VALUES ('tag-a','concept','user'),('tag-b','concept','user');
             INSERT INTO term_evidence(asset_id,term_id,source,method,version,fingerprint) VALUES
               (1,2,'example','tag-a','test',zeroblob(16)),(1,3,'example','tag-b','test',zeroblob(16));",
        )
        .unwrap();
        assert_eq!(ids(&c), with_wd_extension);
        for (method, row) in [("tag-a", 6), ("tag-b", 7)] {
            crate::store::publish_asset_batch(
                &c,
                "example",
                ("example", method),
                &[1],
                &[row].into_iter().collect(),
                None,
            )
            .unwrap();
        }
        assert_eq!(ids(&c).len(), with_wd_extension.len() + 2);
        crate::store::publish_asset_batch(&c, "example", ("example", "tag-a"), &[1], &Default::default(), None)
            .unwrap();
        assert_eq!(ids(&c).len(), with_wd_extension.len() + 1);
        assert_eq!(rows(&c, "SELECT method FROM term_current_evidence WHERE source='example'").len(), 1);
        assert!(c
            .query_row("SELECT method='tag-b' FROM term_current_evidence WHERE source='example'", [], |r| r
                .get::<_, bool>(0))
            .unwrap());
        assert!(c
            .execute(
                "INSERT INTO term_evidence_sets(producer,source,method,scope) VALUES ('t','example','*','all')",
                []
            )
            .is_err());
        assert!(crate::store::rebuild_active_evidence(&c).unwrap().is_empty());
        assert!(rows(&c, "PRAGMA foreign_key_check").is_empty());
    }

    #[test]
    fn v9_to_v10_keeps_old_undo_and_rebuilds_both_kind_triggers() {
        use crate::store::{self, Verdict, VerdictTarget};
        let mut c = Connection::open_in_memory().unwrap();
        crate::configure(&c).unwrap();
        migrate_to(&mut c, 9).unwrap();
        let tag = store::create_term(&c, Kind::Concept, Some("tag"), "user").unwrap();
        let asset = store::register_asset(&c, "library", "a", None).unwrap();
        let action =
            store::judge(&mut c, &[VerdictTarget::Image { asset, term: tag }], Verdict::Yes, "user", "", "测试标签")
                .unwrap();
        assert!(store::create_term(&c, Kind::Mark, Some(FAVORITE_KEY), "user").is_err());
        assert_eq!(migrate_to(&mut c, 10).unwrap(), 9);
        assert!(store::term_by_key(&c, FAVORITE_KEY).unwrap().is_none());
        assert_eq!(store::state(&c, asset, tag).unwrap().as_deref(), Some("confirmed"));
        // 撤销只认当前版本的库；第 10 版以后的升级不碰撤销记录和类型触发器，先升到当前版本再撤销。
        migrate_to(&mut c, SCHEMA_VERSION).unwrap();
        store::undo(&mut c, Some(action)).unwrap();
        assert_ne!(store::state(&c, asset, tag).unwrap().as_deref(), Some("confirmed"));
        let mark = store::create_term(&c, Kind::Mark, Some(FAVORITE_KEY), "user").unwrap();
        c.execute("UPDATE terms SET kind='mark' WHERE key='tag'", []).unwrap();
        assert!(c.execute("UPDATE terms SET kind='unknown' WHERE key=?1", [FAVORITE_KEY]).is_err());
        let targets = [VerdictTarget::Image { asset, term: mark }];
        assert!(store::judge(&mut c, &targets, Verdict::No, "user", "", "测试标记").is_err());
        store::judge(&mut c, &targets, Verdict::Yes, "user", "", "测试标记").unwrap();
        assert_eq!(store::state(&c, asset, mark).unwrap().as_deref(), Some("confirmed"));
        store::judge(&mut c, &targets, Verdict::Clear, "user", "", "清除标记").unwrap();
        assert!(store::state(&c, asset, mark).unwrap().is_none());
    }

    #[test]
    fn v9_to_v10_rolls_back_batches_if_rebuilding_kind_triggers_fails() {
        let mut c = Connection::open_in_memory().unwrap();
        crate::configure(&c).unwrap();
        migrate_to(&mut c, 9).unwrap();
        // 模拟升级到重建类型触发器时失败，前面的批次表已被重建。
        c.execute_batch(
            "INSERT INTO term_evidence_sets(producer,source,method,scope) VALUES ('t','wd','wd_character','all');
             DROP TRIGGER terms_kind_update;",
        )
        .unwrap();
        let objects = "SELECT name,sql FROM sqlite_schema ORDER BY name";
        let before = rows(&c, objects);
        assert!(migrate_to(&mut c, 10).is_err());
        assert_eq!(version(&c).unwrap(), 9);
        assert_eq!(rows(&c, objects), before);
        assert_eq!(rows(&c, "SELECT * FROM term_evidence_sets").len(), 1);
    }

    #[test]
    fn evidence_sets_take_only_replaceable_sources_and_their_own_rows() {
        let c = fresh();
        for (source, method) in REPLACEABLE {
            c.execute(
                "INSERT INTO term_evidence_sets(producer,source,method,scope) VALUES ('test',?1,?2,'assets')",
                [source, method],
            )
            .unwrap();
        }
        assert!(c
            .execute(
                "INSERT INTO term_evidence_sets(producer,source,method,scope) VALUES ('test','wd','tagger','all')",
                []
            )
            .is_err());
        c.execute_batch(
            "INSERT INTO term_assets(namespace,key) VALUES ('library','a'),('library','b');
             INSERT INTO terms(key,kind,origin) VALUES ('p','character','legacy');
             INSERT INTO term_evidence(asset_id,term_id,source,method,version,fingerprint) VALUES
              (1,1,'wd','wd_character','v',zeroblob(16)),(2,1,'wd','wd_character','v',zeroblob(16)),
              (1,1,'ccip_index','char_ext','v',zeroblob(16));
             INSERT INTO term_evidence_set_assets VALUES (1,1);",
        )
        .unwrap();
        c.execute("INSERT INTO term_evidence_set_rows VALUES (1,1)", []).unwrap();
        assert!(c.execute("INSERT INTO term_evidence_set_rows VALUES (1,2)", []).is_err(), "图不在批次范围里");
        assert!(c.execute("INSERT INTO term_evidence_set_rows VALUES (1,3)", []).is_err(), "来源不同");
        for table in ["term_evidence_sets", "term_evidence_set_assets", "term_evidence_set_rows"] {
            assert!(c.execute(&format!("DELETE FROM {table}"), []).is_err(), "{table}");
        }
    }

    #[test]
    fn v2_to_v3_keeps_provenance_evidence_effective_and_rolls_back_whole() {
        let mut c = Connection::open_in_memory().unwrap();
        crate::configure(&c).unwrap();
        migrate_to(&mut c, 2).unwrap();
        c.execute_batch(
            "INSERT INTO term_assets(namespace,key) VALUES ('library','a'),('library','b');
             INSERT INTO terms(key,kind,origin) VALUES ('x','artist','legacy'),('p','character','legacy');
             INSERT INTO term_evidence(asset_id,term_id,source,method,version,fingerprint) VALUES
              (1,1,'provenance','ocr','v4/1',zeroblob(16)),(2,1,'provenance','pixel_hash','v4/2',zeroblob(16)),
              (1,2,'wd','wd_character','m',zeroblob(16));
             INSERT INTO term_evidence_sets(producer,source,method,scope) VALUES ('t','wd','wd_character','all');
             INSERT INTO term_evidence_set_rows VALUES (1,3);",
        )
        .unwrap();
        crate::store::rebuild_active_evidence(&c).unwrap();
        let current = |c: &Connection| rows(c, "SELECT id FROM term_current_evidence ORDER BY id");
        let before = current(&c);
        assert_eq!(before.len(), 3);
        assert_eq!(migrate(&mut c).unwrap(), 2);
        assert_eq!(current(&c), before, "迁移前后有效的证据相同");
        assert_eq!(rows(&c, "SELECT source,method,scope FROM term_evidence_sets ORDER BY id").len(), 2);
        assert!(
            c.execute(
                "INSERT INTO term_evidence_sets(producer,source,method,scope) VALUES ('t','provenance','ocr','all')",
                []
            )
            .is_err(),
            "溯源只按整个来源成批次"
        );
        // 失败的迁移回到第 2 版原样。
        let mut d = Connection::open_in_memory().unwrap();
        crate::configure(&d).unwrap();
        migrate_to(&mut d, 2).unwrap();
        d.execute_batch("CREATE TABLE term_evidence_sets_v3(x);").unwrap();
        let schema = rows(&d, "SELECT name,sql FROM sqlite_master WHERE name<>'term_evidence_sets_v3' ORDER BY name");
        assert!(migrate(&mut d).is_err());
        assert_eq!(version(&d).unwrap(), 2);
        assert_eq!(
            rows(&d, "SELECT name,sql FROM sqlite_master WHERE name<>'term_evidence_sets_v3' ORDER BY name"),
            schema
        );
    }
}

#[cfg(test)]
mod required_backup_tests {
    use super::*;
    #[test]
    fn direct_upgrade_refuses_backup_failure_without_changing_file_database() {
        let root = std::env::temp_dir().join(format!("illustash-backup-boundary-{}", crate::store::new_key().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let mut conn = crate::sqlite::open(root.join("library.sqlite")).unwrap();
        migrate_to(&mut conn, 6).unwrap();
        std::fs::write(root.join("library-backups"), "synthetic blocker").unwrap();
        assert!(migrate(&mut conn).is_err());
        assert_eq!(version(&conn).unwrap(), 6);
        std::fs::remove_file(root.join("library-backups")).unwrap();
        migrate(&mut conn).unwrap();
        std::fs::write(root.join("library-backups").join("sentinel"), "owned elsewhere").unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE synthetic_rowids(value TEXT); INSERT INTO synthetic_rowids(rowid,value) VALUES (27,'committed-wal');").unwrap();
        crate::backup::required(&conn, "synthetic", &crate::platform_backup::capability()).unwrap();
        let backup = std::fs::read_dir(root.join("library-backups"))
            .unwrap()
            .map(|p| p.unwrap().path())
            .find(|p| p.file_name().unwrap().to_string_lossy().starts_with("terms-synthetic-"))
            .unwrap();
        let restored = crate::sqlite::open_with_flags(backup, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            restored
                .query_row("SELECT rowid,value FROM synthetic_rowids", [], |r| Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?
                )))
                .unwrap(),
            (27, "committed-wal".into())
        );
        assert_eq!(std::fs::read_to_string(root.join("library-backups/sentinel")).unwrap(), "owned elsewhere");
        drop((restored, conn));
        std::fs::remove_dir_all(root).unwrap();
    }
}
