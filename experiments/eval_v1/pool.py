"""按公开元数据建评测集的候选池（此步不下载图片）。

- snapshot：从 #380 冻结的 2026-07-13 帖子快照取 START 以后、全年龄、单画师的帖子：含任一目标或
  旧角色的全部收下，其余（无角色、未覆盖角色）按帖子 ID 散列抽 4% 作对照背景；同时统计长尾角色
  的元数据供给，冻结长尾探查名单。
- live：对每个目标头查 2026-07-13 以后的公开帖；对长尾探查名单查该角色的全部公开帖（<50 帖，
  一页就够）。只走 API，返回小 JSON。
"""

import argparse
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor
import json
import subprocess

import duckdb

from common import (COVERAGE, DATA, PARQUET, SNAPSHOT_END, START, STOP, WD_LAST_ID, load, priority,
                    sha_file, split_557, write)
import sampling as upstream
from coverage import api, canonical

upstream.MIN_DATE, upstream.MAX_LIVE_DATE = START, STOP
# 同作品混淆按外观（发色、瞳色、发型）加服装、形态匹配，沿用 #551 257 头一轮的标签集。
upstream.VISUAL.update({'dress', 'school_uniform', 'military_uniform', 'armor', 'maid', 'suit', 'bodysuit',
                        'swimsuit', 'robe', 'hood', 'hat', 'cape', 'coat', 'jacket', 'wings', 'tail',
                        'mechanical_parts', 'mask', 'helmet', 'facial_mark', 'robot', 'monster', 'chibi'})
MULTIPLE = {'multiple_girls', 'multiple_boys', 'multiple_others',
            *(f'{n}{s}' for n in range(2, 6) for s in ('girls', 'boys', 'others')), '6+girls', '6+boys', '6+others'}
COMIC = {'comic', '4koma', '2koma', '3koma', 'multiple_views', 'reference_sheet', 'split_screen',
         'multiple_drawings', 'variations'}
LINE = {'lineart', 'monochrome', 'greyscale', 'sketch', 'spot_color', 'partially_colored', 'unfinished'}
FIELDS = upstream.FIELDS
BACKGROUND_PERCENT = 4
LONGTAIL_MIN_ARTISTS = 4      # 快照阶段先粗筛：截止后至少 4 位画师的全年龄帖
PROBE_MIN_ARTISTS = 8         # 实时复核只查至少 8 位画师的（train 分区约六成，才可能凑够 5 位参考画师）
LONGTAIL_MIN_TEST_ARTISTS = 1  # 其中至少 1 位落在 #557 的 test 分区（评测正例只从这一区取）


def inputs():
    aliases = load(COVERAGE / 'alias-map.json')
    artists = load(DATA / 'artist-aliases.json')
    targets = {e['label']: e for e in load(DATA / 'roster/targets.json')}
    covered = set(load(DATA / 'roster/covered.json'))
    return aliases, artists, targets, covered


def convert(record, aliases, artists):
    row = upstream.post(record, aliases, allow_multitag_solo=True)
    if row:
        row['artist'] = canonical(row['artist'], artists)
        row['split'] = None
        row['flags'] = sorted(set(record['tag_string_general'].split()) & (MULTIPLE | COMIC | LINE))
    return row


