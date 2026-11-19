//! 浏览选区里的「更多像这张的」。
use crate::{
    example_features::{self, Space},
    example_scores::{self, Examples, Scorer},
    http::LibraryContext,
    Error, Result,
};
use rusqlite::{params, params_from_iter, types::Value, Connection};
use std::{collections::HashMap, sync::Arc};

pub(crate) struct Search {
    ids: Vec<String>,
    space: Space,
}

const MAX_EXAMPLES: usize = 20;
const MAX_RESULTS: usize = 500;

impl Search {
    pub(crate) fn parse(filter: &HashMap<String, String>) -> Result<Option<Self>> {
        let Some(like) = filter.get("like") else {
            if filter.contains_key("like_by") || filter.get("order").is_some_and(|v| v == "similar") {
                return Err(Error::invalid("相似检索需要指定例图"));
            }
            return Ok(None);
        };
        let invalid = || Error::invalid("相似检索需要 1–20 张例图");
        // 只接受有界的 UUID 列表。历史图库 ID 和合成库 ID 也走同一套查询。
        if like.len() > 4096 {
            return Err(invalid());
        }
        let mut ids: Vec<String> = serde_json::from_str(like).map_err(|_| invalid())?;
        if ids.is_empty() || ids.len() > MAX_EXAMPLES || ids.iter().any(|id| id.trim().is_empty() || id.len() > 160) {
            return Err(invalid());
        }
        ids.sort_unstable();
        ids.dedup();
        let space = match filter.get("like_by").map(String::as_str).unwrap_or("wd") {
            "wd" => Space::Tags,
            "ccip" => Space::Ccip,
            _ => return Err(Error::invalid("未知相似检索方式")),
        };
        if filter.get("order").is_some_and(|v| !matches!(v.as_str(), "similar" | "newest" | "oldest" | "color")) {
            return Err(Error::invalid("未知排序"));
        }
        Ok(Some(Self { ids, space }))
    }

    /// 针对当前事务的候选集取前 500，再把成员和顺序放进连接的临时表。
    pub(crate) fn rank(
        &self,
        c: &Connection,
        context: &LibraryContext,
        conditions: &str,
        args: &[Value],
    ) -> Result<()> {
        let missing = || Error::invalid("这张图还没有特征，识别完成后再试");
        let examples = serde_json::to_string(&self.ids).expect("a list of UUID strings serializes");
        let existing = c
            .prepare("SELECT uuid FROM assets WHERE uuid IN (SELECT value FROM json_each(?)) ORDER BY uuid")?
            .query_map([&examples], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if existing.is_empty() {
            return Err(missing());
        }
        let source = context.source.as_ref().ok_or_else(missing)?;
        let root = context.library.as_deref().or_else(|| source.parent()).ok_or_else(missing)?;
        let base = root.join("features");
        let matrix = if let Some(cache) = &context.examples {
            cache.get(source, &base, self.space)?
        } else {
            let reader = crate::app_library::read_connection(source)?;
            Arc::new(example_features::load(&reader, &base, self.space)?)
        };
        let positives: Vec<_> = existing.iter().filter_map(|id| matrix.row(id)).collect();
        if positives.is_empty() {
            return Err(missing());
        }
        let mut related = c.prepare(
            "SELECT copy.uuid FROM duplicates example JOIN duplicates copy ON copy.grp=example.grp
             WHERE example.uuid IN (SELECT value FROM json_each(?))",
        )?;
        let mut excluded: Vec<_> = related
            .query_map([examples], |r| r.get::<_, String>(0))?
            .filter_map(|result| result.map(|id| matrix.row(&id)).transpose())
            .collect::<rusqlite::Result<_>>()?;
        excluded.extend(positives.iter().copied());
        let mut stmt = c.prepare(&format!("SELECT a.uuid FROM assets a WHERE {conditions}"))?;
        let candidates = stmt
            .query_map(params_from_iter(args), |r| r.get::<_, String>(0))?
            .filter_map(|result| result.map(|id| matrix.row(&id)).transpose())
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let result = example_scores::top(
            &matrix,
            &Examples { positives: &positives, exclude: &excluded, ..Examples::default() },
            &Scorer::Centroid,
            &candidates,
            MAX_RESULTS,
        );
        c.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS like_rank(uuid TEXT PRIMARY KEY, rank INTEGER NOT NULL);
             DELETE FROM temp.like_rank;",
        )?;
        let mut insert = c.prepare("INSERT INTO temp.like_rank VALUES (?,?)")?;
        for (rank, (row, _)) in result.into_iter().enumerate() {
            insert.execute(params![matrix.uuid(row), rank as i64])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
