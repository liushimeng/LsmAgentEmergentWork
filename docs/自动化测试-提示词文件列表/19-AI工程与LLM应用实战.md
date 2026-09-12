# 自动化测试提示词 — AI 工程与 LLM 应用实战（S01–S10）

> 使用说明见同目录 `README.md`。每条提示词在同一 Session 内按轮次顺序喂入。

## 维度说明

本维度考察 Agent 在**无真实 API Key / GPU / 外网**前提下，把"LLM 客户端 / 流式解析 / RAG 检索 / Agent 循环 / Embedding / Token 估算 / Prompt 缓存 / 网关路由 / Eval"等 AI 工程任务，做成可 Read 可 Bash 的真实可执行产物：本地起 mock HTTP 服务（python `http.server` + 假 SSE chunk）让被测代码 curl 它，再用 Bash + `jq`/grep 对 mock 响应做断言——考察端到端链路（写代码 → 跑 → 印证 → 修复闭环）。

---

### S01 
- **测试状态**: ✅ 已测试(2026-09-12 Windows 第 49 轮 mock 批量回归,router 四轮工具链全过;生成器 TestWorkSpace/gen_round49*.py,详见 tmpPlan/2026-09-12_第49轮S-GR-GQ批量回归与优化报告.md)
用 Python 写一个 OpenAI/Anthropic 协议双适配 LlmClient
- **预期档位**: hard
- **考察维度**: 协议抽象 / 多轮产物依赖 / Bash 验证闭环
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/s01/` 下写 `mock_anthropic.py`：`http.server` 起 127.0.0.1:18101，`POST /v1/messages` 返回固定 JSON `{"content":[{"type":"text","text":"hello-anthropic"}],"usage":{"input_tokens":7,"output_tokens":3}}`；再写 `mock_openai.py` 同端口路径 `/chat/completions` 返回 `{"choices":[{"message":{"content":"hello-openai"}}]}`。后台各 `&`，写 `verify_mock.sh` `curl -sf` 两个端点都得 200 且文本命中，跑完 `kill $PID1 $PID2`。
  2. 写 `client.py`：定义 `LlmClient` 抽象类（`complete(messages) -> str`），两个子类 `AnthropicClient`(走 `/v1/messages` + `x-api-key` 头) / `OpenAIClient`(走 `/chat/completions` + `Authorization: Bearer` 头)，由环境变量 `LLM_PROVIDER=anthropic|openai` 切换 endpoint；用 `urllib.request` 不许 `pip install`。
  3. 写 `run.sh`：`LLM_PROVIDER=anthropic python client.py` 应打印 `hello-anthropic`，切到 `openai` 打印 `hello-openai`；再故意把 mock 端口写错（比如 `18199`），跑 `run.sh` 期望退出码非 0 且 stderr 含 "Connection refused"。
  4. 加上流式：`mock_anthropic.py` 收到 `stream:true` 时返回 3 个 SSE chunk `event: message\ndata: {"delta":{"type":"content_block_delta","text":"hel"}}\n\n` 等；`client.py` 加 `stream_complete` 用 `urllib` 逐行读 chunk 并拼接；写 `verify_stream.sh` 断言拼接结果恰为 `"hel"+"lo"+"! "` (假定 mock 三个 chunk)，丢一 chunk 必须 `FAIL` 退出 1。

### S02 
- **测试状态**: ✅ 已测试(2026-09-12 Windows 第 49 轮 mock 批量回归,router 四轮工具链全过;生成器 TestWorkSpace/gen_round49*.py,详见 tmpPlan/2026-09-12_第49轮S-GR-GQ批量回归与优化报告.md)
用 Python 写一个最小 RAG：分块 + numpy 向量 + 余弦 top-k
- **预期档位**: medium
- **考察维度**: 多文件产物 / Bash 闭环 / 数值断言
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/s02/` 下写 `corpus.txt`（≥10 行短句，每行一个"事实"，覆盖"猫/狗/咖啡/茶/鸟/鱼"等关键词），再写 `build_index.py`：按 3 行滑窗分块（步长 1），每块用**纯 numpy** 写一个最简 bag-of-words 向量（词表来自 corpus 全部词），保存到 `index.npz`（`np.savez`）。
  2. 跑 `python build_index.py && ls -la index.npz` 必须文件存在且 `python -c "import numpy as np; d=np.load('index.npz'); print(d['vecs'].shape)"` 输出形如 `(8, N)`（N 是词表大小）；写 `verify_index.sh` 把 shape 写进 `shape.out`，行数恰为 1。
  3. 写 `query.py`：输入查询字符串，分词后做同样词表→向量、用 numpy 算余弦相似度（`a@b / (norm(a)*norm(b)+1e-9)`），输出 top-3 块的原文与相似度；写 `verify_query.sh`：`echo "我想喝咖啡" | python query.py | tee q.out` 应在 q.out 里命中包含"咖啡"字的行，且 top-1 相似度 > top-3 相似度（用 `awk` 取数值比较）。
  4. 故意把 cosine 分母写成 `norm(a)*norm(b)`（无 epsilon），用空查询跑应不 NaN：写 `verify_robust.sh` `echo "" | python query.py` 期望退出码 0 且输出含 "top-1" 字样；若 NaN 出现（`grep -c nan q.out` ≥1）则视为失败，请 Read `query.py` 加回 epsilon 后重跑。

