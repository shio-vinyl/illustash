"""#611：调用 #551 冻结的 colab_run（分片、逐块回传核对、实例释放），只把并行实例上限从 2 放到 3。

冻结代码不改：从主检出 .worktrees/issue-551-wd-b-expansion 导入，模型、词表和 worker 照旧以那个工作树为准。
用法同 colab_run.py：colab_launch.py <请求清单> <工作目录> [--chunk N] [--runtimes 1-3] [--stop-after N]
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import sys

MAIN = Path(os.environ['ISSUE_611_MAIN'])
sys.path.insert(0, str(MAIN / '.worktrees/issue-551-wd-b-expansion/experiments/issue_551'))
import colab_run  # noqa: E402

MAX_RUNTIMES = 3


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('manifest', type=Path)
    parser.add_argument('work', type=Path)
    parser.add_argument('--chunk', type=int, default=768)
    parser.add_argument('--runtimes', type=int, default=1)
    parser.add_argument('--batch', type=int, default=8)
    parser.add_argument('--stop-after', type=int)
    parser.add_argument('--interrupt', action='store_true')
    args = parser.parse_args()
    if not 0 < args.runtimes <= MAX_RUNTIMES:
        parser.error(f'并行实例 1–{MAX_RUNTIMES} 个')
    shards = colab_run.create_shards(json.loads(args.manifest.read_bytes()), args.work / 'shards', args.chunk)
    output = args.work / 'received'
    output.mkdir(exist_ok=True)
    pending = [path for path in shards if colab_run.completion(path, output) is None]
    print(f'本机已验收 {len(shards) - len(pending)}/{len(shards)} 片，待回传 {len(pending)} 片', flush=True)
    pending = pending[:1 if args.interrupt else args.stop_after]
    if not pending:
        return
    active = min(len(pending), args.runtimes)
    with ThreadPoolExecutor(max_workers=active) as pool:
        jobs = [pool.submit(colab_run.worker, args, output, i, pending[i::active]) for i in range(active)]
        for job in jobs:
            job.result()


if __name__ == '__main__':
    main()
