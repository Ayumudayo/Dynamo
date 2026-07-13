param([switch]$NativeFreshChild)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Assert-Contract {
    param(
        [Parameter(Mandatory)][bool]$Condition,
        [Parameter(Mandatory)][string]$Message
    )

    if (-not $Condition) {
        throw "isolated-process-job-contract: $Message"
    }
}

function Assert-Rejected {
    param(
        [Parameter(Mandatory)][scriptblock]$Action,
        [Parameter(Mandatory)][string]$Case,
        [Parameter(Mandatory)][string]$ExpectedCode
    )

    $caught = $null
    $unexpected = $null
    try {
        $unexpected = & $Action
    }
    catch {
        $caught = $_
    }
    $cleanupFailures = [System.Collections.Generic.List[Exception]]::new()
    foreach ($candidate in @($unexpected)) {
        if ($candidate -is [Dynamo.Perf.Isolation.IsolatedJobProcess]) {
            try {
                Register-ContractProcess -Process $candidate
                Complete-ContractProcess -Process $candidate
            }
            catch {
                $cleanupFailures.Add($_.Exception)
            }
        }
    }
    if ($cleanupFailures.Count -gt 0) {
        throw [AggregateException]::new("$Case unexpected-process cleanup failed", $cleanupFailures)
    }

    Assert-Contract ($null -ne $caught) "$Case unexpectedly succeeded"
    $typedException = $caught.Exception
    while ($typedException -isnot [Dynamo.Perf.Isolation.IsolationInputException] -and $null -ne $typedException.InnerException) {
        $typedException = $typedException.InnerException
    }
    Assert-Contract ($typedException -is [Dynamo.Perf.Isolation.IsolationInputException]) "$Case failed in an untyped phase"
    Assert-Contract ($typedException.Phase -ceq 'ValidateInputs') "$Case failed outside ValidateInputs"
    Assert-Contract ($typedException.Code -ceq $ExpectedCode) "$Case returned the wrong validation code"
    $errorText = "$($caught.Exception.Message)`n$($caught.ScriptStackTrace)"
    Assert-Contract ($errorText -notmatch '(?i)(contract-secret-not-for-output|authorization:\s*bearer|client_secret|access_token|refresh_token|cookie:)') "$Case leaked sensitive input"
}

function New-ChildEnvironment {
    param(
        [Parameter(Mandatory)][string]$TemporaryDirectory,
        [hashtable]$Additional = @{}
    )

    $result = [ordered]@{
        SystemRoot = [Environment]::GetEnvironmentVariable('SystemRoot', 'Process')
        WINDIR = [Environment]::GetEnvironmentVariable('WINDIR', 'Process')
        TEMP = $TemporaryDirectory
        TMP = $TemporaryDirectory
    }
    foreach ($key in $Additional.Keys) {
        $result[$key] = [string]$Additional[$key]
    }
    $result
}

function Read-SharedText {
    param([Parameter(Mandatory)][string]$Path)

    $stream = [IO.FileStream]::new(
        $Path,
        [IO.FileMode]::Open,
        [IO.FileAccess]::Read,
        [IO.FileShare]::ReadWrite)
    $reader = [IO.StreamReader]::new($stream, [Text.UTF8Encoding]::new($false, $true), $true)
    try {
        $reader.ReadToEnd()
    }
    finally {
        $reader.Dispose()
        $stream.Dispose()
    }
}

function Start-ContractProcess {
    param(
        [Parameter(Mandatory)][string]$Executable,
        [Parameter(Mandatory)][AllowEmptyCollection()][object[]]$Arguments,
        [Parameter(Mandatory)][object]$Environment,
        [Parameter(Mandatory)][string]$WorkingDirectory,
        [Parameter(Mandatory)][string]$OutputDirectory,
        [Parameter(Mandatory)][string]$LeafPrefix
    )

    Start-DynamoIsolatedProcess `
        -ExecutablePath $Executable `
        -ArgumentList $Arguments `
        -WorkingDirectory $WorkingDirectory `
        -Environment $Environment `
        -StandardOutputPath (Join-Path $OutputDirectory "$LeafPrefix.stdout.log") `
        -StandardErrorPath (Join-Path $OutputDirectory "$LeafPrefix.stderr.log")
}

function Register-ContractProcess {
    param([Parameter(Mandatory)][object]$Process)

    Assert-Contract ($Process -is [Dynamo.Perf.Isolation.IsolatedJobProcess]) 'attempted to register an invalid process handle'
    $script:children.Add($Process)
    $nativeBirth = [DynamoIsolatedContractNativeV3]::GetProcessCreationFileTime([uint32]$Process.ProcessId)
    Assert-Contract ($nativeBirth -eq $Process.CreationFileTimeUtc) 'process creation identity disagrees with an independent native query'
    $script:processBirths[$Process.ProcessId] = $nativeBirth
}

function Complete-ContractProcess {
    param(
        [Parameter(Mandatory)][object]$Process,
        [switch]$AlreadyExited
    )

    $birthRegistered = $script:processBirths.ContainsKey($Process.ProcessId)
    $birthMatches = $birthRegistered -and $script:processBirths[$Process.ProcessId] -eq $Process.CreationFileTimeUtc
    if (-not $AlreadyExited -and $Process.IsJobOpen) {
        Stop-DynamoIsolatedProcess -Process $Process
    }
    $wait = Wait-DynamoIsolatedProcess -Process $Process -TimeoutMilliseconds 10000
    Assert-Contract $wait.Exited 'process did not reach a terminal state during cleanup'
    if ($Process.IsJobOpen) {
        $evidence = Get-DynamoIsolatedProcessEvidence -Process $Process
        Assert-Contract ($evidence.ActiveProcessCount -eq 0 -and $evidence.ActiveProcessIds.Count -eq 0) 'Job Object retained active PIDs during cleanup'
    }
    Remove-DynamoIsolatedProcess -Process $Process -TimeoutMilliseconds 10000
    $null = $script:children.Remove($Process)
    $null = $script:processBirths.Remove($Process.ProcessId)
    Assert-Contract $birthRegistered 'process had no registered creation identity before cleanup'
    Assert-Contract $birthMatches 'registered process creation identity changed before cleanup'
}

function Set-ContractFailurePoint {
    param([AllowNull()][string]$Point)

    $script:contractFailureField.SetValue($null, $Point)
}

