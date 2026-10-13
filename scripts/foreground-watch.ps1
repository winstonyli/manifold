# Watches the OS foreground across one headless launch (Windows only).
#
#   foreground-watch.ps1 [-Engine webview2|cef] [-Seconds 8] [-Permissive] [-Visible] [-Exe <manifold.exe>]
#
# Whether a background process may take the foreground depends on the moment: refused while
# the user is active in another app, allowed when no window has the foreground or the
# foreground lock has timed out. -Permissive makes it the worst case on purpose: an off-screen
# form takes the foreground first (the same Alt-press fallback as foreground.ps1) and calls
# AllowSetForegroundWindow(ASFW_ANY), so any activation by the instance does take it. It
# steals the foreground for the run (don't type meanwhile) and hands it back at the end.
#
# Starts `manifold.exe --headless` on a throwaway profile (its own APPDATA, one tab on
# -Engine) at BelowNormal, polls every 50 ms for -Seconds, sends QUIT, and keeps polling
# 2 s more while it shuts down. Logs:
#   - every change of GetForegroundWindow (hwnd, class, title, owning process);
#   - each window the instance's processes own (manifold.exe and its engine host) when it
#     first appears, with its styles;
#   - every change of each of those GUI threads' own active/focus window (GetGUIThreadInfo),
#     which shows a window activating inside its thread even when Windows refuses it the
#     foreground.
# -Visible launches a normal on-screen window instead (`--debug-commands`, so QUIT still
# works): it should take the foreground and keep it, its engine host never taking it.
# Ends with "VERDICT foreground unchanged" or "VERDICT foreground taken by <process>" (only
# changes to one of the instance's windows count; a change to some other app is reported but
# not blamed on it). Kills only the processes it started.
param(
    [ValidateSet('webview2', 'cef')][string]$Engine = 'webview2',
    [double]$Seconds = 8,
    [switch]$Permissive,
    [switch]$Visible,
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\debug\manifold.exe')
)
$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class Fw {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int l, t, r, b; }
    [StructLayout(LayoutKind.Sequential)] public struct GUITHREADINFO {
        public int cbSize; public int flags; public IntPtr hwndActive, hwndFocus, hwndCapture, hwndMenuOwner,
        hwndMoveSize, hwndCaret; public RECT rcCaret;
    }
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool AllowSetForegroundWindow(int pid);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool GetGUIThreadInfo(uint tid, ref GUITHREADINFO info);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern IntPtr GetWindowLongPtr(IntPtr h, int i);
    [DllImport("user32.dll")] static extern IntPtr GetWindow(IntPtr h, uint cmd);
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);

    public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
    public static uint Tid(IntPtr h) { uint p; return GetWindowThreadProcessId(h, out p); }

    public static string Describe(IntPtr h) {
        if (h == IntPtr.Zero) return "0 (none)";
        var t = new StringBuilder(256); var c = new StringBuilder(256);
        GetWindowText(h, t, 256); GetClassName(h, c, 256);
        return "0x" + h.ToInt64().ToString("x") + " [" + c + "] '" + t + "' pid " + Pid(h);
    }

    public static string Styles(IntPtr h) {
        return "style=0x" + GetWindowLongPtr(h, -16).ToInt64().ToString("x8") +
            " exstyle=0x" + GetWindowLongPtr(h, -20).ToInt64().ToString("x8") +
            " visible=" + IsWindowVisible(h) + " owner=0x" + GetWindow(h, 4).ToInt64().ToString("x");
    }

    // Top-level windows owned by any of `pids`.
    public static List<IntPtr> WindowsOf(HashSet<uint> pids) {
        var found = new List<IntPtr>();
        EnumWindows((h, l) => { if (pids.Contains(Pid(h))) found.Add(h); return true; }, IntPtr.Zero);
        return found;
    }

    // "active=<window> focus=<window>" for one GUI thread.
    public static string ThreadState(uint tid) {
        var g = new GUITHREADINFO(); g.cbSize = Marshal.SizeOf(typeof(GUITHREADINFO));
        if (!GetGUIThreadInfo(tid, ref g)) return "(no info)";
        return "active=" + Describe(g.hwndActive) + " focus=0x" + g.hwndFocus.ToInt64().ToString("x");
    }
}
'@

