//! 角色目录（#837）：识别出来的角色 tag 属于哪个角色、和上级是什么关系、对应哪个 Bangumi 角色。
//!
//! 数据由 `tools/character_catalog/build.py` 从 D 站的别名和蕴含、Bangumi 整库导出离线生成，第一版编进应用
//! （`tags/character_catalog.json`）。Bangumi 名字按 ID 在线取（`pick` 说明取哪一栏、
//! 哪一段）。无 Bangumi ID 时，可用有独立旁证的 Wikidata CC0 中文标签。每个角色一行，列名在 `columns` 里：
//!
//! - `id`：公共目录 ID，发布过的不改、不复用；
//! - `tag`：D 站规范 tag；`tags`：映射到同一个角色的其他识别 tag（多是 D 站改名前的旧 tag）；
//! - `parent`、`relation`、`action`：上级的目录 ID；关系是造型（`appearance`）、另一种形态（`form`）或
//!   判断不出（`unknown`）；`auto` 自动挂到上级下面，`ask` 整对问用户。自动挂的造型，上级不会再是自动挂的造型；
//! - `variant`：造型名；`bangumi`、`pick`：Bangumi 角色 ID 和取名提示；`wikidata`：Wikidata 条目号。
use crate::error::invalid;
use crate::Result;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// 这个版本认识的目录格式。
pub const FORMAT: &str = "illustash-character-catalog/1";
/// 随应用分发的修订号，用于在启动时选择适用的签名更新。
pub const BUNDLED_REVISION: u32 = 2;
static INSTALLED: OnceLock<Option<Catalog>> = OnceLock::new();
const BUNDLED: &str = include_str!("../../../tags/character_catalog.json");

/// 和上级的关系。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    /// 造型：换装、灵基、节日服。
    Appearance,
    /// 另一种形态：变身、平行版本、年龄、性转、换了身体。
    Form,
    /// D 站有蕴含，判断不出是哪一种。
    Unknown,
}

/// 目录对这层关系的处理。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// 自动挂到上级下面。
    Auto,
    /// 整对问用户。
    Ask,
}

/// 目录里的一个角色。
#[derive(Clone, Debug)]
pub struct Character {
    pub id: u32,
    pub tag: String,
    pub tags: Vec<String>,
    /// 已审核的团体条目，只作检索标签。
    pub group: bool,
    pub parent: Option<u32>,
    pub relation: Option<Relation>,
    pub action: Option<Action>,
    pub variant: Option<String>,
    pub bangumi: Option<u64>,
    pub pick: String,
    pub wikidata: Option<String>,
}

/// 解析好的一份目录。
#[derive(Debug)]
pub struct Catalog {
    pub revision: u32,
    characters: Vec<Character>,
    by_id: HashMap<u32, usize>,
    by_tag: HashMap<String, usize>,
    works: HashMap<String, u64>,
    wikidata_names: HashMap<String, String>,
}

