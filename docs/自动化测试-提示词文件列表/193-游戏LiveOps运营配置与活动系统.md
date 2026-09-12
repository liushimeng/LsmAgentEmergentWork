# 193 游戏 LiveOps 运营配置与活动系统

> 编号段 GE01–GE10 · 聚焦「游戏上线后的运营技术」：远程配置与功能开关 / 活动排期与互斥 / 补偿邮件 / 热修包管理 / AB 实验分流 / 公告跑马灯 / 版本强更与灰度 / 运营数据日报 / 礼码兑换与风控 / 客服查询工具
>
> 与现有维度互补说明：
> - `95-多人在线游戏服务器编程实战`（CM 维）聚焦**玩法侧服务端技术**（匹配/同步/反作弊 netcode）；本文件聚焦**运营侧系统**（配置/活动/礼包/公告），服务器不碰玩法逻辑。
> - `147-游戏公平性与反作弊风控工程`（EM 维）聚焦**作弊对抗**；本文件 GE09 礼码风控聚焦**营销滥用**（批量刷码/羊毛党），攻击者画像不同。
> - `102-电商交易与营销系统编程`（CT 维）CT05 是**电商优惠券模板核销**；本文件 GE09 是游戏道具礼码（含游戏内邮件发放/背包/风控特征差异），互补不重复。
> - `99-推荐系统与个性化工程`（CQ 维）不含游戏运营配置；`107-弹性系统与混沌工程实战`（CY 维）的功能开关是**服务稳定性语境**；本文件 GE01 是**产品运营语境**（玩法开关/数值调整）。
>
> **本文件独特主题**：配置分层下发（全局/渠道/灰度三段覆盖）/ 活动时间轴互斥与资源冲突检测 / 幂等补偿邮件队列 / 热修 semver 兼容矩阵 / 一致性 hash 分桶 AB / 公告优先级与时段静默 / 强更门槛与灰度放量 / 指标日报模板化 / 礼码哈希批量生成与核销风控 / 客服查询脱敏视图。

---

### GE01 远程配置与功能开关

- **预期档位**: medium
- **考察维度**: 配置分层 / 热更新 / 客户端缓存
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ge01_remoteconf.py`：远程配置中心（纯 Python 模拟），含：(a) 配置模型（JSON 树：全局默认层 → 渠道层（ios/android/tap）→ 灰度白名单层，逐层覆盖合并函数 merge）+ (b) CLI：`set/get/dump` 三命令（set 可指定层 + get 输出合并后的最终值与来源层追踪「该值来自灰度层」）+ (c) 版本与下发（每次 set 版本号 +1 + 变更 diff 记录 + 客户端拉取带 version 增量语义说明：304 未变更不重传）+ (d) 功能开关规范（bool 开关命名 `feature_xxx` + 灰度按 uid 百分比 + kill switch 紧急下线语义）+ (e) 客户端缓存策略（本地兜底上次配置 + 拉取失败降级 + 超时默认安全值）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/ge01_remoteconf.py`，断言退出码为 0。
  3. Read `tmpPlan/agent-test/ge01_remoteconf.py`，再 Bash：依次 `set global coin_rate=1.5`、`set channel:ios coin_rate=2.0`、`set gray:uid_100 coin_rate=3.0` 后 `get ios uid_100 coin_rate` 与 `get ios uid_200 coin_rate`，断言前者输出 3.0 且来源=灰度层、后者 2.0 来源=渠道层、`dump` 输出含三层结构。

### GE02 活动排期与互斥检测

- **预期档位**: medium
- **考察维度**: 时间轴冲突 / 资源互斥 / 排期修正
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ge02_calendar.py`：活动排期检测器（输入活动清单 JSON：名称/起止/参与入口/奖励资源/推送位），输出：(a) 时间轴 ASCII 图（按日分格 + 每活动横条 + 重叠区标红说明）+ (b) 冲突检测三类（同入口时段重叠=入口抢占/同奖励资源并发=通胀风险/推送位同时段=消息淹没，各列冲突对）+ (c) 修正建议（错峰偏移天数/入口轮播/奖励错开产出代币）+ (d) 关键节点表（每活动：预热日/开启日/收官日/发奖日 四节点 + 与版本更新日错开的检查）+ (e) 排期治理（提前 2 周冻结/变更需审批流留痕/节假日缓冲）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/ge02_calendar.py`，断言退出码为 0。
  3. Read `tmpPlan/agent-test/ge02_calendar.py`，再 Write `tmpPlan/agent-test/ge02_events.json`：5 个活动（2 个主界面入口重叠 + 2 个同发钻石奖励 + 1 个与版本更新日撞车）。本轮必须显式复用第一级的三类冲突定义。
  4. Bash：`python3 tmpPlan/agent-test/ge02_calendar.py tmpPlan/agent-test/ge02_events.json 2>&1 | tee tmpPlan/agent-test/ge02_schedule.md`，断言检测出 ≥ 3 处冲突、含冲突对名称、`grep -F '错峰\|修正'` 命中、含时间轴字符（`grep -F '──\|██'` 命中）。

