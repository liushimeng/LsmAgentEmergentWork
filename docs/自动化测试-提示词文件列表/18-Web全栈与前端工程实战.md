# 自动化测试提示词 — Web 全栈与前端工程实战（R01–R10）

> 使用说明见同目录 `README.md`。每条提示词在同一 Session 内按轮次顺序喂入。

## 维度说明

本维度考察 Agent **不靠 npm install / 真实浏览器**，用原生 node 跑纯 JS（`node --check` 语法校验 + 执行单测）、用 `curl` + `python3 -m http.server` 做 HTTP 响应断言，把前端逻辑/SSR/短链接服务/Express API/拖拽同步/Markdown 解析/WebSocket 握手等"前端与全栈 Web 工程"任务，做成可 Read 可 Bash 的真实可执行产物——多文件产物、链路闭环、Bash 断言验证。

---

### R01 用纯 JS 实现可拖拽任务看板（零依赖）
- **预期档位**: medium
- **考察维度**: 多轮产物依赖 / DOM 操作 / Bash 验证闭环
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/r01/` 下写一个零依赖 HTML+JS 看板 `kanban.html`：三列「Todo / Doing / Done」，初始各有 2 条任务卡片（数据直接写死数组），卡片用 `<div class="card">` 渲染、列用 `<div class="col">`。写完后 `node --check kanban.html` 不适用，但请用 `python3 -m http.server 8001 -d tmpPlan/agent-test/r01` 后台起服务并 `curl -sf http://127.0.0.1:8001/kanban.html | grep -c 'class="col"'` 必须恰好返回 `3`，否则报错重写。
  2. 在同一个 `kanban.html` 里加原生 `dragstart`/`dragover`/`drop` 事件：把一张卡片从 Todo 拖到 Doing，DOM 必须真的搬家（不是 clone）；写一个测试页 `test_move.html` 用 JSDOM 风格的 mock（`node` 脚本即可，无需 npm），调用 `dispatchEvent` 验证 move 后 Todo 列表少 1、Doing 多 1，断言失败请打印哪一步没触发。
  3. 把看板数据持久化到 `localStorage`：刷新页面后任务保留；再加一个 `export.json` 按钮，把当前数据 `JSON.stringify` 后下载为 `kanban-snapshot.json`，内容必须含三列数组。
  4. 写一个 `verify.js`（node 直接跑）拉起无头解析：解析 `kanban.html`、数三列卡片数量、检查 DOM 至少包含 `draggable="true"`，把结果写到 `tmpPlan/agent-test/r01/verify.out`，期望文件恰好 4 行（"列数/卡片数/draggable/has-storage"），少一行或多空行都算失败。

### R02 用纯 JS 写一个极简 Markdown 解析+预览
- **预期档位**: medium
- **考察维度**: 解析器实现 / Bash 产物落盘 / 多轮扩展
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/r02/` 下写 `md.js`：纯 JS 实现一个最小 Markdown→HTML 解析器（必须自己实现，不许 `require` 任何包），至少支持 `#/##/###` 标题、`**bold**`、`*italic*`、`` `code` ``、链接 `[text](url)`、行内代码转义；入口 `node md.js sample.md > out.html`，`sample.md` 里要塞 3 个 H1、2 个粗体、1 个代码片段、1 个链接。
  2. 跑 `node md.js sample.md > out.html` 然后 `grep -c '<h1>' out.html` 必须恰好返回 `3`，`grep -c '<strong>' out.html` 必须恰好返回 `2`，`grep -F '<code>' out.html | wc -l` 必须 `≥1`，任意一项不过就 fix `md.js` 重跑。
  3. 加 `md2html.html`：左 textarea、右 div 预览，`oninput` 触发解析更新预览；再写 `md.js` 加一个 `--stats` 模式，输出 JSON `{h1:N, h2:N, bold:N, code:N, links:N}` 到 stdout，用 `node md.js --stats sample.md` 跑一次 `jq '.h1'` 验证能 parse 且值 ≥3。
  4. 在 `verify.sh`（bash）里串起来：跑 `node md.js sample.md > out.html` → `grep` 断言 → `node md.js --stats sample.md > stats.json` → `jq -e '.h1 >= 3 and .bold >= 2'` → 全部通过输出 `OK`，否则 `FAIL` 并退出码 `1`；再 `bash verify.sh` 验证退出码恰为 `0`。

