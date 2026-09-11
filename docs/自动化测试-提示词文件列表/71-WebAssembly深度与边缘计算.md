# 71 WebAssembly 深度与边缘计算

> 编号段 BT01–BT10 · 多轮对话测试提示词
> 主题：WebAssembly 字节码与文本格式、wasmtime 执行器、WASI 系统接口、Component Model 多语言组合、Edge Runtime 部署、Capability-based 安全、性能调优 SIMD/多线程、Rust 工具链、与容器对比、WASI Preview 2 特性、Edge 应用场景
> 本文件特殊约束：**无 Wasm 编译/运行工具链**（无 wasmtime/wasmer/wasm-pack 等）→ 全程 python3 模拟 Wasm 栈机指令子集（i32.add/i32.const/local.get 等）、WASI 能力模型、Edge Runtime 调度；如需真实 Wasm 字节码，仅生成 .wat 文本并展示栈机执行模拟
> 已有文件覆盖：桌面应用（Electron/Tauri）、WASM 沙箱（wasmtime 沙箱层）、系统编程（Landlock/Seccomp/io_uring）
> 本文件聚焦：**Wasm 作为应用运行时 + Edge 边缘部署**（非桌面 GUI、非沙箱隔离、非内核交互，而是运行时执行模型、边缘计算落地方案、跨平台 ABI 组合）
> 每条都落到可执行产物：Write 写 Wasm 模拟器 → Bash 跑断言 → Read 核对关键代码 → Bash 跑多场景验证，产物全部在 `tmpPlan/agent-test/` 沙盒内

---

### BT01 Wasm 字节码与文本格式（S-表达式）的理解
- **预期档位**: simple
- **考察维度**: Wasm 二进制格式（.wasm）与文本格式（.wat）转换、模块结构
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 下 Write `wat_min.py`：定义最小 WAT 子集——`(module (func (export "add") (param i32 i32) (result i32) (i32.add (local.get 0) (local.get 1))))`，写一个解析器把 S-表达式转 AST（dict 嵌套）。
  2. Bash 跑 `python3 tmpPlan/agent-test/wat_min.py`，断言解析结果含 `func`/`export "add"`/`i32.add` 节点（grep 校验），且参数列表长度=2、返回类型=i32，印证「WAT S-表达式可解析」。
  3. Read `wat_min.py` 确认 AST 节点定义存在。再 Write `leb128.py`：实现 LEB128 无符号编码——对整数 130 生成字节序列 `[0x82, 0x01]`（低 7 位 + 续位 + 高位 7-bit），演示编码 round-trip。
  4. Bash 跑 `python3 tmpPlan/agent-test/leb128.py`，断言编码 130 输出 `[0x82, 0x01]`、编码 624485 输出 `[0xe5, 0x8e, 0x26]`（grep 校验），且解码函数对任一编码 round-trip 后值不变，用输出印证「LEB128 编码/解码正确」。

### BT02 wasmtime 执行器与 JIT/AOT 编译
- **预期档位**: medium
- **考察维度**: wasmtime 运行时、Cranelift 编译器、JIT vs AOT
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 下 Write `stack_machine.py`：实现最小 Wasm 栈机——指令集 `i32.const N`（压栈）、`i32.add`（弹两值压和）、`local.get idx`（压 local）、`local.set idx`（弹栈写 local）、`call $f`（调用函数），含函数调用栈 `call_stack`。
  2. Bash 跑 `python3 tmpPlan/agent-test/stack_machine.py`，断言执行 `(i32.const 3)(i32.const 4)(i32.add)` 后栈顶=7（grep -F 校验），印证「栈机执行单 i32.add」正确。
  3. Read `stack_machine.py` 确认 `local.get` 实现存在。再 Write `module_inst.py`：实现 Module + Instance 概念——Module 是指令序列（不可变）、Instance 包含线性内存 + globals + 函数引用，演示 `Module.add(a,b)` 被两个 Instance 同时调用但内存隔离。
  4. Bash 跑 `python3 tmpPlan/agent-test/module_inst.py`，断言 Instance1 和 Instance2 都返回 add(3,4)=7，但修改 Instance1 的内存后 Instance2 的内存不变（grep 校验），用输出印证「Module/Instance 内存隔离 + Store-as-isolation-boundary」。

### BT03 WASI 与系统接口抽象
- **预期档位**: medium
- **考察维度**: WASI 系统接口、Preview 1 vs Preview 2
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 下 Write `wasi_caps.py`：实现 WASI capability 模型——类 `WasiCtx` 含 `allowed_paths: set` + `allowed_envs: set`，方法 `open(path)` 检查 `path` 是否在 `allowed_paths` 内，否则抛 `PermissionError`。
  2. Bash 跑 `python3 tmpPlan/agent-test/wasi_caps.py`，断言 open(`/tmp/data`) 当 allowed_paths 含 `/tmp` 时成功、open(`/etc/passwd`) 抛 PermissionError（grep -F 校验），印证「capability-based 访问控制」。
  3. Read `wasi_caps.py` 确认权限检查函数存在。再 Write `wasi_preview2.py`：模拟 WASI Preview 2 的 typed interface——接口 `wasi:filesystem/types` 含 `read(fd, offset, len) -> list<u8>` 类型化方法，与 Preview 1 的 `fd_read`（无类型）对比。
  4. Bash 跑 `python3 tmpPlan/agent-test/wasi_preview2.py`，断言 typed interface 返回值带类型标注（grep -F 校验）且 Preview 2 接口可在不同 runtime（wasmtime/Workers）复用，模拟同一接口在两个 runtime 行为一致，用输出印证「Preview 2 类型化接口的可移植性」。

