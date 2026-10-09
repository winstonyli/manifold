# Usage: powershell -NoProfile -File scripts/cpu-sampler.ps1 -Out FILE -Stop FILE [-MaxMinutes 90]
# Appends "<unix seconds> <total cpu %>" to Out about every 4 s until the Stop file exists (gate.sh creates it) or
# MaxMinutes pass, so it cannot outlive a gate that crashed.
param([string]$Out, [string]$Stop, [int]$MaxMinutes = 90)
$deadline = (Get-Date).AddMinutes($MaxMinutes)
while (-not (Test-Path -LiteralPath $Stop) -and (Get-Date) -lt $deadline) {
    $v = (Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 2 -MaxSamples 2 -ErrorAction SilentlyContinue).CounterSamples[-1].CookedValue
    if ($null -ne $v) { '{0} {1}' -f [DateTimeOffset]::UtcNow.ToUnixTimeSeconds(), [math]::Round($v, 1) | Add-Content -LiteralPath $Out -Encoding ascii }
}
