"""#611 一期：按公开元数据冻结开发数据（训练、校准）的取图清单，只取图不看分数。

名单用去掉变体标签以后重新冻结的 roster-v2.json（16,701 个，2026-11-18 用户定变体这一轮先不做）。
数据：#380 的固定公开帖快照（截至 2026-07-13），全部早于 2026-11-17。
画师分区照 supply.py（种子 illustash-611-v1）：验收区 20% 的画师一律不进开发数据，
留给二期验收；共用评测集 v1 的画师和帖子一律不用。历次研究用过的画师可以用。

合格图口径照 #534 的 sampling.post：全年龄、未删除未封禁、单一画师、静态图（不要视频、
动图包、Flash），单角色帖要带 solo/1girl/1boy；画师按别名归并。不限年份。

每个能训的候选（训练区有至少 3 位画师、校准区至少 2 位，按 supply 的数）：
- 训练正例：训练区每位画师先取 1 张，最多 11 张；不到 5 张时每位画师再补 1 张，补到 5 张为止。
- 校准正例：校准区单人 4 张、多人 2 张，每位画师 1 张，帖子 ID 晚于 WD 截止的优先。
负例和对照（训练区和校准区的画师都取，落在哪区由画师散列决定）：
- 同作品旧角色：每部作品（按候选最常见的作品标签）的旧角色单人图，配额 min(80, 6+3n)，
  n 是该作品里能训的候选数，同一旧角色最多 4 张。旧角色 = WD 词表 ∪ 完整精选索引 ∪ 一级 257 头 ∪ 长尾 v1。
- 对照：WD 截止后的旧角色多人图 2,000 张、无关作品旧角色单人图 2,000 张、无角色图 1,000 张，按帖子散列取。
同一帖子只请求一次，角色身份按公开标签记在清单里。
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

from roster import CUTOFF_POST, ROOT, canonical, digest, read, write
from supply import partition

SEED = 'illustash-611-dev'
STATIC = ('jpg', 'jpeg', 'png', 'webp', 'avif', 'gif')
CONTROLS = {'multi_old': 2000, 'unrelated_old': 2000, 'no_character': 1000}


def priority(key):
    return int.from_bytes(hashlib.sha256(f'{SEED}:{key}'.encode()).digest()[:8], 'big')


QUERY = """
    SELECT id, md5, parent_id, strftime(created_at AT TIME ZONE 'UTC', '%Y-%m-%d %H:%M:%S') AS created,
           tag_string_character, tag_string_artist, tag_string_general, tag_string_copyright,
           media_asset.pixel_hash AS pixel_hash
    FROM read_parquet(?)
    WHERE rating = 'g' AND tag_count_artist = 1 AND tag_count_character BETWEEN {low} AND 7
      AND NOT coalesce(is_deleted, false) AND NOT coalesce(is_banned, false)
      AND lower(file_ext) IN ({exts}) {extra}