fn text(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

impl Catalog {
    /// 解析并检查目录：格式、列、ID 和 tag 不重复、上级存在、自动挂的造型的上级不是另一个自动挂的造型。
    pub fn parse(source: &str) -> Result<Self> {
        let doc: Value = serde_json::from_str(source).map_err(|e| crate::Error::invalid(format!("角色目录：{e}")))?;
        if doc["format"] != FORMAT {
            return invalid("角色目录的格式不是这个版本认识的");
        }
        let revision = doc["revision"].as_u64().and_then(|r| u32::try_from(r).ok()).filter(|r| *r > 0);
        let Some(revision) = revision else { return invalid("角色目录的修订号不正确") };
        let columns: Vec<String> = serde_json::from_value(doc["columns"].clone())
            .map_err(|_| crate::Error::invalid("角色目录的列名不合法"))?;
        if columns.iter().collect::<HashSet<_>>().len() != columns.len() {
            return invalid("角色目录的列名重复");
        }
        let col = |name: &str| columns.iter().position(|c| c == name);
        let need = ["id", "tag", "tags", "parent", "relation", "action", "variant", "bangumi", "pick", "wikidata"];
        let mut at = HashMap::new();
        for name in need {
            let Some(i) = col(name) else { return invalid(format!("角色目录缺少列 {name}")) };
            at.insert(name, i);
        }
        let mut characters = Vec::new();
        let rows = doc["characters"].as_array().ok_or_else(|| crate::Error::invalid("角色目录缺少角色列表"))?;
        for row in rows {
            if row.as_array().is_none_or(|values| values.len() != columns.len()) {
                return invalid("角色目录的行与列不匹配");
            }
            let get = |name: &str| row.get(at[name]).unwrap_or(&Value::Null);
            let id = get("id").as_u64().and_then(|v| u32::try_from(v).ok()).filter(|v| *v > 0);
            let (Some(id), Some(tag)) = (id, text(get("tag"))) else {
                return invalid("角色目录里有一行缺少 ID 或 tag");
            };
            let group = match col("group").map(|index| &row[index]) {
                None | Some(Value::Null) => false,
                Some(Value::Bool(group)) => *group,
                _ => return invalid("角色目录的团体标记不合法"),
            };
            let tags: Vec<String> = serde_json::from_value(get("tags").clone())
                .map_err(|_| crate::Error::invalid("角色目录的别名列表不合法"))?;
            if tags.iter().any(|t| t.trim().is_empty()) || tag.trim().is_empty() {
                return invalid("角色目录的 tag 为空");
            }
            for field in ["relation", "action", "variant", "pick", "wikidata"] {
                if !get(field).is_null() && !get(field).is_string() {
                    return invalid(format!("角色目录：{tag} 的 {field} 字段不合法"));
                }
            }
            let relation = match get("relation").as_str() {
                None => None,
                Some("appearance") => Some(Relation::Appearance),
                Some("form") => Some(Relation::Form),
                Some("unknown") => Some(Relation::Unknown),
                Some(other) => return invalid(format!("角色目录：不认识的关系 {other}")),
            };
            let action = match get("action").as_str() {
                None => None,
                Some("auto") => Some(Action::Auto),
                Some("ask") => Some(Action::Ask),
                Some(other) => return invalid(format!("角色目录：不认识的处理方式 {other}")),
            };
            let parent = get("parent").as_u64().and_then(|v| u32::try_from(v).ok());
            let bangumi = get("bangumi").as_u64().filter(|id| *id > 0);
            if (!get("parent").is_null() && parent.is_none())
                || (!get("bangumi").is_null() && bangumi.is_none())
                || (group && (parent.is_some() || bangumi.is_some()))
                || (action == Some(Action::Auto) && relation != Some(Relation::Appearance))
            {
                return invalid(format!("角色目录：{tag} 的身份关系或外部编号不合法"));
            }
            if parent.is_some() != relation.is_some() || relation.is_some() != action.is_some() {
                return invalid(format!("角色目录：{tag} 的上级、关系和处理方式要一起给"));
            }
            characters.push(Character {
                id,
                tag,
                tags,
                group,
                parent,
                relation,
                action,
                variant: text(get("variant")),
                bangumi,
                pick: get("pick").as_str().unwrap_or("").to_owned(),
                wikidata: text(get("wikidata")),
            });
        }
        let mut by_id = HashMap::new();
        let mut by_tag = HashMap::new();
        for (i, c) in characters.iter().enumerate() {
            if by_id.insert(c.id, i).is_some() {
                return invalid(format!("角色目录：ID {} 重复", c.id));
            }
            for tag in std::iter::once(&c.tag).chain(&c.tags) {
                if by_tag.insert(tag.clone(), i).is_some() {
                    return invalid(format!("角色目录：tag {tag} 出现在两个角色里"));
                }
            }
        }
        for c in &characters {
            let Some(p) = c.parent else { continue };
            let Some(&pi) = by_id.get(&p) else {
                return invalid(format!("角色目录：{} 的上级不存在", c.tag));
            };
            if characters[pi].group {
                return invalid(format!("角色目录：{} 的上级是团体", c.tag));
            }
            if p == c.id {
                return invalid(format!("角色目录：{} 的上级是自己", c.tag));
            }
            if c.action == Some(Action::Auto) && characters[pi].action == Some(Action::Auto) {
                return invalid(format!("角色目录：{} 挂到了另一个自动挂的造型下面", c.tag));
            }
        }
        let mut works = HashMap::new();
        let pairs: Vec<(String, u64)> = serde_json::from_value(doc["works"].clone())
            .map_err(|_| crate::Error::invalid("角色目录的作品列表不合法"))?;
        for (tag, id) in pairs {
            if tag.is_empty() || id == 0 || works.insert(tag, id).is_some() {
                return invalid("角色目录有重复作品或作品编号不合法");
            }
        }
        let wikidata_names: HashMap<String, String> = match doc.get("wikidata_names") {
            None => HashMap::new(),
            Some(names) => serde_json::from_value(names.clone())
                .map_err(|_| crate::Error::invalid("角色目录的 Wikidata 中文名不合法"))?,
        };
        let known: HashSet<_> = characters.iter().filter_map(|role| role.wikidata.as_ref()).collect();
        for (id, name) in &wikidata_names {
            let number = id.strip_prefix('Q').and_then(|n| n.parse::<u64>().ok());
            if !known.contains(id)
                || number.is_none_or(|n| n == 0)
                || name.trim().is_empty()
                || name.chars().count() > 80
            {
                return invalid("角色目录的 Wikidata 中文名或条目号不合法");
            }
        }
        Ok(Self { revision, characters, by_id, by_tag, works, wikidata_names })
    }

    /// 按识别 tag 找角色（规范 tag 和旧 tag 都认）。
    pub fn character(&self, tag: &str) -> Option<&Character> {
        self.by_tag.get(tag).map(|&i| &self.characters[i])
    }

    /// 无 Bangumi 对应时，经制作端独立核对的 Wikidata 简中标签。
    pub fn reviewed_name(&self, tag: &str) -> Option<&str> {
        let role = self.character(tag)?;
        if role.group || role.bangumi.is_some() || role.action == Some(Action::Auto) {
            return None;
        }
        self.wikidata_names.get(role.wikidata.as_ref()?).map(String::as_str)
    }

    /// 按目录 ID 找角色。
    pub fn by_id(&self, id: u32) -> Option<&Character> {
        self.by_id.get(&id).map(|&i| &self.characters[i])
    }

    /// 这个角色的上级。
    pub fn parent(&self, c: &Character) -> Option<&Character> {
        c.parent.and_then(|p| self.by_id(p))
    }

    /// D 站作品 tag 对应的 Bangumi 条目 ID。
    pub fn work(&self, tag: &str) -> Option<u64> {
        self.works.get(tag).copied()
    }

    /// 全部角色，按目录 ID 排。
    pub fn characters(&self) -> &[Character] {
        &self.characters
    }
}

/// 编进应用的那一份目录。
pub fn bundled() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| Catalog::parse(BUNDLED).expect("编进应用的角色目录由测试保证能解析"))
}

