# 自动化测试提示词 — 数据工程与 ETL 管线实战（AE01–AE10）

> 使用说明见同目录 `README.md`。每条提示词在同一 Session 内按轮次顺序喂入。

## 维度说明

本维度从 Agent 能力视角考察**数据工程基础设施**链路：现场生成多源脏 CSV/JSON → python3 ETL 处理 → 落地存储（parquet 不可用时落 CSV/SQLite）→ 数据质量断言（行数对账/空值率/值域）。所有管线在 `tmpPlan/agent-test/` 沙盒内本机跑通，仅用 python3/bash/awk/sort/uniq，不依赖外网或真实集群。

---

### AE01 ETL 管线：多源脏 CSV 到入库
- **预期档位**: medium
- **考察维度**: 管线设计 / 脏数据治理
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 用 Write 产出 3 份脏 CSV：`orders_a.csv`（UTF-8，含空行与重复头）、`orders_b.csv`（GBK 编码，列名不同 `id|名称|金额`）、`orders_c.csv`（金额含千分位 `1,234.56` 与全角字符 `１２３４`）。Bash 用 `file orders_*.csv` 与 `wc -l` 记录原始行数。
  2. python3 写 `etl.py`：读三源 → 统一列名（`order_id/product/amount`）→ 清洗金额（去千分位、全角转半角）→ 去重（按 order_id）→ 落 SQLite `warehouse.db` 的 `orders` 表。跑 `python3 etl.py` 期望 stdout 输出 `loaded=xxx duplicates=yyy bad_rows=zzz`。Read 验证清洗函数覆盖 4 种脏数据。
  3. 故意把 `orders_b.csv` 改成金额列含非数字（如 `N/A`），跑 `python3 etl.py` 期望 `bad_rows` 增加、错误行进 `quarantine.csv` 而非入库。Read `quarantine.csv` 应至少含 1 行。
  4. 断点续跑：`etl.py` 加 `--state etl_state.json` 记录已处理文件。跑两次，期望第二次 `loaded=0`（幂等不重复入库）。Bash 用 `sqlite3 warehouse.db "SELECT COUNT(*) FROM orders;"` 验证行数稳定。写 `etl-notes.md` 总结脏数据 4 项治理策略。

### AE02 数仓分层：ODS/DWD/DWS/ADS
- **预期档位**: medium
- **考察维度**: 数仓建模 / 口径治理
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 写 `layer_sim.py`：模拟 4 层数据流转。ODS 层读 `orders.csv`（原始订单）→ DWD 层清洗 + 关联 `users.csv`（维度表）→ DWS 层聚合（用户日订单数）→ ADS 层输出报表（Top 10 用户）。Bash 跑 `python3 layer_sim.py` 期望每层输出 CSV 文件：`ods_orders.csv`/`dwd_orders.csv`/`dws_user_daily.csv`/`ads_top10.csv`。
  2. Read `layer_sim.py` 验证每层只做一件事：DWD 不聚合、DWS 不读原始 ODS。故意把 DWD 里加一行聚合代码，跑 `--lint` 期望报错「DWD 层禁止聚合」。
  3. 加 SCD Type 2：`users.csv` 有地址变更历史，DWD 层按 `effective_date`/`expired_date` 展开。跑 `python3 layer_sim.py --scd` 期望 `dwd_users.csv` 行数 > `users.csv` 行数（一对多展开）。Read 验证 `current_flag` 字段存在。
  4. 口径治理：写 `metric_registry.yml` 定义 GMV = `SUM(paid_amount) WHERE status='completed'`。`registry_lint.py` 校验每个指标有且仅有一处权威定义；故意在 DWS 与 ADS 两处定义 GMV，跑 lint 期望报错。写 `metric-governance.md` 总结指标字典管理。

### AE03 CDC 增量同步
- **预期档位**: hard
- **考察维度**: 增量同步 / 一致性语义
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 用 Write 产出 `binlog.jsonl`：模拟 MySQL binlog 流（`op: I/U/D` + `before`/`after` + `ts`）。python3 `cdc_sync.py` 解析 binlog → 生成 upsert/delete SQL 落 SQLite `cdc_target.db`。跑 `python3 cdc_sync.py binlog.jsonl` 期望 stdout 输出 `inserts=.. updates=.. deletes=..`。
  2. 故意在 `binlog.jsonl` 里造乱序：同一行 3 次 U 以乱序 ts 出现。跑 `cdc_sync.py --enforce-order` 期望按 ts 重排后最终态与顺序执行一致；不强制排序时态可能不一致。Read 验证排序分支。
  3. 幂等 upsert：对同一行连续 3 次 I，跑 `cdc_sync.py` 期望最终 1 行而非 3 行。Bash 用 `sqlite3 cdc_target.db "SELECT COUNT(*) FROM orders;"` 验证。
  4. 断点恢复：`cdc_sync.py` 加 `--offset` 参数，处理一半时 offset 记录已处理行；重跑 `--resume` 期望只处理剩余行。故意把 offset 文件删掉重跑，期望从头处理但目标表幂等（不重复）。写 `cdc-consistency.md` 总结至少一次/幂等/最终一致三概念。

