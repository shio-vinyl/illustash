//! S3：执行 `character_sql/` 里的归属 SQL，不在 Rust 里翻译它的规则。
//! 这些 SQL 文件是旧实现 Store._refresh_* 的原样截取（#9 已删除）。
use crate::{Error, Result};
use rusqlite::{params_from_iter, Connection, OptionalExtension};

const DEFAULT: &str = include_str!("character_sql/evidence_default.sql");
const HYBRID: &str = include_str!("character_sql/evidence_hybrid.sql");
const MEMBERS: &str = include_str!("character_sql/memberships.sql");
const TAGS: &str = include_str!("character_sql/tag_memberships.sql");
const FOLD: &str = include_str!("character_sql/fold.sql");
const FOLD_LEGACY: &str = include_str!("character_sql/fold_legacy.sql");
const SINGLE: &str = include_str!("character_sql/single_assets.sql");
const LEAD: &str = include_str!("character_sql/lead_assets.sql");
const MULTI: &[&str] = &[
    "multiple_girls",
    "multiple_boys",
    "multiple_others",
    "2girls",
    "3girls",
    "4girls",
    "5girls",
    "6+girls",
    "2boys",
    "3boys",
    "4boys",
    "5boys",
    "6+boys",
];

// 旧 SQL 原样保存，增量范围插在下列锚点上（锚点, 替换成）。
const EVIDENCE_SCOPE: (&str, &str) = ("GROUP BY", "WHERE v.uuid IN (SELECT uuid FROM touched) GROUP BY");
const SINGLE_SCOPE: (&str, &str) = ("value='solo'", "value='solo' AND g.uuid IN (SELECT uuid FROM touched)");
const LEAD_SCOPE: (&str, &str) = ("value='1girl'", "value='1girl' AND g.uuid IN (SELECT uuid FROM touched)");
const MEMBERS_SELECT: &str = "memberships SELECT ";
const MEMBERS_FILTER: &str = " WHERE NOT EXISTS(SELECT 1 FROM terms t";
const MEMBERS_FROM: &str = "FROM membership_rules m WHERE ";

fn once<'a>(sql: &str, anchor: &'a str) -> Result<&'a str> {
    match sql.matches(anchor).count() {
        1 => Ok(anchor),
        n => Err(Error::Schema(format!("SQL 里的锚点应恰好出现一次，实际 {n} 次：{anchor}"))),
    }
}

/// 在锚点处插入范围条件。锚点不是恰好一处就报错：SQL 的写法一变，不能悄悄丢掉过滤、退回整库。
pub(crate) fn scoped(sql: &str, anchor: &str, with: &str) -> Result<String> {
    Ok(sql.replacen(once(sql, anchor)?, with, 1))
}

fn split<'s>(sql: &'s str, anchor: &str) -> Result<(&'s str, &'s str)> {
    Ok(sql.split_once(once(sql, anchor)?).expect("锚点已确认出现一次"))
}

pub fn policy(c: &Connection) -> Result<bool> {
    let mode: Option<String> =
        c.query_row("SELECT value FROM meta WHERE key='identity_policy'", [], |r| r.get(0)).optional()?;
    match mode.as_deref() {
        None | Some("") => Ok(false),
        Some("hybrid") => Ok(true),
        _ => Err(Error::Conflict("角色核心只支持未设置身份策略或 hybrid".into())),
    }
}

fn touch(c: &Connection, uuids: Option<&[String]>) -> Result<()> {
    if let Some(uuids) = uuids {
        c.execute_batch("CREATE TEMP TABLE IF NOT EXISTS touched(uuid TEXT PRIMARY KEY); DELETE FROM touched;")?;
        let mut put = c.prepare("INSERT OR IGNORE INTO touched VALUES (?)")?;
        for uuid in uuids {
            put.execute([uuid])?;
        }
    }
    Ok(())
}

// ---- 候选（派生表 `candidates`）----

