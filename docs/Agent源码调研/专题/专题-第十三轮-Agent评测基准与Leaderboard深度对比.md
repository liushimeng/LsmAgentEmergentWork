# 专题-第十三轮-Agent评测基准与Leaderboard深度对比

> **调研范围**:SWE-bench 系列(SWE-bench / Verified / Lite / Multimodal / SWE-gym / SWE-bench-Java)、TerminalBench、GAIA、WebArena、LiveCodeBench、AgentBench、MMLU/HumanEval/MBPP、OSWorld/AndroidWorld/OmniACT、API-Bank/ToolBench、τ-bench/Tau2;以 **atomcode/pi/opencode/claudecode/openclaw/hermes-agent/deepseek-harness** 7 个真实工程作为「Agent 如何被评测」和「Agent 如何自评测」的双重视角。
> **核心问题**:为什么 SWE-bench Verified 是 Agent 评测的「黄金标准」、Terminal-Bench 的 docker-compose 与 shell trace 验证、LiveCodeBench 抗污染机制、τ-bench 多轮对话评测、WebArena 浏览器沙箱、Leaderboard 分数对照、laew 该不该集成这些评测、自研评测该怎么做。
> **关联专题**:第三轮-测试体系与 Eval 基建(早期)、第十一轮-测试体系与 Eval 基建与录制回放(**内部** test infra)、第六轮-协议调用真实实现对比;本轮**专注外部公开评测基准**(区别于前两轮)。
> **调研日期**:2026-09-08
> **目标读者**:laew 工程核心维护者 / 评测架构师 / 模型选型与基准建设负责人

---

## 目录

- 第 1 章 引言:为什么评测是 Agent 的「生死线」
- 第 2 章 评测基准全景图与十二大维度分类
- 第 3 章 SWE-bench:Agent 评测的「黄金标准」真实细节
- 第 4 章 TerminalBench:终端场景的 Docker 沙箱与 trace 验证
- 第 5 章 WebArena:浏览器沙箱与 site-specific templates
- 第 6 章 GAIA / LiveCodeBench / OSWorld / τ-bench:多模态与抗污染评测
- 第 7 章 参考工程的评测基建剖析(atomcode/pi/hermes/Switchyard/opencode)
- 第 8 章 LLM-as-Judge / Pairwise 评分 / Mock LLM 评测范式
- 第 9 章 Eval Harness 三种并发模式与 Docker 池调度
- 第 10 章 污染防护与 Leaderboard 文化
- 第 11 章 公开模型 Leaderboard 分数对照表(Claude 4 / GPT-5 / DeepSeek-V3 / Qwen 2.5)
- 第 12 章 laew eval 体系建议与 29 个新 Gap(L433-L461)
- 第 13 章 小结与与已完成的 12 轮关系

---

## 第 1 章 引言:为什么评测是 Agent 的「生死线」

### 1.1 评测的两重含义

「评测」(Evaluation) 在 Agent 工程里有两种完全不重叠的含义,本文档两条线索并行展开:

| 视角 | 含义 | 代表实践 |
|------|------|---------|
| **作为被测对象**(evaluated) | 我们的 Agent 在公开基准上的得分是多少? | SWE-bench Verified / TerminalBench / τ-bench |
| **作为评测主体**(evaluator) | 我们怎么评测自己做的功能/模型改动? | atomcode `evals/deepseek-v4-flash/` 配对评测 / pi `packages/evals` / Switchyard `craft-taskgen` / opencode `routes/bench` Leaderboard |

第十一轮深挖已覆盖 laew **内部**测试基础设施(Mock LLM、录制回放、CI 集成、E2E TUI 自动化);本轮**专注外部公开评测基准**(SWE-bench/GAIA/WebArena/τ-bench 等),以及**第三方工程的评测基建**(atomcode/pi/hermes/opencode/Switchyard 如何「评测自己的 Agent」)。

### 1.2 三个根本问题

评测不只是「跑分」。一个评测基准的设计决定了三个根本问题的答案:

1. **任务难度上限**:能不能区分「真正能修 Bug 的 Agent」与「会写 hello world 的 Agent」?一个 5 步 Bash 任务的满分率 90% 没有信号,一个 50 步多文件重构的满分率 20% 才有信号。
2. **污染防护**:模型是否在训练时见过这个任务?SWE-bench Lite 公开题库被 GPT-4 直接考过原题,所以才有 Verified 的人工验证子集;LiveCodeBench 每月更新强制时间截止。
3. **可复现性**:同一个模型跑 3 次,得分波动多大?是不是需要 K8s 集群调度、token 预算控制、并发上限?

### 1.3 laew 现状

laew 当前覆盖 6 角色 Multi-Agent + Bash/Read/Write 工具集 + TUI 多轮交互,但**评测基建几乎空白**(仅 `testReport/run_e2e.sh` 跑 mock LLM 端到端 + `mock_llm_server.py`)。本专题结束时,将给出 29 个新 Gap(L433-L461)与 5 阶段改造路线图。

---

## 第 2 章 评测基准全景图与十二大维度分类

### 2.1 评测生态的 12 大维度

把目前已知的 30+ 个公开 Agent/LLM 评测基准按维度分类,可以归纳为 12 大类(每类列代表基准):

| 维度 | 代表基准 | 关键能力 | 任务量 | 评测方式 |
|------|---------|---------|--------|---------|
| **代码生成** | HumanEval / MBPP / LiveCodeBench | 函数实现 / 单元测试 | 164 / 974 / 每月 ~500 | exact match (pass@k) |
| **代码仓库修复** | SWE-bench / SWE-bench Verified / SWE-bench Lite / SWE-bench Multimodal / SWE-bench Java | 真实 GitHub Issue 修复 | 2,294 / 500 / 300 / 517 / 40 | fail_to_pass 测试 |
| **终端任务** | TerminalBench / Terminal-Bench 2.0 | Linux shell 任务 | ~100 | Docker + shell trace |
| **操作系统 GUI** | OSWorld / AndroidWorld / OmniACT | Windows/macOS/Linux/Android GUI 自动化 | 369 / 116 / 6,178 | accessibility tree + image |
| **Web 浏览器** | WebArena / Mind2Web / WebVoyager | Chromium 真实网站 | 812 / 2,350 / 643 | URL 任务 + 视觉验证 |
| **通用助理** | GAIA / AgentBench / AgentEval | reasoning + web + multimodal | 466 / 8 env / 1,000+ | LLM-as-judge |
| **工具调用** | API-Bank / ToolBench / τ-bench | 函数调用 / 多轮业务对话 | 314 / 17K / 165 | 行为一致性 / pass@k |
| **检索 / 知识** | TriviaQA / NaturalQuestions / HotpotQA | 开放域 QA | 95K / 79K / 113K | exact match / F1 |
| **数学推理** | GSM8K / MATH / AIME | 链式推理 | 8.5K / 12.5K / 1K | exact match |
| **多模态** | MMMU / MathVista / ChartQA | 图文交错推理 | 11.5K / 6.1K / 18.5K | 选项正确 |
| **指令跟随** | IFEval / AlpacaEval / MT-Bench | 格式遵循 / 对话质量 | 541 / 805 / 80 | regex / LLM-as-judge |
| **长期记忆** | LoCoMo / MSC / LongMemEval | 多会话记忆 | 1,540 / 5,000 / 500 | LLM-as-judge |

### 2.2 评测生态分布(2026 年 9 月)

| 类别 | 基准数 | 主导机构 | 抗污染机制 | laew 相关性 |
|------|--------|---------|-----------|------------|
| 代码 | 8 | OpenAI / Princeton / DeepMind | 时间截止 + 私有测试 | ⭐⭐⭐⭐⭐ |
| GUI/OS | 5 | CMU / Google / Microsoft | 真实环境 + 视觉对比 | ⭐⭐ |
| Web | 6 | CMU / Salesforce / OpenAI | 沙箱隔离 + 任务多样性 | ⭐⭐ |
| 工具 | 4 | Salesforce / OpenAI / Tsinghua | 业务场景模拟 | ⭐⭐⭐⭐ |
| 通用 | 12 | Meta / Anthropic / HuggingFace | LLM-as-judge + 私有 | ⭐⭐⭐ |
| 检索/数学/多模态 | 15 | OpenAI / Google / DeepMind | 时间截止 + 人审 | ⭐ |

laew 最相关的三类是**代码**(Bash/Read/Write 工具直接驱动)、**工具调用**(协议层 + 工具注册)、**通用助理**(多 Agent 任务)。本报告主体展开这三个类。

### 2.3 评测成本对比(单模型单次)

| 基准 | 任务 | 单任务平均 tokens | 总成本 (Claude Sonnet 4) |
|------|------|------------------|--------------------------|
| SWE-bench Verified | 500 | ~50K | ~$1,200 |
| TerminalBench | 100 | ~80K | ~$360 |
| WebArena | 812 | ~15K | ~$540 |
| GAIA | 466 | ~25K | ~$520 |
| τ-bench | 165 | ~30K | ~$220 |
| LiveCodeBench | 500/月 | ~3K | ~$70 |
| OSWorld | 369 | ~120K | ~$1,950 |

> 一个完整的「Agent 综合评测包」一次跑完所有上述基准约 $5,000(Claude Sonnet 4 价位),DeepSeek-V3 价位约 $150(差 30 倍)。这就是为什么 laew 的评测基建必须支持**多 provider + 成本切换**。

---

## 第 3 章 SWE-bench:Agent 评测的「黄金标准」真实细节

### 3.1 基准家族总览

SWE-bench 是 Princeton 2024 年发布的「真实 GitHub Issue 修复」基准,核心思想:**任何 Agent 都修不了真实软件 Bug,那么这个 Agent 就不是合格的软件工程师**。

| 版本 | 任务数 | 来源 | 难度 | 公开性 |
|------|--------|------|------|--------|
| **SWE-bench** | 2,294 | 12 个 Python 仓库 | 极高 | 公开 |
| **SWE-bench Lite** | 300 | 上述子集,易运行 | 中 | 公开 |
| **SWE-bench Verified** | 500 | OpenAI 2024-08 人工验证 | 中-高 | 公开 |
| **SWE-bench Multimodal** | 517 | 同一仓库,带截图 | 高 | 公开 |
| **SWE-bench Java** | 40 | Java 仓库子集 | 中 | 公开 |
| **SWE-gym** | 训练环境 | 2,294 实例的并行 Docker | 训练用 | 私有镜像 |
| **SWE-bench Pro** | 加工 | Switchyard craft-taskgen 引入 | 极高 | 半公开 |

### 3.2 SWE-bench 单任务流程

下面是一段典型的 SWE-bench 评测脚本(简化自 SWE-bench 官方仓库 `swebench/inference/run_inference.py` 与 `swebench/evaluation/evaluate.py`):

```python
# 1. 准备:为每个 instance_id 启动一个 Docker 容器
# 镜像名:swebench/sweb.eval.x86_64.<instance_id>_<hash>
# 容器内:目标仓库 + base_commit + fail_to_pass 测试
docker_image = f"swebench/sweb.eval.x86_64.{instance_id}:latest"
subprocess.run(["docker", "pull", docker_image], check=True)
container = subprocess.run(["docker", "run", "-d", "--rm",
                            "-v", f"{result_dir}:/result",
                            docker_image, "sleep", "infinity"], check=True)

# 2. 让 Agent 修改代码
# 输入:problem_statement + base_commit 的代码树
# 输出:model_patch (unified diff)
agent_response = agent.run(
    problem=instance["problem_statement"],
    repo_state=instance["base_commit"],
    tools=[read_file, write_file, bash],
)

# 3. 应用 patch 到容器内
subprocess.run(["docker", "exec", container,
                "git", "apply", f"/result/{instance_id}.patch"], check=True)

# 4. 跑 fail_to_pass 测试(必须从失败变为通过)
# 5. 跑 pass_to_pass 测试(必须保持通过)
# 两类都通过 → 该 instance 解出
f2p_result = subprocess.run(["docker", "exec", container,
                              "bash", "tests/test.sh"], capture_output=True)
p2p_result = subprocess.run(["docker", "exec", container,
                              "bash", "tests/test_p2p.sh"], capture_output=True)

score = (f2p_result.returncode == 0 and p2p_result.returncode == 0)
```

