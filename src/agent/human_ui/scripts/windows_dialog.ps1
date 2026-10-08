# laew 人工介入弹窗 —— Windows 默认脚本(PowerShell WinForms,第 132 轮更新)
# 用法: powershell -NoProfile -STA -ExecutionPolicy Bypass -File <本脚本> <payload.json>
# 动态加载: {工作目录|根目录|~}/.laew/human_ui/windows.ps1 可整体替换本脚本(每次呈现重读,改完即生效)
# stdout 输出结果 JSON: {"status":"answer|cancel|timeout|error","text":"..."}
# 设计见 docs/MCP_Web_Use/03-人工介入弹窗UI动态加载方案.md
#
# 第 132 轮(与 macOS 自绘 NSWindow 版对齐):
# - 说明区改 ReadOnly 多行 TextBox:鼠标可选中、Ctrl+C 可复制(Label 不支持选择);
# - 新增验证码图片区(PictureBox, Zoom 缩放,高 160):payload.image_path 非空且
#   文件存在时展示,人工在弹窗内直接读码,窗高自适应。

param([Parameter(Mandatory = $true)][string]$PayloadPath)

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

function Write-UiResult([string]$status, [string]$text) {
    $obj = @{ status = $status; text = $text }
    Write-Output ($obj | ConvertTo-Json -Compress)
}

try {
    $p = Get-Content -Raw -Encoding UTF8 -Path $PayloadPath | ConvertFrom-Json
} catch {
    Write-UiResult 'error' "payload 读取失败: $($_.Exception.Message)"
    exit 0
}

$options = @()
if ($p.options) { $options = @($p.options | ForEach-Object { [string]$_ }) }
if ($options.Count -eq 0) { $options = @('我已完成人工操作,继续') }
if ($options.Count -gt 6) { $options = $options[0..5] }

$timeoutMs = [int64]$p.timeout_ms
if ($timeoutMs -le 0) { $timeoutMs = 120000 }
$script:startedAt = Get-Date
$script:deadlineAt = $script:startedAt.AddMilliseconds($timeoutMs)
$script:uiStatus = 'cancel'
$script:uiText = ''

# ---- 验证码图片(第 132 轮):文件存在才占位,窗高随之自适应 ----
$imgPath = [string]$p.image_path
$img = $null
if ($imgPath -and (Test-Path -LiteralPath $imgPath)) {
    try { $img = [System.Drawing.Image]::FromFile($imgPath) } catch { $img = $null }
}
$imgAreaH = 0
if ($img) { $imgAreaH = 164 }   # 160 图 + 4 间距

$formH = 320 + $imgAreaH

$form = New-Object System.Windows.Forms.Form
$form.Text = "🔐 人工介入 · $($p.kind_label)"
$form.TopMost = $true
$form.FormBorderStyle = 'FixedDialog'
$form.StartPosition = 'CenterScreen'
$form.MaximizeBox = $false
$form.MinimizeBox = $false
$form.ShowInTaskbar = $true
$form.ClientSize = New-Object System.Drawing.Size(500, $formH)

# 说明区:ReadOnly 多行 TextBox(可鼠标选中 + Ctrl+C 复制,第 132 轮)
$txtMsg = New-Object System.Windows.Forms.TextBox
$txtMsg.Location = New-Object System.Drawing.Point(14, 12)
$txtMsg.Size = New-Object System.Drawing.Size(472, 110)
$txtMsg.Multiline = $true
$txtMsg.ReadOnly = $true
$txtMsg.ScrollBars = 'Vertical'
$txtMsg.BorderStyle = 'None'
$txtMsg.BackColor = $form.BackColor
$txtMsg.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 9)
$msgText = [string]$p.message
if ($p.url) { $msgText += "`r`n`r`n页面: $($p.url)" }
if ($p.page_id) { $msgText += "   页面ID: $($p.page_id)" }
$txtMsg.Text = $msgText
$form.Controls.Add($txtMsg)

# 验证码图片区(有图才显示;无图时高度为 0,后续控件不位移)
$yCursor = 128
if ($img) {
    $pic = New-Object System.Windows.Forms.PictureBox
    $pic.Location = New-Object System.Drawing.Point(14, $yCursor)
    $pic.Size = New-Object System.Drawing.Size(472, 160)
    $pic.SizeMode = 'Zoom'
    $pic.Image = $img
    $form.Controls.Add($pic)
    $yCursor += 164
}

