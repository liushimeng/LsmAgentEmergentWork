# 75 编译器后端与 LLVM 优化

> 编号段 BX01–BX10 · 编译器后端与 LLVM 优化（Compiler Backend & LLVM Optimization）
>
> 运行环境约束：**LLVM/Clang/MLIR 不可用**，全部以 **纯 python3 mini 编译器后端等价实现 + markdown 解释** 跑通。SSA 形式 + φ 函数用 class 建模；mini-IR 用 python 列表 + dict；死代码消除/常量传播手写 Pass；寄存器分配用图着色简化版；指令选择用模式匹配函数。产物落 `tmpPlan/agent-test/` 沙盒。

---

### BX01 SSA 形式与 φ 函数

- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-09_08-S01-S10-AI工程LLM应用测试脚本重构方案.md）
- **预期档位**: medium
- **考察维度**: SSA 建模 + φ 插入 + 支配边界
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bx01/` 下用 Write 写 `ssa_model.py`：实现 mini SSA IR——`Instr(op, dest, args)` 表示一条指令，`BasicBlock(name, instrs)` 表示基本块，`Function(name, blocks)` 表示函数；实现 `to_ssa(func)` 把非 SSA 代码（如 `x = 1; x = x + 1`）转 SSA（`x1 = 1; x2 = x1 + 1`）；Bash 跑 `python ssa_model.py` 后断言 `x2` 出现在输出，写入 `ssa.out`。
  2. 写 `phi_insert.py`：实现 φ 函数插入——`phi(var, preds)` 表示 `var = φ(preds[0].val, preds[1].val, ...)`；给定一个 2 前驱合并点，插入 `x3 = φ(x1, x2)`；Bash 跑后断言 φ 节点出现在合并块头部，写入 `phi.out`。
  3. 写 `dom_frontier.py`：实现支配边界计算——`dominators(blocks)` 返回每块的支配者集合，`dominance_frontier(blocks)` 返回支配边界；对简单 if-else 图断言 `DF(if_block) == {merge_block}`；Bash 跑后断言正确写入 `df.out`。
  4. 写 `ssa_destruct.py`：实现 SSA 消去——把 φ 函数替换为 copy 指令（`x = φ(a,b)` → 前驱块末尾插入 `x = a` / `x = b`）；处理 Lost Copy 与 Swap Problem（用临时变量 `tmp` 打破环）；Bash 跑后断言消去后无 φ 节点，写入 `destruct.out`。

### BX02 LLVM IR 结构：Module / Function / BasicBlock / Instruction

- **预期档位**: medium
- **考察维度**: 四层嵌套 + Use-Def 链 + 值命名
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bx02/` 下用 Write 写 `ir_model.py`：实现 LLVM IR 四层结构——`Module(functions)` / `Function(name, blocks)` / `BasicBlock(name, instrs)` / `Instr(op, dest, args, type)`；实现 `parse_ir(text)` 解析简化 LLVM IR 文本（如 `define i32 @fib(i32 %n) { ... }`）；Bash 跑 `python ir_model.py` 后断言解析 `fib` 函数得 3 个 basic block，写入 `ir.out`。
  2. 写 `use_def.py`：实现 Use-Def 链——`instr.uses` 返回使用此指令结果的其他指令，`value.users` 返回使用此值的所有指令；对 `%2 = add i32 %1, 1` 断言 `%1` 的 users 含 `%2`；Bash 跑后断言正确写入 `usedef.out`。
  3. 写 `replace_uses.py`：实现 `replaceAllUsesWith(old, new)`——把 `old` 的所有使用替换为 `new`，`old.uses` 变空；Bash 跑后断言替换后 `old.uses == []` 且 `new.users` 包含原 `old` 的所有用户，写入 `replace.out`。
  4. 写 `value_naming.py`：实现值命名——`Value(name, type)` 支持数字名（`%1`）与字符串名（`%entry`）；验证命名规则（不能含空格、不能重复）；Bash 跑后断言命名合法，写入 `naming.out`。

### BX03 Pass Manager：Function / Module / Loop Pass

