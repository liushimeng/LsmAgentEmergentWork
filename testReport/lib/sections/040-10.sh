# --- 10. 取消传播端到端(SIGINT 优雅中断,本轮 feat)---
# 由 testReport/run_e2e.sh 自动 source;小节 id=10 —— 改这里不要动入口脚本。
# 验证:-p 任务执行中收到 SIGINT(kill -INT)后,全链路(Yolo/LLM 调用/编排)
# 即时中断并优雅退出,而不是陪 mock 的 4s 延迟跑完全部角色调用。
# 设计见 tmpPlan/2026-09-08_07-取消传播与优雅中断方案.md §3.7。
section "10. 取消传播(SIGINT 优雅中断)"
CANCEL_MOCK_LOG="testReport/mock_requests-cancel-$RUN_ID.jsonl"
CANCEL_MOCK_PORT=18904
python3 scripts/mock_llm_server.py $CANCEL_MOCK_PORT "$CANCEL_MOCK_LOG" --delay-ms 4000 &>/dev/null &
CANCEL_MOCK_PID=$!; wait_port $CANCEL_MOCK_PORT $CANCEL_MOCK_PID || echo "  [WARN] mock 未就绪($CANCEL_MOCK_PORT)" | tee -a "$REPORT"
run "$LAEW" provider add --protocol anthropic --provider-name canceltest --model-name claude-cancel \
  --end-point "http://127.0.0.1:$CANCEL_MOCK_PORT" --api-key sk-cancel >/dev/null 2>&1
ID_CAN=$(run "$LAEW" provider list 2>/dev/null | grep canceltest | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_CAN" >/dev/null 2>&1

# 后台启动 -p 长任务,2s 后定向发 SIGINT
CANCEL_OUT=/tmp/laew-e2e-cancel-out.txt; : > "$CANCEL_OUT"
CANCEL_START=$SECONDS
"$LAEW" -p "长任务取消传播测试" >"$CANCEL_OUT" 2>&1 &
CANCEL_PID=$!
sleep 2
kill -INT $CANCEL_PID 2>/dev/null
CANCEL_DEADLINE=$((SECONDS + 15))
while kill -0 $CANCEL_PID 2>/dev/null && [ $SECONDS -lt $CANCEL_DEADLINE ]; do
  sleep 0.2
done
if kill -0 $CANCEL_PID 2>/dev/null; then
  check 1 "取消后进程及时退出(15s 内)"
  kill -9 $CANCEL_PID 2>/dev/null
else
  check 0 "取消后进程及时退出(15s 内)"
fi
wait $CANCEL_PID 2>/dev/null; CAN_RC=$?
CANCEL_ELAPSED=$((SECONDS - CANCEL_START))
{ echo "    --- cancel 输出摘录(rc=$CAN_RC elapsed=${CANCEL_ELAPSED}s) ---"; sed 's/^/    | /' "$CANCEL_OUT"; echo "    --- end ---"; } | tee -a "$REPORT"
grep -q "已取消" "$CANCEL_OUT"; check $? "输出含「已取消」(优雅中断而非硬杀)"
[ "$CAN_RC" -eq 130 ]; check $? "退出码 130(128+SIGINT 惯例,实际 $CAN_RC)"
[ "$CANCEL_ELAPSED" -lt 6 ]; check $? "总耗时 ${CANCEL_ELAPSED}s < 6s(未陪 4s mock 延迟跑完全链路)"
# 取消路径不应把任务跑完:mock 的请求日志在 --delay-ms 延迟之后才落盘,
# 文件不存在 = 请求在延迟窗口内被取消(最强证据);存在则请求数应极少(正常链路 ≥4 角色)
if [ -f "$CANCEL_MOCK_LOG" ]; then
  CAN_REQS=$(grep -c 'LsmAgentEmergentWork-' "$CANCEL_MOCK_LOG" || true)
  [ "$CAN_REQS" -le 2 ]; check $? "mock 仅收到 ${CAN_REQS} 次角色请求(取消前未跑完全链路)"
else
  check 0 "mock 未落任何请求日志(请求在 4s 延迟窗口内被取消,链路未跑完)"
fi
kill $CANCEL_MOCK_PID 2>/dev/null
run "$LAEW" provider delete "$ID_CAN" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$CANCEL_MOCK_LOG" "$CANCEL_OUT"
