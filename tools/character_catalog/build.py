"""生成角色目录（#837；格式见下，应用读取见 docs/TERMS_CHARACTERS.md「角色目录」）。

    python3 -I tools/character_catalog/build.py 工作目录 Bangumi导出.zip 输出.json [--previous 上一版.json]

输入都在工作目录里（见 work.py）：四份识别来源、D 站缓存、Wikidata、zh-tags（只当核对旁证）；Bangumi 整库导出
另给路径。输出一个 JSON，一个角色一行，方便看差异：

    {"format": "illustash-character-catalog/1", "revision": 1, "built": "2026-11-23", "sources": {…},
     "columns": ["id", "tag", "tags", "parent", "relation", "action", "variant", "bangumi", "pick", "wikidata"],
     "characters": [[…], …], "works": [["azur_lane", 12345], …]}

- `id`：公共目录 ID，正整数，发布过的不再改、不再复用（`--previous` 读上一版沿用）；
- `tag`：D 站规范 tag；`tags`：映射到这个角色的其他识别 tag（多是 D 站改名前的旧 tag）；
- `parent`、`relation`、`action`：上级的目录 ID，关系是 `appearance`（造型）、`form`（另一种形态）或
  `unknown`（有关系、判断不出种类）；`action` 是 `auto`（自动挂到上级下面）或 `ask`（整对问用户）。
  造型的上级沿造型链走到头，所以上级不会是另一个自动挂的造型；
- `variant`：造型名（D 站限定词，如 `swimsuit`）；
- `bangumi`、`pick`：Bangumi 角色 ID 和取名提示（'' 用「简体中文名」，`name` 用原名，`p0`/`p1` 取「A／B」的
  第几段，前面可以带 `name:`）。名字本身不进目录，应用按 ID 在线取；
- `wikidata`：Wikidata 条目号；
- `works`：D 站作品 tag 对应的 Bangumi 条目 ID。

D 站数据（tag、别名、蕴含）是事实数据；wiki 正文、Bangumi 和 zh-tags 的内容都不进目录。
"""
import argparse
import collections
import hashlib
import json
from pathlib import Path
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from tools.character_catalog import (  # noqa: E402
    bangumi, graph as graph_mod, match_bangumi, names as names_mod, relations, tagparse, work as work_mod,
)

FORMAT = 'illustash-character-catalog/1'
COLUMNS = ['id', 'tag', 'tags', 'parent', 'relation', 'action', 'variant', 'bangumi', 'pick', 'wikidata']
ROOT = Path(__file__).resolve().parents[2]


def sha256(path):
    h = hashlib.sha256()
    with open(path, 'rb') as fh:
        for block in iter(lambda: fh.read(1 << 20), b''):
            h.update(block)
    return h.hexdigest()


def variant_of(child, parent):
    extra = tagparse.diff(child, parent)
    if extra:
        return '_'.join(extra)
    cb, _ = tagparse.split(child)
    pb, _ = tagparse.split(parent)
    if cb.startswith(pb + '_'):
        return cb[len(pb) + 1:]
    return cb


def load(work):
    tags = work.danbooru('tags')
    aliases = work.danbooru('aliases_from') + work.danbooru('aliases_to')
    implications = work.danbooru('implications_from') + work.danbooru('implications_to')
    wiki = {r['title']: r for r in work.danbooru('wiki') if not r.get('is_deleted')}
    return graph_mod.Graph(tags, aliases, implications), wiki


