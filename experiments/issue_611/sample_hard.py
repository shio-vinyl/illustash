"""#611 一期扩充：同作品撞脸的难负例（2026-11-18 用户同意追加），只当开发数据。

对一期能训的候选所属的每部作品（候选最常见的作品标签），取这部作品里已覆盖老角色的图：
- 老角色 = WD 词表 ∪ 完整精选索引 ∪ 一级 257 头 ∪ 长尾 v1 ∪ 0.3、0.4 资料包的全部头。
- 外观相近（visual）：老角色单人图和本作品某个目标的外观画像至少共享 2 个外观标签、其中一个是发色或瞳色。
  画像和标签集照共用评测集 v1（#534 sampling.closest_visual，标签集加上 v1 pool.py 的服装、形态标签）：
  目标单人图里出现率 ≥38%（且至少 2 次）的外观标签，至少 2 个才算有画像。
- 其他同作品老角色单人图（other）、含同作品老角色的多人图（multi）。
配额在一期 min(80, 6+3n) 的基础上放大 3 倍：visual min(240, 18+9n)、other min(120, 9+3n)、
multi min(48, 6+3n/2)，n 是这部作品里能训的候选数；同一老角色 visual 最多 8 张、other 最多 4 张，
同一画师在一部作品里最多 2 张（用过的画师可以重复出现）。

不碰：2026-11-17 以后的帖子（快照截至 2026-07-13）、共用评测集 v1 的画师和帖子、验收区（illustash-611-v1
散列的 20%）的画师、一期已经请求过的帖子，以及带名单里任何候选标签的图（那是正例，不是负例）。
"""

import argparse
from collections import Counter, defaultdict
import csv
import hashlib
import json
import os
from pathlib import Path
import re

import duckdb

from roster import ROOT, canonical, digest, read, write
from supply import partition
from sample_dev import STATIC

SEED = 'illustash-611-hard'
COLOR = {'black', 'blonde', 'blue', 'brown', 'green', 'grey', 'orange', 'pink', 'purple', 'red', 'silver', 'white',
         'yellow', 'aqua'}
VISUAL = {*(f'{c}_{f}' for c in COLOR for f in ('hair', 'eyes')), 'short_hair', 'medium_hair', 'long_hair',
          'very_long_hair', 'twintails', 'side_ponytail', 'ponytail', 'single_braid', 'twin_braids', 'hair_bun',
          'animal_ears', 'cat_ears', 'fox_ears', 'rabbit_ears', 'horns', 'glasses', 'eyepatch', 'heterochromia',
          'multicolored_hair', 'dress', 'school_uniform', 'military_uniform', 'armor', 'maid', 'suit', 'bodysuit',
          'swimsuit', 'robe', 'hood', 'hat', 'cape', 'coat', 'jacket', 'wings', 'tail', 'mechanical_parts', 'mask',
          'helmet', 'facial_mark', 'robot', 'monster', 'chibi'}


def priority(key):
    return int.from_bytes(hashlib.sha256(f'{SEED}:{key}'.encode()).digest()[:8], 'big')


