# 49 编译原理与 DSL 实战

> 编号段 AX01–AX10 · 聚焦词法分析、语法分析、抽象语法树、中间表示、代码生成、优化遍与 DSL 设计
> 与现有 17 维度「游戏与趣味编程开发」中编译器小节互补：17 偏示例教学；49 偏原理深度 + 工程实现

### AX01 编译原理全景：从源码到二进制
- **预期档位**: medium
- **考察维度**: 编译流程 + 各阶段产物
- **对话脚本**:
  1. 一个 C 程序从 .c 到可执行文件的全过程：预处理 → 编译 → 汇编 → 链接，每阶段输入/输出/工具。
  2. 编译器前端（Frontend）与后端（Backend）的分界：LLVM IR 让两者解耦的真实价值。
  3. 解释器 vs 编译器 vs JIT（AOT+JIT hybrid）的差异：CPython、PyPy、HotSpot C2/GraalVM 各代表哪一档。
  4. 用一个简单 C 函数 `int add(int a, int b) { return a + b; }` 跟踪 gcc -fdump-tree-all 的每一遍优化输出。

### AX02 词法分析：正则引擎与 DFA
- **预期档位**: medium
- **考察维度**: Lexer + Token 化
- **对话脚本**:
  1. 词法分析（Lexer）的作用与边界：为什么标识符 / 关键字 / 字面量必须分离，哪些属于语法层职责。
  2. NFA → DFA 的子集构造算法：用 Thompson 构造法 + Hopcroft 极小化手工推导一个 (a|b)*abb 的 NFA 到最小 DFA。
  3. Token 类别的设计：关键字表 vs 关键字识别（标识符是否在关键字集合里），哪种更适合现代语言（Rust / Swift）。
  4. 用 Lex/Flex 或手写 lexer 解析 JSON：JSON 的 6 类 token 边界、Unicode 转义、字符串字面量的合法性判定。

### AX03 语法分析：LL / LR / 递归下降
- **预期档位**: hard
- **考察维度**: Parser 算法 + 文法冲突
- **对话脚本**:
  1. 文法（Grammar）的形式化定义：BNF / EBNF / ABNF 各自写法差异，给一个 SQL SELECT 语句的 EBNF。
  2. 递归下降（Recursive Descent）手写 Parser 的 5 个常用技巧：左递归消除、Lookahead、回溯、错误恢复、优先级编码。
  3. LR(0) / SLR / LALR / LR(1) 的对比：表达能力 vs 状态数 vs 实现复杂度，YACC 为何选 LALR。
  4. 用 ANTLR4 或 Pest 写一个四则运算的 parser + evaluator：支持括号、优先级、左结合，画出语法树。

### AX04 抽象语法树（AST）与语义分析
- **预期档位**: medium
- **考察维度**: AST 设计 + 作用域
- **对话脚本**:
  1. AST vs 解析树（Parse Tree）：哪些节点是「有用的」、哪些只是中间产物，给一个 if-else 的两树对比。
  2. Visitor 模式遍历 AST 的 4 种实现：传统 OO Visitor / 模式匹配（Rust enum + match）/ 折叠（Catamorphism）。
  3. 作用域（Scope）链与符号表（Symbol Table）的实现：HashMap 嵌套、Stack、链表三档复杂度。
  4. 写一个最小语言（变量声明 + 赋值 + print）的语义检查器：未定义变量、重复声明、类型不匹配的错误定位。

### AX05 中间表示（IR）与 SSA
- **预期档位**: hard
- **考察维度**: IR 设计 + SSA 形式
- **对话脚本**:
  1. IR 的三档抽象层级（High-level IR / Low-level IR / Machine IR）：LLVM IR、WebAssembly、Cranelift 各属于哪档。
  2. SSA（Static Single Assignment）的核心：每个变量只被赋值一次，φ 节点的语义与转换规则。
  3. 内存 SSA（Mem2Reg、SROA、GVN）与控制流图（CFG）分析：常量传播、死代码消除的算法骨架。
  4. 用 LLVM IR Builder（C++ / Rust inkwell）写一个最小 IR 模块：两个函数 add / main，跑 lli 解释执行。

