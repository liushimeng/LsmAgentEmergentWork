// laew 人工介入弹窗 —— macOS 默认脚本(JXA + AppKit 自绘 NSWindow,第 132 轮重写)
// 用法: osascript -l JavaScript <本脚本> <payload.json>
// 动态加载: {工作目录|根目录|~}/.laew/human_ui/macos.js 可整体替换本脚本(每次呈现重读,改完即生效)
// stdout 输出结果 JSON: {"status":"answer|cancel|timeout|error","text":"..."}
// 设计见 docs/MCP_Web_Use/03-人工介入弹窗UI动态加载方案.md(第 132 轮重写章节)
//
// ===== 第 132 轮为什么弃用 NSAlert+runModal(实测四宗罪) =====
// 1. informativeText 是静态标签,鼠标无法选中/复制;
// 2. 附件 NSTextField 从未成为 first responder → 看着有焦点但打不进字,输入法无效;
// 3. runModal 会话期间主 run loop 跑 NSModalPanelRunLoopMode,而 scheduledTimer
//    注册在 default mode → 倒计时不刷新、超时自灭失效;
// 4. 后台代理进程(osascript)下 NSAlert 模态事件路由不稳,输入框快速连点会提前
//    结束模态会话 → 弹窗误关(被判为 cancel)。
// 新方案:自绘 NSWindow + ObjC.registerSubclass 控制器 + NSApp.run()(default mode,
// 定时器/IME 全部正常),退出用 stop + 投递空事件唤醒(标准 trick)。

ObjC.import('Foundation');
ObjC.import('AppKit');

function pad2(n) { return (n < 10 ? '0' : '') + n; }
function fmtClock(d) {
  return pad2(d.getHours()) + ':' + pad2(d.getMinutes()) + ':' + pad2(d.getSeconds());
}
function fmtDur(ms) {
  var s = Math.max(0, Math.floor(ms / 1000));
  var m = Math.floor(s / 60);
  var r = s % 60;
  return m > 0 ? (m + ' 分 ' + pad2(r) + ' 秒') : (r + ' 秒');
}

// 控制器:选项按钮 / 输入框回车提交 / 取消 / 超时统一走这里收口。
// 注:本机 osascript 的 registerSubclass 返回 undefined(实测),类需经
// `$.LaewHitlController`(即 ObjC.classes)取用 —— 两步分开,兼容两种实现。
ObjC.registerSubclass({
  name: 'LaewHitlController',
  methods: {
    // 选项按钮(sender.tag = 选项下标)与「提交」按钮(sender.tag = -1)
    'submit:': {
      types: ['void', ['id']],
      implementation: function (sender) {
        finish('submit', sender);
      },
    },
    // 输入框回车(NSTextField action)与「取消」按钮(Esc 等价键)
    'cancel:': {
      types: ['void', ['id']],
      implementation: function (sender) {
        finish('cancel', sender);
      },
    },
  },
});
var LaewHitl = $.LaewHitlController;

// ---- 供控制器闭包访问的可变状态(run() 内赋值) ----
var g = {
  input: null,      // NSTextField
  options: [],
  result: null,     // {status, text}
  done: false,
  timeoutMs: 120000,
  startedAt: new Date(),
  tickTimer: null,
  countdownLabel: null,
};

// 收口:把用户动作映射为结果 JSON 语义,停 run loop。
// - submit + 输入框有字 → answer(文本);无字 + tag>=0 → answer(选项 N);
//   无字 + 提交按钮(tag=-1)→ answer(选项 1,对齐旧行为「空回车=选项1」);
// - cancel → cancel;
// - timeout → timeout。
function finish(kind, sender) {
  if (g.done) return;
  g.done = true;
  var typed = '';
  try { typed = String(g.input.stringValue.js || '').trim(); } catch (e) {}
  if (kind === 'timeout') {
    g.result = { status: 'timeout', text: '' };
  } else if (kind === 'cancel') {
    g.result = { status: 'cancel', text: '' };
  } else {
    var idx = -1;
    try { idx = Number(sender && sender.tag !== undefined ? sender.tag : -1); } catch (eTag) {}
    if (typed.length > 0) {
      g.result = { status: 'answer', text: typed };
    } else if (idx >= 0 && idx < g.options.length) {
      g.result = { status: 'answer', text: (idx + 1) + '. ' + g.options[idx] };
    } else {
      // 提交按钮但没输入:回退选项 1(与旧 NSAlert 版「空回车=默认按钮」一致)
      g.result = { status: 'answer', text: '1. ' + g.options[0] };
    }
  }
  try { if (g.tickTimer) g.tickTimer.invalidate; } catch (e) {}
  stopApp();
}

// 停止 NSApp.run():stop 只置标志,需再投递一个空事件唤醒事件循环立即返回。
function stopApp() {
  $.NSApp.stop(null);
  try {
    var ev = $.NSEvent.otherEventWithTypeLocationModifierFlagsTimestampWindowNumberContextSubtypeData1Data2(
      $.NSApplicationDefined, $.NSMakePoint(0, 0), 0, 0.0, 0, null, 0, 0, 0);
    $.NSApp.postEventAtStart(ev, true);
  } catch (e) {}
}

