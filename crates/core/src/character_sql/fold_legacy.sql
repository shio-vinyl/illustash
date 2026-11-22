UPDATE duplicates AS d SET hidden=(rep=0 AND NOT EXISTS(SELECT 1 FROM duplicates y WHERE y.grp=d.grp AND y.uuid<>d.uuid AND (
 EXISTS(SELECT entity FROM memberships WHERE uuid=d.uuid EXCEPT SELECT entity FROM memberships WHERE uuid=y.uuid) OR
 EXISTS(SELECT entity FROM memberships WHERE uuid=y.uuid EXCEPT SELECT entity FROM memberships WHERE uuid=d.uuid))))
