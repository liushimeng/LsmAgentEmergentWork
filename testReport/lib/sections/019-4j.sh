# --- 4j. 结构化输出强制通道端到端(L6/L19,2026-09-09 第 13 轮) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4j —— 改这里不要动入口脚本。
# 方案见 tmpPlan/2026-09-09_13-结构化输出强制通道与forced-tool-choice方案.md
# 4j-1: mock --forced-tool — Quality 每轮强制;Yolo ReAct 延迟强制(探索轮 tool_choice
#       不指名,仅末轮强制 emit;mock 当轮自愿提交 → 1 轮终止),断言 wire 与工具面。
# 4j-2: mock --forced-tool --reject-tool-choice — Quality 首次 forced 请求被 400 拒绝;
#       断言 resilient.rs 自适应降级(第二次请求无 tool_choice)后链路仍贯通。
#       Yolo round 1 无 forced → mock 不拒绝;QC 强制通道被拒后降级。
section "4j. 结构化输出强制通道(L6/L19 forced tool_choice)"
FT_MOCK_LOG="testReport/mock_requests-forced-$RUN_ID.jsonl"
FT_MOCK_PORT=18904
python3 scripts/mock_llm_server.py $FT_MOCK_PORT "$FT_MOCK_LOG" --forced-tool &>/dev/null &
FT_MOCK_PID=$!; wait_port $FT_MOCK_PORT $FT_MOCK_PID || echo "  [WARN] mock 未就绪($FT_MOCK_PORT)" | tee -a "$REPORT"
run "$LAEW" provider add --protocol anthropic --provider-name forced-test --model-name claude-forced-test \
  --end-point "http://127.0.0.1:$FT_MOCK_PORT" --api-key sk-forced-test >/dev/null 2>&1
