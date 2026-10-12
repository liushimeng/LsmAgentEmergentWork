# --- 4p. SubAgent-Work ReAct 强化与工具连续工作模式端到端(第 120 轮,2026-09-23) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4p —— 改这里不要动入口脚本。
# 设计见 docs/SubAgentWork执行层ReAct与连续工作模式/01-设计与解决方案.md
# 4p-1: mock 路由让 SubAgent 连续 5 轮发起**完全相同**的 Read(同参数 + 同文件内容)
#       → LoopGuard 进展键(工具 + 参数稳定 JSON + 结果摘要)连续命中 →
#       第 2 次软提醒 / 第 3 次宽限轮强提醒 / 第 4 次无进展止损,**不跑满 32 轮预算**。
#       断言:SubAgent 请求恰好 4 次 + 第 3 次 system 含【无进展警告】+
#             第 4 次 messages 含「宽限轮」强提醒(user 消息,保证送达 LLM)。
# 4p-2: 断言 SubAgent wire system 含本轮新增的「执行循环(ReAct 模式)」与
#       「连续工作模式」两节 —— 提示词真的送达 Provider,而非只写在源码里。
# 4p-3: LAEW_REACT_GUARD=off 重跑同一 router → 不再无进展止损(开关可回退),
#       SubAgent 请求数显著多于 4(改由 no_text_converge / max_iterations 兜底)。
# 注:同批只读工具**并发执行**由单元测试覆盖(mock 每轮只发一个 tool_use 块,
#     无法在 wire 层构造同批多调用):
#     agent::react_tests::parallel_readonly_batch_preserves_order
#     agent::tool_exec::tests::{readonly_run_of_two_or_more_forms_batch,
#                               parallel_batch_returns_results_by_index}
section "4p. SubAgent-Work ReAct 与连续工作模式端到端(第 120 轮)"
REACT_ROUTER="testReport/router-react-$RUN_ID.json"
cat > "$REACT_ROUTER" <<'JSON'
{
  "rules": [
    {
      "keywords": ["LAEW无进展止损验证"],
      "tools": [
        {"tool": "Read", "args": {"file_path": "Cargo.toml"}},
        {"tool": "Read", "args": {"file_path": "Cargo.toml"}},
        {"tool": "Read", "args": {"file_path": "Cargo.toml"}},
        {"tool": "Read", "args": {"file_path": "Cargo.toml"}},
        {"tool": "Read", "args": {"file_path": "Cargo.toml"}}
      ],
      "yolo": {
        "task_level": "simple",
        "goal_summary": "LAEW无进展止损验证:重复读取 Cargo.toml",
        "purpose": "验证 ReAct 循环守卫的无进展止损",
        "intent": "file_operation",
        "agent_role": "subagent",
        "decomposition_plan": ["重复读取 Cargo.toml"]
      }
    }
  ]
}
JSON
REACT_MOCK_LOG="testReport/mock_requests-react-$RUN_ID.jsonl"
REACT_OFF_LOG="testReport/mock_requests-react-off-$RUN_ID.jsonl"
REACT_MOCK_PORT=18907
REACT_OFF_PORT=18908
python3 scripts/mock_llm_server.py $REACT_MOCK_PORT "$REACT_MOCK_LOG" --prompt-router-file "$REACT_ROUTER" &>/dev/null &
REACT_MOCK_PID=$!
python3 scripts/mock_llm_server.py $REACT_OFF_PORT "$REACT_OFF_LOG" --prompt-router-file "$REACT_ROUTER" &>/dev/null &
REACT_OFF_PID=$!
wait_port $REACT_MOCK_PORT $REACT_MOCK_PID wait_port $REACT_OFF_PORT $REACT_OFF_PID || echo "  [WARN] mock 未就绪" | tee -a "$REPORT"
run "$LAEW" provider add --protocol anthropic --provider-name react-on --model-name m-react-on \
  --end-point "http://127.0.0.1:$REACT_MOCK_PORT" --api-key sk-react-on >/dev/null 2>&1
run "$LAEW" provider add --protocol anthropic --provider-name react-off --model-name m-react-off \
  --end-point "http://127.0.0.1:$REACT_OFF_PORT" --api-key sk-react-off >/dev/null 2>&1
ID_REACT_ON=$(run "$LAEW" provider list 2>/dev/null | grep react-on | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
ID_REACT_OFF=$(run "$LAEW" provider list 2>/dev/null | grep react-off | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
