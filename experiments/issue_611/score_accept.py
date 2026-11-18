"""#611 二期：冻结验收集，并用 2026-11-18 冻结的包做整包验收（不改阈值、不挑头）。

1. 冻结（只看指纹）：读二期回传，按「正例 → 旧角色图 → 对照」、组内按帖子散列的固定顺序逐张过滤：
   缩图 SHA-256 与历史研究（eval-v1 prior/fingerprints.npz）、共用评测集 v1、本 Issue 一期和难负例的任何一张相同，
   或 pHash ≤6 且 dHash ≤8（v1 的阈值），就舍弃；验收集内部也照此去重。
2. 验收：基线 B1 = WD 标签（≥0.35）+ 完整精选索引 + 长尾 v1 + 一级 257 头；叠上 0.4 全部 1,873 头
  （`5869c665…`）和本 Issue 冻结的 5,986 头（`ec747f88…`）。「新增含错误候选」= 包给出的候选既不在真值里、
   B1 也没给过；分母是真值含 B1 已覆盖角色的图。区间按画师聚类重抽样 2,000 次（每位画师 1 张，等于按图）。
   门槛：点估计 ≤1%、95% 上界 ≤1.5%、目标正例精确率 ≥95%。
3. 辅助：在共用评测集 v1 的旧角色图上再算一次（用 v1 已存的 B1 候选），只作参考，不据此改任何东西。
"""

import argparse
from collections import Counter, defaultdict
import json

import numpy as np

import fit_dev
from fit_dev import OUT, PACK_04, PACK_04_SHA, V1, hits, interval, own_features, read_v2, report, write
from common import COVERAGE, PACK_257, PACK_257_SHA, load, priority, sha_file
from coverage import canonical
from evaluate import wd_projection
from shortlist import read_pack

ORDER = ('positive_single', 'positive_multi', 'old_character', 'control_no_character', 'control_uncovered')
FROZEN_611 = 'ec747f88d49471b9c544d368094e43645432ac396559dad4d962c6882bd7da8e'


def bits(values):
    return np.array([int(v) for v in values], dtype=np.uint64)


def near(ph, dh, ref_ph, ref_dh):
    if not len(ref_ph):
        return False
    a = np.bitwise_count(ref_ph ^ np.uint64(ph))
    b = np.bitwise_count(ref_dh ^ np.uint64(dh))
    return bool(((a <= 6) & (b <= 8)).any())


def fingerprints_of(folder):
    """读一组回传分片的缩图 SHA 和指纹。"""
    from zipfile import ZipFile
    from io import BytesIO
    shas, ph, dh = [], [], []
    for archive in sorted((folder / 'received').glob('part-*.zip')):
        with ZipFile(archive) as package, np.load(BytesIO(package.read('features.npz')), allow_pickle=False) as data:
            shas += data['image_shas'].tolist()
            ph += data['phash'].tolist()
            dh += data['dhash'].tolist()
    return shas, ph, dh


