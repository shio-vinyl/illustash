"""研究用：Bangumi 整库导出能给我们范围内的多少角色对上中文名。

    python3 -I tools/character_catalog/probe_bangumi.py 工作目录 导出zip

对法（先认作品，再在作品里认人）：
1. D 站角色的作品：tag 末尾括号（经别名换成规范作品 tag），没有括号的用基础索引给的作品；
2. D 站作品 → Bangumi 条目：作品 wiki 的 `other_names` 和 tag 可读名，跟条目的原名、中文名比；
3. 作品里认人：D 站角色的 wiki `other_names`（多为日文原名）和可读名，跟 Bangumi 角色的原名、日文名、
   罗马字、英文名比，同一作品里只对上一个才算。
对不上作品的，退一步在全库按名字找，名字唯一才算（单列统计）。
"""
import collections
import json
from pathlib import Path
import random
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from tools.character_catalog import bangumi, names as names_mod, probe_relations, tagparse, work as work_mod  # noqa: E402


def main(argv=None):
    args = argv or sys.argv[1:]
    work = work_mod.open_work(args[0])
    characters, subjects, links, bangumi_relations = bangumi.load(args[1])
    g = probe_relations.load_graph(work)
    wiki = probe_relations.wiki_map(work)
    alias = {r['antecedent_name']: r['consequent_name'] for r in work.danbooru('aliases_from')}
    copyrights = {}
    for line in open(work.cache / 'danbooru/copyrights.jsonl', encoding='utf-8'):
        for r in json.loads(line)['rows']:
            copyrights[r['name']] = r['post_count']

    # Bangumi：名字 → 条目；条目 → 角色
    subj_by_name = collections.defaultdict(set)
    for sid, s in subjects.items():
        for n in (s['name'], s['name_cn']):
            if n:
                subj_by_name[bangumi.norm(n)].add(sid)
    chars_of = collections.defaultdict(set)
    for cid, ls in links.items():
        for sid, _ in ls:
            chars_of[sid].add(cid)
    char_by_name = collections.defaultdict(set)
    for cid, c in characters.items():
        for n in c['names']:
            char_by_name[bangumi.norm(n)].add(cid)

    def work_of(tag):
        _, qs = tagparse.split(tag)
        if qs:
            q = qs[-1]
            q = q if q in copyrights else alias.get(q, q)
            if q in copyrights:
                return q
        entry = work.scope.get(tag)
        for c in (entry.copyrights if entry else []):
            c = c if c in copyrights else alias.get(c, c)
            if c in copyrights:
                return c
        return None

    def subjects_of(copyright):
        names = [tagparse.readable(copyright)] + list((wiki.get(copyright) or {}).get('other_names') or [])
        out = set()
        for n in names:
            out |= subj_by_name.get(bangumi.norm(n), set())
        return out

    def danbooru_names(tag):
        names = list((wiki.get(tag) or {}).get('other_names') or [])
        base, _ = tagparse.split(tag)
        names.append(base.replace('_', ' '))
        return {bangumi.norm(n) for n in names if n}

    objs = collections.defaultdict(list)
    for tag in work.scope:
        objs[g.canonical(tag)].append(tag)
    zt = names_mod.zh_tags(work.cache / 'zh-tags/character.csv')

    stat = collections.Counter()
    matched = {}
    how_of = {}
    for canon, tags in objs.items():
        cw = work_of(canon) or next((work_of(t) for t in tags if work_of(t)), None)
        dn = set().union(*(danbooru_names(t) for t in [canon, *tags]))
        hits = set()
        if cw:
            sids = subjects_of(cw)
            if sids:
                stat['作品对上了 Bangumi 条目'] += 1
                pool = set().union(*(chars_of[s] for s in sids))
                hits = {cid for cid in pool if any(bangumi.norm(n) in dn for n in characters[cid]['names'])}
                if len(hits) == 1:
                    stat['作品内唯一对上'] += 1
                elif len(hits) > 1:
                    stat['作品内对上多个'] += 1
            else:
                stat['作品没对上条目'] += 1
        else:
            stat['认不出作品'] += 1
        how = 'work'
        if len(hits) != 1:
            glob = set().union(*(char_by_name.get(n, set()) for n in dn))
            if len(glob) == 1:
                hits = glob
                how = 'global'
                stat['退到全库按名字唯一对上'] += 1
        if len(hits) == 1:
            cid = next(iter(hits))
            matched[canon] = cid
            how_of[canon] = how
            if characters[cid]['zh']:
                stat['对上且有简体中文名'] += 1
    total = len(objs)
    print('目录对象', total)
    for k, v in stat.items():
        print(f'  {k}\t{v}\t{v / total:.1%}')
    # 和 zh-tags 比
    agree = differ = 0
    ex = []
    for canon, cid in matched.items():
        zh = characters[cid]['zh']
        row = zt.get(canon)
        if not zh or not row:
            continue
        a = bangumi.norm(names_mod.zhtext.strip_qualifiers(row['zh'])[0])
        if a == bangumi.norm(zh):
            agree += 1
        else:
            differ += 1
            ex.append((canon, row['zh'], zh, characters[cid]['name']))
    print('Bangumi 中文名与 zh-tags 首选名一致', agree, '不一致', differ)
    by_how = collections.Counter()
    for canon, cid in matched.items():
        zh = characters[cid]['zh']
        row = zt.get(canon)
        if zh and row:
            same = bangumi.norm(names_mod.zhtext.strip_qualifiers(row['zh'])[0]) == bangumi.norm(zh)
            by_how[(how_of[canon], same)] += 1
    print('按对法分的一致/不一致', dict(by_how))
    # 没有确定性线索（D 站别名里没有中文、Wikidata 没有中文、日文名不是纯汉字）的角色，Bangumi 能补多少
    zt_none = 0
    gap = collections.Counter()
    for canon, tags in objs.items():
        page = next((wiki[t] for t in [canon, *tags] if t in wiki), {})
        on = page.get('other_names') or []
        zh_c = [n for n in on if names_mod.zhtext.han(n) and not names_mod.zhtext.kana(n) and not names_mod.zhtext.hangul(n)
                and names_mod.zhtext.gb2312(names_mod.zhtext.strip_qualifiers(n)[0].replace('·', ''))]
        ja = on[0] if on and (names_mod.zhtext.kana(on[0]) or names_mod.zhtext.han(on[0])) else None
        if zh_c or (ja and not names_mod.zhtext.kana(ja)):
            continue
        gap['没有确定性线索'] += 1
        cid = matched.get(canon)
        if cid and characters[cid]['zh']:
            gap['Bangumi 补上（' + how_of[canon] + '）'] += 1
    print('确定性线索之外', dict(gap))
    glob = [(c, characters[matched[c]]['name'], characters[matched[c]]['zh']) for c in matched if how_of[c] == 'global']
    random.Random(3).shuffle(glob)
    print('全库按名字对上的抽样', glob[:25])
    rnd = random.Random(7)
    for e in rnd.sample(ex, min(20, len(ex))):
        print('   ', e)

    # 身份关系的旁证：D 站蕴含给出的角色对，在 Bangumi 里是不是同一个条目、或者有「形态」「进化/前身」关系。
    rel_types = {2001: '形态', 2027: '进化', 2028: '前身', 2020: '改编', 2021: '原型', 2007: '成员'}
    rels = collections.defaultdict(set)
    for a, t, b in bangumi_relations:
        rels[(a, b)].add(t)
        rels[(b, a)].add(t)
    recs = json.load(open(work.root / 'relations.json', encoding='utf-8'))
    tally = collections.defaultdict(collections.Counter)
    for tag, rec in recs.items():
        for parent, r in rec.get('relations', {}).items():
            a, b = matched.get(g.canonical(tag)), matched.get(g.canonical(parent))
            if a is None or b is None:
                tally[r['kind']]['有一边没对上 Bangumi'] += 1
            elif a == b:
                tally[r['kind']]['Bangumi 是同一个条目'] += 1
            elif rels.get((a, b)):
                for t in rels[(a, b)]:
                    tally[r['kind']][f'Bangumi 关系：{rel_types.get(t, t)}'] += 1
            else:
                tally[r['kind']]['两个条目、没有关系'] += 1
    for kind, c in tally.items():
        print(f'== D 站蕴含判为 {kind}：', dict(c.most_common()))


if __name__ == '__main__':
    main()
