#!/usr/bin/env bash
# rebuild_restart_app.sh: 编译并把 laew 输出到本工程根目录。
#
# 用法:
#   ./rebuild_restart_app.sh          # release 构建(版本异常时自动强制重编)
#   ./rebuild_restart_app.sh --debug  # debug 构建
#   ./rebuild_restart_app.sh --force  # release 构建,无论是否一致都 touch build.rs 重编
#   ./rebuild_restart_app.sh --debug --force
#
# 输出:
#   ./laew                      # 二进制(覆盖已存在)
#   ./testReport/build.log      # 编译日志
#
# 设计要点(2026-09-10 第 26 轮加固):
#   * 解决「跨机器 git pull 后 ./rebuild_restart_app.sh 输出旧 laew」的陷阱:
#     - 根因 1: cargo 的 rerun-if-changed 按 mtime 比对,`.git/HEAD` 在 git pull 时
#       mtime 不刷新,cargo 误判无变更。
#     - 根因 2: 兜底脚本之前仅比对「产物哈希 vs HEAD」,无法覆盖「远端领先 HEAD」的
#       情形(用户没 fetch / 网络隔离)。
#     - 根因 3: 用户路径 `/usr/local/bin/laew` 可能覆盖根目录 `./laew` 的认知。
#   * 加固: 增加 --force 选项(主动 touch build.rs);产物/HEAD hash 全部用 --short=8
#     与 build.rs 对齐,避免假阳性;git fetch + 远端 HEAD 对比;PATH 提示更醒目。
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT_DIR"

PROFILE="release"
FORCE=0
for arg in "$@"; do
  case "$arg" in
    --debug) PROFILE="debug" ;;
    --force) FORCE=1 ;;
    --help|-h)
      sed -n '2,15p' "$0"
      exit 0
      ;;
    *) echo "未知选项: $arg"; exit 2 ;;
  esac
done

echo "[rebuild] 根目录: $ROOT_DIR"
echo "[rebuild] 配置  : $PROFILE"
[[ "$FORCE" -eq 1 ]] && echo "[rebuild] 强制   : --force(touch build.rs 穿透 cargo 增量缓存)"

mkdir -p testReport
LOG_FILE="testReport/build.log"

# 拷贝产物到根目录。
# 覆盖前告警:检测正在运行的 laew 进程(2026-09-10 第 20 轮)。
# 运行中的二进制被覆盖会导致 ETXTBSY 或「边跑边换二进制」的静默干扰
# (并行会话共享根目录时尤甚),仅提示不阻塞。
# 2026-09-10: 处理「运行中二进制被占用」(ETXTBSY)。
# Linux 不允许直接 overwrite 正在执行的 binary,但允许 rename(只改目录项,
# 不影响运行中的 inode)。策略: 先重命名旧文件 → 复制新文件 → 清理旧文件。
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

# 当前 HEAD 短哈希(8 位,与 build.rs --short=8 对齐,失败时 unknown)
git_hash() { git rev-parse --short=8 HEAD 2>/dev/null || echo unknown; }

# 尝试远端 main 短哈希(网络可达时),失败回退 unknown
remote_hash() {
  # 仅在 5 秒超时内尝试,避免无网环境卡住脚本
  if command -v timeout >/dev/null 2>&1; then
    git fetch --no-tags --depth=1 origin main 2>/dev/null &
    local pid=$!
    ( sleep 5; kill -9 "$pid" 2>/dev/null || true ) &
    wait "$pid" 2>/dev/null || true
  else
    git fetch --no-tags --depth=1 origin main 2>/dev/null || true
  fi
  git rev-parse --short=8 origin/main 2>/dev/null || echo unknown
}

# 读取 ./laew --version 输出中的 git 哈希(形如 "..., git 3e6c801)")
bin_hash() {
  local v
  v="$("$ROOT_DIR/laew" --version 2>/dev/null || true)"
  v="${v##*git }"
  v="${v%)}"
  printf '%s' "$v"
}

# 1) --force: 主动 touch build.rs 穿透 cargo 增量缓存
if [[ "$FORCE" -eq 1 ]]; then
  echo "[rebuild] --force: touch build.rs 强制 cargo 重跑 build.rs"
  touch build.rs
fi

# 2) 编译
echo "[rebuild] cargo build --$PROFILE  日志: $LOG_FILE"
cargo build --"$PROFILE" 2>&1 | tee "$LOG_FILE"

# 3) 拷贝到工程根目录
install_bin

# 4) 版本校验(2026-09-10): 防止「cargo 指纹缓存导致秒过,产物却是旧的」。
# 现象: git pull 后 cargo build 提示 Finished 0.x 秒(未真正重编),
#       ./laew --version 仍是旧 git 哈希/旧编译时间。
# 原因: 时钟漂移 / mtime 异常等使 cargo 误判「无变化」;或 `.git/HEAD`
#       mtime 在 git pull 后没刷新,但 `.git/refs/heads/main` 已更新,
#       cargo 比 mtime 时漏判。
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

# 5) 远端 HEAD 对比(2026-09-10): 如果远端领先,提示用户先 git pull
REMOTE="$(remote_hash)"
if [[ "$REMOTE" != "unknown" && "$REMOTE" != "$EXPECT" ]]; then
  echo "[rebuild] ⚠️  远端 origin/main ($REMOTE) 领先当前 HEAD ($EXPECT)" >&2
  echo "[rebuild] ⚠️  产物是按本地 HEAD 构建的,可能不是最新代码,建议: git pull" >&2
fi

# 6) 显著展示产物版本与当前提交,便于一眼确认「构建的就是这份代码」
echo "[rebuild] 当前代码: $(git log -1 --format='%h %ci %s' 2>/dev/null || echo "$EXPECT")"
echo "[rebuild] 产物版本: $("$ROOT_DIR/laew" --version)"

# 7) PATH 解析提示: 直接敲 `laew` 时命中的可能不是刚构建的产物
PATH_HIT="$(command -v laew 2>/dev/null || true)"
if [[ -n "$PATH_HIT" && "$PATH_HIT" != "$ROOT_DIR/laew" ]]; then
  echo "" >&2
  echo "[rebuild] ⚠️  ============================================================" >&2
  echo "[rebuild] ⚠️  PATH 中的 laew 解析到: $PATH_HIT" >&2
  echo "[rebuild] ⚠️  与刚构建的产物路径不同: $ROOT_DIR/laew" >&2
  echo "[rebuild] ⚠️  直接敲 'laew' 时命中的可能是旧版本,导致 ./laew --version" >&2
  echo "[rebuild] ⚠️  与 laew --version 输出不一致" >&2
  echo "[rebuild] ⚠️  建议: 用绝对路径 '$ROOT_DIR/laew' 或调整 PATH" >&2
  echo "[rebuild] ⚠️  ============================================================" >&2
fi

echo "[rebuild] 已输出: $ROOT_DIR/laew"
echo "[rebuild] 完成 ✓"
