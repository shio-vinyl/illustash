"""#611 二期：按公开元数据冻结验收集的取图清单（规则、名单、阈值已于 2026-11-18 冻结，取样前不看分数）。

验收区 = 画师散列 illustash-611-v1 的 20%（supply.partition 为 accept）。2026-11-18 Lead 定的 A 方案：
- 来源：#380 的 2026-07-13 快照，加实时 API 补 07-13 到 11-15（帖子 ID 大于快照最大 ID 11,786,128），
  一律早于 2026-11-17 00:00 UTC；不碰留给 0.4 验收的新帖子。实时 API 只取元数据，每秒最多一次请求。
- 排除：历次研究的画师、帖子、父子家族、MD5、像素摘要（eval-v1 prior/boundary.json），v1 评测集的画师和帖子，
  本 Issue 一期和难负例请求过的帖子及其父子家族、MD5、像素摘要。内容指纹（pHash/dHash）要取图后才有，
  回传后再按 v1 的阈值去近重复。
- 每位画师只出 1 张，正例优先（稀缺），再旧角色图，再对照。
组成：
- 正例：冻结包 5,986 个头，每头单人单角色 1 张、多人 1 张，帖子 ID 晚于 WD 截止。
- 旧角色图：真值含 B1 已覆盖角色（v1 的 native-covered，7,652 个）的图，2025-09-26 以后，按画师自然抽取，
  上限 15,000 张；不刻意加浓同作品。
- 对照：无角色图 1,000 张、只含未覆盖角色的图 1,000 张（2025-09-26 以后）。
"""

import argparse
from datetime import datetime, timezone
from collections import Counter, defaultdict
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

import duckdb
import numpy as np

from roster import CUTOFF_POST, ROOT, canonical, digest, read, write
from supply import partition
from sample_dev import STATIC

SEED = 'illustash-611-accept'
SNAPSHOT_MAX_ID = 11_786_128
INDEX_FROZEN = '2025-09-26'
STOP = '2026-11-17'
OLD_LIMIT = 15000
CONTROLS = {'no_character': 1000, 'uncovered': 1000}


def priority(key):
    return int.from_bytes(hashlib.sha256(f'{SEED}:{key}'.encode()).digest()[:8], 'big')


def live_posts(api, cache):
    """实时 API：按 ID 区间分页取 07-13 以后、11-17 以前的全年龄帖。"""
    # API 的 date: 按美东时间算；上界取美东 11-15 的最后一帖（UTC 11-16 05:00 以前），
    # 这样连 UTC 11-17 以后的帖子元数据都不会取到。
    last = api(cache / 'last-before-stop.json', '/posts.json',
               {'tags': f'id:>{SNAPSHOT_MAX_ID} date:2026-11-15', 'limit': 1})
    if not last:
        raise ValueError('找不到 2026-11-15 的帖子')
    upper = last[0]['id']
    query = lambda low, high: f'rating:g id:{low}..{high}'

    def chain(name, low, high, start):
        """在 [low, high] 里按 ID 从大到小分页；name 区分缓存文件。"""
        out, cursor = [], start
        while True:
            for attempt in range(4):       # 偶发的服务端错误：等 10 秒再试，最多 3 次
                try:
                    batch = api(cache / f'{name}b{cursor}.json', '/posts.json',
                                {'tags': query(low, high), 'limit': 200, 'page': f'b{cursor}'})
                    break
                except subprocess.CalledProcessError:
                    if attempt == 3:
                        raise
                    time.sleep(10)
            if not batch:
                break
            out += batch
            cursor = min(p['id'] for p in batch)
            # Danbooru 会从页里滤掉隐藏的帖子，不满 200 条不代表到底了；取到空页或越过下界才停。
            if cursor <= low:
                break
        return out, cursor

    # 先按缓存回放第一轮单线分页取到的部分（不发请求），剩下的 ID 区间切成 4 段并行取；
    # coverage.api 全局限速每秒一次，并行只是不用干等响应。
    posts, cursor = [], upper + 1
    while (cache / f'page-b{cursor}.json').exists():
        batch = json.loads((cache / f'page-b{cursor}.json').read_bytes())
        if not batch:
            break
        posts += batch
        cursor = min(p['id'] for p in batch)
    low = SNAPSHOT_MAX_ID + 1
    edges = np.linspace(low, cursor, 5).astype(int)
    segments = [(int(edges[k]) + (k > 0), int(edges[k + 1]) - (k == 3) * 0) for k in range(4)]
    from concurrent.futures import ThreadPoolExecutor
    with ThreadPoolExecutor(max_workers=4) as pool:
        jobs = [pool.submit(chain, f'seg{k}-{a}-{b_}-', a, b_, b_ + 1) for k, (a, b_) in enumerate(segments)]
        for job in jobs:
            posts += job.result()[0]
    seen, unique = set(), []
    for p in posts:
        if p['id'] not in seen and SNAPSHOT_MAX_ID < p['id'] <= upper:
            seen.add(p['id'])
            unique.append(p)
    return [dict(p, created_at=utc(p['created_at'])) for p in unique if utc(p['created_at'])[:10] < STOP], upper


