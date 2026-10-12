# --- 4n. 自感知 SubAgent 动态启动端到端(第 114 轮,2026-09-22) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4n —— 改这里不要动入口脚本。
# 设计见 docs/自感知SubAgent动态启动/01-设计与解决方案.md §8.2。
# 验证链:① 工具面注册 SubAgent ② action="list" 返回自感知快照(身份/名册/额度)
#   ③ action="batch" 真实并行派发 2 个 explore 子 Agent ④ 子 Agent 请求可在抓包中
#   辨识(身份含 SubAgent-Explore)且**叶子**(system 无「可启动名册」)
#   ⑤ 子报告合并回填父上下文 ⑥ LAEW_SELF_SPAWN=off 时工具面零扩大(严格向后兼容)。
section "4n. 自感知 SubAgent 动态启动端到端(第 114 轮)"
SELF_ROUTER="testReport/router-selfspawn-$RUN_ID.json"
cat > "$SELF_ROUTER" <<'JSON'
{
  "rules": [
    {
      "keywords": ["自感知子代理"],
      "tools": [
        {"tool": "SubAgent", "args": {"action": "list"}},
        {"tool": "SubAgent", "args": {"action": "batch", "max_concurrency": 2, "tasks": [
          {"task": "统计工作目录下 Cargo.toml 的行数,输出数字与结论", "agent_type": "general-purpose", "name": "侦察-A"},
          {"task": "列出 docs 目录下一级条目名,输出清单与结论", "agent_type": "general-purpose", "name": "侦察-B"}
        ]}},
        {"tool": "Bash", "args": {"command": "echo SELFSPAWN_DONE"}}
      ],
      "yolo": {
        "task_level": "medium",
        "goal_summary": "自感知子代理:并行调研 A/B 模块并汇总",
        "purpose": "验证自感知动态子 Agent 能力",
        "intent": "laew_meta",
        "decomposition_plan": ["batch 并行启动 2 个 explore 子 Agent", "汇总两份报告"]
      },
      "mainwork": {
        "workflows": [
          {"id": "wf-1",
           "name": "自感知子代理:并行调研 A/B 模块并汇总",
           "original_prompt": "自感知子代理:并行调研 A/B 模块并汇总",
           "steps": ["调用 SubAgent(action=batch) 并行启动 2 个 explore 子 Agent", "汇总子 Agent 报告"],
           "acceptance": ["报告含 2 个子 Agent 结果"],
           "delegate_to": "subagent"}
        ]
      }
    }
  ]
}
JSON
SELF_MOCK_LOG="testReport/mock_requests-selfspawn-$RUN_ID.jsonl"
SELF_MOCK_OFF_LOG="testReport/mock_requests-selfspawn-off-$RUN_ID.jsonl"
SELF_MOCK_PORT=18901
SELF_MOCK_OFF_PORT=18902
python3 scripts/mock_llm_server.py $SELF_MOCK_PORT "$SELF_MOCK_LOG" --prompt-router-file "$SELF_ROUTER" &>/dev/null &
SELF_MOCK_PID=$!
python3 scripts/mock_llm_server.py $SELF_MOCK_OFF_PORT "$SELF_MOCK_OFF_LOG" --prompt-router-file "$SELF_ROUTER" &>/dev/null &
SELF_MOCK_OFF_PID=$!
wait_port $SELF_MOCK_PORT $SELF_MOCK_PID wait_port $SELF_MOCK_OFF_PORT $SELF_MOCK_OFF_PID || echo "  [WARN] mock 未就绪" | tee -a "$REPORT"
run "$LAEW" provider add --protocol anthropic --provider-name selfspawn-on --model-name m-self-on \
  --end-point "http://127.0.0.1:$SELF_MOCK_PORT" --api-key sk-self-on >/dev/null 2>&1
run "$LAEW" provider add --protocol anthropic --provider-name selfspawn-off --model-name m-self-off \
  --end-point "http://127.0.0.1:$SELF_MOCK_OFF_PORT" --api-key sk-self-off >/dev/null 2>&1
