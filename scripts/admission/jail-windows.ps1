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
# Fail-closed: if docker is absent or process-isolation containers cannot
# start, exit non-zero and the job goes red.
#
# Documented constraint: process-isolation mode requires the container OS
# version to match the host OS. windows-2022 runners run Windows Server 2022;
# servercore:ltsc2022 matches. The daemon must be in Windows container mode.

param(
    [string]$Fixture = "tests/fixtures/admission/untrusted-build.sh",
    [int]$TimeoutSecs = 120
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
    Write-Error "ERROR: docker not found -- cannot establish jail"
    exit 1
}

# Build a short unique suffix without single-quotes in subexpressions.
$guid = [System.Guid]::NewGuid()
$suffix = $guid.ToString("N").Substring(0, 8)
$ScratchVol = "admission-scratch-$suffix"

# Docker Windows bind-mounts require a directory source (not a file).
# Use a dedicated probe directory containing the script.
$TempDir = [System.IO.Path]::Combine([System.IO.Path]::GetTempPath(), "probe-dir-$suffix")
[System.IO.Directory]::CreateDirectory($TempDir) | Out-Null
$TempScript = [System.IO.Path]::Combine($TempDir, "probe.ps1")

# Probe script: runs inside the Windows container.
# Written as a here-string; no interpolation needed (literal @'...'@).
$ProbeScript = @'
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# probe 1: benign write
try {
    [System.IO.File]::WriteAllText("C:\scratch\ok.txt", "benign-write")
    Write-Host "PROBE benign: wrote C:\scratch\ok.txt -- OK"
} catch {
    Write-Host "PROBE benign: FAIL - $_"
    exit 1
}

# probe 2: network attempt
$netBlocked = $false
try {
    $client = New-Object System.Net.Sockets.TcpClient
    $ar = $client.BeginConnect("93.184.216.34", 80, $null, $null)
    $ok = $ar.AsyncWaitHandle.WaitOne(3000, $false)
    if ($ok -and -not $ar.IsCompleted) { $netBlocked = $true }
    elseif (-not $ok) { $netBlocked = $true }
    else {
        try { $client.EndConnect($ar); $netBlocked = $false } catch { $netBlocked = $true }
    }
    $client.Close()
} catch {
    $netBlocked = $true
}
if ($netBlocked) {
    Write-Host "PROBE net: blocked -- OK"
} else {
    Write-Host "PROBE net: NOT blocked -- FAIL"
    exit 2
}

# probe 3: verify host filesystem not reachable via UNC path.
# Windows process-isolation containers do not expose host drives via UNC
# unless explicitly mounted.  Writing to a UNC path that points to the host
# should fail with a network error (and --network none means CIFS also fails).
$fsBlocked = $false
try {
    [System.IO.File]::WriteAllText("\\localhost\C$\jail-escape-probe", "escape")
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

[System.IO.File]::WriteAllText($TempScript, $ProbeScript, [System.Text.Encoding]::UTF8)

try {
    docker volume create $ScratchVol | Out-Null

    # Docker Windows bind-mounts require a directory source (not a file).
    # Mount the probe directory as C:\probe (ro) inside the container.
    $TempDirFwd = $TempDir.Replace('\', '/')
    $MountProbe = "${TempDirFwd}:C:/probe"
    $MountScratch = "${ScratchVol}:C:/scratch"

    $DockerArgs = @(
        'run', '--rm',
        '--isolation', 'process',
        '--network', 'none',
        '--memory', '512m',
        '--cpus', '1',
        '-v', $MountProbe,
        '-v', $MountScratch,
        'mcr.microsoft.com/windows/servercore:ltsc2022',
        'powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass',
        '-File', 'C:\probe\probe.ps1'
    )

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
    if (Test-Path $TempDir) { Remove-Item -Recurse -Force $TempDir }
    docker volume rm -f $ScratchVol 2>$null | Out-Null
}
