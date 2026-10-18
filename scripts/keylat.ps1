# Key-to-screen latency, A/B between builds of the host (Windows only; real window, takes the foreground).
#
#   keylat.ps1 -Build a=<exe>,b=<exe> [-Order a,b,b,a] [-Trials 25]
#
# Each exe is launched with --debug-commands in a visible window (its WebView2 host exe and DLLs must sit beside it,
# as in target/debug, so copy a build's whole set to its own directory). The window loads a page that flips its
# background on every key; the script sends a real key (keybd_event) and polls one screen pixel (GDI) until it
# changes. GDI returns one reading per compositor frame, so each result is quantized to the display's frame time
# (16.7 ms at 60 Hz); a difference between builds is what counts, not the absolute number (the monitor's scanout and
# pixel response are left out). Launches run in -Order (default A B B A over the first two builds) so drift hits both.
#
# Hands off for about 40 s per launch: the window takes the foreground and the script aborts if it loses it. Wait for
# the machine to be quiet first, and record the run through ~/.claude/runs/run.ps1 (-Priority Normal: a lowered
# priority would delay the poll). Prints per-launch medians (a launch can sit in either of two phases, a frame
# apart), then per-build median, mean and p90, and the second build minus the first.
#
# The page is served from 127.0.0.1:8123 (a file:/// address typed in the address bar is searched, not opened).
param(
    [Parameter(Mandatory)][string[]]$Build,
    [string[]]$Order,
    [int]$Trials = 25
)
$ErrorActionPreference = 'Stop'
$Build = @($Build -split ',')
$exes = [ordered]@{}
foreach ($b in $Build) {
    $name, $exe = $b -split '=', 2
    if (-not $exe -or -not (Test-Path $exe)) { throw "-Build wants name=<existing exe>, got '$b'" }
    $exes[$name] = (Resolve-Path $exe).Path
}
$names = @($exes.Keys)
if ($names.Count -lt 2) { throw 'need two builds to compare' }
$Order = if ($Order) { @($Order -split ',') } else { @($names[0], $names[1], $names[1], $names[0]) }
foreach ($o in $Order) { if (-not $exes.Contains($o)) { throw "-Order names '$o', which is not a -Build" } }

Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices; using System.Text; using System.Threading;
public static class K {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] static extern bool BringWindowToTop(IntPtr h);
  [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int hh, uint flags);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, IntPtr pid);
  [DllImport("user32.dll")] static extern bool AttachThreadInput(uint a, uint b, bool attach);
  [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] static extern IntPtr GetDC(IntPtr h);
  [DllImport("user32.dll")] static extern int ReleaseDC(IntPtr h, IntPtr dc);
  [DllImport("gdi32.dll")] static extern uint GetPixel(IntPtr dc, int x, int y);
  [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("kernel32.dll")] static extern bool QueryPerformanceCounter(out long c);
  [DllImport("kernel32.dll")] static extern bool QueryPerformanceFrequency(out long f);
  static long Q() { long c; QueryPerformanceCounter(out c); return c; }

  public static string Title(IntPtr h) { var sb = new StringBuilder(256); GetWindowText(h, sb, 256); return sb.ToString(); }
  // The visible top-level window of `pid` with the largest area (the process makes small throwaway windows first).
  public static IntPtr FindMain(int pid) {
    IntPtr best = IntPtr.Zero; long bestArea = 0;
    EnumWindows((h, l) => {
      uint wp; GetWindowThreadProcessId(h, out wp);
      if (wp == pid && IsWindowVisible(h)) { RECT r; GetWindowRect(h, out r); long a = (long)(r.R - r.L) * (r.B - r.T); if (a > bestArea) { bestArea = a; best = h; } }
      return true;
    }, IntPtr.Zero);
    return best;
  }
  public static bool IsForeground(IntPtr h) { return GetForegroundWindow() == h; }
  public static void Topmost(IntPtr h) { SetWindowPos(h, new IntPtr(-1), 0, 0, 0, 0, 0x13); } // on top, not activated
  public static bool Foreground(IntPtr h) {
    IntPtr fg = GetForegroundWindow();
    uint me = GetCurrentThreadId(), them = fg == IntPtr.Zero ? 0 : GetWindowThreadProcessId(fg, IntPtr.Zero);
    keybd_event(0x12, 0, 0, UIntPtr.Zero); keybd_event(0x12, 0, 2, UIntPtr.Zero); // an Alt tap lets SetForegroundWindow through
    bool att = them != 0 && them != me && AttachThreadInput(me, them, true);
    ShowWindow(h, 5); BringWindowToTop(h); SetForegroundWindow(h);
    if (att) AttachThreadInput(me, them, false);
    return GetForegroundWindow() == h;
  }
  public static POINT Probe(IntPtr h) { RECT r; GetClientRect(h, out r); POINT p = new POINT { X = r.R / 2, Y = r.B * 3 / 4 }; ClientToScreen(h, ref p); return p; }
  public static uint Pixel(POINT p) { IntPtr dc = GetDC(IntPtr.Zero); uint c = GetPixel(dc, p.X, p.Y); ReleaseDC(IntPtr.Zero, dc); return c; }
  public static void Click(POINT p) { SetCursorPos(p.X, p.Y); mouse_event(2, 0, 0, 0, UIntPtr.Zero); mouse_event(4, 0, 0, 0, UIntPtr.Zero); }
  // Milliseconds from a key press to the probe pixel changing, per trial (NaN: no change within 1 s); polls[0] gets
  // the mean interval between readings.
  public static double[] Trials(IntPtr h, POINT p, int n, double[] polls) {
    long f; QueryPerformanceFrequency(out f); var rnd = new Random(); var res = new double[n];
    IntPtr dc = GetDC(IntPtr.Zero); long pollCount = 0, pollTicks = 0;
    Thread.CurrentThread.Priority = ThreadPriority.AboveNormal;
    for (int i = 0; i < n; i++) {
      Thread.Sleep(300 + rnd.Next(400)); // a random phase against the display's frames
      if (GetForegroundWindow() != h) throw new Exception("lost the foreground");
      uint c0 = GetPixel(dc, p.X, p.Y);
      long t0 = Q(); keybd_event(0x41, 0x1E, 0, UIntPtr.Zero); keybd_event(0x41, 0x1E, 2, UIntPtr.Zero);
      double ms = double.NaN; long last = t0;
      while (true) {
        uint c = GetPixel(dc, p.X, p.Y); long t = Q(); pollCount++; pollTicks += t - last; last = t;
        if (c != c0) { ms = (t - t0) * 1000.0 / f; break; }
        if ((t - t0) * 1000.0 / f > 1000) break;
      }
      res[i] = ms;
    }
    ReleaseDC(IntPtr.Zero, dc);
    polls[0] = pollCount == 0 ? 0 : pollTicks * 1000.0 / f / pollCount;
    return res;
  }
}
'@
[void][K]::SetProcessDPIAware()

