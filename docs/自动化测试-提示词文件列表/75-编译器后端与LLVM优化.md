# 75 编译器后端与 LLVM 优化

> 编号段 BX01–BX10 · 编译器后端与 LLVM 优化（Compiler Backend & LLVM Optimization）
> 覆盖维度：SSA 形式与 φ 函数 / LLVM IR 结构（Module/Function/BasicBlock/Instruction）/ Pass Manager（Function/Module/Loop Pass）/ 内联决策与成本模型 / 循环优化（LICM/向量化/展开）/ 寄存器分配（图着色/线性扫描）/ 指令选择（SelectionDAG/GlobalISel）/ Target 描述与代码生成（MachineInstr）/ LTO/ThinLTO 与跨模块优化 / MLIR 多层 IR 与 Dialect / JIT（ORC/LLJIT）实战
> 与「74-编译原理 DSL 实战」互补：本文件聚焦**后端优化与代码生成 + LLVM/MLIR 工具链**，而非前端词法语法/DSL 解释器。

---

### BX01 SSA 形式与 φ 函数
- **预期档位**: medium
- **考察维度**: 理解 SSA（Static Single Assignment）的核心约束、φ 函数的插入与消去算法、支配边界（Dominance Frontier）计算
- **对话脚本**:
  1. 请解释什么是 SSA（Static Single Assignment）形式，为什么现代编译器（如 LLVM）普遍采用 SSA？给出一个非 SSA 代码片段并手动改写为 SSA。
  2. 你给的例子中，合并点插入 φ 函数时我用的是「朴素 φ 插入」。如果控制流图有 5 个前驱的基本块都定义了同一个变量 x，按照 Cytron 的支配边界算法，最少需要几个 φ 函数？画图并推导。
  3. 我听说 LLVM 3.0 之前用 `alloca`/`load`/`store` 做内存 SSA，之后改成 `register SSA` + mem2reg pass。这个演变解决了什么性能问题？请从别名分析（Alias Analysis）和死代码消除两个角度说明。
  4. 在实际工程里，SSA 消去（destruct）有「Lost Copy」和「Swap Problem」两个经典难题。请各举一个例子，并说明教科书解法（并行 copy 插入 / 先打破环）具体怎么做。
  5. 如果我要给 laew 的 AST 加一个简单的常量折叠 pass，但是 AST 还没转 SSA，你会建议我先做 SSA 化还是直接做树上的 folding？说明 trade-off。

### BX02 LLVM IR 结构：Module / Function / BasicBlock / Instruction
- **预期档位**: medium
- **考察维度**: 理解 LLVM IR 的四层嵌套结构、Use-Def 链、值命名约定、与汇编/MIR 的层级关系
- **对话脚本**:
  1. 请用 LLVM IR（文本形式）写一个计算斐波那契的函数 `fib(i32 n) -> i32`，要求用递归写法，标清 Module / Function / BasicBlock / Instruction 四层。
  2. 我跑 `opt -S -mem2reg` 之后发现你写的 phi 节点里有两个前驱都指向同一个 basic block，这是否合法？LLVM 的 φ 节点允许多个 incoming 来自同一个前驱吗？文档哪一节规定了这件事？
  3. 我看到 IR 里大量 `%1`、`%2` 这种数字名字，也见过 `%entry`、`%for.body` 这种有意义的。如果我用 C++ API `Value::setName()` 命名，有没有字符限制？下划线、点号、Unicode 都能通过 verifier 吗？
  4. 请解释 LLVM IR 的 Use-Def 链：一条 `add` 指令的 `uses()` 和它的操作数的 `users()` 分别返回什么？如果我用 `replaceAllUsesWith()` 把一条指令换成另一条，原指令的 use_list 会怎样变化？
  5. 对比 LLVM IR 与 MLIR 的「区域（Region）」概念：LLVM IR 的 basic block 是否构成 MLIR 意义上的 region？为什么 MLIR 需要引入嵌套 region？

### BX03 Pass Manager：Function / Module / Loop Pass
- **预期档位**: hard
- **考察维度**: LLVM Pass 架构、legacy PM vs new PM、分析 pass 与变换 pass 的依赖关系、LoopInfo 与 LoopPass
- **对话脚本**:
  1. 简要对比 LLVM 的 legacy pass manager 和 new pass manager（PMX/PassBuilder）的架构差异，为什么 16.0 之后 legacy 被废弃？
  2. 我要写一个 `FunctionPass`，它需要知道循环嵌套深度。我要声明哪些分析依赖？`getAnalysisUsage()` 里应填 `RequiredTransitive<LoopInfoWrapperPass>` 还是 `Required<LoopInfoWrapperPass>`？区别是什么？
  3. 如果两个 pass A 和 B 都要用到 `LoopInfo`，但 A 在 `runOnFunction` 里把 loop 旋转（rotate）了，B 拿到的 `LoopInfo` 会不会过时？new PM 的 `LoopAnalysisManager` 怎么处理这种失效？
  4. 给我写一个最小的 `LoopPass` 骨架（C++），让它能打印每个 loop 的 header、latch、exit block 的名字。要求能 `opt -S -passes=print-loop-info` 调起来。
  5. BX01 我们聊过 φ 函数与 SSA，BX02 聊过 IR 结构。现在我要在 loop body 里识别归纳变量（induction variable），应该用哪个 LLVM 分析 pass 做支撑？请给出调用链。

