# --- 4p-1 / 4p-2:守卫开启(默认) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4p-1 —— 改这里不要动入口脚本。
run "$LAEW" provider use "$ID_REACT_ON" >/dev/null 2>&1
OUT=$(run "$LAEW" -p "LAEW无进展止损验证:反复读取 Cargo.toml 直到我让你停")
echo "$OUT" | grep -qE "任务收口|outcome|用量"; check $? "4p-1 无进展止损后任务仍正常收口(不崩溃)"
python3 - "$REACT_MOCK_LOG" <<'PYEOF' 2>&1 | tee -a "$REPORT"
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

def all_text(body):
    """请求体全文(system + messages),用于断言 user 消息里的宽限轮强提醒。"""
    return system_of(body) + "\n" + json.dumps(body.get("messages", []), ensure_ascii=False)

sub = [r["body"] for r in reqs if "LsmAgentEmergentWork-SubAgent-Work" in system_of(r["body"])]
# 软提醒锚点必须取**运行时 hint 独有**的整句:提示词里也提到「无进展警告」四个字
# (告知模型止损规则),用它做锚点会假阳性。
NUDGE = "重复同一动作不会产生新信息"
GRACE = "无进展止损宽限轮"
# 4p-1:每次单元尝试的轮次结构固定为 4 轮 ——
#   轮 1 Read(repeats=1 有进展) / 轮 2 Read(repeats=2 → 软提醒入队) /
#   轮 3 system 带软提醒,Read(repeats=3 → 宽限轮强提醒推 user 消息) /
#   轮 4 messages 带强提醒,Read(repeats=4 → 止损终止)。
# 止损后 QC 判 Fail → 单元级局部重试 + 档位级重试会**重放整条链**,所以总数是
# 4 × 尝试次数(实测 9 次尝试 = 36 次请求),不是 4。断言按「每 4 个一组」校验:
# 若守卫失效,每次尝试会跑满 32 轮 → 组数与软提醒数对不上,且总数暴涨。
groups, nudges = len(sub) // 4, sum(1 for b in sub if NUDGE in all_text(b))
chk(len(sub) >= 4, f"SubAgent 至少跑完一次尝试(实际 {len(sub)} 次请求)")
chk(len(sub) % 4 == 0, f"每次尝试恰好 4 轮止损(总请求数应为 4 的倍数,实际 {len(sub)})")
chk(nudges == groups and groups > 0,
    f"每次尝试都触发过 1 次软提醒(尝试 {groups} 组 / 软提醒 {nudges} 次;守卫失效则为 0)")
chk(len(sub) <= 4 * 12, f"总请求数未暴涨(守卫失效会跑满 32 轮/尝试,实际 {len(sub)})")
if len(sub) >= 4:
    # 首次尝试的逐轮结构(提醒分级送达 + 配对安全)
    # 第 140 轮:runtime hints 改走消息流末尾尾注(meta.runtime_tail)保 prompt cache
    # 前缀稳定,断言从 system 改查全量 wire 文本(system + messages)。
    chk(NUDGE not in all_text(sub[0]), "轮 1 不该有无进展软提醒(首次出现该动作)")
    chk(NUDGE not in all_text(sub[1]), "轮 2 刚判定重复,提醒在**下一轮**才注入")
    chk(NUDGE in all_text(sub[2]), "轮 3 请求含无进展软提醒(经 RUNTIME_HINTS 尾注送达)")
    chk("LAEW:RUNTIME_HINTS" in all_text(sub[2]),
        "软提醒包裹在 <<<LAEW:RUNTIME_HINTS>>> 标记内(与用户提示词严格隔离)")
    # 宽限轮强提醒必须作为 **user 消息**出现在轮 4(且只能在 tool_result 全部回填之后,
    # 否则会破坏 assistant tool_use ↔ tool_result 配对被 Provider 400)
    chk(GRACE in all_text(sub[3]), "轮 4 含宽限轮强提醒(user 消息,保证送达 LLM)")
# 止损结论必须流到下游(QC / Yolo 回流)才形成闭环,而不是只在循环内部静默终止。
# 锚点用 early_terminate_reason 的完整值 `doom_loop_no_progress`:弱信号 `doom_loop:Nx`
# 在守卫关闭时**按设计仍会统计**(可观测性不丢),用它做锚点分不清「止损了」和「只是数了数」。
chk(any("doom_loop_no_progress" in json.dumps(r["body"], ensure_ascii=False) for r in reqs),
    "止损原因 doom_loop_no_progress 进入下游请求(QC 证据链 / 失败回流可见)")
# 4p-2:ReAct 提示词真的送达 wire
if sub:
    s0 = system_of(sub[0])
    chk("执行循环(ReAct 模式)" in s0, "SubAgent system 含「执行循环(ReAct 模式)」节")
    chk("Thought(推理)" in s0 and "Observation(观察)" in s0,
        "SubAgent system 含 Thought / Observation 三段规范")
    chk("连续工作模式" in s0, "SubAgent system 含「连续工作模式」节")
    chk("保序串行" in s0, "SubAgent system 说明有副作用调用保序串行(与运行时一致)")
else:
    chk(False, "未见 SubAgent-Work 请求")
sys.exit(0 if ok else 1)
PYEOF
check $? "4p-1/4p-2 wire 断言:无进展止损 4 轮收口 + 提醒分级送达 + ReAct 提示词在 wire 上"
