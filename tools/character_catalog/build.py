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
import itertools
import json
from pathlib import Path
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from tools.character_catalog import (  # noqa: E402
    bangumi, graph as graph_mod, match_bangumi, names as names_mod, relations, reviewed, tagparse, work as work_mod,
)

FORMAT = 'illustash-character-catalog/1'
COLUMNS = ['id', 'tag', 'tags', 'parent', 'relation', 'action', 'variant', 'bangumi', 'pick', 'wikidata', 'group']
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


def assign_ids(objects, previous):
    """规范 tag 优先沿用原 ID。别名拆出时分配新 ID，消失的最高号也永不复用。"""
    old_tags, canonical = {}, {}
    for row in previous.get('characters', []):
        r = dict(zip(previous['columns'], row, strict=True))
        canonical[r['tag']] = r['id']
        for tag in [r['tag'], *r['tags']]:
            old_tags[tag] = r['id']
    high_water = max(previous.get('id_high_water', 0), max(canonical.values(), default=0))
    ids = {tag: canonical[tag] for tag in objects if tag in canonical}
    used = set(ids.values())
    for tag in sorted(objects.keys() - ids.keys()):
        inherited = next((old_tags[t] for t in [tag, *objects[tag]] if t in old_tags and old_tags[t] not in used), None)
        if inherited is None:
            high_water += 1
            inherited = high_water
        ids[tag] = inherited
        used.add(inherited)
    return ids, high_water


def clear_work_conflicts(rows, matcher, parent_of, review):
    """同一个角色 ID 下，互不关联又写明不同作品的规范 tag 只能保留有唯一作品证据的一方。"""
    tag_col = COLUMNS.index('tag')
    id_col = COLUMNS.index('bangumi')
    pick_col = COLUMNS.index('pick')
    by_id = collections.defaultdict(list)
    for row in rows:
        tag, bid = row[tag_col], row[id_col]
        if bid is not None and (works := matcher.qualified_works(tag)):
            by_id[bid].append((row, works))

    def ancestry(tag):
        chain = {tag}
        while tag in parent_of:
            tag = parent_of[tag][0]
            if tag in chain:
                break
            chain.add(tag)
        return chain

    removed = 0
    for bid, group in by_id.items():
        if len(group) < 2:
            continue
        linked = matcher.linked_subjects(bid)

        def evidence(row, works):
            confirmed = reviewed.BANGUMI.get(row[tag_col])
            if confirmed and confirmed[0] == bid:
                return 2
            return int(any(matcher.subjects_of(work) & linked for work in works))

        strengths = [evidence(row, works) for row, works in group]
        ancestors = [ancestry(row[tag_col]) for row, _ in group]
        conflicts = set()
        for a, b in itertools.combinations(range(len(group)), 2):
            if group[a][1] & group[b][1] or ancestors[a] & ancestors[b]:
                continue
            if strengths[a] <= strengths[b]:
                conflicts.add(a)
            if strengths[b] <= strengths[a]:
                conflicts.add(b)
        for i in sorted(conflicts):
            row, _ = group[i]
            review.add(('跨作品共用角色 ID', row[tag_col], str(bid)))
            row[id_col] = row[pick_col] = None
        removed += len(conflicts)
    return removed


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
    review = set()
    for rec in recs.values():
        canon = rec['canonical']
        candidates = rec.get('relations', {})
        if rec.get('prefix'):
            review.add(('仅前缀', canon, rec['prefix']))
        if len(candidates) > 1:
            review.add(('多个候选上级', canon, ','.join(sorted(candidates))))
        if len(candidates) != 1 or canon in reviewed.GROUPS:
            continue
        parent, result = next(iter(candidates.items()))
        if parent in reviewed.GROUPS or (canon, parent) in reviewed.INSTANCES:
            continue
        kind = result['kind']
        paragraph = relations.first_paragraph(wiki.get(canon, {}).get('body', ''))
        added_works = matcher.qualified_works(canon) - matcher.qualified_works(parent)
        if kind == 'appearance' and (added_works or not reviewed.appearance(canon, parent, paragraph)):
            kind = 'unknown'
        if kind != 'appearance':
            review.add((kind, canon, parent))
        parent_of[canon] = (parent, kind)
    for parent, _ in list(parent_of.values()):
        objects.setdefault(parent, [])

    def root(canon):
        """沿自动挂的造型链往上走到头。"""
        seen = set()
        while canon in parent_of and parent_of[canon][1] == 'appearance':
            if canon in seen:
                raise ValueError(f'造型关系成环：{canon}')
            seen.add(canon)
            canon = parent_of[canon][0]
        return canon

    prev = json.loads(Path(previous).read_text(encoding='utf-8')) if previous else {}
    ids, high_water = assign_ids(objects, prev)

    rows, stats, work_hits = [], collections.Counter(), collections.defaultdict(collections.Counter)
    matches = {}
    for canon in sorted(objects, key=lambda c: ids[c]):
        tags = objects[canon]
        names = [canon, *[t for t in tags if t != canon]]
        m = matcher.match(names, review)
        if m and canon not in reviewed.GROUPS:
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
                    review.add(('分不出取哪段', canon, str(m['id'])))
                    bid = pick = None
            stats['bangumi:' + m['how']] += 1
            if m['work']:
                # 只用标题和作品别名能够独立指向的条目；共演、联动、续作都不能单凭票数认作原作。
                subject_ids = matcher.subjects_of(m['work'])
                if len(subject_ids) == 1:
                    sid = next(iter(subject_ids))
                    if any(item == sid for item, _ in links.get(m['id'], [])):
                        work_hits[m['work']][sid] += 1
        if canon in reviewed.BANGUMI:
            bid, subject, pick = reviewed.BANGUMI[canon]
            if bid not in characters or subject not in {sid for sid, _ in links.get(bid, [])}:
                raise ValueError(f'已审核的角色与作品对应发生变化：{canon}')
        qid = None
        for t in [canon, *tags]:
            if t in wd:
                qid = wd[t]['item'].rsplit('/', 1)[-1]
                break
        rows.append([ids[canon], canon, [t for t in tags if t != canon], ids.get(parent), relation, action, variant,
                     bid, pick, qid, canon in reviewed.GROUPS or None])
        stats['objects'] += 1
        stats['with_bangumi'] += bid is not None
        stats[f'relation:{relation}'] += relation is not None
    stats['with_bangumi'] -= clear_work_conflicts(rows, matcher, parent_of, review)
    works = []
    for work_tag, counter in sorted(work_hits.items()):
        sid, n = counter.most_common(1)[0]
        if n >= 2:
            works.append([work_tag, sid])
    return rows, works, stats, sorted(review), high_water


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
    if args.revision < 1:
        p.error('修订号必须是正整数')
    if args.revision > 1 and not args.previous:
        p.error('后续修订需要 --previous 来延续公共 ID')
    if args.previous and json.loads(args.previous.read_bytes())['revision'] >= args.revision:
        p.error('修订号必须比上一版大')
    work = work_mod.open_work(args.work)
    rows, works, stats, review, high_water = build(work, args.dump, args.previous)
    header = {'format': FORMAT, 'revision': args.revision, 'built': time.strftime('%Y-%m-%d'),
              'id_high_water': high_water, 'sources': sources(work, args.dump), 'columns': COLUMNS}
    write(args.output, header, rows, works)
    for k, v in sorted(stats.items()):
        print(f'{k}\t{v}')
    print('works', len(works))
    if args.review:
        args.review.write_text('\n'.join('\t'.join(map(str, r)) for r in review) + '\n', encoding='utf-8')


if __name__ == '__main__':
    main()
