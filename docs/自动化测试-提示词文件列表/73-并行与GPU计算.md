# 73 并行与 GPU 计算

> 编号段 BV01–BV10 · 与 21-系统编程/22-分布式/27-算法竞赛/25-桌面应用多线程 互补：本维度聚焦 **GPU/CUDA 数据并行 + GPU 内存模型 + SIMD + Rust GPU 生态（wgpu/rayon）+ 多卡通信（NCCL）+ Roofline 性能分析**，不覆盖通用多线程/分布式共识/竞赛算法/桌面 UI 线程。

## 维度说明

本维度考察 Agent 在 **GPU 架构（SM/Warps/CUDA cores vs RT/Tensor cores）、CUDA C++ 编程模型（kernel launch / 网格-块-线程）、GPU 内存层次（全局/共享/常量/纹理 + 合并访问）、SIMD（SSE/AVX-512/NEON / Rust `std::simd`）、Rust 并行生态（rayon 并行迭代 / crossbeam 无锁结构）、wgpu 与 Rust 原生 GPU 计算、NCCL/MPI 多卡通信与数据/模型并行、Roofline 性能分析模型、典型并行算法（reduce/scan/稀疏矩阵）、CUDA 调试与 profiling（Nsight Compute）** 等"并行与 GPU 计算"垂直领域中的实战表现。
所有主题都以"动手写并行/GPU 代码或分析性能"为核心，强调数据并行与异构计算工程能力。

---

### BV01 GPU 架构总览：SM、Warps 与三类核心
- **预期档位**: simple
- **考察维度**: GPU 微架构 + 异构核心分类
- **对话脚本**:
  1. 解释 NVIDIA GPU 的 SM（Streaming Multiprocessor）是什么，一个 SM 里包含哪些关键部件（CUDA cores / Tensor cores / RT cores / warp scheduler / register file / shared memory）。
  2. CUDA cores、Tensor cores、RT cores 各自擅长什么计算？为什么深度学习推理主要吃 Tensor cores，而光线追踪离不开 RT cores？
  3. 什么是 Warp？为什么 NVIDIA 的 Warp 大小固定为 32？Warp 内线程的 SIMT 执行和分支 divergence 会带来什么性能问题？
  4. 以 A100/H100 为例，对比两代架构的 SM 数量、Tensor core 代际差异和显存带宽（HBM2e vs HBM3）。

### BV02 CUDA C++ 编程模型：Kernel Launch 与网格-块-线程
- **预期档位**: medium
- **考察维度**: kernel 配置 + 线程索引 + 执行配置
- **对话脚本**:
  1. 写一个最简 CUDA kernel：`__global__ void add(float *a, float *b, float *c, int n)`，实现两个向量的逐元素相加，并给出 host 端调用代码（含 `cudaMalloc` / `cudaMemcpy` / `cudaFree`）。
  2. 解释 `<<<gridDim, blockDim, sharedMemSize, stream>>>` 四个参数的含义。如果 n=1,000,000，你会怎么选 blockDim 和 gridDim？为什么 blockDim 通常选 128 或 256？
  3. 在 kernel 里 `threadIdx.x`、`blockIdx.x`、`blockDim.x` 分别代表什么？推导一维全局索引 `i = blockIdx.x * blockDim.x + threadIdx.x` 的由来。
  4. 加上边界检查 `if (i < n)` 后，当 n 不是 blockDim 整数倍时，最后一个 block 里"多余"的线程会怎样？这种"浪费"在 n 很大时可忽略吗？

### BV03 GPU 内存层次：全局/共享/常量/纹理与合并访问
- **预期档位**: hard
- **考察维度**: 显存层次 + 合并访问 + bank conflict
- **对话脚本**:
  1. 画表对比 GPU 四种主要存储器的位置（on-chip/off-chip）、容量范围、延迟（cycles）、生命周期、典型用途：Global / Shared / Constant / Texture。
  2. 什么是 Coalesced Memory Access（合并访问）？以矩阵按行访问 vs 按列访问为例，解释为什么行访问在 GPU 上快得多，并估算一次非合并访问会浪费多少带宽。
  3. Shared memory 的 bank 是什么？为什么同一 warp 内多个线程访问同一 bank 的不同地址会触发 bank conflict？给出一个 2x 或 4x bank conflict 的具体例子。
  4. 用 shared memory 优化一个 16x16 的矩阵分块乘法（tiling）：写出 kernel 伪代码，说明 `__syncthreads()` 的放置位置，并估算相比 naive 版本能提升多少带宽利用率。