### BT04 Component Model 与多语言组合（WIT 接口）
- **预期档位**: hard
- **考察维度**: Component Model、WIT 接口定义、多语言组合
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 下 Write `wit_min.py`：定义最小 WIT——`world image-processor { export resize: func(width: u32, height: u32, data: list<u8>) -> list<u8>; }`，写 WIT 解析器把 world + exports 转 AST。
  2. Bash 跑 `python3 tmpPlan/agent-test/wit_min.py`，断言解析结果含 world 名 `image-processor` 和 export `resize`（grep 校验），参数列表含 width/height/data，返回类型为 list<u8>。
  3. Read `wit_min.py` 确认 WIT AST 节点存在。再 Write `abi_wire.py`：实现 Component Model ABI wire 格式——`list<u8>` 编码为 `length:u32 LE + bytes`、`string` 编码为 `length:u32 LE + utf8 bytes`，演示 round-trip。
  4. Bash 跑 `python3 tmpPlan/agent-test/abi_wire.py`，断言 list `[1,2,3]` 编为 `b'\x03\x00\x00\x00\x01\x02\x03'`（grep -E 校验），且字符串 `"hi"` 编为 `b'\x02\x00\x00\x00hi`，跨语言 round-trip 后值不变，用输出印证「WIT ABI wire 格式一致」。

### BT05 Edge Runtime：Cloudflare Workers / Fermyon Spin / Deno Deploy
- **预期档位**: medium
- **考察维度**: 主流 Wasm Edge Runtime 理解与对比
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 下 Write `edge_runtime.py`：实现最小 Edge Runtime——`Isolate(module)` 含冷启动时间模拟（`cold_start_ms`）、请求处理函数 `handle_request(req) -> resp`，演示首次调用 50ms 冷启动、二次调用 1ms。
  2. Bash 跑 `python3 tmpPlan/agent-test/edge_runtime.py`，断言 `first_call_ms ≈ 50`、`second_call_ms < 5`（grep 校验），印证「无冷启动 vs 冷启动对比」。
  3. Read `edge_runtime.py` 确认计时函数存在。再 Write `http_trigger.py`：模拟 HTTP trigger——`#[http_component] async fn handle(req: Request) -> Response`，演示 `req.method` + `req.path` 提取，路由 `/api/v1/echo` 返回 200 + body。
  4. Bash 跑 `python3 tmpPlan/agent-test/http_trigger.py`，断言 GET `/api/v1/echo` 返回 200 + body 含 "echo"，POST `/api/v1/echo` 返回 405（grep 校验），用输出印证「Edge Runtime HTTP trigger 路由」。

### BT06 Wasm 安全与 Capability-based Security
- **预期档位**: medium
- **考察维度**: Wasm 安全模型、Capability-based 权限控制
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 下 Write `mem_safety.py`：实现 Wasm 线性内存边界检查——类 `Memory(size)` 含 `read(offset, len)` 和 `write(offset, data)`，访问越界 `[size, size+1]` 抛 `Trap`。
  2. Bash 跑 `python3 tmpPlan/agent-test/mem_safety.py`，断言越界访问抛 Trap 且正常访问成功（grep -F 校验），印证「Wasm 线性内存安全边界」。
  3. Read `mem_safety.py` 确认边界检查存在。再 Write `cap_net.py`：实现网络 capability 控制——类 `NetworkCtx` 含 `allowed_hosts: set`，方法 `connect(host, port)` 检查 host 是否在白名单，否则抛 `PermissionError`；演示 laew Wasm 模块只能连 `api.openai.com:443`，不能连 `evil.com`。
  4. Bash 跑 `python3 tmpPlan/agent-test/cap_net.py`，断言 `connect('api.openai.com', 443)` 成功、`connect('evil.com', 80)` 抛 PermissionError（grep 校验），用输出印证「capability 网络访问控制」。

