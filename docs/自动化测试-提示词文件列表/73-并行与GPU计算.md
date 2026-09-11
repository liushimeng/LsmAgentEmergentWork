# 73 并行与 GPU 计算

> 编号段 BV01–BV10 · 与 21-系统编程/22-分布式/27-算法竞赛/25-桌面应用多线程 互补：本维度聚焦 **GPU/CUDA 数据并行 + GPU 内存模型 + SIMD + Rust GPU 生态（wgpu/rayon）+ 多卡通信（NCCL）+ Roofline 性能分析**，不覆盖通用多线程/分布式共识/竞赛算法/桌面 UI 线程。
>
> 本批测试运行环境约束：**无 GPU/CUDA/OpenCL 硬件**，所有并行范式以纯 CPU + numpy/python multiprocessing + asyncio 跑通，CUDA/SIMD 用 numpy 矢量化 + python `concurrent.futures` 等价模拟；Roofline 模型用 numpy 统计 flop/byte 比率近似。产物落 `tmpPlan/agent-test/` 沙盒。

---

### BV01 GPU 架构总览：SM、Warps 与三类核心（CPU 模拟）

- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-09_08-S01-S10-AI工程LLM应用测试脚本重构方案.md）
- **预期档位**: simple
- **考察维度**: 微架构映射 + SIMT 分支发散 + Roofline 概念
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bv01/` 下用 Write 写 `sm_sim.py`：把一个 1024 元素的 numpy `np.arange(1024)` 当作"SM 的 32 路 warp × 32 个 warp"展开；脚本打印 SM 内部组成（CUDA core 计数、warp scheduler、register file 容量）作为注释字典 `SM = {"cuda_cores": 64, "warps_max": 32, ...}`，并写 `gpu_arch.md` 用 markdown 表格列出 CUDA/Tensor/RT 三类核心的擅长场景（矩阵乘 / 注意力 / 光线追踪 BVH）；用 Bash 跑 `python sm_sim.py > sm.out && head -20 sm.out` 断言 `grep -F '"cuda_cores": 64' sm.out` 命中。
  2. 写 `warp_sim.py`：模拟 32 路 SIMT 执行——给一组输入 `[0]*32 + [1]*16 + [2]*16`（前 32 全 0、中间 16 全 1、后 16 全 2），按"warp 内 if 谓词相同走快路径、否则串行化"统计 cycle：均匀分支 8 cycle、分支发散 16 cycle；用 Bash 跑 `python warp_sim.py` 后 `grep -F "uniform=8" warp.out` 与 `grep -F "divergent=16" warp.out` 都必须命中，否则 Read 修谓词分组逻辑。
  3. 写 `roofline_intensity.py`：随机生成两个 1024×1024 float32 矩阵 `A` `B`，调用 `A @ B` 算 GFLOPs（`2*N**3 / 1e9`）与最小字节传输（`3 * N*N * 4 / 1e9`，读 A 读 B 写 C），打印 `intensity = flops / bytes`；预期 GEMM 的 intensity 在 80-90 FLOPs/Byte 量级；Bash 跑后 `awk '/intensity/ {print $NF}' roof.out` 收集数值，断言 `> 50`。
  4. 加 a100 vs h100 对比表 `arch_compare.md`：用 markdown 表格列 SM 数（108 / 132）、Tensor core 代际（3 / 4）、HBM 带宽（2.0 / 3.35 TB/s）、FP16 Tensor TFLOPS（312 / 989）；用 Bash `grep -c '^|' arch_compare.md` 行数 ≥ 6 行表格才视为合规；跑 `wc -l arch_compare.md` 验证文件 ≥ 10 行。

### BV02 CUDA C++ 编程模型：Kernel Launch 与网格-块-线程（numpy 等价）

- **预期档位**: medium
- **考察维度**: 索引映射 + grid/block 划分 + 边界检查
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bv02/` 下用 Write 写 `vec_add.py`：用 numpy 模拟 `__global__ void add(float*, float*, float*, n)`，函数 `vec_add(a, b, n, block=128)` 按 `block` 划分"block"，每个 block 内 `i = block_id * block + thread_id` 算 `c[i] = a[i] + b[i]`；`host` 端用 `a = np.random.randn(N).astype(np.float32)`、`b = np.random.randn(N).astype(np.float32)`、`c = np.zeros_like(a)`，跑 `vec_add(a, b, n=1_000_000)`；Bash 跑 `python vec_add.py` 后 `python -c "import numpy as np; assert np.allclose(c, a+b); print('OK')"` 必须 `OK`。
  2. 写 `grid_block_choice.md`：markdown 写"为什么 blockDim 选 128 / 256"的两段解释（warp 32 倍数 + register 压力 + occupancy），含 1m 元素的 grid/block 划分表（block=128 → grid=7813 余 16）；Bash 跑 `grep -F 'blockDim=128' grid_block_choice.md` 与 `grep -F 'grid=7813' grid_block_choice.md` 双断言。
  3. 写 `index_derive.py`：函数 `global_1d(block_idx, thread_idx, block_dim)` 返回 `block_idx * block_dim + thread_idx`，循环 5 组 (block, thread, dim) 输出全局索引；Bash 跑后 `awk '/idx/ {print $NF}' idx.out` 收集数值断言（0,0,128)→0 / (1,0,128)→128 / (5,17,128)→657 与预期一致。
  4. 写 `boundary_check.py`：用 `n = 1_000_003`（非 128 整数倍），跑 `vec_add` 并断言 `np.allclose(c[:1_000_000], a[:1_000_000]+b[:1_000_000])` 与 `c[1_000_000:] == 0`（多余线程不写入）；Bash 跑 `python boundary_check.py` 后 `grep -F "BOUNDARY_OK" boundary.out` 必须命中，否则 Read 修边界 `if i < n` 分支重跑。