def snapshot(args):
    aliases, artists, targets, covered = inputs()
    known = set(targets) | covered
    raw = sorted(known | {a for a in aliases if canonical(a, aliases) in known})
    db = duckdb.connect(config={'threads': '2', 'memory_limit': '1500MB',
                                 'temp_directory': str(DATA / 'duckdb-temp')})
    db.execute('CREATE TABLE known(name VARCHAR PRIMARY KEY)')
    db.execute('INSERT INTO known SELECT unnest(?)', [raw])
    fields = FIELDS.replace('created_at', "created_at AT TIME ZONE 'UTC' AS created_at")
    cursor = db.execute(f"""
        WITH eligible AS (
            SELECT {fields} FROM read_parquet(?)
            WHERE created_at >= ?::TIMESTAMPTZ AND rating = 'g' AND tag_count_artist = 1
              AND tag_count_character <= 7
              AND NOT coalesce(is_deleted, false) AND NOT coalesce(is_banned, false)
        ), hit AS (
            SELECT DISTINCT id FROM (
                SELECT id, unnest(string_split(tag_string_character, ' ')) name FROM eligible
            ) SEMI JOIN known USING(name)
        )
        SELECT eligible.*, (eligible.id IN (SELECT id FROM hit)) AS known_hit FROM eligible
        WHERE eligible.id IN (SELECT id FROM hit) OR hash(eligible.id + 9551) % 100 < ?
    """, [str(PARQUET), START + ' 00:00:00+00', BACKGROUND_PERCENT])
    columns = [*FIELDS.split(','), 'known_hit']
    rows, seen = [], Counter()
    for batch in iter(lambda: cursor.fetchmany(4096), []):
        for record in batch:
            values = dict(zip(columns, record))
            seen['scanned'] += 1
            row = convert(values, aliases, artists)
            if row:
                row['background'] = not values['known_hit']
                rows.append(row)
    rows.sort(key=lambda r: r['id'])
    write('pool/snapshot.json', rows)
    # 长尾探查名单：截止后首帖、快照内不足 50 帖、不被任何基线或扩展头覆盖的角色。
    excluded = known
    roles = db.execute("""
        SELECT name, min(id) first_id, count(DISTINCT id) total FROM (
            SELECT id, unnest(string_split(tag_string_character, ' ')) name FROM read_parquet(?)
            WHERE tag_string_character <> '' AND NOT coalesce(is_deleted, false) AND NOT coalesce(is_banned, false)
        ) GROUP BY name
    """, [str(PARQUET)]).fetchall()
    merged = defaultdict(lambda: {'first': 1 << 62, 'total': 0})
    for name, first, total in roles:
        key = canonical(name, aliases)
        merged[key]['first'] = min(merged[key]['first'], first)
        merged[key]['total'] += total
    large = {r['name'] for r in load(COVERAGE / 'inventory.json') if r['post_count'] >= 50}
    tail = {n: v for n, v in merged.items()
            if v['first'] > WD_LAST_ID and v['total'] < 50 and n not in excluded and n not in large}
    raw_tail = sorted(set(tail) | {a for a in aliases if canonical(a, aliases) in tail})
    db.execute('CREATE TABLE tail(name VARCHAR PRIMARY KEY)')
    db.execute('INSERT INTO tail SELECT unnest(?)', [raw_tail])
    supply = db.execute(f"""
        SELECT name, string_agg(DISTINCT tag_string_artist, ' ') FROM (
            SELECT tag_string_artist, unnest(string_split(tag_string_character, ' ')) name
            FROM read_parquet(?)
            WHERE id > ? AND rating = 'g' AND tag_count_artist = 1
              AND NOT coalesce(is_deleted, false) AND NOT coalesce(is_banned, false)
        ) SEMI JOIN tail USING(name) GROUP BY name
    """, [str(PARQUET), WD_LAST_ID]).fetchall()
    artist_sets = defaultdict(set)
    for name, joined in supply:
        artist_sets[canonical(name, aliases)].update(canonical(a, artists) for a in joined.split())
    probe = []
    for name, value in sorted(tail.items()):
        found = artist_sets.get(name, set())
        tests = sum(split_557(a) == 'test' for a in found)
        if len(found) >= LONGTAIL_MIN_ARTISTS and tests >= LONGTAIL_MIN_TEST_ARTISTS:
            probe.append({'tag': name, 'first_id': value['first'], 'snapshot_total': value['total'],
                          'snapshot_artists': len(found), 'snapshot_test_artists': tests})
    write('pool/longtail-probe.json', probe)
    receipt = {
        'parquet_sha256': args.parquet_sha or sha_file(PARQUET), 'window': [START, SNAPSHOT_END],
        'background_percent': BACKGROUND_PERCENT, 'scanned': seen['scanned'], 'rows': len(rows),
        'known_rows': sum(not r['background'] for r in rows), 'background_rows': sum(r['background'] for r in rows),
        'tail_roles': len(tail), 'longtail_probe': len(probe),
        'longtail_rule': {'min_snapshot_artists_after_cutoff': LONGTAIL_MIN_ARTISTS,
                          'min_test_partition_artists': LONGTAIL_MIN_TEST_ARTISTS},
        'snapshot_sha256': sha_file(DATA / 'pool/snapshot.json'),
        'probe_sha256': sha_file(DATA / 'pool/longtail-probe.json'),
    }
    write('pool/snapshot-receipt.json', receipt)
    print(json.dumps(receipt, ensure_ascii=False), flush=True)