### BT07 Wasm 性能调优：SIMD / 多线程 / 内存池
- **预期档位**: hard
- **考察维度**: Wasm 性能优化手段（SIMD-128、共享内存多线程、池化）
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 下 Write `simd_lane.py`：模拟 SIMD-128 4 通道运算——函数 `add_lane4(a: [a0,a1,a2,a3], b: [b0,b1,b2,b3]) -> [a0+b0, a1+b1, a2+b2, a3+b3]`，单次调用并行处理 4 个像素。
  2. Bash 跑 `python3 tmpPlan/agent-test/simd_lane.py`，断言 `add_lane4([1,2,3,4], [10,20,30,40]) = [11,22,33,44]`（grep -F 校验），且运行 1000 次时 SIMD 通道版比标量循环版快 3-4 倍（测量 wall time）。
  3. Read `simd_lane.py` 确认 4 通道并行逻辑存在。再 Write `linear_alloc.py`：实现 linear memory 池——类 `Pool(size)` 含 `alloc(n) -> addr`（bump allocator 分配，返回地址）、`free(addr)` 不真释放（仅记录），演示 1000 次 alloc+free 后总空闲可复用 ≥ 80%。
  4. Bash 跑 `python3 tmpPlan/agent-test/linear_alloc.py`，断言 1000 次 alloc+free 后空闲率 ≥ 80%（grep 校验），且 alloc 复杂度 O(1)（时间测量），用输出印证「bump allocator + 复用空闲块」。

### BT08 Rust → Wasm 工具链：wasm-pack / wasm-bindgen / cargo component
- **预期档位**: medium
- **考察维度**: Rust Wasm 工具链各工具职责
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 下 Write `abi_map.py`：模拟 Rust↔JS ABI 映射——函数 `map_rust_to_js(slice: &[u8]) -> Uint8Array` 把 Rust `&[u8]` 映射到 JS `Uint8Array`（共享同一段线性内存视图），演示 `[1,2,3] ↔ new Uint8Array(mem, offset, 3)`。
  2. Bash 跑 `python3 tmpPlan/agent-test/abi_map.py`，断言 JS 侧读 `Uint8Array` 三个值都正确（grep 校验），且修改 JS 侧值后 Rust 侧再次切片读到新值（共享内存），印证「零拷贝 ABI」。
  3. Read `abi_map.py` 确认共享内存逻辑存在。再 Write `complex_struct.py`：实现复杂结构体 ABI——`struct { width: u32, height: u32, pixels: Vec<u8> }` 序列化为内存布局 `[width][height][len][pixels...]`（小端），演示 round-trip。
  4. Bash 跑 `python3 tmpPlan/agent-test/complex_struct.py`，断言 100×100 图像（width=100,height=100,pixels=30000 字节）序列化后总字节数 = 12+30000=30012（grep 校验），且 round-trip 后 width/height/pixels 完全相等，用输出印证「结构体 ABI 序列化」。

### BT09 Wasm 与容器对比：启动速度 / 镜像体积 / 隔离
- **预期档位**: medium
- **考察维度**: Wasm 与容器（Docker）在边缘场景下差异
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 下 Write `startup_bench.py`：模拟冷启动对比——`wasm_instance(module_bytes)` 启动延迟 5ms、`container_instance(image)` 启动延迟 200ms，演示 1000 次实例化总时间对比。
  2. Bash 跑 `python3 tmpPlan/agent-test/startup_bench.py`，断言 Wasm 1000 实例总时间 < 10s、容器 1000 实例总时间 > 150s（grep -E 校验），印证「Wasm 冷启动优势 30x+」。
  3. Read `startup_bench.py` 确认计时函数存在。再 Write `isolation_strength.py`：对比隔离强度——Wasm 隔离（线性内存边界 + capability）+ 容器隔离（namespace + cgroup + seccomp），模拟攻击场景：恶意 Wasm 模块尝试访问线性内存外 → Trap；恶意容器进程尝试 `kill -9 1` → 失败（受 cgroup 限制）。
  4. Bash 跑 `python3 tmpPlan/agent-test/isolation_strength.py`，断言 Wasm 越界 Trap=True、容器权限不足 kill=True（grep 校验），且 Wasm 隔离在「线性内存安全」维度更强（运行时不可能越界），用输出印证「Wasm 与容器的隔离强度差异」。

### BT10 WASI Preview 2 新特性与实际 Edge 应用
- **预期档位**: hard
- **考察维度**: WASI Preview 2 特性及 Edge 场景落地
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/` 下 Write `wasi_http.py`：模拟 `wasi:http/types` 接口——`IncomingRequest` 含 `method/path/headers/body`、`ResponseOutparam` 含 `set(status, headers, body)`，演示 HTTP 路由处理 `GET /api/health` 返回 200 + `{"ok":true}`。
  2. Bash 跑 `python3 tmpPlan/agent-test/wasi_http.py`，断言 GET `/api/health` 返回 200 + body 含 `ok:true`（grep 校验），且 WASI HTTP 类型签名清晰（typed interface 比 Preview 1 的 fd_read 强）。
  3. Read `wasi_http.py` 确认 HTTP 类型接口存在。再 Write `edge_arch.py`：设计 Edge SSR + AI 推理组合架构——HTTP 入口模块（处理 request）调用图片处理模块（image resize），调用 AI 推理模块（用 mock 模型预测），最后 ResponseOutparam 输出 HTML（含推理结果）。
  4. Bash 跑 `python3 tmpPlan/agent-test/edge_arch.py`，断言完整流程从 HTTP request 到 HTML response 成功（grep 校验），且 3 个模块通过 typed interface 通信（无内存共享），用输出印证「Edge SSR + AI 推理的 Component Model 架构」。