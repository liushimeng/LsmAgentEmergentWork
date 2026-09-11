# 148 公共交通 GTFS 与出行调度仿真

> 编号段 EN01–EN10 · 聚焦「静态时刻表、日历例外、换乘路径、实时延误和运营排班」：用离线数据构建可断言的公交规划系统。
>
> 与现有维度互补说明：
> - 100 地理空间 GIS 偏地图与空间计算；本文件聚焦**GTFS 数据模型、出行链路和服务日历**。
> - 41 数学建模偏通用优化；本文件聚焦公交场景的可达性、延误与司机轮班约束。
> - 140 团队运营偏日常流程；本文件聚焦车辆、线路和站点协同。
>
> **本文件独特主题**：GTFS 清单 / service calendar / 换乘图 / 时刻冲突 / 延误传播 / 票务规则 / 无障碍路径 / 司机轮班 / 客流仿真 / 临时绕行。

### EN01 GTFS 静态数据完整性审计

- **预期档位**: hard
- **考察维度**: foreign key / required field / geographic sanity
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/en01_gtfs.py`：审计模拟 GTFS 目录：agency、routes、stops、trips、stop_times、calendar；检查必填列、外键、递增 stop_sequence、时间格式、经纬度范围和重复 trip_id。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/en01_gtfs.py`，断言退出码为 0
  3. Read `tmpPlan/agent-test/en01_gtfs.py`，再 Write `tmpPlan/agent-test/en01_findings.json`：审计结果按 blocker / warning / info 分级，每条含表、行号、字段、值、修复建议。 本轮必须显式复用第一轮的命名、字段或结论，不允许另起无关主题。
  4. Bash：`python3 -m json.tool tmpPlan/agent-test/en01_findings.json >/dev/null`，断言第二轮产物 JSON 有效。

### EN02 服务日历与例外日建模

- **预期档位**: medium
- **考察维度**: calendar_dates / timezone / DST
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/en02_calendar.py`：把 calendar 周期与 calendar_dates 例外合并成具体服务日；处理节假日加开、停运、跨零点 trip 和时区边界，输出 service_id 生效日期集。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/en02_calendar.py`，断言退出码为 0
  3. Read `tmpPlan/agent-test/en02_calendar.py`，再 Write `tmpPlan/agent-test/en02_services.json`：四个样例：工作日通勤、周末、节假日停运、临时马拉松加开。 本轮必须显式复用第一轮的命名、字段或结论，不允许另起无关主题。
  4. Bash：`python3 -m json.tool tmpPlan/agent-test/en02_services.json >/dev/null`，断言第二轮产物 JSON 有效。

### EN03 最少换乘出行路径

- **预期档位**: hard
- **考察维度**: time-dependent graph / transfer / walking edge
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/en03_router.py`：实现简化 Raptor：按时刻表轮扫 k 次换乘，支持步行换乘边、最早到达和最少换乘两个目标；输出每站最优标记与回溯路径。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/en03_router.py`，断言退出码为 0
  3. Read `tmpPlan/agent-test/en03_router.py`，再 Write `tmpPlan/agent-test/en03_itineraries.json`：三条方案：直达、一次换乘更早、两次换乘避开停运；每条含 leg、wait、walk、transfer、fare_hint。 本轮必须显式复用第一轮的命名、字段或结论，不允许另起无关主题。
  4. Bash：`python3 -m json.tool tmpPlan/agent-test/en03_itineraries.json >/dev/null`，断言第二轮产物 JSON 有效。

### EN04 车辆时刻冲突与折返检测

- **预期档位**: hard
- **考察维度**: block scheduling / turnaround / dwell
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/en04_conflict.py`：把 trip 分配到车辆 block，检测重叠、折返时间不足、场站容量超限、连续驾驶时长和最小停站时间违规；输出可交换 trip 的建议。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/en04_conflict.py`，断言退出码为 0
  3. Read `tmpPlan/agent-test/en04_conflict.py`，再 Write `tmpPlan/agent-test/en04_blocks.json`：样例包含正常折返、重叠冲突、夜间跨日 trip、容量不足和修复后排班。 本轮必须显式复用第一轮的命名、字段或结论，不允许另起无关主题。
  4. Bash：`python3 -m json.tool tmpPlan/agent-test/en04_blocks.json >/dev/null`，断言第二轮产物 JSON 有效。

### EN05 实时延误传播预测

