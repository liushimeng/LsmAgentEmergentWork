#!/usr/bin/env bash
# rebuild.sh: 杀死当前 laew 进程,重新编译并把 laew 输出到本工程根目录。
#
# 用法:
#   ./rebuild.sh                # release 构建
#   ./rebuild.sh --debug        # debug 构建
#
# 输出:
#   ./laew                      # 二进制(覆盖已存在)
#   ./testReport/build.log      # 编译日志
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT_DIR"

PROFILE="release"
case "${1:-}" in
  --debug) PROFILE="debug" ;;
  "") ;;
  *) echo "未知选项: $1"; exit 2 ;;
esac

echo "[rebuild] 根目录: $ROOT_DIR"
echo "[rebuild] 配置 : $PROFILE"

# 1) 杀掉当前 laew 进程(忽略未运行的情况)
if pgrep -x laew >/dev/null 2>&1; then
  echo "[rebuild] 检测到运行中的 laew,正在终止..."
  pkill -x laew || true
  sleep 0.5
fi

# 2) 编译
mkdir -p testReport
LOG_FILE="testReport/build.log"
echo "[rebuild] cargo build --$PROFILE 日志: $LOG_FILE"
cargo build --"$PROFILE" 2>&1 | tee "$LOG_FILE"

BIN_PATH="target/$PROFILE/laew"
if [[ ! -f "$BIN_PATH" ]]; then
  echo "[rebuild] 错误: 未在 $BIN_PATH 找到产物" >&2
  exit 1
fi

# 3) 拷贝到工程根目录
cp -f "$BIN_PATH" "$ROOT_DIR/laew"
chmod +x "$ROOT_DIR/laew"
echo "[rebuild] 已输出: $ROOT_DIR/laew"
echo "[rebuild] 完成 ✓"