def early(args):
    """扩展头和长尾角色在 WD 截止后、START 之前的帖子，只用作新角色的评测正例。

    START 是为了让旧角色图晚于精选索引冻结日；新角色本来就不在 WD 和精选索引里，正例早于 START
    不影响独立性（画师、帖子、指纹照样与全部历史取样隔离）。历次取样已用掉这段时间里大部分画师，
    不放宽画师隔离，只把时间窗往前放到 WD 截止。
    """
    aliases, artists, targets, covered = inputs()
    probe = {p['tag'] for p in load(DATA / 'pool/longtail-probe.json') if p['snapshot_artists'] >= PROBE_MIN_ARTISTS}
    news = {n for n in targets} | probe
    raw = sorted(news | {a for a in aliases if canonical(a, aliases) in news})
    upstream.MIN_DATE = '2024-02-28'
    db = duckdb.connect(config={'threads': '2', 'memory_limit': '1500MB',
                                 'temp_directory': str(DATA / 'duckdb-temp')})
    db.execute('CREATE TABLE news(name VARCHAR PRIMARY KEY)')
    db.execute('INSERT INTO news SELECT unnest(?)', [raw])
    fields = FIELDS.replace('created_at', "created_at AT TIME ZONE 'UTC' AS created_at")
    cursor = db.execute(f"""
        WITH eligible AS (
            SELECT {fields} FROM read_parquet(?)
            WHERE id > ? AND created_at < ?::TIMESTAMPTZ AND rating = 'g' AND tag_count_artist = 1
              AND tag_count_character BETWEEN 1 AND 7
              AND NOT coalesce(is_deleted, false) AND NOT coalesce(is_banned, false)
        ), hit AS (
            SELECT DISTINCT id FROM (
                SELECT id, unnest(string_split(tag_string_character, ' ')) name FROM eligible
            ) SEMI JOIN news USING(name)
        )
        SELECT eligible.* FROM eligible WHERE eligible.id IN (SELECT id FROM hit)
    """, [str(PARQUET), WD_LAST_ID, START + ' 00:00:00+00'])
    columns = FIELDS.split(',')
    rows = []
    for batch in iter(lambda: cursor.fetchmany(4096), []):
        for record in batch:
            row = convert(dict(zip(columns, record)), aliases, artists)
            if row and news.intersection(row['characters']):
                row['background'] = False
                row['early'] = True
                rows.append(row)
    rows.sort(key=lambda r: r['id'])
    write('pool/early.json', rows)
    receipt = {'window': [f'id>{WD_LAST_ID}', START], 'rows': len(rows), 'early_sha256': sha_file(DATA / 'pool/early.json')}
    write('pool/early-receipt.json', receipt)
    print(json.dumps(receipt, ensure_ascii=False), flush=True)


