// laew 人工介入弹窗 —— macOS 默认脚本(JXA + AppKit NSAlert)
// 用法: osascript -l JavaScript <本脚本> <payload.json>
// 动态加载: {工作目录|根目录|~}/.laew/human_ui/macos.js 可整体替换本脚本(每次呈现重读,改完即生效)
// stdout 输出结果 JSON: {"status":"answer|cancel|timeout|error","text":"..."}
// 设计见 docs/MCP_Web_Use/03-人工介入弹窗UI动态加载方案.md

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
    var timeoutMs = p.timeout_ms || 120000;
    var startedAt = new Date();
    var deadline = new Date(startedAt.getTime() + timeoutMs);

    var alert = $.NSAlert.alloc.init;
    alert.messageText = $('🔐 人工介入 · ' + (p.kind_label || p.kind || '人工介入'));
    alert.alertStyle = 0; // NSAlertStyleWarning

    var info = String(p.message || '').replace(/\s+$/, '');
    if (p.url) info += '\n\n页面: ' + String(p.url).slice(0, 160);
    if (p.page_id) info += '\n页面ID: ' + p.page_id;
    alert.informativeText = $(info);

    // 选项按钮(先加者为默认按钮)+ 常驻「取消」(绑 Esc)
    for (var j = 0; j < options.length; j++) alert.addButtonWithTitle($(options[j]));
    alert.addButtonWithTitle($('取消'));
    var btns = alert.buttons;
    var cancelBtn = btns.objectAtIndex(Number(btns.count) - 1);
    cancelBtn.keyEquivalent = $('\u001b');

    // accessory: 时间轴两行(提出/已等待/超时剩余/超时截止,1s 动态刷新)+ 文本输入
    var acc = $.NSView.alloc.initWithFrame($.NSMakeRect(0, 0, 440, 86));
    function mkLabel(y, text) {
      var l = $.NSTextField.alloc.initWithFrame($.NSMakeRect(0, y, 440, 18));
      l.editable = false;
      l.bezeled = false;
      l.drawsBackground = false;
      l.stringValue = $(text);
      l.font = $.NSFont.systemFontOfSize(11);
      return l;
    }
    var l1 = mkLabel(66, '提出时间: ' + fmtClock(startedAt) + '   超时截止: ' + fmtClock(deadline) +
      (p.page_id ? '   页面ID: ' + p.page_id : ''));
    var l2 = mkLabel(46, '已等待: 0 秒   超时剩余: ' + fmtDur(timeoutMs));
    var input = $.NSTextField.alloc.initWithFrame($.NSMakeRect(0, 10, 440, 26));
    input.placeholderString = $('可输入验证码/动态码/说明;留空则按所点按钮');
    acc.addSubview(l1);
    acc.addSubview(l2);
    acc.addSubview(input);
    alert.accessoryView = acc;

    // 持续置顶 + 焦点保持:浮层窗口 + 周期 activate/前置
    alert.window.level = $.NSFloatingWindowLevel;
    function bringFront() {
      $.NSApp.activateIgnoringOtherApps(true);
      alert.window.makeKeyAndOrderFront(null);
    }
    bringFront();

    var result = { status: 'cancel', text: '' };
    var done = false;
    var focusTimer = $.NSTimer.scheduledTimerWithTimeIntervalRepeatsBlock(3.0, true, function () {
      if (done) return;
      bringFront();
    });
    var tickTimer = $.NSTimer.scheduledTimerWithTimeIntervalRepeatsBlock(1.0, true, function () {
      if (done) return;
      var elapsed = (new Date()).getTime() - startedAt.getTime();
      var remain = timeoutMs - elapsed;
      l2.stringValue = $('已等待: ' + fmtDur(elapsed) + '   超时剩余: ' + fmtDur(Math.max(0, remain)));
      if (remain <= 0) {
        result = { status: 'timeout', text: '' };
        // JXA 零参 ObjC 方法用属性访问式调用(带括号在部分桥接版本报 not a function)
        $.NSApp.abortModal;
      }
    });

    var resp = Number(alert.runModal);
    done = true;
    // JXA 零参 ObjC 方法:属性访问式调用;防御式收口(不能让 timer 清理失败
    // 污染 timeout/answer 结果 —— 实测 `timer.invalidate()` 抛 TypeError 会把
    // 120s 自超时吞成 error)
    try { focusTimer.invalidate; } catch (e1) {}
    try { tickTimer.invalidate; } catch (e2) {}

    if (result.status === 'timeout') return JSON.stringify(result);
    var idx = resp - 1000; // NSAlertFirstButtonReturn = 1000
    var typed = String(input.stringValue.js || '').trim();
    if (idx >= 0 && idx < options.length) {
      if (typed.length > 0) {
        result = { status: 'answer', text: typed };
      } else {
        result = { status: 'answer', text: (idx + 1) + '. ' + options[idx] };
      }
    } else {
      result = { status: 'cancel', text: '' };
    }
    return JSON.stringify(result);
  } catch (e) {
    return JSON.stringify({ status: 'error', text: String(e) });
  }
}
