# --- 4. mock LLM + OpenAI 协议端到端 ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4 —— 改这里不要动入口脚本。
section "4. OpenAI 协议端到端(工具调用循环)"
python3 scripts/mock_llm_server.py $MOCK_PORT "$MOCK_LOG" &>/dev/null &
MOCK_PID=$!; wait_port $MOCK_PORT $MOCK_PID || echo "  [WARN] mock 未就绪($MOCK_PORT)" | tee -a "$REPORT"
OUT=$(run "$LAEW" -p "请帮我执行一个测试命令"); echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "返回最终文本"
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