### AX06 代码生成：从 IR 到目标代码
- **预期档位**: hard
- **考察维度**: CodeGen + 寄存器分配
- **对话脚本**:
  1. 指令选择（Instruction Selection）的两种路径：基于规则的 BURS（Burger-Doller）vs 基于动态规划的 Twig。
  2. 寄存器分配的图着色算法（Chaitin）：干涉图构建、简化栈、溢出选择三步伪代码。
  3. 调用约定（Calling Convention）：x86-64 SysV ABI、RISC-V LP64D 的参数传递与栈布局对比。
  4. 用 Cranelift 或 LLVM MCJIT 即时编译一个 AST 函数为机器码执行：耗时、产物大小、安全性考量。

### AX07 优化遍与 Profile-Guided Optimization
- **预期档位**: hard
- **考察维度**: 编译器优化 + PGO
- **对话脚本**:
  1. 经典优化：常量折叠、死代码消除、循环展开、内联、强度削减、向量化，每种给一个具体 IR 变换示例。
  2. 跨过程优化（IPO / LTO）：单文件 vs Whole-Program 的边界，LLVM LTO / ThinLTO 的实现差异。
  3. PGO（Profile-Guided Optimization）三步走：插桩 → 运行收集 → 重编译，gcc -fprofile-generate/-use 的真实案例。
  4. AutoFDO / Propeller：用 perf + 二进制重写实现 feedback-driven 优化，无需源代码插桩。

### AX08 JIT 编译与自适应优化
- **预期档位**: hard
- **考察维度**: JIT + HotSpot 原理
- **对话脚本**:
  1. JIT 编译的三种策略：方法内联触发阈值、C1 客户端编译器、C2 服务端编译器的层级协作。
  2. 去优化（Deoptimization）：从优化后的机器码退回到解释器或 C1，HotSpot 的「OSR（On-Stack Replacement）」机制。
  3. GraalVM 的 Truffle 框架：多语言互操作、partial evaluation、Sulong（LLVM bitcode 运行）。
  4. 用 JITWatch 工具分析一个 Java 程序的 HotSpot 编译日志：C1/C2 编译触发、内联决策、GC 停顿。

### AX09 DSL 设计：从 MiniJinja 到 BAML
- **预期档位**: medium
- **考察维度**: DSL 范式 + 设计权衡
- **对话脚本**:
  1. DSL 的两大类：内部 DSL（嵌入宿主语言，Gradle Kotlin DSL / jQuery）和外部 DSL（独立语法，SQL / HTML / CSS）。
  2. MiniJinja（Tera 的 Rust port）模板引擎的语法扩展：宏、自定义标签、过滤器链的实现。
  3. BAML（Boundary ML）作为 LLM 时代的 DSL：partial streaming、结构化输出、类型安全的提示词工程。
  4. 设计一个最小外部 DSL「域名解析配置」：语法、文法、AST、解释器代码四件套，每件套给关键片段。

### AX10 工程实战：构建一个 Mini-Lisp 解释器
- **预期档位**: hard
- **考察维度**: 编译原理端到端实战
- **对话脚本**:
  1. Mini-Lisp 的语法设计：S-expression、quote、atom、lambda、应用，每条规则对应一个 BNF。
  2. 手写 Lexer + Parser：Rust / Python / Haskell 任选一，写出核心 100 行代码。
  3. AST 求值：递归 eval 函数、闭包捕获、词法作用域，每种语言实现的差异（Rust 借用 vs Haskell 纯函数）。
  4. 扩展路线：尾调用优化、宏系统、字节码编译、JIT；给出从解释器到简单编译器的演进路径。