"""把目录对象对上 Bangumi 角色（#832、#837 制作端）。

先认作品，再在作品里认人；作品认不出、或者作品里对不上的，才在全库里按名字找，而且要有中文名的旁证。

- D 站作品：tag 末尾括号（经别名换成规范作品 tag）→ 基础索引给的作品 → wiki 第一段里第一个作品链接；
- D 站作品 → Bangumi 条目：作品 wiki 的 `other_names`、可读名，跟条目原名、中文名比，可以对上好几个条目
  （同一作品的游戏、动画、漫画各是一个条目）；
- 作品里认人：D 站 wiki 的 `other_names` 和可读名，跟 Bangumi 角色的原名、日文名、英文名、罗马字、中文名比，
  同一作品里只对上一个才算；对上几个时，只用非拉丁字母的名字再筛一次；
- 全库：只用非拉丁字母、至少两个字的名字，全库唯一，并且 Bangumi 的中文名和 zh-tags、Wikidata、
  D 站别名里的中文至少一处一致。

每条结果记下对法（`work`、`work-strict`、`global`）和用来对上的名字，供审核和抽查。
"""
import collections
import re

from tools.character_catalog import bangumi, tagparse, zhtext

LINK = re.compile(r'\[\[([^\]|]+)(?:\|[^\]]*)?\]\]')
SPLIT = re.compile(r'\s*[／/]\s*')


def latin_only(text):
    return not (zhtext.han(text) or zhtext.kana(text) or zhtext.hangul(text))


class Matcher:
    def __init__(self, characters, subjects, links, wiki, alias, copyrights, scope, zh_tags, wikidata):
        self.characters, self.subjects, self.wiki, self.alias = characters, subjects, wiki, alias
        self.copyrights, self.scope, self.zh_tags, self.wikidata = copyrights, scope, zh_tags, wikidata
        self.subj_by_name = collections.defaultdict(set)
        for sid, s in subjects.items():
            for n in (s['name'], s['name_cn']):
                if n:
                    self.subj_by_name[bangumi.norm(n)].add(sid)
        self.chars_of = collections.defaultdict(set)
        for cid, ls in links.items():
            for sid, _ in ls:
                self.chars_of[sid].add(cid)
        self.keys = {}
        self.by_key = collections.defaultdict(set)
        for cid, c in characters.items():
            names = set(c['names'])
            if c['zh']:
                names |= set(SPLIT.split(c['zh']))
            names |= set(SPLIT.split(c['name']))
            keys = {(bangumi.norm(n), latin_only(n)) for n in names if n and len(bangumi.norm(n)) >= 2}
            self.keys[cid] = keys
            for k, is_latin in keys:
                if not is_latin:
                    self.by_key[k].add(cid)

    # ---- D 站一侧 ----
    def copyright(self, name):
        name = name if name in self.copyrights else self.alias.get(name, name)
        return name if name in self.copyrights else None

    def work_of(self, tags):
        for tag in tags:
            _, qs = tagparse.split(tag)
            if qs and self.copyright(qs[-1]):
                return self.copyright(qs[-1])
        for tag in tags:
            entry = self.scope.get(tag)
            for c in (entry.copyrights if entry else []):
                if self.copyright(c):
                    return self.copyright(c)
        for tag in tags:
            body = (self.wiki.get(tag) or {}).get('body') or ''
            for m in LINK.finditer(body.replace('\r', '').split('\n\n', 1)[0]):
                c = self.copyright(m.group(1).strip().lower().replace(' ', '_'))
                if c:
                    return c
        return None

    def subjects_of(self, work):
        names = [tagparse.readable(work)] + list((self.wiki.get(work) or {}).get('other_names') or [])
        out = set()
        for n in names:
            out |= self.subj_by_name.get(bangumi.norm(n), set())
        return out

    def names_of(self, tags):
        out = set()
        for tag in tags:
            for n in (self.wiki.get(tag) or {}).get('other_names') or []:
                if len(bangumi.norm(n)) >= 2:
                    out.add((bangumi.norm(n), latin_only(n)))
            base, _ = tagparse.split(tag)
            readable = base.replace('_', ' ')
            if len(bangumi.norm(readable)) >= 4:
                out.add((bangumi.norm(readable), True))
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
            if strong_only:
                continue
            for n in (self.wiki.get(tag) or {}).get('other_names') or []:
                if zhtext.han(n) and not zhtext.kana(n) and not zhtext.hangul(n):
                    out.add(bangumi.norm(zhtext.strip_qualifiers(n)[0]))
        return out

    # ---- 对上 ----
    def match(self, tags):
        """tags 的第一个是规范名。返回 dict 或 None：{'id', 'how', 'via', 'work', 'subjects'}。"""
        work = self.work_of(tags)
        mine = self.names_of(tags)
        sids = self.subjects_of(work) if work else set()
        if sids:
            pool = set().union(*(self.chars_of[s] for s in sids))
            hits = {cid for cid in pool if self.keys[cid] & mine}
            how = 'work'
            if len(hits) > 1:
                strict = {(k, latin) for k, latin in mine if not latin}
                hits = {cid for cid in hits if {k for k, _ in self.keys[cid]} & {k for k, _ in strict}}
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
            if zh & self.chinese_evidence(tags):
                return {'id': cid, 'how': 'global', 'work': work,
                        'via': sorted(k for k, _ in self.keys[cid] & mine)[:3]}
        return None

    def pick(self, cid, tags):
        """取名提示：'' 用「简体中文名」；'name' 用原名；'p0'、'p1' 取「A／B」的第几段，'?' 是分不出哪段。

        国产作品的原名就是中文名，「简体中文名」常空着或者写的是别名（雷电将军那栏写的是巴尔泽布）：
        「简体中文名」空着、原名能当简体用时取原名；两样都有、原名有旁证而「简体中文名」没有时，也取原名。
        """
        c = self.characters[cid]
        raw, name = c['raw_zh'], c['name']
        usable = zhtext.simplified_usable(name) and '／' not in name and '/' not in name
        field, text = '', raw
        if usable and not raw:
            field, text = 'name', name
        elif usable and raw and bangumi.norm(name) != bangumi.norm(raw):
            # 原名能当简体用、和「简体中文名」不同：默认取原名（国产作品的官方名）；
            # 只有「简体中文名」有强旁证而原名没有时才取「简体中文名」。
            strong = self.chinese_evidence(tags, strong_only=True)
            if not (bangumi.norm(raw) in strong and bangumi.norm(name) not in strong):
                field, text = 'name', name
        parts = SPLIT.split(text) if text else []
        if len(parts) > 1:
            originals = SPLIT.split(name)
            mine = {k for k, _ in self.names_of(tags)}
            for i, part in enumerate(parts):
                o = originals[i] if i < len(originals) else ''
                if bangumi.norm(part) in mine or (o and bangumi.norm(o) in mine):
                    return f'p{i}' if not field else f'{field}:p{i}'
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
