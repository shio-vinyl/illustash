"""按冻结的选头规则和逐头阈值规则冻结 #551 的二级头（在看共用评测集之前完成）。

- 选头：2,100 头包里除 257 头一级候选以外的全部已训头，参数原样沿用（不重训）。
- 阈值：#551 257 头一轮的逐头规则 `expansion_fit.calibrate`：
  max(0.5, 开发负例最高分 + 0.01, 开发正例下四分位分数)，封顶 1.0，按 f32 冻结。
  开发数据 = 历次取样里所有非训练图（#551 各轮校准、留出、补样、257 头盲测，#534 的 5,410 张盲测和
  校准，#557 的开发和盲测），它们都已在之前的验收里用过，这一轮只当开发数据。
- 阈值 = 1.0 的头（开发里没有正例，或负例最高分 ≥ 0.99）永远不会触发，不放进包，单列数量。
"""

from collections import Counter
import json

import numpy as np

from common import D534, D551, D557, DATA, PACK_2100, PACK_2100_SHA, PACK_257, PACK_257_SHA, load, sha_file, write
from expansion_fit import calibrate, matrix
from fit import old_test_inputs, save_pack
from shortlist import read_pack

SOURCES = [(D551 / 'prepared', '551_original'), (D551 / 'shortlist-blind', '551_shortlist_blind'),
           (D551 / 'expansion/development', '551_257_development'), (D551 / 'expansion/covered', '551_257_covered'),
           (D551 / 'expansion/blind', '551_257_blind')]
# 257 头一轮的定向混淆补样（expansion/mined）用了已更名的历史标签，当时判为不能当真实负例，这里同样不用。


def dev_data():
    rows, values, identities = [], [], {}
    for folder, origin in SOURCES:
        receipt = load(folder / 'prepared.json')
        for kind, name in (('manifest', 'manifest.json'), ('features', 'features.npz')):
            if sha_file(folder / name) != receipt[f'{kind}_sha256']:
                raise ValueError(f'{origin} {kind} 摘要不符')
            identities[f'{origin}/{kind}'] = receipt[f'{kind}_sha256']
        batch = load(folder / 'manifest.json')
        with np.load(folder / 'features.npz', allow_pickle=False) as data:
            if data['ids'].tolist() != [r['id'] for r in batch]:
                raise ValueError('开发特征与元数据顺序不符')
            keep = [i for i, r in enumerate(batch) if not (origin == '551_original' and r['split'] == 'train')]
            values.append(data['wd_vectors'][keep])
        rows.extend(dict(batch[i], origin=origin) for i in keep)
    older, old_values = old_test_inputs(D534)
    rows.extend(dict(r, origin='534_test_5410') for r in older)
    values.append(old_values)
    identities['534_test_5410/manifest'] = sha_file(D534 / 'evaluation/manifest.json')
    for stage in ('development', 'test'):
        accepted = load(D557 / f'{stage}-accepted.json')
        with np.load(D557 / f'{stage}-features.npz', allow_pickle=False) as data:
            position = {int(p): i for i, p in enumerate(data['ids'])}
            ix = [position[r['id']] for r in accepted]
            values.append(data['wd_vectors'][ix])
        rows.extend(dict(r, origin=f'557_{stage}') for r in accepted)
        identities[f'557_{stage}/features'] = sha_file(D557 / f'{stage}-features.npz')
    unique = {}
    for i, row in enumerate(rows):
        unique.setdefault(row['id'], i)
    order = sorted(unique.values())
    return [rows[i] for i in order], np.concatenate(values)[order], identities


def main():
    if sha_file(PACK_2100) != PACK_2100_SHA or sha_file(PACK_257) != PACK_257_SHA:
        raise ValueError('冻结包摘要不符')
    if (DATA / 'tier2/freeze.json').exists():
        raise ValueError('二级头已冻结')
    names, weights, biases, _ = read_pack(PACK_2100)
    tier1, *_ = read_pack(PACK_257)
    chosen = [j for j, n in enumerate(names) if n not in set(tier1)]
    rows, x, identities = dev_data()
    scores = matrix(x, weights[chosen], biases[chosen])
    truth = np.array([[names[j] in r['characters'] for j in chosen] for r in rows], dtype=bool)
    every = np.ones(len(rows), dtype=bool)
    thresholds = np.array([calibrate(scores[:, k], truth[:, k], every) for k in range(len(chosen))])
    live = thresholds < 1.0
    kept = [chosen[k] for k in np.flatnonzero(live)]
    labels = [names[j] for j in kept]
    (DATA / 'tier2').mkdir(parents=True, exist_ok=True)
    package = save_pack('tier2', DATA / 'tier2', labels, weights[kept], biases[kept],
                        float(np.float32(0.5)), [float(t) for t in thresholds[live]])
    positives = truth.sum(axis=0)
    per_head = [{'label': names[j], 'threshold': float(thresholds[k]), 'dev_positive_images': int(positives[k]),
                 'dev_positive_artists': len({rows[i]['artist'] for i in np.flatnonzero(truth[:, k])}),
                 'max_dev_negative': float(scores[~truth[:, k], k].max(initial=0.0)),
                 'kept': bool(live[k])} for k, j in enumerate(chosen)]
    write('tier2/heads.json', per_head)
    reasons = Counter('no_dev_positive' if not h['dev_positive_images'] else
                      'negative_too_high' if h['max_dev_negative'] + 0.01 >= 1.0 else 'kept' if h['kept'] else
                      'other' for h in per_head)
    freeze = {
        'rule': 'tier2 = pack_2100 minus tier1_257; threshold = expansion_fit.calibrate over all non-train '
                'development images; heads with threshold 1.0 dropped',
        'pack_2100_sha256': PACK_2100_SHA, 'tier1_257_sha256': PACK_257_SHA,
        'development_images': len(rows), 'development_origins': Counter(r['origin'] for r in rows),
        'inputs': identities, 'candidates': len(chosen), 'kept': len(labels), 'reasons': reasons,
        'threshold_quantiles': {q: float(np.quantile(thresholds[live], q)) for q in (0.1, 0.25, 0.5, 0.75, 0.9)},
        'package': package, 'heads_sha256': sha_file(DATA / 'tier2/heads.json'),
    }
    write('tier2/freeze.json', freeze)
    print(json.dumps(freeze, ensure_ascii=False, indent=1), flush=True)


if __name__ == '__main__':
    main()
