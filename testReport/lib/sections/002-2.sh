# --- 2. 无配置时的 -p 引导报错 ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=2 —— 改这里不要动入口脚本。
section "2. 未配置模型时 -p 的引导提示"
OUT=$(run "$LAEW" -p "hello"); echo "$OUT" | grep -q "provider add"; check $? "提示先 provider add"
