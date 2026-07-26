# Admission sandbox wrapper — Windows x64.
#
# Isolation: Docker Windows container with process isolation.
#
#   docker run --rm
#     --isolation process    (Hyper-V not available on GitHub-hosted runners)
#     --network none         (no outbound network)
#     --read-only            (container filesystem is read-only)
#     --memory 512m          (resource cap)
#     mcr.microsoft.com/windows/servercore:ltsc2022
#     powershell ...probe...
#
# Three probes matching the POSIX fixture contract:
#   BENIGN — write C:\scratch\ok.txt; must succeed.
#   NET    — TCP connect to 93.184.216.34:80; must fail (--network none).
#   FS     — write C:\Windows\System32\jail-escape-probe; must fail (--read-only).
#
# Fail-closed: if docker is absent or process-isolation containers cannot start,
# exit non-zero and the job goes red.
#
# Documented constraint: process-isolation mode requires the container OS version
# to match the host OS. windows-2022 runners run Windows Server 2022; the
# servercore:ltsc2022 image matches. The nanoserver image was not used because
# it does not ship PowerShell.

param(
    [string]$Fixture = "tests/fixtures/admission/untrusted-build.sh",
    [int]$TimeoutSecs = 120
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Fail-closed: docker must be present.
if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
    Write-Error "ERROR: docker not found — cannot establish jail"
    exit 1
}

# The probe script runs entirely in PowerShell inside the container.
# A separate scratch volume is mounted at C:\scratch for the benign write.
$ProbeScript = @'
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ── probe 1: benign write ────────────────────────────────────────────────────
try {
    [System.IO.File]::WriteAllText("C:\scratch\ok.txt", "benign-write")
    Write-Host "PROBE benign: wrote C:\scratch\ok.txt -- OK"
} catch {
    Write-Host "PROBE benign: FAIL - $_"
    exit 1
}

# ── probe 2: network attempt ──────────────────────────────────────────────────
$netBlocked = $false
try {
    $client = New-Object System.Net.Sockets.TcpClient
    $task = $client.ConnectAsync("93.184.216.34", 80)
    $completed = $task.Wait(3000)
    if ($completed -and -not $task.IsFaulted) {
        $client.Close()
        $netBlocked = $false
    } else {
        $netBlocked = $true
    }
} catch {
    $netBlocked = $true
}
if ($netBlocked) {
    Write-Host "PROBE net: blocked -- OK"
} else {
    Write-Host "PROBE net: NOT blocked -- FAIL"
    exit 2
}

# ── probe 3: filesystem escape attempt ───────────────────────────────────────
# --read-only makes the container filesystem read-only; only the mounted
# scratch volume at C:\scratch is writable.
$fsBlocked = $false
try {
    [System.IO.File]::WriteAllText("C:\Windows\System32\jail-escape-probe", "escape")
    $fsBlocked = $false
} catch {
    $fsBlocked = $true
}
if ($fsBlocked) {
    Write-Host "PROBE fs-escape: blocked -- OK"
} else {
    Write-Host "PROBE fs-escape: NOT blocked -- FAIL"
    exit 3
}

Write-Host "All probes passed."
exit 0
'@

$TempScript = [System.IO.Path]::GetTempFileName() + ".ps1"
$ProbeScript | Set-Content -Encoding UTF8 -Path $TempScript
$ScratchVol = "admission-scratch-$([System.Guid]::NewGuid().ToString('N').Substring(0,8))"

try {
    # Create a named volume for the writable scratch dir.
    docker volume create $ScratchVol | Out-Null

    # mcr.microsoft.com/windows/servercore:ltsc2022 ships PowerShell and
    # matches the windows-2022 runner OS version (required for process isolation).
    $DockerArgs = @(
        'run', '--rm',
        '--isolation', 'process',
        '--network', 'none',
        '--read-only',
        '--memory', '512m',
        '--cpus', '1',
        '-v', "${TempScript}:C:\probe.ps1:ro",
        '-v', "${ScratchVol}:C:\scratch",
        'mcr.microsoft.com/windows/servercore:ltsc2022',
        'powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', 'C:\probe.ps1'
    )

    # Start-Process -PassThru returns the process object immediately; -Wait blocks
    # until it exits. ExitCode is populated after the process terminates.
    $Proc = Start-Process docker -ArgumentList $DockerArgs -PassThru -NoNewWindow
    $TimedOut = -not $Proc.WaitForExit($TimeoutSecs * 1000)
    if ($TimedOut) {
        $Proc.Kill()
        Write-Error "ERROR: jail timed out after ${TimeoutSecs}s"
        exit 1
    }
    $ExitCode = $Proc.ExitCode
    if ($ExitCode -ne 0) {
        Write-Error "ERROR: jail probe failed with exit code $ExitCode"
        exit $ExitCode
    }
    Write-Host "Windows jail: all probes passed."
} finally {
    Remove-Item -Force -ErrorAction SilentlyContinue $TempScript
    docker volume rm -f $ScratchVol 2>$null | Out-Null
}