- **预期档位**: hard
- **考察维度**: Pass 架构 + 分析依赖 + 失效处理
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bx03/` 下用 Write 写 `pass_manager.py`：实现 mini PassManager——`PassManager` 注册 `FunctionPass`/`ModulePass`/`LoopPass`，`run(func)` 按序执行；`FunctionPass.run(func)` 返回 `changed: bool`；Bash 跑 `python pass_manager.py` 后断言 3 个 pass 都执行，写入 `pm.out`。
  2. 写 `analysis_pass.py`：实现分析 pass——`LoopAnalysis.run(func)` 返回 `LoopInfo(header, latch, exit_blocks)`；`FunctionPass` 声明依赖 `Required<LoopAnalysis>`；Bash 跑后断言依赖正确解析，写入 `analysis.out`。
  3. 写 `invalidation.py`：实现 pass 失效——`LoopRotate` 修改 loop 后标记 `LoopAnalysis` 失效，下次使用前重新计算；Bash 跑后断言失效后重新计算，写入 `invalidate.out`。
  4. 写 `loop_pass.py`：实现 `PrintLoopPass`——遍历所有 loop 打印 header/latch/exit block 名字；Bash 跑后断言输出含 `header=` `latch=` `exit=` 三行，写入 `loop.out`。

### BX04 内联决策与成本模型

- **预期档位**: hard
- **考察维度**: InlineCost + 阈值 + 冷路径
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bx04/` 下用 Write 写 `inline_cost.py`：实现 InlineCost 启发式——`compute_inline_cost(caller, callee)` 基于 callee 指令数、调用频次、是否 always_inline 返回 cost；`threshold = 220` 默认；Bash 跑 `python inline_cost.py` 后断言小函数（<10 指令）cost < threshold，写入 `inline.out`。
  2. 写 `always_inline.py`：实现 `[[gnu::always_inline]]` 属性——`always_inline` 函数无视 threshold 直接内联；Bash 跑后断言 always_inline 函数被内联，写入 `always.out`。
  3. 写 `pgo_inline.py`：模拟 PGO 感知内联——热路径（`hotness > 0.8`）提高 threshold ×2，冷路径（`hotness < 0.1`）降低 threshold ×0.5；Bash 跑后断言热路径内联更激进，写入 `pgo.out`。
  4. 写 `call_chain.py`：模拟 5 层调用链内联——每层 10 指令小函数，threshold 220 下全部展平；计算展平后 IR 膨胀（5 层 × 10 指令 = 50 指令）；Bash 跑后断言展平后指令数 == 50，写入 `chain.out`。

### BX05 循环优化：LICM / 向量化 / 展开

- **预期档位**: hard
- **考察维度**: LICM + 向量化 + 展开
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bx05/` 下用 Write 写 `licm.py`：实现 LICM（Loop Invariant Code Motion）——`licm(func, loop)` 把循环不变指令提到 preheader；判断条件：指令的所有操作数在循环外定义且循环内无重定义；Bash 跑 `python licm.py` 后断言不变指令被提到 preheader，写入 `licm.out`。
  2. 写 `licm_fail.py`：构造 LICM 不能提的情况——循环内写全局变量（有副作用）或操作数在循环内重定义；Bash 跑后断言这些指令不被外提，写入 `licm_fail.out`。
  3. 写 `vectorize.py`：模拟向量化——`vectorize(loop, width=4)` 把循环体复制 width 次，用向量操作替换标量操作；Bash 跑后断言向量化后循环次数 /4，写入 `vec.out`。
  4. 写 `unroll.py`：实现循环展开——`unroll(loop, factor=4)` 把循环体复制 factor 次，步长 ×factor；Bash 跑后断言展开后循环次数 /4，写入 `unroll.out`。

### BX06 寄存器分配：图着色 vs 线性扫描

- **预期档位**: hard
- **考察维度**: 图着色 + 线性扫描 + spill
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bx06/` 下用 Write 写 `regalloc.py`：实现图着色寄存器分配——`build_interference_graph(func)` 构建冲突图，`color_graph(graph, k)` 用 k 色着色（简化 Welsh-Powell 算法）；Bash 跑 `python regalloc.py` 后断言 5 个变量 3 寄存器着色成功或 spill，写入 `reg.out`。
  2. 写 `spill.py`：实现 spill 决策——当冲突图色数 > k 时，spill 度数最高的变量（插入 load/store）；Bash 跑后断言 spill 后着色成功，写入 `spill.out`。
  3. 写 `linear_scan.py`：实现线性扫描——按活跃区间起点排序，贪心分配寄存器，溢出最远终点区间；Bash 跑后断言线性扫描分配成功，写入 `ls.out`。
  4. 写 `compare_alloc.py`：对比图着色 vs 线性扫描——同一函数分别跑两算法，比较 spill 次数与耗时；Bash 跑后断言图着色 spill ≤ 线性扫描，写入 `compare.out`。

### BX07 指令选择：SelectionDAG vs GlobalISel

