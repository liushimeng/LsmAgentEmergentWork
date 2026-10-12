# --- 5. Anthropic 协议端到端 ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=5 —— 改这里不要动入口脚本。
section "5. Anthropic 协议端到端(工具调用循环)"
OUT=$(run "$LAEW" -p "请帮我执行一个测试命令"); echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "返回最终文本"
