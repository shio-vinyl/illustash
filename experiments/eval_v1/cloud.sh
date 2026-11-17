#!/usr/bin/env bash
# 经 Colab L4 分片提特征（整图 WD v3 嵌入 + 角色分数 + CCIP + 指纹），沿用 #551 冻结的 colab_run/worker。
# 用法：experiments/eval_v1/cloud.sh <请求清单> <工作目录> [--runtimes N]
# 每片回传后核对帖子清单、模型和词表 SHA-256、特征 SHA-256 才算完成；实例释放后记下余额。
set -euo pipefail
cd "$(dirname "$0")/../.."
main=${EVAL_V1_MAIN:-$(git rev-parse --path-format=absolute --git-common-dir | xargs dirname)}
python="$main/.worktrees/issue-380-character-coverage/_scratch/issue-380/venv/bin/python"
runner="$main/.worktrees/issue-551-wd-b-expansion/experiments/issue_551/colab_run.py"
export PYTHONDONTWRITEBYTECODE=1
manifest=$1; work=$2; shift 2
mkdir -p "$work"
usage() {
  uvx -q --from google-colab-cli==0.8.0 colab --auth oauth2 usage </dev/null |
    awk '/Current balance:|Usage rate:|Active assignments:/'
}
usage > "$work/usage-before.txt"
"$python" -u "$runner" "$manifest" "$work" "$@"
usage > "$work/usage-after.txt"
cat "$work/usage-before.txt" "$work/usage-after.txt"