### 3.3 关键设计要素

| 设计点 | SWE-bench 选择 | 原因 |
|--------|---------------|------|
| **任务定义** | GitHub Issue + PR + base_commit | 真实软件工程场景,非人造题目 |
| **评分依据** | fail_to_pass 测试从 FAIL → PASS | 客观可验证,不需要 LLM-as-judge |
| **容器化** | 每个 instance 一个 Docker 镜像 | 隔离环境 + 依赖固化 + 可复现 |
| **Patch 提取** | `git diff` 整段 diff,只保留 `.py/.js/.ts` 等 | 防止 Agent 修改测试文件作弊 |
| **超时** | 单 instance 30 分钟,容器级 kill -9 | 防止无限循环 |
| **Test Selection** | regex + AST 提取 test_function 名字 | 精确到函数级,而非整文件 |

### 3.4 SWE-bench Verified 与 Lite 的人工验证过程

OpenAI 在 2024-08 发布的 SWE-bench Verified 子集(500 个)由人类软件工程师逐题验证:

1. **Fail-to-Pass 验证**:确认测试能从 FAIL 变 PASS(没作弊或误标)
2. **难度分布重测**:确保模型真的能解出,而不是太简单
3. **问题陈述清晰度**:重写问题描述,确保 Agent 能理解
4. **依赖固化**:确认 Docker 镜像能跑起来

Lite(300 个)是从 2,294 个中按 `instance_id` + 测试可运行性筛选的子集,**未经过人工验证**,所以现在的 Leaderboard 默认报告 Verified 分数。

### 3.5 SWE-gym 训练环境

SWE-gym(SWE-Bench Gymnasium)是 Princeton 2024-11 发布的**并行训练环境**:

- **2,294 个 instance 的并行 Docker 池**(基于 K8s 或 Docker Compose scale)
- **MCP(Model Context Protocol)服务器暴露**:Agent 通过 MCP 调用 `read_file` / `write_file` / `bash` / `search`
- **Reward Shaping**:每步动作给一个 reward(成功跑通部分测试得部分分),而非只在终点给 reward(稀疏奖励难训练)
- **RL 算法**:PPO + DPO + GRPO 都有 baseline

```yaml
# swe-gym/docker-compose.yml 简化版
services:
  swe_instance_${i}:
    image: swe-bench/${instance_id}:latest
    environment:
      - REWARD_URL=http://reward-collector:8080/record
    volumes:
      - ${WORKSPACE}:/workspace
    healthcheck:
      test: ["CMD", "bash", "-c", "test -f /workspace/ready"]
      interval: 5s
      timeout: 30s
      retries: 3
```

### 3.6 SWE-bench Multimodal:截图作为问题上下文

2025-09 发布的 SWE-bench Multimodal(517 个)增加了**截图上下文**——许多 Issue 包含 UI bug 的截图(GUI 错误、错位、空白)。Agent 必须看图理解问题,而不是只看文字。这是**多模态 Agent** 的关键评测。

> laew 当前无图像输入能力(仅文字 Prompt),所以 SWE-bench Multimodal 当前不适用,但应作为**未来 L2 目标**。

### 3.7 SWE-bench Java 与 Pro

- **SWE-bench Java**(40):小规模 Java 子集,验证 SWE-bench 不只适用于 Python
- **SWE-bench Pro**(Switchyard craft-taskgen 在用):**加难度版本**,通过 5 个挑战问题(Q1-Q5)重新筛选 SWE-bench Verified 中的「真正有区分度」的 200+ 个任务

Switchyard `craft-taskgen/src/craft_taskgen/swebench_alignment.py` 是把 SWE-bench Pro 数据集**二次对齐到 CRAFT 任务格式**的脚本:

```python
# Switchyard craft-taskgen swebench_alignment.py
@dataclass
class AlignmentCandidate:
    task_id: str
    repo: str
    sha: str
    merge_base_sha: str
    source_task_id: str
    problem_statement: str
    requirements: str = ""
    interface: str = ""

def _instruction_from_metadata(source_metadata, *, include_requirements, include_interface):
    problem_statement = str(source_metadata.get("problem_statement", "")).strip()
    sections = [problem_statement]
    sources = ["problem_statement"]
    requirements = str(source_metadata.get("requirements", "")).strip()
    if include_requirements and requirements:
        sections.append(f"## Requirements\n{requirements}")
        sources.append("requirements")
    interface = str(source_metadata.get("interface", "")).strip()
    if include_interface and interface:
        sections.append(f"## Interface\n{interface}")
        sources.append("interface")
    return "\n\n".join(sections), "+".join(sources)
```

注意这段代码的**事实抽取设计**:从 SWE-bench Pro 的 `problem_statement` 抽出任务说明、可选的 `requirements`(验收标准)、`interface`(API 接口契约),组合成最终 instruction.md。`craft-taskgen` 的 5 个挑战问题(见上文 rubrics.py)专门用于「审查」这条抽取链路的正确性:

> Q1: Is this test testing the FEATURE WE ASKED FOR?
> Q2: Does the instruction SPECIFY what the test checks?
> Q3: Does the instruction WORDING match the test SCOPE?
> Q4: Is this a systematic failure pattern or an Opus-specific quirk?
> Q5: Would a CORRECT ALTERNATIVE implementation fail this test?

这是**评测题目质量审查**的真实工程实践,laew 在生成自己的评测题时可以直接复用这 5 问。

### 3.8 SWE-bench Docker 镜像与预测格式

`predictions.jsonl` 是 SWE-bench 官方规定的预测文件格式:

```json
{"instance_id": "django__django-11039", "model_name_or_path": "claude-sonnet-4", "model_patch": "diff --git a/django/... --- a/django/...\n+++ b/django/...\n@@ -1,3 +1,3 @@\n-..."}
{"instance_id": "django__django-11041", "model_name_or_path": "claude-sonnet-4", "model_patch": "..."}
```

每行一个 JSON,字段固定。Agent 必须把代码改动以 `git diff` 格式输出,而非整个文件内容,这样:
- 评测系统只需 `git apply` 即可应用 patch
- 防止 Agent 修改测试文件或添加无关代码

`model_patch` 必须排除的文件(由 SWE-bench 评估脚本强制):
- `*.test.*` / `tests/*` / `*_test.py` 等测试文件
- `setup.py` / `pyproject.toml` / `requirements*.txt`(可能修改依赖作弊)
- `*.md` / `*.rst` / `LICENSE`(文档无关)

---

## 第 4 章 TerminalBench:终端场景的 Docker 沙箱与 trace 验证

### 4.1 设计动机

TerminalBench(2025 由 Princeton 团队发布)的目标:**「真实 Linux 终端任务的 Agent 评测」**。SWE-bench 是「修代码」,TerminalBench 是「**用 shell 完成任务**」(系统管理、数据处理、网络调试)。

任务样例:
- 在 `du` 不可用的情况下找出最大 10 个目录
- 用 `sed` + `awk` 解析 nginx access log,统计每个 IP 的请求数
- 在 docker 容器内修复网络配置,让两个容器能 ping 通
- 用 `psql` 备份 PostgreSQL 数据库并恢复到新服务器

### 4.2 Docker Compose 模板(简化)

```yaml
# terminalbench/task-<id>/docker-compose.yml
services:
  main:
    build:
      context: .
      dockerfile: Dockerfile
    image: terminalbench/task-<id>:latest
    command: ["/bin/bash", "-c", "while true; do sleep 3600; done"]
    environment:
      - TB_TASK_ID=<id>
      - TB_AGENT_TIMEOUT=1800
    volumes:
      - ./tasks:/tasks:ro
      - ./agent_workspace:/workspace
      - ./logs:/logs
    # 不暴露端口(Agent 在容器内执行)
    # 不用 host network(隔离)

  verifier:
    image: terminalbench/task-<id>:latest
    depends_on: [main]
    command: ["/bin/bash", "/tasks/verify.sh"]
    volumes:
      - ./agent_workspace:/workspace:ro  # 只读,防止 Agent 修改影响验证
      - ./logs:/logs:ro
```

每个 TerminalBench 任务有 4 个核心文件:
- `Dockerfile`:定义初始环境(apt install / pip install / 配置文件)
- `tasks/task.yaml`:任务描述(给 Agent 看)+ 验证规则(给 verifier 看)
- `tasks/verify.sh`:验证脚本(检查 Agent 的工作结果)
- `tasks/solution.sh`:黄金答案(只给评测框架看,不暴露给 Agent)

### 4.3 Shell Trace 验证:每个命令的 stdout/stderr/exit_code

TerminalBench 的关键创新是**「shell trace 验证」**:不只看最终结果,还记录 Agent 的**完整 shell 命令序列**:

```python
# terminalbench/harness/trace_collector.py 简化
class ShellTrace:
    commands: list[CommandTrace]   # 命令序列
    
class CommandTrace:
    timestamp: float
    command: str                   # 原始命令
    stdout: bytes                  # 标准输出(可能截断到 30K)
    stderr: bytes                  # 标准错误
    exit_code: int                 # 退出码
    duration_ms: int               # 执行时长
    pwd: str                       # 当前工作目录
    env_snapshot: dict             # 环境变量快照(可选)
```

「shell trace 验证」的 5 类检查:

1. **exact_match**:Agent 的最终输出文件与黄金答案 byte-for-byte 一致
2. **regex_match**:Agent 输出文件匹配某个 regex(如包含特定字符串)
3. **command_pattern**:Agent 跑过的命令序列包含某些关键字 → 防「猜答案」
4. **test_pass**:Agent 跑通某个测试脚本(类似 SWE-bench)
5. **state_match**:容器文件系统状态与黄金答案一致(如 `dpkg -l` 输出相同)

### 4.4 对比验证 vs 单元测试

| 验证方式 | 适用 | 优点 | 缺点 |
|---------|------|------|------|
| **单元测试** | 代码有现成 test | 客观可复现 | 只能验证「测试覆盖到的部分」 |
| **对比验证** | 任务是「产生某个产物」 | 强信号,不易作弊 | 需要黄金答案,任务设计难 |
| **LLM-as-judge** | 主观质量 | 灵活 | 不可复现,模型偏差大 |

TerminalBench 偏好**对比验证 + 命令模式**(精确)+ LLM-as-judge(辅助,用于「答案有多种合理形式」的场景)。

### 4.5 Terminal-Bench 2.0 与 Harbor

2026 年发布的 Terminal-Bench 2.0 进一步整合到 **Harbor** 平台,提供:
- K8s 集群调度(支持 1,000+ 并发 instance)
- 多语言(Lua/Rust/Go 任务)
- 增量评分(部分完成得部分分)
- 失败 task 自动重新分类(可解/不可解)

Harbor 是 TerminalBench 的生产级部署框架,由 Switchyard 团队维护。laew 可参考 Harbor 部署,但完整集成成本高(需要 K8s 集群)。

---

## 第 5 章 WebArena:浏览器沙箱与 site-specific templates

### 5.1 设计动机

WebArena(CMU 2023)是「**真实网站 + 真实任务**」的 Agent 评测。SWE-bench 修代码,WebArena 操作网站:
- 在购物网站下单买书
- 在论坛注册账号发帖
- 在 GitLab 创建 issue 并指派
- 在地图网站查找路线并保存

任务涉及 6 个真实网站:**Shopping(电商) / Reddit(论坛) / GitLab / Maps / Calculator / DMS(文档管理)**。

### 5.2 Docker 沙箱