### R03 用 Node.js 内置 http 写短链接服务
- **预期档位**: medium
- **考察维度**: HTTP 服务 / Base62 编码 / 多轮增量
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/r03/` 下写 `shortener.js`：仅用 node 内置 `http` + `fs` 实现 3 个端点 `POST /create {url}` → 返回 Base62 短码、`GET /:code` → 302 重定向到原 URL、`GET /stats/:code` → 返回访问次数；存储用 `db.json` 文件持久化。
  2. 后台起服务 `node shortener.js &` 拿 `$!` 存 PID，写 `verify1.sh`：`curl -sf -XPOST http://127.0.0.1:8003/create -H 'content-type: application/json' -d '{"url":"https://example.com/a"}' | jq -r .code > code.txt` → `cat code.txt` 必须 6~8 位 Base62 → `curl -sfI http://127.0.0.1:8003/$(cat code.txt) | head -1` 必须 `302`；最后 `kill $PID`。
  3. 给 `shortener.js` 加访问统计：每次 `/create` 和 `/stats/:code` 都会改 `db.json` 的访问次数字段；写 `verify2.sh` 连发 5 次 GET 后查 `/stats/:code`，断言 `count >= 5`，且 `db.json` 文件可被 `jq .` 解析合法。
  4. 加一个并发安全测试 `verify3.sh`：用 `seq 1 50 | xargs -P10 -I{} curl -sf -XPOST ...` 一次性发 50 个不同 url，统计 `db.json` 里的记录条数必须恰为 `50`，且每个 code 都唯一（`jq -r '.codes|keys|length'` 与 `jq -r '.codes|keys|unique|length'` 相等），否则视作有竞态。

### R04 用 Node.js 写一个最小 SSR 模板引擎
- **预期档位**: medium
- **考察维度**: 模板渲染 / SEO meta / 端到端断言
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/r04/` 下写 `ssr.js`：node 内置 `http`，根据请求路径 `/products` 渲染一个 HTML 商品列表页，模板用最朴素字符串拼接实现（不要引第三方），数据从 `products.json` 读，渲染完返回 `text/html; charset=utf-8`。
  2. 后台 `node ssr.js &` 存 PID，`curl -sf http://127.0.0.1:8004/products | grep -F '<title>商品列表 - SSR Demo</title>'` 必须命中；`grep -F '<meta name="description"'` 也必须命中（title 与 description 都要在响应里）。
  3. 加 JSON-LD：`grep -c 'application/ld+json'` 必须 ≥1，且 JSON-LD 内容里 `@type` 必须是 `ItemList`、`itemListElement.length` 等于商品数（用 `curl ... | grep -oP 'ItemList' | wc -l` 至少 1，`grep -oP 'itemListElement' | wc -l` 等于 5）。
  4. 加分页：`/products?page=2` 时每页 2 条，第二页恰好 2 条商品 div（`grep -c 'class="product"'` 在 page=2 应为 2，page=1 应为 2，page=3 应为 1 ），不满足请改模板重跑，全部通过后 `kill $PID`。

### R05 用 Node.js 写一个 WebSocket 握手+广播最小示例
- **预期档位**: hard
- **考察维度**: WS 协议 / 多客户端同步 / 失败→修复闭环
- **工具链**: Write → Bash → Bash → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/r05/` 下写 `ws_server.js`：纯 node，不引第三方包，自己解析 HTTP Upgrade 握手（`Sec-WebSocket-Key` → `Sec-WebSocket-Accept` 用 SHA-1+base64 magic GUID），客户端连接后把收到的每条消息广播给所有连接（自己实现帧编解码，不许用 `ws` 包）。
  2. 写 `client.js`：同样纯 node 解析+封装 WS 帧，连上后每 200ms 发一条 `{seq:N,msg:"hi"}`，收到广播后打印。`node ws_server.js &` 拿 `$!`；并发 `node client.js & node client2.js &` 两个客户端各起 5 秒，预期服务端日志（写到 `server.log`）至少含 2 个 "client connected" 和 50 条 "broadcast"。
  3. 写一个故意破坏测试：把客户端发消息里的一个字节翻成 0xFF（非法 WS 帧），服务端必须断开该连接且**不能**让其他客户端崩；用 `client_bad.js` 跑 3 次，每次之后 `curl -sf http://127.0.0.1:8005/health`（自己加的 health 端点）必须仍返回 `OK` 且 `server.log` 里出现至少 1 次 "bad frame closed"。
  4. 如果上面"健康端点 503 / 服务端崩 / 帧解析错位"任一发生，请 Read `server.log` 定位修复后重跑直到 4 个验证全过（握手 echo OK、双客户端广播次数 ≥50、bad frame 不连坐、health 端点持续 200），最后 `kill $PID` 并把 `server.log` 留作产物。