/// 候选：由当前有效证据按 (图, 身份, 来源) 取最大分算出。
/// `candidates` 与 `memberships` 一样是核心维护的派生表（#201 第 4 步用户决定），权威是证据批次；写证据的命令在同一个
/// 事务里按图刷新它。判定 SQL 和读者按索引直接查表：从证据现算时按图的条件推不进聚合，增量重算会慢几百倍。
const CANDIDATES: &str =
    "SELECT a.key,t.key,CASE e.source WHEN 'wd' THEN 'WD' WHEN 'ccip_index' THEN 'CCIP' WHEN 'wd_extension' THEN 'WD_EXT' ELSE 'CCIP_LONGTAIL' END,max(e.score)
         FROM term_active_evidence active
 JOIN term_evidence_set_rows current_row ON current_row.set_id=active.set_id
 JOIN term_evidence e ON e.id=current_row.evidence_id AND e.asset_id=active.asset_id
   AND e.source=active.source AND active.method IN (e.method,'*')
 JOIN term_assets a ON a.id=e.asset_id AND a.namespace='library'
         JOIN term_tree r ON r.term_id=e.term_id JOIN terms t ON t.id=e.term_id
         WHERE e.source||'.'||e.method IN ('wd.wd_character','ccip_index.char_ext','wd_extension.linear_head','ccip_longtail.anchor_p75')
         GROUP BY a.key,t.key,e.source";

/// 策略未设置时，候选的身份沿破坏式合并解析到代表（`_candidate` 与 `merge` 都这样写旧表）；hybrid 下是原始身份。
const CANDIDATES_RESOLVED: &str = "WITH RECURSIVE chain(term,key,depth) AS (
   SELECT r.term_id,t.key,0 FROM term_tree r JOIN terms t ON t.id=r.term_id
   UNION ALL SELECT c.term,r2.merged_into,c.depth+1 FROM chain c JOIN terms t2 ON t2.key=c.key
    JOIN term_tree r2 ON r2.term_id=t2.id WHERE r2.merged_into IS NOT NULL AND r2.merged_into<>'' AND c.depth<16),
  resolved(term,key) AS (SELECT term,key FROM chain c WHERE depth=(SELECT max(depth) FROM chain x WHERE x.term=c.term))
 SELECT a.key,k.key,CASE e.source WHEN 'wd' THEN 'WD' WHEN 'ccip_index' THEN 'CCIP' WHEN 'wd_extension' THEN 'WD_EXT' ELSE 'CCIP_LONGTAIL' END,max(e.score)
 FROM term_active_evidence active
 JOIN term_evidence_set_rows current_row ON current_row.set_id=active.set_id
 JOIN term_evidence e ON e.id=current_row.evidence_id AND e.asset_id=active.asset_id
   AND e.source=active.source AND active.method IN (e.method,'*')
 JOIN term_assets a ON a.id=e.asset_id AND a.namespace='library'
 JOIN resolved k ON k.term=e.term_id
 WHERE e.source||'.'||e.method IN ('wd.wd_character','ccip_index.char_ext','wd_extension.linear_head','ccip_longtail.anchor_p75')
 GROUP BY a.key,k.key,e.source";

/// 候选 SQL 里选来源的那一句，和 `assignable_extension` 换成的「只要一级 WD 扩展证据」。
const CANDIDATE_SOURCES: &str = "WHERE e.source||'.'||e.method IN ('wd.wd_character','ccip_index.char_ext','wd_extension.linear_head','ccip_longtail.anchor_p75')";
const ASSIGNABLE_SOURCES: &str =
    "WHERE e.source='wd_extension' AND e.method='linear_head' AND json_extract(e.data,'$.tier') IS NULL";

/// 小范围刷新时由图片驱动：原计划会先扫描全库证据并分组，最后才过滤图片。
const CANDIDATE_SCOPE: (&str, &str) = (
    "FROM term_active_evidence active",
    "FROM candidate_scope scoped CROSS JOIN term_assets scoped_asset
               ON scoped_asset.namespace='library' AND scoped_asset.key=scoped.uuid
             CROSS JOIN term_active_evidence active ON active.asset_id=scoped_asset.id",
);

/// 按图刷新 `candidates`（= 由当前有效证据算出的那几行）。
pub(crate) fn refresh_candidates(c: &Connection, uuids: &[String]) -> Result<()> {
    c.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS candidate_scope(uuid TEXT PRIMARY KEY); DELETE FROM candidate_scope;",
    )?;
    let mut put = c.prepare_cached("INSERT OR IGNORE INTO candidate_scope VALUES (?)")?;
    for u in uuids {
        put.execute([u])?;
    }
    c.execute("DELETE FROM candidates WHERE uuid IN (SELECT uuid FROM candidate_scope)", [])?;
    let select = if policy(c)? { CANDIDATES } else { CANDIDATES_RESOLVED };
    // 全量刷新保留原计划，避免为大集合增加逐图索引探测。
    let select =
        if uuids.len() <= 4096 { scoped(select, CANDIDATE_SCOPE.0, CANDIDATE_SCOPE.1)? } else { select.to_owned() };
    c.execute(
        &format!(
            "WITH rebuilt(uuid,entity,source,score) AS ({select})
             INSERT INTO candidates(uuid,entity,source,score) SELECT * FROM rebuilt WHERE uuid IN (SELECT uuid FROM candidate_scope)"
        ),
        [],
    )?;
    Ok(())
}

