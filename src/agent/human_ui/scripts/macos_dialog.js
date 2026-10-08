// laew 人工介入弹窗 —— macOS 默认脚本(JXA + AppKit 自绘 NSWindow,第 133 轮布局重写)
// 用法: osascript -l JavaScript <本脚本> <payload.json>
// 动态加载: {工作目录|根目录|~}/.laew/human_ui/macos.js 可整体替换本脚本(每次呈现重读,改完即生效)
// stdout 输出结果 JSON: {"status":"answer|cancel|timeout|error","text":"..."}
// 设计见 docs/MCP_Web_Use/03-人工介入弹窗UI动态加载方案.md(第 132/133 轮章节)
//
// ===== 第 134 轮(实测「输入验证码后点提交/选项都没反应」根治) =====
// 三条现场证据(探针注入 + 截图)指向三条独立缺陷,均已修:
//   (1) 布局契约错位:`doLayout` 起始 `y = ch - titleH` 比 `contentHeight` 多扣了一次标题,
//       底部「取消/提交」整行下移 22px,实测 frame y=-6 掉出内容视图(高 300),
//       点击落在被裁边带 → 观感「按了没反应」。已改为 `y = ch`;
//   (2) 最大化分支用 `setFrameDisplay`(frame 坐标,含 32px 标题栏)却按内容高排版,
//       同样差 32px。改为 `setContentSize` + `setFrameOrigin`;
//   (3) `[NSWindow center]` 依赖 `window.screen` 隐式归属,后台 osascript 进程在多屏
//       (本机副屏位于主屏下方 y=-1080)实测把窗口落到副屏,主屏内完全不可见 →
//       同样表现为「点了没反应」。改为 `pickScreen()`(visibleFrame 面积最大)+
//       `placeWindow()` 显式算原点并夹紧到可见区。
// 另:`/hitl <应答>` 终端应急通道(Rust 侧)保证弹窗端万一再出异常也有逃生门。
//
// ===== 第 133 轮(验证码可读性根治,实测云智眼登录整页截图缩到 460px 弹窗后无法读码) =====
// - 窗口 460→560,图区上限 200→300px;新增「⛶ 最大化/还原」(铺满主屏可视区,布局整体
//   重排,验证码放大到满屏可读)与「查看原图」(NSWorkspace 打开 PNG,系统查看器随意缩放);
// - 布局函数化:创建视图与 doLayout(w,h,msgH,imgH) 分离,最大化/还原只是换参重排,
//   杜绝逐控件手改 frame 漂移;图片区下方标注原图像素尺寸;
// - 点击图片本身也可打开原图(NSClickGestureRecognizer,失败静默降级,不影响主流程)。
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

// NSRect 桥接值兼容读取(部分 JXA 版本返回 {origin,size},部分直接 {x,y,width,height})。
function rectVals(r) {
  if (!r) return { x: 0, y: 0, w: 0, h: 0 };
  if (r.size) {
    return {
      x: Number(r.origin.x) || 0, y: Number(r.origin.y) || 0,
      w: Number(r.size.width) || 0, h: Number(r.size.height) || 0,
    };
  }
  return {
    x: Number(r.x) || 0, y: Number(r.y) || 0,
    w: Number(r.width) || 0, h: Number(r.height) || 0,
  };
}

// ===== 第 134 轮:显式选屏 + 显式定位 =====
// 弃用 `[NSWindow center]`:它依赖 `window.screen` 的隐式归属,后台 osascript 进程
// 在多屏环境下实测会落到「主屏下方的副屏」上(本机 frame (1352,-530,560,332),
// 主屏内完全不可见 → 用户看着「弹窗点了没反应」)。这里改为按 visibleFrame 面积
// 选主屏 + 显式算原点并夹紧到可见区。
function pickScreen() {
  var screens = $.NSScreen.screens;
  var best = null, bestArea = -1;
  for (var i = 0; i < screens.count; i++) {
    var s = screens.objectAtIndex(i);
    var vf = rectVals(s.visibleFrame);
    var area = vf.w * vf.h;
    // 同面积时取 visibleFrame 面积(含 Dock/Menubar 的真实可用区)最大的,
    // 再同则取 origin.x/y 较大者(主屏通常在左/上,避免选到副屏)
    if (area > bestArea) { bestArea = area; best = s; }
  }
  return best || $.NSScreen.mainScreen;
}

