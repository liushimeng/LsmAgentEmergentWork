# 自动化测试 — 提示词文件列表

## 是什么

面向 laew（LsmAgentEmergentWork）的**自动化测试提示词集合**，共 **77 维度 × 10 个 = 770 个多轮对话提示词**。
每个提示词设计为 3~5 轮追问，覆盖任务处理、知识问答、工作日常、电脑使用、软件使用、
编码 Coding、界面设计、文件整理处理、LLM 安全攻防、异步编程、软件供应链、技术写作、DDD、
手写 Unix 命令、经典游戏复刻、从零造轮子、小众语言、编辑器插件、业务系统、桌面小工具、
疑难 Bug 攻坚、文件格式解析、编程冷知识、跨平台电脑使用、设计系统、量子计算、机器人 ROS2、
3D 建模与几何、WebAssembly 边缘、密码学与隐私计算、并行 GPU、函数式编程、编译器后端 LLVM、
AR/VR 空间计算、语音交互与对话式 UI、基础设施可观测性与 SRE
等多个角度，**无重复主题**。

## 用途

1. **人工/自动化回归**：逐条投喂给 laew，验证三档分类（simple/medium/hard）是否命中预期、
   多 Agent 编排（Yolo → Plan/Main → SubAgent → Quality-Check → SessionContext）是否顺畅。
2. **多轮上下文验证**：每个提示词的后几轮均显式引用前轮结论（"基于上面的讨论…"），
   用于检验 Session 主上下文连续性与 SessionContext 历史摘要注入。
3. **工具覆盖压测**：提示词设计时刻意覆盖 Bash / Read / Write 三工具与
   `plans/` 产物、`session_memory` 写入等工程特性。

## 目录结构

