#!/usr/bin/env bash
# rebuild_restart_app.sh: 编译并把 laew 输出到本工程根目录。
#
# 用法:
#   ./rebuild_restart_app.sh          # release 构建
#   ./rebuild_restart_app.sh --debug  # debug 构建
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
echo "[rebuild] 配置  : $PROFILE"

# 1) 编译
mkdir -p testReport
LOG_FILE="testReport/build.log"
echo "[rebuild] cargo build --$PROFILE  日志: $LOG_FILE"
cargo build --"$PROFILE" 2>&1 | tee "$LOG_FILE"

BIN_PATH="target/$PROFILE/laew"
if [[ ! -f "$BIN_PATH" ]]; then
  echo "[rebuild] 错误: 未在 $BIN_PATH 找到产物" >&2
  exit 1
fi

# 2) 拷贝到工程根目录
# 覆盖前告警:检测正在运行的 laew 进程(2026-09-10 第 20 轮)。
# 运行中的二进制被覆盖会导致 ETXTBSY 或「边跑边换二进制」的静默干扰
# (并行会话共享根目录时尤甚),仅提示不阻塞。
if pgrep -x laew >/dev/null 2>&1; then
  echo "[rebuild] ⚠️  检测到正在运行的 laew 进程(pgrep -x laew):" >&2
  pgrep -ax laew | sed 's/^/[rebuild]   /' >&2
  echo "[rebuild] ⚠️  即将覆盖 ./laew,运行中的旧实例不受影响,但新启动才用新二进制" >&2
fi
# 2026-09-10: 处理「运行中二进制被占用」(ETXTBSY)。
# Linux 不允许直接 overwrite 正在执行的 binary,但允许 rename(只改目录项,
# 不影响运行中的 inode)。策略: 先重命名旧文件 → 复制新文件 → 清理旧文件。
# 运行中的旧实例继续用旧 inode,新启动的才用新二进制。
if [[ -f "$ROOT_DIR/laew" ]]; then
  mv -f "$ROOT_DIR/laew" "$ROOT_DIR/laew.old" 2>/dev/null \
    || echo "[rebuild] ⚠️  无法重命名旧 laew,尝试直接覆盖" >&2
fi
cp -f "$BIN_PATH" "$ROOT_DIR/laew"
chmod +x "$ROOT_DIR/laew"
rm -f "$ROOT_DIR/laew.old" 2>/dev/null || true
echo "[rebuild] 已输出: $ROOT_DIR/laew"
echo "[rebuild] 完成 ✓"