def utc(stamp):
    return datetime.fromisoformat(stamp).astimezone(timezone.utc).strftime('%Y-%m-%d %H:%M:%S')


def main():
    main_checkout = Path(os.environ.get('ISSUE_611_MAIN') or ROOT.parents[2])
    worktrees = main_checkout / '.worktrees'
    sys.path.insert(0, str(worktrees / 'issue-551-wd-b-expansion/experiments/issue_380'))
    from coverage import api
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--coverage', type=Path, default=worktrees / 'issue-380-character-coverage/_scratch/issue-380/coverage')
    parser.add_argument('--parquet', type=Path, default=worktrees / 'issue-380-character-coverage/_scratch/issue-380/posts.parquet')
    parser.add_argument('--eval-v1', type=Path, required=True)
    parser.add_argument('--out', type=Path, default=Path(os.environ.get('ISSUE_611_OUTPUT', ROOT / '_scratch/issue-611')))
    args = parser.parse_args()
    aliases = read(args.coverage / 'alias-map.json')
    artist_aliases = read(args.eval_v1 / 'artist-aliases.json')
    boundary = read(args.eval_v1 / 'prior/boundary.json')
    evaluation = read(args.eval_v1 / 'eval/manifest.json')
    covered = set(read(args.eval_v1 / 'native-covered.json')['covered_labels'])
    heads = [h['label'] for h in read(args.out / 'freeze/heads.json') if h['kept']]
    head_set = set(heads)
    roster_all = {r['label'] for r in read(args.out / 'roster/roster.json')}

    banned_artists = {canonical(a, artist_aliases) for a in boundary['artists']}
    banned_artists |= {canonical(r['artist'], artist_aliases) for r in evaluation}
    banned_ids = set(boundary['ids']) | {r['id'] for r in evaluation}
    banned_ids |= {int(k) for k in boundary['parents']} | set(boundary['parents'].values())
    banned_md5, banned_pixels = set(boundary['md5']), set(boundary['pixel_hashes'])
    for name in ('dev/requests.json', 'hard/requests.json'):
        for r in read(args.out / name):
            banned_ids.add(r['id'])
            if r.get('parent_id'):
                banned_ids.add(r['parent_id'])
            banned_md5.add(r['md5'])
            if r.get('pixel_hash'):
                banned_pixels.add(r['pixel_hash'])

    def convert(post, md5, parent, created, chars, artist, general, copyrights, pixel, ext):
        if not md5 or not re.fullmatch(r'[0-9a-f]{32}', md5) or str(ext).lower() not in STATIC:
            return None
        if str(created)[:10] >= STOP:
            return None
        artist = canonical(artist, artist_aliases)
        if partition(artist) != 'accept' or artist in banned_artists:
            return None
        if post in banned_ids or (parent and parent in banned_ids) or md5 in banned_md5 or (pixel and pixel in banned_pixels):
            return None
        names = sorted({canonical(c, aliases) for c in chars.split()})
        general = set(general.split())
        single = len(names) == 1 and bool({'solo', '1girl', '1boy'} & general)
        no_person = not names and not {'1girl', '1boy', 'solo', 'multiple_girls', 'multiple_boys'} & general
        return dict(id=post, created_at=str(created)[:19], artist=artist, characters=names, copyrights=copyrights.split(),
                    single=single, no_person=no_person, md5=md5, parent_id=parent, pixel_hash=pixel,
                    url=f'https://cdn.donmai.us/720x720/{md5[:2]}/{md5[2:4]}/{md5}.webp')

    rows = []
    with duckdb.connect(config={'threads': '2', 'memory_limit': '3GB'}) as db:
        cursor = db.execute(f"""
            SELECT id, md5, parent_id, strftime(created_at AT TIME ZONE 'UTC', '%Y-%m-%d %H:%M:%S'),
                   tag_string_character, tag_string_artist, tag_string_general, tag_string_copyright,
                   media_asset.pixel_hash, file_ext
            FROM read_parquet(?)
            WHERE rating = 'g' AND tag_count_artist = 1 AND tag_count_character <= 7 AND id > {CUTOFF_POST}
              AND NOT coalesce(is_deleted, false) AND NOT coalesce(is_banned, false)""", [str(args.parquet)])
        for batch in iter(lambda: cursor.fetchmany(50000), []):
            for record in batch:
                if (row := convert(*record)):
                    row['source'] = 'snapshot'
                    rows.append(row)
    print(f'快照里验收区可用帖 {len(rows):,}', flush=True)
    cache = args.out / 'accept/api'
    cache.mkdir(parents=True, exist_ok=True)
    live, upper = live_posts(api, cache)
    live_kept = 0
    for p in live:
        if p.get('is_deleted') or p.get('is_banned') or p.get('rating') != 'g':
            continue
        artists = p.get('tag_string_artist', '').split()
        chars = p.get('tag_string_character', '').split()
        if len(artists) != 1 or len(chars) > 7:
            continue
        pixel = (p.get('media_asset') or {}).get('pixel_hash')
        row = convert(p['id'], p.get('md5'), p.get('parent_id'), p['created_at'],
                      ' '.join(chars), artists[0], p.get('tag_string_general', ''), p.get('tag_string_copyright', ''),
                      pixel, p.get('file_ext', ''))
        if row:
            row['source'] = 'live_api'
            rows.append(row)
            live_kept += 1
    print(f'实时 API {len(live):,} 条，验收区可用 {live_kept:,}', flush=True)

    by_artist = defaultdict(list)
    for r in rows:
        by_artist[r['artist']].append(r)
    used, selected = set(), []

    def take(row, group, target=None):
        used.add(row['artist'])
        selected.append(dict(row, group=group, **({'target': target} if target else {})))

    # 正例：每头单人 1 张、多人 1 张，稀缺的头先挑（按可用画师数从少到多）。
    pos = defaultdict(lambda: {'single': [], 'multi': []})
    for r in rows:
        kind = 'single' if r['single'] else 'multi' if len(r['characters']) > 1 else None
        if kind is None:      # 单角色标签但没有 solo/1girl/1boy，不当正例
            continue
        for n in set(r['characters']) & head_set:
            pos[n][kind].append(r)
    for kind in ('single', 'multi'):
        order = sorted(heads, key=lambda n: (len({r['artist'] for r in pos[n][kind]}), priority(f'{kind}:{n}')))
        for n in order:
            for r in sorted(pos[n][kind], key=lambda r: priority(f"{kind}:{n}:{r['id']}")):
                if r['artist'] not in used:
                    take(r, f'positive_{kind}', n)
                    break
    # 旧角色图：2025-09-26 以后、真值含 B1 已覆盖角色、不含本 Issue 名单（含变体）的任何角色。
    old_pool = [r for r in rows if r['created_at'][:10] >= INDEX_FROZEN and set(r['characters']) & covered
                and not set(r['characters']) & roster_all]
    artists_old = sorted({r['artist'] for r in old_pool} - used, key=lambda a: priority(f'old-artist:{a}'))
    old_by_artist = defaultdict(list)
    for r in old_pool:
        old_by_artist[r['artist']].append(r)
    for a in artists_old[:OLD_LIMIT]:
        take(min(old_by_artist[a], key=lambda r: priority(f"old:{r['id']}")), 'old_character')
    # 对照
    groups = {'no_character': [r for r in rows if r['no_person'] and r['created_at'][:10] >= INDEX_FROZEN],
              'uncovered': [r for r in rows if r['characters'] and r['created_at'][:10] >= INDEX_FROZEN
                            and not set(r['characters']) & (covered | roster_all)]}
    for group, limit in CONTROLS.items():
        count = 0
        for r in sorted(groups[group], key=lambda r: priority(f"{group}:{r['id']}")):
            if count >= limit:
                break
            if r['artist'] not in used:
                take(r, f'control_{group}')
                count += 1
    selected.sort(key=lambda r: priority(f"order:{r['id']}"))
    if len({r['id'] for r in selected}) != len(selected) or len({r['artist'] for r in selected}) != len(selected):
        raise ValueError('验收清单的帖子或画师重复')
    path = write(args.out / 'accept/requests.json', selected)
    heads_with = {r['target'] for r in selected if r['group'].startswith('positive')}
    receipt = dict(
        seed=SEED, partition='illustash-611-v1 accept 20%', stop=STOP, snapshot_max_id=SNAPSHOT_MAX_ID,
        live_upper_id=upper, live_posts=len(live), live_kept=live_kept, snapshot_kept=len(rows) - live_kept,
        requests=len(selected), groups=Counter(r['group'] for r in selected),
        by_source=Counter(r['source'] for r in selected), heads=len(heads), heads_with_positive=len(heads_with),
        old_character_artists_available=len(artists_old), latest=max(r['created_at'] for r in selected),
        inputs=dict(heads=digest(args.out / 'freeze/heads.json'), dev=digest(args.out / 'dev/requests.json'),
                    hard=digest(args.out / 'hard/requests.json'), boundary=digest(args.eval_v1 / 'prior/boundary.json'),
                    eval_v1=digest(args.eval_v1 / 'eval/manifest.json')),
        requests_sha256=digest(path))
    write(args.out / 'accept/receipt.json', receipt)
    print(json.dumps(receipt, ensure_ascii=False, indent=1))


if __name__ == '__main__':
    main()
