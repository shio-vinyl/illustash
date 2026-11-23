"""在冻结的共用评测集上逐路给出候选（离线，和生产同一套打分）。

- WD：原 WD 角色标签，≥0.35（与 #534 基线一致）；
- 精选索引：完整 7,367 个锚点，生产锚点扫描（最近九项、别名归并、精选 p90）；
- 长尾锚点：#557 v1 的 54 个（增补 p75），可另加新一轮的锚点包；
- 扩展头：f64 累加、sigmoid 后舍入到 f32，与各自冻结的逐头阈值比较。

输出 scores/<名字>.json：每张图 {角色: 分数}，只含过阈值的候选。
"""

import argparse
import csv
import json
from pathlib import Path

import numpy as np

from common import (A557, A557_SHA, CCIP_INDEX, COVERAGE, D534, D557, DATA, INDEX_SHA, PACK_257, PACK_257_SHA,
                    load, sha_file, write)
from coverage import canonical
from evaluate import baseline_entries, matrix_file, native_scan, wd_projection
from expansion_fit import matrix
from shortlist import read_pack

SCAN = D534 / 'native-target/debug/illustash-issue-534-validation'
WD_THRESHOLD = 0.35
THRESHOLD_534 = 0.8008725047111511


def evaluation():
    receipt = load(DATA / 'eval/receipt.json')
    for name, key in (('manifest.json', 'manifest_sha256'), ('features.npz', 'features_sha256')):
        if sha_file(DATA / 'eval' / name) != receipt[key]:
            raise ValueError(f'评测集 {name} 与冻结校验值不符')
    rows = load(DATA / 'eval/manifest.json')
    with np.load(DATA / 'eval/features.npz', allow_pickle=False) as data:
        arrays = {k: data[k] for k in data.files}
    if arrays['ids'].tolist() != [r['id'] for r in rows]:
        raise ValueError('评测特征与清单顺序不符')
    return rows, arrays


def index_entries():
    if sha_file(CCIP_INDEX / 'index.npz') != INDEX_SHA:
        raise ValueError('完整精选索引摘要不符')
    with np.load(CCIP_INDEX / 'index.npz', allow_pickle=False) as packed:
        entries = baseline_entries(load(CCIP_INDEX / 'loaded-characters.json'), packed)
        anchors = packed['anchors']
    return entries, anchors


def longtail(folder):
    """读一个长尾锚点包目录（A-metadata.json + A-anchors.f32）。"""
    metadata = load(folder / 'A-metadata.json')
    anchors = np.fromfile(folder / 'A-anchors.f32', dtype='<f4').reshape(-1, 768)
    return metadata, anchors


def ccip(name, vectors, entries, anchors, aliases):
    location = DATA / 'scores' / f'scan-{name}'
    location.mkdir(parents=True, exist_ok=True)
    features = location / 'vectors.f32'
    matrix_file(features, vectors)
    out = location / 'claims.tsv'
    receipt = native_scan(SCAN, location, entries, anchors, features, out)
    if receipt['live_claim_difference_images']:
        raise ValueError('批量扫描与单图认领不一致')
    tag = {e['id']: canonical(e['tag'], aliases) for e in entries}
    patched = {e['id'] for e in entries if e['patched']}
    index, extra = [{} for _ in vectors], [{} for _ in vectors]
    with out.open() as stream:
        for i, _anchor, entity, distance, limit, rounded in csv.reader(stream, delimiter='\t'):
            if np.float32(limit) > 0 and np.float32(distance) <= np.float32(limit):
                bucket = extra if entity in patched else index
                key = tag[entity]
                bucket[int(i)][key] = max(bucket[int(i)].get(key, 0.0), round(1 - float(rounded), 4))
    return index, extra, receipt


def heads(x, labels, weights, biases, thresholds):
    scores = matrix(x, weights, biases)
    result = []
    for row in scores:
        hit = np.flatnonzero(row >= thresholds.astype(np.float32))
        result.append({labels[j]: round(float(row[j]), 6) for j in hit})
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--longtail', type=Path, action='append', default=[],
                        help='另加的长尾锚点包目录，名字取目录名')
    parser.add_argument('--pack', type=Path, action='append', default=[], help='另加的扩展头包（.bin）')
    args = parser.parse_args()
    rows, arrays = evaluation()
    aliases = load(COVERAGE / 'alias-map.json')
    tags = arrays['wd_tags'].tolist()
    out = {}
    out['wd'] = [{k: v for k, v in wd_projection(raw, tags, aliases).items() if v >= WD_THRESHOLD}
                 for raw in arrays['wd_scores']]
    if sha_file(A557) != A557_SHA:
        raise ValueError('长尾 v1 包摘要不符')
    entries, anchors = index_entries()
    v1_meta, v1_anchors = longtail(D557)
    out['index'], out['lt_v1'], scan = ccip('v1', arrays['ccip_vectors'], entries + v1_meta,
                                            np.concatenate([anchors, v1_anchors]), aliases)
    receipts = {'scan_v1': scan}
    for folder in args.longtail:
        meta, extra = longtail(folder)
        index, added, scan = ccip(folder.name, arrays['ccip_vectors'], entries + v1_meta + meta,
                                  np.concatenate([anchors, v1_anchors, extra]), aliases)
        new_ids = {canonical(e['tag'], aliases) for e in meta}
        out[f'lt_{folder.name}'] = [{k: v for k, v in item.items() if k in new_ids} for item in added]
        out[f'index_with_{folder.name}'] = index
        receipts[f'scan_{folder.name}'] = scan
    x = arrays['wd_vectors']
    if sha_file(PACK_257) != PACK_257_SHA:
        raise ValueError('257 头包摘要不符')
    labels, w, b, t = read_pack(PACK_257)
    out['t1_257'] = heads(x, labels, w, b, t)
    with np.load(D534 / 'evaluation/B-heads.npz', allow_pickle=False) as data:
        names534 = data['labels'].tolist()
        roster = {e['label']: e for e in load(DATA / 'roster/targets.json')}
        # 只取精选索引已覆盖、没随 228 头发布的那 205 个；参数和统一阈值沿用 #534 冻结值。
        keep = [i for i, n in enumerate(names534) if roster.get(n, {}).get('pack') == '534_index_covered']
        out['h534'] = heads(x, [names534[i] for i in keep], data['weight'][keep], data['bias'][keep],
                            np.full(len(keep), THRESHOLD_534))
    t2 = read_pack(DATA / 'tier2/tier2.bin')
    out['t2'] = heads(x, *t2)
    for path in args.pack:
        out[f'pack_{path.stem}'] = heads(x, *read_pack(path))
    for name, values in out.items():
        write(f'scores/{name}.json', values)
    write('scores/receipt.json', {
        'sources': {name: sha_file(DATA / f'scores/{name}.json') for name in out},
        'eval_manifest_sha256': sha_file(DATA / 'eval/manifest.json'), 'scan': receipts,
        'packs': {'t1_257': PACK_257_SHA, 't2': sha_file(DATA / 'tier2/tier2.bin'),
                  'h534_heads_npz': sha_file(D534 / 'evaluation/B-heads.npz'), 'lt_v1': A557_SHA,
                  **{f'pack_{p.stem}': sha_file(p) for p in args.pack}},
    })
    print({name: sum(map(len, values)) for name, values in out.items()}, flush=True)


if __name__ == '__main__':
    main()
