# --- 5d. Debug 模式端到端(Debug Agent 请求可辨识 + 报告落盘,2026-09-09 第 08 轮) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=5d —— 改这里不要动入口脚本。
# 方案见 tmpPlan/2026-09-09_08-Agent身份逐请求注入与抓包可见性.md §2.4。
# 验证:-debug 任务链路贯通;Debug Agent(第 7 角色)的请求以自身 User-Agent
# 出现在抓包(此前 e2e 无 -debug 用例,Debug 数据包永远不可见);
# anthropic metadata.user_id 携带 agent 字段;DebugReport 新生成报告文件。
# 注:mock 无 Debug 角色标记 → 走 subagent 兜底脚本(第 1 次返回工具调用,
# Debug Agent 无工具 → 失败回填;第 2 次返回最终文本),max_iterations=2 内收敛。
