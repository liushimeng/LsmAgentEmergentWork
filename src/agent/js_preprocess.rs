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
}