```
docs/自动化测试-提示词文件列表/
  README.md                              ← 本文件（索引与使用说明）
  01-任务处理与工作日常.md                ← 任务处理 / 工作日常 / 项目管理 / 效率提升
  02-知识问答与通用能力.md                ← 知识问答 / 概念解释 / 跨学科
  03-电脑使用与系统管理.md                ← 电脑使用 / 系统管理 / 运维基础
  04-软件与命令行工具使用.md              ← 软件使用 / 命令行工具 / 开发工具链
  05-编码Coding与调试修复.md              ← 编码 / 调试 / 代码审查 / 重构
  06-界面设计与用户体验.md                ← 界面设计 / TUI / CLI / 交互设计
  07-文件整理处理与数据加工.md            ← 文件处理 / 数据加工 / 批量操作
  08-文档写作与方案规划.md                ← 文档写作 / 方案设计 / 技术规划
  09-数据库与存储技术.md                  ← 数据库 / SQLite / 存储引擎
  10-网络协议与安全基础.md                ← 网络协议 / 安全基础 / Web 技术
  11-综合场景与跨领域实战.md              ← 综合场景 / 跨领域 / 复杂任务
  12-laew元任务与工程特性.md              ← laew 元任务 / 工程特性 / 自测试
  13-数据分析与统计推理.md                ← 数据分析 / 统计推理 / 指标体系 / SQL 分析
  14-学习成长与职业发展.md                ← 求职面试 / 学习路线 / 职业转型 / 知识管理
  15-生活场景与效率软件.md                ← 生活电脑 / 效率软件 / 数字生活整理
  16-多轮对话鲁棒性与边界测试.md          ← 对话鲁棒性 / 边界条件 / 注入防御
  17-游戏与趣味编程开发.md                ← 游戏开发 / 编译器 / 移动端 / 嵌入式 / 函数式 / 宏 / 设计模式 / WASM / 区块链 / IoT
  18-Web全栈与前端工程实战.md             ← 前端框架 / 全栈Web / 前端工程化 / 浏览器API / 性能优化 / 微前端 / Chrome扩展
  19-AI工程与LLM应用实战.md               ← LLM API / RAG / Prompt工程 / Agent框架 / 模型微调 / 多模态 / 向量检索 / LLM网关 / Eval
  20-测试自动化与质量工程.md              ← 测试框架 / Mock / TDD / 契约测试 / 混沌工程 / 覆盖率 / 快照测试 / 质量门禁
  21-系统编程与底层开发.md                ← 内存管理 / 线程同步 / 信号处理 / 文件系统 / 系统调用 / mmap / 锁-free / 内核模块 / 网络编程
  22-分布式系统与微服务架构.md            ← 服务发现 / 负载均衡 / 分布式事务 / 消息队列 / Raft / 分布式缓存 / 服务网格 / 容错 / 追踪 / Event Sourcing
  23-自然语言处理与文本工程.md            ← 中文分词 / 词性标注 / NER / 文本分类 / 情感分析 / 文本摘要 / 文本相似度 / 机器翻译 / QA系统 / 语言模型推理
  24-计算机视觉与图像工程.md              ← 图像滤波 / 边缘检测 / 特征提取 / 目标检测 / 图像分割 / OCR / 人脸识别 / 图像增强 / 视频处理
  25-桌面应用与跨平台开发.md              ← Electron / Tauri / GUI布局 / 事件驱动 / 多线程UI / 系统托盘 / 本地存储 / 自动更新 / 无障碍 / 跨平台构建
  26-编程语言全景与多语言实战.md          ← Go / Java / Kotlin / Swift / Ruby / PHP / Lua / Elixir / Zig / Haskell 每条一种语言
  27-算法竞赛与数据结构进阶.md            ← 背包DP / 最短路 / 网络流 / KMP与AC自动机 / 线段树 / 树状数组 / 并查集 / 计算几何 / 数论 / 博弈论
  28-游戏引擎与图形编程进阶.md            ← 平台物理手感 / 固定时间步 / 碰撞检测CCD / A*寻路 / 行为树AI / 帧同步 / 渲染管线着色器 / 程序化生成 / 粒子系统 / 存档系统
  29-云原生与DevOps工程实战.md            ← Dockerfile优化 / compose编排 / K8s部署排障 / CI-CD / GitOps / Terraform / 监控告警 / 日志管道 / 发布策略
  30-逆向工程与二进制安全分析.md          ← ELF结构 / 反汇编阅读 / gdb调试 / 栈溢出与ROP(本地靶场) / Frida插桩 / 反混淆 / 样本防御分析 / 软件加固 / CVE复现方法论
  31-数据工程与ETL管线实战.md             ← ETL管线 / 数仓分层 / CDC增量同步 / 数据质量 / 调度编排 / Parquet列存 / 流式ETL / 元数据血缘 / 数据脱敏 / 日志管道
  32-移动应用与跨平台开发实战.md          ← React Native / Flutter / SwiftUI / Jetpack Compose / 小程序 / 移动端性能 / 移动端安全 / 热更新
  33-嵌入式系统与物联网开发.md            ← FreeRTOS / Arduino / ESP32 / 嵌入式C / MQTT / 边缘计算 / 嵌入式Linux / 实时系统 / CAN总线 / 物联网平台
  34-区块链与智能合约开发.md              ← Solidity / ERC-20 / DeFi / NFT / Web3 / 链上分析 / Layer2 / ZK / DAO / 跨链桥
  35-音视频处理与流媒体工程.md            ← FFmpeg / H.264 / 音频处理 / HLS / WebRTC / 播放器 / 直播 / ASR / 视频分析 / 性能优化
  36-自动化办公与脚本工程.md              ← Excel / 邮件 / PDF / 爬虫 / CLI工具 / Shell脚本 / UI自动化 / 数据处理 / 定时任务 / 效率工具
  37-产品需求分析与原型设计.md            ← 需求访谈 / 用户故事 / 竞品分析 / 信息架构 / 原型设计 / 用户流程 / 指标体系 / 需求评审 / 路线图 / 设计思维
  38-代码审查与团队协作工程.md            ← Git工作流 / PR写作 / Code Review / 合并冲突 / 提交规范 / 代码风格 / 结对编程 / 技术债务 / 工程文化 / 开源协作
  39-性能调优与Profiling实战.md           ← 性能指标 / 火焰图 / 内存Profiling / IO Profiling / 数据库优化 / Web性能 / 并发性能 / 分布式追踪 / 调优方法论 / 实战案例
  40-正则表达式与文本处理工程.md          ← 正则语法 / 零宽断言 / 日志解析 / 文本清洗 / 词法分析 / 语法分析 / grep-sed-awk / 全文搜索 / NLP预处理 / 文本分类
  41-数学建模与科学计算.md                ← 线性代数 / 优化 / 微分方程 / 贝叶斯 / 蒙特卡洛 / 信号处理 / 数据拟合 / 运筹学 / 数值仿真 / 科学计算工程
  42-API设计艺术与错误语义.md              ← REST/gRPC/GraphQL / 错误码体系 / 分页策略 / API版本演进 / Webhook / 限流 / 幂等 / API网关
  43-国际化工程与RTL布局.md                ← i18n/l10n / ICU MessageFormat / RTL双向文本 / 字体回退 / 时区日历 / 多语言搜索 / 本地化运营
  44-无障碍a11y与包容性设计.md              ← WCAG / ARIA / 屏幕阅读器 / 键盘导航 / 色彩对比 / 替代文本 / 自动化检测 / 法规合规
  45-代码考古与遗留系统迁移.md              ← 遗留代码理解 / 重构策略 / 绞杀者模式 / 数据迁移 / 框架升级 / 单体拆分 / 死代码消除
  46-形式化方法与软件验证.md                ← TLA+ / 模型检测 / Coq+Lean+Isabelle / 属性测试 / 静态分析 / 符号执行 / Dafny+SPARK / 工程化ROI
  47-安全工程与渗透测试.md                  ← 攻击面/STRIDE/Web渗透/SQLi+XSS+SSRF/认证授权漏洞/密码学/漏洞赏金/容器安全/CTF/SDL
  48-操作系统与内核原理.md                  ← 内核架构/进程线程/调度/CFS/内存管理/IPC/同步原语/文件系统/虚拟化容器/eBPF
  49-编译原理与DSL实战.md                   ← 编译流程/词法分析/语法分析/AST/IR+SSA/代码生成/优化/JIT/DSL设计/Mini-Lisp实战
  50-软件架构模式与设计系统.md              ← 架构全景/DDD/CQRS+ES/整洁架构/事件驱动/插件化/API选型/ADR/可演化/ATAM评估
  51-LLM提示词注入与AI红队.md                ← 提示词注入/越狱/PII泄露/Agent劫持/RAG投毒/模型供应链/水印溯源/合规
  52-异步编程范式专题.md                    ← 异步模型演进/Rust tokio/Go goroutine/Node事件循环/Java虚拟线程/Kotlin协程/Reactive/取消传播/反模式
  53-软件供应链与SBOM工程.md                ← 供应链攻击/SBOM标准/CVE监控/制品签名/SLSA/License合规/私有仓库/可复现构建/应急响应
  54-技术写作与知识沉淀.md                  ← 金字塔原理/ADR/API文档/Runbook/Engineering Wiki/RFC/技术博客/演讲/注释即文档/工具链
  55-领域驱动设计DDD专题.md                  ← 通用语言/限界上下文/Context Map/聚合根/值对象/领域服务/领域事件/仓储/事件风暴/团队转型
  56-从零手写Unix命令行工具集.md             ← 手写 cat/ls/wc/grep/find/tree/du/xargs/diff/watch，每条复刻一个经典命令
  57-经典小游戏复刻与游戏编程实战.md         ← 俄罗斯方块/扫雷/2048/推箱子/五子棋AI/生命游戏/打砖块/MUD/数独/塔防
  58-从零造轮子经典系统复刻.md               ← 迷你 Redis/Git/Docker/HTTP服务器/JSON解析器/正则引擎/虚拟机/数据库/协程/编辑器
  59-小众与新兴编程语言巡礼.md               ← Nim/Crystal/Julia/R/Scala3/OCaml+F#/Erlang/Racket/Fortran+COBOL/Odin+V+Gleam
  60-编辑器插件开发与开发环境定制.md         ← VSCode扩展/LSP接入/Neovim配置/JetBrains插件/自制LSP/dotfiles/Shell定制/字体/DevContainer/键位
  61-企业业务系统开发实战.md                 ← 进销存/工单/OA审批流/RBAC/报表引擎/多租户SaaS/CRM/考勤排班/低代码表单/对账结算
  62-桌面小工具软件开发实战.md               ← 截图标注/剪贴板管理/番茄钟/批量重命名GUI/看图/音乐播放器/密码管理器/串口助手/笔记本/监控挂件
  63-疑难Bug攻坚与故障排查实录.md            ← 段错误/内存泄漏/死锁/数据竞争/生产OOM/CPU打满/fd泄漏/时钟时区/乱码/变更归因
  64-文件格式解析与序列化编程.md             ← PNG/ZIP/WAV/xlsx/Protobuf/CBOR+MessagePack/SQLite文件格式/CSV方言/YAML+TOML/自定义二进制协议
  65-编程冷知识与为什么十问.md               ← 浮点/UTF-8/时间/大小端/随机数与UUID/排序稳定性/下标从0/null/删文件/"我机器上能跑"
  66-跨平台电脑使用与虚拟化实战.md           ← PowerShell/WSL2/macOS/虚拟机/远程桌面/NAS共享/外设排障/分区与数据恢复/系统迁移/双系统引导
  67-设计系统与组件库工程实战.md             ← Design Token/组件API/暗色主题/图标系统/栅格响应式/排版系统/动效规范/表单规范/图表规范/组件库治理
  68-量子计算编程与量子算法.md               ← 量子比特与布洛赫球/量子门与电路/量子纠缠/Shor算法/Grover搜索/Qiskit/变分算法VQE-QAOA/量子纠错表面码/NISQ/量子-经典混合
  69-机器人与ROS2工程.md                     ← ROS2节点话题服务动作/URDF与tf2/Navigation2/MoveIt2/Gazebo仿真/BT.CPP行为树/Lifecycle多机/ros2_control/Micro-ROS/排障实战
  70-3D建模与几何处理实战.md                 ← 半边网格/曲面细分/CSG布尔与OpenCASCADE/OpenGL-Vulkan管线/PBR glTF2.0/几何深度学习/参数化生成/多视图几何/3D打印修复/刚体碰撞
  71-WebAssembly深度与边缘计算.md            ← Wasm字节码与S-表达式/wasmtime JIT-AOT/WASI/Component Model与WIT/Edge Runtime-Workers/Capability安全/性能SIMD多线程/Rust工具链/Wasm vs容器/WASI Preview2
  72-密码学与隐私计算.md                     ← AES-GCM-ChaCha20/RSA-ECC-Ed25519/哈希与Argon2/TLS1.3/零知识证明zk-SNARK-STARK/同态加密BFV-CKKS/安全多方计算/PKI证书/后量子密码Kyber-Dilithium/Rust crypto生态
  73-并行与GPU计算.md                        ← GPU SM-Warp架构/CUDA编程模型/内存层次与合并访问/SIMD-AVX512/rayon-crossbeam/wgpu原生GPU/NCCL多卡通信/Roofline模型/并行算法/CUDA profiling
  74-函数式编程进阶.md                       ← λ演算与Curry-Howard/Haskell类型类-Functor-Applicative-Monad/Monad Transformer/解析器组合子-nom/依赖类型Idris-Agda/线性类型与Rust ownership/GADT/惰性求值/代数效应/FP在Rust实战
  75-编译器后端与LLVM优化.md                 ← SSA与φ函数/LLVM IR结构/Pass Manager/内联成本模型/循环优化LICM向量化/寄存器分配/指令选择GlobalISel/Target描述/LTO-ThinLTO/MLIR Dialect/JIT-LLJIT
  76-ARVR与空间计算.md                       ← OpenXR生态/SLAM空间定位/渲染ATW-ASW-SSW/多维交互-手柄手势眼动语音/空间锚点持久化/空间音频HRTF/三维UI舒适UX/MR场景理解遮挡/注视点渲染与热节流/WebXR与three.js
  77-语音交互与对话式UI.md                   ← 语音全链路ASR-对话管理-TTS/Whisper-Paraformer流式ASR/VITS-Bert-VITS流式TTS/VAD端点检测/唤醒词Porcupine/对话管理FSM-LLM/语音UX打断修复/SSML情感语音/Voice Agent全双工AEC/多模态Copilot Voice
  78-基础设施可观测性与SRE.md                ← 三大支柱Logs-Metrics-Traces/OpenTelemetry工程化/SLO-SLI错误预算Burn Rate/Incident Management无责复盘/告警疲劳治理/容量规划Little's Law/混沌工程ChaosMesh/Continuous Profiling eBPF/SRE平台工程/Grafana技术栈一站式
```

