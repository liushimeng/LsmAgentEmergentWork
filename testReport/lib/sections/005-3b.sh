# --- 3b. provider 导入/导出(--inprovider / --outprovider)---
# 由 testReport/run_e2e.sh 自动 source;小节 id=3b —— 改这里不要动入口脚本。
# feat 897ea2d 引入了两条 CLI;原 §3 仅覆盖 add/list/use/delete,
# 本节用独立 JSON 文件 → --inprovider 导入 → provider list 校验 →
# --outprovider 导出 → 校验文件结构(metadata/version/exported_at/count)四项。
section "3b. provider 导入/导出(--inprovider/--outprovider)"
PROV_IMPORT=/tmp/laew-e2e-prov-import.json
PROV_EXPORT=/tmp/laew-e2e-prov-export.json
# 1) 构造 2 条记录的数组 + 1 条与库中已有 mockA 重复记录(校验 skipped)
cat > "$PROV_IMPORT" <<'JSON'
[
  {
    "protocol": "anthropic",
    "provider_name": "imp-anti",
    "model_name": "imp-mdl",
    "end_point": "http://127.0.0.1:18899",
    "api_key": "sk-imp-anti"
  },
  {
    "protocol": "openai",
    "provider_name": "imp-open",
    "model_name": "imp-oai",
    "end_point": "http://127.0.0.1:18899/v1",
    "api_key": "sk-imp-open"
  },
  {
    "protocol": "anthropic",
    "provider_name": "mockA",
    "model_name": "claude-mock",
    "end_point": "http://127.0.0.1:18899",
    "api_key": "sk-mock-ant"
  }
]
JSON
OUT=$(run "$LAEW" --inprovider "$PROV_IMPORT"); check $? "--inprovider 退出码 0"
echo "$OUT" | grep -q "成功: 2"; check $? "--inprovider 报告 2 条成功"
echo "$OUT" | grep -q "跳过(重复): 1"; check $? "--inprovider 报告 1 条重复跳过"
OUT=$(run "$LAEW" provider list)
echo "$OUT" | grep -q "imp-anti"; check $? "导入后 list 含 imp-anti"
echo "$OUT" | grep -q "imp-open"; check $? "导入后 list 含 imp-open"
# 2) --outprovider 导出
run "$LAEW" --outprovider "$PROV_EXPORT"; check $? "--outprovider 退出码 0"
[ -f "$PROV_EXPORT" ]; check $? "导出文件已生成"
python3 - "$PROV_EXPORT" <<'PYEOF' 2>&1 | tee -a "$REPORT"
import json, sys
path = sys.argv[1]
data = json.loads(open(path, encoding="utf-8").read())
ok = True
def chk(cond, name):
    global ok
    print(f"  [{'PASS' if cond else 'FAIL'}] {name}")
    ok = ok and cond
chk(isinstance(data, dict), "导出顶层是对象")
chk(data.get("version") >= "1.1", f"导出含 version>=1.1 (got={data.get('version')!r})")
chk(isinstance(data.get("exported_at"), str) and "T" in data.get("exported_at", ""), "导出含 exported_at (RFC3339)")
chk(isinstance(data.get("count"), int) and data["count"] >= 4, f"导出 count >= 4 (got={data.get('count')})")
chk(isinstance(data.get("providers"), list) and len(data["providers"]) == data.get("count"), "providers 数组长度 == count")
prov_names = {p["provider_name"] for p in data.get("providers", [])}
for need in ("mockA", "mockO", "imp-anti", "imp-open"):
    chk(need in prov_names, f"导出含记录 {need}")
sys.exit(0 if ok else 1)
PYEOF
check $? "导出文件结构(version/exported_at/count/providers 数组)校验"
rm -f "$PROV_IMPORT" "$PROV_EXPORT"