### S03 
- **测试状态**: ✅ 已测试(2026-09-12 Windows 第 49 轮 mock 批量回归,router 四轮工具链全过;生成器 TestWorkSpace/gen_round49*.py,详见 tmpPlan/2026-09-12_第49轮S-GR-GQ批量回归与优化报告.md)
Prompt 工程：写一个 prompt 单元测试框架
- **预期档位**: medium
- **考察维度**: Bash 闭环 / JSON Schema / 多轮产物
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/s03/` 下写 `mock_llm.py`（同 S01 风格）：`POST /complete {prompt}` 返回 JSON `{"text":"<根据 prompt 关键词拼接的固定回应>"}`——本条 mock 会读 prompt 关键词判断"是 few-shot 还是 zero-shot"再回不同文本；后台起在 18201，`curl` 探活。
  2. 写 `prompts/` 下两个文件 `zero_shot.txt` 和 `few_shot.txt`（few_shot 含 3 条示例"输入/输出"），再写 `runner.py`：读 prompt 文件 + 测试用例 JSON `cases.json`，对每条用例拼 `prompt + "\n\n" + input` 发到 mock、断言 mock 返回的 JSON `text` 与期望 `expect_substring` 子串匹配（用 `in` 判断），失败累计到 `report.json`。
  3. 跑 `python runner.py cases.json`，期望 `report.json` 里 `passed==total` 且 `failed==0`；故意改 cases.json 一条 `expect_substring` 为不可能字符串，再跑期望 `failed==1`，输出 `report.json` 后 `jq -e '.failed == 1'` 必须成功；最后改回再跑一次确认变回 0。
  4. 加 CoT 模板 `cot.txt`，mock 升级为识别关键词 "step" 时返回分步文本（如"Step1: ... Step2: ..."）；写 `verify_cot.sh`：`runner.py --mode cot cases_cot.json` 后 `report.json` 必须 `passed==3` 且 `jq -e '.steps_avg >= 2'`（用 mock 返回中 `Step` 出现次数/用例数），失败请 Read mock 修复分步逻辑重跑。

### S04 
- **测试状态**: ✅ 已测试(2026-09-12 Windows 第 49 轮 mock 批量回归,router 四轮工具链全过;生成器 TestWorkSpace/gen_round49*.py,详见 tmpPlan/2026-09-12_第49轮S-GR-GQ批量回归与优化报告.md)
Agent 循环：ReAct 最小复刻 + 工具注册表
- **预期档位**: hard
- **考察维度**: Agent 循环 / 工具注册 / Bash 闭环
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/s04/` 下写 `mock_llm.py`：根据历史消息拼接 prompt，识别到关键词 `Action: add(1,2)` 就返回 `Action: add(1,2)\nObservation: <等待工具结果>`，识别到 `Final:` 就停；后台 18301，`curl` 探活。
  2. 写 `tools.py`：实现 `add(a,b)` `mul(a,b)` 两个工具，`ToolRegistry` 暴露 `register(name, fn, schema)` 与 `call(name, args)`（参数用 `jsonschema` 风格手写校验：类型/必填字段/数值范围，校验失败抛 `ValueError`）；写 `test_tools.py`：`python -m unittest test_tools` 跑 4 条用例（合法 add、缺字段、类型错、超范围）全过。
  3. 写 `agent.py` 实现 ReAct 循环：最多 5 步，每步构造 prompt "Question: ... History: ..." 发给 mock，若回复含 `Action:` 调 registry 并把 Observation 拼回历史，含 `Final:` 跳出；写 `run_case.sh` 用问题 "1+2*3"，mock 故意按"先 add(1,2)=3 再 mul(3,3)=9"返回，期望 agent 最终打印 `Final: 9`。
  4. 加错误恢复：工具返回 `ValueError` 时，prompt 应自动追加 `Error: ... please retry` 并允许继续；写 `run_fail.sh` 让 mock 第一次故意给 `Action: add("a","b")`（错类型），agent 必须 catch 后追加 Error 并继续下一轮拿到 `Final:`；若 agent 直接崩退出 0 视为失败，请 Read `agent.py` 修复异常分支重跑。

