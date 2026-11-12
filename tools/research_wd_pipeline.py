"""WD 图改写的整流程对照实验（#359），不修改产品默认设置。

先按项目规则构建 native/terms 的 release 宿主、worker 和 recognition-compute 示例，再通过
collab_run.py timing 运行本工具。--original-models、--candidate-models 都含 WD 与 ccip/model_feat.onnx；
--runtime 是显式指定的 ORT dylib。--work 必须为空；所有 HOME、图库、缓存和结果都放在其中。
原模型使用产品默认组合（WD CPU、CCIP CoreML），候选使用双 CoreML；交替执行并核对真实后端。
结果为合成数据；不等于 GUI、安装、真实图库或跨平台验收。输出含本机路径，不可原样公开。
"""
import argparse
from contextlib import contextmanager
import os
import json
from pathlib import Path
import statistics
import sys
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from tools import bench_recognition as bench


@contextmanager
def sandbox_native():
    """原生宿主和计算子进程只许访问 localhost；父进程仍可用 top 读取测量条件。

    macOS 拒绝在 Seatbelt 沙箱中启动 top，因此仅包装现有基准的两个原生入口。
    worker 继承宿主的沙箱；这里的替换仅在本次研究脚本运行期间生效。
    """
    original = bench.subprocess.Popen
    policy = '(version 1)(allow default)(deny network*)(allow network* (local ip "localhost:*") (remote ip "localhost:*"))'

    def start(command, *args, **kwargs):
        if isinstance(command, (list, tuple)) and Path(command[0]).name in ('illustash-terms', 'recognition-compute'):
            command = ['/usr/bin/sandbox-exec', '-p', policy, *command]
        return original(command, *args, **kwargs)

    bench.subprocess.Popen = start
    try:
        yield
    finally:
        bench.subprocess.Popen = original


def cache_info(shared):
    root = shared / 'coreml-cache'
    return {p.name: sum(f.stat().st_size for f in p.rglob('*') if f.is_file())
            for p in root.iterdir() if p.is_dir()} if root.exists() else {}


def check_backend(shared, candidate):
    status = bench.used(shared)
    for name, expected in [('wd', 'coreml' if candidate else 'cpu'), ('ccip', 'coreml')]:
        row = status.get(name, {})
        if row.get('used', 'cpu') != expected or row.get('reason'):
            raise RuntimeError(f'{name}: expected {expected}, got {row}')
    return status


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--original-models', type=Path, required=True)
    parser.add_argument('--candidate-models', type=Path, required=True)
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--images', type=int, default=200)
    parser.add_argument('--trials', type=int, default=3)
    cli = parser.parse_args()
    os.environ['ORT_DISABLE_TELEMETRY'] = '1'
    if sys.platform != 'darwin' or cli.images <= 0 or cli.trials <= 0:
        raise SystemExit('需要 macOS，images 和 trials 必须为正数')
    if bench.sha256(cli.original_models/'ccip/model_feat.onnx') != bench.sha256(cli.candidate_models/'ccip/model_feat.onnx'):
        raise SystemExit('两组必须使用相同的 CCIP 模型')
    work = cli.work.resolve()
    work.mkdir(parents=True, exist_ok=True)
    if any(work.iterdir()):
        raise SystemExit('work must be empty')
    before = bench.conditions()
    release = ROOT / 'native/terms/target/release'
    runs = {}
    for label, models in [('original', cli.original_models), ('coreml', cli.candidate_models)]:
        folder = work / label
        folder.mkdir()
        args = SimpleNamespace(work=folder, images=cli.images, trials=cli.trials, seed=213,
                               anchors=2000, terms=release/'illustash-terms',
                               worker=release/'illustash-worker', compute=release/'examples/recognition-compute',
                               models=models.resolve(strict=True), runtime=cli.runtime.resolve(strict=True),
                               openblas=None, directml=None)
        base, resources = bench.layout(args, folder)
        (resources/'onnxruntime/backends.json').write_text(json.dumps(
            {'format': 1, 'wd': 'coreml' if label == 'coreml' else 'cpu', 'ccip': 'coreml'}))
        images, shared = folder/'images', folder/'shared'
        generation_env = bench.environment(folder/'generation-env', shared, resources)
        generated = bench.run_json(args.compute, {'mode': 'bench-generate', 'folder': str(images),
                                   'count': args.images, 'seed': args.seed, 'shared': str(shared),
                                   'anchors': args.anchors}, folder, generation_env)
        runs[label] = dict(args=args, base=base, resources=resources, images=images, shared=shared,
                           generated=generated, trials=[], stages=[], cache=[])
    for phase in ('end_to_end', 'stages'):
        for trial in range(1, cli.trials+1):
            order = ['original', 'coreml'] if trial % 2 else ['coreml', 'original']
            for label in order:
                r = runs[label]
                cache = {'phase': phase, 'trial': trial, 'before': cache_info(r['shared'])}
                if phase == 'end_to_end':
                    result = bench.end_to_end(r['args'], r['base'], r['resources'], r['images'],
                                              r['shared'], r['generated']['characters'], trial, 'default')
                    r['trials'].append(result)
                    print(f'{label} trial {trial}: {result["total_ms"]/1000:.2f} s', flush=True)
                else:
                    env = bench.environment(r['args'].work/'stage-env', r['shared'], r['resources'])
                    (r['shared']/'acceleration.json').unlink(missing_ok=True)
                    result = bench.stage_run(r['args'], r['resources'],
                                sorted(Path(f['path']) for f in r['generated']['files']), r['shared'],
                                Path(r['trials'][0]['package'])/'source.sqlite', trial, env, 'default')
                    r['stages'].append(result)
                    print(f'{label} stages {trial}: WD {statistics.fmean(i["wd"] for i in result["images"]):.2f} ms', flush=True)
                cache['after'] = cache_info(r['shared'])
                cache['acceleration'] = check_backend(r['shared'], label == 'coreml')
                r['cache'].append(cache)
                (work/'checkpoint.json').write_text(json.dumps({k: {f: v[f] for f in ('trials','stages','cache')}
                                                               for k,v in runs.items()}, indent=1))
    after = bench.conditions()
    final = {}
    for label, r in runs.items():
        final[label] = bench.summarise(r['args'], r['generated'], before, after, r['trials'], r['stages'])
        final[label]['cache'] = r['cache']
        final[label]['inputs'] = {name: bench.sha256(path) for name,path in (
            ('wd', r['resources']/'models/wd_swinv2_v3_emb.onnx'),
            ('ccip', r['resources']/'models/ccip/model_feat.onnx'),
            ('runtime', r['resources']/'onnxruntime'/bench.RUNTIME),
            ('terms', r['args'].terms), ('worker', r['args'].worker), ('compute', r['args'].compute))}
        bench.report(final[label])
    (work/'result.json').write_text(json.dumps(final, ensure_ascii=False, indent=1))


if __name__ == '__main__':
    with sandbox_native():
        main()
