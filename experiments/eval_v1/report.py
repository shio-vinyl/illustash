"""共用评测集 v1 上的两级验收报数（离线候选层；主归属另见原生回放）。

口径（#551 2026-11-16 的两级验收）：
- 叠加基线 B1 = WD 标签 + 完整精选索引 + 长尾 v1（54 锚点）+ 一级 257 头；
- 「旧角色图」= 真值里至少有一个 B1 已覆盖角色（WD 词表、精选索引、257 头、v1 锚点）的图；
- 新增含错误候选的图：加上某个包后，该包给出了真值里没有、B1 也没给的候选；
- 精确率按候选条计；召回按真值条计，分母含没进包的角色；
- 区间：每位画师在评测集里只有一张图，按画师聚类重抽样即按图重抽样，2,000 次，种子 551。
"""

from collections import Counter, defaultdict
import argparse
import json

import numpy as np

from common import DATA, load, priority, sha_file, write

RNG_SEED = 551
REPS = 2000
POSITIVE_GROUPS = {'head_solo', 'head_other', 'tail_solo', 'tail_other'}
CONFUSION_GROUPS = {'same_work_visual_old', 'same_work_other_old', 'same_work_multi_old'}


def interval(flags):
    flags = np.asarray(flags, dtype=float)
    if not len(flags):
        return None
    rng = np.random.default_rng(RNG_SEED)
    draws = rng.integers(0, len(flags), size=(REPS, len(flags)))
    means = flags[draws].mean(axis=1)
    return {'rate': float(flags.mean()), 'low': float(np.quantile(means, 0.025)),
            'high': float(np.quantile(means, 0.975)), 'count': int(flags.sum()), 'images': len(flags)}


def ratio(k, n):
    return round(k / n, 4) if n else None


class Data:
    def __init__(self, extra=()):
        self.rows = load(DATA / 'eval/manifest.json')
        receipt = load(DATA / 'scores/receipt.json')
        if receipt['eval_manifest_sha256'] != sha_file(DATA / 'eval/manifest.json'):
            raise ValueError('打分结果与评测集不对应')
        self.sources = {}
        for name in receipt['sources']:
            if sha_file(DATA / f'scores/{name}.json') != receipt['sources'][name]:
                raise ValueError(f'打分结果 {name} 被改动')
            self.sources[name] = [set(item) for item in load(DATA / f'scores/{name}.json')]
        self.truth = [set(r['characters']) for r in self.rows]
        self.targets = {e['label']: e for e in load(DATA / 'roster/targets.json')}
        self.frame = load(DATA / 'frame.json')
        covered = set(load(DATA / 'roster/covered.json'))
        self.t1 = {n for n, e in self.targets.items() if e.get('tier1_257')}
        self.lt1 = {n for n, e in self.targets.items() if e['route'] == 'ccip_longtail'}
        self.covered_b1 = covered | self.t1 | self.lt1
        self.old = [bool(t & self.covered_b1) for t in self.truth]

    def stack(self, *names):
        return [set().union(*(self.sources[n][i] for n in names)) for i in range(len(self.rows))]


def added(data, base, package, subset):
    flags = [bool(package[i] - data.truth[i] - base[i]) for i in subset]
    return interval(flags)


def own_precision(data, package, subset, labels=None):
    tp = fp = 0
    for i in subset:
        mine = package[i] if labels is None else package[i] & labels
        tp += len(mine & data.truth[i])
        fp += len(mine - data.truth[i])
    return {'tp': tp, 'fp': fp, 'precision': ratio(tp, tp + fp)}


def recall(data, predicted, labels, subset=None):
    truth = hits = 0
    for i in subset if subset is not None else range(len(data.rows)):
        actual = data.truth[i] & labels
        truth += len(actual)
        hits += len(actual & predicted[i])
    return {'truth': truth, 'hits': hits, 'recall': ratio(hits, truth)}


def confusion_level(data):
    """每头的同作品混淆程度：评测混淆库里与该头主作品相同的旧角色有几个（只看元数据）。"""
    by_work = defaultdict(set)
    for row in data.rows:
        if row['group'] in CONFUSION_GROUPS:
            for name in row['characters']:
                by_work[row.get('work')].add(name)
    level = {}
    for name, works in data.frame['works'].items():
        olds = set().union(*(by_work.get(w, set()) for w in works)) if works else set()
        level[name] = 'none' if not olds else 'low_1_9' if len(olds) < 10 else 'high_10plus'
    return level