### AE04 数据质量：校验、对账与告警
- **预期档位**: medium
- **考察维度**: 质量规则体系 / 门禁
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 写 `quality_rules.yml`：定义 6 条规则（非空 user_id、行数波动 < 20%、金额 ≥ 0、order_id 唯一、status ∈ [paid/shipped/completed]、外键 user_id 存在于 users）。python3 `quality_check.py quality_rules.yml orders.csv` 跑批，期望输出 `report.json` 含每条规则的 `pass/fail` 与违规行号列表。
  2. 故意把 `orders.csv` 改成含 `user_id=99999`（不存在于 users）的行，跑 `quality_check.py` 期望 `FK_VIOLATION` 规则 fail、违规行号指向具体行。Read 验证 report.json 结构。
  3. 对账：`reconcile.py` 对比 `orders.csv` 与 SQLite `warehouse.db` 的 `SUM(amount)` 与 `COUNT(*)`。故意在 DB 里多插一行，跑 `reconcile.py` 期望差异 > 0 并打印差额。
  4. 强弱规则分级：`quality_rules.yml` 加 `severity: hard/soft`。`quality_check.py --mode strict` 期望任一 hard 规则 fail 则退出码 1、soft 规则 fail 仅告警。故意造一份 1 条 hard fail + 2 条 soft fail 的数据，跑期望退出码 1。写 `quality-gate.md` 总结门禁挂在哪一调度环节。

### AE05 调度编排：DAG 依赖与幂等
- **预期档位**: hard
- **考察维度**: 调度器实现 / 失败恢复
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 写 `dag.yml`：定义 5 个任务（extract/transform/load/validate/report）及其 `depends_on` 列表。python3 `scheduler.py dag.yml` 拓扑排序执行，每个任务模拟为 `sleep(random)` 后写 `_success` 文件。跑期望按依赖顺序执行、无依赖任务并发（用 `time` 验证总时长 < 串行和）。
  2. 故意把 transform 任务改成随机失败（`exit(1)`），跑 `scheduler.py` 期望下游 load/validate/report 被跳过、上游 extract 仍成功。Read 验证 skip 日志。
  3. 幂等重跑：`scheduler.py --rerun` 跑两次，期望已成功的任务直接跳过（读 `_success` 文件）。Bash 用 `ls *.success` 验证任务完成状态一致。
  4. 失败重试与退避：transform 任务配 `retries: 3, backoff: exponential`。故意让 transform 前 2 次失败、第 3 次成功，跑 `scheduler.py` 期望重试 3 次后整体成功。Read 验证退避间隔递增。写 `scheduler-notes.md` 总结幂等设计要点。

### AE06 列式存储：Parquet 降级 CSV 对比
- **预期档位**: medium
- **考察维度**: 存储格式 / 分区治理
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 写 `gen_data.py`：生成 100 万行 CSV `events.csv`（列：`user_id/event_type/timestamp/country/device/amount`）。Bash 用 `wc -l` 与 `du -h` 记录原始行数与大小。
  2. `command -v python3 -c "import pyarrow"` 探测 parquet 可用性。若可用：`python3 to_parquet.py events.csv events.parquet` 转 parquet，期望 `du -h` 显示 parquet < CSV 体积 50%。若不可用降级：`python3 to_csv_optimized.py events.csv events_opt.csv` 用 gzip 压缩 + 列裁剪（只保留查询涉及的列），期望体积 < 原始 CSV。
  3. 分区设计：按 `date` 列分区，`python3 partition.py events.csv partitioned/` 输出 `partitioned/date=2026-01-01/...`。故意造一份含 1000 个日期的数据，跑 `--check` 期望警告「分区过细（>365）」；改按 `month` 分区期望通过。
  4. 小文件合并：`python3 compact.py partitioned/ compacted/` 把每个分区内 < 1MB 的文件合并。Bash 用 `find compacted/ -type f | wc -l` 验证文件数减少。写 `storage-choice.md` 总结行存 vs 列存 vs 压缩 CSV 的取舍。

