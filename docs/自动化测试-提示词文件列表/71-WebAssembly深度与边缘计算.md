# 71 WebAssembly 深度与边缘计算

> 编号段 BT01–BT10 · 多轮对话测试提示词
> 主题：WebAssembly 深度与边缘计算（Wasm 字节码与文本格式、wasmtime 执行器、WASI 系统接口、Component Model 多语言组合、Edge Runtime 部署、Capability-based 安全、性能调优 SIMD/多线程、Rust 工具链、与容器对比、WASI Preview 2 特性、Edge 应用场景）
> 已有文件覆盖：桌面应用（Electron/Tauri）、WASM 沙箱（wasmtime 沙箱层）、系统编程（Landlock/Seccomp/io_uring）
> 本文件聚焦：**Wasm 作为应用运行时 + Edge 边缘部署**（非桌面 GUI、非沙箱隔离、非内核交互，而是运行时执行模型、边缘计算落地方案、跨平台 ABI 组合）

---

### BT01 Wasm 字节码与文本格式（S-表达式）的理解
- **预期档位**: simple
- **考察维度**: 考察对 Wasm 二进制格式（.wasm）与文本格式（.wat）转换、模块结构的理解
- **对话脚本**:
  1. 我想学习 WebAssembly，听说 Wasm 有文本格式和字节码格式，.wat 和 .wasm 分别是什么？S-表达式 `(module (func))` 怎么对应到二进制？给我一个最小可运行的 hello world 的 wat 写法。
  2. 我看到 wat 里有 `(i32.const 42)` 和 `(drop)`，这个栈式虚拟机和我们平时用的寄存器虚拟机有什么区别？为什么 Wasm 选栈式而不是寄存器式（像 Lua 5.1 后来改寄存器那样）？
  3. 我把 wat 转成 wasm 后文件小了很多，.wasm 的 LEB128 编码是怎么压缩数字的？拿 `i32.const 130` 举例说明一下编码过程。
  4. 刚才说的栈式模型让我好奇：Wasm 的 locals 和 stack 是什么关系？函数参数算 local 还是 stack？调用 `call $add` 时参数从哪来？

### BT02 wasmtime 执行器与 JIT/AOT 编译
- **预期档位**: medium
- **考察维度**: 考察对 wasmtime 运行时、Cranelift 编译器、JIT vs AOT 的理解
- **对话脚本**:
  1. 我想在 Rust 项目里嵌入一个 Wasm 运行时，wasmtime 和 wasmer 选哪个？它们底层编译器分别是什么？Cranelift 和 LLVM 在 Wasm 场景下各有什么优劣？
  2. 你说 wasmtime 默认是 JIT，那 AOT 模式怎么开？`wasmtime compile` 编译出来的 `.cwasm` 文件能跨机器用吗？还是绑定 CPU 架构？
  3. 我写了个 Rust 函数编译成 Wasm，想在 wasmtime 里调用。实例化 Module → Instance → 调用 export 函数，这个流程里 Store 是什么角色？为什么 wasmtime 要强制所有 Wasm 访问都经过 Store？
  4. 上一轮提到 Store 是资源隔离边界，那如果我想在同一个进程里跑两个互不干扰的 Wasm 实例，是开两个 Store 就行，还是必须两个 Engine？Engine 和 Store 各自的职责边界在哪？
  5. 我听说 Cranelift 有单趟编译（singlepass）模式，适合短生命周期的 Wasm（比如边缘计算冷启动）。singlepass 和默认的优化编译在编译速度和运行速度上怎么权衡？Cloudflare Workers 用的是哪种？

