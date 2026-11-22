"""研究用：按作品统计角色 tag 的张数分布，看热门作品里有多少角色因为张数不够没进识别范围。

作品按 tag 末尾括号里的限定词认（先在 D 站别名里换成规范的作品 tag，例如 kancolle → kantai_collection）；
没有括号的角色 tag 用基础索引自带的作品，两样都没有的记为「认不出作品」。
作品热度用 D 站作品类 tag 的张数。要先抓作品类 tag（--fetch 一次即可）。
"""
import collections
import json

from tools.character_catalog import danbooru, tagparse, work as work_mod


def fetch_copyrights(client, target, min_posts):
    page = 1
    with open(target, 'w', encoding='utf-8') as out:
        while True:
            rows = client.get('tags.json', {
                'search[category]': 3, 'search[post_count]': f'>={min_posts}', 'search[order]': 'count',
                'only': 'name,post_count', 'limit': 1000, 'page': page,
            })
            out.write(json.dumps({'page': page, 'rows': rows}, ensure_ascii=False) + '\n')
            if len(rows) < 1000:
                break
            page += 1


def main(argv=None):
    p = work_mod.parser(__doc__.splitlines()[0])
    p.add_argument('--fetch', action='store_true')
    p.add_argument('--top', type=int, default=40)
    args = p.parse_args(argv)
    work = work_mod.open_work(args.work)
    cache = work.cache / 'danbooru'
    client = danbooru.Client()
    if args.fetch:
        fetch_copyrights(client, cache / 'copyrights.jsonl', 200)
    copyrights = {}
    for line in open(cache / 'copyrights.jsonl', encoding='utf-8'):
        for r in json.loads(line)['rows']:
            copyrights[r['name']] = r['post_count']
    universe = []
    for line in open(cache / 'universe.jsonl', encoding='utf-8'):
        universe += [r for r in json.loads(line)['rows'] if not r.get('is_deprecated')]
    quals = sorted({tagparse.split(r['name'])[1][-1] for r in universe if tagparse.split(r['name'])[1]})
    if args.fetch:
        danbooru.fetch(client, cache, 'aliases_from', [q for q in quals if q not in copyrights])
    alias = {r['antecedent_name']: r['consequent_name'] for r in work.danbooru('aliases_from')}

    def work_of(tag):
        _, qs = tagparse.split(tag)
        if qs:
            q = qs[-1]
            q = q if q in copyrights else alias.get(q, q)
            if q in copyrights:
                return q
        entry = work.scope.get(tag)
        for c in (entry.copyrights if entry else []):
            if c in copyrights:
                return c
        return None

    per = collections.defaultdict(lambda: collections.Counter())
    unknown = collections.Counter()
    for r in universe:
        w = work_of(r['name'])
        band = 'ge50' if r['post_count'] >= 50 else '20-49'
        inside = r['name'] in work.scope
        if w is None:
            unknown[band] += 1
            continue
        per[w][band] += 1
        per[w]['in_scope'] += inside
        if band == '20-49' and not inside:
            per[w]['missed_20_49'] += 1
    print('张数>=20 的角色 tag', len(universe), '认不出作品的', dict(unknown))
    ranked = sorted(per, key=lambda w: -copyrights[w])
    print(f'按作品 tag 张数排前 {args.top} 的作品：角色（>=50 / 20-49）、在识别范围里的、20-49 张且不在范围里的')
    for w in ranked[:args.top]:
        c = per[w]
        print(f'  {w}\t作品 {copyrights[w]}\t角色 {c["ge50"]}/{c["20-49"]}\t范围内 {c["in_scope"]}\t漏 {c["missed_20_49"]}')
    for top in (50, 100, 200, 500):
        missed = sum(per[w]['missed_20_49'] for w in ranked[:top])
        print(f'前 {top} 部作品里，20-49 张且不在识别范围里的角色 tag', missed)


if __name__ == '__main__':
    main()
