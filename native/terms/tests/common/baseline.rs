//! 第一个公开测试版的对照基准（#201 第 1 步）：旧表删除后（词条核心 v8），重放的各个场景必须继续产出的结果。
//!
//! 由以前的 `tests/replay.rs`（`record_baseline`）和 `tests/contracts.rs` 在词条核心第 7 版的 main 上录制
//! （PR #239；两者都在 git 历史里），由 `baseline.rs` 和 `contracts.rs` 重放。比较规则（#201 中商定；
//! Python 的固定期望在 `tests/fixed_expectations.py` 里遵循同样的规则）：
//! - 响应经 `Masks::output` 遮盖后按字节比较：临时根目录、时钟、学习戳记、操作系统错误文本和当前词条核心版本；
//! - 每个调用之后，该调用所用数据库中除 `RETIRED` 以外的每张表都逐行比较（带类型单元格的有序多重集，文本经过
//!   `Masks::text`）；整数主键是普通列，与其他列一样比较，隐式 rowid 不比较；
//! - `meta` 保留它的键；部分值按 `meta_value` 的规则比较；
//! - 数据库表结构本身不比较（v8 会改动它；它有自己的测试）。
#![allow(dead_code)] // baseline.rs 和 contracts.rs 共用；各自只用到一部分
use regex::Regex;
use rusqlite::{types::ValueRef, Connection};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

pub const FORMAT: i64 = 1;

/// 词条核心 v8 停用的旧表：内容移到 `term_*`，在那里比较。录制里仍有身份关系表（它们在录制之后才停用）：
/// 它们录下的改动也不计入（`apply`）。
pub const RETIRED: [&str; 11] = [
    "entities",
    "links",
    "work_review",
    "names",
    "display_names",
    "name_aliases",
    "identity_baseline",
    "identity_relations",
    "identity_evidence",
    "identity_decisions",
    "identity_decision_evidence",
];

/// 不参与比较的列：对打包细节求的哈希，与以前的录制一样；以及词条核心 v8 新增、对照基准（在词条核心第 7 版上录制）
/// 不可能有的列（#201 第 1 步比较规则第 5 条：v8 新增的东西逐项登记后排除，每列都写明理由）：
/// - `actions.label_kind` / `label_count`：带数量的步骤的种类和数量（#201）；历史据此显示的内容在响应里比较。
/// - `term_names.seq`：名称的顺序，取自原来 `names` 的 rowid（#201）；读取方据此得到的顺序在响应里比较。
/// - `term_tree` 的身份属性和 `seq`：这些属性从 `terms.attrs` 移到这里（#201），在 `terms.attrs` 里比较
///   （`ATTRIBUTES`）；`seq` 是身份的顺序，取自原来 `entities` 的 rowid，读取方据此得到的顺序在响应里比较。
pub const SKIP_COLUMNS: [(&str, &str); 12] = [
    ("term_packages", "manifest"),
    ("term_evidence", "fingerprint"),
    ("actions", "label_kind"),
    ("actions", "label_count"),
    ("term_names", "seq"),
    ("term_tree", "variant"),
    ("term_tree", "artist"),
    ("term_tree", "label_only"),
    ("term_tree", "mascot"),
    ("term_tree", "mascot_origin"),
    ("term_tree", "original"),
    ("term_tree", "seq"),
];

/// 词条核心 v8 从 `terms.attrs` 移到 `term_tree` 的身份属性（#201），顺序与以前的镜像写入时相同。
/// 身份的 `terms.attrs` 按以前的写法、用它的 `term_tree` 行拼出这个对象来比较；其他词条保留自己的值。
pub const ATTRIBUTES: [&str; 6] = ["variant", "artist", "label_only", "mascot", "mascot_origin", "original"];

