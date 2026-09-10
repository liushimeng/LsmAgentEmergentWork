# 76 AR/VR 与空间计算

> 编号段 BY01–BY10 · 主题：AR/VR & Spatial Computing（OpenXR 开放生态 / Apple visionOS / Meta Quest / WebXR 浏览器端 / SLAM 与空间定位 / ATW 异步时间扭曲与空间扭曲 / 手柄·手势·眼动·语音多维交互 / 空间锚点与持久化坐标 / HRTF 空间音频与环境遮蔽 / 三维空间 UI/UX 距离与可读性 / MR 场景理解与平面检测 / 注视点渲染与热节流 / three.js 与 Babylon.js 实战）
>
> 互补性说明：本组聚焦 **XR 应用层与空间计算**，与已有的「游戏引擎（渲染管线下游）」「计算机视觉（通用 CV 算法）」「桌面应用（2D GUI）」「3D 建模（DCC 工具链）」均不重叠 —— 本组考察的是 **穿戴式空间设备的运行时、交互范式、浏览器 XR API、空间持久化与舒适 UX**，适合验证 Yolo 对 XR/空间计算领域的任务分类与多轮追问拆解能力。
>
> 维度覆盖：XR 生态标准化（BY01）/ SLAM 与空间定位（BY02）/ ATW 与空间扭曲渲染（BY03）/ 多维交互输入（BY04）/ 空间锚点与持久化（BY05）/ 空间音频 HRTF（BY06）/ 三维 UI/UX 舒适设计（BY07）/ MR 场景理解与遮挡（BY08）/ 注视点渲染与热节流性能（BY09）/ WebXR 与 three.js 实战（BY10）。

---

### BY01 OpenXR 开放生态与跨平台运行时选型

- **预期档位**: hard
- **考察维度**: 对比 OpenXR 1.0/1.1 标准扩展（XR_KHR_composition_layer / XR_FB_hand_tracking / XR_EXT_eye_gaze）在 Apple visionOS、Meta Quest、SteamVR、Windows Mixed Reality 四大平台的运行时支持矩阵与 conformance 测试差异。
- **对话脚本**:
  1. 我正在为一个跨头显的企业培训应用做技术选型。请对比 OpenXR 在 Apple visionOS、Meta Quest 3、SteamVR（Valve Index）和 Windows Mixed Reality 四大平台上的运行时支持情况，重点说明核心 1.0 规范与各厂商扩展（如 XR_FB_hand_tracking、XR_EXT_eye_gaze_interaction）的兼容性差异，以及 compositor swapchain 格式（如 Vulkan/Metal/D3D12）的选择约束。
  2. 上一轮你提到了 Vulkan 与 Metal 的 swapchain 差异。如果我们要在 Quest 上启用 XR_FB_update_skinning 与 XR_FB_passthrough 两个扩展，但同时在 visionOS 上回退到 ARKit 原生坐标对齐，请给出一个基于 openxr_runtime.json 的运行时发现与扩展探测策略，并说明如何让同一份应用代码在两个平台间优雅降级。
  3. 进一步地，我想引入 XR_MSFT_unbounded_reference_space 做大空间定位，但需要在不支持的设备回退到 local_floor_ext。请设计一个三层的 Space 管理抽象（Stage / LocalFloor / Unbounded），用伪代码说明如何按运行时能力注入不同的 reference space，并分析 session lifecycle（从 xrBeginSession 到 lost event）在这三种 Space 下的恢复语义。
  4. 最后，考虑到 OpenXR 的 conformance 测试（conformance suite）需要各厂商实现，请评估 Khronos Conformance Test Suite 对 Quest 与 Pico 4 Enterprise 的实际覆盖盲区，以及我的团队在自研跨平台 XR Runtime 时，哪些测试用例必须自己补全（尤其关注 composition layer 深度排序与 input subsystem 多动作绑定两块）。

---

### BY02 SLAM 与空间定位基础

