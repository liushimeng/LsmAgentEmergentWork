## 测试用例设计:Read 工具新增 5 个单元测试

基于现有 6 个测试用例的覆盖分析,以下 5 个测试针对未覆盖的关键场景。

---

### 测试 1: 正常读取绝对路径文件 - 验证输出格式

**测试名称**: `normal_read_absolute_path_verifies_output_format`

**输入构造方式**:
```rust
let mut f = NamedTempFile::new().unwrap();
writeln!(f, "alpha").unwrap();
writeln!(f, "beta").unwrap();
writeln!(f, "gamma").unwrap();
let p = f.path().to_str().unwrap().to_string();
let out = ReadTool.execute(json!({"file_path": p})).await.unwrap();
```

**预期断言**:
- `out.contains("<<<")` —— 输出包含 header 起始标记
- `out.contains(">>>")` —— 输出包含 header 结束标记
- `out.contains("lines 1-3 / total 3")` —— header 格式正确,显示行范围和总行数
- `out.contains("alpha") && out.contains("beta") && out.contains("gamma")` —— 内容完整
- 行号宽度对齐:对于 total=3,宽度为 1,验证 `"1\talpha"` 格式存在

---

### 测试 2: 绝对路径不存在 - 验证错误消息格式

**测试名称**: `nonexistent_absolute_path_error_message_format`

**输入构造方式**:
```rust
// 构造一个绝对不会存在的绝对路径
let nonexistent = "/tmp/this_file_does_not_exist_12345_test.txt";
let err = ReadTool.execute(json!({"file_path": nonexistent})).await.unwrap_err();
```

**预期断言**:
- `matches!(err, AgentError::ToolExecution { .. })` —— 返回 ToolExecution 错误
- 错误 reason 包含 `"stat 失败"` —— 标识 stat 系统调用失败
- 错误 reason 包含路径字符串 —— 便于定位问题

---

### 测试 3: 传入目录路径 - 验证返回"不是普通文件"错误

**测试名称**: `directory_path_returns_not_a_file_error`

**输入构造方式**:
```rust
// 使用 tempdir() 构造目录路径
let dir = tempfile::tempdir().unwrap();
let dir_path = dir.path().to_str().unwrap().to_string();
let err = ReadTool.execute(json!({"file_path": dir_path})).await.unwrap_err();
```

**预期断言**:
- `matches!(err, AgentError::ToolExecution { .. })` —— 返回 ToolExecution 错误
- 错误 reason 包含 `"不是普通文件"` —— 精确匹配源码第 82 行错误消息
- 错误 reason 包含目录路径 —— 便于定位

---

### 测试 4: 单行超过 4000 字符 - 验证截断标记

**测试名称**: `single_line_exceeds_4000_chars_truncates_with_marker`

**输入构造方式**:
```rust
// 构造单行 5000+ 字符的文本(无换行符)
let mut f = NamedTempFile::new().unwrap();
let long_line = "x".repeat(5000);
f.write_all(long_line.as_bytes()).unwrap();
let p = f.path().to_str().unwrap().to_string();
let out = ReadTool.execute(json!({"file_path": p})).await.unwrap();
```

**预期断言**:
- `out.contains("...[截断]")` —— 截断标记存在
- 截断后每行最多 4000 字符 + `"...[截断]"` 标记
- 验证输出行长度 <= 4000 + "...[截断]".len() = 4006

---

### 测试 5: 相对路径正常读取文件 - 验证路径解析正确命中工作目录

**测试名称**: `relative_path_resolves_to_working_directory`

**输入构造方式**:
```rust
// 创建临时目录并切换工作目录
let dir = tempfile::tempdir().unwrap();
let original_dir = std::env::current_dir().unwrap();
std::env::set_current_dir(&dir).unwrap();

// 在临时目录中创建文件
let file_path = dir.path().join("test_relative.txt");
std::fs::write(&file_path, "relative content\n").unwrap();

// 使用相对路径调用
let out = ReadTool.execute(json!({"file_path": "test_relative.txt"})).await.unwrap();

// 恢复原始工作目录(无论测试是否通过)
std::env::set_current_dir(&original_dir).unwrap();
```

**预期断言**:
- `out.contains("relative content")` —— 相对路径成功解析并读取内容
- `out.contains("total 1")` —— 行数统计正确
- 无错误返回 —— 证明 resolve_path 正确将相对路径解析为工作目录下的绝对路径