### BT03 WASI 与系统接口抽象
- **预期档位**: medium
- **考察维度**: 考察对 WASI（WebAssembly System Interface）的理解，Preview 1 vs Preview 2
- **对话脚本**:
  1. Wasm 沙箱里不能直接访问文件系统，WASI 是怎么解决这个问题的？`wasi_snapshot_preview1` 里有哪些核心接口？给我列几个常用的比如 fd_read / fd_write / proc_exit。
  2. 我用 `wasm32-wasi` target 编译 Rust 代码，生成的 .wasm 里 import 了 `wasi_snapshot_preview1` 的函数。如果我在 wasmtime 里跑，怎么给它传一个"虚拟目录"让它能读 ./data 下的文件？
  3. 上一轮提到虚拟目录，WASI 的 Capability-based 访问控制是什么意思？是不是说 Wasm 模块不能凭空打开 `/etc/passwd`，必须宿主显式授予某个目录的 capability？这和 Docker 的 volume mount 有什么本质区别？
  4. 我想让同一个 Wasm 模块在 wasmtime（本地）和 Cloudflare Workers（边缘）都能跑，但 Workers 没有文件系统。WASI 的接口在不同运行时里是怎么实现兼容的？有没有"部分实现"的情况？

### BT04 Component Model 与多语言组合（WIT 接口）
- **预期档位**: hard
- **考察维度**: 考察对 Component Model、WIT 接口定义、多语言 Wasm 组合的理解
- **对话脚本**:
  1. 我听说 Wasm Component Model 解决了"多语言互操作"问题。以前 Rust 编译的 Wasm 和 Go 编译的 Wasm 互相调不了，Component Model 是怎么解决的？WIT 文件长什么样？
  2. 给我写一个 WIT 接口：定义一个 `image-processor` world，包含 `resize(width: u32, height: u32) -> list<u8>` 和 `grayscale() -> list<u8>`。然后用 `cargo component` 生成 Rust 绑定，写个实现。
  3. 上一轮生成了 Rust 实现，现在我想用 Python（通过 componentize-py）来消费这个 Component。Python 端怎么 import 这个 WIT 接口？类型映射是怎样的（比如 `list<u8>` 对应 Python 的 bytes 还是 bytearray）？
  4. Component Model 的 ABI 是怎么做到语言中立的？字符串在不同语言之间传递时，内存谁分配、谁释放？WIT 里的 `string` 和 `list<T>` 在 wire format 里怎么编码？
  5. 我理解 Component Model 是 Wasm 的"ABI 标准"，那它和 WASI 的关系是什么？WASI Preview 2 是不是已经用 Component Model 重写了？

### BT05 Edge Runtime：Cloudflare Workers / Fermyon Spin / Deno Deploy
- **预期档位**: medium
- **考察维度**: 考察对主流 Wasm Edge Runtime 的理解与对比
- **对话脚本**:
  1. 我想把 Wasm 部署到边缘计算平台，Cloudflare Workers、Fermyon Spin、Deno Deploy 这三个各有什么特点？它们底层都是 V8 isolate 吗？还是有的用 wasmtime？
  2. Cloudflare Workers 的"无冷启动"是怎么做到的？传统 Lambda 冷启动几百毫秒，Workers 声称 0ms，是因为 V8 isolate 比容器轻量，还是因为 Wasm 本身启动快？
  3. 上一轮提到 V8 isolate，Fermyon Spin 用的是 wasmtime，那 Spin 和 Workers 在 Wasm 执行模型上有什么本质区别？Spin 的"触发器"（HTTP trigger / Redis trigger）是怎么和 Wasm 模块绑定的？
  4. 我想写一个 HTTP 边缘函数：接收请求 → 处理 → 返回响应。在 Workers 里是 `addEventListener('fetch', ...)`，在 Spin 里是 `#[http_component]` 宏。这两种写法背后分别对应什么 ABI？Spin 的 HTTP 请求/响应怎么映射到 Wasm 的线性内存？
  5. Deno Deploy 支持直接部署 TypeScript 编译的 Wasm 吗？还是说 Deno 本身跑 TS，Wasm 只是作为模块被 import？这三家里对 Rust → Wasm 的支持哪家最成熟？

