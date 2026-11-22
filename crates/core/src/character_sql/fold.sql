-- 只在一份副本涵盖全组人工结论，并且所有副本的排除状态相同时折叠。
WITH copies AS MATERIALIZED (
 SELECT d.grp,d.uuid,d.pos,coalesce(a.width,0)*coalesce(a.height,0) AS pixels,
        EXISTS(SELECT 1 FROM excluded e WHERE e.uuid=d.uuid) AS is_excluded
 FROM duplicates d LEFT JOIN assets a ON a.uuid=d.uuid
 /* group scope */
), judgments AS MATERIALIZED (
 SELECT x.grp,x.uuid,s.entity,s.status
 FROM copies x JOIN assertions s ON s.uuid=x.uuid
), totals AS (
 SELECT grp,count(*) AS n FROM (SELECT DISTINCT grp,entity,status FROM judgments) GROUP BY grp
), per_copy AS (
 SELECT uuid,count(*) AS n FROM judgments GROUP BY uuid
), visibility AS (
 SELECT grp,min(is_excluded)=max(is_excluded) AS same_pool FROM copies GROUP BY grp
), ranked AS (
 SELECT x.grp,x.uuid,coalesce(p.n,0)=coalesce(t.n,0) AND v.same_pool AS foldable,
        row_number() OVER (PARTITION BY x.grp
          ORDER BY coalesce(p.n,0) DESC,x.pixels DESC,x.pos,x.uuid) AS rank
 FROM copies x JOIN visibility v USING(grp) LEFT JOIN totals t USING(grp) LEFT JOIN per_copy p USING(uuid)
)
UPDATE duplicates AS d SET rep=(d.uuid=r.uuid), hidden=(r.foldable AND d.uuid<>r.uuid)
FROM ranked r
WHERE r.rank=1 AND d.grp=r.grp
  AND (d.rep IS NOT (d.uuid=r.uuid) OR d.hidden IS NOT (r.foldable AND d.uuid<>r.uuid))
