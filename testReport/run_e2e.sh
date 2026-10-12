#!/usr/bin/env bash
# run_e2e.sh: laew 端到端自动化验证入口(不依赖真实 LLM,使用本地 mock 服务)
# 输出: testReport/e2e-<时间戳>.txt
#
# 用法:
#   bash testReport/run_e2e.sh                  # 全量(231 断言,约 70s)
#   bash testReport/run_e2e.sh --list           # 列出全部小节名
#   bash testReport/run_e2e.sh --only '4q|8'    # 只跑匹配小节(前缀正则)
#   bash testReport/run_e2e.sh --skip '8|4b'    # 跳过匹配小节(前缀正则)
#   bash testReport/run_e2e.sh --keep-artifacts # 保留 mock 抓包 jsonl(默认全通过即清)
#   bash testReport/run_e2e.sh --timing         # 额外打印逐小节耗时表
#
# 结构(由 2500+ 行单文件按小节拆分而来):
#   testReport/run_e2e.sh         本入口:参数解析 + --list + 小节调度 + 收尾
#   testReport/lib/harness.sh     前奏:产物路径/并发锁/mock/端口/计时/断言 helper
#   testReport/lib/summary.sh     收尾:汇总 + 耗时表 + 产物清理 + 退出码
#   testReport/lib/sections/*.sh  43 个小节,文件名 = 执行序号-小节 id(排序即执行顺序)
#
# 注意:--only/--skip 会跳过小节正文,跨小节共享的变量(如 §3 建的 ID_A/ID_O、
# §8 tmux 依赖的 mockA 记录)可能因此缺失;只跑单小节时先确认其前置依赖已就位。
set -uo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
LIB_DIR="$ROOT_DIR/testReport/lib"
SEC_DIR="$LIB_DIR/sections"

# ---- CLI 参数 ----
E2E_ONLY=""; E2E_SKIP=""; E2E_LIST=0
E2E_KEEP_ARTIFACTS="${LAEW_E2E_KEEP_ARTIFACTS:-0}"
E2E_TIMING="${LAEW_E2E_TIMING:-}"
while [ $# -gt 0 ]; do
  case "$1" in
    --only)          E2E_ONLY="${2:-}"; shift 2 ;;
    --only=*)        E2E_ONLY="${1#*=}"; shift ;;
    --skip)          E2E_SKIP="${2:-}"; shift 2 ;;
    --skip=*)        E2E_SKIP="${1#*=}"; shift ;;
    --list)          E2E_LIST=1; shift ;;
    --keep-artifacts) E2E_KEEP_ARTIFACTS=1; shift ;;
    --timing)        E2E_TIMING=1; shift ;;
    -h|--help)       awk 'NR>1 && /^#/ {sub(/^# ?/,""); print; next} NR>1 {exit}' "$0"; exit 0 ;;
    *) echo "未知参数: $1(用 --help 查看用法)" >&2; exit 2 ;;
  esac
done
export LAEW_E2E_KEEP_ARTIFACTS="$E2E_KEEP_ARTIFACTS"
export LAEW_E2E_TIMING=$([ -n "$E2E_TIMING" ] && echo all || echo top)

# ---- 小节清单:文件名的序号前缀决定执行顺序,id/标题取各文件首行 `# --- ` 标记 ----
section_files() { ls "$SEC_DIR"/*.sh 2>/dev/null | sort; }
section_title() { grep -m1 '^# --- ' "$1" | sed -E 's/^# --- //; s/ *---$//'; }
section_id()    { section_title "$1" | sed -E 's/^([0-9]+[a-z0-9-]*)[.: ].*/\1/'; }

if [ "$E2E_LIST" -eq 1 ]; then
  section_files | while IFS= read -r f; do section_title "$f"; done
  exit 0
fi

# ---- 调度:命中 --only(优先于 --skip)/--skip 的小节按序 source ----
# 过滤语义与拆分前的 awk 自改写一致:前缀正则匹配「id 标题」,--only 非空时忽略 --skip。
SELECTED=(); SEL_COUNT=0
while IFS= read -r f; do
  [ -n "$f" ] || continue
  _key="$(section_id "$f") $(section_title "$f")"
  if [ -n "$E2E_ONLY" ]; then
    [[ "$_key" =~ ^($E2E_ONLY) ]] || continue
  elif [ -n "$E2E_SKIP" ] && [[ "$_key" =~ ^($E2E_SKIP) ]]; then
    continue
  fi
  SELECTED+=("$f"); SEL_COUNT=$((SEL_COUNT + 1))
done < <(section_files)

if [ -n "$E2E_ONLY$E2E_SKIP" ]; then
  echo "小节过滤: only='${E2E_ONLY}' skip='${E2E_SKIP}' → $SEL_COUNT/$(section_files | wc -l | tr -d ' ') 个小节"
fi

source "$LIB_DIR/harness.sh"
if [ "$SEL_COUNT" -gt 0 ]; then
  for _f in "${SELECTED[@]}"; do source "$_f"; done
fi
source "$LIB_DIR/summary.sh"
