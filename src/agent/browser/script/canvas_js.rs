//! Canvas 2D 的 JS 薄壳(第 56 轮)。
//!
//! 像素/路径/变换/渐变的**权威状态**在宿主侧(`canvas_raster.rs`,JS 线程
//! thread_local),本文件只提供与浏览器同形的 `ctx` 对象:所有绘制命令
//! 序列化成 JSON 经原生入口 `__lsmCv(key, opJson, extra?)` 分发。
//! 旧实现(Boa 逐像素循环 + 路径/文字/渐变 noop)已整体退役;
//! `data-lsm-canvas` → 宿主图片管线通道保持不变。

pub const CANVAS_JS: &str = r##"
var __lsmCanvases = [];
var __lsmCanvasByKey = {};

function __lsmCanvasError(message) {
  __cerror("canvas: " + String(message));
}

// ---- 宿主分发 ----

// 返回值:多数 op 返回 JSON 字符串(解析回对象);get_pixels 返回 Uint8Array。
function __cvRaw(el, op, extra) {
  return __lsmCv(el.__canvasKey, JSON.stringify(op), extra || null);
}

function __cvOp(el, op, extra) {
  try {
    var r = __cvRaw(el, op, extra);
    if (r && typeof r === "object" && r.buffer) return r;
    if (typeof r === "string") return JSON.parse(r);
  } catch (e) {
    __lsmCanvasError((e && e.message) || e);
  }
  return undefined;
}

function __lsmAttachCanvas(el, selector) {
  if (!el || el.__canvasAttached) return;
  el.__canvasAttached = true;
  el.__canvasSelector = selector || null;
  var key = attr("data-lsm-id") || selector || __lsmDynKey(el);
  el.__canvasKey = key;
  if (key && __lsmCanvasByKey[key]) el.__ctx2d = __lsmCanvasByKey[key];
  var reused = !!el.__ctx2d;
  function attr(name) {
    try {
      if (el.__attrs && el.__attrs[name] != null) return el.__attrs[name];
      if (el.getAttribute) return el.getAttribute(name);
    } catch (e) {}
    return null;
  }
  function dimension(name, fallback) {
    var initial = parseInt(attr(name), 10);
    if (!(initial >= 0)) initial = fallback;
    el["__canvas" + name] = initial;
    Object.defineProperty(el, name, {
      configurable: true,
      enumerable: true,
      get: function () { return el["__canvas" + name]; },
      set: function (v) {
        var n = Number(v);
        if (!(n >= 0)) n = 0;
        n = Math.min(4096, n >>> 0);
        el["__canvas" + name] = n;
        try {
          if (el.__attrs) el.__attrs[name] = String(n);
          else if (el.setAttribute) el.setAttribute(name, String(n));
        } catch (e) {}
        // 宽高重设 = 全量重置画布(浏览器语义:内容清空、状态归零)。
        __cvOp(el, { op: "resize", w: el.width, h: el.height });
        if (el.__ctx2d) el.__ctx2d.__dirty = true;
      }
    });
  }
  dimension("width", 300);
  dimension("height", 150);
  // 宿主侧建状态(同 key 已存在则保留,等价旧实现的像素复用语义)。
  __cvOp(el, { op: "attach", w: el.width, h: el.height });
  el.getContext = function (kind) {
    var k = String(kind || "").toLowerCase();
    if (k.indexOf("2d") !== 0) {
      if (k.indexOf("webgl") === 0) window.__lsmWebGLAsked = (window.__lsmWebGLAsked || 0) + 1;
      return null;
    }
    if (!el.__ctx2d) {
      el.__ctx2d = __mkCanvas2d(el);
      if (key) __lsmCanvasByKey[key] = el.__ctx2d;
    }
    return el.__ctx2d;
  };
  el.toDataURL = function () {
    if (!el.__ctx2d) el.getContext("2d");
    return __lsmCanvasDataUrl(el);
  };
  if (!reused) __lsmCanvases.push(el);
}

function __lsmCanvasDataUrl(canvas) {
  var r = __cvOp(canvas, { op: "to_data_url" });
  return (r && r.url) || "data:image/png;base64,";
}

