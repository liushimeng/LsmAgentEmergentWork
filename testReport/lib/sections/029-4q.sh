# --- 4q. Main-Work 编排层 ReAct 化与 MCP_Web_Use 信息收集端到端(第 122 轮,2026-09-23) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4q —— 改这里不要动入口脚本。
# 设计见 docs/Main-Work工具扩展与ReAct改造/01-设计与解决方案.md
# 4q-1: mock 路由让 Main-Work 在编排前调用 MCP_Web_Use(action=open) → 验证
#       Main-Work 工具面含 MCP_Web_Use,且编排 JSON 输出合法。
# 4q-2: 断言 Main-Work wire system 含本轮新增的「编排循环(ReAct 模式)」与
#       「任务前提验证清单」两节 —— 提示词真的送达 Provider,而非只写在源码里。
# 4q-3: mock 让 Main-Work 连续多轮返回相同 Read → LoopGuard 止损(编排层接入)。
# 注:Main-Work 编排 JSON 解析与 topo 分层并行由单元测试覆盖。
# (原 4m 节已被第 77 轮 Read 工具多模态占用,沿用 4q 顺位。)
section "4q. Main-Work ReAct 与 MCP_Web_Use 信息收集端到端(第 122 轮)"
MAIN_WORK_ROUTER="testReport/router-mainwork-$RUN_ID.json"
# Main-Work 拆解链路:Yolo → Main-Work → SubAgent。
# 关键词命中后,Yolo 走 Simple 即可(Main-Work 需要 medium 才能跑到)。
# 这里改用 medium(用关键词 + 改造后 Yolo 最低判 medium)触发 Main-Work。
cat > "$MAIN_WORK_ROUTER" <<'JSON'
{
  "rules": [
    {
      "keywords": ["LAEWMainWorkReAct"],
      "tools": [
        {"tool": "Bash", "args": {"command": "ls Cargo.toml"}},
        {"tool": "Read", "args": {"file_path": "Cargo.toml"}},
        {"tool": "Read", "args": {"file_path": "Cargo.toml"}},
        {"tool": "Read", "args": {"file_path": "Cargo.toml"}},
        {"tool": "Read", "args": {"file_path": "Cargo.toml"}}
      ],
      "yolo": {
        "task_level": "medium",
        "goal_summary": "LAEWMainWorkReAct 编排层验证:多步工作流拆解",
        "purpose": "验证 Main-Work ReAct 化与 MCP_Web_Use 信息收集",
        "intent": "code_refactor",
        "agent_role": "mainwork",
        "decomposition_plan": ["读 Cargo.toml", "列 wf 列表"]
      }
    }
  ]
}
JSON
MAIN_WORK_MOCK_LOG="testReport/mock_requests-mainwork-$RUN_ID.jsonl"
MAIN_WORK_MOCK_PORT=18909
python3 scripts/mock_llm_server.py $MAIN_WORK_MOCK_PORT "$MAIN_WORK_MOCK_LOG" --prompt-router-file "$MAIN_WORK_ROUTER" &>/dev/null &
MAIN_WORK_MOCK_PID=$!
wait_port $MAIN_WORK_MOCK_PORT $MAIN_WORK_MOCK_PID || echo "  [WARN] mock 未就绪" | tee -a "$REPORT"
run "$LAEW" provider add --protocol anthropic --provider-name mainwork-on --model-name m-mainwork \
  --end-point "http://127.0.0.1:$MAIN_WORK_MOCK_PORT" --api-key sk-mainwork >/dev/null 2>&1
ID_MAIN_WORK=$(run "$LAEW" provider list 2>/dev/null | grep mainwork-on | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)

run "$LAEW" provider use "$ID_MAIN_WORK" >/dev/null 2>&1
OUT=$(run "$LAEW" -p "LAEWMainWorkReAct 多步工作流拆解任务")
# 4q-1:MCP_Web_Use 工具面注册 + 主链路贯通
echo "$OUT" | grep -qE "任务收口|outcome|用量"; check $? "4q-1 Main-Work 编排主链路贯通(任务正常收口)"
python3 - "$MAIN_WORK_MOCK_LOG" "$ID_MAIN_WORK" <<'PYEOF' 2>&1 | tee -a "$REPORT"
import json, sys
reqs = [json.loads(l) for l in open(sys.argv[1], encoding="utf-8") if l.strip()]
ok = True
def chk(cond, name):
    global ok
    print(f"  [{'PASS' if cond else 'FAIL'}] {name}")
    ok = ok and cond

def system_of(body):
    s = body.get("system")
    if isinstance(s, list):
        s = "\n".join(p.get("text", "") for p in s if isinstance(p, dict))
    return s if isinstance(s, str) else ""