### BV03 GPU 内存层次与合并访问（numpy stride 模拟）

- **预期档位**: hard
- **考察维度**: 内存层次 + 合并访问 + bank conflict 等价
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bv03/` 下用 Write 写 `memory_table.md`：markdown 表格 4 行（Global / Shared / Constant / Texture），列：位置、容量、延迟 cycles、生命周期、典型用途；Bash `grep -c '^|' memory_table.md` ≥ 6（表头+分隔+4 数据行）才合规；`wc -l memory_table.md` ≥ 10 行说明含解释段。
  2. 写 `coalesced.py`：用 numpy stride 模拟"合并 vs 非合并"——对 1024×1024 float32 矩阵，分别按 `arr[i, :].sum()`（行访问，合并）与 `arr[:, i].sum()`（列访问，非合并）跑 1000 次循环，统计耗时；Bash `python -c` 用 `time.time()` 测两次，断言行访问 < 列访问 × 1.3 倍（不严格倍率只测趋势），写入 `coalesce.out`。
  3. 写 `bank_conflict.py`：模拟 32 线程访问 shared memory `smem[32]`，全部访问 `smem[thread_id]`（无冲突，1 cycle）与全部访问 `smem[0]`（广播，1 cycle）vs 全部访问 `smem[thread_id % 4]`（4 路 bank conflict，4 cycle）；脚本打印三组 cycle 数；Bash 跑后断言 `grep -F "broadcast=1" bank.out`、`grep -F "conflict=4" bank.out` 都命中。
  4. 写 `tiling_16x16.py`：实现 16×16 矩阵分块乘法 naive 版（O(N^3) 三层循环）vs tiled 版（用 numpy `A[i:i+16, j:j+16]` 子块视图复用），跑 N=512 比耗时，断言 tiled 比 naive 快（耗时比 < 1.0）写入 `tile.out`；若不满足，请 Read 确认子块切法（避免不必要 copy）后重跑。

### BV04 SIMD 编程：SSE / AVX-512 / NEON 与 Rust std::simd（numpy 矢量化）

- **预期档位**: medium
- **考察维度**: 向量化等价 + 尾部处理 + 自动矢量化失败
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bv04/` 下用 Write 写 `simd_explain.md`：markdown 写 SIMD vs SIMT 对比表（SIMD 单指令多数据 / SIMT 单指令多线程 / lane 数 / 典型指令集 / 代表硬件）；Bash `grep -c '^|' simd_explain.md` ≥ 6 + `grep -F 'SIMD' simd_explain.md` 命中。
  2. 写 `simd_add.py`：实现三个版本的 float 数组逐元素相加——`naive_loop`（python for）、`numpy_vec`（`a + b`）、`numpy_chunked`（按 16 元素切块 `np.add`）；随机生成 65536 元素（16 倍数），三版本跑 100 次取平均耗时；Bash 跑 `python simd_add.py` 后断言 `numpy_vec_avg < naive_loop_avg` 与 `numpy_chunked_avg ≈ numpy_vec_avg`（误差 < 30%），写到 `simd.out`。
  3. 写 `tail_handler.py`：处理"非 16 倍数"数组（如 65540 元素）——主循环按 16 处理，尾部用 `for i in range(end_aligned, n): c[i] = a[i] + b[i]`；Bash 跑 `python tail_handler.py` 后断言 `np.allclose(c, a+b)` 与 `c.shape == (65540,)`，写入 `tail.out`。
  4. 写 `autovec_fail.md`：markdown 列出"编译器自动矢量化失败"四类场景（指针别名 / 条件分支 / 跨迭代依赖 / 不对齐内存），每条配 1 个 numpy/python 例子；Bash 跑 `grep -c '^- ' autovec_fail.md` ≥ 4（条目数）才视为合规；`wc -l autovec_fail.md` ≥ 15 行说明有解释段。