def quota(n):
    return {'visual': min(240, 18 + 9 * n), 'other': min(120, 9 + 3 * n), 'multi': min(48, 6 + (3 * n) // 2)}


def main():
    main_checkout = Path(os.environ.get('ISSUE_611_MAIN') or ROOT.parents[2])
    worktrees = main_checkout / '.worktrees'
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--coverage', type=Path, default=worktrees / 'issue-380-character-coverage/_scratch/issue-380/coverage')
    parser.add_argument('--parquet', type=Path, default=worktrees / 'issue-380-character-coverage/_scratch/issue-380/posts.parquet')
    parser.add_argument('--eval-v1', type=Path, required=True)
    parser.add_argument('--out', type=Path, default=Path(os.environ.get('ISSUE_611_OUTPUT', ROOT / '_scratch/issue-611')))
    args = parser.parse_args()
    aliases = read(args.coverage / 'alias-map.json')
    artist_aliases = read(args.eval_v1 / 'artist-aliases.json')
    evaluation = read(args.eval_v1 / 'eval/manifest.json')
    v1_artists = {canonical(r['artist'], artist_aliases) for r in evaluation}
    v1_ids = {r['id'] for r in evaluation}
    roster_all = {r['label'] for r in read(args.out / 'roster/roster.json')}      # 含变体，图上带这些标签就不当负例
    supply = {r['label']: r for r in read(args.out / 'supply/supply.json')}
    dev = read(args.out / 'dev/requests.json')
    requested = {r['id'] for r in dev}
    chosen = sorted({x.split(':', 1)[1] for r in dev for x in r['roles'] if x.startswith('train:')})
    works = Counter(supply[n]['work'] for n in chosen if supply[n]['work'] not in (None, 'original'))
    with (worktrees / 'issue-551-wd-b-expansion/tags/tags_v3.csv').open(encoding='utf-8', newline='') as stream:
        old = {canonical(r['name'], aliases) for r in csv.DictReader(stream) if r['category'] == '4'}
    old |= {canonical(r['tag'], aliases) for r in
            read(worktrees / 'issue-534-character-expansion/_scratch/issue-534/index/loaded-characters.json')}
    old |= {canonical(r['label'], aliases) for r in read(args.eval_v1 / 'roster/targets.json')
            if r['tier1_257'] or r['route'] == 'ccip_longtail'}
    for pack in ('character-pack-0.3/wd-extension-0.3.bin', 'character-pack-0.4/wd-extension-0.4.bin'):
        with (args.eval_v1 / 'deliver' / pack).open('rb') as stream:
            old |= {canonical(n, aliases) for n in json.loads(stream.readline())['labels']}
    old -= roster_all

    exts = ','.join(f"'{e}'" for e in STATIC)
    base_sql = f"""
        SELECT id, md5, tag_string_character, tag_string_artist, tag_string_general, tag_string_copyright,
               strftime(created_at AT TIME ZONE 'UTC', '%Y-%m-%d %H:%M:%S'), parent_id, media_asset.pixel_hash
        FROM read_parquet(?)
        WHERE rating = 'g' AND tag_count_artist = 1 AND tag_count_character BETWEEN 1 AND 7
          AND NOT coalesce(is_deleted, false) AND NOT coalesce(is_banned, false) AND lower(file_ext) IN ({exts})
          AND id IN (SELECT DISTINCT p.id FROM read_parquet(?) p,
                     unnest(string_split(p.tag_string_character, ' ')) AS u(c) JOIN names n ON n.raw = u.c)"""
    profiles = defaultdict(dict)
    pools = defaultdict(lambda: defaultdict(list))
    with duckdb.connect(config={'threads': '2', 'memory_limit': '3GB'}) as db:
        # 第一遍：目标的外观画像（只用元数据，全部合格单人帖）。
        target_raw = sorted(set(chosen) | {a for a in aliases if canonical(a, aliases) in set(chosen)})
        db.execute('CREATE TEMP TABLE names(raw VARCHAR PRIMARY KEY)')
        db.executemany('INSERT INTO names VALUES (?)', [[n] for n in target_raw])
        visual_counts, solo_counts = defaultdict(Counter), Counter()
        cursor = db.execute(base_sql, [str(args.parquet)] * 2)
        for batch in iter(lambda: cursor.fetchmany(50000), []):
            for post, md5, chars, artist, general, copyrights, *_ in batch:
                names = {canonical(c, aliases) for c in chars.split()}
                general = set(general.split())
                if len(names) == 1 and {'solo', '1girl', '1boy'} & general:
                    (name,) = names
                    if name in supply:
                        solo_counts[name] += 1
                        visual_counts[name].update(general & VISUAL)
        for name in chosen:
            work = supply[name]['work']
            n = solo_counts[name]
            profile = {t for t, k in visual_counts[name].items() if k >= max(2, n * 0.38)}
            if work in works and len(profile) >= 2:
                profiles[work][name] = profile
        print(f'画像：{sum(map(len, profiles.values())):,} 个目标', flush=True)

        # 第二遍：同作品老角色的图。
        db.execute('DROP TABLE names')
        db.execute('CREATE TEMP TABLE names(raw VARCHAR PRIMARY KEY)')
        old_raw = sorted(old | {a for a in aliases if canonical(a, aliases) in old})
        db.executemany('INSERT INTO names VALUES (?)', [[n] for n in old_raw])
        excluded = Counter()
        cursor = db.execute(base_sql, [str(args.parquet)] * 2)
        for batch in iter(lambda: cursor.fetchmany(50000), []):
            for post, md5, chars, artist, general, copyrights, created, parent, pixel in batch:
                if not md5 or not re.fullmatch(r'[0-9a-f]{32}', md5):
                    continue
                titles = set(copyrights.split()) & works.keys()
                if not titles:
                    continue
                artist = canonical(artist, artist_aliases)
                if post in v1_ids or artist in v1_artists:
                    excluded['v1'] += 1
                    continue
                if partition(artist) == 'accept':
                    excluded['accept'] += 1
                    continue
                if post in requested:
                    excluded['already_requested'] += 1
                    continue
                names = sorted({canonical(c, aliases) for c in chars.split()})
                if set(names) & roster_all:
                    excluded['has_candidate_tag'] += 1
                    continue
                olds = set(names) & old
                if not olds:
                    continue
                general = set(general.split())
                single = len(names) == 1 and bool({'solo', '1girl', '1boy'} & general)
                row = dict(id=post, created_at=created, artist=artist, characters=names, copyrights=copyrights.split(),
                           solo=single or 'solo' in general, md5=md5, parent_id=parent, pixel_hash=pixel,
                           url=f'https://cdn.donmai.us/720x720/{md5[:2]}/{md5[2:4]}/{md5}.webp', split=partition(artist))
                visual = general & VISUAL
                for title in titles:
                    if single:
                        evidence = {}
                        for target, profile in profiles.get(title, {}).items():
                            shared = profile & visual
                            if len(shared) >= 2 and any(t.endswith(('_hair', '_eyes')) for t in shared):
                                evidence[target] = len(shared)
                        if evidence:
                            near = min(evidence, key=lambda k: (-evidence[k], k))
                            pools[title]['visual'].append(dict(row, confuser_of=near))
                        else:
                            pools[title]['other'].append(row)
                    elif len(names) > 1:
                        pools[title]['multi'].append(row)
        print('老角色帖读完', excluded, flush=True)

    selected, roles = {}, defaultdict(set)
    report = Counter()
    for title, n in sorted(works.items()):
        limits = quota(n)
        for kind, per_character in (('visual', 8), ('other', 4), ('multi', 0)):
            rows = sorted(pools[title][kind], key=lambda r: priority(f"{title}:{kind}:{r['id']}"))
            count, per_char, per_artist = 0, Counter(), Counter()
            for r in rows:
                if count >= limits[kind]:
                    break
                key = r['characters'][0]
                if per_character and per_char[key] >= per_character:
                    continue
                if per_artist[r['artist']] >= 2:
                    continue
                per_char[key] += 1
                per_artist[r['artist']] += 1
                selected.setdefault(r['id'], r)
                roles[r['id']].add(f'hard_{kind}:{title}')
                count += 1
            report[kind] += count
            report[f'{kind}_quota'] += limits[kind]
    manifest = [dict(selected[p], roles=sorted(roles[p])) for p in sorted(selected, key=lambda p: priority(f'order:{p}'))]
    path = write(args.out / 'hard/requests.json', manifest)
    receipt = dict(seed=SEED, works=len(works), targets_with_profile=sum(map(len, profiles.values())),
                   old_characters=len(old), requests=len(manifest), selection=report,
                   by_split=Counter(r['split'] for r in manifest), latest=max(r['created_at'] for r in manifest),
                   inputs=dict(dev_requests=digest(args.out / 'dev/requests.json'),
                               roster=digest(args.out / 'roster/roster.json'), supply=digest(args.out / 'supply/supply.json')),
                   requests_sha256=digest(path))
    write(args.out / 'hard/receipt.json', receipt)
    print(json.dumps(receipt, ensure_ascii=False, indent=1))


if __name__ == '__main__':
    main()