thread_local! {
    static OVERRIDE: std::cell::Cell<Option<&'static Catalog>> = const { std::cell::Cell::new(None) };
}

/// 选定进程使用的已安装目录。调用方在启动时校验签名清单和文件，后续修订在下次启动时生效。
pub fn initialize(load: impl FnOnce() -> Option<Catalog>) {
    INSTALLED.get_or_init(|| load().filter(|catalog| catalog.revision > BUNDLED_REVISION));
}

/// 导入和取名共用的目录；测试可以用 [`override_for_thread`] 在当前线程换一份。
pub fn current() -> &'static Catalog {
    OVERRIDE.with(std::cell::Cell::get).or_else(|| INSTALLED.get().and_then(Option::as_ref)).unwrap_or_else(bundled)
}

/// 只给测试用：在当前线程换用另一份目录，`None` 换回编进应用的那份。
#[doc(hidden)]
pub fn override_for_thread(catalog: Option<&'static Catalog>) {
    OVERRIDE.with(|o| o.set(catalog));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_parses_and_keeps_its_invariants() {
        let c = Catalog::parse(BUNDLED).unwrap();
        assert_eq!(c.revision, BUNDLED_REVISION);
        assert!(c.characters().len() > 10_000);
        // 改名前的旧 tag 指向同一个角色。
        let old = c.character("kousaka_honoka").unwrap();
        assert_eq!(old.tag, "kosaka_honoka");
        // 造型挂在本体下、自动处理。
        let skin = c.character("hiyori_(swimsuit)_(blue_archive)").unwrap();
        assert_eq!(skin.relation, Some(Relation::Appearance));
        assert_eq!(skin.action, Some(Action::Auto));
        assert_eq!(c.parent(skin).unwrap().tag, "hiyori_(blue_archive)");
        assert_eq!(skin.variant.as_deref(), Some("swimsuit"));
    }

    #[test]
    fn rejects_bad_rows() {
        let doc = |rows: &str| {
            format!(
                r#"{{"format":"{FORMAT}","revision":1,"columns":["id","tag","tags","parent","relation","action","variant","bangumi","pick","wikidata"],"characters":[{rows}],"works":[]}}"#
            )
        };
        let ok = r#"[1,"a",[],null,null,null,null,null,"",null],[2,"b_(x)",["b_old"],1,"appearance","auto","x",null,"",null]"#;
        let c = Catalog::parse(&doc(ok)).unwrap();
        assert_eq!(c.character("b_old").unwrap().id, 2);
        for bad in [
            r#"[1,"a",[],null,null,null,null,null,"",null],[1,"b",[],null,null,null,null,null,"",null]"#,
            r#"[1,"a",[],null,null,null,null,null,"",null],[2,"b",["a"],null,null,null,null,null,"",null]"#,
            r#"[1,"a",[],9,"form","ask",null,null,"",null]"#,
            r#"[1,"a",[],null,"form",null,null,null,"",null]"#,
            r#"[1,"a",[],null,null,null,null,null,"",null],[2,"b",[],1,"appearance","auto",null,null,"",null],[3,"c",[],2,"appearance","auto",null,null,"",null]"#,
            r#"[1,"a",[],null,"weird","ask",null,null,"",null]"#,
        ] {
            assert!(Catalog::parse(&doc(bad)).is_err(), "{bad}");
        }
        assert!(Catalog::parse(&doc(ok).replace(FORMAT, "other/1")).is_err());
    }
}
