# --- 4o. 自定义子 Agent 类型 + 运行记录/续跑端到端(第 115 轮,2026-09-22) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4o —— 改这里不要动入口脚本。
# 设计见 docs/自感知SubAgent自定义类型与运行持久化/01-设计与解决方案.md §8.2。
# 验证链:① `.laew/agents/*.md` 定义即生效(action=list 名册含自定义类型)
#   ② agent_type 用自定义 id 真实派发(报告 agent_type=自定义 id)
#   ③ 子 Agent 系统提示词含定义文件正文 + 自定义身份(抓包可见)
#   ④ readonly 声明收窄工具面(子 Agent 请求不含 Write)
#   ⑤ action=history 读到落库记录(run_id 可在 tool_result 里拿到)
#   ⑥ action=resume 用 $LAST_RUN_ID 占位符续跑 → 报告带 resumed_from 血缘,
#      且子 Agent 的 user prompt 含「前序结论」种子上下文
# 注:laew 在**工作目录**下发现定义文件,故本节在临时工作目录里启动 laew
#   (根目录/DB 仍由 binary 所在目录决定,不污染仓库)。
section "4o. 自定义子 Agent 类型与运行记录(第 115 轮)"
CUSTOM_WS="/tmp/laew-e2e-root/ws-customagent"
rm -rf "$CUSTOM_WS"; mkdir -p "$CUSTOM_WS/.laew/agents"
cat > "$CUSTOM_WS/.laew/agents/fe-reviewer.md" <<'MD'
---
label: 前端审查
description: 前端专项审查:可访问性 / 状态管理 / 渲染性能
extends: code-reviewer
tools: Read, Glob, Grep
readonly: true
---
CUSTOM_BODY_MARKER:逐条给出「文件:行 -> 问题 -> 建议」,按 P0/P1/P2 分级;不修改任何文件。
MD
CUSTOM_ROUTER="testReport/router-customagent-$RUN_ID.json"
cat > "$CUSTOM_ROUTER" <<'JSON'
{
  "rules": [
    {
      "keywords": ["自定义子代理"],
      "tools": [
        {"tool": "SubAgent", "args": {"action": "list"}},
        {"tool": "SubAgent", "args": {"action": "launch", "agent_type": "fe-reviewer", "name": "前端审查",
          "task": "审查 src/tui 的输入处理,输出 P0-P2 问题清单"}},
        {"tool": "SubAgent", "args": {"action": "history", "limit": 5}},
        {"tool": "SubAgent", "args": {"action": "resume", "run_id": "$LAST_RUN_ID",
          "task": "把 P0 问题展开成修复补丁计划"}},
        {"tool": "Bash", "args": {"command": "echo CUSTOMAGENT_DONE"}}
      ],
      "yolo": {
        "task_level": "medium",
        "goal_summary": "自定义子代理:用 fe-reviewer 审查 TUI",
        "purpose": "验证自定义子 Agent 类型与运行记录",
        "intent": "laew_meta",
        "decomposition_plan": ["用自定义类型 fe-reviewer 审查", "汇总问题清单"]
      },
      "mainwork": {
        "workflows": [
          {"id": "wf-1",
           "name": "自定义子代理:用 fe-reviewer 审查 TUI",
           "original_prompt": "自定义子代理:用 fe-reviewer 审查 TUI",
           "steps": ["调用 SubAgent(action=launch, agent_type=fe-reviewer)", "续跑 P0 修复计划"],
           "acceptance": ["产出 P0-P2 问题清单"],
           "delegate_to": "subagent"}
        ]
      }
    }
  ]
}
JSON
CUSTOM_MOCK_LOG="testReport/mock_requests-customagent-$RUN_ID.jsonl"
CUSTOM_MOCK_PORT=18903
python3 scripts/mock_llm_server.py $CUSTOM_MOCK_PORT "$CUSTOM_MOCK_LOG" --prompt-router-file "$CUSTOM_ROUTER" &>/dev/null &
CUSTOM_MOCK_PID=$!
wait_port $CUSTOM_MOCK_PORT $CUSTOM_MOCK_PID || echo "  [WARN] mock 未就绪" | tee -a "$REPORT"
run "$LAEW" provider add --protocol anthropic --provider-name customagent --model-name m-custom \
  --end-point "http://127.0.0.1:$CUSTOM_MOCK_PORT" --api-key sk-custom >/dev/null 2>&1