# 时间轴(提出/超时截止 固定;已等待/超时剩余 动态)
$lblTime = New-Object System.Windows.Forms.Label
$lblTime.Location = New-Object System.Drawing.Point(14, $yCursor)
$lblTime.Size = New-Object System.Drawing.Size(472, 20)
$lblTime.Text = "提出时间: $($script:startedAt.ToString('HH:mm:ss'))   超时截止: $($script:deadlineAt.ToString('HH:mm:ss'))"
$form.Controls.Add($lblTime)
$yCursor += 22

$lblLive = New-Object System.Windows.Forms.Label
$lblLive.Location = New-Object System.Drawing.Point(14, $yCursor)
$lblLive.Size = New-Object System.Drawing.Size(472, 20)
$lblLive.Text = '已等待: 0 秒   超时剩余: --'
$form.Controls.Add($lblLive)
$yCursor += 28

# 输入区
$textBox = New-Object System.Windows.Forms.TextBox
$textBox.Location = New-Object System.Drawing.Point(14, $yCursor)
$textBox.Size = New-Object System.Drawing.Size(472, 24)
$textBox.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 10)
$form.Controls.Add($textBox)
$yCursor += 34

# 操作区:选项按钮 + 常驻「取消」
$btnY = $yCursor
$btnX = 14
$cancelBtn = New-Object System.Windows.Forms.Button
$cancelBtn.Text = '取消(&C)'
$cancelBtn.Size = New-Object System.Drawing.Size(90, 30)
$cancelBtn.Location = New-Object System.Drawing.Point(($form.ClientSize.Width - 104), $btnY)
$cancelBtn.Add_Click({
    $script:uiStatus = 'cancel'
    $form.Close()
})
$form.Controls.Add($cancelBtn)
$form.CancelButton = $cancelBtn

for ($i = 0; $i -lt $options.Count; $i++) {
    $btn = New-Object System.Windows.Forms.Button
    $btn.Text = [string]$options[$i]
    $btn.Size = New-Object System.Drawing.Size(110, 30)
    $btn.Location = New-Object System.Drawing.Point($btnX, $btnY)
    $optIdx = $i
    $btn.Add_Click({
        $typed = $textBox.Text.Trim()
        if ($typed.Length -gt 0) {
            $script:uiText = $typed
        } else {
            $script:uiText = "$($optIdx + 1). $($options[$optIdx])"
        }
        $script:uiStatus = 'answer'
        $form.Close()
    }.GetNewClosure())
    $form.Controls.Add($btn)
    $btnX += 118
}

# 时间轴刷新 + 持续获取焦点 + 超时自灭
$timer = New-Object System.Windows.Forms.Timer
$timer.Interval = 1000
$timer.Add_Tick({
    $elapsed = ((Get-Date) - $script:startedAt).TotalMilliseconds
    $remain = $timeoutMs - $elapsed
    $elapsedS = [Math]::Max(0, [int][Math]::Floor($elapsed / 1000))
    $remainS = [Math]::Max(0, [int][Math]::Floor($remain / 1000))
    $lblLive.Text = "已等待: $elapsedS 秒   超时剩余: $remainS 秒"
    if ($remain -le 0) {
        $script:uiStatus = 'timeout'
        $form.Close()
        return
    }
    if (-not $form.Focused) { [void]$form.Activate() }
})
$timer.Start()

# 表单关闭时释放图片资源(避免锁定截图文件)
$form.Add_FormClosed({
    if ($img) { $img.Dispose(); $img = $null }
})

# 初始拉焦:窗口激活 + 输入框获得键盘焦点(第 132 轮:焦点必须落在输入框)
$form.Add_Shown({ [void]$form.Activate(); [void]$textBox.Focus(); [void]$textBox.Select() })
[void]$form.ShowDialog()
$timer.Stop()

switch ($script:uiStatus) {
    'answer' { Write-UiResult 'answer' $script:uiText }
    'timeout' { Write-UiResult 'timeout' '' }
    default { Write-UiResult 'cancel' '' }
}
exit 0
