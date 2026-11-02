# 天猫搜索区、Logo、骨架卡与绝对定位布局根因

## 背景

2026-10-30 天猫真机复测在 Fetch/Console 修复后，搜索框、分类和登录入口已能挂载，但窗口仍出现搜索区错位、Logo 撑高、商品卡片/表格区域异常。按用户要求停止任务后，使用完整动态 DOM 在禁用脚本与图片的本地 mock 中重排，区分**几何错误**与**接口无数据时的正常骨架**。

## 根因与修复

### 1. 搜索框 6.2px：脱流坐标进入 Flex 内在尺寸

`form{display:flex}` 中，`width:100%` 的 combobox 本应占满容器。绝对定位按钮在 MAX_CONTENT_WIDTH 测量中产生远端文本坐标，旧实现跳过 `ElementBox.out_of_flow`，却仍聚合对应 `TextLine`，导致 Flex 总基础宽异常，combobox 被 shrink 到 6.2259px。

修复：内在尺寸测量不发射 absolute/fixed 子树；正式布局照常发射。

### 2. Logo 652px：`height:0` 被当成 auto

真实 Logo 使用 `height:0;padding-top:72px;overflow:hidden` 的图像替换结构。样式层 `filter(>0)` 丢弃零高度，元素退回 `font-size:200px;line-height:290px` 的内容高度，再加 padding 变成 652px。

修复：width/height 接受合法零值；最终几何恢复 240×72。

### 3. 骨架卡 2016px：百分比 padding 预按视口求解

`.skeleton .img-wrapper{padding-top:100%;width:100%}` 用来维持正方形。旧实现把 100% 提前按 1920px 视口宽变成固定 padding，卡片文字区再叠加后总高约 2016px。

修复：ComputedStyle 保留四侧百分比，布局期按真实包含块宽求解；测量哨兵不当真实父宽。304px 卡片的图像区恢复约 304px，而不是 1920px。

### 4. 输入父盒 6.8px：middle 只计控件 descent

控件的 `vertical-align:middle` 只把 `h*20%` 计入行盒，父盒 `overflow:hidden` 因而裁掉大部分输入框。

修复：按「盒中心对齐基线 + 半个 x 高度」分别计算中心线以上/以下贡献，恢复完整 34px 行高。

### 5. 搜索按钮在输入框左侧：包含块错误 + 二次平移

旧定位使用最近任意父盒或 Flex 局部测量 cb，`right:2px` 被解释到零宽父盒；使用祖先全局坐标后又会随片段再次平移，非首 Flex 条目偏移更大。

修复：
- `position.rs` 沿 DOM 查最近定位/transform 祖先，并使用其已布局实际几何；
- `DisplayItem.out_of_flow` 与 `ElementBox.out_of_flow` 在片段平移时跳过脱流项，普通片段内容保持原行为。

## 验证

`tests/web_layout_regressions.rs` 用一份真实 CSS 最小模型覆盖：

- `#q` / combobox 宽 792px、行高 34px；
- Logo 高 72px；
- 比例骨架 304×304；
- 首个与非首 Flex 条目的 `right:2px` 都相对同一定位祖先，不重复平移。

完整天猫 DOM 复测：搜索框 6.2259→792px、Logo 652→72px、骨架卡 2016→约 400px（图像区 + 文本区），并生成最终截图。

## 数据缺失与布局问题必须分开

mtop 返回 `FAIL_SYS_ILLEGAL_ACCESS::非法请求` 且 `data:{}` 时，商品名称、价格、评论表本来就无法生成；灰色骨架不是“商品表格渲染成功”。只有 DOM 中存在商品文本/表格而几何或像素异常时，才继续查布局/渲染。不得用占位数据伪装成商品数据已加载。
