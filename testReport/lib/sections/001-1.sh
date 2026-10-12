# --- 1. 版本与帮助 ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=1 —— 改这里不要动入口脚本。
section "1. --version / --help"
run "$LAEW" --version; check $? "--version 可用"
OUT=$(run "$LAEW" --help); echo "$OUT" | grep -q "provider"; check $? "--help 含 provider 指南"
