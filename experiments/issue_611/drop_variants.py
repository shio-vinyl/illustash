"""#611：从冻结名单里去掉变体标签，重新冻结（2026-11-18 用户定：变体这一轮先不做）。

变体标签 = 一个候选在公开快照里的全年龄合格帖中，一半以上同时带一个已覆盖角色
（WD 词表 ∪ 完整精选索引）的标签，基本是已有角色的服装、形态或皮肤。判定只用 supply.py 的元数据计数。
输出 roster/roster-v2.json，原名单 roster.json 保留不动。
"""

import argparse
from collections import Counter
import json
import os
from pathlib import Path

from roster import ROOT, digest, read, write


def is_variant(row):
    return bool(row['g_posts']) and row['with_covered_posts'] / row['g_posts'] >= 0.5


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, default=Path(os.environ.get('ISSUE_611_OUTPUT', ROOT / '_scratch/issue-611')))
    args = parser.parse_args()
    roster_path, supply_path = args.out / 'roster/roster.json', args.out / 'supply/supply.json'
    roster = read(roster_path)
    supply = {r['label']: r for r in read(supply_path)}
    if set(supply) != {r['label'] for r in roster}:
        raise ValueError('供给表和名单不一致')
    kept = [r for r in roster if not is_variant(supply[r['label']])]
    dropped = [r for r in roster if is_variant(supply[r['label']])]
    path = write(args.out / 'roster/roster-v2.json', kept)
    receipt = dict(
        rule='去掉变体标签：合格全年龄帖里一半以上同时带 WD 或完整精选索引已覆盖角色',
        source_roster_sha256=digest(roster_path), supply_sha256=digest(supply_path),
        before=len(roster), dropped_variants=len(dropped), selected=len(kept),
        post_count_bands=dict(Counter(r['band'] for r in kept)),
        dropped_by_band=dict(Counter(r['band'] for r in dropped)), roster_sha256=digest(path))
    write(args.out / 'roster/receipt-v2.json', receipt)
    print(json.dumps(receipt, ensure_ascii=False, indent=1))


if __name__ == '__main__':
    main()
