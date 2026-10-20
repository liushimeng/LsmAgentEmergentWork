//! 资源加载调度器：优先级 + 并发控制。
//!
//! 阶段四：按优先级调度资源加载请求，控制并发数量。
//! 优先级：主文档 > 阻塞 CSS > 字体 > 图片 > 脚本 > 预加载。
//! 同优先级按 FIFO；高优先级可抢占低优先级的信号量槽位。

use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// 资源优先级（数值越小优先级越高）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourcePriority {
    /// 主文档 HTML（最高）。
    Document = 0,
    /// 阻塞渲染的 CSS。
    BlockingCss = 1,
    /// 字体。
    Font = 2,
    /// 图片。
    Image = 3,
    /// 脚本。
    Script = 4,
    /// 预加载（最低）。
    Preload = 5,
}

impl ResourcePriority {
    /// 默认并发上限。
    pub const DEFAULT_CONCURRENCY: usize = 6;

    /// 从 Content-Type 推断优先级。
    pub fn from_content_type(ct: &str, is_script: bool, is_blocking_css: bool) -> Self {
        let ct = ct.to_ascii_lowercase();
        if is_script {
            return Self::Script;
        }
        if ct.contains("html") {
            return Self::Document;
        }
        if is_blocking_css || ct.contains("css") {
            return Self::BlockingCss;
        }
        if ct.contains("font") || ct.contains("ttf") || ct.contains("woff") {
            return Self::Font;
        }
        if ct.starts_with("image/") {
            return Self::Image;
        }
        Self::Preload
    }
}

/// 调度队列条目。
#[derive(Debug, Clone)]
struct QueueEntry {
    priority: ResourcePriority,
    url: String,
    /// 入队序号（FIFO 稳定排序）。
    seq: u64,
}

impl PartialEq for QueueEntry {
    fn eq(&self, other: &Self) -> bool {
        self.seq == other.seq
    }
}

impl Eq for QueueEntry {}

impl PartialOrd for QueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for QueueEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // 优先级高（数值小）的先出队；同优先级按序号 FIFO
        self.priority
            .cmp(&other.priority)
            .then_with(|| self.seq.cmp(&other.seq))
    }
}

/// 资源加载调度器。
#[derive(Debug)]
pub struct ResourceScheduler {
    /// 等待队列（最小堆）。
    queue: BinaryHeap<Reverse<QueueEntry>>,
    /// 入队序号计数器。
    seq: u64,
    /// 并发上限。
    concurrency: usize,
    /// 当前活跃请求数。
    active: usize,
}

impl Default for ResourceScheduler {
    fn default() -> Self {
        Self::new(ResourcePriority::DEFAULT_CONCURRENCY)
    }
}

impl ResourceScheduler {
    /// 创建指定并发上限的调度器。
    pub fn new(concurrency: usize) -> Self {
        Self {
            queue: BinaryHeap::new(),
            seq: 0,
            concurrency: concurrency.max(1),
            active: 0,
        }
    }

    /// 提交加载请求（进入等待队列）。
    pub fn submit(&mut self, url: String, priority: ResourcePriority) {
        self.queue.push(Reverse(QueueEntry {
            priority,
            url,
            seq: self.seq,
        }));
        self.seq += 1;
    }

    /// 是否可以开始下一个请求。
    pub fn can_start(&self) -> bool {
        self.active < self.concurrency && !self.queue.is_empty()
    }

    /// 取出下一个要加载的 URL。
    pub fn next(&mut self) -> Option<String> {
        if self.can_start() {
            self.active += 1;
            self.queue.pop().map(|e| e.0.url)
        } else {
            None
        }
    }

    /// 标记请求完成（释放并发槽位）。
    pub fn finish(&mut self) {
        self.active = self.active.saturating_sub(1);
    }

    /// 等待队列长度。
    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    /// 当前活跃请求数。
    pub fn active_count(&self) -> usize {
        self.active
    }

    /// 并发上限。
    pub fn concurrency_limit(&self) -> usize {
        self.concurrency
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_fifo_同优先级() {
        let mut s = ResourceScheduler::new(2);
        s.submit("http://a.com/1".to_string(), ResourcePriority::Image);
        s.submit("http://a.com/2".to_string(), ResourcePriority::Image);
        s.submit("http://a.com/3".to_string(), ResourcePriority::Image);
        assert_eq!(s.next().as_deref(), Some("http://a.com/1"));
        assert_eq!(s.next().as_deref(), Some("http://a.com/2"));
        // 并发已满
        assert_eq!(s.next(), None);
        s.finish();
        assert_eq!(s.next().as_deref(), Some("http://a.com/3"));
    }

    #[test]
    fn scheduler_优先级排序() {
        let mut s = ResourceScheduler::new(10);
        s.submit("http://a.com/img".to_string(), ResourcePriority::Image);
        s.submit("http://a.com/css".to_string(), ResourcePriority::BlockingCss);
        s.submit("http://a.com/doc".to_string(), ResourcePriority::Document);
        // Document 优先级最高
        assert_eq!(s.next().as_deref(), Some("http://a.com/doc"));
        assert_eq!(s.next().as_deref(), Some("http://a.com/css"));
        assert_eq!(s.next().as_deref(), Some("http://a.com/img"));
    }

    #[test]
    fn scheduler_并发上限() {
        let mut s = ResourceScheduler::new(2);
        assert_eq!(s.concurrency_limit(), 2);
        s.submit("http://a.com/1".to_string(), ResourcePriority::Document);
        s.submit("http://a.com/2".to_string(), ResourcePriority::Document);
        s.submit("http://a.com/3".to_string(), ResourcePriority::Document);
        assert!(s.can_start());
        assert!(s.next().is_some());
        assert!(s.next().is_some());
        assert!(!s.can_start());
        s.finish();
        assert!(s.can_start());
    }

    #[test]
    fn scheduler_finish_释放槽位() {
        let mut s = ResourceScheduler::new(1);
        s.submit("http://a.com/1".to_string(), ResourcePriority::Document);
        assert!(s.next().is_some());
        assert!(!s.can_start());
        s.finish();
        // 释放后需要重新提交才能继续
        s.submit("http://a.com/2".to_string(), ResourcePriority::Document);
        assert!(s.can_start());
    }

    #[test]
    fn from_content_type_css() {
        assert_eq!(
            ResourcePriority::from_content_type("text/css", false, true),
            ResourcePriority::BlockingCss
        );
    }

    #[test]
    fn from_content_type_font() {
        assert_eq!(
            ResourcePriority::from_content_type("font/ttf", false, false),
            ResourcePriority::Font
        );
    }

    #[test]
    fn from_content_type_image() {
        assert_eq!(
            ResourcePriority::from_content_type("image/png", false, false),
            ResourcePriority::Image
        );
    }

    #[test]
    fn from_content_type_script() {
        assert_eq!(
            ResourcePriority::from_content_type("text/javascript", true, false),
            ResourcePriority::Script
        );
    }
}
