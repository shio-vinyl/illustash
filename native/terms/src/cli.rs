//! 词条核心的命令行。标准输出只有一行 JSON，错误写标准错误。
//!
//! - `validate <证据包目录>`：只读校验，不连数据库、不联网。
//! - `migrate --db <library.sqlite>`：建词条表或升级到当前版本（一个事务）；库必须已存在。
//! - `character-command --db <library.sqlite>`：角色业务命令（character_commands），标准输入给一个 JSON 命令
//!   `{"op": …, …}`；一个核心事务，`effect` 给出时同事务写 backend_effects。
//! - `learning-snapshot --db <library.sqlite>`：S3 学习输入的版本化指纹（只读）。
//! - `publish-learning --db <library.sqlite>`：标准输入给完整学习批次；原子发布或返回 stale。
//! - `reproject-characters --db <library.sqlite>`：S3 从人工结论与候选（`assertions`、`candidates`）重算角色的归属派生表。
//! - `reproject --db <library.sqlite>`：由证据和裁决整表重建投影（投影随时可以算回来）。
//! - `undo --db <library.sqlite> --action <id>`：撤销最近一步，且它必须正是 `<id>`、由词条核心写入。
//!   由 tests/reference/web/terms_core.py（#9 已删除） 调用。
//!
//! - `library-create --dest <新包.pilib> --folder <来源目录> [--reference <参考.sqlite>] [--name <名称>] [--preset pixiv|x|other]`：显式创建新库，不扫描、升级或启动 worker。
//!
//! 独立程序 `illustash-terms` 和桌面外壳（desktop/，第一个参数是这几个子命令时）都走这里。
//! - `import --db <library.sqlite> --library-id <id> --package <证据包目录> [--new-assets]`：导入证据包；
//!   `--new-assets` 允许登记本库还没进 `assets` 的图（同步导入新图时先交 WD 证据）。
//! - `tag --db <library.sqlite> --name <标签> --verdict yes|clear --label <历史文案>`：贴上 / 去掉用户标签，
//!   标准输入给图的 uuid 数组。
//! - `judge --db <library.sqlite> --term <词条 key> --verdict yes|no|clear --label <历史文案>`：人工裁决，
//!   标准输入给图的 `assets.uuid` 列表（JSON 数组）；不在图库里的图略过。
//!
//! - `names-command --db <library.sqlite>`：名字写入（names_commands），标准输入给 JSON 命令。
//! - `names-project --db <library.sqlite>`：整表重建显示投影（names_view）。
//! - `names-search --db <library.sqlite>`：名字搜索（names_search），标准输入给 `{"query": …}`，输出 `{"hits": {身份: 排名}}`。
//!
//! 原来的收编、切换与核对命令（`sync-*`、`cutover-*`、`verify-*`）随 #201 第 4 步删除：核心第 8 版拒绝没切换完的
//! 图库，新建图库由 `new_library` 一开始就在核心里。
//!
//! 退出码：0 成功；2 用法或输入不合法；3 历史已变化或与数据冲突；4 数据库版本不对；5 要调用方给一个新 id
//! （标准错误是它在命令里的 JSON 指针，什么都没写）；1 其他错误。

use crate::package::{Package, Status};
use crate::store::{self, Undo};
use crate::Error;
use serde_json::json;
use std::path::Path;

pub const COMMANDS: [&str; 25] = [
    "recognition-import",
    "wd-extension",
    "ccip-longtail",
    "folder-scan",
    "photos-scan",
    "library-scan",
    "library-create",
    "http",
    "serve",
    "wd-pending",
    "names-command",
    "names-project",
    "names-search",
    "character-command",
    "identity-command",
    "learning-snapshot",
    "publish-learning",
    "reproject-characters",
    "validate",
    "migrate",
    "import",
    "judge",
    "tag",
    "reproject",
    "undo",
];

/// 第一个参数是不是词条核心的子命令（桌面外壳据此决定是跑命令还是开窗口）。
pub fn is_command(first: &str) -> bool {
    COMMANDS.contains(&first)
}

