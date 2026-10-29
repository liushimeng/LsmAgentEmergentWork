//! Canvas 2D 的真实像素子集。
//!
//! JS 线程持有每个画布的 RGBA8 缓冲;宿主只负责 PNG 编码与图片缓存。
//! 路径/文字/渐变等仍由兼容占位承接,本模块只承诺矩形、像素读写和 Canvas 间贴图。

pub const CANVAS_JS: &str = r##"
var __lsmCanvases = [];
var __lsmCanvasByKey = {};

function __lsmCanvasError(message) {
  __cerror("canvas: " + String(message));
}

function __lsmAttachCanvas(el, selector) {
  if (!el || el.__canvasAttached) return;
  el.__canvasAttached = true;
  el.__canvasSelector = selector || null;
  var key = attr("data-lsm-id") || selector || __lsmDynKey(el);
  el.__canvasKey = key;
  if (key && __lsmCanvasByKey[key]) el.__ctx2d = __lsmCanvasByKey[key];
  var reused = !!el.__ctx2d;
  el.__canvasDirty = false;
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
        if (el.__ctx2d) {
          var other = name === "width" ? el.height : el.width;
          el.__ctx2d.__pixels = new Uint8ClampedArray(n * other * 4);
          el.__ctx2d.__dirty = true;
        }
      }
    });
  }
  dimension("width", 300);
  dimension("height", 150);
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
  var ctx = canvas.__ctx2d;
  if (!ctx) return "data:image/png;base64,";
  var pixels = ctx.__pixels;
  var binary = "";
  // 每字节对应一个 Latin-1 码元;btoa 的宿主实现同样按二进制语义编码。
  for (var i = 0; i < pixels.length; i++) binary += String.fromCharCode(pixels[i]);
  return __lsmCanvasPng(canvas.width, canvas.height, btoa(binary));
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

function __canvasColor(value) {
  var s = String(value == null ? "#000000" : value).trim().toLowerCase();
  var named = {
    black: [0, 0, 0, 255], white: [255, 255, 255, 255], red: [255, 0, 0, 255],
    green: [0, 128, 0, 255], blue: [0, 0, 255, 255], yellow: [255, 255, 0, 255],
    cyan: [0, 255, 255, 255], magenta: [255, 0, 255, 255], gray: [128, 128, 128, 255],
    grey: [128, 128, 128, 255], transparent: [0, 0, 0, 0]
  };
  if (named[s]) return named[s].slice();
  if (s.charAt(0) === "#") {
    var h = s.slice(1);
    if (h.length === 3 || h.length === 4) {
      var out = [];
      for (var i = 0; i < h.length; i++) out.push(parseInt(h.charAt(i) + h.charAt(i), 16));
      if (out.length === 3) out.push(255);
      return out;
    }
    if (h.length === 6 || h.length === 8) {
      var out6 = [];
      for (var j = 0; j < h.length; j += 2) out6.push(parseInt(h.substr(j, 2), 16));
      if (out6.length === 3) out6.push(255);
      return out6;
    }
  }
  var m = s.match(/^rgba?\(([^)]+)\)$/);
  if (m) {
    var parts = m[1].split(",");
    if (parts.length >= 3) {
      var alpha = parts.length > 3 ? Math.round(parseFloat(parts[3]) * 255) : 255;
      return [parseInt(parts[0], 10) || 0, parseInt(parts[1], 10) || 0,
              parseInt(parts[2], 10) || 0, isNaN(alpha) ? 255 : alpha];
    }
  }
  return [0, 0, 0, 255];
}

function __canvasClampByte(v) { return Math.max(0, Math.min(255, Math.round(v))); }

function __canvasBlend(ctx, x, y, color, alphaScale) {
  var canvas = ctx.canvas;
  x |= 0; y |= 0;
  if (x < 0 || y < 0 || x >= canvas.width || y >= canvas.height) return;
  var idx = (y * canvas.width + x) * 4;
  var p = ctx.__pixels;
  var sa = (color[3] / 255) * ctx.globalAlpha * (alphaScale == null ? 1 : alphaScale);
  if (!(sa >= 0)) sa = 0;
  sa = Math.max(0, Math.min(1, sa));
  var da = p[idx + 3] / 255;
  var oa = sa + da * (1 - sa);
  if (oa <= 0) {
    p[idx] = p[idx + 1] = p[idx + 2] = p[idx + 3] = 0;
    return;
  }
  for (var c = 0; c < 3; c++) {
    p[idx + c] = __canvasClampByte((color[c] * sa + p[idx + c] * da * (1 - sa)) / oa);
  }
  p[idx + 3] = __canvasClampByte(oa * 255);
}

function __canvasRect(x, y, w, h) {
  x = Number(x) || 0; y = Number(y) || 0; w = Number(w) || 0; h = Number(h) || 0;
  if (w < 0) { x += w; w = -w; }
  if (h < 0) { y += h; h = -h; }
  return [Math.round(x), Math.round(y), Math.round(w), Math.round(h)];
}

