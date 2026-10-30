//! 京东真机「显示不全/布局混乱」回归单测(2026-10-30,真实 CSS 原文)。
//!
//! 真机证据(`MCP_Web_LsmAgentBrowser_Use_jd_02` + 浏览器 dump/inspect):
//! 1. 服务承诺文案「品类齐全，轻松购物」末字被裁:`.mod_service_unit{width:180px;
//!    padding-left:45px}` 未声明 box-sizing,Chrome 按初始值 content-box 给内容宽
//!    180px,内核旧默认 border-box 给 135px,`p{white-space:nowrap}` 9 字 162px
//!    硬裁;
//! 2. 登录页「密码登录/短信登录」逐字换行撑爆 `.login-box`(height 2661):
//!    `.form-login-type` 是 flex 项(display:inline-block; font-size:20px;
//!    line-height:16px),内核把 flex 项基线宽压成 ~1 字宽 → CJK 逐字断行。
//!
//! 本文件按约定放同级兄弟文件(tests.rs 已超 1800 行),只收真实 CSS 原文的
//! 几何断言;修复在 values.rs / flex.rs 等通用层,无站点特判。

use super::{compute, Layout};
use crate::agent::browser::document::Document;
use crate::agent::browser::render::FontSet;

fn font() -> FontSet {
    FontSet::fallback()
}

fn g(layout: &Layout, sel: &str) -> (f64, f64, f64, f64) {
    layout
        .rect_for(sel)
        .unwrap_or_else(|| panic!("缺盒子:{sel}"))
}

// ===================== 服务承诺文案盒宽(content-box 语义) =====================

#[test]
fn 京东服务承诺_unit宽180含padding45_文案盒应为180不被压到135() {
    // `.mod_service_txt` 的 clip 宽 = 父 `.mod_service_unit` 内容宽。
    // box-sizing 未声明 → CSS 初始值 content-box → 内容宽 = width = 180。
    let html = r#"<html><head><style>
.mod_service_list { overflow: hidden; height: 42px; }
.mod_service_item { float: left; width: 297px; }
.mod_service_unit { position: relative; margin: 0 auto; padding-left: 45px; width: 180px; }
.mod_service_txt { overflow: hidden; width: 100%; height: 42px; line-height: 42px;
  font-size: 18px; font-weight: 700; text-overflow: ellipsis; white-space: nowrap; color: #444; }
</style></head><body>
<div class="mod_service_list">
  <div class="mod_service_item">
    <div class="mod_service_unit">
      <p class="mod_service_txt">品类齐全，轻松购物</p>
    </div>
  </div>
</div>
</body></html>"#;
    let doc = Document::parse(html, "https://a.test/");
    let layout = compute(&doc, 1200.0, &font());
    let unit = g(&layout, "html > body > div > div > div");
    assert!(
        (unit.2 - 225.0).abs() < 0.5,
        "content-box 语义下 .mod_service_unit 总宽 = 180+45 = 225(Chrome computed 同),实际 {unit:?}"
    );
    let txt = g(&layout, "html > body > div > div > div > p");
    assert!(
        (txt.2 - 180.0).abs() < 0.5,
        "p 的 width:100% 应相对父**内容宽** 180 求解(Chrome 实测 content-box),实际 {txt:?}"
    );
}

// ===================== 登录页 flex 项逐字换行(真实 CSS 原文) =====================

#[test]
fn 京东登录tab_flex行内块项按maxcontent出宽_不逐字换行() {
    // passport 登录页 login2024.css 原文 + 静态壳原结构。
    let html = r#"<html><head><style>
.login-form .login-box{width:306px;padding:20px}
.form-login-type-toggle{display:-ms-flexbox;display:flex;-ms-flex-pack:center;justify-content:center;-ms-flex-align:center;align-items:center;margin-bottom:16px}
.form-login-type-toggle .form-login-type{display:inline-block;font-family:PingFang SC;font-size:20px;font-weight:600;line-height:16px;margin-right:20px;cursor:pointer;color:#1A1A1A}
.form-login-type-toggle .select-toggle{color:#FA2C19;font-weight:600}
</style></head><body>
<div class="login-form"><div class="login-box">
  <div class="form-login-type-toggle">
    <div id="pwd-login" class="form-login-type select-toggle">密码登录</div>
    <div id="sms-login" class="form-login-type">短信登录</div>
  </div>
</div></div>
</body></html>"#;
    let doc = Document::parse(html, "https://a.test/");
    let layout = compute(&doc, 1200.0, &font());
    let pwd = g(&layout, "#pwd-login");
    // 「密码登录」4 字 × 20px ≈ 80px:行内块按 max-content 出基线宽,
    // 逐字换行(每行 1 字)会得到 ~20px 宽 × 4 行高——即真机 2661 高的根形态。
    assert!(
        pwd.2 > 70.0,
        "flex 项基线宽应≈max-content(4字×20px≈80),不被压成逐字换行的 ~20px: {pwd:?}"
    );
    assert!(
        pwd.3 < 32.0,
        "单行文本盒高应≈行高(20px 字号),逐字换行会到 4 行 ≈64px+: {pwd:?}"
    );
    let sms = g(&layout, "#sms-login");
    assert!(sms.2 > 70.0, "同上: {sms:?}");
}

// ===================== JS 懒加载图片兜底(2026-10-30 京东) =====================

#[test]
fn 懒加载data_src族参与图片预取收集() {
    use crate::agent::browser::images::collect_image_urls;
    let html = r#"<html><body>
<img src="https://img.example/real.png" data-src="https://img.example/lazy.png">
<img data-lazyimg='{"src":"https://img.example/j.png"}'>
<img data-original="https://img.example/o.png">
</body></html>"#;
    let urls = collect_image_urls(html, "https://a.test/", 1200.0);
    for want in [
        "https://img.example/real.png",
        "https://img.example/lazy.png",
        "https://img.example/j.png",
        "https://img.example/o.png",
    ] {
        assert!(urls.iter().any(|u| u == want), "预取应含 {want}: {urls:?}");
    }
}

#[test]
fn lazy_attr_url_json与裸值归一() {
    use crate::agent::browser::urlutil::lazy_attr_url;
    assert_eq!(lazy_attr_url("https://a/x.png"), "https://a/x.png");
    assert_eq!(
        lazy_attr_url(r#"{"src":"https://a/j.png","w":300}"#),
        "https://a/j.png"
    );
    assert_eq!(lazy_attr_url(r#"{"other":1}"#), r#"{"other":1}"#);
}
