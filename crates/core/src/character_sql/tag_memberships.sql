INSERT INTO tag_memberships SELECT m.uuid,m.entity,m.status,m.origin,m.sources,m.score,m.source_count,m.ccip_only,
            printf('%d%d%d%020.12f%d',m.status='confirmed',m.source_count<>1,1-m.ccip_only,coalesce(m.score,1),
            NOT EXISTS(SELECT 1 FROM family_confirmed f WHERE f.uuid=m.uuid AND f.entity<>m.entity))
            FROM membership_rules m WHERE  EXISTS(SELECT 1 FROM terms t JOIN term_tree r ON r.term_id=t.id WHERE t.key=m.entity AND (r.mascot=1 OR (r.label_only=1 AND r.original=0)))
