# 自动化测试提示词 — 领域驱动设计 DDD 专题（BD01–BD10）

> 使用说明:聚焦 DDD 全栈实践:战略设计 / 战术模式 / 事件风暴 / 上下文映射 / 聚合根设计 / 限界上下文集成;每条要求 laew 写 Python 领域模型 + 领域事件 CSV + 事件风暴产物 + 规则校验脚本,可运行的业务逻辑必须断言状态机路径、不变量与事件落点,不靠口头讲解模式。

## 维度说明
考察 laew 把领域模型落成可验证代码 + 可执行产出的能力:概念→写 Python 聚合根/值对象(Write)→运行状态机(Bash)→断言不变量(Read+Bash);事件风暴与上下文映射落盘为 CSV/markdown,脚本核对必备字段;规则校验覆盖聚合根跨态禁则、事件命名格式、BC 边界。

---

### BD01 通用语言(Ubiquitous Language)建立与维护
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: simple
- **考察维度**: 术语表结构化 / 漂移检测规则
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ddd-lang/glossary.csv` 写一份术语表 12 条:每行 `term, business_meaning, tech_alias, context, owner`,至少覆盖「订单」在客服/财务/仓库三个上下文的差异。
  2. `Bash` 跑 `wc -l glossary.csv` 断言 = 12,`awk -F, '{print $3}' glossary.csv | grep -cE '(Entity|Service|Manager)'` 检测术语漂移(技术词),要求 ≤2 个。
  3. `Read` glossary 后写 `detect_drift.py`:扫描代码注释或变量名里出现 tech_alias 但缺失对应 business 词时告警,输出 `drift.txt`。
  4. 故意造一份漂移:把 2 个 business_meaning 列写成 Entity 形式,重跑必须非 0 输出漂移的 term,补齐 business_meaning 后再跑恢复 0。

### BD02 限界上下文(Bounded Context)划分实战
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: hard
- **考察维度**: BC 边界落盘 / 领域事件→聚合→BC
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ddd-bc/events.csv` 写电商领域事件 15 条:`event_name, aggregate, context`,覆盖 Order/Inventory/Payment/Marketing/User 5 个聚合。
  2. `Bash` 跑 `awk -F, '{print $3}' events.csv | sort -u | wc -l` 断言恰好 5 个 BC,`awk -F, '{print $2}' events.csv | sort -u | wc -l` 断言 ≥5 聚合。
  3. `Read` events 后写 `derive_bc.py`:按 event→aggregate→BC 自动聚类,输出 `bc_map.csv` 含 `bc, event_count, agg_count`。
  4. 故意把 2 条事件的 context 写错(跨 BC 误标),重跑 derive_bc.py 后 `diff <(sort bc_map.csv) <(sort expected.csv)` 非空,定位修正事件后重跑 diff 空;再写 `storm.md` 设计 3 小时工作坊议程(参与者/产出物/时间盒),`Bash` `grep -cE '^### ' storm.md` ≥ 5。

### BD03 上下文映射(Context Map)与集成模式
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: medium
- **考察维度**: 8 种 Context Map 模式 / ACL 翻译
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ddd-cm/map.csv` 写 5 对 BC 集成关系:`upstream, downstream, pattern`,pattern 必须在 Partnership/Shared Kernel/Customer-Supplier/Conformist/ACL/Open-Host Service/Published Language/Separate Ways 八选。
  2. `Bash` 跑 `awk -F, 'NR>1{print $3}' map.csv | sort -u | wc -l` 断言模式数 ≥3(不可全用同一种)。
  3. `Read` map 后写 `acl.py` 最小 ACL 翻译:Order 上游模型 `{order_id, total_cents, address}` 翻译成 Marketing 促销上下文 `{promo_order_id, discount_eligible, region}`,`Bash` 跑 `python3 acl.py` 输入示例 order,输出必须含 `promo_order_id`。
  4. `Write` `check_cm.py` 校验 map.csv:upstream 与 downstream 必须都在已有 BC 列表中(不允许悬空),`Bash` 跑退出码 0,故意造一对悬空 BC 重跑必须非 0 指出缺失。

### BD04 聚合根(Aggregate Root)设计与一致性边界
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: hard
- **考察维度**: 聚合根状态机 / 不变量保护
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ddd-agg/order.py` 写 Order 聚合根(`@dataclass` 含 `status/items/total_cents`),含方法 `add_item/split/ship/cancel`,每次操作必须校验不变量(status 顺序 + 金额非负)。
  2. `Bash` 跑 `python3 order.py` 演示合法路径(NEW→PAID→SHIPPED),断言 exit 0。
  3. `Read` order.py 后写 `test_order.py` 用 `assert` 覆盖 5 条业务不变量:NEW 状态不可 ship、PAID 不可再 add_item、金额必须 >0、cancel 后不能再次 PAID、ship 前必须 PAID,`python3 -m unittest` 必须全部通过且 `OK`。
  4. 故意加一条「跨态禁则」:test_illegal_ship 期望抛异常,跑出来通过;然后把 cancel 方法的禁则校验逻辑「注释掉」,跑 test_order.py 期望 test_cancel 失败,验证不变量保护生效。

