# 208 端侧 AI 推理与设备端 LLM 工程

> 编号段 GT01–GT10 · 聚焦「在端侧设备上部署与运行 AI 模型的全栈工程」：模型压缩 / 量化与剪枝 / 端侧推理引擎 / NPU 与硬件加速 / 模型格式 / 内存管理 / 端云协同 / 端侧 RAG / 模型热更新 / 端侧评估与监控
>
> 与现有维度互补说明：
> - `82-数字孪生与工业IoT`（DI 维）聚焦**工业场景边缘 AI 部署**（预测性维护/RUL）；本文件聚焦**消费端/移动端/嵌入式的本地大模型与多模态**（手机/平板/笔记本/树莓派/IoT 网关），是更广义的端侧 AI 范畴。
> - `71-WebAssembly深度与边缘计算`（DO 维）聚焦**边缘 Runtime 基础设施**（wasmtime/Capability 安全）；本文件聚焦**模型运行时的工程化**（量化/算子/内存/调度），是"模型怎么在端上跑"。
> - `77-语音交互与对话式UI`（DS 维）聚焦**语音链路**（ASR/TTS/VAD）；本文件聚焦**通用 LLM 与多模态的端侧部署**，可作为语音 Agent 的大脑但不止于语音。
> - `162-Homelab自托管服务与家庭实验室治理`（EO 维）聚焦**家庭服务器自托管**；本文件聚焦**单机端侧推理硬件选型**（CPU/GPU/NPU/Apple Silicon/高通 Hexagon），二者目标设备不同。
>
> **本文件独特主题**：量化 PTQ/QAT INT4-INT8 / 剪枝结构化/非结构化 / GGUF/AWQ/GPTQ/ONNX 格式 / llama.cpp/ollama/MLX/CoreML/TFLite / Apple Silicon Metal / 高通 Hexagon NPU / 端侧内存映射 mmap / 推测解码 / KV cache 量化 / 端云混合推理 / Speculative Edge / 端侧 RAG / Embedding 本地化 / 模型热更新与灰度 / 端侧基准 MMLU/GSM8K 移动版 / 隐私保护边界。

---

### GT01 模型量化 PTQ/QAT 与精度损失

- **预期档位**: medium
- **考察维度**: 对称/非对称 / 静态/动态 / QAT 训练感知
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gt01_quant.py`：量化模拟器（Python，无框架依赖），实现：(a) **对称量化**（max|abs| / scale，zero_point=0）；(b) **非对称量化**（(max-min)/255 scale，(0 - min)/scale zero_point）；(c) **per-tensor / per-channel** 量化粒度（per-channel 对 weight 效果好）；(d) **静态 PTQ**：用校准集（100 个样本）算 min/max + 截断百分位（p99 避免异常值）；(e) **QAT 模拟**：训练时 fake quant（量化-反量化伪节点算子，让 weight 适应量化误差），导出后真正量化；(f) **精度损失度量**：MSE / cosine similarity / KL 散度（前 vs 后激活分布）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/gt01_quant.py && python3 tmpPlan/agent-test/gt01_quant.py`，对一个 4×4 随机权重矩阵做 INT8 量化→反量化，断言 MSE < 1e-3 + KL < 1e-2。
  3. Read `tmpPlan/agent-test/gt01_quant.py`，再 Bash 断言：含对称+非对称、含 PTQ/QAT、含 cosine similarity 计算。

### GT02 GGUF 格式与 llama.cpp 部署

- **预期档位**: medium
- **考察维度**: gguf 结构 / mmap / 推理参数
- **工具链**: Bash → Write → Read → Bash
- **对话脚本**:
  1. Bash：检查环境 `which llama-cli / ollama / ollama --version`（若装），下载一个 7B Q4_K_M GGUF 模型（仅记录下载命令与文件大小，不真下载 100GB）：`huggingface-cli download TheBloke/Llama-2-7B-Chat-GGUF llama-2-7b-chat.Q4_K_M.gguf --local-dir tmpPlan/agent-test/gt02_models`。
  2. Write `tmpPlan/agent-test/gt02_launch.py`：GGUF 推理启动封装（Python subprocess + llama.cpp CLI），实现：(a) **推理参数构造**：`--model` / `--prompt` / `--n-predict 256` / `--temp 0.7` / `--top-p 0.9` / `--top-k 40` / `--repeat-penalty 1.1` / `--ctx-size 2048` / `--threads 8` / `--n-gpu-layers 99`；(b) **GGUF 文件结构解析**：`general.architecture + llama.block_count + llama.embedding_length + llama.attention.head_count + tokenizer.ggml.model` + quantization_version；(c) **mmap 加载**：GGUF 用 mmap 避免全部 read 到内存；(d) **关键参数对资源的影响**：Q4_K_M (4.85 bits/weight) vs Q8_0 (8 bits) vs F16 (16 bits) — 7B 模型内存对比；(e) **跨平台二进制**：Linux/macOS/Windows 都有 llama.cpp release，Apple Silicon 用 Metal 加速。
  3. Bash：`python3 -m py_compile tmpPlan/agent-test/gt02_launch.py && python3 tmpPlan/agent-test/gt02_launch.py --dry-run`，断言生成的命令行与 GGUF 参数解析输出齐全。