def live(args):
    aliases, artists, targets, _ = inputs()
    probe = [] if args.heads_only else [p for p in load(DATA / 'pool/longtail-probe.json')
                                        if p['snapshot_artists'] >= PROBE_MIN_ARTISTS]
    cache = DATA / 'pool/live'
    cache.mkdir(parents=True, exist_ok=True)
    jobs = [('head', name) for name, e in sorted(targets.items()) if e['route'] == 'wd_head']
    jobs += [('tail', name) for name in sorted({p['tag'] for p in probe} |
                                               {n for n, e in targets.items() if e['route'] == 'ccip_longtail'})]
    if args.heads_only:
        # 只预热扩展头的实时缓存（快照还没跑完时先跑网络部分）；正式 live 阶段会读同一缓存。
        jobs = [j for j in jobs if j[0] == 'head']

    def fetch(job):
        kind, name = job
        key = f'{kind}-{priority(name):016x}.json'
        if kind == 'head':
            params = {'tags': f'{name} rating:g date:{SNAPSHOT_END}..{STOP}', 'limit': 200, 'only': FIELDS}
        else:
            params = {'tags': f'{name} date:..{STOP}', 'limit': 100, 'only': FIELDS}
        try:
            records = api(cache / key, '/posts.json', params)
        except subprocess.CalledProcessError as error:
            return job, None, f'http_{error.returncode}'
        if not isinstance(records, list):
            return job, None, 'not_a_list'
        return job, records, None

    results, errors, receipt_rows = {}, {}, []
    with ThreadPoolExecutor(max_workers=4) as executor:
        for i, (job, records, error) in enumerate(executor.map(fetch, jobs), 1):
            if error:
                errors['/'.join(job)] = error
                continue
            receipt_rows.append({'kind': job[0], 'tag': job[1], 'records': len(records),
                                 'page_full': len(records) >= (200 if job[0] == 'head' else 100)})
            for record in records:
                results[record['id']] = record
            if i % 200 == 0:
                print(f'live {i}/{len(jobs)}，失败 {len(errors)}', flush=True)
    rows, tail_posts = [], defaultdict(list)
    for record in results.values():
        names = {canonical(t, aliases) for t in record['tag_string_character'].split()}
        for name in names:
            tail_posts[name].append({'id': record['id'], 'created_at': str(record['created_at']),
                                     'rating': record.get('rating'), 'is_deleted': record.get('is_deleted'),
                                     'is_banned': record.get('is_banned'),
                                     'artist': canonical((record['tag_string_artist'].split() or [''])[0], artists),
                                     'artists': len(record['tag_string_artist'].split())})
        row = convert(record, aliases, artists)
        if row:
            row['background'] = False
            rows.append(row)
    if args.heads_only:
        print(f'预热完成：{len(jobs)} 个查询，失败 {len(errors)}', flush=True)
        return
    rows.sort(key=lambda r: r['id'])
    write('pool/live.json', rows)
    probed = {name for kind, name in jobs if kind == 'tail'}
    write('pool/live-tail-posts.json', {name: sorted(tail_posts.get(name, []), key=lambda r: r['id'])
                                        for name in sorted(probed)})
    receipt = {'jobs': len(jobs), 'probe_min_artists': PROBE_MIN_ARTISTS, 'probed_tails': len(probe), 'errors': errors, 'records': len(results), 'rows': len(rows),
               'full_pages': Counter(r['kind'] for r in receipt_rows if r['page_full']),
               'live_sha256': sha_file(DATA / 'pool/live.json'),
               'tail_posts_sha256': sha_file(DATA / 'pool/live-tail-posts.json')}
    write('pool/live-receipt.json', receipt)
    print(json.dumps(receipt, ensure_ascii=False), flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('stage', choices=('snapshot', 'early', 'live'))
    parser.add_argument('--parquet-sha', help='已知的快照摘要，省去对 4 GB 文件重算')
    parser.add_argument('--heads-only', action='store_true')
    arguments = parser.parse_args()
    (DATA / 'duckdb-temp').mkdir(parents=True, exist_ok=True)
    {'snapshot': snapshot, 'early': early, 'live': live}[arguments.stage](arguments)