### BX04 内联决策与成本模型
- **预期档位**: hard
- **考察维度**: InlineCost 启发式、阈值调度、AlwaysInline vs Attribute 驱动、冷代码路径处理
- **对话脚本**:
  1. LLVM 的 `InlineCost` 是怎么给候选调用站点评分的？阈值（threshold）、bonus、penalty 各是怎么算的？为什么小函数默认内联，大函数有「折扣」？
  2. 我写了个 `[[gnu::always_inline]]` 的辅助函数，但用 `opt -O0` 跑却没被内联。这是 why？`always_inline` 属性在哪一级 pass 才会被尊重？
  3. 如果我想在 `-O2` 下对「热路径上的调用」提高内联积极性，对「冷路径上的调用」关闭内联，LLVM 有什么机制能感知调用频次？PGO 的 `block-count` 怎么传给 InlineCost？
  4. 假设我有一棵 5 层深的调用链 A→B→C→D→E，每层都是 10 条 IR 的小函数，默认阈值下 LLVM 会把哪些层级全部展平？展平后 IR 膨胀多少？给出估算过程。
  5. BX03 我们说过 loop rotate 会触发 LoopInfo 失效。如果我在 inliner 里把 callee 展开到 caller 的 loop header 里，这个变换会同时让哪些分析 pass 失效？新 PM 如何声明？

### BX05 循环优化：LICM / 向量化 / 展开
- **预期档位**: hard
- **考察维度**: LoopInvariantCodeMotion、LoopUnroll、LoopVectorizer 的合法性条件与性能权衡
- **对话脚本**:
  1. 写一个 C 循环让 LLVM 的 LICM pass 把一条 load 提到 preheader，再写一个循环让 LICm **不能**提同一条 load（提示：考虑别名与副作用）。解释判断条件。
  2. `#pragma clang loop unroll(disable)` 和 `#pragma clang loop unroll_count(4)` 分别映射到 LLVM 的哪个 metadata 节点？我可以直接在 IR 里手插这个 metadata 吗？
  3. 假设我有一个带 `llvm.loop.vectorize.enable` metadata 的循环，但 vectorizer 最终还是标量化了。我该怎么看 vectorizer 的 debug 日志找出原因？`-Rpass-analysis=loop-vectorize` 输出里关键词有哪些？
  4. 我的循环里有跨迭代写后读（RAW）依赖，vectorizer 会说「unsafe dep」。有没有办法用 `llvm.memcpy` 或者 runtime alias check 绕过？各自的代价是什么？
  5. BX01 我们讨论过 φ 函数插入。LICM 把指令提到 preheader 后，原来在 loop body 里用到该指令的 φ 节点会发生什么变化？SSA 属性是否还保持？

### BX06 寄存器分配：图着色 vs 线性扫描
- **预期档位**: hard
- **考察维度**: Chaitin-Briggs 图着色、线性扫描（Poletto-Lin）、LLVM 的 RegAllocGreedy / RegAllocBasic、spill 代价
- **对话脚本**:
  1. 图着色寄存器分配（Chaitin-Briggs）的核心步骤是什么？什么叫「简化 → 溢出 → 选择」三部曲？与线性扫描比，各自的适用场景？
  2. LLVM 默认的 `regalloc=greedy` 是用图着色还是线性扫描？它的「区间分割（split interval）」机制是怎么解决「大量短活跃区间拖垮图着色」的问题的？
  3. 如果 target 只有 8 个通用寄存器，但函数有 20 个同时活跃的虚拟寄存器，greedy 一定会 spill。LLVM 怎么决定 spill 哪条虚拟寄存器的活跃区间？有没有办法看 spill 决策日志？
  4. 我看 LLVM 里有 `RegAllocBasic`，说是给 -O0 用的 fast allocator。它具体做了哪些近似？为什么它能比 greedy 快 5-10 倍？
  5. BX05 我们提到 LICM 会提到 preheader 一条 load。如果这条 load 的虚拟寄存器在 loop 体里被大量使用，会不会让 greedy 被迫 split/spill？有没有办法用 `llvm::Rematerializable` 属性缓解？