### BV04 SIMD 编程：SSE / AVX-512 / NEON 与 Rust std::simd
- **预期档位**: medium
- **考察维度**: 向量化指令 + 跨平台 SIMD + 自动向量化
- **对话脚本**:
  1. 解释 SIMD 与 SIMT 的区别：SSE/AVX/NEON 是 CPU 侧的"单指令多数据"，而 CUDA 是 GPU 侧的"单指令多线程"。为什么 CPU 上也能做数据并行？
  2. 用 x86 AVX-512 内联函数（intrinsics）写一个 float 数组逐元素相加的函数：`_mm512_loadu_ps` / `_mm512_add_ps` / `_mm512_storeu_ps`，并处理尾部不足 16 个元素的情况。
  3. 用 Rust 的 `std::simd`（portable simd）重写同样的功能，说明 `f32x16` 和 `Simd<f32, 16>` 的语义，以及如何用 `simd_chunks` 处理任意长度数组。
  4. 讨论：编译器"自动向量化"在什么情况下会失败？`#[target_feature(enable = "avx512f")]` 和运行时 `is_x86_feature_detected!("avx512f")` 双路径分发怎么做？

### BV05 Rust 并行生态：rayon 与 crossbeam
- **预期档位**: medium
- **考察维度**: 数据并行 + 工作窃取 + 无锁结构
- **对话脚本**:
  1. rayon 的 `par_iter()` 和 `par_iter_mut()` 背后用的是什么调度算法？解释"工作窃取"（work-stealing）为什么比静态均分更适合负载不均的并行任务。
  2. 用 rayon 并行化一个"大数组求最大值"任务：对比 `par_iter().max()`、`par_iter().fold().reduce()` 和手动 `chunks + scope` 三种写法的性能差异。
  3. crossbeam 的 `ArrayQueue` 和 `SegQueue` 是无锁（lock-free）的，解释 CAS（Compare-And-Swap）和 ABA 问题，crossbeam 如何用 epoch-based reclamation 避免内存回收的 use-after-free。
  4. 用 rayon + crossbeam channel 实现一个生产者-消费者流水线：一个线程读文件、多个线程并行处理、一个线程写结果，保证顺序输出。

### BV06 wgpu 与 Rust 原生 GPU 计算
- **预期档位**: hard
- **考察维度**: 跨平台 GPU + compute shader + WGSL
- **对话脚本**:
  1. wgpu 是什么？它如何做到"一套 Rust 代码跑在 Vulkan / Metal / Direct32 / WebGPU 上"？与直接用 CUDA 写 kernel 相比，wgpu 的优缺点是什么？
  2. 用 wgpu 写一个 compute shader（WGSL 语言）：实现两个矩阵的逐元素相乘。给出 Rust 端的 device/queue 初始化、shader 编译、bind group 绑定和 dispatch 调用代码。
  3. WGSL 的 `@group(0) @binding(0) var<storage, read> input: array<f32>` 和 `@group(0) @binding(1) var<storage, read_write> output: array<f32>` 分别代表什么？`@builtin(global_invocation_id)` 对应 CUDA 里的什么？
  4. 讨论：wgpu 在 Web 端（WebGPU）和原生端的性能差距主要来自哪里？什么场景下你会选 wgpu 而不是 CUDA？

