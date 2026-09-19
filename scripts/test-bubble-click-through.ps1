<#
  气泡"吃不吃点击"的**真点击**对照实验（诊断脚本，不属于应用运行路径）

  ## 为什么样式位还不够

  `probe-window-styles.ps1` 读的是 `WS_EX_TRANSPARENT` 位——那是**机制**层面的证据。
  本脚本回答的是**行为**层面：在气泡与宠物重叠的点上打一次真鼠标点击，看事件到底落到谁身上。

  ## 实验设计（A/B 对照）

  前置：应用以 `WHALE_PET_AUTOTEST=8` 启动（见 `main.ts`），气泡被**故意压在宠物身体上**。

  两个探针点（都由日志里的锚点算出来，宠物命中区为 `x 2241~2399, y 133~320`）：

  | 点 | 坐标 | 落在哪 |
  | --- | --- | --- |
  | 重叠点 | `(anchorX, anchorY-40)` | 气泡**和**宠物都覆盖 |
  | 对照点 | `(anchorX, 140)` | 只在宠物命中区内，**气泡覆盖不到** |

  判据只看宠物日志里有没有**这一点的按下记录**（`按下: 来源=dom 指针=(x,y)`）：

  - 对照点有反应、重叠点也有反应 → **气泡不吃点击（穿透成立）**；
  - 对照点有反应、重叠点没反应 → **气泡吃掉了这一下**；
  - 对照点没反应 → 这一次实验无效（宠物此刻不可点，例如它正处于穿透态），不计入结论。

  之所以按坐标匹配、而不是按"是否切到了点击回应动画"匹配：动画文件名在日志里是
  **百分号编码**的（`%E7%82%B9%E5%87%BB...`），用中文关键字匹配会永远失败（踩过这个坑）。

  之所以必须有对照点：宠物自己也有"非交互态整窗穿透"的行为，只看重叠点会把
  "宠物恰好在穿透"误判成"气泡吃点击"。

  ## 用法

  ```powershell
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\test-bubble-click-through.ps1 `
    -TargetPid 1234 -LogPath D:\programs\deepseek\sakura\src-tauri\target\autotest.log
  ```
#>
param(
  # 桌宠进程号（用于确认进程活着；点击本身是按屏幕坐标注入的）
  [Parameter(Mandatory = $true)][int]$TargetPid,
  # 应用日志路径：脚本从中解析气泡锚点，并在每次点击后检查新增内容
  [Parameter(Mandatory = $true)][string]$LogPath,
  # 对照轮数（每轮 = 重叠点一次 + 对照点一次）
  [int]$Rounds = 2,
  # 点击后等待多久再读日志（毫秒）
  [int]$SettleMs = 1500
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path $LogPath)) { throw "日志不存在：$LogPath" }

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class PetInput {
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, int dx, int dy, uint dwData, IntPtr dwExtraInfo);

  private const uint LEFTDOWN = 0x0002;
  private const uint LEFTUP = 0x0004;

  /// 把光标移到目标点并做一次**真实**左键点击（走系统输入队列，不是合成 DOM 事件）
  public static void Click(int x, int y) {
    SetCursorPos(x, y);
    System.Threading.Thread.Sleep(150);
    mouse_event(LEFTDOWN, 0, 0, 0, IntPtr.Zero);
    System.Threading.Thread.Sleep(60);
    mouse_event(LEFTUP, 0, 0, 0, IntPtr.Zero);
  }
}
'@

# 从某个字节偏移开始读日志（用字节偏移而不是时间戳：日志行的时钟与本地时钟不同源）
function Read-LogFrom([string]$path, [long]$offset) {
  $fs = [System.IO.File]::Open($path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
  try {
    [void]$fs.Seek($offset, [System.IO.SeekOrigin]::Begin)
    $reader = New-Object System.IO.StreamReader($fs, [System.Text.Encoding]::UTF8)
    return $reader.ReadToEnd()
  } finally {
    $fs.Dispose()
  }
}

# 解析气泡锚点：日志里 self-test 那行 `锚点=(x,y)`
$anchorPattern = '锚点=\((\d+),(\d+)\)'
$anchorLine = Select-String -Path $LogPath -Pattern '气泡穿透自测' -Encoding UTF8 | Select-Object -Last 1
if (-not $anchorLine) { throw "日志里找不到 '气泡穿透自测'（应用是否以 WHALE_PET_AUTOTEST=8 启动？）" }
$null = $anchorLine.Line -match $anchorPattern
$anchorX = [int]$Matches[1]
$anchorY = [int]$Matches[2]

$overlapPoint = @{ X = $anchorX; Y = $anchorY - 40 }   # 气泡 + 宠物都覆盖
$controlPoint = @{ X = $anchorX; Y = 140 }            # 只落在宠物命中区

Write-Host "气泡锚点 = ($anchorX,$anchorY)"
Write-Host "重叠点  = ($($overlapPoint.X),$($overlapPoint.Y))   对照点 = ($($controlPoint.X),$($controlPoint.Y))"

# 自校验：气泡窗的创建日志里有矩形，确认"重叠点确实被气泡覆盖"是这次实验的前提
$bubbleLine = Select-String -Path $LogPath -Pattern '气泡窗已创建' -Encoding UTF8 | Select-Object -Last 1
if ($bubbleLine -and $bubbleLine.Line -match '原点=\((-?\d+),(-?\d+)\) 尺寸=(\d+)×(\d+)') {
  $bx = [int]$Matches[1]; $by = [int]$Matches[2]; $bw = [int]$Matches[3]; $bh = [int]$Matches[4]
  $covered = ($overlapPoint.X -ge $bx) -and ($overlapPoint.X -lt ($bx + $bw)) -and
             ($overlapPoint.Y -ge $by) -and ($overlapPoint.Y -lt ($by + $bh))
  Write-Host "气泡窗矩形 = ($bx,$by $bw×$bh)；重叠点被气泡覆盖：$covered"
  if (-not $covered) { throw '前置条件不成立：重叠点不在气泡窗内，实验无意义' }
} else {
  Write-Host '提示：日志里没找到气泡窗矩形，无法自校验重叠关系'
}

$overlapHits = 0
$overlapMiss = 0
$controlHits = 0
$controlMiss = 0

for ($round = 1; $round -le $Rounds; $round++) {
  if (-not (Get-Process -Id $TargetPid -ErrorAction SilentlyContinue)) { throw "桌宠进程 $TargetPid 已退出" }

  Write-Host "---- 第 $round 轮 ----"

  # 一轮内先点重叠点、再点对照点；每次点击前后各取一次字节偏移，只检查"这一次点击"带来的新增日志
  foreach ($case in @(
      @{ Name = '重叠点（气泡覆盖）'; Point = $overlapPoint },
      @{ Name = '对照点（无气泡）';   Point = $controlPoint }
    )) {
    $before = (Get-Item $LogPath).Length
    [PetInput]::Click($case.Point.X, $case.Point.Y)
    Start-Sleep -Milliseconds $SettleMs
    $newText = Read-LogFrom $LogPath $before
    # 判据：宠物日志里出现"这一坐标的按下记录"（动画文件名是百分号编码的，不能按中文关键字匹配）
    $reacted = $newText -match ('指针=\(' + $case.Point.X + ',' + $case.Point.Y + '\)')
    if ($case.Name -like '重叠*') { if ($reacted) { $overlapHits++ } else { $overlapMiss++ } }
    else { if ($reacted) { $controlHits++ } else { $controlMiss++ } }
    $verdict = if ($reacted) { '宠物收到了这一点上的按下' } else { '宠物毫无反应' }
    Write-Host ("  {0,-18} → {1}" -f $case.Name, $verdict)
  }
}

Write-Host '===== 汇总 ====='
Write-Host "重叠点：有反应 $overlapHits 次 / 无反应 $overlapMiss 次"
Write-Host "对照点：有反应 $controlHits 次 / 无反应 $controlMiss 次"

if ($controlHits -eq 0) {
  Write-Host '结论：**实验无效**——对照点都没反应，说明宠物此刻不可点（例如处于穿透态），无法判定气泡行为。'
} elseif ($overlapHits -gt 0) {
  Write-Host '结论：**气泡不吃点击**——点击穿过气泡落到了宠物身上。'
} else {
  Write-Host '结论：**气泡吃掉了点击**（对照点有反应而重叠点没有）。'
}
