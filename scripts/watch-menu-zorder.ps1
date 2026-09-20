param(
  [string]$ProcessName = 'whale-pet-desktop',
  [int]$Seconds = 25,
  [int]$IntervalMs = 300
)
$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class Zrec {
  [DllImport("user32.dll")] public static extern IntPtr GetTopWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h, uint cmd);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern int GetWindowLongPtrW(IntPtr h, int index);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  public static RECT Rect(IntPtr h) { RECT r; GetWindowRect(h, out r); return r; }
  public static uint Pid(IntPtr h) { uint pid; GetWindowThreadProcessId(h, out pid); return pid; }
  public static int ExStyle(IntPtr h) { return GetWindowLongPtrW(h, -20); }

  // 一次遍历：返回该进程所有"可见顶层窗口"的 Z 序与尺寸（Z 越小越靠上）
  public static List<string> Snapshot(uint targetPid) {
    var list = new List<string>();
    IntPtr h = GetTopWindow(IntPtr.Zero);
    int i = 0;
    while (h != IntPtr.Zero && i < 5000) {
      uint pid; GetWindowThreadProcessId(h, out pid);
      if (pid == targetPid && IsWindowVisible(h)) {
        RECT r = Rect(h);
        int w = r.R - r.L, ht = r.B - r.T;
        bool topmost = (ExStyle(h) & 0x8) != 0;
        int z = i;
        list.Add(z + "|" + w + "x" + ht + "|" + (topmost ? "TOPMOST" : "-") + "|" + r.L + "," + r.T);
      }
      h = GetWindow(h, 2);
      i++;
    }
    return list;
  }
}
'@

$procs = @(Get-Process -Name $ProcessName -ErrorAction SilentlyContinue)
if ($procs.Count -eq 0) {
  Write-Host ('找不到进程 {0}：请先启动应用' -f $ProcessName)
  exit 1
}
$pid0 = [uint32]$procs[0].Id
Write-Host ('监控进程 {0}（pid={1}），共 {2} 秒，每 {3}ms 采样一次。' -f $ProcessName, $pid0, $Seconds, $IntervalMs)
Write-Host '请现在：把鼠标移到宠物身上 → 右键弹出菜单 → 让鼠标停在宠物身体上（压住菜单）→ 不要动。'
Write-Host ''
Write-Host '时间   Z序快照（每项 = z|尺寸|是否置顶|位置）'
Write-Host '-----  ------------------------------------------------------------'

$sw = [System.Diagnostics.Stopwatch]::StartNew()
$last = ''
while ($sw.Elapsed.TotalSeconds -lt $Seconds) {
  $snap = ([Zrec]::Snapshot($pid0) | Where-Object { $_ -notmatch '\|0x0\|' }) -join '  '
  if ($snap -ne $last) {
    $line = ('{0,5:N1}s  {1}' -f $sw.Elapsed.TotalSeconds, $snap)
    Write-Host $line
    $last = $snap
  }
  Start-Sleep -Milliseconds $IntervalMs
}
Write-Host ''
Write-Host '把上面整段输出发我。判读方法：'
Write-Host '  780x520 是菜单窗、420x236 是宠物窗；谁前面的 z 更小，谁就在屏幕更上层。'
Write-Host '  如果出现「420x236 的 z 小于 780x520 的 z」，就是宠物被抬到菜单之上了。'