$work = Join-Path $env:TEMP 'manifold-keylat'
New-Item -ItemType Directory -Force $work | Out-Null
Set-Content -Encoding ascii "$work\lat2.html" @'
<!doctype html><meta charset=utf-8><body style="margin:0;background:#000;overflow:hidden">
<input id=i style="position:fixed;inset:0;width:100%;height:100%;opacity:0;border:0" autofocus>
<script>let n=0;i.oninput=()=>{i.value='';document.body.style.background=(++n%2)?'#fff':'#000'};i.focus()</script>
'@
$page = 'http://127.0.0.1:8123/lat2.html'
# ADDRESSBAR then a pause then ENTER: the typed text must land before the key does.
@('NEWTAB', 'WAITTABS', 'GRAPHOPEN', 'SLEEP 1', 'FOCUSADDRESS', "ADDRESSBAR $page", 'SLEEP 2', 'ENTER', 'SLEEP 600') |
    Set-Content -Encoding ascii "$work\keylat.script"

function Wait-Until([scriptblock]$cond, [int]$sec, [string]$what) {
    $end = (Get-Date).AddSeconds($sec)
    while (-not (& $cond)) { if ((Get-Date) -gt $end) { throw "timed out: $what" }; Start-Sleep -Milliseconds 100 }
}
function Median($v) { $s = @($v | Sort-Object); if ($s.Count) { $s[[int](($s.Count - 1) / 2)] } }

New-Item -ItemType File -Force "$work\empty.txt" | Out-Null
$srv = Start-Process -FilePath python -ArgumentList "-P -m http.server 8123 --bind 127.0.0.1 -d `"$work`"" -WindowStyle Hidden -PassThru `
    -RedirectStandardInput "$work\empty.txt" -RedirectStandardOutput "$work\srv.out" -RedirectStandardError "$work\srv.err"
