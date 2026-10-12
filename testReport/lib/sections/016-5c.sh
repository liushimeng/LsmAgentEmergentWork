# --- 5c. Context 自动压缩端到端(Compact Agent,feat 本轮) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=5c —— 改这里不要动入口脚本。
# 设计见 docs/Context设置与自动压缩设计/01-设计与解决方案.md §4
# 手法:切到 context_max_size=100 的 provider(阈值 80 token),管道模式连发 6 轮长消息任务。
# 第 5 轮起主上下文 = [项目上下文(保护), u1..uN],尾部 4 条保护 → 早期消息进入压缩段
# (每条约 500 字符,越过 MIN_SEGMENT_TOKENS=100 阈值),Compact Agent 被真实调用。
section "5c. Context 自动压缩(Compact Agent 触发)"
run "$LAEW" provider add --protocol anthropic --provider-name compact-test --model-name claude-compact \
  --end-point http://127.0.0.1:$MOCK_PORT --api-key sk-compact --context-max-size 100 >/dev/null 2>&1
ID_CP=$(run "$LAEW" provider list 2>/dev/null | grep compact-test | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_CP" >/dev/null 2>&1
LONGMSG=$(python3 -c "print('上下文压缩填充内容' * 60)")
OUT=$( { for i in 1 2 3 4 5 6; do echo "第${i}轮压缩测试任务 $LONGMSG"; done; echo "/exit"; } | run "$LAEW" )
echo "$OUT" | grep -q "Context 已自动压缩"; check $? "多轮后触发自动压缩(输出含压缩提示)"
grep -q "LsmAgentEmergentWork-Compact" "$MOCK_LOG"; check $? "mock 日志含 Compact Agent 请求(真实调用)"
# 第 08 轮:User-Agent 头部也携带 Compact 角色名(抓包层面可辨识,不再与 SubAgent-Work 同形)
grep -q '"user-agent": "LsmAgentEmergentWork-Compact/' "$MOCK_LOG"; check $? "Compact 请求 User-Agent 携带自身角色名"
echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "压缩后任务链路仍贯通"
run "$LAEW" provider delete "$ID_CP" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
