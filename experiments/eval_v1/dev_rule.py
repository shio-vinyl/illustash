"""#551 下一轮：只用开发数据起草更严的二级选头和阈值规则（不读评测集 v1 的任何分数）。

开发数据 = 历次研究全部非训练图，比上一轮定二级阈值时多加了 #534 评测清单的 train 和 calibration
两组（这批是 #534 的 228 头的训练和校准数据，没用来训练二级头，对二级头是没见过的图）。

做法：按画师散列把开发数据分成 A、B 两半。在 A 上按各条规则定阈值，在 B 上数加到 B1
（WD + 精选索引 + 长尾 v1 + 一级 257 头）以后的新增误报和召回，用来比较规则。选定后用全部开发数据
重新定阈值，冻结规则、名单和包。冻结之后，只在评测集 v1 冻结以后新出现的帖子上验收。
"""

import argparse
from collections import Counter, defaultdict
import hashlib
import json

import numpy as np

from common import COVERAGE, D534, D557, DATA, PACK_2100, PACK_257, load, sha_file, write
from coverage import canonical
from evaluate import wd_projection
from expansion_fit import matrix
from fit import save_pack
from score import ccip, index_entries, longtail, heads as head_sets
from shortlist import read_pack
from tier2 import SOURCES

ARRAYS = ('wd_vectors', 'ccip_vectors', 'wd_scores')


def dev_data():
    rows, parts = [], {k: [] for k in ARRAYS}
    tags = None

    def add(batch, data, keep, origin):
        """batch 和 keep 一一对应：batch[k] 是特征第 keep[k] 行的元数据。"""
        nonlocal tags
        tags = data['wd_tags'] if tags is None else tags
        if not np.array_equal(tags, data['wd_tags']):
            raise ValueError('WD 角色词表不一致')
        for key in ARRAYS:
            parts[key].append(data[key][keep])
        rows.extend(dict(row, origin=origin) for row in batch)

    for folder, origin in SOURCES:
        batch = load(folder / 'manifest.json')
        with np.load(folder / 'features.npz', allow_pickle=False) as data:
            if data['ids'].tolist() != [r['id'] for r in batch]:
                raise ValueError('开发特征与元数据顺序不符')
            keep = [i for i, r in enumerate(batch) if not (origin == '551_original' and r['split'] == 'train')]
            add([batch[i] for i in keep], data, keep, origin)
    # #534 评测清单的三组都可当二级头的开发数据；特征散在几个回传目录里，按帖子 ID 和缩图 SHA 对上。
    manifest = load(D534 / 'evaluation/manifest.json')
    wanted = {(r['id'], r['sha256']): r for r in manifest}
    found = {}
    for name in ('infer-colab', 'infer-supplement-colab', 'infer-part-1-colab', 'infer-part-2-colab',
                 'infer-part-3-colab', 'infer-part-4-colab'):
        with np.load(D534 / name / 'features.npz', allow_pickle=False) as data:
            for i, (post, sha) in enumerate(zip(data['ids'].tolist(), data['image_shas'].tolist())):
                if (post, sha) in wanted and (post, sha) not in found:
                    found[post, sha] = (name, i)
    by_file = defaultdict(list)
    for key, (name, i) in found.items():
        by_file[name].append((i, wanted[key]))
    for name, items in sorted(by_file.items()):
        items.sort(key=lambda pair: pair[0])
        with np.load(D534 / name / 'features.npz', allow_pickle=False) as data:
            add([row for _, row in items], data, [i for i, _ in items], '534_evaluation')
    for stage in ('development', 'test'):
        accepted = load(D557 / f'{stage}-accepted.json')
        with np.load(D557 / f'{stage}-features.npz', allow_pickle=False) as data:
            position = {int(p): i for i, p in enumerate(data['ids'])}
            add(accepted, data, [position[r['id']] for r in accepted], f'557_{stage}')
    arrays = {k: np.concatenate(v) for k, v in parts.items()}
    first = {}
    for i, row in enumerate(rows):
        first.setdefault(row['id'], i)
    order = sorted(first.values())
    evaluation = load(DATA / 'eval/manifest.json')
    banned_ids, banned_artists = {r['id'] for r in evaluation}, {r['artist'] for r in evaluation}
    artist_aliases = load(DATA / 'artist-aliases.json')
    keep = [i for i in order if rows[i]['id'] not in banned_ids
            and canonical(rows[i]['artist'], artist_aliases) not in banned_artists]
    return [rows[i] for i in keep], {k: v[keep] for k, v in arrays.items()}, tags