### AE07 流式 ETL：模拟 Kafka 消费
- **预期档位**: hard
- **考察维度**: 流计算 / 状态与背压
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 用 Write 产出 `clickstream.jsonl`：模拟点击流（`user_id/event/page/ts`）。python3 `stream_etl.py` 用 `asyncio.Queue` 模拟 Kafka 消费：producer 每秒发 1000 条、consumer 做 5 秒滚动窗口 UV/PV 统计。跑 30 秒期望 stdout 每 5 秒输出 `window=..s uv=.. pv=..`。
  2. 乱序事件：故意在 `clickstream.jsonl` 里造 10% 乱序（ts 比前一条早）。`stream_etl.py --watermark 2s` 用事件时间 + 水位线处理，期望乱序事件被正确归入窗口而非丢弃。Read 验证水位线逻辑。
  3. 精确去重：`stream_etl.py --dedup` 用 `set` 存已见 `event_id`。故意造 20% 重复 event_id，跑期望 `dedup_rate` 输出 ≈ 20%。
  4. 削峰与背压：producer 突发 10x 流量，`stream_etl.py --backpressure` 用 `Queue(maxsize=10000)` 限流。跑期望 `dropped` 计数 > 0 且 consumer 不 OOM。写 `streaming-notes.md` 总结窗口/水位线/背压三概念。

### AE08 元数据与数据血缘
- **预期档位**: medium
- **考察维度**: 元数据体系 / 血缘采集
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 写 3 个 SQL 文件：`create_users.sql`、`create_orders.sql`、`create_report.sql`（含 `INSERT INTO report SELECT ... FROM users JOIN orders`）。python3 `lineage.py` 解析 SQL 提取表级依赖，输出 `lineage.json`。期望 `report` 依赖 `users` 与 `orders`。Read 验证解析器处理了 JOIN。
  2. 故意在 `create_report.sql` 里加子查询 `(SELECT ... FROM payments)`，跑 `lineage.py` 期望 `payments` 也被识别为依赖。
  3. 生成 mermaid 图：`lineage.py --mermaid` 输出 `lineage.mmd`。Bash 用 `grep -c "report" lineage.mmd` 验证节点存在。
  4. 变更影响分析：`lineage.py --impact users` 输出所有下游表。期望 `report` 在列表中。故意把 `users` 改成不存在的表，跑期望报错。写 `lineage-notes.md` 总结动态 SQL/临时表血缘难点。

### AE09 数据脱敏与合规
- **预期档位**: medium
- **考察维度**: 脱敏技术 / 访问治理
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 写 `pii.csv`：含手机号、身份证、姓名、地址列。python3 `mask.py` 实现 4 种脱敏函数：手机号 `138****5678`、身份证 `110***********1234`、姓名 `张*三`、地址 `北京市朝阳区***`。跑 `python3 mask.py pii.csv masked.csv` 期望输出文件每列脱敏后格式正确。
  2. 故意把 `pii.csv` 改成含非法手机号（如 `123`），跑 `mask.py` 期望该行标记为 `INVALID` 而非崩溃。Read 验证校验分支。
  3. 保格式哈希：`mask.py --method hash` 用 `hashlib.sha256` 对手机号哈希，期望同一手机号哈希值一致（可逆性验证）。Bash 用 `awk -F, '{print $1}' masked.csv | sort | uniq -d` 验证无重复哈希碰撞（小样本）。
  4. 访问治理：写 `access_policy.yml` 定义开发/分析师/管理员三角色对 `pii.csv` 的列级权限。`policy_lint.py` 校验每个角色有明确列白名单；故意让分析师角色访问身份证列，跑 lint 期望报错。写 `pii-governance.md` 总结最小权限与审计日志。

### AE10 日志数据管道实战
- **预期档位**: medium
- **考察维度**: 端到端管道 / 排障
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 用 Write 产出 `app.log`：含多行异常堆栈（`Traceback ...` 后跟多行）、不同时区时间戳（UTC+8/UTC+0）、毫秒精度。python3 `log_pipeline.py` 解析：合并多行堆栈、归一化时间戳到 UTC、解析失败行进 `parse_failures.log`。跑 `python3 log_pipeline.py app.log parsed.jsonl` 期望 `parsed.jsonl` 每行合法 JSON。
  2. 故意把 `app.log` 里一行时间戳改成非法格式（如 `2026-13-01`），跑 `log_pipeline.py` 期望该行进 `parse_failures.log` 而非崩溃。Read 验证失败兜底字段。
  3. 入库：`log_pipeline.py --to-sqlite parsed.jsonl logs.db` 落 SQLite。Bash 用 `sqlite3 logs.db "SELECT COUNT(*) FROM logs;"` 验证行数 = `parsed.jsonl` 行数。
  4. 排障动线：写 `troubleshoot.md` 描述「日志延迟 30 分钟」故障的排查步骤（采集积压/入库吞吐/查询慢三层）。故意在 `log_pipeline.py` 里加 `sleep(0.01)` 模拟入库慢，跑 `--profile` 期望输出每阶段耗时。写 `log-pipeline-notes.md` 总结三层面排查顺序。