def subsets(data):
    every = range(len(data.rows))
    groups = defaultdict(list)
    for i, row in enumerate(data.rows):
        groups[row['group']].append(i)
    old = [i for i in every if data.old[i]]
    return {
        'all': list(every), 'old_character_images': old,
        'same_work_confusion': [i for i in old if data.rows[i]['group'] in CONFUSION_GROUPS],
        'same_work_visual': [i for i in old if data.rows[i]['group'] == 'same_work_visual_old'],
        'multi_person_old': [i for i in old if len(data.truth[i]) >= 2 or data.rows[i]['group'].startswith('multi')],
        'unrelated_old': groups['unrelated_old'], 'no_character': groups['no_character'],
        'other_character': groups['other_character'], 'multi_other': groups['multi_other'],
        'target_positives': [i for i in every if data.rows[i]['group'] in POSITIVE_GROUPS],
    }


def package_report(data, base_names, package_names, labels, sample_heads=6, seed='eval-v1-heads'):
    base = data.stack(*base_names)
    package = data.stack(*package_names)
    parts = subsets(data)
    result = {'base': base_names, 'package': package_names, 'heads': len(labels)}
    result['introduced_false_images'] = {name: added(data, base, package, idx) for name, idx in parts.items()}
    positives = [i for i in parts['target_positives'] if data.rows[i].get('focus') in labels]
    result['own_precision_on_target_positives'] = own_precision(data, package, positives)
    result['own_precision_all_images'] = own_precision(data, package, parts['all'])
    result['recall_own'] = recall(data, package, labels)
    together = [base[i] | package[i] for i in range(len(data.rows))]
    result['recall_stack'] = recall(data, together, labels)
    result['recall_base'] = recall(data, base, labels)
    level = confusion_level(data)
    strata = defaultdict(list)
    for name in sorted(labels):
        entry = data.targets.get(name, {})
        strata[(entry.get('band', '?'), level.get(name, 'none'))].append(name)
    by_stratum = {}
    for (band, conf), names in sorted(strata.items()):
        chosen = set(names)
        cell = {'heads': len(names), 'recall_own': recall(data, package, chosen),
                'own_precision_all_images': own_precision(data, package, parts['all'], chosen)}
        flags = [bool((package[i] & chosen) - data.truth[i] - base[i]) for i in parts['old_character_images']]
        cell['introduced_false_old_images'] = int(sum(flags))
        picks = sorted(names, key=lambda n: priority(n, seed))[:sample_heads]
        cell['sampled_heads'] = []
        for k, name in enumerate(picks):
            one = {name}
            r = recall(data, package, one)
            p = own_precision(data, package, parts['all'], one)
            cell['sampled_heads'].append({'head': f'{band}/{conf}/S{k + 1}', 'truth': r['truth'], 'hits': r['hits'],
                                          'false_candidates': p['fp']})
        by_stratum[f'{band}|{conf}'] = cell
    result['strata'] = by_stratum
    return result, level


def per_head(data, base_names, package_names, labels):
    base = data.stack(*base_names)
    package = data.stack(*package_names)
    level = confusion_level(data)
    works = data.frame['works']
    out = {}
    for name in sorted(labels):
        positives = [i for i in range(len(data.rows)) if name in data.truth[i]]
        hits = [i for i in positives if name in package[i]]
        false = [i for i in range(len(data.rows)) if name in package[i] and name not in data.truth[i]]
        introduced = [i for i in false if name not in base[i]]
        own_works = set(works.get(name, []))
        negatives = {data.rows[i]['artist'] for i in range(len(data.rows))
                     if data.old[i] and name not in data.truth[i]
                     and own_works.intersection(data.rows[i]['copyrights'])}
        solo = [i for i in positives if data.rows[i]['solo'] and data.truth[i] == {name}]
        out[name] = {'positives': len(positives), 'positive_artists': len({data.rows[i]['artist'] for i in positives}),
                     'hits': len(hits), 'recall': ratio(len(hits), len(positives)),
                     'solo_truth': len(solo), 'solo_hits': sum(name in package[i] for i in solo),
                     'false_candidates': len(false), 'introduced_false': len(introduced),
                     'base_hits': sum(name in base[i] for i in positives),
                     'same_work_old_negative_artists': len(negatives), 'confusion': level.get(name, 'none'),
                     'band': data.targets.get(name, {}).get('band')}
    return out


