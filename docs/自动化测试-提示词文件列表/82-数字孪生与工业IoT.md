# 82 数字孪生与工业IoT

> 编号段 CA41–CA50 · 聚焦数字孪生与工业 IoT 工程：OPC UA / MQTT Sparkplug B / 工业协议 / 实时映射 / 仿真集成 / 预测性维护 / 边缘-云协同 / 工业 AI / 孪生数据治理
>
> 与现有维度互补说明：
> - `33-嵌入式系统与物联网开发`（AH01–AH10）聚焦嵌入式硬件与终端设备（FreeRTOS / ESP32 / CAN 总线）；本文件聚焦**工业级数字孪生系统**（OPC UA / Sparkplug B / 实时镜像 / 预测性维护 / 工业 AI 部署）。
> - `69-机器人与ROS2工程`（BR01–BR10）聚焦机器人中间件（ROS2 节点 / Navigation2 / MoveIt2）；本文件聚焦**工业 4.0 场景下的数字孪生**（工厂级仿真 / 实时数据同步 / 工业协议 / IT-OT 融合）。
> - `70-3D建模与几何处理实战`（BS01–BS10）聚焦 3D 建模与几何处理；本文件聚焦**数字孪生的 3D 可视化 + 实时数据驱动 + 仿真集成**。
>
> **本文件独特主题**：数字孪生（Digital Twin）六维模型 / OPC UA 工业协议（信息建模 / 地址空间 / Pub-Sub）/ MQTT Sparkplug B（工业 MQTT 增强）/ IT-OT 融合 / 实时数据流（时序数据库 InfluxDB / TimescaleDB）/ 仿真集成（Unity / Unreal / NVIDIA Omniverse）/ 预测性维护（时序异常检测 / 剩余寿命 RUL 预测）/ 工业 AI 部署（边缘推理 / 模型蒸馏）/ 孪生数据治理（数据血缘 / 同步延迟 / 一致性）/ 工业安全（ISA/IEC 62443 / 工业网络分段）。

---

### CA41 数字孪生六维模型：PE/VE/CN/DD/SS/FE
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-09_15-批量测试优化与状态标记方案.md）
- **预期档位**: medium
- **考察维度**: 数字孪生概念 + 六维模型 + 工业 4.0 范式
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 一份「数字孪生六维模型」说明到 `tmpPlan/agent-test/dt_six_dim.md`：6 行表格（维度=PE/VE/CN/DD/SS/FE，每行含「名称/作用/工业 4.0 映射」三列），每行用 `| ... |` 语法。
  2. Bash 校验：`grep -c '^| ' tmpPlan/agent-test/dt_six_dim.md` 断言行数 ≥ 8（含表头 1 + 分隔 1 + 6 数据行）；`grep -c 'PE\|VE\|CN\|DD\|SS\|FE' tmpPlan/agent-test/dt_six_dim.md` 断言六维缩写全部出现。
  3. Read 后 Write 一段 python3 `tmpPlan/agent-test/dt_level.py`：定义 `DigitalTwinLevel` 枚举（LEVEL1_MODEL / LEVEL2_SHADOW / LEVEL3_TWIN / LEVEL4_AUTONOMOUS），函数 `classify(bidirectional: bool, ai_driven: bool) -> int` 按特征返回等级；测试 4 个场景断言返回正确等级。
  4. Bash：`python3 tmpPlan/agent-test/dt_level.py` 必须输出 4 行等级判定；`grep -c 'LEVEL' tmpPlan/agent-test/dt_level.py` 断言 ≥ 5 处（枚举 + 测试）。

### CA42 OPC UA 信息建模：CNC 机床地址空间
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-09_15-批量测试优化与状态标记方案.md）
- **预期档位**: hard
- **考察维度**: OPC UA 协议 + 信息建模 + 地址空间
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 一段 python3 `tmpPlan/agent-test/opcua_model.py`：用 dataclass 定义 `OPCUANode(node_id, name, node_type, value)`；构造 CNC 机床树：`ns=1;s=Line1.Machine01.Spindle01.Speed=1500` / `...Spindle01.Temperature=45` / `...Feed01.Position=100.0` / `...Controller.Program="O0001"`；实现 `browse(path)` 按路径返回节点。
  2. Bash：`python3 -c "exec(open('tmpPlan/agent-test/opcua_model.py').read()); print(browse('Line1.Machine01.Spindle01.Speed').value)"` 必须输出 1500；`browse('Line1.Machine01.Controller.Program').value` 必须输出 O0001。
  3. Read opcua_model.py 后加 Pub/Sub 模拟：`Publisher` 类每 100ms 发一次 Spindle01.Speed 数据（asyncio），`Subscriber` 类订阅并记录最近 10 个值；Write 测试脚本 `tmpPlan/agent-test/opcua_pubsub.py` 跑 1 秒后断言 subscriber 收到 ≥ 8 条消息。
  4. Bash：`python3 tmpPlan/agent-test/opcua_pubsub.py` 必须输出「received N messages」且 N ≥ 8；`grep -c 'asyncio' tmpPlan/agent-test/opcua_pubsub.py` 断言 ≥ 1。

