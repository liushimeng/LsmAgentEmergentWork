# 188 Csharp 与 DotNet 跨平台编程实战

> 编号段 FZ01–FZ10 · 聚焦「C# 语言特性 + .NET 生态工程」：语言特性速览 / dotnet CLI 工程管理 / LINQ 集合管道 / async-await 与 Task / ASP.NET Core 最小 API / EF Core 建模 / xUnit 测试 / 跨平台 GUI / NuGet 包管理 / .NET 诊断工具链
>
> 与现有维度互补说明：
> - `26-编程语言全景与多语言实战`（Z 维）覆盖 Go/Java/Kotlin/Swift/Ruby/PHP/Lua/Elixir/Zig/Haskell；`59-小众与新兴编程语言巡礼`（BH 维）覆盖 Nim/Crystal/Julia/R/Scala3/OCaml+F#/Erlang/Racket/Fortran+COBOL/Odin+V+Gleam —— **两大语言维度均未覆盖 C#/.NET**，本文件填补该空白（F# 仅作 ML 范式代表出现，与本文件的 C# 工程生态不同）。
> - `18-Web全栈与前端工程实战`（R 维）聚焦 JS/TS 前端栈；本文件 FZ05 的 ASP.NET Core 是**后端 API**维度。
> - `61-企业业务系统开发实战`（BJ 维）是**业务建模**维度（语言无关）；本文件提供 C#/.NET 的**语言与工具链**实现底座。
>
> **跨平台运行约定**（对齐 Z 维 26 号文件模式）：每条先 `command -v dotnet` 探测；dotnet 不可用时降级为「产出 C# 源码 + python3 语义等价版对拍」，保证 macOS/Windows/Ubuntu/CentOS 7 全平台可执行。**本文件独特主题**：record 与模式匹配 / dotnet new-build-test-publish 全链 / LINQ 管道与惰性求值 / Task 组合器 WhenAll / minimal API 路由与契约 / EF Core Code First 迁移 / xUnit Theory 数据驱动 / Avalonia 与 MAUI 选型 / NuGet 版本与锁定 / dotnet-counters 与 dump 诊断。

---

### FZ01 C# 语言特性速览生成器

- **预期档位**: medium
- **考察维度**: 类型系统 / 现代特性 / 语义对拍
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/fz01_lang.py`：C# 特性速览生成器，产出 `fz01_notes.md`：(a) 类型基础（强类型/class vs struct 栈堆语义/nullable 引用类型 `string?`）+ (b) 现代 C# 十特性（record/模式匹配 switch 表达式/init-only/top-level statements/using 声明/范围运算符 `..`/原始字符串字面量/required 成员/primary constructor/file-scoped namespace 各一段 5 行内示例）+ (c) 与 Java 差异五点（属性 vs getter-setter/struct 值类型/事件与委托/异步无检查异常/LINQ vs Stream）+ (d) 同时产出 `fz01_records.cs`（record 定义 + 模式匹配演示的合法 C# 源码）与 `fz01_equiv.py`（dataclass + match 的 Python 语义等价版）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/fz01_lang.py && python3 tmpPlan/agent-test/fz01_lang.py`，断言生成三个产物文件且 `python3 -m py_compile` 等价版通过、退出码 0。
  3. Read `tmpPlan/agent-test/fz01_records.cs`，再 Bash：`command -v dotnet >/dev/null && dotnet --version || echo NO_DOTNET`；若输出 NO_DOTNET 则跑 `python3 tmpPlan/agent-test/fz01_equiv.py` 断言退出码 0；两者至少其一执行成功。

### FZ02 dotnet CLI 工程管理

- **预期档位**: medium
- **考察维度**: 工程创建 / 构建发布 / 全平台安装
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/fz02_cli.py`：dotnet CLI 速查生成器，输出：(a) 安装矩阵（Ubuntu apt 或 dotnet-install 脚本 / CentOS 7 rpm 或脚本 + CentOS 7 glibc 老旧需选 LTS 版本注意 / macOS brew install dotnet-sdk / Windows winget install Microsoft.DotNet.SDK.8）+ (b) 工程命令族（dotnet new console/classlib/xunit/webapi + -o 输出目录 + sln 组织多项目）+ (c) 构建与运行（dotnet build/run + 配置 Debug/Release + 输出目录 bin/obj 结构说明 + --no-restore 增量提速）+ (d) 发布矩阵（dotnet publish -c Release / 框架依赖 FDD vs 自包含 SCD + rid 列表 linux-x64/osx-x64/win-x64 + 单文件发布 /p:PublishSingleFile=true）+ (e) 全链演练命令序列（8 条命令从空目录到可执行产物）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/fz02_cli.py && python3 tmpPlan/agent-test/fz02_cli.py > tmpPlan/agent-test/fz02_cli.md`，断言退出码 0。
  3. Read `tmpPlan/agent-test/fz02_cli.md`，再 Bash 断言四组关键词：`grep -F 'dotnet new'`、`grep -F 'dotnet publish'`、`grep -F 'CentOS 7'`、`grep -F 'PublishSingleFile\|自包含'` 各命中。

