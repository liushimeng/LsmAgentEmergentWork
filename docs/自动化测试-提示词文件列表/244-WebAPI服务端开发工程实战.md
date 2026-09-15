# 244 Web API 服务端开发工程实战

> 编号段 API01–API10 · 聚焦「laew 从零搭建 Web API 服务(FastAPI / Express / Axum / Gin)」:RESTful 路由 / JWT 鉴权 / 请求校验 / CRUD / 分页排序过滤 / 中间件链 / 错误统一处理 / 数据库迁移 / Swagger 文档 / 集成测试 + mock 客户端 / Docker 容器化部署。

> 与现有维度互补说明:
> - `18-Web全栈与前端工程实战`(WEB 维)是**前端 + 全栈**;本文件是**纯后端 API 工程**。
> - `42-API设计艺术与错误语义`(DS 维)是**设计原则**;本文件是**API 服务编程实现**。
> - `61-企业业务系统开发实战`(ENT 维)是**业务系统整体**;本文件是**API 服务端专项**。
> - `22-分布式系统与微服务架构`(DIST 维)是**分布式理论**;本文件是**单服务 API 实现**。
> - `194-静态站点生成与个人发布工程`(SSG 维)是**静态站点**;本文件是**动态 API 服务**。
>
> **本文件独特主题**:FastAPI(Pydantic v2 + async) / Express(middleware 链) / Axum(tower + extractor) / Gin(中间件管道) 四栈选型 / JWT(access+refresh 双 token) / 请求校验(Schema + 业务规则) / CRUD 分页排序过滤 / 统一错误处理(error handler 中间件) / SQLAlchemy/GORM 迁移 / Swagger 自动生成 / 集成测试(TestClient/httpx) / Docker + docker-compose 部署。

---

### API01 用户注册登录 API

- **预期档位**: medium
- **考察维度**: FastAPI 项目结构 / Pydantic 校验 / JWT 签发 / bcrypt 哈希
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/api-auth/main.py`:FastAPI 用户认证 API:(a) `POST /auth/register`:接收 `{username, email, password}`,Pydantic 校验(email 格式 + 密码 ≥ 8 位),bcrypt 哈希后存库,返回 `{id, username, email}`;(b) `POST /auth/login`:校验用户名+密码,签发 JWT access_token(30min) + refresh_token(7d);(c) `GET /auth/me`:需 Authorization Bearer,返回当前用户信息;(d) `POST /auth/refresh`:用 refresh_token 换新 access_token;(e) 错误:用户名已存在 409 / 密码错误 401 / token 过期 401。
  2. Bash:`pip install fastapi uvicorn pyjwt bcrypt pydantic 2>/dev/null; python -c "import ast; ast.parse(open('tmpPlan/agent-test/api-auth/main.py').read()); print('OK')"`;`grep -q 'FastAPI' tmpPlan/agent-test/api-auth/main.py && grep -q 'jwt' tmpPlan/agent-test/api-auth/main.py && echo OK`。
  3. Read:加"登录限流(同 IP 5 次/分钟,超限返回 429)"中间件,Write 回文件。
  4. Bash:`grep -q '@app.post("/auth/register"' tmpPlan/agent-test/api-auth/main.py && grep -q '@app.post("/auth/login"' tmpPlan/agent-test/api-auth/main.py && grep -q 'bcrypt' tmpPlan/agent-test/api-auth/main.py && echo PASS`。

### API02 博客 CRUD API

- **预期档位**: medium
- **考察维度**: CRUD / 分页排序过滤 / 一对多关系 / 中间件鉴权
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/api-blog/main.py`:博客 CRUD API:(a) 数据模型:User(id,name) + Post(id,title,content,author_id,created_at,updated_at);(b) CRUD:`POST /posts`(需登录) / `GET /posts`(公开,支持 `?page=1&size=20&sort=created_at:desc&author_id=xxx`) / `GET /posts/{id}` / `PUT /posts/{id}`(仅作者可改) / `DELETE /posts/{id}`(仅作者可删);(c) 分页响应:`{items:[...], total, page, size, pages}`;(d) 关联:Post 响应嵌入 `author:{id,name}`;(e) 过滤:`?q=关键词` 搜索 title+content。
  2. Bash:`python -c "import ast; ast.parse(open('tmpPlan/agent-test/api-blog/main.py').read()); print('OK')"`;`grep -q 'page' tmpPlan/agent-test/api-blog/main.py && grep -q 'sort' tmpPlan/agent-test/api-blog/main.py && echo OK`。
  3. Read:加"评论功能"(Comment 模型 + `/posts/{id}/comments` CRUD),Write 回文件。
  4. Bash:`grep -q '@app.get("/posts"' tmpPlan/agent-test/api-blog/main.py && grep -q 'author_id' tmpPlan/agent-test/api-blog/main.py && grep -q 'Comment' tmpPlan/agent-test/api-blog/main.py && echo PASS`。

