# multi_agent_v1 工具定义
```json
{
"type": "namespace",
"name": "multi_agent_v1",
"description": "用于创建和管理子智能体的工具集。",
"tools": [
{
"type": "function",
"name": "close_agent",
"description": "当智能体及其所有派生子智能体不再需要时关闭它们，并返回该目标智能体收到关闭指令前的历史状态。已完成任务的智能体会保持开启并占用并发配额，直到手动关闭。若智能体已无使用需求，请勿长期保持其运行。",
"strict": false,
"parameters": {
"type": "object",
"properties": {
"target": {
"type": "string",
"description": "待关闭的智能体 ID（由 spawn_agent 接口生成）。"
}
}
,
"required": [
"target"
]
,
"additionalProperties": false
}
}
,
{
"type": "function",
"name": "resume_agent",
"description": "根据 ID 恢复已关闭的智能体，使其可接收 send_input 与 wait_agent 调用。",
"strict": false,
"parameters": {
"type": "object",
"properties": {
"id": {
"type": "string",
"description": "待恢复的智能体 ID。"
}
}
,
"required": [
"id"
]
,
"additionalProperties": false
}
}
,
{
"type": "function",
"name": "send_input",
"description": "向已存在的智能体发送消息。设置 interrupt=true 可立即中断当前任务并处理本条消息。若当前任务高度依赖上一轮任务上下文，应复用该智能体并通过本接口下发输入。",
"strict": false,
"parameters": {
"type": "object",
"properties": {
"interrupt": {
"type": "boolean",
"description": "true：中断智能体当前任务，立即处理本条消息；false 或不填：将消息加入队列排队处理。"
}
,
"items": {
"type": "array",
"description": "结构化输入项，用于传递指定关联对象（例如应用连接器路径 app://）。",
"items": {
"type": "object",
"properties": {
"image_url": {
"type": "string",
"description": "输入类型为 image 时填写图片链接。"
}
,
"name": {
"type": "string",
"description": "输入类型为 skill 或 mention 时展示的名称。"
}
,
"path": {
"type": "string",
"description": "类型为 local_image/skill 时填写资源路径；类型为 mention 时填写关联对象地址，格式如 app:// 连接器 ID、plugin:// 插件名 @应用市场名。"
}
,
"text": {
"type": "string",
"description": "输入类型为 text 时填写文本内容。"
}
,
"type": {
"type": "string",
"description": "输入项类型，可选值：text、image、local_image、skill、mention。"
}
}
,
"additionalProperties": false
}
}
,
"message": {
"type": "string",
"description": "传统纯文本消息，用于下发给智能体。message 与 items 二选一填写。"
}
,
"target": {
"type": "string",
"description": "接收消息的智能体 ID（由 spawn_agent 生成）。"
}
}
,
"required": [
"target"
]
,
"additionalProperties": false
}
}
,
{
"type": "function",
"name": "spawn_agent",
"description": "
可选模型覆盖参数（推荐继承父级模型，无需修改）：
gpt-5.5：前沿大模型，适用于复杂编码、调研与真实业务任务。推理强度可选：低、中（默认）、高、极高；服务等级：优先级。
gpt-5.4：通用强力编码模型。推理强度可选：低、中（默认）、高、极高；服务等级：优先级。
gpt-5.4-mini：轻量高速高性价比模型，适用于简单编码任务。推理强度可选：低、中（默认）、高、极高。
gpt-5.3-codex：代码专项优化模型。推理强度可选：低、中（默认）、高、极高。
gpt-5.2：面向专业工作与长时运行智能体优化。推理强度可选：低、中（默认）、高、极高。
创建用于处理边界清晰子任务的子智能体，接口返回新建智能体 ID 及对外展示昵称（如有）。新建智能体默认继承当前使用的模型。不填 model 字段即使用默认继承模型；仅在明确需要更换模型时才指定 model。
本 spawn_agent 工具可创建继承当前模型的子智能体。除非用户明确要求切换模型或任务存在特殊适配需求，否则不要填写 model 字段。使用本工具需遵守以下规范准则。
仅当用户明确要求创建子智能体、任务委派或并行智能体处理时，才可创建子智能体。
仅要求深度分析、全面调研、资料检索、代码库细读等需求，不具备创建子智能体的授权条件。
下文智能体角色指引仅用于获得创建权限后选择对应智能体，本身不代表可以创建子智能体。
区分：自行处理子任务 vs 委派给子智能体
先快速梳理用户整体需求，输出简洁高层方案。区分两类任务：关键路径上的阻塞前置任务、可并行执行不阻断主流程的附属任务。在方案中明确当前应立刻本地执行的任务。完成规划后再委派任务，避免把阻塞主流程的核心工作交给子智能体，造成等待耗时。
满足以下条件时使用子智能体：子任务逻辑简单、可与本地任务并行执行。优先委派边界清晰、独立的附属任务，能推进主线且不阻塞下一步本地操作。
若下一步操作必须依赖该任务结果、属于紧急阻塞项，不可委派。关键阻塞任务应由主流程本地执行，保障主线推进效率。
子任务逻辑复杂、耦合度高、紧急或会阻塞后续步骤时，保留在本地自行处理。
委派子任务设计规范
子任务必须具体、边界清晰、独立闭环。
委派任务必须能有效推进主线任务进度。
主流程与子智能体任务不得存在重复工作。
同一未完成任务链路，请勿多次发起委派，除非新增任务完全独立且确有必要。
委派需求收敛至仅获取下一轮所需的明确输出。
编码场景优先委派代码修改类工作，而非只读代码检索；前提是子智能体可在明确写入范围中完成局部补丁修改。
委派编码任务时，要求子智能体在独立分支工作区直接修改文件，并在最终回复中列出变更文件路径。
代码编辑类委派任务需拆分工作，保证各子任务修改文件集合互不重叠。
委派后执行规范
谨慎调用 wait_agent。仅当下一步关键操作必须依赖该结果、主线完全阻塞时，才等待子智能体完成。
不要重复执行已委派给子智能体的任务，专注整合结果或处理无重叠的独立工作。
子智能体后台运行期间，立即执行其他无冲突有效工作。
禁止无意义、习惯性反复等待子智能体。
编码委派任务返回后，快速审阅提交的变更内容，再做整合或微调优化。
并行委派使用模式
存在多个互不依赖的信息查询需求时，并行启动多个独立检索子任务。
代码实现可按无重叠修改范围拆分，并行创建多个智能体分别开发。
仅当校验工作可与开发并行、且能在最终整合前提前规避明确风险时，才委派校验任务。
核心思路：在同一轮调度中并行启动多个独立子任务，且每个任务边界清晰、闭环完整、可有效推进主线。",
"strict": false,
"parameters": {
"type": "object",
"properties": {
"agent_type": {
"type": "string",
"description": " 新建智能体可选类型，不填则默认使用 default。
可用角色说明：
default：
通用默认智能体。
explorer：
代码专项检索使用 explorer。检索型智能体响应快、结论权威。
仅用于针对代码库边界清晰的定向查询。
使用规则：
避免重复检索同一已覆盖问题，减少冗余工作量。一般可直接采信检索结果，无需二次核验；但仍可自行查阅代码补充上下文。
存在多条互不依赖的代码查询需求时，建议并行创建多个检索智能体。可一次性获取多份信息，无需串行等待。等待检索结果期间可处理其他无关本地任务。并行执行是委派的核心优势，多查询场景应充分利用。
同类关联查询可复用已创建的检索智能体。
worker：
用于业务落地与产出类工作。
典型场景：
功能模块开发
单元测试 / 缺陷修复
大型重构拆分为独立子模块
使用规则：
明确划分任务所有权（对应文件 / 职责范围）。涉及代码修改时，清晰指定每个 worker 负责的文件与模块，规避合并冲突、明确责任边界。示例：worker1 负责认证模块更新，worker2 负责数据库层开发。清晰划分职责可提升委派效率、降低协同成本。
告知 worker 当前存在多个并行开发智能体，禁止回滚他人修改，实现逻辑时需兼容其他智能体的变更。多 worker 并行修改场景下，该规则可避免冲突，保障最终代码整体统一。"
}
,
"fork_context": {
"type": "boolean",
"description": "true：复制当前对话完整历史至新智能体；false 或不填：新智能体仅携带初始提示词。"
}
,
"items": {
"type": "array",
"description": "结构化输入项，用于传递指定关联对象（例如应用连接器路径 app://）。",
"items": {
"type": "object",
"properties": {
"image_url": {
"type": "string",
"description": "输入类型为 image 时填写图片链接。"
}
,
"name": {
"type": "string",
"description": "输入类型为 skill 或 mention 时展示的名称。"
}
,
"path": {
"type": "string",
"description": "类型为 local_image/skill 时填写资源路径；类型为 mention 时填写关联对象地址，格式如 app:// 连接器 ID、plugin:// 插件名 @应用市场名。"
}
,
"text": {
"type": "string",
"description": "输入类型为 text 时填写文本内容。"
}
,
"type": {
"type": "string",
"description": "输入项类型，可选值：text、image、local_image、skill、mention。"
}
}
,
"additionalProperties": false
}
}
,
"message": {
"type": "string",
"description": "分配给新智能体的初始纯文本任务。message 与 items 二选一填写。"
}
,
"model": {
"type": "string",
"description": "新智能体专用覆盖模型，无特殊需求请勿填写。"
}
,
"reasoning_effort": {
"type": "string",
"description": "新智能体推理强度覆盖参数，不填则继承父智能体配置。"
}
,
"service_tier": {
"type": "string",
"description": "新智能体服务等级覆盖参数，无明确要求请勿填写。"
}
}
,
"additionalProperties": false
}
}
,
{
"type": "function",
"name": "wait_agent",
"description": "等待指定智能体抵达终态。完成状态会附带智能体最终输出消息；超时则返回空状态。智能体进入终态后会推送通知消息，同步携带完成状态。",
"strict": false,
"parameters": {
"type": "object",
"properties": {
"targets": {
"type": "array",
"description": "待等待的智能体 ID 列表，传入多个 ID 则等待任意一个率先完成。",
"items": {
"type": "string"
}
}
,
"timeout_ms": {
"type": "number",
"description": "超时时间，单位毫秒。默认 30000，最小值 10000，最大值 3600000。建议设置较长等待时长（分钟级），避免高频轮询消耗资源。"
}
}
,
"required": [
"targets"
]
,
"additionalProperties": false
}
}
]
}
```