# 4q-1:Main-Work 工具面含 MCP_Web_Use(取任意一次 Main-Work 请求,断言 tools 列表)
mw = [r["body"] for r in reqs if "LsmAgentEmergentWork-Main-Work" in system_of(r["body"])]
chk(len(mw) > 0, f"Main-Work 至少发 1 次请求(实际 {len(mw)})")
if mw:
    tools = mw[0].get("tools", [])
    tool_names = [t.get("name", "") for t in tools]
    chk("MCP_Web_Use" in tool_names,
        f"Main-Work 工具面含 MCP_Web_Use(实际 {tool_names})")
    chk("Bash" in tool_names, "Main-Work 工具面仍含 Bash")
    chk("Read" in tool_names, "Main-Work 工具面仍含 Read")
    chk("Glob" in tool_names, "Main-Work 工具面仍含 Glob")
    chk("Grep" in tool_names, "Main-Work 工具面仍含 Grep")
    chk("TodoWrite" in tool_names, "Main-Work 工具面含 TodoWrite")
    chk("SubAgent" in tool_names, "Main-Work 工具面含 SubAgent")
    chk("Write" not in tool_names, "Main-Work 仍不持 Write")
    chk("Edit" not in tool_names, "Main-Work 仍不持 Edit")

    # 4q-2:ReAct 提示词真的送达 wire
    s = system_of(mw[0])
    chk("编排循环(ReAct 模式" in s,
        "Main-Work system 含「编排循环(ReAct 模式)」节")
    chk("任务前提验证" in s,
        "Main-Work system 含「任务前提验证清单」节")
    chk("TaskFocus" in s,
        "Main-Work system 含「TaskFocus」编排节奏关键字")
    chk("Thought(推理)" in s and "Observation(观察)" in s,
        "Main-Work system 含 Thought / Observation 三段规范")

    # 4q-3:Main-Work 编排层接入 LoopGuard(编排层 doom_loop 由单元测试覆盖:
    #     agent::react_tests::main_work_runner_doom_loop_stops
    #     agent::main_work::tests::main_work_*。
    # mock_router 在 retry attempt 之间锁的是 session_id,Main-Work 每次重试都
    # 拿 tools[0],不会形成「同 Read 连续 N 次」语义;端到端层面断言软提醒
    # 不可靠,改由单元测试钉死。)
else:
    chk(False, "未见 Main-Work 请求,跳过工具面/提示词/wire 断言")
sys.exit(0 if ok else 1)
PYEOF
check $? "4q-1/4q-2 wire 断言:Main-Work 工具面含 MCP_Web_Use + ReAct 提示词送达"

kill $MAIN_WORK_MOCK_PID 2>/dev/null; wait $MAIN_WORK_MOCK_PID 2>/dev/null
sleep 0.2
rm -f "$MAIN_WORK_ROUTER" "$MAIN_WORK_MOCK_LOG"
run "$LAEW" provider delete "$ID_MAIN_WORK" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1


section "5d. Debug 模式端到端(Debug Agent)"
# 第 152 轮:DebugReport 落**产物根**(默认 = 启动时工作目录),不再落二进制所在目录。
# 本节 laew 从仓库根启动,工作目录即仓库根 → 报告落在 <repo>/DebugReport/。
DBG_REAL_DIR="$(pwd)/DebugReport"
DBG_MARKER=$(mktemp)
OUT=$(run "$LAEW" -p "请帮我执行一个测试命令" -debug)
echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "-debug 任务链路贯通(输出含 MOCK_FINAL_ANSWER)"
grep -q '"user-agent": "LsmAgentEmergentWork-Debug/' "$MOCK_LOG"; check $? "mock 日志含 Debug Agent 请求(User-Agent 头部可辨识)"
# user_id 是嵌在请求体里的 JSON 字符串,mock 落盘后内层引号被转义(\"agent\":\"...\")
grep -q 'agent\\":\\"LsmAgentEmergentWork-Debug' "$MOCK_LOG"; check $? "anthropic metadata.user_id.agent 携带 Debug 角色"
DBG_NEW_FILES=$(find "$DBG_REAL_DIR" -name 'debug_report_*.md' -newer "$DBG_MARKER" 2>/dev/null)
[ -n "$DBG_NEW_FILES" ]; check $? "DebugReport 新生成报告文件"
rm -f "$DBG_MARKER"
# 清理本节生成的报告(mock 产物,无保留价值;DebugReport 本身已 gitignore)
echo "$DBG_NEW_FILES" | while read -r f; do [ -n "$f" ] && rm -f "$f"; done