def parse_claims(path, entries, aliases, count):
    """读已经扫好的生产锚点扫描结果（同 score.ccip 的解析）。"""
    import csv
    tag = {e['id']: canonical(e['tag'], aliases) for e in entries}
    patched = {e['id'] for e in entries if e['patched']}
    index, extra = [{} for _ in range(count)], [{} for _ in range(count)]
    with path.open() as stream:
        for i, _anchor, entity, distance, limit, rounded in csv.reader(stream, delimiter='\t'):
            if np.float32(limit) > 0 and np.float32(distance) <= np.float32(limit):
                bucket = extra if entity in patched else index
                bucket[int(i)][tag[entity]] = round(1 - float(rounded), 4)
    return index, extra


def half(artist):
    return 'A' if hashlib.sha256(f'eval-v1-devrule:{artist}'.encode()).digest()[0] < 128 else 'B'


def logit(p):
    p = np.clip(p, 1e-7, 1 - 1e-7)
    return np.log(p / (1 - p))


def sigmoid(x):
    return 1 / (1 + np.exp(-x))


def thresholds(rule, scores, truth, mask, same_work, level, positive_artists):
    """按规则逐头定阈值；mask 是用来定阈值的那部分开发图。返回阈值（1.0 = 不进包）。"""
    out = np.ones(scores.shape[1])
    for j in range(scores.shape[1]):
        s, t = scores[mask, j], truth[mask, j]
        negatives, positives = s[~t], np.sort(s[t])
        if not len(positives):
            continue
        negmax = float(negatives.max(initial=0.0))
        q25 = float(positives[len(positives) // 4])
        base = max(0.5, negmax + 0.01, q25)
        sw = s[~t & same_work[mask, j]]
        swmax = float(sw.max(initial=0.0))
        name, delta = rule
        if name in ('R0', 'R0m'):
            value = base
        elif name == 'R1':      # 所有头：比开发负例最高分在 logit 上再高 δ
            value = max(base, float(sigmoid(logit(negmax) + delta)))
        elif name == 'R2':      # 按同作品混淆程度给余量：要比同作品旧角色的最高分高出一截
            extra = {'none': 0.0, 'low': delta / 2, 'high': delta}[level[j]]
            value = max(base, float(sigmoid(logit(swmax) + extra)) if extra else base)
        elif name == 'R3':      # R2 + 同作品混淆多、但开发里同作品负例太少的头，下限提到 0.95
            extra = {'none': 0.0, 'low': delta / 2, 'high': delta}[level[j]]
            value = max(base, float(sigmoid(logit(swmax) + extra)) if extra else base)
            if level[j] == 'high' and (~t & same_work[mask, j]).sum() < 20:
                value = max(value, 0.95)
        else:
            raise ValueError(name)
        if name in ('R0m', 'R1', 'R2', 'R3') and positive_artists[j] < 2:
            continue      # 开发里只有一位正例画师的头证据太少，不进包
        out[j] = min(1.0, float(np.float32(value)))
    return out


def measure(scores, limits, truth, labels, base, old, same_mask, subset, truth_rows, extra=None):
    """extra：另外叠上的一组候选（每图一个 {角色: 分数}），一起算新增误报；召回只算本组头。"""
    hit = scores[subset] >= limits.astype(np.float32)
    introduced = same = tp = fp = 0
    for row, i in zip(hit, subset):
        names = {labels[j] for j in np.flatnonzero(row)}
        if extra is not None:
            names |= set(extra[i])
        actual = truth_rows[i]
        tp += len(names & actual)
        fp += len(names - actual)
        wrong = names - actual - base[i]
        if old[i] and wrong:
            introduced += 1
            same += bool(same_mask[i])
    truth_count = int(truth[subset].sum())
    hits = int((hit & truth[subset]).sum())
    olds = int(sum(old[i] for i in subset))
    sames = int(sum(old[i] and same_mask[i] for i in subset))
    return {'old_images': olds, 'introduced_false_old_images': introduced,
            'rate': round(introduced / olds, 5) if olds else None,
            'same_work_old_images': sames, 'introduced_same_work': same,
            'same_work_rate': round(same / sames, 5) if sames else None,
            'truth': truth_count, 'hits': hits, 'recall': round(hits / truth_count, 4) if truth_count else None,
            'tp': tp, 'fp': fp, 'precision': round(tp / (tp + fp), 4) if tp + fp else None,
            'heads': int((limits < 1).sum())}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--freeze', nargs=2, metavar=('RULE', 'DELTA'), help='选定后冻结这一条规则')
    args = parser.parse_args()
    rows, arrays, tags = dev_data()
    aliases = load(COVERAGE / 'alias-map.json')
    truth_rows = [set(r['characters']) for r in rows]
    covered = set(load(DATA / 'native-covered.json')['covered_labels'])
    old = np.array([bool(t & covered) for t in truth_rows])
    # B1 在开发图上的候选：WD 标签、精选索引、长尾 v1、一级 257 头。
    wd = [{k for k, v in wd_projection(raw, tags.tolist(), aliases).items() if v >= 0.35} for raw in arrays['wd_scores']]
    entries, anchors = index_entries()
    v1_meta, v1_anchors = longtail(D557)
    cache = DATA / 'tier2-next/dev-ccip.json'
    ids = [r['id'] for r in rows]
    if cache.exists() and load(cache)['ids'] == ids:
        cached = load(cache)
        index, lt1 = cached['index'], cached['lt1']
    else:
        scanned = DATA / 'scores/scan-dev-rule'
        vectors = np.asarray(arrays['ccip_vectors'], dtype='<f4')
        if ((scanned / 'claims.json').exists() and load(scanned / 'claims.json')['features'] == len(rows)
                and (scanned / 'vectors.f32').read_bytes() == vectors.tobytes()):
            index, lt1 = parse_claims(scanned / 'claims.tsv', entries + v1_meta, aliases, len(rows))
        else:
            index, lt1, _ = ccip('dev-rule', vectors, entries + v1_meta, np.concatenate([anchors, v1_anchors]), aliases)
        write('tier2-next/dev-ccip.json', {'ids': ids, 'index': index, 'lt1': lt1})
    t1 = head_sets(arrays['wd_vectors'], *read_pack(PACK_257))
    base = [wd[i] | set(index[i]) | set(lt1[i]) | set(t1[i]) for i in range(len(rows))]
    names, weights, biases, _ = read_pack(PACK_2100)
    tier1, *_ = read_pack(PACK_257)
    chosen = [j for j, n in enumerate(names) if n not in set(tier1)]
    labels = [names[j] for j in chosen]
    scores = matrix(arrays['wd_vectors'], weights[chosen], biases[chosen])
    column = {n: j for j, n in enumerate(labels)}
    truth = np.zeros((len(rows), len(labels)), dtype=bool)
    for i, t in enumerate(truth_rows):
        for n in t & column.keys():
            truth[i, column[n]] = True
    works = load(DATA / 'frame.json')['works']
    head_works = [set(works.get(n, [])) for n in labels]
    copyrights = [set(r['copyrights']) for r in rows]
    heads_of_work = defaultdict(list)
    for j, w in enumerate(head_works):
        for x in w:
            heads_of_work[x].append(j)
    same_work = np.zeros((len(rows), len(labels)), dtype=bool)
    for i, c in enumerate(copyrights):
        if old[i]:
            for x in c:
                same_work[i, heads_of_work.get(x, [])] = True
    # 混淆程度：开发图里与该头主作品相同的旧角色有几个（只看元数据）。
    olds_by_work = defaultdict(set)
    for t, c in zip(truth_rows, copyrights):
        for w in c:
            olds_by_work[w].update(t & covered)
    level = []
    for w in head_works:
        n = len(set().union(*(olds_by_work.get(x, set()) for x in w))) if w else 0
        level.append('none' if n == 0 else 'low' if n < 10 else 'high')
    artist_aliases = load(DATA / 'artist-aliases.json')
    part = np.array([half(canonical(r['artist'], artist_aliases)) for r in rows])
    A, B = part == 'A', part == 'B'
    subset_b = np.flatnonzero(B)
    any_same = same_work.any(axis=1)
    positive_artists_a = [len({rows[i]['artist'] for i in np.flatnonzero(A & truth[:, j])}) for j in range(len(labels))]
    positive_artists_all = [len({rows[i]['artist'] for i in np.flatnonzero(truth[:, j])}) for j in range(len(labels))]
    grid = [('R0', 0.0), ('R0m', 0.0), *[('R1', d) for d in (1.0, 2.0, 3.0)], *[('R2', d) for d in (2.0, 3.0, 4.0, 6.0)],
            *[('R3', d) for d in (2.0, 3.0, 4.0)]]
    # 0.3 的 166 个二级头（#534 头里没过一级的），统一阈值 0.8009。#534 评测清单的 train 和 calibration
    # 是这批头自己的训练和定阈值数据，量整体数时从 B 里去掉这两组，免得把 166 头的误报算少。
    h534 = load(DATA / 'report/h534-per-head.json')
    rest = [n for n, v in h534.items() if v['gate']]
    with np.load(D534 / 'evaluation/B-heads.npz', allow_pickle=False) as data:
        names534 = data['labels'].tolist()
        ix = [names534.index(n) for n in rest]
        data_weight, data_bias = data['weight'].copy(), data['bias'].copy()
        h166 = head_sets(arrays['wd_vectors'], rest, data['weight'][ix], data['bias'][ix],
                         np.full(len(rest), 0.8008725047111511))
    own534 = np.array([r['origin'] == '534_evaluation' and r.get('split') in ('train', 'calibration') for r in rows])
    clean_b = np.array([i for i in subset_b if not own534[i]])
    # 同一条规则也用到这 166 头上（只用它们没见过的开发图定阈值），看整体能不能压下来。
    scores166 = matrix(arrays['wd_vectors'], data_weight[ix], data_bias[ix])
    col166 = {n: j for j, n in enumerate(rest)}
    truth166 = np.zeros((len(rows), len(rest)), dtype=bool)
    for i, t in enumerate(truth_rows):
        for n in t & col166.keys():
            truth166[i, col166[n]] = True
    works166 = [set(works.get(n, [])) for n in rest]
    same166 = np.zeros((len(rows), len(rest)), dtype=bool)
    by_work166 = defaultdict(list)
    for j, w in enumerate(works166):
        for x in w:
            by_work166[x].append(j)
    for i, c in enumerate(copyrights):
        if old[i]:
            for x in c:
                same166[i, by_work166.get(x, [])] = True
    level166 = []
    for w in works166:
        n = len(set().union(*(olds_by_work.get(x, set()) for x in w))) if w else 0
        level166.append('none' if n == 0 else 'low' if n < 10 else 'high')
    a_clean = A & ~own534
    artists166 = [len({rows[i]['artist'] for i in np.flatnonzero(a_clean & truth166[:, j])}) for j in range(len(rest))]
    if args.freeze:
        rule = (args.freeze[0], float(args.freeze[1]))
        every = np.ones(len(rows), dtype=bool)
        limits = thresholds(rule, scores, truth, every, same_work, level, positive_artists_all)
        kept = [chosen[j] for j in np.flatnonzero(limits < 1)]
        folder = DATA / 'tier2-next'
        folder.mkdir(exist_ok=True)
        package = save_pack('tier2-next', folder, [names[j] for j in kept], weights[kept], biases[kept],
                            0.5, [float(x) for x in limits[limits < 1]])
        heads = [{'label': labels[j], 'threshold': float(limits[j]), 'confusion': level[j],
                  'dev_positive_artists': positive_artists_all[j], 'kept': bool(limits[j] < 1)}
                 for j in range(len(labels))]
        write('tier2-next/heads.json', heads)
        freeze = {'rule': {'name': rule[0], 'delta_logit': rule[1]}, 'development_images': len(rows),
                  'development_origins': Counter(r['origin'] for r in rows), 'candidates': len(labels),
                  'kept': len(kept), 'by_confusion': Counter(h['confusion'] for h in heads if h['kept']),
                  'package': package, 'heads_sha256': sha_file(DATA / 'tier2-next/heads.json'),
                  'frozen_at': '2026-11-18', 'validation': '只在评测集 v1 冻结（2026-11-17）以后新出现的帖子上验收'}
        # 0.3 的 166 个二级头按同一条规则重定阈值（只用它们没见过的开发图），不低于原统一阈值 0.8009。
        limits166 = np.maximum(thresholds(rule, scores166, truth166, ~own534, same166, level166,
                                          [len({rows[i]['artist'] for i in np.flatnonzero(~own534 & truth166[:, j])})
                                           for j in range(len(rest))]), 0.8008725)
        keep166 = np.flatnonzero(limits166 < 1)
        package166 = save_pack('tier2-166-retightened', folder, [rest[j] for j in keep166], data_weight[ix][keep166],
                               data_bias[ix][keep166], 0.8008725, [float(x) for x in limits166[keep166]])
        freeze['retightened_03_tier2_166'] = {'kept': int(len(keep166)), 'of': len(rest), 'package': package166,
                                              'note': '改 0.3 已冻结的 166 头阈值，需用户决定'}
        write('tier2-next/freeze.json', freeze)
        print(json.dumps(freeze, ensure_ascii=False, indent=1))
        return
    results, combined, both = {}, {}, {}
    only166 = measure(np.zeros((len(rows), 0)), np.ones(0), np.zeros((len(rows), 0), dtype=bool), [], base, old,
                      any_same, clean_b, truth_rows, extra=h166)
    print('166 only', only166, flush=True)
    for rule in grid:
        limits = thresholds(rule, scores, truth, A, same_work, level, positive_artists_a)
        key = f'{rule[0]}:{rule[1]}'
        results[key] = measure(scores, limits, truth, labels, base, old, any_same, subset_b, truth_rows)
        combined[key] = measure(scores, limits, truth, labels, base, old, any_same, clean_b, truth_rows, extra=h166)
        limits166 = np.maximum(thresholds(rule, scores166, truth166, a_clean, same166, level166, artists166), 0.8008725)
        hit166 = scores166 >= limits166.astype(np.float32)
        tightened = [{rest[j]: 1.0 for j in np.flatnonzero(row)} for row in hit166]
        both[key] = measure(scores, limits, truth, labels, base, old, any_same, clean_b, truth_rows, extra=tightened)
        both[key]['heads_166_kept'] = int((limits166 < 1).sum())
        both[key]['recall_166'] = round(float((hit166 & truth166)[clean_b].sum() / max(1, truth166[clean_b].sum())), 4)
        print(rule, results[key], 'combined', combined[key], 'both-tightened', both[key], flush=True)
    write('tier2-next/dev-rules.json', {'development_images': len(rows), 'half_A': int(A.sum()), 'half_B': int(B.sum()),
                                        'origins': Counter(r['origin'] for r in rows),
                                        'confusion_levels': Counter(level), 'results': results,
                                        'combined_with_03_tier2_166': combined, 'only_03_tier2_166': only166,
                                        'combined_with_166_under_same_rule': both,
                                        'combined_subset_images': int(len(clean_b))})


if __name__ == '__main__':
    main()