function run(argv) {
  try {
    if (!argv || argv.length < 1) {
      return JSON.stringify({ status: 'error', text: '缺少 payload 参数' });
    }
    var raw = $.NSString.stringWithContentsOfFileEncodingError(
      $(argv[0]), $.NSUTF8StringEncoding, null);
    if (!raw) {
      return JSON.stringify({ status: 'error', text: 'payload 读取失败: ' + argv[0] });
    }
    var p = JSON.parse(raw.js);

    var options = [];
    if (p.options && p.options.length) {
      for (var i = 0; i < p.options.length && i < 6; i++) options.push(String(p.options[i]));
    }
    if (options.length === 0) options.push('我已完成人工操作,继续');
    g.options = options;
    g.timeoutMs = p.timeout_ms || 120000;
    g.startedAt = new Date();
    var deadline = new Date(g.startedAt.getTime() + g.timeoutMs);

    // ===== 布局常量(自上而下堆叠,窗高随图片有无自适应) =====
    var W = 460, PAD = 14;
    var textW = W - PAD * 2;

    var info = String(p.message || '').replace(/\s+$/, '');
    if (p.url) info += '\n\n页面: ' + String(p.url).slice(0, 200);
    if (p.page_id) info += '\n页面ID: ' + p.page_id;

    // ===== 验证码图片(第 132 轮):CDP 截图路径,人工在弹窗里直接读码 =====
    var imgView = null;
    var imgH = 0;
    if (p.image_path && String(p.image_path).length > 0) {
      try {
        var nsimg = $.NSImage.alloc.initWithContentsOfFile($(String(p.image_path)));
        if (nsimg && nsimg.isValid) {
          var sz = nsimg.size;
          var iw = Number(sz.width) || 1, ih = Number(sz.height) || 1;
          imgH = Math.max(60, Math.min(200, Math.round(textW * ih / iw)));
          imgView = $.NSImageView.alloc.initWithFrame($.NSMakeRect(PAD, 0, textW, imgH));
          imgView.image = nsimg;
          imgView.imageScaling = $.NSImageScaleProportionallyDown;
          imgView.imageAlignment = $.NSImageAlignCenter;
        }
      } catch (eImg) { imgView = null; imgH = 0; }
    }

    var msgH = 84;    // 说明区(NSTextView,约 4 行,可滚动)
    var titleH = 22;
    var lineH = 18;   // 时间轴单行高
    var inputH = 26;
    var btnH = 30;
    var gap = 8;
    var optCount = options.length;

    // 自上而下逐段累计(与下方 y -= 递减严格对应),避免窗高算错导致底部按钮被裁
    var consumed = titleH + gap + msgH + gap;
    if (imgH > 0) consumed += imgH + gap;
    consumed += (lineH + gap) * 2;                  // 提出时间 + 倒计时两行
    consumed += inputH + gap;
    consumed += optCount * (btnH + 6) - 6 + gap;    // 选项按钮纵排(尾距还原为 gap)
    consumed += btnH;
    var contentH = consumed + PAD;

    // ===== 窗口:Titled(无关闭/缩放按钮 → 杜绝误关),浮动置顶 =====
    var rect = $.NSMakeRect(0, 0, W, contentH);
    var win = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer(
      rect, $.NSWindowStyleMaskTitled, $.NSBackingStoreBuffered, false);
    win.title = $('🔐 人工介入 · ' + (p.kind_label || p.kind || '人工介入'));
    win.level = $.NSFloatingWindowLevel; // 视觉永远置顶;不再周期抢键盘焦点
    win.center;

    var ctrl = LaewHitl.alloc.init;
    var y = contentH - titleH; // AppKit 坐标系:y 自顶向下递减(先算顶边)

    function addLabel(h, text, font, color) {
      var l = $.NSTextField.alloc.initWithFrame($.NSMakeRect(PAD, y - h, textW, h));
      l.editable = false;
      l.bezeled = false;
      l.drawsBackground = false;
      l.stringValue = $(text);
      l.font = font || $.NSFont.systemFontOfSize(11);
      if (color) l.textColor = color;
      win.contentView.addSubview(l);
      y -= h + gap;
      return l;
    }

    addLabel(titleH, '🔐 ' + (p.kind_label || p.kind || '人工介入') + ' —— 请人工处理',
      $.NSFont.boldSystemFontOfSize(13));

    // 说明区:NSTextView(selectable)→ 鼠标拖选 + ⌘C 复制(第 132 轮用户诉求①)
    var scroll = $.NSScrollView.alloc.initWithFrame($.NSMakeRect(PAD, y - msgH, textW, msgH));
    scroll.hasVerticalScroller = true;
    scroll.borderType = $.NSBezelBorder;
    scroll.drawsBackground = false;
    var tv = $.NSTextView.alloc.initWithFrame($.NSMakeRect(0, 0, textW - 4, msgH));
    tv.editable = false;
    tv.selectable = true;      // ★ 可选中复制的关键
    tv.richText = false;
    tv.drawsBackground = false;
    tv.font = $.NSFont.systemFontOfSize(12);
    tv.string = $(info);
    scroll.documentView = tv;
    win.contentView.addSubview(scroll);
    y -= msgH + gap;

    if (imgView) {
      imgView.frame = $.NSMakeRect(PAD, y - imgH, textW, imgH);
      win.contentView.addSubview(imgView);
      y -= imgH + gap;
    }

    addLabel(lineH, '提出时间: ' + fmtClock(g.startedAt) + '   超时截止: ' + fmtClock(deadline));
    var countdown = addLabel(lineH, '已等待: 0 秒   超时剩余: ' + fmtDur(g.timeoutMs));
    g.countdownLabel = countdown;

    // 输入框:显式 first responder + 回车即提交(诉求②:键盘/输入法可正常输入)
    var input = $.NSTextField.alloc.initWithFrame($.NSMakeRect(PAD, y - inputH, textW, inputH));
    input.placeholderString = $('可输入验证码/动态码/说明;留空点选项或提交');
    input.font = $.NSFont.systemFontOfSize(13);
    input.target = ctrl;
    input.action = 'submit:';
    input.tag = -1; // 输入框回车:有字=文本答案,无字=选项 1
    win.contentView.addSubview(input);
    g.input = input;
    y -= inputH + gap;

    // 选项按钮(纵向全宽;点击即提交,输入框有字时文本优先)
    for (var oi = 0; oi < optCount; oi++) {
      var ob = $.NSButton.alloc.init;
      ob.title = $(options[oi]);
      ob.frame = $.NSMakeRect(PAD, y - btnH, textW, btnH);
      ob.bezelStyle = $.NSRoundedBezelStyle;
      ob.tag = oi;
      ob.target = ctrl;
      ob.action = 'submit:';
      win.contentView.addSubview(ob);
      y -= btnH + 6;
    }
    y += 6 - gap; // 尾按钮的 +6 间距还原为标准 gap(与 consumed 公式一致)

    // 底部:提交(⌅)+ 取消(⎋)
    var cancelW = 96, submitW = 96;
    var cancelBtn = $.NSButton.alloc.init;
    cancelBtn.title = $('取消');
    cancelBtn.frame = $.NSMakeRect(PAD, y - btnH, cancelW, btnH);
    cancelBtn.bezelStyle = $.NSRoundedBezelStyle;
    cancelBtn.keyEquivalent = $('\u001b'); // Esc = 取消
    cancelBtn.target = ctrl;
    cancelBtn.action = 'cancel:';
    win.contentView.addSubview(cancelBtn);

    var submitBtn = $.NSButton.alloc.init;
    submitBtn.title = $('提交');
    submitBtn.frame = $.NSMakeRect(PAD + cancelW + 10, y - btnH, submitW, btnH);
    submitBtn.bezelStyle = $.NSRoundedBezelStyle;
    submitBtn.keyEquivalent = $('\r'); // ⌅ = 提交
    submitBtn.tag = -1;
    submitBtn.target = ctrl;
    submitBtn.action = 'submit:';
    win.contentView.addSubview(submitBtn);

    // ===== 焦点与激活:Regular 策略 + 启动激活一次 + 双保险 first responder =====
    $.NSApp.setActivationPolicy($.NSApplicationActivationPolicyRegular);
    win.initialFirstResponder = input;
    win.makeKeyAndOrderFront(null);
    $.NSApp.activateIgnoringOtherApps(true);
    // 0.05s 一次性 timer:窗口上屏后再补一次 makeFirstResponder(initialFirstResponder
    // 依赖首次 key view 解析,双保险防「看着有焦点打不进字」)
    var focusOnce = $.NSTimer.scheduledTimerWithTimeIntervalRepeatsBlock(0.05, false, function () {
      try { win.makeFirstResponder(input); } catch (e) {}
    });

    // ===== 秒级倒计时 + 超时自灭(app.run() 下 default mode 定时器正常 fire) =====
    g.tickTimer = $.NSTimer.scheduledTimerWithTimeIntervalRepeatsBlock(1.0, true, function () {
      if (g.done) return;
      var elapsed = (new Date()).getTime() - g.startedAt.getTime();
      var remain = g.timeoutMs - elapsed;
      try {
        g.countdownLabel.stringValue =
          $('已等待: ' + fmtDur(elapsed) + '   超时剩余: ' + fmtDur(Math.max(0, remain)));
      } catch (e) {}
      if (remain <= 0) finish('timeout', null);
    });

    $.NSApp.run; // 属性访问式调用(零参方法惯例,带括号会把 void 返回值当函数调)。
                 // 阻塞至 stopApp();期间定时器/IME/选择交互全部正常

    try { focusOnce.invalidate; } catch (e1) {}
    if (!g.result) g.result = { status: 'cancel', text: '' }; // 兜底:run 意外返回视为取消
    return JSON.stringify(g.result);
  } catch (e) {
    return JSON.stringify({ status: 'error', text: String(e) });
  }
}