function __mkCanvas2d(canvas) {
  var noop = function () {};
  var ctx = {
    canvas: canvas,
    __pixels: new Uint8ClampedArray(canvas.width * canvas.height * 4),
    fillStyle: "#000000", strokeStyle: "#000000", lineWidth: 1, lineCap: "butt", lineJoin: "miter",
    miterLimit: 10, font: "10px sans-serif", textAlign: "start", textBaseline: "alphabetic",
    globalAlpha: 1, globalCompositeOperation: "source-over", imageSmoothingEnabled: true,
    shadowBlur: 0, shadowColor: "rgba(0,0,0,0)", shadowOffsetX: 0, shadowOffsetY: 0,
    save: noop, restore: noop, beginPath: noop, closePath: noop, moveTo: noop, lineTo: noop,
    quadraticCurveTo: noop, bezierCurveTo: noop, arc: noop, arcTo: noop, ellipse: noop, rect: noop,
    fill: noop, stroke: noop, clip: noop,
    translate: noop, rotate: noop, scale: noop, transform: noop, setTransform: noop, resetTransform: noop,
    setLineDash: noop, getLineDash: function () { return []; },
    fillText: noop, strokeText: noop,
    measureText: function (t) {
      var size = parseFloat(ctx.font) || 10;
      return { width: String(t == null ? "" : t).length * size * 0.55 };
    },
    createLinearGradient: function () { return { addColorStop: noop }; },
    createRadialGradient: function () { return { addColorStop: noop }; },
    createPattern: function () { return {}; },
    isPointInPath: function () { return false; },
    isPointInStroke: function () { return false; },
    getContextAttributes: function () { return { alpha: true }; }
  };
  function dirty() { ctx.__dirty = true; }
  ctx.clearRect = function (x, y, w, h) {
    var r = __canvasRect(x, y, w, h);
    for (var yy = r[1]; yy < r[1] + r[3]; yy++) {
      if (yy < 0 || yy >= canvas.height) continue;
      for (var xx = r[0]; xx < r[0] + r[2]; xx++) {
        if (xx < 0 || xx >= canvas.width) continue;
        var idx = (yy * canvas.width + xx) * 4;
        ctx.__pixels[idx] = ctx.__pixels[idx + 1] = ctx.__pixels[idx + 2] = ctx.__pixels[idx + 3] = 0;
      }
    }
    dirty();
  };
  ctx.fillRect = function (x, y, w, h) {
    var r = __canvasRect(x, y, w, h);
    var color = __canvasColor(ctx.fillStyle);
    for (var yy = r[1]; yy < r[1] + r[3]; yy++) {
      for (var xx = r[0]; xx < r[0] + r[2]; xx++) __canvasBlend(ctx, xx, yy, color, 1);
    }
    dirty();
  };
  ctx.strokeRect = function (x, y, w, h) {
    var old = ctx.fillStyle;
    ctx.fillStyle = ctx.strokeStyle;
    var lw = Math.max(1, Number(ctx.lineWidth) || 1);
    ctx.fillRect(x, y, w, lw);
    ctx.fillRect(x, y + h - lw, w, lw);
    ctx.fillRect(x, y, lw, h);
    ctx.fillRect(x + w - lw, y, lw, h);
    ctx.fillStyle = old;
  };
  ctx.createImageData = function (w, h) {
    if (typeof w === "object" && w) { h = w.height; w = w.width; }
    w = Math.max(0, w | 0); h = Math.max(0, h | 0);
    return { width: w, height: h, data: new Uint8ClampedArray(w * h * 4) };
  };
  ctx.getImageData = function (x, y, w, h) {
    x |= 0; y |= 0; w = Math.max(0, w | 0); h = Math.max(0, h | 0);
    var out = ctx.createImageData(w, h);
    for (var yy = 0; yy < h; yy++) {
      for (var xx = 0; xx < w; xx++) {
        var sx = x + xx, sy = y + yy;
        if (sx < 0 || sy < 0 || sx >= canvas.width || sy >= canvas.height) continue;
        var si = (sy * canvas.width + sx) * 4, di = (yy * w + xx) * 4;
        for (var c = 0; c < 4; c++) out.data[di + c] = ctx.__pixels[si + c];
      }
    }
    return out;
  };
  ctx.putImageData = function (image, x, y) {
    if (!image || !image.data) return;
    x |= 0; y |= 0;
    var w = image.width | 0, h = image.height | 0;
    for (var yy = 0; yy < h; yy++) {
      for (var xx = 0; xx < w; xx++) {
        var dx = x + xx, dy = y + yy;
        if (dx < 0 || dy < 0 || dx >= canvas.width || dy >= canvas.height) continue;
        var si = (yy * w + xx) * 4, di = (dy * canvas.width + dx) * 4;
        for (var c = 0; c < 4; c++) ctx.__pixels[di + c] = image.data[si + c];
      }
    }
    dirty();
  };
  ctx.drawImage = function (source) {
    // 第一阶段支持 Canvas→Canvas;远程图片仍走宿主 <img>/CSS 图片管线。
    if (!source || !source.__ctx2d) return;
    var a = arguments;
    var sx = 0, sy = 0, sw = source.width, sh = source.height;
    var dx, dy, dw, dh;
    if (a.length === 3) {
      dx = a[1]; dy = a[2]; dw = sw; dh = sh;
    } else if (a.length === 5) {
      dx = a[1]; dy = a[2]; dw = a[3]; dh = a[4];
    } else if (a.length >= 9) {
      sx = a[1]; sy = a[2]; sw = a[3]; sh = a[4];
      dx = a[5]; dy = a[6]; dw = a[7]; dh = a[8];
    } else return;
    var sp = source.__ctx2d.__pixels;
    for (var yy = 0; yy < Math.round(dh); yy++) {
      for (var xx = 0; xx < Math.round(dw); xx++) {
        var srcX = Math.floor(sx + xx * sw / dw), srcY = Math.floor(sy + yy * sh / dh);
        if (srcX < 0 || srcY < 0 || srcX >= source.width || srcY >= source.height) continue;
        var si = (srcY * source.width + srcX) * 4;
        __canvasBlend(ctx, Math.round(dx) + xx, Math.round(dy) + yy,
                      [sp[si], sp[si + 1], sp[si + 2], sp[si + 3]], 1);
      }
    }
    dirty();
  };
  return ctx;
}
"##;