### FZ03 LINQ 与集合管道

- **预期档位**: medium
- **考察维度**: 查询语法 / 惰性求值 / 管道设计
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/fz03_linq.py`：LINQ 管道演示器，产出 `fz03_pipeline.cs`（订单集合 → Where 过滤 → GroupBy 分组 → Select 投影 → OrderBy 排序 → Take 截断 的完整 C# 管道 + 注释每步惰性求值语义）与 `fz03_pipeline.py`（Python 生成器表达式语义等价版，逐行对照注释），再产出说明段：(a) 方法链 vs 查询表达式语法对照 + (b) 延迟执行陷阱（多次枚举重复计算/修改源后再枚举/用 ToList() 固化时机）+ (c) 常用算子速查（Select/Where/Aggregate/SelectMany/ToDictionary/First vs FirstOrDefault 异常语义）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/fz03_linq.py && python3 tmpPlan/agent-test/fz03_linq.py && python3 -m py_compile tmpPlan/agent-test/fz03_pipeline.py`，断言退出码 0。
  3. Read `tmpPlan/agent-test/fz03_pipeline.cs`，再 Bash：`command -v dotnet >/dev/null && echo HAS_DOTNET || python3 tmpPlan/agent-test/fz03_pipeline.py | tee tmpPlan/agent-test/fz03_out.txt`；断言对拍输出含分组结果数字与 `grep -F '惰性\|延迟'` 在 .cs 或说明中命中。

### FZ04 async-await 与 Task 并发

- **预期档位**: hard
- **考察维度**: Task 组合器 / 取消令牌 / 异常聚合
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/fz04_async_gen.py`：异步模型对比器（直接运行无需输入），产出两个文件：`fz04_async.cs`（async Task 方法 + Task.WhenAll 并发三任务 + CancellationToken 协作取消 + try/catch 聚合异常的 C# 示例源码）与 `fz04_demo.py`（asyncio.gather 并发 + asyncio.wait_for 超时 + 取消传播的 Python 语义等价可运行版），stdout 说明段含：(a) async/await 状态机直觉（编译器重写为回调 + 不等于线程 + ConfigureAwait 语境）+ (b) Task 组合器对照（WhenAll vs WhenAny vs Python gather/return_exceptions）+ (c) 三条铁律（async void 禁用/不 .Result 死锁/取消要传 CancellationToken）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/fz04_async_gen.py && python3 tmpPlan/agent-test/fz04_async_gen.py > tmpPlan/agent-test/fz04_notes.md && python3 tmpPlan/agent-test/fz04_demo.py`，断言退出码 0、demo 输出含 `gather` 或 `完成` 字样。
  3. Read `tmpPlan/agent-test/fz04_async.cs`，再 Bash：断言 `grep -F 'Task.WhenAll'` 命中、`grep -F 'CancellationToken'` 命中、`grep -F 'async Task'` 命中、`grep -F '死锁\|ConfigureAwait' tmpPlan/agent-test/fz04_notes.md` 命中。

### FZ05 ASP.NET Core 最小 API

