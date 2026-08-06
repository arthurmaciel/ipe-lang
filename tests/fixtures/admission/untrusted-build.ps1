# Tier-2 admission-sandbox probe wrapper -- Windows-native (CreateProcessW, no shell).
#
# The Windows returning build jail (ipe_sandbox build_in_jail) runs payload[0]
# directly through CreateProcessW -- there is no shell, so the POSIX
# untrusted-build.sh (driven via /usr/bin/env ... /bin/sh) cannot execute here.
# This is its Windows-native equivalent: powershell.exe is the interpreter, so
# the jail invokes it as payload[0] and hands this script as -File, exactly as a
# CreateProcessW payload. It implements the SAME wrapper-owned per-axis exit
# contract the decoder (CapabilityAxis::from_exit_code) reads.
#
# It runs ONLY the tier2 differential-confinement contract (the enforce/control
# admission modes live in tools/scripts/admission/jail-windows.ps1 and the POSIX
# fixture). Inside a jail scoped to a package's DECLARED capability set, a DENIED
# action names the axis the native code demanded but the declared set withheld
# (used-but-undeclared). The exit code is the wrapper-owned per-axis denial
# signal the Tier-2 decoder reads -- never scraped from the payload's stdout.
#
# Config travels through NAMED PARAMETERS on the command line (which flow through
# CreateProcessW), never the environment: the Windows jail scrubs the child
# environment to a fixed minimal allowlist, so an env-carried selector would
# either be dropped or require widening the env axis. The untrusted build command
# follows the "--" terminator and is captured in $args -- run as this wrapper's
# CHILD, so the wrapper always owns the exit and the untrusted build can never
# forge a clean exit.
#
# Axis selector (-Tier2Axis):
#   none        the FULL declared-scoped real-build run: NO fixed axis probe runs
#               (a fixed probe would fabricate a demand the package never made).
#               The signal is the child build's own exit -- clean (0) is positive
#               proof the build reached no withheld axis; a non-zero child build
#               is BuildFailed (exit 6), never clean.
#   network     probe ONLY the network axis (skip the fs-escape probe).
#   filesystem  probe ONLY the filesystem axis (skip the network probe).
#   both        probe both axes (the wrapper-probe-only enforce/control shape).
#
# Exit codes (the single source of truth mirrored by the Rust decoder
# ipe_sandbox::build_jail::CapabilityAxis::from_exit_code / JailOutcome::decode,
# and identical to the POSIX fixture):
#   0   probes clean: build clean + no withheld axis demanded (AXIS_EXIT_CLEAN)
#   6   the untrusted child build failed for an ordinary (non-capability) reason
#       with NO withheld axis demanded -- BuildFailed (TIER2_EXIT_BUILD_FAILED).
#       THE LOAD-BEARING HINGE: the wrapper NEVER exits 0 when the child build
#       failed, or a broken build would forge a clean certify.
#   10  network denied -- the native code demanded the withheld network axis
#       (AXIS_EXIT_NETWORK, used-but-undeclared)
#   11  fs-escape denied -- the native code demanded the withheld filesystem axis
#       (AXIS_EXIT_FILESYSTEM, used-but-undeclared)

