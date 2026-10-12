# --- 4p-3:LAEW_REACT_GUARD=off 回退 ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4p-3 —— 改这里不要动入口脚本。
run "$LAEW" provider use "$ID_REACT_OFF" >/dev/null 2>&1
# 子壳内 export:`VAR=x run …`(函数前缀赋值)在 POSIX 下行为未定义,不可靠
OUT=$(export LAEW_REACT_GUARD=off; run "$LAEW" -p "LAEW无进展止损验证:反复读取 Cargo.toml 直到我让你停")
python3 - "$REACT_OFF_LOG" <<'PYEOF' 2>&1 | tee -a "$REPORT"
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
sub = [r["body"] for r in reqs if "LsmAgentEmergentWork-SubAgent-Work" in system_of(r["body"])]
NUDGE = "重复同一动作不会产生新信息"
def all_text(body):
    """请求体全文(system + messages)——第 140 轮 hints 走消息尾注,查全量。"""
    return system_of(body) + "\n" + json.dumps(body.get("messages", []), ensure_ascii=False)
chk(len(sub) > 4, f"守卫关闭后不再 4 轮止损(实际 {len(sub)} 次,由 no_text_converge/max_iterations 兜底)")
chk(all(NUDGE not in all_text(b) for b in sub),
    "守卫关闭后请求不再出现无进展软提醒(LAEW_REACT_GUARD=off 真的生效)")
# 注:弱信号 `doom_loop:Nx` 仍会出现(守卫关闭只停「干预」,不停「统计」),
# 所以这里断言的是**没有真的止损**(early_terminate_reason 不含 doom_loop_no_progress)。
chk(not any("doom_loop_no_progress" in json.dumps(r["body"], ensure_ascii=False) for r in reqs),
    "守卫关闭后不再真的止损(下游无 doom_loop_no_progress 早终止结论)")
sys.exit(0 if ok else 1)
PYEOF
check $? "4p-3 wire 断言:LAEW_REACT_GUARD=off 可回退到第 119 轮行为"

kill $REACT_MOCK_PID 2>/dev/null; wait $REACT_MOCK_PID 2>/dev/null
kill $REACT_OFF_PID 2>/dev/null; wait $REACT_OFF_PID 2>/dev/null
sleep 0.2
rm -f "$REACT_ROUTER" "$REACT_MOCK_LOG" "$REACT_OFF_LOG"
run "$LAEW" provider delete "$ID_REACT_ON" >/dev/null 2>&1
run "$LAEW" provider delete "$ID_REACT_OFF" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