- **预期档位**: hard
- **考察维度**: minimal API / 路由与契约 / 中间件管道
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/fz05_api.py`：最小 API 设计器（输入资源描述 JSON），产出 `fz05_program.cs`（MapGet/MapPost/MapPut/MapDelete 四路由 + DTO record + 校验返回 ProblemDetails + app.Use 中间件打点）与 `fz05_contract.md`：(a) 路由表（方法/路径/入参/出参/状态码五列表格）+ (b) 与 Controller 风格取舍（minimal 轻量 vs Controller 适合大项目模型绑定）+ (c) 中间件管道顺序图（UseRouting → UseAuthentication → UseAuthorization → MapEndpoints 文本图 + 顺序错乱的后果）+ (d) 本地跑法（dotnet run + curl 冒烟 4 条命令）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/fz05_api.py`，断言退出码为 0。
  3. Read `tmpPlan/agent-test/fz05_api.py`，再 Write `tmpPlan/agent-test/fz05_resource.json`：资源=图书管理（GET 列表分页/GET 详情/POST 创建校验 ISBN/PUT 更新/DELETE 软删除）。本轮必须显式复用第一级的路由表与状态码设计。
  4. Bash：`python3 tmpPlan/agent-test/fz05_api.py tmpPlan/agent-test/fz05_resource.json 2>&1 | tee tmpPlan/agent-test/fz05_api_out.md`，断言 `grep -F 'MapGet'` 命中、`grep -F 'MapPost'` 命中、`grep -F 'ProblemDetails\|校验'` 命中、`grep -F '中间件\|UseAuthorization'` 命中。

### FZ06 EF Core 数据建模与迁移

- **预期档位**: hard
- **考察维度**: Code First / Fluent API / 迁移生命周期
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/fz06_ef.py`：EF Core 建模器（输入表结构 JSON），产出 `fz06_entities.cs`（DbContext + 实体类 + Fluent API OnModelCreating：主键/唯一索引/关系级联行为）与 `fz06_migrations.md`：(a) 迁移命令链（dotnet ef migrations add Initial → database update → script 生成 SQL）+ (b) 迁移治理（命名规范/生产禁止自动迁移/down 回滚/迁移与 Git 冲突合并策略）+ (c) 三种建模方式取舍（Data Annotation vs Fluent API vs 约定 + 何时必须 Fluent）+ (d) SQLite/PostgreSQL Provider 差异与连接串写法。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/fz06_ef.py`，断言退出码为 0。
  3. Read `tmpPlan/agent-test/fz06_ef.py`，再 Write `tmpPlan/agent-test/fz06_tables.json`：博客系统 4 表（blog/post/tag/post_tag 多对多 + 软删除标记 + 唯一索引 slug）。本轮必须显式复用第一级的 Fluent 约定与迁移治理。
  4. Bash：`python3 tmpPlan/agent-test/fz06_ef.py tmpPlan/agent-test/fz06_tables.json 2>&1 | tee tmpPlan/agent-test/fz06_ef_out.md`，断言 `grep -F 'DbContext'` 命中、`grep -F 'HasIndex\|HasOne\|HasMany'` 命中、`grep -F 'migrations add\|database update'` 命中、`grep -F '回滚\|down'` 命中。

### FZ07 xUnit 单元测试工程

- **预期档位**: medium
- **考察维度**: Fact 与 Theory / 断言语义 / 覆盖策略
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/fz07_test.py`：测试工程生成器，产出 `fz07_discount.cs`（被测函数：满减折扣计算）与 `fz07_discount_tests.cs`（[Fact] 常规 + [Theory]+[InlineData] 数据驱动 6 组 + 边界：0 元/负数抛 ArgumentException 断言 Throws）与对照段：(a) xUnit vs NUnit vs MSTest 三家选择 + (b) AAA 结构约定（Arrange-Act-Assert 注释分节）+ (c) dotnet test 常用参数（--filter 类/方法级筛选 + --collect 覆盖率 + --logger trx CI 集成）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/fz07_test.py && python3 tmpPlan/agent-test/fz07_test.py`，断言两个 .cs 产物生成。
  3. Read `tmpPlan/agent-test/fz07_discount_tests.cs`，再 Bash：断言 `grep -F '[Theory]'` 命中、`grep -c 'InlineData' ` ≥ 6、`grep -F 'Throws'` 命中、`grep -F 'dotnet test'` 在说明段命中。

### FZ08 .NET 跨平台 GUI 选型与 Avalonia 入门

