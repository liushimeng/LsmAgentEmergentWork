# --- 7f. 会话持久化与跨进程恢复(第 96 轮,/sessions /resume --resume --sessions) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=7f —— 改这里不要动入口脚本。
# 知识库:专题-第八轮-Session持久化与崩溃恢复 §7 P0/P1;实现 tmpPlan/2026-09-19_02。
# 链路:进程 1 两轮任务自动落盘 → 进程 2 /sessions 列出 → CLI --sessions 列出 →
# 进程 3 --resume 恢复 + 第三轮 + /export 断言 3 轮 → 进程 4 /resume 序号 + 自动快照分支。
# 端口 18913:避开并行会话 mock 占用(见 tmux-test-sandbox-traps 经验)。
section "7f. 会话持久化与跨进程恢复(/sessions /resume --resume)"
rm -rf /tmp/laew-e2e-hist-root /tmp/laew-e2e-hist-work
mkdir -p /tmp/laew-e2e-hist-root /tmp/laew-e2e-hist-work
cp laew /tmp/laew-e2e-hist-root/laew
python3 scripts/mock_llm_server.py 18913 "$ROOT_DIR/testReport/mock_requests-7f-$TS.jsonl" &>/dev/null &
HIST_MOCK_PID=$!; wait_port 18913 $HIST_MOCK_PID || echo "  [WARN] mock 未就绪(18913)" | tee -a "$REPORT"
HISTBIN=/tmp/laew-e2e-hist-root/laew
run "$HISTBIN" provider add --protocol anthropic --provider-name histA --model-name claude-mock --end-point http://127.0.0.1:18913 --api-key sk-hist >/dev/null 2>&1
# 1) 进程 1:两轮任务 → 每轮收口自动整快照落盘
OUT=$(cd /tmp/laew-e2e-hist-work && printf "第一轮你好\n第二轮继续\n/exit\n" | run timeout 120 "$HISTBIN")
USAGE_COUNT=$(echo "$OUT" | grep -c "本次用量")
[ "$USAGE_COUNT" = "2" ]; check $? "7f-1 首进程两轮任务完成(本次用量 x2,实际 $USAGE_COUNT)"
# 2) 进程 2(TUI 管道模式):/sessions 列出持久化会话,标题=首轮输入预览,2 轮
OUT=$(cd /tmp/laew-e2e-hist-work && printf "/sessions\n/exit\n" | run timeout 60 "$HISTBIN")
echo "$OUT" | grep -q "历史会话"; check $? "7f-2 /sessions 列出跨进程历史会话"
echo "$OUT" | grep -q "第一轮你好"; check $? "7f-3 会话标题为首轮输入预览"
echo "$OUT" | grep -q " 2 轮"; check $? "7f-4 列表显示 2 轮计数"
# 3) CLI --sessions 快速列出(不进 TUI);顺带提取原会话完整 id 供第 5 步前缀恢复
OUT=$(cd /tmp/laew-e2e-hist-work && run timeout 60 "$HISTBIN" --sessions)
echo "$OUT" | grep -q "第一轮你好"; check $? "7f-5 --sessions CLI 列出持久化会话"
HIST_ID=$(echo "$OUT" | grep "第一轮你好" | awk '{print $NF}' | head -1)
# 4) 进程 3:--resume 启动恢复(最近一次)→ 第三轮 → /export 验证 3 轮(2 恢复 + 1 新增)
OUT=$(cd /tmp/laew-e2e-hist-work && printf "第三轮追问\n/export hist.md\n/exit\n" | run timeout 120 "$HISTBIN" --resume)
echo "$OUT" | grep -q "已恢复会话"; check $? "7f-6 --resume 启动恢复生效"
grep -q "第三轮追问" /tmp/laew-e2e-hist-work/hist.md 2>/dev/null; check $? "7f-7 恢复后新轮次进入导出"
TURNS=$(grep -c "^## " /tmp/laew-e2e-hist-work/hist.md 2>/dev/null)
[ "$TURNS" = "3" ]; check $? "7f-8 导出共 3 轮(2 恢复 + 1 新增,实际 $TURNS)"
# 5) 进程 4:先聊一轮(新会话落盘)→ /resume <原会话 id 前缀>:当前已有轮次 → 自动快照分支
# 2026-09-22 第 115 轮加固:此前取 `:0:24`(=  `YYYYMMDD-HHMMSS-<device>` ),当同一秒内
# 有多个会话(7f 的 4 个进程各自很快,常落在同一秒)时前缀命中多个 → laew 打印
# 「前缀命中 N 个会话,请补长前缀」→ 7f-9/7f-10 假失败(实测:新增 §4o 后时序偏移即复现)。
# 取 40 字符仍是**前缀**(继续验证前缀恢复语义),但已包含纳秒段,足以唯一。
OUT=$(cd /tmp/laew-e2e-hist-work && printf "先聊一轮\n/resume ${HIST_ID:0:40}\n你好\n/exit\n" | run timeout 120 "$HISTBIN")
echo "$OUT" | grep -q "已自动存为分支"; check $? "7f-9 /resume 前自动快照当前对话"
echo "$OUT" | grep -q "已恢复会话"; check $? "7f-10 /resume session-id 前缀恢复生效"
kill $HIST_MOCK_PID 2>/dev/null
rm -rf /tmp/laew-e2e-hist-root /tmp/laew-e2e-hist-work
