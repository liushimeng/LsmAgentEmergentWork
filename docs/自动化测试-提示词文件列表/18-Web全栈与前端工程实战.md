# 自动化测试提示词 — Web 全栈与前端工程实战（R01–R10）

> 使用说明见同目录 `README.md`。每条提示词在同一 Session 内按轮次顺序喂入。

## 维度说明

本维度考察 Agent 在**前端框架、全栈 Web 开发、前端工程化、
浏览器 API、性能优化、微前端、Chrome 扩展**等"现代 Web 工程"场景中的表现，
与 E 维度（通用编码调试）和 Q 维度（游戏与趣味编程）互补：
E 考察"通用软件开发"，Q 考察"特定编程范式与垂直领域"，
本维度考察"浏览器端与全栈 Web 的工程实现"。
所有主题都以"动手写 Web 代码"为核心，强调前端工程实战能力。

---

### R01 用 React + TypeScript 开发任务看板（Kanban）
- **预期档位**: hard
- **考察维度**: 前端框架 + 状态管理 + 拖拽交互
- **对话脚本**:
  1. 我想用 React + TypeScript 开发一个任务看板（Kanban），包含"待办/进行中/完成"三列，先设计数据模型和组件结构。
  2. 实现核心功能：添加任务、编辑任务标题、删除任务，用 `useReducer` 管理状态。
  3. 加上拖拽功能：用 `@dnd-kit` 实现任务在列间拖动，拖动后自动更新状态。
  4. 讨论：如果要做"多人实时协作看板"（多用户同时操作），前端架构和状态管理要怎么调整？

### R02 用 Vue 3 + Pinia 开发在线 Markdown 编辑器
- **预期档位**: medium
- **考察维度**: Vue 生态 + Markdown 解析 + 实时预览
- **对话脚本**:
  1. 我想用 Vue 3 + Pinia 开发一个在线 Markdown 编辑器，左侧编辑区、右侧实时预览，先设计组件拆分方案。
  2. 实现核心功能：用 `marked` 或 `markdown-it` 解析 Markdown，支持代码高亮（`highlight.js`）。
  3. 加上本地存储：编辑内容自动保存到 `localStorage`，刷新页面不丢失。
  4. 讨论：如果要支持"导出 PDF"和"导出 HTML"功能，实现方案有什么不同？

### R03 用 Node.js + Express 开发短链接服务
- **预期档位**: hard
- **考察维度**: 后端 API + 数据库 + 重定向逻辑
- **对话脚本**:
  1. 我想用 Node.js + Express 开发一个短链接服务，先设计 API 接口（创建短链、访问短链、查看统计）。
  2. 实现核心功能：用 Base62 编码生成短码，用 SQLite 存储映射关系，访问短码时 302 重定向到原链接。
  3. 加上统计功能：记录每次访问的 IP、User-Agent、访问时间，提供 `/stats/:code` 接口查看。
  4. 讨论：短链接服务的性能瓶颈在哪？如果要支持每秒 10 万次重定向，架构要怎么优化？

### R04 用 Next.js 开发 SSR 电商商品列表页
- **预期档位**: hard
- **考察维度**: SSR/SSG + SEO + 服务端数据获取
- **对话脚本**:
  1. 我想用 Next.js 开发一个电商商品列表页，支持服务端渲染（SSR），先设计页面结构和数据流。
  2. 实现核心功能：用 `getServerSideProps` 获取商品数据，支持分页、排序（价格/销量）、分类筛选。
  3. 加上 SEO 优化：动态生成 `<title>`、`<meta description>`、结构化数据（JSON-LD）。
  4. 讨论：SSR 和 SSG 在电商场景下怎么选？哪些页面适合 SSG，哪些必须 SSR？

### R05 用 WebSocket 实现多人协作文档编辑器
- **预期档位**: hard
- **考察维度**: 实时通信 + 冲突解决 + OT/CRDT
- **对话脚本**:
  1. 我想用 WebSocket 实现一个多人协作文档编辑器，多个用户可以同时编辑同一篇文档，先设计通信协议。
  2. 实现核心功能：用 `ws` 库搭建 WebSocket 服务器，客户端用 `contenteditable` 实现富文本编辑，实时同步光标位置。
  3. 加上冲突处理：当两个用户同时编辑同一段落时，用 Operational Transformation (OT) 或 CRDT 解决冲突。
  4. 讨论：OT 和 CRDT 各有什么优缺点？在什么场景下选哪种？