param(
    [ValidateSet("none", "network", "filesystem", "both")]
    [string]$Tier2Axis = "both",
    [string]$ScratchDir = "",
    [string]$EscapePath = "",
    [string]$NetHost = "1.1.1.1",
    [int]$NetPort = 53,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$UntrustedBuild = @()
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# Wrapper-owned per-axis exit codes -- must equal the Rust decoder's constants.
$EXIT_CLEAN = 0
$EXIT_BUILD_FAILED = 6
$EXIT_NET_DENIED = 10
$EXIT_FS_DENIED = 11

# The fs-escape target lives OUTSIDE the ACLed scratch. Default to the process
# temp root when the caller passes none, so the probe still targets an
# out-of-scratch path (denied under a filesystem-withholding jail, writable to
# the control principal outside the jail).
if ([string]::IsNullOrEmpty($EscapePath)) {
    $EscapePath = [System.IO.Path]::Combine($env:TEMP, "tier2-escape-probe")
}

# Attempt an out-of-scratch write. Returns $true on success, $false on
# denial/failure (a read-only/denied target throws, caught quietly).
function Test-FsWrite {
    try {
        [System.IO.File]::WriteAllText($EscapePath, "jail-escape")
        return $true
    } catch {
        return $false
    }
}

# Attempt a TCP connect. Returns $true on success, $false on denial/failure.
# A withheld-network AppContainer denies the socket (connect throws / times out).
function Test-NetConnect {
    $client = $null
    try {
        $client = New-Object System.Net.Sockets.TcpClient
        $async = $client.BeginConnect($NetHost, $NetPort, $null, $null)
        $ok = $async.AsyncWaitHandle.WaitOne(5000, $false)
        if (-not $ok) {
            return $false
        }
        $client.EndConnect($async)
        return $true
    } catch {
        return $false
    } finally {
        if ($null -ne $client) {
            $client.Close()
        }
    }
}

# -- benign write (jailed run only): prove the scratch is writable -------------
if (-not [string]::IsNullOrEmpty($ScratchDir)) {
    try {
        [System.IO.File]::WriteAllText(
            [System.IO.Path]::Combine($ScratchDir, "ok.txt"), "benign-write")
        Write-Host "PROBE benign: wrote scratch ok.txt -- OK"
    } catch {
        # A false-deny of the benign write is an environment/setup error, not a
        # capability denial; surface it as BuildFailed (never clean).
        Write-Host "PROBE benign: FAIL (scratch not writable) -- $_"
        exit $EXIT_BUILD_FAILED
    }
}

# -- run the untrusted child build (the strictly subordinate tail) -------------
# The wrapper runs its untrusted build as a CHILD before its own fixed axis
# probe. A withheld-axis operation inside the child is denied by the jail; that
# same withheld axis also trips the wrapper's post-build probe below (the jail
# withholds the axis for the whole session), so the per-axis code names it. A
# child that fails for an ordinary reason with no axis demanded is code 6
# (BuildFailed) -- the wrapper NEVER exits 0 when the child build failed.
$childStatus = 0
if ($UntrustedBuild.Count -gt 0) {
    $exe = $UntrustedBuild[0]
    $childArgs = @()
    if ($UntrustedBuild.Count -gt 1) {
        $childArgs = $UntrustedBuild[1..($UntrustedBuild.Count - 1)]
    }
    try {
        & $exe @childArgs
        $childStatus = $LASTEXITCODE
        if ($null -eq $childStatus) {
            $childStatus = 0
        }
    } catch {
        # A spawn failure (executable unfindable, etc.) is an ordinary build
        # failure, never a capability denial.
        $childStatus = 1
    }
    Write-Host "PROBE child-build tier2: exit $childStatus"
}

# -- FULL declared-scoped run (-Tier2Axis none): child-exit-only ---------------
# On the FULL declared-scoped real-build run the wrapper runs NO fixed axis
# probe: a fixed probe would fabricate a capability demand the package never made
# (a socket / out-of-scratch write the build did not do), so no declared set
# other than {network,filesystem} could ever certify. The signal is the child
# build's own exit: a withheld axis is withheld by capability REMOVAL (the
# AppContainer denies the socket; the escape path is not ACLed writable), so a
# build that reaches it is denied / errors -> non-zero -> BuildFailed. A build
# that caught the error and exited anyway performed NO effect (a caught denial is
# a no-op), so exit 0 is positive proof the build reached no withheld axis. The
# child build we run is a fixed cargo build of a generated probe crate (our argv,
# our probe main), so the untrusted crate cannot own this exit's meaning.
if ($Tier2Axis -eq "none") {
    if ($childStatus -ne 0) {
        Write-Host "PROBE full-run tier2: child build failed (exit $childStatus) -- BuildFailed"
        exit $EXIT_BUILD_FAILED
    }
    Write-Host "PROBE full-run tier2: child build clean -- no withheld axis reached"
    exit $EXIT_CLEAN
}

# -- network probe -------------------------------------------------------------
# Skip when the caller selected the filesystem axis only, so a granted-but-
# unrouted host cannot trip a spurious network denial.
if ($Tier2Axis -eq "filesystem") {
    Write-Host "PROBE net tier2: skipped (axis=filesystem)"
} else {
    if (Test-NetConnect) {
        Write-Host "PROBE net tier2: reached -- network axis not withheld/not demanded"
    } else {
        Write-Host "PROBE net tier2: denied -- the code demanded the withheld network axis"
        exit $EXIT_NET_DENIED
    }
}

# -- filesystem-escape probe ---------------------------------------------------
# Skip when the caller selected the network axis only; the run then proves a
# CLEAN outcome (exit 0) for the network case (subject to the child-build hinge).
if ($Tier2Axis -eq "network") {
    Write-Host "PROBE fs-escape tier2: skipped (axis=network)"
    if ($childStatus -ne 0) {
        Write-Host "PROBE child-build tier2: ordinary failure (exit $childStatus), no axis demanded"
        exit $EXIT_BUILD_FAILED
    }
    Write-Host "All probes passed (tier2)."
    exit $EXIT_CLEAN
}

if (Test-FsWrite) {
    Write-Host "PROBE fs-escape tier2: wrote -- filesystem axis not withheld/not demanded"
    Remove-Item -LiteralPath $EscapePath -Force -ErrorAction SilentlyContinue
} else {
    Write-Host "PROBE fs-escape tier2: denied -- the code demanded the withheld filesystem axis"
    exit $EXIT_FS_DENIED
}

# The load-bearing hinge (tier2): no withheld axis was denied above, so a
# non-zero child build here is an ORDINARY build failure -- never a clean certify.
if ($childStatus -ne 0) {
    Write-Host "PROBE child-build tier2: ordinary failure (exit $childStatus), no axis demanded"
    exit $EXIT_BUILD_FAILED
}

Write-Host "All probes passed (tier2)."
exit $EXIT_CLEAN
