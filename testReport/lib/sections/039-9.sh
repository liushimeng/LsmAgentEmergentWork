# --- 9. provider delete ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=9 —— 改这里不要动入口脚本。
section "9. provider delete"
run "$LAEW" provider delete "$ID_O"; check $? "删除 openai 记录"
OUT=$(run "$LAEW" provider list); echo "$OUT" | grep -vq mockO; check $? "list 不再显示 mockO"
