# --- 4k. Diff 渲染 + 语法高亮端到端(L1436-L1445 D5 + L1641-L1700 D10,第十八/十九轮 P0) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4k —— 改这里不要动入口脚本。
# 验证:/diff 命令读取两个文件并输出带 ANSI 着色的 diff;
# /diff 输出含 +++ / --- 标题行、新增/删除行着色标记;
# 代码围栏(```lang ... ```)在主屏输出时按语言高亮(输出含 ANSI 转义序列)。
section "4k. Diff 渲染与语法高亮(D5+D10 P0)"
DIFF_DIR=$(mktemp -d)
cat > "$DIFF_DIR/old.rs" <<'EOF'
fn hello() {
    println!("hello");
}
fn world() {}
EOF
cat > "$DIFF_DIR/new.rs" <<'EOF'
pub fn hello() {
    println!("hello");
}
fn new_added() { /* 新增行 */ }
fn world() {}
EOF
DIFF_OUT=$(run "$LAEW" -p "/diff $DIFF_DIR/old.rs $DIFF_DIR/new.rs")
# 剥离 ANSI 转义序列后的纯文本(后续 grep 用)
DIFF_PLAIN=$(echo "$DIFF_OUT" | sed -E 's/\x1b\[[0-9;]*m//g')
echo "$DIFF_PLAIN" | grep -qF "+++"; check $? "4k-1 diff 输出含 +++ 标题行"
echo "$DIFF_PLAIN" | grep -qF -- "---"; check $? "4k-1 diff 输出含 --- 标题行"
echo "$DIFF_PLAIN" | grep -qF "new_added"; check $? "4k-1 diff 输出含新增行内容"
echo "$DIFF_PLAIN" | grep -q "fn hello"; check $? "4k-1 diff 输出含原行内容"
# ANSI 转义序列检查(行着色):剥离前后对比长度,原输出明显更长(含 ANSI)
[ "${#DIFF_OUT}" -gt "${#DIFF_PLAIN}" ]; check $? "4k-1 diff 输出含 ANSI 转义序列(着色标记)"
# 用法错误提示
DIFF_ERR=$(run "$LAEW" -p "/diff")
echo "$DIFF_ERR" | grep -q "用法"; check $? "4k-2 /diff 无参数时输出用法提示"
# 文件不存在提示
DIFF_NOFILE=$(run "$LAEW" -p "/diff /nonexistent/a.rs /nonexistent/b.rs")
echo "$DIFF_NOFILE" | grep -q "diff 错误"; check $? "4k-3 /diff 文件不存在时输出错误提示"
rm -rf "$DIFF_DIR"