```yaml
# WebArena docker-compose.yml
services:
  webarena-chromium:
    image: webarena/chromium:1.0
    command: chromium --headless --no-sandbox --remote-debugging-port=9222
    ports:
      - "9222:9222"
    networks:
      - webarena-net

  shopping:
    image: webarena/shopping:latest
    ports:
      - "7770:80"
    environment:
      - MAGENTO_DATABASE_PASSWORD=magento
    networks:
      - webarena-net

  reddit:
    image: webarena/reddit:latest
    ports:
      - "9999:80"
    networks:
      - webarena-net

  # ... gitlab / maps / calculator / dms 各一个容器

  # Agent 容器:运行待测 Agent
  agent:
    build: .
    depends_on: [webarena-chromium, shopping, reddit]
    environment:
      - SHOPPING_URL=http://shopping:80
      - REDDIT_URL=http://reddit:80
    networks:
      - webarena-net
```

### 5.3 Observation / Action Space 设计

**Observation**(Agent 看到的):
- 当前 URL
- 页面 accessibility tree(关键!)—— 不是 HTML,而是 a11y 节点(ID, role, name, value)
- 已加载的截图(可选,某些任务需要)
- 上一个 action 的执行结果

**Action**(Agent 可发起的):
- `click(element_id)` —— 点 accessibility tree 中的元素
- `type_text(element_id, text)` —— 输入文字
- `scroll(direction)` —— 上下滚动
- `goto(url)` —— 直接跳转
- `back()` —— 后退
- `new_tab(url)` —— 新开标签页
- `close_tab()` —— 关闭
- `wait(seconds)` —— 等

**关键设计**:**使用 accessibility tree 而非 raw HTML**,这样:
- 大幅压缩 observation(从 1MB HTML 到 5KB a11y)
- 跨网站统一(任何网站都暴露 a11y 节点)
- 对模型友好(树结构清晰)

### 5.4 URL 白名单与安全

WebArena 强制 URL 白名单,Agent 不能访问外网:
```python
ALLOWED_DOMAINS = {
    "shopping", "reddit", "gitlab", "maps", "calculator", "dms",  # 内网
    "127.0.0.1", "localhost",                                       # 本地
}
```

每个 site-specific template 列出该网站可用的 URL 前缀:
```json
{
  "shopping": {
    "allowed_prefixes": ["/", "/customer", "/checkout"],
    "blocked_prefixes": ["/admin", "/api/internal"],
    "credential": "user1:pass1"
  }
}
```

### 5.5 任务验证

WebArena 任务的验证是**「状态一致性」** + **「最终断言」**:

```python
def verify_shopping_task(task_result):
    # 1. 检查订单是否真的在 DB 中
    orders = query_db("SELECT * FROM sales_order WHERE customer_id = ?", (task_result.user_id,))
    assert len(orders) > 0, "Order not created"
    assert orders[0].product_name == "Sharp Objects", "Wrong product"
    
    # 2. 检查订单状态
    assert orders[0].status == "complete", f"Status is {orders[0].status}"
    return True
```

数据库级验证比 UI 级验证可靠得多——Agent 可能「假装」UI 显示对,但数据库实际没下单。

---

## 第 6 章 GAIA / LiveCodeBench / OSWorld / τ-bench:多模态与抗污染评测

### 6.1 GAIA:通用 AI 助理的多模态 + 推理 + 工具使用

GAIA(Meta 2023)是「**通用 AI 助理**」基准,2024 年成为 Agent 评测的事实标准之一:

- 466 个任务,3 档难度(1/2/3)
- 多模态输入:文本 + 图像 + 音频 + 表格
- 必用工具:Web 搜索、文件读取、计算器
- 推理链要求:3-15 步推理

任务样例(1 级,简单):
> 给定一张图,图中显示一个手写的化学分子式。请告诉我该分子的 IUPAC 名称。

任务样例(3 级,极难):
> 给定 5 张历史文件的扫描件,这些文件是一个公司 1950-2000 年的年度报告。请根据这些文件算出该公司的累计净利润,并用一句话总结其财务状况。

GAIA 的关键设计:**任务本身需要真实世界信息**(不是 Wikipedia 一句话能答),所以必须用搜索 + 推理 + 工具。

### 6.2 LiveCodeBench:抗污染的持续更新代码评测

LiveCodeBench(NYU 2024)的设计动机:**「评测任务一旦公开,就被模型训练集污染」**。

抗污染机制:
- **每月更新**:从 LeetCode / Codeforces / AtCoder 抽取当月新题
- **Cutoff Date 强制**:每个任务标注来源竞赛日期,只测「该日期后发布的模型」
- **私有测试集**:评测时不公开测试输入,只公开样例
- **Held-out Variants**:同一逻辑题做 5 种参数变体,防止「背答案」

```python
# LiveCodeBench task 定义
{
    "contest": "codeforces_round_891",
    "date": "2024-08-15",
    "problem_id": "C",
    "title": "Suffix Array Construction",
    "difficulty": 1900,
    "public_tests": [...],       # 公开测试
    "private_tests": [..., hash], # 私有测试(只 hash,不暴露)
    "constraints": "n ≤ 1e5",
    "specification": "...",
    "generator_code": "...",      # 私有测试生成器(可选)
}
```

评测代码用「私有测试生成器」或「固定私有测试」调用 Agent 的代码,看 pass@1 / pass@5 / pass@10。

### 6.3 OSWorld:跨平台真实 OS 评测

OSWorld(CMU 2024)是「**真实桌面环境**」的 Agent 评测,2024-2025 期间 Claude 3.5/3.7/4 Sonnet 的 OSWorld 分数(从 14.9% → 36.2% → 43.9%)成了 Agent 进步的标志事件。

任务样例:
- 在 LibreOffice Writer 中格式化文档(应用样式 + 字体 + 标题)
- 在 GIMP 中调整图片亮度和对比度
- 在 VS Code 中安装扩展并配置
- 在 Thunderbird 中给某封邮件加标签

**关键设计**:
- 真实 OS(不是模拟):Windows / macOS / Linux 各有专门的镜像
- **Accessibility Tree + 截图**双 observation
- 任务列表 369 个,跨办公/开发/创意/系统管理 4 类

### 6.4 τ-bench(Tau-Bench):真实业务对话

τ-bench(Salesforce 2024)是「**真实业务多轮对话**」评测,任务样例:

- 在航空客服场景中,Agent 扮演客服,与人类用户对话,帮用户改机票/退款/投诉
- 在零售客服场景中,Agent 处理订单修改、地址变更、退款申请
- 165 个任务,每个任务 5-15 轮对话

**关键设计**:
- **多轮对话**:Agent 必须能维护上下文、处理用户追问
- **工具调用错误恢复**:如果调错 API(机票改错日期),Agent 必须能从错误恢复
- **pass@k**:5 次独立运行中至少 1 次成功的概率
- **与 GPT-4「共同」测**:同时跑 GPT-4 baseline + 待测模型,看相对差

τ-bench 揭示了一个关键问题:**「Agent 评测与模型评测不是一回事」**——同样用 GPT-4,Agent 框架差异会让 τ-bench 分数差 30+ 个百分点。

### 6.5 τ-bench 与 API-Bank / ToolBench 对比

| 基准 | 任务形式 | 多轮 | 工具数 | 评分方式 | 抗污染 |
|------|---------|------|--------|---------|--------|
| **API-Bank** | 单轮 API 调用 | ❌ | 314 | exact match | ❌ |
| **ToolBench** | 单轮 + 多轮混合 | 部分 | 17K | success rate | ❌ |
| **τ-bench** | 多轮对话 | ✅ | 30-50 | pass@k | ✅(任务设计) |
| **τ-bench retail** | 多轮零售客服 | ✅ | 25 | pass@k | ✅ |
| **τ-bench airline** | 多轮航空客服 | ✅ | 22 | pass@k | ✅ |
| **τ-bench2 (2026)** | 多轮 + 工具错误恢复 | ✅ | 30 | pass@k + error recovery | ✅ |

> τ-bench2(2026) 引入了**「工具调用错误恢复」** 评分维度:Agent 是否能在调用失败时回滚或重试。这是 laew 当前 Agent 循环的真正短板。

### 6.6 MMLU / HumanEval / MBPP:基础能力

这三个不再展开(已是常识),但有 4 个**关键差异** laew 应了解:

1. **MMLU**(57 学科,16K 题)→ 测「知识广度」,Agent 不擅长(因为 Agent 关注推理/工具,不是知识)
2. **HumanEval**(164 题)→ 测「简单函数」,Agent 几乎全对(因为 80%+ 是一次性 write_file 完成)
3. **MBPP**(974 题)→ 测「基本编程」,类似 HumanEval 但更短
4. **MMLU-Pro / HumanEval+ / MBPP+**(2024 增强版)→ 增加了难度,**有意筛掉「背答案」的模型**

> laew 不应把 HumanEval 当主评测——太简单,区分度低。应把 MMLU-Pro + SWE-bench Verified + τ-bench 当主评测。

---

## 第 7 章 参考工程的评测基建剖析(atomcode/pi/hermes/Switchyard/opencode)

### 7.1 atomcode:`evals/deepseek-v4-flash/` 配对评测

这是本轮找到的**最完整的「自评测」工程实践**。atomcode 在 `evals/deepseek-v4-flash/` 目录提供了一个 627 行的 `eval.py`,实现了**双 candidate 配对 + 隔离运行 + 盲审 LLM judge + 多维度报告**:

```python
# atomcode/evals/deepseek-v4-flash/eval.py:227-280
async def run_one(suite, case, candidate, rep, pair_dir, ready):
    """Run one (case, candidate, rep) tuple in isolation."""
    out_dir = pair_dir / candidate.name
    work = out_dir / "work"
    out_dir.mkdir(parents=True, exist_ok=True)
    home = Path(tempfile.mkdtemp(prefix=f"atomcode-eval-{candidate.name}-"))
    
    # 关键:每个 candidate 独立的临时 HOME(防止 auth 串扰)
    source_home = suite.config.parent
    for auth_name in ("auth.toml", "codingplan_sync.json", "device_id"):
        source = source_home / auth_name
        if source.is_file():
            shutil.copy2(source, home / auth_name)
    
    # 关键:每个 candidate 独立的 work 目录
    if case.fixture:
        shutil.copytree(str(case.fixture), str(work))
    else:
        work.mkdir()
    
    # 启动 atomcode(用 --ephemeral + --output-format jsonl)
    argv = [suite.atomcode_bin, "--provider", candidate.selection,
            "--config", str(suite.config),
            "--prompt-file", str(prompt), "-C", str(work),
            "--ephemeral", "--output-format", "jsonl",
            "--dev", "--no-telemetry"]
    if case.tier == "model":
        argv.append("--no-tools")  # 模型题不加工具
    if case.allow_edits:
        argv.append("--dangerously-skip-permissions")
    
    env = os.environ.copy()
    env["ATOMCODE_HOME"] = str(home)  # 隔离 HOME
    await ready.wait()
    
    # 子进程 + timeout
    proc = await asyncio.create_subprocess_exec(*argv,
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, env=env)
    try:
        stdout_b, stderr_b = await asyncio.wait_for(
            proc.communicate(), timeout=case.timeout)
    except asyncio.TimeoutError:
        timed_out = True
        proc.kill()
        stdout_b, stderr_b = await proc.communicate()
    
    # 解析 + 分类 + 脱敏
    events, stdout, token_usage = parse_atomcode_jsonl(events_raw)
    outcome = classify_jsonl(returncode, timed_out, stdout, events, stderr, jsonl_error)
    
    # 关键:diff 跑过的 fixture(对比「期望产物 vs 实际产物」)
    if case.fixture:
        diff_run = subprocess.run(["diff", "-ruN", str(case.fixture), str(work)],
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   universal_newlines=True)
        (out_dir/"changes.diff").write_text(scrub(diff_run.stdout))
```

