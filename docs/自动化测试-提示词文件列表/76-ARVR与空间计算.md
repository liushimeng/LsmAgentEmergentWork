# 76 AR/VR 与空间计算

> 编号段 BY01–BY10 · 主题：AR/VR & Spatial Computing（OpenXR 开放生态 / Apple visionOS / Meta Quest / WebXR 浏览器端 / SLAM 与空间定位 / ATW 异步时间扭曲与空间扭曲 / 手柄·手势·眼动·语音多维交互 / 空间锚点与持久化坐标 / HRTF 空间音频与环境遮蔽 / 三维空间 UI/UX 距离与可读性 / MR 场景理解与平面检测 / 注视点渲染与热节流 / three.js 与 Babylon.js 实战）
>
> 运行环境约束：**无头显 / 无 GPU / 无 3D 引擎**，全部以 **纯 python3 + numpy + PIL 软件光栅等价实现** 跑通。SLAM 降级为 EKF 位姿估计模拟；WebXR 降级为 python 解析 3D JSON 场景结构；空间音频降级为距离衰减模型；注视点渲染降级为分辨率分布统计。产物落 `tmpPlan/agent-test/` 沙盒。

---

### BY01 OpenXR 开放生态与跨平台运行时选型

- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-09_08-S01-S10-AI工程LLM应用测试脚本重构方案.md）
- **预期档位**: hard
- **考察维度**: 平台兼容矩阵 + 扩展探测 + 降级策略
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/by01/` 下用 Write 写 `platform_matrix.md`：表格 4 行（Apple visionOS / Meta Quest 3 / SteamVR / Windows MR），列：核心 1.0 支持率、Hand Tracking 扩展、Eye Gaze 扩展、Compositor Swapchain 后端、Passthrough 支持；Bash `grep -c '^|' platform_matrix.md` ≥ 6 + `grep -F 'Quest 3' platform_matrix.md` 命中。
  2. 写 `runtime_probe.py`：模拟 `openxr_runtime.json` 发现与扩展探测——`RuntimeProbe` 类含 `probe_extensions(runtime_name)` 返回支持的扩展列表（如 Quest 支持 `XR_FB_hand_tracking`，visionOS 支持 `XR_ARKit_spatial_tracking`）；Bash 跑 `python runtime_probe.py` 后断言 `probe("quest3")` 含 `"XR_FB_hand_tracking"`，写入 `probe.out`。
  3. 写 `graceful_degrade.py`：实现"按运行时能力注入"——`SpaceManager` 根据探测结果选择 `Stage` / `LocalFloor` / `Unbounded` 三层抽象；Bash 跑后断言 Quest 注入 `LocalFloor`，visionOS 注入 `Stage`，写入 `degrade.out`。
  4. 写 `lifecycle_recovery.md`：markdown 写 session lifecycle 从 `xrBeginSession` 到 `lost event` 的三种 Space 恢复语义（Stage 重置 origin / LocalFloor 保留高度 / Unbounded 重新建立锚点）；Bash `wc -l lifecycle_recovery.md` ≥ 15 + `grep -F 'xrBeginSession' lifecycle_recovery.md` 命中。

### BY02 SLAM 与空间定位基础（EKF 位姿模拟）

- **预期档位**: medium
- **考察维度**: VIO 原理 + EKF 模拟 + relocalization
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/by02/` 下用 Write 写 `vio_overview.md`：markdown 解释 VIO 原理（视觉 + IMU 融合）+ 纯视觉 SLAM 漂移原因 + IMU 如何纠正尺度不确定性；配 ASCII 流程图；Bash `grep -F 'IMU' vio_overview.md` 命中 + `wc -l vio_overview.md` ≥ 12。
  2. 写 `ekf_pose.py`：实现 2D EKF 位姿估计——状态 `(x, y, theta)`，预测步用 IMU 角速度/线速度积分，更新步用模拟观测（landmark 距离 + 角度）；Bash 跑 `python ekf_pose.py` 后断言位姿估计误差 < 0.1m（模拟 100 步后），写入 `ekf.out`。
  3. 写 `relocalize.py`：模拟 relocalization——给定当前观测与数据库 keyframe 用 BoW 风格匹配（简化为余弦相似度），找到最相似的 keyframe 后用其位姿作为先验；Bash 跑后断言匹配命中正确 keyframe，写入 `reloc.out`。
  4. 写 `multiuser_anchor.md`：markdown 对比 ARCore Cloud Anchor / ARKit Collaborative Session / QR/ArUco 共享坐标系 3 种方案的"世界坐标对齐误差"在 1m/5m/20m 三种距离下的具体影响；Bash `grep -c '^|' multiuser_anchor.md` ≥ 6 + `grep -F 'Cloud Anchor' multiuser_anchor.md` 命中。