### BV07 多卡通信：NCCL、MPI 与数据/模型并行
- **预期档位**: hard
- **考察维度**: 集合通信 + 并行策略 + 通信隐藏
- **对话脚本**:
  1. NCCL（NVIDIA Collective Communications Library）支持哪些原语（AllReduce / Broadcast / AllGather / ReduceScatter）？为什么分布式训练几乎离不开 AllReduce？
  2. 解释数据并行（Data Parallelism）和模型并行（Model Parallelism）的区别。8 卡训练时，数据并行下每张卡都持有完整模型副本，AllReduce 同步的是什么？
  3. 用 PyTorch 的 `torch.distributed` 写一个最小 DDP 示例：`init_process_group` + `DistributedDataParallel`，并说明 `NCCL_BACKEND` 和 `GLOO_BACKEND` 的适用场景。
  4. 什么是"通信隐藏"（communication hiding）？解释 gradient accumulation + async AllReduce 如何把计算和通信 overlap，给出时序图。

### BV08 Roofline 性能分析模型
- **预期档位**: medium
- **考察维度**: 计算强度 + 性能上界 + 优化方向
- **对话脚本**:
  1. 画一个 Roofline 模型的示意图：横轴是 Arithmetic Intensity（FLOPs/Byte），纵轴是 Attained GFLOPS。解释"内存墙"（memory wall）和"计算墙"（compute roof"分别对应图中的什么。
  2. 一个矩阵乘法 C = A×B（M×K 乘 K×N）的算术强度大约是多少 FLOPs/Byte？为什么 GEMM 通常能撞上 compute roof，而向量加法只能撞上 memory roof？
  3. 如果一个 kernel 实测性能落在 Roofline 曲线的"内存段"左侧，你应该优先优化什么（合并访问 / shared memory tiling / 增大 blockDim）？如果在"计算段"右侧呢（Tensor core / 降低精度 FP16/BF16）？
  4. 用 Nsight Compute 实测一个向量加法的 kernel：你会关注哪些指标（dram__bytes.sum、sm__throughput、smsp__inst_executed）？如何从这些指标判断它是否已经接近理论带宽上限？

### BV09 典型并行算法：Reduce、Scan 与稀疏矩阵
- **预期档位**: hard
- **考察维度**: 并行原语 + 工作效率 + 稀疏存储
- **对话脚本**:
  1. 并行 reduce（求和）的 naive 实现是每步折半相加，总工作量 O(n)，但步数 O(log n)。解释 Brent's theorem 如何把"工作量 × 步数"映射到 p 处理器上的实际耗时。
  2. 写一个高效的 CUDA parallel scan（prefix sum）：用 Blelloch 两阶段（upsweep + downsweep）算法，说明为什么需要 `__syncthreads()` 和 shared memory。
  3. 稀疏矩阵存储格式 CSR / CSC / COO / ELL / HYB 各有什么优劣？什么场景下 CSR 的 SpMV（稀疏矩阵-向量乘）会成为 GPU 上的性能瓶颈？
  4. 用 Rust 的 `sprs` 库实现一个 CSR 矩阵与稠密向量的乘法，并用 rayon 并行化：对比单线程和 8 线程的加速比，解释为什么稀疏矩阵的并行度受行长度方差影响。

### BV10 CUDA 调试与 Profiling：Nsight Compute 实战
- **预期档位**: medium
- **考察维度**: 性能剖析 + 瓶颈定位 + 迭代优化
- **对话脚本**:
  1. Nsight Compute 和 Nsight Systems 的区别是什么？一个看"单个 kernel 的微架构指标"，一个看"全系统时间线"——分别在什么阶段用？
  2. 你有一个矩阵乘法 kernel，Nsight Compute 报告显示 `sm__throughput.avg.pct_of_peak_sustained_elapsed` 只有 15%，但 `dram__throughput.avg.pct_of_peak_sustained_elapsed` 有 85%。这说明瓶颈在哪里？下一步该优化什么？
  3. 解释 Nsight Compute 里"Memory Workload Analysis"中 L1TEX / L2 / Device Memory 的命中率含义。如果 L1 命中率低但 L2 命中率高，说明什么？
  4. 经过 shared memory tiling 优化后，你的 kernel 计算强度从 1 FLOP/Byte 提升到 8 FLOP/Byte，但实测性能只提升了 2 倍。结合 BV08 的 Roofline 模型，分析可能的原因（shared memory bank conflict / occupancy 低 / 同步开销）。
