# --- 7. TUI 冒烟(管道喂命令,非 TTY 回退路径) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=7 —— 改这里不要动入口脚本。
section "7. TUI 冒烟测试"
OUT=$(printf '/help\n/model\n/provider list\n/new\n/exit\n' | run "$LAEW")
echo "$OUT" | grep -q "程序目录"; check $? "TUI 横幅显示程序目录(第 152 轮:根目录改名)"
echo "$OUT" | grep -q "工作目录"; check $? "TUI 横幅显示工作目录"
echo "$OUT" | grep -q "产物目录"; check $? "TUI 横幅显示产物目录(第 152 轮)"
echo "$OUT" | grep -q "项目说明"; check $? "TUI 横幅显示项目说明状态"
echo "$OUT" | grep -q "当前模型"; check $? "TUI 横幅显示当前模型"
echo "$OUT" | grep -q "Session"; check $? "TUI 横幅显示 Session ID"
# 2026-09-24 TUI 显示完整化:横幅所有行必须**严格等宽**。
# 旧实现边框写死 58 内宽 + 每行 45/46 独立填充预算,实测行宽在 59~61 之间跳动
# (右边框参差、被截断的行顶穿边框);现由 textfit::InfoBox 统一按
# min(内容自然宽, 终端可用宽) 排版。
if command -v python3 >/dev/null 2>&1; then
  # PYTHONIOENCODING=utf-8:Windows 控制台默认 GBK,打印含 ✓/║ 的失败面板会先抛
  # UnicodeEncodeError 把真实断言结果盖掉(实测本机踩到);报告也保持 ASCII 友好。
  PYTHONIOENCODING=utf-8 python3 - "$OUT" <<'PYEOF'
import sys, unicodedata
if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
def w(s):
    return sum(2 if unicodedata.east_asian_width(c) in "WF" else 1 for c in s)
rows = [l.rstrip("\n") for l in sys.argv[1].splitlines()]
try:
    top = next(i for i, l in enumerate(rows) if l.startswith("╔"))
    bot = next(i for i, l in enumerate(rows[top:], top) if l.startswith("╚"))
except StopIteration:
    print("  banner box (╔..╚) not found"); sys.exit(1)
box = rows[top:bot + 1]
widths = sorted({w(l) for l in box})
if len(widths) != 1:
    print(f"  banner rows NOT equal width: {widths}")
    for l in box:
        print(f"    {w(l):>3} |{l}")
    sys.exit(1)
if not (40 <= widths[0] <= 200):
    print(f"  banner box width out of range: {widths[0]}"); sys.exit(1)
print(f"  banner {len(box)} rows all {widths[0]} columns OK")
PYEOF
  check $? "TUI 横幅所有行严格等宽"
else
  echo "  [SKIP] python3 不可用,跳过横幅等宽校验"
fi
echo "$OUT" | grep -q "provider add"; check $? "/help 输出命令指南"
echo "$OUT" | grep -q "开启新会话\|已开启新会话"; check $? "/new 命令生效"
# 第 114 轮:/agents 自感知面板(名册 / 上限 / 最近作业;无需 LLM,离线可查)
OUT=$(printf '/agents\n/exit\n' | run "$LAEW")
echo "$OUT" | grep -q "自感知动态子 Agent"; check $? "/agents 输出自感知面板"
echo "$OUT" | grep -q "general-purpose"; check $? "/agents 列出子 Agent 类型名册"
echo "$OUT" | grep -q "LAEW_SELF_SPAWN"; check $? "/agents 显示开关与上限"
echo "$OUT" | grep -q "explore"; check $? "/agents 列出只读侦察类型"