ID_CUSTOM=$(run "$LAEW" provider list 2>/dev/null | grep customagent | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_CUSTOM" >/dev/null 2>&1
OUT=$(cd "$CUSTOM_WS" && run "$LAEW" -p "自定义子代理:用 fe-reviewer 子 Agent 审查 TUI 输入处理,并汇总")
echo "$OUT" | grep -qE "任务收口|outcome|用量"; check $? "4o-1 自定义类型任务正常收口"
python3 - "$CUSTOM_MOCK_LOG" <<'PYEOF' 2>&1 | tee -a "$REPORT"
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

def tool_names(r):
    return [t.get("name") for t in (r["body"].get("tools") or [])]

def tool_results(r):
    out = []
    for m in r["body"].get("messages", []) or []:
        if m.get("role") == "tool":
            c = m.get("content")
            out.append(c if isinstance(c, str) else json.dumps(c, ensure_ascii=False))
            continue
        c = m.get("content")
        if isinstance(c, list):
            for p in c:
                if isinstance(p, dict) and p.get("type") == "tool_result":
                    inner = p.get("content")
                    if isinstance(inner, list):
                        inner = "".join(x.get("text", "") for x in inner if isinstance(x, dict))
                    out.append(inner if isinstance(inner, str) else json.dumps(inner, ensure_ascii=False))
    return out

def user_texts(r):
    out = []
    for m in r["body"].get("messages", []) or []:
        if m.get("role") != "user":
            continue
        c = m.get("content")
        if isinstance(c, str):
            out.append(c)
        elif isinstance(c, list):
            for p in c:
                if isinstance(p, dict) and p.get("type") == "text":
                    out.append(p.get("text", ""))
    return "\n".join(out)

results = [t for r in reqs for t in tool_results(r)]

# ① 名册:自定义类型可见
snap = [t for t in results if '"can_spawn"' in t]
chk(bool(snap), "4o-2 action=list 返回自感知快照")
chk(bool(snap) and '"fe-reviewer"' in snap[0] and '"custom": true' in snap[0].replace('"custom":true', '"custom": true'),
    "4o-3 快照名册含自定义类型 id")
chk(bool(snap) and "fe-reviewer.md" in snap[0], "4o-4 名册含定义文件来源路径")

# ② 派发:报告 agent_type 为自定义 id
launch = [t for t in results if '"agent_type": "fe-reviewer"' in t or '"agent_type":"fe-reviewer"' in t]
chk(bool(launch), "4o-5 launch 报告 agent_type=fe-reviewer(自定义类型真实派发)")
chk(bool(launch) and '"origin"' in launch[0], "4o-6 报告含 origin 字段")

# ③ 子 Agent 身份与定义正文
children = [r for r in reqs if "SubAgent-FeReviewer" in systems(r)]
chk(bool(children), f"4o-7 抓到自定义子 Agent 请求(实际 {len(children)} 条)")
chk(bool(children) and all("CUSTOM_BODY_MARKER" in systems(r) for r in children),
    "4o-8 子 Agent 系统提示词含定义文件正文")
chk(bool(children) and all("自定义定义文件" in systems(r) for r in children),
    "4o-9 子 Agent 身份标注为自定义类型")
chk(bool(children) and all("不能再启动子 Agent" in systems(r) for r in children),
    "4o-10 自定义子 Agent 仍为叶子语义")

# ④ readonly 收窄:子 Agent 工具面不含 Write/Bash
chk(bool(children) and all("Write" not in tool_names(r) for r in children),
    "4o-11 readonly 自定义类型不得持有 Write")
chk(bool(children) and all("Bash" not in tool_names(r) for r in children),
    "4o-12 readonly 自定义类型不得持有 Bash")

# ⑤ history:落库记录可查
hist = [t for t in results if '"persist"' in t and '"runs"' in t]
chk(bool(hist), "4o-13 action=history 返回运行记录(persist/runs)")
chk(bool(hist) and '"persist": true' in hist[0].replace('"persist":true', '"persist": true'),
    "4o-14 history 报告持久化已开启")
chk(bool(hist) and '"fe-reviewer"' in hist[0], "4o-15 history 含自定义类型作业")

# ⑥ resume:血缘 + 前序种子上下文
resumed = [t for t in results if '"origin": "resume"' in t or '"origin":"resume"' in t]
chk(bool(resumed), "4o-16 action=resume 成功续跑(报告 origin=resume)")
chk(bool(resumed) and '"resumed_from"' in resumed[0], "4o-17 续跑报告带 resumed_from 血缘")
resume_children = [r for r in children if "【前序结论】" in user_texts(r)]
chk(bool(resume_children), "4o-18 续跑子 Agent 的 user prompt 含前序结论种子上下文")
chk(bool(resume_children) and any("【前序任务】" in user_texts(r) for r in resume_children),
    "4o-19 续跑种子含前序任务")
sys.exit(0 if ok else 1)
PYEOF
check $? "4o-2..19 自定义类型与运行记录断言组"

# ⑦ 关闭持久化:工具面与提示词不变,但不再落库(直接查 e2e 独立库;用 python3
#    而非 sqlite3 CLI —— e2e 已硬依赖 python3,CLI 在部分环境缺失)
count_subagent_rows() {
  python3 -c 'import sqlite3
try:
    con = sqlite3.connect("/tmp/laew-e2e-root/LsmAgentEmergentWork.db")
    print(con.execute("SELECT COUNT(*) FROM subagent_run").fetchone()[0])
except Exception:
    print("no-db")'
}
CUSTOM_DB_BEFORE=$(count_subagent_rows)
if [ "$CUSTOM_DB_BEFORE" != "no-db" ] && [ "${CUSTOM_DB_BEFORE:-0}" -ge 1 ]; then
  check 0 "4o-20 subagent_run 表已落库($CUSTOM_DB_BEFORE 行)"
else
  check 1 "4o-20 subagent_run 表已落库(实际 $CUSTOM_DB_BEFORE)"
fi
OUT=$(cd "$CUSTOM_WS" && LAEW_SUBAGENT_PERSIST=off run "$LAEW" -p "自定义子代理:用 fe-reviewer 子 Agent 审查 TUI 输入处理,并汇总")
CUSTOM_DB_AFTER=$(count_subagent_rows)
if [ "$CUSTOM_DB_BEFORE" = "$CUSTOM_DB_AFTER" ]; then
  check 0 "4o-21 LAEW_SUBAGENT_PERSIST=off 零新增落库(仍 $CUSTOM_DB_AFTER 行)"
else
  check 1 "4o-21 LAEW_SUBAGENT_PERSIST=off 零新增落库(前 $CUSTOM_DB_BEFORE → 后 $CUSTOM_DB_AFTER)"
fi
kill $CUSTOM_MOCK_PID 2>/dev/null
rm -f "$CUSTOM_ROUTER"
run "$LAEW" provider delete "$ID_CUSTOM" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
