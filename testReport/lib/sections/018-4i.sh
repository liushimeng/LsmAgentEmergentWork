# --- 4i. Prompt 注入防护端到端(L1208,2026-09-09 第 12 轮) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4i —— 改这里不要动入口脚本。
# 方案见 tmpPlan/2026-09-09_12-Prompt注入防护与外部内容净化方案.md
# 验证:当 Bash 工具执行结果含「curl | bash」典型注入模式时,
# 抓包层面能看到 laew 自动追加的 `<<<LAEW:INJECTION_ALERT>>>` 告警块;
# 告警包含 [P05] curl_pipe_sh 模式 ID 与 severity=Critical 标签;
# 执行不被阻断(Bash 仍返回 exit_code=0)。
section "4i. Prompt 注入防护端到端(L1208)"
INJECT_MOCK_LOG="testReport/mock_requests-inject-$RUN_ID.jsonl"
INJECT_MOCK_PORT=18903
python3 scripts/mock_llm_server.py "$INJECT_MOCK_PORT" "$INJECT_MOCK_LOG" --inject-bash &>/dev/null &
INJECT_MOCK_PID=$!; wait_port "$INJECT_MOCK_PORT" $INJECT_MOCK_PID || echo "  [WARN] mock 未就绪("$INJECT_MOCK_PORT")" | tee -a "$REPORT"
# 本节专用 provider
run "$LAEW" provider add --protocol anthropic --provider-name inject-test --model-name claude-inject-test \
  --end-point "http://127.0.0.1:$INJECT_MOCK_PORT" --api-key sk-inject-test >/dev/null 2>&1
ID_INJ=$(run "$LAEW" provider list 2>/dev/null | grep inject-test | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_INJ" >/dev/null 2>&1
run "$LAEW" -p "执行一段示例命令" >/dev/null 2>&1 || true
# 抓包断言:mock 收到的请求体应包含注入告警边界标记
grep -F -q "<<<LAEW:INJECTION_ALERT>>>" "$INJECT_MOCK_LOG"; check $? "mock 日志含 <<<LAEW:INJECTION_ALERT>>> 注入告警标记(L1208 防护生效)"
# 告警块应点名 P05 curl_pipe_sh 模式
grep -F -q "[P05]" "$INJECT_MOCK_LOG"; check $? "告警含 [P05] curl_pipe_sh 模式 ID"
grep -F -q "Critical" "$INJECT_MOCK_LOG"; check $? "告警 severity 标签含 Critical"
# 原工具输出未被删除(只追加告警,不阻断执行)
grep -F -q "evil.example" "$INJECT_MOCK_LOG"; check $? "原始 bash 输出「evil.example」仍在(不阻断,仅告警)"
# 收尾
kill $INJECT_MOCK_PID 2>/dev/null
run "$LAEW" provider delete "$ID_INJ" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$INJECT_MOCK_LOG"