### BY03 渲染与异步时间扭曲 ATW/ASW/SSW（python 软件光栅）

- **预期档位**: medium
- **考察维度**: 帧预算 + 扭曲管线 + reprojection 伪影
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/by03/` 下用 Write 写 `frame_budget.py`：模拟 72Hz 帧时间预算 13.9ms——分配 AEC 1ms / VAD 0.5ms / ASR 2ms / LLM 5ms / TTS 2ms / 网络 1ms / 渲染 2ms / 缓冲 0.4ms；Bash 跑 `python frame_budget.py` 后断言总和不超 13.9ms，写入 `budget.out`。
  2. 写 `atw_asw_ssw.md`：markdown 对比 ATW（仅头部姿态）、ASW（深度缓冲估计运动矢量）、SSW（光流估计运动矢量）三种扭曲管线的输入依赖与典型场景；Bash `grep -F 'ATW' atw_asw_ssw.md` 与 `grep -F 'SSW' atw_asw_ssw.md` 双命中。
  3. 写 `software_raster.py`：用 PIL + numpy 实现"立体几何软件光栅"——给定左右眼视差 `d`，把 RGB 图偏移 `d/2` 像素生成左右眼视图；Bash 跑 `python software_raster.py` 后 `ls -la stereo_*.png` 至少 2 张图，写入 `raster.out`。
  4. 写 `reprojection_artifacts.md`：markdown 列出 late-stage reprojection 在 UI 层（文字准星）与场景层分别的伪影（文字拉伸、边缘锯齿、Z-fighting 扭曲）+ 规避建议（分层深度、独立 alpha、HUD 贴屏）；Bash `grep -c '^- ' reprojection_artifacts.md` ≥ 4。

### BY04 多维交互输入：手柄 / 手势 / 眼动 / 语音（输入置信度模型）

- **预期档位**: hard
- **考察维度**: 多模态对比 + 仲裁状态机 + 置信度
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/by04/` 下用 Write 写 `input_compare.md`：表格 4 行（手柄射线 / 裸手手势 / 眼动注视 / 语音命令），列：延迟 ms、精度、学习成本、误触率、环境约束；填入典型数字范围（手柄 ~20ms / 手势 ~50ms / 眼动 ~30ms / 语音 ~300ms）；Bash `grep -c '^|' input_compare.md` ≥ 6 + `grep -F 'ms' input_compare.md` 命中。
  2. 写 `input_fsm.py`：实现输入仲裁状态机——状态 `FarRay`（>30cm 射线） / `NearTouch`（≤30cm 直接手触） / `EyeTarget`（注视候选） / `VoiceConfirm`（"确认"/"取消"），事件驱动转移；Bash 跑 `python input_fsm.py` 后断言远到近转移正确，写入 `fsm.out`。
  3. 写 `confidence_model.py`：实现输入置信度融合——`confidence = 0.4*ctrl_imu + 0.3*hand_vis + 0.2*eye_stab + 0.1*voice_active`（各分量 0-1）；Bash 跑后断言 confidence < 0.3 时返回 fallback 信号，写入 `conf.out`。
  4. 写 `tap_detect.md`：markdown 对比"基于关节角度变化率"vs"基于指尖速度反向积分"两种敲击检测方法 + 自适应灵敏度切换策略；Bash `grep -c '^|' tap_detect.md` ≥ 4 + `grep -F 'tap' tap_detect.md` 命中。

### BY05 空间锚点与持久化坐标（SQLite + Geohash 模拟）