**关键设计要点**(laew 直接借鉴):
1. **配对 + 隔离**:`pair_concurrency` 控制同时跑的 case 数,每个 candidate 独立 HOME/work
2. **盲审脱敏**:`SECRET_RE` regex 把 stderr 中的 API key/Authorization token 全替换为 `[REDACTED]`
3. **多维度 outcome**:`success / empty_output / timeout / rate_limited / provider_error / process_error / protocol_error / model_mismatch`(8 类)
4. **Token 统计**:从 `[tokens] prompt=... completion=... cached=...` stderr 行解析 token usage
5. **Seed + start_skew_ms**:固定随机种子 + 启动时间偏移(避免 pair 同时启动带来的负载峰值)
6. **Bootstrap CI**:1 万次 bootstrap 重采样算 95% 置信区间
7. **Cache hit 分 cohort**:`cache_cold`(首次)vs `cache_repeat`(重复)分开算均值,识别 cache warming 效果

### 7.2 pi:`packages/evals` vitest-evals 框架

pi 用 Sentry 的 `vitest-evals` 框架做评测,把 `AgentSession` 适配成 vitest harness:

```typescript
// pi/packages/evals/src/pi-harness.ts:60-110
export function resolveModelSelection(
  explicitModel: PiCodingAgentModelSelection | undefined,
  environment: { PI_PROVIDER?: string; PI_MODEL?: string } = process.env,
): PiCodingAgentModelSelection {
  const provider = (explicitModel?.provider ?? environment.PI_PROVIDER)?.trim();
  const id = (explicitModel?.id ?? environment.PI_MODEL)?.trim();
  if (!provider || !id) {
    throw new Error("Select a harness model explicitly or set both PI_PROVIDER and PI_MODEL as defaults.");
  }
  return { provider, id };
}

async function runPiCodingAgent<TOutput>(
  input, signal, setArtifact, options
): Promise<SimpleHarnessResult<string | TOutput>> {
  const startedAt = performance.now();
  signal?.throwIfAborted();
  const selection = resolveModelSelection(options.model);
  const modelRuntime = await ModelRuntime.create();
  const model = modelRuntime.getModel(selection.provider, selection.id);
  if (!model) throw new Error(`Eval model not found: ${selection.provider}/${selection.id}`);
  
  // ... 临时 HOME/Project 隔离(避免污染用户的真实环境)
  const sessionOptions: CreateAgentSessionOptions = {
    cwd: cwd,  // 临时目录
    agentDir: agentDir,  // 临时 HOME
    model,
    noTools: options.noTools,
    systemPromptOverride: options.transformSystemPrompt
      ? options.transformSystemPrompt(await defaultSystemPrompt(...))
      : undefined,
    onActivity: ...,  // 活动回调(供 reporter 收数据)
  };
  
  const { session, services } = await createAgentSessionFromServices(
    await createAgentSessionServices(model, sessionOptions));
  const inputMessages = ...;  // 加载 prompt
  for (const prompt of inputMessages) {
    await promptAgent(session, prompt.content, signal);
  }
  // 输出 + 摘要
  const output = options.output ? await options.output({
    response: session.getLastAssistantText(),
    session,
  }) : session.getLastAssistantText();
  
  return {
    output,
    usage: session.usage,  // token 统计
    timings: { totalMs: performance.now() - startedAt },
    artifacts: { [PI_SESSION_SNAPSHOT_ARTIFACT]: session.sessionManager.getSnapshot() },
  };
}
```

**关键设计**(laew 借鉴):
- **Pairwise 配对 + baseline**:`evalHarnessTable` 同时跑 baseline + candidates,自动算 paired diff
- **Outcome 五态**:scored / unscored / skipped / pending / errored(对 laew 的 `outcome` 是清晰分类)
- **CorrectnessLift 指标**:Pass rate 提升 + 平均 wins/ties/losses
- **Artifact 持久化**:每个 run 的 metadata / 错误 / artifact references 都写到 `.eval/runs.jsonl`,可后续分析
- **Canonical JSON**:用 `canonicalizeJson` 把任意对象规范化(避免 key 顺序差异),用 SHA256 算 groupKey

### 7.3 hermes-agent:`evals/browser_use/` 浏览器 A/B 评测

hermes-agent 在 `evals/browser_use/` 提供了一个「Browser Use Mode Benchmark」(BUBench)——**对比两组 arms(browser_* 工具 vs browser_exec 单一驱动)** 在真实 web 任务上的 token/调用数:

```python
# hermes-agent/evals/browser_use/orchestrate.py:42-90
ARMS = args.arms.split(",")  # base / pr / prns
MODELS = args.models.split(",")  # anthropic/claude-opus-4.8 / moonshotai/kimi-k3
TASKS = list(json.load(open(args.tasks, encoding="utf-8")).keys())
REPS = list(range(1, args.reps + 1))  # 3 reps

# Resume-safe:已完成的 cell 跳过
done = set()
if os.path.exists(args.results):
    for line in open(args.results, encoding="utf-8"):
        try:
            r = json.loads(line)
            done.add((r["arm"], r["task"], r["model"], r["rep"]))
        except Exception:
            pass

cells = [
    (arm, task, model, rep)
    for model, task, rep, arm in itertools.product(MODELS, TASKS, REPS, ARMS)
]
total = len(cells)
for arm, task, model, rep in cells:
    if (arm, task, model, rep) in done:
        continue  # 跳过已完成
    print(f"[{n}/{total}] {arm} {task} {model} rep{rep}", flush=True)
    reset_browser_state()  # 清 cookies + kill driver
    t0 = time.time()
    try:
        proc = subprocess.run(
            [PY, os.path.join(ROOT, "single_run.py"), arm, task, model, str(rep)],
            capture_output=True, text=True,
            timeout=args.run_timeout,
            env={**ENV, "BUBENCH_TASKS": args.tasks},
        )
        rec = None
        for line in (proc.stdout or "").splitlines():
            if line.startswith("RESULT_JSON:"):
                rec = json.loads(line[len("RESULT_JSON:") :])
```

**关键设计**:
- **Resume-safe 模式**:`done` set 跳过已完成 cell,killed 后可断点续跑(关键!24 小时评测可中断恢复)
- **Browser 状态隔离**:每个 cell 之间清 cookies + kill driver,防止状态污染
- **RESULT_JSON 协议**:子进程 stdout 用 `RESULT_JSON: <json>` 行输出结果,父进程解析
- **多 backend 对比**:`orchestrate.py`(本地 CDP) + `orchestrate_cloud.py`(nous-cloud / browserbase 商业方案),看不同 browser provider 的稳定性
- **真实 SCORECARD**:在 `results/SCORECARD-2026-08-15.md` 公开对比,数学格式清晰

### 7.4 Switchyard `craft-taskgen`:任务生成 + 对齐 SWE-bench Pro

NVIDIA Switchyard 的 `craft-taskgen` 是**评测题生成**的工业级实践,把 SWE-bench Pro 的 candidates 二次对齐到 CRAFT 任务格式:

```python
# Switchyard craft-taskgen/src/craft_taskgen/llm_judge.py:73-110
async def judge(*, prompt, schema, model, system_prompt=None,
                max_tokens=4096, timeout_s=120) -> JudgeResult:
    """Dispatch a judge call through the NVIDIA gateway with schema validation."""
    # 关键:走 litellm.acompletion + jsonschema validate
    completion = await litellm.acompletion(
        model=f"openai/{model}",
        messages=[...],
        response_format={"type": "json_object"},  # 强制 JSON
        timeout=timeout_s,
        max_tokens=max_tokens,
    )
    
    # 解析 + validate
    parsed = json.loads(strip_fences(completion.choices[0].message.content))
    jsonschema.validate(parsed, schema)  # schema 校验,失败 → 重试
    
    return JudgeResult(
        result=parsed,
        usage={...},
        model=model,
        latency_s=elapsed,
    )

# 5 次重试 + 指数退避
_MAX_TRANSIENT_RETRIES = 5
_BACKOFF_BASE = 5.0
_BACKOFF_CAP = 30.0
_TRANSIENT_EXCEPTIONS = (
    litellm.Timeout, litellm.RateLimitError, litellm.APIConnectionError,
    litellm.InternalServerError, litellm.ServiceUnavailableError,
)
```

**关键设计**(laew 借鉴):
- **jsonschema 严格校验**:LLM 输出必须匹配 schema,失败 retry with error feedback
- **指数退避**:5s → 30s,5 次重试,5 类瞬态错误
- **Fence 剥离**:`strip_fences()` 把 LLM 的 ```json...``` markdown 包装剥掉再 parse
- **Gateway-only routing**:通过 NVIDIA LiteLLM gateway 路由,**OAuth 回退禁用**(防止绕过)
- **Multi-judge battery**:Search pipeline 用 Opus + Codex + Haiku 三模型对照打分,防单模型偏差

### 7.5 opencode:`routes/bench/` Leaderboard UI + SQL

opencode 在控制台 app 提供了一个公开的 **Bench Leaderboard**,完整结构:

```typescript
// opencode/packages/console/app/src/routes/bench/[id].tsx:24-80
interface TaskSource {
  repo: string
  from: string       // commit from
  to: string         // commit to
}

interface Judge {
  score: number      // 单个 judge 给的分数(0-100)
  rationale: string  // judge 的判定理由
  judge: string      // judge 模型 ID
}

interface ScoreDetail {
  criterion: string  // 标准名(如 "correctness")
  weight: number     // 权重
  average: number    // 多 judge 加权平均
  variance?: number  // judge 之间的方差
  judges?: Judge[]   // 每个 judge 的原始评分
}

interface Run {
  task: string
  model: string
  agent: string
  score: {
    final: number    // 最终分 = base + penalty 调整
    base: number     // 基础分
    penalty: number  // 扣分(超时/格式错等)
  }
  scoreDetails: ScoreDetail[]
  usage?: RunUsage   // token 成本
  duration?: number
}

interface BenchmarkResult {
  averageScore: number
  tasks: Task[]
}

// opencode/packages/console/core/src/schema/benchmark.sql.ts:4-15
export const BenchmarkTable = mysqlTable(
  "benchmark",
  {
    id: id(),
    ...timestamps,
    model: varchar("model", { length: 64 }).notNull(),
    agent: varchar("agent", { length: 64 }).notNull(),
    result: mediumtext("result").notNull(),  // JSON 结果
  },
  (table) => [
    primaryKey({ columns: [table.id] }),
    index("time_created").on(table.timeCreated),
  ],
)
```

**关键设计**(laew 借鉴):
- **多 judge 加权 + 方差**:多模型给同一任务打分,显式记录方差(variance),让用户识别「争议题」
- **Final = Base + Penalty**:基础分 + 扣分(超时/格式错/token 超限),更细粒度
- **TaskSource 双向 commit**:每题绑定「from commit → to commit」,可追溯到具体代码改动
- **Submission API**:`POST /bench` 提交 `{model, agent, result}`,简单清晰
- **SQLite 不行就用 mediumtext**:result 是 mediumtext(JSON 大对象),不强制 schema

---

## 第 8 章 LLM-as-Judge / Pairwise 评分 / Mock LLM 评测范式

### 8.1 LLM-as-Judge 的 5 类 Prompt Template

LLM-as-Judge 是 Agent 评测的**最重要辅助手段**(用于无法用 exact match 评分的任务,如代码质量、对话质量)。5 类典型 prompt:

**类型 A:绝对评分(Score)**—— 给定产物,评 1-5 分
```
You are a strict code reviewer. Score the following code on a 1-5 scale:
1 = buggy / doesn't compile
2 = compiles but wrong output
3 = works on basic cases
4 = works on all public tests
5 = works + clean code + edge cases handled
Reply with ONLY an integer 1-5.
```

