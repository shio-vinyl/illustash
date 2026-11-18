"""#611：只按公开元数据统计冻结名单里每个角色能取到多少合格图，用来估算取图量和算力。

读 #380 的固定公开帖快照（截至 2026-07-13），不联网、不取图、不看任何识别分数。
合格图的口径照 #534/#551 的 sampling.post：全年龄（rating g）、未删除未封禁、单一画师，
能拼出 720 缩图；单角色帖要带 solo/1girl/1boy。画师按 2026-11-18 的画师别名归并，
一律排除共用评测集 v1 的画师和帖子；历次研究（#380、#534、#551、#557、#456）用过的画师和帖子只从验收区排除。

画师按拟定的散列分区（种子 illustash-611-v1）：验收 20%、训练 56%、校准 24%。每个角色在各区分别数
单人单角色图的不同画师（全部年份、帖子 ID 晚于 WD 截止的），以及含该角色的多人图的不同画师。
另记每个角色最常见的作品标签，用来估同作品混淆。
"""

import argparse
from collections import Counter, defaultdict
import hashlib
import json
import os
from pathlib import Path
import re

import csv

import duckdb

from roster import CUTOFF_POST, ROOT, canonical, digest, read, write

SEED = 'illustash-611-v1'


def partition(artist):
    slot = int.from_bytes(hashlib.sha256(f'{SEED}:{artist}'.encode()).digest()[:8], 'big') % 100
    return 'accept' if slot < 20 else 'train' if slot < 76 else 'calibration'


