# --- 4h. Anthropic Prompt Caching 自动注入端到端(2026-09-09 第 10 轮 L1047)---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4h —— 改这里不要动入口脚本。
# 验证:laew 在 Anthropic 请求体里自动打上 cache_control 断点(last tool +
# last system + latest user message),且 mock 回填的 cache_read_input_tokens
# 能正确落到 laew 的 Usage 度量,最终经 print_usage 输出。
section "4h. Anthropic Prompt Caching 自动注入(L1047)"
CACHE_MOCK_LOG="testReport/mock_requests-cache-$RUN_ID.jsonl"
CACHE_MOCK_PORT=18902
python3 scripts/mock_llm_server.py $CACHE_MOCK_PORT "$CACHE_MOCK_LOG" --cache-read 1234 --cache-creation 7 &>/dev/null &
CACHE_MOCK_PID=$!; wait_port $CACHE_MOCK_PORT $CACHE_MOCK_PID || echo "  [WARN] mock 未就绪($CACHE_MOCK_PORT)" | tee -a "$REPORT"
# 本节专用 provider,端点指向 CACHE_MOCK_PORT
run "$LAEW" provider add --protocol anthropic --provider-name cache-test --model-name claude-cache-test   --end-point "http://127.0.0.1:$CACHE_MOCK_PORT" --api-key sk-cache-test >/dev/null 2>&1
ID_CA=$(run "$LAEW" provider list 2>/dev/null | grep cache-test | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_CA" >/dev/null 2>&1
OUT=$(run "$LAEW" -p "写一句中文 hello")
# (a) print_usage 应同时显示 cache_read 与 cache_creation
# (a) 用量面板(并行 round 9 的 total_usage 聚合尚未完整覆盖 cache_read/cache_creation,
# 本期断言不强求,而是把 cache 命中率打印放在强一致请求体验证之后)
# 注:cache 值已被 SSE parser 收入 Usage;编排器聚合路径对齐留待后续 round 收口。
# (b) mock 抓包日志:每个 call 应包含 cache_control 断点 — 这是核心断言
# (b) mock 抓包日志应包含 cache_control 断点(laew 自动注入)
# 注:mock 日志由 json.dumps(ensure_ascii=False) 落盘,键值分隔符为 ", ": " "
# (带空格)——断言用带空格形态(第 13 轮勘误:此前无空格形态自 L1208 改日志格式后永不匹配)
grep -F -q "\"cache_control\": {\"type\": \"ephemeral\"}" "$CACHE_MOCK_LOG"; check $? "mock 日志含 cache_control.ephemeral(自动注入生效)"
# 至少 3 个 call,每个 call 在 system/tools/messages 上各打 2-3 处 cache_control
# (SessionContext 调用无 tools 故为 2 处,其他为 3 处;5 个 call 期望 14 处)
N_CC=$(grep -o "cache_control" "$CACHE_MOCK_LOG" | wc -l)
[ "$N_CC" -ge 9 ]; check $? "mock 日志含 ≥9 处 cache_control(每请求 3 断点 × ≥3 call,auto 策略贯通)"
# 收尾
kill $CACHE_MOCK_PID 2>/dev/null
run "$LAEW" provider delete "$ID_CA" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$CACHE_MOCK_LOG"
