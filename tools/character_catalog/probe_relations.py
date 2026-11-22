"""研究用：蕴含给出的候选关系，按种类计数并举例；列出还要补抓的范围外 tag。"""
import collections
import json
import random

from tools.character_catalog import graph as graph_mod, relations, work as work_mod


def load_graph(work):
    tags = work.danbooru('tags')
    aliases = work.danbooru('aliases_from') + work.danbooru('aliases_to')
    implications = work.danbooru('implications_from') + work.danbooru('implications_to')
    return graph_mod.Graph(tags, aliases, implications)


def wiki_map(work):
    return {r['title']: r for r in work.danbooru('wiki')}


def main(argv=None):
    p = work_mod.parser(__doc__)
    p.add_argument('--examples', type=int, default=12)
    p.add_argument('--write-extra', action='store_true', help='把还没抓的范围外 tag 写进 work/extra_names.txt')
    args = p.parse_args(argv)
    work = work_mod.open_work(args.work)
    g = load_graph(work)
    wiki = wiki_map(work)
    scope = work.scope

    # 范围外、出现在别名或蕴含另一端的 tag：要补抓类别和 wiki。
    extra = set()
    for t in scope:
        c = g.canonical(t)
        if c not in scope:
            extra.add(c)
        for p_ in g.up.get(c, ()):
            if p_ not in scope:
                extra.add(p_)
    unknown_cat = sorted(t for t in extra if t not in g.tags)
    print('范围外的关联 tag', len(extra), '其中还没抓类别', len(unknown_cat))
    if args.write_extra:
        (work.root / 'extra_names.txt').write_text('\n'.join(sorted(extra)) + '\n', encoding='utf-8')

    recs = relations.analyze(scope, g, wiki)
    kinds = collections.Counter()
    examples = collections.defaultdict(list)
    has_parent = 0
    for tag, rec in recs.items():
        if 'relations' in rec:
            has_parent += 1
            for parent, r in rec['relations'].items():
                kinds[r['kind']] += 1
                examples[r['kind']].append((tag, parent, r['why'], parent in scope))
        elif 'prefix' in rec:
            kinds['prefix'] += 1
            examples['prefix'].append((tag, rec['prefix'], '', rec['prefix'] in scope))
    print('有角色类上级的范围内 tag', has_parent)
    print('候选关系按种类', dict(kinds))
    rnd = random.Random(837)
    for kind, rows in examples.items():
        print(f'== {kind}（{len(rows)}）上级也在范围内的 {sum(1 for r in rows if r[3])}')
        for row in rnd.sample(rows, min(args.examples, len(rows))):
            print('   ', row[0], '→', row[1], '|', row[2][:110])
    multi = [t for t, r in recs.items() if len(r.get('parents', ())) > 1]
    print('蕴含多个角色的 tag', len(multi), multi[:10])
    json.dump(recs, open(work.root / 'relations.json', 'w', encoding='utf-8'), ensure_ascii=False, indent=0)


if __name__ == '__main__':
    main()
