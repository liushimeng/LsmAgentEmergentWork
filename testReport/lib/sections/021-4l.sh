# --- 4l. D1 @文件提及端到端(L1426,2026-09-10 第二十八轮) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4l —— 改这里不要动入口脚本。
# 方案见 tmpPlan/2026-09-10_13-D1-文件提及与路径补全方案.md。
# 验证:-p 模式下 @文件 命中真实文件时,内容以 <<<LAEW:ATTACHMENTS>>> 附件块
# 真实出现在发往 LLM 的 wire 请求体;目录提及内联条目;不存在路径静默跳过
# 并打印 [laew] 跳过提示,且不产生附件块。
section "4l. D1 @文件提及端到端(L1426)"
MENTION_DIR=/tmp/laew-e2e-mention; rm -rf "$MENTION_DIR"; mkdir -p "$MENTION_DIR/sub"
printf 'CANARY-MENTION-7721\n' > "$MENTION_DIR/canary.txt"
printf 'sub-inner\n' > "$MENTION_DIR/sub/inner.txt"
# 正例 1:文件提及 → wire 含附件块标记 + 文件内容
OUT=$(cd "$MENTION_DIR" && run "$LAEW" -p "请总结 @canary.txt 的内容")
grep -F -q "LAEW:ATTACHMENTS" "$MOCK_LOG"; check $? "4l-1 mock 日志含 <<<LAEW:ATTACHMENTS>>> 附件块标记"
grep -F -q "CANARY-MENTION-7721" "$MOCK_LOG"; check $? "4l-2 wire 请求体含 @文件真实内容(读取注入生效)"
echo "$OUT" | grep -q "已附加 1 个"; check $? "4l-3 CLI 打印已附加提示"
# 正例 2:目录提及 → 条目列表内联
OUT=$(cd "$MENTION_DIR" && run "$LAEW" -p "看看 @sub 里有什么")
grep -F -q "inner.txt" "$MOCK_LOG"; check $? "4l-4 wire 请求体含目录条目 inner.txt(目录内联生效)"
# 负例:不存在路径 → 跳过提示 + 无新增附件块(用唯一 token 观察日志增量)
MISS_MARK="MISS-CANARY-$$"
OUT=$(cd "$MENTION_DIR" && run "$LAEW" -p "看 @$MISS_MARK.txt")
echo "$OUT" | grep -q "跳过 @ 提及"; check $? "4l-5 不存在路径打印跳过提示"
grep -F -q "$MISS_MARK" "$MOCK_LOG" && ! grep -F -q "<<<FILE: $MISS_MARK.txt>>>" "$MOCK_LOG"; check $? "4l-6 不存在路径不产生附件块(原文透传)"
rm -rf "$MENTION_DIR"