def build(work, dump, previous=None):
    g, wiki = load(work)
    scope = work.scope
    alias = {r['antecedent_name']: r['consequent_name'] for r in work.danbooru('aliases_from')}
    copyrights = {}
    for line in open(work.cache / 'danbooru/copyrights.jsonl', encoding='utf-8'):
        for r in json.loads(line)['rows']:
            copyrights[r['name']] = r['post_count']
    zt = names_mod.zh_tags(work.cache / 'zh-tags/character.csv')
    wd = names_mod.wikidata(work.cache / 'wikidata/p13236.json')
    characters, subjects, links, _ = bangumi.load(dump)
    matcher = match_bangumi.Matcher(characters, subjects, links, wiki, alias, copyrights, scope, zt, wd)

    # 目录对象：规范 tag → 映射到它的范围内 tag。
    objects = collections.defaultdict(list)
    for tag in sorted(scope):
        objects[g.canonical(tag)].append(tag)
    recs = relations.analyze(scope, g, wiki)
    parent_of = {}
    for tag, rec in recs.items():
        canon = rec['canonical']
        for parent, r in rec.get('relations', {}).items():
            parent_of[canon] = (parent, r['kind'])
    for parent, _ in list(parent_of.values()):
        objects.setdefault(parent, [])

    def root(canon):
        """沿自动挂的造型链往上走到头。"""
        seen = set()
        while canon in parent_of and parent_of[canon][1] == 'appearance' and canon not in seen:
            seen.add(canon)
            canon = parent_of[canon][0]
        return canon

    old = {}
    if previous:
        prev = json.loads(Path(previous).read_text(encoding='utf-8'))
        cols = prev['columns']
        for row in prev['characters']:
            r = dict(zip(cols, row))
            old[r['tag']] = r['id']
            for t in r['tags']:
                old.setdefault(t, r['id'])
    next_id = max(old.values(), default=0) + 1
    ids = {}
    for canon in sorted(objects):
        known = old.get(canon) or next((old[t] for t in objects[canon] if t in old), None)
        if known:
            ids[canon] = known
        else:
            ids[canon] = next_id
            next_id += 1

    rows, stats, work_hits = [], collections.Counter(), collections.defaultdict(collections.Counter)
    review = []
    matches = {}
    for canon in sorted(objects, key=lambda c: ids[c]):
        tags = objects[canon]
        names = [canon, *[t for t in tags if t != canon]]
        m = matcher.match(names)
        if m:
            matches[canon] = m
    for canon in sorted(objects, key=lambda c: ids[c]):
        tags = objects[canon]
        parent = relation = action = variant = None
        if canon in parent_of:
            p, kind = parent_of[canon]
            if kind == 'appearance':
                parent, relation, action = root(p), 'appearance', 'auto'
                variant = variant_of(canon, p)
            else:
                parent, relation, action = p, kind, 'ask'
        m = matches.get(canon)
        bid = pick = None
        if m:
            # 自动挂的造型显示的是上级的名字加造型名，不另记 Bangumi。
            if action != 'auto':
                bid = m['id']
                pick = matcher.pick(m['id'], [canon, *tags])
                if pick == '?':
                    review.append(('分不出取哪段', canon, characters[m['id']]['zh']))
                    bid = pick = None
            stats['bangumi:' + m['how']] += 1
            if m['work']:
                for sid, _ in links.get(m['id'], []):
                    work_hits[m['work']][sid] += 1
        qid = None
        for t in [canon, *tags]:
            if t in wd:
                qid = wd[t]['item'].rsplit('/', 1)[-1]
                break
        rows.append([ids[canon], canon, [t for t in tags if t != canon], ids.get(parent), relation, action, variant,
                     bid, pick, qid])
        stats['objects'] += 1
        stats['with_bangumi'] += bid is not None
        stats[f'relation:{relation}'] += relation is not None
    works = []
    for work_tag, counter in sorted(work_hits.items()):
        sid, n = counter.most_common(1)[0]
        if n >= 2:
            works.append([work_tag, sid])
    return rows, works, stats, review


def sources(work, dump):
    stamps = []
    for kind in ('tags', 'aliases_from', 'aliases_to', 'implications_from', 'implications_to', 'wiki'):
        path = work.cache / 'danbooru' / f'{kind}.jsonl'
        with open(path, encoding='utf-8') as fh:
            for line in fh:
                stamps.append(json.loads(line)['at'])
    inputs = json.loads((work.root / 'inputs.json').read_text(encoding='utf-8'))
    return {
        'danbooru': {'fetched': [min(stamps), max(stamps)]},
        'bangumi': {'dump': Path(dump).name, 'sha256': sha256(dump)},
        'wikidata': {'property': 'P13236', 'fetched': (work.cache / 'wikidata/FETCHED').read_text().strip()},
        'scope': {name: sha256(path) for name, path in {**inputs, 'wd_tags': ROOT / 'tags/tags_v3.csv'}.items()},
    }


def write(path, header, rows, works):
    lines = ['{' + ', '.join(f'{json.dumps(k)}: {json.dumps(v, ensure_ascii=False)}' for k, v in header.items()) + ',',
             '"characters": [']
    lines += [json.dumps(r, ensure_ascii=False, separators=(',', ':')) + (',' if i < len(rows) - 1 else '')
              for i, r in enumerate(rows)]
    lines += ['],', '"works": [']
    lines += [json.dumps(w, ensure_ascii=False, separators=(',', ':')) + (',' if i < len(works) - 1 else '')
              for i, w in enumerate(works)]
    lines += [']}']
    Path(path).write_text('\n'.join(lines) + '\n', encoding='utf-8')
    json.loads(Path(path).read_text(encoding='utf-8'))


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument('work', type=Path)
    p.add_argument('dump', type=Path)
    p.add_argument('output', type=Path)
    p.add_argument('--previous', type=Path)
    p.add_argument('--revision', type=int, default=1)
    p.add_argument('--review', type=Path, help='把要人工看的条目写到这里（不进仓库）')
    args = p.parse_args(argv)
    work = work_mod.open_work(args.work)
    rows, works, stats, review = build(work, args.dump, args.previous)
    header = {'format': FORMAT, 'revision': args.revision, 'built': time.strftime('%Y-%m-%d'),
              'sources': sources(work, args.dump), 'columns': COLUMNS}
    write(args.output, header, rows, works)
    for k, v in sorted(stats.items()):
        print(f'{k}\t{v}')
    print('works', len(works))
    if args.review:
        args.review.write_text('\n'.join('\t'.join(map(str, r)) for r in review) + '\n', encoding='utf-8')


if __name__ == '__main__':
    main()
