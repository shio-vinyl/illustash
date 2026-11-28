"""生成目录修订抽查用的证据和覆盖率。输出留在制作工作目录。"""
import collections
import random

from tools.character_catalog import series

SAMPLE_SEED = 848
SAMPLE_PER_SERIES = 5


def revision(previous, columns, rows, matches, work_by_tag):
    old_rows = (dict(zip(previous['columns'], raw, strict=True)) for raw in previous.get('characters', []))
    old = {row['id']: row for row in old_rows}
    by_work = collections.defaultdict(list)
    for raw in rows:
        row = dict(zip(columns, raw, strict=True))
        tag = row['tag']
        prior = old.get(row['id'], {})
        by_work[work_by_tag[tag]].append((row, prior))

    def coverage(group):
        return {'total': len(group),
                'previous': sum(old.get('bangumi') is not None for _, old in group),
                'current': sum(new['bangumi'] is not None for new, _ in group)}

    rng = random.Random(SAMPLE_SEED)
    report = {'seed': SAMPLE_SEED, 'overall': coverage(list(item for group in by_work.values() for item in group)),
              'series': {}, 'changed_bangumi': []}
    for work in sorted(series.TITLES):
        group = by_work[work]
        additions = [row['tag'] for row, old in group
                     if row['bangumi'] is not None and row['bangumi'] != old.get('bangumi')]
        sample = sorted(rng.sample(sorted(additions), min(SAMPLE_PER_SERIES, len(additions))))
        report['series'][work] = {**coverage(group), 'sample': sample}
    for group in by_work.values():
        for row, old in group:
            if row['bangumi'] != old.get('bangumi'):
                tag = row['tag']
                report['changed_bangumi'].append({'tag': tag, 'id': row['id'], 'previous': old.get('bangumi'),
                                                  'current': row['bangumi'], 'match': matches.get(tag)})
    report['changed_bangumi'].sort(key=lambda item: item['id'])
    return report