- **期望档位**: medium
- **考察维度**: 解释视觉惯性里程计（VIO）、点云地图、重定位（relocalization）与闭环检测（loop closure）的协作流程，以及 ARCore/ARKit/Vision Pro 在稀疏/稠密建图上的取舍。
- **对话脚本**:
  1. 我要在 AR 平板应用里实现稳定的小物体放置。请从 VIO（视觉惯性里程计）原理讲起，说明为什么纯视觉 SLAM 会漂移、IMU 融合如何纠正尺度不确定性，并对比 ARCore、ARKit 和 Meta Insight 三类 SLAM 在后端优化（如 Pose Graph / Factor Graph / Bundle Adjustment）策略上的主要差异。
  2. 上一轮你提到了 ARKit 的稠密场景重建（Scene Reconstruction）。当用户在光线较弱、白墙较多的办公室里打开应用，平面检测频繁失败，请分析可能的失效原因（特征点不足、IMU 噪声、纹理缺失），并提出三种客户端侧的补偿策略（如延长初始化等待、引导用户扫动、开启 LiDAR 辅助）。
  3. 现在假设我们需要在同一房间的不同时段（早中晚光线变化）重定位到已建立的地图。请设计一个基于「视觉词袋（BoW）+ 点云描述子 + IMU 先验」的混合 relocalization 流程，描述关键帧数据库的更新策略（何时增、何时删、如何按光照分桶），并分析在 Quest 3 上该流程的算力预算（是否必须 offload 到云端）。
  4. 最后，我想实现多用户共享同一个地图锚点。请对比三种技术路线（ARCore Cloud Anchor、ARKit Collaborative Session、自研基于 QR/ArUco 标记的共享坐标系）的优缺点，重点说明「世界坐标系对齐误差」在 1m、5m、20m 三种距离下对协作体验的具体影响，并给出误差补偿的工程建议。

---

### BY03 渲染与异步时间扭曲（ATW）/ 空间扭曲（ASW/SSW）

- **预期档位**: medium
- **考察维度**: 剖析 ATW/ASW/SSW 三种时间扭曲管线的差异、late-stage reprojection 对 UI 与文本清晰度的影响，以及在单通道立体渲染（Single Pass Stereo）下的扭曲边界处理。
- **对话脚本**:
  1. 我们的 VR 移动应用帧率频繁掉到 55fps，目标刷新率是 72fps。请从「掉帧到扭曲」的完整链路讲起：从 compositor 在 v-sync 前未收到新帧说起，解释 ATW（异步时间扭曲）、ASW（异步空间扭曲）和 SSW（同步空间扭曲）三者的差异、输入依赖（仅头部姿态 vs 深度缓冲 vs 光流），以及分别在 Quest、Pico 4、PCVR 上的典型开启策略。
  2. 上一轮你提到 SSW 需要深度缓冲来估计运动矢量。如果在单通道立体渲染（Single Pass Stereo, instanced）下，UI 层（如文字准星）与场景层分别在不同 draw call 提交且深度测试策略不同，请分析 late-stage reprojection 在扭曲 UI 与 3D 物体时可能产生的伪影（如文字拉伸、边缘锯齿、Z-fighting 扭曲），并给出绘制时的规避建议（分层深度、独立 alpha、HUD 贴屏）。
  3. 我想在应用里加入「动态分辨率 + 注视点渲染 + ATW」三件套联合调优。请给出一个帧时间预算模型（以 72Hz 为例，13.9ms 帧预算），说明当 GPU 耗时超标时，三者应如何按优先级顺序降级（如先降外围分辨率 → 再缩小 foveation 范围 → 最后切到 ASW），并分析这种级联对视觉质量的具体影响。
  4. 进一步地，如果用户在使用 VR 时佩戴近视眼镜导致实际感知的面板 PPI 低于标称值，请定量估算 ATW 引入的几何误差与重投影抖动（reprojection wobble）在边缘视场角（如 Quest 的 95° 边缘）下的像素偏移量，并提出三种客户端补偿手段（如固定 foveation、提高渲染分辨率上限、UI 避让边缘）。

