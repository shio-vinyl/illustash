"""WD SwinV2 v3 的 Core ML 图改写实验（#359）；不安装模型、不修改产品默认设置。

仅接受 models.json 固定的原始模型与 ONNX Runtime 1.30.0。使用独立环境中的 numpy、onnx、
onnxruntime；输出仅供研究，batch 固定为 1，必须另行验算数值。原始文件和已有输出均不覆盖。
用法：python tools/research_wd_coreml.py ORIGINAL.onnx OUTPUT.onnx
导出会创建 CPU 会话，应按 AGENTS.md 经 collab_run.py bg 运行。
"""
import argparse
import os
import hashlib
import json
from pathlib import Path
import tempfile

import numpy as np
import onnx
from onnx import helper, numpy_helper, shape_inference
# 在 ORT 初始化之前关闭研究运行的遥测（与 v1.30.0 的运行时开关一致）。
os.environ['ORT_DISABLE_TELEMETRY'] = '1'
import onnxruntime as ort

SOURCE_SHA256 = 'ae25f5f12b2d5891728fb7768fa63d65bf13a83d6d4823e3a1bf7b42875c74ff'


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1 << 20), b''):
            digest.update(block)
    return digest.hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def attribute(node, name):
    return next((a for a in node.attribute if a.name == name), None)


