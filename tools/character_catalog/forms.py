"""从 Bangumi 对应生成身份关系的人工核对清单，并写入已核对的形态。"""
import collections
import itertools

from tools.character_catalog import reviewed

# https://github.com/bangumi/common/blob/master/person_relations.yml
BANGUMI_FORM = 2001


def add(rows, columns, matcher, archive_relations, review):
    """同一编号与归档形态可提出候选，关系本身须经独立审核。"""
    id_col, tag_col, parent_col, relation_col, action_col, bangumi_col = (
        columns.index(key) for key in ('id', 'tag', 'parent', 'relation', 'action', 'bangumi')
    )
    by_id = {row[id_col]: row for row in rows}
    by_tag = {row[tag_col]: row for row in rows}
    by_bangumi = collections.defaultdict(list)
    for row in rows:
        if row[bangumi_col] is not None:
            by_bangumi[row[bangumi_col]].append(row[tag_col])
    candidates = {tuple(sorted((a, b))): '同 Bangumi 角色' for tags in by_bangumi.values()
                  for a, b in itertools.combinations(tags, 2)}
    for left, kind, right in archive_relations:
        if kind == BANGUMI_FORM and left != right:
            for a, b in itertools.product(by_bangumi.get(left, ()), by_bangumi.get(right, ())):
                candidates[tuple(sorted((a, b)))] = 'Bangumi 形态'

    for (a, b), reason in sorted(candidates.items()):
        a_works, b_works = matcher.qualified_works(a), matcher.qualified_works(b)
        if a_works and b_works and not a_works & b_works:
            continue
        if frozenset((a, b)) in reviewed.DISTINCT:
            continue
        child, parent = (b, a) if reviewed.FORMS.get(b) == a else (a, b)
        child_row, parent_row = by_tag[child], by_tag[parent]
        parent_id = parent_row[id_col]
        if child_row[parent_col] not in (None, parent_id) or child_row[action_col] == 'auto':
            continue
        if child_row[relation_col] == 'form' or parent_row[action_col] == 'auto':
            continue
        if reviewed.FORMS.get(child) != parent:
            review.add((reason, a, b))
            continue
        ancestor = parent_row
        seen = {child_row[id_col]}
        while ancestor is not None and ancestor[id_col] not in seen:
            seen.add(ancestor[id_col])
            ancestor = by_id.get(ancestor[parent_col])
        if ancestor is not None:
            review.add(('形态关系成环', child, parent))
            continue
        child_row[parent_col], child_row[relation_col], child_row[action_col] = parent_id, 'form', 'ask'
        review.add(('已核对形态', child, parent))