/// 执行一条命令（不含程序名），返回退出码。
pub fn run(args: &[String]) -> u8 {
    if args.first().is_some_and(|a| a == "http") {
        let code = match crate::http::run(&args[1..]) {
            Ok(code) => code,
            Err(e) => {
                let (code, message) = error_response(e);
                eprintln!("{message}");
                code
            }
        };
        crate::diagnostics::logs::flush();
        return code;
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["recognition-import", "--library", _library, "--root", root, "--model-specs"] => crate::app_recognition::model_specs(Path::new(root)),
        ["recognition-import", "--library", library, "--root", root, "--resume"] => crate::app_library::Library::open(Path::new(library)).and_then(|lib| crate::app_recognition::resume_at(&lib,Path::new(root),&std::sync::atomic::AtomicBool::new(false),&|_,_| Ok(()))),
        ["recognition-import", "--library", library, "--root", root, "--package", package] => crate::app_library::Library::open(Path::new(library)).and_then(|lib| crate::app_recognition::import_package(&lib,Path::new(package),Path::new(root),&std::sync::atomic::AtomicBool::new(false),&|_,_| Ok(()))),
        ["ccip-longtail", "--library", library, "--root", root, "--package", package] =>
            crate::app_library::Library::open(Path::new(library)).and_then(|lib|
                crate::app_ccip_longtail::apply_file(&lib, Path::new(root), Path::new(package), &|| Ok(()), &crate::app_imports::Silent)),
        ["ccip-longtail", "--library", library, "--withdraw"] =>
            crate::app_library::Library::open(Path::new(library)).and_then(|lib| crate::app_ccip_longtail::withdraw(&lib, &crate::app_imports::Silent)),
        ["ccip-longtail", "--library", library, "--status"] =>
            crate::app_library::Library::open(Path::new(library)).and_then(|lib| crate::app_ccip_longtail::status(&lib, &crate::app_resources::default_root())),
        ["ccip-longtail", ..] => Err(Error::invalid(
            "用法：ccip-longtail --library <图库.pilib> (--root <资源目录> --package <参考包> | --status | --withdraw)"
        )),
        ["wd-extension", "--library", library, "--package", package] => crate::app_library::Library::open(Path::new(library)).and_then(|lib| crate::app_wd_extension::apply(&lib,Path::new(package))),
        ["wd-extension", "--library", library, "--withdraw"] => crate::app_library::Library::open(Path::new(library)).and_then(|lib| crate::app_wd_extension::withdraw(&lib)),
        ["wd-extension", ..] => Err(Error::invalid("用法：wd-extension --library <图库.pilib> --package <扩展包> | wd-extension --library <图库.pilib> --withdraw")),
        ["folder-scan", "--db", db] => {
            let mut input=String::new();
            let source = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).map_err(Error::from).and_then(|_| crate::strict_json::parse(&input).map_err(Error::Invalid));
            source.and_then(|source| crate::app_folder::scan(&source, Path::new(db), &|| crate::app_sync::check(&std::sync::atomic::AtomicBool::new(false)), &|_,_| {}).map(|s| s.counts))
        },
        ["photos-scan", "--db", db] => {
            let mut input=String::new();
            let source = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).map_err(Error::from).and_then(|_| crate::strict_json::parse(&input).map_err(Error::Invalid));
            source.and_then(|source| crate::app_photos::scan(&source, Path::new(db), &|| crate::app_sync::check(&std::sync::atomic::AtomicBool::new(false)), &|_,_| {}))
        }
        ["library-scan", "--library", path] => crate::app_library::Library::open(Path::new(path)).and_then(|library| crate::app_sync::run(&library,&std::sync::atomic::AtomicBool::new(false),&|_| {})),
        ["library-create", rest @ ..] => create_library(rest),
        ["serve"] => crate::service::run(),
        ["wd-pending", "--db", db, "--source", source] => existing(Path::new(db)).and_then(|mut c| {
            let mut input = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)?;
            let input = crate::strict_json::parse(&input).map_err(Error::Invalid)?;
            let uuids: Vec<String> = serde_json::from_value(input["uuids"].clone()).map_err(|_| Error::Invalid("WD 范围必须是图片列表".into()))?;
            crate::wd_pending::pending(&mut c, Path::new(source), &uuids, &input["library_id"])
        }),
        ["character-command", "--db", db] => {
            let mut input = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
                .map_err(Error::from)
                .and_then(|_| crate::strict_json::parse(&input).map_err(Error::Invalid))
                .and_then(|input| character_request(Path::new(db), &input))
        },
        ["identity-command", "--db", db] => existing(Path::new(db)).and_then(|mut c| {
            crate::schema::require_current(&c)?;
            let mut input=String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)?;
            let input=crate::strict_json::parse(&input).map_err(Error::Invalid)?;
            let tx=c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            crate::schema::begin_writing(&tx)?;
            let result=crate::character_identity::command(&tx,&input)?;
            if let Some(effect)=input.get("effect").and_then(serde_json::Value::as_str) {
                tx.execute("INSERT INTO backend_effects(kind) VALUES (?)", [effect])?;
            }
            crate::schema::commit_writing(tx)?;
            Ok(result)
        }),
        ["learning-snapshot", "--db", db] => existing(Path::new(db)).and_then(|mut c| crate::character_learning::snapshot(&mut c)),
        ["publish-learning", "--db", db] => existing(Path::new(db)).and_then(|mut c| {
            let mut input = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)?;
            let input = crate::strict_json::parse(&input).map_err(Error::Invalid)?;
            crate::character_learning::publish(&mut c, &input)
        }),
        ["reproject-characters", "--db", db] => existing(Path::new(db)).and_then(|mut c| {
            crate::schema::require_current(&c)?;
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            crate::schema::begin_writing(&tx)?;
            crate::character_projection::rebuild(&tx, None)?;
            crate::schema::commit_writing(tx)?;
            Ok(json!({"rebuilt": true}))
        }),
        ["validate", dir] => validate(Path::new(dir)),
        ["migrate", "--db", db] => migrate(Path::new(db)),
        ["names-command", "--db", db] => existing(Path::new(db)).and_then(|mut c| {
            let args: serde_json::Value = serde_json::from_reader(std::io::stdin())
                .map_err(|e| Error::Invalid(format!("名字命令不是 JSON：{e}")))?;
            crate::names_commands::run(&mut c, &args)
        }),
        ["names-search", "--db", db] => existing(Path::new(db)).and_then(|c| {
            let args: serde_json::Value = serde_json::from_reader(std::io::stdin())
                .map_err(|e| Error::Invalid(format!("搜索参数不是 JSON：{e}")))?;
            let query = args
                .get("query")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| Error::Invalid("搜索参数要有字符串 query".into()))?;
            Ok(json!({"hits": crate::names_search::hits(&c, query)?}))
        }),
        ["names-project", "--db", db] => existing(Path::new(db)).and_then(|mut c| {
            crate::schema::require_current(&c)?;
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let (shown, keys) = crate::names_view::rebuild(&tx)?;
            tx.commit()?;
            Ok(json!({"shown": shown, "keys": keys}))
        }),
        ["import", "--db", db, "--library-id", library, "--package", dir] => import(Path::new(db), library, Path::new(dir), false),
        ["import", "--db", db, "--library-id", library, "--package", dir, "--new-assets"] => {
            import(Path::new(db), library, Path::new(dir), true)
        }
        ["tag", "--db", db, "--name", name, "--verdict", verdict, "--label", label] => tag(Path::new(db), name, verdict, label),
        ["judge", "--db", db, "--term", term, "--verdict", verdict, "--label", label] => judge(Path::new(db), term, verdict, label),
        ["reproject", "--db", db] => existing(Path::new(db)).and_then(|mut c| {
            crate::schema::require_current(&c)?;
            crate::store::project_all(&mut c)?;
            let rows: i64 = c.query_row("SELECT count(*) FROM term_projection", [], |x| x.get(0))?;
            Ok(json!({"projection_rows": rows}))
        }),
        ["undo", "--db", db, "--action", action] => match action.parse() {
            Ok(action) => undo(Path::new(db), store::ActionId::from_raw(action)),
            Err(_) => Err(Error::Invalid(format!("步骤 id 不是整数：{action}"))),
        },
        _ => Err(Error::Invalid(
            "用法：library-create --dest <新包.pilib> --folder <来源目录> [--reference <参考.sqlite>] [--name <名称>] [--preset x|pixiv|other] | learning-snapshot --db <library.sqlite> | publish-learning --db <library.sqlite>（标准输入：完整学习批次） | reproject-characters --db <library.sqlite> | validate <证据包目录> | migrate --db <library.sqlite> | import --db <library.sqlite> --library-id <id> --package <目录> | tag --db <library.sqlite> --name <标签> --verdict yes|clear --label <文案>（标准输入：图的 uuid 数组） | judge --db <library.sqlite> --term <key> --verdict yes|no|clear --label <文案>（标准输入：图的 uuid 数组） | reproject --db <library.sqlite> | undo --db <library.sqlite> --action <id>"
                .into(),
        )),
    };
    match result {
        Ok(out) => {
            println!("{out}");
            if out.get("status").and_then(serde_json::Value::as_str) == Some("pending") {
                2
            } else {
                0
            }
        }
        Err(e) => {
            let (code, message) = error_response(e);
            eprintln!("{message}");
            code
        }
    }
}

