#!/usr/bin/env bash
# 经 Colab L4 分片提特征（整图 WD v3 嵌入 + 角色分数 + CCIP + 指纹），沿用 #551 冻结的 colab_run/worker。
# 用法：experiments/eval_v1/cloud.sh <请求清单> <工作目录> [--runtimes N]
# 每片回传后核对帖子清单、模型和词表 SHA-256、特征 SHA-256 才算完成。
#
# 实例生命周期：colab_run 每个实例在 finally 里 stop 并核验释放，释放不了就报错停下。
# 这里再加一道兜底：脚本无论怎样退出（出错、被打断），都对本次工作目录 runs/*/sessions.json 里
# 记下的每个会话再 stop 一次，然后查服务端 `colab sessions` 和余额页的 Active assignments，
# 不为 0 就报错，提醒人工处理。开跑前若已有活动分配，也不开新的。
set -euo pipefail
cd "$(dirname "$0")/../.."
main=${EVAL_V1_MAIN:-$(git rev-parse --path-format=absolute --git-common-dir | xargs dirname)}
python="$main/.worktrees/issue-380-character-coverage/_scratch/issue-380/venv/bin/python"
runner="$main/.worktrees/issue-551-wd-b-expansion/experiments/issue_551/colab_run.py"
export PYTHONDONTWRITEBYTECODE=1
manifest=$1; work=$2; shift 2
mkdir -p "$work"
colab=(uvx -q --from google-colab-cli==0.8.0 colab --auth oauth2)
usage() { "${colab[@]}" usage </dev/null | awk '/Current balance:|Usage rate:|Active assignments:/'; }
active() { usage | awk '/Active assignments:/ {print $3}'; }

cleanup() {
  status=$?
  for config in "$work"/runs/*/sessions.json; do
    [[ -f "$config" ]] || continue
    for name in $("$python" -c 'import json,sys; print(" ".join(json.load(open(sys.argv[1]))))' "$config"); do
      echo "兜底释放：$name" >&2
      "${colab[@]}" --config "$config" stop -s "$name" </dev/null >/dev/null 2>&1 || true
    done
  done
  usage > "$work/usage-after.txt" || true
  "${colab[@]}" sessions </dev/null > "$work/sessions-after.txt" 2>&1 || true
  cat "$work/usage-before.txt" "$work/usage-after.txt" >&2 || true
  left=$(awk '/Active assignments:/ {print $3}' "$work/usage-after.txt" 2>/dev/null || echo unknown)
  if [[ "$left" != 0 ]]; then
    echo "Colab 仍有活动分配（$left），请在 colab sessions / 余额页核对并释放" >&2
    exit 3
  fi
  exit $status
}

usage > "$work/usage-before.txt"
if [[ "$(awk '/Active assignments:/ {print $3}' "$work/usage-before.txt")" != 0 ]]; then
  echo "开跑前已有 Colab 活动分配，不开新实例" >&2
  exit 3
fi
trap cleanup EXIT
trap 'exit 130' INT TERM
"$python" -u "$runner" "$manifest" "$work" "$@"