/// 读取一张表所用的查询（`terms` 表见 `ATTRIBUTES`）。
fn select(c: &Connection, name: &str, columns: &[String]) -> String {
    let physical: bool = c
        .query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='term_records')", [], |r| {
            r.get(0)
        })
        .unwrap();
    let moved: bool = name == "terms"
        && !physical
        && c.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('term_tree') WHERE name='variant')", [], |r| {
            r.get(0)
        })
        .unwrap();
    if !moved {
        return format!("SELECT * FROM \"{name}\"");
    }
    let object = ATTRIBUTES.iter().map(|a| format!("'{a}',r.{a}")).collect::<Vec<_>>().join(",");
    let list = columns
        .iter()
        .map(|col| {
            if col == "attrs" {
                format!("CASE WHEN r.term_id IS NULL THEN t.attrs ELSE json_object({object}) END")
            } else {
                format!("t.\"{col}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("SELECT {list} FROM \"{name}\" t LEFT JOIN term_tree r ON r.term_id=t.id")
}

static CLOCK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})?").unwrap());
static STAMP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"learning-v1:[0-9a-f]{64}").unwrap());
/// Python 时代的录制在输入里带着的机器路径（录制机器的临时目录、脚本首行里的主目录）：不随每次运行变化，
/// 但也不适合放进公开仓库（#201）。
static MACHINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/var/folders/[^/\s'\x22]+/[^/\s'\x22]+/T/|/Users/[^/\s'\x22]+/").unwrap());

/// 把录制文本里的机器路径改成通用写法；对所有录制和比较的内容都适用。
pub fn scrub(text: &str) -> String {
    MACHINE
        .replace_all(
            text,
            |c: &regex::Captures| if c[0].starts_with("/var/") { "/tmp/recording/" } else { "/home/user/" },
        )
        .into_owned()
}

/// 操作系统自己的错误文本和错误号（macOS 和 Windows 的措辞和编号不同）。
static OS_ERROR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^：:\n]*\(os error \d+\)").unwrap());

/// 响应和单元格如何遮盖：先处理随每次运行变化的写法（临时根目录），再应用上面的规则。
pub struct Masks<'a> {
    /// 真实临时路径 -> `@ROOT@` 占位符。
    pub roots: &'a dyn Fn(&str) -> String,
    /// 本次构建的词条核心版本：提到它的响应按「当前版本」比较。
    pub version: i64,
}

