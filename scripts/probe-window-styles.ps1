<#
  窗口样式 / 命中归属 外部探针（诊断脚本，不属于应用运行路径）

  ## 为什么需要它

  气泡窗（`bubble-*`）的设计要求是"整窗点击穿透、绝不挡下层点击"。判断这一点
  必须从**进程外**读窗口的真实扩展样式位：

  - `WS_EX_TRANSPARENT`（0x20）：命中测试时被跳过——**这一位是"穿透"的唯一开关**，
    宠物窗在"非交互态"下正是靠它让点击落到下层应用的（用户已验收的行为）；
  - `WS_EX_LAYERED`（0x80000）：分层窗，WebView2 透明渲染的基础。

  为什么要进程外（不在应用里打印）：进程内 `GetWindowLongPtrW` 读到的是"我刚设上的值"，
  无法回答"这个值后来有没有被 WebView2 抹掉"。这个脚本读的是**任何时刻的真实值**。

  ## 用法

  ```powershell
  # 先拿到桌宠进程号（宠物窗与气泡窗都在这个进程里）
  Get-Process whale-pet-desktop | Select-Object -ExpandProperty Id
  # 再按节奏采样（默认 12 秒、每 500ms 一拍）
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\probe-window-styles.ps1 -TargetPid 1234
  ```

  小窗（宽度 < `-SmallWidth`，即宠物窗/气泡窗）会额外打印**全部子窗口**——
  WebView2 的窗口是多层嵌套的，鼠标实际命中的可能是最内层，只读顶层不够。
#>
param(
  # 桌宠进程号（宠物窗与气泡窗都在这个进程里）
  [Parameter(Mandatory = $true)][int]$TargetPid,
  # 采样总时长（秒）
  [int]$Seconds = 12,
  # 采样间隔（毫秒）
  [int]$IntervalMs = 500,
  # "小窗"阈值（像素）：宽度小于它的顶层窗口会被展开打印子窗口
  [int]$SmallWidth = 700
)

$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;

public static class WinProbe {
  public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);

  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr lParam);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder buf, int max);
  [DllImport("user32.dll")] public static extern IntPtr GetWindowLongPtrW(IntPtr hWnd, int index);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);

  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
'@

$WS_EX_TRANSPARENT = 0x00000020
$WS_EX_LAYERED     = 0x00080000
$WS_EX_NOACTIVATE  = 0x08000000
$GWL_EXSTYLE       = -20

function Get-Class([IntPtr]$h) {
  $sb = New-Object System.Text.StringBuilder 256
  [void][WinProbe]::GetClassNameW($h, $sb, $sb.Capacity)
  $sb.ToString()
}

function Get-Rect([IntPtr]$h) {
  $r = New-Object WinProbe+RECT
  if (-not [WinProbe]::GetWindowRect($h, [ref]$r)) { return $null }
  [pscustomobject]@{ X = $r.Left; Y = $r.Top; W = $r.Right - $r.Left; H = $r.Bottom - $r.Top }
}

function Get-OwnerPid([IntPtr]$h) {
  $p = 0
  [void][WinProbe]::GetWindowThreadProcessId($h, [ref]$p)
  $p
}

function Get-ExStyle([IntPtr]$h) { [int64][WinProbe]::GetWindowLongPtrW($h, $GWL_EXSTYLE) }

function Format-Window([IntPtr]$h, [int]$depth = 0) {
  $r = Get-Rect $h
  $ex = Get-ExStyle $h
  $flags = @()
  if ($ex -band $WS_EX_TRANSPARENT) { $flags += 'T' }
  if ($ex -band $WS_EX_LAYERED)     { $flags += 'L' }
  if ($ex -band $WS_EX_NOACTIVATE)  { $flags += 'NA' }
  $tag = if ($flags.Count -gt 0) { $flags -join '|' } else { '-' }
  '{0}{1,-28} pid={2,-6} rect=({3},{4} {5}x{6}) {7,-8} ex=0x{8:X}' -f `
    ('  ' * $depth), (Get-Class $h), (Get-OwnerPid $h), $r.X, $r.Y, $r.W, $r.H, $tag, $ex
}

function Get-DirectChildren([IntPtr]$parent) {
  $out = New-Object System.Collections.ArrayList
  $cb = [WinProbe+EnumProc]{
    param([IntPtr]$h, [IntPtr]$l)
    [void]$out.Add($h)
    return $true
  }
  [void][WinProbe]::EnumChildWindows($parent, $cb, [IntPtr]::Zero)
  $out
}

# 递归打印指定窗口的全部后代（只对"小窗"调用，避免刷屏）
function Show-Tree([IntPtr]$root, [int]$depth = 1) {
  foreach ($h in (Get-DirectChildren $root)) {
    Write-Host (Format-Window $h $depth)
    Show-Tree $h ($depth + 1)
  }
}

$deadline = (Get-Date).AddSeconds($Seconds)
$tick = 0

while ((Get-Date) -lt $deadline) {
  $tick++
  $tops = New-Object System.Collections.ArrayList
  $cb = [WinProbe+EnumProc]{
    param([IntPtr]$h, [IntPtr]$l)
    if ((Get-OwnerPid $h) -eq $TargetPid) { [void]$tops.Add($h) }
    return $true
  }
  [void][WinProbe]::EnumWindows($cb, [IntPtr]::Zero)

  Write-Host ("---- 第 {0} 拍 {1:HH:mm:ss.fff}：顶层窗口 {2} 个 ----" -f $tick, (Get-Date), $tops.Count)
  foreach ($h in $tops) {
    Write-Host (Format-Window $h)
    $r = Get-Rect $h
    if ($r.W -gt 0 -and $r.W -lt $SmallWidth) { Show-Tree $h }
  }
  Start-Sleep -Milliseconds $IntervalMs
}

Write-Host '---- 采样结束 ----'
