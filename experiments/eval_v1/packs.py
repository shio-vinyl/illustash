"""组装要回放或交给 #602 的扩展头包（参数原样拼接，不重训）。

- native：`illustash-wd-extension/1`（旧二进制只认这一版），257 头 + #534 的 205 头（统一阈值 0.8009），
  用于原生回放看 205 头当一级时的主归属变化；
- release：`illustash-wd-extension/2`（#602 的分级格式，逐头 thresholds + tiers），
  一级 = 257 头 + 205 头里过一级门槛的；二级 = 冻结的二级头 + 205 头里其余的。
"""

import argparse
import json

import numpy as np

from common import D534, DATA, PACK_257, load, sha_file, write
from colab_run import MODELS, TAG_SHA
from fit import save_pack
from score import THRESHOLD_534
from shortlist import read_pack


def heads_534():
    roster = {e['label']: e for e in load(DATA / 'roster/targets.json')}
    with np.load(D534 / 'evaluation/B-heads.npz', allow_pickle=False) as data:
        names = data['labels'].tolist()
        keep = [i for i, n in enumerate(names) if roster.get(n, {}).get('pack') == '534_index_covered']
        return [names[i] for i in keep], data['weight'][keep], data['bias'][keep]


def save_v2(path, labels, weights, biases, thresholds, tiers):
    header = {'format': 'illustash-wd-extension/2', 'wd_model_sha256': MODELS['wd_swinv2_v3_emb.onnx'],
              'wd_tags_sha256': TAG_SHA, 'labels': labels, 'thresholds': thresholds, 'tiers': tiers}
    with path.open('wb') as stream:
        stream.write(json.dumps(header, separators=(',', ':')).encode() + b'\n')
        stream.write(np.asarray(weights, dtype='<f4').tobytes())
        stream.write(np.asarray(biases, dtype='<f4').tobytes())
    return {'heads': len(labels), 'tier1': tiers.count(1), 'tier2': tiers.count(2),
            'bytes': path.stat().st_size, 'sha256': sha_file(path)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('native', 'release'))
    parser.add_argument('--with-t2', action='store_true', help='把 1,595 个二级头也放进包（没过二级预算时不放）')
    parser.add_argument('--drop-tier1', action='append', default=[], help='原生回放没过一级的 #534 头，降到二级')
    args = parser.parse_args()
    labels, w, b, t = read_pack(PACK_257)
    names, w5, b5 = heads_534()
    folder = DATA / 'packs'
    folder.mkdir(exist_ok=True)
    if args.mode == 'native':
        info = save_pack('t1_257_plus_h534', folder, labels + names, np.concatenate([w, w5]), np.r_[b, b5],
                         float(t[0]), [*map(float, t), *[THRESHOLD_534] * len(names)])
        write('packs/native.json', info)
        print(info)
        return
    gates = load(DATA / 'report/h534-per-head.json')
    t2_labels, t2_w, t2_b, t2_t = read_pack(DATA / 'tier2/tier2.bin')
    if not args.with_t2:
        t2_labels, t2_w, t2_b, t2_t = [], t2_w[:0], t2_b[:0], t2_t[:0]
    first = [n for n in names if not gates[n]['gate'] and n not in args.drop_tier1]
    rest = [n for n in names if n not in first]
    index = {n: i for i, n in enumerate(names)}
    all_labels = labels + first + t2_labels + rest
    weights = np.concatenate([w, w5[[index[n] for n in first]], t2_w, w5[[index[n] for n in rest]]])
    biases = np.concatenate([b, b5[[index[n] for n in first]], t2_b, b5[[index[n] for n in rest]]])
    thresholds = [*map(float, t), *[THRESHOLD_534] * len(first), *map(float, t2_t), *[THRESHOLD_534] * len(rest)]
    tiers = [1] * (len(labels) + len(first)) + [2] * (len(t2_labels) + len(rest))
    name = 'character-pack-wd-candidate' + ('-with-t2' if args.with_t2 else '')
    info = save_v2(folder / f'{name}.bin', all_labels, weights, biases, thresholds, tiers)
    write(f'packs/{name}.json', dict(info, tier1_from_534=len(first), tier2_from_534=len(rest),
                                       tier2_from_551=len(t2_labels), dropped_after_native=args.drop_tier1))
    print(info)


if __name__ == '__main__':
    main()
