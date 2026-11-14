# Usage: powershell -File redux	oolsoffscreen.ps1 -Exe targeteleasebenilla.exe -Log run.log
# (with the WOW_* run variables in the environment). Starts benilla.exe and moves its window off every monitor (bottom of the z-order, never
# activated, never minimized) the moment it appears, so a probe run never covers the screen.
param([string]$Exe, [string]$Log, [int]$TimeoutSec = 150)

Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Win {
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
}
"@
Add-Type -AssemblyName System.Windows.Forms
$vs = [System.Windows.Forms.SystemInformation]::VirtualScreen
$x = $vs.Left - 5000
$HWND_BOTTOM = [IntPtr]1
$SWP = 0x0001 -bor 0x0010 -bor 0x0200  # NOSIZE | NOACTIVATE | NOOWNERZORDER

$p = Start-Process $Exe -RedirectStandardError $Log -RedirectStandardOutput "$Log.out" -PassThru -NoNewWindow
$moved = $false
$deadline = (Get-Date).AddSeconds($TimeoutSec)
while (-not $p.HasExited -and (Get-Date) -lt $deadline) {
  $p.Refresh()
  $h = $p.MainWindowHandle
  if ($h -ne [IntPtr]::Zero) {
    if ([Win]::IsIconic($h)) { [Win]::ShowWindow($h, 4) | Out-Null }  # SW_SHOWNOACTIVATE
    [Win]::SetWindowPos($h, $HWND_BOTTOM, $x, $vs.Top, 0, 0, $SWP) | Out-Null
    if (-not $moved) { "offscreen: moved window to x=$x"; $moved = $true }
  }
  Start-Sleep -Milliseconds 40
}
if (-not $p.HasExited) { Stop-Process $p -Force; "offscreen: timeout, killed" }
"exit $($p.ExitCode)"