### S05 
- **测试状态**: ✅ 已测试(2026-09-12 Windows 第 49 轮 mock 批量回归,router 四轮工具链全过;生成器 TestWorkSpace/gen_round49*.py,详见 tmpPlan/2026-09-12_第49轮S-GR-GQ批量回归与优化报告.md)
模型量化感知：用 numpy 模拟 INT8/INT4 量化的精度损失
- **预期档位**: medium
- **考察维度**: 数值精度 / Bash 闭环 / 多轮产物
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/s05/` 下写 `gen_weights.py`：用 `numpy.random.randn(1024, 1024).astype(np.float32)` 生成伪"权重矩阵"保存到 `w_fp32.npy`；写 `quantize.py` 实现对称 INT8 量化（`scale = max(|w|)/127`, `q = clip(round(w/scale), -128, 127)`, `dq = q*scale`）和 INT4（`-8..7` 同理）；脚本接受 `--mode fp32|int8|int4`，输出 `w_<mode>.npy`。
  2. 跑 `python quantize.py --mode int8 && python quantize.py --mode int4`，`ls w_*.npy` 至少 3 个文件；写 `verify_quant.sh` 用 `python -c "import numpy as np; ..."` 计算 `w_fp32` 与 `w_int8` / `w_int4` 的均方误差 MSE，写到 `mse.out`，期望 int8 的 MSE 显著小于 int4（`awk` 比较两个数）。
  3. 加"矩阵乘法验证"：`python matmul.py` 加载 `w_fp32` 与 `w_int8`/`w_int4` 分别与同一个随机向量 `x`（`np.random.randn(1024).astype(np.float32)`）做点积，比较结果相对误差；写 `verify_matmul.sh` 断言 `|out_int8 - out_fp32| / |out_fp32|` < `|out_int4 - out_fp32| / |out_fp32|`（绝对值），输出到 `relerr.out`。
  4. 写 `report.md` 把 MSE/相对误差两张表用 markdown 表格写进去；最后 `python report.py` 自动生成（不许手写）必须包含 `| mode | mse | relerr |` 表头与三行数据，列数对齐（`awk -F'|' '{print NF}' report.md` 第二行 NF 应为 4）。

### S06 
- **测试状态**: ✅ 已测试(2026-09-12 Windows 第 49 轮 mock 批量回归,router 四轮工具链全过;生成器 TestWorkSpace/gen_round49*.py,详见 tmpPlan/2026-09-12_第49轮S-GR-GQ批量回归与优化报告.md)
多模态：用 PIL+numpy 模拟"图片预处理→视觉 token 计费"
- **预期档位**: medium
- **考察维度**: 图像处理 / 数值断言 / Bash 闭环
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/s06/` 下写 `gen_image.py` 用 `PIL.Image` 生成两张渐变图 `a.png` (256x256 红→蓝) `b.png` (512x512 棋盘格黑白)；写 `preprocess.py`：把图片 resize 到最长边 1568、JPEG 质量 85 保存到 `a_small.jpg` / `b_small.jpg`，计算 vision token = `ceil(W/512) * ceil(H/512)`（Anthropic 计费启发式），输出 JSON `tokens.json`。
  2. 跑 `python preprocess.py && cat tokens.json`，期望 `a` 的 token = `ceil(256/512)*ceil(256/512)=1`，`b` 缩到 1568 后 token = `ceil(1568/512)*ceil(1568/512)=3*3=9`（`jq -e '.a==1 and .b==9'` 必须通过）；若不过，请 Read `preprocess.py` 修 ceil 逻辑。
  3. 加"双图对比"：`compare.py` 计算两张图的逐像素均方根误差（`np.sqrt(((a.astype(float)-b.astype(float))**2).mean())`）作为"视觉相似度"，并按 token 加权估算"对比请求成本"（`cost = (tokens_a + tokens_b) * 0.003`），输出 `compare.json`；写 `verify_compare.sh` 期望 `cost > 0` 且 `rmse > 0`。
  4. 加"压缩对比"：再生成 `a_png` (不压缩 PNG) 与 `a_small.jpg`，`ls -la a_small.jpg a.png` 中 jpg 体积应小于 png（`awk` 比较数字）；若不满足说明 PIL 默认 PNG 反而更小，请加 `optimize=True` 重跑验证。

