# 44-真实输入通道与跨工具参数对齐与 HITL 补齐(2026-10-24)

> 实测方式:`laew --debug -f prompt_baidu_route.md`,工作目录 `D:\MyLocalGit\LsmBashDir\Dir_Chrome_01`。
> 任务:百度地图公共交通路线规划(沙河镇民院小区 → 环球影城度假区)+ 截图 + `出行计划-01.md`。
> 结果:三次执行(1 首发 + 2 局部重试)均卡在「输入已写入但搜索无结果」,任务未达成,终止测试后根因定位与修复。
> 前篇:`41-双浏览器工具实测结论与百度地图场景对比.md`(F1~F5);本篇是同场景的第二轮实测缺陷闭环。

## 一、结论速览

| # | 缺陷 | 根因 | 修复 |
|---|------|------|------|
| G1 | 输入框有字但站点认为没输入,搜索恒无结果 | `type_text` 走 JS setter + 合成 `Event('input')`,重 JS 站点内部输入模型只认可信输入流 | 新增 `type_text_real`:CDP `Input.insertText` + 逐字符 `rawKeyDown/keyUp`;JS setter 降级保留 |
| G2 | `eval_js` 传 `expression` 直接 1001 | Chrome 只认 `params.script`,Lsm 认 5 别名;双工具参数命名不互通 | `eval_script_arg` 五别名对齐(`script/expression/function/js/code`) |
| G3 | 登录墙无法人工介入 | Chrome 工具 `control_action` 枚举缺 `request_human`(Lsm 有完整实现) | 抽 `human_assist::request_human_action` 公共层,Chrome/Lsm 共用 |
| G4 | UI 单元被催早收口(第 4 轮禁 inspect) | `explore_budget = max_iter/4` 对 UI 任务过紧 + 文案硬禁令 | UI 角色预算放宽到 `max(公式, n/2)`;文案软化为「优先写操作,验证类 inspect 允许」 |
| G5 | `wait(ms+selector)` 语义混乱 | selector 分支静默吞掉 `ms` | 定稿「先 settle 后等就绪」,信封回显 `mode` |
| G6 | 背景陈述被升格为硬验收项 | 拆解层把「登录后才有…」写进 expected_output,QC 硬门判 Fail 空转两轮 | 拆解规则补「背景陈述不进验收标准」 |
| G7 | 旧 page_id 一次 2004 往返 | `list` 列出后 7s 页失效,报文无存活页清单 | `resolve_page` 失效时内联当前存活页 |

## 二、G1 真实输入通道(本轮核心)

### 现象

`input_text` 对 `.route-start-input` / `.route-end-input` 返回 `hit:true`,DOM `value`
读回正确文本,但百度地图 43 次探测 `hasMinute=false`,页面停在「请选择准确的起点、
途经点或终点」歧义态;联想下拉不出(`click_text("民院小区")` → `text_not_found`)。

### 根因

`chrome_browser::type_text` 的实现是「原生 setter 赋值 + `dispatchEvent(new Event('input'))`」。
现代重 JS 站点的输入框维护自己的内部模型(联想检索、校验、提交有效位),只认
**可信输入事件流**(`isTrusted=true` 的 beforeinput/input/keydown/keyup);合成事件
不更新其内部状态——文本框「看起来有字」,应用层认为「没有输入」。

### 修复(`src/agent/chrome_browser/page.rs`)

1. `type_text_real`:滚动入视野 → 聚焦并清空旧值 → `Input.insertText` 整段插入
   (IME 语义,产生可信 beforeinput/input)→ 逐字符补发 `rawKeyDown/keyUp`
   (**不带 `text`**),覆盖联想/快捷键监听;**不自动回车**,提交时机由模型用
   `key_press(Enter)` 显式决定(避免误选联想项/误提交表单)。
2. `type_text_ex` 改为双通道:先 `type_text_real`(信封 `mode=cdp_insert_text`),
   失败降级 `type_text`(信封 `mode=js_setter`),行为向后兼容。
3. 逐字符上限 64,防异常长粘贴把 CDP 打成 O(n) 风暴。

### 短回归发现的两处伴生缺陷(已修)

- **输入翻倍**:逐字符 `keyDown` **带 `text`** 会再插一次字符(CDP 语义:keyDown
  的 text 字段会触发文本插入),与 `insertText` 叠加导致 value 是两遍文本
  (实测 `value="…民院小区…民院小区"`)。改用 `rawKeyDown`(只触发 keydown 监听,
  不插字)后正常。
- **`inspect(text)` 读不到表单控件文本**:`element_text` 走 `innerText`,INPUT/
  TEXTAREA 的 innerText 恒为空(实测 `.route-start-input` 回 `chars:0`),模型无法
  核对「输入是否写进去」。改为 INPUT/TEXTAREA 读 `value`、SELECT 读选中项 text。