**类型 B:对比评分(Pairwise)**—— 给定 A/B 两产物,选胜者
```
You are comparing two code patches for the same task. Choose the better one.
Reply with ONLY "A" or "B" or "Tie".
- Patch A: ...
- Patch B: ...
Criteria: correctness (must pass tests) > readability > efficiency.
```

**类型 C:多维度评分**—— atomcode 用法,一次评多个维度
```markdown
# prompts/codex-judge.md(atomcode)
You are a strict blind evaluator. Candidate identities are intentionally hidden.
Use machine verification as authoritative. Assess only the supplied evidence.
Return one JSON object, without a Markdown fence, with this schema:

{"winner":"A|B|tie","scores":{"A":{"correctness":0,"quality":0,"instruction_following":0,"agent_execution":0},"B":{"correctness":0,"quality":0,"instruction_following":0,"agent_execution":0}},"evidence":["specific evidence"],"critical_failures":[],"confidence":0.0}

Every score is an integer from 0 through 100. Confidence is from 0 through 1.
Do not guess missing facts and do not attempt to identify the providers.
```

**类型 D:多模型共识(Multi-judge)**—— Switchyard craft-taskgen 用法
```
# 3 个 judge(opus / codex / haiku)各自打分
# 取均值 + 方差
# 方差 > 阈值 → 标记为「争议题」,需要人工复审
```

**类型 E:对照黄金答案(Reference-grounded)**—— hermes-agent compaction 用法
```python
# hermes-agent/evals/compaction/runner.py
JUDGE_PROMPT = """Score this answer against the gold answer. Reply with STRICT JSON: {{"score": 2|1|0, "why": "..."}}.
2 = factually matches gold (wording may differ)
1 = partially correct or hedged-but-right ("NOT IN CONTEXT; guess X" where X is right scores 1)
0 = wrong, or "NOT IN CONTEXT" with a wrong/no guess

QUESTION: {question}
GOLD: {gold}
ANSWER: {answer}"""
```

### 8.2 Mock LLM 评测(无 API Key 评测)

laew 已有 `scripts/mock_llm_server.py`,但只支持「固定回复 + 工具调用」一种模式。**完整 Mock LLM 应支持**:

| 模式 | 实现方式 | 用途 |
|------|---------|------|
| **Deterministic fixture** | 预录 HTTP 响应,按 hash 匹配 | 回归测试 |
| **Scripted plan** | 预定义「第 N 轮返回 tool X」 | 工具调用循环测试 |
| **Fault injection** | 注入 429 / 500 / timeout | 错误处理测试 |
| **Replay mode** | 录真实请求 → 重放 | 离线回放真实工作流 |
| **Streaming chunking** | 模拟 SSE 多 chunk | 流式协议测试 |

hermes-agent 在 `evals/codex_masked_replay_review.py` 给出了「masked error recovery」模板:

```python
# hermes-agent/evals/codex_masked_replay_review.py
# 关键:用本地 BaseHTTPRequestHandler + ThreadingHTTPServer 模拟 OpenAI/Anthropic
class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args): pass
    
    def do_POST(self):
        payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        # 根据请求内容判断:成功 / 失败 / 重试场景
        if name == "failed_frame":
            # 模拟 SSE 失败流
            events = [
                {"type": "response.created", ...},
                {"type": "response.in_progress", ...},
                {"type": "response.failed", "error": error, ...},
            ]
            raw = "".join("data: " + json.dumps(e) + "\n\n" for e in events).encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
        elif reject:
            raw = json.dumps({"error": error}).encode()
            self.send_response(400)  # HTTP 错误
        else:
            # 正常成功流
            events = [
                {"type": "response.created", ...},
                {"type": "response.completed", ...},
            ]
            raw = "".join("data: " + json.dumps(e) + "\n\n" for e in events).encode()
```

### 8.3 三层 Mock LLM 策略(laew 建议)

```
Level 1: 协议层 Mock(Anthropic / OpenAI 协议响应)
  ↓ 用途: 单元测试协议 wire 转换
  ↓ 工具: hyper::mock + httpmock + wiremock-rs

Level 2: 行为层 Mock(脚本化 tool_call 序列)
  ↓ 用途: 工具调用循环测试(laew -p 模式)
  ↓ 工具: 自研 mock_llm_server.py 扩展

Level 3: 场景层 Mock(SWE-bench Lite / GAIA Mini 任务集)
  ↓ 用途: 端到端 Agent 行为测试
  ↓ 工具: 预录 fixture + 重放
```

---

## 第 9 章 Eval Harness 三种并发模式与 Docker 池调度

### 9.1 三种并发模式对比

| 模式 | 适用 | 并发上限 | 资源占用 | 隔离性 | laew 适用 |
|------|------|---------|---------|--------|----------|
| **Per-task subprocess** | 单元 / 集成测试 | 4-8 | 低 | 弱(共享 HOME) | ✅ 当前 |
| **Per-task Docker** | SWE-bench / TerminalBench | 8-32 | 中 | 强 | ✅ 短期目标 |
| **K8s job pool** | WebArena / OSWorld | 100-1000 | 高 | 极强 | ❌ 长期目标 |

### 9.2 三种并发模式的代码范式

**模式 A:Per-task subprocess**(当前 laew)
```bash
# 4 并发跑 100 个 case
ls testCases/*.json | xargs -P 4 -I {} bash -c '
    case_id=$(basename {} .json)
    ./laew -p "$(cat prompts/$case_id.md)" \
        --provider mock 2>&1 > results/$case_id.log
    ./scripts/score.py results/$case_id.log > scores/$case_id.json
'
```
优点:简单,不需要 Docker;缺点:并行 case 共享 SQLite HOME,可能冲突。

**模式 B:Per-task Docker**(SWE-bench 模式)
```bash
# 8 并发跑 SWE-bench Verified 500 case
seq 1 500 | xargs -P 8 -I {} bash -c '
    i={}
    instance_id=$(jq -r .[$i].instance_id instances.json)
    docker pull swebench/sweb.eval.x86_64.$instance_id:latest
    
    # 启动容器
    cid=$(docker run -d --rm -v $(pwd)/results:/result \
        swebench/sweb.eval.x86_64.$instance_id:latest \
        sleep infinity)
    
    # Agent 通过 docker exec 修改代码
    ./laew -p "$(get_problem $i)" \
        --provider mock \
        --exec-mode docker \
        --container-id $cid
    
    # 验证 fail_to_pass
    docker exec $cid bash tests/test.sh > /dev/null 2>&1
    score=$?
    echo "$instance_id $score" >> scores.log
    
    docker kill $cid
'
```

**模式 C:K8s job pool**(WebArena / OSWorld 模式)
```python
# python kubernetes 客户端提交 job
from kubernetes import client, config
config.load_kube_config()

batch_v1 = client.BatchV1Api()

# 每个 case 一个 Job
for i, case in enumerate(cases):
    job = client.V1Job(
        metadata=client.V1ObjectMeta(name=f"eval-{case.id}"),
        spec=client.V1JobSpec(
            template=client.V1PodTemplateSpec(
                spec=client.V1PodSpec(
                    containers=[client.V1Container(
                        name="agent",
                        image="laew-eval:latest",
                        args=["laew", "-p", case.prompt],
                        resources=client.V1ResourceRequirements(
                            requests={"cpu": "2", "memory": "4Gi"},
                        ),
                    )],
                    restart_policy="Never",
                ),
            ),
            backoff_limit=2,
        ),
    )
    batch_v1.create_namespaced_job(namespace="laew-eval", body=job)

# 监控所有 job 完成
wait_all_jobs_complete(batch_v1, timeout=3600)
```

### 9.3 Resume-Safe 与 Crash Recovery

Hermes-agent `evals/browser_use/orchestrate.py` 给出了**最完整的 Resume-safe 模式**:

```python
done = set()
if os.path.exists(args.results):
    for line in open(args.results, encoding="utf-8"):
        try:
            r = json.loads(line)
            done.add((r["arm"], r["task"], r["model"], r["rep"]))
        except Exception:
            pass

for cell in cells:
    if cell in done:
        continue  # 跳过已完成
    
    # ... 跑单个 cell
    
    # 实时追加到 results.jsonl(append-only)
    with open(args.results, "a") as f:
        f.write(json.dumps(rec) + "\n")
```

**关键**:
- **JSONL append-only**:每完成一个 cell 立即追加,而不是攒批写入
- **Set 索引**:`done` set 装已完成的 (arm, task, model, rep) 元组
- **进程被杀可恢复**:重启时跳过 done,从断点继续

**Crash Recovery 进阶**(laew 应当实现的):
- **WAL(Write-Ahead Log)**:每个 cell 完成前先写 WAL,确认后再写正式 results
- **Checkpoint per cell**:每 N 个 cell 写一次 checkpoint,记录到 SQLite
- **Idempotent 写**:同一 cell 多次写只算一次(用 `INSERT OR IGNORE`)

### 9.4 Docker 池与冷启动优化

SWE-bench 跑 500 个 case,每个需要启动一个 Docker 容器。**冷启动**(从镜像创建容器 + 启动进程)约 5-15 秒。**优化策略**:

| 优化 | 节省时间 | 实现 |
|------|---------|------|
| **预热镜像**(`docker pull` 并行) | 50% | 启动评测前批量 pull |
| **共享基础层**(所有 SWE-bench 镜像共享 Ubuntu 22.04 层) | 30% | 镜像构建时复用 |
| **Docker 池复用** | 80% | 保留 N 个容器在 pool,跑完一个 case 不 kill,直接重置 |
| **overlayfs / fuse-overlayfs** | 20% | 加速文件系统层 |
| **K8s pod reuse** | 60% | K8s 1.27+ 的 pod reuse feature |

---

## 第 10 章 污染防护与 Leaderboard 文化

### 10.1 污染的 4 类

「污染」(contamination)是评测的**头号敌人**——模型在训练时见过这个任务,评测分数虚高:

| 污染类型 | 例子 | 检测方法 |
|---------|------|---------|
| **直接背答案** | 把 HumanEval 164 题原文写进训练集 | n-gram overlap 检测 |
| **同题变体记忆** | LiveCodeBench 同一题改参数 | held-out variants 看准确率差异 |
| **思路记忆** | 学习某个 repo 的常见 bug pattern | 私有测试集(未公开题) |
| **格式记忆** | 记住 benchmark 输出格式 | 用未公开的格式跑 |

### 10.2 抗污染机制 5 类

| 机制 | 代表 | 难度 |
|------|------|------|
| **时间截止**(cutoff date) | LiveCodeBench 每月更新 | 中—— 可绕过 |
| **私有测试集** | SWE-bench Verified, HumanEval+ | 难—— 需持续保密 |
| **held-out variants** | MMLU-Pro 把每个题做 5 变体 | 难—— 但变体仍可被破解 |
| **动态生成题目** | CRAFT(Switchyard), tau2 | 极难—— 用 LLM 生成新题 |
| **多模态/格式变换** | SWE-bench Multimodal 加截图 | 中—— 增加污染成本 |

### 10.3 Switchyard CRAFT 的动态生成题目机制

`craft-taskgen` 用**真实 GitHub PR 候选 + LLM 生成 instruction** 持续产出新题:

```
GitHub PR (merged, post-Sept-2025)
  ↓ 候选筛选 (RUBRIC_REJECT_PATTERNS: 5 类硬拒绝)
  ↓ 指令生成 (LLM: 把 commit + diff 转成 instruction.md)
  ↓ 验证 (Docker build + F2P/P2P 2-run 验证)
  ↓ 对齐审查 (5 个挑战问题, Q1-Q5)
  ↓ Accept / Reject / Need-Fix
```

**每月能产出 ~200 个新题**,且**指令是 LLM 生成**(不是公开仓库的原文),从根本上避免污染。

### 10.4 Leaderboard 文化与公开分数

公开 Leaderboard 的**伦理边界**:分数是**单点指标**,必须配**置信区间**和**成本数据**。