fn validate(dir: &Path) -> crate::Result<serde_json::Value> {
    let p = Package::open(dir)?;
    let done = p.manifest.assets.iter().filter(|a| a.status == Status::Completed).count();
    Ok(json!({
        "fingerprint": p.fingerprint,
        "library_id": p.manifest.library_id,
        "producer": p.manifest.producer,
        "assets": p.manifest.assets.len(),
        "completed": done,
        "evidence": p.rows.len(),
    }))
}

use crate::app_service::existing;

/// 名字收编的报告：通过写标准输出；不通过按冲突退出（退出码 3），报告写标准错误。
fn migrate(db: &Path) -> crate::Result<serde_json::Value> {
    let from = crate::migrate(&mut existing(db)?)?;
    Ok(json!({"from": from, "to": crate::SCHEMA_VERSION}))
}

fn undo(db: &Path, action: store::ActionId) -> crate::Result<serde_json::Value> {
    let mut conn = existing(db)?;
    Ok(match store::undo(&mut conn, Some(action))? {
        Undo::Undone { action, label } => json!({"status": "undone", "action": action.raw(), "label": label}),
        // 调用方判断这一步属于词条核心，这里却不认：两边规则不一致，不能当成撤销成功。
        Undo::Foreign { action } => {
            return Err(Error::Conflict(format!("历史第 {} 步不是词条核心写的，不能由它撤销", action.raw())))
        }
        Undo::Nothing => return Err(Error::Conflict(store::HISTORY_CHANGED.into())),
    })
}