function __lsmFlushCanvases() {
  for (var i = 0; i < __lsmCanvases.length; i++) {
    var el = __lsmCanvases[i];
    if (!el.__ctx2d || !el.__ctx2d.__dirty) continue;
    try {
      var url = __lsmCanvasDataUrl(el);
      if (el.__attrs) {
        // 动态节点:未挂载时随 __serEl 序列化;已挂载时由 __lsmFlushDynAttrs 下发。
        el.__attrs["data-lsm-canvas"] = url;
      } else if (el.__canvasSelector && el.setAttribute) {
        // 静态快照节点:setAttribute 同时更新快照属性并回写宿主 DOM。
        el.setAttribute("data-lsm-canvas", url);
      }
      el.__ctx2d.__dirty = false;
    } catch (e) {
      __lsmCanvasError((e && e.stack) || e);
    }
  }
}

// ---- ctx 对象 ----

// 解析 ctx.font 为 {px, bold}:canvas font 简写 "italic bold 16px/1.5 Arial"。
function __cvFontSpec(font) {
  var s = String(font || "");
  var m = s.match(/(\d+(?:\.\d+)?)px/);
  return {
    px: m ? parseFloat(m[1]) : 10,
    bold: /bold|,\s*[7-9]00\b|^\s*[7-9]00\b/.test(s)
  };
}

