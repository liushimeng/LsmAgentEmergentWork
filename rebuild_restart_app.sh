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

mkdir -p testReport
LOG_FILE="testReport/build.log"

# 拷贝产物到根目录。
# 覆盖前告警:检测正在运行的 laew 进程(2026-09-10 第 20 轮)。
# 运行中的二进制被覆盖会导致 ETXTBSY 或「边跑边换二进制」的静默干扰
# (并行会话共享根目录时尤甚),仅提示不阻塞。
# 2026-09-10: 处理「运行中二进制被占用」(ETXTBSY)。
# Linux 不允许直接 overwrite 正在执行的 binary,但允许 rename(只改目录项,
# 不影响运行中的 inode)。策略: 先重命名旧文件 → 复制新文件 → 清理旧文件。
# 运行中的旧实例继续用旧 inode,新启动的才用新二进制。
install_bin() {
  local bin="target/$PROFILE/laew"
  if [[ ! -f "$bin" ]]; then
    echo "[rebuild] 错误: 未在 $bin 找到产物" >&2
    exit 1
  fi
  if pgrep -x laew >/dev/null 2>&1; then
    echo "[rebuild] ⚠️  检测到正在运行的 laew 进程(pgrep -x laew):" >&2
    pgrep -ax laew | sed 's/^/[rebuild]   /' >&2
    echo "[rebuild] ⚠️  即将覆盖 ./laew,运行中的旧实例不受影响,但新启动才用新二进制" >&2
  fi
  if [[ -f "$ROOT_DIR/laew" ]]; then
    mv -f "$ROOT_DIR/laew" "$ROOT_DIR/laew.old" 2>/dev/null \
      || echo "[rebuild] ⚠️  无法重命名旧 laew,尝试直接覆盖" >&2
  fi
  cp -f "$bin" "$ROOT_DIR/laew"
  chmod +x "$ROOT_DIR/laew"
  rm -f "$ROOT_DIR/laew.old" 2>/dev/null || true
}

# 当前 HEAD 短哈希(与 build.rs 的取法一致,失败时同为 unknown)
git_hash() { git rev-parse --short HEAD 2>/dev/null || echo unknown; }

# 读取 ./laew --version 输出中的 git 哈希(形如 "..., git 3e6c801)")
bin_hash() {
  local v
  v="$("$ROOT_DIR/laew" --version 2>/dev/null || true)"
  v="${v##*git }"
  v="${v%)}"
  printf '%s' "$v"
}

# 1) 编译
echo "[rebuild] cargo build --$PROFILE  日志: $LOG_FILE"
cargo build --"$PROFILE" 2>&1 | tee "$LOG_FILE"

# 2) 拷贝到工程根目录
install_bin

# 3) 版本校验(2026-09-10): 防止「cargo 指纹缓存导致秒过,产物却是旧的」。
# 现象: git pull 后 cargo build 提示 Finished 0.x 秒(未真正重编),
#       ./laew --version 仍是旧 git 哈希/旧编译时间。
# 原因: 时钟漂移 / mtime 异常等使 cargo 误判「无变化」。
# 处理: 产物哈希 ≠ 当前 HEAD 时,touch build.rs 强制触发重编
#       (build.rs 重跑 → rustc-env 变化 → 按当前源码重新编译),仍不一致则报错退出。
EXPECT="$(git_hash)"
ACTUAL="$(bin_hash)"
if [[ -z "$ACTUAL" || "$ACTUAL" != "$EXPECT" ]]; then
  echo "[rebuild] ⚠️  产物版本异常: ./laew 报告 git ${ACTUAL:-<空>},当前 HEAD 是 $EXPECT" >&2
  echo "[rebuild] 触发强制重编(touch build.rs 穿透 cargo 增量缓存)" >&2
  touch build.rs
  cargo build --"$PROFILE" 2>&1 | tee -a "$LOG_FILE"
  install_bin
  ACTUAL="$(bin_hash)"
  if [[ -z "$ACTUAL" || "$ACTUAL" != "$EXPECT" ]]; then
    echo "[rebuild] 错误: 强制重编后 ./laew --version 仍为 git ${ACTUAL:-<空>}(期望 $EXPECT)" >&2
    echo "[rebuild] 请排查: git 状态 / 系统时钟 / target 目录,可尝试 cargo clean 后重跑" >&2
    exit 1
  fi
fi

# 4) 显著展示产物版本与当前提交,便于一眼确认「构建的就是这份代码」
echo "[rebuild] 当前代码: $(git log -1 --format='%h %ci %s' 2>/dev/null || echo "$EXPECT")"
echo "[rebuild] 产物版本: $("$ROOT_DIR/laew" --version)"

# 5) PATH 解析提示: 直接敲 `laew` 时命中的可能不是刚构建的产物
PATH_HIT="$(command -v laew 2>/dev/null || true)"
if [[ -n "$PATH_HIT" && "$PATH_HIT" != "$ROOT_DIR/laew" ]]; then
  echo "[rebuild] ⚠️  PATH 中的 laew 解析到 $PATH_HIT,不是刚构建的产物" >&2
  echo "[rebuild] ⚠️  请使用 $ROOT_DIR/laew 或调整 PATH,否则可能运行到旧版本" >&2
fi

echo "[rebuild] 已输出: $ROOT_DIR/laew"
echo "[rebuild] 完成 ✓"