### S07 
- **测试状态**: ✅ 已测试(2026-09-12 Windows 第 49 轮 mock 批量回归,router 四轮工具链全过;生成器 TestWorkSpace/gen_round49*.py,详见 tmpPlan/2026-09-12_第49轮S-GR-GQ批量回归与优化报告.md)
Token 成本控制：写一个 Token 估算器 + 压缩触发器
- **预期档位**: medium
- **考察维度**: 估算算法 / 阈值触发 / Bash 闭环
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/s07/` 下写 `estimator.py`：实现两个估算函数 `by_chars(s)`（`len(s)//4 + len(s)//10` 即字符/4 +10% buffer）与 `by_words(s)`（按空格分词 ×1.3），返回 int；写 `test_estimator.py` `unittest` 跑 ≥4 个用例（全英文/全中文/混合/空字符串），`python -m unittest test_estimator -v` 全过，输出含 "OK"。
  2. 写 `trigger.py`：给定 `context_max = 800000` 和 `current_tokens`，若 `current_tokens / context_max >= 0.8` 返回 `"compress"`（同时推荐压缩率 50%），若 `>= 0.95` 返回 `"aggressive:20%"`，否则 `"ok"`；写 `verify_trigger.sh` 用三组输入 `700000/700001/799999` 跑，结果分别应为 `ok/compress/aggressive`，`tee` 到 `trigger.out`。
  3. 加"压缩模拟"：`compress_sim.py` 接受一段长文本和目标比例 `0.5`，做"保留首段系统提示 + 末 4 条消息 + 中间段落截断到 N 字符"的硬截断（不调 LLM），保存到 `compressed.txt`；写 `verify_compress.sh` 输入一个 10000 字符的生成文本（heredoc），期望输出长度 ∈ [4500, 5500]，且**首 200 字符必须等于输入首 200 字符**（`head -c 200 compressed.txt` 与原文件 diff 必须为空）。
  4. 写一个"溢出兜底"测试：`mock_400.py` 起 127.0.0.1:18701，对 `POST /complete` 返回 400 + `{"error":"prompt is too long"}`；`retry.py` 收到此错误时连续三次降级（截短→折叠→暴露），写 `verify_overflow.sh` 跑后 `retry.out` 含 "attempt1:truncate / attempt2:fold / attempt3:expose" 三行（任一缺失即 fail）。