### GT03 Apple Silicon MLX 与统一内存架构

- **预期档位**: medium
- **考察维度**: MLX 框架 / Metal / 统一内存
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gt03_mlx.py`：MLX 风格推理概念模拟（Python，无 MLX 依赖），实现：(a) **统一内存 UMA**：M1/M2/M3 系列 CPU/GPU/ANE 共享同一段 DDR5 内存，无需拷贝（vs NVIDIA 独立 VRAM）；(b) **MLX 三件套**：`mlx.core`（类似 NumPy API）/ `mlx.nn`（类似 PyTorch nn）/ `mlx.optimizers`，关键特性 lazy evaluation（图捕获 + 延迟编译 Metal shader）；(c) **Metal Performance Shaders** 加速：MPSGraph 编译算子、AMX 矩阵扩展、ANE（Apple Neural Engine）专用算子；(d) **小模型场景**：iPhone 15 Pro 跑 3B INT4 模型 ~30 token/s（与 llama.cpp Metal backend 对比）；(e) **跨平台挑战**：MLX 仅 Apple Silicon；非 Apple 平台等价物为 NVIDIA CUDA + llama.cpp CUDA backend / AMD ROCm + onnxruntime-rocm。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/gt03_mlx.py && python3 tmpPlan/agent-test/gt03_mlx.py`，非 Mac 平台打印概念演示文档。
  3. Read `tmpPlan/agent-test/gt03_mlx.py`，再 Bash 断言：含 UMA 概念、含 MLX 三件套 API、含 Metal/MPS/ANE 加速路径。

### GT04 ONNX 运行时与跨硬件推理