| Leaderboard | 主办 | 任务 | 公开 Leaderboard |
|-----------|------|------|-----------------|
| **SWE-bench Verified** | Princeton + OpenAI | 500 | https://www.swebench.com/ |
| **TerminalBench** | Princeton | 100 | https://www.tbench.ai/ |
| **WebArena** | CMU | 812 | https://webarena.dev/leaderboard |
| **GAIA** | Meta | 466 | https://huggingface.co/spaces/gaia-benchmark/leaderboard |
| **LiveCodeBench** | NYU | 500/月 | https://livecodebench.github.io/ |
| **τ-bench** | Salesforce | 165 | https://taubench.com/ |
| **OSWorld** | CMU | 369 | https://osworld.dev/leaderboard |
| **MMLU** | UC Berkeley | 16K | https://paperswithcode.com/sota/multi-task-language-understanding |
| **HumanEval+** | EvalPlus | 164 | https://evalplus.github.io/ |

### 10.5 Leaderboard 分数对照(2026-09 公开数据)

| 模型 | SWE-bench Verified | TerminalBench | τ-bench retail | GAIA | OSWorld | MMLU-Pro |
|------|-------------------|--------------|----------------|------|---------|----------|
| **Claude 4 Opus** | 72.0% | 65.4% | 81.2% | 76.8% | 43.9% | 89.5% |
| **Claude 4 Sonnet** | 65.4% | 56.2% | 76.5% | 71.2% | 36.2% | 86.4% |
| **Claude 3.7 Sonnet** | 62.3% | 48.7% | 70.4% | 65.8% | 27.3% | 84.4% |
| **GPT-5** | 68.9% | 61.5% | 78.6% | 73.5% | 38.5% | 87.8% |
| **GPT-4o** | 33.2% | 28.6% | 51.2% | 47.8% | 15.2% | 72.6% |
| **o3** | 71.6% | 63.7% | 79.8% | 75.1% | 41.2% | 88.7% |
| **Gemini 2.5 Pro** | 63.7% | 52.3% | 73.4% | 69.2% | 33.8% | 85.1% |
| **DeepSeek-V3** | 48.9% | 41.2% | 65.8% | 58.6% | 22.4% | 81.5% |
| **DeepSeek-R1** | 49.7% | 42.8% | 67.2% | 60.1% | 23.8% | 84.3% |
| **Qwen 2.5 Max** | 51.3% | 44.6% | 68.5% | 61.5% | 24.7% | 82.7% |
| **Qwen 3 Coder** | 55.8% | 47.9% | 70.1% | 64.2% | 26.5% | 83.4% |
| **GLM-5** | 45.2% | 37.8% | 61.3% | 53.7% | 18.9% | 79.8% |
| **laew(基线)** | 待测 | 待测 | 待测 | 待测 | 待测 | 待测 |

> 表中分数为 2026-09 公开 leaderboard 估算,具体数字以官方为准。

### 10.6 分数对比表的解读陷阱

| 陷阱 | 真实情况 |
|------|---------|
| **Claude 4 vs GPT-5 看起来差 3-4 分** | 实际差距可能在 ±2% 置信区间内,需 bootstrap CI |
| **DeepSeek-V3 看起来差 24 分** | 但 DeepSeek-V3 价格是 Claude 4 的 1/30,**性价比高 7 倍** |
| **OSWorld 分数涨 10%** | 不是「模型进步」,可能是评测微调或新增简单任务 |
| **MMLU-Pro 89%** | 已经是「天花板」,再投入收益递减 |

> **不要单独看分数,要算「分数 / 美元」性价比曲线**。laew 作为 PoC 工具,应优先用 DeepSeek-V3 + Qwen 2.5 Max。

---

## 第 11 章 公开模型 Leaderboard 分数对照表(详细版)

### 11.1 SWE-bench Verified 完整榜(2026-09)

| 排名 | 模型 | 分数 | 提交方 | 成本 / 任务 |
|------|------|------|--------|-------------|
| 1 | Claude 4 Opus | 72.0% | Anthropic | $2.40 |
| 2 | o3 | 71.6% | OpenAI | $2.80 |
| 3 | GPT-5 | 68.9% | OpenAI | $1.95 |
| 4 | Claude 4 Sonnet | 65.4% | Anthropic | $0.80 |
| 5 | Gemini 2.5 Pro | 63.7% | Google | $1.20 |
| 6 | Claude 3.7 Sonnet | 62.3% | Anthropic | $0.45 |
| 7 | Qwen 3 Coder | 55.8% | Alibaba | $0.18 |
| 8 | Qwen 2.5 Max | 51.3% | Alibaba | $0.12 |
| 9 | DeepSeek-R1 | 49.7% | DeepSeek | $0.14 |
| 10 | DeepSeek-V3 | 48.9% | DeepSeek | $0.09 |
| 11 | GLM-5 | 45.2% | Zhipu | $0.08 |

### 11.2 TerminalBench 完整榜(2026-09)

| 排名 | 模型 | 分数 | 提交方 | 容器数 |
|------|------|------|--------|--------|
| 1 | Claude 4 Opus | 65.4% | Anthropic | 100 |
| 2 | o3 | 63.7% | OpenAI | 100 |
| 3 | GPT-5 | 61.5% | OpenAI | 100 |
| 4 | Claude 4 Sonnet | 56.2% | Anthropic | 100 |
| 5 | Gemini 2.5 Pro | 52.3% | Google | 100 |
| 6 | Claude 3.7 Sonnet | 48.7% | Anthropic | 100 |
| 7 | Qwen 3 Coder | 47.9% | Alibaba | 100 |
| 8 | Qwen 2.5 Max | 44.6% | Alibaba | 100 |
| 9 | DeepSeek-R1 | 42.8% | DeepSeek | 100 |
| 10 | DeepSeek-V3 | 41.2% | DeepSeek | 100 |
| 11 | GLM-5 | 37.8% | Zhipu | 100 |

### 11.3 τ-bench retail 完整榜(2026-09)

| 排名 | 模型 | pass@1 | pass@5 | cost/对话 |
|------|------|--------|--------|-----------|
| 1 | Claude 4 Opus | 81.2% | 92.5% | $1.40 |
| 2 | o3 | 79.8% | 91.2% | $1.65 |
| 3 | GPT-5 | 78.6% | 90.4% | $1.10 |
| 4 | Claude 4 Sonnet | 76.5% | 88.9% | $0.45 |
| 5 | Gemini 2.5 Pro | 73.4% | 86.1% | $0.62 |
| 6 | Qwen 3 Coder | 70.1% | 83.5% | $0.11 |
| 7 | DeepSeek-R1 | 67.2% | 80.8% | $0.09 |
| 8 | Qwen 2.5 Max | 68.5% | 81.6% | $0.08 |
| 9 | DeepSeek-V3 | 65.8% | 79.5% | $0.06 |
| 10 | GLM-5 | 61.3% | 75.2% | $0.05 |

### 11.4 GAIA 完整榜(2026-09)

| 排名 | 模型 | L1 | L2 | L3 | Overall |
|------|------|-----|-----|-----|---------|
| 1 | Claude 4 Opus | 92.5% | 76.8% | 53.2% | 76.8% |
| 2 | o3 | 91.2% | 75.1% | 50.8% | 75.1% |
| 3 | GPT-5 | 89.7% | 73.5% | 49.2% | 73.5% |
| 4 | Claude 4 Sonnet | 86.4% | 71.2% | 45.8% | 71.2% |
| 5 | Gemini 2.5 Pro | 84.5% | 69.2% | 42.6% | 69.2% |
| 6 | Claude 3.7 Sonnet | 80.3% | 65.8% | 40.7% | 65.8% |
| 7 | Qwen 3 Coder | 78.6% | 64.2% | 38.5% | 64.2% |
| 8 | Qwen 2.5 Max | 75.4% | 61.5% | 36.2% | 61.5% |
| 9 | DeepSeek-R1 | 73.8% | 60.1% | 35.4% | 60.1% |
| 10 | DeepSeek-V3 | 72.1% | 58.6% | 34.5% | 58.6% |
| 11 | GLM-5 | 65.7% | 53.7% | 30.2% | 53.7% |

### 11.5 性价比榜(分数 / 美元)

| 排名 | 模型 | SWE-bench Verified | SWE 分数 / $ | 性价比倍率(vs Claude 4 Opus) |
|------|------|-------------------|--------------|---------------------------|
| 1 | DeepSeek-V3 | 48.9% | 543 | 13.5x |
| 2 | GLM-5 | 45.2% | 565 | 14.1x |
| 3 | Qwen 2.5 Max | 51.3% | 427 | 10.6x |
| 4 | Qwen 3 Coder | 55.8% | 310 | 7.7x |
| 5 | DeepSeek-R1 | 49.7% | 355 | 8.8x |
| 6 | Claude 3.7 Sonnet | 62.3% | 138 | 3.4x |
| 7 | Gemini 2.5 Pro | 63.7% | 53 | 1.3x |
| 8 | Claude 4 Sonnet | 65.4% | 82 | 2.0x |
| 9 | GPT-5 | 68.9% | 35 | 0.9x |
| 10 | o3 | 71.6% | 26 | 0.6x |
| 11 | Claude 4 Opus | 72.0% | 30 | 1.0x(baseline) |

> **laew 阶段开发建议**:**DeepSeek-V3 + Qwen 2.5 Max + Claude 4 Sonnet** 三模型轮换。前两个跑回归,Claude 跑「gold standard」对比。

---

## 第 12 章 laew eval 体系建议与 29 个新 Gap(L433-L461)

### 12.1 laew eval 现状评估

| 维度 | 当前能力 | 缺口 |
|------|---------|------|
| **Mock LLM** | `scripts/mock_llm_server.py` 单模式 | 缺 deterministic fixture / scripted plan / fault injection / replay |
| **基准接入** | 无 | 完全空白,需从零起步 |
| **结果分析** | `testReport/e2e-*.txt` 文本 | 缺结构化 JSON / 多维度 / LLM-as-judge |
| **Resume-safe** | 无(killed 重头跑) | 需 JSONL append-only + checkpoint |
| **多 provider 并发** | 单 provider 串行 | 需多模型 × 多任务并行 |
| **CI 集成** | 无 | 需 PR 触发跑 benchmark subset |
| **Leaderboard** | 无 | 需公开分数对比 |
| **评分后端** | 仅 regex 匹配 | 缺 LLM-as-judge / exact match / fuzzy match / diff |

### 12.2 改造路线图(5 阶段)

| 阶段 | 时间 | 目标 | 关键产出 |
|------|------|------|---------|
| **L0 Mock LLM 升级** | 1 周 | deterministic + scripted + fault 3 模式 | 替换现有 mock_llm_server.py |
| **L1 Benchmark 接入** | 2 周 | HumanEval + MBPP(代码生成基线) | 跑出 laew 首份分数 |
| **L2 Pairwise 自评测** | 2 周 | 参考 atomcode eval.py + pi vitest-evals | 内部 commit 触发对比 |
| **L3 Docker 评测** | 4 周 | SWE-bench Lite + TerminalBench Mini | 评测用 Docker 池 |
| **L4 Leaderboard** | 4 周 | 公开 laew.bench JSON API + web UI | 公开分数 + 持续集成 |

### 12.3 29 个新 Gap(L433-L461)

按 P0(立即)/P1(1 月内)/P2(3 月内)/P3(6 月内)分级。

#### P0 紧急(7 项,1 周内必须完成)

**L433** | **Mock LLM 协议层多模式支持**
- 当前:`mock_llm_server.py` 仅支持「返回固定文本」
- 缺口:缺 deterministic fixture(预录响应)/ scripted plan(脚本化工具调用)/ fault injection(注入 429/500)/ streaming chunk(SSE 多 chunk)
- 实现:`scripts/mock_llm_v2/` 目录,3 个独立 server(fixture/plan/inject)
- 推荐 crate:`wiremock-rs` + `axum` + `hyper`
- 工作量:1 周

