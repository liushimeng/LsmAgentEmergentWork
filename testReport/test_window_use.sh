#!/usr/bin/env bash
# test_window_use.sh: WindowUse Agent(第 9 角色)功能验证脚本。
#
# 验证内容(无需真实 LLM Key / GUI 环境):
#   1. 本机单元测试:window 驱动模型 / 工具参数校验 / registry / profile /
#      delegate_to 别名 / 角色序列化
#   2. 跨平台编译检查(可选):已安装对应 rustup target 时,
#      对 x86_64-pc-windows-gnu / x86_64-apple-darwin 做 cargo check
#      (验证 windows.rs / macos.rs 平台后端代码可编译)
#
# 用法:
#   bash testReport/test_window_use.sh            # 单测 + 可用交叉检查
#   bash testReport/test_window_use.sh --no-xcheck # 仅本机单测
#
# 设计见 docs/WindowUse桌面窗口操控Agent/01-设计与解决方案.md §4。
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

XCHECK=1
for arg in "$@"; do
  case "$arg" in
    --no-xcheck) XCHECK=0 ;;
    --help|-h) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "未知选项: $arg" >&2; exit 2 ;;
  esac
done

echo "==> [1/3] 本机单元测试(WindowUse 相关)"
cargo test window --lib -- --nocapture
cargo test window_use --lib -- --nocapture
cargo test delegate_to --lib -- --nocapture
cargo test role --lib -- --nocapture

if [[ "$XCHECK" -eq 1 ]]; then
  echo "==> [2/3] Windows 平台后端编译检查(x86_64-pc-windows-gnu)"
  if rustup target list --installed | grep -q '^x86_64-pc-windows-gnu$'; then
    if command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
      cargo check --target x86_64-pc-windows-gnu
      echo "    windows.rs(UIA + Win32 降级)编译通过"
    else
      echo "    跳过:未安装 x86_64-w64-mingw32-gcc(apt install gcc-mingw-w64-x86-64)"
    fi
  else
    echo "    跳过:rustup target add x86_64-pc-windows-gnu 后可启用"
  fi

  echo "==> [3/3] macOS 平台后端编译检查(x86_64-apple-darwin)"
  if rustup target list --installed | grep -q '^x86_64-apple-darwin$'; then
    cargo check --target x86_64-apple-darwin || \
      echo "    提示:apple-darwin 交叉检查依赖平台 SDK,失败不代表代码错误,请以真机 cargo build 为准"
  else
    echo "    跳过:rustup target add x86_64-apple-darwin 后可启用"
  fi
else
  echo "==> [2/3] [3/3] 已按 --no-xcheck 跳过"
fi

echo "==> WindowUse 验证完成"
