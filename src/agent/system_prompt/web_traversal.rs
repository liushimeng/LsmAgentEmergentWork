//! 遍历覆盖纪律提示词段(第 146 轮)。
//!
//! 独立成文件的原因与 `web_evidence.rs` 相同:`system_prompt/mod.rs` 已超
//! 「单文件 ≤1800 行」红线,新增文案一律落子模块,`mod.rs` 只保留 `mod` 声明 +
//! 一行 `append_base`。配套的工具层实现见
//! `src/agent/tools/mcp_web_use/visit_ledger.rs`(台账唯一事实源)。
//!
//! 背景(实测事故 `llaew_20261010_125445.log`,77 分钟未跑完):菜单 9 页只深访
//! 6 页,同一批页面被反复遍历(R1 wf-3 与 R2 wf-2/3/4 大面积重测),「哪些页面
//! 去过/没去过」系统层面无人知晓 —— SubAgent 只能自己发明 ledger 约定,单元间
//! 不共享、QC 无法机械对账。第 146 轮把访问台账下沉到工具层
//! (`inspect(info=coverage)`),本段把使用纪律写进提示词。

/// SubAgent-Work 遍历覆盖纪律段(仅在 Agent 持有 `MCP_Web_Use` 时注入)。
pub(crate) const WEB_TRAVERSAL_PROMPT_SECTION: &str = r##"

---

【遍历覆盖纪律】

22. **遍历/走查类任务(「遍历所有页面/所有控件/全面走查」)开工先对账覆盖台账**:
    `MCP_Web_Use(action=inspect, info="coverage")` 返回工具层自动记账的
    distinct_pages / 每页访问次数 / top_repeats —— 这是零幻觉的机械事实。
    - **未访问页面优先**:台账里没有的路由先去,已访问过的不重做;
    - **页面清单来源**:站点侧边栏菜单(extract_links 或 elements)× 覆盖台账
      求差集,得到「还没去过的页面」清单,按清单推进;
    - 每页操作**先验证本页无阻断再深入**(blockers),离开一页前确认该页
      台账/证据已落盘。
22.1 **重复访问警示**:open / navigate / new_tab 的响应带 `visit_note`
    (`该 URL 本次会话已访问 N 次`)时,**先停下自查**:是打转(同一页反复
    进入却无新动作)还是确有必要(如验证修复)?打转即换策略或跳过该页,
    不要机械重进。
22.2 **遍历 ≠ 截图收集**:每页的核心交互(click/下拉/拖拽/tab)各抽样验证并
    记录结果即可,不必对同一控件反复截图;证据以文本台账(DOM 片段/操作前后
    状态)为主,截图仅作辅助(Read 读图片只回存根,视觉判读走
    `control(screenshot, ocr=true)` / `inspect(info=ocr)`)。
"##;
