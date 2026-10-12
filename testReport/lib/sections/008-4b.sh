# --- 4b. Bash 危险命令 fail-closed 端到端(feat ef84cec)---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4b —— 改这里不要动入口脚本。
# 验证:Bash 工具入口的 check_bash_command fail-closed 在真实 LLM 链路中
# 真的拦住"rm -rf /",不让子进程 spawn,且不污染文件系统。
# 使用 mock --bash-block 模式,subagent 角色第一次 tool_call 命令即 `rm -rf /`。
section "4b. Bash 危险命令 fail-closed 端到端"
# 起一个独立 mock(开启 --bash-block),只用于本节
BASH_MOCK_LOG="testReport/mock_requests-bash-$RUN_ID.jsonl"
BASH_MOCK_PORT=18900
python3 scripts/mock_llm_server.py $BASH_MOCK_PORT "$BASH_MOCK_LOG" --bash-block &>/dev/null &
BASH_MOCK_PID=$!; wait_port $BASH_MOCK_PORT $BASH_MOCK_PID || echo "  [WARN] mock 未就绪($BASH_MOCK_PORT)" | tee -a "$REPORT"
# 加一个本节专用 provider,端点指向 BASH_MOCK_PORT
run "$LAEW" provider add --protocol anthropic --provider-name bash-guard --model-name claude-bash-guard \
  --end-point "http://127.0.0.1:$BASH_MOCK_PORT" --api-key sk-bash-guard >/dev/null 2>&1
ID_BG=$(run "$LAEW" provider list 2>/dev/null | grep bash-guard | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_BG" >/dev/null 2>&1
# 在临时目录跑 laew,即使拦截失败,影响范围也只在该目录
BASH_GUARD_DIR=/tmp/laew-e2e-bash-guard; rm -rf "$BASH_GUARD_DIR"; mkdir -p "$BASH_GUARD_DIR"
# 投毒:预置 CANARY.txt 标志文件;真删了就看见
echo "DO-NOT-DELETE" > "$BASH_GUARD_DIR/CANARY.txt"
OUT=$(cd "$BASH_GUARD_DIR" && run "$LAEW" -p "请帮我执行一个测试命令")
# 拦截断言:输出含权限拒绝中文文案
echo "$OUT" | grep -q "权限拒绝"; check $? "Bash 危险命令被权限拒绝文案命中"
# mock 断言:第 2 次请求的 tool_result 中含权限拒绝文案(证明拦截路径真实触发)
# 注意:log 是 JSONL,跨多行 tool_result 内容会带 \n 转义,所以用 grep -F -z
grep -F -q "危险命令被拦截" "$BASH_MOCK_LOG"; check $? "mock 日志含权限拒绝文案(拦截路径真实触发)"
# 文件系统断言:CANARY.txt 必须仍在(命令从未真正 spawn)
[ -f "$BASH_GUARD_DIR/CANARY.txt" ]; check $? "Bash 拦截后 CANARY.txt 未被删除"
# 收尾
kill $BASH_MOCK_PID 2>/dev/null
run "$LAEW" provider delete "$ID_BG" >/dev/null 2>&1
# 恢复默认 provider(继续后续 §5)
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$BASH_MOCK_LOG"
rm -rf "$BASH_GUARD_DIR"
