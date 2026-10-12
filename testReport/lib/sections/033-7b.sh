# --- 7b. 自定义斜杠命令 + 会话导出(D2/D8,2026-09-10 第 17 轮) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=7b —— 改这里不要动入口脚本。
section "7b. 自定义斜杠命令与会话导出"
# 2026-09-11 第三十四轮修复:基础 mock 在 §5d 后已被 kill,本节 /e2e-hello
# dispatch 依赖 mock 编排 —— 此前静默降级为 [agent error],断言只覆盖本地
# 输出仍在通过,真实编排链路从未被测到。本节独立拉起 mock 复用 18899 端口。
python3 scripts/mock_llm_server.py $MOCK_PORT "$ROOT_DIR/testReport/mock_requests-7b-$RUN_ID.jsonl" &>/dev/null &
MOCK7B_PID=$!; wait_port $MOCK_PORT $MOCK7B_PID || echo "  [WARN] mock 未就绪($MOCK_PORT)" | tee -a "$REPORT"
# 2026-09-17 第 71 轮:§5e delete 掉 active 记录后主库处于无 provider 状态,
# TUI dispatch 守门会拦截 /e2e-hello(不再走 NoopLlm 空转)。本节 dispatch 前
# 显式切回 mockA(端点=本节 mock 端口),恢复真实编排链路覆盖。
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -rf /tmp/laew-e2e-cmd-work; mkdir -p /tmp/laew-e2e-cmd-work/.laew/commands
cat > /tmp/laew-e2e-cmd-work/.laew/commands/e2e-hello.md <<'CMDEOF'
---
description: e2e 测试命令
argument-hint: [name]
---
请向 $ARGUMENTS 问好,并说明 $1 是第一个参数。
CMDEOF
# /commands 列出自定义命令(纯本地,不依赖 mock)
OUT=$(cd /tmp/laew-e2e-cmd-work && printf '/commands\n/exit\n' | run "$LAEW")
echo "$OUT" | grep -q "e2e-hello"; check $? "/commands 列出自定义命令"
echo "$OUT" | grep -q "\.laew/commands"; check $? "/commands 显示来源路径"
# 自定义命令 dispatch(经 mock 编排) + /export 落盘断言
OUT=$(cd /tmp/laew-e2e-cmd-work && printf '/e2e-hello laew\n/export\n/exit\n' | run "$LAEW")
echo "$OUT" | grep -q "custom command.*e2e-hello"; check $? "自定义命令 dispatch 显示来源"
echo "$OUT" | grep -q "已导出 Markdown"; check $? "/export 导出成功提示"
# 第 23 轮(D07)起导出成功行显示相对文件名(长路径收敛),按文件名在工作目录定位
EXPORT_NAME=$(echo "$OUT" | grep -oE "laew-export-[0-9]+-[0-9]+\.md" | head -1)
EXPORT_FILE="/tmp/laew-e2e-cmd-work/${EXPORT_NAME:-not-found}"
[ -n "$EXPORT_NAME" ] && [ -f "$EXPORT_FILE" ]; check $? "/export 文件已落盘(${EXPORT_FILE:-未找到})"
grep -q "请向 laew 问好" "$EXPORT_FILE" 2>/dev/null; check $? "导出含命令展开提示词"
grep -q "/e2e-hello laew" "$EXPORT_FILE" 2>/dev/null; check $? "导出含原始命令输入"
# 2026-09-11 第三十四轮修复:原写法 `grep -q "1 轮对话" "$OUT"` 把 laew 的多行
# 输出整体当作「文件名」传给 grep(ENAMETOOLONG → 恒 FAIL,即遗留 FAIL-4);
# 与本节其它断言统一为 stdin 管道写法。
echo "$OUT" | grep -q "1 轮对话"; check $? "/export 提示轮数"
# 显式路径拒绝覆盖(保护用户文件)
touch /tmp/laew-e2e-cmd-work/exists.md
OUT=$(cd /tmp/laew-e2e-cmd-work && printf '/export exists.md\n/exit\n' | run "$LAEW")
echo "$OUT" | grep -q "拒绝覆盖"; check $? "/export 显式路径已存在时拒绝覆盖"
kill $MOCK7B_PID 2>/dev/null
rm -rf /tmp/laew-e2e-cmd-work