"""


def records(db, parquet, low=1, extra='', params=()):
    sql = QUERY.format(low=low, exts=','.join(f"'{e}'" for e in STATIC), extra=extra)
    cursor = db.execute(sql, [str(parquet), *params])
    yield from iter(lambda: cursor.fetchmany(50000), [])


def main():
    main_checkout = Path(os.environ.get('ISSUE_611_MAIN') or ROOT.parents[2])
    worktrees = main_checkout / '.worktrees'
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--coverage', type=Path, default=worktrees / 'issue-380-character-coverage/_scratch/issue-380/coverage')
    parser.add_argument('--parquet', type=Path, default=worktrees / 'issue-380-character-coverage/_scratch/issue-380/posts.parquet')
    parser.add_argument('--eval-v1', type=Path, required=True)
    parser.add_argument('--out', type=Path, default=Path(os.environ.get('ISSUE_611_OUTPUT', ROOT / '_scratch/issue-611')))
    args = parser.parse_args()
    roster = {r['label']: r for r in read(args.out / 'roster/roster-v2.json')}
    supply = {r['label']: r for r in read(args.out / 'supply/supply.json')}
    aliases = read(args.coverage / 'alias-map.json')
    artist_aliases = read(args.eval_v1 / 'artist-aliases.json')
    evaluation = read(args.eval_v1 / 'eval/manifest.json')
    v1_artists = {canonical(r['artist'], artist_aliases) for r in evaluation}
    v1_ids = {r['id'] for r in evaluation}
    with (worktrees / 'issue-551-wd-b-expansion/tags/tags_v3.csv').open(encoding='utf-8', newline='') as stream:
        old = {canonical(r['name'], aliases) for r in csv.DictReader(stream) if r['category'] == '4'}
    old |= {canonical(r['tag'], aliases) for r in
            read(worktrees / 'issue-534-character-expansion/_scratch/issue-534/index/loaded-characters.json')}
    targets_v1 = read(args.eval_v1 / 'roster/targets.json')
    old |= {canonical(r['label'], aliases) for r in targets_v1 if r['tier1_257'] or r['route'] == 'ccip_longtail'}
    package_04 = {canonical(r['label'], aliases) for r in targets_v1 if r['route'] == 'wd_head'}

    def trainable(r):
        return (r['solo_train'] + r['solo_mt_train'] >= 3
                and r['solo_calibration'] + r['solo_mt_calibration'] + r['multi_calibration'] >= 2)

    chosen = sorted(name for name in roster if trainable(supply[name]))
    chosen_set = set(chosen)

    def convert(record):
        post, md5, parent, created, chars, artist, general, copyrights, pixel = record
        if not md5 or not re.fullmatch(r'[0-9a-f]{32}', md5) or post in v1_ids:
            return None
        artist = canonical(artist, artist_aliases)
        part = partition(artist)
        if artist in v1_artists or part == 'accept':
            return None
        names = sorted({canonical(c, aliases) for c in chars.split()})
        general = set(general.split())
        if len(names) == 1 and not {'solo', '1girl', '1boy'} & general:
            return None
        return dict(id=post, created_at=created, artist=artist, characters=names, copyrights=copyrights.split(),
                    solo='solo' in general or (len(names) == 1), md5=md5, parent_id=parent, pixel_hash=pixel,
                    url=f'https://cdn.donmai.us/720x720/{md5[:2]}/{md5[2:4]}/{md5}.webp', split=part)

    raw = sorted(chosen_set | {a for a in aliases if canonical(a, aliases) in chosen_set})
    by_candidate = defaultdict(list)
    with duckdb.connect(config={'threads': '2', 'memory_limit': '3GB'}) as db:
        db.execute('CREATE TEMP TABLE names(raw VARCHAR PRIMARY KEY)')
        db.executemany('INSERT INTO names VALUES (?)', [[n] for n in raw])
        extra = """AND id IN (SELECT DISTINCT p.id FROM read_parquet(?) p,
                       unnest(string_split(p.tag_string_character, ' ')) AS u(c) JOIN names n ON n.raw = u.c)"""
        for batch in records(db, args.parquet, extra=extra, params=[str(args.parquet)]):
            for record in batch:
                row = convert(record)
                if not row:
                    continue
                hits = set(row['characters']) & chosen_set
                for name in hits:
                    by_candidate[name].append(row)
        print(f'候选帖读完：{sum(map(len, by_candidate.values())):,} 条', flush=True)

        selected = {}
        roles = defaultdict(set)

        def take(row, role):
            selected.setdefault(row['id'], row)
            roles[row['id']].add(role)

        report = Counter()
        for name in chosen:
            rows = sorted(by_candidate[name], key=lambda r: priority(f"{name}:{r['id']}"))
            single = [r for r in rows if r['solo']]
            train = [r for r in single if r['split'] == 'train']
            picked, artists = [], set()
            for r in train:
                if r['artist'] not in artists and len(picked) < 11:
                    picked.append(r)
                    artists.add(r['artist'])
            for r in train:
                if len(picked) >= 5:
                    break
                if r not in picked and sum(p['artist'] == r['artist'] for p in picked) < 2:
                    picked.append(r)
            for r in picked:
                take(r, f'train:{name}')
            calibration = sorted((r for r in rows if r['split'] == 'calibration'),
                                 key=lambda r: (r['id'] <= CUTOFF_POST, priority(f"{name}:{r['id']}")))
            for solo, limit in ((True, 4), (False, 2)):
                seen = set()
                for r in calibration:
                    if r['solo'] == solo and r['artist'] not in seen and len(seen) < limit:
                        seen.add(r['artist'])
                        take(r, f'calibration:{name}')
            report['train_images'] += len(picked)

        works = Counter(supply[n]['work'] for n in chosen if supply[n]['work'] not in (None, 'original'))
        # 第二遍：旧角色单人图（同作品负例）和对照。只读含旧角色或无角色的帖子。
        old_raw = sorted(old | {a for a in aliases if canonical(a, aliases) in old})
        db.execute('CREATE TEMP TABLE olds(raw VARCHAR PRIMARY KEY)')
        db.executemany('INSERT INTO olds VALUES (?)', [[n] for n in old_raw])
        pools = defaultdict(list)
        controls = defaultdict(list)
        extra_old = """AND (tag_count_character = 0 OR id IN (SELECT DISTINCT p.id FROM read_parquet(?) p,
                       unnest(string_split(p.tag_string_character, ' ')) AS u(c) JOIN olds o ON o.raw = u.c))"""
        for batch in records(db, args.parquet, low=0, extra=extra_old, params=[str(args.parquet)]):
            for record in batch:
                if record[4] == '':
                    # 无角色对照：WD 截止后、没有人物标签的图。
                    post, md5, parent, created, _chars, artist, general, copyrights, pixel = record
                    if post <= CUTOFF_POST or {'1girl', '1boy', 'solo', 'multiple_girls', 'multiple_boys'} & set(general.split()):
                        continue
                    artist = canonical(artist, artist_aliases)
                    if (md5 and re.fullmatch(r'[0-9a-f]{32}', md5) and post not in v1_ids
                            and artist not in v1_artists and partition(artist) != 'accept'):
                        controls['no_character'].append(dict(
                            id=post, created_at=created, artist=artist, characters=[], copyrights=copyrights.split(),
                            solo=False, md5=md5, parent_id=parent, pixel_hash=pixel, split=partition(artist),
                            url=f'https://cdn.donmai.us/720x720/{md5[:2]}/{md5[2:4]}/{md5}.webp'))
                    continue
                row = convert(record)
                if not row or set(row['characters']) & (chosen_set | package_04):
                    continue
                olds_here = set(row['characters']) & old
                if not olds_here:
                    continue
                if row['solo'] and len(row['characters']) == 1:
                    character = row['characters'][0]
                    for title in set(row['copyrights']) & works.keys():
                        pools[('work', title)].append(row)
                    if row['id'] > CUTOFF_POST and not set(row['copyrights']) & works.keys():
                        controls['unrelated_old'].append(row)
                elif len(row['characters']) > 1 and row['id'] > CUTOFF_POST:
                    controls['multi_old'].append(row)
        print('旧角色帖读完', flush=True)

        def fill(rows, limit, role, per_character, key):
            rows = sorted(rows, key=lambda r: priority(f"{key}:{r['id']}"))
            count, artists, per = 0, set(), Counter()
            for r in rows:
                if count >= limit:
                    break
                character = r['characters'][0] if r['characters'] else ''
                if r['artist'] in artists or (per_character and per[character] >= per_character):
                    continue
                artists.add(r['artist'])
                per[character] += 1
                take(r, role)
                count += 1
            return count

        for title, n in sorted(works.items()):
            report['same_work_old'] += fill(pools[('work', title)], min(80, 6 + 3 * n), f'same_work_old:{title}', 4,
                                            f'work:{title}')
        for kind, limit in CONTROLS.items():
            # 无角色图没有角色可分，只按画师去重。
            per_character = 0 if kind == 'no_character' else 2
            report[kind] += fill(controls[kind], limit, f'control:{kind}', per_character, f'control:{kind}')

    manifest = []
    for post in sorted(selected, key=lambda p: priority(f'order:{p}')):
        manifest.append(dict(selected[post], roles=sorted(roles[post])))
    path = write(args.out / 'dev/requests.json', manifest)
    role_counts = Counter(role.split(':')[0] for row in manifest for role in row['roles'])
    receipt = dict(
        seed=SEED, candidates_trainable_by_supply=len(chosen),
        works=len(works), requests=len(manifest), role_counts=role_counts, selection=report,
        by_split=Counter(r['split'] for r in manifest), post_wd=sum(r['id'] > CUTOFF_POST for r in manifest),
        latest=max(r['created_at'] for r in manifest),
        inputs=dict(roster=digest(args.out / 'roster/roster-v2.json'), supply=digest(args.out / 'supply/supply.json'),
                    eval_v1_manifest=digest(args.eval_v1 / 'eval/manifest.json'),
                    eval_v1_targets=digest(args.eval_v1 / 'roster/targets.json')),
        requests_sha256=digest(path))
    write(args.out / 'dev/receipt.json', receipt)
    print(json.dumps(receipt, ensure_ascii=False, indent=1))


if __name__ == '__main__':
    main()