### API03 电商订单 API

- **预期档位**: hard
- **考察维度**: 事务处理 / 状态机 / 库存扣减 / 幂等性 / 金额计算
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/api-shop/main.py`:电商订单 API:(a) 模型:Product(id,name,price,stock) + Order(id,user_id,status,total) + OrderItem(order_id,product_id,qty,price);(b) 订单状态机:`PENDING → PAID → SHIPPED → DELIVERED → COMPLETED` + `PENDING → CANCELED`;(c) `POST /orders`:校验库存 → 扣减库存 → 创建订单(事务包裹,任一失败回滚)→ 返回订单号;(d) `POST /orders/{id}/pay`:模拟支付,状态 PAID,记录 paid_at;(e) 幂等:同 `Idempotency-Key` 头重复请求返回同一订单;(f) 金额:用 `Decimal` 避免浮点误差,总价 = Σ(qty × price)。
  2. Bash:`python -c "import ast; ast.parse(open('tmpPlan/agent-test/api-shop/main.py').read()); print('OK')"`;`grep -q 'Decimal' tmpPlan/agent-test/api-shop/main.py && grep -q 'status' tmpPlan/agent-test/api-shop/main.py && echo OK`。
  3. Read:加"退款接口"(退款金额校验 + 库存回滚 + 状态 CANCELED),Write 回文件。
  4. Bash:`grep -q 'PAID' tmpPlan/agent-test/api-shop/main.py && grep -q 'CANCELED' tmpPlan/agent-test/api-shop/main.py && grep -q 'Idempotency' tmpPlan/agent-test/api-shop/main.py && echo PASS`。

### API04 RESTful 文件管理 API

- **预期档位**: medium
- **考察维度**: 文件上传下载 / MIME 类型 / 分片上传 / 存储抽象
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/api-files/main.py`:文件管理 API:(a) `POST /files`:multipart/form-data 上传,校验 MIME(白名单:图片/pdf/doc,单文件 ≤10MB),存储到 `./uploads/{year}/{month}/{uuid}.{ext}`;(b) `GET /files`:分页列表,返回 `{id, filename, size, mime, uploaded_at, url}`;(c) `GET /files/{id}`:文件下载(`FileResponse` + 正确 Content-Type + Content-Disposition);(d) `DELETE /files/{id}`:删除 DB 记录 + 物理文件;(e) 存储抽象:定义 `StorageBackend` 接口,本地实现 `LocalStorage`,预留 `S3Storage` 占位。
  2. Bash:`python -c "import ast; ast.parse(open('tmpPlan/agent-test/api-files/main.py').read()); print('OK')"`;`grep -q 'UploadFile' tmpPlan/agent-test/api-files/main.py && grep -q 'mime' tmpPlan/agent-test/api-files/main.py && echo OK`。
  3. Read:加"图片缩略图生成(用 Pillow resize 到 200x200 缓存)",Write 回文件。
  4. Bash:`grep -q 'POST /files' tmpPlan/agent-test/api-files/main.py && grep -q 'DELETE /files' tmpPlan/agent-test/api-files/main.py && grep -q 'StorageBackend' tmpPlan/agent-test/api-files/main.py && echo PASS`。

### API05 GraphQL API 服务