- **预期档位**: medium
- **考察维度**: 锚点存储 + R-Tree 索引 + 健康度
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/by05/` 下用 Write 写 `anchor_compare.md`：表格 5 行（ARKit ARAnchor / ARCore Anchor / OpenXR XR_EXT_spatial_anchor / Azure Spatial Anchors / Niantic Lightship VPS），列：首次注册精度、跨设备漂移、离线可用、持久化年限；Bash `grep -c '^|' anchor_compare.md` ≥ 6 + `grep -F 'Azure' anchor_compare.md` 命中。
  2. 写 `anchor_db.py`：用 python 内置 sqlite3 建锚点表 `anchors(id, x, y, z, qw, qx, qy, qz, payload, created_at)`，插入 20 个测试锚点，按距离 `(0,0,0)` 排序输出最近 5 个；Bash 跑 `python anchor_db.py` 后断言距离递增，写入 `anchor.out`。
  3. 写 `geohash_index.py`：实现 Geohash 编码——`(x, y)` 转 8 字符 geohash；给定用户位置 `(0,0)` 查邻近 8 个 hash 前缀；Bash 跑后断言返回正确邻近，写入 `gh.out`。
  4. 写 `anchor_health.py`：实现"锚点健康度"评分——`score = 0.5*reloc_success + 0.3*pos_variance_inv + 0.2*user_feedback`；健康度 < 0.4 标记失效；Bash 跑后断言失效锚点被标记，写入 `health.out`。

### BY06 空间音频：HRTF 与环境遮蔽（距离衰减 + 简化滤波）

- **预期档位**: medium
- **考察维度**: HRTF + Ambisonics + occlusion
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/by06/` 下用 Write 写 `hrtf_basics.md`：markdown 解释 HRTF 个性化（generic vs individualized）+ Ambisonics 一阶/二阶/三阶对比（还原细节 vs CPU 占用）；Bash `grep -F 'HRTF' hrtf_basics.md` 命中 + `grep -c '^|' hrtf_basics.md` ≥ 4。
  2. 写 `hrtf_sim.py`：用 numpy 实现简化的 HRTF——左右耳时间差 `itd = sin(angle) * ear_distance / sound_speed`，强度差 `ild = cos(angle)`；Bash 跑 `python hrtf_sim.py` 后断言左/右耳信号不同，写入 `hrtf.out`。
  3. 写 `occlusion.py`：模拟 occlusion + 早期反射——给定声源/听者/障碍物位置，计算遮挡衰减（`factor = 0.0 if blocked else 1.0`）+ 1 个 image source 反射（墙面镜像）；Bash 跑后断言有遮挡时音量衰减，写入 `occ.out`。
  4. 写 `multi_source.py`：模拟 8 路语音同时空间化——`SpatialMixer` 用 numpy 加权和混合 8 个 HRTF 处理后的信号，断言输出 shape == (N,) 且峰值 < 1（防溢出），写入 `mix.out`。

### BY07 三维空间 UI/UX：距离 / 可读性 / 舒适度（PIL 字号验证）

- **预期档位**: simple
- **考察维度**: 字号计算 + LOD + 晕动症缓解
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/by07/` 下用 Write 写 `font_size_calc.py`：用 1 arcmin = 1/60 度计算"Quest 3 单眼 2064×2208、95° FOV、距离 1m"下最小舒适字号（像素高度 = 距离 × tan(1/60 deg) × PPI_inch）；Bash 跑 `python font_size_calc.py` 后断言字号 > 20 px，写入 `font.out`。
  2. 写 `lod_strategy.md`：markdown 写三种 LOD——近场 ≤1m 可点选控件 / 中场 1-3m 信息面板 / 远场 >3m badge 指引；Bash `grep -c '^|' lod_strategy.md` ≥ 4 + `grep -F 'LOD' lod_strategy.md` 命中。
  3. 写 `virtual_nose.py`：用 PIL 渲染"虚拟鼻"图像——固定半透明灰条 `alpha=0.3`，放在视野下方中央；Bash 跑 `python virtual_nose.py` 后 `ls nose.png` 存在，写入 `nose.out`。
  4. 写 `a11y_layout.md`：markdown 列 3 种 XR 无障碍适配——单眼兼容布局 / 高对比模式（黄黑 / 蓝黄）/ 大字体放大模式 + 运行时根据系统设置自动切换；Bash `grep -c '^- ' a11y_layout.md` ≥ 3 + `wc -l a11y_layout.md` ≥ 8。

### BY08 MR 场景理解：平面检测 / 物体遮挡 / 场景网格（几何启发式）

- **预期档位**: hard
- **考察维度**: 平面分类 + 遮挡方案 + 物理 mesh
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/by08/` 下用 Write 写 `plane_compare.md`：表格 3 行（ARKit Scene Reconstruction / Meta Scene Model / Microsoft Spatial Mapping），列：平面分类、网格更新频率、语义标签支持；Bash `grep -c '^|' plane_compare.md` ≥ 5。
  2. 写 `plane_classifier.py`：用 numpy + 法线启发式分类——输入 mesh patch（位置 + 法线 + 包围盒），按法线方向分类（朝上=Floor / 朝下=Ceiling / 水平=Table / 垂直=Wall / 其他=Other）；Bash 跑后断言分类正确率 > 80%，写入 `cls.out`。
  3. 写 `occlusion_strategies.md`：markdown 对比三种遮挡方案（深度缓冲 per-pixel / 人体分割 / SfM 稠密重建）的精度、延迟 ms、算力占用、处理多人能力；Bash `grep -c '^|' occlusion_strategies.md` ≥ 5 + `grep -F 'occlusion' occlusion_strategies.md` 命中。
  4. 写 `dynamic_mesh.py`：模拟"动态物理 mesh 增量更新"——给定一组移动顶点（标记 `moved=True`），更新其相邻三角形；Bash 跑后断言只更新移动顶点的邻接三角形，写入 `mesh.out`。