---

### BY04 多维交互输入：手柄 / 手势 / 眼动 / 语音

- **预期档位**: hard
- **考察维度**: 对比 XR 四类主流输入模态的延迟、精度、学习成本与误触率，以及在同一应用中做输入融合（input fusion）时的仲裁策略与 fallback 链。
- **对话脚本**:
  1. 我们的 MR 应用需要同时支持手柄射线、裸手手势、眼动注视和语音命令四种输入。请先建立一张对比表，从「延迟（端到端 ms）」「精度（角度/距离）」「学习成本」「误触率」「环境约束（光照/噪声）」五个维度对比这四种模态，并给出典型数字范围（如手柄射线延迟 ~20ms、手势 ~50ms、眼动 ~30ms、语音 ~300ms+）。
  2. 上一轮你建立了对比表。现在我需要做一个「远场抓取 + 近场操控 + 语音确认」的混合交互。请设计一个输入仲裁状态机：当用户手伸到远处时用射线聚焦、手收回到 30cm 内时切换到直接手触、同时注视点作为候选目标、语音（"确认"/"取消"）作为最终确认。请用伪代码说明事件优先级与竞争解决（如手与眼目标冲突时以谁为准）。
  3. 进一步地，当用户双手在视野外（如背后、放下）时，系统应如何优雅降级输入？请设计一个「输入置信度（confidence）」模型，融合控制器 IMU 活跃度、手部追踪可见性、注视稳定度、语音活动检测四项信号，并给出当 confidence < 0.3 时的三种 fallback 方案（射线保持 / 回退到头动瞄准 / 弹出语音提示）。
  4. 最后，假设我们要在 Quest 3 上做双手指点打字（virtual keyboard）。请分析「手指敲击检测」在裸手追踪下的两种主流方法（基于关节角度变化率 vs 基于指尖速度反向积分），对比它们的误识别率与算力占用，并给出一种基于上下文（如当前焦点在搜索框 vs 普通场景）的自适应敲击灵敏度切换策略。

---

### BY05 空间锚点与持久化坐标

- **预期档位**: medium
- **考察维度**: 对比 ARKit ARAnchor、ARCore Anchor/Geospatial Anchor、OpenXR Spatial Anchor、Azure Spatial Anchors、Niantic Lightship VPS 五类锚点在精度、持久化年限、跨设备共享与离线可用性上的差异。
- **对话脚本**:
  1. 我要做一个需要在商场内跨周、跨设备共享的 AR 导览应用。请先对比 ARKit ARAnchor + ARWorldMap、ARCore Anchor + Cloud Anchors、OpenXR XR_EXT_spatial_anchor、Azure Spatial Anchors (ASA)、Niantic Lightship VPS 五类持久化方案在「首次注册精度」「跨设备漂移」「离线可用」「持久化年限（含厂商策略）」四个维度的表现。
  2. 上一轮你提到 ASA 依赖 Azure 云端。如果我们希望锚点在云端服务不可用时仍能本地持久化，请设计一个「本地 SQLite 锚点库 + 云端 ASA 双写」的混合存储架构，描述锚点创建、更新、删除、再发现的完整流程，并重点分析在「网络分区」场景下本地与云端锚点版本冲突的解决策略（如向量时钟 / 最后写入胜出 / 人工合并）。
  3. 进一步地，同一商场内可能密集部署了上百个锚点，用户头显一次 scan 只看到其中 5–10 个。请设计一个「锚点索引 + 空间邻近查询」的加载策略：用 R-Tree 或 Geohash 组织锚点，当用户移动时按距离远近 LOD 加载 AR 内容（近处完整模型、远处 icon、更远卸载）。给出数据结构选型与查询伪代码。
  4. 最后，假设商场翻新导致某个锚点的真实位置发生偏移（如店铺搬迁）。请设计一个「锚点健康度」监控机制，融合重定位成功率、最近 N 次访问位置方差、用户显式反馈三项信号，自动标记失效锚点并触发重新注册流程，同时向协作用户推送「该锚点暂时不可用」的降级提示。

