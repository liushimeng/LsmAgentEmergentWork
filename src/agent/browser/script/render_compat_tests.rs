//! 二进制字符串与 Canvas 真实像素回归。与超线 tests.rs 同级拆分。

use super::tests::{eval_on, run_scripts};
use super::JsHandle;

const CANVAS_PAGE: &str = r#"<html><body><canvas id="c" width="4" height="2"></canvas><div id="root"></div></body></html>"#;

#[test]
fn base64_二进制字符串全码元往返且不utf8化() {
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        r#"
        var raw = '';
        for (var i = 0; i < 256; i++) raw += String.fromCharCode(i);
        var decoded = atob(btoa(raw));
        var same = decoded.length === 256;
        for (var j = 0; j < 256; j++) same = same && decoded.charCodeAt(j) === j;
        var unicodeRejected = false;
        try { btoa('中'); } catch (e) { unicodeRejected = true; }
        (btoa('ÿ\u0000\u0080') === '/wCA' && same && unicodeRejected) ? 'ok' : 'bad';
        "#,
        CANVAS_PAGE,
        "https://a.test/",
    );
    assert_eq!(reply.outcome.value, "ok", "{:?}", reply.outcome.error);
    handle.shutdown();
}

#[test]
fn base64_atob_接受空白与省略_padding() {
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        "atob(' YQ ').charCodeAt(0) === 97 && atob('YWI').length === 2 ? 'ok' : 'bad'",
        CANVAS_PAGE,
        "https://a.test/",
    );
    assert_eq!(reply.outcome.value, "ok", "{:?}", reply.outcome.error);
    handle.shutdown();
}

#[test]
fn canvas_静态元素真实像素且跨快照查询保留() {
    let handle = JsHandle::spawn();
    let first = eval_on(
        &handle,
        r#"
        var c = document.getElementById('c');
        var ctx = c.getContext('2d');
        ctx.fillStyle = '#ff0000';
        ctx.fillRect(0, 0, 2, 1);
        Array.prototype.join.call(ctx.getImageData(0, 0, 1, 1).data, ',');
        "#,
        CANVAS_PAGE,
        "https://a.test/",
    );
    assert_eq!(first.outcome.value, "255,0,0,255", "{:?}", first.outcome.error);
    assert!(first.outcome.actions.iter().any(|a| {
        a.kind == "set_attr" && a.key == "data-lsm-canvas" && a.value.starts_with("data:image/png;base64,")
    }));
    let second = eval_on(
        &handle,
        "Array.prototype.join.call(document.getElementById('c').getContext('2d').getImageData(0, 0, 1, 1).data, ',')",
        CANVAS_PAGE,
        "https://a.test/",
    );
    assert_eq!(second.outcome.value, "255,0,0,255", "重新查询包装后像素丢失");
    let dims = eval_on(&handle, "document.getElementById('c').width + 'x' + document.getElementById('c').height", CANVAS_PAGE, "https://a.test/");
    assert_eq!(dims.outcome.value, "4x2", "静态 Canvas 固有尺寸读取错误");
    handle.shutdown();
}

#[test]
fn canvas_动态元素真实_png_与canvas间贴图() {
    let handle = JsHandle::spawn();
    let reply = run_scripts(
        &handle,
        vec![r#"
        var src = document.createElement('canvas');
        src.width = 2; src.height = 2;
        var sctx = src.getContext('2d');
        sctx.fillStyle = 'rgba(0,0,255,1)'; sctx.fillRect(0, 0, 2, 2);
        var dst = document.createElement('canvas');
        dst.width = 4; dst.height = 4; dst.id = 'dst';
        document.getElementById('root').appendChild(dst);
        var dctx = dst.getContext('2d');
        dctx.drawImage(src, 0, 0, 4, 4);
        window.__pixel = Array.prototype.join.call(dctx.getImageData(3, 3, 1, 1).data, ',');
        "#],
        CANVAS_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    let png = reply
        .outcome
        .actions
        .iter()
        .find(|a| a.kind == "set_attr" && a.key == "data-lsm-canvas")
        .unwrap_or_else(|| panic!("动态 Canvas 未回写 PNG:{:?}", reply.outcome.actions));
    let (bytes, mime) = crate::agent::browser::images::decode_data_url(&png.value).expect("应产生 PNG data URL");
    assert_eq!(mime, "image/png");
    let image = image::load_from_memory(&bytes).expect("Canvas PNG 应可解码").to_rgba8();
    assert_eq!(image.get_pixel(3, 3).0, [0, 0, 255, 255]);
    let pixel = eval_on(&handle, "window.__pixel", CANVAS_PAGE, "https://a.test/");
    assert_eq!(pixel.outcome.value, "0,0,255,255");
    handle.shutdown();
}

#[test]
fn canvas_尺寸重置清空像素且缓冲长度同步() {
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        r#"
        var c = document.getElementById('c');
        var ctx = c.getContext('2d');
        ctx.fillStyle = 'red'; ctx.fillRect(0, 0, 4, 2);
        c.height = 3;
        var p = ctx.getImageData(0, 0, 1, 1).data;
        (ctx.getImageData(0, 0, 4, 3).data.length === 48 && p[3] === 0) ? 'ok' : 'bad';
        "#,
        CANVAS_PAGE,
        "https://a.test/",
    );
    assert_eq!(reply.outcome.value, "ok", "{:?}", reply.outcome.error);
    handle.shutdown();
}
