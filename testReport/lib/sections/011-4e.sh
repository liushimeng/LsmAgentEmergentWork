# --- 4e. WorkFlow 依赖分层 + 同层 SubAgent 并行调度端到端(本轮 feat)---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4e —— 改这里不要动入口脚本。
# 验证:medium 档位下 Main-Work 拆出 3 个互相独立(无 depends_on)的 WorkFlow 时,
# orchestrator 自动分层并在同层并发执行(tokio::spawn + Semaphore 上限 3)。
section "4e. WorkFlow 同层并行调度端到端(--parallel-wfs 模式)"
PAR_MOCK_LOG="testReport/mock_requests-par-$RUN_ID.jsonl"
PAR_MOCK_PORT=18903
python3 scripts/mock_llm_server.py $PAR_MOCK_PORT "$PAR_MOCK_LOG" --parallel-wfs &>/dev/null &
PAR_MOCK_PID=$!; wait_port $PAR_MOCK_PORT $PAR_MOCK_PID || echo "  [WARN] mock 未就绪($PAR_MOCK_PORT)" | tee -a "$REPORT"
run "$LAEW" provider add --protocol anthropic --provider-name parwfs --model-name claude-par \
  --end-point "http://127.0.0.1:$PAR_MOCK_PORT" --api-key sk-par >/dev/null 2>&1
ID_PAR=$(run "$LAEW" provider list 2>/dev/null | grep parwfs | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_PAR" >/dev/null 2>&1
OUT=$(run "$LAEW" -p "请并行执行三个独立验证流程")
check 0 "并行调度模式 laew 正常退出"
echo "$OUT" | grep -q "并行调度"; check $? "输出含 WorkFlow 并行调度日志(分层并发真实触发)"
# mock 侧佐证:SubAgent-Work 角色请求数 ≥ 3(3 个同层流程各自执行)
SUB_CNT=$(grep -c 'LsmAgentEmergentWork-SubAgent-Work' "$PAR_MOCK_LOG")
[ "$SUB_CNT" -ge 3 ]; check $? "mock 日志收到 >=3 次 SubAgent-Work 请求(实际 $SUB_CNT 次)"
# Main-Work 角色也被真实调用(medium 档位拆 WorkFlow)
grep -q 'LsmAgentEmergentWork-Main-Work' "$PAR_MOCK_LOG"; check $? "mock 日志含 Main-Work 请求(medium 档位链路)"
kill $PAR_MOCK_PID 2>/dev/null
run "$LAEW" provider delete "$ID_PAR" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$PAR_MOCK_LOG"
