# 20 · select_dropdown 选项优先与选后确认闭环(第 164 轮)

日期:2026-10-13 · 实现:`src/agent/tools/mcp_web_use/control/form_ctl.rs`
(`probe_element_js` / `confirm_panel_js` / `act_select_dropdown` 重写)
背景事故:`llaew_20261013_084832.log` 闲鱼任务 wf-1,**08:53:10–08:57:54 约 4.7 分钟
卡在「区域→北京」下拉选择**,最终如实放弃(选中态已置但筛选未生效)。

## 1. 事故还原

1. 模型 iter7 用 `click` 点开「区域」面板;
2. iter8 调 `select_dropdown(option_text=北京, trigger_text=区域)` —— 旧实现先去点
   触发器,**组件库 toggle 语义把已开的面板关回去**,后续选项探测全部落空(2002);
3. 模型 fallback `eval_js` 点中「北京」(`provItemActive` 类名已置 = 选中态),
   但 **areaText 仍回显「区域」、列表样本仍含河北/天津 —— 选择未生效**;
4. 模型 iter10–16 对同一 toggle 控件**反复盲点**(点偶数次 = 取消选中),4.7 分钟、
   6 轮 LLM 后放弃,并在产物文档中如实记录「区域筛选疑似仅对距离最近排序生效」。

三个工具层缺口:①面板已被前序动作点开时再去点触发器 = 反向事故;②选中选项后
没有「确定/完成」确认按钮通道;③选后没有回读事实(触发器文本变化与否),
模型只能靠盲试点对 toggle 控件。

## 2. 方案(通用性优先,非闲鱼特例)

### 2.1 选项优先(`probe_element_js`)
- 新增 find-only 变体 `probe_element_js`:与 `pick_element_js` 同一套
  匹配(叶子全文本/自身直接文本精确优先、小容器模糊兜底)与可见性语义
  (几何尺寸 + 计算样式),但**不做 scrollIntoView/click**,返回
  `{found, count, hint, tag, cls, text}`。
- `act_select_dropdown` 新流程:
  1. **先 probe 选项** —— 可见则直接 `pick_element_js` 点击,全程不碰触发器
     (面板已被前序动作点开时零浪费,也杜绝 toggle 关回事故);
  2. 选项未命中 → 点击触发器展开面板(此刻面板确实未开,点击安全);
  3. 随后选项搜索最多 3 次带等待探测(保留第 158 轮自愈);
  4. 仍失败错误携带诊断(文本命中数/可见数/预探测结论),一步定位。

### 2.2 选后确认按钮通道(`confirm_panel_js`)
- 选中选项后自动探测面板内确认类按钮并点击:
  - 确认词表:`确定/确认/完成/应用/OK/Ok/ok/保存`;
  - 显式 `selector` scope 时信任该范围,只要求可见;
  - 无 scope 时要求候选位于「面板状」容器内(8 层祖先内出现 fixed/absolute
    定位或 role=dialog/listbox/menu/tooltip/presentation)——防止误点页面其它
    表单的提交按钮;
  - 多候选取面积最小者,响应带 `confirm_clicked`/`confirm_text` 事实。

### 2.3 选后回读(应用事实,纯 advisory)
- `trigger_after`:选项点击后重新 probe 触发器原文 —— `found=false` 表示
  触发器文本已变化(多已回显选中值);仍 `found=true` 且筛选未生效时模型应换策略。
- `value_after`:面板 scope 文本回读(既有)。
- 响应新增 `applied_hint` 明示:**切勿对同一选项重复盲点(toggle 会取消选中)**,
  先看 `trigger_after`/`value_after` 事实再决策(切换排序/级联第二级/URL 参数)。

## 3. 边界与不做的事
- 不硬判「选后必须生效」:站点行为不可强制(闲鱼区域筛选疑似仅对「距离最近」
  排序生效属站点侧决策),如实上报即可;
- 不改变 `pick_element_js` 签名与语义(既有调用零改动);
- 级联 `cascader_text` 通道保持原样,确认按钮探测在第一级与级联全部完成后执行一次。

## 4. 测试
- `probe_element_js_is_find_only`:probe 不得含 click/scrollIntoView 副作用,
  双分支 found 标记、诊断语义、scope 限定;
- `confirm_panel_js_guards_unscoped_click`:词表、fixed/role 面板识别、
  显式 scope 走信任路径(`const scoped = true`)。