- **预期档位**: hard
- **考察维度**: GraphQL Schema / Resolver / DataLoader / 订阅(Subscription) / Playground
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
 1. Write `tmpPlan/agent-test/api-graphql/main.py`:Ariadne + FastAPI GraphQL 服务:(a) Schema:`type User {id name posts:[Post]}; type Post {id title author:User comments:[Comment]}; type Query {user(id:ID!):User posts(page:Int):[Post]}; type Mutation {createPost(input:PostInput!):Post!};`;(b) Resolver:`resolve_user_posts` 用 DataLoader 批量加载(N+1 优化);(c) Mutation:输入校验 + 错误返回 `UserError {field message}`;(d) Subscription:`type Subscription {postAdded:Post!}` 用 `asyncio.Queue` 推送;(e) 集成 GraphQL Playground 在 `/graphql` 路径。
  2. Bash:`pip install ariadne 2>/dev/null; python -c "import ast; ast.parse(open('tmpPlan/agent-test/api-graphql/main.py').read()); print('OK')"`;`grep -q 'graphql' tmpPlan/agent-test/api-graphql/main.py && grep -q 'resolver' tmpPlan/agent-test/api-graphql/main.py && echo OK`。
  3. Read:加"文件上传 Mutation(multipart spec 实现)",Write 回文件。
  4. Bash:`grep -q 'Query' tmpPlan/agent-test/api-graphql/main.py && grep -q 'Mutation' tmpPlan/agent-test/api-graphql/main.py && grep -q 'Subscription' tmpPlan/agent-test/api-graphql/main.py && echo PASS`。

### API06 实时通信 WebSocket API

- **预期档位**: medium
- **考察维度**: WebSocket 握手 / 房间 / 广播 / 心跳 / 断线重连
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/api-ws/main.py`:FastAPI WebSocket 聊天服务:(a) `/ws/{room}`:WebSocket 端点,连接时加入房间,断开时移除;(b) 消息协议:`{"type":"join|leave|message|ping|pong", "payload":...}`;(c) 广播:房间内所有人收到 `{"type":"message", "from":user, "text":msg, "ts":timestamp}`;(d) 心跳:客户端每 30s ping,服务端 pong 回复,60s 无消息主动断开;(e) 房间列表:`GET /rooms` 返回 `{name, user_count, users:[...]}`。
  2. Bash:`python -c "import ast; ast.parse(open('tmpPlan/agent-test/api-ws/main.py').read()); print('OK')"`;`grep -q 'WebSocket' tmpPlan/agent-test/api-ws/main.py && grep -q 'broadcast' tmpPlan/agent-test/api-ws/main.py && echo OK`。
  3. Read:加"消息持久化(历史消息存入 SQLite,新连接返回最近 50 条)",Write 回文件。
  4. Bash:`grep -q 'websocket' tmpPlan/agent-test/api-ws/main.py && grep -q 'room' tmpPlan/agent-test/api-ws/main.py && grep -q 'pong' tmpPlan/agent-test/api-ws/main.py && echo PASS`。

### API07 API 网关与限流中间件

- **预期档位**: hard
- **考察维度**: 反向代理 / 限流 / 熔断 / 请求日志 / 请求 ID
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/api-gateway/main.py`:API 网关中间件:(a) 反向代理:路径 `/users/**` → 用户服务 `:8001`,`/orders/**` → 订单服务 `:8002`;(b) 限流:令牌桶算法,每 IP 100 请求/分钟,超限 429 + `Retry-After` 头;(c) 熔断:下游连续 5 次失败 → 开路(直接 503),30s后半开探测;(d) 请求 ID:每请求生成 `X-Request-ID`,全链路透传;(e) 日志:每请求记录 `{request_id, method, path, status, duration_ms, client_ip}`。
  2. Bash:`python -c "import ast; ast.parse(open('tmpPlan/agent-test/api-gateway/main.py').read()); print('OK')"`;`grep -q 'proxy' tmpPlan/agent-test/api-gateway/main.py && grep -q 'rate_limit' tmpPlan/agent-test/api-gateway/main.py && echo OK`。
  3. Read:加"请求/响应修改(注入 `X-Gateway-Version` 头 + 响应压缩 gzip)",Write 回文件。
  4. Bash:`grep -q 'circuit' tmpPlan/agent-test/api-gateway/main.py && grep -q 'Request-ID' tmpPlan/agent-test/api-gateway/main.py && grep -q '8001' tmpPlan/agent-test/api-gateway/main.py && echo PASS`。

### API08 OAuth2 第三方登录 API

