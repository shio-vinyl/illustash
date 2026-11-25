"""把目录对象对上 Bangumi 角色（#832、#837 制作端）。

先认作品，再在作品里认人；作品认不出、或者作品里对不上的，才在全库里按名字找，而且要有中文名的旁证。

- D 站作品：tag 末尾括号（经别名换成规范作品 tag）→ 基础索引给的作品 → wiki 第一段里第一个作品链接；
- D 站作品 → Bangumi 条目：作品 wiki 的 `other_names`、可读名，跟条目原名、中文名比，可以对上好几个条目
  （同一作品的游戏、动画、漫画各是一个条目）；
- 作品里认人：D 站 wiki 的 `other_names` 和可读名，跟 Bangumi 角色的原名、日文名、英文名、罗马字、中文名比，
  同作品共用的未核实别名及明显合称不参与匹配；对上几个时，用非拉丁字母的名字再筛一次；
- 全库：只用非拉丁字母、至少两个字的名字，全库唯一、没有已知作品冲突，并且 Bangumi 的中文名和 zh-tags、Wikidata、
  D 站别名里的中文至少一处一致。

每条结果记下对法（`work`、`work-strict`、`global`）和用来对上的名字，供审核和抽查。
"""
import collections
import re

from tools.character_catalog import bangumi, names, tagparse, zhtext

LINK = re.compile(r'\[\[([^\]|]+)(?:\|[^\]]*)?\]\]')
SPLIT = re.compile(r'\s*[／/]\s*')
GROUP_NAME = re.compile(r'(?:チーム|[\s_]team)$', re.I)
# 这些作品的条目原名可能是国际服标题或繁体标题，不能只凭标题两栏是否相同判别。
CHINESE_ORIGINALS = {
    'arknights', 'azur_lane', 'genshin_impact', "girls'_frontline", 'honkai:_star_rail', 'honkai_impact_3rd',
    'nu_carnival', 'path_to_nowhere', 'punishing:_gray_raven', 'reverse:1999', 'to_be_hero_x',
    'warship_girls_r', 'wuthering_waves', 'zenless_zone_zero',
}


def personal(name):
    """合称不能拿来定位成员。去掉作品限定词后检查末尾。"""
    base, _ = zhtext.strip_qualifiers(name)
    return bool(base and not names.groupish(base) and not GROUP_NAME.search(base))


def latin_only(text):
    return not (zhtext.han(text) or zhtext.kana(text) or zhtext.hangul(text))