ID_FT=$(run "$LAEW" provider list 2>/dev/null | grep forced-test | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_FT" >/dev/null 2>&1
OUT=$(run "$LAEW" -p "执行一次链路验证")
echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "4j-1 forced tool_use 链路贯通(输出含 MOCK_FINAL_ANSWER)"
# wire 断言(python 解析 JSONL,不依赖字段顺序/空格):
#  - Yolo 请求恰好 1 次(emit 命中 1 轮终止,无续轮)
#  - 其 tool_choice 为 tool 指名 submit_task_classification + 禁并行
#  - Quality 请求 tool_choice 指名 submit_quality_report
python3 - "$FT_MOCK_LOG" <<'PYEOF' 2>&1 | tee -a "$REPORT"
import json, sys
reqs = [json.loads(l) for l in open(sys.argv[1], encoding="utf-8")]
def by_agent(marker):
    out = []
    for r in reqs:
        s = r["body"].get("system")
        if isinstance(s, list):
            s = "\n".join(p.get("text", "") for p in s if isinstance(p, dict))
        elif not isinstance(s, str):
            s = ""
        if marker in s:
            out.append(r)
    return out
ok = True
def chk(cond, name):
    global ok
    print(f"  [{'PASS' if cond else 'FAIL'}] {name}")
    ok = ok and cond
yolo = by_agent("LsmAgentEmergentWork-Yolo")
quality = by_agent("LsmAgentEmergentWork-Quality-Check")
chk(len(yolo) == 1, f"Yolo 请求恰好 1 次(mock 当轮自愿提交 emit,实际 {len(yolo)})")
if yolo:
    tc = yolo[0]["body"].get("tool_choice") or {}
    # 2026-09-22 ReAct 延迟强制:Yolo 探索轮 tool_choice 不指名(model 可自由调用
    # Read/Glob/Grep/Bash/MCP_Web_Use);仅最终轮强制 emit。mock 当轮自愿提交 → 探索轮
    # wire 上 tool_choice 缺省/auto。
    chk(tc.get("type") != "tool", f"Yolo 探索轮 tool_choice 不指名(实际 {tc.get('type')!r})")
    tools = [t.get("name") for t in yolo[0]["body"].get("tools", [])]
    chk("submit_task_classification" in tools, f"emit 工具在 tools 列表中(实际 {tools})")
    # 2026-09-22 ReAct 改造:Yolo 信息收集型工具面 5 件必须都在 tools 中。
    for t in ["Read", "Glob", "Grep", "Bash", "MCP_Web_Use"]:
        chk(t in tools, f"Yolo 信息收集工具面含 {t}(实际 {tools})")
if quality:
    tc = quality[0]["body"].get("tool_choice") or {}
    chk(tc.get("name") == "submit_quality_report", f"Quality wire 指名 submit_quality_report(实际 {tc.get('name')!r})")
else:
    chk(False, "未见 Quality 请求")
sys.exit(0 if ok else 1)
PYEOF
check $? "4j-1 wire 断言:forced tool_choice 指名 + emit 工具注册 + 1 轮终止"
# kill + wait 确保 mock 进程退出、端口释放(kill 是异步信号,不等待会在连续
# 两次跑 e2e 时让下一轮同端口 mock bind 失败,残留旧进程以旧 STATE/旧日志路径服务)
kill $FT_MOCK_PID 2>/dev/null; wait $FT_MOCK_PID 2>/dev/null; sleep 0.2
run "$LAEW" provider delete "$ID_FT" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$FT_MOCK_LOG"

# 4j-2: forced 被 Provider 拒绝 → resilient 自动降级 auto 重试 → 链路仍贯通
FT2_MOCK_LOG="testReport/mock_requests-forced-reject-$RUN_ID.jsonl"
FT2_MOCK_PORT=18905
python3 scripts/mock_llm_server.py $FT2_MOCK_PORT "$FT2_MOCK_LOG" --forced-tool --reject-tool-choice &>/dev/null &
FT2_MOCK_PID=$!; wait_port $FT2_MOCK_PORT $FT2_MOCK_PID || echo "  [WARN] mock 未就绪($FT2_MOCK_PORT)" | tee -a "$REPORT"
run "$LAEW" provider add --protocol anthropic --provider-name forced-reject --model-name claude-forced-reject \
  --end-point "http://127.0.0.1:$FT2_MOCK_PORT" --api-key sk-forced-reject >/dev/null 2>&1
ID_FR=$(run "$LAEW" provider list 2>/dev/null | grep forced-reject | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_FR" >/dev/null 2>&1
OUT=$(run "$LAEW" -p "执行一次降级验证")
echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "4j-2 forced 被拒后降级重试,链路仍贯通(MOCK_FINAL_ANSWER)"
python3 - "$FT2_MOCK_LOG" <<'PYEOF' 2>&1 | tee -a "$REPORT"
import json, sys
reqs = [json.loads(l) for l in open(sys.argv[1], encoding="utf-8")]
def system_of(body):
    s = body.get("system")
    if isinstance(s, list):
        s = "\n".join(p.get("text", "") for p in s if isinstance(p, dict))
    return s if isinstance(s, str) else ""

def all_bodies():
    return [r["body"] for r in reqs]

def yolo_reqs():
    return [b for b in all_bodies() if "LsmAgentEmergentWork-Yolo" in system_of(b)]
ok = True
def chk(cond, name):
    global ok
    print(f"  [{'PASS' if cond else 'FAIL'}] {name}")
    ok = ok and cond
ys = yolo_reqs()
# 第 120 轮修正陈旧断言:第 119 轮 Yolo 改「延迟强制」后,Yolo **首轮本就不带** forced
# tool_choice(探索轮 auto)→ mock 的 --reject-tool-choice 无从拒绝 → Yolo 恒 1 次请求。
# 「forced 被拒 → resilient 降级 auto 重试」这条链路现在只有 **Quality-Check** 会走到
# (QC 每轮强制指名 submit_quality_report)。原断言盯 Yolo 是自第 119 轮起的存量失败。
chk(len(ys) == 1, f"Yolo 请求恰好 1 次(延迟强制:首轮无 forced,mock 不拒绝,实际 {len(ys)})")
if ys:
    chk("tool_choice" not in ys[0] or (ys[0].get("tool_choice") or {}).get("type") != "tool",
        f"Yolo 探索轮不携带 forced tool_choice(实际 {ys[0].get('tool_choice')})")
qcs = [b for b in all_bodies() if "LsmAgentEmergentWork-Quality-Check" in system_of(b)]
chk(len(qcs) == 2, f"Quality 请求恰好 2 次(forced 被拒 + 降级 auto 重试,实际 {len(qcs)})")
if len(qcs) == 2:
    chk((qcs[0].get("tool_choice") or {}).get("type") == "tool", "QC 第 1 次携带 forced tool_choice(被拒)")
    chk("tool_choice" not in qcs[1], f"QC 第 2 次降级后不携带 tool_choice(实际 {qcs[1].get('tool_choice')})")
sys.exit(0 if ok else 1)
PYEOF
check $? "4j-2 wire 断言:第一次 forced 被拒 → 第二次降级无 tool_choice"
kill $FT2_MOCK_PID 2>/dev/null; wait $FT2_MOCK_PID 2>/dev/null; sleep 0.2
run "$LAEW" provider delete "$ID_FR" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$FT2_MOCK_LOG"
