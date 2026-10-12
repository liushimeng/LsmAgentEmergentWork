# --- 3c. context_max_size 参数(CLI + 导入导出,feat 本轮) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=3c —— 改这里不要动入口脚本。
# 设计见 docs/Context设置与自动压缩设计/01-设计与解决方案.md §3
section "3c. context_max_size 参数(CLI/导入导出)"
run "$LAEW" provider add --protocol anthropic --provider-name ctxK --model-name m-ctx \
  --end-point http://127.0.0.1:$MOCK_PORT --api-key sk-ctx --context-max-size 256K
check $? "add 带 --context-max-size 256K"
OUT=$(run "$LAEW" provider list)
echo "$OUT" | grep "ctxK" | grep -q "ctx: 256K"; check $? "list 显示 ctx: 256K(K/M 后缀解析)"
echo "$OUT" | grep "mockA" | grep -q "ctx: 800K"; check $? "缺省记录自动补默认 800K"
# 非法值应报错(非 0 退出)
if run "$LAEW" provider add --protocol anthropic --provider-name bad --model-name m \
  --end-point http://127.0.0.1:$MOCK_PORT --api-key sk-bad --context-max-size abc >/dev/null 2>&1; then
  check 1 "非法 context-max-size 被拒绝"
else
  check 0 "非法 context-max-size 被拒绝"
fi
# 旧版格式(无 context_max_size 字段)导入 → 补默认 800K
PROV_OLD=/tmp/laew-e2e-prov-old.json
cat > "$PROV_OLD" <<'JSON'
{"protocol":"anthropic","provider_name":"oldfmt","model_name":"m-old","end_point":"http://127.0.0.1:18899","api_key":"sk-old"}
JSON
run "$LAEW" --inprovider "$PROV_OLD" >/dev/null 2>&1; check $? "旧版格式(无 ctx 字段)导入成功"
OUT=$(run "$LAEW" provider list)
echo "$OUT" | grep "oldfmt" | grep -q "ctx: 800K"; check $? "旧版格式导入后自动补默认 800K"
# 导出携带 context_max_size 字段
PROV_EXPORT2=/tmp/laew-e2e-prov-export2.json
run "$LAEW" --outprovider "$PROV_EXPORT2" >/dev/null 2>&1
grep -q '"context_max_size": 256000' "$PROV_EXPORT2"; check $? "导出文件含 context_max_size 字段(256000)"
rm -f "$PROV_OLD" "$PROV_EXPORT2"
