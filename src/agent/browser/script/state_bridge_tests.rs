//! 第 42 轮:dialog 方法桥回归。
//!
//! `show/showModal/close` 必须是普通函数属性;第 41 轮曾用 defineProperty
//! 存取器实现,页面脚本 `dialog.showModal()` 实际调用 getter 返回的 undefined,
//! 标准模态链路当场 TypeError。

use super::tests::eval_on;
use super::JsHandle;

const DIALOG_PAGE: &str =
    "<html><head><title>对话框</title></head><body><dialog id='d'>内容</dialog></body></html>";

#[test]
fn dialog三方法是普通函数() {
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        "var d = document.querySelector('#d'); [typeof d.show, typeof d.showModal, typeof d.close].join(',')",
        DIALOG_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    assert_eq!(reply.outcome.value, "function,function,function");
    handle.shutdown();
}

#[test]
fn showModal产生打开与模态标记动作() {
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        "document.querySelector('#d').showModal();",
        DIALOG_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    assert!(reply
        .outcome
        .actions
        .iter()
        .any(|a| a.kind == "set_attr" && a.key == "open"));
    assert!(reply
        .outcome
        .actions
        .iter()
        .any(|a| a.kind == "set_attr" && a.key == "data-lsm-modal"));
    handle.shutdown();
}

#[test]
fn show只产生打开动作不写模态标记() {
    let handle = JsHandle::spawn();
    let reply = eval_on(
        &handle,
        "document.querySelector('#d').show();",
        DIALOG_PAGE,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    assert!(reply
        .outcome
        .actions
        .iter()
        .any(|a| a.kind == "set_attr" && a.key == "open"));
    assert!(!reply
        .outcome
        .actions
        .iter()
        .any(|a| a.key == "data-lsm-modal"), "非模态 show 不应铺 backdrop");
    handle.shutdown();
}

#[test]
fn close移除打开与模态标记动作() {
    let handle = JsHandle::spawn();
    let page = "<html><body><dialog id='d' open data-lsm-modal>内容</dialog></body></html>";
    let reply = eval_on(
        &handle,
        "document.querySelector('#d').close();",
        page,
        "https://a.test/",
    );
    assert!(reply.outcome.ok, "{:?}", reply.outcome.error);
    assert!(reply
        .outcome
        .actions
        .iter()
        .any(|a| a.kind == "remove_attr" && a.key == "open"));
    assert!(reply
        .outcome
        .actions
        .iter()
        .any(|a| a.kind == "remove_attr" && a.key == "data-lsm-modal"));
    handle.shutdown();
}
