"""角色资料包 0.5 的 WD 部分（用户 2026-11-18 定）。

组成：0.4 的 WD 部分原样（1,873 头：一级 296、二级 1,577），后面接 #611 二期验收通过的 5,986 头，全部二级。
合成 7,859 头的 `illustash-wd-extension/2` 包。CCIP 部分沿用 0.3 的 92 锚点文件，不重做。

只拼接冻结的参数和阈值，不重训、不改阈值：二期验收量的就是这两份包并在一起的结果。先核对两个输入的 SHA-256，
再按加载规则自查：labels 不重复、thresholds 在 0–1、tiers 只有 1 和 2、系数长度 = 头数 × 1025 × 4、
前 1,873 头的参数与 0.4 逐字节相同。
用法：run.sh pack_05 <输出文件>
"""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np

from fit_dev import OUT, PACK_04, PACK_04_SHA, read_v2
from score_accept import FROZEN_611

PACK_611 = OUT / 'freeze/wd-611.bin'


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    for path, expected in ((PACK_04, PACK_04_SHA), (PACK_611, FROZEN_611)):
        if sha(path) != expected:
            raise ValueError(f'冻结输入摘要不符：{path}')
    h04, w04, b04 = read_v2(PACK_04)
    h611, w611, b611 = read_v2(PACK_611)
    if h04['format'] != 'illustash-wd-extension/2' or h611['format'] != 'illustash-wd-extension/1':
        raise ValueError('输入格式不符')
    for key in ('wd_model_sha256', 'wd_tags_sha256'):
        if h04[key] != h611[key]:
            raise ValueError(f'基础模型不一致：{key}')
    labels = h04['labels'] + h611['labels']
    thresholds = h04['thresholds'] + h611['thresholds']
    tiers = h04['tiers'] + [2] * len(h611['labels'])
    problems = []
    if (len(h04['labels']), len(h611['labels'])) != (1873, 5986):
        problems.append('头数不符')
    if len(set(labels)) != len(labels):
        problems.append('labels 有重复')
    if not all(0.0 <= x <= 1.0 for x in thresholds):
        problems.append('有阈值不在 0–1')
    if set(tiers) != {1, 2} or tiers.count(1) != 296:
        problems.append('级别不符')
    if problems:
        raise ValueError('；'.join(problems))
    header = {'format': 'illustash-wd-extension/2', 'wd_model_sha256': h04['wd_model_sha256'],
              'wd_tags_sha256': h04['wd_tags_sha256'], 'labels': labels, 'thresholds': thresholds, 'tiers': tiers}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.output.with_suffix(args.output.suffix + '.partial')
    with temporary.open('wb') as stream:
        stream.write(json.dumps(header, separators=(',', ':')).encode() + b'\n')
        stream.write(np.concatenate([w04, w611]).astype('<f4').tobytes())
        stream.write(np.concatenate([b04, b611]).astype('<f4').tobytes())
    temporary.replace(args.output)
    check, w, b = read_v2(args.output)
    body = args.output.stat().st_size - len(args.output.read_bytes().split(b'\n', 1)[0]) - 1
    if body != len(labels) * 1025 * 4:
        raise ValueError('系数长度不符')
    if check != header or w[:1873].tobytes() != w04.tobytes() or b[:1873].tobytes() != b04.tobytes() \
            or w[1873:].tobytes() != w611.tobytes() or b[1873:].tobytes() != b611.tobytes():
        raise ValueError('回读核对不符')
    info = {'heads': len(labels), 'tier1': tiers.count(1), 'tier2': tiers.count(2),
            'bytes': args.output.stat().st_size, 'sha256': sha(args.output),
            'inputs': {'wd_04': PACK_04_SHA, 'wd_611': FROZEN_611}}
    print(json.dumps(info, ensure_ascii=False))


if __name__ == '__main__':
    main()