### R06 用 Tailwind CDN + Alpine CDN 做 Landing Page（零本地依赖）
- **预期档位**: medium
- **考察维度**: 多轮产物依赖 / Bash 验证闭环 / Bash 断言
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/r06/` 下写 `index.html`：单文件 HTML，CDN 引入 Tailwind 与 Alpine（用 jsDelivr/unpkg 链接），页面分 Hero / Features / Pricing / FAQ / CTA 5 段，先用 `<script src="https://cdn.jsdelivr.net/...tailwindcss">` 引入并写死内容。
  2. 因 CDN 在沙盒里不可访问，请改成本地策略：把 Tailwind 与 Alpine 的内容**预下载**到 `vendor/` 失败时降级——本次脚本里允许**声明** CDN URL，但仍需在 `verify.sh` 中先 `curl -sI https://cdn.jsdelivr.net/npm/alpinejs@3 | head -1` 探活，若返回非 200 则改写 `index.html` 用 `<script>` 内联写一个最小 Alpine 替身（暴露 `x-data` 解析与 `x-show` 切换）。
  3. 加 Intersection Observer 让 Features 段滚到视口时加 `opacity-100`；写一个 `node check.js`（用 `node` 自带 `vm` + 字符串解析 HTML，不引第三方包）数 `<section>` 个数必须恰为 5，数 `data-aos` 或自定义 `data-reveal` 属性个数 ≥4。
  4. 把整页文字提一个 `content.json`，写 `node render.js` 从 JSON 生成 `index.html`（模板替换占位符 `{{hero_title}}` 等），运行 `node render.js && node check.js`，期望 `check.js` 输出 `sections=5 reveals>=4 links>=10` 三行全 OK，否则 fail 重写。

### R07 用 TypeScript 编译器（tsc 无 runtime 依赖）写类型安全 API 客户端
- **预期档位**: hard
- **考察维度**: Schema 生成 / 多文件产物 / Bash 闭环
- **工具链**: Write → Bash → Bash → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/r07/` 下用 `npx --yes typescript@5 tsc --init`（允许联网拉一次 tsc）生成 `tsconfig.json`，写 `api.ts`：手写 3 个 interface（User / Post / Comment），定义 `Client` 类的 `getPosts(): Promise<Post[]>` 等方法**仅签名不实现**（`throw new Error("TODO")`）。
  2. 跑 `npx tsc --noEmit api.ts` 必须 0 error（类型对、签名齐），把输出写到 `tsc.out`，期望文件大小 0 字节或仅提示（用 `wc -c tsc.out` 校验）；故意在 `api.ts` 加一行错类型赋值后再 `npx tsc --noEmit`，期望 `tsc.out` 含 "error TS" 字样（`grep -c 'error TS' tsc.out` ≥1）。
  3. 用 tsc 的 `tsc --declaration --emitDeclarationOnly` 生成 `api.d.ts`，再写一个 `consumer.ts` `import` 这个 `.d.ts` 编译，必须通过；删 `api.ts` 只留 `api.d.ts` 后 `tsc consumer.ts` 仍能编译（验证类型可单独分发）。
  4. 写一个 `verify.sh` 串联三步：(a) `tsc --noEmit api.ts` 0 错 → (b) `tsc --declaration` 生成 `.d.ts` → (c) `tsc consumer.ts` 通过 → 写 `OK` 到 `verify.out`；任何一步失败写 `FAIL: <step>` 并 exit 1。

### R08 用 Lighthouse CLI 子集：纯前端性能审计脚本
- **预期档位**: medium
- **考察维度**: 性能指标计算 / Bash 闭环 / 产物落盘
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/r08/` 下写 `slow.html`：一个故意拖慢的页面——同步在 `<script>` 里跑 50ms 计算（`for(let i=0;i<1e7;i++)`）、引一个 3MB 的 `<img src="big.svg">`（用脚本生成纯 SVG 当大文件，写到 `big.svg`）。
  2. 写 `audit.js`（node）：用 `http.createServer` 起 `python3 -m http.server` 替代品也行，自己用 `Date.now()` 在脚本里测三段耗时——`load HTML` / `parse + execute sync` / `load big.svg`（`fs.statSync('big.svg').size`），输出 `{fcp_ms, parse_ms, img_kb}` 到 `audit.json`。
  3. 优化 `slow.html`：把同步循环挪到 `setTimeout(0)` 分片、把 `<img>` 加 `loading="lazy"` + `decoding="async"`、在 `<head>` 加 `<link rel="preload">`；再跑 `node audit.js > audit_after.json`，断言 `audit_after.json` 里 `parse_ms` 比 `audit.json` 同字段小（用 `jq` 比较整数）。
  4. 写 `verify.sh`：对比前后两次指标，`jq -e '.fcp_ms < 100 and .parse_ms < 30 and .img_kb >= 3000'` 对原版、对优化版分别检查并打印 `before/after`，期望 after 的 `parse_ms` 比 before 至少小 50%（`jq '(.after.parse_ms * 2) < .before.parse_ms'`），最终写 `report.md` 含 before/after 表格。