// 把窗口按内容尺寸落到指定屏幕可见区正中,超高时优先保证「标题栏 + 输入框」可见。
function placeWindow(win, w, h) {
  var s = pickScreen();
  var vf = rectVals(s.visibleFrame);
  var wW = Math.min(w, Math.max(320, vf.w - 40));
  var wH = Math.min(h, Math.max(240, vf.h - 40));
  var x = Math.round(vf.x + (vf.w - wW) / 2);
  var y = Math.round(vf.y + (vf.h - wH) / 2);
  // setContentSize 走**内容视图**语义,frame 由 AppKit 自行加标题栏;
  // 不用 setFrameDisplay(那是 frame 坐标,会与 doLayout 的内容高度差 32px)。
  try {
    win.setContentSize($.NSMakeSize(wW, wH));
    win.setFrameOrigin($.NSMakePoint(x, y));
  } catch (e) {
    win.setFrameDisplay($.NSMakeRect(x, y, wW, wH), true);
  }
}

// 控制器:选项按钮 / 输入框回车提交 / 取消 / 放大缩小 / 查看原图 / 超时统一走这里收口。
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
    // ⛶ 最大化/还原
    'toggleZoom:': {
      types: ['void', ['id']],
      implementation: function () {
        try { if (g.toggleZoom) g.toggleZoom(); } catch (e) {}
      },
    },
    // 查看原图(按钮 + 图片双击/单击手势共用)
    'openOrig:': {
      types: ['void', ['id']],
      implementation: function () {
        try { if (g.openOrig) g.openOrig(); } catch (e) {}
      },
    },
  },
});
var LaewHitl = $.LaewHitlController;

