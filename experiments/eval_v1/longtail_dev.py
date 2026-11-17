"""#557 第二轮的开发取样：长尾角色的参考图和校准图（只按元数据，和评测集严格隔离）。

口径沿用 #557 第一轮：
- 角色：评测集冻结时实时复核为「不足 50 帖、首帖在 WD 截止后、WD/精选索引/扩展头都没覆盖」，
  且不是 v1 已有的 54 个锚点；
- 参考图：#557 train 分区画师的单人单角色全年龄帖，可早于 START 但须晚于 WD 截止，每角色最多 12 位画师；
- 校准图：calibration 分区画师、START 以后的帖，每角色最多 4 位；
- 至少 5 位参考画师、1 位校准画师才取。评测正例只在 test 分区，三区互不相交；
- 与历史取样和评测集的画师、帖子、家族、MD5、像素摘要都不重叠。
"""

from collections import Counter, defaultdict
import json

from common import COVERAGE, DATA, START, STOP, WD_LAST_ID, load, priority, sha_file, split_557, write
from coverage import canonical
import sampling as upstream
from sample import family_roots

QUOTA = {'train': 12, 'calibration': 4}
MINIMUM = {'train': 5, 'calibration': 1}


def eligible_roles(targets):
    roles = []
    for name, posts in load(DATA / 'pool/live-tail-posts.json').items():
        if targets.get(name, {}).get('route') == 'ccip_longtail':
            continue
        public = [p for p in posts if not p['is_deleted'] and not p['is_banned']]
        if len(posts) >= 100 or len(public) >= 50 or not public or min(p['id'] for p in public) <= WD_LAST_ID:
            continue
        roles.append((name, len(public)))
    return roles


def main():
    if (DATA / 'lt2/requests.json').exists():
        raise ValueError('#557 第二轮开发取样已冻结')
    char_aliases = load(COVERAGE / 'alias-map.json')
    artist_aliases = load(DATA / 'artist-aliases.json')
    targets = {e['label']: e for e in load(DATA / 'roster/targets.json')}
    prior = load(DATA / 'prior/boundary.json')
    evaluation = load(DATA / 'requests.json')
    upstream.MIN_DATE, upstream.MAX_LIVE_DATE = '2024-02-28', STOP
    roles = eligible_roles(targets)
    rows_by_role, ancestry = {}, []
    for name, _ in roles:
        records = load(DATA / 'pool/live' / f'tail-{priority(name):016x}.json')
        rows = []
        for record in records:
            row = upstream.post(record, char_aliases, allow_multitag_solo=True)
            if not row or name not in row['characters'] or row['id'] <= WD_LAST_ID:
                continue
            row['artist'] = canonical(row['artist'], artist_aliases)
            row['split'] = split_557(row['artist'])
            if row['split'] != 'train' and row['created_at'][:10] < START:
                continue
            rows.append(row)
        rows_by_role[name] = rows
        ancestry.extend(rows)
    parents = {int(k): v for k, v in prior['parents'].items()}
    roots = family_roots([*ancestry, *evaluation], parents)
    banned = {('artist', a) for a in prior['artists']} | {('id', p) for p in prior['ids']}
    banned |= {('family', roots.get(p, p)) for p in prior['ids']} | {('md5', m) for m in prior['md5']}
    banned |= {('pixel_hash', p) for p in prior['pixel_hashes']}
    for row in evaluation:
        banned |= {('artist', row['artist']), ('id', row['id']), ('family', roots.get(row['id'], row['id']))}
        banned |= {(k, row[k]) for k in ('md5', 'pixel_hash') if row.get(k)}

    def keys(row):
        result = {('artist', row['artist']), ('id', row['id']), ('family', roots.get(row['id'], row['id']))}
        return result | {(k, row[k]) for k in ('md5', 'pixel_hash') if row.get(k)}

    selected, states, counts = {}, Counter(), {}
    for name, total in sorted(roles, key=lambda r: (r[1], r[0])):
        chosen = {}
        for split, quota in QUOTA.items():
            pool = [r for r in rows_by_role[name] if r['split'] == split and not keys(r) & banned
                    and (split != 'train' or (r['solo'] and r['characters'] == [name]))]
            pool.sort(key=lambda r: (not r['solo'], len(r['characters']), priority(r['id'])))
            artists, picked = set(), []
            for row in pool:
                if row['artist'] in artists or len(picked) >= quota:
                    continue
                artists.add(row['artist'])
                picked.append(row)
            chosen[split] = picked
        counts[name] = {split: len(items) for split, items in chosen.items()}
        if any(counts[name][s] < m for s, m in MINIMUM.items()):
            states['insufficient_samples'] += 1
            continue
        states['sampled'] += 1
        for split, items in chosen.items():
            for row in items:
                entry = selected.setdefault(row['id'], dict(row, group='target', focus_roles=[]))
                entry['focus_roles'].append(name)
    rows = sorted(selected.values(), key=lambda r: r['id'])
    if any(r['split'] == 'test' for r in rows):
        raise ValueError('开发取样混入 test 分区')
    write('lt2/requests.json', rows)
    receipt = {'roles': len(roles), 'states': states, 'images': len(rows),
               'splits': Counter(r['split'] for r in rows), 'quota': QUOTA, 'minimum': MINIMUM,
               'requests_sha256': sha_file(DATA / 'lt2/requests.json'),
               'eval_requests_sha256': sha_file(DATA / 'requests.json'),
               'roles_sampled': sorted(n for n, c in counts.items()
                                       if all(c[s] >= m for s, m in MINIMUM.items()))}
    write('lt2/sample-receipt.json', receipt)
    write('lt2/sample-counts.json', counts)
    print(json.dumps({k: v for k, v in receipt.items() if k != 'roles_sampled'}, ensure_ascii=False), flush=True)


if __name__ == '__main__':
    main()