### CA43 MQTT Sparkplug B：NBIRTH/NDATA/NDEATH 协议
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-09_15-批量测试优化与状态标记方案.md）
- **预期档位**: medium
- **考察维度**: Sparkplug B 规范 + 工业场景适配
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 一段 python3 `tmpPlan/agent-test/sparkplug.py`：定义 `SparkplugMessage(msg_type, group_id, edge_node_id, device_id, payload)`；实现 `topic(msg)` 返回 `spBv1.0/<group_id>/<msg_type>/<edge_node_id>[/<device_id>]`；构造 NBIRTH/NDATA/NDEATH 各一条消息并打印 topic。
  2. Bash：`python3 tmpPlan/agent-test/sparkplug.py` 必须输出 3 行 topic，每行含 `spBv1.0`；`grep -c 'NBIRTH\|NDATA\|NDEATH' tmpPlan/agent-test/sparkplug.py` 断言 ≥ 3。
  3. Read sparkplug.py 后加 `Broker` 类模拟：`subscribe(topic_filter, callback)` + `publish(msg)`；构造 1 个 Edge Node + 3 个 Device，Edge 发 NBIRTH 后每个 Device 发 NDATA；断言 broker 收到 4 条消息。
  4. Bash：`python3 -c "exec(open('tmpPlan/agent-test/sparkplug.py').read()); b=Broker(); ...; print(b.received_count)"` 必须输出 4；`grep -c 'class Broker' tmpPlan/agent-test/sparkplug.py` 断言 ≥ 1。

### CA44 IT-OT 融合网关：Modbus → MQTT 协议转换
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-09_15-批量测试优化与状态标记方案.md）
- **预期档位**: hard
- **考察维度**: IT-OT 差异 + 融合架构 + 协议网关
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 一段 python3 `tmpPlan/agent-test/modbus_mqtt_gw.py`：模拟 Modbus 寄存器表 `{slave_id: {reg_addr: value}}`（3 个 slave 各 10 个寄存器）；`ModbusMaster` 类 `read_holding_registers(slave, addr, n)` 返回值；`MQTTPublisher` 类 `publish(topic, payload)` 打印日志；`Gateway` 类每 100ms 轮询所有 slave 并 publish 到 `modbus/<slave_id>/<addr>`。
  2. Bash：`python3 tmpPlan/agent-test/modbus_mqtt_gw.py` 必须输出 ≥ 30 条 publish 日志（3 slave × 10 reg）；`grep -c 'modbus/' tmpPlan/agent-test/modbus_mqtt_gw.log` 断言 ≥ 30。
  3. Read modbus_mqtt_gw.py 后 Write `tmpPlan/agent-test/gw_test.sh`：bash 启动 gateway 后台跑 2 秒，grep 日志断言 `modbus/1/0` 出现 ≥ 10 次（100ms 轮询 × 2 秒 ≈ 20 次，允许 ≥ 10），kill 后台。
  4. Bash：`bash tmpPlan/agent-test/gw_test.sh` 必须输出 PASS；`grep -c 'Gateway' tmpPlan/agent-test/modbus_mqtt_gw.py` 断言 ≥ 1。

### CA45 时序数据模拟：传感器写入 + 降采样
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-09_15-批量测试优化与状态标记方案.md）
- **预期档位**: medium
- **考察维度**: 时序数据特征 + 数据库选型 + 写入性能
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 一段 python3 `tmpPlan/agent-test/ts_db.py`：用 sqlite3 建表 `sensor_data(ts INTEGER, device_id TEXT, temperature REAL, vibration REAL)`；插入 1000 条模拟数据（10 个设备 × 100 秒，temperature 在 20-80 间随机 + 5% 异常值 > 100）。
  2. Bash：`python3 -c "import sqlite3; c=sqlite3.connect('tmpPlan/agent-test/ts.db'); print(c.execute('SELECT COUNT(*) FROM sensor_data').fetchone()[0])"` 必须输出 1000；`c.execute('SELECT COUNT(*) FROM sensor_data WHERE temperature>100').fetchone()[0]` 必须 ≥ 40（5% 异常）。
  3. Read ts_db.py 后加降采样函数 `downsample(device_id, window='10s')`：每 10 秒聚合 avg/min/max；Write 测试脚本 `tmpPlan/agent-test/downsample.py` 跑 device_0 的降采样，断言返回 10 行（100 秒 / 10 秒）。
  4. Bash：`python3 tmpPlan/agent-test/downsample.py` 必须输出 10 行聚合结果；`grep -c 'AVG\|MIN\|MAX' tmpPlan/agent-test/downsample.py` 断言 ≥ 3。