// ---- 供控制器闭包访问的可变状态(run() 内赋值) ----
var g = {
  input: null,        // NSTextField
  options: [],
  result: null,       // {status, text}
  done: false,
  timeoutMs: 120000,
  startedAt: new Date(),
  tickTimer: null,
  countdownLabel: null,
  toggleZoom: null,   // 第 133 轮:最大化/还原(布局函数闭包)
  openOrig: null,     // 第 133 轮:系统查看器打开原图
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

    // ===== 布局常量(第 133 轮:窗口加宽 + 布局函数化) =====
    var W = 560, PAD = 16;
    var titleH = 22;
    var lineH = 18;   // 时间轴/提示单行高
    var inputH = 28;
    var btnH = 30;
    var gap = 8;
    var optCount = options.length;

    var info = String(p.message || '').replace(/\s+$/, '');
    if (p.url) info += '\n\n页面: ' + String(p.url).slice(0, 200);
    if (p.page_id) info += '\n页面ID: ' + p.page_id;

    // ===== 视图引用(先创建,doLayout 统一排布;最大化/还原 = 换参重排) =====
    var refs = {};    // {title,msgScroll,img,imgHint,t1,t2,input,optBtns[],cancel,submit,zoom,orig}

    var title = $.NSTextField.alloc.initWithFrame($.NSMakeRect(0, 0, 10, 10));
    title.editable = false; title.bezeled = false; title.drawsBackground = false;
    title.font = $.NSFont.boldSystemFontOfSize(13);
    refs.title = title;

    var scroll = $.NSScrollView.alloc.initWithFrame($.NSMakeRect(0, 0, 10, 10));
    scroll.hasVerticalScroller = true;
    scroll.borderType = $.NSBezelBorder;
    scroll.drawsBackground = false;
    var tv = $.NSTextView.alloc.initWithFrame($.NSMakeRect(0, 0, 10, 10));
    tv.editable = false;
    tv.selectable = true;      // ★ 可选中复制的关键(第 132 轮)
    tv.richText = false;
    tv.drawsBackground = false;
    tv.font = $.NSFont.systemFontOfSize(12);
    tv.string = $(info);
    scroll.documentView = tv;
    refs.msgScroll = scroll;

    // ===== 验证码图片(第 132/133 轮):元素裁剪图优先,人工在弹窗里直接读码 =====
    var nsimg = null, imgRatio = 0, imgNatW = 0, imgNatH = 0;
    if (p.image_path && String(p.image_path).length > 0) {
      try {
        var tmp = $.NSImage.alloc.initWithContentsOfFile($(String(p.image_path)));
        if (tmp && tmp.isValid) {
          nsimg = tmp;
          var sz = tmp.size;
          imgNatW = Number(sz.width) || 1;
          imgNatH = Number(sz.height) || 1;
          imgRatio = imgNatH / imgNatW;
        }
      } catch (eImg) { nsimg = null; }
    }
    var img = null;
    if (nsimg) {
      img = $.NSImageView.alloc.initWithFrame($.NSMakeRect(0, 0, 10, 10));
      img.image = nsimg;
      img.imageScaling = $.NSImageScaleProportionallyDown;
      img.imageAlignment = $.NSImageAlignCenter;
      // 点击图片打开原图(手势失败静默降级,纯附加能力)
      try {
        var gesture = $.NSClickGestureRecognizer.alloc.initWithTargetAction(
          LaewHitl.alloc.init, 'openOrig:');
        img.addGestureRecognizer(gesture);
      } catch (eGest) {}
    }
    refs.img = img;

    var imgHint = $.NSTextField.alloc.initWithFrame($.NSMakeRect(0, 0, 10, 10));
    imgHint.editable = false; imgHint.bezeled = false; imgHint.drawsBackground = false;
    imgHint.font = $.NSFont.systemFontOfSize(10);
    imgHint.textColor = $.NSColor.secondaryLabelColor;
    imgHint.stringValue = $('原图 ' + imgNatW + '×' + imgNatH + ' px · 点击图片或「查看原图」可放大细读');
    refs.imgHint = nsimg ? imgHint : null;

    function mkLine() {
      var l = $.NSTextField.alloc.initWithFrame($.NSMakeRect(0, 0, 10, 10));
      l.editable = false; l.bezeled = false; l.drawsBackground = false;
      l.font = $.NSFont.systemFontOfSize(11);
      return l;
    }
    refs.t1 = mkLine();
    refs.t2 = mkLine();

    var input = $.NSTextField.alloc.initWithFrame($.NSMakeRect(0, 0, 10, 10));
    input.placeholderString = $('可输入验证码/动态码/说明;留空点选项或提交');
    input.font = $.NSFont.systemFontOfSize(13);
    input.target = LaewHitl.alloc.init;
    input.action = 'submit:';
    input.tag = -1; // 输入框回车:有字=文本答案,无字=选项 1
    refs.input = input;
    g.input = input;

    refs.optBtns = [];
    for (var oi = 0; oi < optCount; oi++) {
      var ob = $.NSButton.alloc.init;
      ob.title = $(options[oi]);
      ob.bezelStyle = $.NSRoundedBezelStyle;
      ob.tag = oi;
      ob.target = LaewHitl.alloc.init;
      ob.action = 'submit:';
      refs.optBtns.push(ob);
    }

    function mkBtn(titleText, keyEq, action, tag) {
      var b = $.NSButton.alloc.init;
      b.title = $(titleText);
      b.bezelStyle = $.NSRoundedBezelStyle;
      if (keyEq) b.keyEquivalent = $(keyEq);
      if (tag !== undefined) b.tag = tag;
      b.target = LaewHitl.alloc.init;
      b.action = action;
      return b;
    }
    refs.cancel = mkBtn('取消', '\u001b', 'cancel:');
    refs.submit = mkBtn('提交', '\r', 'submit:', -1);
    refs.zoom = mkBtn('⛶ 最大化', null, 'toggleZoom:');
    refs.orig = mkBtn('查看原图', null, 'openOrig:');

    // 全部挂到 contentView(创建顺序即 z 序,均不重叠)
    var cv = null; // run() 内拿到 win 后统一添加

    // ===== 布局函数:给定**内容视图**宽/高、说明区高、图区高,自上而下重排全部子视图 =====
    // AppKit 坐标系:y 自顶向下递减。
    // 第 134 轮修正:起始 y 必须从 ch 起算(contentHeight 逐项累加 place 序列时,标题只占
    // 一次 titleH)。此前写成 `ch - titleH` 又把整摞控件下推一行,而 contentHeight 没有
    // 为此留预算 → 底部「取消/提交」整行下移 22px,y=-6 掉出内容视图(实测),点击落在被裁
    // 的边带上,用户观感为「点了没反应」。
    function doLayout(cw, ch, msgH, imgH) {
      var textW = cw - PAD * 2;
      var y = ch;
      function place(v, h) {
        v.frame = $.NSMakeRect(PAD, y - h, textW, h);
        y -= h + gap;
      }
      place(refs.title, titleH);
      place(refs.msgScroll, msgH);
      if (refs.img && imgH > 0) {
        place(refs.img, imgH);
        place(refs.imgHint, 14);
      }
      place(refs.t1, lineH);
      place(refs.t2, lineH);
      place(refs.input, inputH);
      for (var bi = 0; bi < refs.optBtns.length; bi++) place(refs.optBtns[bi], btnH);
      // 底部按钮行:左 取消/提交,右 查看原图/⛶ 最大化(place 已留标准 gap,无需修正)
      var bw = 84, ow = 110, zw = 110, inner = 8;
      var rightX = cw - PAD - zw;
      var origX = rightX - inner - ow;
      refs.zoom.frame = $.NSMakeRect(rightX, y - btnH, zw, btnH);
      refs.orig.frame = $.NSMakeRect(origX, y - btnH, ow, btnH);
      refs.cancel.frame = $.NSMakeRect(PAD, y - btnH, bw, btnH);
      refs.submit.frame = $.NSMakeRect(PAD + bw + inner, y - btnH, bw, btnH);
    }

    // 由内容推出窗高(与 doLayout 的 place 序列逐项对应)
    function contentHeight(msgH, imgH) {
      var consumed = titleH + gap + msgH + gap;
      if (imgH > 0) consumed += imgH + gap + 14 + gap;
      consumed += (lineH + gap) * 2;
      consumed += inputH + gap;
      consumed += optCount * (btnH + gap);          // 选项按钮纵排(place 逐项对应)
      consumed += btnH;
      return consumed + PAD;
    }

    // 图区高:按图片纵横比适配文本宽,夹在 [60, cap] 之间
    function imgHeightFor(textW, cap) {
      if (!imgRatio) return 0;
      return Math.max(60, Math.min(cap, Math.round(textW * imgRatio)));
    }

    var normalMsgH = 90;
    var normalImgH = imgHeightFor(W - PAD * 2, 300);
    var normalH = contentHeight(normalMsgH, normalImgH);

    // ===== 窗口:Titled(无关闭/缩放按钮 → 杜绝误关),浮动置顶 =====
    var rect = $.NSMakeRect(0, 0, W, normalH);
    var win = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer(
      rect, $.NSWindowStyleMaskTitled, $.NSBackingStoreBuffered, false);
    win.title = $('🔐 人工介入 · ' + (p.kind_label || p.kind || '人工介入'));
    win.level = $.NSFloatingWindowLevel; // 视觉永远置顶;不再周期抢键盘焦点

    cv = win.contentView;
    cv.addSubview(refs.title);
    cv.addSubview(refs.msgScroll);
    if (refs.img) { cv.addSubview(refs.img); cv.addSubview(refs.imgHint); }
    cv.addSubview(refs.t1);
    cv.addSubview(refs.t2);
    cv.addSubview(refs.input);
    for (var ai = 0; ai < refs.optBtns.length; ai++) cv.addSubview(refs.optBtns[ai]);
    cv.addSubview(refs.cancel);
    cv.addSubview(refs.submit);
    cv.addSubview(refs.zoom);
    cv.addSubview(refs.orig);

    refs.t1.stringValue = $('提出时间: ' + fmtClock(g.startedAt) + '   超时截止: ' + fmtClock(deadline));
    refs.t2.stringValue = $('已等待: 0 秒   超时剩余: ' + fmtDur(g.timeoutMs));
    g.countdownLabel = refs.t2;

    doLayout(W, normalH, normalMsgH, normalImgH);
    placeWindow(win, W, normalH);

    // ===== 第 133 轮:最大化/还原(铺满主屏可视区,布局整体重排) =====
    var zoomed = false;
    g.toggleZoom = function () {
      try {
        if (!zoomed) {
          var vf = rectVals(pickScreen().visibleFrame);
          var zw = Math.max(W, vf.w - 40), zh = Math.max(normalH, vf.h - 40);
          var zMsgH = Math.min(240, Math.max(90, Math.round(zh * 0.22)));
          var zImgH = imgHeightFor(zw - PAD * 2, Math.min(900, Math.round(zh * 0.55)));
          var zH = Math.max(zh, contentHeight(zMsgH, zImgH));
          placeWindow(win, zw, zH);
          doLayout(zw, rectVals(win.contentView.frame).h, zMsgH, zImgH);
          refs.zoom.title = $('⛶ 还原');
          zoomed = true;
        } else {
          placeWindow(win, W, normalH);
          doLayout(W, rectVals(win.contentView.frame).h, normalMsgH, normalImgH);
          refs.zoom.title = $('⛶ 最大化');
          zoomed = false;
        }
        try { win.makeFirstResponder(input); } catch (eF) {}
      } catch (eZ) {}
    };

    // ===== 第 133 轮:查看原图(系统查看器打开 PNG,可随意缩放/全屏) =====
    var imagePath = (p.image_path && nsimg) ? String(p.image_path) : '';
    g.openOrig = function () {
      try {
        if (imagePath) $.NSWorkspace.sharedWorkspace.openFile($(imagePath));
      } catch (eO) {}
    };

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
