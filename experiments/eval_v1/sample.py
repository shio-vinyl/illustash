"""只按公开元数据冻结共用评测集 v1 的取图请求（不看任何识别分数）。

组成（同一位画师在整套里最多一张；帖子、父子家族、MD5、像素摘要都不重复，且与历史取样不相交）：

1. 扩展头正例：#551 的 2,100 头每头最多 4 张「单人单角色」+ 2 张其他（多人、漫画分格等）；#534 的
   205 头要逐头判一级（至少 8 位正例画师），每头最多 8 + 4 张。供给少的头先取。
2. 长尾正例：#557 的 54 个锚点和长尾探查名单里实时复核仍不足 50 帖、首帖在 WD 截止后的角色，每个
   最多 2 张，只取 #557 test 分区的画师（参考图和校准只用另两区，互不相交）。
3. 同作品混淆库（跨头共用）：目标角色主作品里的旧角色单人图。先取外观/服装/形态和某个目标的
   画像相近的，再取同作品其他旧角色，另取同作品的旧角色多人图。按作品配额，作品里目标越多配额越大。
4. 对照：无关作品旧角色、多人旧角色、多人含未覆盖角色、无角色、未覆盖的其他角色。
"""

from collections import Counter, defaultdict
import json

from common import DATA, START, STOP, WD_LAST_ID, load, priority, sha_file, split_557, write
import sampling as base

PER_HEAD = {'2100': {'solo': 4, 'other': 2}, '534_index_covered': {'solo': 8, 'other': 4}}
PER_TAIL = 2
CONTROLS = {'unrelated_old': 900, 'multi_old': 700, 'multi_other': 300, 'no_character': 400,
            'other_character': 400}
CONTROL_PER_CHARACTER = 20


