"""从冻结产物复封 WD 角色扩展的模型文件。使用 Python 3.12 和 NumPy 2.5.3。

第 1 版：python tools/build_wd_extension.py <#534 实验目录> b-534.bin
第 2 版：python tools/build_wd_extension.py <#551 实验目录> b-551-shortlist.bin --v2-base b-534.bin

#534 输入来自 2026-11-16 冻结的公开角色库存和评测。Danbooru 帖子 Parquet 固定在
ebb02a630201c7b51487e45fb90b3fcf4cbedc20（截至 2026-07-13 UTC），API 补查限定到
2026-11-15，DeepGHS Character Index 固定在 5ee6075ff09f82c27e23f33044b9f9cabaf1e8c3。
WD 向量来自 DeepGHS 的 4ec31f2aeabf66353f3ee3ccd20d2f07e716851b 导出版本。
#551 在同一底座上冻结训练头和入选清单。第 2 版直接沿用第 1 版的标签顺序、参数字节和
有效阈值，按清单顺序追加七个入选头，每个使用 f32 0.99。脚本核验输入和产物的 SHA-256。
"""
import argparse
import hashlib
import io
import json
from pathlib import Path

import numpy as np


V1_INPUTS = {
    "evaluation/A-coverage.json": "3531cddeaf0148cbaa93ef0c3643d6cd5ade00adda04f09de5def27807a95040",
    "sample/roster.json": "38ee7dda68563b3800e76d791905c19e844db4eecf43fbc8c8821704484d1344",
    "evaluation/thresholds.json": "3ece000d79d0f620beb68f3a2a40a97e850d3b70e710aa37485a43affb8b5ff5",
    "evaluation/B-heads.npz": "5bc45fcc05e039270aa0261bc1e6a4687c707abe743f00e63ed77fe57b620028",
}
V2_INPUTS = {
    "fit/heads.npz": "bdb55342f78c5cba1fd9e67adc04b1598c2c423b70f566eb78bf5b4e013bf06b",
    "shortlist/shortlist.json": "401dec4beab0cb5282a04c179404dd434fdb0ffff62f2573244b7647f0bdb7e7",
}
V1_BYTES = 940_701
V1_SHA256 = "9da0365457e479eff189150926289c6cf8325cd9bb98c0bd2504c26d7dcae471"
V2_BYTES = 974_043
V2_SHA256 = "a81d2bc3358c3a75f1b52594a9e65c33b41162a18fe033f11b0d339e1e6478ca"


def sha(data):
    return hashlib.sha256(data).hexdigest()


def frozen_file(path, expected):
    data = path.read_bytes()
    if sha(data) != expected:
        raise ValueError(f"输入与冻结产物不符：{path.name}")
    return data


def frozen_inputs(experiment, fingerprints):
    return {name: frozen_file(experiment / name, digest) for name, digest in fingerprints.items()}


def freeze_v1(inputs):
    coverage = json.loads(inputs["evaluation/A-coverage.json"])
    roster = json.loads(inputs["sample/roster.json"])
    if set(coverage) != {row["name"] for row in roster} or len(coverage) != 500:
        raise ValueError("目标名单与实验不符")
    outside = {tag for tag, status in coverage.items() if status["state"] != "covered_by_loaded_curated_reference"}
    threshold = json.loads(inputs["evaluation/thresholds.json"])["B_calibration_1pct"]

    with np.load(io.BytesIO(inputs["evaluation/B-heads.npz"]), allow_pickle=False) as archive:
        labels = archive["labels"].tolist()
        keep = [idx for idx, name in enumerate(labels) if name in outside]
        if (len(outside), len(labels), len(keep)) != (251, 433, 228):
            raise ValueError("扩展名单未得到 228 个头")
        names = [labels[idx] for idx in keep]
        weight = archive["weight"][keep].astype("<f4").tobytes()
        bias = archive["bias"][keep].astype("<f4").tobytes()
    manifest = {
        "format": "illustash-wd-extension/1",
        "wd_model_sha256": "ae25f5f12b2d5891728fb7768fa63d65bf13a83d6d4823e3a1bf7b42875c74ff",
        "wd_tags_sha256": "298633d94d0031d2081c0893f29c82eab7f0df00b08483ba8f29d1e979441217",
        "threshold": threshold,
        "labels": names,
    }
    return manifest, weight, bias


def freeze_v2(inputs, base):
    header, _, coefficients = base.partition(b"\n")
    manifest = json.loads(header)
    count = len(manifest["labels"])
    weight, bias = coefficients[:-count * 4], coefficients[-count * 4:]
    names = json.loads(inputs["shortlist/shortlist.json"])["labels"]
    if len(set(names)) != 7 or len(names) != 7 or set(names).intersection(manifest["labels"]):
        raise ValueError("扩展名单须包含七个未在第 1 版中出现的头")
    with np.load(io.BytesIO(inputs["fit/heads.npz"]), allow_pickle=False) as archive:
        positions = {name: i for i, name in enumerate(archive["labels"].tolist())}
        keep = [positions[name] for name in names]
        weight += archive["weight"][keep].astype("<f4").tobytes()
        bias += archive["bias"][keep].astype("<f4").tobytes()
    manifest["labels"] += names
    manifest["thresholds"] = [manifest["threshold"]] * count + [float(np.float32(0.99))] * len(names)
    return manifest, weight, bias


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("experiment", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--v2-base", type=Path, help="复封第 2 版时使用的冻结 b-534.bin")
    args = parser.parse_args()

    if args.v2_base is None:
        version, expected_bytes, expected_sha = 1, V1_BYTES, V1_SHA256
        manifest, weight, bias = freeze_v1(frozen_inputs(args.experiment, V1_INPUTS))
    else:
        version, expected_bytes, expected_sha = 2, V2_BYTES, V2_SHA256
        base = frozen_file(args.v2_base, V1_SHA256)
        manifest, weight, bias = freeze_v2(frozen_inputs(args.experiment, V2_INPUTS), base)
    encoded_names = json.dumps(manifest["labels"], ensure_ascii=False, separators=(",", ":")).encode()
    # 第 2 版沿用 #551 的 ASCII 转义规则，保持冻结头部的字节。
    header = json.dumps(manifest, ensure_ascii=version == 2, separators=(",", ":")).encode()
    data = header + b"\n" + weight + bias
    digest = sha(data)
    if len(data) != expected_bytes or digest != expected_sha:
        raise ValueError(f"输出大小或 SHA-256 与 WD 角色扩展 v{version} 不符")
    with args.output.open("xb") as output:
        output.write(data)
    print(json.dumps({"package_sha256": digest, "labels_sha256": sha(encoded_names),
                      "weight_sha256": sha(weight), "bias_sha256": sha(bias), "bytes": len(data)}))


if __name__ == "__main__":
    main()
