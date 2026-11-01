//! Preact 调度兼容守卫。
//!
//! 京东使用的 Preact 变体在 passive effect 首步同步 `setState` 时，Boa
//! 原生 Promise reaction 会把首次 effect 反复重入，最终触发递归上限。
//! 只把识别明确的 Preact `nextTick` 延后到后续任务，不改写其他 Promise 链。

const NEXT_TICK_DECL: &str = "var nextTick=function";
const EFFECT_SCHEDULER_DECL: &str = "function invokeScheduleEffects";
const RENDER_QUEUE_DECL: &str = "function enqueueRender";
const NATIVE_NEXT_TICK: &str = "return gn.then(ee);";
const DEFERRED_NEXT_TICK: &str = "return new Promise(function(__lsmResolve,__lsmReject){setTimeout(function(){try{__lsmResolve(ee())}catch(__lsmError){__lsmReject(__lsmError)}},0);});";

/// 延迟 Preact 渲染回调，同时保留 nextTick 的 Promise 返回值与异常通道。
pub(super) fn defer_next_tick(source: &str) -> String {
    if !source.contains(NEXT_TICK_DECL)
        || !source.contains(EFFECT_SCHEDULER_DECL)
        || !source.contains(RENDER_QUEUE_DECL)
    {
        return source.to_string();
    }
    source.replace(NATIVE_NEXT_TICK, DEFERRED_NEXT_TICK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 完整preact特征才把nextTick改为延迟任务() {
        let source = "var nextTick=function(ee){return gn.then(ee);};function invokeScheduleEffects(){}function enqueueRender(){}";
        let guarded = defer_next_tick(source);
        assert!(!guarded.contains(NATIVE_NEXT_TICK));
        assert!(guarded.contains("new Promise"));
        assert!(guarded.contains("setTimeout"));
        assert!(guarded.contains("__lsmReject(__lsmError)"));
    }

    #[test]
    fn 普通promise链不做源码改写() {
        let source = "var nextTick=function(ee){return gn.then(ee);};";
        assert_eq!(defer_next_tick(source), source);
    }
}
