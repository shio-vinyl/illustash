"""下一版角色资料包 WD 部分的制包脚本（验收通过后才出正式包）。

组成（用户 2026-11-18 定）：一级 296 头，取 0.3 WD 部分原样；二级 145 头，是 0.3 的 166 头按 R1 δ=2
重定阈值后的；二级 1,432 头，是 R1 δ=2 新选的。合成 1,873 头的 `illustash-wd-extension/2` 包。
CCIP 部分沿用 0.3 的 92 锚点文件，不重做。

只拼接冻结的参数和阈值，不重训、不改阈值。先逐项核对输入 SHA-256，再按 #602（PR #614）加载时的规则自查：
labels 不重复、thresholds 在 0–1、tiers 只有 1 和 2、系数长度 = 头数 × 1025 × 4。
用法：next_pack.py <输出目录>，正式出包前先写到草稿目录试一遍。
"""

import argparse
import json
from pathlib import Path

import numpy as np

from common import DATA, sha_file, write
from colab_run import MODELS, TAG_SHA
from packs import save_v2
from shortlist import read_pack

WD03 = DATA / 'packs/character-pack-wd-candidate.bin'
WD03_SHA = 'd43b162d32e85e97cb59fea124634f55a7a97ac0ab46c27c2666043426f3ed48'
T2_145 = DATA / 'tier2-next/tier2-166-retightened.bin'
T2_145_SHA = '5f822f76241e88089c6441c6be5d08a4d67da5d296ee7fcd7199ae9836afd697'
T2_1432 = DATA / 'tier2-next/tier2-next.bin'
T2_1432_SHA = '0b1615f4bc8dab3a124a710760d20198cf0fe5f4688e283656a61e5d9680f55a'
CCIP03 = DATA / 'deliver/character-pack-0.3/ccip-longtail-0.3.zip'
CCIP03_SHA = 'a254a3d6f0cbba431dc973cc3f8626c68c1e9efce189c853fb0baa711b1873aa'


def read_v2(path):
    with path.open('rb') as stream:
        header = json.loads(stream.readline())
        values = np.frombuffer(stream.read(), dtype='<f4')
    n = len(header['labels'])
    return header, values[:-n].reshape(n, 1024), values[-n:]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    for path, expected in ((WD03, WD03_SHA), (T2_145, T2_145_SHA), (T2_1432, T2_1432_SHA), (CCIP03, CCIP03_SHA)):
        if sha_file(path) != expected:
            raise ValueError(f'冻结输入摘要不符：{path}')
    header, w03, b03 = read_v2(WD03)
    tier1 = [i for i, t in enumerate(header['tiers']) if t == 1]
    if len(tier1) != 296:
        raise ValueError('0.3 的一级头数不是 296')
    labels = [header['labels'][i] for i in tier1]
    weights, biases, thresholds = [w03[tier1]], [b03[tier1]], [header['thresholds'][i] for i in tier1]
    for path in (T2_145, T2_1432):
        names, w, b, t = read_pack(path)
        labels += names
        weights.append(w)
        biases.append(b)
        thresholds += [float(x) for x in t]
    tiers = [1] * 296 + [2] * (len(labels) - 296)
    problems = []
    if len(labels) != 1873:
        problems.append(f'头数 {len(labels)} 不是 1,873')
    if len(set(labels)) != len(labels):
        problems.append('labels 有重复')
    if not all(0.0 <= x <= 1.0 for x in thresholds):
        problems.append('有阈值不在 0–1')
    if problems:
        raise ValueError('；'.join(problems))
    args.output.mkdir(parents=True, exist_ok=True)
    path = args.output / 'wd-extension.bin'
    info = save_v2(path, labels, np.concatenate(weights), np.concatenate(biases), thresholds, tiers)
    if path.stat().st_size - len(path.read_bytes().split(b'\n', 1)[0]) - 1 != len(labels) * 1025 * 4:
        raise ValueError('系数长度不符')
    check, *_ = read_v2(path)
    if check['format'] != 'illustash-wd-extension/2' or check['wd_model_sha256'] != MODELS['wd_swinv2_v3_emb.onnx'] \
            or check['wd_tags_sha256'] != TAG_SHA or set(check['tiers']) != {1, 2}:
        raise ValueError('包头核对不符')
    info.update(inputs={'wd_03': WD03_SHA, 'tier2_145': T2_145_SHA, 'tier2_1432': T2_1432_SHA, 'ccip_03': CCIP03_SHA},
                ccip_file=str(CCIP03.name))
    write(args.output / 'wd-extension.json', info)
    print(json.dumps(info, ensure_ascii=False))


if __name__ == '__main__':
    main()