### S08 
- **测试状态**: ✅ 已测试(2026-09-12 Windows 第 49 轮 mock 批量回归,router 四轮工具链全过;生成器 TestWorkSpace/gen_round49*.py,详见 tmpPlan/2026-09-12_第49轮S-GR-GQ批量回归与优化报告.md)
向量检索：用 numpy 写一个混合检索（BM25 + 向量 + RRF）
- **预期档位**: hard
- **考察维度**: 多路检索 / 融合算法 / Bash 闭环
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/s08/` 下写 `corpus.jsonl`（≥20 行，每行 `{"id":i,"text":"..."}`，覆盖"咖啡/茶/猫/狗/汽车/火车"主题），写 `build_bm25.py`：手写 BM25（`tf* (k1+1)/(tf+k1*(1-b+b*dl/avgdl))`，k1=1.5 b=0.75），对每条 doc 算 token→idf 字典保存到 `bm25.pkl`（用 pickle 或 JSON 序列化）。
  2. 跑 `python build_bm25.py && ls -la bm25.pkl`，期望 `python -c "import pickle; d=pickle.load(open('bm25.pkl','rb')); print(len(d['idf']))"` 输出 ≥10 个词；写 `verify_bm25.sh` 把 idf 词典大小写到 `bm25.out`。
  3. 写 `hybrid.py`：`query="我想喝咖啡看小狗"` 同时跑 BM25 top-5 与向量 top-5（向量复用 S02 的 numpy 余弦），用 RRF `score = sum(1/(60+rank))` 融合两路，给出最终 top-3；`python hybrid.py > hybrid.out`，期望 top-3 中至少含 1 条咖啡相关 + 1 条狗相关（用关键词 `grep` 命中）。
  4. 加"消融对比"：`ablation.py` 分别跑纯 BM25 / 纯向量 / 混合三路，输出三路 top-3 的 id 与 score 写到 `ablation.json`；`verify_ablation.sh` 用 `jq -e '.hybrid[0].score > .bm25[0].score and .hybrid[0].score > .vector[0].score'`（启发式：融合分应不低于任一单路），失败请 Read `hybrid.py` 调整 RRF k 或重排逻辑重跑。

### S09 
- **测试状态**: ✅ 已测试(2026-09-12 Windows 第 49 轮 mock 批量回归,router 四轮工具链全过;生成器 TestWorkSpace/gen_round49*.py,详见 tmpPlan/2026-09-12_第49轮S-GR-GQ批量回归与优化报告.md)
LLM 网关：用 Python 写一个多 Provider 故障转移网关
- **预期档位**: hard
- **考察维度**: 网关路由 / 熔断 / 故障转移闭环
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/s09/` 下写 `mock_p1.py` 127.0.0.1:18901 返回 200 "from-p1"；`mock_p2.py` 18902 返回 200 "from-p2"；`mock_p3.py` 18903 永远返回 500；后台起三个进程存 PID。写 `gateway.py`：按顺序尝试 providers，连续 5 次失败熔断该 provider 60 秒（用内存字典 `circuit[provider] = unblock_ts`）。
  2. 跑 5 次 `curl mock_p3` 让它累计 5 次失败（`verify_open.sh` 直接 `python gateway.py --provider p3 5 次` 全 500），第 6 次 gateway 必须跳过 p3 直接返回 p1 或 p2，写 `verify_circuit.sh`：`python gateway.py --provider p3` 第 6 次后 stderr 含 "circuit open: p3" 且响应来自 p1/p2（`grep -F from-p` 命中 p1/p2 之一）。
  3. 加半开探测：写 `verify_halfopen.sh`，等 60 秒（缩短可改 `gateway.py` 把熔断时长设 2 秒跑测试），重置 p3 mock 为 200 再请求一次，gateway 应在 half-open 状态放一次探测，探测成功关闭熔断（`grep -F "circuit closed: p3"` 命中）；失败请 Read `gateway.py` 修复状态机重跑。
  4. 写 `verify_end.sh` 完整跑一遍：3 个 mock 跑起 → gateway 跑 8 次（3 次走 p3 失败 → 触发熔断 → 后续走 p1/p2）→ 写 `summary.json` `{p1_calls, p2_calls, p3_calls, total}`，`jq -e '.p3_calls <= 2 and (.p1_calls + .p2_calls) >= 6'` 通过；最后 `kill` 三个 PID。

### S10 
- **测试状态**: ✅ 已测试(2026-09-12 Windows 第 49 轮 mock 批量回归,router 四轮工具链全过;生成器 TestWorkSpace/gen_round49*.py,详见 tmpPlan/2026-09-12_第49轮S-GR-GQ批量回归与优化报告.md)
LLM 应用评估：写一个 LLM-as-Judge 评分器 + 回归集
- **预期档位**: medium
- **考察维度**: Eval 体系 / 多轮产物 / Bash 闭环
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/s10/` 下写 `judge_mock.py` 127.0.0.1:19001，POST `/judge {candidate, reference}` 返回 5 个维度分数 JSON `{accuracy, relevance, fluency, safety, helpfulness}`（每个 1-5，由 mock 根据 candidate 关键词"good/bad/error"启发式打分）；后台探活。
  2. 写 `eval_set.jsonl`（≥6 行，覆盖"闲聊/知识/代码/翻译/摘要/工具调用"6 类），每行含 `candidate` `reference` `expected_min`；写 `judge.py` 遍历集合调 mock、累加每类平均分，输出 `eval_report.json`；`python judge.py && jq -e '.per_category | length == 6'` 必须通过。
  3. 加"位置偏差校正"：mock 默认顺序打分，但 `judge.py` 加 `--swap` 把 candidate/reference 互换重打一次，取两次均值；写 `verify_swap.sh`：`python judge.py --swap > report2.json`，期望两报告中每类均分差 ≤ 1.0（`jq -e '...'` 数值比较），超差请 Read `judge.py` 加位置均衡重跑。
  4. 写一个回归脚本 `regress.sh`：跑 `judge.py` 两次（正常/交换）→ 合并 → 与上次 `baseline.json` 对比，若任一类别均分跌幅 ≥ 1.0 则 exit 1（视为模型退化）；故意把某行 `expected_min` 改到 5，再跑 `bash regress.sh` 期望 exit 1 且 `regress.out` 含 "FAIL category=code"。