- **预期档位**: medium
- **考察维度**: onnxruntime / EP / 算子兼容
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gt04_onnx.py`：ONNX 推理封装（Python `onnxruntime` 需安装），实现：(a) **Execution Provider 链式 fallback**：`CUDA EP → TensorRT EP → CPU EP`（按优先级自动选可用硬件）；(b) **IO Binding**：input/output 直接绑 GPU 内存避免 CPU↔GPU 拷贝；(c) **模型优化**：图优化 `session_options.graph_optimization_level = ORT_ENABLE_ALL` + 算子融合；(d) **动态 shape**：`inputs["x"] = np.zeros((1, 3, 224, 224), dtype=np.float32)` 与固定 shape 性能差异；(e) **量化模型**：加载 `model_quantized.onnx`（INT8 校准后的 QDQ 格式）vs FP32，对比内存 + 延迟。
  2. Bash：`pip install onnx onnxruntime --quiet && python3 -m py_compile tmpPlan/agent-test/gt04_onnx.py && python3 tmpPlan/agent-test/gt04_onnx.py --dummy`（dummy 模式构造随机输入跑推理），断言输出 shape 正确 + 列出可用 EP。
  3. Read `tmpPlan/agent-test/gt04_onnx.py`，再 Bash 断言：含 EP fallback、含 IO Binding、含图优化配置。

### GT05 端侧内存管理与 KV cache 优化

- **预期档位**: hard
- **考察维度**: mmap / KV cache 量化 / 推测解码
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gt05_kv.py`：KV cache 内存精算与优化策略（Python），实现：(a) **KV cache 体积公式**：`2 × layers × heads × head_dim × seq_len × bytes_per_elem`（7B 模型 32 层 32 头 128 维 FP16 = 1MB/token → 2048 ctx = 2GB）；(b) **PagedAttention**（vLLM 思想）：把 KV cache 切成固定大小页（如 16 token 一页），按需分配避免碎片；(c) **KV cache INT8 量化**：把 K/V 单独量化到 INT8，内存减半但需注意 RoPE 位置编码兼容性；(d) **Sliding Window Attention**：Mistral 风格只保留最近 N 个 token 的 KV，老的 evict（需配合 Rolling Buffer）；(e) **Speculative Decoding**：小模型 draft → 大模型 verify，可 2-3× 加速但需 batch > 1；(f) **Memory-mapped weight**：GGUF mmap 不占 RSS，需要时按页 demand load。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/gt05_kv.py && python3 tmpPlan/agent-test/gt05_kv.py`，给定 7B + 2048 ctx 参数输出 6 种策略的内存预算表。
  3. Read `tmpPlan/agent-test/gt05_kv.py`，再 Bash 断言：含 KV 体积公式、含 PagedAttention、含 Sliding Window、含 Speculative。

### GT06 端侧 RAG 与本地知识库

- **预期档位**: medium
- **考察维度**: 嵌入式 / 向量库 / 重排
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gt06_rag.py`：端侧 RAG 引擎（Python + sqlite-vec / faiss-cpu），实现：(a) **本地嵌入模型**：用 `all-MiniLM-L6-v2` ONNX 量化版（90MB）替代 OpenAI Embedding API，断网可用；(b) **向量数据库**：`sqlite-vec` 扩展（SQLite 原生支持向量检索）/ `lancedb`（嵌入式列存）/ `chroma`（轻量 Python 内置）；(c) **分块策略**：`RecursiveCharacterTextSplitter(chunk_size=512, overlap=50)` + 文档结构感知（按 Markdown 标题）；(d) **混合检索**：BM25 + 向量相似度 RRF 融合（k=60）；(e) **Rerank**：用本地 cross-encoder (ms-marco-MiniLM) 重排 top 50 → top 5；(f) **隐私边界**：所有嵌入与生成 100% 本地，云 API 仅作可选用 fallback。
  2. Bash：`pip install sentence-transformers faiss-cpu rank-bm25 --quiet && python3 -m py_compile tmpPlan/agent-test/gt06_rag.py && python3 tmpPlan/agent-test/gt06_rag.py`，对 5 段示例文档索引 + 1 个查询，断言返回 top-3 包含答案片段 + 全程无网络请求（用 `requests` mock 验证）。
  3. Read `tmpPlan/agent-test/gt06_rag.py`，再 Bash 断言：含 RRF、含本地 cross-encoder rerank、含 sqlite-vec 或 faiss 选择说明。

### GT07 模型热更新与差分升级

- **预期档位**: medium
- **考察维度**: 差分 patch / A/B / 回滚
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gt07_update.py`：端侧模型热更新器（Python），实现：(a) **更新检查**：启动时后台线程 GET `https://models.example.com/api/latest?current=v1.2.3`，对比 semver + 模型 hash；(b) **差分下载**：服务端 `bsdiff` 生成 patch / 客户端 `bspatch` 应用（vs 全量下载百 MB GGUF，patch 可能仅 50MB）；(c) **原子切换**：新模型下载到 `models/v1.2.4/model.gguf.tmp`，fsync 后 rename 覆盖 + atomic symlink flip；(d) **A/B 灰度**：50% 请求走新模型，统计 latency/quality/error_rate 决定是否放量；(e) **自动回滚**：若新模型 5xx 率 > 阈值或 latency P99 > 旧模型 2×，自动切回旧版本；(f) **断点续传**：HTTP Range + ETag 验证，避免弱网下重新下载。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/gt07_update.py && python3 tmpPlan/agent-test/gt07_update.py --simulate`，断言模拟完整流程：检查→下载→切换→监控→回滚决策。
  3. Read `tmpPlan/agent-test/gt07_update.py`，再 Bash 断言：含 bsdiff/bspatch、含 A/B 灰度、含自动回滚策略、含 HTTP Range 续传。

### GT08 端侧基准测试与质量评估