impl Masks<'_> {
    pub fn text(&self, text: &str) -> String {
        let text = scrub(&(self.roots)(text));
        let text = CLOCK.replace_all(&text, "<clock>");
        STAMP.replace_all(&text, "<stamp>").into_owned()
    }

    /// 用于比较的响应：先做 `text`，再遮盖操作系统错误文本，以及响应里写明的当前词条核心版本
    /// （`migrate` 的 `from`/`to`、版本拒绝信息、升级备份的文件名）。
    pub fn output(&self, text: &str) -> String {
        let text = OS_ERROR.replace_all(&self.text(text), "<os error>").into_owned();
        let v = self.version;
        let numbers = Regex::new(&format!(r#""(from|to)":{v}([,}}])"#)).unwrap();
        let text = numbers.replace_all(&text, r#""$1":"<terms-version>"$2"#);
        text.replace(&format!("程序是第 {v} 版"), "程序是第 <terms-version> 版")
            .replace(&format!("程序支持的第 {v} 版"), "程序支持的第 <terms-version> 版")
            .replace(&format!("-before-v{v}-"), "-before-v<terms-version>-")
    }
}

/// 按含义而不是按字面比较的 `meta` 值。
/// - `terms_schema`：当前版本记作 `<terms-version>`（录制时是 7，词条核心 v8 中是 8）；
/// - `names_version` 统计对其触发器所在表的写入，而 v8 移动了这些表：只比较各投影是否与它同步；
/// - `identity_graph_version` 对随每次运行变化的值求哈希，与以前一样不比较。
fn meta_value(key: &str, value: &str, names_version: Option<&str>, version: i64) -> String {
    match key {
        "terms_schema" if value == version.to_string() => "<terms-version>".into(),
        "names_version" => "<names_version>".into(),
        "names_projected" | "names_display_projected" if Some(value) == names_version => "<names_version>".into(),
        "names_projected" | "names_display_projected" => "<stale>".into(),
        "identity_graph_version" => "<derived>".into(),
        _ => value.to_owned(),
    }
}

/// 用于比较的一张表：它的列，以及带重复次数的行（遮盖后单元格的规范 JSON 文本）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: BTreeMap<String, usize>,
}

pub type State = BTreeMap<String, Table>;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Addition {
    name: String,
    introduced_in: i64,
    checks: Vec<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Additions {
    reference_terms_version: i64,
    tables: Vec<Addition>,
    meta_keys: Vec<Addition>,
    physical_views: Vec<PhysicalView>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PhysicalView {
    physical: String,
    view: String,
    introduced_in: i64,
    checks: Vec<String>,
}

static ADDITIONS: LazyLock<Additions> = LazyLock::new(|| {
    let additions: Additions = serde_json::from_str(include_str!("../../../../tests/baseline_additions.json")).unwrap();
    for entries in [&additions.tables, &additions.meta_keys] {
        let mut names = std::collections::BTreeSet::new();
        for entry in entries {
            assert!(!entry.name.is_empty() && entry.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
            assert!(entry.introduced_in > additions.reference_terms_version && !entry.checks.is_empty());
            assert!(names.insert(&entry.name));
        }
    }
    let mut mapped = std::collections::BTreeSet::new();
    for entry in &additions.physical_views {
        assert!(entry.introduced_in > additions.reference_terms_version && !entry.checks.is_empty());
        for name in [&entry.physical, &entry.view] {
            assert!(!name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
            assert!(mapped.insert(name));
        }
    }
    additions
});

/// 只从录制的分组、图片大小、人工结论和排除状态推算 #836 的两列派生结果。
pub fn current_fold(state: &State) -> Option<Table> {
    let duplicates = state.get("duplicates")?;
    if duplicates.rows.is_empty() {
        return None;
    }
    let assets = state.get("assets")?;
    let assertions = state.get("assertions")?;
    let rows =
        |table: &Table| table.rows.keys().map(|r| serde_json::from_str::<Vec<Value>>(r).unwrap()).collect::<Vec<_>>();
    let columns = |table: &Table, names| {
        let names: &[&str] = names;
        names.iter().map(|name| table.columns.iter().position(|c| c == *name).unwrap()).collect::<Vec<_>>()
    };
    let excluded = state.get("excluded").map_or_else(BTreeSet::new, |excluded| {
        let idx = columns(excluded, &["uuid"])[0];
        rows(excluded).into_iter().map(|row| row[idx].as_str().unwrap().to_owned()).collect()
    });
    let asset = columns(assets, &["uuid", "width", "height"]);
    let size = rows(assets)
        .into_iter()
        .map(|r| {
            let px = i128::from(r[asset[1]].as_i64().unwrap_or(0)) * i128::from(r[asset[2]].as_i64().unwrap_or(0));
            (r[asset[0]].as_str().unwrap().to_owned(), px)
        })
        .collect::<BTreeMap<_, _>>();
    let a = columns(assertions, &["uuid", "entity", "status"]);
    let mut decisions: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    for row in rows(assertions) {
        decisions
            .entry(row[a[0]].as_str().unwrap().into())
            .or_default()
            .insert((row[a[1]].as_str().unwrap().into(), row[a[2]].as_str().unwrap().into()));
    }
    let cols = columns(duplicates, &["uuid", "grp", "pos", "rep", "hidden"]);
    let mut groups: BTreeMap<String, Vec<Vec<Value>>> = BTreeMap::new();
    for row in rows(duplicates) {
        groups.entry(row[cols[1]].as_str().unwrap().into()).or_default().push(row);
    }
    let uuid = |row: &[Value]| row[cols[0]].as_str().unwrap().to_owned();
    let empty = BTreeSet::new();
    let all_decisions = |id: &str| decisions.get(id).unwrap_or(&empty);
    let mut expected = Table { columns: duplicates.columns.clone(), rows: BTreeMap::new() };
    for group in groups.into_values() {
        let all: BTreeSet<_> = group.iter().flat_map(|r| all_decisions(&uuid(r)).iter()).collect();
        let best = group
            .iter()
            .min_by_key(|r| {
                let id = uuid(r);
                (
                    std::cmp::Reverse(all_decisions(&id).len()),
                    std::cmp::Reverse(size.get(&id).copied().unwrap_or(0)),
                    r[cols[2]].as_i64().unwrap(),
                    id,
                )
            })
            .unwrap();
        let id = uuid(best);
        let foldable = all_decisions(&id).len() == all.len()
            && group.iter().all(|row| excluded.contains(&uuid(row)) == excluded.contains(&id));
        for mut row in group {
            let rep = uuid(&row) == id;
            row[cols[3]] = Value::from(i64::from(rep));
            row[cols[4]] = Value::from(i64::from(foldable && !rep));
            *expected.rows.entry(serde_json::to_string(&row).unwrap()).or_default() += 1;
        }
    }
    Some(expected)
}

fn meta_key_row(row: &str, key: &str) -> bool {
    let cells: Vec<Value> = serde_json::from_str(row).unwrap();
    cells.first().and_then(Value::as_str) == Some(key)
}

/// 只用于实时应用库与 v7 冻结参考的对照：精确分离新状态，原始 state/exact 与当前库比较不变。
/// 冻结侧若已有同名状态，直接失败，不能通过这份清单藏掉旧值。
pub fn historical_projection(got: &State, frozen: &State, recorded: i64, current: i64) -> Result<State, String> {
    if recorded != ADDITIONS.reference_terms_version {
        return Ok(got.clone());
    }
    let live_version = got.get("meta").and_then(|table| {
        table.rows.keys().find_map(|row| {
            let cells: Vec<Value> = serde_json::from_str(row).unwrap();
            if cells.first().and_then(Value::as_str) != Some("terms_schema") {
                return None;
            }
            let version = cells.get(1)?.as_str()?;
            if version == "<terms-version>" {
                Some(current)
            } else {
                version.parse().ok()
            }
        })
    });
    let Some(live_version) = live_version else { return Ok(got.clone()) };
    let mut out = got.clone();
    for entry in ADDITIONS.physical_views.iter().filter(|entry| entry.introduced_in <= live_version) {
        if frozen.contains_key(&entry.physical) {
            return Err(format!("物理视图映射不能隐藏冻结侧已有表 {}", entry.physical));
        }
        match (got.get(&entry.physical), got.get(&entry.view)) {
            (None, None) => (),
            (Some(raw), Some(effective)) if raw == effective => {
                out.remove(&entry.physical);
                if entry.view == "terms" {
                    historical_attributes(&mut out)?;
                }
            }
            _ => return Err(format!("物理表 {} 与有效视图 {} 的完整行不一致", entry.physical, entry.view)),
        }
    }
    for table in ADDITIONS.tables.iter().filter(|entry| entry.introduced_in <= live_version) {
        if frozen.contains_key(&table.name) {
            return Err(format!("新增状态清单不能隐藏冻结侧已有表 {}", table.name));
        }
        out.remove(&table.name);
    }
    for key in ADDITIONS.meta_keys.iter().filter(|entry| entry.introduced_in <= live_version) {
        if frozen.get("meta").is_some_and(|table| table.rows.keys().any(|row| meta_key_row(row, &key.name))) {
            return Err(format!("新增状态清单不能隐藏冻结侧已有 meta 键 {}", key.name));
        }
        if let Some(meta) = out.get_mut("meta") {
            meta.rows.retain(|row, _| !meta_key_row(row, &key.name));
        }
    }
    Ok(out)
}

// 第 9 版的当前读数保留原始 attrs 和结构属性。只在旧参考投影里沿用第 8 版的历史表达。
fn historical_attributes(out: &mut State) -> Result<(), String> {
    let Some(tree) = out.get("term_tree") else { return Ok(()) };
    if !tree.columns.iter().any(|c| c == "variant") {
        return Ok(());
    }
    let index =
        |name: &str| tree.columns.iter().position(|c| c == name).ok_or_else(|| format!("历史属性缺少列 {name}"));
    let id = index("term_id")?;
    let attributes = ATTRIBUTES.iter().map(|name| index(name)).collect::<Result<Vec<_>, _>>()?;
    let mut objects = BTreeMap::new();
    for row in tree.rows.keys() {
        let cells: Vec<Value> = serde_json::from_str(row).unwrap();
        let object = format!(
            "{{{}}}",
            ATTRIBUTES
                .iter()
                .zip(&attributes)
                .map(|(name, &i)| format!("{}:{}", json!(name), cells[i]))
                .collect::<Vec<_>>()
                .join(",")
        );
        objects.insert(cells[id].to_string(), object);
    }
    let terms = out.get_mut("terms").ok_or("历史属性缺少 terms")?;
    let id = terms.columns.iter().position(|c| c == "id").ok_or("历史属性缺少词条 ID")?;
    let attrs = terms.columns.iter().position(|c| c == "attrs").ok_or("历史属性缺少词条 attrs")?;
    let mut rewritten = BTreeMap::new();
    for (row, count) in &terms.rows {
        let mut cells: Vec<Value> = serde_json::from_str(row).unwrap();
        if let Some(object) = objects.get(&cells[id].to_string()) {
            cells[attrs] = json!(object);
        }
        *rewritten.entry(json!(cells).to_string()).or_default() += count;
    }
    terms.rows = rewritten;
    let tree = out.get_mut("term_tree").expect("上面已读取结构");
    let keep: Vec<_> = (0..tree.columns.len()).filter(|i| !attributes.contains(i)).collect();
    tree.columns = keep.iter().map(|&i| tree.columns[i].clone()).collect();
    let mut rewritten = BTreeMap::new();
    for (row, count) in &tree.rows {
        let cells: Vec<Value> = serde_json::from_str(row).unwrap();
        *rewritten.entry(json!(keep.iter().map(|&i| &cells[i]).collect::<Vec<_>>()).to_string()).or_default() += count;
    }
    tree.rows = rewritten;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn cell(value: ValueRef, masks: &Masks) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(v) => json!(v),
        ValueRef::Real(v) => json!({"real": format!("{:016x}", v.to_bits())}),
        ValueRef::Text(v) => json!(masks.text(&String::from_utf8_lossy(v))),
        ValueRef::Blob(v) => json!({"blob": hex(v)}),
    }
}

pub fn tables(c: &Connection) -> Vec<String> {
    let mut s = c
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap();
    s.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

/// 一个数据库参与比较的内容。
pub fn state(c: &Connection, masks: &Masks) -> State {
    let names_version: Option<String> = c
        .query_row(
            "SELECT value FROM meta WHERE key='names_version' AND EXISTS(SELECT 1 FROM sqlite_master WHERE name='meta')",
            [],
            |r| r.get(0),
        )
        .ok();
    let mut out = State::new();
    let mut names = tables(c);
    let raw = names.iter().any(|name| name == "term_records");
    for entry in &ADDITIONS.physical_views {
        if names.contains(&entry.physical)
            && c.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='view' AND name=?1)",
                [&entry.view],
                |r| r.get::<_, bool>(0),
            )
            .unwrap()
        {
            names.push(entry.view.clone());
        }
    }
    for name in names {
        if RETIRED.contains(&name.as_str()) {
            continue;
        }
        let all: Vec<String> = {
            let mut s = c.prepare(&format!("PRAGMA table_info(\"{name}\")")).unwrap();
            s.query_map([], |r| r.get::<_, String>(1)).unwrap().map(Result::unwrap).collect()
        };
        let keep: Vec<usize> = (0..all.len())
            .filter(|&i| {
                !SKIP_COLUMNS.contains(&(name.as_str(), all[i].as_str()))
                    || (raw && name == "term_tree" && ATTRIBUTES.contains(&all[i].as_str()))
            })
            .collect();
        let mut table = Table { columns: keep.iter().map(|&i| all[i].clone()).collect(), rows: BTreeMap::new() };
        let mut s = c.prepare(&select(c, &name, &all)).unwrap();
        let mut q = s.query([]).unwrap();
        while let Some(row) = q.next().unwrap() {
            let mut cells: Vec<Value> = keep.iter().map(|&i| cell(row.get_ref(i).unwrap(), masks)).collect();
            if name == "meta" {
                if let (Some(Value::String(key)), Some(Value::String(value))) = (cells.first(), cells.get(1)) {
                    cells[1] = json!(meta_value(key, value, names_version.as_deref(), masks.version));
                }
            }
            *table.rows.entry(Value::Array(cells).to_string()).or_default() += 1;
        }
        out.insert(name, table);
    }
    out
}

/// 一个调用改动了什么：每张表删掉和新增的行（含重复），表是新建的或列变了时给出列；以及消失的表。
pub fn delta(before: &State, after: &State) -> Value {
    let mut tables = Map::new();
    for (name, table) in after {
        let old = before.get(name);
        let mut entry = Map::new();
        if old.map(|t| &t.columns) != Some(&table.columns) {
            entry.insert("columns".into(), json!(table.columns));
        }
        let (removed, added) = match old {
            Some(t) if t.columns == table.columns => difference(&t.rows, &table.rows),
            // 列变了：原有的行全部算删除，现在的行全部算新增。
            Some(t) => (expand(&t.rows), expand(&table.rows)),
            None => (Vec::new(), expand(&table.rows)),
        };
        if !removed.is_empty() {
            entry.insert("-".into(), json!(removed));
        }
        if !added.is_empty() {
            entry.insert("+".into(), json!(added));
        }
        if !entry.is_empty() {
            tables.insert(name.clone(), Value::Object(entry));
        }
    }
    let gone: Vec<&String> = before.keys().filter(|k| !after.contains_key(*k)).collect();
    let mut out = Map::new();
    if !tables.is_empty() {
        out.insert("tables".into(), Value::Object(tables));
    }
    if !gone.is_empty() {
        out.insert("gone".into(), json!(gone));
    }
    Value::Object(out)
}

fn expand(rows: &BTreeMap<String, usize>) -> Vec<String> {
    rows.iter().flat_map(|(r, n)| std::iter::repeat_n(r.clone(), *n)).collect()
}

/// 只在 `old` 中的行和只在 `new` 中的行（多重集之差），各自按规范顺序排列。
fn difference(old: &BTreeMap<String, usize>, new: &BTreeMap<String, usize>) -> (Vec<String>, Vec<String>) {
    let mut removed = Vec::new();
    let mut added = Vec::new();
    for (row, &n) in old {
        let m = new.get(row).copied().unwrap_or(0);
        removed.extend(std::iter::repeat_n(row.clone(), n.saturating_sub(m)));
    }
    for (row, &n) in new {
        let m = old.get(row).copied().unwrap_or(0);
        added.extend(std::iter::repeat_n(row.clone(), n.saturating_sub(m)));
    }
    (removed, added)
}

/// 把录制的 `delta` 应用到 `before` 上的结果。
pub fn apply(before: &State, delta: &Value) -> Result<State, String> {
    let mut out = before.clone();
    if let Some(gone) = delta.get("gone").and_then(Value::as_array) {
        for name in gone {
            out.remove(name.as_str().unwrap());
        }
    }
    if let Some(tables) = delta.get("tables").and_then(Value::as_object) {
        for (name, change) in tables.iter().filter(|(name, _)| !RETIRED.contains(&name.as_str())) {
            let table = out.entry(name.clone()).or_default();
            if let Some(columns) = change.get("columns") {
                table.columns = columns.as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_owned()).collect();
            }
            for row in change.get("-").and_then(Value::as_array).into_iter().flatten() {
                let row = row.as_str().unwrap();
                match table.rows.get_mut(row) {
                    Some(n) if *n > 1 => *n -= 1,
                    Some(_) => {
                        table.rows.remove(row);
                    }
                    None => return Err(format!("{name}: recorded removal of a row that is not there: {row}")),
                }
            }
            for row in change.get("+").and_then(Value::as_array).into_iter().flatten() {
                *table.rows.entry(row.as_str().unwrap().to_owned()).or_default() += 1;
            }
        }
    }
    Ok(out)
}

/// 期望状态与实际状态之间的前几处差异，用于失败信息。
pub fn differences(want: &State, got: &State, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    for name in want.keys().chain(got.keys().filter(|k| !want.contains_key(*k))) {
        let (w, g) = (want.get(name), got.get(name));
        match (w, g) {
            (Some(_), None) => out.push(format!("{name}: table missing")),
            (None, Some(_)) => out.push(format!("{name}: table not in the baseline")),
            (Some(w), Some(g)) if w.columns != g.columns => {
                out.push(format!("{name}: columns {:?}, baseline {:?}", g.columns, w.columns))
            }
            (Some(w), Some(g)) => {
                let (missing, extra) = difference(&w.rows, &g.rows);
                for row in missing.iter().take(limit) {
                    out.push(format!("{name}: missing {row}"));
                }
                for row in extra.iter().take(limit) {
                    out.push(format!("{name}: extra {row}"));
                }
            }
            (None, None) => unreachable!(),
        }
    }
    out
}

/// 把数据库导出成能以相同 rowid 重建它的 SQL，分成三部分分开存放，让各录制共用表结构：表；行（表使用隐式 rowid 时
/// 写出显式 rowid，另含 `sqlite_sequence`）；最后按表结构中的顺序写索引、视图和触发器，这样写入保护触发器看不到
/// 恢复过程。文本经过 `text`（临时根目录换成占位符）。
pub struct Dump {
    pub tables: String,
    pub rows: String,
    pub objects: String,
}

pub fn dump(c: &Connection, text: &dyn Fn(&str) -> String) -> Dump {
    let mut out = Dump { tables: String::new(), rows: String::new(), objects: String::new() };
    let user_version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    if user_version != 0 {
        out.tables.push_str(&format!("PRAGMA user_version={user_version};\n"));
    }
    let schema: Vec<(String, String, Option<String>)> = c
        .prepare("SELECT type,name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_autoindex_%' ORDER BY rowid")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for (kind, name, sql) in &schema {
        let Some(sql) = sql else { continue };
        let part = if kind != "table" {
            &mut out.objects
        } else if name != "sqlite_sequence" {
            &mut out.tables
        } else {
            continue;
        };
        part.push_str(&text(sql));
        part.push_str(";\n");
    }
    for (kind, name, sql) in &schema {
        if kind != "table" {
            continue;
        }
        let sql = sql.as_deref().unwrap_or_default().to_uppercase();
        let implicit = !sql.contains("WITHOUT ROWID") && !integer_key(c, name) && name != "sqlite_sequence";
        let select = if implicit { "SELECT rowid,* FROM" } else { "SELECT * FROM" };
        let mut s = c.prepare(&format!("{select} \"{name}\"")).unwrap();
        let columns: Vec<String> = s.column_names().map(|n| format!("\"{}\"", n.replace('"', "\"\""))).collect();
        let n = columns.len();
        let mut q = s.query([]).unwrap();
        while let Some(row) = q.next().unwrap() {
            let values: Vec<String> = (0..n).map(|i| literal(row.get_ref(i).unwrap(), text)).collect();
            if name == "sqlite_sequence" {
                out.rows.push_str(&format!(
                    "DELETE FROM sqlite_sequence WHERE name={0}; INSERT INTO sqlite_sequence(name,seq) VALUES ({0},{1});\n",
                    values[0], values[1]
                ));
            } else {
                out.rows.push_str(&format!(
                    "INSERT INTO \"{name}\"({}) VALUES ({});\n",
                    columns.join(","),
                    values.join(",")
                ));
            }
        }
    }
    out
}

fn integer_key(c: &Connection, table: &str) -> bool {
    // rowid 别名：恰好一个主键列，声明为 INTEGER。
    let keys: Vec<String> = c
        .prepare(&format!("SELECT type FROM pragma_table_info('{}') WHERE pk>0", table.replace('\'', "''")))
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    keys.len() == 1 && keys[0].eq_ignore_ascii_case("INTEGER")
}

fn literal(value: ValueRef, text: &dyn Fn(&str) -> String) -> String {
    match value {
        ValueRef::Null => "NULL".into(),
        ValueRef::Integer(v) => v.to_string(),
        ValueRef::Real(v) => real_literal(v),
        ValueRef::Text(v) => format!("'{}'", text(&String::from_utf8_lossy(v)).replace('\'', "''")),
        ValueRef::Blob(v) => format!("x'{}'", hex(v)),
    }
}

/// SQLite 读回后各位不变的 REAL 字面量：用 Rust 最短的可往返形式，SQLite 能精确解析。
fn real_literal(v: f64) -> String {
    if v.is_nan() {
        "NULL".into()
    } else if v.is_infinite() {
        if v > 0.0 {
            "9e999".into()
        } else {
            "-9e999".into()
        }
    } else {
        let s = format!("{v:?}");
        if s.contains('.') || s.contains('e') || s.contains("inf") {
            s
        } else {
            format!("{s}.0")
        }
    }
}

/// 原样列出每张表和每个表结构对象（带类型的单元格、rowid），文本经过 `text`：用来检查 `dump` 能重建数据库
/// （文本按 `dump` 写出时的样子）。
pub fn exact(c: &Connection, text: &dyn Fn(&str) -> String) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let mut s = c
        .prepare(
            "SELECT type,name,tbl_name,coalesce(sql,'') FROM sqlite_master WHERE name NOT LIKE 'sqlite_autoindex_%'",
        )
        .unwrap();
    let mut schema: Vec<String> = s
        .query_map([], |r| {
            Ok(format!(
                "{}|{}|{}|{}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                text(&r.get::<_, String>(3)?)
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    schema.sort();
    out.insert("@schema".into(), schema);
    let tables: Vec<(String, String)> = c
        .prepare("SELECT name,coalesce(sql,'') FROM sqlite_master WHERE type='table'")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for (name, sql) in tables {
        // `sqlite_sequence` 的行按 SQLite 第一次需要它们的顺序恢复：它们的 rowid 不带任何信息。
        let plain = sql.to_uppercase().contains("WITHOUT ROWID") || name == "sqlite_sequence";
        let select = if plain { "*" } else { "rowid,*" };
        let mut s = c.prepare(&format!("SELECT {select} FROM \"{name}\"")).unwrap();
        let n = s.column_count();
        let mut q = s.query([]).unwrap();
        let mut rows = Vec::new();
        while let Some(row) = q.next().unwrap() {
            rows.push(
                (0..n)
                    .map(|i| match row.get_ref(i).unwrap() {
                        ValueRef::Null => "n".to_owned(),
                        ValueRef::Integer(v) => format!("i{v}"),
                        ValueRef::Real(v) => format!("r{:016x}", v.to_bits()),
                        ValueRef::Text(v) => format!("t{}", hex(text(&String::from_utf8_lossy(v)).as_bytes())),
                        ValueRef::Blob(v) => format!("b{}", hex(v)),
                    })
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        rows.sort();
        out.insert(name, rows);
    }
    out.insert(
        "@user_version".into(),
        vec![c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap().to_string()],
    );
    out
}

/// 用 `dump` 的输出重建数据库（写入行时关闭外键，与恢复录制时一样）。
pub fn restore(c: &Connection, tables: &str, rows: &str, objects: &str) {
    c.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    c.execute_batch(&format!("BEGIN;\n{tables}{rows}{objects}COMMIT;")).unwrap();
}

/// 按文件头判断文件是不是 SQLite 数据库。
pub fn is_database(path: &std::path::Path) -> bool {
    let mut head = [0u8; 16];
    std::fs::File::open(path).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head)).is_ok()
        && &head == b"SQLite format 3\0"
}

/// 数据库所处的词条核心版本（0：没有词条表）。
pub fn terms_version(c: &Connection) -> i64 {
    c.query_row("SELECT CAST(value AS INTEGER) FROM meta WHERE key='terms_schema'", [], |r| r.get(0)).unwrap_or(0)
}

/// 还有领域未切换到词条核心的图库：在它上面的调用是在搭建场景，而不是检验切换后的词条核心，这些调用归入检查点。
pub fn before_cutover(c: &Connection) -> bool {
    let has_entities: bool = c
        .query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='entities')", [], |r| {
            r.get(0)
        })
        .unwrap_or(false);
    if !has_entities {
        return false;
    }
    let switched: i64 = c
        .query_row(
            "SELECT count(*) FROM meta WHERE key IN ('terms_artists','terms_labels','terms_characters','terms_names') AND value='core'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    switched != 4
}

#[cfg(test)]
mod historical_projection_tests {
    use super::*;

    fn table(columns: &[&str], rows: &[Value]) -> Table {
        Table {
            columns: columns.iter().map(|s| (*s).to_owned()).collect(),
            rows: rows.iter().map(|v| (v.to_string(), 1)).collect(),
        }
    }

    fn states() -> (State, State) {
        let old_meta = table(&["key", "value"], &[json!(["terms_schema", "<terms-version>"]), json!(["keep", "old"])]);
        let frozen = State::from([
            ("meta".into(), old_meta),
            ("actions".into(), table(&["id", "label"], &[json!([1, "旧行为"])])),
        ]);
        let mut live = frozen.clone();
        live.get_mut("meta").unwrap().rows.insert(json!(["human_revision", "1"]).to_string(), 1);
        for name in ["human_state_marks", "human_state_mark_undo"] {
            live.insert(name.into(), table(&["value"], &[json!(["new-state"])]));
        }
        (live, frozen)
    }

    #[test]
    fn projection_keeps_exact_state_and_unlisted_or_changed_old_data() {
        let (live, frozen) = states();
        assert_eq!(historical_projection(&live, &frozen, 7, 9).unwrap(), frozen);
        assert!(live.contains_key("human_state_marks"));
        let mut unknown = live.clone();
        unknown.insert("human_future".into(), Table::default());
        assert_ne!(historical_projection(&unknown, &frozen, 7, 9).unwrap(), frozen);
        let mut changed = live.clone();
        *changed.get_mut("actions").unwrap().rows.values_mut().next().unwrap() = 2;
        assert_ne!(historical_projection(&changed, &frozen, 7, 9).unwrap(), frozen);
        assert_eq!(historical_projection(&live, &frozen, 8, 9).unwrap(), live);
        assert_eq!(historical_projection(&live, &frozen, 7, 8).unwrap(), live);
    }

    #[test]
    fn a_frozen_table_or_meta_key_cannot_be_hidden() {
        let (live, frozen) = states();
        for name in ["human_state_marks", "human_state_mark_undo", "meta"] {
            let mut existing = frozen.clone();
            existing.insert(name.into(), live[name].clone());
            assert!(historical_projection(&live, &existing, 7, 9).is_err());
        }
    }

    #[test]
    fn physical_view_mapping_requires_equal_typed_rows_and_keeps_raw_state() {
        let (mut live, mut frozen) = states();
        let terms = table(&["id", "key"], &[json!([1, "synthetic"])]);
        live.insert("term_records".into(), terms.clone());
        live.insert("terms".into(), terms.clone());
        frozen.insert("terms".into(), terms);
        assert_eq!(historical_projection(&live, &frozen, 7, 9).unwrap(), frozen);
        for changed in [table(&["id", "key"], &[json!(["1", "synthetic"])]), Table::default()] {
            let mut mismatch = live.clone();
            mismatch.insert("term_records".into(), changed);
            assert!(historical_projection(&mismatch, &frozen, 7, 9).is_err());
        }
        let mut hidden = live.clone();
        hidden.get_mut("terms").unwrap().rows.clear();
        assert!(historical_projection(&hidden, &frozen, 7, 9).is_err());
        frozen.insert("term_records".into(), live["term_records"].clone());
        assert!(historical_projection(&live, &frozen, 7, 9).is_err());
        assert!(live.contains_key("term_records"));
    }
}
