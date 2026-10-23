# 28 — eval_js 全局词法泄漏根治 / select_dropdown 动作型确认闭环 / Windows HITL 弹窗按钮白底白字修复

- 日期:2026-10-15
- 触发:闲鱼倒买倒卖任务实测(`D:\MyLocalGit\LsmBashDir\Dir02`,`llaew_20261015_182610.log`),wf-1.step 单元 3 次局部重试耗尽后人工终止;另含人工报障「HITL 弹窗按钮看不清楚都是白色」。
- 三个缺陷全部机制级根治,与具体站点无关。

## 一、eval_js 顶层词法声明跨调用泄漏(defect 1+2)

### 现象与根因

SubAgent iter 3 执行含 `const sleep=...` 的脚本成功,iter 5 再执行含同名 `const sleep` 的脚本即
`SyntaxError: Identifier 'sleep' has already been declared`(code 2002),模型改写函数形态
后成功——白烧 1 轮迭代。

根因:CDP `Runtime.evaluate` 在页面**全局词法环境**执行,顶层 `const/let/class` 声明创建的
全局词法绑定**跨 evaluate 调用常驻**。模型给临时变量起名高度趋同(`sleep`/`result`/`items`),
会话越长越容易踩中。

同轮发现的伴生缺陷:`act_eval_js`(用户面 eval_js)为支持 `await_promise` 自拼
EvaluateParams,**绕过了** `eval_js_string` 网关的裸 return 整形;且其自带的「失败自动包
IIFE 重试」是死代码——重试判定在 `EvaluateParams::build()` 上,build 不解析 JS 语法恒 Ok,
重试从未发生过。

### 方案(输入侧整形,零成本吸收)

- `eval_sanitize::shield_toplevel_lexical(js)`:扫描到顶层裸 `const/let/class` 声明且
  无顶层裸 `await` 时,整个脚本包进块级作用域 `{ <原文> }`——块级词法不泄漏,重复调用
  互不污染;块完成值 = 最后一条语句值,值语义不变;`var`/`function` 可重复声明不触发;
  纯对象字面量表达式(无词法关键字)不触发。
- 与既有 `wrap_bare_toplevel_return`(裸 return → IIFE)串联:裸 return 优先走 IIFE
  (自带作用域),词法屏蔽对其零扰动。网关 `eval_js_string` 与 `act_eval_js` 同语义。
- `act_eval_js` 死代码重试下沉为**求值期重试**:首次 `page.evaluate` 真失败且表达式
  非函数形态时,按 await_promise 包 async/普通 IIFE 再试一次,共享同一截止预算
  (不叠加整份 timeout)。
- 验证:新增单测 4 组(误伤/边界/保守口径/串联);新增真浏览器冒烟
  `mcp_web_use_eval_js_lexical_shield_smoke`(同页两次 `const sleep` 不再炸、值语义
  不变、window 赋值不受屏蔽影响),已实测通过。

## 二、select_dropdown 选后确认闭环缺口(defect 3)

### 现象与根因

闲鱼「区域」筛选 = ant-popover:选中省份(北京)后,面板内出现**动作型确认按钮**
「查看999+件宝贝」,必须再点它筛选才落到 URL。实测:

- SubAgent 用 eval_js 已探测到该按钮(iter 8 `popBtns:[{t:"查看999+件宝贝"}]`),但
  **迭代预算(10)恰好耗尽**,差一步未点 → QC 判「机制性阻塞」;
- 工具路径 `select_dropdown(trigger_text=区域, option_text=北京)` 两次尝试均
  「文本命中 0 个」——触发器点击后面板未挂 DOM(toggle 语义下首击可能把已开面板关回),
  3 次选项探测空转;
- `confirm_panel_js` 确认词表 `['确定','确认','完成','应用','OK','Ok','ok','保存']`
  不含动作型文案,即便选中也会静默跳过确认步。

### 方案

- `confirm_panel_js` 词表 + 正则双通道:新增 `CONFIRM_RE = [/^查看\s*[\d+]+\s*件/,
  /^查看全部/, /^(立即查看|去查看)$/]`;无显式 scope 时的「面板状容器」约束(fixed/
  absolute 祖先或 role=dialog 等)不变,防误点页面级按钮。
- 选项搜索循环第 2/3 次探测前**重踢一次触发器**(toggle 自愈),响应补
  `trigger_rekicked` 事实;连续两次重踢后面板状态与原始点击同相,不引入新抖动。
- 失败文案补级联提示(省份→城市两级面板传 `cascader_text` 选第二级)。
- 验证:新增 `confirm_panel_js_covers_action_style_buttons` 单测。

## 三、Windows HITL 弹窗按钮白底白字(defect 4,人工报障)

### 根因

`windows_theme.rs::draw_text_line` 只调 `SetTextColor` 未设背景模式。DC 默认
`OPAQUE` 模式 + 默认背景色纯白,`DrawTextW` 会**先把传入矩形整个用背景色擦白**再画字:
主按钮(蓝底白字)被整矩形刷白后白字不可见;次按钮同样被刷白——观感「按钮全是白色、
文字和背景一个色」。深/浅配色板下均成立(刷白色是 GDI 默认,与主题色无关)。

### 方案

- `draw_text_line` 绘制前 `SetBkMode(hdc, TRANSPARENT)`、绘制后恢复旧模式。
  自绘调用处均已先用 `fill_rounded_rect` 铺满控件面,透明模式无残影风险。
- `draw_switch_row` 补行底自绘(此前行底依赖 OPAQUE 擦除副作用遮默认按钮面;
  文字改透明模式后必须显式填充):整行按 Secondary 按钮形态(btn_bg 底 + 描边)填充。

## 四、回归

- `cargo test --lib`:2317 通过(含新增 5 组单测);
- `cargo test` 全量:单测 + 全部集成测试绿;
- `cargo test --test web_use_smoke mcp_web_use_eval_js_lexical_shield_smoke -- --ignored`:
  真浏览器实测通过;
- 弹窗观感:真机拉起 `laew __hitl-dialog` 目检(深/浅两档由系统外观决定)。
