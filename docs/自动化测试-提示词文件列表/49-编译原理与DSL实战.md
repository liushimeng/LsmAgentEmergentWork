# 自动化测试提示词 — 编译原理与 DSL 实战（AX01–AX10）

> 使用说明见同目录 `README.md`。每条提示词在同一 Session 内按轮次顺序喂入。
> 与现有 17 维度「游戏与趣味编程开发」中编译器小节互补：17 偏示例教学；49 偏原理深度 + 工程实现

## 维度说明

本维度考察 Agent 在 **编译原理工程化** 上的动手能力：Mini-Lisp/表达式 DSL 词法+语法+求值器 python 完整实现 + 测试断言（先写 failing 测试再实现）、JIT 降级为"树遍历解释器 vs 字节码编译执行"性能对比。
所有产物落到 `tmpPlan/agent-test/` 沙盒，不依赖 LLVM/Clang 重工具链，纯 Python 实现。
与 README 其它维度互补：Q 维度考察游戏与趣味编程，AX 聚焦"编译层"——词法/语法/IR/字节码/优化。

---

### AX01 编译流程全貌：写一个 4 阶段 mini 编译器
- **预期档位**: medium
- **考察维度**: 编译流程 + 各阶段产物
- **工具链**: Write → Bash → Bash → Read
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ax01/` 写 `mini_c.py`：实现 4 阶段——`tokenize(source)` → `parse(tokens)` → `ir_gen(ast)` → `codegen(ir)`，输入 `"int main() { return 1 + 2; }"` 走完全链路，每阶段打印产物到 `stages.log`。
  2. 写 `test_mini.py` 用 unittest：断言 tokenize 产出的 token 数 ≥ 10、parse 返回 ast 非空、ir 含 `add` 节点、codegen 返回非空字符串。
  3. 跑 `python3 -m unittest test_mini.py -v` 输出 "OK"；故意在 tokenize 里漏 'return' 关键字，断言 parse 阶段 fail → 改回正确 lexer → 全过。
  4. 在 `compiler_notes.md` 写"前端/后端分界"：LLVM IR 解耦的价值 + CPython/PyPy/HotSpot 各自属于哪档（解释器 vs AOT vs JIT），每档 1 行 + 1 例。

### AX02 词法分析：手写 lexer + DFA 模拟
- **预期档位**: medium
- **考察维度**: Lexer + Token 化
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ax02/` 写 `lexer.py`：实现 `tokenize(source)` 产出 6 类 token（KEYWORD/IDENT/NUMBER/STRING/OP/EOF），用正则顺序匹配，输入 `"let x = 42 + y"` 输出 7 个 token。
  2. 写 `test_lexer.py` 用 unittest：(a) 空输入 → EOF only、(b) `"let x = 42"` → 5 tokens（含 let/x/=/42）、(c) `"\"hi\""` → STRING token 值为 hi。
  3. 跑脚本输出 "3/3 pass"；故意把 NUMBER 正则放 KEYWORD 前，断言 `let` 被识别为 NUMBER 触发 fail → 改回顺序 → 全过。
  4. 在 `dfa_notes.md` 写"NFA → DFA 子集构造"：Thompson 构造法 4 步（ε 闭包 / 转移 / 接受态 / Hopcroft 最小化），每步 1 行 + 一个例子字符。

### AX03 语法分析：递归下降表达式 parser
- **预期档位**: hard
- **考察维度**: Parser 算法 + 文法冲突
- **工具链**: Write → Bash → Bash → Read
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ax03/` 写 `parser.py`：实现递归下降 parser 处理表达式文法 `E → E + T | E - T | T`、`T → T * F | T / F | F`、`F → (E) | NUM`，输入 `"1 + 2 * (3 - 4)"` 输出 AST。
  2. 写 `test_parser.py`：断言 AST 顶层 op 是 '+'、右子树顶层 op 是 '*'、叶子节点值分别为 1/2/3/4。
  3. 跑脚本输出 "4 项 AST 结构断言全过"；故意把左递归写成无限递归（直接 `def parse_E(): parse_E() + parse_T()`），断言栈溢出 → 改掉左递归（循环实现）→ 全过。
  4. 在 `lr_notes.md` 写 LR(0)/SLR/LALR/LR(1) 对比：表达能力 vs 状态数 vs 实现复杂度，每档 1 行 + YACC 选 LALR 的理由。

### AX04 AST 求值 + 语义检查
- **预期档位**: medium
- **考察维度**: AST 设计 + 作用域
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ax04/` 写 `eval_ast.py`：实现 `eval(node, env)` —— 支持 `let x = expr`（赋值到 env）、`x`（查 env）、`expr1 + expr2`；输入 `["let", "x", ["+", 1, 2]]; ["*", "x", 3]` 输出 `{"x": 3, "result": 9}`。
  2. 写 `test_eval.py`：断言 "x" 未定义时抛 KeyError、`let x=1; let x=2; x` 输出 2（遮蔽）、`1 + "str"` 抛 TypeError。
  3. 跑脚本输出 "4/4 pass"；故意把 let 实现成不覆盖旧值，断言 fail → 改回覆盖 → 全过。
  4. 在 `scope_notes.md` 写"作用域三实现"：HashMap 嵌套 / Stack / 链表，每实现 1 行 + 复杂度对比（查找 O(1)/O(n)/O(n)）。

