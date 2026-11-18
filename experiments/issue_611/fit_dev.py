"""#611 一期：训练老长尾角色的 WD 扩展头，按 #551 冻结的 R1 δ=2 定阈值，在开发数据上估整包误报。

- 训练：一期清单里训练区的图（split=train），照 #551 的 fit.fit_heads（至少 5 张、3 位画师；
  同作品、近邻和随机负例，逻辑回归，标准化折进权重）。
- 开发数据：一期清单里校准区的图，加上 #551 定 0.4 阈值用的 47,762 张历次开发图（eval_v1 dev_rule.dev_data）。
  历次开发图里和本轮训练图同帖子或同 MD5 的去掉，免得训练过的图又拿来定阈值。
- 阈值：dev_rule.thresholds(('R1', 2.0))，即 max(0.5, 负例最高分 + 0.01, 正例下四分位,
  sigmoid(logit(负例最高分) + 2))，开发正例画师少于 2 位的头不进包。
- 开发估计（--estimate）：按画师散列把开发数据分两半，A 半定阈值，只在 B 半里本轮新取的图上量。
  这批图 0.4 的头没见过。基线 B1 = WD 标签（≥0.35）+ 完整精选索引 + 长尾 v1 + 一级 257 头；
  叠上去的是 0.4 全部 1,873 头（`5869c665…`）和本 Issue 的头。「新增含错误候选」= 包给出的候选
  既不在真值里、B1 也没给过；分母是真值含旧角色的图。区间按画师聚类重抽样 2,000 次。
- 冻结（--freeze）：用全部开发数据定阈值，存包和逐头记录。

只读主检出 .worktrees 和 #628 工作区的冻结数据，产物写 _scratch/issue-611。
"""

import argparse
from collections import Counter, defaultdict
import hashlib
import json
import os
from pathlib import Path
import sys
from zipfile import ZipFile
from io import BytesIO

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
OUT = Path(os.environ.get('ISSUE_611_OUTPUT', ROOT / '_scratch/issue-611'))
EVAL_V1_TREE = Path(os.environ.get('ISSUE_611_EVAL_V1_TREE',
                                   ROOT.parents[0] / 'issue-431-432-cleanup-f09a7a'))
sys.path.insert(0, str(EVAL_V1_TREE / 'experiments/eval_v1'))
from common import COVERAGE, D557, DATA as V1, PACK_257, PACK_257_SHA, load, sha_file  # noqa: E402
from coverage import canonical  # noqa: E402
from dev_rule import dev_data, thresholds  # noqa: E402
from evaluate import native_scan, matrix_file, wd_projection  # noqa: E402
from expansion_fit import matrix  # noqa: E402
from fit import fit_heads, save_pack  # noqa: E402
from score import SCAN, index_entries, longtail  # noqa: E402
from shortlist import read_pack  # noqa: E402

PACK_04 = V1 / 'deliver/character-pack-0.4/wd-extension-0.4.bin'
PACK_04_SHA = '5869c6658af63e91f9e1989dee81ed98e5d6797a42527e676201d8de1fb42fbe'
RULE = ('R1', 2.0)
HEAD_BLOCK = 512
SEED = 611


def write(path, data):
    path = OUT / path
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + '.partial')
    temporary.write_text(json.dumps(data, ensure_ascii=False, separators=(',', ':')) + '\n')
    temporary.replace(path)
    return path


def half(artist):
    return 'A' if hashlib.sha256(f'illustash-611-devrule:{artist}'.encode()).digest()[0] < 128 else 'B'


def read_v2(path):
    with path.open('rb') as stream:
        header = json.loads(stream.readline())
        values = np.frombuffer(stream.read(), dtype='<f4')
    n = len(header['labels'])
    return header, values[:-n].reshape(n, 1024), values[-n:]


def own_features():
    """读一期回传的特征，按请求清单顺序拼起来；取图失败的帖子不在里面。"""
    requests = load(OUT / 'dev/requests.json')
    by_id = {r['id']: r for r in requests}
    folder = OUT / 'cloud-dev'
    shards = sorted((folder / 'shards').glob('part-*.json'))
    rows, parts = [], defaultdict(list)
    failed = 0
    for shard in shards:
        receipt = load(folder / 'received' / f'{shard.stem}.json')
        archive = folder / 'received' / f'{shard.stem}.zip'
        if receipt['manifest_sha256'] != sha_file(shard) or receipt['archive_sha256'] != sha_file(archive):
            raise ValueError(f'回传分片与收据不符：{shard.name}')
        with ZipFile(archive) as package:
            report = json.loads(package.read('receipt.json'))
            with np.load(BytesIO(package.read('features.npz')), allow_pickle=False) as data:
                for key in ('wd_vectors', 'ccip_vectors', 'wd_scores'):
                    parts[key].append(data[key])
                rows.extend(dict(by_id[int(i)], origin='611_dev') for i in data['ids'])
                tags = data['wd_tags']
        failed += len(report['failures'])
    if len(rows) + failed != len(requests):
        raise ValueError('回传行数与请求清单对不上')
    return rows, {k: np.concatenate(v) for k, v in parts.items()}, tags, failed