function Invoke-FailedLaunchRecoveryCase {
    param(
        [Parameter(Mandatory)][string]$FailurePoint,
        [Parameter(Mandatory)][string]$LeafPrefix,
        [Parameter(Mandatory)][bool]$ExpectedJob
    )

    $caught = $null
    $unexpected = $null
    Set-ContractFailurePoint -Point $FailurePoint
    try {
        $unexpected = Start-ContractProcess `
            -Executable $script:pwsh `
            -Arguments @('-NoProfile', '-NonInteractive', '-Command', 'Start-Sleep -Seconds 60') `
            -Environment (New-ChildEnvironment -TemporaryDirectory $script:artifactRoot) `
            -WorkingDirectory $script:repository `
            -OutputDirectory $script:artifactRoot `
            -LeafPrefix $LeafPrefix
    }
    catch {
        $caught = $_
    }
    finally {
        Set-ContractFailurePoint -Point $null
    }

    if ($unexpected -is [Dynamo.Perf.Isolation.IsolatedJobProcess]) {
        Register-ContractProcess -Process $unexpected
    }
    Assert-Contract ($null -ne $caught) "$FailurePoint unexpectedly returned a process"

    $typedException = $null
    $pending = [System.Collections.Generic.Queue[Exception]]::new()
    $pending.Enqueue($caught.Exception)
    while ($pending.Count -gt 0 -and $null -eq $typedException) {
        $candidate = $pending.Dequeue()
        if ($candidate -is [Dynamo.Perf.Isolation.IsolationLaunchCleanupException]) {
            $typedException = $candidate
            break
        }
        if ($candidate -is [AggregateException]) {
            foreach ($inner in $candidate.InnerExceptions) {
                if ($null -ne $inner) { $pending.Enqueue($inner) }
            }
        }
        elseif ($null -ne $candidate.InnerException) {
            $pending.Enqueue($candidate.InnerException)
        }
    }
    Assert-Contract ($typedException -is [Dynamo.Perf.Isolation.IsolationLaunchCleanupException]) "$FailurePoint did not return a typed recovery exception"
    $recovery = $typedException.RecoveryProcess
    Assert-Contract ($recovery -is [Dynamo.Perf.Isolation.FailedLaunchRecoveryProcess]) "$FailurePoint did not transfer a failed-launch recovery lease"
    Assert-Contract ($recovery -is [Dynamo.Perf.Isolation.IIsolationRecoveryProcess]) "$FailurePoint recovery lease does not implement the lifecycle contract"
    $script:recoveryLeases.Add($recovery)
    Assert-Contract ($recovery.IsJobOpen -eq $ExpectedJob) "$FailurePoint returned the wrong Job ownership state"
    Assert-Contract ($recovery.ProcessId -gt 0 -and $recovery.CreationFileTimeUtc -gt 0) "$FailurePoint recovery lease has no process identity"

    $externalProcess = [Diagnostics.Process]::GetProcessById($recovery.ProcessId)
    try {
        $null = $externalProcess.Handle
        $externalBirth = [DynamoIsolatedContractNativeV3]::GetProcessCreationFileTime([uint32]$recovery.ProcessId)
        Assert-Contract ($externalBirth -eq $recovery.CreationFileTimeUtc) "$FailurePoint process identity disagrees with an independent handle"
        $preRecoveryWait = Wait-DynamoIsolatedProcess -Process $recovery -TimeoutMilliseconds 0
        Assert-Contract (-not $preRecoveryWait.Exited) "$FailurePoint process exited before caller recovery"
        if ($ExpectedJob) {
            $preRecoveryEvidence = Get-DynamoIsolatedProcessEvidence -Process $recovery
            Assert-Contract ($preRecoveryEvidence.IsProcessInJob) "$FailurePoint process is absent from its recovery Job"
            Assert-Contract ($preRecoveryEvidence.ActiveProcessIds -contains [ulong]$recovery.ProcessId) "$FailurePoint PID is absent from recovery Job evidence"
        }
        else {
            $noJobError = $null
            try { $null = Get-DynamoIsolatedProcessEvidence -Process $recovery }
            catch { $noJobError = $_ }
            Assert-Contract ($null -ne $noJobError) "$FailurePoint process-only recovery unexpectedly reported Job evidence"
        }

        Stop-DynamoIsolatedProcess -Process $recovery
        $recoveryWait = Wait-DynamoIsolatedProcess -Process $recovery -TimeoutMilliseconds 10000
        Assert-Contract $recoveryWait.Exited "$FailurePoint caller Stop did not terminate the failed launch"
        if ($ExpectedJob) {
            $postStopEvidence = Get-DynamoIsolatedProcessEvidence -Process $recovery
            Assert-Contract ($postStopEvidence.ActiveProcessCount -eq 0 -and $postStopEvidence.ActiveProcessIds.Count -eq 0) "$FailurePoint Job did not converge to active-zero"
        }
        Assert-Contract ($externalProcess.WaitForExit(10000)) "$FailurePoint process survived caller recovery"
        Remove-DynamoIsolatedProcess -Process $recovery -TimeoutMilliseconds 10000
        $null = $script:recoveryLeases.Remove($recovery)
        Assert-Contract (-not $recovery.IsJobOpen) "$FailurePoint recovery Job handle remained open after Remove"
        $disposedError = $null
        try { $null = Wait-DynamoIsolatedProcess -Process $recovery -TimeoutMilliseconds 0 }
        catch { $disposedError = $_ }
        Assert-Contract ($null -ne $disposedError) "$FailurePoint recovery process handle remained usable after Remove"

        foreach ($suffix in @('stdout.log', 'stderr.log')) {
            $outputPath = Join-Path $script:artifactRoot "$LeafPrefix.$suffix"
            $outputEvidence = [DynamoIsolatedContractNativeV3]::CapturePath($outputPath, $false)
            Assert-Contract (([IO.FileAttributes]$outputEvidence.Attributes -band ([IO.FileAttributes]::Directory -bor [IO.FileAttributes]::ReparsePoint)) -eq 0) "$FailurePoint output cleanup readback is not a regular file"
        }
    }
    finally {
        $externalProcess.Dispose()
    }
}

function Assert-NativeIdentity {
    param(
        [Parameter(Mandatory)][object]$Actual,
        [Parameter(Mandatory)][object]$Expected,
        [Parameter(Mandatory)][string]$Label
    )

    Assert-Contract (
        $Actual.FinalPath -ceq $Expected.FinalPath -and
        $Actual.Identity -ceq $Expected.Identity
    ) "$Label native identity changed"
}

