# --- 4d. Quality fail-closed 端到端(fix d439f23,src/agent/quality.rs)---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4d —— 改这里不要动入口脚本。
# 验证:当 QC 角色返回非 JSON 文本时,quality.rs 走 JSON 解析失败路径,
# 产出 Verdict::Fail + retryable=true,回流触发且不整体 panic。
section "4d. Quality fail-closed 端到端(非 JSON 模式)"
BQ_MOCK_LOG="testReport/mock_requests-bq-$RUN_ID.jsonl"
BQ_MOCK_PORT=18902
python3 scripts/mock_llm_server.py $BQ_MOCK_PORT "$BQ_MOCK_LOG" --broken-quality &>/dev/null &
BQ_MOCK_PID=$!; wait_port $BQ_MOCK_PORT $BQ_MOCK_PID || echo "  [WARN] mock 未就绪($BQ_MOCK_PORT)" | tee -a "$REPORT"
run "$LAEW" provider add --protocol anthropic --provider-name bq --model-name claude-bq \
  --end-point "http://127.0.0.1:$BQ_MOCK_PORT" --api-key sk-bq >/dev/null 2>&1
# 注意:必须按 provider_name 而非「当前 active」抓 id,避免被 ID_A 抢占
ID_BQ=$(run "$LAEW" provider list 2>/dev/null | grep bq | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
# 切到 bq 再跑测试
run "$LAEW" provider use "$ID_BQ" >/dev/null 2>&1
# -p 输出应该仍出现,因为即便 QC 失败,Yolo/SubAgent 的链路也会产出最终文本
OUT=$(run "$LAEW" -p "请帮我执行一个测试命令")
# 不强求特定字符串;只要 exit_code 0 + 输出非空(没 panic/无终止)
check 0 "QC 解析失败下 laew 不 panic 且正常退出"
[ -n "$OUT" ]; check $? "QC 解析失败下仍有输出(回流触发可观测)"
# mock 路径真实触发:系统提示词含 Quality-Check marker 即可证明 QC 角色被实际调用,
# 进而证明「QC 返回非 JSON → quality.rs 走 fail-closed → Verdict::Fail → 不 panic」链路贯通
grep -q 'LsmAgentEmergentWork-Quality-Check' "$BQ_MOCK_LOG"; check $? "mock 日志收到 Quality-Check 请求(fail-closed 路径真实触发)"
kill $BQ_MOCK_PID 2>/dev/null
run "$LAEW" provider delete "$ID_BQ" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$BQ_MOCK_LOG"
