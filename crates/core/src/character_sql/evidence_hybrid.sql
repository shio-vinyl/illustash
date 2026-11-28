INSERT INTO family_evidence
            SELECT uuid,entity,sources,score,source_count,ccip_only,wd,ccip,
                   row_number() OVER (PARTITION BY uuid ORDER BY coalesce((SELECT r.mascot=1 OR (r.label_only=1 AND r.original=0) FROM terms x JOIN term_tree r ON r.term_id=x.id WHERE x.key=entity),0),
                   wd IS NOT NULL AND ccip IS NOT NULL DESC,
                   coalesce(wd,0)>=0.9 OR coalesce(ccip,0)>=0.95 DESC,
                   wd IS NOT NULL DESC,coalesce(wd,0) DESC,coalesce(ccip,0) DESC,entity)
            FROM (SELECT v.uuid,e.character_id AS entity,group_concat(DISTINCT v.source ORDER BY v.source) AS sources,
                   max(v.score) AS score,count(DISTINCT v.source) AS source_count,min(v.source<>'WD') AS ccip_only,
                   max(CASE WHEN v.source='WD' THEN v.score END) AS wd,max(CASE WHEN v.source<>'WD' THEN v.score END) AS ccip
                  FROM (SELECT uuid,entity,source,score FROM candidates WHERE source NOT IN ('WD_EXT','CCIP_LONGTAIL') UNION ALL SELECT uuid,entity,'PROTO',score FROM proto_claims) v JOIN identity_nodes e ON e.id=v.entity JOIN assets a ON a.uuid=v.uuid
                  
                  GROUP BY e.character_id,v.uuid)