def work_quota(n):
    """作品里目标角色越多，混淆库配额越大；封顶以免几部大作品占满整套。"""
    return {'visual': min(80, 6 + 3 * n), 'other': min(40, 3 + n), 'multi': min(16, 2 + n // 2)}


def family_roots(rows, parents):
    parent = dict(parents)
    for row in rows:
        if row.get('parent_id'):
            parent.setdefault(row['id'], row['parent_id'])

    def find(post):
        trail = []
        while post in parent and post not in trail:
            trail.append(post)
            post = parent[post]
        return post

    return {post: find(post) for post in {*parent, *(r['id'] for r in rows)}}


class Sampler:
    def __init__(self, roots, used):
        self.roots = roots
        self.used = set(used)
        self.selected = []

    def keys(self, row):
        keys = {('id', row['id']), ('artist', row['artist']), ('family', self.roots.get(row['id'], row['id']))}
        keys.update((k, row[k]) for k in ('md5', 'pixel_hash') if row.get(k))
        return keys

    def take(self, rows, count, group, per_character=None, **extra):
        taken, counts = [], Counter()
        # START 以后的帖子优先，早期帖子（只在正例池里）排在后面。
        for row in sorted(rows, key=lambda r: (bool(r.get('early')), priority(r['id']))):
            if len(taken) >= count:
                break
            keys = self.keys(row)
            truth = tuple(row['characters'])
            if keys & self.used or (per_character and truth and counts[truth] >= per_character):
                continue
            self.used.update(keys)
            item = dict(row, group=group, split='test', family=self.roots.get(row['id'], row['id']), **extra)
            self.selected.append(item)
            taken.append(item)
            counts[truth] += 1
        return len(taken)


def main():
    targets = {e['label']: e for e in load(DATA / 'roster/targets.json')}
    covered = set(load(DATA / 'roster/covered.json'))
    prior = load(DATA / 'prior/boundary.json')
    rows = {r['id']: r for r in load(DATA / 'pool/snapshot.json')}
    rows.update({r['id']: r for r in load(DATA / 'pool/live.json')})   # 实时 API 的标签更新
    rows = [r for r in rows.values() if START <= r['created_at'][:10] <= STOP]
    # WD 截止后、START 之前的帖子只给新角色当正例（见 pool.early）；混淆库和对照仍只用 START 以后。
    recent = {r['id'] for r in rows}
    early = [r for r in load(DATA / 'pool/early.json') if r['id'] not in recent]
    parents = {int(k): v for k, v in prior['parents'].items()}
    roots = family_roots([*rows, *early], parents)
    prior_families = {roots.get(p, p) for p in prior['ids']}
    used = {('artist', a) for a in prior['artists']} | {('id', p) for p in prior['ids']}
    used |= {('family', f) for f in prior_families} | {('md5', m) for m in prior['md5']}
    used |= {('pixel_hash', p) for p in prior['pixel_hashes']}

    # 长尾：探查名单经实时复核，按 #557 的口径确认仍不足 50 帖、首帖在 WD 截止后。
    tail_posts = load(DATA / 'pool/live-tail-posts.json')
    tails, tail_states = set(), Counter()
    for name, posts in tail_posts.items():
        public = [p for p in posts if not p['is_deleted'] and not p['is_banned']]
        anchored = targets.get(name, {}).get('route') == 'ccip_longtail'
        if len(posts) >= 100 or len(public) >= 50:
            tail_states['not_below_50'] += 1
            if anchored:
                tails.add(name)   # 已发布锚点照样要评，只是不再算作新扩容候选
        elif not public or min(p['id'] for p in public) <= WD_LAST_ID:
            tail_states['first_post_unverified'] += 1
        else:
            tail_states['eligible'] += 1
            tails.add(name)
    tails |= {n for n, e in targets.items() if e['route'] == 'ccip_longtail'}
    heads = {n for n, e in targets.items() if e['route'] == 'wd_head'}
    news = heads | tails

    # 主作品与外观画像：只用元数据（全部候选帖，含历史取样过的帖子）。
    by_role = defaultdict(list)
    for row in [*rows, *early]:
        for name in news.intersection(row['characters']):
            by_role[name].append(row)
    works, profiles = {}, defaultdict(dict)
    for name in sorted(news):
        own = by_role[name]
        solo = [r for r in own if r['solo'] and r['characters'] == [name]]
        basis = solo or own
        counts = Counter(w for r in basis for w in r['copyrights'] if w != 'original')
        works[name] = sorted(w for w, n in counts.items() if n >= max(1, len(basis) * 0.55))
        tags = Counter(t for r in solo for t in r['visual'])
        profile = {t for t, n in tags.items() if n >= max(2, len(solo) * 0.38)}
        if len(profile) >= 2:
            for work in works[name]:
                profiles[work][name] = profile
    work_targets = defaultdict(set)
    for name, items in works.items():
        for work in items:
            work_targets[work].add(name)

    eligible = [r for r in rows if not (('artist', r['artist']) in used or ('id', r['id']) in used)]
    eligible_early = [r for r in early if not (('artist', r['artist']) in used or ('id', r['id']) in used)]
    sampler = Sampler(roots, used)
    counts = {}
    # 1. 扩展头正例：供给少的先取。
    pool = defaultdict(lambda: {'solo': [], 'other': []})
    for row in [*eligible, *eligible_early]:
        for name in heads.intersection(row['characters']):
            kind = 'solo' if row['solo'] and row['characters'] == [name] else 'other'
            pool[name][kind].append(row)
    for name in sorted(heads, key=lambda n: (len({r['artist'] for k in pool[n].values() for r in k}), n)):
        counts[name] = {kind: sampler.take(pool[name][kind], quota, f'head_{kind}', focus=name)
                        for kind, quota in PER_HEAD[targets[name]['pack']].items()}
    # 2. 长尾正例：只取 #557 test 分区的画师。
    tail_pool = defaultdict(list)
    for row in [*eligible, *eligible_early]:
        if split_557(row['artist']) != 'test':
            continue
        for name in tails.intersection(row['characters']):
            tail_pool[name].append(row)
    for name in sorted(tails, key=lambda n: (len(tail_pool[n]), n)):
        items = sorted(tail_pool[name], key=lambda r: (not (r['solo'] and r['characters'] == [name]),))
        solo = [r for r in items if r['solo'] and r['characters'] == [name]]
        other = [r for r in items if r not in solo]
        got = sampler.take(solo, PER_TAIL, 'tail_solo', focus=name)
        got += sampler.take(other, PER_TAIL - got, 'tail_other', focus=name)
        counts[name] = {'tail': got}
    # 3. 同作品混淆库。
    old_solo = [r for r in eligible if r['solo'] and len(r['characters']) == 1
                and r['characters'][0] in covered and not news.intersection(r['characters'])]
    old_multi = [r for r in eligible if len(r['characters']) >= 2 and set(r['characters']) <= covered
                 and not news.intersection(r['characters'])]
    by_work_solo, by_work_multi = defaultdict(list), defaultdict(list)
    for row in old_solo:
        for work in row['copyrights']:
            if work in work_targets:
                by_work_solo[work].append(row)
    for row in old_multi:
        for work in row['copyrights']:
            if work in work_targets:
                by_work_multi[work].append(row)
    work_counts = {}
    for work in sorted(work_targets, key=lambda w: (len(by_work_solo[w]), w)):
        quota = work_quota(len(work_targets[work]))
        visual, other = [], []
        for row in by_work_solo[work]:
            near = base.closest_visual(dict(row, copyrights=[work]), profiles)
            (visual if near else other).append(dict(row, confuser_of=near) if near else row)
        work_counts[work] = {
            'targets': len(work_targets[work]),
            'visual': sampler.take(visual, quota['visual'], 'same_work_visual_old', 4, work=work),
            'other': sampler.take(other, quota['other'], 'same_work_other_old', 3, work=work),
            'multi': sampler.take(by_work_multi[work], quota['multi'], 'same_work_multi_old', 3, work=work),
        }
    # 4. 对照。
    target_works = set(work_targets)
    controls = {
        'unrelated_old': [r for r in old_solo if not target_works.intersection(r['copyrights'])],
        'multi_old': [r for r in old_multi if not target_works.intersection(r['copyrights'])],
        'multi_other': [r for r in eligible if len(r['characters']) >= 2 and not news.intersection(r['characters'])
                        and not set(r['characters']) <= covered],
        'no_character': [r for r in eligible if not r['characters']],
        'other_character': [r for r in eligible if r['solo'] and len(r['characters']) == 1
                            and r['characters'][0] not in covered and r['characters'][0] not in news],
    }
    control_counts = {g: sampler.take(controls[g], q, g, CONTROL_PER_CHARACTER) for g, q in CONTROLS.items()}

    selected = sorted(sampler.selected, key=lambda r: r['id'])
    if len({r['artist'] for r in selected}) != len(selected):
        raise ValueError('评测集出现重复画师')
    write('requests.json', selected)
    write('frame.json', {'works': works, 'work_targets': {w: sorted(t) for w, t in sorted(work_targets.items())},
                         'tails': sorted(tails), 'heads': sorted(heads)})
    receipt = {
        'requests_sha256': sha_file(DATA / 'requests.json'), 'frame_sha256': sha_file(DATA / 'frame.json'),
        'images': len(selected), 'groups': Counter(r['group'] for r in selected),
        'heads': len(heads), 'heads_with_positive': sum(1 for n in heads if sum(counts[n].values())),
        'heads_with_full_solo_quota': sum(1 for n in heads
                                          if counts[n]['solo'] >= PER_HEAD[targets[n]['pack']]['solo']),
        'tails': len(tails), 'tail_states': tail_states,
        'tails_with_positive': sum(1 for n in tails if counts[n]['tail']),
        'works': len(work_targets), 'controls': control_counts,
        'rules': {'per_head': PER_HEAD, 'per_tail': PER_TAIL, 'work_quota': 'visual min(80,6+3n), other min(40,3+n), '
                  'multi min(16,2+n//2)', 'controls': CONTROLS, 'window': [START, STOP],
                  'tail_artist_partition': 'illustash-557-v1 test'},
        'inputs': {name: sha_file(DATA / path) for name, path in (
            ('targets', 'roster/targets.json'), ('covered', 'roster/covered.json'),
            ('prior', 'prior/boundary.json'), ('snapshot', 'pool/snapshot.json'),
            ('live', 'pool/live.json'), ('early', 'pool/early.json'), ('tail_posts', 'pool/live-tail-posts.json'),
            ('artist_aliases', 'artist-aliases.json'))},
    }
    write('sample-receipt.json', receipt)
    write('sample-counts.json', {'targets': counts, 'works': work_counts})
    print(json.dumps(receipt, ensure_ascii=False, indent=1), flush=True)


if __name__ == '__main__':
    main()