## 编号规则

| 编号段 | 维度 | 主要考察 | 预期档位分布 |
|--------|------|----------|--------------|
| A01–A10 | 任务处理与工作日常 | 任务拆解、流程优化、职场场景 | simple~medium |
| B01–B10 | 知识问答与通用能力 | 概念解释、多轮追问、跨学科 | 全 simple |
| C01–C10 | 电脑使用与系统管理 | 系统命令、排查、运维基础 | simple~medium |
| D01–D10 | 软件与命令行工具使用 | git/tmux/docker/jq 等工具实操 | simple~medium |
| E01–E10 | 编码 Coding 与调试修复 | 写代码、修 bug、代码审查 | medium~hard |
| F01–F10 | 界面设计与用户体验 | TUI/CLI 设计、交互、配色 | medium~hard |
| G01–G10 | 文件整理处理与数据加工 | 批量处理、格式转换、数据清洗 | medium 为主 |
| H01–H10 | 文档写作与方案规划 | 长文生成、方案设计、技术文档 | medium~hard |
| I01–I10 | 数据库与存储技术 | SQLite、数据库设计、存储优化 | simple~medium |
| J01–J10 | 网络协议与安全基础 | HTTP/TCP/安全基础、Web 技术 | simple~medium |
| K01–K10 | 综合场景与跨领域实战 | 复杂任务、跨领域、综合应用 | hard 为主 |
| L01–L10 | laew 元任务与工程特性 | 用 laew 测 laew、工程特性验证 | 全档位混合 |
| M01–M10 | 数据分析与统计推理 | A/B 测试、因果推断、指标体系、SQL 分析 | simple~medium |
| N01–N10 | 学习成长与职业发展 | 简历/面试、学习路线、转型规划、知识管理 | simple~medium |
| O01–O10 | 生活场景与效率软件 | 家庭网络、Excel/PDF、媒体库、收件箱整理 | simple~medium |
| P01–P10 | 多轮对话鲁棒性与边界测试 | 改主意/矛盾指令/长文埋点/注入防御等对抗场景 | 全档位混合 |
| Q01–Q10 | 游戏与趣味编程开发 | 游戏开发/编译器/移动端/嵌入式/函数式/宏/设计模式/WASM/区块链/IoT | hard 为主 |
| R01–R10 | Web 全栈与前端工程实战 | 前端框架/全栈Web/浏览器API/性能优化/微前端/Chrome扩展/SSR/端到端类型安全 | medium~hard |
| S01–S10 | AI 工程与 LLM 应用实战 | LLM API/RAG/Prompt工程/Agent框架/微调/多模态/Token成本/向量检索/LLM网关/Eval | medium~hard |
| T01–T10 | 测试自动化与质量工程 | 测试框架/Mock/TDD/契约测试/混沌工程/覆盖率/快照测试/质量门禁 | medium~hard |
| U01–U10 | 系统编程与底层开发 | 内存管理/线程同步/信号处理/文件系统/系统调用/mmap/锁-free/内核模块/网络编程 | medium~hard |
| V01–V10 | 分布式系统与微服务架构 | 服务发现/负载均衡/分布式事务/消息队列/Raft/分布式缓存/服务网格/容错/追踪/Event Sourcing | medium~hard |
| W01–W10 | 自然语言处理与文本工程 | 中文分词/词性标注/NER/文本分类/情感分析/文本摘要/文本相似度/机器翻译/QA系统/语言模型推理 | medium~hard |
| X01–X10 | 计算机视觉与图像工程 | 图像滤波/边缘检测/特征提取/目标检测/图像分割/OCR/人脸识别/图像增强/视频处理 | medium~hard |
| Y01–Y10 | 桌面应用与跨平台开发 | Electron/Tauri/GUI布局/事件驱动/多线程UI/系统托盘/本地存储/自动更新/无障碍/跨平台构建 | medium~hard |
| Z01–Z10 | 编程语言全景与多语言实战 | Go/Java/Kotlin/Swift/Ruby/PHP/Lua/Elixir/Zig/Haskell 语言特色特性与跨语言迁移 | medium~hard |
| AA01–AA10 | 算法竞赛与数据结构进阶 | 背包DP/最短路/网络流/KMP/线段树/树状数组/并查集/计算几何/数论/博弈论 | medium~hard |
| AB01–AB10 | 游戏引擎与图形编程进阶 | 平台物理/固定时间步/碰撞CCD/A*寻路/行为树/帧同步/着色器/程序化生成/粒子/存档 | medium~hard |
| AC01–AC10 | 云原生与 DevOps 工程实战 | Dockerfile优化/compose/K8s部署排障/CI-CD/GitOps/Terraform/监控告警/日志管道/发布策略 | medium 为主 |
| AD01–AD10 | 逆向工程与二进制安全分析 | ELF/反汇编/gdb/栈溢出与ROP(本地靶场)/Frida/反混淆/样本防御分析/软件加固/CVE复现 | medium~hard |
| AE01–AE10 | 数据工程与 ETL 管线实战 | ETL/数仓分层/CDC/数据质量/调度编排/Parquet/流式ETL/元数据血缘/脱敏/日志管道 | medium 为主 |
| AF01–AF10 | 移动应用与跨平台开发实战 | React Native/Flutter/SwiftUI/Compose/小程序/移动性能/安全/热更新/网络/Flutter游戏 | medium~hard |
| AH01–AH10 | 嵌入式系统与物联网开发 | FreeRTOS/Arduino/ESP32/嵌入式C/MQTT/边缘计算/嵌入式Linux/实时系统/CAN/物联网平台 | medium~hard |
| AI01–AI10 | 区块链与智能合约开发 | Solidity/ERC-20/DeFi/NFT/Web3/链上分析/Layer2/ZK/DAO/跨链桥 | medium~hard |
| AJ01–AJ10 | 音视频处理与流媒体工程 | FFmpeg/H.264/音频处理/HLS/WebRTC/播放器/直播/ASR/视频分析/性能优化 | medium~hard |
| AK01–AK10 | 自动化办公与脚本工程 | Excel/邮件/PDF/爬虫/CLI工具/Shell脚本/UI自动化/数据处理/定时任务/效率工具 | simple~medium |
| AL01–AL10 | 产品需求分析与原型设计 | 需求访谈/用户故事/竞品分析/信息架构/原型设计/用户流程/指标体系/需求评审/路线图/设计思维 | simple~medium |
| AM01–AM10 | 代码审查与团队协作工程 | Git工作流/PR写作/Code Review/合并冲突/提交规范/代码风格/结对编程/技术债务/工程文化/开源协作 | simple~medium |
| AN01–AN10 | 性能调优与 Profiling 实战 | 性能指标/火焰图/内存Profiling/IO Profiling/数据库优化/Web性能/并发性能/分布式追踪/调优方法论/实战案例 | medium~hard |
| AO01–AO10 | 正则表达式与文本处理工程 | 正则语法/零宽断言/日志解析/文本清洗/词法分析/语法分析/grep-sed-awk/全文搜索/NLP预处理/文本分类 | simple~medium |
| AP01–AP10 | 数学建模与科学计算 | 线性代数/优化/微分方程/贝叶斯/蒙特卡洛/信号处理/数据拟合/运筹学/数值仿真/科学计算工程 | medium~hard |
| AQ01–AQ10 | API 设计艺术与错误语义 | REST/gRPC/GraphQL协议选型 / 错误码体系 / 分页策略 / 版本演进 / Webhook / 限流 / 幂等 / 网关 | medium~hard |
| AR01–AR10 | 国际化工程与 RTL 布局 | i18n/l10n 工程化 / ICU MessageFormat / RTL双向文本 / 字体回退 / 时区日历 / 多语言搜索 / 本地化运营 | simple~medium |
| AS01–AS10 | 无障碍 a11y 与包容性设计 | WCAG/ARIA / 屏幕阅读器 / 键盘导航 / 色彩对比 / 替代文本 / 自动化检测 / 法规合规 | simple~medium |
| AT01–AT10 | 代码考古与遗留系统迁移 | 遗留代码理解 / 重构策略 / 绞杀者模式 / 数据迁移 / 框架升级 / 单体拆分 / 死代码消除 | medium~hard |
| AU01–AU10 | 形式化方法与软件验证 | TLA+/模型检测 / Coq+Lean+Isabelle / 属性测试 / 静态分析 / 符号执行 / Dafny+SPARK / 工程化ROI | medium~hard |
| AV01–AV10 | 安全工程与渗透测试 | STRIDE/Web渗透/SQLi/XSS+SSRF/认证授权漏洞/密码学/漏洞赏金/容器安全/CTF/SDL | medium~hard |
| AW01–AW10 | 操作系统与内核原理 | 内核架构/进程线程/调度/CFS/内存管理/IPC/同步原语/文件系统/虚拟化容器/eBPF | medium~hard |
| AX01–AX10 | 编译原理与 DSL 实战 | 编译流程/词法分析/语法分析/AST/IR+SSA/代码生成/优化/JIT/DSL设计/Mini-Lisp实战 | medium~hard |
| AY01–AY10 | 软件架构模式与设计系统 | 架构全景/DDD/CQRS+ES/整洁架构/事件驱动/插件化/API选型/ADR/可演化/ATAM评估 | medium~hard |
| AZ01–AZ10 | LLM 提示词注入与 AI 红队 | 提示词注入/越狱/PII泄露/Agent劫持/RAG投毒/模型供应链/水印溯源/合规 | medium~hard |
| BA01–BA10 | 异步编程范式专题 | 异步模型演进/Rust tokio/Go goroutine/Node事件循环/Java虚拟线程/Kotlin协程/Reactive/取消传播/反模式 | medium~hard |
| BB01–BB10 | 软件供应链与 SBOM 工程 | 供应链攻击/SBOM标准/CVE监控/制品签名/SLSA/License合规/私有仓库/可复现构建/应急响应 | medium~hard |
| BC01–BC10 | 技术写作与知识沉淀 | 金字塔原理/ADR/API文档/Runbook/Engineering Wiki/RFC/技术博客/演讲/注释即文档/工具链 | simple~medium |
| BD01–BD10 | 领域驱动设计 DDD 专题 | 通用语言/限界上下文/Context Map/聚合根/值对象/领域服务/领域事件/仓储/事件风暴/团队转型 | medium~hard |
| BE01–BE10 | 从零手写 Unix 命令行工具集 | cat/ls/wc/grep/find/tree/du/xargs/diff/watch 逐个复刻实现 | medium 为主 |
| BF01–BF10 | 经典小游戏复刻与游戏编程实战 | 俄罗斯方块/扫雷/2048/推箱子/五子棋AI/生命游戏/打砖块/MUD/数独/塔防 | medium~hard |
| BG01–BG10 | 从零造轮子：经典系统复刻 | 迷你 Redis/Git/Docker/HTTP服务器/JSON解析器/正则引擎/虚拟机/数据库/协程/编辑器 | hard 为主 |
| BH01–BH10 | 小众与新兴编程语言巡礼 | Nim/Crystal/Julia/R/Scala3/OCaml+F#/Erlang/Racket/Fortran+COBOL/Odin+V+Gleam | medium~hard |
| BI01–BI10 | 编辑器插件开发与开发环境定制 | VSCode扩展/LSP接入/Neovim/JetBrains插件/自制LSP/dotfiles/Shell定制/字体/DevContainer/键位 | medium 为主 |
| BJ01–BJ10 | 企业业务系统开发实战 | 进销存/工单/审批流/RBAC/报表引擎/多租户SaaS/CRM/考勤排班/低代码表单/对账结算 | medium~hard |
| BK01–BK10 | 桌面小工具软件开发实战 | 截图标注/剪贴板/番茄钟/批量重命名/看图/音乐播放器/密码管理器/串口助手/笔记本/监控挂件 | medium~hard |
| BL01–BL10 | 疑难 Bug 攻坚与故障排查实录 | 段错误/内存泄漏/死锁/数据竞争/生产OOM/CPU打满/fd泄漏/时钟时区/乱码/变更归因 | medium~hard |
| BM01–BM10 | 文件格式解析与序列化编程 | PNG/ZIP/WAV/xlsx/Protobuf/CBOR/SQLite文件格式/CSV方言/YAML+TOML/自定义二进制协议 | medium~hard |
| BN01–BN10 | 编程冷知识与「为什么」十问 | 浮点/UTF-8/时间/大小端/随机数与UUID/排序稳定性/下标从0/null/删文件/环境差异 | simple~medium |
| BO01–BO10 | 跨平台电脑使用与虚拟化实战 | PowerShell/WSL2/macOS/虚拟机/远程桌面/NAS/外设排障/分区与数据恢复/系统迁移/双系统 | simple~medium |
| BP01–BP10 | 设计系统与组件库工程实战 | Design Token/组件API/暗色主题/图标系统/栅格/排版/动效/表单规范/图表规范/组件库治理 | medium 为主 |
| BQ01–BQ10 | 量子计算编程与量子算法 | 量子比特与布洛赫球/量子门与电路/量子纠缠/Shor算法/Grover搜索/Qiskit/变分算法VQE-QAOA/量子纠错表面码/NISQ/量子-经典混合 | medium~hard |
| BR01–BR10 | 机器人与 ROS2 工程 | ROS2节点话题服务动作/URDF与tf2/Navigation2/MoveIt2/Gazebo仿真/BT.CPP行为树/Lifecycle多机/ros2_control/Micro-ROS/排障实战 | medium~hard |
| BS01–BS10 | 3D 建模、CAD 与几何处理实战 | 半边网格/曲面细分/CSG布尔与OpenCASCADE/OpenGL-Vulkan管线/PBR glTF2.0/几何深度学习/参数化生成/多视图几何/3D打印修复/刚体碰撞 | medium~hard |
| BT01–BT10 | WebAssembly 深度与边缘计算 | Wasm字节码与S-表达式/wasmtime JIT-AOT/WASI/Component Model与WIT/Edge Runtime/Capability安全/性能SIMD多线程/Rust工具链/Wasm vs容器/WASI Preview2 | medium~hard |
| BU01–BU10 | 密码学与隐私计算 | AES-GCM-ChaCha20/RSA-ECC-Ed25519/哈希与Argon2/TLS1.3/零知识证明/同态加密/安全多方计算/PKI证书/后量子密码Kyber-Dilithium/Rust crypto生态 | medium~hard |
| BV01–BV10 | 并行与 GPU 计算 | GPU SM-Warp架构/CUDA编程模型/内存层次与合并访问/SIMD-AVX512/rayon-crossbeam/wgpu原生GPU/NCCL多卡通信/Roofline模型/并行算法/CUDA profiling | medium~hard |
| BW01–BW10 | 函数式编程进阶 | λ演算与Curry-Howard/Haskell类型类-Functor-Applicative-Monad/Monad Transformer/解析器组合子-nom/依赖类型Idris-Agda/线性类型与Rust ownership/GADT/惰性求值/代数效应/FP在Rust实战 | medium~hard |
| BX01–BX10 | 编译器后端与 LLVM 优化 | SSA与φ函数/LLVM IR结构/Pass Manager/内联成本模型/循环优化LICM向量化/寄存器分配/指令选择GlobalISel/Target描述/LTO-ThinLTO/MLIR Dialect/JIT-LLJIT | medium~hard |
| BY01–BY10 | AR/VR 与空间计算 | OpenXR生态/SLAM空间定位/渲染ATW-ASW-SSW/多维交互-手柄手势眼动语音/空间锚点持久化/空间音频HRTF/三维UI舒适UX/MR场景理解遮挡/注视点渲染与热节流/WebXR与three.js | medium~hard |
| BZ01–BZ10 | 语音交互与对话式 UI | 语音全链路ASR-对话管理-TTS/Whisper-Paraformer流式ASR/VITS-Bert-VITS流式TTS/VAD端点检测/唤醒词Porcupine/对话管理FSM-LLM/语音UX打断修复/SSML情感语音/Voice Agent全双工AEC/多模态Copilot Voice | medium~hard |
| CA01–CA10 | 基础设施可观测性与 SRE | 三大支柱Logs-Metrics-Traces/OpenTelemetry工程化/SLO-SLI错误预算Burn Rate/Incident Management无责复盘/告警疲劳治理/容量规划Little's Law/混沌工程ChaosMesh/Continuous Profiling eBPF/SRE平台工程/Grafana技术栈一站式 | medium~hard |

