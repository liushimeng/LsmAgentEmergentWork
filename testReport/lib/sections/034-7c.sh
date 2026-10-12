# --- 7c. D3 对话 Rewind 与分支(2026-09-10 第二十四轮) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=7c —— 改这里不要动入口脚本。
# 2026-09-17 第 71 轮改造:原「独立根目录 + 无 provider」依赖 NoopLlm 空转产生
# 对话轮次(QC fail-closed 回流属预期噪音);NoopLlm 改为 Err + dispatch 守门后,
# 无 provider 的提示词被 fail-fast 拦截、不再累积轮次。改为独立根目录 + 专用
# mock + provider add,让两轮提示词真实走编排链路(与交互 TUI 行为一致),
# 轮次累积语义不变。
section "7c. D3 对话 Rewind 与分支"
rm -rf /tmp/laew-e2e-rw-root /tmp/laew-e2e-rw-work
mkdir -p /tmp/laew-e2e-rw-root /tmp/laew-e2e-rw-work
cp laew /tmp/laew-e2e-rw-root/laew
# §7b 的 mock 已 kill,18899 端口空闲;本节独立拉起专用 mock
python3 scripts/mock_llm_server.py $MOCK_PORT "$ROOT_DIR/testReport/mock_requests-7c-$RUN_ID.jsonl" &>/dev/null &
MOCK7C_PID=$!; wait_port $MOCK_PORT $MOCK7C_PID || echo "  [WARN] mock 未就绪($MOCK_PORT)" | tee -a "$REPORT"
run /tmp/laew-e2e-rw-root/laew provider add --protocol anthropic --provider-name rw --model-name claude-rw --end-point http://127.0.0.1:$MOCK_PORT --api-key sk-rw >/dev/null 2>&1
OUT=$(cd /tmp/laew-e2e-rw-work && printf '第一轮问题\n第二轮问题\n/rewind\n/rewind 99\n/rewind 2\n/branches\n/switch rewind-1\n/undo\n/fork\n/clear\n/rewind\n/exit\n' | run timeout 90 /tmp/laew-e2e-rw-root/laew)
echo "$OUT" | grep -q "可回退的对话轮次(共 2 轮"; check $? "/rewind 列出 2 轮"
echo "$OUT" | grep -q "#1 \[.*第一轮问题"; check $? "/rewind 轮次预览含时间与原文"
echo "$OUT" | grep -q "无效轮次编号: 99"; check $? "/rewind 越界编号报错"
echo "$OUT" | grep -q "已回退到第 2 轮之前(移除 1 轮对话"; check $? "/rewind 2 回退成功"
echo "$OUT" | grep -q "已存为分支 rewind-1"; check $? "/rewind 回退前自动存分支"
echo "$OUT" | grep -q "rewind-1 \[.* 2 轮 | /rewind 2 回退前"; check $? "/branches 列出分支明细"
echo "$OUT" | grep -q "已切换到分支 rewind-1"; check $? "/switch 恢复分支(2 轮)"
echo "$OUT" | grep -q "已自动存为分支 switch-2"; check $? "/switch 切换前自动快照"
echo "$OUT" | grep -q "已存为分支 rewind-3"; check $? "/undo 撤销末轮"
echo "$OUT" | grep -q "已从当前对话分叉出新会话"; check $? "/fork 分叉新会话"
echo "$OUT" | grep -q "已存为分支 fork-4"; check $? "/fork 分叉前自动存分支"
echo "$OUT" | grep -q "已自动保存分支 clear-5"; check $? "/clear 自动快照找回提示"
echo "$OUT" | grep -q "当前会话还没有可回退的对话轮次"; check $? "/clear 后无可回退轮次"
kill $MOCK7C_PID 2>/dev/null
rm -rf /tmp/laew-e2e-rw-root /tmp/laew-e2e-rw-work