function Remove-PinnedFlatRoot {
    param(
        [Parameter(Mandatory)][object]$RootLease,
        [string[]]$AllowedReparseLeaves = @()
    )

    $rootEvidence = $RootLease.Verify()
    Assert-Contract (([IO.FileAttributes]$rootEvidence.Attributes -band [IO.FileAttributes]::Directory) -ne 0) 'pinned cleanup root is not a directory'
    Assert-Contract (([IO.FileAttributes]$rootEvidence.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0) 'pinned cleanup root is a reparse point'

    $allowed = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($leaf in $AllowedReparseLeaves) {
        $fullLeaf = [IO.Path]::GetFullPath($leaf)
        Assert-Contract (([IO.Path]::GetDirectoryName($fullLeaf)) -ceq $RootLease.Path) 'approved reparse leaf escaped its pinned cleanup root'
        $null = $allowed.Add($fullLeaf)
    }

    $emptied = $false
    for ($pass = 0; $pass -lt 8; $pass++) {
        $items = @(Get-ChildItem -LiteralPath $RootLease.Path -Force -ErrorAction Stop)
        if ($items.Count -eq 0) {
            $emptied = $true
            break
        }
        foreach ($item in $items) {
            $leafPath = [IO.Path]::GetFullPath($item.FullName)
            Assert-Contract (([IO.Path]::GetDirectoryName($leafPath)) -ceq $RootLease.Path) 'cleanup enumeration escaped its pinned root'
            $leafLease = $null
            try {
                $leafLease = [DynamoIsolatedContractNativeV3]::PinPath($leafPath)
                $leafEvidence = $leafLease.Verify()
                Assert-Contract (([IO.Path]::GetDirectoryName($leafEvidence.FinalPath)) -ceq $rootEvidence.FinalPath) 'pinned cleanup leaf resolved outside its pinned parent'
                $isDirectory = (([IO.FileAttributes]$leafEvidence.Attributes -band [IO.FileAttributes]::Directory) -ne 0)
                $isReparse = (([IO.FileAttributes]$leafEvidence.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0)
                if ($isDirectory -and -not $isReparse) {
                    throw 'isolated-process-job-contract: flat cleanup encountered an unexpected real directory; root preserved'
                }
                if ($isReparse -and -not $allowed.Contains($leafPath)) {
                    throw 'isolated-process-job-contract: flat cleanup encountered an unapproved reparse point; root preserved'
                }
                if (-not $isReparse -and $allowed.Contains($leafPath)) {
                    throw 'isolated-process-job-contract: approved reparse leaf was replaced; root preserved'
                }
                $leafLease.DeletePinned()
            }
            finally {
                if ($null -ne $leafLease) { $leafLease.Dispose() }
            }
        }
    }
    Assert-Contract $emptied 'pinned cleanup root did not become empty within its bounded pass count'
    $null = $RootLease.Verify()
    $RootLease.DeletePinned()
}

if (-not $IsWindows) {
    Write-Output '{"contract":"isolated-process-job","status":"skip","reason":"windows-only"}'
    exit 0
}
if ($PSVersionTable.PSVersion -lt [version]'7.4') {
    throw 'isolated-process-job-contract: PowerShell 7.4 or newer is required'
}

if (-not $NativeFreshChild) {
    if ([string]::IsNullOrWhiteSpace($PSCommandPath)) {
        throw 'isolated-process-job-contract: the contract must be launched from its script path'
    }
    $freshStart = [Diagnostics.ProcessStartInfo]::new()
    $freshStart.FileName = [IO.Path]::GetFullPath((Get-Command pwsh -ErrorAction Stop).Source)
    $freshStart.UseShellExecute = $false
    $freshStart.CreateNoWindow = $true
    $freshStart.RedirectStandardOutput = $true
    $freshStart.RedirectStandardError = $true
    foreach ($argument in @('-NoProfile', '-NonInteractive', '-File', [IO.Path]::GetFullPath($PSCommandPath), '-NativeFreshChild')) {
        $freshStart.ArgumentList.Add($argument)
    }
    $freshProcess = [Diagnostics.Process]::Start($freshStart)
    $freshStdout = $freshProcess.StandardOutput.ReadToEndAsync()
    $freshStderr = $freshProcess.StandardError.ReadToEndAsync()
    try {
        if (-not $freshProcess.WaitForExit(120000)) {
            $freshProcess.Kill($true)
            $freshProcess.WaitForExit()
            throw 'isolated-process-job-contract: fresh child exceeded its 120-second deadline'
        }
        $stdoutText = $freshStdout.GetAwaiter().GetResult().Trim()
        $stderrText = $freshStderr.GetAwaiter().GetResult().Trim()
        Assert-Contract ("$stdoutText`n$stderrText" -notmatch 'contract-secret-not-for-output') 'fresh child leaked sensitive input'
        if ($freshProcess.ExitCode -ne 0) {
            throw "isolated-process-job-contract: fresh child failed ($($freshProcess.ExitCode)): $stderrText"
        }
        Assert-Contract ([string]::IsNullOrWhiteSpace($stderrText)) 'fresh child emitted unexpected standard error'
        $freshResult = $stdoutText | ConvertFrom-Json -ErrorAction Stop
        Assert-Contract ($freshResult.contract -ceq 'isolated-process-job' -and $freshResult.status -ceq 'pass') 'fresh child did not return the required pass record'
        Write-Output $stdoutText
        exit 0
    }
    finally {
        $freshProcess.Dispose()
    }
}

$repository = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$modulePath = Join-Path $repository 'scripts/perf/isolated-process-job.psm1'
$isolationModule = Import-Module -Force -PassThru $modulePath
Assert-Contract ([Dynamo.Perf.Isolation.NativeLauncher]::SourceVersion -ceq 'dynamo-isolated-job-native-2026-07-13-v5') 'native source version marker does not match the contract'
$exportedFunctions = @($isolationModule.ExportedFunctions.Keys | Sort-Object)
$expectedFunctions = @(
    'Get-DynamoIsolatedProcessEvidence'
    'Remove-DynamoIsolatedProcess'
    'Start-DynamoIsolatedProcess'
    'Stop-DynamoIsolatedProcess'
    'Wait-DynamoIsolatedProcess'
)
Assert-Contract (($exportedFunctions -join "`n") -ceq ($expectedFunctions -join "`n")) 'module exported an unexpected lifecycle ABI'
$isolatedProcessType = 'Dynamo.Perf.Isolation.IsolatedJobProcess' -as [type]
Assert-Contract ($null -eq $isolatedProcessType.GetMethod('CloseJob')) 'process handle retained the explicit CloseJob API'
Assert-Contract ($null -eq $isolatedProcessType.GetMethod('MarkClosedJobTerminationVerified')) 'process handle retained closed-Job snapshot mutation'
Assert-Contract ($null -eq $isolatedProcessType.GetProperty('IsTerminationVerified')) 'process handle retained snapshot termination state'
$contractFailureField = ('Dynamo.Perf.Isolation.NativeLauncher' -as [type]).GetField(
    'contractFailurePoint',
    [Reflection.BindingFlags]::NonPublic -bor [Reflection.BindingFlags]::Static)
Assert-Contract ($null -ne $contractFailureField -and -not $contractFailureField.IsPublic) 'contract failure injection is not private'

if ($null -eq ('DynamoIsolatedContractNativeV3' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;

public sealed class DynamoContractPathEvidence
{
    public string FinalPath { get; internal set; }
    public string Identity { get; internal set; }
    public uint Attributes { get; internal set; }
}

public sealed class DynamoContractPinnedPath : IDisposable
{
    SafeFileHandle handle;

    internal DynamoContractPinnedPath(string path, SafeFileHandle ownedHandle,
        DynamoContractPathEvidence evidence)
    {
        Path = path;
        handle = ownedHandle;
        FinalPath = evidence.FinalPath;
        Identity = evidence.Identity;
    }

    public string Path { get; private set; }
    public string FinalPath { get; private set; }
    public string Identity { get; private set; }

    public byte[] ReadAllBytes()
    {
        Verify();
        return DynamoIsolatedContractNativeV3.ReadPinnedBytes(handle);
    }

    public DynamoContractPathEvidence Verify()
    {
        if (handle == null || handle.IsClosed || handle.IsInvalid)
            throw new ObjectDisposedException("DynamoContractPinnedPath");
        var evidence = DynamoIsolatedContractNativeV3.ReadEvidence(handle);
        if (!String.Equals(evidence.FinalPath, FinalPath, StringComparison.Ordinal) ||
            !String.Equals(evidence.Identity, Identity, StringComparison.Ordinal))
            throw new InvalidOperationException("Pinned cleanup identity changed.");
        return evidence;
    }

    public void DeletePinned()
    {
        Verify();
        DynamoIsolatedContractNativeV3.MarkDelete(handle);
        handle.Dispose();
        handle = null;
        if (File.Exists(Path) || Directory.Exists(Path))
            throw new IOException("A path still occupies the deleted pinned leaf.");
    }

    public void Dispose()
    {
        if (handle != null)
        {
            handle.Dispose();
            handle = null;
        }
    }
}

public static class DynamoIsolatedContractNativeV3
{
    const uint GENERIC_READ = 0x80000000;
    const uint DELETE = 0x00010000;
    const uint FILE_READ_ATTRIBUTES = 0x80;
    const uint FILE_SHARE_READ = 1, FILE_SHARE_WRITE = 2;
    const uint OPEN_EXISTING = 3;
    const uint FILE_FLAG_BACKUP_SEMANTICS = 0x02000000;
    const uint FILE_FLAG_OPEN_REPARSE_POINT = 0x00200000;
    const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;
    const uint SYNCHRONIZE = 0x00100000;
    const int FILE_DISPOSITION_INFO_EX_CLASS = 21;
    const uint FILE_DISPOSITION_FLAG_DELETE = 0x1;
    const uint FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE = 0x10;

    [StructLayout(LayoutKind.Sequential)]
    struct FILE_ATTRIBUTE_TAG_INFO { public uint FileAttributes; public uint ReparseTag; }

    [StructLayout(LayoutKind.Sequential)]
    struct FILE_ID_INFO {
        public ulong VolumeSerialNumber;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 16)] public byte[] FileId;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct FILETIME { public uint Low; public uint High; }

    [StructLayout(LayoutKind.Sequential)]
    struct FILE_DISPOSITION_INFO_EX { public uint Flags; }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern SafeFileHandle CreateFileW(string name, uint access, uint share, IntPtr security,
        uint creation, uint flags, IntPtr template);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern uint GetFinalPathNameByHandleW(SafeFileHandle handle, StringBuilder path, uint length, uint flags);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetFileInformationByHandleEx(SafeFileHandle handle, int informationClass,
        out FILE_ATTRIBUTE_TAG_INFO information, uint length);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetFileInformationByHandleEx(SafeFileHandle handle, int informationClass,
        out FILE_ID_INFO information, uint length);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern SafeWaitHandle OpenProcess(uint access, bool inherit, uint processId);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetProcessTimes(SafeWaitHandle process, out FILETIME creation,
        out FILETIME exit, out FILETIME kernel, out FILETIME user);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetFileInformationByHandle(SafeFileHandle handle, int informationClass,
        ref FILE_DISPOSITION_INFO_EX information, uint length);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetFileSizeEx(SafeFileHandle handle, out long size);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool ReadFile(SafeFileHandle handle, [Out] byte[] buffer, uint bytesToRead,
        out uint bytesRead, IntPtr overlapped);

    public static DynamoContractPinnedPath PinPath(string path)
    {
        string fullPath = Path.GetFullPath(path);
        var handle = CreateFileW(fullPath, GENERIC_READ | DELETE | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ, IntPtr.Zero, OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS, IntPtr.Zero);
        if (handle.IsInvalid)
        {
            int error = Marshal.GetLastWin32Error();
            handle.Dispose();
            throw new Win32Exception(error, "contract pinned path open failed");
        }
        try
        {
            return new DynamoContractPinnedPath(fullPath, handle, ReadEvidence(handle));
        }
        catch
        {
            handle.Dispose();
            throw;
        }
    }

    public static DynamoContractPathEvidence CapturePath(string path, bool directory)
    {
        uint flags = FILE_FLAG_OPEN_REPARSE_POINT | (directory ? FILE_FLAG_BACKUP_SEMANTICS : 0);
        using (var handle = CreateFileW(path, FILE_READ_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE,
            IntPtr.Zero, OPEN_EXISTING, flags, IntPtr.Zero)) {
            if (handle.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error(), "contract path open failed");
            return ReadEvidence(handle);
        }
    }

    internal static DynamoContractPathEvidence ReadEvidence(SafeFileHandle handle)
    {
        FILE_ATTRIBUTE_TAG_INFO attributes;
        if (!GetFileInformationByHandleEx(handle, 9, out attributes,
            (uint)Marshal.SizeOf(typeof(FILE_ATTRIBUTE_TAG_INFO))))
            throw new Win32Exception(Marshal.GetLastWin32Error(), "contract path attributes failed");
        FILE_ID_INFO identity;
        if (!GetFileInformationByHandleEx(handle, 18, out identity,
            (uint)Marshal.SizeOf(typeof(FILE_ID_INFO))))
            throw new Win32Exception(Marshal.GetLastWin32Error(), "contract path identity failed");
        var pathBuffer = new StringBuilder(32768);
        uint pathLength = GetFinalPathNameByHandleW(handle, pathBuffer, (uint)pathBuffer.Capacity, 0);
        if (pathLength == 0 || pathLength >= pathBuffer.Capacity)
            throw new Win32Exception(Marshal.GetLastWin32Error(), "contract final path failed");
        return new DynamoContractPathEvidence {
            FinalPath = pathBuffer.ToString(),
            Identity = identity.VolumeSerialNumber.ToString("x16") + ":" +
                BitConverter.ToString(identity.FileId).Replace("-", "").ToLowerInvariant(),
            Attributes = attributes.FileAttributes
        };
    }

    internal static void MarkDelete(SafeFileHandle handle)
    {
        var disposition = new FILE_DISPOSITION_INFO_EX {
            Flags = FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE
        };
        if (!SetFileInformationByHandle(handle, FILE_DISPOSITION_INFO_EX_CLASS, ref disposition,
            (uint)Marshal.SizeOf(typeof(FILE_DISPOSITION_INFO_EX))))
            throw new Win32Exception(Marshal.GetLastWin32Error(), "contract pinned deletion failed");
    }

    internal static byte[] ReadPinnedBytes(SafeFileHandle handle)
    {
        long size;
        if (!GetFileSizeEx(handle, out size))
            throw new Win32Exception(Marshal.GetLastWin32Error(), "contract pinned size query failed");
        if (size < 0 || size > 1024 * 1024)
            throw new IOException("Pinned contract marker exceeded its size limit.");
        var result = new byte[(int)size];
        int offset = 0;
        while (offset < result.Length)
        {
            int count = Math.Min(65536, result.Length - offset);
            var chunk = new byte[count];
            uint read;
            if (!ReadFile(handle, chunk, (uint)count, out read, IntPtr.Zero))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "contract pinned read failed");
            if (read == 0)
                throw new EndOfStreamException("Pinned contract marker ended before its reported size.");
            Buffer.BlockCopy(chunk, 0, result, offset, (int)read);
            offset += (int)read;
        }
        return result;
    }

    public static ulong GetProcessCreationFileTime(uint processId)
    {
        using (var process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, false, processId)) {
            if (process.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error(), "contract process open failed");
            FILETIME creation, exit, kernel, user;
            if (!GetProcessTimes(process, out creation, out exit, out kernel, out user))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "contract process birth query failed");
            return ((ulong)creation.High << 32) | creation.Low;
        }
    }
}
'@
}