### CA46 3D 拓扑可视化：Unicode box-drawing 工厂树
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-09_15-批量测试优化与状态标记方案.md）
- **预期档位**: hard
- **考察维度**: 仿真引擎选型 + 实时数据绑定 + 渲染管线
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 一段 python3 `tmpPlan/agent-test/topo_view.py`：定义工厂树 `Factory → [Line1, Line2] → [Machine01, Machine02] → [Spindle, Feed, Controller]`；用 `├── └── │   ` Unicode box-drawing 字符绘制层级树；异常节点（如 Spindle01 温度 > 80）用 `[!]` 标记。
  2. Bash：`python3 tmpPlan/agent-test/topo_view.py` 必须输出含 `├──` 和 `└──` 的树形图；`grep -c '\[!\]' tmpPlan/agent-test/topo_view.py` 断言 ≥ 1（异常标记逻辑）。
  3. Read topo_view.py 后加实时刷新：每 2 秒重新读取传感器数据（从 ts.db）并重绘；Write 测试脚本 `tmpPlan/agent-test/topo_realtime.py` 跑 6 秒后断言输出 ≥ 3 帧。
  4. Bash：`timeout 8 python3 tmpPlan/agent-test/topo_realtime.py 2>&1 | grep -c 'Frame'` 断言 ≥ 3；`grep -c 'box' tmpPlan/agent-test/topo_view.py` 断言 ≥ 1（Unicode 字符常量）。

### CA47 预测性维护：Isolation Forest 异常检测
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-09_15-批量测试优化与状态标记方案.md）
- **预期档位**: hard
- **考察维度**: 异常检测算法 + RUL 预测 + 工程落地
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 一段 python3 `tmpPlan/agent-test/pdm_detect.py`：用 numpy 生成 500 条正常振动数据（均值 0，std 1）+ 20 条异常（均值 5，std 2）；实现简化 Isolation Forest（10 棵树，每树随机选特征 + 随机切分，路径长度 < 阈值判异常）；输出异常索引列表。
  2. Bash：`python3 tmpPlan/agent-test/pdm_detect.py` 必须输出 ≥ 15 条异常检测（召回率 ≥ 75%）；`grep -c 'IsolationForest\|isolation' tmpPlan/agent-test/pdm_detect.py` 断言 ≥ 1。
  3. Read pdm_detect.py 后加 RUL 预测：用线性退化模型 `RUL = (current_value - failure_threshold) / degradation_rate`；Write 测试脚本 `tmpPlan/agent-test/rul_predict.py` 模拟 100 步退化，断言 RUL 预测误差 ≤ 10%。
  4. Bash：`python3 tmpPlan/agent-test/rul_predict.py` 必须输出「RUL error ≤ 10%」；`grep -c 'degradation' tmpPlan/agent-test/rul_predict.py` 断言 ≥ 1。

### CA48 边缘 AI 部署：模型蒸馏 + ONNX 模拟
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-09_15-批量测试优化与状态标记方案.md）
- **预期档位**: hard
- **考察维度**: 模型压缩 + 边缘部署 + 工业落地
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 一段 python3 `tmpPlan/agent-test/distill.py`：模拟 Teacher（大模型，推理慢但准）和 Student（小模型，推理快但略差）；Teacher 用 `time.sleep(0.01)` 模拟延迟 + 95% 准确率；Student 用 `time.sleep(0.001)` + 85% 准确率；蒸馏：Student 在 Teacher 的 soft label 上训练（模拟为准确率提升到 90%）。
  2. Bash：`python3 tmpPlan/agent-test/distill.py` 必须输出 Teacher/Student 延迟比 ≥ 5x、准确率提升 ≥ 5%；`grep -c 'soft_label\|distill' tmpPlan/agent-test/distill.py` 断言 ≥ 2。
  3. Read distill.py 后 Write `tmpPlan/agent-test/onnx_sim.py`：模拟 ONNX Runtime 推理—— 加载一个 10 层 MLP（numpy 实现），输入 100 条数据，输出推理延迟 + 准确率；断言延迟 ≤ 1ms/条。
  4. Bash：`python3 tmpPlan/agent-test/onnx_sim.py` 必须输出「latency ≤ 1ms」；`grep -c 'ONNX\|onnx' tmpPlan/agent-test/onnx_sim.py` 断言 ≥ 1。

