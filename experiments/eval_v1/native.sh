#!/usr/bin/env bash
# 共用评测集 v1 的原生主归属回放：#551 的隔离图库夹具和 native_replay（#534 runner），完整精选索引。
# 用法：experiments/eval_v1/native.sh <输出目录> <包1.bin> [包2.bin …]
# 第一个包是对照（一级 257 头）；每个包装上、读回全部详情、撤回，核对既有主归属和撤回恢复。
# 重型命令：经 collab_run.py run 排队执行。
set -euo pipefail
cd "$(dirname "$0")/../.."
main=${EVAL_V1_MAIN:-$(git rev-parse --path-format=absolute --git-common-dir | xargs dirname)}
wt="$main/.worktrees"
python="$wt/issue-380-character-coverage/_scratch/issue-380/venv/bin/python"
previous="$wt/issue-534-character-expansion/_scratch/issue-534"
native="$wt/issue-551-native-replay"
binary="$native/native/terms/target/release/illustash-terms"
experiments="$wt/issue-551-wd-b-expansion/experiments/issue_551"
export PYTHONPATH="$previous/extra-deps" OPENBLAS_NUM_THREADS=1 VECLIB_MAXIMUM_THREADS=1 PYTHONDONTWRITEBYTECODE=1
export ILLUSTASH_TEST_WAIT_SECONDS=${ILLUSTASH_TEST_WAIT_SECONDS:-600}
data=_scratch/eval-v1
out=$1; shift
mkdir -p "$out"
if [[ ! -f "$data/native/holdout-fixture.zip" ]]; then
  "$python" -u "$experiments/pack_native_holdout.py" --manifest "$data/eval/manifest.json" \
    --prev-fixture "$previous/b-acceptance/windows-fixture.zip" --index "$previous/index" \
    --scan "$previous/native-target/debug/illustash-issue-534-validation" \
    --source "$data/eval/features.npz" --out "$data/native" --covered-labels "$data/native-covered.json"
fi
replay_args=(--runner "$previous/b-acceptance/windows-replay.py" --fixture "$data/native/holdout-fixture.zip"
  --root "$out/library" --repo "$native" --binary "$binary")
"$python" -u "$experiments/native_replay.py" seed "${replay_args[@]}"
packages=()
for p in "$@"; do packages+=(--package "$(cd "$(dirname "$p")" && pwd)/$(basename "$p")"); done
"$python" -u "$experiments/native_replay.py" quality "${replay_args[@]}" "${packages[@]}" \
  --check-baseline-reinstall --output "$out/native-report.json"
