"""研究用：中文名候选的来源对照（zh-tags 与 Wikidata、D 站 wiki 别名）。"""
import collections
import csv
import json

from tools.character_catalog import work as work_mod, zhtext


def zh_tags(work):
    rows = {}
    with open(work.cache / 'zh-tags/character.csv', encoding='utf-8-sig', newline='') as fh:
        for row in csv.DictReader(fh):
            rows[row['tag']] = row
    return rows


def wikidata(work):
    data = json.loads((work.cache / 'wikidata/p13236.json').read_text(encoding='utf-8'))
    out = {}
    for row in data['results']['bindings']:
        out[row['tag']['value']] = {k: v['value'] for k, v in row.items()}
    return out


def main(argv=None):
    args = work_mod.parser(__doc__).parse_args(argv)
    work = work_mod.open_work(args.work)
    zt = zh_tags(work)
    wd = wikidata(work)
    agree = differ = 0
    kinds = collections.Counter()
    samples = collections.defaultdict(list)
    for tag in work.scope:
        if tag not in zt or tag not in wd:
            continue
        w = wd[tag].get('zh_hans') or wd[tag].get('zh_cn')
        if not w:
            continue
        z = zt[tag]['zh']
        if zhtext.key(z) == zhtext.key(w):
            agree += 1
            continue
        differ += 1
        aliases = [a for a in zt[tag]['aliases'].split('|') if a]
        if any(zhtext.key(a) == zhtext.key(w) for a in aliases):
            kind = 'Wikidata 的名字在 zh-tags 别名里'
        elif zhtext.key(w) in zhtext.key(z) or zhtext.key(z) in zhtext.key(w):
            kind = '一个包含另一个'
        else:
            kind = '完全不同'
        kinds[kind] += 1
        if len(samples[kind]) < 12:
            samples[kind].append((tag, z, w))
    print('明确简体的 Wikidata 标签与 zh-tags 首选名：一致', agree, '不一致', differ)
    for kind, n in kinds.most_common():
        print(' ', kind, n)
        for s in samples[kind]:
            print('    ', s)
    # zh-tags 首选名本身的形态
    shape = collections.Counter()
    for tag in work.scope:
        if tag not in zt:
            continue
        z = zt[tag]['zh']
        base, quals = zhtext.strip_qualifiers(z)
        shape['带括号限定词' if quals else '无限定词'] += 1
        if not zhtext.simplified_usable(z):
            shape['不能当简体用（应用口径）'] += 1
    print('zh-tags 首选名形态', dict(shape))


if __name__ == '__main__':
    main()