- **预期档位**: medium
- **考察维度**: tok/s / TTFT / 内存 / 精度
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gt08_bench.py`：端侧推理基准（Python），实现：(a) **吞吐指标**：`tokens/s`（生成阶段）+ `TTFT`（Time To First Token，首 token 时延）+ `prefill_time`（提示词处理时间）；(b) **质量指标**：用 `lm-eval-harness` 子集（MMLU 5-shot 子集 100 题 / GSM8K 50 题 / HumanEval 10 题）在端侧推理上跑，统计 accuracy；(c) **资源指标**：`RSS` 常驻内存 / `peak GPU memory` / `CPU %` / `温度`（macOS `osascript`、Linux `sensors`、Windows WMI）；(d) **不同 batch/ctx 下的扩展性**：batch=1 vs 4 vs 8 / ctx=512 vs 2048 vs 8192 的 latency & memory；(e) **报告输出**：JSON + Markdown 表格 + 是否达"流畅聊天"标准（>20 tok/s）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/gt08_bench.py && python3 tmpPlan/agent-test/gt08_bench.py --dry-run`，断言报告框架生成（含字段定义与单位说明）；若环境有模型则跑真实基准。
  3. Read `tmpPlan/agent-test/gt08_bench.py`，再 Bash 断言：含 tok/s/TTFT/RSS 指标、含 MMLU 子集评估、含温度监控跨平台方案。

### GT09 高通 Hexagon NPU 与 Android NNAPI

- **预期档位**: medium
- **考察维度**: Hexagon DSP / NNAPI / 设备矩阵
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gt09_mobile.py`：移动端 NPU 部署概念指南（Python 文档生成），实现：(a) **设备矩阵**：高通 Snapdragon 8 Gen 3 (Hexagon V73 + Adreno 750) / Apple A17 Pro (Neural Engine 35 TOPS) / MediaTek Dimensity 9300 (APU 790) / Samsung Exynos 2400 (NPU 17 TOPS) —— 性能对比表；(b) **NNAPI / Core ML** 框架调用链：Android 应用 → NNAPI delegate → Hexagon DSP / Adreno GPU / Cortex-A CPU；iOS 应用 → Core ML → ANE；(c) **算子覆盖**：NPU 强 GEMM/Conv/Attention，弱 dynamic shape / sparse / custom op；fallback 到 CPU；(d) **端侧模型部署**：`llama.cpp Android NDK build` + `QNN SDK` (高通) + `execuTorch` (Meta PyTorch 端侧) 对比；(e) **iOS 与 Android 性能差**：iPhone 15 Pro 跑 Phi-3 3.8B INT4 ~25 tok/s，Pixel 8 Pro 跑 ~18 tok/s（同模型同量化）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/gt09_mobile.py && python3 tmpPlan/agent-test/gt09_mobile.py > tmpPlan/agent-test/gt09_mobile.md`，断言退出码 0 且输出含设备对比表。
  3. Read `tmpPlan/agent-test/gt09_mobile.md`，再 Bash 断言：含 4 个 SoC 平台对比、含 NNAPI/Core ML 调用链、含 execuTorch/QNN SDK 工具链。

### GT10 端云协同与隐私边界设计

- **预期档位**: hard
- **考察维度**: 分级路由 / 端云 RAG / 隐私契约
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gt10_split.py`：端云协同路由引擎（Python），实现：(a) **请求分级**：本地能答的（简单 QA / 个人笔记查询）→ 端侧模型 / 需要世界知识的（最新新闻 / 复杂推理）→ 云端大模型 / 敏感数据（医疗/财务）→ 必走端侧；(b) **Speculative Edge**：端侧小模型先 draft N 个 token，云端大模型 verify（节省云端 tokens 70%+ 但保留大模型质量）；(c) **端云 RAG**：本地文档先在端侧 Embedding + 检索 top-k，再把片段 + query 发云端大模型综合；(d) **隐私契约**：明确定义哪些数据上云（用户可关闭）+ 端侧永远处理（PII 字段不出端）；(e) **断网降级**：网络失败时 fallback 到纯端侧 + 队列暂存待恢复后异步同步（与 D13 离线模式集成）；(f) **成本优化**：云端 token 账单监控 + 端云比自动调整（白天 4G 偏云 / 夜间 WiFi 多云）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/gt10_split.py && python3 tmpPlan/agent-test/gt10_split.py`，模拟 5 类请求（个人笔记 / 翻译 / 最新新闻 / 医疗问诊 / 代码生成）展示路由决策。
  3. Read `tmpPlan/agent-test/gt10_split.py`，再 Bash 断言：含 5 类请求分类、含 Speculative Edge、含端云 RAG、含断网降级、含成本监控。