$Exe = (Resolve-Path $Exe).Path
$root = Join-Path ([IO.Path]::GetTempPath()) ("fgwatch-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force (Join-Path $root 'manifold') | Out-Null
"{`"has_shown_first_launch_demo`": true, `"default_starting_view_kind`": `"$Engine`"}" |
    Out-File -Encoding ascii (Join-Path $root 'manifold\settings.json')

$sw = [Diagnostics.Stopwatch]::StartNew()
function Log([string]$s) { Write-Output ("{0,6} ms  {1}" -f $sw.ElapsedMilliseconds, $s) }

$psi = New-Object Diagnostics.ProcessStartInfo
$psi.FileName = $Exe
$psi.Arguments = $(if ($Visible) { '--debug-commands' } else { '--headless' })
$psi.UseShellExecute = $false
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.EnvironmentVariables['APPDATA'] = $root

$fg0 = [Fw]::GetForegroundWindow()
$orig = $fg0
Log "start foreground: $([Fw]::Describe($fg0)) ($((Get-Process -Id ([Fw]::Pid($fg0)) -EA 0).ProcessName))"
$form = $null
if ($Permissive) {
    # The worst case for a background launch: the foreground lock open to every process.
    Add-Type -AssemblyName System.Windows.Forms
    $form = New-Object System.Windows.Forms.Form
    $form.Text = 'foreground-watch'
    $form.FormBorderStyle = 'None'
    $form.ShowInTaskbar = $false
    $form.StartPosition = 'Manual'
    $form.Location = New-Object System.Drawing.Point(-32000, -32000)
    $form.Size = New-Object System.Drawing.Size(64, 64)
    $form.Show()
    [System.Windows.Forms.Application]::DoEvents()
    if (-not [Fw]::SetForegroundWindow($form.Handle) -or [Fw]::GetForegroundWindow() -ne $form.Handle) {
        # Alt down, set, Alt up: the Alt-up lands on the form, so no menu opens elsewhere.
        [Fw]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero)
        [void][Fw]::SetForegroundWindow($form.Handle)
        [Fw]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 100
    }
    if ([Fw]::GetForegroundWindow() -ne $form.Handle) {
        $form.Close()
        Write-Output 'SKIP could not take the foreground for -Permissive'
        exit 0
    }
    [void][Fw]::AllowSetForegroundWindow(-1) # ASFW_ANY
    $fg0 = $form.Handle
    Log "permissive: foreground taken by an off-screen form, AllowSetForegroundWindow(ASFW_ANY)"
}
$p = [Diagnostics.Process]::Start($psi)
$outTask = $p.StandardOutput.ReadToEndAsync()
$errTask = $p.StandardError.ReadToEndAsync()
try { $p.PriorityClass = 'BelowNormal' } catch {}
Log "launched manifold pid $($p.Id) (engine $Engine, profile $root)"
$p.StandardInput.WriteLine('WAITTABS')
$p.StandardInput.Flush()

$pids = New-Object 'System.Collections.Generic.HashSet[uint32]'
[void]$pids.Add([uint32]$p.Id)
$seen = @{}
$threadState = @{}
$fg = $fg0
$takenBy = $null
$quitAt = $Seconds * 1000
$endAt = $quitAt + 2000
$quitSent = $false
$nextScan = 0
while ($sw.ElapsedMilliseconds -lt $endAt) {
    $now = $sw.ElapsedMilliseconds
    if (-not $quitSent -and $now -ge $quitAt) {
        $atQuit = [Fw]::GetForegroundWindow()
        $atQuitDesc = [Fw]::Describe($atQuit)
        $atQuitOwn = [Fw]::Pid($atQuit) -eq $p.Id
        Log 'sending QUIT'
        try { $p.StandardInput.WriteLine('QUIT'); $p.StandardInput.Flush() } catch {}
        $quitSent = $true
    }
    if ($now -ge $nextScan) {
        # Child processes (the engine host and its own children), every 500 ms: CIM is slow.
        foreach ($c in Get-CimInstance Win32_Process -Filter "ParentProcessId=$($p.Id)" -EA 0) {
            if ($pids.Add([uint32]$c.ProcessId)) { Log "child process $($c.ProcessId) $($c.Name)" }
        }
        $nextScan = $now + 500
    }
    foreach ($h in [Fw]::WindowsOf($pids)) {
        $k = $h.ToInt64()
        if (-not $seen.ContainsKey($k)) {
            $seen[$k] = $true
            Log "window appeared: $([Fw]::Describe($h)) $([Fw]::Styles($h))"
        }
        $tid = [Fw]::Tid($h)
        if ($tid -ne 0) {
            $st = [Fw]::ThreadState($tid)
            if ($threadState[$tid] -ne $st) { $threadState[$tid] = $st; Log "thread $tid of pid $([Fw]::Pid($h)): $st" }
        }
    }
    $cur = [Fw]::GetForegroundWindow()
    if ($cur -ne $fg) {
        $owner = [Fw]::Pid($cur)
        $mine = $pids.Contains([uint32]$owner)
        Log ("foreground -> $([Fw]::Describe($cur)) ($((Get-Process -Id $owner -EA 0).ProcessName))" + $(if ($mine) { '  <== INSTANCE' } else { '' }))
        if ($mine -and -not $takenBy) { $takenBy = "$([Fw]::Describe($cur))" }
        $fg = $cur
    }
    if ($form) { [System.Windows.Forms.Application]::DoEvents() }
    Start-Sleep -Milliseconds 50
}
if ($form) {
    # Hand the foreground back only if the form still has it and it was not a manifold window.
    $origName = (Get-Process -Id ([Fw]::Pid($orig)) -EA 0).ProcessName
    if ([Fw]::GetForegroundWindow() -eq $form.Handle -and $orig -ne [IntPtr]::Zero -and $origName -notlike 'manifold*') {
        [void][Fw]::SetForegroundWindow($orig)
    }
    $form.Close()
    $form.Dispose()
}

if (-not $p.WaitForExit(10000)) {
    Log 'still running 10 s after QUIT: killing its tree'
    & taskkill /F /T /PID $p.Id | Out-Null
}
$log = $outTask.Result + $errTask.Result
foreach ($line in ($log -split "`r?`n" | Where-Object { $_ } | Select-Object -Last 5)) { Log "app: $line" }
Remove-Item -Recurse -Force $root -EA 0
if ($Visible) {
    Write-Output ("VERDICT visible: foreground at QUIT was $atQuitDesc" + $(if ($atQuitOwn) { ' (its own window: OK)' } else { ' (NOT its own window)' }))
} elseif ($takenBy) { Write-Output "VERDICT foreground taken by $takenBy" } else { Write-Output 'VERDICT foreground unchanged' }
