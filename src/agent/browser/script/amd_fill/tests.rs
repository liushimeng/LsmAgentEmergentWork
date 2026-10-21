//! amd_fill.rs 测试：依赖数组补齐的正确性与安全性。

use super::*;

#[test]
fn 无define原样返回() {
    let src = "var a = 1; console.log(a);";
    assert_eq!(amd_fill_deps(src), src);
}

#[test]
fn 已有deps数组不触碰() {
    let src = "define('m', ['a', 'b'], function(a, b) { return a + b; });";
    assert_eq!(amd_fill_deps(src), src);
}

#[test]
fn 无deps工厂补齐三标识符与静态依赖() {
    let src = "define('m', function(require, exports, module) {\n\
                 var util = require('util.js');\n\
                 var other = require('other.js');\n\
                 exports.x = 1;\n\
               });";
    let out = amd_fill_deps(src);
    assert!(out.contains("define('m', ['require', 'exports', 'module', \"util.js\", \"other.js\"], function"),
            "{}", out);
}

#[test]
fn 匿名define也补齐() {
    let src = "define(function(require) { require('a'); });";
    let out = amd_fill_deps(src);
    assert!(out.contains("define(['require', 'exports', 'module', \"a\"], function"), "{}", out);
}

#[test]
fn 动态require与字符串内require不误收集() {
    let src = "define('m', function(require) {\n\
                 var name = 'x';\n\
                 require(name);\n\
                 var s = \"require('fake')\";\n\
                 // require('commented')\n\
               });";
    let out = amd_fill_deps(src);
    assert!(out.contains("['require', 'exports', 'module'], function"), "{}", out);
    assert!(!out.contains("\"fake\""), "{}", out);
    assert!(!out.contains("\"commented\""), "{}", out);
}

#[test]
fn 嵌套函数内的require也收集() {
    let src = "define('m', function(require, exports, module) {\n\
                 function init() { var t = require('inner/mod.js'); return t; }\n\
                 exports.init = init;\n\
               });";
    let out = amd_fill_deps(src);
    assert!(out.contains("\"inner/mod.js\""), "{}", out);
}

#[test]
fn 工厂体逐字节不动() {
    let src = "define('m', function(require, exports, module) {\n\
                 var s = 'define(fake, function(){})';\n\
                 exports.s = s;\n\
               });\n\
               define('n', ['x'], function() { return 1; });";
    let out = amd_fill_deps(src);
    assert!(out.contains("var s = 'define(fake, function(){})';"), "{}", out);
    assert!(out.contains("define('n', ['x'], function() { return 1; });"), "{}", out);
}

#[test]
fn 多个define各自补齐() {
    let src = ";define('a', function(require) { require('one'); });\n\
               ;define('b', function(require) { require('two'); });";
    let out = amd_fill_deps(src);
    assert!(out.contains("define('a', ['require', 'exports', 'module', \"one\"], function"), "{}", out);
    assert!(out.contains("define('b', ['require', 'exports', 'module', \"two\"], function"), "{}", out);
}

#[test]
fn 重复require去重() {
    let src = "define('m', function(require) { require('dup'); require('dup'); });";
    let out = amd_fill_deps(src);
    let n = out.matches("\"dup\"").count();
    assert_eq!(n, 1, "{}", out);
}

/// 真实 tangram 文件探针（env 门控，不入库）：验证补齐结果可被站点装载器消费。
/// 用法：`LAEW_DIAG_LIB=/path/to/libs.js cargo test --lib 真实tangram -- --nocapture --ignored`
#[test]
#[ignore]
fn 真实tangram工厂补齐验证() {
    let Ok(path) = std::env::var("LAEW_DIAG_LIB") else {
        eprintln!("skip: 未设置 LAEW_DIAG_LIB");
        return;
    };
    let src = std::fs::read_to_string(&path).expect("读取文件");
    let out = amd_fill_deps(&src);
    assert!(
        out.contains("define('common:widget/ui/tangram/tangram.js', ['require', 'exports', 'module', \"common:widget/ui/util/util.js\""),
        "补齐结果缺失 util.js 依赖,头 300 字符: {}",
        &out[..out.len().min(300)]
    );
    // 已有 deps 的 define 不被破坏。
    assert!(out.contains("define('zepto', [], function"), "{}", out);
    eprintln!("补齐 OK,输出 {} 字节(原 {} 字节)", out.len(), src.len());
}