### 短回归结果(2026-10-24,regress-input-01.md,修复后二轮)

| 检查项 | 修复前 | 修复后(二轮回归) |
|--------|--------|--------|
| input_text 返回 | `{hit:true}`(无通道标识) | `{hit:true, mode:"cdp_insert_text"}` |
| 输入框文本 | 双写(首轮)/读不到(innerText 恒空) | **逐字一致 10 字**,`inspect(text)` 读 `value` |
| 联想下拉 | 不出(`text_not_found`) | **`.ui3-suggest-wrap` 可见,10 条候选** |
| 站点感知输入 | 43 次 `hasMinute=false` | 联想面板即时响应 |

## 三、G2/G3 跨工具一致性

### G2 eval_js 别名

Lsm(`browser/api.rs`)接受 `["expression","script","function","js","code"]`;
Chrome 只认 `script`。实测模型两次按 Lsm 习惯传 `expression` → 1001,各浪费一轮。
修复:`eval_script_arg` 按同序取首个非空值,错误文案点明别名集。

### G3 request_human

QC 建议「未登录则请人工完成登录」,模型调 `control(request_human)` 被枚举校验拒绝
(「字段 'control_action' 取值不在允许范围内」)——Chrome 侧 HITL 断路,只能硬写
「未登录」结论。修复:`human_assist::request_human_action` 公共入口(reason 白名单 /
超时分档 / 取消置旗),Chrome `control_action` 枚举加 `request_human`,page_id 可选
(无页走任务级提问),与 Lsm 语义一致。

## 四、G4 explore_budget 与文案

SubAgent `max_iterations=16` → `explore_budget=4`;第 4 轮即注入「禁止再开新
inspect/screenshot/eval_js」。模型 thinking 明确被该 hint 束缚(「The hint forbids
more inspect…」),登录态核验、路线入口探查被迫收口 → QC 判 Fail → 重试。

修复(二处):
1. `agent_loop.rs`:UI 角色(`HintRole::Ui`,按 trace 实际动过浏览器/桌面工具判定)
   预算放宽到 `max(公式预算, max_iter/2)`。
2. `runtime_hints.rs` Ui 文案:「禁止」→「**优先**推进写操作;inspect 仅用于验证
   刚做的写操作或确认关键状态」。

## 五、G5 wait 复合参数

模型写 `{ms:3000, selector:..., timeout_ms:15000}` 表达「先 settle 再等就绪」,
旧实现 selector 分支吞掉 `ms`。定稿:两者同给时**先 `wait(ms)` 再 `wait_for_selector`**,
信封回显 `mode: settle_then_selector | selector | fixed`。

## 六、G6 拆解背景项

用户原文第 1 条「登录后的网站,才有一些复杂功能」是背景陈述,Main-Work 拆解把
「登录状态已确认(已登录,或已在页面内完成登录流程)」写进 expected_output,
QC 按硬门判 Fail,引发两轮重试(约占总耗时 40%)。修复:`plan_prompt.rs` 重要规则补
「验收标准只收用户明确要求的交付物;背景陈述/经验说明不得升格为硬验收项,
可在未决阻碍里标注为观察项」。

## 七、G7 与 P2 收尾

- `resolve_page`:page_id 失效时在 2004 报文里**内联当前存活页**,省掉一次
  `action=list` 往返(实测 list 后 7s 页失效,模型拿着旧 id 空转一轮)。
- `hint_role` 注释补列 `MCP_Web_Chrome_Use`(代码原本正确,注释漏列)。
- inspect 信封顶层语义键统一 `result`(与 control 对齐),`data` 保留兼容字段。
- 测试互斥:改 `LAEW_CHROME_USE` 的用例与读开关的用例加 `ENV_LOCK` 串行
  (并行下 4030 污染 1001 断言,实测已翻车)。

## 八、验收

| 项 | 验收 | 状态 |
|----|------|------|
| G1 | 百度地图起终点键入后联想/可提交,路线面板出现「分钟」 | 单测 + 短回归 |
| G2 | `params.expression` 等效 `params.script` | 单测覆盖 5 别名 |
| G3 | Chrome 可发起 HITL(reason 白名单共享) | 单测 + schema 断言 |
| G4 | UI 单元不再被硬禁 inspect | 单测断言无「禁止」 |
| G5 | 复合参数信封回显 mode | 实现定稿 |
| G6 | 背景陈述不进验收标准 | 提示词级规则 |
| G7 | 2004 内联存活页 | 实现定稿 |

回归:`cargo test --lib` 2444 通过;`web_use_engine` 19 通过;`protocol_wire` 14 通过;
`agent_loop` 9 通过。
