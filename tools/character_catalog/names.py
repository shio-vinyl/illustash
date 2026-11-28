"""名称包的候选和校对（#832 制作端）。

先认实体，再挑译名：目录对象按 D 站规范 tag 认（`graph.canonical`），每个对象从三处收名字候选：

- `zh-tags`：amenorira/danbooru-tags-data-zh 的 `zh`（首选中文名）和 `aliases`（语言混杂，按文字分类）；
- `wiki`：D 站 wiki 的 `other_names`（不分语言、不保证第一项是本名，只当候选和旁证）；
- `wikidata`：P13236（Danbooru tag）对上的条目的 zh-hans / zh-cn / zh / ja / en 标签。

中文首选名的状态：

- `reviewed`：zh-tags 的首选名通过排除规则，并且至少有一条独立旁证（Wikidata 中文标签一致，或和日文原名同形，
  或出现在本角色的 D 站 wiki 别名里）；
- `single`：只有 zh-tags 一个来源，排除规则也通过；
- `conflict`：同一作品里有别的角色用同一个首选名，或者像合称、团体名；
- 没有候选的不写。

日文名：D 站 wiki 别名第一项含假名或汉字、且和 Wikidata 日文标签或 zh-tags 别名里的日文一致时算 `reviewed`，
只有 wiki 一处时算 `single`。英文名只收 Wikidata 的英文标签（可读 tag 由应用自己生成，不进包）。
"""
import collections
import csv
import json
import re

from tools.character_catalog import tagparse, zhtext

# 合称、团体名的字样（#830）。只在不止一个角色共用、或者整名以它结尾时才算数，避免误伤本名里带这些字的角色。
GROUP_MARKS = ('双子', '雙子', '组合', '組合', '二人组', '二人組', '三人组', '三人組', '姐妹', '兄妹', '兄弟', '姉妹',
               '夫妇', '夫婦', 'コンビ', 'ユニット', 'トリオ', '小队', '小隊', '队', '隊', '团', '團', 'CP')


def zh_tags(path):
    rows = {}
    with open(path, encoding='utf-8-sig', newline='') as fh:
        for row in csv.DictReader(fh):
            rows[row['tag']] = row
    return rows


def wikidata(path):
    data = json.loads(path.read_text(encoding='utf-8'))
    return {r['tag']['value']: {k: v['value'] for k, v in r.items()} for r in data['results']['bindings']}


def groupish(name):
    base, _ = zhtext.strip_qualifiers(name)
    return any(base.endswith(m) for m in GROUP_MARKS)


def work_key(tag, copyrights):
    """同作品判断用的键：基础索引给的作品，没有就用 tag 最后一个括号。"""
    if copyrights:
        return copyrights[0]
    _, quals = tagparse.split(tag)
    return quals[-1] if quals else ''


def candidates(canon, tags, zt, wiki, wd):
    """一个目录对象的名字候选（不判断状态）。tags 是映射到它的全部 tag（含规范名）。"""
    out = {'zh': [], 'zh_aliases': [], 'ja': [], 'en': [], 'wiki': [], 'wikidata': {}}
    for t in [canon, *[x for x in tags if x != canon]]:
        row = zt.get(t)
        if row and not out['zh']:
            if row['zh'].strip():
                out['zh'].append(row['zh'].strip())
            out['zh_aliases'] = [a for a in row['aliases'].split('|') if a.strip()]
        page = wiki.get(t)
        if page and not out['wiki'] and not page.get('is_deleted'):
            out['wiki'] = list(page.get('other_names') or [])
        if t in wd and not out['wikidata']:
            out['wikidata'] = wd[t]
    return out


def zh_corroboration(zh, cand):
    """zh-tags 首选名的独立旁证，返回说明列表。"""
    k = zhtext.key(zh)
    why = []
    w = cand['wikidata']
    for field in ('zh_hans', 'zh_cn', 'zh'):
        if w.get(field) and zhtext.key(w[field]) == k:
            why.append(f'wikidata:{field}')
            break
    if any(zhtext.key(n) == k for n in cand['wiki']):
        why.append('wiki')
    ja = [n for n in cand['wiki'][:1] if zhtext.han(n)] + ([w['ja']] if w.get('ja') else [])
    if any(not zhtext.kana(n) and zhtext.key(n) == k for n in ja):
        why.append('ja-same')
    return why


def ja_name(cand):
    first = cand['wiki'][:1]
    w = cand['wikidata'].get('ja')
    aliases_ja = [a for a in cand['zh_aliases'] if zhtext.kana(a)]
    if first and (zhtext.kana(first[0]) or (zhtext.han(first[0]) and zhtext.cp932(first[0].replace('・', '')))):
        name = first[0]
        agree = (w and zhtext.key(w) == zhtext.key(name)) or any(zhtext.key(a) == zhtext.key(name) for a in aliases_ja)
        return name, 'reviewed' if agree else 'single'
    if w and (zhtext.kana(w) or zhtext.han(w)):
        return w, 'single'
    return None, None


def build(objects, zt, wiki, wd, works, family=None):
    """objects: {canon: [tags…]}；works: {canon: 作品键}；family: {canon: 家族根}（本体、造型、形态、改名算一家，
    一家之内共用名字不算冲突）。返回 {canon: 名字记录}。"""
    family = family or {}
    recs = {}
    for canon, tags in objects.items():
        cand = candidates(canon, tags, zt, wiki, wd)
        rec = {'zh': None, 'zh_status': None, 'zh_why': [], 'ja': None, 'ja_status': None, 'en': None}
        if cand['zh']:
            zh = cand['zh'][0]
            base, quals = zhtext.strip_qualifiers(zh)
            rec['zh_full'] = zh
            rec['zh'] = base
            rec['zh_quals'] = quals
            rec['zh_why'] = zh_corroboration(zh, cand)
            rec['zh_status'] = 'reviewed' if rec['zh_why'] else 'single'
            if groupish(zh):
                rec['zh_status'] = 'conflict'
                rec['zh_why'].append('像合称或团体名')
        rec['ja'], rec['ja_status'] = ja_name(cand)
        if cand['wikidata'].get('en'):
            rec['en'] = cand['wikidata']['en']
        rec['aliases'] = sorted({*cand['zh_aliases'], *cand['wiki']} - {rec['zh'] or '', rec['ja'] or ''})
        recs[canon] = rec
    # 同一作品里共用首选中文名：都标成冲突（本体和自己的造型共名不算）。
    by = collections.defaultdict(list)
    for canon, rec in recs.items():
        if rec['zh']:
            by[(works.get(canon, ''), zhtext.key(rec['zh']))].append(canon)
    for (_, _), canons in by.items():
        roots = {family.get(c, c) for c in canons}
        if len(roots) > 1:
            for canon in canons:
                others = [c for c in canons if family.get(c, c) != family.get(canon, canon)]
                recs[canon]['zh_status'] = 'conflict'
                recs[canon]['zh_why'].append('同作品共用：' + ','.join(others)[:200])
    return recs
