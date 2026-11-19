"""读一个或几个进程的内存（macOS `footprint`），给测量脚本用：`python3 -m tools.ui_perf.footprint 进程号…` 打印 JSON。

每个进程给出：footprint（系统记账的物理占用，和活动监视器的「内存」一列同口径）、graphics（名字里带「(graphics)」的
各类之和，即解码后的图片位图、合成层的后备存储这类图形内存，按 dirty + swapped 计，和 footprint 同口径），以及
graphics 里最大的几类。单位 MiB。读不到的进程（已经退出、没有权限）写 error。
"""
from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path

MIB = 1 << 20


def read(pids: list[int]) -> dict[str, dict]:
    out: dict[str, dict] = {}
    for pid in pids:
        with tempfile.TemporaryDirectory(prefix='illustash-footprint-') as tmp:
            path = Path(tmp) / 'fp.json'
            r = subprocess.run(['footprint', '-p', str(pid), '-f', 'bytes', '-j', str(path)], capture_output=True, text=True)
            if r.returncode or not path.is_file():
                out[str(pid)] = {'error': (r.stderr or r.stdout).strip()[:300] or f'footprint 退出 {r.returncode}'}
                continue
            data = json.loads(path.read_text())
        proc = next((p for p in data.get('processes', []) if p.get('pid') == pid), None)
        if not proc:
            out[str(pid)] = {'error': 'footprint 没有这个进程'}
            continue
        graphics = {name: c.get('dirty', 0) + c.get('swapped', 0) for name, c in proc.get('categories', {}).items()
                    if '(graphics)' in name}
        top = sorted(graphics.items(), key=lambda kv: -kv[1])[:4]
        out[str(pid)] = {'name': proc.get('name'), 'footprint': round(proc.get('footprint', 0) / MIB, 1),
                         'graphics': round(sum(graphics.values()) / MIB, 1),
                         'graphics_top': {k: round(v / MIB, 1) for k, v in top if v}}
    return out


if __name__ == '__main__':
    print(json.dumps(read([int(x) for x in sys.argv[1:]]), ensure_ascii=False))