### BV05 Rust 并行生态：rayon 与 crossbeam（python 等价）

- **预期档位**: medium
- **考察维度**: 工作窃取 + 无锁 CAS + 流水线
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bv05/` 下用 Write 写 `work_stealing.md`：markdown 解释 rayon 的工作窃取 vs 静态均分优劣，配 1 张 ASCII 时序图（4 worker + 不均负载）；Bash `grep -F 'work-stealing' work_stealing.md` 命中 + `grep -c '^|' work_stealing.md` ≥ 4（表格行）。
  2. 写 `par_max.py`：用 `concurrent.futures.ThreadPoolExecutor` 模拟 rayon `par_iter().max()`——把 1024×1024 float32 数组按 256 元素切块，多线程分别求块内 max，最后 reduce；与 `np.max` 单线程版比耗时，断言多线程不显著慢于单线程（开销 < 5x），写入 `parmax.out`。
  3. 写 `lock_free.py`：实现 `ArrayQueue`（定长无锁队列）用 `threading.Lock` 模拟 CAS 重试——生产者/消费者各起 1 线程，跑 10000 次 put/get，最终断言队列空且计数为 0；Bash 跑 `python lock_free.py` 后 `grep -F "QUEUE_OK" lockfree.out` 必须命中，否则 Read 修重试逻辑。
  4. 写 `pipeline.py`：用 `concurrent.futures` + `queue.Queue` 模拟 rayon+crossbeam channel 流水线——1 个 reader 线程从文件读行、N 个 worker 线程并行反转字符串、1 个 writer 收集结果按输入顺序写出；输入生成 1000 行随机字符串，断言输出顺序与输入一致且每行被反转；Bash 跑后 `grep -F "PIPELINE_OK" pipeline.out` 命中。

### BV06 wgpu 与 Rust 原生 GPU 计算（WebGPU 不可用 → 文档 + 等价实现说明）

- **预期档位**: hard
- **考察维度**: WGSL 模拟 + compute shader 等价 + 跨平台取舍
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bv06/` 下用 Write 写 `wgpu_overview.md`：markdown 解释 wgpu 跨平台机制（Vulkan/Metal/D3D12/WebGPU 后端映射表）+ WGSL 与 CUDA 的 5 条对比（语法 / 内存模型 / 调度粒度 / 调试工具 / 性能上限）；Bash `grep -c '^|' wgpu_overview.md` ≥ 8（2 张表格）+ `grep -F 'WGSL' wgpu_overview.md` 命中。
  2. 写 `compute_equivalent.py`：用 numpy 模拟 wgpu compute shader 的"逐元素乘法"——函数 `compute_mul(a, b, workgroup=64)` 按 workgroup 划分（等价 `@builtin(global_invocation_id)`），每个 invocation 处理 `i = wg_id * workgroup + local_id`；Bash 跑 `python compute_equivalent.py` 后断言 `np.allclose(c, a*b)` 写入 `compute.out`。
  3. 写 `wgsl_grammar.md`：WGSL 关键语法备忘（`@group(0) @binding(0) var<storage, read> input: array<f32>` / `@builtin(global_invocation_id)` / `var<uniform>` 等），至少 8 行；Bash `wc -l wgsl_grammar.md` ≥ 8 + `grep -F '@builtin(global_invocation_id)' wgsl_grammar.md` 命中。
  4. 写 `tradeoff.md`：markdown 列出"什么场景选 wgpu / 什么场景选 CUDA"决策树（5 条分支），如"是否需要 RTX 专属扩展→是→CUDA；否→wgpu"；Bash 跑 `grep -c '^- ' tradeoff.md` ≥ 5（决策条目）+ Read `tradeoff.md` 检查无明显错误。