- **预期档位**: hard
- **考察维度**: 模式匹配 + Legalization + GlobalISel
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bx07/` 下用 Write 写 `isel.py`：实现 SelectionDAG 风格指令选择——`pattern_match(ir_node, patterns)` 把 IR 节点匹配到目标指令模式（如 `add(reg, imm) → ADDI`）；Bash 跑 `python isel.py` 后断言 `add(x, 1)` 匹配到 `ADDI`，写入 `isel.out`。
  2. 写 `legalize.py`：实现 Legalization——当目标不支持某操作（如 64 位加法在 32 位 target），拆分为多条指令（`ADD_LO` + `ADC_HI`）；Bash 跑后断言 64 位加被拆为 2 条 32 位指令，写入 `legal.out`。
  3. 写 `global_isel.py`：模拟 GlobalISel 四步——`IRTranslator`（IR→MI）→ `Legalizer` → `RegBankSelect` → `InstructionSelect`；每步打印日志；Bash 跑后断言 4 步都执行，写入 `gisel.out`。
  4. 写 `td_pattern.py`：模拟 tablegen 模式——`Pattern(lhs, rhs)` 表示"把 lhs 模式替换为 rhs 指令"；Bash 跑后断言模式匹配正确，写入 `td.out`。

### BX08 Target 描述与代码生成：MachineInstr / MC 层

- **预期档位**: hard
- **考察维度**: MachineInstr + MCStreamer + target 描述
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bx08/` 下用 Write 写 `mir_model.py`：实现 MachineInstr——`MachineInstr(op, operands)` 表示一条机器指令，`MachineOperand(reg/imm/mem)` 表示操作数；Bash 跑 `python mir_model.py` 后断言 `MOV(R0, 5)` 正确构造，写入 `mir.out`。
  2. 写 `mc_streamer.py`：实现 MCStreamer——`emit_instruction(instr)` 把 MachineInstr 写入 section（`.text` 或 `.data`）；Bash 跑后断言指令写入 `.text` section，写入 `mc.out`。
  3. 写 `target_desc.py`：模拟 target 描述——`RegisterClass(name, regs)` 表示寄存器类，`Instruction(name, operands, encoding)` 表示指令；Bash 跑后断言寄存器类与指令定义正确，写入 `td.out`。
  4. 写 `elf_emit.py`：模拟 ELF 生成——`emit_elf(funcs)` 把函数列表转为简化 ELF 文件头 + `.text` section；Bash 跑后断言输出文件以 `\x7fELF` 开头，写入 `elf.out`。

### BX09 LTO / ThinLTO 与跨模块优化

- **预期档位**: hard
- **考察维度**: LTO + ThinLTO + 跨模块内联
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bx09/` 下用 Write 写 `lto_model.py`：实现 LTO 模型——`link_time_optimize(modules)` 合并多个模块 IR，跨模块内联/常量传播；Bash 跑 `python lto_model.py` 后断言跨模块内联发生，写入 `lto.out`。
  2. 写 `summary_index.py`：实现 ThinLTO summary index——`FuncSummary(name, size, calls, bbcount)` 表示函数摘要；Bash 跑后断言摘要包含 `size` 与 `bbcount` 字段，写入 `summary.out`。
  3. 写 `wpd.py`：模拟 Whole-Program Devirtualization——`devirtualize(call_site, possible_targets)` 把虚调用转为直接调用（当只有一个可能目标时）；Bash 跑后断言虚调用被转为直接调用，写入 `wpd.out`。
  4. 写 `import_list.py`：模拟 ThinLTO import 列表——`compute_import_list(summary, threshold=20)` 只内联 size < threshold 的函数；Bash 跑后断言大函数不被导入，写入 `import.out`。

### BX10 MLIR 多层 IR 与 Dialect，以及 JIT（ORC/LLJIT）实战

- **预期档位**: hard
- **考察维度**: Dialect + Region + Pass + JIT
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/bx10/` 下用 Write 写 `dialect_model.py`：实现 MLIR Dialect 模型——`Dialect(name, ops)` 表示方言，`Operation(name, regions, operands)` 表示操作，`Region(blocks)` 表示嵌套区域；Bash 跑 `python dialect_model.py` 后断言 `func.func` 操作含 1 个 region，写入 `dialect.out`。
  2. 写 `lowering_chain.py`：模拟 `tensor → linalg → scf → llvm` lowering——每层 dialect 打印日志；Bash 跑后断言 4 层都执行，写入 `lower.out`。
  3. 写 `pass_schedule.py`：实现 MLIR PassManager——`pm.add_pass(name)` 按序执行 pass，`OpPassManager` 在 region 上跑 pass；Bash 跑后断言 pass 按序执行，写入 `pass.out`。
  4. 写 `jit_sim.py`：模拟 ORC/LLJIT JIT——`LLJIT.compile(module)` 把 IR 编译为可执行代码，`jit.lookup(name)` 查找符号；Bash 跑后断言 `fib(10)` 返回 55，写入 `jit.out`。