fn import(db: &Path, library: &str, dir: &Path, new_assets: bool) -> crate::Result<serde_json::Value> {
    let package = Package::open(dir)?;
    let mut conn = existing(db)?;
    crate::schema::require_current(&conn)?;
    let r = store::import_package_with(&mut conn, &package, library, new_assets)?;
    Ok(json!({"package": r.package, "added": r.added, "duplicate": r.duplicate, "skipped": r.skipped,
              "created_terms": r.created_terms, "already": r.already}))
}

fn judge(db: &Path, term: &str, verdict: &str, label: &str) -> crate::Result<serde_json::Value> {
    let verdict = match verdict {
        "yes" => store::Verdict::Yes,
        "no" => store::Verdict::No,
        "clear" => store::Verdict::Clear,
        other => return Err(Error::Invalid(format!("裁决只能是 yes、no、clear，不是 {other}"))),
    };
    let uuids = stdin_uuids()?;
    let mut conn = existing(db)?;
    crate::schema::require_current(&conn)?;
    let (term, _) = store::term_by_key(&conn, term)?.ok_or_else(|| Error::Invalid(format!("词条不存在：{term}")))?;
    write_verdicts(&mut conn, term, &uuids, verdict, label)
}

/// 用户标签：按名字找概念，「加上」时没有就建；「去掉」是人收回（clear），不是否定。
fn tag(db: &Path, name: &str, verdict: &str, label: &str) -> crate::Result<serde_json::Value> {
    let verdict = match verdict {
        "yes" => store::Verdict::Yes,
        "clear" => store::Verdict::Clear,
        other => return Err(Error::Invalid(format!("用户标签只能 yes（加上）或 clear（去掉），不是 {other}"))),
    };
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.chars().count() > 60 {
        return Err(Error::Invalid("标签应为 1–60 个字符".into()));
    }
    let uuids = stdin_uuids()?;
    let mut conn = existing(db)?;
    crate::schema::require_current(&conn)?;
    let create = (verdict == store::Verdict::Yes).then_some("user");
    let term = crate::legacy_labels::concept_by_name(&conn, &name, create)?
        .ok_or_else(|| Error::Invalid(format!("没有标签「{name}」")))?;
    write_verdicts(&mut conn, term, &uuids, verdict, label)
}

fn stdin_uuids() -> crate::Result<Vec<String>> {
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)?;
    let uuids: Vec<String> =
        serde_json::from_str(&input).map_err(|e| Error::Invalid(format!("标准输入应为图的 uuid 数组：{e}")))?;
    if uuids.is_empty() || uuids.len() > 5000 {
        return Err(Error::Invalid("一次 1–5000 张图".into()));
    }
    Ok(uuids)
}