### BV07 多卡通信：NCCL、MPI 与数据/模型并行（无 NCCL → 通信模式说明 + mini 模拟）

- **预期档位**: hard
- **考察维度**: 集合通信 + 数据/模型并行 + 通信隐藏
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bv07/` 下用 Write 写 `nccl_primitives.md`：表格 4 行（AllReduce / Broadcast / AllGather / ReduceScatter），列：用途、典型场景、通信量；Bash `grep -c '^|' nccl_primitives.md` ≥ 6。
  2. 写 `allreduce_sim.py`：用 python `multiprocessing` 模拟 4 个 worker 的 AllReduce——每个 worker 持一个 1024 元素 numpy 数组，`mp.Queue` + ring 算法 4 步通信，最终每个 worker 持有完整 sum；断言 4 个 worker 结果一致且等于 4× 原始和；Bash 跑 `python allreduce_sim.py` 后 `grep -F "ALLREDUCE_OK" allreduce.out` 命中。
  3. 写 `dp_vs_mp.md`：markdown 写数据并行 vs 模型并行对比表（每卡模型副本 / 通信内容 / 适用模型规模 / 显存占用），含 AllReduce 同步"梯度"在 DP 中的角色解释；Bash `grep -c '^|' dp_vs_mp.md` ≥ 6 + `grep -F 'gradient' dp_vs_mp.md` 命中。
  4. 写 `comm_hide_sim.py`：模拟"梯度累积 + 异步 AllReduce overlap"——10 步前向，每步算本地梯度放入待发送队列；通信线程在后台把 5 步累积的梯度做 AllReduce；Bash 跑后断言最终所有 worker 梯度对齐且总耗时 < 串行（梯度+通信）耗时，写入 `commhide.out`。

### BV08 Roofline 性能分析模型（numpy flop/byte 比率）

- **预期档位**: medium
- **考察维度**: 计算强度 + Roofline 曲线 + 优化方向
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bv08/` 下用 Write 写 `roofline_plot.py`：模拟 A100 GPU 峰值——内存带宽 2.0 TB/s、FP32 峰值 19.5 TFLOPS；定义函数 `roofline(ai_gflops_per_byte, mem_bw, peak_compute)` 返回 `min(peak_compute, ai * mem_bw)`；生成 ai 从 0.1 到 100 的点用 ASCII 画屋顶线（每行选最高 GFLOPS），写入 `roofline.txt`；Bash 跑 `python roofline_plot.py` 后 `grep -c '#' roofline.txt` ≥ 10 行图表。
  2. 写 `intensity_calc.py`：用 numpy 实测 4 个 kernel 的 flop/byte——向量加（1 FLOP/byte）、矩阵乘（~85）、dot product（~0.5）、点积转置（~8）；Bash 跑后 `awk '/intensity/ {print $(NF-1), $NF}' intensity.out` 收集 4 个数值，断言 GEMM > dot > vec_add > dot_transposed 与预期顺序一致。
  3. 写 `optimize_choice.md`：markdown 表 4 行（位置 / 优化手段 / 举例 / 提升倍数）——内存段左侧→合并访问/shared tiling / GEMM 段右侧→Tensor core / FP16 / 算子融合；Bash `grep -c '^|' optimize_choice.md` ≥ 6 + `grep -F 'tensor core' optimize_choice.md` 命中。
  4. 写 `dram_metrics.md`：markdown 列出 Nsight Compute 关键指标 6 条（`dram__bytes.sum` / `sm__throughput.avg.pct_of_peak_sustained_elapsed` / `smsp__inst_executed` / L1/L2 hit rate / `stall_long_sb` / `l1tex__t_sectors_pipe_lsu_mem_global_op_ld.sum`），每条 1 行解释；Bash `grep -c '^- ' dram_metrics.md` ≥ 6 + `wc -l dram_metrics.md` ≥ 12。

