-- 只在一份副本涵盖该组全部人工结论时折叠。组内有矛盾或人工结论分散时，没有这样的副本。
WITH copies AS MATERIALIZED (
 SELECT d.grp,d.uuid,d.pos,coalesce(a.width,0)*coalesce(a.height,0) AS pixels
 FROM duplicates d LEFT JOIN assets a ON a.uuid=d.uuid
 /* group scope */
), judgments AS MATERIALIZED (
 SELECT x.grp,x.uuid,s.entity,s.status
 FROM copies x JOIN assertions s ON s.uuid=x.uuid AND s.origin<>'album'
), totals AS (
 SELECT grp,count(*) AS n FROM (SELECT DISTINCT grp,entity,status FROM judgments) GROUP BY grp
), per_copy AS (
 SELECT uuid,count(*) AS n FROM judgments GROUP BY uuid
), ranked AS (
 SELECT x.grp,x.uuid,coalesce(p.n,0)=coalesce(t.n,0) AS complete,
        row_number() OVER (PARTITION BY x.grp
          ORDER BY coalesce(p.n,0) DESC,x.pixels DESC,x.pos,x.uuid) AS rank
 FROM copies x LEFT JOIN totals t USING(grp) LEFT JOIN per_copy p USING(uuid)
)
UPDATE duplicates AS d SET rep=(d.uuid=r.uuid), hidden=(r.complete AND d.uuid<>r.uuid)
FROM ranked r
WHERE r.rank=1 AND d.grp=r.grp
  AND (d.rep IS NOT (d.uuid=r.uuid) OR d.hidden IS NOT (r.complete AND d.uuid<>r.uuid))