### BT06 Wasm 安全与 Capability-based Security
- **预期档位**: medium
- **考察维度**: 考察对 Wasm 安全模型、Capability-based 权限控制的理解
- **对话脚本**:
  1. Wasm 沙箱号称"内存安全"，但 Spectre 类侧信道攻击对 Wasm 有效吗？浏览器里的 Wasm 和独立运行时（wasmtime）的安全边界有什么不同？
  2. Capability-based security 在 Wasm 里具体怎么体现？我写了一个 Wasm 模块，它能不能偷偷发网络请求？宿主怎么确保它只能访问我允许的域名？
  3. 上一轮提到网络能力，WASI 的 `sockets` 扩展（tcp/udp）是怎么做 capability 控制的？是不是必须显式给 Wasm 模块传一个 `TcpSocket` handle，它才能 connect？和 Unix 的 capability（CAP_NET_RAW）有什么对应关系？
  4. 我想实现一个"插件系统"：用户上传 Wasm 插件，我的宿主程序加载执行。怎么防止插件耗尽 CPU/内存？wasmtime 的 `Store` 有资源限制（memory_limit / epoch interruption）吗？
  5. 插件系统还要防"信息泄露"：两个不同用户的 Wasm 插件之间能不能通过共享内存通信？Wasm 的线性内存隔离是模块级还是实例级？

### BT07 Wasm 性能调优：SIMD / 多线程 / 内存池
- **预期档位**: hard
- **考察维度**: 考察对 Wasm 性能优化手段（SIMD-128、共享内存多线程、池化）的理解
- **对话脚本**:
  1. 我想用 Wasm 做图片处理（比如卷积滤波），纯标量运算太慢。Wasm 的 SIMD-128 扩展怎么用？Rust 里 `std::arch::wasm32::*` 有 `v128` 类型，给我写个 4 像素并行 RGBA 转灰度的例子。
  2. 上一轮 SIMD 让单指令处理 4 个像素，但我想用多线程并行处理整张图。Wasm 的多线程是怎么实现的？`shared` 内存 + `atomic` 指令？Rust 的 `std::thread` 能直接编译到 Wasm 线程吗？
  3. 你说 Wasm 线程依赖 `SharedArrayBuffer`，那在浏览器里需要 COOP/COEP 响应头。在 wasmtime 里跑多线程 Wasm 需要额外配置吗？线程池大小怎么控制？
  4. 我注意到 Wasm 的线性内存每次 `memory.grow` 都可能导致重新分配。频繁处理大图片时内存碎片严重。有没有办法预分配一个大内存池，让 Wasm 模块自己管理分配？`#[global_allocator]` 能换成自定义的 bump allocator 吗？
  5. 综合前面的 SIMD + 多线程 + 内存池，给我设计一个高性能 Wasm 图片处理器的架构：输入图片怎么进 Wasm 内存、worker 线程怎么分片、输出怎么零拷贝回宿主？

### BT08 Rust → Wasm 工具链：wasm-pack / wasm-bindgen / cargo component
- **预期档位**: medium
- **考察维度**: 考察对 Rust Wasm 工具链各工具职责的理解
- **对话脚本**:
  1. 我想把 Rust 库编译成 Wasm 给 JS 调用，听说要用 wasm-pack 和 wasm-bindgen。这两个工具分别做什么？`#[wasm_bindgen]` 宏展开后生成了什么？
  2. 我写了个 Rust 函数 `pub fn process(data: &[u8]) -> Vec<u8>`，加了 `#[wasm_bindgen]` 后编译报错说不支持切片。wasm-bindgen 对参数类型有什么限制？`&[u8]` 和 `Vec<u8>` 怎么映射到 JS 的 Uint8Array？
  3. 上一轮的类型映射让我好奇：如果我想在 JS 和 Rust 之间传复杂结构体（比如 `{ width: u32, height: u32, pixels: Vec<u8> }`），wasm-bindgen 怎么处理？是序列化成 JSON 还是直接内存布局？性能差异多大？
  4. 我听说 `cargo component` 是新一代工具，和 wasm-pack 是什么关系？`cargo component build` 生成的是 Component Model 格式的 .wasm，那它还能被 JS 直接 import 吗？还是必须用 JS 的 Component Model 运行时？
  5. 我的 Rust 库依赖了 `tokio` 异步运行时，编译到 Wasm 后 `async fn` 怎么和 JS 的 Promise 对接？wasm-bindgen 的 `#[wasm_bindgen(js_name = "...")]` 对 async 函数做了什么转换？