/// 候选里能补主归属的 WD 扩展候选（#602）：照候选的同一套 SQL 重算 `images` 选出的图，只算一级头的证据（不带
/// `tier: 2`）。结果在临时表 `assignable_extension(uuid, entity)` 里，身份的写法和 `candidates` 完全相同。
/// 只看还没有归属、有 WD 扩展候选的图，所以和补归属本身一样只碰少数图。
pub(crate) fn assignable_extension(c: &Connection, images: &str) -> Result<()> {
    c.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS assignable_scope(uuid TEXT PRIMARY KEY); DELETE FROM assignable_scope;
         CREATE TEMP TABLE IF NOT EXISTS assignable_extension(uuid TEXT NOT NULL,entity TEXT NOT NULL,
           PRIMARY KEY(uuid,entity)); DELETE FROM assignable_extension;",
    )?;
    c.execute(&format!("INSERT OR IGNORE INTO assignable_scope {images}"), [])?;
    let select = if policy(c)? { CANDIDATES } else { CANDIDATES_RESOLVED };
    let select = scoped(select, CANDIDATE_SCOPE.0, &CANDIDATE_SCOPE.1.replace("candidate_scope", "assignable_scope"))?;
    let select = scoped(&select, CANDIDATE_SOURCES, ASSIGNABLE_SOURCES)?;
    c.execute(
        &format!(
            "WITH rebuilt(uuid,entity,source,score) AS ({select})
             INSERT OR IGNORE INTO assignable_extension SELECT uuid,entity FROM rebuilt"
        ),
        [],
    )?;
    Ok(())
}

pub fn evidence(c: &Connection, uuids: Option<&[String]>) -> Result<()> {
    evidence_in(c, uuids, policy(c)?)
}
pub fn legacy_evidence(c: &Connection) -> Result<()> {
    let mode: Option<String> =
        c.query_row("SELECT value FROM meta WHERE key='identity_policy'", [], |r| r.get(0)).optional()?;
    evidence_in(c, None, mode.as_deref() == Some("hybrid"))
}
fn evidence_in(c: &Connection, uuids: Option<&[String]>, hybrid: bool) -> Result<()> {
    let sql = if hybrid { HYBRID } else { DEFAULT };
    touch(c, uuids)?;
    if uuids.is_some() {
        c.execute("DELETE FROM family_evidence WHERE uuid IN (SELECT uuid FROM touched)", [])?;
        c.execute(&scoped(sql, EVIDENCE_SCOPE.0, EVIDENCE_SCOPE.1)?, [])?;
    } else {
        c.execute("DELETE FROM family_evidence", [])?;
        c.execute(sql, [])?;
    }
    Ok(())
}

pub fn single(c: &Connection, uuids: Option<&[String]>) -> Result<()> {
    touch(c, uuids)?;
    for (table, sql, (anchor, with)) in [("single_assets", SINGLE, SINGLE_SCOPE), ("lead_assets", LEAD, LEAD_SCOPE)] {
        let scope = if uuids.is_some() { " WHERE uuid IN (SELECT uuid FROM touched)" } else { "" };
        c.execute(&format!("DELETE FROM {table}{scope}"), [])?;
        let sql = if uuids.is_some() { scoped(sql, anchor, with)? } else { sql.to_owned() };
        c.execute(&sql, params_from_iter(MULTI))?;
    }
    Ok(())
}

