# Lists the visible top-level windows of a process as JSON (title, rect,
# extended style, click-through flag). Used by phase3-acceptance.mjs to check
# that floating robot windows are layered + click-through when idle.
# Reads window metadata only; never captures pixels.
# Usage: list-windows.ps1 -ProcessId 1234
param([Parameter(Mandatory = $true)][int]$ProcessId)
$ErrorActionPreference = 'Stop'
Add-Type @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class OrkWinList {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  public delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")] public static extern IntPtr GetWindowLongPtr(IntPtr h, int i);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  public static List<string> List(uint target) {
    var rows = new List<string>();
    EnumWindows((h, p) => {
      uint pid; GetWindowThreadProcessId(h, out pid);
      if (pid == target && IsWindowVisible(h)) {
        RECT r; GetWindowRect(h, out r);
        var sb = new StringBuilder(256); GetWindowText(h, sb, 256);
        long ex = GetWindowLongPtr(h, -20).ToInt64();
        rows.Add(String.Format("{{\"title\":\"{0}\",\"x\":{1},\"y\":{2},\"w\":{3},\"h\":{4},\"exStyle\":{5},\"layered\":{6},\"clickThrough\":{7},\"topmost\":{8}}}",
          sb.ToString().Replace("\\", "\\\\").Replace("\"", "\\\""), r.Left, r.Top, r.Right - r.Left, r.Bottom - r.Top, ex,
          ((ex & 0x80000) != 0) ? "true" : "false", ((ex & 0x20) != 0) ? "true" : "false", ((ex & 0x8) != 0) ? "true" : "false"));
      }
      return true;
    }, IntPtr.Zero);
    return rows;
  }
}
"@
[void][OrkWinList]::SetProcessDPIAware()
$ids = @($ProcessId) + @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$ProcessId" | ForEach-Object { $_.ProcessId })
$rows = @()
foreach ($id in $ids) { $rows += [OrkWinList]::List([uint32]$id) }
"[" + ($rows -join ",") + "]"
