# --- 4c. JSON 自动修复链端到端(feat 533d6c9,src/agent/json_repair.rs)---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4c —— 改这里不要动入口脚本。
# 验证:当 LLM 返回含 smart quote / 全角逗号 / trailing comma 的"半坏 JSON"时,
# src/agent/json_repair.rs 的 8 段修复链真实生效,Yolo 分类仍能解析成功。
section "4c. JSON 自动修复链端到端(半坏 JSON 模式)"
FLAKY_MOCK_LOG="testReport/mock_requests-flaky-$RUN_ID.jsonl"
FLAKY_MOCK_PORT=18901
python3 scripts/mock_llm_server.py $FLAKY_MOCK_PORT "$FLAKY_MOCK_LOG" --flaky &>/dev/null &
FLAKY_MOCK_PID=$!; wait_port $FLAKY_MOCK_PORT $FLAKY_MOCK_PID || echo "  [WARN] mock 未就绪($FLAKY_MOCK_PORT)" | tee -a "$REPORT"
run "$LAEW" provider add --protocol anthropic --provider-name flaky --model-name claude-flaky \
  --end-point "http://127.0.0.1:$FLAKY_MOCK_PORT" --api-key sk-flaky >/dev/null 2>&1
ID_FLAKY=$(run "$LAEW" provider list 2>/dev/null | grep flaky | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_FLAKY" >/dev/null 2>&1
OUT=$(run "$LAEW" -p "请帮我执行一个测试命令")
# Yolo 解析被半坏 JSON 触发修复后,后续 subagent → Bash echo 应正常返回 MOCK_FINAL_ANSWER
echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "JSON 修复链生效后端到端链路仍贯通"
# 修复路径真实触发:mock 响应含 smart quote(修复前已损坏);检查原始响应是否含 smart quote
# 由于 mock 只记录请求不记录响应,改用「响应未被解析直接抛错则最后无文本」的反向校验:
# 若没有 JSON 修复,链路会在 Yolo 分类处挂掉,根本不会进入 subagent/Bash。
# 此处只要 subagent tool_call 真发生 → 修复一定触发了。
grep -q '"name": "Bash"' "$FLAKY_MOCK_LOG"; check $? "mock 日志含 Bash tool_call(修复后链路已贯通)"
kill $FLAKY_MOCK_PID 2>/dev/null
run "$LAEW" provider delete "$ID_FLAKY" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$FLAKY_MOCK_LOG"
