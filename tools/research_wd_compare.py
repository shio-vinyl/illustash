"""按现行 DirectML 容差比较 WD 输出（#359）；只接受完整、同形、有限的二维数组。

用法：python tools/research_wd_compare.py REFERENCE.npz CANDIDATE.npz tags/tags_v3.csv
NPZ 包含 probs（图片×标签）、emb（图片×向量）；输入图片与行顺序必须由调用方保证相同。
不通过时以非零状态退出；阈值集合、0.001 保存边界和四位小数均按既有验收口径报告。
"""
import argparse
import csv
import json
import numpy as np


def compare(reference, candidate, tags):
    arrays = {}
    for name in ('probs', 'emb'):
        a, b = reference[name], candidate[name]
        if a.ndim != 2 or a.shape != b.shape or not all(a.shape):
            raise ValueError(f'{name}: 数量或维度不一致：{a.shape} / {b.shape}')
        if not np.isfinite(a).all() or not np.isfinite(b).all():
            raise ValueError(f'{name}: 含 NaN 或无穷值')
        arrays[name] = (a.astype(np.float64), b.astype(np.float64))
    p, q = arrays['probs']
    e, f = arrays['emb']
    if len(p) != len(e) or p.shape[1] != len(tags):
        raise ValueError('标签表或向量的行数不匹配')
    en, fn = np.linalg.norm(e, axis=1), np.linalg.norm(f, axis=1)
    if np.any(en == 0) or np.any(fn == 0):
        raise ValueError('不能比较零向量的余弦')
    cosine = (e*f).sum(1)/en/fn
    difference = np.abs(p-q)
    keep = np.array([t['category'] in ('0', '4', '9') for t in tags])

    def stored(values):
        return {(i,j): round(float(np.float32(values[i,j])), 4)
                for i,j in zip(*np.nonzero((values >= .001) & keep))}

    a, b = stored(p), stored(q)
    inout = a.keys() ^ b.keys()
    boundary_ok = all(abs(a.get(k, b.get(k))-.001) < 2e-5+5e-5 for k in inout)
    confidence = max((abs(a[k]-b[k]) for k in a.keys() & b.keys()), default=0)
    cuts = {str(c): len({k for k,v in a.items() if v >= c} ^ {k for k,v in b.items() if v >= c})
            for c in (.2, .35, .5, .85)}
    passed = bool(difference.max() <= 2e-5 and cosine.min() >= .9999999999 and boundary_ok
                  and confidence <= 1e-4+1e-9 and not any(cuts.values()))
    return dict(images=len(p), probs_max_abs=float(difference.max()), emb_cos_min=float(cosine.min()),
                stored_inout=len(inout), stored_inout_at_0001_boundary=boundary_ok,
                stored_last_digit_changed=sum(a[k] != b[k] for k in a.keys() & b.keys()),
                stored_conf_max_diff=confidence, set_changes_at_cuts=cuts,
                raw_threshold_flips={str(c): int(((p >= c) != (q >= c))[:,keep].sum())
                                     for c in (.2, .35, .5, .85)},
                pass_dml_bounds=passed)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reference')
    parser.add_argument('candidate')
    parser.add_argument('tags')
    args = parser.parse_args()
    with open(args.tags, newline='', encoding='utf-8') as stream:
        tags = list(csv.DictReader(stream))
    with np.load(args.reference, allow_pickle=False) as reference, np.load(args.candidate, allow_pickle=False) as candidate:
        result = compare(reference, candidate, tags)
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result['pass_dml_bounds'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