### R06 用 Tailwind CSS + Alpine.js 开发 Landing Page
- **预期档位**: medium
- **考察维度**: 原子化 CSS + 轻量 JS + 响应式设计
- **对话脚本**:
  1. 我想用 Tailwind CSS + Alpine.js 开发一个 SaaS 产品的 Landing Page，先设计页面结构（Hero / 功能特性 / 定价 / FAQ / CTA）。
  2. 实现核心功能：用 Tailwind 的工具类完成响应式布局，用 Alpine.js 实现 FAQ 手风琴展开/收起、移动端菜单切换。
  3. 加上动画效果：滚动时元素渐入（Intersection Observer）、数字计数动画。
  4. 讨论：Tailwind 的原子化 CSS 和传统 CSS Modules / CSS-in-JS 相比，有什么优劣？

### R07 用 tRPC + Prisma 开发类型安全的 API
- **预期档位**: hard
- **考察维度**: 端到端类型安全 + ORM + 全栈 TypeScript
- **对话脚本**:
  1. 我想用 tRPC + Prisma 开发一个类型安全的博客 API，从前端到后端全程 TypeScript，先设计数据库 Schema。
  2. 实现核心功能：用 Prisma 定义 User / Post / Comment 模型，用 tRPC 定义 CRUD 路由，前端调用时自动获得类型提示。
  3. 加上认证：用 NextAuth.js 实现 JWT 登录，tRPC middleware 校验权限。
  4. 讨论：tRPC 和 REST/GraphQL 相比，类型安全的代价是什么？什么场景下不适合用 tRPC？

### R08 前端性能优化：Lighthouse 评分从 60 到 95
- **预期档位**: medium
- **考察维度**: 性能分析 + 优化策略 + 工具链
- **对话脚本**:
  1. 我的网站 Lighthouse 评分只有 60 分，请帮我分析可能的原因和优化方向（FCP / LCP / TBT / CLS 四个指标）。
  2. 针对"图片加载慢"的问题，给出具体优化方案：WebP 格式、懒加载、响应式图片 `srcset`、CDN 加速。
  3. 针对"JavaScript 执行时间长"的问题，给出优化方案：代码分割（dynamic import）、Tree Shaking、Web Worker 卸载计算。
  4. 讨论：Core Web Vitals 的阈值是多少？如何持续监控生产环境的性能指标？

### R09 开发 Chrome 扩展：网页截图与标注工具
- **预期档位**: medium
- **考察维度**: 浏览器扩展 API + 截图 + Canvas 绘图
- **对话脚本**:
  1. 我想开发一个 Chrome 扩展，功能是对网页截图并标注（画框、箭头、文字），先设计扩展的架构（popup / content script / background service worker）。
  2. 实现核心功能：用 `chrome.tabs.captureVisibleTab` 截取当前页面，在 popup 中用 Canvas 实现标注工具。
  3. 加上导出功能：将标注后的图片导出为 PNG 并下载，支持复制到剪贴板。
  4. 讨论：Chrome 扩展的 Manifest V3 和 V2 有什么重大变化？对扩展开发有什么影响？

### R10 用 Module Federation 实现微前端架构
- **预期档位**: hard
- **考察维度**: 微前端 + 模块联邦 + 独立部署
- **对话脚本**:
  1. 我想用 Webpack 5 的 Module Federation 实现微前端架构，主应用加载多个子应用，先设计整体架构方案。
  2. 实现核心功能：创建一个 Shell 主应用 + 两个子应用（React 子应用 + Vue 子应用），通过 Module Federation 运行时加载。
  3. 加上通信机制：主应用和子应用之间通过自定义事件或共享状态库（如 Redux）通信。
  4. 讨论：微前端的性能开销在哪？什么规模的项目才适合引入微前端？
