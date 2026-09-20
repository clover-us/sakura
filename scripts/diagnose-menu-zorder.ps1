param(
  [string]$ProcessName = 'whale-pet-desktop',
  [int]$WaitSeconds = 0
)
$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class Zdiag2 {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetTopWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h, uint cmd);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder b, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetWindowLongPtrW(IntPtr h, int index);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }

  public static string Title(IntPtr h) { var sb = new StringBuilder(512); GetWindowTextW(h, sb, 512); return sb.ToString(); }
  public static string ClassName(IntPtr h) { var sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  public static RECT Rect(IntPtr h) { RECT r; GetWindowRect(h, out r); return r; }
  public static int ExStyle(IntPtr h) { return GetWindowLongPtrW(h, -20); }
  public static uint Pid(IntPtr h) { uint pid; GetWindowThreadProcessId(h, out pid); return pid; }

  public static int ZIndex(IntPtr target) {
    IntPtr h = GetTopWindow(IntPtr.Zero);
    int i = 0;
    while (h != IntPtr.Zero && i < 5000) {
      if (h == target) return i;
      h = GetWindow(h, 2);
      i++;
    }
    return -1;
  }
  public static List<IntPtr> AllTopLevel() {
    var list = new List<IntPtr>();
    EnumWindows((h, l) => { list.Add(h); return true; }, IntPtr.Zero);
    return list;
  }
}
'@

if ($WaitSeconds -gt 0) { Start-Sleep -Seconds $WaitSeconds }

$procs = @(Get-Process -Name $ProcessName -ErrorAction SilentlyContinue)
if ($procs.Count -eq 0) {
  Write-Host ('找不到进程 {0}：请先启动应用并右键弹出菜单，再跑本脚本' -f $ProcessName)
  exit 1
}
$pids = @{}
foreach ($p in $procs) { $pids[[uint32]$p.Id] = $true }

$rows = @()
foreach ($h in [Zdiag2]::AllTopLevel()) {
  $wpid = [Zdiag2]::Pid($h)
  if (-not $pids.ContainsKey($wpid)) { continue }
  $r = [Zdiag2]::Rect($h)
  $ex = [Zdiag2]::ExStyle($h)
  $flags = @()
  if ($ex -band 0x00000008) { $flags += 'TOPMOST' }
  if ($ex -band 0x00080000) { $flags += 'LAYERED' }
  if ($ex -band 0x00000020) { $flags += 'TRANSPARENT' }
  if ($ex -band 0x08000000) { $flags += 'NOACTIVATE' }
  $rows += [pscustomobject]@{
    Z       = [Zdiag2]::ZIndex($h)
    Hwnd    = ('0x{0}' -f $h.ToInt64().ToString('X'))
    Pid     = $wpid
    Title   = [Zdiag2]::Title($h)
    Visible = [Zdiag2]::IsWindowVisible($h)
    Size    = ('{0}x{1}' -f ($r.R - $r.L), ($r.B - $r.T))
    At      = ('({0},{1})' -f $r.L, $r.T)
    Flags   = ($flags -join '|')
  }
}

Write-Host ('=== {0} 的顶层窗口（Z 越小越靠上）===' -f $ProcessName)
$rows | Sort-Object Z | Format-Table -AutoSize | Out-String -Width 220 | Write-Host

$visible = @($rows | Where-Object { $_.Visible })
if ($visible.Count -lt 2) {
  Write-Host '提示：只看到一个可见窗口——请先右键弹出菜单（菜单窗应该同时可见）再跑本脚本。'
  exit 0
}
$pet = $visible | Where-Object { $_.Size -eq '420x236' } | Select-Object -First 1
$menu = $visible | Where-Object { $_.Size -eq '780x520' } | Select-Object -First 1
Write-Host ''
if ($pet -and $menu) {
  if ($menu.Z -lt $pet.Z) {
    Write-Host ('结论：菜单窗在宠物窗【之上】（menu z={0} 小于 pet z={1}）=> Z 序正常' -f $menu.Z, $pet.Z)
  } else {
    Write-Host ('结论：宠物窗在菜单窗【之上】（pet z={0} 小于 menu z={1}）=> 这就是菜单被人物盖住的直接原因' -f $pet.Z, $menu.Z)
  }
} else {
  Write-Host ('没同时认出两个窗口：宠物={0} 菜单={1}。把上面的表整段发我即可。' -f [bool]$pet, [bool]$menu)
}

# ---------- 环境信息：装的是哪一版、DPI、显示器 ----------
Write-Host ''
Write-Host '=== 环境信息 ==='
foreach ($p in $procs) {
  Write-Host ('进程 {0}: pid={1} 路径={2}' -f $ProcessName, $p.Id, $p.Path)
  if ($p.Path) {
    $f = Get-Item $p.Path -ErrorAction SilentlyContinue
    if ($f) { Write-Host ('           文件时间={0} 大小={1} 产品版本={2}' -f $f.LastWriteTime, $f.Length, $f.VersionInfo.ProductVersion) }
  }
}
# 宠物窗在哪个显示器、那块屏的 DPI
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Zdpi {
  [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr h, uint flags);
  [DllImport("shcore.dll")] public static extern int GetDpiForMonitor(IntPtr m, int type, out uint x, out uint y);
  public static int DpiOf(IntPtr h) {
    uint x = 0, y = 0;
    IntPtr m = MonitorFromWindow(h, 2); // NEAREST
    try { if (GetDpiForMonitor(m, 0, out x, out y) == 0) return (int)x; } catch {}
    return 0;
  }
}
'@
$anyHwnd = [IntPtr]::Zero
foreach ($r in $rows) { if ($r.Visible) { $anyHwnd = [IntPtr]([Convert]::ToInt64($r.Hwnd.Substring(2), 16)); break } }
if ($anyHwnd -ne [IntPtr]::Zero) {
  $dpi = [Zdpi]::DpiOf($anyHwnd)
  if ($dpi -gt 0) { Write-Host ('显示器 DPI = {0}（缩放 {1}%）' -f $dpi, [math]::Round($dpi * 100 / 96)) }
}
Get-CimInstance Win32_VideoController -ErrorAction SilentlyContinue |
  Select-Object -First 3 Name, DriverVersion |
  ForEach-Object { Write-Host ('显卡: {0} 驱动 {1}' -f $_.Name, $_.DriverVersion) }
Write-Host ('系统: {0}' -f (Get-CimInstance Win32_OperatingSystem -ErrorAction SilentlyContinue).Caption)