ID_SS_ON=$(run "$LAEW" provider list 2>/dev/null | grep selfspawn-on | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
ID_SS_OFF=$(run "$LAEW" provider list 2>/dev/null | grep selfspawn-off | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_SS_ON" >/dev/null 2>&1
OUT=$(run "$LAEW" -p "自感知子代理:并行调研 A/B 模块并汇总")
echo "$OUT" | grep -qE "任务收口|outcome|用量"; check $? "4n-1 动态启动任务正常收口"
python3 - "$SELF_MOCK_LOG" <<'PYEOF' 2>&1 | tee -a "$REPORT"
import json, sys
path = sys.argv[1]
reqs = [json.loads(l) for l in open(path, encoding="utf-8") if l.strip()]
ok = True
def chk(cond, name):
    global ok
    print(f"  [{'PASS' if cond else 'FAIL'}] {name}")
    ok = ok and cond

def systems(r):
    sysf = r["body"].get("system")
    if isinstance(sysf, str):
        return sysf
    if isinstance(sysf, list):
        return "\n".join(p.get("text", "") for p in sysf if isinstance(p, dict))
    return ""

def tool_results(r):
    out = []
    for m in r["body"].get("messages", []) or []:
        c = m.get("content")
        if isinstance(c, list):
            for p in c:
                if isinstance(p, dict) and p.get("type") == "tool_result":
                    inner = p.get("content")
                    if isinstance(inner, list):
                        inner = "".join(x.get("text", "") for x in inner if isinstance(x, dict))
                    out.append(inner if isinstance(inner, str) else json.dumps(inner, ensure_ascii=False))
    return out

def tool_names(r):
    return [t.get("name") for t in (r["body"].get("tools") or [])]

delegating = [r for r in reqs if "LsmAgentEmergentWork-SubAgent-Work" in systems(r)]
chk(bool(delegating), "4n-2 抓包含 SubAgent-Work 角色请求")
chk(bool(delegating) and all("SubAgent" in tool_names(r) for r in delegating),
    "4n-3 SubAgent-Work 的 tools 含 SubAgent(委派工具已注册)")

snap = [t for r in reqs for t in tool_results(r) if '"can_spawn"' in t]
chk(bool(snap), "4n-4 action=list 返回自感知快照(含 can_spawn)")
chk(bool(snap) and '"roster"' in snap[0] and '"remaining"' in snap[0],
    "4n-5 快照含名册与剩余额度")

batch = [t for r in reqs for t in tool_results(r) if '"children"' in t]
chk(bool(batch), "4n-6 batch 返回逐子报告(children)")
chk(bool(batch) and '"count":2' in batch[0].replace(" ", "") and '"ok":2' in batch[0].replace(" ", ""),
    "4n-7 batch 成功 2 个子 Agent(ok=2)")

children = [r for r in reqs if "LsmAgentEmergentWork-SubAgent-General" in systems(r)]
names = set()
for r in children:
    for line in systems(r).splitlines():
        if "LsmAgentEmergentWork-SubAgent-General-" in line:
            names.add(line.strip())
chk(len(children) >= 2, f"4n-8 子 Agent 发起独立 LLM 请求(实际 {len(children)} 条)")
chk(len(names) >= 2, f"4n-9 两个子 Agent 身份可辨识(实际 {len(names)} 个)")
chk(all("你可启动的子 Agent 类型" not in systems(r) for r in children),
    "4n-10 子 Agent 为叶子(无自感知名册)")
chk(bool(children) and all("不能再启动子 Agent" in systems(r) for r in children),
    "4n-11 子 Agent 提示词含叶子语义")

merged = [t for r in reqs for t in tool_results(r) if "### [general-purpose]" in t]
chk(bool(merged), "4n-12 batch 合并文本回填父上下文(### [general-purpose])")
chk(bool(merged) and merged[0].count("### [general-purpose]") >= 2,
    "4n-13 合并文本含 2 个子报告")
sys.exit(0 if ok else 1)
PYEOF
check $? "4n-2..13 自感知动态启动断言组"

# ⑥ 关闭开关:工具面零扩大(向后兼容)
run "$LAEW" provider use "$ID_SS_OFF" >/dev/null 2>&1
OUT=$(LAEW_SELF_SPAWN=off run "$LAEW" -p "自感知子代理:并行调研 A/B 模块并汇总")
python3 - "$SELF_MOCK_OFF_LOG" <<'PYEOF' 2>&1 | tee -a "$REPORT"
import json, sys
path = sys.argv[1]
reqs = [json.loads(l) for l in open(path, encoding="utf-8") if l.strip()]
ok = True
def chk(cond, name):
    global ok
    print(f"  [{'PASS' if cond else 'FAIL'}] {name}")
    ok = ok and cond

def tool_names(r):
    return [t.get("name") for t in (r["body"].get("tools") or [])]

def systems(r):
    sysf = r["body"].get("system")
    if isinstance(sysf, str):
        return sysf
    if isinstance(sysf, list):
        return "\n".join(p.get("text", "") for p in sysf if isinstance(p, dict))
    return ""

chk(bool(reqs), "4n-14 off 模式仍正常发出请求")
chk(all("SubAgent" not in tool_names(r) for r in reqs),
    "4n-15 LAEW_SELF_SPAWN=off:tools 不含 SubAgent(工具面零扩大)")
chk(all("你可启动的子 Agent 类型" not in systems(r) for r in reqs),
    "4n-16 off 模式:提示词不含自感知名册")
sys.exit(0 if ok else 1)
PYEOF
check $? "4n-14..16 关闭开关向后兼容"
kill $SELF_MOCK_PID $SELF_MOCK_OFF_PID 2>/dev/null
rm -f "$SELF_ROUTER"
# 复原共享状态:本节切换过当前 provider 并杀掉了自带 mock —— 后续 §5d/§6 依赖
# 主 mock(mockA,$MOCK_LOG),必须切回;本节新增的两条接入记录一并清理。
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
run "$LAEW" provider delete "$ID_SS_ON" >/dev/null 2>&1
run "$LAEW" provider delete "$ID_SS_OFF" >/dev/null 2>&1