def block_scores(x, weights, biases, begin):
    return matrix(x, weights[begin:begin + HEAD_BLOCK], biases[begin:begin + HEAD_BLOCK])


def rule_thresholds(x, rows, labels, weights, biases, mask):
    """逐块定阈值，避免一次拼出（图数 × 头数）的整张分数矩阵。"""
    index = {n: j for j, n in enumerate(labels)}
    out = np.ones(len(labels))
    artists_by_head = [set() for _ in labels]
    for i, r in enumerate(rows):
        if mask[i]:
            for n in set(r['characters']) & index.keys():
                artists_by_head[index[n]].add(r['artist'])
    for begin in range(0, len(labels), HEAD_BLOCK):
        names = labels[begin:begin + HEAD_BLOCK]
        scores = block_scores(x, weights, biases, begin)
        truth = np.zeros(scores.shape, dtype=bool)
        local = {n: j for j, n in enumerate(names)}
        for i, r in enumerate(rows):
            for n in set(r['characters']) & local.keys():
                truth[i, local[n]] = True
        dummy = np.zeros(scores.shape, dtype=bool)
        positive_artists = [len(artists_by_head[begin + j]) for j in range(len(names))]
        out[begin:begin + len(names)] = thresholds(RULE, scores, truth, mask, dummy, ['none'] * len(names),
                                                   positive_artists)
    return out, [len(a) for a in artists_by_head]


def hits(x, labels, weights, biases, limits):
    keep = np.flatnonzero(limits < 1)
    result = [dict() for _ in range(len(x))]
    for begin in range(0, len(keep), HEAD_BLOCK):
        chosen = keep[begin:begin + HEAD_BLOCK]
        scores = matrix(x, weights[chosen], biases[chosen])
        for i, j in zip(*np.nonzero(scores >= limits[chosen].astype(np.float32))):
            result[i][labels[chosen[j]]] = float(scores[i, j])
    return result


def ccip_claims(name, vectors, aliases):
    entries, anchors = index_entries()
    v1_meta, v1_anchors = longtail(D557)
    entries = entries + v1_meta
    location = OUT / 'scan' / name
    location.mkdir(parents=True, exist_ok=True)
    features = location / 'vectors.f32'
    matrix_file(features, np.asarray(vectors, dtype='<f4'))
    out = location / 'claims.tsv'
    receipt = native_scan(SCAN, location, entries, np.concatenate([anchors, v1_anchors]), features, out)
    if receipt['live_claim_difference_images']:
        raise ValueError('批量扫描与单图认领不一致')
    import csv
    tag = {e['id']: canonical(e['tag'], aliases) for e in entries}
    claims = [set() for _ in vectors]
    with out.open() as stream:
        for i, _anchor, entity, distance, limit, _rounded in csv.reader(stream, delimiter='\t'):
            if np.float32(limit) > 0 and np.float32(distance) <= np.float32(limit):
                claims[int(i)].add(tag[entity])
    return claims


def interval(flags, clusters, draws=2000):
    groups = defaultdict(list)
    for f, c in zip(flags, clusters):
        groups[c].append(f)
    sums = np.array([sum(v) for v in groups.values()], dtype=float)
    counts = np.array([len(v) for v in groups.values()], dtype=float)
    rng = np.random.default_rng(SEED)
    samples = []
    for _ in range(draws):
        pick = rng.integers(0, len(sums), len(sums))
        samples.append(sums[pick].sum() / counts[pick].sum())
    return [round(float(np.percentile(samples, 2.5)), 5), round(float(np.percentile(samples, 97.5)), 5)]


