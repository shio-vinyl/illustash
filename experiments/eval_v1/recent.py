"""#551 下一轮摸底：评测集 v1 冻结后，新帖子按 v1 口径能攒多快（只查元数据，不取图）。

评测集 v1 的时间窗到 2026-11-17 为止，摸底时（2026-11-18）新帖子几乎还没有，所以用最近 7 个完整日
（2026-11-11 至 2026-11-17）的公开帖当代表样本估每天的量。Danbooru 2026-11-05 至 11-10 没有任何帖子（停站），
所以只取恢复以后连续的 7 天。可以这样替代的原因：
- 历史排除名单和 v1 的画师已经从这批帖子里扣掉，剩下的就是「还没被用过的画师」的帖子；
- 往后每天的新帖子，也要扣掉同一份名单。

口径照 v1：全年龄、单画师、能取到 720 缩图、单角色帖要有 solo/1girl/1boy；画师按别名归并，每位只算一张；
帖子、父子家族、MD5、像素摘要不重复（内容指纹要取图才能查，这里不算）。
"""

from collections import Counter, defaultdict
import json

import numpy as np

from common import COVERAGE, DATA, load, write
from coverage import api, canonical
import sampling as upstream

FIRST, LAST = '2026-11-11', '2026-11-17'
QUERY_END = '2026-11-18'   # Danbooru 的 date:a..b 不含 b 当天
DAYS = 7


def fetch():
    cache = DATA / 'recent/cache'
    cache.mkdir(parents=True, exist_ok=True)
    rows, cursor, page = [], '', 0
    while True:
        params = {'tags': f'rating:g date:{FIRST}..{QUERY_END}', 'limit': 200, 'only': upstream.FIELDS}
        if cursor:
            params['page'] = f'b{cursor}'
        batch = api(cache / f'page-{page:04d}.json', '/posts.json', params)
        rows.extend(batch)
        if len(batch) < 200:
            return rows
        cursor = min(r['id'] for r in batch)
        page += 1


def main():
    aliases = load(COVERAGE / 'alias-map.json')
    artist_aliases = load(DATA / 'artist-aliases.json')
    prior = load(DATA / 'prior/boundary.json')
    evaluation = load(DATA / 'eval/manifest.json')
    development = load(DATA / 'lt2/requests.json')
    used_artists = set(prior['artists']) | {r['artist'] for r in evaluation} | {r['artist'] for r in development}
    used_posts = set(prior['ids']) | {r['id'] for r in evaluation} | {r['id'] for r in development}
    used_md5 = set(prior['md5']) | {r['md5'] for r in evaluation if r.get('md5')}
    covered = set(load(DATA / 'native-covered.json')['covered_labels'])
    tier2 = {h['label'] for h in load(DATA / 'tier2/heads.json') if h['kept']}
    bands = {e['label']: e['band'] for e in load(DATA / 'roster/targets.json')}
    upstream.MIN_DATE, upstream.MAX_LIVE_DATE = FIRST, LAST
    raw = fetch()
    days = defaultdict(list)
    for record in raw:
        row = upstream.post(record, aliases, allow_multitag_solo=True)
        if not row:
            continue
        row['artist'] = canonical(row['artist'], artist_aliases)
        if row['artist'] in used_artists or row['id'] in used_posts or (row['md5'] and row['md5'] in used_md5):
            continue
        days[row['created_at'][:10]].append(row)
    seen_artists, curve, daily = set(), [], {}
    old_cum, positive_cum = 0, Counter()
    for day in sorted(days):
        # 同一位画师只算第一次出现的那张；按帖子 ID 顺序，等于按时间先后。
        fresh = []
        for row in sorted(days[day], key=lambda r: r['id']):
            if row['artist'] not in seen_artists:
                seen_artists.add(row['artist'])
                fresh.append(row)
        old = [r for r in fresh if covered.intersection(r['characters'])]
        positives = Counter()
        for r in fresh:
            for name in tier2.intersection(r['characters']):
                kind = 'solo' if r['solo'] and r['characters'] == [name] else 'other'
                positives[f"{bands.get(name, '?')}/{kind}"] += 1
        old_cum += len(old)
        positive_cum.update(positives)
        daily[day] = {'eligible_posts': len(days[day]), 'new_artists': len(fresh), 'old_images': len(old),
                      'tier2_positives': dict(positives)}
        curve.append({'day': day, 'old_images_cumulative': old_cum,
                      'tier2_positives_cumulative': dict(positive_cum)})
    # 新画师累计数随天数变慢（同一批活跃画师反复发帖）。用 log-log 拟合 累计 = a·天数^b 外推。
    x = np.log(np.arange(1, len(curve) + 1))
    y = np.log([c['old_images_cumulative'] for c in curve])
    b, log_a = np.polyfit(x, y, 1)
    reach = {target: float(np.exp((np.log(target) - log_a) / b)) for target in (5000, 10000, 15000)}
    heads_hit = defaultdict(set)
    for day in sorted(days):
        for r in days[day]:
            for name in tier2.intersection(r['characters']):
                heads_hit[bands.get(name, '?')].add(name)
    result = {
        'window': [FIRST, LAST], 'raw_records': len(raw), 'days': daily, 'curve': curve,
        'old_images_per_day_mean': round(old_cum / DAYS, 1),
        'fit_cumulative_old_images': {'a': float(np.exp(log_a)), 'b': float(b)},
        'days_to_reach_old_images': reach,
        'tier2_positive_images_7d': dict(positive_cum),
        'tier2_heads_with_any_positive_7d': {k: len(v) for k, v in heads_hit.items()},
        'tier2_heads_by_band': dict(Counter(bands.get(n, '?') for n in tier2)),
    }
    write('recent/report.json', result)
    print(json.dumps({k: v for k, v in result.items() if k not in ('days', 'curve')}, ensure_ascii=False, indent=1))


if __name__ == '__main__':
    main()
