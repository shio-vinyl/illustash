#!/usr/bin/env bash
# #611 研究脚本入口：用 #380 冻结的 Python 环境和 #534 的附加依赖（duckdb）。
# 用法：experiments/issue_611/run.sh <脚本名> [参数…]
# 只读主检出 .worktrees 下的冻结数据和共用评测集 v1 的排除边界，产物写本工作树 _scratch/issue-611。
set -euo pipefail
cd "$(dirname "$0")/../.."
main=${ISSUE_611_MAIN:-$(git rev-parse --path-format=absolute --git-common-dir | xargs dirname)}
export ISSUE_611_MAIN="$main"
export PYTHONPATH="$main/.worktrees/issue-534-character-expansion/_scratch/issue-534/extra-deps"
export OPENBLAS_NUM_THREADS=1 VECLIB_MAXIMUM_THREADS=1 PYTHONDONTWRITEBYTECODE=1
python="$main/.worktrees/issue-380-character-coverage/_scratch/issue-380/venv/bin/python"
script=$1; shift
exec "$python" -u "experiments/issue_611/$script.py" "$@"
