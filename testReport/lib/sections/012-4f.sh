# --- 4f. 上下文溢出自动三级恢复端到端(feat 2026-09-09_06,src/agent/overflow.rs)---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4f —— 改这里不要动入口脚本。
# 验证:subagent 第 1 次工具调用产生大输出(~28.9K 字符)、第 2 次调用收到
# HTTP 400 "prompt is too long" 时,Agent 循环自动「排水截短工具结果 → 重试」,
# 链路最终仍返回最终文本,而不是任务失败。
section "4f. 上下文溢出自动恢复端到端(--overflow-once 模式)"
OVF_MOCK_LOG="testReport/mock_requests-ovf-$RUN_ID.jsonl"
OVF_MOCK_PORT=18904
python3 scripts/mock_llm_server.py $OVF_MOCK_PORT "$OVF_MOCK_LOG" --overflow-once &>/dev/null &
OVF_MOCK_PID=$!; wait_port $OVF_MOCK_PORT $OVF_MOCK_PID || echo "  [WARN] mock 未就绪($OVF_MOCK_PORT)" | tee -a "$REPORT"
run "$LAEW" provider add --protocol anthropic --provider-name ovf --model-name claude-ovf \
  --end-point "http://127.0.0.1:$OVF_MOCK_PORT" --api-key sk-ovf >/dev/null 2>&1
ID_OVF=$(run "$LAEW" provider list 2>/dev/null | grep ovf | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_OVF" >/dev/null 2>&1
OUT=$(run "$LAEW" -p "请帮我执行一个测试命令")
# 断言1:排水恢复后重试成功,链路贯通仍返回最终文本
echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "溢出排水恢复后仍返回最终文本"
# 断言2:subagent 角色请求 ≥ 3 次(第 2 次 400 后确实重试了)
# 注:用 SubAgent 单元任务提示词的「【SubFlow」标记计数——Yolo/QC 的系统提示词
# 也会引用 SubAgent-Work 这个名字,按名字 grep 会跨角色误计。
OVF_SUB_CALLS=$(grep -c '【SubFlow' "$OVF_MOCK_LOG")
[ "$OVF_SUB_CALLS" -ge 3 ]; check $? "subagent 请求 ${OVF_SUB_CALLS} 次(>=3,溢出后重试真实发生)"
# 断言3:重试请求携带截短后的工具结果(排水在 wire 层真实生效)
grep -F -q "overflow-drain" "$OVF_MOCK_LOG"; check $? "重试请求含排水截短标记(overflow-drain)"
kill $OVF_MOCK_PID 2>/dev/null
run "$LAEW" provider delete "$ID_OVF" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$OVF_MOCK_LOG"