def freeze():
    rows, arrays, _tags, failed = own_features(OUT / 'accept/requests.json', OUT / 'cloud-accept', '611_accept')
    with np.load(V1 / 'prior/fingerprints.npz', allow_pickle=False) as data:
        ref_sha = set(data['image_shas'].tolist())
        ref_ph, ref_dh = [data['phash'].tolist()], [data['dhash'].tolist()]
    with np.load(V1 / 'eval/features.npz', allow_pickle=False) as data:
        ref_sha |= set(data['image_shas'].tolist())
        ref_ph.append(data['phash'].tolist())
        ref_dh.append(data['dhash'].tolist())
    for name in ('cloud-dev', 'cloud-hard'):
        s, p, d = fingerprints_of(OUT / name)
        ref_sha |= set(s)
        ref_ph.append(p)
        ref_dh.append(d)
    ref_ph, ref_dh = bits(sum(ref_ph, [])), bits(sum(ref_dh, []))
    shas, phs, dhs = fingerprints_of(OUT / 'cloud-accept')
    order = sorted(range(len(rows)), key=lambda i: (ORDER.index(rows[i]['group']), priority(rows[i]['id'])))
    kept, excluded = [], Counter()
    own_sha = set()
    own_ph, own_dh = np.zeros(len(rows), dtype=np.uint64), np.zeros(len(rows), dtype=np.uint64)
    for i in order:
        sha, ph, dh = shas[i], int(phs[i]), int(dhs[i])
        if sha in ref_sha or sha in own_sha:
            excluded[rows[i]['group'], 'same_thumbnail'] += 1
            continue
        if near(ph, dh, ref_ph, ref_dh) or near(ph, dh, own_ph[:len(kept)], own_dh[:len(kept)]):
            excluded[rows[i]['group'], 'near_duplicate'] += 1
            continue
        own_ph[len(kept)], own_dh[len(kept)] = ph, dh
        kept.append(i)
        own_sha.add(sha)
    kept.sort()
    manifest = [rows[i] for i in kept]
    path = write('accept/manifest.json', manifest)
    np.savez_compressed(OUT / 'accept/features.npz', ids=np.array([r['id'] for r in manifest]),
                        **{k: v[kept] for k, v in arrays.items()}, wd_tags=_tags)
    receipt = {'requested': len(load(OUT / 'accept/requests.json')), 'fetch_failures': failed, 'kept': len(kept),
               'excluded': [{'group': g, 'reason': r, 'count': n} for (g, r), n in sorted(excluded.items())],
               'groups': Counter(r['group'] for r in manifest), 'reference_fingerprints': int(len(ref_ph)),
               'manifest_sha256': sha_file(path), 'features_sha256': sha_file(OUT / 'accept/features.npz')}
    write('accept/freeze.json', receipt)
    print(json.dumps(receipt, ensure_ascii=False, indent=1), flush=True)


def strata(rows, old, covered, works_611):
    """旧角色图分层：同作品（和本 Issue 某个头同作品）/ 无关作品；单人 / 多人。"""
    out = defaultdict(list)
    for i, r in enumerate(rows):
        if not old[i]:
            continue
        same = bool(set(r['copyrights']) & works_611)
        out['same_work' if same else 'unrelated_work'].append(i)
        out['single' if len(r['characters']) == 1 else 'multi'].append(i)
    return out