def report(subset, rows, base, packs, old, targets_611, same):
    """packs：{名字: 每图候选 dict}。按 dev_rule.measure 的口径算新增含错误候选的图。"""
    result = {}
    combos = {name: [name] for name in packs}
    combos['0.4+611'] = list(packs)
    for combo, names in combos.items():
        flags, same_flags, tp, fp, tp_target, fp_target = [], [], 0, 0, 0, 0
        for i in subset:
            given = set().union(*(packs[n][i].keys() for n in names))
            actual = set(rows[i]['characters'])
            wrong = given - actual - base[i]
            tp += len(given & actual)
            fp += len(given - actual)
            if old[i]:
                flags.append(bool(wrong))
                same_flags.append(bool(wrong) if same[i] else None)
            if actual & targets_611:
                own = set(packs['611'][i]) if '611' in names else set()
                tp_target += len(own & actual)
                fp_target += len(own - actual)
        clusters = [rows[i]['artist'] for i in subset if old[i]]
        same_only = [f for f in same_flags if f is not None]
        result[combo] = {
            'old_images': len(flags), 'introduced_false_old_images': int(sum(flags)),
            'rate': round(sum(flags) / len(flags), 5) if flags else None,
            'interval_95': interval(flags, clusters) if flags else None,
            'same_work_old_images': len(same_only), 'introduced_same_work': int(sum(same_only)),
            'tp': tp, 'fp': fp, 'target_tp': tp_target, 'target_fp': fp_target,
            'target_precision': round(tp_target / (tp_target + fp_target), 4) if tp_target + fp_target else None,
        }
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--estimate', action='store_true', help='A 半定阈值，在 B 半本轮新图上估整包')
    parser.add_argument('--freeze', action='store_true', help='全部开发数据定阈值，冻结包')
    parser.add_argument('--workers', type=int, default=4)
    args = parser.parse_args()
    if sha_file(PACK_257) != PACK_257_SHA:
        raise ValueError('257 头包摘要不符')
    aliases = load(COVERAGE / 'alias-map.json')
    roster = {r['label']: r for r in load(OUT / 'roster/roster-v2.json')}
    supply = {r['label']: r for r in load(OUT / 'supply/supply.json')}
    rows, arrays, tags, failed = own_features()
    print(f'本轮特征 {len(rows):,} 张，取图失败 {failed}', flush=True)

    # 训练（结果按输入缓存：训练行的帖子 ID 摘要不变就复用）
    train_idx = [i for i, r in enumerate(rows) if r['split'] == 'train']
    train_rows = [dict(rows[i], focus_roles=[x.split(':', 1)[1] for x in rows[i]['roles'] if x.startswith('train:')])
                  for i in train_idx]
    key = hashlib.sha256(json.dumps([r['id'] for r in train_rows]).encode()).hexdigest()
    fit_dir = OUT / 'fit'
    fit_dir.mkdir(exist_ok=True)
    stamp = fit_dir / 'input.json'
    if not (stamp.exists() and load(stamp)['train_ids_sha256'] == key and (fit_dir / 'heads.npz').exists()):
        targets = sorted({n for r in train_rows for n in r['focus_roles']} & roster.keys())
        fit_heads(train_rows, arrays['wd_vectors'][train_idx], targets, fit_dir, args.workers)
        write('fit/input.json', {'train_ids_sha256': key, 'train_images': len(train_rows), 'targets': len(targets)})
    with np.load(fit_dir / 'heads.npz', allow_pickle=False) as data:
        labels, weights, biases = data['labels'].tolist(), data['weight'], data['bias']
    training = load(fit_dir / 'training.json')
    print(f'训成 {len(labels):,} 头', flush=True)

    # 开发数据：本轮校准区 + 历次开发图（去掉和本轮训练图重合的）
    trained_ids = {rows[i]['id'] for i in train_idx}
    trained_md5 = {rows[i]['md5'] for i in train_idx}
    hist_rows, hist_arrays, hist_tags = dev_data()
    if not np.array_equal(hist_tags, tags):
        raise ValueError('WD 角色词表不一致')
    keep_hist = [i for i, r in enumerate(hist_rows) if r['id'] not in trained_ids and r.get('md5') not in trained_md5]
    own_cal = [i for i, r in enumerate(rows) if r['split'] == 'calibration']
    own_ids = {rows[i]['id'] for i in own_cal}
    keep_hist = [i for i in keep_hist if hist_rows[i]['id'] not in own_ids]
    artist_aliases = load(V1 / 'artist-aliases.json')
    dev_rows = [rows[i] for i in own_cal] + [dict(hist_rows[i], characters=sorted(
        {canonical(c, aliases) for c in hist_rows[i]['characters']})) for i in keep_hist]
    dev = {k: np.concatenate([arrays[k][own_cal], hist_arrays[k][keep_hist]]) for k in arrays}
    x = dev['wd_vectors']
    origins = Counter(r['origin'] for r in dev_rows)
    print(f'开发数据 {len(dev_rows):,} 张：{dict(origins)}；历次开发图因与训练重合去掉 '
          f'{len(hist_rows) - len(keep_hist) - sum(hist_rows[i]["id"] in own_ids for i in range(len(hist_rows)))}', flush=True)
    labels_611 = set(labels)

    if args.estimate:
        part = np.array([half(canonical(r['artist'], artist_aliases)) for r in dev_rows])
        limits, _ = rule_thresholds(x, dev_rows, labels, weights, biases, part == 'A')
        subset = [i for i in range(len(dev_rows)) if part[i] == 'B' and dev_rows[i]['origin'] == '611_dev']
        sx = x[subset]
        sub_rows = [dev_rows[i] for i in subset]
        covered = set(load(V1 / 'native-covered.json')['covered_labels'])
        old = [bool(set(r['characters']) & covered) for r in sub_rows]
        wd = [{k for k, v in wd_projection(raw, tags.tolist(), aliases).items() if v >= 0.35}
              for raw in dev['wd_scores'][subset]]
        claims = ccip_claims('dev-b', dev['ccip_vectors'][subset], aliases)
        t1_labels, t1_w, t1_b, t1_t = read_pack(PACK_257)
        t1 = hits(sx, list(t1_labels), t1_w, t1_b, np.asarray(t1_t, dtype=float))
        base = [wd[i] | claims[i] | set(t1[i]) for i in range(len(subset))]
        if sha_file(PACK_04) != PACK_04_SHA:
            raise ValueError('0.4 WD 包摘要不符')
        header, w04, b04 = read_v2(PACK_04)
        packs = {'0.4': hits(sx, header['labels'], w04, b04, np.asarray(header['thresholds'], dtype=float)),
                 '611': hits(sx, labels, weights, biases, limits)}
        works = {supply[n]['work'] for n in labels if supply[n]['work'] not in (None, 'original')}
        same = [o and bool(set(r['copyrights']) & works) for o, r in zip(old, sub_rows)]
        idx = list(range(len(subset)))
        result = {'rule': {'name': RULE[0], 'delta_logit': RULE[1]}, 'heads_trained': len(labels),
                  'heads_kept_on_half_A': int((limits < 1).sum()), 'measured_images': len(subset),
                  'overall': report(idx, sub_rows, base, packs, old, labels_611, same)}
        bands = defaultdict(list)
        for i, r in enumerate(sub_rows):
            for n in set(r['characters']) & labels_611:
                bands[roster[n]['band']].append(i)
        target_hits = Counter()
        for band, items in bands.items():
            names = {n for n in labels_611 if roster[n]['band'] == band}
            t = sum(len(set(sub_rows[i]['characters']) & names) for i in items)
            h = sum(len(set(packs['611'][i]) & set(sub_rows[i]['characters']) & names) for i in items)
            target_hits[band] = {'truth': t, 'hits': h, 'recall': round(h / t, 4) if t else None}
        result['recall_by_band'] = target_hits
        write('estimate/dev-estimate.json', result)
        print(json.dumps(result, ensure_ascii=False, indent=1), flush=True)

    if args.freeze:
        limits, artists = rule_thresholds(x, dev_rows, labels, weights, biases, np.ones(len(dev_rows), dtype=bool))
        kept = np.flatnonzero(limits < 1)
        folder = OUT / 'freeze'
        folder.mkdir(exist_ok=True)
        package = save_pack('wd-611', folder, [labels[j] for j in kept], weights[kept], biases[kept], 0.5,
                            [float(v) for v in limits[kept]])
        heads = [{'label': n, 'band': roster[n]['band'], 'threshold': float(limits[j]),
                  'dev_positive_artists': artists[j], 'train_positives': training[n]['positives'],
                  'kept': bool(limits[j] < 1)} for j, n in enumerate(labels)]
        path = write('freeze/heads.json', heads)
        freeze = {'rule': {'name': RULE[0], 'delta_logit': RULE[1]}, 'development_images': len(dev_rows),
                  'development_origins': origins, 'trained': len(labels), 'kept': int(len(kept)),
                  'kept_by_band': Counter(h['band'] for h in heads if h['kept']),
                  'not_trained': Counter(v.get('state') for v in training.values() if v.get('state') != 'trained'),
                  'package': package, 'heads_sha256': sha_file(path)}
        write('freeze/freeze.json', freeze)
        print(json.dumps(freeze, ensure_ascii=False, indent=1), flush=True)


if __name__ == '__main__':
    main()