### BX07 指令选择：SelectionDAG vs GlobalISel
- **预期档位**: hard
- **考察维度**: SelectionDAG 模式匹配、Legalization、GlobalISel 的 vs 特点、tablegen 描述参与
- **对话脚本**:
  1. SelectionDAG ISel 从 DAG 构建到指令输出会经历哪几个 phase（Combine / Legalize / Select / Schedule）？每一阶段会丢信息吗？为什么 Legalize 可能把一条节点膨胀成三条？
  2. GlobalISel 用 MachineIR 取代 DAG，号称「每段 pass 都能工作」。它把指令选择拆成 IRTranslator → Legalizer → RegBankSelect → InstructionSelect 四步，每步解决了什么问题？
  3. 如果我想让 target 支持一条新的向量指令 `vaddw.32 v0, v1, v2`，在 SelectionDAG 下我要在 .td 文件里写哪些内容？需要定义 Record、Pattern 匹配、类型约束？
  4. BX02 聊过 LLVM IR 的 Value，BX06 聊过虚拟寄存器。GlobalISel 的 MachineOperand 里「register operand」和「IR Value」是什么关系？何时发生从 virtual register 到 physical register 的映射？
  5. 传说 SelectionDAG 会在 ABI  lowering 阶段把一条 call 膨胀成 10+ 条 target node，GlobalISel 能缓解这个吗？其 CallLowering pass 是如何与 RegBankSelect 协作的？

### BX08 Target 描述与代码生成：MachineInstr / MC 层
- **预期档位**: hard
- **考察维度**: MachineInstr 与 MCInst 的层级、MCStreamer / MCObjectWriter、target description（td）的作用
- **对话脚本**:
  1. LLVM 后端从 LLVM IR 到 .o 文件要经过哪些 IR 层级（IR → MI → MCInst）？MachineInstr 和 MCInst 分别承载什么信息？为什么需要两套？
  2. td 文件里的 `RegisterClass`、`Instruction`、`DAGPattern` 三类定义在 TableGen 展开后各生成哪些 .inc 文件？哪个被 SelectionDAG 直接引用？
  3. 如果我要给 laew 自创的「伪 target」生成 ELF，我至少需要写几个 td 文件？要定义几个 RegisterClass 才能跑通最简单的 `return 0`？
  4. MCStreamer 在 `EmitInstruction` 里能看到什么？它怎么知道当前 section 该放 .text 还是 .data？与 `MCObjectWriter` 的分工是什么？
  5. BX07 聊过指令选择、BX06 聊过寄存器分配。在机器码 Emitter 之前，还有一个「branch relaxation」阶段，它解决什么问题？为什么 ARM 上经常需要？

### BX09 LTO / ThinLTO 与跨模块优化
- **预期档位**: hard
- **考察维度**: Link-Time Optimization、ThinLTO 并行化、Whole-Program Devirtualization、Summary 索引
- **对话脚本**:
  1. LTO 和 ThinLTO 的核心区别是什么？为什么说 ThinLTO 更适合百万行 C++ 项目的增量构建？
  2. ThinLTO 的「summary index」里每条记录有什么字段？`FuncSummary` 里的 `bbcount` 和 `callsiteheight` 分别是怎么被跨模块 InlineCost 用的？
  3. 如果模块 A 调用了模块 B 的 `virtual foo()`，Whole-Program Devirtualization（WPD）怎么把它变成直接调用？需要哪些 analysis？
  4. ThinLTO 的并行调度里，一个 symbol 被两个 compile unit 同时导入（import），谁是「单源事实」？冲突怎么解决？
  5. BX04 我们讨论过内联阈值。ThinLTO 的 import 列表由外部 linker/lld 决定，如果我希望「仅跨 module 内联小于 20 IR 的函数」，应该怎么配置 `-thinlto-single-module-eq` 之类参数？

### BX10 MLIR 多层 IR 与 Dialect，以及 JIT（ORC/LLJIT）实战
- **预期档位**: hard
- **考察维度**: MLIR Dialect 体系、Region 嵌套、Pass 调度、ORC/LLJIT 的层、自定义 Dialect + JIT 端到端
- **对话脚本**:
  1. MLIR 为什么要把 IR 做成多层的 Dialect？对比 LLVM IR 的「单层方言」，多层 IR 在表达张量 / 循环 / 控制流上有什么优势？举一个 `tensor` → `linalg` → `scf` → `llvm` lowering chain 的例子。
  2. 如果我自创一个 `laew` Dialect，需要实现 `Dialect`、`Type`、`Operation` 三类实体中的哪些 boilerplate？MLIR 的 `ODS` 框架能自动生成什么？
  3. MLIR 的 `PassManager` 和 LLVM 的 `PassManager` 有何异同？MLIR 的 `OpPassManager` 在 region 上跑 pass 时，会不会让嵌套 region 里的 analysis 失效？
  4. LLVM ORC JIT 的 `LLJIT` 三层（ObjectLinkingLayer / CompileLayer / TransformLayer）各自干什么的？如果我要让 JIT 能查主进程的全局符号，应该怎么设 `JITDylib` 的 linkage？
  5. BX02-BX09 我们讨论了 SSA、内联、循环、寄存器分配、指令选择、LTO、MLIR Dialect。现在综合任务：给我写一个端到端 demo，用 C++ API 构建一个 `func.func`（MLIR），跑 `convert-vector-to-scf` → `convert-scf-to-llvm` → `reconcile-unrealized-casts`，然后通过 LLJIT 执行。哪些 pass 必须显式加？为什么不能只靠 `PassManager::run()` 一口气跑完？