### GE03 补偿邮件与奖励发放

- **预期档位**: medium
- **考察维度**: 幂等发放 / 批量队列 / 审计留痕
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ge03_compense.py`：补偿邮件系统（SQLite 存储，仅标准库），含：(a) 数据模型（mail 表：id/uid/标题/附件 JSON/created/claimed + 发放批次表 batch：批次号/原因/人数/状态）+ (b) CLI：`send --batch B1 --uids file --reason 停机补偿 --attach coin:100,gem:20`（幂等：同 batch+uid 唯一约束重复 send 跳过并计数）/ `claim <uid>`（领取改 claimed + 再领拒绝）+ (c) 失败处理（无效 uid 落失败清单文件不阻断批次）+ (d) 审计（每次操作输出：批次/时间/成功/跳过/失败 三计数 + batch 表可追溯）+ (e) 防错设计（附件格式白名单校验 + 金额上限熔断：单批次总币量 > 阈值需 --force 双确认）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/ge03_compense.py`，断言退出码为 0；造 uid 文件（含 1 个无效 uid）。
  3. Read `tmpPlan/agent-test/ge03_compense.py`，再 Bash：`send` 批次 B1 → 重复 `send` 同批次断言 `跳过` 计数=全员且无新增 → `claim` 两次同一 uid 断言第二次拒绝（`已领取` 字样）→ 输出审计行含三计数。

### GE04 热修包管理

- **预期档位**: hard
- **考察维度**: 版本兼容 / 差量更新 / 回滚链
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ge04_hotfix.py`：热修管理器（输入热修清单 JSON：补丁号/依赖版本范围/变更文件/时间），输出：(a) semver 兼容矩阵（客户端版本 × 补丁 的适用表 + `>=1.2.0 <1.4.0` 范围语法说明 + 破坏性变更必须升 minor 的纪律）+ (b) 补丁链完整性（每个补丁的 pre-patch 依赖形成链 + 断链检测：客户端装了 3 未装 2 → 报错清单）+ (c) 差量设计（文件级 diff 清单 + 只下发变更文件 + 累计差量超 30% 建议合入整包更新的阈值论证）+ (d) 回滚（补丁互斥组：新补丁上线自动失效旧补丁的同文件段 + 紧急回滚=标记补丁 disabled 客户端下次校验跳过）+ (e) 发布纪律（灰度渠道先行 24h + 崩溃率监控阈值 + 回滚预案演练）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/ge04_hotfix.py`，断言退出码为 0。
  3. Read `tmpPlan/agent-test/ge04_hotfix.py`，再 Write `tmpPlan/agent-test/ge04_patches.json`：4 个补丁（p3 依赖 p2 但故意漏配导致客户端 1.2.5 断链 + p4 与 p1 改同文件需互斥）。本轮必须显式复用第一级的断链检测与互斥组。
  4. Bash：`python3 tmpPlan/agent-test/ge04_hotfix.py tmpPlan/agent-test/ge04_patches.json 2>&1 | tee tmpPlan/agent-test/ge04_matrix.md`，断言报出 `断链` 或缺失依赖、`grep -F '互斥'` 命中、含版本×补丁矩阵表（`|` 行 ≥ 3）、`grep -F '回滚'` 命中。

### GE05 AB 实验分流

- **预期档位**: hard
- **考察维度**: 一致性分桶 / 实验指标 / 显著性判断
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ge05_abtest.py`：AB 分流引擎，含：(a) 分桶算法（uid + 实验盐 sha1 → 取模 100 → 桶区间映射组名 + 同 uid 同实验永远同组的确定性 + 不同实验盐隔离避免相关）+ (b) CLI：`assign <实验> <uid>` 输出组名；`simulate <实验> <uid数>` 输出各组占比（验证均匀性 ±2%）+ (c) 互斥层（同层实验共用分桶空间不重叠 + 落选 uid 进 default 说明）+ (d) 指标判定（两组转化样本做双比例 z 检验：内置函数算 z 与 p 近似 + p<0.05 才可信 + 样本量不足提示继续观察）+ (e) 实验纪律（一次一变量/最小观察周期 7 天覆盖周末/护栏指标：崩溃率与留存不劣化）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/ge05_abtest.py`，断言退出码为 0。
  3. Read `tmpPlan/agent-test/ge05_abtest.py`，再 Bash：`simulate exp_a 10000` 断言两组占比在 48%-52% 区间；`assign exp_a u_42` 连跑两次断言组名相同；内置样本跑 z 检验断言输出含 `p=` 或 `显著` 字样。

