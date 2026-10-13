# Foreground-window helper for `gate focus-loss` (Windows only).
#
#   foreground.ps1 -ProcessId <pid> [-HoldSeconds 1.0]
#
# Gives the visible top-level window of process <pid> the OS foreground, checks it has it,
# then at once opens a small borderless form off-screen and takes the foreground for that,
# so the window loses it (WM_ACTIVATE/WM_KILLFOCUS, winit's `Focused(false)`). Holds the
# form for -HoldSeconds (pumping its messages), hands the foreground back to whatever had
# it first (only if the form still has it, and never to a manifold process's window), and
# closes the form.
# One process for the whole cycle: other windows (the Claude app) take the foreground back
# within seconds, so a gap of a PowerShell start between the two steps lost it every time.
#
# Prints "OK hwnd=<h> take=<method> steal=<method> before=<fg before the steal>
# during=<fg at the end of the hold> lost=<1|0> restored=<1|0>" plus described windows,
# or "SKIP <reason>" (no such window, or the foreground refused).
#
# SetForegroundWindow is refused unless the caller owns the last input event or the
# foreground; the fallbacks are AttachThreadInput to the foreground thread, then a
# simulated Alt press around the call (keybd_event: Alt down, set, Alt up, so the Alt-up
# lands on the new foreground window and no menu opens in the old one).
param(
    [Parameter(Mandatory = $true)][int]$ProcessId,
    [double]$HoldSeconds = 1.0
)
$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public static class Fg {
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool BringWindowToTop(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool AttachThreadInput(uint a, uint b, bool attach);
    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern IntPtr GetWindow(IntPtr h, uint cmd);
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);

    public static uint ProcessOf(IntPtr h) { uint pid; GetWindowThreadProcessId(h, out pid); return pid; }

    public static string Describe(IntPtr h) {
        var t = new StringBuilder(256); var c = new StringBuilder(256);
        GetWindowText(h, t, 256); GetClassName(h, c, 256);
        uint pid; GetWindowThreadProcessId(h, out pid);
        return h.ToInt64() + " '" + t + "' [" + c + "] pid " + pid;
    }

    // Visible, unowned top-level windows of one process, less winit's helper window
    // ("Winit Thread Event Target", which reports itself visible).
    public static List<IntPtr> WindowsOf(uint pid) {
        var found = new List<IntPtr>();
        EnumWindows((h, l) => {
            uint p; GetWindowThreadProcessId(h, out p);
            var c = new StringBuilder(256); GetClassName(h, c, 256);
            if (p == pid && IsWindowVisible(h) && GetWindow(h, 4 /* GW_OWNER */) == IntPtr.Zero
                && c.ToString() != "Winit Thread Event Target") found.Add(h);
            return true;
        }, IntPtr.Zero);
        return found;
    }

    static bool Settled(IntPtr h) { Thread.Sleep(100); return GetForegroundWindow() == h; }

    // The method that worked, or null if every one was refused.
    public static string Take(IntPtr h) {
        if (GetForegroundWindow() == h) return "already";
        SetForegroundWindow(h);
        if (Settled(h)) return "plain";
        uint ignored;
        uint fgThread = GetWindowThreadProcessId(GetForegroundWindow(), out ignored);
        uint me = GetCurrentThreadId();
        if (fgThread != 0 && fgThread != me && AttachThreadInput(me, fgThread, true)) {
            BringWindowToTop(h);
            SetForegroundWindow(h);
            AttachThreadInput(me, fgThread, false);
            if (Settled(h)) return "attach";
        }
        keybd_event(0x12 /* VK_MENU */, 0, 0, UIntPtr.Zero);
        SetForegroundWindow(h);
        keybd_event(0x12, 0, 2 /* KEYEVENTF_KEYUP */, UIntPtr.Zero);
        if (Settled(h)) return "alt";
        return null;
    }
}
'@
Add-Type -AssemblyName System.Windows.Forms

$wins = [Fg]::WindowsOf([uint32]$ProcessId)
if ($wins.Count -ne 1) {
    $list = ($wins | ForEach-Object { [Fg]::Describe($_) }) -join '; '
    Write-Output "SKIP expected one visible top-level window for pid ${ProcessId}, found $($wins.Count): $list"
    exit 0
}
$h = $wins[0]
# Built and shown before the take, so the steal follows it at once.
$form = New-Object System.Windows.Forms.Form
$form.Text = 'focus-loss-probe'
$form.FormBorderStyle = 'None'
$form.ShowInTaskbar = $false
$form.StartPosition = 'Manual'
$form.Location = New-Object System.Drawing.Point(-32000, -32000)
$form.Size = New-Object System.Drawing.Size(64, 64)
try {
    $orig = [Fg]::GetForegroundWindow()
    $form.Show()
    [System.Windows.Forms.Application]::DoEvents()
    $take = [Fg]::Take($h)
    if (-not $take) { Write-Output "SKIP could not take foreground for $([Fg]::Describe($h))"; exit 0 }
    $before = [Fg]::GetForegroundWindow()
    $steal = [Fg]::Take($form.Handle)
    if (-not $steal) { Write-Output "SKIP could not take foreground for the throwaway form"; exit 0 }
    $deadline = [DateTime]::UtcNow.AddSeconds($HoldSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        [System.Windows.Forms.Application]::DoEvents()
        Start-Sleep -Milliseconds 50
    }
    $during = [Fg]::GetForegroundWindow()
    $lost = [int]($during -ne $h)
    # Hand it back only if the form still has it (the user may have clicked somewhere since), and never to a
    # manifold window: before 2026-10-07 a headless instance or its engine host could hold it (foreground-watch.ps1).
    $restored = 0
    $origPid = [uint32]0
    if ($orig -ne [IntPtr]::Zero) { $origPid = [Fg]::ProcessOf($orig) }
    $origName = (Get-Process -Id $origPid -ErrorAction SilentlyContinue).ProcessName
    if ($during -eq $form.Handle -and $orig -ne [IntPtr]::Zero -and $origName -and $origName -notlike 'manifold*') {
        $restored = [int][bool]([Fg]::Take($orig))
    }
    Write-Output ("OK hwnd=$($h.ToInt64()) take=$take steal=$steal before=$($before.ToInt64()) " +
        "during=$($during.ToInt64()) lost=$lost restored=$restored")
    Write-Output "  window: $([Fg]::Describe($h))"
    Write-Output "  orig:   $([Fg]::Describe($orig))"
    Write-Output "  during: $([Fg]::Describe($during))"
} finally {
    $form.Close()
    $form.Dispose()
}