### BD05 值对象(Value Object)与不可变性
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: medium
- **考察维度**: VO 不可变 / 构造时校验
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ddd-vo/vos.py` 写 4 个值对象:`Money(amount, currency)`、`Email(value)`、`DateRange(start, end)`、`Address(city, street)`,全部 `@dataclass(frozen=True)` + 构造时校验。
  2. `Bash` 跑 `grep -cE '@dataclass\(frozen=True\)' vos.py` 断言 = 4,`grep -cE 'raise ValueError' vos.py` 断言 ≥4(每 VO 一校验)。
  3. `Write` `test_vo.py`:构造合法 VO、构造非法 VO(如 Money 金额负数、Email 无 @、DateRange start>end)必须抛 ValueError,`python3 -m unittest` 全部通过。
  4. 故意让 Money 校验允许负金额(注释 `if amount < 0`),重跑 test_vo 必须失败,恢复后再跑 0;`Write` `vo_usage.md` 列 4 个 VO 在 Order 聚合根中的使用点。

### BD06 领域服务(Domain Service)与应用服务分层
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: medium
- **考察维度**: 三层分工 / 领域逻辑可测
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ddd-svc/` 写三层:`application/task_service.py`(编排 + 事务 + DTO)、`domain/difficulty_evaluator.py`(3 档分类逻辑)、`infrastructure/llm_client.py`(HTTP 占位)。
  2. `Bash` 跑 `grep -RE 'import .*infrastructure' application/ domain/` 断言无反向依赖(数值=0),`grep -RE 'import .*domain' application/` 断言 ≥1。
  3. `Write` `test_difficulty.py`:用 5 条任务样本(simple/medium/hard)验证 difficulty_evaluator 分类结果,`Bash` 跑 `python3 test_difficulty.py` 期望分类命中率 ≥4/5。
  4. 故意把 domain/difficulty_evaluator.py 里的一条分类阈值调错,重跑命中率 <4,定位修正后恢复 ≥4;`Write` `layer.md` 列三层调用链+分层校验规则 5 条。

### BD07 领域事件(Domain Event)与解耦
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: hard
- **考察维度**: 事件流落盘 / 订阅关系核对
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ddd-ev/events.csv` 写 8 条领域事件:`event_name(version 后缀 V1/V2 至少 1 条), aggregate, subscribers(JSON 列表)`,至少 3 个 aggregate、每个事件 ≥1 subscriber。
  2. `Bash` 跑 `awk -F, '{print $2}' events.csv | sort -u | wc -l` 断言 ≥3 聚合,`awk -F, '{print $1}' events.csv | grep -c 'V[0-9]'` 断言至少 1 条带版本后缀。
  3. `Read` events 后写 `publish.py` 最小事件总线:`subscribe(topic, handler)` + `publish(topic, event)` + handler 抛异常不阻断其他订阅者,跑 3 个订阅者 + 1 个抛异常订阅者,`Bash` 跑断言其余 2 个订阅者仍收到事件。
  4. `Write` `check_events.py` 校验:每个事件的 subscriber 必须引用已存在的 BC(不允许悬空),`Bash` 跑退出码 0,故意造一条悬空 subscriber 重跑必须非 0 指出缺失。

### BD08 仓储(Repository)模式与持久化
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: medium
- **考察维度**: 仓储接口 / 持久化无关
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ddd-repo/domain/` 写 `task_repo.py`(接口:`find_by_id/save/find_incomplete`),在 `infrastructure/` 写 `sqlite_task_repo.py`(用 Python sqlite3 实现)。
  2. `Bash` 跑 `grep -RE 'import .*(infrastructure|sqlite)' domain/` 断言无反向依赖(数值=0)。
  3. `Write` `test_repo.py`:建临时 sqlite、save 3 条任务、find_by_id 验证内容一致、find_incomplete 过滤已完成,`Bash` 跑 `python3 test_repo.py` 全部断言通过。
  4. 故意把 sqlite 实现里的 save 方法改为覆盖(不校验主键),重跑 find_by_id 必须失败,恢复后 0;`Write` `repo_design.md` 列接口 3 方法 + 实现要点 + 持久化无关设计 4 条。

### BD09 事件风暴(Event Storming)工作坊
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: medium
- **考察维度**: 工作坊议程 / 产出物清单
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ddd-storm/agenda.md` 设计一场 3 小时工作坊:6 段(开场/事件识别/命令识别/聚合划分/BC 边界/总结),每段含时间盒(总时长 180min)。
  2. `Bash` 跑 `awk -F'[()]' '/^## /{sum+=$2} END{print sum}' agenda.md` 断言总时长 = 180(min)。
  3. `Write` `check_agenda.py` 校验:每段必须含「产出物」关键词(至少 3 类:事件列表/BC 草图/Context Map/待澄清问题),`Bash` 跑退出码 0。
  4. `Read` 后加一份「参与者角色分工」`roles.md`:5 个角色(业务/开发/测试/UX/引导师)每角色 ≥3 职责,`Bash` `grep -cE '^### ' roles.md` 断言 = 5,缺失补齐。

### BD10 DDD 落地陷阱与团队转型
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: medium
- **考察维度**: 转型路线图 / 3 阶段核对
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ddd-adopt/roadmap.md` 写一份 18 个月 DDD 转型路线图:3 阶段(探索/推广/收敛),每阶段 6 个月,每阶段 ≥4 项关键工作。
  2. `Bash` 跑 `grep -cE '^## (探索|推广|收敛)' roadmap.md` 断言 = 3,`grep -cE '^### ' roadmap.md` 断言 ≥ 12(每阶段 4 项)。
  3. `Write` `check_roadmap.py` 校验:每阶段必须含「风险」「SLA」「负责人」「产出物」四关键词,`Bash` 跑退出码 0。
  4. 故意把「推广」阶段的负责人写成空,重跑必须非 0 指出缺负责人,补齐后再跑恢复 0;`Write` `anti_pattern.md` 列 5 个 DDD 落地陷阱(教条化/过度设计/名词驱动/文档驱动/单兵作战),每项 ≥2 行描述。