---

## 完整测试代码(可添加到 read.rs 的 tests mod 中)

```rust
#[tokio::test]
async fn normal_read_absolute_path_verifies_output_format() {
    let mut f = NamedTempFile::new().unwrap();
    writeln!(f, "alpha").unwrap();
    writeln!(f, "beta").unwrap();
    writeln!(f, "gamma").unwrap();
    let p = f.path().to_str().unwrap().to_string();
    let out = ReadTool.execute(json!({"file_path": p})).await.unwrap();
    // 验证 header 格式: <<< path (lines 1-3 / total 3) >>>
    assert!(out.contains("<<<"), "应包含 header 起始标记");
    assert!(out.contains(">>>"), "应包含 header 结束标记");
    assert!(out.contains("lines 1-3 / total 3"), "header 应显示行范围和总行数");
    // 验证内容完整
    assert!(out.contains("alpha"));
    assert!(out.contains("beta"));
    assert!(out.contains("gamma"));
    // 验证行号格式(右对齐 + tab 分隔)
    assert!(out.contains("1\talpha"), "行号应右对齐并 tab 分隔");
}

#[tokio::test]
async fn nonexistent_absolute_path_error_message_format() {
    let nonexistent = "/tmp/this_file_does_not_exist_12345_test.txt";
    let err = ReadTool.execute(json!({"file_path": nonexistent})).await.unwrap_err();
    match err {
        AgentError::ToolExecution { reason, .. } => {
            assert!(reason.contains("stat 失败"), "错误应标识 stat 失败,实际: {reason}");
            assert!(reason.contains(nonexistent), "错误应包含尝试的路径,实际: {reason}");
        }
        other => panic!("预期 ToolExecution 错误,实际 {other:?}"),
    }
}

#[tokio::test]
async fn directory_path_returns_not_a_file_error() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_str().unwrap().to_string();
    let err = ReadTool.execute(json!({"file_path": dir_path})).await.unwrap_err();
    match err {
        AgentError::ToolExecution { reason, .. } => {
            assert!(reason.contains("不是普通文件"), "错误应提示'不是普通文件',实际: {reason}");
            assert!(reason.contains(&dir_path), "错误应包含目录路径,实际: {reason}");
        }
        other => panic!("预期 ToolExecution 错误,实际 {other:?}"),
    }
}

#[tokio::test]
async fn single_line_exceeds_4000_chars_truncates_with_marker() {
    let mut f = NamedTempFile::new().unwrap();
    let long_line = "x".repeat(5000);
    f.write_all(long_line.as_bytes()).unwrap();
    let p = f.path().to_str().unwrap().to_string();
    let out = ReadTool.execute(json!({"file_path": p})).await.unwrap();
    assert!(out.contains("...[截断]"), "超长行应被截断并标注");
    // 验证截断后长度:4000 字符 + "...[截断]" = 4006 字符
    let truncated_line = format!("{}...[截断]", "x".repeat(4000));
    assert!(out.contains(&truncated_line), "截断后内容应为 4000 字符 + 标记");
}

#[tokio::test]
async fn relative_path_resolves_to_working_directory() {
    let dir = tempfile::tempdir().unwrap();
    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(&dir).unwrap();

    let file_path = dir.path().join("test_relative.txt");
    std::fs::write(&file_path, "relative content\n").unwrap();

    let result = ReadTool.execute(json!({"file_path": "test_relative.txt"})).await;

    // 恢复原始工作目录
    std::env::set_current_dir(&original_dir).unwrap();

    let out = result.unwrap();
    assert!(out.contains("relative content"), "相对路径应成功解析并读取");
    assert!(out.contains("total 1"), "行数统计应正确");
}
```

---

## 覆盖场景总结

| 测试 | 覆盖的源码行 | 未覆盖场景 |
|-----|-------------|-----------|
| 测试 1 | 96-118 | header 格式、行号宽度对齐 |
| 测试 2 | 57-78 | 绝对路径不存在时的错误格式 |
| 测试 3 | 79-84 | `metadata.is_file()` 检查 |
| 测试 4 | 107-113 | 单行 >4000 字符截断分支 |
| 测试 5 | 186-198 | 相对路径工作目录命中分支 |