### CA49 工业安全：Purdue 模型 + 网络分段
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-09_15-批量测试优化与状态标记方案.md）
- **预期档位**: hard
- **考察维度**: 工业网络安全 + 标准合规 + 攻击防护
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 一段 python3 `tmpPlan/agent-test/purdue_model.py`：定义 `PurdueLevel` 枚举（LEVEL0_PROCESS ~ LEVEL5_ENTERPRISE）；`Zone` 类含 `name, level, allowed_protocols`；构造工厂网络：Level 0/1 = PLC/SCADA（只允许 Modbus），Level 2 = HMI（允许 OPC UA），Level 3 = MES（允许 MQTT/HTTP），Level 4/5 = ERP/Internet（允许 HTTPS）。
  2. Bash：`python3 tmpPlan/agent-test/purdue_model.py` 必须输出 6 个 level 的协议白名单；`grep -c 'Modbus\|OPC_UA\|MQTT\|HTTPS' tmpPlan/agent-test/purdue_model.py` 断言 ≥ 4。
  3. Read purdue_model.py 后加 `check_access(src_level, dst_level, protocol)` 函数：跨层访问必须经过防火墙（只允许相邻层 + 白名单协议）；Write 测试脚本 `tmpPlan/agent-test/purdue_access.py` 跑 5 个场景（含 2 个违规），断言违规被拦截。
  4. Bash：`python3 tmpPlan/agent-test/purdue_access.py` 必须输出「blocked: 2」；`grep -c 'blocked\|allowed' tmpPlan/agent-test/purdue_access.py` 断言 ≥ 5。

### CA50 laew 工业 4.0 集成：Modbus 工具 + 安全确认
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-09_15-批量测试优化与状态标记方案.md）
- **预期档位**: hard
- **考察维度**: 工程化升级 + 工业场景适配
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write 一段 python3 `tmpPlan/agent-test/laew_industrial.py`：模拟 laew 的 `ModbusReadTool` / `ModbusWriteTool`—— read 直接返回寄存器值；write 必须经过 `SafetyChecker`（检查值范围 + 二次确认标记 `confirmed=True`），未确认的 write 抛 `SafetyError`。
  2. Bash：`python3 tmpPlan/agent-test/laew_industrial.py` 必须输出「read OK」+「write without confirm: SafetyError」+「write with confirm: OK」；`grep -c 'SafetyError\|confirmed' tmpPlan/agent-test/laew_industrial.py` 断言 ≥ 2。
  3. Read laew_industrial.py 后 Write `tmpPlan/agent-test/laew_audit.sh`：bash 脚本模拟审计日志—— 每次工具调用追加到 `tmpPlan/agent-test/audit.log`（含 timestamp/tool/params/status），跑 5 次调用后断言日志行数 ≥ 5。
  4. Bash：`bash tmpPlan/agent-test/laew_audit.sh` 必须输出 5 行日志；`wc -l tmpPlan/agent-test/audit.log` 断言 ≥ 5；`grep -c 'SafetyError\|OK' tmpPlan/agent-test/audit.log` 断言 ≥ 3。

---

## 与 laew 工程的具体对接点

1. **CA41 → Yolo 任务分类扩展**：当前 Yolo 只分 simple/medium/hard，可扩展工业领域分类（设备诊断 / 工艺优化 / 安全审计 / 数据分析）。
2. **CA42 → OPC UA 客户端**：未来路线，可加 `opcua` crate 作为 Work Agent 工具。
3. **CA43 → MQTT 客户端**：未来路线，可加 `rumqttc` crate 让 laew 接入工业 IoT 平台。
4. **CA44 → Modbus 工具**：参考 CA42 设计 `ModbusReadTool` + `ModbusWriteTool`，配合安全确认机制。
5. **CA45 → 时序监控表**：建议在 SQLite 加 `llm_call_metrics` 表（timestamp / agent / latency / tokens），作为 laew 自身的可观测性基础设施（参考 CA78 SRE 专题）。
6. **CA49 → BashTool 工业安全**：当前 BashTool 无安全限制，工业场景必须加「白名单 + 二次确认 + 审计日志」。