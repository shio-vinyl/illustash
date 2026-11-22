"""研究用：按目录对象算中文名、日文名的状态分布，并举例。"""
import collections
import json
import random

from tools.character_catalog import names as names_mod, probe_relations, work as work_mod


def objects_of(work, graph):
    objs = collections.defaultdict(list)
    for tag in work.scope:
        objs[graph.canonical(tag)].append(tag)
    return objs


def families(work, graph):
    """按别名和蕴含（任何种类）把目录对象并成家族，返回 {规范名: 家族根}。"""
    parent = {}

    def find(x):
        while parent.get(x, x) != x:
            parent[x] = parent.get(parent[x], parent[x])
            x = parent[x]
        return x

    for tag in work.scope:
        c = graph.canonical(tag)
        parent.setdefault(c, c)
        for p_ in graph.ancestors(c):
            a, b = find(c), find(p_)
            if a != b:
                parent[a] = b
    return {c: find(c) for c in parent}


def main(argv=None):
    p = work_mod.parser(__doc__)
    p.add_argument('--examples', type=int, default=10)
    args = p.parse_args(argv)
    work = work_mod.open_work(args.work)
    g = probe_relations.load_graph(work)
    wiki = probe_relations.wiki_map(work)
    objs = objects_of(work, g)
    works = {}
    for canon, tags in objs.items():
        cps = next((work.scope[t].copyrights for t in tags if work.scope[t].copyrights), [])
        works[canon] = names_mod.work_key(canon, cps)
    zt = names_mod.zh_tags(work.cache / 'zh-tags/character.csv')
    wd = names_mod.wikidata(work.cache / 'wikidata/p13236.json')
    family = families(work, g)
    recs = names_mod.build(objs, zt, wiki, wd, works, family)
    print('目录对象', len(objs), '（范围 tag', len(work.scope), '）')
    zs = collections.Counter(r['zh_status'] for r in recs.values())
    js = collections.Counter(r['ja_status'] for r in recs.values())
    print('中文名状态', dict(zs))
    print('日文名状态', dict(js))
    why = collections.Counter(w.split('：')[0] for r in recs.values() for w in r['zh_why'])
    print('中文旁证与问题', dict(why))
    print('有英文名（Wikidata）', sum(1 for r in recs.values() if r['en']))
    rnd = random.Random(832)
    for status in ('single', 'conflict', 'reviewed'):
        rows = [(c, r['zh_full'], r['ja'], r['zh_why'][:2]) for c, r in recs.items() if r['zh_status'] == status]
        print(f'== {status}（{len(rows)}）')
        for row in rnd.sample(rows, min(args.examples, len(rows))):
            print('   ', row)
    none = [c for c, r in recs.items() if not r['zh']]
    print('== 没有中文候选', len(none), none[:15])
    json.dump(recs, open(work.root / 'names.json', 'w', encoding='utf-8'), ensure_ascii=False, indent=0)


if __name__ == '__main__':
    main()
