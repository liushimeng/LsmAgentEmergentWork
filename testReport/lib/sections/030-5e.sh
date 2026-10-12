# --- 5e. MCP_Web_Use 浏览器操控工具链路端到端(2026-09-18 第 89 轮重写) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=5e —— 改这里不要动入口脚本。
# 验证:Yolo 判 medium → Main-Work 拆出 delegate_to="subagent" 的 WorkFlow →
# SubAgent-Work 持 MCP_Web_Use 工具(请求体 tools 数组可辨识)→ 首调
# MCP_Web_Use(action=open, data: URL)→ QC/SessionContext 收口。
# 无浏览器机器上 open 返回 3001 信封(is_error=false),链路仍贯通 ——
# 兼容"未安装浏览器"是工具的设计约束。
section "5e. MCP_Web_Use 浏览器操控工具链路端到端(第 89 轮)"
WEBUSE_MOCK_PORT=$((MOCK_PORT + 61))
WEBUSE_MOCK_LOG="$ROOT_DIR/testReport/mock_requests-5e-$RUN_ID.jsonl"
WEBUSE_ROUTER=$(mktemp)
cat > "$WEBUSE_ROUTER" <<'JSONEOF'
{
  "rules": [
    {"keywords": ["WEB_E2E 打开网页"],
     "yolo": {"task_level": "medium", "goal_summary": "用浏览器打开网页并截图",
              "purpose": "验证浏览器操控工具链路", "intent": "web_automation",
              "decomposition_plan": ["打开页面", "截图"]},
     "mainwork": {"workflows": [{"id": "wf-1", "name": "WEB_E2E 打开网页截图",
                                  "steps": ["MCP_Web_Use action=open 打开 data: 页面", "截图落盘"],
                                  "acceptance": ["页面已打开并截图"],
                                  "delegate_to": "subagent"}]},
     "tools": [{"tool": "MCP_Web_Use",
                 "args": {"action": "open",
                          "url": "data:text/html,<html><head><title>laew-e2e</title></head><body><h1>WEB_E2E_OK</h1></body></html>"}}]}
  ]
}
JSONEOF
python3 scripts/mock_llm_server.py $WEBUSE_MOCK_PORT "$WEBUSE_MOCK_LOG" --prompt-router-file "$WEBUSE_ROUTER" &>/dev/null &
WEBUSE_MOCK_PID=$!; wait_port $WEBUSE_MOCK_PORT $WEBUSE_MOCK_PID || echo "  [WARN] mock 未就绪($WEBUSE_MOCK_PORT)" | tee -a "$REPORT"
run "$LAEW" provider add --protocol anthropic --provider-name mockW --model-name mock-webuse \
    --end-point "http://127.0.0.1:$WEBUSE_MOCK_PORT" --api-key sk-webuse-e2e
LIST_WEBUSE=$(run "$LAEW" provider list 2>&1)
ID_WEBUSE=$(echo "$LIST_WEBUSE" | grep mockW | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_WEBUSE" >/dev/null 2>&1
OUT=$(run timeout 90 "$LAEW" -p "WEB_E2E 打开网页并截图存档")
echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "5e-1 浏览器任务链路贯通(终答保真)"
grep -q '"user-agent": "LsmAgentEmergentWork-SubAgent-Work/' "$WEBUSE_MOCK_LOG"; check $? "5e-2 mock 收到 SubAgent-Work 请求(执行器唯一化)"
grep -q '"name": "MCP_Web_Use"' "$WEBUSE_MOCK_LOG"; check $? "5e-3 请求体 tools 含 MCP_Web_Use 工具定义"
grep -q 'WEB_E2E_OK' "$WEBUSE_MOCK_LOG"; check $? "5e-4 SubAgent 首调发出 MCP_Web_Use(action=open,url=data:...)"
kill $WEBUSE_MOCK_PID 2>/dev/null
run "$LAEW" provider delete "$ID_WEBUSE" >/dev/null 2>&1
rm -f "$WEBUSE_ROUTER" "$WEBUSE_MOCK_LOG"

kill $MOCK_PID 2>/dev/null