/// 重算指定图片（或全库）的角色归属，并刷新相应副本组的代表和可见性。
pub fn memberships(c: &Connection, uuids: Option<&[String]>) -> Result<()> {
    memberships_before_excluded(c, uuids)?;
    if uuids.is_some() {
        fold_touched(c)
    } else {
        fold(c)
    }
}
/// 只更新角色归属。需要重算排除状态的调用方在该步骤之后统一刷新折叠。
pub(crate) fn memberships_before_excluded(c: &Connection, uuids: Option<&[String]>) -> Result<()> {
    memberships_in(c, uuids, true)
}
/// 按旧版存储结构重建全库角色归属，并沿用该结构的副本折叠规则。
pub fn legacy_memberships(c: &Connection) -> Result<()> {
    memberships_in(c, None, false)?;
    c.execute(FOLD_LEGACY, [])?;
    Ok(())
}
fn memberships_in(c: &Connection, uuids: Option<&[String]>, core: bool) -> Result<()> {
    touch(c, uuids)?;
    let ids = uuids.map(|u| u.iter().collect::<std::collections::BTreeSet<_>>());
    if core {
        // 两个包都命中时保留已有扩展归属的来源。基础规则仍先计算。
        c.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS previous_extension_sources(uuid TEXT PRIMARY KEY,source TEXT NOT NULL);
             DELETE FROM previous_extension_sources;",
        )?;
        let scope = if uuids.is_some() { "AND uuid IN (SELECT uuid FROM touched)" } else { "" };
        c.execute(
            &format!(
                "INSERT INTO previous_extension_sources
                      SELECT uuid,sources FROM memberships WHERE sources IN ('WD_EXT','CCIP_LONGTAIL') {scope}"
            ),
            [],
        )?;
    }
    if uuids.is_none() {
        // 两张归属表共享同一组规则结果；原规则及排序键原样计算，临时集合仅存在于这次事务。
        c.execute_batch("DROP TABLE IF EXISTS temp.projected_membership_rules;")?;
        let select = split(split(MEMBERS, MEMBERS_SELECT)?.1, MEMBERS_FILTER)?.0;
        c.execute(&format!("CREATE TEMP TABLE projected_membership_rules AS SELECT {select}"), [])?;
    }
    for (table, sql) in [("memberships", MEMBERS), ("tag_memberships", TAGS)] {
        if let Some(ids) = &ids {
            if ids.len() <= 10 {
                let mut delete = c.prepare(&format!("DELETE FROM {table} WHERE uuid=?"))?;
                let mut insert = c.prepare(&format!("{} AND m.uuid=?", sql.trim_end()))?;
                for uuid in ids {
                    delete.execute([uuid])?;
                    insert.execute([uuid])?;
                }
                continue;
            }
            c.execute(&format!("DELETE FROM {table} WHERE uuid IN (SELECT uuid FROM touched)"), [])?;
            c.execute(&format!("{} AND m.uuid IN (SELECT uuid FROM touched)", sql.trim_end()), [])?;
        } else {
            c.execute(&format!("DELETE FROM {table}"), [])?;
            let materialized = split(sql, MEMBERS_FROM)?.1;
            c.execute(
                &format!("INSERT INTO {table} SELECT m.* FROM projected_membership_rules m WHERE {materialized}"),
                [],
            )?;
        }
    }
    if uuids.is_none() {
        c.execute_batch("DROP TABLE temp.projected_membership_rules;")?;
    }
    if core {
        crate::character_extension::fill_unassigned(c, uuids.is_some())?;
        c.execute_batch("DELETE FROM previous_extension_sources;")?;
    }
    if core {
        mirror(c, uuids.is_some())
    } else {
        Ok(())
    }
}

/// 切换时整片建一次人物 / 皮肤的投影镜像（此前只有核心命令按受影响的图增量维护；S5 真实副本上发现切换后这一片是空的）。
/// 只按当前的人工结论和候选表镜像，不重算派生表：旧版保留下来的增量结果（第 16 节已批准的 ≤1e-6 例外）原样进镜像。
pub(crate) fn mirror_all(c: &Connection) -> Result<()> {
    mirror(c, false)
}