### BV09 典型并行算法：Reduce、Scan 与稀疏矩阵（numpy mini 实现）

- **预期档位**: hard
- **考察维度**: 并行 reduce / Blelloch scan / CSR SpMV
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bv09/` 下用 Write 写 `parallel_reduce.py`：实现两种 reduce——naive（每步折半 O(log n) 步）、brent（work × depth，p 个 processor 分块本地 reduce + 树形合并）；随机生成 1024 元素数组比耗时；Bash 跑 `python parallel_reduce.py` 后断言 `np.isclose(naive_sum, brent_sum)` 且 `brent_time <= naive_time * 1.5`（p=4 时 brent 不慢多少），写入 `reduce.out`。
  2. 写 `blelloch_scan.py`：实现 Blelloch 独占 scan——upsweep 建树 + downsweep 下推；输入 16 元素 `[1]*16` 期望输出 `[0, 1, 2, ..., 15]`；Bash 跑后断言 `np.array_equal(out, np.arange(16))` 写入 `scan.out`。
  3. 写 `csr_spmv.py`：用 numpy 实现 CSR SpMV——输入 CSR 三元组 `(indptr, indices, data)` 与稠密向量 `x`，函数 `csr_spmv(indptr, indices, data, x, n_rows)` 输出 `y[i] = sum(data[k] * x[indices[k]]) for k in indptr[i]:indptr[i+1]`；随机生成 1000 行稀疏矩阵（每行 ~10 非零）与 numpy 稠密版对比，断言 `np.allclose(csr_y, dense_y)` 写入 `spmv.out`。
  4. 写 `parallel_speedup.py`：CSR 矩阵 1000 行按 100 行/块切 10 块，`concurrent.futures.ThreadPoolExecutor` 10 worker 并行 SpMV，比单线程耗时；Bash 跑后断言 `parallel_time < serial_time`（任意线程开销 < 计算收益时）写入 `speedup.out`。

### BV10 CUDA 调试与 Profiling：Nsight Compute 实战（无 Nsight → mini profiler）

- **预期档位**: medium
- **考察维度**: profile 指标 + 瓶颈定位 + 迭代优化
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bv10/` 下用 Write 写 `mini_profiler.py`：用 `time.perf_counter()` 给 4 个阶段打时间戳（AEC 前处理 / VAD / ASR 模拟 / TTS 模拟）——实际改用 numpy 计算阶段（vec_add / matmul / scan / reduce）；输出 CSV `trace.csv` `phase,start_ns,duration_ns`；Bash 跑 `python mini_profiler.py && head trace.csv` 断言 5 行（表头+4 阶段）。
  2. 写 `bottleneck_analyze.py`：对某次 profile 输出计算 `sm_throughput = (peak - actual) / peak * 100%` 与 `dram_throughput` 比例，给瓶颈判定规则（`sm 低 dram 高 → 内存墙` / `sm 高 dram 低 → 计算墙` / `都低 → 启动开销`）；Bash 跑 `python bottleneck_analyze.py < trace.csv` 后 `grep -F "BOTTLENECK=" bottleneck.out` 必须命中 `memory_wall` 或 `compute_wall` 之一。
  3. 写 `l1_l2_hitrate.md`：markdown 解释"L1 命中率低 + L2 高"的含义——L1 是 per-SM 一级 cache，L1 miss 但 L2 hit 表示数据被多个 SM 共享；附示例数字（如 L1 30%、L2 85%）；Bash `grep -F 'L1' l1_l2_hitrate.md` 命中 + `wc -l l1_l2_hitrate.md` ≥ 8。
  4. 写 `tiling_iterate.py`：实现一个"假设提升计算强度从 1→8 但实测只快 2 倍"的场景——跑 naive GEMM 1024×1024 得耗时 T1；跑 tiled GEMM 得耗时 T2；计算 `expected_speedup = 8 / 1 = 8`、`actual_speedup = T1/T2`、`gap = expected - actual`；写 markdown 报告 `tiling_report.md` 列出 gap 的三个可能原因（bank conflict / occupancy 低 / 同步开销）；Bash 跑后 `grep -F "actual_speedup" tiling_report.md` 命中。
