# 自动化测试 — 提示词文件列表

## 是什么

面向 laew（LsmAgentEmergentWork）的**自动化测试提示词集合**，共 **154 维度 × 10 个 = 1540 个多轮对话提示词**。
每个提示词设计为 3~5 轮追问，覆盖任务处理、知识问答、工作日常、电脑使用、软件使用、
编码 Coding、界面设计、文件整理处理、LLM 安全攻防、异步编程、软件供应链、技术写作、DDD、
手写 Unix 命令、经典游戏复刻、从零造轮子、小众语言、编辑器插件、业务系统、桌面小工具、
疑难 Bug 攻坚、文件格式解析、编程冷知识、跨平台电脑使用、设计系统、量子计算、机器人 ROS2、
3D 建模与几何、WebAssembly 边缘、密码学与隐私计算、并行 GPU、函数式编程、编译器后端 LLVM、
AR/VR 空间计算、语音交互与对话式 UI、基础设施可观测性与 SRE、Agent 记忆与上下文工程、
隐私计算与数据合规工程、边缘计算与离线优先架构、数字孪生与工业 IoT、人机交互与可解释 AI、
软件工程认知科学与开发者成长、元编程与反射编程范式、数据库内核与存储引擎实现、
开发者效能工程与个人知识管理、类型系统设计与实现、实时系统与低延迟工程、
编程语言历史与演进、软件估算与项目规划、创意编程与生成艺术、
包管理与依赖解析工程、网络协议底层与套接字编程、多人在线游戏服务器、体素沙盒无限世界、
HTML5 小游戏 Canvas、物理仿真与粒子特效、推荐系统与个性化、地理空间 GIS 地图、
金融账务量化回测、电商交易营销、压缩编码与数据校验、历法时间与重复规则、
OJ 在线判题平台、图算法与社交网络分析、弹性系统与混沌工程实战、
人因工程与软件组织行为学、搜索引擎与信息检索工程、即时通讯 IM 系统编程、
排版渲染与打印引擎、构建系统与编译缓存工程、生物信息学与基因数据编程、
音乐编程与音频合成、规则引擎与业务决策自动化、天文计算与航天编程、
**跨平台 Shell 脚本移植与健壮性工程**、**Linux 服务器治理进阶 systemd 与批量运维**、
**游戏品类与系统编程进阶（放置/卡牌/棋类/猜词/数值）**、**模拟器与复古平台编程**、
**气象与环境数据编程**、**智能家居与家庭自动化编程**、**数字电路与逻辑仿真编程**、
**逻辑编程与约束求解实战**、**Windows 系统管理与 PowerShell 运维实战**、**macOS 桌面运维与 Apple 生态开发实战**、**容器编排与 Kubernetes 运维实战**、**CI/CD 流水线工程与 GitOps 实战**、**基础设施即代码 IaC 与 Ansible 自动化运维**、**日志分析与监控告警工程实战**、**数据库运维与备份恢复工程实战**、**网络安全审计与合规自动化实战**、**自动化测试框架与测试工程实践**、**办公自动化与文档处理工程实战**、**跨平台应急响应与系统取证**、**可复现实验与科学计算**、**插件化命令行应用与命令总线**、**游戏本地化与全球发布**、**桌面自动化与操作回放**、**团队工作流与运营自动化**、**多媒体资产交付与批量转码**、**边缘函数与事件流集成**、**技术知识库与可验证问答**、**多语言运行时与内存模型实验**、**开源许可证合规与依赖义务追踪**、**网络数据包解码与流量回放工程**、**游戏公平性与反作弊风控工程**、**公共交通 GTFS 与出行调度仿真**、**微电网储能与能源调度编程**、**合同条款抽取与法务义务追踪**、**数据标注流水线与样本质检工程**、**二进制体积与符号膨胀治理**、**仓储物流与库存路径优化**、**远程开发会话与终端多路复用治理**等多个角度，**无重复主题**。

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
  79-Agent记忆与上下文工程.md               ← 三层记忆STM-LTM-Episodic/Context Engineering七要素/MemGPT虚拟上下文分页/RAG进阶HyDE-ReRank-GraphRAG-Self-RAG/长程一致性/反思机制ReAct-Reflexion-Self-Refine/多Agent共享记忆/记忆压缩/记忆评估LOCOMO
  80-隐私计算与数据合规工程.md               ← 数据脱敏分级与工程化/差分隐私ε预算/联邦学习横向纵向拆分/GDPR-PIPL-CCPA合规框架对比/同意管理/数据驻留与跨境传输/PETs隐私增强技术选型/数据生命周期治理/Privacy by Design/laew合规升级路径
  81-边缘计算与离线优先架构.md               ← 离线优先设计哲学/Service Worker生命周期/PWA标准/IndexedDB/CRDT合并算法/Operational Transformation/Edge Runtime架构/边缘AI推理ONNX-TF.js-WebGPU/网络弹性/同步协议ETag-CDC/laew离线优先升级
  82-数字孪生与工业IoT.md                   ← 数字孪生六维模型/OPC UA工业协议与信息建模/MQTT Sparkplug B/IT-OT融合与协议网关/时序数据库InfluxDB-TimescaleDB/仿真集成Unity-Omniverse/预测性维护异常检测RUL/边缘AI部署蒸馏/工业安全ISA-IEC 62443/laew工业4.0集成
  83-人机交互与可解释AI.md                   ← 心智模型与AI系统理解/信任校准/可解释AI LIME-SHAP-Attention/决策溯源/算法审计/以人为本AI/人机分工HITL/AI UX模式流式-不确定性-来源-拒绝/协作智能/AI错误恢复3R原则/laew AI UX优化路线
  84-软件工程认知科学与开发者成长.md         ← 认知负荷/心流/深度工作/调试心智/技能习得曲线/源码阅读方法论/工程判断力/技术领导力/反脆弱职业/AI时代思考深度/laew 思考伙伴
  85-元编程与反射编程范式.md                 ← 同像性/宏系统/反射/过程宏/编译期计算/注解与装饰器/代码生成/运行时代码加载/模板元编程/元对象协议
  86-数据库内核与存储引擎实现.md             ← B+树/LSM树/MVCC/WAL/查询优化器/火山模型/列式存储/两阶段提交/新型存储引擎/数据库基准测试
  87-开发者效能工程与个人知识管理.md         ← 个人知识管理/Zettelkasten/第二大脑/工具链自动化/环境复现/键盘流/快捷方式/工作流优化/注意力管理/持续学习系统
  88-类型系统设计与实现.md                   ← 类型系统/类型推断/Hindley-Milner/泛型/依赖类型/线性类型/类型类/行多态/类型级编程/类型安全
  89-实时系统与低延迟工程.md                 ← 实时调度/低延迟网络/高频交易/无锁数据结构/内核旁路/DPDK/RDMA/时间敏感网络/确定性延迟/延迟分析
  90-编程语言历史与演进.md                   ← 语言谱系/范式融合/设计哲学/消亡与复兴/中文编程/可视化编程/语言设计方法论/编程语言为什么/未来趋势/语言生态
  91-软件估算与项目规划.md                   ← 软件估算/三点估算/敏捷估算/项目计划/里程碑/风险管理/进度跟踪/范围管理/沟通计划/项目复盘
  92-创意编程与生成艺术.md                   ← 生成艺术/创意编程/Shader编程/分形几何/细胞自动机/L系统/数据可视化/声音艺术/交互艺术/代码美学
  93-包管理与依赖解析工程.md                 ← 语义化版本/依赖解析算法/锁文件/Monorepo/私有仓库/依赖地狱/可复现构建/Cargo/npm/pip对比/包安全/包管理设计
  94-网络协议底层与套接字编程.md             ← 套接字编程/IO多路复用/epoll/kqueue/TCP状态机/HTTP实现/WebSocket/QUIC/协议设计/网络调试/高性能网络
  95-多人在线游戏服务器编程实战.md           ← 大厅房间架构/ELO匹配/状态同步与延迟补偿/帧同步服务端/断线重连观战/服务器权威反作弊/赛季排行榜/KCP-QUIC选型/房间二进制协议/机器人压测
  96-体素沙盒与无限世界编程.md               ← 区块与调色板压缩/贪心网格合并/三维密度地形/流式加载与滞后卸载/脏区块重网格化/天光BFS与顶点AO/种子与修改diff存档/meshing多线程管线/视锥剔除与LOD/区块所有权同步
  97-HTML5小游戏与Canvas编程实战.md          ← rAF游戏循环与后台节流/离屏分层渲染/雪碧图动画与资源预载/键盘触屏虚拟摇杆/场景栈与过渡/Web Audio音效/摄像机与视差卷轴/HUD与像素完美/微信小游戏适配与开放数据域/性能与内存排查
  98-物理仿真与粒子特效编程.md               ← Verlet积分与约束/质点弹簧布料撕裂/绳索拖拽/压力软体/SPH流体与邻居搜索/Boids群体行为/烟花爆炸特效/Voronoi碎裂破坏/物理手感量化调参/引擎选型与自制边界
  99-推荐系统与个性化工程.md                 ← UserCF与ItemCF/矩阵分解ALS/CTR预估LR-FM-DeepFM/多目标融合/多路召回漏斗/冷启动与Bandit/多样性打散MMR/特征工程与泄漏排查/interleaking与护栏指标/级联排序架构
  100-地理空间GIS与地图编程.md               ← WGS84-GCJ02-BD09火星坐标/GeoJSON解析校验/R树四叉树索引/地理编码/Haversine与地理围栏/路网路径规划/瓦片金字塔slippy tile/OSM数据处理/Marching Squares等值线/PostGIS空间SQL
  101-金融账务与量化回测编程.md              ← 金额精度与decimal/复式记账分录/流水幂等与冲正/K线聚合与技术指标/事件驱动回测/滑点手续费建模/风控仓位与凯利/夏普等绩效指标/paper trading数据管道
  102-电商交易与营销系统编程.md              ← 购物车合并/SPU-SKU建模/预占防超卖/秒杀削峰与库存分桶/优惠券模板核销/订单状态机与超时关单/支付回调验签/facets搜索筛选/黄牛防刷风控/大促降级预案
  103-压缩编码与数据校验编程.md              ← RLE与Delta/Huffman规范码/LZ77滑动窗口/LZW词典/算术编码区间/CRC查表/Base64变体/zstd-LZ4选型基准/FNV-xxHash滚动哈希/Parquet列存编码
  104-历法时间与重复规则编程.md              ← 闰年规则与改历/儒略日MJD/农历转换查表与天文/节气太阳黄经/月相潮汐/ISO8601手写解析/tzdata与DST判定/RRule重复规则引擎/cron解析与下次触发/月历调休与iCal导出
  105-在线判题OJ与算法教学平台.md            ← 判题沙箱rlimit-seccomp/测试点与子任务计分/spj特判与交互题/winnowing代码查重/数据生成器与对拍/ACM-IOI-CF赛制排名/算法步骤回放可视化/题库难度校准/判题队列弹性伸缩/AI错因讲解
  106-图算法应用与社交网络分析.md            ← CSR图存储/PageRank幂迭代/Louvain社区发现/双向BFS社交距离/三种中心性/好友推荐与图嵌入/Feed推拉模式/力导向可视化/知识图谱三元组/Pregel超步计算
  107-弹性系统与混沌工程实战.md              ← 韧性模式六件套/指数退避抖动四式/熔断器三态机/Bulkhead舱壁/Chaos注入矩阵/GameDay红蓝对抗/blameless postmortem五段论/自适应弹性/优雅降级金字塔/DR演练RTO-RPO
  108-人因工程与软件组织行为学.md             ← Conway定律反向利用/Team Topologies四类团队/DORA四指标/SPACE五维度/第二大脑组织记忆飞轮/远程异步writing-first/会议税与决策日志/技术债四象限治理/工程文化与OKR晋升机制
  109-搜索引擎与信息检索工程.md               ← 倒排索引与压缩/TF-IDF与BM25调参/查询纠错改写/前缀补全Trie/高亮动态摘要/点击日志分析/MAP·NDCG评估/段合并NRT/多字段多语言/SQLite FTS5迷你引擎
  110-即时通讯IM系统编程实战.md               ← 长连接网关心跳/ACK重传去重/会话内seqid排序/离线消息多端漫游/已读回执未读数/写扩散vs读扩散/历史分页游标/撤回编辑内容审核/Signal协议简化/WebSocket迷你IM
  111-排版渲染与打印引擎编程.md               ← Knuth-Plass断行/字体度量基线行高/Markdown AST渲染管线/HTML→PDF分页/撤销重做栈/表格列宽跨页/语法高亮引擎/@media print/墨水屏刷新抖动/网格版式自动化
  112-构建系统与编译缓存工程.md               ← Make依赖图/-MMD头依赖/Ninja并行调度/ccache内容寻址CAS/远程缓存distcc分布式编译/Bazel hermetic沙箱/sysroot交叉编译/deb-rpm打包/构建性能关键路径/inotify watch/手写迷你构建系统
  113-生物信息学与基因数据编程.md             ← FASTA-FASTQ与Phred质量/Smith-Waterman比对/BLAST种子扩展/de Bruijn图组装/VCF变异检测/NJ系统发育树/PDB结构解析/单细胞RNA-seq管线/Snakemake工作流/基因数据合规
  114-音乐编程与音频合成实战.md               ← MIDI事件流VLQ/减法合成器ADSR/步进音序器lookahead调度/FFT调音器/onset与BPM估计/和弦进行自动伴奏/五线谱MusicXML渲染/延迟混响失真效果器链/马尔可夫作曲/VST插件宿主
  115-规则引擎与业务决策自动化.md             ← 规则与代码分离/Rete网络/DMN决策表命中策略/中文规则DSL/保险核保分层/实时风控决策/积分等级状态机/冲突检测优先级/规则版本灰度/手写迷你Rete
  116-天文计算与航天编程实战.md               ← 赤道地平坐标转换/简化星历/开普勒方程牛顿迭代/日出日落晨昏蒙影/TLE解析SGP4过境预测/星野对齐叠加降噪/凌日光变BLS/NASA开放API/FITS格式WCS/霍曼转移Δv
  117-跨平台Shell脚本移植与健壮性工程.md      ← GNU与BSD差异地图/bash·zsh·fish方言/POSIX sh子集/shebang与CRLF陷阱/trap错误处理链/getopts与CLI惯例/管道子shell变量丢失/bats测试/bash→PowerShell对译/脚本分发与环境锁定
  118-Linux服务器治理进阶systemd服务化与批量运维.md ← systemd服务化部署/依赖与socket激活/timer替代cron/journalctl深度/cgroups v2配额/免Agent批量巡检/SSH密钥治理/升级窗口/幂等脚本/自愈预案
  119-游戏品类与系统编程进阶放置卡牌棋类猜词与数值.md ← 放置挂机离线结算/卡牌效果触发引擎/万智牌回合优先权/国际象棋走法生成Zobrist/negamax与UCI协议/Wordle信息熵猜词/经营经济供需通胀/抽卡保底PRD/模组加载兼容/热座多人框架
  120-模拟器与复古平台编程.md                 ← 模拟器原理地图/CHIP-8指令循环/6502寻址模式周期/内存映射MMIO/卡带Mapper bank切换/PPU扫描线渲染/APU声音通道/周期精确同步/即时存档回放/trace调试器
  121-气象与环境数据编程.md                   ← GRIB2·BUFR·NetCDF格式/eccodes解析/站点缺测插补/气候态距平SPI/台风BEST TRACK/雷达Z-R关系/AQI分段计算/分级技巧评分/多源API聚合/风羽站点填图
  122-智能家居与家庭自动化编程.md             ← Home Assistant实体模型/Zigbee2MQTT组网/自动化三段式模板/传感器去抖治理/能源统计电费/本地语音链路/Zigbee排障LQI/Tailscale远程安全/recorder持久化/Lovelace场景设计
  123-数字电路与逻辑仿真编程.md               ← 卡诺图化简/MUX译码器七段/超前进位ALU/触发器亚稳态/Mealy-Moore状态机/Verilog阻塞非阻塞/事件驱动仿真器delta周期/惯性延迟冒险/流水线转发气泡/NAND2Tetris门级CPU
  124-逻辑编程与约束求解实战.md               ← Prolog统一回溯/迷你Prolog解释器/Datalog不动点/八皇后地图着色建模/MRV弧一致AC-3/DPLL与CDCL/Z3排班优化/会议排课UNSAT诊断/配送软约束松弛/声明式思维选型
  125-Windows系统管理与PowerShell运维实战.md   ← PowerShell对象管道/WMI-CIM查询/Windows事件日志/注册表ACL/服务进程管理/WinRM远程/计划任务/性能计数器/Windows更新API
  126-macOS桌面运维与Apple生态开发实战.md      ← Homebrew Formula-Cask/launchd plist/Defaults偏好读写/AppleScript GUI自动/plist解析编辑/diskutil磁盘管理/Keychain密钥存储/Xcode工具链/TCC隐私权限
  127-容器编排与Kubernetes运维实战.md          ← Dockerfile多阶段构建/Docker Compose健康检查与依赖/K8s Deployment-Service-Ingress/ConfigMap/Secret/PVC存储/滚动更新回滚/HPA自动扩缩/Helm Chart打包/命名空间RBAC
  128-CI-CD流水线工程与GitOps实战.md            ← GitLab CI stage依赖/GitLab matrix矩阵/构建缓存优化/制品语义化版本/环境晋升审批门/ArgoCD Application-CRD/Flux Kustomization/SonarQube质量门禁/Vault密钥注入
  129-基础设施即代码IaC与Ansible自动化运维.md   ← Terraform resource-module-state/Ansible Playbook-Role/Cloud-Init初始化/Packer镜像构建/Pulumi编程式IaC/Checkov合规扫描/Drift漂移检测/多云输出聚合
  130-日志分析与监控告警工程实战.md             ← Filebeat采集-grok解析/PromQL rate-histogram/Grafana仪表盘JSON/Alertmanager路由抑制/SLO burn-rate/分布式Trace分析/LogQL日志检索/日志降噪采样/事件关联时间线
  131-数据库运维与备份恢复工程实战.md           ← MySQL慢查询索引优化/PostgreSQL VACUUM统计信息/Redis RDB-AOF集群/MySQL全量增量备份/PITR时间点恢复/GTID主从切换/连接池泄漏检测/在线Schema迁移/CDC跨库同步
  132-网络安全审计与合规自动化实战.md           ← Nmap NSE脚本扫描/CIS Benchmark基线/testssl.sh证书审计/iptables规则审计/SSH加固sshd_config/Trivy容器扫描/ModSecurity WAF/Sigma SIEM规则/应急响应自动化
  133-自动化测试框架与测试工程实践.md           ← pytest fixture-parametrize/Mock patch测试/pytest-BDD Gherkin/Pact契约测试/Locust性能测试/Playwright视觉回归/factory_boy数据工厂/testcontainers环境隔离/Allure报告度量/变异测试充分性
  134-办公自动化与文档处理工程实战.md           ← openpyxl报表样式公式/pandas CSV清洗转换/reportlab PDF生成/pdfplumber表格提取/python-docx模板填充/Markdown批量转换/smtplib邮件自动化/imaplib自动分类/文件批量整理/APScheduler工作流编排
  135-跨平台应急响应与系统取证实战.md           ← 证据哈希链/多源时间线归一/进程端口漂移/launchd-systemd-cron-计划任务/脚本风险扫描/USB外设画像/浏览器下载痕迹/容量与deleted-open风险/事件处置报告/便携工具包自检
  136-可复现实验与科学计算工程.md               ← 假设协议与seed/原始数据QC/bootstrap置信区间/参数扫描敏感性/不确定度传播/失败实验台账/环境指纹/图表数据包/科学验收断言/一键复现包
  137-插件化命令行应用与命令总线工程.md         ← 插件manifest契约/argparse子命令/生命周期钩子/分层配置覆盖/JSON-CSV-table格式器/检查点取消/能力权限/API版本协商/错误码语义/CLI行为回归
  138-游戏本地化与全球发布管线.md               ← 字符串抽取/复数性别ICU/伪本地化截断/字体回退/文化合规筛查/商店关键词/年龄分级问卷/分区域发布清单/补丁说明/LQA缺陷门禁
  139-桌面自动化与用户操作回放.md               ← 语义Action录制/a11y稳定selector/确定性回放幂等/等待抖动重试/截图隐私遮罩/数据驱动矩阵/macOS-Windows-Linux adapter/权限kill switch/失败自愈诊断/套件趋势
  140-团队工作流与运营自动化.md                 ← 会议行动项/工单SLA路由/值班公平性/审批事件溯源/费用政策检查/入职开通/KPI汇总异常归因/RACI决策日志/跨班次交接/运营周报
  141-多媒体资产交付与批量转码.md               ← 资产清单来源链/哈希去重版本链/渠道编码矩阵/并发转码dry-run/业务元数据sidecar/SRT-VTT章节/交付manifest/QC门禁/断点回滚/渠道交接
  142-边缘函数与事件流集成实战.md               ← 事件信封schema/HMAC防重放幂等/队列背压/退避抖动DLQ/Transactional Outbox/边缘合规路由/冷启动预算/配置密钥发布/Trace延迟归因/混沌故障矩阵
  143-技术知识库与可验证问答工程.md             ← 语料来源分级/实体关系抽取/引用行级定位/矛盾过期检测/迷你知识图谱/检索计划/证据包答案/保鲜责任人/问答评测评分/维护状态机
  144-多语言运行时与内存模型实验.md             ← 栈堆逃逸记账/借用检查玩具/mark-sweep分代GC/AoS-SoA缓存/并发可见性/异步调度开销/UTF-8-16字符串/整数溢出语义/结构体ABI布局/运行时选型
  145-开源许可证合规与依赖义务追踪.md           ← SPDX清单/许可证兼容矩阵/NOTICE归属/文本漂移/环境策略门禁/vendored来源/商标与运行时义务/替代方案/例外台账/发布证据包
  146-网络数据包解码与流量回放工程.md           ← 链路帧解码/TCP流重组/DNS配对/TLS握手时间线/HTTP重建/环形索引/黄金回放diff/QOE抖动/丢包矩阵/匿名化校验
  147-游戏公平性与反作弊风控工程.md             ← 遥测契约/物理不变量/行为统计/技能延迟匹配/举报分诊/确定性回放/处罚申诉/权威端校正/灰度评估/公平仪表盘
  148-公共交通GTFS与出行调度仿真.md             ← GTFS完整性/服务日历例外/换乘路径/车辆折返冲突/延误传播/票务规则/无障碍路径/司机轮班/客流仿真/临时绕行
  149-微电网储能与能源调度编程.md               ← 负荷分段/预报误差备用/电池退化调度/并离网互锁/需求响应/峰谷套利/逆变器QC/黑启动/碳流核算/孪生校准
  150-合同条款抽取与法务义务追踪.md             ← 条款结构化/义务矩阵/修订差异/到期提醒/主体画像/redline评审/履约证据链/模板校验/争议升级/保留审计
  151-数据标注流水线与样本质检工程.md           ← 标注规范版本/多人一致性/分歧仲裁/难例队列/标签漂移/mask质检/脱敏授权/派单公平/金标陷阱/数据集卡片
  152-二进制体积与符号膨胀治理.md               ← 段清单/符号聚合/strip变体/优化矩阵/依赖体积归因/资源膨胀/体积启动权衡/跨平台差异/预算门禁/回归分派
  153-仓储物流与库存路径优化.md                 ← ABC库位/拣货TSP/循环盘点/收货上架/波次截止/退货分级/冷链越限/月台预约/智能装箱/安全库存
  154-远程开发会话与终端多路复用治理.md         ← tmux清单/命令风险/断线恢复/SSH体检/端口转发图/scrollback索引/长任务通知/结对ACL/环境漂移/事件回放
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
| CA11–CA20 | Agent 记忆与上下文工程 | 三层记忆STM-LTM-Episodic/Context Engineering七要素/MemGPT虚拟上下文分页/RAG进阶HyDE-ReRank-GraphRAG-Self-RAG/长程一致性/反思机制ReAct-Reflexion-Self-Refine/多Agent共享记忆/记忆压缩/记忆评估LOCOMO | medium~hard |
| CA21–CA30 | 隐私计算与数据合规工程 | 数据脱敏分级与工程化/差分隐私ε预算/联邦学习横向纵向拆分/GDPR-PIPL-CCPA合规框架对比/同意管理/数据驻留与跨境传输/PETs隐私增强技术选型/数据生命周期治理/Privacy by Design/laew合规升级路径 | medium~hard |
| CA31–CA40 | 边缘计算与离线优先架构 | 离线优先设计哲学/Service Worker生命周期/PWA标准/IndexedDB/CRDT合并算法/Operational Transformation/Edge Runtime架构/边缘AI推理ONNX-TF.js-WebGPU/网络弹性/同步协议ETag-CDC/laew离线优先升级 | medium~hard |
| CA41–CA50 | 数字孪生与工业 IoT | 数字孪生六维模型/OPC UA工业协议与信息建模/MQTT Sparkplug B/IT-OT融合与协议网关/时序数据库InfluxDB-TimescaleDB/仿真集成Unity-Omniverse/预测性维护异常检测RUL/边缘AI部署蒸馏/工业安全ISA-IEC 62443/laew工业4.0集成 | medium~hard |
| CA51–CA60 | 人机交互与可解释 AI | 心智模型与AI系统理解/信任校准/可解释AI LIME-SHAP-Attention/决策溯源/算法审计/以人为本AI/人机分工HITL/AI UX模式流式-不确定性-来源-拒绝/协作智能/AI错误恢复3R原则/laew AI UX优化路线 | medium~hard |
| CB01–CB10 | 软件工程认知科学与开发者成长 | 认知负荷理论/心流/深度工作/调试心智模型/技能习得曲线/源码阅读方法论/工程判断力/技术领导力/反脆弱职业/AI时代思考深度 | medium~hard |
| CC01–CC10 | 元编程与反射编程范式 | 同像性/宏系统/反射/过程宏/编译期计算/注解与装饰器/代码生成/运行时代码加载/模板元编程/元对象协议 | medium~hard |
| CD01–CD10 | 数据库内核与存储引擎实现 | B+树/LSM树/MVCC/WAL/查询优化器/火山模型/列式存储/两阶段提交/新型存储引擎/数据库基准测试 | medium~hard |
| CE01–CE10 | 开发者效能工程与个人知识管理 | 个人知识管理/Zettelkasten/第二大脑/工具链自动化/环境复现/键盘流/快捷方式/工作流优化/注意力管理/持续学习系统 | simple~medium |
| CF01–CF10 | 类型系统设计与实现 | 类型系统/类型推断/Hindley-Milner/泛型/依赖类型/线性类型/类型类/行多态/类型级编程/类型安全 | medium~hard |
| CG01–CG10 | 实时系统与低延迟工程 | 实时调度/低延迟网络/高频交易/无锁数据结构/内核旁路/DPDK/RDMA/时间敏感网络/确定性延迟/延迟分析 | medium~hard |
| CH01–CH10 | 编程语言历史与演进 | 语言谱系/范式融合/设计哲学/消亡与复兴/中文编程/可视化编程/语言设计方法论/编程语言为什么/未来趋势/语言生态 | simple~medium |
| CI01–CI10 | 软件估算与项目规划 | 软件估算/三点估算/敏捷估算/项目计划/里程碑/风险管理/进度跟踪/范围管理/沟通计划/项目复盘 | medium~hard |
| CJ01–CJ10 | 创意编程与生成艺术 | 生成艺术/创意编程/Shader编程/分形几何/细胞自动机/L系统/数据可视化/声音艺术/交互艺术/代码美学 | medium~hard |
| CK01–CK10 | 包管理与依赖解析工程 | 语义化版本/依赖解析算法/锁文件/Monorepo/私有仓库/依赖地狱/可复现构建/Cargo/npm/pip对比/包安全/包管理设计 | medium~hard |
| CL01–CL10 | 网络协议底层与套接字编程 | 套接字编程/IO多路复用/epoll/kqueue/TCP状态机/HTTP实现/WebSocket/QUIC/协议设计/网络调试/高性能网络 | medium~hard |
| CM01–CM10 | 多人在线游戏服务器编程实战 | 大厅房间架构/ELO匹配/状态同步与延迟补偿/帧同步服务端/断线重连观战/服务器权威反作弊/赛季排行榜/KCP-QUIC选型/房间二进制协议/机器人压测 | medium~hard |
| CN01–CN10 | 体素沙盒与无限世界编程 | 区块与调色板压缩/贪心网格合并/三维密度地形/流式加载滞后卸载/脏区块重网格化/天光BFS与顶点AO/种子与修改diff存档/meshing多线程管线/视锥剔除与LOD/区块所有权同步 | medium~hard |
| CO01–CO10 | HTML5 小游戏与 Canvas 编程实战 | rAF游戏循环与后台节流/离屏分层渲染/雪碧图与资源预载/虚拟摇杆/场景栈与过渡/Web Audio音效/视差卷轴摄像机/HUD与像素完美/微信小游戏适配/性能与内存排查 | medium 为主 |
| CP01–CP10 | 物理仿真与粒子特效编程 | Verlet积分与约束/质点弹簧布料撕裂/绳索拖拽/压力软体/SPH流体/Boids群体行为/烟花爆炸特效/Voronoi碎裂/物理手感量化/引擎选型边界 | medium~hard |
| CQ01–CQ10 | 推荐系统与个性化工程 | UserCF-ItemCF/矩阵分解ALS/CTR预估LR-FM-DeepFM/多目标融合/多路召回漏斗/冷启动Bandit/多样性MMR/特征泄漏排查/interleaving与护栏/级联排序架构 | medium~hard |
| CR01–CR10 | 地理空间 GIS 与地图编程 | 火星坐标转换/GeoJSON校验/R树四叉树/地理编码/Haversine与地理围栏/路网路径规划/瓦片金字塔/OSM数据处理/等值线热力图/PostGIS空间SQL | medium~hard |
| CS01–CS10 | 金融账务与量化回测编程 | 金额精度decimal/复式记账/流水幂等冲正/K线聚合/技术指标/事件驱动回测/滑点手续费/风控仓位凯利/夏普绩效/paper trading | medium~hard |
| CT01–CT10 | 电商交易与营销系统编程 | 购物车合并/SPU-SKU/预占防超卖/秒杀削峰/优惠券核销/订单状态机关单/支付回调验签/facets搜索/黄牛风控/大促降级预案 | medium~hard |
| CU01–CU10 | 压缩编码与数据校验编程 | RLE-Delta/Huffman/LZ77-LZSS/LZW/算术编码/CRC查表/Base64变体/zstd-LZ4基准/非密码学哈希/Parquet列存编码 | simple~hard |
| CV01–CV10 | 历法时间与重复规则编程 | 闰年与改历/儒略日/农历转换/节气黄经/月相潮汐/ISO8601解析/tzdata与DST/RRule引擎/cron解析/月历调休与iCal | simple~hard |
| CW01–CW10 | 在线判题 OJ 与算法教学平台 | 判题沙箱/测试点子任务/spj与交互题/代码查重/数据生成对拍/三种赛制排名/算法回放可视化/题库难度校准/判题队列弹性/AI错因讲解 | medium~hard |
| CX01–CX10 | 图算法应用与社交网络分析 | CSR存储/PageRank/Louvain社区/双向BFS社交距离/中心性/好友推荐图嵌入/Feed推拉/力导向布局/知识图谱/Pregel超步 | medium~hard |
| CY01–CY10 | 弹性系统与混沌工程实战 | 韧性模式六件套/指数退避抖动四式/熔断器三态机/Bulkhead舱壁/Chaos注入矩阵/GameDay红蓝对抗/blameless postmortem五段论/自适应弹性/优雅降级金字塔/DR演练RTO-RPO | medium~hard |
| CZ01–CZ10 | 人因工程与软件组织行为学 | Conway定律反向利用/Team Topologies四类团队/DORA四指标/SPACE五维度/第二大脑组织记忆飞轮/远程异步writing-first/会议税与决策日志/技术债四象限治理/工程文化与OKR晋升机制 | simple~hard |
| DA01–DA10 | 搜索引擎与信息检索工程 | 倒排索引压缩/BM25调参/查询纠错改写/前缀补全Trie/高亮摘要/点击日志/MAP·NDCG评估/段合并/多字段多语言/FTS5迷你引擎 | medium~hard |
| DB01–DB10 | 即时通讯 IM 系统编程实战 | 长连接心跳/ACK重传去重/seqid排序/离线漫游/已读未读数/写扩散读扩散/历史分页/撤回编辑审核/Signal协议/迷你IM | medium~hard |
| DC01–DC10 | 排版渲染与打印引擎编程 | Knuth-Plass断行/字体度量/Markdown管线/HTML→PDF/撤销重做栈/表格跨页/语法高亮/打印样式/墨水屏/网格版式 | simple~hard |
| DD01–DD10 | 构建系统与编译缓存工程 | Make依赖图/Ninja调度/CAS缓存/远程缓存分布式编译/hermetic沙箱/交叉编译/deb-rpm打包/构建剖析/watch脚本/迷你构建系统 | simple~hard |
| DE01–DE10 | 生物信息学与基因数据编程 | FASTA-FASTQ质量/Smith-Waterman/BLAST种子扩展/de Bruijn组装/VCF变异/NJ建树/PDB结构/单细胞管线/Snakemake/基因合规 | simple~hard |
| DF01–DF10 | 音乐编程与音频合成实战 | MIDI解析/减法合成/步进音序器/FFT调音器/BPM估计/自动伴奏/五线谱渲染/效果器链/马尔可夫作曲/插件宿主 | medium~hard |
| DG01–DG10 | 规则引擎与业务决策自动化 | 规则引擎选型/Rete网络/DMN决策表/规则DSL/核保/实时风控/积分等级/冲突检测/版本灰度/迷你Rete | simple~hard |
| DH01–DH10 | 天文计算与航天编程实战 | 天球坐标转换/简化星历/开普勒方程/日出日落蒙影/TLE与SGP4/星野叠加/凌日BLS/NASA API/FITS格式/霍曼转移 | simple~hard |
| DI01–DI10 | 跨平台 Shell 脚本移植与健壮性工程 | GNU与BSD差异/bash·zsh·fish方言/POSIX子集/shebang与CRLF/trap清理链/getopts/子shell变量丢失/bats测试/PowerShell对译/分发与锁定 | simple~hard |
| DJ01–DJ10 | Linux 服务器治理进阶 | systemd服务化/socket激活/timer治理/journalctl/cgroups配额/批量巡检/密钥轮换/升级窗口/幂等脚本/自愈预案 | medium~hard |
| DK01–DK10 | 游戏品类与系统编程进阶 | 放置离线结算/卡牌效果引擎/优先权与堆/象棋走法Zobrist/UCI协议/信息熵猜词/经济供需/保底PRD/模组加载/热座框架 | medium~hard |
| DL01–DL10 | 模拟器与复古平台编程 | 原理地图/CHIP-8/6502寻址周期/MMIO镜像/Mapper bank/PPU扫描线/APU通道/周期精确/存档回放/调试查看器 | medium~hard |
| DM01–DM10 | 气象与环境数据编程 | GRIB2解析/缺测插补/气候距平SPI/台风路径/雷达Z-R/AQI/分级检验/多源API/风羽填图 | simple~hard |
| DN01–DN10 | 智能家居与家庭自动化编程 | HA实体模型/Zigbee组网/自动化编排/传感器治理/能源统计/本地语音/离线排障/远程安全/持久化备份/仪表盘场景 | simple~hard |
| DO01–DO10 | 数字电路与逻辑仿真编程 | 卡诺图/组合器件/加法器ALU/触发器亚稳态/FSM电路/Verilog子集/事件驱动仿真/延迟冒险/流水线/门级CPU | medium~hard |
| DP01–DP10 | 逻辑编程与约束求解实战 | Prolog统一回溯/迷你解释器/Datalog/CSP建模/传播启发式/DPLL-CDCL/Z3优化/排课诊断/配送松弛/选型判断 | medium~hard |
| DQ01–DQ10 | Windows 系统管理与 PowerShell 运维实战 | cmdlet管道/WMI-CIM/事件日志/注册表ACL/服务进程/组策略/WinRM/计划任务/性能计数器/Windows更新 | medium~hard |
| DR01–DR10 | macOS 桌面运维与 Apple 生态开发实战 | Homebrew/launchd/defaults/AppleScript/plist/diskutil/系统权限/Xcode CLI/Swift脚本/Keychain | medium~hard |
| DS01–DS10 | 容器编排与 Kubernetes 运维实战 | Dockerfile多阶段/Compose健康检查/Pod-Deployment-Service-Ingress/ConfigMap-Secret/PVC/滚动回滚/HPA/Helm/RBAC | medium~hard |
| DT01–DT10 | CI/CD 流水线工程与 GitOps 实战 | GitLab CI/GitHub Actions/Jenkins/构建缓存/制品管理/环境晋升/ArgoCD-Flux/语义版本/质量门禁/密钥注入 | medium~hard |
| DU01–DU10 | 基础设施即代码 IaC 与 Ansible 自动化运维 | Terraform状态与模块/Ansible Role/动态Inventory/Cloud-Init/Packer/Pulumi/Terratest/Drift合规/多云编排 | medium~hard |
| DV01–DV10 | 日志分析与监控告警工程实战 | ELK-Loki/PromQL/Grafana/Alertmanager/grok-regex/慢查询/分布式Trace/SLO错误预算/采样降噪/事件关联 | medium~hard |
| DW01–DW10 | 数据库运维与备份恢复工程实战 | MySQL慢查询/PG VACUUM/Redis持久化/Mongo副本集/全量增量PITR/主从切换/连接池/Schema迁移/归档/CDC | medium~hard |
| DX01–DX10 | 网络安全审计与合规自动化实战 | 端口扫描/漏洞扫描/CIS基线/TLS审计/防火墙/SSH加固/容器扫描/WAF/SIEM关联/应急自动化 | medium~hard |
| DY01–DY10 | 自动化测试框架与测试工程实践 | pytest fixture-parametrize/mock patch/BDD/Pact契约/Locust-k6/视觉回归/测试数据工厂/testcontainers/报告度量/变异测试 | medium~hard |
| DZ01–DZ10 | 办公自动化与文档处理工程实战 | Excel-CSV/清洗转换/PDF生成解析/Word模板/Markdown批量/邮件收发/批量整理/OCR/报告生成/工作流编排 | medium~hard |
| EA01–EA10 | 跨平台应急响应与系统取证实战 | 证据哈希链/时间线归一/进程端口漂移/跨平台持久化/脚本风险/USB外设/下载痕迹/容量deleted-open/处置报告/工具包自检 | simple~hard |
| EB01–EB10 | 可复现实验与科学计算工程 | 假设seed/数据QC/bootstrap/参数扫描/不确定度/失败归档/环境指纹/图表数据/科学验收/复现包 | simple~hard |
| EC01–EC10 | 插件化命令行应用与命令总线工程 | 插件契约/argparse子命令/生命周期钩子/配置覆盖/输出格式器/检查点取消/能力权限/版本协商/错误语义/行为回归 | medium~hard |
| ED01–ED10 | 游戏本地化与全球发布管线 | 字符串抽取/复数性别/伪本地化/字体回退/文化合规/商店元数据/年龄分级/发布清单/补丁说明/LQA门禁 | simple~hard |
| EE01–EE10 | 桌面自动化与用户操作回放 | Action schema/a11y selector/回放幂等/等待重试/证据遮罩/数据矩阵/OS adapter/安全策略/失败诊断/套件趋势 | medium~hard |
| EF01–EF10 | 团队工作流与运营自动化 | 会议行动项/工单路由/值班公平/审批状态机/费用政策/入职清单/KPI汇总/决策RACI/交接Runbook/运营周报 | simple~hard |
| EG01–EG10 | 多媒体资产交付与批量转码 | 资产清单/哈希去重/编码矩阵/转码调度/元数据sidecar/字幕章节/交付包/QC门禁/断点回滚/交接报告 | medium~hard |
| EH01–EH10 | 边缘函数与事件流集成实战 | 事件契约/HMAC幂等/背压/重试DLQ/Outbox/边缘路由/冷启动预算/配置密钥/Trace归因/混沌矩阵 | hard |
| EI01–EI10 | 技术知识库与可验证问答工程 | 来源分级/实体关系/引用定位/矛盾检测/知识图谱/检索计划/证据包/保鲜治理/问答评测/维护工作流 | medium~hard |
| EJ01–EJ10 | 多语言运行时与内存模型实验 | 栈堆逃逸/借用检查/GC实验/缓存布局/内存可见性/异步调度/字符串编码/整数溢出/结构体ABI/运行时选型 | medium~hard |
| EK01–EK10 | 开源许可证合规与依赖义务追踪 | SPDX归一/兼容矩阵/NOTICE/文本漂移/策略门禁/vendored来源/商标义务/替代方案/例外台账/发布证据 | medium~hard |
| EL01–EL10 | 网络数据包解码与流量回放工程 | 链路帧/TCP重组/DNS配对/TLS时间线/HTTP重建/环形索引/黄金回放/抖动QoE/丢包矩阵/匿名化 | medium~hard |
| EM01–EM10 | 游戏公平性与反作弊风控工程 | 遥测契约/物理不变量/行为评分/匹配公平/举报分诊/回放确定性/处罚申诉/权威校正/灰度评估/运营仪表盘 | medium~hard |
| EN01–EN10 | 公共交通GTFS与出行调度仿真 | GTFS审计/日历例外/换乘路由/车辆冲突/延误传播/票务计算/无障碍路径/司机轮班/客流仿真/绕行通知 | medium~hard |
| EO01–EO10 | 微电网储能与能源调度编程 | 负荷分段/预报备用/电池退化/并离网切换/需求响应/峰谷套利/逆变器质检/黑启动/碳核算/参数校准 | medium~hard |
| EP01–EP10 | 合同条款抽取与法务义务追踪 | 条款抽取/义务矩阵/修订差异/提醒升级/主体风险/redline/证据链/模板校验/争议路径/保留审计 | medium~hard |
| EQ01–EQ10 | 数据标注流水线与样本质检工程 | 规范版本/一致性/分歧路由/难例队列/漂移/mask质检/脱敏授权/派单公平/金标陷阱/数据集卡片 | medium~hard |
| ER01–ER10 | 二进制体积与符号膨胀治理 | 段清单/符号聚合/strip变体/优化矩阵/依赖归因/资源膨胀/体积性能权衡/平台差异/预算门禁/回归分派 | medium~hard |
| ES01–ES10 | 仓储物流与库存路径优化 | ABC库位/拣货路径/循环盘点/收货上架/波次批次/退货处置/冷链越限/月台预约/智能装箱/安全库存 | medium~hard |
| ET01–ET10 | 远程开发会话与终端多路复用治理 | tmux清单/命令风险/重连恢复/SSH lint/端口转发/scrollback索引/长任务/结对权限/环境体检/事件回放 | medium~hard |

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

- 新增主题时**先检索本库 1540 个主题名，确认不重复**再追加；编号沿类别字母顺延（Z 之后用 AA/AB/…，AP 之后用 AQ/AR/…，AY 之后用 AZ/BA/BB/…，BP 之后用 BQ/BR/BS/…，CA 之后用 CB/CC/CD/…，CL 之后用 CM/CN/CO/…，CX 之后用 CY/CZ/DA/…，DH 之后用 DI/DJ/DK/…，DP 之后用 DQ/DR/DS/DT/DU/DV/DW/DX/DY/DZ/EA/EB/EC/ED/EE/EF/EG/EH/EI/EJ/EK/EL/EM/EN/EO/EP/EQ/ER/ES/ET/…）。
- 修改 laew 功能后（如新增工具、改档位策略），同步修订受影响条目的「预期档位」。
- 每组提示词控制在 3~5 轮；后轮必须与前轮有显式承接关系，保证"多轮"语义成立。
- 实测发现某条实际档位与预期不符时，先记录现象（写入 testReport/ 验证报告），
  再决定是调预期还是调分类器。
