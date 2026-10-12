# --- 4g. Write 沙箱白名单端到端(feat 2026-09-09_08,src/agent/sandbox_hook/) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4g —— 改这里不要动入口脚本。
# 验证:Write/Edit 只能在「工作目录 + 系统临时目录」写入;Read 任意读、Bash 不受限。
# 负例(--write-outside):mock 让 subagent 第一次工具调用 Write 到用户 Home 下
#   canary 文件(白名单外且可写,若拦截失效会真实落盘)→ 必须被 SandboxViolation 拦截。
# 正例(--write-inside):同一链路写工作目录内相对路径 → 必须放行落盘(防误伤)。
# 两个场景都在临时目录跑 laew,保证 工作目录 ≠ 根目录 ≠ Home。
section "4g. Write 沙箱白名单端到端(--write-outside / --write-inside)"
SBX_CANARY="$HOME/laew-sandbox-e2e-canary.txt"
rm -f "$SBX_CANARY"

# 4g-1 负例:白名单外写入被拦截
SBX_OUT_LOG="testReport/mock_requests-sbx-out-$RUN_ID.jsonl"
SBX_OUT_PORT=18905
python3 scripts/mock_llm_server.py $SBX_OUT_PORT "$SBX_OUT_LOG" --write-outside &>/dev/null &
SBX_OUT_PID=$!; wait_port $SBX_OUT_PORT $SBX_OUT_PID || echo "  [WARN] mock 未就绪($SBX_OUT_PORT)" | tee -a "$REPORT"
run "$LAEW" provider add --protocol anthropic --provider-name sbx-out --model-name claude-sbx-out \
  --end-point "http://127.0.0.1:$SBX_OUT_PORT" --api-key sk-sbx-out >/dev/null 2>&1
ID_SBXO=$(run "$LAEW" provider list 2>/dev/null | grep sbx-out | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_SBXO" >/dev/null 2>&1
SBX_OUT_DIR=/tmp/laew-e2e-sbx-out; rm -rf "$SBX_OUT_DIR"; mkdir -p "$SBX_OUT_DIR"
OUT=$(cd "$SBX_OUT_DIR" && run "$LAEW" -p "请帮我执行一个测试命令")
echo "$OUT" | grep -q "沙箱拦截"; check $? "越界 Write 命中沙箱拦截文案"
[ ! -f "$SBX_CANARY" ]; check $? "Home 下 canary 文件未被写入(拦截真实生效)"
grep -F -q "沙箱拦截" "$SBX_OUT_LOG"; check $? "mock 日志 tool_result 含拦截文案(wire 级验证)"
kill $SBX_OUT_PID 2>/dev/null
run "$LAEW" provider delete "$ID_SBXO" >/dev/null 2>&1
rm -f "$SBX_OUT_LOG"; rm -rf "$SBX_OUT_DIR"

# 4g-2 正例:工作目录内写入放行
SBX_IN_LOG="testReport/mock_requests-sbx-in-$RUN_ID.jsonl"
SBX_IN_PORT=18906
python3 scripts/mock_llm_server.py $SBX_IN_PORT "$SBX_IN_LOG" --write-inside &>/dev/null &
SBX_IN_PID=$!; wait_port $SBX_IN_PORT $SBX_IN_PID || echo "  [WARN] mock 未就绪($SBX_IN_PORT)" | tee -a "$REPORT"
run "$LAEW" provider add --protocol anthropic --provider-name sbx-in --model-name claude-sbx-in \
  --end-point "http://127.0.0.1:$SBX_IN_PORT" --api-key sk-sbx-in >/dev/null 2>&1
ID_SBXI=$(run "$LAEW" provider list 2>/dev/null | grep sbx-in | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_SBXI" >/dev/null 2>&1
SBX_IN_DIR=/tmp/laew-e2e-sbx-in; rm -rf "$SBX_IN_DIR"; mkdir -p "$SBX_IN_DIR"
OUT=$(cd "$SBX_IN_DIR" && run "$LAEW" -p "请帮我执行一个测试命令")
echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "白名单内写入后链路仍收口最终文本"
[ -f "$SBX_IN_DIR/sandbox-ok.txt" ]; check $? "工作目录内 sandbox-ok.txt 成功落盘"
grep -q "laew sandbox ok" "$SBX_IN_DIR/sandbox-ok.txt"; check $? "落盘文件内容正确"
grep -F -q "[Write] 写入" "$SBX_IN_LOG"; check $? "mock 日志 tool_result 含写入成功回执"
kill $SBX_IN_PID 2>/dev/null
run "$LAEW" provider delete "$ID_SBXI" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$SBX_IN_LOG"; rm -rf "$SBX_IN_DIR"; rm -f "$SBX_CANARY"