def tier1_gate(record):
    reasons = []
    if record['positive_artists'] < 8:
        reasons.append('positive_artists<8')
    if record['hits'] < 4:
        reasons.append('hits<4')
    if record['recall'] is None or record['recall'] < 0.5:
        reasons.append('recall<50%')
    if record['introduced_false']:
        reasons.append('introduced_false>0')
    if record['same_work_old_negative_artists'] < 20:
        reasons.append('same_work_old_negative_artists<20')
    return reasons


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--longtail', default=None, help='长尾第二轮的来源名（scores/lt_<名>.json）')
    args = parser.parse_args()
    data = Data()
    b1 = ['wd', 'index', 'lt_v1', 't1_257']
    b0 = ['wd', 'index', 'lt_v1']
    t2_labels = {h['label'] for h in load(DATA / 'tier2/heads.json') if h['kept']}
    h534_labels = {n for n, e in data.targets.items() if e['pack'] == '534_index_covered'}
    report = {'eval': load(DATA / 'eval/receipt.json')['manifest_sha256'],
              'images': len(data.rows), 'old_character_images': sum(data.old)}
    report['tier1_257_over_b0'], _ = package_report(data, b0, ['t1_257'], data.t1)
    report['tier2_over_b1'], level = package_report(data, b1, ['t2'], t2_labels)
    heads534 = per_head(data, b1, ['h534'], h534_labels)
    gates = {name: tier1_gate(record) for name, record in heads534.items()}
    report['h534'] = {
        'heads': len(heads534), 'tier1_pass': sum(not g for g in gates.values()),
        'gate_failures': Counter(r for g in gates.values() for r in g),
        'only_index_vs_with_heads': {'index_only': recall(data, data.stack('index'), h534_labels),
                                     'b1': recall(data, data.stack(*b1), h534_labels),
                                     'b1_plus_heads': recall(data, data.stack(*b1, 'h534'), h534_labels),
                                     'heads_own_precision_all': own_precision(data, data.stack('h534'),
                                                                              range(len(data.rows)))},
        'package_over_b1': package_report(data, b1, ['h534'], h534_labels)[0],
    }
    write('report/h534-per-head.json', {n: dict(r, gate=gates[n]) for n, r in heads534.items()})
    # 整套叠加：二级头和 #534 的 205 头一起加在 B1 上（205 头里过一级的照样只按候选计）。
    report['t2_and_h534_over_b1'] = package_report(data, b1, ['t2', 'h534'], t2_labels | h534_labels)[0]
    if args.longtail:
        from coverage import canonical
        from common import COVERAGE
        aliases = load(COVERAGE / 'alias-map.json')
        lt_labels = {canonical(e['tag'], aliases) for e in load(DATA / args.longtail / 'A-metadata.json')}
        # 第二轮锚点加进来以后，精选索引自己的候选也可能因最近九项被挤掉，所以基线换成同一次扫描的 index。
        base_lt = ['wd', f'index_with_{args.longtail}', 'lt_v1', 't1_257']
        report['longtail_over_b1'] = package_report(data, base_lt, [f'lt_{args.longtail}'], lt_labels)[0]
        report['longtail_index_displacement'] = {
            'index_candidates_before': sum(map(len, data.sources['index'])),
            'index_candidates_after': sum(map(len, data.sources[f'index_with_{args.longtail}'])),
            'correct_index_candidates_lost': sum(len((a - b) & data.truth[i]) for i, (a, b) in enumerate(
                zip(data.sources['index'], data.sources[f'index_with_{args.longtail}']))),
            'wrong_index_candidates_removed': sum(len((a - b) - data.truth[i]) for i, (a, b) in enumerate(
                zip(data.sources['index'], data.sources[f'index_with_{args.longtail}']))),
        }
        report['longtail_per_anchor'] = per_head(data, base_lt, [f'lt_{args.longtail}'], lt_labels)
        report['longtail_v1_over_wd_index'] = package_report(data, ['wd', 'index'], ['lt_v1'], data.lt1)[0]
    # #602 要的量：每张图的扩展候选命中数（二级单列），按 5 万张图的规模估证据行数（每条命中一行证据、一行候选）。
    volume = {}
    for name in ('t1_257', 't2', 'h534'):
        counts = np.array([len(item) for item in data.sources[name]])
        volume[name] = {'mean_hits_per_image': round(float(counts.mean()), 4), 'max_hits_per_image': int(counts.max()),
                        'images_with_hits': int((counts > 0).sum()),
                        'p99_hits_per_image': float(np.quantile(counts, 0.99)),
                        'evidence_rows_per_50k_images': int(round(counts.mean() * 50000))}
    report['hit_volume'] = volume
    write('report/report.json', report)
    write('report/tier2-per-head.json', per_head(data, b1, ['t2'], t2_labels))
    print(json.dumps(report, ensure_ascii=False, indent=1)[:20000], flush=True)


if __name__ == '__main__':
    main()