### R09 用 Node.js 实现 Chrome 扩展 manifest 静态校验器
- **预期档位**: medium
- **考察维度**: JSON Schema / Bash 闭环 / 多轮产物
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/r09/` 下写 `manifest.json`（manifest v3，最简字段：`manifest_version:3`, `name`, `version:"0.1.0"`, `permissions:["activeTab"]`, `action.default_popup:"popup.html"`）和 `popup.html`（含一段 `<canvas id="c">`），再写 `verify_manifest.js` 用 node 解析 JSON 并校验必填字段，缺一个字段就退出码 1 并打印缺哪个。
  2. 故意把 `manifest.json` 的 `version` 改成 `"v1"`（非法）跑 `node verify_manifest.js`，期望退出码 `1` 且 `stderr` 含 "version must be semver"；改回 `"0.1.0"` 再跑，退出码必须 `0`。两次结果都 `tee` 到 `verify.out`。
  3. 写 `popup_check.js`：解析 `popup.html`，要求至少 1 个 `<canvas>`、至少 1 个 `<script>` 标签、`canvas` 有 `id` 属性；任意缺失退出码 `1`；跑通后写 `popup_check.out` 含 "OK canvas script id" 三行。
  4. 写 `verify.sh` 串联 `manifest` 与 `popup` 两步并把 `popup.html` 用 `python3 -m http.server` 起服务（后台 PID 存 `$!`），`curl -sfI` 必须 200、`curl -s` 拉到的 HTML 必须含 `<canvas id="c">`；最后 `kill $PID`，`verify.sh` 退出码 `0` 即视为整套合规。

### R10 用 Node.js 写一个 Module Federation 风格的"远程模块加载器"
- **预期档位**: hard
- **考察维度**: 多文件产物 / 沙箱执行 / Bash 闭环
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/r10/` 下建两个子目录 `shell/` 与 `remote-react/`：shell 里 `index.html` + `shell.js`，remote 里 `entry.js` 导出一个 `render(el)` 函数（纯 JS 实现一个迷你"组件"返回 `<h2>Hello from remote</h2>`）；shell 用 `<script type="module">` 通过 `fetch('/remote/entry.js')` 拉源码再 `new Function()` 执行（沙箱）。
  2. 后台 `python3 -m http.server 8010 -d tmpPlan/agent-test/r10` + `curl -sf http://127.0.0.1:8010/shell/index.html | grep -c '<script'` 必须 ≥2；`curl -sf http://127.0.0.1:8010/remote-react/entry.js | grep -F 'render'` 必须命中。
  3. 写 `verify_load.js`（node）：模拟浏览器，`vm` 模块 `runInNewContext` 加载 `entry.js`，拿到导出对象后调用 `render({})` 应返回含 `<h2>` 的字符串；再加载 shell 的 `shell.js`（同样 `vm`），断言它能把 remote 的 `render` 注入到 DOM（用一个假的 `document` mock 数 `appendChild` 调用次数 ≥1）。
  4. 写 `verify.sh`：起 http server、跑 `node verify_load.js`、把 `verify_load.js` 的 `console.log` 写进 `run.out`，期望恰好包含 "remote loaded"、"shell wired"、"render called" 三行；缺一行就 fail exit 1，最后 `kill $PID`。