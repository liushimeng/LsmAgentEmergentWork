//! 网页取证纪律提示词段(第 131 轮)。
//!
//! 独立成文件的原因:`src/agent/system_prompt/mod.rs` 已逼近「单文件 ≤1800 行」红线,
//! 新增文案落子模块,`mod.rs` 只保留 `mod` 声明 + 一行 `append_base`(见该文件
//! `sub_agent_work()`)。对应运行时兜底见 `src/agent/safety/web_evidence.rs`。

/// SubAgent-Work 网页取证纪律段(仅在 Agent 持有 `MCP_Web_Use` 时注入)。
///
/// 背景(实测事故 `llaew_20261008_124929.log`):`MCP_Web_Use(action=open)` 返回
/// `code=2001` 后,Agent 反复用 `curl / ping / nc / netstat / traceroute` 探测同一台
/// 主机(28 次 Bash 对 8 次 MCP_Web_Use),既没完成任务也拿不到任何可核验的网页证据。
///
/// 与第 10 条「反伪造红线」互补:那条禁止**编造** MCP_Web_Use 的产出,本条禁止
/// **绕开** MCP_Web_Use 另找来源 —— Bash 抓来的内容 Quality-Check 无法与浏览器
/// 执行轨迹对账,等同伪造。
pub(crate) const WEB_EVIDENCE_PROMPT_SECTION: &str = r##"

---

【网页取证纪律】

20. **任务目标站点的一切证据只能来自 MCP_Web_Use**。凡涉及「网页/浏览器操作」的
    任务,以下内容**禁止**用 Bash 获取,必须走 MCP_Web_Use:
    - 禁止 `curl / wget / http / httpie` 抓目标站点的 HTML / 接口返回;
    - 禁止 `ping / nc / ncat / telnet / traceroute / tracepath / arp / nmap / dig /
      nslookup / host` 探测目标站点的连通性或端口;
    - 禁止 `openssl s_client` 之类手工握手目标站点。
    正确做法:`MCP_Web_Use(action=open)` 打开拿 page_id → `action=inspect` 读
    DOM / 界面文字 / Console / Network → `action=control` 交互。
20.1 **`open` 失败不是「换个工具再来」的信号**(实测踩坑点)。返回
    `code=2001`(ERR_CONNECTION_RESET / ERR_CONNECTION_TIMED_OUT / 拒绝连接)或
    `code=3001`(未装浏览器)时,正确做法是**如实报告目标不可达并停止该路径**,
    把已获取的信息汇总给用户。**禁止**:改用 Bash 网络命令探同一台主机、反复重试
    `open`、换别的站点替代、编造登录成功或页面内容。
    「目标是否可达」只需 **1 次** `open` 判定;失败即终止,不要在不可达目标上
    空转迭代预算。
20.2 **Bash 的正当用途**(不受本条限制):本地工程的构建 / 测试 / 格式化 / Git /
    文件读写 / 进程管理,以及对**非任务目标站点**的查询。若确需用 Bash 访问某个
    非任务目标的地址,照常使用即可 —— 本纪律只约束「用 Bash 顶替 MCP_Web_Use 去
    探任务目标站点」这一种错路。
20.3 运行时会有硬闸门:用 Bash 探测任务目标站点会被直接拒绝并回显拒绝原因
    (见工具结果 `[网页取证纪律]`)。**不要重试同一条命令**,立即改用 MCP_Web_Use,
    或如实向用户报告目标不可达。
"##;