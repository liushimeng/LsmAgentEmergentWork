# --- 6. 协议请求格式校验(抓包日志) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=6 —— 改这里不要动入口脚本。
section "6. 请求格式校验(mock_requests)"
python3 - "$MOCK_LOG" <<'PYEOF' 2>&1 | tee -a "$REPORT"
import json, sys
path = sys.argv[1]
reqs = [json.loads(l) for l in open(path, encoding="utf-8")]
anth = [r for r in reqs if "v1/messages" in r["path"]]
oai = [r for r in reqs if "chat/completions" in r["path"]]
ok = True
def chk(cond, name):
    global ok
    print(f"  [{'PASS' if cond else 'FAIL'}] {name}")
    ok = ok and cond
chk(len(anth) >= 2 and len(oai) >= 2, f"两种协议均有 ≥2 次请求 (anthropic={len(anth)}, openai={len(oai)})")
if anth:
    b = anth[0]["body"]
    # L1047(第 10 轮)起 system 可为 content block 数组(携带 cache_control 断点),
    # 兼容断言两种形态:字符串 或 [{type:text, text:...}] 数组
    _sys = b.get("system")
    _sys_ok = isinstance(_sys, str) or (
        isinstance(_sys, list)
        and all(isinstance(x, dict) and x.get("type") == "text" and "text" in x for x in _sys)
    )
    chk(_sys_ok, f"anthropic: system 为顶层字符串或 text 块数组 (实际类型: {type(_sys).__name__})")
    # D4 工作区感知(2026-09-13 第 01 轮):运行时环境 brief 拼在 system 末尾,
    # 8 角色全生效(执行层 SubAgent 也能看到工程类型 / 工具链 / git 状态)。
    _sys_text = _sys if isinstance(_sys, str) else "".join(
        x.get("text", "") for x in _sys if isinstance(x, dict)
    ) if isinstance(_sys, list) else ""
    chk("LAEW:WORKSPACE" in _sys_text, "anthropic: system 含运行时工作区 brief(D4 全角色注入)")
    chk("cargo" in _sys_text, "anthropic: 工作区 brief 含工程工具链建议(cargo)")
    # 双 Agent 架构:Yolo(入口层,信息收集型工具面)+ Work(执行层,全套工具)
    # 2026-09-22:Yolo 与 SubAgent-Work 均含 Bash;取首个含 Bash 的请求校验 wire 格式,
    # 取该请求的 tools 数组即可同时覆盖两边(均使用 input_schema 规范)。
    work_req = next((r for r in anth if any(
        t.get("name") == "Bash" for t in r["body"].get("tools", [])
    )), anth[-1])
    b_tools = work_req["body"].get("tools", [])
    chk(any(t.get("name") == "Bash" and "input_schema" in t for t in b_tools), "anthropic: tools 含 Bash 且带 input_schema")
    # 项目上下文注入断言(本节 -p 在仓库根目录运行,工作目录=仓库根,命中 CLAUDE.md)
    def _texts(m):
        c = m.get("content")
        if isinstance(c, list):
            return [x.get("text", "") for x in c if isinstance(x, dict) and x.get("type") == "text"]
        return [c] if isinstance(c, str) else []
    msgs = b.get("messages", [])
    ctx_msgs = [m for m in msgs if any("LAEW:PROJECT_CONTEXT" in t for t in _texts(m))]
    chk(len(ctx_msgs) == 1, "anthropic: 首请求含且仅含 1 条项目上下文注入消息")
    if len(ctx_msgs) == 1:
        t0 = "\n".join(_texts(ctx_msgs[0]))
        chk("工作目录:" in t0 and "CLAUDE.md" in t0, "anthropic: 注入消息含工作目录与说明文件来源")
        # D4 工作区感知(2026-09-13 第 01 轮):说明文件正文之后追加工作区快照段
        chk("工作区快照" in t0, "anthropic: 注入消息含工作区快照段(D4)")
        chk("Git:" in t0, "anthropic: 工作区快照含 Git 状态行(仓库根为 git 仓库)")
    users = [m for m in msgs if m.get("role") == "user"]
    # 2026-09-11 第 34 轮:merge_adjacent_same_role 将 PROJECT_CONTEXT 与用户提示词
    # 合并为同一条 user 消息的两个 text block(兼容严格 Anthropic 网关的 400 拒绝)。
    # 用户提示词位于 content 数组末位,检查最后一条 text block 而非首条。
    last_user_blocks = _texts(users[-1]) if users else []
    last_user = last_user_blocks[-1].strip() if last_user_blocks else ""
    chk(last_user == "请帮我执行一个测试命令", "anthropic: 用户提示词原文独立成条(未与上下文混淆)")