function __mkCanvas2d(canvas) {
  var el = canvas;
  var dashList = [];
  var ctx = {
    canvas: canvas,
    fillStyle: "#000000", strokeStyle: "#000000", lineWidth: 1, lineCap: "butt", lineJoin: "miter",
    miterLimit: 10, font: "10px sans-serif", textAlign: "start", textBaseline: "alphabetic",
    globalAlpha: 1, globalCompositeOperation: "source-over", imageSmoothingEnabled: true,
    shadowBlur: 0, shadowColor: "rgba(0,0,0,0)", shadowOffsetX: 0, shadowOffsetY: 0,
    lineDashOffset: 0,
    __dirty: false
  };
  function sh() {
    return {
      blur: Number(ctx.shadowBlur) || 0,
      color: String(ctx.shadowColor || ""),
      ox: Number(ctx.shadowOffsetX) || 0,
      oy: Number(ctx.shadowOffsetY) || 0
    };
  }
  function common() {
    return { alpha: ctx.globalAlpha, comp: String(ctx.globalCompositeOperation || "source-over") };
  }
  function op(fields, bytes) {
    var o = common();
    for (var k in fields) o[k] = fields[k];
    var r = __cvOp(el, o, bytes);
    if (r) ctx.__dirty = true;
    return r;
  }

  // 路径
  ctx.beginPath = function () { __cvOp(el, { op: "begin_path" }); };
  ctx.closePath = function () { __cvOp(el, { op: "close_path" }); };
  ctx.moveTo = function (x, y) { __cvOp(el, { op: "path_move", x: x, y: y }); };
  ctx.lineTo = function (x, y) { __cvOp(el, { op: "path_line", x: x, y: y }); };
  ctx.quadraticCurveTo = function (cpx, cpy, x, y) {
    __cvOp(el, { op: "path_quad", cpx: cpx, cpy: cpy, x: x, y: y });
  };
  ctx.bezierCurveTo = function (c1x, c1y, c2x, c2y, x, y) {
    __cvOp(el, { op: "path_cubic", cp1x: c1x, cp1y: c1y, cp2x: c2x, cp2y: c2y, x: x, y: y });
  };
  ctx.arc = function (cx, cy, r, a0, a1, ccw) {
    __cvOp(el, { op: "path_arc", cx: cx, cy: cy, r: r, a0: a0, a1: a1, ccw: !!ccw });
  };
  ctx.arcTo = function (x1, y1, x2, y2, r) {
    __cvOp(el, { op: "path_arc_to", x1: x1, y1: y1, x2: x2, y2: y2, r: r });
  };
  ctx.ellipse = function (cx, cy, rx, ry, rot, a0, a1, ccw) {
    __cvOp(el, { op: "path_ellipse", cx: cx, cy: cy, rx: rx, ry: ry, rot: rot || 0, a0: a0, a1: a1, ccw: !!ccw });
  };
  ctx.rect = function (x, y, w, h) { __cvOp(el, { op: "path_rect", x: x, y: y, w: w, h: h }); };

  // 绘制
  ctx.fill = function (rule) {
    op({ op: "fill", rule: rule === "evenodd" ? "evenodd" : "nonzero", style: ctx.fillStyle, shadow: sh() });
  };
  ctx.stroke = function () {
    op({
      op: "stroke", style: ctx.strokeStyle, line_w: ctx.lineWidth, cap: ctx.lineCap,
      join: ctx.lineJoin, dash_offset: ctx.lineDashOffset, shadow: sh()
    });
  };
  ctx.clip = function (rule) {
    __cvOp(el, { op: "clip", rule: rule === "evenodd" ? "evenodd" : "nonzero" });
  };
  ctx.clearRect = function (x, y, w, h) {
    op({ op: "clear_rect", x: x, y: y, w: w, h: h });
  };
  ctx.fillRect = function (x, y, w, h) {
    op({ op: "fill_rect", x: x, y: y, w: w, h: h, style: ctx.fillStyle, shadow: sh() });
  };
  ctx.strokeRect = function (x, y, w, h) {
    op({
      op: "stroke_rect", x: x, y: y, w: w, h: h, style: ctx.strokeStyle,
      line_w: ctx.lineWidth, cap: ctx.lineCap, shadow: sh()
    });
  };

  // 文字
  ctx.fillText = function (t, x, y, maxW) {
    op({
      op: "fill_text", text: String(t == null ? "" : t), x: x, y: y,
      max_w: maxW == null ? 0 : Number(maxW), f: __cvFontSpec(ctx.font),
      align: ctx.textAlign, base: ctx.textBaseline, style: ctx.fillStyle, shadow: sh()
    });
  };
  ctx.strokeText = function (t, x, y, maxW) {
    op({
      op: "stroke_text", text: String(t == null ? "" : t), x: x, y: y,
      max_w: maxW == null ? 0 : Number(maxW), f: __cvFontSpec(ctx.font),
      align: ctx.textAlign, base: ctx.textBaseline, style: ctx.strokeStyle,
      line_w: ctx.lineWidth, shadow: sh()
    });
  };
  ctx.measureText = function (t) {
    var r = __cvOp(el, { op: "measure", text: String(t == null ? "" : t), f: __cvFontSpec(ctx.font) });
    return { width: (r && r.width) || 0 };
  };

  // 变换与状态
  ctx.save = function () { __cvOp(el, { op: "save" }); };
  ctx.restore = function () { __cvOp(el, { op: "restore" }); };
  ctx.translate = function (x, y) { __cvOp(el, { op: "xform", kind: "translate", x: x, y: y }); };
  ctx.rotate = function (rad) { __cvOp(el, { op: "xform", kind: "rotate", rad: rad }); };
  ctx.scale = function (sx, sy) { __cvOp(el, { op: "xform", kind: "scale", sx: sx, sy: sy }); };
  ctx.transform = function (a, b, c, d, e, f) {
    __cvOp(el, { op: "xform", kind: "mult", a: a, b: b, c: c, d: d, e: e, f: f });
  };
  ctx.setTransform = function (a, b, c, d, e, f) {
    __cvOp(el, { op: "xform", kind: "set", a: a, b: b, c: c, d: d, e: e, f: f });
  };
  ctx.resetTransform = function () { __cvOp(el, { op: "xform", kind: "reset" }); };

  // 虚线
  ctx.setLineDash = function (d) {
    dashList = (d && typeof d.length === "number") ? Array.prototype.slice.call(d).map(Number) : [];
    __cvOp(el, { op: "set_dash", dash: dashList });
  };
  ctx.getLineDash = function () { return dashList.slice(); };

  // 渐变与图案
  ctx.createLinearGradient = function (x0, y0, x1, y1) {
    var r = __cvOp(el, { op: "grad_create", kind: "linear", x0: x0, y0: y0, x1: x1, y1: y1 });
    var id = (r && r.id) || 0;
    return {
      __cvGrad: id,
      addColorStop: function (o, c) {
        __cvOp(el, { op: "grad_stop", id: id, offset: Number(o), color: String(c) });
      }
    };
  };
  ctx.createRadialGradient = function (x0, y0, r0, x1, y1, r1) {
    var r = __cvOp(el, { op: "grad_create", kind: "radial", x0: x0, y0: y0, r0: r0, x1: x1, y1: y1, r1: r1 });
    var id = (r && r.id) || 0;
    return {
      __cvGrad: id,
      addColorStop: function (o, c) {
        __cvOp(el, { op: "grad_stop", id: id, offset: Number(o), color: String(c) });
      }
    };
  };
  // pattern 为哑对象(56 号文档:非目标,赋值后不画)。
  ctx.createPattern = function () { return { __cvPattern: 1 }; };

  // 像素
  ctx.createImageData = function (w, h) {
    if (typeof w === "object" && w) { h = w.height; w = w.width; }
    w = Math.max(0, w | 0); h = Math.max(0, h | 0);
    return { width: w, height: h, data: new Uint8ClampedArray(w * h * 4) };
  };
  ctx.getImageData = function (x, y, w, h) {
    x |= 0; y |= 0; w = Math.max(0, w | 0); h = Math.max(0, h | 0);
    var raw;
    try {
      raw = __cvRaw(el, { op: "get_pixels", x: x, y: y, w: w, h: h }, null);
    } catch (e) {
      __lsmCanvasError(e);
    }
    var data = new Uint8ClampedArray(w * h * 4);
    if (raw && typeof raw === "object" && raw.buffer) data.set(raw);
    return { width: w, height: h, data: data };
  };
  ctx.putImageData = function (image, dx, dy, dirtyX, dirtyY, dirtyW, dirtyH) {
    if (!image || !image.data) return;
    var view;
    try {
      var d = image.data;
      view = d.buffer ? new Uint8Array(d.buffer, d.byteOffset || 0, d.byteLength) : new Uint8Array(d);
    } catch (e) {
      __lsmCanvasError(e);
      return;
    }
    op({
      op: "put_pixels", x: dx | 0, y: dy | 0, w: image.width | 0, h: image.height | 0,
      dx: dirtyX == null ? -1 : dirtyX | 0, dy: dirtyY == null ? -1 : dirtyY | 0,
      dw: dirtyW == null ? -1 : dirtyW | 0, dh: dirtyH == null ? -1 : dirtyH | 0
    }, view);
  };

  // 图像
  ctx.drawImage = function (source) {
    if (!source) return;
    var a = arguments;
    var srcDesc = null;
    if (source.__ctx2d && source.__canvasKey) {
      srcDesc = { canvas: source.__canvasKey };
    } else if (source.__lsmImgId) {
      srcDesc = { img: source.__lsmImgId };
    } else {
      // img-like 元素:懒取回 + 解码(结果缓存在元素上)。
      var url = String((source.__attrs && source.__attrs.src) || source.src || "");
      if (!url) return;
      if (!source.__lsmImgId) {
        var r = __cvOp(el, { op: "image_decode", url: url });
        source.__lsmImgId = (r && r.id) || 0;
        if (r && r.w && !source.naturalWidth) {
          source.naturalWidth = r.w;
          source.naturalHeight = r.h || r.w;
        }
      }
      if (!source.__lsmImgId) return;
      srcDesc = { img: source.__lsmImgId };
    }
    var sw = source.width, sh = source.height;
    var sx = 0, sy = 0, dx, dy, dw, dh;
    if (a.length === 3) {
      dx = a[1]; dy = a[2]; dw = sw; dh = sh;
    } else if (a.length === 5) {
      dx = a[1]; dy = a[2]; dw = a[3]; dh = a[4];
    } else if (a.length >= 9) {
      sx = a[1]; sy = a[2]; sw = a[3]; sh = a[4];
      dx = a[5]; dy = a[6]; dw = a[7]; dh = a[8];
    } else return;
    op({
      op: "draw_image", src: srcDesc, sx: sx, sy: sy, sw: sw, sh: sh,
      dx: dx, dy: dy, dw: dw, dh: dh, smoothing: ctx.imageSmoothingEnabled !== false
    });
  };

  // 命中
  ctx.isPointInPath = function (x, y, rule) {
    var r = __cvOp(el, { op: "is_point_in_path", x: x, y: y, rule: rule === "evenodd" ? "evenodd" : "nonzero" });
    return !!(r && r.inside);
  };
  ctx.isPointInStroke = function () { return false; };
  ctx.getContextAttributes = function () { return { alpha: true }; };

  return ctx;
}
"##;