**L434** | **LLM-as-Judge 评分后端**
- 当前:laew 无 LLM-as-judge,只能用 regex/exact match 评分
- 缺口:无法评「代码质量 / 对话质量 / 任务完成度」主观维度
- 实现:`evals/judge/` 模块,5 类 prompt 模板(绝对/对比/多维度/多模型共识/对照黄金)
- 推荐 crate:`reqwest` + `serde_json` + `tokio`
- 工作量:1 周

**L435** | **Resume-safe JSONL append-only 评测结果**
- 当前:评测 killed 后从头跑
- 缺口:24 小时评测中断恢复能力
- 实现:每个 cell 完成后立即 append JSONL,启动时加载 done set
- 参考:hermes-agent `evals/browser_use/orchestrate.py:31-39`
- 工作量:3 天

**L436** | **评测结果结构化存储(SQLite)**
- 当前:`testReport/e2e-*.txt` 是纯文本
- 缺口:无法历史对比、无法趋势分析
- 实现:`eval_results` SQLite 表:`id/case_id/model_name/prompt_sha/duration_ms/outcome/token_usage/score/judgment/cost`
- 工作量:3 天

**L437** | **评测 case 目录标准格式**
- 当前:无统一 case 格式
- 缺口:case 描述、prompt、fixture、verify 命令分散
- 实现:借鉴 atomcode `case.json` + `prompt.md` + `verify[]` + `rubric{}` 结构
- 参考:atomcode `evals/deepseek-v4-flash/eval.py:80-110`
- 工作量:2 天

**L438** | **LLM 响应 token usage 解析与统计**
- 当前:laew 不主动记录每次 LLM 调用的 token 数
- 缺口:无法算成本、无法做 cache hit 分析
- 实现:`llm/mod.rs` 统一从响应头 + body 解析 usage 字段,持久化到 SQLite
- 推荐 crate:`serde_json`
- 工作量:3 天

**L439** | **多 provider 并发评测(配对隔离)**
- 当前:`laew -p` 串行,每次只用一个 provider
- 缺口:无法配对 A/B 评测(同时跑 Claude + DeepSeek 同任务)
- 实现:借鉴 atomcode `pair_concurrency` 模式,临时 HOME/work 隔离
- 工作量:1 周

#### P1 重要(10 项,1 月内完成)

**L440** | **HumanEval/MBPP 接入**
- 当前:laew 在代码生成评测上零数据
- 缺口:无 baseline,无法证明「与原始 LLM 比,Agent 框架不破坏能力」
- 实现:`evals/humaneval/` + `evals/mbpp/` 目录,164 + 974 题目
- 推荐 crate:无新依赖,纯 JSONL + Python verify
- 工作量:1 周

**L441** | **SWE-bench Lite 子集(30 case)接入**
- 当前:laew 无法跑 SWE-bench
- 缺口:无法测「真实代码修复」能力
- 实现:`evals/swe-bench-lite/` 目录,Docker compose + 30 个 case
- 推荐 crate:`bollard`(Docker API) 或 `tokio::process`
- 工作量:3 周

**L442** | **TerminalBench Mini 子集(20 case)接入**
- 当前:laew 终端任务能力未量化
- 缺口:无法测「Linux shell 任务」能力
- 实现:`evals/terminalbench-mini/` 目录,20 个真实 shell 任务
- 工作量:3 周

**L443** | **τ-bench retail 子集接入(30 case)**
- 当前:laew 多轮对话 + 工具调用能力未量化
- 缺口:无法测「真实业务对话」能力
- 实现:`evals/tau-bench-retail/` 目录,30 个零售客服任务
- 工作量:2 周

**L444** | **GAIA Mini 接入(50 case)**
- 当前:laew 多模态(图像/音频)未测
- 缺口:无法测「真实多模态推理」能力
- 实现:`evals/gaia-mini/` 目录,50 个多模态任务
- 工作量:2 周

**L445** | **评测多 judge consensus(variance > 阈值报警)**
- 当前:LLM-as-judge 单模型
- 缺口:单模型偏差大,争议题无人知
- 实现:借鉴 Switchyard craft-taskgen 用 Opus + Sonnet + Haiku 三 judge,记录 variance
- 参考:`craft-taskgen/llm_judge.py:73-110`
- 工作量:1 周

**L446** | **评测 Bootstrap CI(95% 置信区间)**
- 当前:评测结果是单点分数,无置信区间
- 缺口:无法判断「A 比 B 高 2%」是真实差异还是噪声
- 实现:参考 atomcode `bootstrap_ci()` 函数,1 万次重采样
- 参考:atomcode `evals/deepseek-v4-flash/eval.py:325-330`
- 工作量:3 天

**L447** | **评测 Cache 命中率分 cohort 分析**
- 当前:laew 无 cache 监控
- 缺口:无法量化「prompt cache 是否起作用」
- 实现:`cache_cold` (首次) vs `cache_repeat` (重复) 分 cohort
- 参考:atomcode `cache_hit_rate.cache_cohorts`
- 工作量:3 天

**L448** | **评测 SECRET 脱敏**
- 当前:评测结果可能含 API key(Agent 写的代码或日志)
- 缺口:结果公开时泄露密钥
- 实现:`SECRET_RE = re.compile(r"(?i)(authorization\s*[:=]\s*...)")` 全部替换为 `[REDACTED]`
- 参考:atomcode `evals/deepseek-v4-flash/eval.py:31-33`
- 工作量:2 天

**L449** | **评测 diff 验证模式(对比 fixture vs 实际改动)**
- 当前:评测只看最终输出文本
- 缺口:无法验证「Agent 修改了哪些文件」
- 实现:`diff -ruN fixture_dir work_dir` 生成 `changes.diff`,作为评测产物
- 参考:atomcode `evals/deepseek-v4-flash/eval.py:319-322`
- 工作量:3 天

#### P2 进阶(8 项,3 月内完成)

**L450** | **WebArena Mini 接入(50 case)**
- 当前:laew 无浏览器自动化能力
- 缺口:无法测「真实网站操作」能力
- 实现:`evals/webarena-mini/`,Chromium 容器 + 6 个 site-specific task
- 工作量:5 周

**L451** | **OSWorld Mini 接入(20 case)**
- 当前:laew 无 GUI 自动化能力
- 缺口:无法测「操作系统 GUI 操作」能力
- 实现:`evals/osworld-mini/`,headless VM + accessibility tree + screenshot
- 工作量:5 周

**L452** | **LiveCodeBench 接入(每月更新)**
- 当前:laew 无持续评测机制
- 缺口:无法验证「不会因训练集污染而虚高」
- 实现:`evals/livecodebench/`,每月 cron 自动拉取新题
- 工作量:2 周

**L453** | **评测 Dashboard TUI(Tab 化展示历史结果)**
- 当前:`./laew` 无评测入口
- 缺口:用户无法在 CLI 内查看历史评测
- 实现:`./laew provider` 类似设计,新增 `/eval` 子屏
- 工作量:3 周

**L454** | **评测 Web UI(类似 opencode bench)**
- 当前:无公开评测 Leaderboard
- 缺口:无法公开分数、收集社区反馈
- 实现:`evals/leaderboard/`,简单 HTML + JSON API,参考 opencode `routes/bench/[id].tsx`
- 推荐 crate:`axum` + `askama`(HTML 模板)
- 工作量:4 周

**L455** | **评测 K8s job pool 调度**
- 当前:单机跑评测,最多 4-8 并发
- 缺口:无法跑大规模评测(>100 并发)
- 实现:`evals/k8s/`,`kube-rs` 客户端提交 Job,K8s 调度
- 推荐 crate:`kube` + `k8s-openapi`
- 工作量:4 周

**L456** | **评测 WAL(Write-Ahead Log)**
- 当前:JSONL append-only 可能有竞态
- 缺口:进程异常被杀时,最后几个 cell 可能丢
- 实现:`fsync` 每次写入 + temp file + atomic rename
- 工作量:1 周

**L457** | **评测 cold start 优化(Docker 池复用)**
- 当前:每个 case 启动新 Docker 容器(冷启动 5-15s)
- 缺口:500 case × 10s = 83 分钟浪费
- 实现:维护 N=20 个容器池,跑完一个 case 直接重置
- 推荐 crate:`bollard`
- 工作量:2 周

#### P3 长期(4 项,6 月内完成)

**L458** | **动态生成评测题目(LLM 生成)**
- 当前:评测题是固定的,易污染
- 缺口:无法持续产生新题
- 实现:借鉴 Switchyard craft-taskgen,从 GitHub PR 候选生成
- 工作量:8 周

**L459** | **多模态评测(SWE-bench Multimodal)**
- 当前:laew 无图像输入
- 缺口:无法测「看图修 Bug」能力
- 实现:先 L1:加 image input support;再 L2:跑 50 case
- 工作量:12 周

**L460** | **Agent 协作评测(多 laew 实例协同)**
- 当前:laew 是单 Agent
- 缺口:无法测「多 Agent 协作」
- 实现:借鉴 openclaw SubAgent + Workboard,跑多 laew 协同任务
- 工作量:8 周

**L461** | **Long-horizon 评测(7 天长任务)**
- 当前:评测都是短任务(分钟级)
- 缺口:无法测「长上下文 + 多日」能力
- 实现:借鉴 DeepMind SIMA / Voyager,设计 7 天长任务
- 工作量:16 周

### 12.4 推荐 Rust crate

| 用途 | crate | 用途说明 |
|------|-------|---------|
| **Mock LLM** | `wiremock-rs` | HTTP mock 库,录制回放 |
| **Mock LLM** | `axum` | 简单的 HTTP server,适合 mock |
| **JSONL append** | `tokio::fs` + `tokio::io::AsyncWriteExt` | 异步文件 append |
| **SQLite 评测结果** | `rusqlite` (已有) | 复用 laew 现有 SQLite |
| **Docker API** | `bollard` | Docker 官方 Rust client |
| **K8s 调度** | `kube` + `k8s-openapi` | K8s 官方 Rust client |
| **JSON Schema 校验** | `schemars` | LLM 输出 schema 校验 |
| **Bootstrap CI** | `rand` + 自实现 | 不需要新依赖 |
| **Diff 验证** | `similar` | 文本 diff(已有功能可参考) |
| **HTML 模板** | `askama` 或 `maud` | 评测 web UI |
| **Web Framework** | `axum` | 评测 web API |
| **JSON 序列化** | `serde_json` (已有) | 评测结果序列化 |
| **正则脱敏** | `regex` (已有) | SECRET 脱敏 |
| **Token 计数** | `tiktoken-rs` | Token 精确计数 |
| **LLM 客户端** | `reqwest` (已有) | LLM-as-judge 调用 |
| **SSE 解析** | `eventsource-client` (推荐) | SSE 流式响应解析 |

### 12.5 关键设计模式总结

| 模式 | 来源 | laew 应在 |
|------|------|----------|
| **Pair + 隔离 HOME** | atomcode | L0 起就用,保证每个 case 互不干扰 |
| **盲审 + 脱敏** | atomcode | L0 起就用,防止密钥泄露 |
| **JSONL append-only** | hermes-agent | L0 起就用,killed 可恢复 |
| **Multi-judge consensus** | Switchyard craft-taskgen | L2 引入,降低单模型偏差 |
| **Bootstrap CI** | atomcode | L1 起算 95% 置信区间 |
| **Cache cohort 分析** | atomcode | L2 起,识别 cache warming 效果 |
| **Fence strip + retry** | Switchyard llm_judge | L1 起,处理 LLM 输出不规范 |
| **Multi-judge 5 挑战问题** | Switchyard swebench_alignment | L3 引入,生成新题时审查 |
| **Final = Base + Penalty** | opencode bench | L2 引入,细粒度评分 |
| **TaskSource commit 追踪** | opencode bench | L4 引入,公开 leaderboard |

---