## 每条提示词的字段

```markdown
### Xnn 主题名
- **预期档位**: simple / medium / hard
- **考察维度**: 一句话说明考点
- **对话脚本**:
  1. 第一轮提示词
  2. 第二轮提示词（引用前轮）
  ...
```

## 使用方式

### 1. TUI 手动多轮（推荐）

```bash
./laew            # 进入 TUI,逐条粘贴每轮提示词,观察 Agent 编排与多轮上下文
```

一组提示词在同一个 Session 内连续喂入；测完一组后 `/new` 开新 Session，避免组间污染。

### 2. `-f` 文件模式跑单条

把某一组的第一轮提示词存成临时文件（如 `tmpPlan/A01.md`），再：

```bash
./laew -f tmpPlan/A01.md
```

注意：`-f` 是单轮模式，多轮追问请用 TUI。

### 3. 自动化批量（可结合 testReport/）

可参考 `testReport/run_e2e.sh` 的 mock LLM 思路，把每组提示词第一轮作为输入、
后续轮作为断言锚点，扩展出多轮自动化用例。

## 维护约定

- 新增主题时**先检索本库 770 个主题名，确认不重复**再追加；编号沿类别字母顺延（Z 之后用 AA/AB/…，AP 之后用 AQ/AR/…，AY 之后用 AZ/BA/BB/…，BP 之后用 BQ/BR/BS/…，CA 之后用 CB/CC/CD/…）。
- 修改 laew 功能后（如新增工具、改档位策略），同步修订受影响条目的「预期档位」。
- 每组提示词控制在 3~5 轮；后轮必须与前轮有显式承接关系，保证"多轮"语义成立。
- 实测发现某条实际档位与预期不符时，先记录现象（写入 testReport/ 验证报告），
  再决定是调预期还是调分类器。
