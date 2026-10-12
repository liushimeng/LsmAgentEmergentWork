# --- 7e. D8 会话成本估算与 /cost 面板(2026-09-17 第 76 轮) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=7e —— 改这里不要动入口脚本。
# 价格表语义:有价模型(gpt-4o-mini)→ 用量行成本 + /cost 分解 + 导出累计成本行;
# 无价模型(claude-mock,内置表未收录)→ 仅 token 统计,不虚报 $0。
section "7e. D8 会话成本估算与 /cost 面板"
rm -rf /tmp/laew-e2e-cost-root /tmp/laew-e2e-cost-work
mkdir -p /tmp/laew-e2e-cost-root /tmp/laew-e2e-cost-work
cp laew /tmp/laew-e2e-cost-root/laew
python3 scripts/mock_llm_server.py $MOCK_PORT "$ROOT_DIR/testReport/mock_requests-7e-$RUN_ID.jsonl" &>/dev/null &
MOCK7E_PID=$!; wait_port $MOCK_PORT $MOCK7E_PID || echo "  [WARN] mock 未就绪($MOCK_PORT)" | tee -a "$REPORT"
COSTBIN=/tmp/laew-e2e-cost-root/laew
run "$COSTBIN" provider add --protocol anthropic --provider-name costA --model-name gpt-4o-mini --end-point http://127.0.0.1:$MOCK_PORT --api-key sk-cost-a >/dev/null 2>&1
run "$COSTBIN" provider add --protocol anthropic --provider-name costB --model-name claude-mock --end-point http://127.0.0.1:$MOCK_PORT --api-key sk-cost-b >/dev/null 2>&1
OUT=$(run "$COSTBIN" provider list)
ID_COST_A=$(echo "$OUT" | grep costA | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
ID_COST_B=$(echo "$OUT" | grep costB | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
# 1) 无价模型:任务后 /cost 显示「无内置参考价」,用量行不追加成本
OUT=$(cd /tmp/laew-e2e-cost-work && printf "/provider use $ID_COST_B\n你好\n/cost\n/exit\n" | run timeout 90 "$COSTBIN")
echo "$OUT" | grep -q "无内置参考价"; check $? "7e-1 无价模型 /cost 仅统计 token"
echo "$OUT" | grep "本次用量" | grep -q "成本≈"; [ $? -ne 0 ]; check $? "7e-2 无价模型用量行不追加成本"
# 2) 有价模型:新进程(空会话)→ 任务 → /cost 分解 + 用量行成本 + /export 累计成本行
OUT=$(cd /tmp/laew-e2e-cost-work && printf "/provider use $ID_COST_A\n你好\n/cost\n/export cost.md\n/exit\n" | run timeout 90 "$COSTBIN")
echo "$OUT" | grep -q "成本分解(按当前模型价)"; check $? "7e-3 /cost 显示成本分解"
echo "$OUT" | grep -q "gpt-4o-mini"; check $? "7e-4 /cost 显示模型名"
echo "$OUT" | grep "本次用量" | grep -qF "成本≈$"; check $? "7e-5 用量行尾追加成本"
echo "$OUT" | grep -q "会话实记累计"; check $? "7e-6 /cost 显示实记累计"
grep -q "累计成本(估算)" /tmp/laew-e2e-cost-work/cost.md 2>/dev/null; check $? "7e-7 导出含累计成本行"
kill $MOCK7E_PID 2>/dev/null
rm -rf /tmp/laew-e2e-cost-root /tmp/laew-e2e-cost-work