- **预期档位**: medium
- **考察维度**: GUI 框架选型 / MVVM / 跨平台矩阵
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/fz08_gui.py`：GUI 选型顾问（输入目标应用画像 JSON），输出：(a) 框架矩阵（WPF 仅 Windows / WinUI 3 Windows / MAUI 移动+桌面但 Linux 缺席 / Avalonia 全平台 / Blazor Hybrid Web 技术栈 —— 平台覆盖×成熟度×学习曲线×生态 四维评分表）+ (b) MVVM 模式最小样例（Avalonia XAML 视图 + ViewModel 属性绑定 + Command 的 C# 代码骨架 30 行内）+ (c) 桌面三平台打包（dotnet publish rid + 自包含 + macOS dmg/Windows msix/Linux AppImage 通道说明）+ (d) 与 Electron/Tauri 的对比结论（内存占用/启动速度/团队技能匹配三判据）+ (e) 选型建议（按输入画像给唯一推荐 + 理由链）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/fz08_gui.py`，断言退出码为 0。
  3. Read `tmpPlan/agent-test/fz08_gui.py`，再 Write `tmpPlan/agent-test/fz08_profile.json`：画像=内部工具 + 必须跑 Windows+Ubuntu+macOS + 团队会 C# 不想引入前端栈 + 常驻内存敏感。本轮必须显式复用第一级的评分矩阵。
  4. Bash：`python3 tmpPlan/agent-test/fz08_gui.py tmpPlan/agent-test/fz08_profile.json 2>&1 | tee tmpPlan/agent-test/fz08_choice.md`，断言 `grep -F 'Avalonia'` 命中、`grep -F 'MVVM\|ViewModel'` 命中、`grep -F 'Electron\|Tauri'` 命中、结论段含 `推荐` 字样。

### FZ09 NuGet 包管理与依赖治理

- **预期档位**: medium
- **考察维度**: 包引用 / 版本浮动 / 私有源
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/fz09_nuget.py`：NuGet 治理指南生成器，输出：(a) 包管理三件套（dotnet add package / .csproj 中 PackageReference 版本节点 / dotnet restore）+ (b) 版本语义（SemVer 主次修 + 浮动版本 `6.0.*` 何时可用何时禁用 + Central Package Management Directory.Packages.props 统一多项目版本）+ (c) 锁定与可复现（packages.lock.json + RestorePackagesWithLockFile + CI `--locked-mode`）+ (d) 私有源（nuget.config 配置 source 段 + Azure DevOps/GitLab 私有 feed + 凭据不进仓的管理）+ (e) 供应链安全（nuget.org 依赖漏洞扫描 dotnet list package --vulnerable + 签名包验证说明）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/fz09_nuget.py && python3 tmpPlan/agent-test/fz09_nuget.py > tmpPlan/agent-test/fz09_nuget.md`，断言退出码 0。
  3. Read `tmpPlan/agent-test/fz09_nuget.md`，再 Bash 断言四组关键词：`grep -F 'add package'`、`grep -F 'Directory.Packages.props\|中央'`、`grep -F 'nuget.config'`、`grep -F 'vulnerable\|漏洞'` 各命中。

### FZ10 .NET 性能与诊断工具链

- **预期档位**: hard
- **考察维度**: 计数器 / dump 分析 / 性能门禁
- **工具链**: Write → Bash → Read → Write
- **对话脚本**:
  1. Write `tmpPlan/agent-test/fz10_diag.py`：诊断手册生成器（输入故障现象 JSON），输出：(a) 工具矩阵（dotnet-counters 实时指标 / dotnet-dump 抓快照 / dotnet-trace 采样 / dotnet-gcdump 堆对比 + 每工具安装命令 dotnet tool install --global）+ (b) 按现象路由（CPU 高→dotnet-trace 看热点函数 / 内存涨→gcdump 两点对比看对象增量 / GC 压力→counters 看 gen2 比例 / 卡顿→trace 看线程等待）+ (c) dump 分析入门（dotnet-dump collect → analyze 里 `clrstack`/`dumpheap -stat`/`gcroot` 三命令含义）+ (d) 跨平台注意（Linux/macOS 用 dotnet 工具族替代 Windows 专属 PerfView / 生产容器内 --diag 的边车模式）+ (e) 性能门禁（BenchmarkDotNet 微基准三板斧 + CI 阈值卡点思路）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/fz10_diag.py`，断言退出码为 0。
  3. Read `tmpPlan/agent-test/fz10_diag.py`，再 Write `tmpPlan/agent-test/fz10_issue.json`：现象=生产 API 内存每 6 小时涨 1GB + 重启恢复 + Linux 容器部署 + 无 Windows 机器。本轮必须显式复用第一级的按现象路由。
  4. Bash：`python3 tmpPlan/agent-test/fz10_diag.py tmpPlan/agent-test/fz10_issue.json 2>&1 | tee tmpPlan/agent-test/fz10_runbook.md`，断言 `grep -F 'dotnet-gcdump\|gcdump'` 命中、`grep -F 'dumpheap\|gcroot'` 命中、`grep -F 'dotnet-counters\|counters'` 命中、`grep -F 'PerfView\|Linux'` 命中。
