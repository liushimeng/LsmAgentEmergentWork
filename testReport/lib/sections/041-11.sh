# --- 11. MCP server 配置 CRUD + mock MCP 连通性 ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=11 —— 改这里不要动入口脚本。
# 设计见 docs/Use_MCP/01-设计与解决方案.md。用 scripts/mock_mcp_server.py
# (NDJSON stdio 最小 MCP server)验证 `laew mcp add/list/test/del` 全链路:
# initialize 握手 → tools/list(2 工具)→ 断开。
section "11. laew mcp CRUD + mock MCP server 连通性"
MOCK_MCP="$ROOT_DIR/scripts/mock_mcp_server.py"
run "$LAEW" mcp add --name mockmcp --transport stdio --command python3 --args "[\"$MOCK_MCP\"]"
check $? "mcp add stdio 记录"
run "$LAEW" mcp add --name mockhttp --transport http --url http://127.0.0.1:9/mcp --timeout-ms 5000 >/dev/null 2>&1
check $? "mcp add http 记录"
OUT=$(run "$LAEW" mcp list); echo "$OUT" | grep -q "mockmcp"; check $? "mcp list 显示 mockmcp"
echo "$OUT" | grep -q "mockhttp"; check $? "mcp list 显示 mockhttp"
OUT=$(run "$LAEW" mcp test mockmcp 2>&1); echo "$OUT" | grep -q "握手成功"; check $? "mcp test 握手成功"
echo "$OUT" | grep -q "echo"; check $? "mcp test 列出 echo 工具"
echo "$OUT" | grep -q "fail"; check $? "mcp test 列出 fail 工具"
OUT=$(run "$LAEW" mcp del mockhttp 2>&1); echo "$OUT" | grep -q "已删除"; check $? "mcp del 删除记录"
OUT=$(run "$LAEW" mcp list); echo "$OUT" | grep -q "mockhttp"; [ $? -ne 0 ]; check $? "删除后 list 不再显示 mockhttp"
# 校验失败路径:stdio 缺 --command 应被拒
if "$LAEW" mcp add --name badmcp --transport stdio >/dev/null 2>&1; then
  check 1 "stdio 缺 command 应拒绝"
else
  check 0 "stdio 缺 command 应拒绝"
fi