/// 只替换角色/皮肤这一部分；画师/标签的投影保持不动。
fn mirror(c: &Connection, touched: bool) -> Result<()> {
    // S2 登记旧证据/裁决引用的图片。原型可能是另一张已在图库里的图片的第一条证据：在这里登记它。
    let scope = if touched { " WHERE uuid IN (SELECT uuid FROM touched)" } else { "" };
    // 语义同 library_asset，排序也相同：现有的登记不动，缺失或已移除的只在 assets 里存在时才登记。
    c.execute(
        &format!(
            "INSERT INTO term_assets(namespace,key)
          SELECT 'library',u.uuid FROM (
            SELECT uuid FROM memberships{scope} UNION SELECT uuid FROM tag_memberships{scope}
          ) u JOIN assets a ON a.uuid=u.uuid
          WHERE NOT EXISTS(SELECT 1 FROM term_assets t
            WHERE t.namespace='library' AND t.key=u.uuid AND t.removed_at IS NULL)
          ORDER BY u.uuid
          ON CONFLICT(namespace,key) DO UPDATE SET removed_at=NULL"
        ),
        [],
    )?;
    let scope = if touched {
        " AND asset_id IN (SELECT id FROM term_assets WHERE namespace='library' AND key IN (SELECT uuid FROM touched))"
    } else {
        ""
    };
    // 两个 IN 列表会让 SQLite 逐一探查每个（词条，受影响图片）组合。
    // 改为按 asset_id 驱动删除，按主键判断角色归属。
    let owned = if touched {
        "EXISTS (SELECT 1 FROM term_tree r WHERE r.term_id=term_projection.term_id)"
    } else {
        "term_id IN (SELECT term_id FROM term_tree)"
    };
    c.execute(&format!("DELETE FROM term_projection WHERE {owned}{scope}"), [])?;
    let scope = if touched { " AND m.uuid IN (SELECT uuid FROM touched)" } else { "" };
    c.execute(
        &format!(
            "INSERT INTO term_projection(asset_id,term_id,state,score,sources)
      SELECT a.id,t.id,m.status,m.score,m.sources FROM (
        SELECT uuid,entity,status,score,sources FROM memberships
        UNION ALL SELECT uuid,entity,status,score,sources FROM tag_memberships
        UNION ALL SELECT v.uuid,v.entity,v.status,m.score,m.sources FROM variant_memberships v
          JOIN memberships m ON m.uuid=v.uuid AND m.entity=v.parent_id
      ) m JOIN terms t ON t.key=m.entity JOIN term_tree r ON r.term_id=t.id
      JOIN term_assets a ON a.namespace='library' AND a.key=m.uuid
      WHERE a.removed_at IS NULL{scope}"
        ),
        [],
    )?;
    Ok(())
}

pub fn rebuild(c: &Connection, uuids: Option<&[String]>) -> Result<()> {
    evidence(c, uuids)?;
    single(c, uuids)?;
    memberships(c, uuids)
}

/// 刷新旧规则允许留下延迟派生的图片，供新建与资料撤销共用。
pub(crate) fn refresh_delayed(c: &Connection) -> Result<()> {
    // 旧版允许学习微小分差及身份拆分留下延迟派生，新建会把它们刷新。
    // 这些差异只可能落在学习 / 身份声明涉及的图片，保留刷新语义，不扫无关图片。
    let uuids = c
        .prepare_cached(
            "SELECT uuid FROM proto_claims
             UNION SELECT uuid FROM identity_scopes UNION SELECT uuid FROM identity_suspended",
        )?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !uuids.is_empty() {
        evidence(c, Some(&uuids))?;
        memberships(c, Some(&uuids))?;
    }
    Ok(())
}

/// 在调用方现有的事务里刷新副本组的代表和折叠状态。
pub fn fold(c: &Connection) -> Result<()> {
    c.execute(FOLD, [])?;
    Ok(())
}