### BT09 Wasm 与容器对比：启动速度 / 镜像体积 / 隔离
- **预期档位**: medium
- **考察维度**: 考察对 Wasm 与容器（Docker）在边缘场景下差异的理解
- **对话脚本**:
  1. 有人说"Wasm 会取代 Docker"，真的吗？从启动速度、镜像体积、隔离强度三个维度对比一下 Wasm 和容器。
  2. 启动速度方面：Docker 容器冷启动大概 100-500ms，Wasm 实例化只要 1-10ms。这差距是因为 Wasm 不需要启动 Linux 用户态（init/systemd），还是因为 Wasm 模块比镜像小得多？
  3. 上一轮提到镜像体积，一个 Alpine 容器镜像大概 5MB，而同等功能的 Wasm 模块可能只有 100KB。但 Wasm 模块运行时需要 wasmtime 运行时（几十 MB），那整体占用谁大？在边缘节点上跑 1000 个实例时内存占用怎么算？
  4. 隔离强度方面：Docker 用 namespace + cgroup + seccomp 做进程隔离，Wasm 用线性内存沙箱 + capability。如果 Wasm 模块被恶意代码利用（比如通过宿主导入函数），它能逃逸到宿主系统吗？和容器逃逸（CVE-2019-5736）比哪个更难？
  5. 综合来看，什么场景下 Wasm 比容器更适合？什么场景下容器仍然是更好的选择？给我一个决策树：什么时候选 Wasm、什么时候选容器、什么时候两者结合（比如 Kata + Wasm）。

### BT10 WASI Preview 2 新特性与实际 Edge 应用
- **预期档位**: hard
- **考察维度**: 考察对 WASI Preview 2 特性及 Edge 场景落地的综合理解
- **对话脚本**:
  1. WASI Preview 2 相比 Preview 1 最大的变化是什么？听说引入了 Component Model 和"基于类型的方法调用"（typed interfaces），能具体说说吗？`wasi:http` 和 Preview 1 的 `fd_*` 有什么本质区别？
  2. 我想用 WASI Preview 2 写一个边缘 SSR（服务端渲染）应用：接收 HTTP 请求 → 渲染 HTML → 返回。`wasi:http/types` 的 `IncomingRequest` 和 `ResponseOutparam` 怎么用？给我写个 Rust 伪代码。
  3. 上一轮 SSR 场景里，如果我想在边缘做 AI 推理（比如用 ONNX 模型做图片分类），Wasm 里跑 ONNX Runtime 可行吗？WASI 的 `filesystem` 和 `random` 扩展够不够？推理性能相比原生 Rust 慢多少？
  4. 综合前面几轮：如果我要在 Cloudflare Workers 上部署一个"图片处理 + AI 推理"的边缘服务，用 Rust → Wasm + Component Model + WASI Preview 2 的技术栈，给我设计完整架构：模块拆分（HTTP 入口 / 图片处理 / AI 推理）、模块间通信方式、性能瓶颈在哪。
  5. 最后展望：Wasm GC（垃圾回收）提案和 Exception Handling 提案对边缘计算有什么意义？如果 Wasm 能直接操作 GC 对象（比如 JS 的 Object），Component Model 的 ABI 会怎么变化？Wasm 有没有可能成为"通用计算单元"，统一浏览器、边缘、嵌入式三个场景？