### GE06 公告与跑马灯编排

- **预期档位**: simple
- **考察维度**: 优先级队列 / 时段静默 / 多端适配
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ge06_notice.py`：公告编排器（输入公告清单 JSON），输出：(a) 优先级模型（紧急维护=置顶弹窗/活动=跑马灯轮播/社区=公告页归档 + 同级 FIFO）+ (b) 跑马灯排程表（轮播顺序 + 每条展示秒数 + 频次（每 10 分钟一轮）文本时间表）+ (c) 时段静默（23:00-8:00 非紧急公告不推送 + 时区策略：按服务器统一还是玩家本地——结论给玩家本地并附理由）+ (d) 多端适配（文本长度截断规则：弹窗 200 字/跑马灯 40 字 + 富文本降级纯文本 + 跳转链接深链 scheme）+ (e) 模板化（占位符 {server}/{start_time} + 多语言键说明引用 138 号 ED 维不重复展开）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/ge06_notice.py`，断言退出码为 0。
  3. Read `tmpPlan/agent-test/ge06_notice.py`，再 Write `tmpPlan/agent-test/ge06_notices.json`：6 条公告（1 紧急维护 + 3 活动 + 1 社区 + 1 深夜待静默的版本预告）。本轮必须显式复用第一级的优先级与静默模型。
  4. Bash：`python3 tmpPlan/agent-test/ge06_notice.py tmpPlan/agent-test/ge06_notices.json 2>&1 | tee tmpPlan/agent-test/ge06_plan.md`，断言紧急公告置顶标记、静默时段公告被标注（`静默` 命中）、含轮播顺序表、`grep -F '截断'` 命中。

### GE07 版本强更与灰度发布

- **预期档位**: medium
- **考察维度**: 强更门槛 / 灰度放量 / 停服策略
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ge07_release.py`：版本发布编排器（输入版本与渠道信息 JSON），输出：(a) 强更判定（最低运行版本 min_version 与最新版 semver 比较 → 三档：可玩可更新提示/核心资源不兼容弹窗强更/服务器协议不兼容停服维护 + 客户端收到 426 类响应码的处理链）+ (b) 灰度放量曲线（渠道阶段：内部 1% → 10% → 50% → 全量 + 每阶段观察指标：崩溃率/启动耗时/付费成功率 + 异常回退上一阶段动作）+ (c) 兼容期策略（新旧协议并存窗口 ≥ 2 周 + 服务器双协议路由 + 老版本引导文案模板）+ (d) 商店审核时差（App Store 审核期间安卓先发的分渠道版本矩阵管理 + 过审后同步开服）+ (e) 回滚预案（包回滚不可行时的热修兜底 + 数据库向后兼容纪律：新版本可读旧数据）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/ge07_release.py`，断言退出码为 0。
  3. Read `tmpPlan/agent-test/ge07_release.py`，再 Write `tmpPlan/agent-test/ge07_release.json`：新版本 2.0.0 含协议不兼容改动 + min_version 提到 1.8.0 + iOS 审核中安卓先发。本轮必须显式复用第一级的三档判定与放量曲线。
  4. Bash：`python3 tmpPlan/agent-test/ge07_release.py tmpPlan/agent-test/ge07_release.json 2>&1 | tee tmpPlan/agent-test/ge07_rollout.md`，断言 `grep -F '强更\|426'` 命中、`grep -F '灰度\|1%'` 命中、`grep -F '双协议\|兼容'` 命中、`grep -F '回滚\|热修'` 命中。

### GE08 运营数据日报

