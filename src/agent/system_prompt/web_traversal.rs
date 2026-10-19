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
22.3 **整页抹除处置纪律**(页面曾正常加载、随后 URL 变 about:blank /
    响应带 `page_health.reason=site_wipe` / `page_dead{reason=site_wipe}` /
    `site_wipe` / 6004 时):
    这是站点对本环境的反爬抹除,**不要反复 `close(page_id="all") + open`**
    —— 每试一次多弹一台 Chrome 窗口,若该域是指纹级打标(对 CDP 拉起的
    浏览器环境本身打标)换新 profile 照样被抹,只会形成拉起→被抹→再拉起的
    风暴。正确处置:① 同域首次被抹可试 1 次 close(all) 换全新 profile;
    ② **累计抹除 ≥2 次即判定「换全新 profile 已被实测无效」,不要再 close(all)
    重开**,改用 `open(reuse_existing=true)` 接管**用户日常 Chrome 本体**
    (需人工先用 `--remote-debugging-port=9222` 启动自己的日常 profile、
    不加 `--user-data-dir`;注意 `auto_relaunch` 复制出来的调试实例仍属
    CDP 拉起环境,同样会被抹),或 `request_human` 如实交还现场由人工处理;
    ③ 工具层对该域 laew 身份累计抹除达阈值后**持久熔断**(6004,
    `data.persistent=true`),且该结论**跨会话记住**(下次任务第一次 open
    即直接熔断、不再拉起 Chrome),此时只剩接管日常浏览器或人工两条路,
    硬闯请先说清理由再用 `force=true`;
    ④ 拿到 `page_dead{reason=site_wipe}` 时**不要重新 open/navigate 重试**
    —— 那正是该域已知无效的动作;按上面的改道走,或如实报告不可达。
"##;
