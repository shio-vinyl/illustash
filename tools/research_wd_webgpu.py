"""#359 WebGPU 图捕获实验：固定设备缓冲区，计时包含上传、推理及 CPU 读回。

用法：python tools/research_wd_webgpu.py CONFIG.json OUTPUT.npz
依赖隔离环境中的 numpy、onnxruntime==1.30.0、onnxruntime-ep-webgpu==0.5.0。
CONFIG 提供 model（原始 WD ONNX）、inputs（图片×448×448×3 的 float32 NPY），
可选 limit 为取前几张。输出 probs、emb、times，必须另用 research_wd_compare.py 验算。
通过 collab_run.py timing 在禁网沙箱运行；不改变产品配置。
"""
import ctypes
from importlib.metadata import version
from pathlib import Path
import json
import os
import resource
import sys
import time

os.environ['ORT_DISABLE_TELEMETRY'] = '1'
import numpy as np
import onnxruntime as ort
import onnxruntime_ep_webgpu as webgpu

assert ort.__version__ == '1.30.0' and version('onnxruntime-ep-webgpu') == '0.5.0'
assert not Path(sys.argv[2]).exists(), '拒绝覆盖已有输出'
cfg = json.load(open(sys.argv[1]))
x = np.load(cfg['inputs'], mmap_mode='r')
count = cfg.get('limit', len(x))
assert 0 < count <= len(x)
x = x[:count]
options = ort.SessionOptions()
options.log_severity_level = 3
options.enable_mem_pattern = False
options.intra_op_num_threads = 4
options.add_free_dimension_override_by_name('batch_size', 1)
ort.register_execution_provider_library('webgpu', webgpu.get_library_path())
devices = [d for d in ort.get_ep_devices() if d.ep_name == webgpu.get_ep_name()]
assert devices, 'No WebGPU device'
options.add_provider_for_devices(devices[:1], {'enableGraphCapture': '1'})
started = time.perf_counter()
session = ort.InferenceSession(cfg['model'], options)
session.disable_fallback()
assert 'WebGpuExecutionProvider' in session.get_providers()
binding = session.io_binding()
input_value = session.create_ortvalue_from_shape_and_type([1,448,448,3], np.float32, 'webgpu')
binding.bind_ortvalue_input(session.get_inputs()[0].name, input_value)
outputs = []
for desc in session.get_outputs():
    shape = desc.shape
    assert len(shape) == 2 and shape[0] == 1 and isinstance(shape[1], int), shape
    value = session.create_ortvalue_from_shape_and_type(shape, np.float32, 'webgpu')
    outputs.append(value)
    binding.bind_ortvalue_output(desc.name, value)
load_ms = (time.perf_counter()-started)*1000

def infer(index):
    values = np.ascontiguousarray(x[index][None])
    start = time.perf_counter()
    input_value.update_inplace(values)
    session.run_with_iobinding(binding)
    result = binding.copy_outputs_to_cpu()
    return (time.perf_counter()-start)*1000, result

warmup = [infer(i % len(x))[0] for i in range(3)]
before = resource.getrusage(resource.RUSAGE_SELF)
times, probabilities, embeddings = [], [], []
for i in range(len(x)):
    elapsed, (p,e) = infer(i)
    times.append(elapsed)
    probabilities.append(p.reshape(-1).copy())
    embeddings.append(e.reshape(-1).copy())
after = resource.getrusage(resource.RUSAGE_SELF)
np.savez(sys.argv[2], probs=np.stack(probabilities), emb=np.stack(embeddings), times=np.array(times))
buf = (ctypes.c_uint64 * 42)()
libc = ctypes.CDLL('/usr/lib/libSystem.B.dylib')
assert libc.proc_pid_rusage(os.getpid(),4,ctypes.byref(buf)) == 0
print(json.dumps(dict(images=len(x), providers=session.get_providers(), load_ms=load_ms, warmup_ms=warmup,
    median_ms=float(np.median(times)), mean_ms=float(np.mean(times)), peak_footprint_mib=buf[30]/2**20,
    run_cpu_ms_per_image=((after.ru_utime+after.ru_stime)-(before.ru_utime+before.ru_stime))*1000/len(x),
    graph_capture=True, timing_includes='upload, captured run, CPU readback')), flush=True)