### BY09 性能优化：注视点渲染 / 热节流 / 功耗预算（python 阶梯模拟）

- **预期档位**: medium
- **考察维度**: 降级阶梯 + 热模型 + benchmark
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/by09/` 下用 Write 写 `thermal_model.py`：模拟"功耗墙→温度→频率→帧率"负反馈——给定 SoC 功耗 5W、温度阈值 45°C，超过则帧率 90→72→60 阶梯降；Bash 跑 `python thermal_model.py` 后断言降级阶梯正确触发，写入 `thermal.out`。
  2. 写 `ffr_vs_efr.md`：markdown 对比 Fixed Foveated Rendering (FFR) vs Eye-tracked Foveated Rendering (EFR)——填充率节省、视觉质量损失、硬件依赖；推荐 Quest 3 用 High FFR、Quest Pro 用 EFR + Low FFR；Bash `grep -F 'FFR' ffr_vs_efr.md` 与 `grep -F 'EFR' ffr_vs_efr.md` 双命中。
  3. 写 `degrade_ladder.py`：实现 5 级降级阶梯——① 降阴影分辨率 → ② 降后处理 → ③ 降渲染分辨率 → ④ 扩展 FFR 范围 → ⑤ 关闭 SSAO + 锁定 72Hz；给定 GPU 时间序列，模拟自动降级与恢复；Bash 跑后断言触发条件正确，写入 `ladder.out`。
  4. 写 `benchmark_phase.py`：模拟"启动前 30 秒基准压力测试"——测量稳态帧率与温度上升斜率，按斜率选择初始画质档位；Bash 跑后断言档位选择合理，写入 `bench.out`。

### BY10 WebXR 与 three.js / Babylon.js 实战（JSON 场景解析）

- **预期档位**: hard
- **考察维度**: WebXR API + three.js/Babylon.js + 多人同步
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/by10/` 下用 Write 写 `webxr_session.md`：markdown 对比 `immersive-ar` vs `immersive-vr` 两种 session 在 hit-test / anchors / camera / hand-tracking / depth-sensing 上的功能差异矩阵；Bash `grep -c '^|' webxr_session.md` ≥ 6 + `grep -F 'immersive-ar' webxr_session.md` 命中。
  2. 写 `scene_parser.py`：解析 GLTF 风格的 JSON 场景文件——读 `scenes/nodes/meshes` 字段，统计 mesh 数、顶点总数、triangle 数；Bash 跑 `python scene_parser.py < scene.json` 后断言解析正确，写入 `parse.out`。
  3. 写 `anchor_persist.py`：模拟 WebXR 锚点持久化——`Anchor(uuid, pos, quat)` 序列化为 JSON 存 IndexedDB 风格本地文件；下次会话从文件恢复；Bash 跑后断言 round-trip 一致，写入 `persist.out`。
  4. 写 `multi_user_sync.py`：模拟多人姿态同步——25 关节 × 4 floats 编码为 100 floats/帧，90Hz 上行带宽 = 100 × 4 × 90 = 36KB/s/用户；Bash 跑后断言带宽计算正确，写入 `sync.out`。