class Matcher:
    def __init__(self, characters, subjects, links, wiki, alias, copyrights, scope, zh_tags, wikidata):
        self.characters, self.subjects, self.wiki, self.alias = characters, subjects, wiki, alias
        self.links = links
        self.copyrights, self.scope, self.zh_tags, self.wikidata = copyrights, scope, zh_tags, wikidata
        self.subj_by_name = collections.defaultdict(set)
        for sid, s in subjects.items():
            for n in (s['name'], s['name_cn']):
                if n:
                    self.subj_by_name[bangumi.norm(n)].add(sid)
        self.known_subjects = set()
        for work in copyrights:
            self.known_subjects.update(self.subjects_of(work))
        self.chars_of = collections.defaultdict(set)
        chinese_subjects = {sid for sid, s in subjects.items()
                            if zhtext.han(s['name']) and not zhtext.kana(s['name']) and not zhtext.hangul(s['name'])
                            and bangumi.norm(s['name']) == bangumi.norm(s['name_cn'])}
        self.chinese_originals = set()
        for cid, ls in links.items():
            for sid, _ in ls:
                self.chars_of[sid].add(cid)
                if sid in chinese_subjects:
                    self.chinese_originals.add(cid)
        self.keys = {}
        self.by_key = collections.defaultdict(set)
        for cid, c in characters.items():
            labels = set(c['names'])
            for value in (c['name'], c['zh']):
                labels.update(SPLIT.split(value))
            keys = {(bangumi.norm(n), latin_only(n)) for n in labels if personal(n) and len(bangumi.norm(n)) >= 2}
            self.keys[cid] = keys
            for k, is_latin in keys:
                if not is_latin:
                    self.by_key[k].add(cid)

        self.shared_names = set()
        owners = {}
        for tag in scope:
            canon = alias.get(tag, tag)
            base, _ = tagparse.split(canon)
            work = self.work_of([canon, tag])
            if not work:
                continue
            for name in self.wiki_names([tag]):
                key = (work, bangumi.norm(zhtext.strip_qualifiers(name)[0]))
                prior = owners.setdefault(key, base)
                if prior != base:
                    self.shared_names.add(key)

    # ---- D 站一侧 ----
    def copyright(self, name):
        name = name if name in self.copyrights else self.alias.get(name, name)
        return name if name in self.copyrights else None

    def work_of(self, tags):
        for tag in tags:
            _, qs = tagparse.split(tag)
            if qs and (work := self.copyright(qs[-1])):
                return work
        return next(self.work_clues(tags), None)

    def work_clues(self, tags):
        for tag in tags:
            entry = self.scope.get(tag)
            for c in (entry.copyrights if entry else []):
                if (work := self.copyright(c)):
                    yield work
        for tag in tags:
            body = (self.wiki.get(tag) or {}).get('body') or ''
            for m in LINK.finditer(body.replace('\r', '').split('\n\n', 1)[0]):
                c = self.copyright(m.group(1).strip().lower().replace(' ', '_'))
                if c:
                    yield c

    def qualified_works(self, tag):
        _, qualifiers = tagparse.split(tag)
        return {work for qualifier in qualifiers if (work := self.copyright(qualifier))}

    def work_names(self, work):
        names = [tagparse.readable(work), *((self.wiki.get(work) or {}).get('other_names') or [])]
        return {bangumi.norm(name) for name in names}

    def subjects_of(self, work):
        out = set()
        for name in self.work_names(work):
            out |= self.subj_by_name.get(name, set())
        return out

    def wiki_names(self, tags):
        return (n for tag in tags for n in (self.wiki.get(tag) or {}).get('other_names') or [])

    def personal_wiki_names(self, tags, work):
        for name in self.wiki_names(tags):
            key = bangumi.norm(zhtext.strip_qualifiers(name)[0])
            if personal(name) and (work, key) not in self.shared_names:
                yield name

    def names_of(self, tags, work):
        out = {(bangumi.norm(n), latin_only(n)) for n in self.personal_wiki_names(tags, work)
               if len(bangumi.norm(n)) >= 2}
        for tag in tags:
            base = tagparse.readable(tag)
            if len(bangumi.norm(base)) >= 4:
                out.add((bangumi.norm(base), latin_only(base)))
        return out

    def chinese_evidence(self, tags, strong_only=False):
        """zh-tags、Wikidata 的中文（strong），加上 D 站别名里的中文（比较用的键）。

        D 站别名里绰号、合称混在一起（#830），所以只当弱旁证。
        """
        out = set()
        for tag in tags:
            row = self.zh_tags.get(tag)
            if row and row['zh']:
                out.add(bangumi.norm(zhtext.strip_qualifiers(row['zh'])[0]))
            w = self.wikidata.get(tag) or {}
            for f in ('zh_hans', 'zh_cn', 'zh'):
                if w.get(f):
                    out.add(bangumi.norm(w[f]))
        if not strong_only:
            for n in self.personal_wiki_names(tags, self.work_of(tags)):
                if zhtext.han(n) and not zhtext.kana(n) and not zhtext.hangul(n):
                    out.add(bangumi.norm(zhtext.strip_qualifiers(n)[0]))
        return out

    # ---- 对上 ----
    def linked_subjects(self, cid):
        return {sid for sid, _ in self.links.get(cid, ())}

    def work_conflicts(self, tags, work, expected, linked):
        # 两侧都要有已知作品。用角色页和索引里的具体子作交叉核对，系列总标签的
        # 精确标题未收齐时也可以沿用已有旁证；这里不建立新的对应关系。
        if work == 'original' or not expected or not linked or not linked <= self.known_subjects or linked & expected:
            return False
        if any(self.subjects_of(other) & linked for other in self.work_clues(tags)):
            return False
        names = {name for name in self.work_names(work) if len(name) >= 4}
        return not any(name in bangumi.norm(self.subjects[sid][field])
                       for sid in linked for field in ('name', 'name_cn') for name in names)

    def match(self, tags, review=None):
        """tags 的第一个是规范名。返回 dict 或 None：{'id', 'how', 'via', 'work'}。"""
        work = self.work_of(tags)
        mine = self.names_of(tags, work)
        sids = self.subjects_of(work) if work else set()
        if sids:
            pool = set().union(*(self.chars_of[s] for s in sids))
            hits = {cid for cid in pool if self.keys[cid] & mine}
            how = 'work'
            if len(hits) > 1:
                strict = {k for k, latin in mine if not latin}
                hits = {cid for cid in hits if any(k in strict for k, _ in self.keys[cid])}
                how = 'work-strict'
            if len(hits) == 1:
                cid = next(iter(hits))
                return {'id': cid, 'how': how, 'work': work, 'via': sorted(k for k, _ in self.keys[cid] & mine)[:3]}
        glob = set()
        for k, latin in mine:
            if not latin:
                glob |= self.by_key.get(k, set())
        if len(glob) == 1:
            cid = next(iter(glob))
            c = self.characters[cid]
            zh = {bangumi.norm(x) for x in SPLIT.split(c['zh']) if x} if c['zh'] else set()
            if personal(c['name']) and zh & self.chinese_evidence(tags):
                linked = self.linked_subjects(cid)
                if self.work_conflicts(tags, work, sids, linked):
                    if review is not None:
                        review.add(('全库命中与已知作品冲突', tags[0], str(cid)))
                    return None
                return {'id': cid, 'how': 'global', 'work': work,
                        'via': sorted(k for k, _ in self.keys[cid] & mine)[:3]}
        return None

    def pick(self, cid, tags):
        """取名提示：'' 用「简体中文名」；'name' 用原名；'p0'、'p1' 取「A／B」的第几段，'?' 是分不出哪段。

        国产作品的原名就是中文名，「简体中文名」常空着或者写的是别名（雷电将军那栏写的是巴尔泽布）：
        有原作语言证据时取可用的原名；日文汉字原名与已有中文名冲突时，保留信息框的中文名。
        """
        c = self.characters[cid]
        raw, name = c['raw_zh'], c['name']
        usable = all(zhtext.simplified_usable(part) for part in SPLIT.split(name))
        field, text = '', raw
        strong = self.chinese_evidence(tags, strong_only=True)
        work = self.work_of(tags)
        original_chinese = work in CHINESE_ORIGINALS or cid in self.chinese_originals
        if usable and bangumi.norm(name) != bangumi.norm(raw) and (
            (original_chinese and (bangumi.norm(raw) not in strong or bangumi.norm(name) in strong))
            or (not raw and bangumi.norm(name) in strong)
        ):
            field, text = 'name', name
        parts = SPLIT.split(text) if text else []
        if len(parts) > 1:
            # 英文/日文等字段同为逐段写法时，规范 tag 与单段全名的精确对应最直接。
            aligned = [SPLIT.split(label) for label in (name, *c.get('labelled', {}).values())]
            aligned = [labels for labels in aligned if len(labels) == len(parts)]
            keys = [{bangumi.norm(part), *(bangumi.norm(labels[i]) for labels in aligned)}
                    for i, part in enumerate(parts)]
            literal = {bangumi.norm(tagparse.readable(tags[0]))}
            wiki = {k for k, _ in self.names_of(tags, work)}
            # zh-tags 只在对应角色的 wiki 已经印证了同一段时参与消歧。
            exact_hits = [i for i, labels in enumerate(keys) if labels & literal]
            wiki_hits = [i for i, labels in enumerate(keys) if labels & wiki]
            confirmed_hits = [i for i in wiki_hits if keys[i] & strong]
            for hits in (exact_hits, confirmed_hits, wiki_hits):
                if len(hits) == 1:
                    return f'{field}:p{hits[0]}' if field else f'p{hits[0]}'
            return '?'
        return field

    @staticmethod
    def chosen(c, hint):
        """按提示取出中文名（应用里同样的规则）；'?' 取不出。"""
        if hint == '?':
            return ''
        field, _, part = hint.partition(':') if ':' in hint else (('name', '', '') if hint == 'name' else ('', '', hint))
        text = c['name'] if field == 'name' else c['raw_zh']
        if part.startswith('p'):
            parts = SPLIT.split(text)
            i = int(part[1:])
            return parts[i] if i < len(parts) else ''
        return text