---

### BY06 空间音频：HRTF 与环境遮蔽

- **预期档位**: medium
- **考察维度**: 剖析 HRTF 个性化（generic vs individualized）、Ambisonics 阶数与环境声混响、射线投射遮挡（occlusion）与透射（transmission）的计算开销，以及在 XR 中对音频源距离衰减曲线的设计。
- **对话脚本**:
  1. 我们的社交 VR 应用里需要 8 个用户同时语音聊天，且每个虚拟声源要有空间方位感。请从 HRTF（头部相关传输函数）讲起：解释通用 HRTF 与个性化 HRTF（基于 3D 头扫 / 耳照片 / 数值仿真）的定位精度差异，并对比 Ambisonics 一阶 / 二阶 / 三阶在还原声场细节与 CPU 占用之间的取舍。
  2. 上一轮你提到个性化 HRTF 需要校准。如果用户拒绝做个性化校准，系统只能用 generic HRTF，那么在前-后混淆（front-back confusion）与仰角判断上会有什么典型问题？请给出三种客户端缓解策略（如加入早期反射混响提供距离线索、用头部微小运动动态辨别前后、利用视觉对齐辅助声源定位），并分析每种策略对计算预算的影响。
  3. 现在我想在场景中加入「房间声学」效果：墙壁对高频的遮挡、走廊的低频透射、大空间的混响尾迹。请设计一个简化的实时声学模拟管线：射线投射做 direct occlusion + 少量 image-source 做早期反射 + 反馈延迟网络（FDN）做后期混响，并给出在 Quest 单核上的 CPU 预算分配建议（如 occlusion 1ms、反射 2ms、混响 1ms、总预算 ≤ 4ms）。
  4. 最后，当 8 路语音流需要同时空间化时，请分析直接在 DSP 上跑 8 路 HRTF 的代价（每路卷积 ~128–512 阶 FIR），并给出两种优化方案：对远距离声源降阶 HRTF + 简化遮挡、对非焦点声源改用 Ambisonics panner 混合，请量化两种方案分别节省的 CPU 百分比。

---

### BY07 三维空间 UI/UX：距离 / 可读性 / 舒适度

- **预期档位**: simple
- **考察维度**: 考察三维空间中文本舒适度（视角、距离、字体大小）、UI 深度层级与遮挡、以及减少晕动症（vection sickness）的静态参考框架（如虚拟鼻 / 地平线 / cockpit）设计。
- **对话脚本**:
  1. 我要在 VR 应用中设计一套 HUD 与可交互面板系统。请从「人眼最小分辨角（~1 arcmin）」出发，计算在 Quest 3（单眼 2064x2208、95° FOV）上，距离眼睛 1m 处可舒适阅读的最小文字像素高度，并给出字号、行距、对比度在室内外两种光照场景下的推荐值。
  2. 上一轮你给出了 1m 距离的字号。当用户佩戴头显在 0.5m（手臂伸直）到 5m（房间对角）之间移动时，UI 的渲染策略应如何变化？请设计三种 LOD：近场（≤1m）可点选控件、中场（1–3m）信息面板、远场（>3m）badge / 箭头指引，并说明每种 LOD 在锚定策略（世界锁 vs 头锁 vs 身体锁）上的选择。
  3. 现在我想引入「虚拟鼻（virtual nose）」和「地平线参考线」两种减少晕动症的手法。请从视觉-前庭冲突（visuo-vestibular mismatch）原理讲起，对比这两种手法在缓解「主动运动（用户自己移动视角）」vs「被动运动（如坐过山车动画）」两种场景下的有效性，并给出各自在 3D 渲染管线中的实现要点（如鼻模型始终渲染在视野下方、深度写入关闭、半透明 alpha ≤ 0.3）。
  4. 最后，考虑到部分用户可能有斜视（strabismus）或单眼弱视，请给出三种 XR UI 层面的无障碍适配方案：单眼兼容布局（避免双眼视差依赖的关键信息）、高对比模式（黄黑 / 蓝黄主题）、大字体放大模式（按当前距离动态缩放），并说明如何在运行时根据系统无障碍设置自动切换。