- **预期档位**: medium
- **考察维度**: 指标口径 / 日报模板 / 异动标注
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ge08_daily.py`：运营日报生成器（输入当日与昨日指标 JSON），输出 Markdown 日报：(a) 核心指标卡（DAU/新增/次留/7 留/付费率/ARPPU/ARPU 七指标：今日值、环比 Δ%、周同比 Δ%）+ (b) 异动标注（|Δ| ≥ 15% 标 ⚠ 并附「先查口径再查活动再查事故」三步归因清单）+ (c) 漏斗段（下载→激活→注册→次日回访 四级转化率 + 最弱环节高亮）+ (d) 活动速报（当日进行中活动参与率与产出监控表）+ (e) 免责口径注脚（时区/统计延迟/渠道包差异三行固定注脚防误读）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/ge08_daily.py`，断言退出码为 0。
  3. Read `tmpPlan/agent-test/ge08_daily.py`，再 Write `tmpPlan/agent-test/ge08_metrics.json`：今日/昨日两组指标（付费率从 3.2% 跌到 2.1% 制造异动 + DAU 微涨）。本轮必须显式复用第一级的异动阈值与归因清单。
  4. Bash：`python3 tmpPlan/agent-test/ge08_daily.py tmpPlan/agent-test/ge08_metrics.json 2>&1 | tee tmpPlan/agent-test/ge08_report.md`，断言含 ⚠ 标记、`grep -F '归因'` 命中、含七指标表头、`grep -F '注脚\|口径'` 命中。

### GE09 礼码兑换与风控

- **预期档位**: hard
- **考察维度**: 批量生成 / 核销幂等 / 滥用风控
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ge09_giftcode.py`：礼码系统（SQLite），含：(a) 生成器（`gen <前缀> <数量> <奖励>`：随机 12 位 Base32 码 + 全局唯一约束 + 只存 sha256 不存明文防拖库 + 导出 CSV 一次性明文下发渠道）+ (b) 兑换（`redeem <码> <uid>`：哈希查表 → 有效性（类型：通用码/一码一用/每 UID 一用）→ 核销原子性（UPDATE ... WHERE claimed=0 判影响行数防并发双花）+ (c) 风控（同 uid 10 分钟内尝试 ≥ 5 个失败码=爆破嫌疑记录 + 同 IP 段兑换频次统计 + 小号关联：同设备指纹 uid 群体兑换同一码标记）+ (d) 对账（渠道分发数 vs 核销数 vs 库存余量 三方对账表）+ (e) 巡检（泄露检测：异常兑换速率告警阈值说明）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/ge09_giftcode.py`，断言退出码为 0；`gen NEWYEAR 100` 生成。
  3. Read 生成的 CSV 前 3 行，再 Bash：`redeem` 同一码同 uid 两次断言第二次拒绝（`已兑换`）；不同 uid 各兑一次断言均成功；伪造 6 次错误码兑换断言风控记录出现（`爆破\|嫌疑` 命中）；对账表三计数输出。

### GE10 客服查询工具

- **预期档位**: medium
- **考察维度**: 数据聚合 / 脱敏视图 / 操作留痕
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/ge10_support.py`：客服查询台（输入玩家数据 JSON：账号/充值/背包/邮件/登录记录），输出客服视图 Markdown：(a) 玩家 360 卡（注册渠道/最近登录/累计充值/VIP 等级/当前封禁状态 六行速览）+ (b) 脱敏规则（手机号与身份证中间打码 + 充值金额客服可见但卡号不可见 + 会话聊天记录只显示最近 3 天——权限最小化原则说明）+ (c) 常见诉求路由（「没收到奖励」→ 自动带出邮件领取记录与活动参与记录核查段 /「账号被盗」→ 登录 IP 异地变更段 /「误封申诉」→ 处罚记录与证据引用段，每类自动预取相关数据段）+ (d) 操作留痕（客服每步查询写审计日志：谁/何时/查了谁/看了什么段 + 高危操作（改数据/发补偿）二次审批流说明）+ (e) SLA（查询响应 < 3s 的索引建议 + 复杂工单升级路径）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/ge10_support.py`，断言退出码为 0。
  3. Read `tmpPlan/agent-test/ge10_support.py`，再 Write `tmpPlan/agent-test/ge10_player.json`：玩家数据（含手机号 13812345678/近 3 天与 30 天聊天/邮件 5 封/登录 IP 换城市）。本轮必须显式复用第一级的脱敏与诉求路由。
  4. Bash：`python3 tmpPlan/agent-test/ge10_support.py tmpPlan/agent-test/ge10_player.json 2>&1 | tee tmpPlan/agent-test/ge10_view.md`，断言手机号被打码（含 `****` 或 `138****`）、`grep -F '审计\|留痕'` 命中、`grep -F '异地\|IP'` 命中、含至少 3 类诉求路由段标题。