def score():
    receipt = load(OUT / 'accept/freeze.json')
    if sha_file(OUT / 'accept/manifest.json') != receipt['manifest_sha256'] or \
            sha_file(OUT / 'accept/features.npz') != receipt['features_sha256']:
        raise ValueError('验收集与冻结校验值不符')
    for path, expected in ((PACK_257, PACK_257_SHA), (PACK_04, PACK_04_SHA), (OUT / 'freeze/wd-611.bin', FROZEN_611)):
        if sha_file(path) != expected:
            raise ValueError(f'冻结包摘要不符：{path}')
    rows = load(OUT / 'accept/manifest.json')
    with np.load(OUT / 'accept/features.npz', allow_pickle=False) as data:
        arrays = {k: data[k] for k in data.files}
    aliases = load(COVERAGE / 'alias-map.json')
    roster = {r['label']: r for r in load(OUT / 'roster/roster-v2.json')}
    supply = {r['label']: r for r in load(OUT / 'supply/supply.json')}
    covered = set(load(V1 / 'native-covered.json')['covered_labels'])
    x = arrays['wd_vectors']
    labels, w, b, t = read_pack(OUT / 'freeze/wd-611.bin')
    labels = list(labels)
    labels_611 = set(labels)
    tags = arrays['wd_tags'].tolist()
    wd = [{k for k, v in wd_projection(raw, tags, aliases).items() if v >= 0.35} for raw in arrays['wd_scores']]
    claims = fit_dev.ccip_claims('accept', arrays['ccip_vectors'], aliases)
    t1_labels, t1_w, t1_b, t1_t = read_pack(PACK_257)
    t1 = hits(x, list(t1_labels), t1_w, t1_b, np.asarray(t1_t, dtype=float))
    base = [wd[i] | claims[i] | set(t1[i]) for i in range(len(rows))]
    header, w04, b04 = read_v2(PACK_04)
    packs = {'0.4': hits(x, header['labels'], w04, b04, np.asarray(header['thresholds'], dtype=float)),
             '611': hits(x, labels, w, b, np.asarray(t, dtype=float))}
    old = [bool(set(r['characters']) & covered) for r in rows]
    works_611 = {supply[n]['work'] for n in labels if supply[n]['work'] not in (None, 'original')}
    same = [o and bool(set(r['copyrights']) & works_611) for o, r in zip(old, rows)]
    idx = list(range(len(rows)))
    result = {'images': len(rows), 'groups': Counter(r['group'] for r in rows),
              'overall': report(idx, rows, base, packs, old, labels_611, same)}
    for name, items in strata(rows, old, covered, works_611).items():
        result[f'old_{name}'] = report(items, rows, base, packs, old, labels_611, same)
    controls = [i for i, r in enumerate(rows) if r['group'].startswith('control')]
    result['controls_any_611_candidate'] = sum(bool(packs['611'][i]) for i in controls)
    # 召回和精确率：按帖子数档、单人/多人
    rec = Counter()
    for i, r in enumerate(rows):
        truth = set(r['characters']) & labels_611
        got = set(packs['611'][i])
        kind = 'single' if len(r['characters']) == 1 else 'multi'
        for n in truth:
            band = roster[n]['band']
            for key in (band, kind, 'all'):
                rec[f'{key}:truth'] += 1
                rec[f'{key}:hit'] += n in got
        if truth:
            for n in got:
                band = roster[n]['band']
                for key in (band, kind, 'all'):
                    rec[f'{key}:{"tp" if n in truth else "fp"}'] += 1
    result['by_band_and_kind'] = {
        k: {'truth': rec[f'{k}:truth'], 'recall': round(rec[f'{k}:hit'] / rec[f'{k}:truth'], 4) if rec[f'{k}:truth'] else None,
            'target_precision': round(rec[f'{k}:tp'] / (rec[f'{k}:tp'] + rec[f'{k}:fp']), 4)
            if rec[f'{k}:tp'] + rec[f'{k}:fp'] else None}
        for k in ('all', 'single', 'multi', '50-99', '100-499', '500-999', '1000+')}
    gate = result['overall']['0.4+611']
    result['gate'] = {'point_le_1pct': gate['rate'] <= 0.01, 'upper_le_1_5pct': gate['interval_95'][1] <= 0.015,
                      'target_precision_ge_95pct': (gate['target_precision'] or 0) >= 0.95}
    # 逐头的错误候选（旧角色图上新增的），给 Issue 写过不了的头用
    per_head = Counter()
    for i in idx:
        if old[i]:
            for n in set(packs['611'][i]) - set(rows[i]['characters']) - base[i]:
                per_head[n] += 1
    result['false_old_by_head'] = {'heads_with_any': len(per_head), 'max': max(per_head.values(), default=0),
                                   'top20_share': round(sum(v for _, v in per_head.most_common(20)) /
                                                        max(1, sum(per_head.values())), 3)}
    write('accept/per-head-false.json', dict(per_head))
    # 辅助：v1 评测集的旧角色图
    v1_rows = load(V1 / 'eval/manifest.json')
    with np.load(V1 / 'eval/features.npz', allow_pickle=False) as data:
        v1x = data['wd_vectors']
    v1_base = [set() for _ in v1_rows]
    for source in ('wd', 'index', 'lt_v1', 't1_257'):
        for i, item in enumerate(load(V1 / f'scores/{source}.json')):
            v1_base[i] |= set(item)
    v1_rows = [dict(r, characters=sorted({canonical(c, aliases) for c in r['characters']})) for r in v1_rows]
    v1_packs = {'0.4': hits(v1x, header['labels'], w04, b04, np.asarray(header['thresholds'], dtype=float)),
                '611': hits(v1x, labels, w, b, np.asarray(t, dtype=float))}
    v1_old = [bool(set(r['characters']) & covered) for r in v1_rows]
    v1_same = [o and bool(set(r.get('copyrights', [])) & works_611) for o, r in zip(v1_old, v1_rows)]
    result['auxiliary_v1'] = report(list(range(len(v1_rows))), v1_rows, v1_base, v1_packs, v1_old, labels_611, v1_same)
    write('accept/result.json', result)
    print(json.dumps(result, ensure_ascii=False, indent=1), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('step', choices=('freeze', 'score'))
    args = parser.parse_args()
    freeze() if args.step == 'freeze' else score()


if __name__ == '__main__':
    main()