$pwsh = [IO.Path]::GetFullPath((Get-Command pwsh -ErrorAction Stop).Source)
$contractId = "$PID-$([guid]::NewGuid().ToString('N'))"
$suiteRoot = Join-Path ([IO.Path]::GetTempPath()) "dynamo-isolated-job-contract-$contractId"
$artifactRoot = Join-Path $suiteRoot 'artifacts'
$executableSentinelRoot = Join-Path ([IO.Path]::GetTempPath()) "dynamo-isolated-job-contract-executable-sentinel-$contractId"
$outputSentinelRoot = Join-Path ([IO.Path]::GetTempPath()) "dynamo-isolated-job-contract-output-sentinel-$contractId"
$ownedMarker = Join-Path $suiteRoot '.dynamo-isolated-job-contract-owned'
$executableSentinelMarker = Join-Path $executableSentinelRoot '.dynamo-isolated-job-contract-owned'
$executableSentinelLeaf = Join-Path $executableSentinelRoot 'sentinel.exe'
$outputSentinelMarker = Join-Path $outputSentinelRoot '.dynamo-isolated-job-contract-owned'
$children = [System.Collections.Generic.List[object]]::new()
$recoveryLeases = [System.Collections.Generic.List[object]]::new()
$processBirths = [System.Collections.Generic.Dictionary[int,ulong]]::new()
$junctionLeaves = [System.Collections.Generic.List[string]]::new()
$testError = $null
$cleanupErrors = [System.Collections.Generic.List[Exception]]::new()
$suiteRootLease = $null
$executableSentinelLease = $null
$outputSentinelLease = $null
$artifactBaseline = $null
$suiteRootEvidence = $null
$markerEvidence = $null
$executableSentinelMarkerEvidence = $null
$outputSentinelMarkerEvidence = $null
$markerNonce = ([guid]::NewGuid().ToString('N') + [guid]::NewGuid().ToString('N'))
$executableSentinelNonce = ([guid]::NewGuid().ToString('N') + [guid]::NewGuid().ToString('N'))
$outputSentinelNonce = ([guid]::NewGuid().ToString('N') + [guid]::NewGuid().ToString('N'))
$markerHash = $null
$executableSentinelHash = $null
$outputSentinelHash = $null
$ambientSentinelName = 'DYNAMO_AMBIENT_SENTINEL'
$ambientSentinelBefore = [Environment]::GetEnvironmentVariable($ambientSentinelName, 'Process')
[Environment]::SetEnvironmentVariable($ambientSentinelName, 'contract-secret-not-for-output', 'Process')

