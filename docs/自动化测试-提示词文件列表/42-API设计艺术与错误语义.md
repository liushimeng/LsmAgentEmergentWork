# 42 API 设计艺术与错误语义

> 编号段 AQ01–AQ10 · 聚焦 API 协议选型、错误码语义、分页策略、版本演进、Webhook 与限流工程

## 维度说明

本维度考察 Agent 在 **API 协议工程化** 上的动手能力：能起 mock 服务、构造请求、解析响应、断言语义层（状态码 / 错误体结构 / 分页游标 / 限流头 / 幂等键），并把"概念对比/选型矩阵"翻译成可执行脚本。
所有产物落到 `tmpPlan/agent-test/` 沙盒，不引入真网络依赖。
与同 README 的 E 维度（编码实现）互补：E 考察通用编码能力；AQ 聚焦"协议层"——请求/响应/语义/状态码/链路。

---

### AQ01 REST 资源建模与 URI 设计
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-09_15-DirectAnswer短路与input-token与QC产物可见性修复方案.md）
- **预期档位**: medium
- **考察维度**: REST 资源建模 + mock 服务端落盘
- **工具链**: Write → Bash → Bash → Read
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/aq01/` 写一个 python `http.server` 实现的 mock 图书管理 API，提供 `/books`、`/authors`、`/books/{id}/authors` 三个端点，GET 返回固定 JSON。
  2. 用 `curl` 跑一遍三个端点，把响应体分别存到 `books.json`、`author_42.json`、`book_1_authors.json`，断言每个文件都包含 `"id"` 字段。
  3. 在 `models.md` 里画资源关系图：图书—作者—借阅的 ER，并说明嵌套 URI `/books/{id}/authors` 何时用、何时退化到 query 参数。
  4. 用 `grep -c '"id"'` 数每个 JSON 的 id 字段出现次数，写一行 shell 验证"嵌套资源数 = 该书作者数"，确认 mock 数据自洽。

### AQ02 RPC vs REST vs GraphQL：写一遍同一份订单
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: hard
- **考察维度**: 协议对比 + 同一业务用三种风格各实现一遍
- **工具链**: Write → Write → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/aq02/` 写 `rest_order.py`（http.server + JSON）、`grpc_order.proto`（手写 Protobuf 消息体+服务定义）、`graphql_order.py`（http.server 处理 `query { order(id) { id, total, items { sku, qty } } }`），实现同一个"订单查询"接口。
  2. 用 `protoc --decode_raw` 或纯文本解析，对比 `rest_order.py` 的 JSON、`grpc_order.proto` 的二进制编码、`graphql_order.py` 的响应三者在「同一订单 id=1」下的字段数与可读性，把对比写进 `protocol_diff.md`。
  3. 写 `bench.py` 用 python `requests` 串行打 100 次本地 mock，统计三个端点的平均响应字节数（`len(resp.content)`）与耗时（`time.perf_counter`），断言 REST 字节数 > GraphQL 字节数 > gRPC 字节数。
  4. 写 `select.md` 总结：移动端 BFF、跨服务内部通信、第三方开放接口分别该选哪种协议，引用你自己跑出的字节数/耗时数据作论据。

### AQ03 错误码体系：本地构造并断言分类
- **预期档位**: medium
- **考察维度**: 错误建模 + 客户端可操作性
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/aq03/` 写一个 mock 错误响应生成器 `errors.py`：能根据 `?code=invalid_param|rate_limited|internal|not_found` 返回 4 种结构化错误体（code/message/reason/suggestion 四字段）。
  2. 用 `curl -o resp.json -w "%{http_code}\n" "http://127.0.0.1:18003/...?code=invalid_param"`，断言 HTTP 状态码=400 且响应 JSON 含 `"reason":"E1001"` 和 `"suggestion"` 字段。
  3. 写 `retry_decision.py` 实现重试判定：4xx 全部不重试（除 408/429），5xx 全部重试最多 3 次，给定一组 `(code, retry)` 测试用例断言通过。
  4. 把"哪些码应该重试"的判定表写到 `retry_policy.md`，至少 8 行（含 200/201/400/401/403/404/408/409/429/500/502/503），每一行用 markdown 表格，code 与 retry(Y/N) 列分明。

### AQ04 分页策略：游标的稳定性
- **预期档位**: medium
- **考察维度**: 大数据集分页 + 增量游标不漂移
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/aq04/` 写 `seed.py`，向 `tmpPlan/agent-test/aq04/items.db`（用 python 内置 sqlite3）灌 1000 条记录，主键 id 与 created_at 字段。
  2. 写 `cursor_api.py` 暴露 `/items?cursor=<created_at>&limit=50` 接口，按 created_at 升序翻页，响应里 next_cursor 用最后一条的 created_at。
  3. 用 `bash` 循环：先请求 cursor=0 取前 50 条，再请求上一步的 next_cursor 取下 50 条直到 1000 条拿完，断言所有返回的 id 集合等于 `{1..1000}`，无重复无丢失。
  4. 中途在数据库 `INSERT` 一条 created_at 处于第一页中间的记录，重跑翻页脚本，断言游标分页不出现"跳行"（offset 分页会跳，cursor 不会），用 `wc -l` 数两轮结果行数应当一致。