def main():
    main_checkout = Path(os.environ.get('ISSUE_611_MAIN') or ROOT.parents[2])
    worktrees = main_checkout / '.worktrees'
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--coverage', type=Path, default=worktrees / 'issue-380-character-coverage/_scratch/issue-380/coverage')
    parser.add_argument('--parquet', type=Path, default=worktrees / 'issue-380-character-coverage/_scratch/issue-380/posts.parquet')
    parser.add_argument('--eval-v1', type=Path, required=True, help='共用评测集 v1 的 _scratch/eval-v1 目录')
    parser.add_argument('--out', type=Path, default=Path(os.environ.get('ISSUE_611_OUTPUT', ROOT / '_scratch/issue-611')))
    args = parser.parse_args()
    roster_path = args.out / 'roster/roster.json'
    roster = {r['label']: r for r in read(roster_path)}
    aliases = read(args.coverage / 'alias-map.json')
    artist_aliases = read(args.eval_v1 / 'artist-aliases.json')
    boundary = read(args.eval_v1 / 'prior/boundary.json')
    evaluation = read(args.eval_v1 / 'eval/manifest.json')
    # v1 评测集的画师和帖子一律不用；历次研究用过的画师只从验收区排除，训练和校准可以用。
    v1_artists = {canonical(r['artist'], artist_aliases) for r in evaluation}
    v1_ids = {r['id'] for r in evaluation}
    prior_artists = {canonical(a, artist_aliases) for a in boundary['artists']}
    prior_ids, prior_md5 = set(boundary['ids']), set(boundary['md5'])
    # 已覆盖的角色（WD 词表 ∪ 完整精选索引），用来认出「和已覆盖角色同帖」的变体标签。
    with (worktrees / 'issue-551-wd-b-expansion/tags/tags_v3.csv').open(encoding='utf-8', newline='') as stream:
        covered = {canonical(r['name'], aliases) for r in csv.DictReader(stream) if r['category'] == '4'}
    index_rows = read(worktrees / 'issue-534-character-expansion/_scratch/issue-534/index/loaded-characters.json')
    covered |= {canonical(r['tag'], aliases) for r in index_rows}
    raw = sorted(set(roster) | {a for a in aliases if canonical(a, aliases) in roster})

    stats = defaultdict(lambda: defaultdict(set))
    works = defaultdict(Counter)
    posts_seen, with_covered = Counter(), Counter()
    excluded = prior_excluded = rows_read = 0
    with duckdb.connect(config={'threads': '2', 'memory_limit': '3GB'}) as db:
        db.execute('CREATE TEMP TABLE names(raw VARCHAR PRIMARY KEY)')
        db.executemany('INSERT INTO names VALUES (?)', [[n] for n in raw])
        # 先按角色标签展开、和名单做哈希连接，再取整行；比逐行 list_has_any 快得多。
        cursor = db.execute("""
            WITH posts AS (
                SELECT id, md5, tag_string_character, tag_string_artist, tag_string_general, tag_string_copyright
                FROM read_parquet(?)
                WHERE rating = 'g' AND tag_count_artist = 1 AND tag_count_character BETWEEN 1 AND 7
                  AND NOT coalesce(is_deleted, false) AND NOT coalesce(is_banned, false)
            ), hit AS (
                SELECT DISTINCT p.id FROM posts p, unnest(string_split(p.tag_string_character, ' ')) AS u(c)
                JOIN names n ON n.raw = u.c
            )
            SELECT posts.* FROM posts SEMI JOIN hit USING (id)
        """, [str(args.parquet)])
        for records in iter(lambda: cursor.fetchmany(20000), []):
            for post, md5, chars, artist, general, copyrights in records:
                rows_read += 1
                if not md5 or not re.fullmatch(r'[0-9a-f]{32}', md5):
                    continue
                artist = canonical(artist, artist_aliases)
                if post in v1_ids or artist in v1_artists:
                    excluded += 1
                    continue
                prior = post in prior_ids or md5 in prior_md5 or artist in prior_artists
                names = {canonical(c, aliases) for c in chars.split()}
                general = set(general.split())
                single = len(names) == 1 and bool({'solo', '1girl', '1boy'} & general)
                solo_multitag = len(names) > 1 and 'solo' in general
                part = partition(artist)
                if part == 'accept' and prior:
                    prior_excluded += 1
                    continue
                late = post > CUTOFF_POST
                for name in names & roster.keys():
                    bucket = stats[name]
                    works[name].update(copyrights.split())
                    posts_seen[name] += 1
                    with_covered[name] += bool(names & covered)
                    kind = 'solo' if single else 'solo_mt' if solo_multitag else 'multi' if len(names) > 1 else None
                    if kind:
                        bucket[f'{kind}_{part}'].add(artist)
                        if late:
                            bucket[f'{kind}_{part}_post_wd'].add(artist)
    keys = [f'{kind}_{part}{suffix}' for kind in ('solo', 'solo_mt', 'multi') for part in ('train', 'calibration', 'accept')
            for suffix in ('', '_post_wd')]
    rows = []
    for name, meta in sorted(roster.items()):
        bucket = stats.get(name, {})
        top = works[name].most_common(1)
        rows.append(dict(label=name, band=meta['band'], post_count=meta['post_count'],
                         work=top[0][0] if top else None, g_posts=posts_seen[name],
                         with_covered_posts=with_covered[name], **{k: len(bucket.get(k, ())) for k in keys}))
    path = write(args.out / 'supply/supply.json', rows)
    receipt = dict(seed=SEED, partition={'accept': 20, 'train': 56, 'calibration': 24},
                   roster_sha256=digest(roster_path), parquet_bytes=args.parquet.stat().st_size,
                   boundary_sha256=digest(args.eval_v1 / 'prior/boundary.json'),
                   eval_manifest_sha256=digest(args.eval_v1 / 'eval/manifest.json'),
                   artist_aliases_sha256=digest(args.eval_v1 / 'artist-aliases.json'),
                   rows_read=rows_read, excluded_v1_posts=excluded, excluded_prior_from_accept=prior_excluded, supply_sha256=digest(path))
    write(args.out / 'supply/receipt.json', receipt)
    print(json.dumps(receipt, ensure_ascii=False, indent=1))


if __name__ == '__main__':
    main()