try {
    $null = New-Item -ItemType Directory -Path $suiteRoot
    $null = New-Item -ItemType Directory -Path $artifactRoot
    [IO.File]::WriteAllText($ownedMarker, $markerNonce, [Text.UTF8Encoding]::new($false))
    $null = New-Item -ItemType Directory -Path $executableSentinelRoot
    [IO.File]::WriteAllText($executableSentinelMarker, $executableSentinelNonce, [Text.UTF8Encoding]::new($false))
    [IO.File]::WriteAllBytes($executableSentinelLeaf, [byte[]](0x4d, 0x5a, 0x00, 0x00))
    $null = New-Item -ItemType Directory -Path $outputSentinelRoot
    [IO.File]::WriteAllText($outputSentinelMarker, $outputSentinelNonce, [Text.UTF8Encoding]::new($false))

    $suiteRootLease = [DynamoIsolatedContractNativeV3]::PinPath($suiteRoot)
    $executableSentinelLease = [DynamoIsolatedContractNativeV3]::PinPath($executableSentinelRoot)
    $outputSentinelLease = [DynamoIsolatedContractNativeV3]::PinPath($outputSentinelRoot)
    $suiteRootEvidence = $suiteRootLease.Verify()
    $artifactBaseline = [DynamoIsolatedContractNativeV3]::CapturePath($artifactRoot, $true)
    $markerEvidence = [DynamoIsolatedContractNativeV3]::CapturePath($ownedMarker, $false)
    $executableSentinelMarkerEvidence = [DynamoIsolatedContractNativeV3]::CapturePath($executableSentinelMarker, $false)
    $outputSentinelMarkerEvidence = [DynamoIsolatedContractNativeV3]::CapturePath($outputSentinelMarker, $false)
    Assert-Contract (([IO.FileAttributes]$suiteRootEvidence.Attributes -band [IO.FileAttributes]::Directory) -ne 0) 'suite root native evidence is not a directory'
    Assert-Contract (([IO.FileAttributes]$suiteRootEvidence.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0) 'suite root native evidence is a reparse point'
    Assert-Contract (([IO.FileAttributes]$markerEvidence.Attributes -band ([IO.FileAttributes]::Directory -bor [IO.FileAttributes]::ReparsePoint)) -eq 0) 'ownership marker is not a regular file'
    $markerHash = (Get-FileHash -LiteralPath $ownedMarker -Algorithm SHA256).Hash.ToLowerInvariant()
    $executableSentinelHash = (Get-FileHash -LiteralPath $executableSentinelMarker -Algorithm SHA256).Hash.ToLowerInvariant()
    $outputSentinelHash = (Get-FileHash -LiteralPath $outputSentinelMarker -Algorithm SHA256).Hash.ToLowerInvariant()
    $environment = New-ChildEnvironment -TemporaryDirectory $artifactRoot -Additional @{
        DYNAMO_CONTRACT_VALUE = 'readback-ok'
    }

    $staleTypeScript = 'Add-Type ''namespace Dynamo.Perf.Isolation { public static class NativeLauncher { public const string SourceVersion = "stale-native-source"; } }'';try{Import-Module -Force $args[0] -ErrorAction Stop}catch{if($_.Exception.Message -match "stale native type"){exit 0};exit 91};exit 92'
    $staleType = Start-ContractProcess `
        -Executable $pwsh `
        -Arguments @('-NoProfile', '-NonInteractive', '-CommandWithArgs', $staleTypeScript, $modulePath) `
        -Environment (New-ChildEnvironment -TemporaryDirectory $artifactRoot) `
        -WorkingDirectory $repository `
        -OutputDirectory $artifactRoot `
        -LeafPrefix 'stale-native-type'
    Register-ContractProcess -Process $staleType
    $staleTypeWait = Wait-DynamoIsolatedProcess -Process $staleType -TimeoutMilliseconds 15000
    Assert-Contract ($staleTypeWait.Exited -and $staleTypeWait.ExitCode -eq 0) 'stale native type marker was not rejected in a fresh child'
    Complete-ContractProcess -Process $staleType -AlreadyExited

    # The child proves that the supplied environment is the exact child-local block,
    # and that both inherited output handles remain usable after suspended assignment.
    $readback = Start-ContractProcess `
        -Executable $pwsh `
        -Arguments @(
            '-NoProfile', '-NonInteractive', '-Command',
            'if($null -ne [Environment]::GetEnvironmentVariable("DYNAMO_AMBIENT_SENTINEL")){exit 97}; [Console]::Out.Write($env:DYNAMO_CONTRACT_VALUE); [Console]::Error.Write("stderr-ok"); [Threading.Thread]::Sleep(250)'
        ) `
        -Environment $environment `
        -WorkingDirectory $repository `
        -OutputDirectory $artifactRoot `
        -LeafPrefix 'readback'
    Register-ContractProcess -Process $readback
    $readbackEvidence = Get-DynamoIsolatedProcessEvidence -Process $readback
    Assert-Contract $readbackEvidence.IsProcessInJob 'readback child is not a member of its Job Object'
    Assert-Contract ($readbackEvidence.CreationFileTimeUtc -gt 0) 'readback child has no creation-time identity'
    Assert-Contract ($readbackEvidence.ActiveProcessIds -contains [ulong]$readback.ProcessId) 'readback child PID is absent from job evidence'
    $readbackWait = Wait-DynamoIsolatedProcess -Process $readback -TimeoutMilliseconds 15000
    Assert-Contract $readbackWait.Exited 'readback child did not exit before its deadline'
    Assert-Contract ($readbackWait.ExitCode -eq 0) 'readback child returned a nonzero exit code'
    Assert-Contract (([IO.File]::ReadAllText((Join-Path $artifactRoot 'readback.stdout.log'))) -ceq 'readback-ok') 'standard-output readback did not match'
    Assert-Contract (([IO.File]::ReadAllText((Join-Path $artifactRoot 'readback.stderr.log'))) -ceq 'stderr-ok') 'standard-error readback did not match'
    Complete-ContractProcess -Process $readback -AlreadyExited

    # These values exercise the documented CommandLineToArgvW-compatible quoting
    # cases: spaces, embedded quotes, quoted trailing backslashes,
    # backslashes immediately before a quote, Unicode, and an empty argument.
    $quotingScript = '[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false);[Console]::Out.Write((ConvertTo-Json -Compress -InputObject ([string[]]$args)))'
    $quoting = Start-ContractProcess `
        -Executable $pwsh `
        -Arguments @(
            '-NoProfile', '-NonInteractive', '-CommandWithArgs', $quotingScript,
            'space value', 'quote"inside', 'quoted trailing\', 'slashes\\\"inside', '한글-✓-Δ', ''
        ) `
        -Environment (New-ChildEnvironment -TemporaryDirectory $artifactRoot) `
        -WorkingDirectory $repository `
        -OutputDirectory $artifactRoot `
        -LeafPrefix 'quoting'
    Register-ContractProcess -Process $quoting
    $quotingWait = Wait-DynamoIsolatedProcess -Process $quoting -TimeoutMilliseconds 15000
    Assert-Contract ($quotingWait.Exited -and $quotingWait.ExitCode -eq 0) 'quoting child failed'
    $quotedArguments = @(([IO.File]::ReadAllText((Join-Path $artifactRoot 'quoting.stdout.log'))) | ConvertFrom-Json -ErrorAction Stop)
    Assert-Contract ($quotedArguments.Count -eq 6) 'quoting child received the wrong argument count'
    Assert-Contract ($quotedArguments[0] -ceq 'space value') 'space-bearing argument changed'
    Assert-Contract ($quotedArguments[1] -ceq 'quote"inside') 'quote-bearing argument changed'
    Assert-Contract ($quotedArguments[2] -ceq 'quoted trailing\') 'quoted trailing-backslash argument changed'
    Assert-Contract ($quotedArguments[3] -ceq 'slashes\\\"inside') 'backslashes-before-quote argument changed'
    Assert-Contract ($quotedArguments[4] -ceq '한글-✓-Δ') 'Unicode argument changed'
    Assert-Contract ($quotedArguments[5] -ceq '') 'empty argument changed'
    Complete-ContractProcess -Process $quoting -AlreadyExited

    # A post-output, pre-process failure must preserve the exact CREATE_NEW leaves.
    # Cleanup may never close those handles and then delete whatever later appears at
    # the same path.
    $invalidExecutable = Join-Path $artifactRoot 'invalid-executable.exe'
    [IO.File]::WriteAllText($invalidExecutable, 'not-a-windows-image', [Text.UTF8Encoding]::new($false))
    $failedLaunch = $null
    try {
        $null = Start-ContractProcess `
            -Executable $invalidExecutable `
            -Arguments @() `
            -Environment (New-ChildEnvironment -TemporaryDirectory $artifactRoot) `
            -WorkingDirectory $repository `
            -OutputDirectory $artifactRoot `
            -LeafPrefix 'failed-launch'
    }
    catch {
        $failedLaunch = $_
    }
    Assert-Contract ($null -ne $failedLaunch) 'invalid executable unexpectedly launched'
    Assert-Contract ($failedLaunch.Exception -isnot [Dynamo.Perf.Isolation.IsolationInputException]) 'invalid executable failed before the post-output creation phase'
    foreach ($failedLeaf in @('failed-launch.stdout.log', 'failed-launch.stderr.log')) {
        $failedPath = Join-Path $artifactRoot $failedLeaf
        Assert-Contract (Test-Path -LiteralPath $failedPath -PathType Leaf) 'failed-launch output was deleted by path during cleanup'
        $failedEvidence = [DynamoIsolatedContractNativeV3]::CapturePath($failedPath, $false)
        Assert-Contract (([IO.FileAttributes]$failedEvidence.Attributes -band ([IO.FileAttributes]::Directory -bor [IO.FileAttributes]::ReparsePoint)) -eq 0) 'failed-launch output is not a preserved regular file'
    }

    # Every ownership window after CreateProcess is fault-injected. Cleanup is
    # deliberately reported as unconfirmed so the exception must transfer the
    # exact live process handle (and Job handle when assigned) to caller recovery.
    Invoke-FailedLaunchRecoveryCase -FailurePoint 'RawPostCreate:Recover' -LeafPrefix 'recover-raw-post-create' -ExpectedJob $false
    Invoke-FailedLaunchRecoveryCase -FailurePoint 'PreAssign:Recover' -LeafPrefix 'recover-pre-assign' -ExpectedJob $false
    Invoke-FailedLaunchRecoveryCase -FailurePoint 'PostAssignPreResume:Recover' -LeafPrefix 'recover-post-assign' -ExpectedJob $true
    Invoke-FailedLaunchRecoveryCase -FailurePoint 'PostResume:Recover' -LeafPrefix 'recover-post-resume' -ExpectedJob $true

    # The parent creates one explicit descendant. Remove must retain the same open Job
    # handle while TerminateJobObject converges to active-zero before disposing it.
    $descendantScript = '$psi=[Diagnostics.ProcessStartInfo]::new();$psi.FileName=$env:DYNAMO_CONTRACT_PWSH;$psi.UseShellExecute=$false;$psi.CreateNoWindow=$true;$psi.ArgumentList.Add("-NoProfile");$psi.ArgumentList.Add("-NonInteractive");$psi.ArgumentList.Add("-Command");$psi.ArgumentList.Add("Start-Sleep -Seconds 60");$child=[Diagnostics.Process]::Start($psi);[Console]::Out.Write($child.Id);[Console]::Out.Flush();Start-Sleep -Seconds 60'
    $tree = Start-ContractProcess `
        -Executable $pwsh `
        -Arguments @('-NoProfile', '-NonInteractive', '-Command', $descendantScript) `
        -Environment (New-ChildEnvironment -TemporaryDirectory $artifactRoot -Additional @{ DYNAMO_CONTRACT_PWSH = $pwsh }) `
        -WorkingDirectory $repository `
        -OutputDirectory $artifactRoot `
        -LeafPrefix 'tree'
    Register-ContractProcess -Process $tree

    $descendantPid = 0
    $treeOutput = Join-Path $artifactRoot 'tree.stdout.log'
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while ([DateTime]::UtcNow -lt $deadline -and $descendantPid -eq 0) {
        try {
            $candidate = Read-SharedText -Path $treeOutput
            if ($candidate -cmatch '^[1-9][0-9]*$') {
                $descendantPid = [int]$candidate
                break
            }
        }
        catch { }
        Start-Sleep -Milliseconds 50
    }
    Assert-Contract ($descendantPid -gt 0) 'descendant PID was not published before the deadline'

    $treeEvidence = Get-DynamoIsolatedProcessEvidence -Process $tree
    Assert-Contract $treeEvidence.IsProcessInJob 'tree parent is not a member of its Job Object'
    Assert-Contract ($treeEvidence.ActiveProcessCount -ge 2) 'Job Object did not report the parent and descendant as active'
    Assert-Contract ($treeEvidence.ActiveProcessIds -contains [ulong]$tree.ProcessId) 'tree parent PID is absent from job evidence'
    Assert-Contract ($treeEvidence.ActiveProcessIds -contains [ulong]$descendantPid) 'descendant PID is absent from job evidence'
    $descendant = [Diagnostics.Process]::GetProcessById($descendantPid)
    try {
        $null = $descendant.Handle
        $descendantBirth = [DynamoIsolatedContractNativeV3]::GetProcessCreationFileTime([uint32]$descendantPid)
        Assert-Contract ($descendantBirth -gt 0) 'descendant has no independent creation identity'
        Assert-Contract (-not $descendant.HasExited) 'descendant exited before Stop+Remove was exercised'
        Remove-DynamoIsolatedProcess -Process $tree -TimeoutMilliseconds 10000
        Assert-Contract ($descendant.WaitForExit(10000)) 'descendant survived verified Stop+Remove'
    }
    finally {
        $descendant.Dispose()
    }
    $null = $children.Remove($tree)
    $null = $processBirths.Remove($tree.ProcessId)

    # KILL_ON_JOB_CLOSE is tested only as an operating-system host-crash backstop.
    # The nested host exits via Environment.Exit without invoking Remove/Dispose;
    # the inner child must die while the outer verification Job remains open.
    $backstopReady = Join-Path $artifactRoot 'backstop.ready.json'
    $backstopRelease = Join-Path $artifactRoot 'backstop.release'
    $backstopHostScript = @(
        '$ErrorActionPreference=''Stop'''
        'Import-Module -Force $args[0]'
        '$childEnvironment=[ordered]@{SystemRoot=[Environment]::GetEnvironmentVariable(''SystemRoot'',''Process'');WINDIR=[Environment]::GetEnvironmentVariable(''WINDIR'',''Process'');TEMP=$args[3];TMP=$args[3]}'
        '$inner=Start-DynamoIsolatedProcess -ExecutablePath $args[1] -ArgumentList @(''-NoProfile'',''-NonInteractive'',''-Command'',''Start-Sleep -Seconds 60'') -WorkingDirectory $args[2] -Environment $childEnvironment -StandardOutputPath (Join-Path $args[3] ''backstop-inner.stdout.log'') -StandardErrorPath (Join-Path $args[3] ''backstop-inner.stderr.log'')'
        '$payload=[ordered]@{processId=$inner.ProcessId;creationFileTimeUtc=$inner.CreationFileTimeUtc}|ConvertTo-Json -Compress'
        '[IO.File]::WriteAllText($args[4],$payload,[Text.UTF8Encoding]::new($false))'
        '$deadline=[DateTime]::UtcNow.AddSeconds(20)'
        'while(-not (Test-Path -LiteralPath $args[5])){if([DateTime]::UtcNow -ge $deadline){[Environment]::Exit(92)};Start-Sleep -Milliseconds 25}'
        '[Environment]::Exit(23)'
    ) -join ';'
    $backstopHost = Start-ContractProcess `
        -Executable $pwsh `
        -Arguments @('-NoProfile', '-NonInteractive', '-CommandWithArgs', $backstopHostScript, $modulePath, $pwsh, $repository, $artifactRoot, $backstopReady, $backstopRelease) `
        -Environment (New-ChildEnvironment -TemporaryDirectory $artifactRoot) `
        -WorkingDirectory $repository `
        -OutputDirectory $artifactRoot `
        -LeafPrefix 'backstop-host'
    Register-ContractProcess -Process $backstopHost

    $backstopPayload = $null
    $backstopDeadline = [DateTime]::UtcNow.AddSeconds(20)
    while ([DateTime]::UtcNow -lt $backstopDeadline -and $null -eq $backstopPayload) {
        try {
            $backstopPayload = [IO.File]::ReadAllText($backstopReady) | ConvertFrom-Json -ErrorAction Stop
        }
        catch {
            $backstopPayload = $null
            Start-Sleep -Milliseconds 25
        }
    }
    Assert-Contract ($null -ne $backstopPayload -and [int]$backstopPayload.processId -gt 0) 'backstop host did not publish its inner child identity'
    $backstopChild = [Diagnostics.Process]::GetProcessById([int]$backstopPayload.processId)
    try {
        $null = $backstopChild.Handle
        $backstopBirth = [DynamoIsolatedContractNativeV3]::GetProcessCreationFileTime([uint32]$backstopPayload.processId)
        Assert-Contract ($backstopBirth -eq [uint64]$backstopPayload.creationFileTimeUtc) 'backstop inner child creation identity changed'
        Assert-Contract (-not $backstopChild.HasExited) 'backstop inner child exited before its host crash'
        [IO.File]::WriteAllText($backstopRelease, 'release', [Text.UTF8Encoding]::new($false))
        $backstopHostWait = Wait-DynamoIsolatedProcess -Process $backstopHost -TimeoutMilliseconds 10000
        Assert-Contract ($backstopHostWait.Exited -and $backstopHostWait.ExitCode -eq 23) 'backstop host did not take the abrupt-exit path'
        Assert-Contract ($backstopChild.WaitForExit(10000)) 'KILL_ON_JOB_CLOSE did not terminate the inner child after host exit'
    }
    finally {
        $backstopChild.Dispose()
    }
    Complete-ContractProcess -Process $backstopHost -AlreadyExited

    # Explicit TerminateJobObject remains available for timeout/error paths that want
    # deterministic intent before the final SafeHandle close.
    $terminating = Start-ContractProcess `
        -Executable $pwsh `
        -Arguments @('-NoProfile', '-NonInteractive', '-Command', 'Start-Sleep -Seconds 60') `
        -Environment (New-ChildEnvironment -TemporaryDirectory $artifactRoot) `
        -WorkingDirectory $repository `
        -OutputDirectory $artifactRoot `
        -LeafPrefix 'terminate'
    Register-ContractProcess -Process $terminating
    Stop-DynamoIsolatedProcess -Process $terminating
    $terminateWait = Wait-DynamoIsolatedProcess -Process $terminating -TimeoutMilliseconds 10000
    Assert-Contract $terminateWait.Exited 'TerminateJobObject did not stop the child'
    $terminatedEvidence = Get-DynamoIsolatedProcessEvidence -Process $terminating
    Assert-Contract ($terminatedEvidence.ActiveProcessCount -eq 0) 'terminated Job Object still reports active processes'
    Complete-ContractProcess -Process $terminating -AlreadyExited

    # A long wait must not hold the lifecycle lock and delay an urgent termination.
    $concurrent = Start-ContractProcess `
        -Executable $pwsh `
        -Arguments @('-NoProfile', '-NonInteractive', '-Command', 'Start-Sleep -Seconds 60') `
        -Environment (New-ChildEnvironment -TemporaryDirectory $artifactRoot) `
        -WorkingDirectory $repository `
        -OutputDirectory $artifactRoot `
        -LeafPrefix 'concurrent-wait'
    Register-ContractProcess -Process $concurrent
    $concurrentWait = $concurrent.WaitAsync(30000)
    Start-Sleep -Milliseconds 100
    $terminateClock = [Diagnostics.Stopwatch]::StartNew()
    Stop-DynamoIsolatedProcess -Process $concurrent
    $terminateClock.Stop()
    Assert-Contract ($terminateClock.ElapsedMilliseconds -lt 2000) 'concurrent Wait blocked urgent Job termination'
    $concurrentResult = $concurrentWait.GetAwaiter().GetResult()
    Assert-Contract $concurrentResult.Exited 'concurrent Wait did not observe Job termination'
    $concurrentEvidence = Get-DynamoIsolatedProcessEvidence -Process $concurrent
    Assert-Contract ($concurrentEvidence.ActiveProcessCount -eq 0) 'concurrent termination left active Job members'
    Complete-ContractProcess -Process $concurrent -AlreadyExited

    $negativeEnvironment = New-ChildEnvironment -TemporaryDirectory $artifactRoot -Additional @{
        DYNAMO_SECRET_FIXTURE = 'contract-secret-not-for-output'
    }
    Assert-Rejected -Case 'relative executable path' -ExpectedCode 'PathMustBeAbsolute' -Action {
        Start-ContractProcess -Executable 'pwsh.exe' -Arguments @('-Version') -Environment $negativeEnvironment -WorkingDirectory $repository -OutputDirectory $artifactRoot -LeafPrefix 'relative-executable'
    }
    Assert-Rejected -Case 'device namespace executable path' -ExpectedCode 'PathNamespaceUnsupported' -Action {
        Start-ContractProcess -Executable ("\\?\$pwsh") -Arguments @('-Version') -Environment $negativeEnvironment -WorkingDirectory $repository -OutputDirectory $artifactRoot -LeafPrefix 'device-executable'
    }
    Assert-Rejected -Case 'relative output path' -ExpectedCode 'PathMustBeAbsolute' -Action {
        Start-DynamoIsolatedProcess -ExecutablePath $pwsh -ArgumentList @('-Version') -WorkingDirectory $repository -Environment $negativeEnvironment -StandardOutputPath 'relative.stdout.log' -StandardErrorPath (Join-Path $artifactRoot 'relative.stderr.log')
    }

    $existingOutput = Join-Path $artifactRoot 'existing.stdout.log'
    [IO.File]::WriteAllText($existingOutput, 'owner-data', [Text.UTF8Encoding]::new($false))
    Assert-Rejected -Case 'existing output leaf' -ExpectedCode 'OutputMustBeAbsent' -Action {
        Start-DynamoIsolatedProcess -ExecutablePath $pwsh -ArgumentList @('-Version') -WorkingDirectory $repository -Environment $negativeEnvironment -StandardOutputPath $existingOutput -StandardErrorPath (Join-Path $artifactRoot 'existing.stderr.log')
    }
    Assert-Contract (([IO.File]::ReadAllText($existingOutput)) -ceq 'owner-data') 'existing output leaf was modified'

    Assert-Rejected -Case 'control character in argument' -ExpectedCode 'ArgumentControlCharacter' -Action {
        Start-ContractProcess -Executable $pwsh -Arguments @('-Command', "bad`nargument") -Environment $negativeEnvironment -WorkingDirectory $repository -OutputDirectory $artifactRoot -LeafPrefix 'argument-control'
    }
    Assert-Rejected -Case 'non-string argument' -ExpectedCode 'ArgumentType' -Action {
        Start-ContractProcess -Executable $pwsh -Arguments @('-Command', 7) -Environment $negativeEnvironment -WorkingDirectory $repository -OutputDirectory $artifactRoot -LeafPrefix 'argument-type'
    }
    Assert-Rejected -Case 'invalid environment key' -ExpectedCode 'EnvironmentKey' -Action {
        Start-ContractProcess -Executable $pwsh -Arguments @('-Version') -Environment ([ordered]@{ 'BAD=KEY' = 'contract-secret-not-for-output' }) -WorkingDirectory $repository -OutputDirectory $artifactRoot -LeafPrefix 'environment-key'
    }
    Assert-Rejected -Case 'non-string environment value' -ExpectedCode 'EnvironmentType' -Action {
        Start-ContractProcess -Executable $pwsh -Arguments @('-Version') -Environment ([ordered]@{ GOOD_KEY = 7 }) -WorkingDirectory $repository -OutputDirectory $artifactRoot -LeafPrefix 'environment-type'
    }
    Assert-Rejected -Case 'same output leaf' -ExpectedCode 'OutputPathsMustDiffer' -Action {
        $same = Join-Path $artifactRoot 'same-output.log'
        Start-DynamoIsolatedProcess -ExecutablePath $pwsh -ArgumentList @('-Version') -WorkingDirectory $repository -Environment $negativeEnvironment -StandardOutputPath $same -StandardErrorPath $same
    }

    $executableSentinelJunction = Join-Path $artifactRoot 'executable-sentinel-junction'
    $null = New-Item -ItemType Junction -Path $executableSentinelJunction -Target $executableSentinelRoot
    $junctionLeaves.Add($executableSentinelJunction)
    Assert-Rejected -Case 'reparse executable ancestor' -ExpectedCode 'PathFinalMismatch' -Action {
        Start-ContractProcess -Executable (Join-Path $executableSentinelJunction (Split-Path -Leaf $executableSentinelLeaf)) -Arguments @('-Version') -Environment $negativeEnvironment -WorkingDirectory $repository -OutputDirectory $artifactRoot -LeafPrefix 'reparse-executable'
    }

    $outputJunction = Join-Path $artifactRoot 'output-sentinel-junction'
    $null = New-Item -ItemType Junction -Path $outputJunction -Target $outputSentinelRoot
    $junctionLeaves.Add($outputJunction)
    Assert-Rejected -Case 'reparse output directory' -ExpectedCode 'PathReparse' -Action {
        Start-ContractProcess -Executable $pwsh -Arguments @('-Version') -Environment $negativeEnvironment -WorkingDirectory $repository -OutputDirectory $outputJunction -LeafPrefix 'reparse-output'
    }
}
catch {
    $testError = $_
}
finally {
    try {
        [Environment]::SetEnvironmentVariable($ambientSentinelName, $ambientSentinelBefore, 'Process')
    }
    catch {
        $cleanupErrors.Add($_.Exception)
    }
    foreach ($child in @($children)) {
        try {
            Complete-ContractProcess -Process $child
        }
        catch {
            $cleanupErrors.Add($_.Exception)
        }
    }
    foreach ($recovery in @($recoveryLeases)) {
        try {
            Remove-DynamoIsolatedProcess -Process $recovery -TimeoutMilliseconds 10000
            $null = $recoveryLeases.Remove($recovery)
        }
        catch {
            $cleanupErrors.Add($_.Exception)
        }
    }

    $cleanupCanDelete = ($cleanupErrors.Count -eq 0)
    $artifactLease = $null
    $markerLease = $null
    $executableSentinelMarkerLease = $null
    $outputSentinelMarkerLease = $null
    try {
        if ($cleanupCanDelete) {
            $requiredEvidence = @(
                $suiteRootLease,
                $executableSentinelLease,
                $outputSentinelLease,
                $artifactBaseline,
                $suiteRootEvidence,
                $markerEvidence,
                $executableSentinelMarkerEvidence,
                $outputSentinelMarkerEvidence,
                $markerHash,
                $executableSentinelHash,
                $outputSentinelHash
            )
            Assert-Contract (-not ($requiredEvidence -contains $null)) 'cleanup native ownership evidence is incomplete; roots preserved'

            $fullTempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
            foreach ($ownedRoot in @($suiteRoot, $executableSentinelRoot, $outputSentinelRoot)) {
                $fullOwnedRoot = [IO.Path]::GetFullPath($ownedRoot)
                Assert-Contract ($fullOwnedRoot.StartsWith($fullTempRoot, [StringComparison]::OrdinalIgnoreCase)) 'cleanup root escaped the OS temporary directory'
            }

            $currentSuiteEvidence = $suiteRootLease.Verify()
            Assert-NativeIdentity -Actual $currentSuiteEvidence -Expected $suiteRootEvidence -Label 'suite root'
            foreach ($sentinelRootLease in @($executableSentinelLease, $outputSentinelLease)) {
                $sentinelRootEvidence = $sentinelRootLease.Verify()
                Assert-Contract (([IO.FileAttributes]$sentinelRootEvidence.Attributes -band [IO.FileAttributes]::Directory) -ne 0) 'sentinel cleanup root is not a directory'
                Assert-Contract (([IO.FileAttributes]$sentinelRootEvidence.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0) 'sentinel cleanup root became a reparse point'
            }

            $artifactLease = [DynamoIsolatedContractNativeV3]::PinPath($artifactRoot)
            Assert-NativeIdentity -Actual ($artifactLease.Verify()) -Expected $artifactBaseline -Label 'artifact root'
            $markerLease = [DynamoIsolatedContractNativeV3]::PinPath($ownedMarker)
            $executableSentinelMarkerLease = [DynamoIsolatedContractNativeV3]::PinPath($executableSentinelMarker)
            $outputSentinelMarkerLease = [DynamoIsolatedContractNativeV3]::PinPath($outputSentinelMarker)
            Assert-NativeIdentity -Actual ($markerLease.Verify()) -Expected $markerEvidence -Label 'suite marker'
            Assert-NativeIdentity -Actual ($executableSentinelMarkerLease.Verify()) -Expected $executableSentinelMarkerEvidence -Label 'executable sentinel marker'
            Assert-NativeIdentity -Actual ($outputSentinelMarkerLease.Verify()) -Expected $outputSentinelMarkerEvidence -Label 'output sentinel marker'
            foreach ($pinnedMarker in @($markerLease, $executableSentinelMarkerLease, $outputSentinelMarkerLease)) {
                $pinnedMarkerEvidence = $pinnedMarker.Verify()
                Assert-Contract (([IO.FileAttributes]$pinnedMarkerEvidence.Attributes -band ([IO.FileAttributes]::Directory -bor [IO.FileAttributes]::ReparsePoint)) -eq 0) 'pinned ownership marker is not a regular file'
            }
            $markerBytes = $markerLease.ReadAllBytes()
            $executableSentinelBytes = $executableSentinelMarkerLease.ReadAllBytes()
            $outputSentinelBytes = $outputSentinelMarkerLease.ReadAllBytes()
            Assert-Contract (([Text.Encoding]::UTF8.GetString($markerBytes)) -ceq $markerNonce) 'suite ownership marker nonce changed'
            Assert-Contract (([Text.Encoding]::UTF8.GetString($executableSentinelBytes)) -ceq $executableSentinelNonce) 'executable sentinel marker nonce changed'
            Assert-Contract (([Text.Encoding]::UTF8.GetString($outputSentinelBytes)) -ceq $outputSentinelNonce) 'output sentinel marker nonce changed'
            Assert-Contract (([Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($markerBytes)).ToLowerInvariant()) -ceq $markerHash) 'suite ownership marker hash changed'
            Assert-Contract (([Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($executableSentinelBytes)).ToLowerInvariant()) -ceq $executableSentinelHash) 'executable sentinel marker hash changed'
            Assert-Contract (([Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($outputSentinelBytes)).ToLowerInvariant()) -ceq $outputSentinelHash) 'output sentinel marker hash changed'

            Remove-PinnedFlatRoot -RootLease $artifactLease -AllowedReparseLeaves @($junctionLeaves)
            $markerLease.DeletePinned()
            Remove-PinnedFlatRoot -RootLease $suiteRootLease
            $executableSentinelMarkerLease.DeletePinned()
            Remove-PinnedFlatRoot -RootLease $executableSentinelLease
            $outputSentinelMarkerLease.DeletePinned()
            Remove-PinnedFlatRoot -RootLease $outputSentinelLease
        }
    }
    catch {
        $cleanupErrors.Add($_.Exception)
    }
    finally {
        foreach ($lease in @(
            $artifactLease,
            $markerLease,
            $executableSentinelMarkerLease,
            $outputSentinelMarkerLease,
            $suiteRootLease,
            $executableSentinelLease,
            $outputSentinelLease
        )) {
            if ($null -ne $lease) { $lease.Dispose() }
        }
    }
}

$allFailures = [System.Collections.Generic.List[Exception]]::new()
if ($null -ne $testError) { $allFailures.Add($testError.Exception) }
foreach ($cleanupError in $cleanupErrors) { $allFailures.Add($cleanupError) }
if ($allFailures.Count -eq 1) {
    throw $allFailures[0]
}
if ($allFailures.Count -gt 1) {
    throw [AggregateException]::new('isolated-process-job contract and cleanup failures', $allFailures)
}

Write-Output '{"contract":"isolated-process-job","status":"pass"}'