- **预期档位**: hard
- **考察维度**: delay propagation / recovery slack / eta
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/en05_delay.py`：从车辆位置和已过站实际时间预测后续 ETA；使用线路历史、停站冗余、交通状态类别和换乘等待，输出置信区间与恢复点。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/en05_delay.py`，断言退出码为 0
  3. Read `tmpPlan/agent-test/en05_delay.py`，再 Write `tmpPlan/agent-test/en05_alerts.json`：三个乘客提示：错过换乘、追加备用方案、延误恢复；包含 affected_trip、new_eta、confidence、advice。 本轮必须显式复用第一轮的命名、字段或结论，不允许另起无关主题。
  4. Bash：`python3 -m json.tool tmpPlan/agent-test/en05_alerts.json >/dev/null`，断言第二轮产物 JSON 有效。

### EN06 票务规则与出行费用计算

- **预期档位**: medium
- **考察维度**: fare media / transfer discount / caps
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/en06_fare.py`：根据区间、换乘、票卡类型、时段和日封顶计算票价；支持学生、老人、单程票、通勤卡，禁止重复享受换乘优惠。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/en06_fare.py`，断言退出码为 0
  3. Read `tmpPlan/agent-test/en06_fare.py`，再 Write `tmpPlan/agent-test/en06_receipts.json`：五张费用明细：normal、student、跨线换乘、封顶后免费、无效区间。 本轮必须显式复用第一轮的命名、字段或结论，不允许另起无关主题。
  4. Bash：`python3 -m json.tool tmpPlan/agent-test/en06_receipts.json >/dev/null`，断言第二轮产物 JSON 有效。

### EN07 车站无障碍路径建模

- **预期档位**: medium
- **考察维度**: accessibility / graph / evacuation
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/en07_access.py`：为站内电梯、扶梯、楼梯、闸机和站台构建带属性图；电梯故障时重算轮椅路径，输出距离、耗时、障碍和备用出口。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/en07_access.py`，断言退出码为 0
  3. Read `tmpPlan/agent-test/en07_access.py`，再 Write `tmpPlan/agent-test/en07_routes.json`：两条轮椅路径：电梯正常和电梯故障绕行；含 step、distance、barrier、assist_required。 本轮必须显式复用第一轮的命名、字段或结论，不允许另起无关主题。
  4. Bash：`python3 -m json.tool tmpPlan/agent-test/en07_routes.json >/dev/null`，断言第二轮产物 JSON 有效。

### EN08 司机轮班与休息合规

- **预期档位**: hard
- **考察维度**: roster / rest rule / fairness
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/en08_roster.py`：把 trip 段组成司机班次；检查连续驾驶、吃饭、夜间休息、加班上限、场站交接和班次间隔，并按工时与周末班次数评估公平。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/en08_roster.py`，断言退出码为 0
  3. Read `tmpPlan/agent-test/en08_roster.py`，再 Write `tmpPlan/agent-test/en08_roster.csv`：字段：driver、shift_id、start、end、trip_ids、drive_minutes、break_minutes、violation、fairness_score。 本轮必须显式复用第一轮的命名、字段或结论，不允许另起无关主题。
  4. Bash：`test -s tmpPlan/agent-test/en08_roster.csv`，断言产物非空。

### EN09 客流载荷与增开班次仿真

- **预期档位**: hard
- **考察维度**: load simulation / capacity / headway
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/en09_load.py`：模拟乘客按 OD 到站、上车、乘坐和下车；计算每段满载率、留乘人数、站台等待和换乘拥挤，评估加开、区间车或大站快车方案。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/en09_load.py`，断言退出码为 0
  3. Read `tmpPlan/agent-test/en09_load.py`，再 Write `tmpPlan/agent-test/en09_scenarios.json`：三方案对比：原计划、加开一班、缩短间隔；输出成本占位、运力、p95 等待、拥挤站排名。 本轮必须显式复用第一轮的命名、字段或结论，不允许另起无关主题。
  4. Bash：`python3 -m json.tool tmpPlan/agent-test/en09_scenarios.json >/dev/null`，断言第二轮产物 JSON 有效。

### EN10 临时封路绕行运营预案

- **预期档位**: medium
- **考察维度**: detour / passenger notice / rollback
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/en10_detour.py`：根据停用站点和替代站点生成绕行 trip 变更：skipstops、shape、时间调整、换乘接驳和乘客通知分层；支持生效窗口和回滚。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/en10_detour.py`，断言退出码为 0
  3. Read `tmpPlan/agent-test/en10_detour.py`，再 Write `tmpPlan/agent-test/en10_notice.md`：面向 App、站台屏幕、客服和司机的四类通知；包含生效时间、替代路线、无障碍指引和恢复条件。 本轮必须显式复用第一轮的命名、字段或结论，不允许另起无关主题。
  4. Bash：`test -s tmpPlan/agent-test/en10_notice.md`，断言产物非空。
