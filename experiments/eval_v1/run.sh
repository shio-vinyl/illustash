#!/usr/bin/env bash
# 共用评测集 v1 的运行入口：用 #380 冻结的 Python 环境，单线程 BLAS。
# 用法：experiments/eval_v1/run.sh <脚本名> [参数…]
set -euo pipefail
cd "$(dirname "$0")/../.."
main=${EVAL_V1_MAIN:-$(git rev-parse --path-format=absolute --git-common-dir | xargs dirname)}
export EVAL_V1_MAIN="$main"
export PYTHONPATH="$main/.worktrees/issue-534-character-expansion/_scratch/issue-534/extra-deps"
export OPENBLAS_NUM_THREADS=1 VECLIB_MAXIMUM_THREADS=1 PYTHONDONTWRITEBYTECODE=1
python="$main/.worktrees/issue-380-character-coverage/_scratch/issue-380/venv/bin/python"
script=$1; shift
exec "$python" -u "experiments/eval_v1/$script.py" "$@"