$results = @{}; $pollMs = @()
try {
    $cpu = [int](Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 2 -MaxSamples 2).CounterSamples[-1].CookedValue
    "machine cpu before: $cpu%"
    foreach ($name in $Order) {
        $prof = Join-Path $work "prof-$name-$([guid]::NewGuid().ToString('N').Substring(0, 6))"
        $p = Start-Process -FilePath $exes[$name] -PassThru `
            -ArgumentList '--debug-commands', '--profile-dir', "`"$prof`"", '--script', "`"$work\keylat.script`""
        try {
            Wait-Until { [K]::FindMain($p.Id) -ne [IntPtr]::Zero } 40 'main window'
            # The scripted ENTER only reaches egui when the window has the OS focus: take it before the script gets there.
            # A hashtable carries state out of the scriptblock, whose plain-variable assignments are local to each call.
            $st = @{ h = [IntPtr]::Zero; prev = -1; since = $null; title = '' }
            Wait-Until {
                $hh = [K]::FindMain($p.Id) # the biggest window, not the throwaway the process makes first
                $st.title = if ($hh -eq [IntPtr]::Zero) { '' } else { [K]::Title($hh) }
                if ($st.title -eq '') { return $false }
                $st.h = $hh
                if (-not [K]::IsForeground($hh)) { [void][K]::Foreground($hh); [K]::Topmost($hh) }
                if ($st.title -like 'Welcome*') { return $false } # not navigated yet
                $px = [K]::Pixel([K]::Probe($hh))
                if ($px -eq $st.prev) { if (-not $st.since) { $st.since = Get-Date }; ((Get-Date) - $st.since).TotalSeconds -ge 1.5 }
                else { $st.since = $null; $st.prev = $px; $false }
            } 90 "page load (window title '$($st.title)')"
            $h = $st.h; $pt = [K]::Probe($h)
            Wait-Until { [K]::Foreground($h) } 10 'foreground'
            [K]::Click($pt) # focuses the page's full-size input
            $polls = New-Object double[] 1
            $warm = [K]::Trials($h, $pt, 3, $polls)
            if (@($warm | Where-Object { -not [double]::IsNaN($_) }).Count -eq 0) { throw 'warm-up keys changed nothing on screen' }
            $r = [K]::Trials($h, $pt, $Trials, $polls)
            if (-not $results.ContainsKey($name)) { $results[$name] = @() }
            $results[$name] += $r
            $pollMs += $polls[0]
            $ok = @($r | Where-Object { -not [double]::IsNaN($_) })
            '{0} launch: n={1} ok={2} median {3:N1} ms, mean poll {4:N2} ms' -f $name, $r.Count, $ok.Count, (Median $ok), $polls[0]
        } finally {
            & taskkill /PID $p.Id /T /F | Out-Null
        }
    }
    $cpu2 = [int](Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 1 -MaxSamples 2).CounterSamples[-1].CookedValue
    $summary = foreach ($name in $names) {
        if (-not $results.ContainsKey($name)) { continue }
        $v = @($results[$name] | Where-Object { -not [double]::IsNaN($_) } | Sort-Object)
        [pscustomobject]@{
            build = $name; n = $v.Count; timeouts = $results[$name].Count - $v.Count
            median = [math]::Round((Median $v), 1); mean = [math]::Round(($v | Measure-Object -Average).Average, 1)
            p90 = [math]::Round($v[[int](($v.Count - 1) * 0.9)], 1)
        }
    }
    $summary | Format-Table | Out-String | Write-Output
    'mean poll interval {0:N2} ms; machine cpu before/after {1}%/{2}%' -f (($pollMs | Measure-Object -Average).Average), $cpu, $cpu2
    if ($summary.Count -ge 2) {
        '{0} minus {1}: median {2:N1} ms, mean {3:N1} ms' -f $summary[1].build, $summary[0].build, ($summary[1].median - $summary[0].median), ($summary[1].mean - $summary[0].mean)
    }
    if ($env:RUN_DIR) { $results | ConvertTo-Json | Set-Content -Encoding utf8 "$env:RUN_DIR\samples.json" }
} finally {
    if ($srv -and -not $srv.HasExited) { Stop-Process -Id $srv.Id -Force }
}
