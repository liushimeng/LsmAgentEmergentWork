# --- 7d. Markdown 富文本渲染(TUI 任务结果路径,2026-09-15) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=7d —— 改这里不要动入口脚本。
# 设计见 docs/TUIMarkdown富文本渲染/01-设计与解决方案.md。
# prompt router 驱动 simple 档任务:Bash printf 产出含 Markdown 的工具输出,
# SubAgent 终答携带该摘录(BUG-M4 终答摘录机制)进入 Executed 渲染路径。断言:
#  1) 链路贯通;2) 可见文本 100% 保真;3) 语法标记被渲染符号替换(▍/•/│);
#  4) 屏幕输出含 ANSI 着色序列;5) /export transcript 保持纯文本(同源分流)。
section "7d. Markdown 富文本渲染"
MD_ROUTER=/tmp/laew-e2e-md-router.json
cat > "$MD_ROUTER" <<'JSONEOF'
{"rules": [
  {"keywords": ["MD_E2E_MARKDOWN"],
   "yolo": {"task_level": "simple", "goal_summary": "输出并解读 Markdown 样例",
            "purpose": "验证 TUI Markdown 富文本渲染", "intent": "verify",
            "decomposition_plan": ["生成样例"]},
   "tools": [
     {"call_no": 1, "tool": "Bash",
      "args": {"command": "printf '# 一级标题\\n正文**加粗**与`内联码`。\\n- 列表甲\\n> 引用乙\\n'"}}
   ]}
]}
JSONEOF
python3 scripts/mock_llm_server.py $MOCK_PORT "$ROOT_DIR/testReport/mock_requests-7d-$RUN_ID.jsonl" --prompt-router-file "$MD_ROUTER" &>/dev/null &
MOCK7D_PID=$!; wait_port $MOCK_PORT $MOCK7D_PID || echo "  [WARN] mock 未就绪($MOCK_PORT)" | tee -a "$REPORT"
# 2026-09-16 第 61 轮 5e 补加的 provider use 可能改写当前记录 → 7d 跑到这里时
# provider 已切到 mockW,任务链路会走 mockW(mockA 上的 router 已无意义)。
# 切回 mockA 保持 7d 既有行为(MockA 是默认 mock,Yolo/SubAgent 都可达)。
ID_7D=$(run "$LAEW" provider list 2>&1 | grep mockA | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_7D" >/dev/null 2>&1
rm -rf /tmp/laew-e2e-md-work; mkdir -p /tmp/laew-e2e-md-work
OUT=$(cd /tmp/laew-e2e-md-work && printf '处理 MD_E2E_MARKDOWN 任务\n/export\n/exit\n' | run timeout 90 "$LAEW")
MD_OUT_PLAIN=$(printf '%s\n' "$OUT" | sed -E 's/\x1b\[[0-9;]*m//g')
echo "$MD_OUT_PLAIN" | grep -q "MOCK_FINAL_ANSWER"; check $? "7d-1 Markdown 任务链路贯通(终答保真)"
echo "$MD_OUT_PLAIN" | grep -qF "一级标题"; check $? "7d-2 标题文本保真"
echo "$MD_OUT_PLAIN" | grep -qF "加粗"; check $? "7d-2b 粗体文字保真"
echo "$MD_OUT_PLAIN" | grep -qF "内联码"; check $? "7d-2c 行内码文字保真"
echo "$MD_OUT_PLAIN" | grep -qF "列表甲"; check $? "7d-2d 列表文字保真"
echo "$MD_OUT_PLAIN" | grep -qF "引用乙"; check $? "7d-2e 引用文字保真"
# 标记符号被替换:** / # / - / > 不再以原文形态出现在渲染行中
# 2026-09-16 第 61 轮:trace 区会原文保留工具参数以利于排查;检查渲染正文时
# 排除 [tool] / [stage] 行,不能把「参数里的 Markdown 源码」误判为富文本渲染失败。
echo "$MD_OUT_PLAIN" | grep -v -E '^[[:space:]]*\[(tool|stage)\]' | grep -qF "**加粗**"; [ $? -ne 0 ]; check $? "7d-3 ** 标记被渲染剥离"
echo "$MD_OUT_PLAIN" | grep -qF "▍ 一级标题"; check $? "7d-4 标题渲染为 ▍ 前缀"
echo "$MD_OUT_PLAIN" | grep -qF "• 列表甲"; check $? "7d-5 列表渲染为 • 符号"
echo "$MD_OUT_PLAIN" | grep -qF "│ 引用乙"; check $? "7d-6 引用渲染为 │ 竖线"
# 屏幕输出含 ANSI 序列(原输出更长于剥离后)
[ "${#OUT}" -gt "${#MD_OUT_PLAIN}" ]; check $? "7d-7 任务结果输出含 ANSI 着色序列"
# transcript/导出纯文本分流:导出文件保留原文 **加粗**,且不含 ESC 序列
EXPORT_NAME7D=$(printf '%s\n' "$MD_OUT_PLAIN" | grep -oE "laew-export-[0-9]+-[0-9]+\.md" | head -1)
EXPORT7D="/tmp/laew-e2e-md-work/${EXPORT_NAME7D:-not-found}"
[ -n "$EXPORT_NAME7D" ] && [ -f "$EXPORT7D" ]; check $? "7d-8 /export 文件已落盘(${EXPORT_NAME7D:-未找到})"
grep -qF '**加粗**' "$EXPORT7D" 2>/dev/null; check $? "7d-9 导出为原始 Markdown 文本(** 未剥离)"
grep -q $'\033[' "$EXPORT7D" 2>/dev/null; [ $? -ne 0 ]; check $? "7d-10 导出不含 ANSI(纯文本同源)"
kill $MOCK7D_PID 2>/dev/null
rm -f "$MD_ROUTER" "$ROOT_DIR/testReport/mock_requests-7d-$RUN_ID.jsonl"; rm -rf /tmp/laew-e2e-md-work
