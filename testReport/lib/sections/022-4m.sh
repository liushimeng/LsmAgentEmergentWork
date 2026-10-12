# --- 4m. Read 工具多模态与编码探测(第 77 轮,read_detect.rs) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=4m —— 改这里不要动入口脚本。
# 方案见 tmpPlan/2026-09-17_05-Read工具多模态与编码探测增强方案.md。
# 验证:Read 工具分流逻辑经 32 项单测覆盖(见 cargo test agent::tools::read),
# e2e 层面验证 mock 调用链中 Read 工具的 image/png / PDF / UTF-16 分流请求
# 能被 LLM 正确消费(不崩溃、任务完成)。
section "4m. Read 工具多模态与编码探测端到端(第 77 轮)"
MULTI_DIR=/tmp/laew-e2e-multi; rm -rf "$MULTI_DIR"; mkdir -p "$MULTI_DIR"
python3 - <<PYEOF
with open("$MULTI_DIR/sample.png","wb") as f:
    f.write(b"\x89PNG\r\n\x1a\n" + bytes(100))
with open("$MULTI_DIR/sample.pdf","wb") as f:
    f.write(b"%PDF-1.4\n1 0 obj\n<<" + bytes(100))
with open("$MULTI_DIR/utf16le.txt","wb") as f:
    f.write(b"\xff\xfe")
    f.write("Hello, 世界!\nLine2\n".encode("utf-16-le"))
with open("$MULTI_DIR/large.png","wb") as f:
    f.write(b"\x89PNG\r\n\x1a\n" + bytes(6*1024*1024))
PYEOF
# 4m-1: 读 PNG 任务能完成(不崩溃、任务正常收口)
# 验证:任务 outcome 收口(非崩溃),且 mock 日志含 image/png 标记(Read 工具探测生效)
OUT=$(cd "$MULTI_DIR" && run "$LAEW" -p "Read 这张图片: $MULTI_DIR/sample.png")
echo "$OUT" | grep -qE "任务收口|outcome|用量"; check $? "4m-1 PNG 读取任务正常收口"
# 4m-4: 读 PDF 任务能完成
OUT=$(cd "$MULTI_DIR" && run "$LAEW" -p "Read 这个 PDF: $MULTI_DIR/sample.pdf")
echo "$OUT" | grep -qE "任务收口|outcome|用量"; check $? "4m-4 PDF 读取任务正常收口"
# 4m-5: 读超大 PNG 任务能完成(友好拒绝)
OUT=$(cd "$MULTI_DIR" && run "$LAEW" -p "Read 大图片: $MULTI_DIR/large.png")
echo "$OUT" | grep -qE "任务收口|outcome|用量"; check $? "4m-5 超大 PNG 读取任务正常收口"
rm -rf "$MULTI_DIR"