### AQ05 API 版本演进：Sunset Header 客户端感知
- **预期档位**: medium
- **考察维度**: 版本策略 + 弃用通知
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/aq05/` 写 `v1_server.py`，所有响应都带 `Sunset: Sat, 01 Mar 2026 00:00:00 GMT` + `Deprecation: true` + `Link: </v2/orders>; rel="successor-version"` 三个 header。
  2. 用 `curl -I` 抓 headers 到 `v1_headers.txt`，断言文件包含三行（Sunset/Deprecation/Link），并用 `awk` 解析出 Sunset 日期写到 `deprecation_date.txt`。
  3. 写 `client.py` 实现客户端感知：请求 v1 时打印 `WARNING: v1 已弃用，请于 <Sunset 日期> 前迁移到 v2`，迁移窗口 ≤30 天时升级为 `ERROR`。
  4. 用 `unittest`（python 内置）跑 3 个测试：未过期打印 WARNING、临期打印 ERROR、过期拒绝请求，断言三条用例全过。

### AQ06 Webhook 签名与退避：本地接收验证
- **预期档位**: hard
- **考察维度**: HMAC 签名验证 + 指数退避 + 重放防护
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/aq06/` 写 `sender.py`：用 `hmac.new(secret, body, sha256).hexdigest()` 算 X-Signature 头，X-Timestamp 用 `time.time()`，随机生成 3 个事件 POST 到本地 receiver。
  2. 写 `receiver.py`：用 http.server 处理 POST，校验 `abs(now - X-Timestamp) < 300`（防重放）与 HMAC 签名，把每个事件落盘到 `events.jsonl`（每行一个 JSON）。
  3. 用 bash 重放 sender 一次（同一时间戳+签名），断言 events.jsonl 行数=3（重放被拒），用 `wc -l events.jsonl` 验证。
  4. 写 `retry.py` 实现指数退避：base=1s, factor=2, jitter=±0.2, max=5 次，对失败投递用 `time.sleep` 实际等待，跑完打印总耗时与每次退避秒数到 `retry_log.txt`。

### AQ07 限流算法：本地实现四种并对比
- **预期档位**: hard
- **考察维度**: 限流算法 + 公平性
- **工具链**: Write → Bash → Bash → Read
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/aq07/` 写 `token_bucket.py`、`leaky_bucket.py`、`fixed_window.py`、`sliding_window.py` 四个独立模块，每个暴露 `try_acquire(now, key) -> bool`，参数：容量=5、窗口=1s、突发=2。
  2. 写 `simulate.py`：模拟 100 个请求在 1s 内按泊松到达（`numpy.random.poisson` 可用），依次过四道闸门，每道闸门打印 `allowed=X denied=Y` 到 `result_<algo>.txt`。
  3. 用 `awk` 汇总四个 result 文件：被拒数应当满足 fixed_window ≥ sliding_window ≈ token_bucket ≥ leaky_bucket（边界突发场景），把对比写进 `compare.md`。
  4. 修一个 bug：token_bucket 在 capacity=0 时应当 deny，写测试 `assert token_bucket.try_acquire(0, 0, 0) == False`，跑通。

### AQ08 幂等性：Idempotency-Key 端到端验证
- **预期档位**: hard
- **考察维度**: 幂等设计 + 二次提交防重
- **工具链**: Write → Bash → Bash → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/aq08/` 写 `order_api.py`：POST /orders 接受 `Idempotency-Key` header，把 key 存到 `tmpPlan/agent-test/aq08/idem.db`（python 内置 sqlite3），已存在则返回上次结果而不创建新订单。
  2. 用 curl 带同一 key 连续 POST 3 次（订单体故意不同），断言第 2/3 次响应体的 `order_id` 与第 1 次完全相同；用 `sqlite3 idem.db 'select count(*) from orders'` 断言行数=1。
  3. 不带 key 的请求应当每次都新建订单，POST 2 次不带 key 断言行数=2（=1+1）。
  4. 写 `expire.py` 模拟 24h 过期：直接 SQL 删旧 key，再 POST 同一 key 断言创建了新订单（行数 +1），整个流程 sql 查询结果用 `tee` 落到 `idempotency_log.txt`。

### AQ09 限流降级：429 + Retry-After 决策树
- **预期档位**: medium
- **考察维度**: DX + 限流响应头解析
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/aq09/` 写 `rate_limited_api.py`：每窗口只允许 3 次请求，超出返回 429 + `Retry-After: 2` + JSON `{"degrade":"fallback"}`。
  2. 写 `client.py` 解析 429：用 `int(headers['Retry-After'])` 决定 sleep 秒数，被限流时打印 `提示：触发降级，回退到本地缓存`。
  3. 跑 `client.py` 连续请求 5 次，把每次的 status、retry-after、提示行落到 `degrade_log.txt`，断言第 4/5 行包含 `429` 与 `降级`。
  4. 写 `policy.md`：把"静默丢弃 / 返回缓存 / 切换备用服务"三种降级策略整理成一张表（触发条件/优点/缺点/适用场景），每行 4 列。

### AQ10 协议抓包：本地分析一次完整 HTTP 往返
- **预期档位**: hard
- **考察维度**: 抓包分析 + 协议层观察
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/aq10/` 写 `echo_api.py`：返回 `X-Request-Id`、`X-RateLimit-Remaining`、`Content-Type: application/json` 三种典型 header。
  2. 用 `curl -v` 请求 2 次（一次带 Accept 头，一次不带），把完整 stderr+stdout 抓包到 `trace.log`。
  3. 用 `grep -E '^(>|<|\*)' trace.log` 提取请求/响应行，断言 `> X-Request-Id` 出现 1 次、`< HTTP/1.0 200` 出现 2 次、`< X-RateLimit-Remaining: 99` 出现至少 1 次。
  4. 写 `protocol_notes.md`：用上面抓包数据说明 HTTP 方法、header 顺序、keep-alive 行为，至少 6 行并引用 trace.log 的真实行号。