/// 在同一事务里刷新临时表 `touched` 中图片所属的全部副本组。
pub fn fold_touched(c: &Connection) -> Result<()> {
    let scope = "WHERE d.grp IN (SELECT grp FROM duplicates WHERE uuid IN (SELECT uuid FROM touched))";
    c.execute(&scoped(FOLD, "/* group scope */", scope)?, [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_keeps_manual_decisions_visible_and_prefers_the_larger_copy_on_a_tie() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE assets(uuid TEXT PRIMARY KEY,width INTEGER,height INTEGER);
             CREATE TABLE assertions(uuid TEXT,entity TEXT,status TEXT,origin TEXT,PRIMARY KEY(uuid,entity));
             CREATE TABLE memberships(uuid TEXT,entity TEXT);
             CREATE TABLE excluded(uuid TEXT PRIMARY KEY);
             CREATE TABLE duplicates(uuid TEXT PRIMARY KEY,grp TEXT,pos INTEGER,rep INTEGER,hidden INTEGER);
             INSERT INTO duplicates VALUES
                ('same-small','same',1,0,0),('same-large','same',2,1,0),
                ('machine-small','machine',1,0,0),('machine-large','machine',2,1,0),
                ('human-small','human',1,0,0),('human-large','human',2,1,0),
                ('conflict-small','conflict',1,0,0),('conflict-large','conflict',2,1,1),
                ('split-small','split',1,0,0),('split-large','split',2,1,1),
                ('album-small','album',1,0,0),('album-large','album',2,1,0);
             INSERT INTO assets SELECT uuid,10*pos,10*pos FROM duplicates;
             INSERT INTO memberships VALUES ('same-small','A'),('same-large','A'),
                ('machine-small','A'),('machine-large','B');
             INSERT INTO assertions VALUES
                ('same-small','A','confirmed','review'),('same-large','A','confirmed','review'),
                ('album-small','A','confirmed','album'),
                ('human-small','A','confirmed','review'),('human-small','B','rejected','review'),
                ('conflict-small','A','confirmed','review'),('conflict-large','A','rejected','review'),
                ('split-small','A','confirmed','review'),('split-large','B','confirmed','review');",
        )
        .unwrap();
        fold(&c).unwrap();
        let rows: Vec<(String, bool, bool)> = c
            .prepare("SELECT uuid,rep,hidden FROM duplicates ORDER BY grp,pos")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            rows,
            [
                ("album-small", true, false),
                ("album-large", false, true),
                ("conflict-small", false, false),
                ("conflict-large", true, false),
                ("human-small", true, false),
                ("human-large", false, true),
                ("machine-small", false, true),
                ("machine-large", true, false),
                ("same-small", false, true),
                ("same-large", true, false),
                ("split-small", false, false),
                ("split-large", true, false),
            ]
            .map(|(id, rep, hidden)| (id.to_owned(), rep, hidden)),
        );
        let preserved: i64 = c
            .query_row("SELECT count(*) FROM assertions WHERE uuid='human-small' AND origin='review'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(preserved, 2);
    }

    #[test]
    fn every_scope_anchor_is_replaced_exactly_once() {
        for sql in [DEFAULT, HYBRID] {
            let scoped = scoped(sql, EVIDENCE_SCOPE.0, EVIDENCE_SCOPE.1).unwrap();
            assert_eq!(scoped.matches("IN (SELECT uuid FROM touched)").count(), 1);
            // 与原先按 18 个空格替换的结果只差空白。
            let before = sql.replace(
                "                  GROUP BY",
                "WHERE v.uuid IN (SELECT uuid FROM touched)\n                  GROUP BY",
            );
            assert!(scoped.split_whitespace().eq(before.split_whitespace()));
        }
        for (sql, (anchor, with)) in [(SINGLE, SINGLE_SCOPE), (LEAD, LEAD_SCOPE)] {
            let scoped = scoped(sql, anchor, with).unwrap();
            assert_eq!(scoped.matches("g.uuid IN (SELECT uuid FROM touched)").count(), 1);
        }
        assert!(split(MEMBERS, MEMBERS_SELECT).unwrap().1.starts_with("m.uuid,m.entity,"));
        assert!(split(MEMBERS, MEMBERS_FILTER).is_ok());
        for sql in [MEMBERS, TAGS] {
            assert!(split(sql, MEMBERS_FROM).is_ok());
        }
    }

    #[test]
    fn candidate_scope_anchor_appears_once_under_both_policies() {
        for select in [CANDIDATES_RESOLVED, CANDIDATES] {
            let scoped = scoped(select, CANDIDATE_SCOPE.0, CANDIDATE_SCOPE.1).unwrap();
            assert_eq!(scoped.matches("FROM candidate_scope scoped").count(), 1);
        }
    }

    #[test]
    fn missing_or_repeated_anchor_is_an_error() {
        let reflowed = DEFAULT.replace("GROUP BY", "GROUP\n BY");
        assert!(matches!(scoped(&reflowed, EVIDENCE_SCOPE.0, EVIDENCE_SCOPE.1), Err(Error::Schema(_))));
        assert!(matches!(scoped("a GROUP BY b GROUP BY c", "GROUP BY", "x"), Err(Error::Schema(_))));
        assert!(matches!(split("a b", "c"), Err(Error::Schema(_))));
        assert_eq!(scoped("a GROUP BY b", "GROUP BY", "x GROUP BY").unwrap(), "a x GROUP BY b");
    }
}
