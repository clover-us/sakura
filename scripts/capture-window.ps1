<#
  窗口截图探针（诊断脚本，不属于应用运行路径）

  ## 为什么需要它

  "界面好不好看"只能看，不能靠推断。而本项目的窗口里装的是 **WebView2**：

  - `PrintWindow` 抓不到 WebView2 的合成内容（返回全黑，DirectComposition 的已知行为）；
  - 因此这里走**屏幕合成截图**（`Graphics.CopyFromScreen`），抓的是 DWM 实际合成后的画面，
    WebView2 的内容能被正常拍到。

  代价与前提：目标窗口必须**真的在屏幕上、没有被别的窗口盖住**（屏幕截图抓的是"最上面的东西"）。
  所以脚本会先把目标窗口提到前台，再抓它自己的矩形。

  ## 用法

  ```powershell
  # 按标题关键字抓（默认输出到 %TEMP%\sakura-verify\capture-<标题>.png）
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\capture-window.ps1 -TitleLike '设置'
  # 也可以指定输出路径与等待时间（默认等 600ms 让窗口完成重绘）
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\capture-window.ps1 -TitleLike '设置' -Out shot.png -DelayMs 900
  ```

  退出码：0 = 抓到并保存；1 = 没找到窗口 / 抓取失败。
#>
param(
  # 窗口标题的匹配片段（不区分大小写）
  [Parameter(Mandatory = $true)][string]$TitleLike,
  # 输出文件（默认写到临时目录）
  [string]$Out = '',
  # 抓之前的等待（毫秒）：先激活窗口，再等它重绘完
  [int]$DelayMs = 700
)

$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class WinCap {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder b, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
'@

function Get-Title([IntPtr]$h) {
  $sb = New-Object System.Text.StringBuilder 512
  [void][WinCap]::GetWindowTextW($h, $sb, $sb.Capacity)
  $sb.ToString()
}

# 收集**所有**匹配的可见窗口，最后取面积最大的那个：
# 只取"第一个命中"会踩坑——别的应用（比如带"设置"二字的窗口）可能排在前面，
# 于是脚本把无关窗口提到前台、截了一张别人的图（实测踩过）。
$candidates = New-Object System.Collections.ArrayList
$cb = [WinCap+EnumProc]{
  param([IntPtr]$h, [IntPtr]$l)
  if (-not [WinCap]::IsWindowVisible($h)) { return $true }
  $t = Get-Title $h
  if ($t -and ($t.ToLower().Contains($TitleLike.ToLower()))) {
    $r = New-Object WinCap+RECT
    $area = 0
    if ([WinCap]::GetWindowRect($h, [ref]$r)) { $area = ($r.R - $r.L) * ($r.B - $r.T) }
    [void]$candidates.Add([pscustomobject]@{ H = $h; Title = $t; Area = $area })
  }
  return $true
}
[void][WinCap]::EnumWindows($cb, [IntPtr]::Zero)
if ($candidates.Count -eq 0) {
  $found = [IntPtr]::Zero
  $foundTitle = ''
} else {
  $best = $candidates | Sort-Object -Property Area -Descending | Select-Object -First 1
  $found = $best.H
  $foundTitle = $best.Title
}

if ($found -eq [IntPtr]::Zero) {
  Write-Error "没有找到标题包含「$TitleLike」的可见窗口"
  exit 1
}

# 提到前台再抓：否则抓到的是盖在它上面的窗口
[void][WinCap]::ShowWindow($found, 5)   # SW_SHOW
[void][WinCap]::SetForegroundWindow($found)
Start-Sleep -Milliseconds $DelayMs

$r = New-Object WinCap+RECT
if (-not [WinCap]::GetWindowRect($found, [ref]$r)) { Write-Error '取窗口矩形失败'; exit 1 }
$w = $r.R - $r.L
$h2 = $r.B - $r.T
if ($w -le 0 -or $h2 -le 0) { Write-Error "窗口矩形异常：${w}x${h2}"; exit 1 }

if ([string]::IsNullOrWhiteSpace($Out)) {
  $dir = Join-Path $env:TEMP 'sakura-verify'
  New-Item -ItemType Directory -Force -Path $dir | Out-Null
  $safe = ($TitleLike -replace '[^\w\u4e00-\u9fa5]', '_')
  $Out = Join-Path $dir "capture-$safe.png"
}

$bmp = New-Object System.Drawing.Bitmap $w, $h2
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.L, $r.T, 0, 0, (New-Object System.Drawing.Size $w, $h2))
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose()
$bmp.Dispose()

Write-Host "已保存：$Out（窗口「$foundTitle」${w}x${h2} @ $($r.L),$($r.T)）"
exit 0