def rewrite(model):
    """规则严格限于固定模型的已知模式；条件不满足即停止，避免静默改写不等价的广播。"""
    model = shape_inference.infer_shapes(model, strict_mode=True)
    graph = model.graph
    initializers = {i.name: i for i in graph.initializer}
    graph_outputs = {i.name for i in graph.output}
    shapes = {v.name: tuple(d.dim_value if d.HasField('dim_value') else None
                           for d in v.type.tensor_type.shape.dim)
              for v in [*graph.input, *graph.value_info, *graph.output]}
    shapes.update({i.name: tuple(i.dims) for i in graph.initializer})
    consumers = {}
    for node in graph.node:
        for name in node.input:
            consumers.setdefault(name, []).append(node)
    counts = dict(rank6=0, expand=0, l2=0, gemm=0)

    for node in graph.node:
        if node.op_type != 'Reshape' or node.input[1] not in initializers:
            continue
        shape = numpy_helper.to_array(initializers[node.input[1]])
        if shape.size != 6:
            continue
        require(shape[0] == 1 and np.all(shape > 0), f'{node.name}: 非固定六维形状')
        require(node.output[0] not in graph_outputs, f'{node.name}: 模型输出不能降维')
        users = consumers.get(node.output[0], [])
        require(users and all(u.op_type == 'Transpose' for u in users), f'{node.name}: 非 Transpose 消费者')
        for trans in users:
            perm = attribute(trans, 'perm')
            require(perm is not None and list(perm.ints)[0] == 0, f'{trans.name}: 首轴发生移动')
            downstream = consumers.get(trans.output[0], [])
            require(trans.output[0] not in graph_outputs and downstream, f'{trans.name}: 末端输出')
            for nxt in downstream:
                require(nxt.op_type == 'Reshape' and nxt.input[1] in initializers,
                        f'{nxt.name}: 非常量 Reshape')
                require(0 not in numpy_helper.to_array(initializers[nxt.input[1]]),
                        f'{nxt.name}: 目标形状依赖输入轴')
            axes = [p-1 for p in perm.ints[1:]]
            del perm.ints[:]
            perm.ints.extend(axes)
        name = node.input[1] + '_rank5'
        if name not in initializers:
            value = numpy_helper.from_array(shape[1:].astype(np.int64), name)
            graph.initializer.append(value)
            initializers[name] = value
        node.input[1] = name
        counts['rank6'] += 1

    for node in list(graph.node):
        if node.op_type != 'Expand':
            continue
        require(node.input[1] in initializers and node.output[0] not in graph_outputs,
                f'{node.name}: 非常量 Expand 或模型输出')
        target = tuple(numpy_helper.to_array(initializers[node.input[1]]).tolist())
        users = consumers.get(node.output[0], [])
        require(users, f'{node.name}: 没有消费者')
        source_shape = shapes[node.input[0]]
        require(all(isinstance(d, int) and d > 0 for d in source_shape + target),
                f'{node.name}: 广播形状未知')
        for user in users:
            require(user.op_type in ('Div', 'Mul', 'Add', 'Sub') and len(user.input) == 2,
                    f'{user.name}: 非逐元素二元操作')
            for index, name in enumerate(user.input):
                if name == node.output[0]:
                    other_shape = shapes[user.input[1-index]]
                    require(other_shape == target and np.broadcast_shapes(source_shape, other_shape) == target,
                            f'{node.name}: 隐式广播改变输出形状')
                    user.input[index] = node.input[0]
        graph.node.remove(node)
        counts['expand'] += 1

    nodes = []
    for node in graph.node:
        if node.op_type != 'ReduceL2':
            nodes.append(node)
            continue
        axes = attribute(node, 'axes')
        keep = attribute(node, 'keepdims')
        require(axes is not None and (keep is None or keep.i == 1), f'{node.name}: 未知 L2 模式')
        x, y = node.input[0], node.output[0]
        name = node.name + '_axes'
        graph.initializer.append(numpy_helper.from_array(np.array(axes.ints, np.int64), name))
        nodes.extend([
            helper.make_node('Mul', [x, x], [y+'_sq'], name=node.name+'_sq'),
            helper.make_node('ReduceSum', [y+'_sq', name], [y+'_sum'], name=node.name+'_sum', keepdims=1),
            helper.make_node('Sqrt', [y+'_sum'], [y], name=node.name+'_sqrt'),
        ])
        counts['l2'] += 1
    del graph.node[:]
    graph.node.extend(nodes)

    for node in graph.node:
        if node.op_type != 'Gemm':
            continue
        trans_b = attribute(node, 'transB')
        if trans_b is not None and trans_b.i == 1:
            continue
        require(node.input[1] in initializers, f'{node.name}: 权重不是常量')
        trans_a = attribute(node, 'transA')
        require(trans_a is None or trans_a.i == 0, f'{node.name}: 未知 transA')
        weights = numpy_helper.to_array(initializers[node.input[1]])
        require(weights.ndim == 2, f'{node.name}: 非矩阵权重')
        name = node.input[1] + '_T'
        if name not in initializers:
            value = numpy_helper.from_array(np.ascontiguousarray(weights.T), name)
            graph.initializer.append(value)
            initializers[name] = value
        node.input[1] = name
        if trans_b is None:
            node.attribute.append(helper.make_attribute('transB', 1))
        else:
            trans_b.i = 1
        counts['gemm'] += 1

    require(counts == dict(rank6=51, expand=48, l2=48, gemm=96), f'改写数量变化：{counts}')
    used = {name for node in graph.node for name in node.input} | graph_outputs
    for value in list(graph.initializer):
        if value.name not in used:
            graph.initializer.remove(value)
    del graph.value_info[:]
    model = shape_inference.infer_shapes(model, strict_mode=True)
    onnx.checker.check_model(model)
    return model, counts


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    require(ort.__version__ == '1.30.0', '此实验固定使用 onnxruntime==1.30.0')
    require(not args.output.exists(), '拒绝覆盖已有输出')
    require(sha256(args.source) == SOURCE_SHA256, '源模型不是 models.json 固定版本')
    with tempfile.TemporaryDirectory(prefix='wd-basic-') as temporary:
        basic = Path(temporary) / 'basic.onnx'
        options = ort.SessionOptions()
        options.intra_op_num_threads = 4
        options.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_BASIC
        options.add_free_dimension_override_by_name('batch_size', 1)
        options.optimized_model_filepath = str(basic)
        session = ort.InferenceSession(str(args.source), options, providers=['CPUExecutionProvider'])
        del session
        model, counts = rewrite(onnx.load(basic))
        # 研究工具不处理并发输出；写失败时保留错误供调用者处理。
        with args.output.open('xb') as stream:
            stream.write(model.SerializeToString())
    print(json.dumps(dict(source_sha256=SOURCE_SHA256, output_sha256=sha256(args.output),
                          ort=ort.__version__, onnx=onnx.__version__, rewrites=counts,
                          nodes=len(model.graph.node)), ensure_ascii=False))


if __name__ == '__main__':
    main()