### AX05 IR + SSA：三地址码转换
- **预期档位**: hard
- **考察维度**: IR 设计 + SSA 形式
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ax05/` 写 `to_ta.py`：把 AST 转三地址码（TAC）——每个中间值用 `t0, t1, ...` 命名，输入 `a + b * c` 输出 `t0 = b * c; t1 = a + t0`。
  2. 写 `test_ta.py`：断言 TAC 中临时变量数 ≥ 2、操作数顺序正确、无副作用（同表达式多次出现共享临时变量）。
  3. 跑脚本输出 "3 项 TAC 断言全过"；故意把 `t1 = a + t0` 写成 `t1 = a + t1`（引用自身），断言 fail → 改回 → 全过。
  4. 在 `ssa_notes.md` 写 SSA φ 节点：定义 + 转换规则（每基本块入口插 φ 当多个前驱定义同名变量），画一个 2 前驱例子 ASCII 3 行。

### AX06 代码生成：AST → 伪汇编
- **预期档位**: hard
- **考察维度**: CodeGen + 寄存器分配
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ax06/` 写 `codegen.py`：把 TAC 转伪汇编（LOAD/ADD/STORE/MUL 指令集），用 4 个虚拟寄存器 R0-R3，溢出时用 stack slot，输入 `a + b * c` 输出 5 行汇编。
  2. 写 `test_codegen.py`：断言输出行数 ≥ 4、MUL 指令在 ADD 之前（正确顺序）、所有目标寄存器 ∈ {R0-R3}。
  3. 跑脚本输出 "3 项汇编断言全过"；故意把寄存器分配改成固定 R0，断言 ADD/MUL 冲突 → 改回简单线性分配 → 全过。
  4. 在 `reg_alloc_notes.md` 写 Chaitin 算法三步：干涉图构建 / 简化栈 / 溢出选择，每步 1 行 + 1 个关键规则。

### AX07 优化遍：常量折叠 + 死代码消除
- **预期档位**: hard
- **考察维度**: 编译器优化 + PGO
- **工具链**: Write → Bash → Bash → Read
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ax07/` 写 `optimizer.py`：实现 2 个优化遍——`const_fold(tac)`（`t0 = 1 + 2` → `t0 = 3`）+ `dce(tac)`（删未使用的临时变量赋值）。
  2. 写 `test_opt.py`：输入 `a = 1 + 2; b = a * 3; c = 10`（c 未使用），断言折叠后 `a = 3`、dce 后删 `c = 10`。
  3. 跑脚本输出 "2 项优化断言全过"；故意把 dce 规则写反（删使用的、留未使用），断言 fail → 改回 → 全过。
  4. 在 `pgo_notes.md` 写"AutoFDO vs PGO"：插桩（gcc -fprofile-generate）vs perf + 二进制重写，各 1 行 + 优缺点 1 行。

### AX08 JIT 降级：树遍历解释器 vs 字节码 VM
- **预期档位**: hard
- **考察维度**: JIT + 自适应优化
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ax08/` 写 `tree_interp.py`：树遍历解释器直接 eval AST；写 `bytecode_vm.py`：先把 AST 编译成自定义字节码（PUSH/ADD/MUL/RETURN），再跑 VM 执行。
  2. 写 `bench.py`：对同一个复杂 AST（50 节点）跑 1000 轮，输出两者耗时比 `tree=Xms bytecode=Yms` 到 `bench.log`，断言 Y < X（字节码更快）。
  3. 故意把 bytecode_vm 的 dispatch 改成线性 search（模拟低效实现），断言 Y > X（演示效率退化）→ 改回直接分派 → Y < X。
  4. 在 `jit_notes.md` 写"HotSpot C1/C2 层级协作"：C1 快速编译 + C2 深度优化 + OSR（On-Stack Replacement）在运行时替换，每层 1 行。

### AX09 DSL 设计：外部 DSL "任务配置"
- **预期档位**: medium
- **考察维度**: DSL 范式 + 设计权衡
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ax09/` 写 `task_dsl.py`：实现一个外部 DSL——语法 `task "name" { deps: [a, b]; cmd: "echo hi" }`，用 pyparsing 或手写 recursive descent 解析，输出 `Task` 对象列表。
  2. 写 `test_dsl.py`：断言解析 3 个 task 含 2 个 deps、deps 列表顺序与输入一致、cmd 字符串正确。
  3. 跑脚本输出 "3 项 DSL 断言全过"；故意把 deps 解析成字符串而非列表，断言 fail → 改回 list → 全过。
  4. 在 `dsl_notes.md` 写"内部 vs 外部 DSL"：Gradle Kotlin DSL / jQuery（内部）vs SQL / HTML / CSS（外部），每类 1 行 + 1 个关键设计决策。

### AX10 Mini-Lisp 解释器：端到端
- **预期档位**: hard
- **考察维度**: 编译原理端到端实战
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ax10/` 写 `mini_lisp.py`：完整 Mini-Lisp——lexer（S-expression 括号 token）、parser（AST 嵌套 list）、eval（支持 `define/lambda/if/quote/+/print`），用 Write。
  2. 先写 `test_mini_lisp.py` 含 5 个 failing 测试（先红）：`(lambda (x) (+ x 1)) 5 → 6`、`(define fact (lambda (n) (if (< n 2) 1 (* n (fact (- n 1)))))) (fact 5) → 120`、`(quote (1 2 3)) → [1,2,3]`。
  3. 跑测试故意触发 fail → 补全 eval 的 lambda/if/quote 支持 → 重跑断言 "5/5 pass" 到 `lisp.log`。
  4. 在 `lisp_extensions.md` 写"从解释器到编译器演进"：尾调用优化 / 宏 / 字节码编译 / JIT，每阶 1 行 + 关键改造点 1 词。
