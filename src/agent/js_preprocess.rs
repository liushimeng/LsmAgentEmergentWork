//! eval_js 脚本预处理(第 181 轮,2026-10-27 京东实测)。
//!
//! 两浏览器内核(Chrome/CDP 与 Boa)的 `eval_js` 求值的都是**完整程序**,
//! 顶层 `return` 是语法错误。模型(尤其是每个新开的 SubAgent 会话)常写
//! `const out=[]; …; return JSON.stringify(out)` 这类「函数体风格」脚本,
//! 实测每个会话都要为 `SyntaxError: Illegal return statement` 白费 1~2 轮
//! LLM 往返后才想起 IIFE。顶层 return 的脚本**必然编译失败**,自动包一层
//! IIFE 语义等价(返回值即求值结果);唯一差异是顶层 `var` 不再泄漏全局,
//! 而带顶层 return 的脚本本就是按函数体写的,不受影响。

/// 检测脚本是否含**顶层** return(行首语句 + 花括号深度 0 的近似判定)。
///
/// 深度按行内 `{`/`}` 计数累计,不解析字符串/模板字面量——误差方向是**漏判**
/// (误把函数内 return 判为顶层),漏判时不包裹,行为与未预处理完全一致,
/// 由工具错误信息兜底提示;误判方向(顶层被判为函数内)不存在,因为包裹
/// 整个程序语义不变。
pub fn has_top_level_return(script: &str) -> bool {
    let mut depth: i64 = 0;
    for line in script.lines() {
        let trimmed = line.trim_start();
        if depth <= 0
            && (trimmed.starts_with("return ")
                || trimmed.starts_with("return;")
                || trimmed == "return"
                || trimmed.starts_with("return\t"))
        {
            return true;
        }
        for ch in line.chars() {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
    }
    false
}

/// 顶层 return 存在时把整个脚本包成 IIFE;否则原样返回。
pub fn wrap_top_level_return(script: &str) -> String {
    if has_top_level_return(script) {
        format!("(function(){{\n{script}\n}})()")
    } else {
        script.to_string()
    }
}

/// Console 面板实时 JS 交互的求值包装(58 号设计文档)。
///
/// 语义对齐 Chrome DevTools Console:
/// * **表达式/语句均可**:经间接 eval(`(0,eval)`)在**页面全局作用域**执行,
///   `var`/`function` 声明落全局、`x = 1` 隐式全局照常持久,与 Chrome 一致;
/// * **快捷语法**:`$$()` 等效 `document.querySelectorAll`、`$_` 上一条表达式
///   返回值、`$0` 最近选中元素(`last_sel` 为宿主侧稳定选择器,取不到为 null);
///   以全局遮蔽方式注入(Chrome 控制台同语义——控制台快捷量遮蔽站点同名),
///   求值结束(含抛错)在 `finally` 里**恢复站点原值**;
/// * **返回值**经 `__str`(PRELUDE 字符串化)渲染成字符串;异常原样抛出,
///   由 `act_eval` 既有错误链路收口(不在此层吞)。
pub fn wrap_console_eval(expr: &str, last_sel: Option<&str>) -> String {
    let expr_lit = serde_json::to_string(expr).unwrap_or_else(|_| "\"\"".to_string());
    let sel_lit = match last_sel {
        Some(s) => serde_json::to_string(s).unwrap_or_else(|_| "null".to_string()),
        None => "null".to_string(),
    };
    format!(
        r#"(function(){{
  var G = (typeof globalThis !== "undefined") ? globalThis : this;
  var __str2 = (typeof __str === "function") ? __str : function (x) {{
    if (x === null) return "null";
    if (x === undefined) return "undefined";
    try {{ return JSON.stringify(x); }} catch (e) {{ return String(x); }}
  }};
  var __names = ["$$", "$_", "$0", "$1", "$2", "$3", "$4"];
  var __saved = {{}};
  for (var __i = 0; __i < __names.length; __i++) {{
    var __k = __names[__i];
    if (Object.prototype.hasOwnProperty.call(G, __k)) __saved[__k] = {{ had: true, v: G[__k] }};
    else __saved[__k] = {{ had: false }};
  }}
  try {{
    G.$$ = function (s) {{ return document.querySelectorAll(s); }};
    G.$_ = G.__lsmLastEvalRet;
    G.$0 = ({sel_lit} !== null && typeof {sel_lit} === "string") ? document.querySelector({sel_lit}) : null;
    G.$1 = null; G.$2 = null; G.$3 = null; G.$4 = null;
    var __r = (0, eval)({expr_lit});
    G.__lsmLastEvalRet = __r;
    return __str2(__r);
  }} finally {{
    for (var __j = 0; __j < __names.length; __j++) {{
      var __kk = __names[__j];
      if (__saved[__kk] && __saved[__kk].had) G[__kk] = __saved[__kk].v;
      else delete G[__kk];
    }}
  }}
}})()"#,
        expr_lit = expr_lit,
        sel_lit = sel_lit
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 顶层_return_被检测并包裹() {
        // 京东实测的典型失败脚本(Plan iter=11 原型)
        let s = "const out=[];document.querySelectorAll('li').forEach(li=>{out.push(li.textContent)});\nreturn JSON.stringify(out)";
        assert!(has_top_level_return(s));
        let wrapped = wrap_top_level_return(s);
        assert!(wrapped.starts_with("(function(){"), "{wrapped}");
        assert!(wrapped.ends_with("})()"), "{wrapped}");
    }

    #[test]
    fn 函数内_return_不包裹() {
        let s = "(function(){\nconst out=[];\nreturn JSON.stringify(out);\n})()";
        assert!(!has_top_level_return(s), "函数体内的 return 不算顶层");
        assert_eq!(wrap_top_level_return(s), s);
        let arrow = "const f = x => {\n  return x + 1;\n};\nf(1)";
        assert!(!has_top_level_return(arrow));
    }

    #[test]
    fn 混合_回调箭头内的行首_return_不包裹() {
        // forEach 回调体从新行开始写 return:深度 1,不是顶层
        let s = "const out=[];\nitems.forEach(li => {\nreturn out.push(li)\n});\nout.length";
        assert!(!has_top_level_return(s), "{s}");
    }

    #[test]
    fn 顶层_if_内单行_return_不检测_但也不误伤() {
        // `if (x) return y;` 单行形式不在检测范围(漏判方向,由错误信息兜底)
        let s = "const a=1;\nif (a) return 2;";
        assert!(!has_top_level_return(s));
    }

    #[test]
    fn 首行_return_与_return_分号变体() {
        assert!(has_top_level_return("return 1"));
        assert!(has_top_level_return("  return;"));
        assert!(has_top_level_return("return\tx"));
        assert!(!has_top_level_return("returnOf(1)"));
        assert!(!has_top_level_return(""));
    }

    #[test]
    fn console求值包装_表达式转义与快捷语法() {
        // 表达式以 JSON 字符串字面量嵌入:引号/反斜杠/换行安全。
        let w = wrap_console_eval("a\"b\\c\nd", None);
        assert!(w.contains(r#""a\"b\\c\nd""#), "表达式按 JSON 字面量嵌入:{w}");
        // 快捷语法:$$ = querySelectorAll;$_ = 上一条返回值;$0 无选择器时为 null。
        assert!(w.contains("document.querySelectorAll"), "{w}");
        assert!(w.contains("__lsmLastEvalRet"), "$_ 读上一条返回值:{w}");
        assert!(w.contains("G.$0 = (null !== null"), "无选择器 $0 为 null:{w}");
        // 间接 eval:全局作用域(语句/声明持久)。
        assert!(w.contains("(0, eval)"), "{w}");
        // finally 恢复站点原值。
        assert!(w.contains("finally"), "{w}");
    }

    #[test]
    fn console求值包装_0号元素选择器嵌入() {
        let w = wrap_console_eval("1", Some("#main"));
        assert!(w.contains(r##"document.querySelector("#main")"##), "{w}");
        // 选择器自身带引号时 JSON 转义仍安全。
        let w2 = wrap_console_eval("1", Some(r#"a"b"#));
        assert!(w2.contains(r##"document.querySelector("a\"b")"##), "{w2}");
        // 顶层 return 检测不得误伤包装(返回语句都在花括号内)。
        assert!(!has_top_level_return(&wrap_console_eval("return 1", None)));
    }
}