---

### BY08 MR 场景理解：平面检测 / 物体遮挡 / 场景网格

- **预期档位**: hard
- **考察维度**: 剖析平面检测的类型约束（垂直/水平/任意）、场景网格（Scene Mesh）的更新策略、语义分割与物体遮挡（occlusion）在 MR 中的工程实现，以及与物理引擎的耦合。
- **对话脚本**:
  1. 我要在 MR 应用中实现「虚拟物体放置在真实桌面 + 虚拟球弹跳到真实地面 + 真实人遮挡虚拟物体」的效果。请先对比 ARKit Scene Reconstruction、Meta Scene Model、Microsoft Spatial Mapping 三者在「平面分类（垂直/水平）」「网格更新频率」「语义标签（天花板/地板/桌面/墙）」四个维度的支持差异。
  2. 上一轮你提到了语义标签。假设我想在 ARKit 未直接提供语义的情况下（如只有几何 mesh），用客户端推理补全：请设计一个基于几何启发式 + 轻量神经网络的平面分类流水线，输入为局部 mesh patch（位置 + 法线 + 包围盒），输出为 {Floor, Ceiling, Table, Wall, Other} 五类，并估算在 A17 Pro 神经引擎上的推理延迟（如 ~2ms / patch）。
  3. 进一步地，当真实用户走过遮挡虚拟物体时，我需要实时 occlusion。请对比三种主流遮挡方案：基于深度缓冲的 per-pixel遮挡（如 Quest 的 depth submission）、基于人体分割的 HoloLens 方案、基于实时 SfM 的稠密重建方案，从「遮挡精度」「延迟（ms）」「算力占用」「处理多人」四个维度给出对比表，并说明我们应用在 Meta Quest 上应优先选择哪种及其原因。
  4. 最后，虚拟球与真实地面的物理交互需要「真实地面的实时几何」。请设计一个「动态物理 mesh」方案：从 Scene Mesh 中提取地面 sub-mesh → 异步生成 PhysX/Chaos triangle mesh collider → 当 mesh 变化时增量更新（而非全量重建），请给出增量更新策略（如只更新移动顶点的相邻三角形）与在帧时间内的预算约束（如 ≤ 3ms）。

---

### BY09 性能优化：注视点渲染 / 热节流 / 功耗预算