fn write_verdicts(
    conn: &mut rusqlite::Connection,
    term: store::TermId,
    uuids: &[String],
    verdict: store::Verdict,
    label: &str,
) -> crate::Result<serde_json::Value> {
    let mut targets = Vec::new();
    for uuid in uuids {
        if let Some(asset) = store::library_asset(conn, uuid)? {
            targets.push(store::VerdictTarget::Image { asset, term });
        }
    }
    if targets.is_empty() {
        return Err(Error::Invalid("图片不存在".into()));
    }
    let action = store::judge(conn, &targets, verdict, "user", "", label)?;
    Ok(json!({"action": action.raw(), "assets": targets.len()}))
}

pub(crate) use crate::app_service::character_request;

/// 只用于新建的入口：所有文件系统输入都显式给出；不打开当前或默认图库。
fn create_library(args: &[&str]) -> crate::Result<serde_json::Value> {
    let input = create_library_args(args)?;
    crate::app_create::create_folder(
        Path::new(input.dest),
        Path::new(input.folder),
        input.name,
        input.preset,
        input.reference.map(Path::new),
    )
}

#[derive(Debug)]
struct CreateLibraryArgs<'a> {
    dest: &'a str,
    folder: &'a str,
    reference: Option<&'a str>,
    name: Option<&'a str>,
    preset: Option<&'a str>,
}

fn create_library_args<'a>(args: &[&'a str]) -> crate::Result<CreateLibraryArgs<'a>> {
    let mut dest = None;
    let mut folder = None;
    let mut reference = None;
    let mut name = None;
    let mut preset = None;
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let target = match *flag {
            "--dest" => &mut dest,
            "--folder" => &mut folder,
            "--reference" => &mut reference,
            "--name" => &mut name,
            "--preset" => &mut preset,
            _ => return Err(Error::Invalid(format!("不认识建库参数：{flag}"))),
        };
        if target.is_some() {
            return Err(Error::Invalid(format!("重复建库参数：{flag}")));
        }
        let value = args.next().ok_or_else(|| Error::Invalid(format!("建库参数缺少值：{flag}")))?;
        if value.starts_with("--") || value.is_empty() {
            return Err(Error::Invalid(format!("建库参数缺少值：{flag}")));
        }
        *target = Some(*value);
    }
    let required =
        |value: Option<&'a str>, flag: &str| value.ok_or_else(|| Error::Invalid(format!("建库必须显式提供 {flag}")));
    Ok(CreateLibraryArgs {
        dest: required(dest, "--dest")?,
        folder: required(folder, "--folder")?,
        reference,
        name,
        preset,
    })
}

pub(crate) fn error_response(e: Error) -> (u8, String) {
    match e {
        Error::Invalid(m) => (2, m),
        Error::Conflict(m) => (3, m),
        Error::Schema(m) => (4, m),
        Error::NeedId(m) => (5, m),
        other => (1, other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fresh_creation_requires_explicit_paths_and_never_falls_back() {
        let input = super::create_library_args(&[
            "--reference",
            "ref.sqlite",
            "--folder",
            "source",
            "--dest",
            "new.pilib",
            "--preset",
            "x",
            "--name",
            "图库",
        ])
        .unwrap();
        assert_eq!((input.dest, input.folder, input.reference), ("new.pilib", "source", Some("ref.sqlite")));
        assert_eq!(input.name, Some("图库"));
        assert_eq!(input.preset, Some("x"));
        assert!(super::create_library_args(&["--dest", "new.pilib", "--folder", "source"])
            .unwrap()
            .reference
            .is_none());
        for (args, message) in [
            (vec![], "建库必须显式提供 --dest"),
            (vec!["--dest", "new.pilib"], "建库必须显式提供 --folder"),
            (vec!["--dest", "a", "--dest", "b"], "重复建库参数：--dest"),
            (vec!["--python", "python"], "不认识建库参数：--python"),
            (vec!["--folder"], "建库参数缺少值：--folder"),
        ] {
            assert!(matches!(super::create_library_args(&args), Err(crate::Error::Invalid(v)) if v == message));
        }
    }

    /// 桌面外壳只把 `COMMANDS` 里的子命令当命令跑（其余当成开窗口），所以 `run` 认的每个子命令都要在表里。
    #[test]
    fn every_command_run_accepts_is_listed() {
        let source = include_str!("cli.rs");
        let body = &source[source.find("let result = match args.as_slice()").unwrap()
            ..source.find("_ => Err(Error::Invalid(").unwrap()];
        let mut found = 0;
        for line in body.lines() {
            if let Some(rest) = line.trim_start().strip_prefix("[\"") {
                let name = &rest[..rest.find('"').unwrap()];
                assert!(super::is_command(name), "{name} 不在 COMMANDS 里");
                found += 1;
            }
        }
        assert!(found >= 24, "{found}");
    }
}