if anth:
    # 角色化 mock 后请求序号随角色分流变化,改为扫描:执行循环必须把 tool_result 回填到后续请求
    chk(any(
        any(c.get("type") == "tool_result" for m in r["body"]["messages"] for c in m.get("content", []) if isinstance(c, dict))
        for r in anth), "anthropic: 执行循环请求含 tool_result 块")
if oai:
    b = oai[0]["body"]
    chk(b["messages"][0]["role"] == "system", "openai: system 转为首条 system 消息")
    chk(any(t.get("type") == "function" and "parameters" in t.get("function", {}) for t in b.get("tools", [])), "openai: tools[].function.parameters")
if oai:
    chk(any(m.get("role") == "tool" for r in oai for m in r["body"]["messages"]), "openai: 执行循环请求含 role=tool 消息")

# --- 请求头校验(User-Agent / Authorization / X-Session-Id) ---
def non_empty(v): return isinstance(v, str) and v.strip() != ""
if anth:
    h = anth[0]["headers"]
    chk(non_empty(h.get("user-agent")), f"anthropic: User-Agent 已携带 ({h.get('user-agent','')[:40]})")
    chk(h.get("authorization","").startswith("Bearer "), "anthropic: Authorization: Bearer <key>")
    chk(non_empty(h.get("x-session-id")), "anthropic: X-Session-Id 已携带")
    chk(non_empty(h.get("x-api-key")), "anthropic: x-api-key 保留")
    # metadata.user_id 解析
    meta = anth[0]["body"].get("metadata", {})
    uid_str = meta.get("user_id", "")
    try:
        uid = json.loads(uid_str)
        chk(non_empty(uid.get("device_id")), "anthropic: metadata.user_id.device_id")
        chk(non_empty(uid.get("session_id")), "anthropic: metadata.user_id.session_id")
        chk(uid.get("account_uuid") == "", "anthropic: metadata.user_id.account_uuid 为空")
        # 第 08 轮:请求体自身携带发起角色(agent 字段),抓包不必解析 system 也能辨识
        chk(non_empty(uid.get("agent")), f"anthropic: metadata.user_id.agent 携带发起角色 ({uid.get('agent','')})")
    except Exception as e:
        chk(False, f"anthropic: metadata.user_id 解析失败: {e}")
if oai:
    h = oai[0]["headers"]
    chk(non_empty(h.get("user-agent")), f"openai: User-Agent 已携带 ({h.get('user-agent','')[:40]})")
    chk(h.get("authorization","").startswith("Bearer "), "openai: Authorization: Bearer <key>")
    chk(non_empty(h.get("x-session-id")), "openai: X-Session-Id 已携带")

# --- Agent 身份逐请求注入(2026-09-09 第 08 轮,方案 tmpPlan/2026-09-09_08) ---
# 按 User-Agent 汇总角色集合:主 mock 日志覆盖 §4(OpenAI)/§5(Anthropic)/
# §5b/§5c(Compact)/§5d(Debug)的完整链路,六个角色都必须在头部层面可辨识。
# 修复前:所有请求 UA 均为 LsmAgentEmergentWork-SubAgent-Work(构造期烧死)。
import re as _re
def agent_of(ua):
    m = _re.match(r"^(LsmAgentEmergentWork-[A-Za-z-]+)/", ua or "")
    return m.group(1) if m else None
all_reqs = anth + oai
chk(all((r["headers"].get("user-agent") or "").startswith("LsmAgentEmergentWork-")
        for r in all_reqs), "全部请求 User-Agent 均为 LsmAgentEmergentWork-* 形态")
roles = sorted(set(filter(None, (agent_of(r["headers"].get("user-agent", "")) for r in all_reqs))))
required = {
    "LsmAgentEmergentWork-Yolo",
    "LsmAgentEmergentWork-SubAgent-Work",
    "LsmAgentEmergentWork-Quality-Check",
    "LsmAgentEmergentWork-SessionContext",
    "LsmAgentEmergentWork-Compact",
    "LsmAgentEmergentWork-Debug",
}
missing = required - set(roles)
chk(not missing, f"User-Agent 覆盖 6 角色(实际: {roles}; 缺: {sorted(missing)})")
sys.exit(0 if ok else 1)
PYEOF
check $? "协议 wire 格式校验"
