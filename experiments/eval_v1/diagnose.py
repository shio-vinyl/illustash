"""二级误报的分布诊断（只描述，不据此改阈值或名单）。"""

from collections import Counter
import json

from common import DATA, load, write
from report import Data

data = Data()
heads = {h['label']: h for h in load(DATA / 'tier2/heads.json') if h['kept']}
base = data.stack('wd', 'index', 'lt_v1', 't1_257')
t2 = data.sources['t2']
per_head = Counter()
per_image_heads = []
bucket = lambda t: '0.5(下限)' if t <= 0.5 else '0.5–0.9' if t < 0.9 else '0.9–0.99' if t < 0.99 else '≥0.99'
fp_bucket, hit_bucket = Counter(), Counter()
for i in range(len(data.rows)):
    for name in t2[i]:
        b = bucket(heads[name]['threshold'])
        if name in data.truth[i]:
            hit_bucket[b] += 1
        elif data.old[i] and name not in base[i]:
            per_head[name] += 1
            fp_bucket[b] += 1
counts = sorted(per_head.values(), reverse=True)
heads_by_bucket = Counter(bucket(h['threshold']) for h in heads.values())
result = {
    'heads_with_introduced_old_fp': len(per_head), 'introduced_old_fp_candidates': sum(counts),
    'top10_share': round(sum(counts[:10]) / max(1, sum(counts)), 3),
    'top50_share': round(sum(counts[:50]) / max(1, sum(counts)), 3),
    'max_per_head': counts[0] if counts else 0,
    'by_threshold_bucket': {b: {'heads': heads_by_bucket[b], 'hits': hit_bucket[b], 'introduced_old_fp': fp_bucket[b]}
                            for b in sorted(heads_by_bucket)},
}
write('report/tier2-diagnose.json', result)
print(json.dumps(result, ensure_ascii=False, indent=1))
