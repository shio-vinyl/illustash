//! 角色目录（#837）：识别出来的角色 tag 属于哪个角色、和上级是什么关系、对应哪个 Bangumi 角色。
//!
//! 数据由 `tools/character_catalog/build.py` 从 D 站的别名和蕴含、Bangumi 整库导出离线生成，第一版编进应用
//! （`tags/character_catalog.json`）。目录里不存名字：名字由应用按 Bangumi ID 在线取（`pick` 说明取哪一栏、
//! 哪一段）。每个角色一行，列名在 `columns` 里：
//!
//! - `id`：公共目录 ID，发布过的不改、不复用；
//! - `tag`：D 站规范 tag；`tags`：映射到同一个角色的其他识别 tag（多是 D 站改名前的旧 tag）；
//! - `parent`、`relation`、`action`：上级的目录 ID；关系是造型（`appearance`）、另一种形态（`form`）或
//!   判断不出（`unknown`）；`auto` 自动挂到上级下面，`ask` 整对问用户。自动挂的造型，上级不会再是自动挂的造型；
//! - `variant`：造型名；`bangumi`、`pick`：Bangumi 角色 ID 和取名提示；`wikidata`：Wikidata 条目号。
use crate::error::invalid;
use crate::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

/// 这个版本认识的目录格式。
pub const FORMAT: &str = "illustash-character-catalog/1";
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
        let columns: Vec<&str> = doc["columns"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        let col = |name: &str| columns.iter().position(|c| *c == name);
        let need = ["id", "tag", "tags", "parent", "relation", "action", "variant", "bangumi", "pick", "wikidata"];
        let mut at = HashMap::new();
        for name in need {
            let Some(i) = col(name) else { return invalid(format!("角色目录缺少列 {name}")) };
            at.insert(name, i);
        }
        let mut characters = Vec::new();
        for row in doc["characters"].as_array().into_iter().flatten() {
            let get = |name: &str| row.get(at[name]).unwrap_or(&Value::Null);
            let id = get("id").as_u64().and_then(|v| u32::try_from(v).ok()).filter(|v| *v > 0);
            let (Some(id), Some(tag)) = (id, text(get("tag"))) else {
                return invalid("角色目录里有一行缺少 ID 或 tag");
            };
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
            if parent.is_some() != relation.is_some() || relation.is_some() != action.is_some() {
                return invalid(format!("角色目录：{tag} 的上级、关系和处理方式要一起给"));
            }
            characters.push(Character {
                id,
                tag,
                tags: get("tags").as_array().into_iter().flatten().filter_map(text).collect(),
                parent,
                relation,
                action,
                variant: text(get("variant")),
                bangumi: get("bangumi").as_u64(),
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
            for t in std::iter::once(&c.tag).chain(&c.tags) {
                if by_tag.insert(t.clone(), i).is_some() {
                    return invalid(format!("角色目录：tag {t} 出现在两个角色里"));
                }
            }
        }
        for c in &characters {
            let Some(p) = c.parent else { continue };
            let Some(&pi) = by_id.get(&p) else {
                return invalid(format!("角色目录：{} 的上级不存在", c.tag));
            };
            if p == c.id {
                return invalid(format!("角色目录：{} 的上级是自己", c.tag));
            }
            if c.action == Some(Action::Auto) && characters[pi].action == Some(Action::Auto) {
                return invalid(format!("角色目录：{} 挂到了另一个自动挂的造型下面", c.tag));
            }
        }
        let mut works = HashMap::new();
        for w in doc["works"].as_array().into_iter().flatten() {
            if let (Some(tag), Some(id)) = (w.get(0).and_then(text), w.get(1).and_then(Value::as_u64)) {
                works.insert(tag, id);
            }
        }
        Ok(Self { revision, characters, by_id, by_tag, works })
    }

    /// 按识别 tag 找角色（规范 tag 和旧 tag 都认）。
    pub fn character(&self, tag: &str) -> Option<&Character> {
        self.by_tag.get(tag).map(|&i| &self.characters[i])
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

/// 导入时用的目录：平时是编进应用的那份；测试可以用 [`override_for_thread`] 在当前线程换一份。
pub fn current() -> &'static Catalog {
    OVERRIDE.with(std::cell::Cell::get).unwrap_or_else(bundled)
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
