"""#611：冻结「WD 之前就有、WD 词表和完整精选索引都没覆盖」的老长尾角色候选名单。

口径和 #551 的 freeze_roster.py 相同，只把首帖条件反过来：
- 2026-11-16 的 Danbooru 角色标签普查（#380），合并活跃别名、排除弃用标签，现行帖子数 ≥50；
- 规范化以后不在 WD v3 词表（category 4）里，也不在 #534 实际装载的完整精选索引（7,367 锚点）里；
- 首个可见帖子 ID ≤ 7,220,105（WD 截止）。首帖证据先查 #380 的固定公开帖快照，
  快照里没有的再用 #380 当时逐个查过的实时 API 结果。

另外核对和已发、待发各包的目标（共用评测集 v1 的 2,100 + 205 头、长尾锚点）互不重叠。
只读主检出 .worktrees 下的冻结数据，产物写到本工作树 _scratch/issue-611（或 ISSUE_611_OUTPUT）。
"""

import argparse
from collections import Counter
import csv
import hashlib
import json
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CUTOFF_POST = 7_220_105
MIN_POSTS = 50


def read(path):
    return json.loads(Path(path).read_bytes())


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def canonical(label, aliases):
    visited = set()
    while label in aliases:
        if label in visited:
            raise ValueError(f'角色别名出现循环：{label}')
        visited.add(label)
        label = aliases[label]
    return label


def band(posts):
    if posts < 100:
        return '50-99'
    if posts < 500:
        return '100-499'
    if posts < 1000:
        return '500-999'
    return '1000+'


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + '.partial')
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=1, sort_keys=True) + '\n', encoding='utf-8')
    temporary.replace(path)
    return path


def main():
    main_checkout = Path(os.environ.get('ISSUE_611_MAIN') or ROOT.parents[2])
    worktrees = main_checkout / '.worktrees'
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--coverage', type=Path, default=worktrees / 'issue-380-character-coverage/_scratch/issue-380/coverage')
    parser.add_argument('--index', type=Path, default=worktrees / 'issue-534-character-expansion/_scratch/issue-534/index')
    parser.add_argument('--wd-tags', type=Path, default=worktrees / 'issue-551-wd-b-expansion/tags/tags_v3.csv')
    parser.add_argument('--eval-targets', type=Path, required=True,
                        help='共用评测集 v1 的 roster/targets.json（2,100 + 205 头和长尾锚点）')
    parser.add_argument('--out', type=Path, default=Path(os.environ.get('ISSUE_611_OUTPUT', ROOT / '_scratch/issue-611')))
    args = parser.parse_args()
    inputs = {
        'tag_inventory': args.coverage / 'inventory.json',
        'aliases': args.coverage / 'alias-map.json',
        'first_in_snapshot': args.coverage / 'first-in-snapshot.json',
        'first_in_live_tail': args.coverage / 'live-tail.json',
        'snapshot_report': args.coverage / 'snapshot-summary.json',
        'wd_tags': args.wd_tags,
        'index_characters': args.index / 'loaded-characters.json',
        'index_report': args.index / 'index-report.json',
        'eval_targets': args.eval_targets,
    }
    aliases = read(inputs['aliases'])
    snapshot = read(inputs['first_in_snapshot'])
    live_tail = {r['name']: r['first_public_post'] for r in read(inputs['first_in_live_tail'])}
    index_report = read(inputs['index_report'])
    index_rows = read(inputs['index_characters'])
    if index_report['expected'] != index_report['attempted'] or index_report['loaded_anchors'] != len(index_rows):
        raise ValueError('只使用完整索引')
    indexed = {canonical(r['tag'], aliases) for r in index_rows}
    with args.wd_tags.open(encoding='utf-8', newline='') as stream:
        wd = {canonical(row['name'], aliases) for row in csv.DictReader(stream) if row['category'] == '4'}
    published = {canonical(r['label'], aliases): r['pack'] for r in read(inputs['eval_targets'])}

    counts = Counter()
    selected = []
    inventory = read(inputs['tag_inventory'])
    if len({r['name'] for r in inventory}) != len(inventory):
        raise ValueError('角色清单的规范标签重复')
    for row in inventory:
        label = row['name']
        if row['post_count'] < MIN_POSTS or canonical(label, aliases) != label or row.get('is_deprecated', False):
            counts['not_eligible_tag'] += 1
            continue
        if label in wd:
            counts['already_wd'] += 1
            continue
        if cached := snapshot.get(label):
            first_id, date, _ = cached
            source = 'post_snapshot'
        elif current := live_tail.get(label):
            first_id, date = current['id'], current['created_at']
            source = 'danbooru_live_tail'
        else:
            counts['unknown_first_post'] += 1
            continue
        if first_id > CUTOFF_POST:
            counts['first_post_after_wd'] += 1
            continue
        counts['first_post_at_or_before_wd'] += 1
        if label in indexed:
            counts['already_in_loaded_index'] += 1
            continue
        if label in published:
            counts[f'already_in_pack_{published[label]}'] += 1
            continue
        selected.append(dict(label=label, tag_id=row['id'], post_count=row['post_count'], band=band(row['post_count']),
                             first_post_id=first_id, first_post_source=source, first_post_date=str(date)[:10]))
    selected.sort(key=lambda r: r['label'])
    roster = write(args.out / 'roster/roster.json', selected)
    bands = Counter(r['band'] for r in selected)
    first_year = Counter(r['first_post_date'][:4] for r in selected)
    receipt = dict(
        rule=dict(min_posts=MIN_POSTS, first_post_id_at_most=CUTOFF_POST, not_in='WD v3 词表 ∪ 完整精选索引 ∪ 已发各包目标'),
        inventory_date='2026-11-16', posts_snapshot=read(inputs['snapshot_report'])['snapshot'],
        index_revision=index_report['revision'], index_anchor_sha256=index_report['anchor_file_sha256'],
        loaded_index_anchors=len(index_rows), canonical_index_labels=len(indexed), canonical_wd_labels=len(wd),
        inputs_sha256={key: digest(path) for key, path in inputs.items()}, decisions=dict(counts),
        selected=len(selected), source_counts=Counter(r['first_post_source'] for r in selected),
        post_count_bands={k: bands[k] for k in ('50-99', '100-499', '500-999', '1000+')},
        first_post_year=dict(sorted(first_year.items())), roster_sha256=digest(roster))
    write(args.out / 'roster/receipt.json', receipt)
    print(json.dumps(receipt, ensure_ascii=False, indent=1))


if __name__ == '__main__':
    main()
