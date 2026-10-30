//! 临时真站探针 #4:钩 __lsmFetchNative 抓 GeeTest JSONP 响应体(验完即删)。

use lsm_agent::agent::tools::mcp_web_lsm_browser_use::McpWebLsmBrowserUseTool;
use lsm_agent::agent::tools::Tool;
use serde_json::{json, Value};

const PROBE_DIR: &str = "/Users/dev02/Desktop/MyLocalGit/LsmBashDir/Dir_LsmAgentBrowser_zhipin_01/probe4";

async fn exec(tool: &McpWebLsmBrowserUseTool, args: Value) -> Value {
    let text = tool.execute(args).await.unwrap();
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("返回不是 JSON:{e}\n{text}"))
}

#[tokio::test]
#[ignore] // 真站探针:显式 --ignored 运行
async fn zhipin_geetest_响应体取证() {
    std::fs::create_dir_all(PROBE_DIR).unwrap();
    std::env::set_var("LAEW_JS_CHAIN_TRACE", "1");

    let tool = McpWebLsmBrowserUseTool;
    let open = exec(
        &tool,
        json!({"action": "open", "url": "https://www.zhipin.com", "mode": "hidden", "reuse": false}),
    )
    .await;
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();
    println!("page_id={page_id}");

    // 钩住原生 fetch 桥:把 geetest/captcha 相关响应体前 500 字符打进 console。
    let hook_script = r#"(function(){
      var o = globalThis.__lsmFetchNative;
      if (!o) return 'no-native';
      if (o.__hooked) return 'already';
      var w = function(u, s) {
        var r = o(u, s);
        try {
          var us = String(u);
          if (us.indexOf('geetest') >= 0 || us.indexOf('captcha') >= 0) {
            console.log('[fetchbody] ' + us.slice(0, 150) + ' => ' + String(r).slice(0, 500));
          }
        } catch (e) {}
        return r;
      };
      w.__hooked = 1;
      globalThis.__lsmFetchNative = w;
      return 'hooked';
    })()"#;
    let hook = exec(
        &tool,
        json!({"action": "control", "control_action": "eval_js", "page_id": page_id,
               "params": {"expression": hook_script}}),
    )
    .await;
    println!("hook={}", serde_json::to_string(&hook).unwrap());

    // 等待一轮 reset 循环自然发生(不点击,观察稳态)
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    let console1 = exec(
        &tool,
        json!({"action": "inspect", "info": "console", "page_id": page_id,
               "params": {"max_array_items": 2000}}),
    )
    .await;
    std::fs::write(
        format!("{PROBE_DIR}/console_idle.json"),
        serde_json::to_string_pretty(&console1).unwrap(),
    )
    .unwrap();

    // 点击雷达按钮,触发 bind 弹层链
    let click = exec(
        &tool,
        json!({"action": "control", "control_action": "click", "page_id": page_id,
               "selector": "#rowCode"}),
    )
    .await;
    println!("click={}", serde_json::to_string(&click).unwrap());
    tokio::time::sleep(std::time::Duration::from_secs(20)).await;

    let console2 = exec(
        &tool,
        json!({"action": "inspect", "info": "console", "page_id": page_id,
               "params": {"max_array_items": 2000}}),
    )
    .await;
    std::fs::write(
        format!("{PROBE_DIR}/console_click.json"),
        serde_json::to_string_pretty(&console2).unwrap(),
    )
    .unwrap();

    // 顺手取 widget 内部状态
    let state_script = r#"(function(){
      var out = {};
      try {
        out.keys = Object.keys(globalThis).filter(function(k) {
          return /geetest|initG|gt_/i.test(k);
        }).slice(0, 30);
      } catch (e) { out.k = String(e); }
      try {
        var h = document.querySelector('.geetest_holder');
        out.holderClass = h ? h.className : null;
        var t = document.querySelector('.geetest_radar_tip');
        out.tip = t ? t.textContent : null;
        var p = document.querySelector('.geetest_panel');
        out.panel = p ? p.className : null;
      } catch (e) { out.dom = String(e); }
      return JSON.stringify(out);
    })()"#;
    let state = exec(
        &tool,
        json!({"action": "control", "control_action": "eval_js", "page_id": page_id,
               "params": {"expression": state_script}}),
    )
    .await;
    println!("state={}", serde_json::to_string_pretty(&state).unwrap());
    println!("PROBE_DIR={PROBE_DIR}");
}
