//! 质量门禁:Phase/Squad/Task 三层 QC。
//!
//! 复用现有 QualityRunner,提供分层质量检查能力。

use serde::{Deserialize, Serialize};

/// 质量门禁结果。
#[derive(Debug, Clone)]
pub struct QualityGateResult {
    pub passed: bool,
    pub level: QualityLevel,
    pub issues: Vec<String>,
    pub suggestion: String,
}

/// 质量检查层级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityLevel {
    Task,
    Squad,
    Phase,
    Goal,
}

/// 质量门禁。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityGate {
    pub level: QualityLevel,
    pub enabled: bool,
    pub strict_mode: bool,
}

impl QualityGate {
    pub fn new(level: QualityLevel) -> Self {
        Self {
            level,
            enabled: true,
            strict_mode: false,
        }
    }

    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict_mode = strict;
        self
    }

    pub fn check(&self, output: &str, expected: &str) -> QualityGateResult {
        let mut issues = Vec::new();
        let mut passed = true;

        // 空输出检查
        if output.trim().is_empty() {
            issues.push("输出为空".to_string());
            passed = false;
        }

        // 严格模式:检查是否包含期望关键词
        if self.strict_mode && !expected.is_empty() {
            let expected_keywords: Vec<&str> = expected.split(';').collect();
            let missing: Vec<&&str> = expected_keywords
                .iter()
                .filter(|kw| !output.contains(**kw))
                .collect();
            if !missing.is_empty() {
                issues.push(format!("缺少期望内容: {:?}", missing));
                passed = false;
            }
        }

        // 失败措辞检查
        let failure_indicators = ["失败", "错误", "error", "failed", "失败", "无法"];
        if failure_indicators.iter().any(|ind| output.contains(ind)) {
            if self.strict_mode {
                issues.push("输出包含失败措辞".to_string());
                passed = false;
            }
        }

        QualityGateResult {
            passed,
            level: self.level,
            issues,
            suggestion: if passed {
                String::new()
            } else {
                "请修复上述问题后重试".to_string()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_gate_passes_for_good_output() {
        let gate = QualityGate::new(QualityLevel::Task);
        let result = gate.check("任务已完成,所有测试通过", "完成");
        assert!(result.passed);
        assert!(result.issues.is_empty());
    }

    #[test]
    fn quality_gate_fails_for_empty_output() {
        let gate = QualityGate::new(QualityLevel::Task);
        let result = gate.check("", "完成");
        assert!(!result.passed);
        assert!(result.issues.iter().any(|i| i.contains("为空")));
    }

    #[test]
    fn quality_gate_strict_missing_keyword() {
        let gate = QualityGate::new(QualityLevel::Task).with_strict(true);
        let result = gate.check("做完了", "完成;测试");
        assert!(!result.passed);
        assert!(result.issues.iter().any(|i| i.contains("缺少")));
    }

    #[test]
    fn quality_gate_strict_failure_phrase() {
        let gate = QualityGate::new(QualityLevel::Task).with_strict(true);
        let result = gate.check("执行失败: 无法连接", "完成");
        assert!(!result.passed);
        assert!(result.issues.iter().any(|i| i.contains("失败")));
    }

    #[test]
    fn quality_gate_non_strict_allows_failure_phrase() {
        let gate = QualityGate::new(QualityLevel::Task).with_strict(false);
        let result = gate.check("已处理失败情况,最终成功", "成功");
        assert!(result.passed);
    }

    #[test]
    fn quality_level_serializes() {
        let level = QualityLevel::Squad;
        let json = serde_json::to_string(&level).unwrap();
        assert_eq!(json, "\"squad\"");
    }
}
