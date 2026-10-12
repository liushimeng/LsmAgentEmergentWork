# --- 2b. 未配置模型时 TUI 管道模式守门(第 71 轮,tmpPlan/2026-09-17_03) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=2b —— 改这里不要动入口脚本。
# 非 TTY 管道输入走 TUI run() 回退,与交互 TUI 同一条 dispatch_prompt 链路。
# 修复前:NoopLlm 占位文本进全链路 → Yolo 降级 → QC fail-closed → 3 轮盲重试,
# 以「Quality 报告解析失败」误导收口;修复后:守门秒回指引,不进编排器。
section "2b. 未配置模型时 TUI 管道模式守门"
OUT=$(echo "查看一下,当前进程列表" | timeout 10 "$LAEW" 2>&1)
echo "$OUT" | grep -q "provider add"; check $? "TUI 管道模式提示先 provider add"
echo "$OUT" | grep -q "未配置"; check $? "TUI 管道模式含未配置指引"
echo "$OUT" | grep -q "未进入编排器"; check $? "守门明确说明未进入编排器"
echo "$OUT" | grep -q "orchestrator 调度中"; [ $? -ne 0 ]; check $? "未触发编排器调度"
echo "$OUT" | grep -q "Quality 报告解析失败"; [ $? -ne 0 ]; check $? "不再出现误导性 Quality 解析失败"