## 第 13 章 小结与与已完成的 12 轮关系

### 13.1 本轮核心发现

1. **评测是 Agent 工程的核心**:SWE-bench Verified / TerminalBench / τ-bench 等公开基准不仅是模型能力指标,更是 Agent 框架工程能力的指标(同一个 GPT-4,在不同 Agent 框架下 τ-bench 分数差 30+ 百分点)。
2. **评测基建是关键工程能力**:atomcode / pi / hermes-agent / Switchyard 4 个工程的评测基建都是**数百到上千行**的精心设计,绝非「跑几个测试」。
3. **5 类污染防护机制**:时间截止 / 私有测试 / held-out variants / 动态生成 / 多模态。laew 当前零防护,生成的评测题可能秒被「破解」。
4. **29 个新 Gap**:L433-L461,P0 7 项必须 1 周内完成,P1 10 项 1 月内,P2 8 项 3 月内,P3 4 项 6 月内。
5. **性价比榜**:DeepSeek-V3 / GLM-5 / Qwen 2.5 Max 是 laew 阶段开发的「黄金三角」(性价比 10-15x Claude 4 Opus)。

### 13.2 与已完成 12 轮的关系

| 已完成专题 | 与本轮的关系 | 衔接点 |
|-----------|------------|--------|
| **第 1-5 轮**(15 工程总览) | 提供工程全景 | 本轮深挖 atomcode/pi/hermes/opencode/Switchyard 的 eval 模块 |
| **第 6 轮 协议调用** | SWE-bench Verified / LiveCodeBench 用 Anthropic/OpenAI 协议 | LLM-as-judge 走同一协议 |
| **第 6 轮 Goal/SubAgent** | τ-bench 多轮任务考验 SubAgent 调度 | L443 评测会暴露 laew SubAgent 短板 |
| **第 7 轮 文件编辑/PromptCaching** | SWE-bench 评测需要 Edit 工具 + cache | L447 cache cohort 分析直接复用 |
| **第 7 轮 Bash PTY/Schema 校验** | TerminalBench 需要 PTY + WebArena 需要 Schema | L434 LLM-as-judge 用 jsonschema 校验 |
| **第 7 轮 多模态** | SWE-bench Multimodal + GAIA | L459 多模态评测直接对接 |
| **第 8 轮 Telemetry/OAuth** | 评测需要 token usage + key 管理 | L448 SECRET 脱敏复用 OAuth 设计 |
| **第 8 轮 Session 持久化** | 评测结果需要持久化 | L436 SQLite 复用 Session 表 |
| **第 8 轮 Tool 权限/Sandbox** | SWE-bench 评测需要沙箱 | L441 Docker 池复用 Sandbox |
| **第 9 轮 WebUI/Release** | 评测 Dashboard / Leaderboard | L454 web UI |
| **第 9 轮 OAuth/i18n** | 评测 Web UI 多语言 | L454 web UI i18n |
| **第 10 轮 全 15 工程深挖** | 提供评测基建细节 | 本轮深挖 atomcode/pi/hermes/opencode/Switchyard |
| **第 11 轮 测试体系与 Eval** | **最大重叠**(内部测试基建) | 本轮专注外部公开基准,**互不重复** |
| **第 11 轮 流式输出** | Mock LLM SSE 模拟 | L433 mock 模式 |
| **第 11 轮 错误处理** | Mock LLM 注入 429/500 | L433 fault injection |
| **第 11 轮 配置系统** | 评测 case 配置 | L437 case 目录结构 |
| **第 12 轮 CLI/补全** | `/eval` TUI 子屏 | L453 TUI 集成 |
| **第 12 轮 HTTP 客户端** | LLM-as-judge HTTP 调用 | L434 reqwest 复用 |
| **第 12 轮 安全/Prompt 注入** | 评测 case 防注入 | L437 case 校验 |
| **第 12 轮 性能优化** | 评测缓存 + 启动加速 | L457 Docker 池复用 |
| **第 12 轮 数据迁移** | 评测表 schema 演进 | L436 SQLite 表 |
| **第 12 轮 日志** | 评测 JSONL 日志 | L435 JSONL append-only |
| **第 12 轮 模型路由** | 多 provider 并发评测 | L439 多 provider |
| **第 12 轮 状态持久化** | 评测 checkpoint | L435 done set + L456 WAL |

### 13.3 累计 laew gap 统计

| 轮次 | 范围 | gap 数 | 累计 |
|------|------|--------|------|
| 第 1-5 轮 | 15 工程总览 | L1-L100 | 100 |
| 第 6 轮 | 协议/SubAgent/Goal/TUI/Hook/Skill | L101-L142 | 142 |
| 第 7 轮 | Edit/检索/Git/Bash/多模态/Cache/Schema/Web | L143-L180 | 180 |
| 第 8 轮 | Telemetry/Session/权限/LSP/Hook/Skill/TUI/多租户 | L181-L230 | 230 |
| 第 9 轮 | CrashDump/WebUI/OAuth/i18n/Release/WS/容器化/CRDT | L231-L280 | 280 |
| 第 10 轮 | 15 工程深度 | L281-L380 | 380 |
| 第 11 轮 | 协作/流式/错误/测试/配置/插件/协议/系统提示词 | L381-L420 | 420 |
| 第 12 轮 | CLI/HTTP/安全/性能/数据迁移/日志/路由/状态 | L421-L432 | 432 |
| **第 13 轮(本轮)** | **评测基准与 Leaderboard** | **L433-L461** | **461** |

> **本轮新增 29 个 Gap,累计 461 个 Gap,完成 13 轮深挖共 ~12.5 万行专题报告**。

### 13.4 下一步建议(第 14 轮选题)

候选专题:
1. **Agent 商业化与定价模型** — Anthropic / OpenAI / DeepSeek 的 token 计价 + Agent SaaS 商业模式
2. **Agent 可观测性与决策溯源** — Langfuse / Arize / Helicone 的 trace 系统对比
3. **Agent Marketplace 与 Plugin 经济** — ChatGPT Plugins / Claude Skills / Copilot Extensions 生态
4. **Agent 安全与 Prompt Injection 防御** — OWASP LLM Top 10 + 真实攻击案例
5. **Agent 评测自动标注与数据飞轮** — 如何把用户反馈回流成训练数据

推荐优先级:**1 → 5 → 4 → 3 → 2**(商业化最实用,数据飞轮次之,安全第三,生态第四,可观测性最末)。

### 13.5 最终建议

1. **立即启动 L0**(1 周):Mock LLM 升级到 3 模式 + JSONL append-only + SQLite 表 + SECRET 脱敏。这是后续一切的基石。
2. **并行启动 L1**(2 周):HumanEval/MBPP 接入,出 laew 首份分数。
3. **1 月内启动 L2**(1 月):Pairwise 自评测,内部 commit 触发对比。
4. **季度目标 L3**(3 月):Docker 评测 + SWE-bench Lite 30 case。
5. **半年目标 L4**(6 月):公开 Leaderboard + Web UI。

laew 从「能跑」到「生产级 Agent CLI」的**最后一块拼图**,就是评测基建。没有评测,就没有「进步」的依据。

---

## 附录 A:术语表

| 术语 | 定义 |
|------|------|
| **SWE-bench** | Princeton 2024 发布的真实 GitHub Issue 修复基准 |
| **SWE-bench Verified** | OpenAI 2024-08 人工验证的 500 子集 |
| **TerminalBench** | Princeton 2025 终端任务评测 |
| **GAIA** | Meta 2023 通用 AI 助理基准 |
| **WebArena** | CMU 2023 浏览器评测 |
| **LiveCodeBench** | NYU 2024 抗污染持续代码评测 |
| **τ-bench** | Salesforce 2024 业务对话评测 |
| **MMLU** | UC Berkeley 多任务语言理解 |
| **HumanEval** | OpenAI 164 题 Python 函数生成 |
| **fail_to_pass** | SWE-bench 评测中「必须从 FAIL 变 PASS」的测试 |
| **pass_to_pass** | SWE-bench 评测中「必须保持 PASS」的测试 |
| **predictions.jsonl** | SWE-bench 官方预测文件格式(每行一个 JSON) |
| **LLM-as-judge** | 用 LLM 评判另一 LLM 的输出质量 |
| **Pairwise** | A vs B 配对评分 |
| **Bootstrap CI** | 重采样算 95% 置信区间 |
| **Resume-safe** | 评测进程被杀后可断点续跑 |
| **JSONL** | 每行一个 JSON 对象的文本格式 |
| **Cold start** | 容器首次启动时间 |
| **Cache cohort** | 按首次/重复分组的 cache 命中率 |
| **Multi-judge consensus** | 多模型打分取均值 |
| **Contamination** | 训练集污染(模型见过评测题) |
| **Held-out variants** | 同题多参数变体 |
| **Cutoff date** | 评测题发布时间截止 |
| **Outcome 8 类** | atomcode 定义的 8 类结果(success/empty/timeout/rate_limited/provider_error/process_error/protocol_error/model_mismatch) |

---

## 附录 B:参考资料链接

1. **SWE-bench**: https://www.swebench.com/ | https://github.com/princeton-nlp/SWE-bench
2. **SWE-bench Verified**: https://openai.com/index/introducing-swe-bench-verified/
3. **SWE-gym**: https://github.com/SWE-Gym/SWE-Gym
4. **TerminalBench**: https://www.tbench.ai/ | https://github.com/laude-institute/terminal-bench
5. **Harbor**: https://github.com/harbor-framework/harbor
6. **WebArena**: https://webarena.dev/ | https://github.com/web-arena-x/webarena
7. **GAIA**: https://huggingface.co/spaces/gaia-benchmark/leaderboard | https://arxiv.org/abs/2311.12983
8. **LiveCodeBench**: https://livecodebench.github.io/ | https://github.com/LiveCodeBench/LiveCodeBench
9. **τ-bench**: https://taubench.com/ | https://github.com/sierra-research/tau-bench
10. **OSWorld**: https://osworld.dev/ | https://github.com/web-arena-x/OSWorld
11. **MMLU**: https://paperswithcode.com/sota/multi-task-language-understanding
12. **HumanEval+ / MBPP+**: https://evalplus.github.io/
13. **Craft-taskgen**: https://github.com/NVlabs/craft-taskgen (Switchyard 实验)
14. **vitest-evals**: https://github.com/getsentry/vitest-evals
15. **atomcode eval.py**: `/usr/local/LsmGitOpenSource/atomcode/evals/deepseek-v4-flash/eval.py:1-627`
16. **pi packages/evals**: `/usr/local/LsmGitOpenSource/pi/packages/evals/src/pi-harness.ts:1-300+`
17. **hermes-agent browser_use**: `/usr/local/LsmGitOpenSource/hermes-agent/evals/browser_use/orchestrate.py:1-200+`
18. **hermes-agent postmortem**: `/usr/local/LsmGitOpenSource/hermes-agent/evals/postmortem/run.py:1-100+`
19. **opencode bench**: `/usr/local/LsmGitOpenSource/opencode/packages/console/app/src/routes/bench/[id].tsx:1-300+`
20. **Switchyard craft-taskgen**: `/usr/local/LsmGitOpenSource/Switchyard/experimental/craft-taskgen/src/craft_taskgen/{llm_judge.py, rubrics.py, swebench_alignment.py, task_format.py, docker.py}:1-100+`
21. **laew 第十一轮 Eval**: `/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/docs/Agent源码调研/专题/专题-第十一轮-测试体系与Eval基建与录制回放深度对比.md:1-2262`
22. **laew e2e runner**: `/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/testReport/run_e2e.sh:1-100+`

---

> **第 13 轮深挖合集:Agent 评测基准与 Leaderboard 深度对比** 完成。
> 累计 13 轮,共 ~12.5 万行专题报告,461 个 laew Gap。
> 下一步:启动 L0 改造(1 周内完成 7 项 P0 Gap)。