- **预期档位**: hard
- **考察维度**: Authorization Code 流 / state 防 CSRF / token 换用户信息 / 绑定解绑
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/api-oauth/main.py`:OAuth2 第三方登录:(a) `/auth/{provider}/login`:跳转 GitHub/Google 授权页,携带 `state`(session 存) + `redirect_uri`;(b) `/auth/{provider}/callback`:校验 state → 用 code 换 access_token → 拉取用户信息(email, avatar, name);(c) 本地绑定:若 email 已存在 → 关联账号;不存在 → 创建新账号 + 第三方绑定记录;(d) `GET /auth/me/connections`:列出已绑定第三方;(e) `DELETE /auth/{provider}`:解绑(需保留至少一种登录方式)。
  2. Bash:`python -c "import ast; ast.parse(open('tmpPlan/agent-test/api-oauth/main.py').read()); print('OK')"`;`grep -q 'state' tmpPlan/agent-test/api-oauth/main.py && grep -q 'callback' tmpPlan/agent-test/api-oauth/main.py && echo OK`。
  3. Read:加"GitHub App 组织成员校验(仅允许特定组织成员登录)",Write 回文件。
  4. Bash:`grep -q 'github' tmpPlan/agent-test/api-oauth/main.py && grep -q 'google' tmpPlan/agent-test/api-oauth/main.py && grep -q 'CSRF' tmpPlan/agent-test/api-oauth/main.py && echo PASS`。

### API09 数据分析聚合 API

- **预期档位**: medium
- **考察维度**: 聚合查询 / 时间序列 / 缓存 / 异步任务 / CSV 导出
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/api-analytics/main.py`:数据分析聚合 API:(a) `POST /events`:埋点事件上报(`{event, properties, ts})`,写入 SQLite;(b) `GET /analytics/overview`:总事件数 / 独立用户数 / Top 10 事件类型;(c) `GET /analytics/trend?event=x&granularity=hour&days=7`:时间序列(按小时/天聚合);(d) 缓存:聚合结果 Redis/内存缓存 5 分钟,`Cache-Control: max-age=300`;(e) `GET /analytics/export?format=csv`:异步导出(后台任务 + 通知完成),返回下载链接。
  2. Bash:`python -c "import ast; ast.parse(open('tmpPlan/agent-test/api-analytics/main.py').read()); print('OK')"`;`grep -q 'GROUP BY' tmpPlan/agent-test/api-analytics/main.py && grep -q 'cache' tmpPlan/agent-test/api-analytics/main.py && echo OK`。
  3. Read:加"漏斗分析接口(多步骤转化率查询)",Write 回文件。
  4. Bash:`grep -q 'events' tmpPlan/agent-test/api-analytics/main.py && grep -q 'trend' tmpPlan/agent-test/api-analytics/main.py && grep -q 'export' tmpPlan/agent-test/api-analytics/main.py && echo PASS`。

### API10 完整 API 项目脚手架

- **预期档位**: hard
- **考察维度**: 项目结构 / 配置管理 / 日志 / 健康检查 / Docker / CI
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/api-scaffold/` 完整项目:(a) 结构:`app/{main.py, config.py, database.py, models/, routers/, schemas/, services/, middleware/, core/}` + `tests/{conftest.py, test_*.py}` + `alembic/` + `Dockerfile` + `docker-compose.yml` + `.github/workflows/ci.yml`;(b) config.py:`pydantic-settings BaseSettings` 从 `.env` 读配置(env: dev/test/prod);(c) 日志:`structlog` JSON 格式 + request_id 注入;(d) 健康检查:`GET /health` 返回 `{status, version, db_ok, uptime}`;(e) CI:lint(ruff) + test(pytest + coverage ≥ 80%) + build Docker。
  2. Bash:`find tmpPlan/agent-test/api-scaffold -type f | wc -l` 断言文件 ≥ 12 个;`grep -q 'FastAPI' tmpPlan/agent-test/api-scaffold/app/main.py && grep -q 'pydantic_settings' tmpPlan/agent-test/api-scaffold/app/config.py && echo OK`。
  3. Read:补"OpenAPI 自定义(servers 多环境 / tags 分组 / operation_id 规范)",Write 回文件。
  4. Bash:`test -f tmpPlan/agent-test/api-scaffold/Dockerfile && test -f tmpPlan/agent-test/api-scaffold/docker-compose.yml && test -f tmpPlan/agent-test/api-scaffold/.github/workflows/ci.yml && echo PASS`。
