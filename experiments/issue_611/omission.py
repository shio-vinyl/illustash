"""#611：估难负例的漏标率下限。

从 sample_hard.py 的难负例里按类固定抽样（外观相近 300、其他单人 150、多人 150），用 Danbooru 公开 API
（每秒最多一次，#380 coverage.api 的节流和 429 退避）查这些帖子现在的角色标签，
和 2026-07-13 快照对比：现在带上名单里候选标签（冻结名单 16,701 个，另报含变体的 25,824 个）的，
就是快照时漏标、后来补上的。这是下限：至今仍没人补的漏标数不到。只读元数据，不取图。
"""

import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path

from roster import ROOT, canonical, read, write

SEED = 'illustash-611-omission'
SAMPLE = {'visual': 300, 'other': 150, 'multi': 150}


def main():
    main_checkout = Path(os.environ.get('ISSUE_611_MAIN') or ROOT.parents[2])
    worktrees = main_checkout / '.worktrees'
    import sys
    sys.path.insert(0, str(worktrees / 'issue-551-wd-b-expansion/experiments/issue_380'))
    from coverage import api
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--coverage', type=Path, default=worktrees / 'issue-380-character-coverage/_scratch/issue-380/coverage')
    parser.add_argument('--out', type=Path, default=Path(os.environ.get('ISSUE_611_OUTPUT', ROOT / '_scratch/issue-611')))
    args = parser.parse_args()
    aliases = read(args.coverage / 'alias-map.json')
    frozen = {r['label'] for r in read(args.out / 'roster/roster-v2.json')}
    with_variants = {r['label'] for r in read(args.out / 'roster/roster.json')}
    rows = read(args.out / 'hard/requests.json')
    order = lambda r: hashlib.sha256(f"{SEED}:{r['id']}".encode()).digest()
    picked = []
    for kind, n in SAMPLE.items():
        pool = sorted((r for r in rows if any(x.startswith(f'hard_{kind}:') for x in r['roles'])), key=order)
        picked += [dict(r, kind=kind) for r in pool[:n]]
    cache = args.out / 'omission/api'
    cache.mkdir(parents=True, exist_ok=True)
    current = {}
    for begin in range(0, len(picked), 100):
        ids = [r['id'] for r in picked[begin:begin + 100]]
        batch = api(cache / f'posts-{begin}.json', '/posts.json', {'tags': 'id:' + ','.join(map(str, ids)), 'limit': 100})
        for post in batch:
            current[post['id']] = post
    counts = Counter()
    examples = []
    for r in picked:
        post = current.get(r['id'])
        if post is None:
            counts[f"{r['kind']}:not_visible_now"] += 1
            continue
        now = {canonical(c, aliases) for c in post.get('tag_string_character', '').split()}
        counts[f"{r['kind']}:checked"] += 1
        if now & frozen:
            counts[f"{r['kind']}:gained_frozen_candidate"] += 1
            examples.append({'id': r['id'], 'kind': r['kind']})
        if now & with_variants:
            counts[f"{r['kind']}:gained_any_candidate"] += 1
        if now != set(r['characters']):
            counts[f"{r['kind']}:character_tags_changed"] += 1
    result = {'sample': SAMPLE, 'counts': dict(sorted(counts.items())), 'gained_post_ids': examples}
    write(args.out / 'omission/api-check.json', result)
    print(json.dumps(result, ensure_ascii=False, indent=1))


if __name__ == '__main__':
    main()