- **预期档位**: medium
- **考察维度**: 剖析可变速率着色（VRS）/ 注视点渲染（FR）的 GPU 收益、头显热节流（thermal throttling）对帧率的阶梯式影响，以及移动 XR 上 CPU/GPU/功耗的三维预算模型。
- **对话脚本**:
  1. 我们的 VR 游戏在 Quest 3 上经常因热节流从 90Hz 掉到 72Hz 再到 60Hz。请从 XR2 Gen 2 的 thermal design 讲起：解释「功耗墙 → 温度触发 → 频率下调 → 帧率切换」的负反馈链，并对比「主动降渲染负载（动态分辨率 / 关闭后处理）」vs「被动等降频」两种策略对用户体验的差异（画面抖动 vs 短暂模糊）。
  2. 上一轮你提到了动态分辨率。如果我想启用「固定注视点渲染（Fixed Foveated Rendering, FFR）」与「眼动追踪注视点渲染（Eye-tracked Foveated Rendering, EFR）」两者中的一种，请从 GPU 填充率（fillrate）节省、视觉质量损失、硬件依赖三方面对比，并给出在「有眼动追踪的 Quest Pro」与「无眼动追踪的 Quest 3」上各自的推荐档位（如 Quest 3 用 High FFR、Quest Pro 用 EFR + Low 静态 FFR 组合）。
  3. 现在我需要建立一套「帧时间预算自动调节器」：每帧监测 GPU/CPU 耗时与 SoC 温度，按优先级逐步降级。请设计一个五级降级阶梯：① 降阴影分辨率 → ② 降后处理（Bloom/SSAO）→ ③ 降渲染分辨率 → ④ 扩展 FFR 范围 → ⑤ 关闭 SSAO + 锁定 72Hz，并给出每级触发的阈值（如 GPU > 11ms 触发 ①、> 12.5ms 触发 ②）与恢复回切条件（如持续 5 帧 < 10ms 回切）。
  4. 最后，考虑到不同用户的 GPU 体质差异与外界温度（夏天户外 MR vs 冬天室内 VR），请设计一个「热模型自适应」方案：应用启动前 30 秒跑一次基准压力测试（benchmark phase），测量稳态帧率与温度上升斜率，据此自动选择初始画质档位（Low/Medium/High），并给出该基准测试应包含的 3 类场景（密集多粒子、复杂光照、大视场角）及其权重。

---

### BY10 WebXR 与 three.js / Babylon.js 实战

- **预期档位**: hard
- **考察维度**: 剖析 WebXR API 核心流程（session / reference space / hit test / anchors）、three.js 与 Babylon.js 在 XR 模式下的性能/功能差异，以及浏览器端做 SLAM 锚点持久化与跨会话恢复的工程方案。
- **对话脚本**:
  1. 我要用 WebXR 做一个跨手机 AR（ARKit/ARCore via 浏览器）与跨头显 VR（Quest Browser）的 Web 应用。请从 `navigator.xr.requestSession('immersive-ar' | 'immersive-vr')` 讲起，说明两种 session 在功能支持（hit-test / anchors / camera / hand-tracking / depth-sensing）上的差异矩阵，以及在 Chrome Android、Quest Browser、Safari（iOS WebXR polyfill）三个运行时下需要特别处理的兼容性问题。
  2. 上一轮你提到了 WebXR 的功能差异。如果我在 three.js 与 Babylon.js 之间做选型，请对比两者在「XR 相机 rig 管理」「控制器/手势输入抽象」「后处理管线（bloom/SSAO/MSAO）在单通道立体下的表现」「GLTF/USDZ 加载」四个维度的差异，并给出在「我要支持 WebXR hand-tracking + 复杂后处理 + 跨平台」的前提下推荐哪个及具体原因。
  3. 进一步地，用户希望在手机 AR 会话中放置一个锚点并下次打开页面时恢复。请设计一个完整的 WebXR 锚点持久化方案：① 用 `XRHitTestSource` 获取命中点 → ② 创建 `XRAnchor` 并关联虚拟物体 → ③ 将 anchor 的 uuid + 自定义世界坐标系序列化到 IndexedDB → ④ 下次会话启动时用 `XRFrame.createAnchor()` 从持久化 id 恢复，并说明当浏览器或 OS 清理了 SLAM 数据时的「锚点丢失」降级 UI 提示方案。
  4. 最后，当多人通过浏览器进入同一个 VR 房间时，我需要同步各自的手柄与头部姿态。请设计一个最小化的 WebXR 多人同步架构：前端 WebXR 采集 pose + 手骨骼数据 → 通过 WebRTC DataChannel 或 WebSocket 广播到 SFU → 远端用 three.js Avatar 还原。请重点分析「骨骼数据压缩」（如 25 关节 × 4 四元数 + 1 平移 = 9 floats/关节 vs 量化为 16bit 有符号整数）与「发送频率」（头部/手柄 90Hz、手骨骼 30Hz）的带宽预算，估算单用户上行带宽（KB/s），并提出一种基于卡尔曼预测的丢包补偿方案。
