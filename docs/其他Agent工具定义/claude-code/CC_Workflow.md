# Workflow 工具定义
```json
{
    "name": "Workflow",
    "description": "执行流程脚本，以确定性方式调度多个子代理。流程在后台运行，本工具会立即返回任务ID，流程执行完毕后你将收到 <task-notification> 通知。可使用 /workflows 命令查看实时运行进度。

    流程可统筹多个代理协同工作，适用于以下场景：拆分任务并并行执行以保证覆盖全面、正式提交前通过多方独立评审与交叉校验提升结果可靠性、处理单上下文无法承载的大规模任务（如代码迁移、审计、全域检索等）。脚本用于定义整体执行逻辑：哪些环节并行分发、哪些环节做校验、哪些环节汇总结果。

    **仅在用户明确启用多代理调度时，才可调用本工具**。流程会启动大量代理并消耗海量令牌，必须由用户主动提出使用，不可自行推断启用。明确启用包含以下情形：
    - 用户在指令中包含关键词「ultracode」（系统提示消息会予以确认）。
    - 当前会话已开启 ultracode 模式（系统提示消息会予以确认），详见下方「Ultracode 模式」说明。
    - 用户使用原话直接要求运行流程或启用多代理调度（例如“使用流程”、“运行流程”、“分发代理任务”、“用子代理统筹执行”）。仅任务本身适合使用流程，但用户未明确提出，不算作启用。
    - 用户调用的技能或斜杠指令中，明确要求使用流程编排工具。
    - 用户要求运行某个已命名或已保存的指定流程。

    其余所有任务，即便明显适合并行执行，**也禁止调用本工具**。可单独使用代理工具创建子代理，或简要说明多代理流程可实现的效果、大致资源消耗，并询问用户是否需要启用。同时告知用户，后续可直接输入“使用流程”来快速启用，省去询问步骤。

    实际使用时，推荐**混合执行**策略：先在线上完成前置探查（列出文件、确认通道、梳理变更范围），整理出任务清单后，再调用流程工具进行流水线处理。无需在创建任务之初就确定完整执行架构，只需在进入调度环节前规划好即可。

    可分多轮组合使用的常见单阶段流程：
    - **理解分析**：多代理并行读取相关子模块 → 输出结构化梳理结果
    - **方案设计**：多个代理独立输出不同实现思路 → 综合打分并汇总最优方案
    - **内容评审**：按不同维度检索问题 → 交叉对抗校验（示例见下文）
    - **信息调研**：多维度并行检索 → 深度精读内容 → 整合归纳结论
    - **代码迁移**：扫描目标目录 → 隔离工作区逐个转换 → 结果校验

    大型任务可按阶段串行执行，每完成一个阶段并查看结果后，再启动下一阶段。全程保持交互可控，每个流程仅负责范围明确的一批并行任务。

    **Ultracode 模式**：若系统提示显示该模式已开启，则默认长期启用流程能力，所有正式任务均需编写并运行流程脚本。目标是产出最详尽、准确的结果，令牌消耗不受限制。对于多阶段任务（分析→设计→开发→评审），通常按阶段依次执行多个流程，阶段之间保留交互环节。优先使用流程调度与交叉校验保障结果质量，仅纯对话类操作或简单机械编辑使用单个代理。若提示该模式已关闭，则恢复前文的「用户主动启用」规则。

    通过 `script` 字段直接传入脚本内容，**不要先将脚本写入文件**。每次调用本工具，都会自动将脚本保存至会话目录，并在返回结果中给出文件路径。如需迭代修改流程，可使用文件写入/编辑工具修改该文件，再通过 `{scriptPath: "<文件路径>"}` 重新调用流程工具，无需重复传入完整脚本。

    所有流程脚本必须以 `export const meta = {...}` 开头，示例如下：
      export const meta = {
        name: 'find-flaky-tests',
        description: 'Find flaky tests and propose fixes',   // 单行描述，将展示在权限弹窗中
        phases: [                                            // 与 phase() 调用一一对应
          { title: 'Scan', detail: 'grep test logs for retries' },
          { title: 'Fix', detail: 'one agent per flaky test' },
        ],
      }
      // 脚本主体从此处开始，可使用 agent()/parallel()/pipeline()/phase()/log() 接口
      phase('Scan')
      const flaky = await agent('grep CI logs for retry markers', {schema: FLAKY_SCHEMA})
      ...

    `meta` 对象**必须为纯字面量**，禁止使用变量、函数调用、扩展运算符或模板字符串。必填字段：`name`、`description`；可选字段：`whenToUse`（展示在流程列表中）、`phases`。`meta.phases` 内的阶段名称必须与代码中 phase() 调用的名称完全匹配；未匹配到的 phase() 会单独创建进度分组。如需某一阶段使用指定模型，可在阶段配置中新增 `model` 字段。

    ### 脚本内置接口说明
    - agent(prompt: string, opts?: {label?: string, phase?: string, schema?: object, model?: string, isolation?: 'worktree', agentType?: string}): Promise<any>
      启动子代理。未指定 schema 时，返回代理输出的纯文本字符串；指定 JSON 结构约束后，代理将强制使用结构化输出工具，接口直接返回校验后的对象，无需手动解析。若任务中途被用户终止，返回值为 null，可通过 .filter(Boolean) 过滤空值。
      opts.label：自定义展示名称；
      opts.phase：显式指定当前代理所属进度分组，在流水线/并行任务中使用，避免全局阶段状态冲突，相同阶段名称会归入同一分组；
      opts.model：为当前代理单独指定模型，若无特殊需求建议留空，代理将沿用会话默认模型；仅在明确确认需更换模型时使用；
      opts.isolation: 'worktree'：在全新 Git 工作区中运行代理，**资源开销较高**（单个代理启动耗时约200-500毫秒并占用磁盘空间），仅用于多代理并行修改文件、存在内容冲突的场景；文件无变更时，工作区会自动删除；
      opts.agentType：指定自定义子代理类型（如「内容探索」「代码评审」），取自代理工具同一份注册列表，可与 schema 搭配使用，自定义代理的系统提示后会追加结构化输出要求。

    - pipeline(items, stage1, stage2, ...): Promise<any[]>
      按条目依次执行全流程，**阶段之间无全局等待屏障**。例如条目A执行到第三阶段时，条目B可能仍在第一阶段。这是多阶段任务的**默认执行方式**。整体耗时取决于单条任务的最长执行链路，而非各阶段耗时总和。
      每个阶段回调函数可接收参数(上一阶段结果, 原始条目, 索引)，后续阶段可通过原始条目和索引做标识，无需依靠前序结果传递上下文。若某个阶段执行报错，对应条目结果置为 null，且不再执行后续阶段。

    - parallel(thunks: Array<() => Promise<any>>): Promise<any[]>
      并发执行多个任务，**存在全局等待屏障**：必须等待所有任务执行完毕后，接口才会返回。单个任务执行报错或代理异常，对应结果位会置为 null，接口本身不会抛出异常，使用前建议通过 .filter(Boolean) 过滤空值。仅在必须集齐所有任务结果时使用。

    - log(message: string): void
      向用户输出进度提示，展示在进度树上方的提示行中。

    - phase(title: string): void
      开启新执行阶段，后续所有 agent() 调用都会在进度展示中归入该阶段分组。

    - args: any
      流程工具入参中 args 字段的原始值，未传参则为 undefined。传入数组、对象时，需使用标准 JSON 格式，**禁止传入字符串化后的内容**（字符串格式会导致脚本内 filter、map 等数组方法报错）。用于给通用流程传递动态参数，如调研问题、目标路径、配置对象等，无需额外读取外部文件。

    - budget: {total: number|null, spent(): number, remaining(): number}
      本轮会话的令牌上限，由用户类似「+500k」的指令指定。未设置上限时，budget.total 为 null。
      budget.spent()：统计本轮主程序及所有流程已消耗的输出令牌，令牌池全局共享，不按单个流程隔离；
      budget.remaining()：返回剩余可用令牌，计算规则为 max(0, 总上限 - 已消耗)，无上限时返回无穷大。
      该上限为**硬性限制**，一旦已消耗值达到总上限，后续 agent() 调用会直接抛出异常。
      用法示例：动态循环 `while (budget.total && budget.remaining() > 50_000) { ... }`、动态扩缩容 `const FLEET = budget.total ? Math.floor(budget.total / 100_000) : 5`。

    - workflow(nameOrRef: string | {scriptPath: string}, args?: any): Promise<any>
      在当前流程内嵌套执行其他流程并返回其结果。传入名称可调用已保存的流程，传入 {scriptPath} 可执行本地脚本文件。子流程共享当前任务的并发上限、代理计数、终止信号与令牌配额，子流程的代理任务会在 /workflows 中以「▸ 流程名」分组展示，令牌消耗计入全局统计。子流程的 args 即为当前接口传入的参数。**仅支持单层嵌套**，子流程内禁止再次调用本接口。流程名称不存在、脚本文件无法读取或子脚本语法错误时会抛出异常，建议捕获异常做容错处理。

    子代理明确知晓输出内容为程序返回值，而非面向用户的对话内容，因此只会返回原始数据。使用 schema 约束结构化输出时，校验逻辑在工具层完成，格式不匹配会自动重试。

    流程内的代理可通过工具检索功能调用当前会话已授权的全部 MCP 工具，结构约束文件按需加载。注意：交互式登录的 MCP 服务（如 claude.ai）在后台定时任务场景下可能无法正常使用。

    脚本使用标准 JavaScript，**不支持 TypeScript**，类型注解、接口、泛型均会解析失败。脚本运行在异步环境中，可直接使用 await。支持 JavaScript 原生内置对象（JSON、Math、数组等），**禁用 Date.now()、Math.random()、无参 new Date()**（会导致任务续跑异常）；时间戳通过 args 传入，随机逻辑可基于条目索引修改代理提示/名称实现。脚本无法访问文件系统及 Node.js 原生接口。

    **优先使用 pipeline 流水线模式**。仅当必须集齐上一阶段所有结果才能继续时，才使用阶段间全局等待屏障。

    ### 等待屏障（parallel）适用场景（仅以下情况合理使用）
    阶段N需要结合上一阶段**全部条目**的全局数据：
    - 对全部结果做去重、合并，再执行后续高开销操作；
    - 结果总数为0时直接提前终止（如未发现问题，跳过后续校验）；
    - 阶段N的提示内容需要引用其他条目结果做对比分析。

    ### 等待屏障不适用场景
    - 仅需对结果做扁平化、遍历、过滤：可直接在流水线的单个阶段内处理，示例：pipeline(items, 阶段A, 结果转换函数, 阶段B)；
    - 仅逻辑上划分不同阶段：流水线模式本身就是为分阶段设计，逻辑分段不等于需要全局同步；
    - 单纯为了代码整洁：等待屏障会增加耗时，若多个任务执行速度差异较大，慢速任务会拖慢全部流程。

    简易判断规则：
    若代码写为：
      const a = await parallel(...)
      const b = transform(a)        // 扁平化、遍历、过滤，无跨条目依赖
      const c = await parallel(b.map(...))
    中间的数据转换完全不需要全局等待，改写为流水线，将转换逻辑放入阶段内即可。无法判断时，默认使用流水线。

    ### 并发限制
    单个流程中，agent() 并发数量上限为 16 与「CPU核心数-2」两者中的较小值，超出上限的任务会排队等待空闲资源。parallel、pipeline 可传入最多 4096 个条目，超出会直接报错，不会静默截断。单个流程全生命周期内，代理总数量上限为 1000，用于防止死循环导致资源耗尽。

    ### 经典多阶段示例（默认流水线，单条任务评审完成后立即校验，无等待浪费）
      export const meta = {
        name: 'review-changes',
        description: 'Review changed files across dimensions, verify each finding',
        phases: [{ title: 'Review' }, { title: 'Verify' }],
      }
      const DIMENSIONS = [{key: 'bugs', prompt: '...'}, {key: 'perf', prompt: '...'}]
      const results = await pipeline(
        DIMENSIONS,
        d => agent(d.prompt, {label: `review:${d.key}`, phase: 'Review', schema: FINDINGS_SCHEMA}),
        review => parallel(review.findings.map(f => () =>
          agent(`Adversarially verify: ${f.title}`, {label: `verify:${f.file}`, phase: 'Verify', schema: VERDICT_SCHEMA})
            .then(v => ({...f, verdict: v}))
        ))
      )
      const confirmed = results.flat().filter(Boolean).filter(f => f.verdict?.isReal)
      return { confirmed }
      // 性能维度还在评审时，漏洞维度的结果已开始校验，全程无空闲等待

    ### 必须使用全局等待屏障示例（需集齐全部结果统一去重）
      const all = await parallel(DIMENSIONS.map(d => () => agent(d.prompt, {schema: FINDINGS_SCHEMA})))
      const deduped = dedupeByFileAndLine(all.filter(Boolean).flatMap(r => r.findings))  // 必须获取全部结果再去重
      const verified = await parallel(deduped.map(f => () => agent(verifyPrompt(f), {schema: VERDICT_SCHEMA})))

    ### 循环模式示例
    1. 数量达标循环（收集指定条数结果）
      const bugs = []
      while (bugs.length < 10) {
        const result = await agent("Find bugs in this codebase.", {schema: BUGS_SCHEMA})
        bugs.push(...result.bugs)
        log(`${bugs.length}/10 found`)
      }

    2. 令牌配额循环（根据用户设置的令牌上限控制执行深度）
      const bugs = []
      while (budget.total && budget.remaining() > 50_000) {
        const result = await agent("Find bugs in this codebase.", {schema: BUGS_SCHEMA})
        bugs.push(...result.bugs)
        log(`${bugs.length} found, ${Math.round(budget.remaining()/1000)}k remaining`)
      }

    ### 组合模式示例（穷尽式评审：检索 → 去重 → 多维度评审 → 无新结果则终止）
      const seen = new Set(), confirmed = []
      let dry = 0
      while (dry < 2) {                                              // 连续两轮无新结果则终止
        const found = (await parallel(FINDERS.map(f => () =>          // 等待本轮所有检索任务完成
          agent(f.prompt, {phase: 'Find', schema: BUGS})))).filter(Boolean).flatMap(r => r.bugs)
        const fresh = found.filter(b => !seen.has(key(b)))           // 对比历史结果去重
        if (!fresh.length) { dry++; continue }
        dry = 0; fresh.forEach(b => seen.add(key(b)))
        const judged = await parallel(fresh.map(b => () =>           // 每条新发现并发评审
          parallel(['correctness','security','repro'].map(lens => () =>   // 多维度交叉校验
            agent(`Judge "${b.desc}" via the ${lens} lens — real?`, {phase: 'Verify', schema: VERDICT})))
            .then(vs => ({ b, real: vs.filter(Boolean).filter(v => v.real).length >= 2 }))))
        confirmed.push(...judged.filter(v => v.real).map(v => v.b))
      }
      return confirmed
      // 与全部历史结果去重，而非仅和已确认结果对比，避免重复处理旧问题

    ### 通用质量校验模式
    - 对抗式校验：为每条结论启动多名校验代理，指令要求**反驳结论**，超过半数反驳则剔除该结论，过滤看似合理但实际错误的内容。
        const votes = await parallel(Array.from({length: 3}, () => () =>
          agent(`Try to refute: ${claim}. Default to refuted=true if uncertain.`, {schema: VERDICT})))
        const survives = votes.filter(Boolean).filter(v => !v.refuted).length >= 2
    - 多视角校验：问题存在多种故障形态时，为校验代理分配不同检测方向（正确性、安全性、性能、可复现性），弥补单一重复校验的盲区。
    - 评审专家组：多名代理从不同角度输出方案（优先实现、优先风控、优先用户体验），评审打分后整合最优方案，并吸收备选方案亮点，适用于开放型方案设计。
    - 空结果终止循环：适用于漏洞、异常、边界场景等未知体量的检索任务，连续多轮无新结果则停止，避免遗漏尾部问题。
    - 多维度并行检索：多个代理使用不同检索策略（按目录、按内容、按实体、按时间），各司其职，单一检索方式无法覆盖全部内容时使用。
    - 完整性复检：最后启动一名代理做全局复盘，检查是否存在遗漏维度、未校验结论、未阅读资源，将发现的问题作为新一轮任务。
    - 禁止静默截断：若流程限制结果数量、禁止重试、抽样筛选，必须通过 log 说明被舍弃的内容，避免用户误以为已完成全量覆盖。

    ### 执行尺度把控
    按用户需求匹配执行强度：仅要求“查找问题”，使用少量检索代理+单次校验；要求“全面审计/深度核查”，扩充检索代理数量、采用3-5轮对抗校验并增加结果汇总环节。调研、评审、审计类任务偏向详尽执行，快速核查类任务精简流程。

    可根据任务需求自由组合各类执行逻辑（锦标赛式对比、自修复循环、分级处理等），不限于上述示例。

    本工具适用于**需要确定性控制流**（循环、条件判断、任务分发）的多步骤调度场景，区别于纯模型自主决策的执行方式。

    ## 任务续跑
    工具返回结果中包含 runId。任务暂停、异常终止或脚本修改后，可携带 `scriptPath` 和 `resumeFromRunId` 重新调用本工具实现续跑：未做修改的 agent() 调用会直接读取缓存结果，仅修改或新增的调用会重新执行。**同脚本+同入参可实现全缓存命中**。
    脚本禁用时间相关随机接口正是为了保障续跑能力。若运行日志丢失，可读取日志目录下 agent-<id>.jsonl 文件，手动编写续跑脚本。",
    "input_schema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "script": {
                "description": "完整流程脚本。脚本必须以 `export const meta = { name, description, phases }` 开头（纯字面量，不可包含计算逻辑），主体部分使用 agent()/parallel()/pipeline()/phase() 等接口编写。",
                "type": "string",
                "maxLength": 524288
            },
            "name": {
                "description": "预定义流程名称（内置流程或存放于 .claude/workflows/ 目录下的流程），调用后会自动加载对应完整脚本。",
                "type": "string"
            },
            "description": {
                "description": "该字段无效，流程描述请在脚本的 meta 配置块中定义。",
                "type": "string"
            },
            "title": {
                "description": "该字段无效，流程标题请在脚本的 meta 配置块中定义。",
                "type": "string"
            },
            "args": {
                "description": "可选入参，脚本内可通过全局变量 args 直接获取原始值。数组、对象请使用标准JSON格式传入，禁止使用字符串序列化格式（会导致脚本内数组方法报错）。主要用于带参数的通用流程（如传入调研问题）。"
            },
            "scriptPath": {
                "description": "本地流程脚本文件路径。每次调用流程工具都会将脚本保存至会话目录，并在返回结果中给出路径。迭代修改时，使用文件写入/编辑工具更新脚本，再通过本路径重新调用，无需重复传入完整脚本。该字段优先级高于 script 和 name。",
                "type": "string"
            },
            "resumeFromRunId": {
                "description": "用于续跑的历史流程运行ID。未修改的 agent() 调用会直接读取缓存，仅修改/新增调用重新执行。仅限同一会话使用，续跑前请先通过任务停止工具终止原有任务。",
                "type": "string",
                "pattern": "^wf_[a-z0-9-]{6,}$"
            }
        },
        "additionalProperties": false
    }
}
```