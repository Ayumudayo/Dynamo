#requires -Version 7.4

[CmdletBinding(DefaultParameterSetName = 'Recover')]
param(
    [Parameter(Mandatory, ParameterSetName = 'Initialize')]
    [Parameter(Mandatory, ParameterSetName = 'Advance')]
    [Parameter(Mandatory, ParameterSetName = 'Recover')]
    [ValidateSet('Initialize', 'Advance', 'Recover')]
    [string] $Mode,

    [Parameter(Mandatory, ParameterSetName = 'Initialize')]
    [Parameter(Mandatory, ParameterSetName = 'Advance')]
    [Parameter(Mandatory, ParameterSetName = 'Recover')]
    [ValidatePattern('^[a-z][a-z0-9-]{0,63}$')]
    [string] $Wave,

    [Parameter(Mandatory, ParameterSetName = 'Initialize')]
    [Parameter(Mandatory, ParameterSetName = 'Advance')]
    [ValidatePattern('^[a-z][a-z0-9-]{0,63}$')]
    [string] $Unit,

    [Parameter(Mandatory, ParameterSetName = 'Advance')]
    [ValidatePattern('^[0-9a-f]{40}$')]
    [string] $OldTip,

    [Parameter(Mandatory, ParameterSetName = 'Initialize')]
    [Parameter(Mandatory, ParameterSetName = 'Advance')]
    [ValidatePattern('^[0-9a-f]{40}$')]
    [string] $NewTip,

    [Parameter(Mandatory, ParameterSetName = 'Initialize')]
    [Parameter(Mandatory, ParameterSetName = 'Advance')]
    [ValidatePattern('^[0-9a-f]{64}$')]
    [string] $MicroplanSha256
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
if ($Mode -cne $PSCmdlet.ParameterSetName) { throw "Mode $Mode does not match parameter set $($PSCmdlet.ParameterSetName)." }
if ($Wave -cnotmatch '^[a-z][a-z0-9-]{0,63}$') { throw 'Wave must be the canonical lowercase slug.' }
if ($Wave -match '^(con|prn|aux|nul|com[1-9]|lpt[1-9])$') { throw 'Wave is a reserved filesystem device name.' }
if ($Mode -ne 'Recover') {
    if ($Unit -cnotmatch '^[a-z][a-z0-9-]{0,63}$' -or $NewTip -cnotmatch '^[0-9a-f]{40}$' -or $MicroplanSha256 -cnotmatch '^[0-9a-f]{64}$') {
        throw 'Unit/tip/microplan values are not canonical lowercase values.'
    }
    if ($Unit -match '^(con|prn|aux|nul|com[1-9]|lpt[1-9])$') { throw 'Unit is a reserved filesystem device name.' }
    if ($Mode -eq 'Advance' -and $OldTip -cnotmatch '^[0-9a-f]{40}$') { throw 'OldTip is not a canonical lowercase OID.' }
}

$script:ZeroOid = '0' * 40
$script:ZeroSha256 = '0' * 64
$script:Utf8 = [System.Text.UTF8Encoding]::new($false, $true)
$script:RefName = "refs/dynamo-remediation/$Wave/integration"
$script:AllowedFailpoints = @(
    'AfterLeaseCreate',
    'AfterIntentTempFsync',
    'AfterIntentRowRename',
    'AfterRefCas',
    'AfterTerminalTempFsync',
    'AfterTerminalRowRename',
    'AfterLeaseRename',
    'AfterClaimCreate',
    'AfterClaimArchive',
    'AfterClaimTakeover',
    'AfterRecoveryTerminalRowRename',
    'AfterClaimRename'
)
$script:AllowedBarriers = @('AfterNormalSnapshot','BeforeLeaseCreate','AfterActiveLeaseRead','BeforeClaimCreate','AfterActiveClaimRead','BeforeClaimTakeover')
$script:TestControlsValidated = $false
$script:TestOwnerMismatchPath = $null

if (-not ('DynamoRemediationNative' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
using System.Text;

public static class DynamoRemediationNative
{
    const uint FILE_READ_ATTRIBUTES = 0x80;
    const uint FILE_SHARE_READ = 1, FILE_SHARE_WRITE = 2, FILE_SHARE_DELETE = 4;
    const uint OPEN_EXISTING = 3;
    const uint FILE_FLAG_BACKUP_SEMANTICS = 0x02000000;
    const uint FILE_FLAG_OPEN_REPARSE_POINT = 0x00200000;
    const uint MOVEFILE_WRITE_THROUGH = 0x8;

    [StructLayout(LayoutKind.Sequential)]
    struct BY_HANDLE_FILE_INFORMATION {
        public uint FileAttributes;
        public System.Runtime.InteropServices.ComTypes.FILETIME CreationTime;
        public System.Runtime.InteropServices.ComTypes.FILETIME LastAccessTime;
        public System.Runtime.InteropServices.ComTypes.FILETIME LastWriteTime;
        public uint VolumeSerialNumber;
        public uint FileSizeHigh;
        public uint FileSizeLow;
        public uint NumberOfLinks;
        public uint FileIndexHigh;
        public uint FileIndexLow;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct FILE_ID_INFO {
        public ulong VolumeSerialNumber;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 16)] public byte[] FileId;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern SafeFileHandle CreateFileW(string name, uint access, uint share, IntPtr security,
        uint creation, uint flags, IntPtr template);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetFileInformationByHandle(SafeFileHandle handle, out BY_HANDLE_FILE_INFORMATION info);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetFileInformationByHandleEx(SafeFileHandle handle, int infoClass,
        out FILE_ID_INFO info, uint size);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern uint GetFinalPathNameByHandleW(SafeFileHandle handle, StringBuilder path, uint length, uint flags);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool MoveFileExW(string existingName, string newName, uint flags);

    static string LongPath(string path)
    {
        if (path.StartsWith(@"\\?\", StringComparison.Ordinal)) return path;
        if (path.StartsWith(@"\\", StringComparison.Ordinal)) return @"\\?\UNC\" + path.Substring(2);
        return @"\\?\" + Path.GetFullPath(path);
    }

    public static string GetIdentity(string path)
    {
        if (!RuntimeInformation.IsOSPlatform(OSPlatform.Windows)) {
            var item = new FileInfo(path);
            return item.FullName + "|" + item.CreationTimeUtc.Ticks.ToString();
        }
        using (var h = CreateFileW(LongPath(path), FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, IntPtr.Zero, OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, IntPtr.Zero)) {
            if (h.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error(), "CreateFileW failed for identity");
            FILE_ID_INFO i;
            if (!GetFileInformationByHandleEx(h, 18, out i, (uint)Marshal.SizeOf(typeof(FILE_ID_INFO))))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            return i.VolumeSerialNumber.ToString("x16") + ":" + BitConverter.ToString(i.FileId).Replace("-", "").ToLowerInvariant();
        }
    }

    public static string GetFinalPath(string path)
    {
        if (!RuntimeInformation.IsOSPlatform(OSPlatform.Windows)) return Path.GetFullPath(path);
        using (var h = CreateFileW(LongPath(path), FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, IntPtr.Zero, OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, IntPtr.Zero)) {
            if (h.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error(), "CreateFileW failed for final path");
            var b = new StringBuilder(32768);
            uint n = GetFinalPathNameByHandleW(h, b, (uint)b.Capacity, 0);
            if (n == 0 || n >= b.Capacity) throw new Win32Exception(Marshal.GetLastWin32Error());
            string value = b.ToString();
            if (value.StartsWith(@"\\?\UNC\", StringComparison.OrdinalIgnoreCase)) return @"\\" + value.Substring(8);
            if (value.StartsWith(@"\\?\", StringComparison.OrdinalIgnoreCase)) return value.Substring(4);
            return value;
        }
    }

    public static void MoveNoReplaceWriteThrough(string source, string destination)
    {
        if (File.Exists(destination) || Directory.Exists(destination)) throw new IOException("Destination already exists: " + destination);
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows)) {
            if (!MoveFileExW(LongPath(source), LongPath(destination), MOVEFILE_WRITE_THROUGH))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "MoveFileExW failed");
        } else {
            File.Move(source, destination, false);
        }
    }
}
'@
}

function Get-UtcNowCanonical {
    [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ss.fffffffZ', [Globalization.CultureInfo]::InvariantCulture)
}

function Read-CanonicalJsonFile {
    param(
        [Parameter(Mandatory)][string] $Path,
        [string[]] $ExpectedKeys
    )
    $pathBefore = Assert-SafeExistingPath -Path $Path -LeafType File
    Assert-RestrictiveFile $Path
    $bytes = [IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -eq 0) { throw "Empty JSON file: $Path" }
    if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xef -and $bytes[1] -eq 0xbb -and $bytes[2] -eq 0xbf) { throw "UTF-8 BOM is forbidden: $Path" }
    foreach ($b in $bytes) { if ($b -eq 13) { throw "CR is forbidden in canonical JSON: $Path" } }
    $options = [Text.Json.JsonDocumentOptions]@{ AllowTrailingCommas = $false; CommentHandling = [Text.Json.JsonCommentHandling]::Disallow }
    $jsonText = $script:Utf8.GetString($bytes)
    $document = [Text.Json.JsonDocument]::Parse($jsonText, $options)
    try {
        if ($document.RootElement.ValueKind -ne [Text.Json.JsonValueKind]::Object) { throw "JSON root must be an object: $Path" }
        $value = Convert-JsonElement $document.RootElement
    } finally {
        $document.Dispose()
    }
    if ($ExpectedKeys) {
        $actual = @($value.Keys)
        if ($actual.Count -ne $ExpectedKeys.Count -or [string]::Join("`n", $actual) -cne [string]::Join("`n", $ExpectedKeys)) {
            throw "Unexpected canonical key order/schema: $Path"
        }
    }
    $reserialized = ConvertTo-CanonicalBytes $value
    if (-not (Test-BytesEqual $bytes $reserialized)) {
        throw "JSON is not its canonical minimal serialization: $Path"
    }
    $pathAfter = Assert-SafeExistingPath -Path $Path -LeafType File
    if ($pathAfter.Identity -cne $pathBefore.Identity -or $pathAfter.Owner -cne $pathBefore.Owner -or $pathAfter.AclSha256 -cne $pathBefore.AclSha256) {
        throw "JSON leaf identity/owner/ACL changed during read: $Path"
    }
    [pscustomobject]@{
        Path = $Path
        Bytes = $bytes
        Value = $value
        Sha256 = Get-Sha256Bytes $bytes
        Identity = $pathAfter.Identity
        Owner = $pathAfter.Owner
        AclSha256 = $pathAfter.AclSha256
    }
}

function Read-StableBytes([string] $Path) {
    $before = Assert-SafeExistingPath -Path $Path -LeafType File
    Assert-RestrictiveFile $Path
    $bytes = [IO.File]::ReadAllBytes($Path)
    $after = Assert-SafeExistingPath -Path $Path -LeafType File
    if ($before.Identity -cne $after.Identity -or $before.Owner -cne $after.Owner -or $before.AclSha256 -cne $after.AclSha256) { throw "Leaf identity/owner/ACL changed during read: $Path" }
    $bytes
}

function Convert-LfBlobToDeterministicCrlf([byte[]] $Bytes) {
    $stream = [IO.MemoryStream]::new()
    try {
        foreach ($byte in $Bytes) {
            if ($byte -eq 13) { throw 'Committed helper blob unexpectedly contains CR bytes.' }
            if ($byte -eq 10) { $stream.WriteByte(13) }
            $stream.WriteByte($byte)
        }
        $stream.ToArray()
    } finally { $stream.Dispose() }
}

function Write-CreateNewDurable([string] $Path, [byte[]] $Bytes) {
    $options = [IO.FileOptions]::WriteThrough
    $stream = [IO.FileStream]::new($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None, 4096, $options)
    try {
        $stream.Write($Bytes, 0, $Bytes.Length)
        $stream.Flush($true)
    } finally { $stream.Dispose() }
    $readback = [IO.File]::ReadAllBytes($Path)
    if (-not (Test-BytesEqual $Bytes $readback)) {
        throw "Durable readback mismatch: $Path"
    }
}

function Move-NoReplaceDurable([string] $Source, [string] $Destination) {
    [DynamoRemediationNative]::MoveNoReplaceWriteThrough($Source, $Destination)
    if (Test-Path -LiteralPath $Source) { throw "Source remained after rename: $Source" }
    if (-not (Test-Path -LiteralPath $Destination -PathType Leaf)) { throw "Destination missing after rename: $Destination" }
}

function Assert-CanonicalRecordUnchanged([object] $Expected, [object] $Actual, [string] $Label) {
    if ($Actual.Sha256 -cne $Expected.Sha256 -or
        -not (Test-BytesEqual $Actual.Bytes $Expected.Bytes) -or
        $Actual.Identity -cne $Expected.Identity -or
        $Actual.Owner -cne $Expected.Owner -or
        $Actual.AclSha256 -cne $Expected.AclSha256) {
        throw "$Label bytes/hash/native identity/owner/ACL changed."
    }
}

function Move-VerifiedCanonicalRecord {
    param(
        [Parameter(Mandatory)][string] $Source,
        [Parameter(Mandatory)][string] $Destination,
        [Parameter(Mandatory)][object] $Expected,
        [Parameter(Mandatory)][scriptblock] $Reader,
        [Parameter(Mandatory)][string] $Label
    )
    if (Test-Path -LiteralPath $Destination) { throw "$Label destination already exists." }
    $sourceParent = Split-Path -Parent $Source
    $destinationParent = Split-Path -Parent $Destination
    $sourceParentRecord = Assert-SafeExistingPath -Path $sourceParent -LeafType Directory
    $destinationParentRecord = Assert-SafeExistingPath -Path $destinationParent -LeafType Directory
    $sourceReadback = & $Reader $Source
    Assert-CanonicalRecordUnchanged $Expected $sourceReadback "$Label source"
    Move-NoReplaceDurable $Source $Destination
    $destinationReadback = & $Reader $Destination
    Assert-CanonicalRecordUnchanged $Expected $destinationReadback "$Label destination"
    $sourceParentReadback = Assert-SafeExistingPath -Path $sourceParent -LeafType Directory
    $destinationParentReadback = Assert-SafeExistingPath -Path $destinationParent -LeafType Directory
    if ($sourceParentRecord.Identity -cne $sourceParentReadback.Identity -or $sourceParentRecord.Owner -cne $sourceParentReadback.Owner -or $sourceParentRecord.AclSha256 -cne $sourceParentReadback.AclSha256 -or
        $destinationParentRecord.Identity -cne $destinationParentReadback.Identity -or $destinationParentRecord.Owner -cne $destinationParentReadback.Owner -or $destinationParentRecord.AclSha256 -cne $destinationParentReadback.AclSha256) {
        throw "$Label parent directory identity/owner/ACL changed during rename."
    }
    $destinationReadback
}

function Invoke-Failpoint([string] $Name) {
    if ($Name -notin $script:AllowedFailpoints) { throw "Unknown internal failpoint: $Name" }
    $requested = [Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_TEST_FAILPOINT', 'Process')
    if ($requested -eq $Name) {
        if ([Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_TEST_MODE', 'Process') -cne '1') {
            throw 'A failpoint was requested outside explicit remediation test mode.'
        }
        $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar
        if (-not $script:CommonDirectory.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Remediation failpoints are accepted only in an OS-temp-owned fixture repository.'
        }
        [Console]::Error.WriteLine("Injected remediation failpoint: $Name")
        [Environment]::Exit(97)
    }
}

function Assert-FailpointConfiguration {
    $requested = [Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_TEST_FAILPOINT', 'Process')
    $barrier = [Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_TEST_BARRIER', 'Process')
    $processQueryFailure = [Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_TEST_PROCESS_QUERY_FAILURE', 'Process')
    $processQueryFailurePid = [Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_TEST_PROCESS_QUERY_FAILURE_PID', 'Process')
    $ownerMismatchPath = [Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_TEST_OWNER_MISMATCH_PATH', 'Process')
    if (-not [string]::IsNullOrEmpty($requested) -and $requested -cnotin $script:AllowedFailpoints) { throw "Unknown remediation failpoint: $requested" }
    if (-not [string]::IsNullOrEmpty($barrier) -and $barrier -cnotin $script:AllowedBarriers) { throw "Unknown remediation barrier: $barrier" }
    if (-not [string]::IsNullOrEmpty($processQueryFailure) -and $processQueryFailure -cne 'AccessDenied') { throw "Unknown remediation process-query failure: $processQueryFailure" }
    if ([string]::IsNullOrEmpty($processQueryFailure) -xor [string]::IsNullOrEmpty($processQueryFailurePid)) { throw 'Process-query failure mode and PID must be supplied together.' }
    if (-not [string]::IsNullOrEmpty($processQueryFailurePid)) {
        [int64]$parsedFailurePid = 0
        if ($processQueryFailurePid -cnotmatch '^[1-9][0-9]{0,9}$' -or
            -not [int64]::TryParse($processQueryFailurePid, [Globalization.NumberStyles]::None, [Globalization.CultureInfo]::InvariantCulture, [ref]$parsedFailurePid) -or
            $parsedFailurePid -gt [int]::MaxValue) {
            throw 'Process-query failure PID is not a canonical positive platform PID.'
        }
    }
    if ([string]::IsNullOrEmpty($requested) -and [string]::IsNullOrEmpty($barrier) -and [string]::IsNullOrEmpty($processQueryFailure) -and [string]::IsNullOrEmpty($ownerMismatchPath)) {
        $script:TestControlsValidated = $true
        return
    }
    if ([Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_TEST_MODE', 'Process') -cne '1') {
        throw 'Remediation failpoints/barriers require explicit test mode.'
    }
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar
    if (-not $script:CommonDirectory.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Remediation test controls are accepted only in an OS-temp-owned fixture repository.'
    }
    if (-not [string]::IsNullOrEmpty($barrier)) {
        $barrierRoot = [Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_TEST_BARRIER_ROOT', 'Process')
        if ([string]::IsNullOrWhiteSpace($barrierRoot) -or -not [IO.Path]::IsPathFullyQualified($barrierRoot)) { throw 'Remediation barrier root must be an explicit absolute path.' }
        $barrierRoot = [IO.Path]::GetFullPath($barrierRoot)
        if (-not $barrierRoot.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'Remediation barrier root escaped the OS temp directory.' }
        Assert-SafeExistingPath -Path $barrierRoot -LeafType Directory | Out-Null
    }
    if (-not [string]::IsNullOrEmpty($ownerMismatchPath)) {
        if (-not [IO.Path]::IsPathFullyQualified($ownerMismatchPath)) { throw 'Owner-mismatch test path must be absolute.' }
        $ownerMismatchPath = [IO.Path]::GetFullPath($ownerMismatchPath)
        $commonPrefix = $script:CommonDirectory.TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar
        if (-not $ownerMismatchPath.StartsWith($commonPrefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'Owner-mismatch test path escaped the Git common directory.' }
        Assert-SafeExistingPath -Path $ownerMismatchPath -LeafType Directory | Out-Null
        $script:TestOwnerMismatchPath = $ownerMismatchPath
    }
    $script:TestControlsValidated = $true
}

function Invoke-TestBarrier([string] $Name) {
    if ($Name -cnotin $script:AllowedBarriers) { throw "Unknown internal remediation barrier: $Name" }
    $requested = [Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_TEST_BARRIER', 'Process')
    if ($requested -cne $Name) { return }
    if ([Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_TEST_MODE', 'Process') -cne '1') { throw 'A remediation barrier was requested outside explicit test mode.' }
    $rootValue = [Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_TEST_BARRIER_ROOT', 'Process')
    if ([string]::IsNullOrWhiteSpace($rootValue) -or -not [IO.Path]::IsPathFullyQualified($rootValue)) { throw 'Remediation barrier root must be an explicit absolute path.' }
    $root = [IO.Path]::GetFullPath($rootValue)
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar
    if (-not $root.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -or
        -not $script:CommonDirectory.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Remediation barriers are accepted only in OS-temp fixtures.'
    }
    Assert-SafeExistingPath -Path $root -LeafType Directory | Out-Null
    $readyPath = Join-Path $root "$Name.$PID.$([Guid]::NewGuid().ToString('N').ToLowerInvariant()).ready"
    Write-CreateNewDurable $readyPath $script:Utf8.GetBytes("$Name`n")
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (@(Get-ChildItem -LiteralPath $root -File -Force | Where-Object { $_.Name.StartsWith("$Name.", [StringComparison]::Ordinal) -and $_.Name.EndsWith('.ready', [StringComparison]::Ordinal) }).Count -lt 2) {
        if ([DateTime]::UtcNow -ge $deadline) { throw "Remediation barrier timed out: $Name" }
        Start-Sleep -Milliseconds 20
    }
}

function Invoke-Git {
    param([Parameter(Mandatory)][string[]] $Arguments, [switch] $AllowFailure)
    $psi = [Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = 'git'
    $psi.WorkingDirectory = $script:RepositoryProbeRoot
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true
    foreach ($name in @('GIT_DIR','GIT_WORK_TREE','GIT_COMMON_DIR','GIT_INDEX_FILE','GIT_OBJECT_DIRECTORY','GIT_ALTERNATE_OBJECT_DIRECTORIES','GIT_NAMESPACE','GIT_REPLACE_REF_BASE','GIT_CEILING_DIRECTORIES','GIT_DISCOVERY_ACROSS_FILESYSTEM','GIT_CONFIG','GIT_CONFIG_GLOBAL','GIT_CONFIG_SYSTEM','GIT_CONFIG_COUNT')) {
        $null = $psi.Environment.Remove($name)
    }
    $psi.Environment['GIT_CONFIG_NOSYSTEM'] = '1'
    $psi.Environment['GIT_TERMINAL_PROMPT'] = '0'
    $psi.ArgumentList.Add('--no-replace-objects')
    $psi.ArgumentList.Add('-c'); $psi.ArgumentList.Add('core.hooksPath=NUL')
    $psi.ArgumentList.Add('-c'); $psi.ArgumentList.Add('protocol.file.allow=never')
    $psi.ArgumentList.Add('-C'); $psi.ArgumentList.Add($script:RepositoryProbeRoot)
    foreach ($argument in $Arguments) { $psi.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($psi)
    $stdout = $process.StandardOutput.ReadToEnd()
    $stderr = $process.StandardError.ReadToEnd()
    $process.WaitForExit()
    if ($process.ExitCode -ne 0 -and -not $AllowFailure) {
        throw "git failed ($($process.ExitCode)): $stderr"
    }
    [pscustomobject]@{ ExitCode = $process.ExitCode; Stdout = $stdout.TrimEnd("`r", "`n"); Stderr = $stderr.TrimEnd("`r", "`n") }
}

function Get-GitBlobBytes([string] $Revision, [string] $RepositoryPath) {
    if ($RepositoryPath -notmatch '^[a-zA-Z0-9._/-]+$' -or $RepositoryPath.Contains('..')) { throw "Unsafe repository path: $RepositoryPath" }
    $psi = [Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = 'git'
    $psi.WorkingDirectory = $script:RepositoryProbeRoot
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true
    foreach ($name in @('GIT_DIR','GIT_WORK_TREE','GIT_COMMON_DIR','GIT_INDEX_FILE','GIT_OBJECT_DIRECTORY','GIT_ALTERNATE_OBJECT_DIRECTORIES','GIT_NAMESPACE','GIT_REPLACE_REF_BASE','GIT_CEILING_DIRECTORIES','GIT_DISCOVERY_ACROSS_FILESYSTEM','GIT_CONFIG','GIT_CONFIG_GLOBAL','GIT_CONFIG_SYSTEM','GIT_CONFIG_COUNT')) { $null = $psi.Environment.Remove($name) }
    $psi.Environment['GIT_CONFIG_NOSYSTEM'] = '1'; $psi.Environment['GIT_TERMINAL_PROMPT'] = '0'
    foreach ($argument in @('--no-replace-objects','-c','core.hooksPath=NUL','-c','protocol.file.allow=never','-C',$script:RepositoryProbeRoot,'cat-file','blob',"${Revision}:$RepositoryPath")) { $psi.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($psi)
    $memory = [IO.MemoryStream]::new()
    try {
        $copy = $process.StandardOutput.BaseStream.CopyToAsync($memory)
        $stderr = $process.StandardError.ReadToEndAsync()
        $null = $copy.GetAwaiter().GetResult()
        $errorText = $stderr.GetAwaiter().GetResult()
        $process.WaitForExit()
        if ($process.ExitCode -ne 0) { throw "git cat-file failed ($($process.ExitCode)): $errorText" }
        $memory.ToArray()
    } finally { $memory.Dispose(); $process.Dispose() }
}

function Get-AclRecord([string] $Path) {
    if ($IsWindows) {
        $item = Get-Item -LiteralPath $Path -Force
        $acl = [IO.FileSystemAclExtensions]::GetAccessControl($item)
        return [pscustomobject]@{
            Owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
            Group = $acl.GetGroup([Security.Principal.SecurityIdentifier]).Value
            InheritanceProtected = [bool]$acl.AreAccessRulesProtected
            Acl = $acl.GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::Access)
        }
    }
    $item = Get-Item -LiteralPath $Path -Force
    [pscustomobject]@{ Owner = 'unsupported'; Group = 'unsupported'; InheritanceProtected = $true; Acl = [string]$item.UnixFileMode }
}

function Get-PathRecord([string] $Path) {
    if (-not $IsWindows) { throw 'Native file identity and ACL inspection are unsupported on this platform.' }
    $full = [IO.Path]::GetFullPath($Path)
    if (-not (Test-Path -LiteralPath $full)) { throw "Path does not exist: $full" }
    $item = Get-Item -LiteralPath $full -Force
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Reparse point rejected: $full" }
    $final = [DynamoRemediationNative]::GetFinalPath($full)
    if ($final -cne $full) { throw "Lexical/final path mismatch: $full -> $final" }
    $acl = Get-AclRecord $full
    $nativeIdentity = [DynamoRemediationNative]::GetIdentity($full)
    $identityParts = $nativeIdentity.Split(':')
    if ($identityParts.Count -ne 2) { throw 'Native identity representation is malformed.' }
    $platform = if ($IsWindows) { 'windows' } else { 'posix' }
    $aclPreimage = [ordered]@{
        platform = $platform
        owner = $acl.Owner
        group = $acl.Group
        inheritance_protected = $acl.InheritanceProtected
        descriptor = $acl.Acl
    }
    $aclSha256 = Get-Sha256Text 'dynamo-acl-fingerprint-v1' (ConvertTo-CanonicalBytes $aclPreimage)
    $identityPreimage = [ordered]@{
        platform = $platform
        volume_or_device = $identityParts[0]
        file_id_or_inode = $identityParts[1]
    }
    [pscustomobject]@{
        Path = $full
        Identity = $nativeIdentity
        IdentitySha256 = Get-Sha256Text 'dynamo-native-identity-v1' (ConvertTo-CanonicalBytes $identityPreimage)
        Owner = $acl.Owner
        Acl = $acl.Acl
        AclSha256 = $aclSha256
    }
}

function Assert-SafeExistingPath {
    param([string] $Path, [ValidateSet('Any','File','Directory')][string] $LeafType = 'Any')
    $full = Resolve-ReparseFreeExistingPath -Path $Path
    $leaf = Get-Item -LiteralPath $full -Force
    if ($LeafType -eq 'File' -and $leaf.PSIsContainer) { throw "Expected regular file: $full" }
    if ($LeafType -eq 'Directory' -and -not $leaf.PSIsContainer) { throw "Expected directory: $full" }
    Get-PathRecord $full
}

function New-RestrictiveDirectorySecurity {
    $current = [Security.Principal.WindowsIdentity]::GetCurrent().User
    $system = [Security.Principal.SecurityIdentifier]::new('S-1-5-18')
    $security = [Security.AccessControl.DirectorySecurity]::new()
    $security.SetOwner($current)
    $security.SetAccessRuleProtection($true, $false)
    $inheritance = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
    $propagation = [Security.AccessControl.PropagationFlags]::None
    foreach ($sid in @($current, $system)) {
        $rule = [Security.AccessControl.FileSystemAccessRule]::new($sid, [Security.AccessControl.FileSystemRights]::FullControl, $inheritance, $propagation, [Security.AccessControl.AccessControlType]::Allow)
        $security.AddAccessRule($rule)
    }
    $security
}

function Set-RestrictiveDirectory([string] $Path) {
    if ($IsWindows) {
        [IO.FileSystemAclExtensions]::SetAccessControl([IO.DirectoryInfo]::new($Path), (New-RestrictiveDirectorySecurity))
    } else { [IO.Directory]::SetUnixFileMode($Path, [IO.UnixFileMode]'UserRead, UserWrite, UserExecute') }
}

function Assert-RestrictivePathAcl {
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][ValidateSet('Directory','File')][string] $Kind
    )
    if ($IsWindows) {
        $current = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
        if ($script:TestControlsValidated -and $null -ne $script:TestOwnerMismatchPath -and [IO.Path]::GetFullPath($Path) -ceq $script:TestOwnerMismatchPath) {
            $current = 'S-1-0-0'
        }
        $system = 'S-1-5-18'
        $item = if ($Kind -eq 'Directory') { [IO.DirectoryInfo]::new($Path) } else { [IO.FileInfo]::new($Path) }
        $acl = [IO.FileSystemAclExtensions]::GetAccessControl($item)
        if ($acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -cne $current) {
            throw "Control $($Kind.ToLowerInvariant()) owner mismatch: $Path"
        }
        if ($Kind -eq 'Directory' -and -not $acl.AreAccessRulesProtected) {
            throw "Control directory inheritance must be protected: $Path"
        }
        $rules = @($acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]))
        if ($rules.Count -ne 2) { throw "Control $($Kind.ToLowerInvariant()) must have exactly two ACEs: $Path" }
        $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        foreach ($rule in $rules) {
            $sid = $rule.IdentityReference.Value
            if ($sid -notin @($current,$system) -or $rule.AccessControlType -ne [Security.AccessControl.AccessControlType]::Allow -or
                $rule.FileSystemRights -ne [Security.AccessControl.FileSystemRights]::FullControl -or
                $rule.PropagationFlags -ne [Security.AccessControl.PropagationFlags]::None) {
                throw "Unexpected control $($Kind.ToLowerInvariant()) ACE: $Path"
            }
            if ($Kind -eq 'Directory') {
                $expectedInheritance = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
                if ($rule.IsInherited -or $rule.InheritanceFlags -ne $expectedInheritance) {
                    throw "Unexpected control-directory ACE inheritance: $Path"
                }
            } elseif ($rule.InheritanceFlags -ne [Security.AccessControl.InheritanceFlags]::None) {
                throw "Unexpected control-file ACE inheritance flags: $Path"
            }
            if (-not $seen.Add($sid)) { throw "Duplicate control $($Kind.ToLowerInvariant()) ACE principal: $Path" }
        }
        if (-not $seen.Contains($current) -or -not $seen.Contains($system)) { throw "Control $($Kind.ToLowerInvariant()) ACE principals are not exact: $Path" }
    } else {
        $mode = (Get-Item -LiteralPath $Path -Force).UnixFileMode
        $expectedMode = if ($Kind -eq 'Directory') { [IO.UnixFileMode]'UserRead, UserWrite, UserExecute' } else { [IO.UnixFileMode]'UserRead, UserWrite' }
        if ($mode -ne $expectedMode) { throw "Control $($Kind.ToLowerInvariant()) mode is not restrictive: $Path" }
    }
}

function Assert-RestrictiveDirectory([string] $Path) {
    Assert-RestrictivePathAcl -Path $Path -Kind Directory
}

function Assert-RestrictiveFile([string] $Path) {
    Assert-RestrictivePathAcl -Path $Path -Kind File
}

function Ensure-SafeDirectoryChain([string] $TrustedRoot, [string[]] $Children) {
    $rootRecord = Assert-SafeExistingPath -Path $TrustedRoot -LeafType Directory
    $current = $rootRecord.Path
    foreach ($child in $Children) {
        if ($child -notmatch '^[a-zA-Z0-9._-]+$') { throw "Unsafe directory component: $child" }
        $parentRecord = Assert-SafeExistingPath -Path $current -LeafType Directory
        $next = Join-Path $current $child
        if (-not (Test-Path -LiteralPath $next)) {
            if (-not $IsWindows) { throw 'Restrictive create-at-birth ACL is unsupported on this platform.' }
            [IO.FileSystemAclExtensions]::CreateDirectory((New-RestrictiveDirectorySecurity), $next) | Out-Null
        }
        $record = Assert-SafeExistingPath -Path $next -LeafType Directory
        Assert-RestrictiveDirectory $next
        $parentReadback = Assert-SafeExistingPath -Path $current -LeafType Directory
        if ($parentReadback.Identity -cne $parentRecord.Identity -or $parentReadback.Owner -cne $parentRecord.Owner -or $parentReadback.AclSha256 -cne $parentRecord.AclSha256) {
            throw "Parent identity/owner/ACL changed while deriving $next"
        }
        $rootReadback = Assert-SafeExistingPath -Path $TrustedRoot -LeafType Directory
        if ($rootReadback.Identity -cne $rootRecord.Identity -or $rootReadback.Owner -cne $rootRecord.Owner -or $rootReadback.AclSha256 -cne $rootRecord.AclSha256) {
            throw "Trusted root identity/owner/ACL changed while deriving $next"
        }
        $parentPrefix = $current.TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar
        if (-not $record.Path.StartsWith($parentPrefix, [StringComparison]::Ordinal)) { throw "Directory escaped trusted root: $next" }
        $current = $record.Path
    }
    $current
}

function Get-PropertyRecursive([object] $Value, [string[]] $Names) {
    if ($Value -is [Collections.IDictionary]) {
        foreach ($name in $Names) { if ($Value.Contains($name)) { return $Value[$name] } }
        foreach ($key in $Value.Keys) {
            $found = Get-PropertyRecursive $Value[$key] $Names
            if ($null -ne $found) { return $found }
        }
    } elseif (($Value -is [Collections.IEnumerable]) -and -not ($Value -is [string])) {
        foreach ($item in $Value) {
            $found = Get-PropertyRecursive $item $Names
            if ($null -ne $found) { return $found }
        }
    }
    $null
}

function Assert-LowerHex([string] $Value, [int] $Length, [string] $Label) {
    if ($Value -cnotmatch "^[0-9a-f]{$Length}$") { throw "Invalid $Label" }
}

function Assert-JsonInt64Token([object] $Value, [string] $Label) {
    if ($Value -isnot [int64]) { throw "$Label must be a JSON signed 64-bit integer token." }
}

function Assert-CanonicalUtcTimestamp([object] $Value, [string] $Label) {
    if ($Value -isnot [string]) { throw "$Label must be a JSON string." }
    $text = [string]$Value
    if ($text -cnotmatch '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{7}Z$') { throw "$Label is not canonical UTC." }
    $format = "yyyy-MM-dd'T'HH:mm:ss.fffffff'Z'"
    [DateTimeOffset]$parsed = [DateTimeOffset]::MinValue
    $styles = [Globalization.DateTimeStyles]::AssumeUniversal -bor [Globalization.DateTimeStyles]::AdjustToUniversal
    if (-not [DateTimeOffset]::TryParseExact($text, $format, [Globalization.CultureInfo]::InvariantCulture, $styles, [ref]$parsed) -or
        $parsed.UtcDateTime.ToString($format, [Globalization.CultureInfo]::InvariantCulture) -cne $text) {
        throw "$Label is not a calendar-valid canonical UTC timestamp."
    }
}

function Assert-PersistedOperationTuple([string] $UnitName, [string] $Old, [string] $New, [string] $Label) {
    if ($UnitName -cnotmatch '^[a-z][a-z0-9-]{0,63}$' -or $UnitName -match '^(con|prn|aux|nul|com[1-9]|lpt[1-9])$') {
        throw "$Label unit is not a canonical non-reserved slug."
    }
    Assert-LowerHex $Old 40 "$Label old_tip"
    Assert-LowerHex $New 40 "$Label new_tip"
    if ($New -ceq $script:ZeroOid) { throw "$Label new_tip may not be the zero OID." }
    if ($Old -ceq $script:ZeroOid) {
        if ($New -cne $script:Binding.ExecutionBaseline) { throw "$Label initialize tuple must target the immutable execution baseline." }
    } elseif ($Old -ceq $New) {
        throw "$Label advance tuple must use distinct old/new tips."
    }
}

function Assert-DeadOwnerForCommonIdentity([Collections.IDictionary] $Owner, [string] $CommonIdentitySha256, [string] $Label) {
    if ([string]$Owner['machine_identity_sha256'] -cne (Get-MachineIdentitySha256)) { throw "$Label belongs to another machine and cannot be proven dead." }
    if ([string]$Owner['git_common_dir_identity_sha256'] -cne $CommonIdentitySha256) { throw "$Label common-directory identity mismatch." }
    $currentStart = Get-ProcessStartIdentity ([int]$Owner['pid'])
    if ($null -eq $currentStart) { return }
    if ($currentStart -ceq [string]$Owner['process_start_identity']) { throw "$Label owner process is still alive." }
    # A live PID with a different process-birth identity proves PID reuse.
}

function Assert-BindingAndManifest([string] $CommonDirectory) {
    $controlRoot = Join-Path $CommonDirectory 'dynamo-remediation'
    Assert-SafeExistingPath -Path $controlRoot -LeafType Directory | Out-Null
    Assert-RestrictiveDirectory $controlRoot
    $bindingPath = Join-Path $CommonDirectory 'dynamo-remediation/plan-set-binding-v1.json'
    $bindingProbe = Read-CanonicalJsonFile -Path $bindingPath
    Assert-JsonInt64Token $bindingProbe.Value['schema_version'] 'binding schema_version'
    [int64]$bindingSchemaVersion = $bindingProbe.Value['schema_version']
    try { $bindingKeys = Get-PlanSetBindingKeys $bindingSchemaVersion }
    catch { throw 'Binding schema version mismatch.' }
    $binding = Read-CanonicalJsonFile -Path $bindingPath -ExpectedKeys $bindingKeys
    $keys = @($binding.Value.Keys)
    if ($keys.Count -lt 2 -or $keys[-1] -cne 'binding_sha256') { throw 'binding_sha256 must be the final binding field.' }
    $bindingHash = [string]$binding.Value['binding_sha256']
    Assert-LowerHex $bindingHash 64 'binding_sha256'
    $preimage = [ordered]@{}
    foreach ($key in $keys[0..($keys.Count - 2)]) { $preimage[$key] = $binding.Value[$key] }
    $calculated = Get-Sha256Text 'dynamo-plan-set-binding-v1' (ConvertTo-CanonicalBytes $preimage)
    if ($calculated -cne $bindingHash) { throw 'Binding hash mismatch.' }
    Assert-JsonInt64Token $binding.Value['schema_version'] 'binding schema_version'
    Assert-JsonInt64Token $binding.Value['manifest_bytes'] 'binding manifest_bytes'
    if ($bindingSchemaVersion -notin @(1,2) -or [string]$binding.Value['audit_baseline'] -cne '03ec755eb109975ecc8911f26cc75ee482f32a7a') { throw 'Binding schema/audit baseline mismatch.' }
    $hashFields = @('plan_set_sha256','manifest_sha256','git_common_dir_identity_sha256','git_common_dir_acl_sha256','bundle_prepared_row_sha256','binding_sha256')
    if ($bindingSchemaVersion -eq 1) { $hashFields += @('publisher_sha256','integration_helper_sha256','publisher_contract_test_sha256','integration_contract_test_sha256') }
    else { $hashFields += @('control_schema_sha256') }
    foreach ($field in $hashFields) {
        Assert-LowerHex ([string]$binding.Value[$field]) 64 "binding $field"
    }

    $execution = [string](Get-PropertyRecursive $binding.Value @('execution_baseline'))
    $planSet = [string](Get-PropertyRecursive $binding.Value @('plan_set_sha256'))
    Assert-LowerHex $execution 40 'execution_baseline'
    Assert-LowerHex $planSet 64 'plan_set_sha256'

    $commonRecord = Get-PathRecord $CommonDirectory
    if ([string]$binding.Value['git_common_dir_native_path'] -cne $commonRecord.Path) { throw 'Git common-directory native path mismatch.' }
    if ([string]$binding.Value['git_common_dir_identity_sha256'] -cne $commonRecord.IdentitySha256) { throw 'Git common-directory native identity mismatch.' }
    if ([string]$binding.Value['git_common_dir_owner'] -cne $commonRecord.Owner) { throw 'Git common-directory owner mismatch.' }
    if ([string]$binding.Value['git_common_dir_acl_sha256'] -cne $commonRecord.AclSha256) { throw 'Git common-directory ACL mismatch.' }

    $manifestPath = [string](Get-PropertyRecursive $binding.Value @('manifest_native_path','manifest_path'))
    $manifestHash = [string](Get-PropertyRecursive $binding.Value @('manifest_sha256'))
    $manifestBytes = Get-PropertyRecursive $binding.Value @('manifest_bytes')
    if (-not [IO.Path]::IsPathFullyQualified($manifestPath)) { throw 'Bound manifest path must be absolute.' }
    if ([IO.Path]::GetFileName($manifestPath) -cne 'plan-set-manifest-v1.json') { throw 'Bound manifest filename is not fixed.' }
    $manifestDirectory = [IO.DirectoryInfo]::new([IO.Path]::GetDirectoryName($manifestPath))
    if ($manifestDirectory.Name -cne $planSet -or $manifestDirectory.Parent.Name -cne $execution -or
        $manifestDirectory.Parent.Parent.Name -cne 'plan-set' -or $manifestDirectory.Parent.Parent.Parent.Name -cne 'Dynamo') {
        throw 'Bound manifest does not have the fixed evidence bundle derivation.'
    }
    Assert-LowerHex $manifestHash 64 'manifest_sha256'
    $manifestKeys = @('schema_version','plan_set_sha256','audit_baseline','execution_baseline','git_common_dir_identity_sha256','payloads','controls','gitignore_evidence','published_at')
    $manifest = Read-CanonicalJsonFile -Path $manifestPath -ExpectedKeys $manifestKeys
    if ($manifest.Sha256 -cne $manifestHash) { throw 'Bound manifest hash mismatch.' }
    if ([int64]$manifestBytes -ne $manifest.Bytes.LongLength) { throw 'Bound manifest byte count mismatch.' }
    if ([string](Get-PropertyRecursive $manifest.Value @('execution_baseline')) -cne $execution -or
        [string](Get-PropertyRecursive $manifest.Value @('plan_set_sha256')) -cne $planSet) { throw 'Manifest/binding identity mismatch.' }
    Assert-JsonInt64Token $manifest.Value['schema_version'] 'manifest schema_version'
    Assert-CanonicalUtcTimestamp $manifest.Value['published_at'] 'manifest published_at'
    if ([int64]$manifest.Value['schema_version'] -ne 1 -or [string]$manifest.Value['audit_baseline'] -cne [string]$binding.Value['audit_baseline'] -or
        [string]$manifest.Value['git_common_dir_identity_sha256'] -cne [string]$binding.Value['git_common_dir_identity_sha256']) { throw 'Manifest scalar contract mismatch.' }

    $manifestCore = [ordered]@{
        schema_version = [int64]$manifest.Value['schema_version']
        audit_baseline = [string]$manifest.Value['audit_baseline']
        execution_baseline = [string]$manifest.Value['execution_baseline']
        git_common_dir_identity_sha256 = [string]$manifest.Value['git_common_dir_identity_sha256']
        payloads = @($manifest.Value['payloads'])
        controls = @($manifest.Value['controls'])
    }
    $manifestCoreBytes = ConvertTo-CanonicalBytes $manifestCore
    $canonicalCoreSha256 = Get-Sha256Bytes $manifestCoreBytes
    $calculatedPlanSetSha256 = Get-Sha256Text 'dynamo-plan-set-v1' $manifestCoreBytes
    if ($calculatedPlanSetSha256 -cne $planSet) { throw 'Manifest deterministic plan-set digest mismatch.' }
    $sourceSet = [ordered]@{
        payloads = @($manifest.Value['payloads'])
        controls = @($manifest.Value['controls'])
    }
    $sourceSetSha256 = Get-Sha256Text 'dynamo-source-set-v1' (ConvertTo-CanonicalBytes $sourceSet)

    $expectedPayloadPaths = @(
        'docs/superpowers/plans/2026-07-12-dashboard-ux-remediation.md',
        'docs/superpowers/plans/2026-07-12-performance-remediation.md',
        'docs/superpowers/plans/2026-07-12-security-performance-dashboard-remediation-program.md',
        'docs/superpowers/plans/2026-07-12-security-remediation.md',
        'docs/superpowers/plans/2026-07-13-wave0-bootstrap.md'
    )
    $payloadRows = @($manifest.Value['payloads'])
    if ($payloadRows.Count -ne $expectedPayloadPaths.Count) { throw 'Manifest payload set is not exact.' }
    $bundleRoot = [IO.Path]::GetDirectoryName($manifestPath)
    Assert-SafeExistingPath -Path $bundleRoot -LeafType Directory | Out-Null
    Assert-RestrictiveDirectory $bundleRoot
    for ($index = 0; $index -lt $payloadRows.Count; $index++) {
        $payload = $payloadRows[$index]
        if ($payload -isnot [Collections.IDictionary] -or [string]::Join("`n", @($payload.Keys)) -cne "path`nbytes`nsha256" -or [string]$payload['path'] -cne $expectedPayloadPaths[$index]) { throw 'Manifest payload row schema/order mismatch.' }
        Assert-JsonInt64Token $payload['bytes'] 'manifest payload bytes'
        Assert-LowerHex ([string]$payload['sha256']) 64 'payload sha256'
        $payloadPath = [IO.Path]::GetFullPath((Join-Path $bundleRoot ([string]$payload['path']).Replace('/',[IO.Path]::DirectorySeparatorChar)))
        $bundlePrefix = $bundleRoot.TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar
        if (-not $payloadPath.StartsWith($bundlePrefix,[StringComparison]::Ordinal)) { throw 'Payload path escaped the immutable bundle.' }
        $payloadBytes = Read-StableBytes $payloadPath
        if ($payloadBytes.LongLength -ne [int64]$payload['bytes'] -or (Get-Sha256Bytes $payloadBytes) -cne [string]$payload['sha256']) { throw "Payload byte/hash mismatch: $($payload['path'])" }
    }
    $expectedBundleLeaves = @($manifestPath) + @($expectedPayloadPaths | ForEach-Object { [IO.Path]::GetFullPath((Join-Path $bundleRoot $_.Replace('/',[IO.Path]::DirectorySeparatorChar))) }) | Sort-Object
    $bundleItems = @(Get-ChildItem -LiteralPath $bundleRoot -Recurse -Force)
    if (@($bundleItems | Where-Object { ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 }).Count -ne 0) { throw 'Immutable plan bundle contains a reparse point.' }
    $actualBundleLeaves = @($bundleItems | Where-Object { -not $_.PSIsContainer } | ForEach-Object FullName | Sort-Object)
    if ([string]::Join("`n",$actualBundleLeaves) -cne [string]::Join("`n",$expectedBundleLeaves)) { throw 'Immutable plan bundle contains missing or extra leaves.' }
    $ignoreRows = @($manifest.Value['gitignore_evidence'])
    if ($ignoreRows.Count -ne $expectedPayloadPaths.Count) { throw 'Manifest gitignore evidence set is not exact.' }
    for ($index = 0; $index -lt $ignoreRows.Count; $index++) {
        $ignore = $ignoreRows[$index]
        if ($ignore -is [Collections.IDictionary]) { Assert-JsonInt64Token $ignore['rule_line'] 'manifest gitignore rule_line' }
        if ($ignore -isnot [Collections.IDictionary] -or [string]::Join("`n",@($ignore.Keys)) -cne "path`nrule_source`nrule_line`npattern" -or
            [string]$ignore['path'] -cne $expectedPayloadPaths[$index] -or [int64]$ignore['rule_line'] -lt 1 -or
            [string]$ignore['rule_source'] -notmatch '^[a-zA-Z0-9._/-]+$' -or [string]$ignore['rule_source'] -match '(^|/)\.\.(/|$)') { throw 'Manifest gitignore-evidence row schema/order mismatch.' }
        $ignoreSourceBytes = Get-GitBlobBytes $execution ([string]$ignore['rule_source'])
        $ignoreSource = $script:Utf8.GetString($ignoreSourceBytes).Replace("`r`n","`n")
        $ignoreLines = $ignoreSource.Split("`n")
        if ([int64]$ignore['rule_line'] -gt $ignoreLines.Count -or $ignoreLines[[int64]$ignore['rule_line'] - 1] -cne [string]$ignore['pattern']) {
            throw 'Manifest gitignore evidence does not match the execution-baseline rule line.'
        }
    }

    $controls = Get-PropertyRecursive $manifest.Value @('controls')
    $controlRows = @($controls)
    $controlHashes = [ordered]@{}
    if ($bindingSchemaVersion -eq 1) {
        $controlBindingFields = [ordered]@{
            'scripts/remediation/publish-plan-set.ps1' = 'publisher_sha256'
            'scripts/remediation/update-integration-ref.ps1' = 'integration_helper_sha256'
            'tests/scripts/plan-set-publisher-contract.ps1' = 'publisher_contract_test_sha256'
            'tests/scripts/integration-ref-journal-contract.ps1' = 'integration_contract_test_sha256'
        }
        foreach ($path in $controlBindingFields.Keys) { $controlHashes[$path] = [string]$binding.Value[$controlBindingFields[$path]] }
    } else {
        Assert-JsonInt64Token $binding.Value['control_schema_version'] 'binding control_schema_version'
        if ([int64]$binding.Value['control_schema_version'] -ne 2 -or [string]$binding.Value['control_schema_path'] -cne 'scripts/remediation/control-schema-v2.json') {
            throw 'Binding v2 control-schema descriptor mismatch.'
        }
        [byte[]]$schemaBytes = Get-GitBlobBytes $execution ([string]$binding.Value['control_schema_path'])
        if ((Get-Sha256Bytes $schemaBytes) -cne [string]$binding.Value['control_schema_sha256']) { throw 'Bound control schema blob mismatch.' }
        $schemaDocument = [Text.Json.JsonDocument]::Parse($script:Utf8.GetString($schemaBytes), [Text.Json.JsonDocumentOptions]@{ AllowTrailingCommas = $false; CommentHandling = [Text.Json.JsonCommentHandling]::Disallow })
        try { $schemaValue = Convert-JsonElement $schemaDocument.RootElement } finally { $schemaDocument.Dispose() }
        if (-not (Test-BytesEqual $schemaBytes (ConvertTo-CanonicalBytes $schemaValue)) -or [string]::Join("`n", @($schemaValue.Keys)) -cne "schema_version`ncontrols" -or
            [int64]$schemaValue['schema_version'] -ne 2) { throw 'Control schema is not canonical v2 data.' }
        $schemaControls = @($schemaValue['controls'])
        if ($schemaControls.Count -lt 1 -or [string]::Join("`n", $schemaControls) -cne [string]::Join("`n", @($schemaControls | Sort-Object -CaseSensitive)) -or
            @($schemaControls | Sort-Object -Unique -CaseSensitive).Count -ne $schemaControls.Count -or $schemaControls -notcontains [string]$binding.Value['control_schema_path']) {
            throw 'Control schema paths are not exact self-protecting ordinal data.'
        }
        foreach ($row in @($binding.Value['control_hashes'])) {
            if ($row -isnot [Collections.IDictionary] -or [string]::Join("`n", @($row.Keys)) -cne "path`nsha256" -or -not ($schemaControls -contains [string]$row['path'])) { throw 'Binding v2 control-hash row mismatch.' }
            Assert-LowerHex ([string]$row['sha256']) 64 'binding control hash'
            if ($controlHashes.Contains([string]$row['path'])) { throw 'Binding v2 has duplicate control hash path.' }
            $controlHashes[[string]$row['path']] = [string]$row['sha256']
        }
        if ([string]::Join("`n", @($controlHashes.Keys)) -cne [string]::Join("`n", $schemaControls)) { throw 'Binding v2 control hashes do not exactly follow the schema.' }
        $controlBindingFields = [ordered]@{}
        foreach ($path in $schemaControls) { $controlBindingFields[$path] = $path }
    }
    if ($controlRows.Count -ne $controlBindingFields.Count) { throw 'Manifest controls set is not exact.' }
    $actualControlPaths = [Collections.Generic.List[string]]::new()
    foreach ($control in $controlRows) {
        if ($control -isnot [Collections.IDictionary] -or [string]::Join("`n", @($control.Keys)) -cne "path`nbytes`nsha256") { throw 'Manifest control-row schema mismatch.' }
        Assert-JsonInt64Token $control['bytes'] 'manifest control bytes'
        $controlPath = [string]$control['path']
        $actualControlPaths.Add($controlPath)
        if (-not $controlBindingFields.Contains($controlPath) -or [string]$control['sha256'] -cne [string]$controlHashes[$controlPath]) { throw "Manifest/binding control hash mismatch: $controlPath" }
        $blobBytes = Get-GitBlobBytes $execution $controlPath
        if ($blobBytes.LongLength -ne [int64]$control['bytes'] -or (Get-Sha256Bytes $blobBytes) -cne [string]$control['sha256']) { throw "Execution-baseline control blob mismatch: $controlPath" }
    }
    if ([string]::Join("`n", $actualControlPaths) -cne [string]::Join("`n", @($controlBindingFields.Keys | Sort-Object))) { throw 'Manifest controls are not ordinal sorted.' }
    $selfRelative = 'scripts/remediation/update-integration-ref.ps1'
    $selfExpected = [string]$controlHashes['scripts/remediation/update-integration-ref.ps1']
    Assert-LowerHex $selfExpected 64 'committed helper sha256'
    $selfCommittedBytes = Get-GitBlobBytes $execution $selfRelative
    $selfPathBefore = Assert-SafeExistingPath -Path $PSCommandPath -LeafType File
    if ($selfPathBefore.Identity -cne $scriptPathRecord.Identity -or $selfPathBefore.Owner -cne $scriptPathRecord.Owner -or $selfPathBefore.AclSha256 -cne $scriptPathRecord.AclSha256) {
        throw 'Running integration-ref helper changed after canonical path validation.'
    }
    $selfWorkingBytes = [IO.File]::ReadAllBytes($PSCommandPath)
    $selfPathAfter = Assert-SafeExistingPath -Path $PSCommandPath -LeafType File
    if ($selfPathBefore.Identity -cne $selfPathAfter.Identity -or $selfPathBefore.Owner -cne $selfPathAfter.Owner -or $selfPathBefore.AclSha256 -cne $selfPathAfter.AclSha256) {
        throw 'Running integration-ref helper identity/owner/ACL changed during raw-byte verification.'
    }
    $attributesBytes = Get-GitBlobBytes $execution '.gitattributes'
    $attributesText = $script:Utf8.GetString($attributesBytes)
    if (@($attributesText.Split("`n") | Where-Object { $_ -ceq '*.ps1 text eol=crlf' }).Count -ne 1) {
        throw 'Execution-baseline .gitattributes lacks the exact deterministic PowerShell CRLF rule.'
    }
    $selfCommittedCrlfBytes = Convert-LfBlobToDeterministicCrlf $selfCommittedBytes
    if (-not (Test-BytesEqual $selfWorkingBytes $selfCommittedBytes) -and -not (Test-BytesEqual $selfWorkingBytes $selfCommittedCrlfBytes)) {
        throw 'Running integration-ref helper raw bytes differ from its execution-baseline blob under deterministic LF/CRLF checkout semantics.'
    }
    $selfActual = Get-Sha256Bytes $selfCommittedBytes
    if ($selfActual -cne $selfExpected) { throw 'Committed integration-ref helper hash mismatch.' }

    $preparedHash = [string]$binding.Value['bundle_prepared_row_sha256']
    Assert-LowerHex $preparedHash 64 'bundle prepared publication row sha256'
    $publicationRoot = Join-Path $CommonDirectory "dynamo-remediation/publication-state-v1/$execution/$planSet"
    Assert-SafeExistingPath -Path $publicationRoot -LeafType Directory | Out-Null
    Assert-RestrictiveDirectory $publicationRoot
    $publicationRowKeys = @('schema_version','seq','phase','execution_baseline','plan_set_sha256','attempt_id','generation','lease_sha256','evidence_root_identity_sha256','manifest_sha256','manifest_bytes','bundle_prepared_row_sha256','binding_sha256','utc','prev_row_sha256','row_sha256')
    $publicationRows = Join-Path $publicationRoot 'journal/rows'
    Assert-SafeExistingPath -Path $publicationRows -LeafType Directory | Out-Null
    Assert-RestrictiveDirectory $publicationRows
    $rowLeaves = @(Get-ChildItem -LiteralPath $publicationRows -File -Force | Sort-Object Name)
    $expectedNames = @('00000000000000000001-bundle-prepared.json','00000000000000000002-binding-committed.json')
    if ([string]::Join("`n", @($rowLeaves.Name)) -cne [string]::Join("`n", $expectedNames)) { throw 'Publication must contain exactly the two fixed final rows.' }
    $prepared = Read-CanonicalJsonFile -Path $rowLeaves[0].FullName -ExpectedKeys $publicationRowKeys
    $committed = Read-CanonicalJsonFile -Path $rowLeaves[1].FullName -ExpectedKeys $publicationRowKeys
    foreach ($record in @($prepared,$committed)) {
        $rowPreimage = [ordered]@{}
        foreach ($key in $publicationRowKeys[0..($publicationRowKeys.Count - 2)]) { $rowPreimage[$key] = $record.Value[$key] }
        if ((Get-Sha256Text 'dynamo-publication-row-v1' (ConvertTo-CanonicalBytes $rowPreimage)) -cne [string]$record.Value['row_sha256']) { throw 'Publication row hash mismatch.' }
        foreach ($numericField in @('schema_version','seq','generation','manifest_bytes')) { Assert-JsonInt64Token $record.Value[$numericField] "publication row $numericField" }
        if ([string]$record.Value['execution_baseline'] -cne $execution -or [string]$record.Value['plan_set_sha256'] -cne $planSet) { throw 'Publication row plan binding mismatch.' }
        if ([int64]$record.Value['schema_version'] -ne 1 -or [string]$record.Value['attempt_id'] -cnotmatch '^[0-9a-f]{32}$' -or [int64]$record.Value['generation'] -lt 1) { throw 'Publication row scalar schema mismatch.' }
        if ([string]$record.Value['manifest_sha256'] -cne $manifestHash -or [int64]$record.Value['manifest_bytes'] -ne $manifest.Bytes.LongLength) { throw 'Publication row manifest binding mismatch.' }
        foreach ($field in @('lease_sha256','evidence_root_identity_sha256','manifest_sha256','bundle_prepared_row_sha256','binding_sha256','prev_row_sha256','row_sha256')) { Assert-LowerHex ([string]$record.Value[$field]) 64 "publication row $field" }
        Assert-CanonicalUtcTimestamp $record.Value['utc'] 'publication row utc'
    }
    if ([int64]$prepared.Value['seq'] -ne 1 -or [string]$prepared.Value['phase'] -cne 'BundlePrepared' -or
        [string]$prepared.Value['prev_row_sha256'] -cne $script:ZeroSha256 -or [string]$prepared.Value['row_sha256'] -cne $preparedHash -or
        [string]$prepared.Value['bundle_prepared_row_sha256'] -cne $script:ZeroSha256 -or [string]$prepared.Value['binding_sha256'] -cne $script:ZeroSha256) { throw 'BundlePrepared row is not exact.' }
    if ([int64]$committed.Value['seq'] -ne 2 -or [string]$committed.Value['phase'] -cne 'BindingCommitted' -or
        [string]$committed.Value['prev_row_sha256'] -cne $preparedHash -or [string]$committed.Value['bundle_prepared_row_sha256'] -cne $preparedHash -or
        [string]$committed.Value['binding_sha256'] -cne $bindingHash -or [int64]$committed.Value['generation'] -lt [int64]$prepared.Value['generation']) {
        throw 'BindingCommitted row does not close the prepared publication.'
    }
    foreach ($field in @('execution_baseline','plan_set_sha256','attempt_id','evidence_root_identity_sha256','manifest_sha256','manifest_bytes')) {
        if ([string]$prepared.Value[$field] -cne [string]$committed.Value[$field]) { throw "Publication rows disagree at $field" }
    }
    $activePublication = Join-Path $publicationRoot 'active-publication.lock'
    if (Test-Path -LiteralPath $activePublication) { throw 'Active publication lease remains after BindingCommitted.' }
    $publicationLeases = Join-Path $publicationRoot 'leases'
    Assert-SafeExistingPath -Path $publicationLeases -LeafType Directory | Out-Null
    Assert-RestrictiveDirectory $publicationLeases
    $publicationLeaseArchive = Join-Path $publicationLeases 'archives'
    Assert-SafeExistingPath -Path $publicationLeaseArchive -LeafType Directory | Out-Null
    Assert-RestrictiveDirectory $publicationLeaseArchive
    $closedPattern = ("closed-publication.{0}.g{1:D10}.{2}.lock" -f [string]$committed.Value['attempt_id'], [int64]$committed.Value['generation'], [string]$committed.Value['lease_sha256'])
    $closedLeaves = @(Get-ChildItem -LiteralPath $publicationLeases -File -Force | Where-Object { $_.Name -eq $closedPattern })
    if ($closedLeaves.Count -ne 1) { throw 'Exact closed publication lease is missing.' }
    $publicationLeaseKeys = @('schema_version','execution_baseline','plan_set_sha256','attempt_id','generation','evidence_root_native_path','evidence_root_identity_sha256','owner','source_set_sha256','canonical_core_sha256','expected_tail_sha256','prior_lease_sha256','created_at','lease_sha256')
    $leaseRecords = [Collections.Generic.List[object]]::new()
    $archivedPublicationLeaseRecords = [Collections.Generic.List[object]]::new()
    foreach ($archivedLeaf in @(Get-ChildItem -LiteralPath $publicationLeaseArchive -File -Force | Sort-Object Name)) {
        $archivedRecord = Read-CanonicalJsonFile -Path $archivedLeaf.FullName -ExpectedKeys $publicationLeaseKeys
        $archivedPublicationLeaseRecords.Add($archivedRecord)
        $leaseRecords.Add($archivedRecord)
    }
    $closedPublicationLease = Read-CanonicalJsonFile -Path $closedLeaves[0].FullName -ExpectedKeys $publicationLeaseKeys
    $leaseRecords.Add($closedPublicationLease)
    $priorLeaseSha256 = $script:ZeroSha256
    [int64]$expectedLeaseGeneration = 1
    $leasesByGeneration = @{}
    $leaseInvariant = $null
    foreach ($leaseRecord in $leaseRecords) {
        $leasePreimage = [ordered]@{}
        foreach ($key in $publicationLeaseKeys[0..($publicationLeaseKeys.Count - 2)]) { $leasePreimage[$key] = $leaseRecord.Value[$key] }
        if ((Get-Sha256Text 'dynamo-publication-lease-v1' (ConvertTo-CanonicalBytes $leasePreimage)) -cne [string]$leaseRecord.Value['lease_sha256']) { throw 'Publication lease hash mismatch.' }
        Assert-JsonInt64Token $leaseRecord.Value['schema_version'] 'publication lease schema_version'
        Assert-JsonInt64Token $leaseRecord.Value['generation'] 'publication lease generation'
        if ([int64]$leaseRecord.Value['schema_version'] -ne 1 -or [int64]$leaseRecord.Value['generation'] -lt 1) { throw 'Publication lease scalar schema mismatch.' }
        Assert-OwnerSchema $leaseRecord.Value['owner'] ([string]$leaseRecord.Value['attempt_id'])
        if ([string]$leaseRecord.Value['owner']['git_common_dir_identity_sha256'] -cne $commonRecord.IdentitySha256) { throw 'Publication lease owner common-directory identity mismatch.' }
        foreach ($hashField in @('evidence_root_identity_sha256','source_set_sha256','canonical_core_sha256','expected_tail_sha256','prior_lease_sha256','lease_sha256')) {
            Assert-LowerHex ([string]$leaseRecord.Value[$hashField]) 64 "publication lease $hashField"
        }
        Assert-CanonicalUtcTimestamp $leaseRecord.Value['created_at'] 'publication lease created_at'
        if ([string]$leaseRecord.Value['source_set_sha256'] -cne $sourceSetSha256 -or
            [string]$leaseRecord.Value['canonical_core_sha256'] -cne $canonicalCoreSha256) {
            throw 'Publication lease source-set/raw-core digest mismatch.'
        }
        $currentInvariant = [string]::Join("`n", @([string]$leaseRecord.Value['evidence_root_native_path'],[string]$leaseRecord.Value['evidence_root_identity_sha256'],[string]$leaseRecord.Value['source_set_sha256'],[string]$leaseRecord.Value['canonical_core_sha256']))
        if ($null -eq $leaseInvariant) { $leaseInvariant = $currentInvariant } elseif ($currentInvariant -cne $leaseInvariant) { throw 'Publication lease immutable inputs changed across generations.' }
        if ([string]$leaseRecord.Value['execution_baseline'] -cne $execution -or [string]$leaseRecord.Value['plan_set_sha256'] -cne $planSet -or
            [string]$leaseRecord.Value['attempt_id'] -cne [string]$committed.Value['attempt_id'] -or [int64]$leaseRecord.Value['generation'] -ne $expectedLeaseGeneration -or
            [string]$leaseRecord.Value['prior_lease_sha256'] -cne $priorLeaseSha256 -or
            [string]$leaseRecord.Value['evidence_root_identity_sha256'] -cne [string]$committed.Value['evidence_root_identity_sha256']) {
            throw 'Publication lease generation chain is not exact.'
        }
        $expectedPublicationTail = if ($expectedLeaseGeneration -le [int64]$prepared.Value['generation']) { $script:ZeroSha256 } else { $preparedHash }
        if ([string]$leaseRecord.Value['expected_tail_sha256'] -cne $expectedPublicationTail) { throw 'Publication lease generation binds the wrong durable row tail.' }
        $expectedName = if ($expectedLeaseGeneration -eq [int64]$committed.Value['generation']) {
            "closed-publication.$([string]$leaseRecord.Value['attempt_id']).g$($expectedLeaseGeneration.ToString('D10')).$([string]$leaseRecord.Value['lease_sha256']).lock"
        } else {
            "publication-lease.$([string]$leaseRecord.Value['attempt_id']).g$($expectedLeaseGeneration.ToString('D10')).$([string]$leaseRecord.Value['lease_sha256']).lock"
        }
        if ([IO.Path]::GetFileName($leaseRecord.Path) -cne $expectedName) { throw 'Publication lease filename/generation/hash mismatch.' }
        $leasesByGeneration[$expectedLeaseGeneration] = $leaseRecord
        $priorLeaseSha256 = [string]$leaseRecord.Value['lease_sha256']
        $expectedLeaseGeneration++
    }
    if ($leaseRecords.Count -ne [int64]$committed.Value['generation']) { throw 'Publication lease archives are not a contiguous complete chain.' }
    foreach ($archivedPublicationLease in $archivedPublicationLeaseRecords) {
        Assert-DeadOwnerForCommonIdentity -Owner $archivedPublicationLease.Value['owner'] -CommonIdentitySha256 $commonRecord.IdentitySha256 -Label 'Archived publication lease'
    }
    if (-not $leasesByGeneration.ContainsKey([int64]$prepared.Value['generation']) -or
        [string]$leasesByGeneration[[int64]$prepared.Value['generation']].Value['lease_sha256'] -cne [string]$prepared.Value['lease_sha256']) {
        throw 'BundlePrepared does not bind its publication lease generation.'
    }
    if ([string]$closedPublicationLease.Value['execution_baseline'] -cne $execution -or [string]$closedPublicationLease.Value['plan_set_sha256'] -cne $planSet -or
        [string]$closedPublicationLease.Value['attempt_id'] -cne [string]$committed.Value['attempt_id'] -or
        [int64]$closedPublicationLease.Value['generation'] -ne [int64]$committed.Value['generation'] -or
        [string]$closedPublicationLease.Value['lease_sha256'] -cne [string]$committed.Value['lease_sha256']) {
        throw 'Closed publication lease does not bind BindingCommitted.'
    }
    $evidenceRootRecord = Get-PathRecord ([string]$closedPublicationLease.Value['evidence_root_native_path'])
    if ($evidenceRootRecord.IdentitySha256 -cne [string]$closedPublicationLease.Value['evidence_root_identity_sha256']) { throw 'Publication evidence-root native identity changed.' }
    $expectedManifestPath = [IO.Path]::GetFullPath((Join-Path $evidenceRootRecord.Path "Dynamo/plan-set/$execution/$planSet/plan-set-manifest-v1.json"))
    if ($manifestPath -cne $expectedManifestPath) { throw 'Binding manifest path is not the exact path derived from the closed publication lease evidence root.' }
    $evidencePrefix = $evidenceRootRecord.Path.TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar
    if (-not $manifestPath.StartsWith($evidencePrefix, [StringComparison]::Ordinal)) { throw 'Bound manifest escaped its publication evidence root.' }
    $allowedOrphans = [Collections.Generic.List[string]]::new()
    foreach ($tmpRelative in @('journal/tmp','binding-tmp')) {
        $tmpDirectory = Join-Path $publicationRoot $tmpRelative
        if (Test-Path -LiteralPath $tmpDirectory) {
            Assert-SafeExistingPath -Path $tmpDirectory -LeafType Directory | Out-Null
            Assert-RestrictiveDirectory $tmpDirectory
            if (@(Get-ChildItem -LiteralPath $tmpDirectory -Force).Count -ne 0) { throw "Unarchived publication temp remains: $tmpRelative" }
        }
    }
    $rowOrphanDirectory = Join-Path $publicationRoot 'journal/orphans'
    if (Test-Path -LiteralPath $rowOrphanDirectory) {
        Assert-SafeExistingPath -Path $rowOrphanDirectory -LeafType Directory | Out-Null
        Assert-RestrictiveDirectory $rowOrphanDirectory
        foreach ($orphan in Get-ChildItem -LiteralPath $rowOrphanDirectory -Force) {
            if ($orphan.PSIsContainer -or ($orphan.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Publication row orphan directory/reparse entry is forbidden.' }
            $orphanMatch = [regex]::Match($orphan.Name, '^(?<attempt>[0-9a-f]{32})\.g(?<generation>[0-9]{10})\.(?<sequence>[0-9]{20})\.(?<slug>bundle-prepared|binding-committed)\.(?<nonce>[0-9a-f]{32})\.(?<hash>[0-9a-f]{64})\.orphan$')
            if (-not $orphanMatch.Success) { throw 'Publication row orphan filename mismatch.' }
            $orphanRecord = Read-CanonicalJsonFile -Path $orphan.FullName -ExpectedKeys $publicationRowKeys
            if ($orphanRecord.Sha256 -cne $orphanMatch.Groups['hash'].Value) { throw 'Publication row orphan raw hash mismatch.' }
            $orphanValue = $orphanRecord.Value
            $orphanPreimage = [ordered]@{}
            foreach ($key in $publicationRowKeys[0..($publicationRowKeys.Count - 2)]) { $orphanPreimage[$key] = $orphanValue[$key] }
            if ((Get-Sha256Text 'dynamo-publication-row-v1' (ConvertTo-CanonicalBytes $orphanPreimage)) -cne [string]$orphanValue['row_sha256']) { throw 'Publication row orphan self-hash mismatch.' }
            foreach ($numericField in @('schema_version','seq','generation','manifest_bytes')) { Assert-JsonInt64Token $orphanValue[$numericField] "publication row orphan $numericField" }
            Assert-CanonicalUtcTimestamp $orphanValue['utc'] 'publication row orphan utc'
            $expectedOrphanPhase = if ($orphanMatch.Groups['slug'].Value -ceq 'bundle-prepared') { 'BundlePrepared' } else { 'BindingCommitted' }
            if ([int64]$orphanValue['schema_version'] -ne 1 -or
                [string]$orphanValue['attempt_id'] -cne $orphanMatch.Groups['attempt'].Value -or
                [int64]$orphanValue['generation'] -ne [int64]::Parse($orphanMatch.Groups['generation'].Value, [Globalization.CultureInfo]::InvariantCulture) -or
                [int64]$orphanValue['seq'] -ne [int64]::Parse($orphanMatch.Groups['sequence'].Value, [Globalization.CultureInfo]::InvariantCulture) -or
                [string]$orphanValue['phase'] -cne $expectedOrphanPhase -or
                [string]$orphanValue['execution_baseline'] -cne $execution -or [string]$orphanValue['plan_set_sha256'] -cne $planSet -or
                [string]$orphanValue['attempt_id'] -cne [string]$committed.Value['attempt_id'] -or
                [string]$orphanValue['evidence_root_identity_sha256'] -cne [string]$committed.Value['evidence_root_identity_sha256'] -or
                [string]$orphanValue['manifest_sha256'] -cne $manifestHash -or [int64]$orphanValue['manifest_bytes'] -ne $manifest.Bytes.LongLength) {
                throw 'Publication row orphan canonical tuple/binding mismatch.'
            }
            foreach ($field in @('lease_sha256','evidence_root_identity_sha256','manifest_sha256','bundle_prepared_row_sha256','binding_sha256','prev_row_sha256','row_sha256')) {
                Assert-LowerHex ([string]$orphanValue[$field]) 64 "publication row orphan $field"
            }
            $orphanLeaseMatches = @($archivedPublicationLeaseRecords | Where-Object {
                [string]$_.Value['attempt_id'] -ceq [string]$orphanValue['attempt_id'] -and
                [int64]$_.Value['generation'] -eq [int64]$orphanValue['generation'] -and
                [string]$_.Value['lease_sha256'] -ceq [string]$orphanValue['lease_sha256']
            })
            if ($orphanLeaseMatches.Count -ne 1) { throw 'Publication row orphan is not bound to exactly one archived dead-owner lease.' }
            if ($expectedOrphanPhase -ceq 'BundlePrepared') {
                if ([int64]$orphanValue['seq'] -ne 1 -or [string]$orphanValue['prev_row_sha256'] -cne $script:ZeroSha256 -or
                    [string]$orphanValue['bundle_prepared_row_sha256'] -cne $script:ZeroSha256 -or [string]$orphanValue['binding_sha256'] -cne $script:ZeroSha256) {
                    throw 'Publication BundlePrepared orphan invariants failed.'
                }
            } elseif ([int64]$orphanValue['seq'] -ne 2 -or [string]$orphanValue['prev_row_sha256'] -cne $preparedHash -or
                [string]$orphanValue['bundle_prepared_row_sha256'] -cne $preparedHash -or [string]$orphanValue['binding_sha256'] -cne $bindingHash) {
                throw 'Publication BindingCommitted orphan invariants failed.'
            }
            $allowedOrphans.Add($orphan.FullName)
        }
    }
    $bindingOrphanDirectory = Join-Path $publicationRoot 'binding-orphans'
    if (Test-Path -LiteralPath $bindingOrphanDirectory) {
        Assert-SafeExistingPath -Path $bindingOrphanDirectory -LeafType Directory | Out-Null
        Assert-RestrictiveDirectory $bindingOrphanDirectory
        foreach ($orphan in Get-ChildItem -LiteralPath $bindingOrphanDirectory -Force) {
            if ($orphan.PSIsContainer -or ($orphan.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Binding orphan directory/reparse entry is forbidden.' }
            $bindingOrphanMatch = [regex]::Match($orphan.Name, '^[0-9a-f]{32}\.binding\.(?<hash>[0-9a-f]{64})\.orphan$')
            if (-not $bindingOrphanMatch.Success) { throw 'Binding orphan filename mismatch.' }
            $bindingOrphan = Read-CanonicalJsonFile -Path $orphan.FullName -ExpectedKeys $bindingKeys
            if ($bindingOrphan.Sha256 -cne $bindingOrphanMatch.Groups['hash'].Value -or -not (Test-BytesEqual $bindingOrphan.Bytes $binding.Bytes)) {
                throw 'Binding orphan must be a byte-identical canonical copy of the fixed binding.'
            }
            $allowedOrphans.Add($orphan.FullName)
        }
    }
    $publicationItems = @(Get-ChildItem -LiteralPath $publicationRoot -Recurse -Force)
    if (@($publicationItems | Where-Object { ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 }).Count -ne 0) { throw 'Publication state contains a reparse point.' }
    $expectedPublicationDirectories = @(
        (Join-Path $publicationRoot 'binding-orphans'),
        (Join-Path $publicationRoot 'binding-tmp'),
        (Join-Path $publicationRoot 'journal'),
        (Join-Path $publicationRoot 'journal/orphans'),
        (Join-Path $publicationRoot 'journal/rows'),
        (Join-Path $publicationRoot 'journal/tmp'),
        (Join-Path $publicationRoot 'leases'),
        (Join-Path $publicationRoot 'leases/archives')
    ) | ForEach-Object { [IO.Path]::GetFullPath($_) } | Sort-Object
    $actualPublicationDirectories = @($publicationItems | Where-Object PSIsContainer | ForEach-Object FullName | Sort-Object)
    if ([string]::Join("`n", $actualPublicationDirectories) -cne [string]::Join("`n", $expectedPublicationDirectories)) {
        throw 'Publication state directory layout is not exact.'
    }
    foreach ($publicationDirectory in $actualPublicationDirectories) { Assert-RestrictiveDirectory $publicationDirectory }
    $allPublicationLeaves = @($publicationItems | Where-Object { -not $_.PSIsContainer })
    $allowedPublicationLeaves = @(@($rowLeaves[0].FullName,$rowLeaves[1].FullName) + @($leaseRecords.Path) + @($allowedOrphans)) | Sort-Object
    if ([string]::Join("`n", @($allPublicationLeaves.FullName | Sort-Object)) -cne [string]::Join("`n", $allowedPublicationLeaves)) {
        throw 'Unexpected publication row, temp, archive, or lease leaf remains.'
    }

    [pscustomobject]@{
        ExecutionBaseline = $execution
        PlanSetSha256 = $planSet
        BindingSha256 = $bindingHash
        PublicationTailSha256 = [string]$committed.Value['row_sha256']
        CommonDirectoryRecord = $commonRecord
    }
}

function Get-MachineIdentitySha256 {
    $material = if ($IsWindows) {
        try { (Get-ItemPropertyValue -LiteralPath 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Cryptography' -Name MachineGuid -ErrorAction Stop).ToString().ToLowerInvariant() }
        catch { throw "Machine identity is unavailable: $($_.Exception.Message)" }
    } else {
        $path = if (Test-Path -LiteralPath '/etc/machine-id') { '/etc/machine-id' } else { '/var/lib/dbus/machine-id' }
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw 'Machine identity is unavailable.' }
        [IO.File]::ReadAllText($path).Trim().ToLowerInvariant()
    }
    Get-Sha256Text 'dynamo-machine-identity-v1' ($script:Utf8.GetBytes($material))
}

function Get-ProcessStartIdentity([int] $ProcessId) {
    if ($script:TestControlsValidated -and
        [Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_TEST_PROCESS_QUERY_FAILURE', 'Process') -ceq 'AccessDenied' -and
        [Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_TEST_PROCESS_QUERY_FAILURE_PID', 'Process') -ceq $ProcessId.ToString([Globalization.CultureInfo]::InvariantCulture)) {
        throw "Process birth identity cannot be queried unambiguously for PID ${ProcessId}: injected access denied."
    }
    try {
        $process = [Diagnostics.Process]::GetProcessById($ProcessId)
        try {
            $raw = $process.StartTime.ToUniversalTime().ToFileTimeUtc().ToString([Globalization.CultureInfo]::InvariantCulture)
            return Get-Sha256Text 'dynamo-process-birth-v1' ($script:Utf8.GetBytes($raw))
        }
        finally { $process.Dispose() }
    } catch [ArgumentException] { return $null }
    catch { throw "Process birth identity cannot be queried unambiguously for PID ${ProcessId}: $($_.Exception.Message)" }
}

function New-Owner([string] $AttemptId) {
    [ordered]@{
        machine_identity_sha256 = Get-MachineIdentitySha256
        pid = [int64]$PID
        process_start_identity = Get-ProcessStartIdentity $PID
        attempt_id = $AttemptId
        git_common_dir_identity_sha256 = $script:Binding.CommonDirectoryRecord.IdentitySha256
    }
}

function Assert-OwnerSchema([Collections.IDictionary] $Owner, [string] $ExpectedAttemptId) {
    $expected = @('machine_identity_sha256','pid','process_start_identity','attempt_id','git_common_dir_identity_sha256')
    if ([string]::Join("`n", @($Owner.Keys)) -cne [string]::Join("`n", $expected)) { throw 'Owner key order/schema mismatch.' }
    Assert-LowerHex ([string]$Owner['machine_identity_sha256']) 64 'owner machine identity'
    Assert-LowerHex ([string]$Owner['process_start_identity']) 64 'owner process birth identity'
    Assert-LowerHex ([string]$Owner['git_common_dir_identity_sha256']) 64 'owner common-directory identity'
    Assert-JsonInt64Token $Owner['pid'] 'owner pid'
    if ([int64]$Owner['pid'] -le 0 -or [int64]$Owner['pid'] -gt [int]::MaxValue) { throw 'Owner PID must be a positive platform PID.' }
    if ([string]$Owner['attempt_id'] -cne $ExpectedAttemptId) { throw 'Owner attempt ID mismatch.' }
}

function Assert-OwnerDead([Collections.IDictionary] $Owner) {
    foreach ($key in @('machine_identity_sha256','pid','process_start_identity','attempt_id','git_common_dir_identity_sha256')) {
        if (-not $Owner.Contains($key)) { throw "Owner is missing $key" }
    }
    if ([string]$Owner['machine_identity_sha256'] -cne (Get-MachineIdentitySha256)) { throw 'Remote-machine ownership is ambiguous and cannot be recovered.' }
    if ([string]$Owner['git_common_dir_identity_sha256'] -cne $script:Binding.CommonDirectoryRecord.IdentitySha256) { throw 'Owner common-directory identity mismatch.' }
    $currentStart = Get-ProcessStartIdentity ([int]$Owner['pid'])
    if ($null -eq $currentStart) { return }
    if ($currentStart -ceq [string]$Owner['process_start_identity']) { throw 'Recorded owner process is still alive.' }
    # A live PID with a different platform birth identity is conclusive PID reuse.
}

function New-LeaseValue([string] $AttemptId, [string] $WaveName, [string] $UnitName, [string] $Old, [string] $New, [string] $Microplan, [Collections.IDictionary] $Owner, [int64] $ExpectedSeq, [string] $Tail) {
    $lease = [ordered]@{
        schema_version = 1
        execution_baseline = $script:Binding.ExecutionBaseline
        plan_set_sha256 = $script:Binding.PlanSetSha256
        wave = $WaveName
        ref_name = $script:RefName
        unit = $UnitName
        attempt_id = $AttemptId
        old_tip = $Old
        new_tip = $New
        microplan_sha256 = $Microplan
        owner = $Owner
        expected_seq = $ExpectedSeq
        expected_tail_sha256 = $Tail
        created_at = Get-UtcNowCanonical
    }
    $lease['lease_sha256'] = Get-Sha256Text 'dynamo-integration-lease-v1' (ConvertTo-CanonicalBytes $lease)
    $lease
}

function Read-Lease([string] $Path) {
    $keys = @('schema_version','execution_baseline','plan_set_sha256','wave','ref_name','unit','attempt_id','old_tip','new_tip','microplan_sha256','owner','expected_seq','expected_tail_sha256','created_at','lease_sha256')
    $record = Read-CanonicalJsonFile -Path $Path -ExpectedKeys $keys
    $preimage = [ordered]@{}
    foreach ($key in $keys[0..($keys.Count - 2)]) { $preimage[$key] = $record.Value[$key] }
    $actual = Get-Sha256Text 'dynamo-integration-lease-v1' (ConvertTo-CanonicalBytes $preimage)
    if ($actual -cne [string]$record.Value['lease_sha256']) { throw 'Lease hash mismatch.' }
    Assert-JsonInt64Token $record.Value['schema_version'] 'lease schema_version'
    Assert-JsonInt64Token $record.Value['expected_seq'] 'lease expected_seq'
    if ([int64]$record.Value['schema_version'] -ne 1) { throw 'Lease schema version mismatch.' }
    if ([string]$record.Value['wave'] -cne $Wave) { throw 'Lease wave mismatch.' }
    if ([string]$record.Value['execution_baseline'] -cne $script:Binding.ExecutionBaseline -or
        [string]$record.Value['plan_set_sha256'] -cne $script:Binding.PlanSetSha256 -or
        [string]$record.Value['ref_name'] -cne $script:RefName) { throw 'Lease fixed binding/ref mismatch.' }
    if ([string]$record.Value['attempt_id'] -cnotmatch '^[0-9a-f]{32}$') { throw 'Lease attempt ID is not canonical Guid N.' }
    Assert-OwnerSchema $record.Value['owner'] ([string]$record.Value['attempt_id'])
    if ([string]$record.Value['owner']['git_common_dir_identity_sha256'] -cne $script:Binding.CommonDirectoryRecord.IdentitySha256) { throw 'Lease owner common-directory identity does not match the immutable binding.' }
    if ([int64]$record.Value['expected_seq'] -lt 1) { throw 'Lease expected_seq must be positive.' }
    Assert-CanonicalUtcTimestamp $record.Value['created_at'] 'lease created_at'
    Assert-LowerHex ([string]$record.Value['old_tip']) 40 'lease old_tip'
    Assert-LowerHex ([string]$record.Value['new_tip']) 40 'lease new_tip'
    Assert-LowerHex ([string]$record.Value['microplan_sha256']) 64 'lease microplan_sha256'
    Assert-LowerHex ([string]$record.Value['expected_tail_sha256']) 64 'lease expected_tail_sha256'
    Assert-PersistedOperationTuple -UnitName ([string]$record.Value['unit']) -Old ([string]$record.Value['old_tip']) -New ([string]$record.Value['new_tip']) -Label 'Lease'
    $record
}

function Get-RefTip {
    $symbolic = Invoke-Git -Arguments @('symbolic-ref','-q',$script:RefName) -AllowFailure
    if ($symbolic.ExitCode -eq 0) { throw "Symbolic remediation ref is forbidden: $script:RefName" }
    if ($symbolic.ExitCode -ne 1) { throw "Could not classify remediation ref as direct or missing: $($symbolic.Stderr)" }
    $classification = Invoke-Git -Arguments @('show-ref','--exists',$script:RefName) -AllowFailure
    if ($classification.ExitCode -eq 2) { return $null }
    if ($classification.ExitCode -ne 0) { throw "Could not classify remediation ref existence: $($classification.Stderr)" }
    $result = Invoke-Git -Arguments @('show-ref','--verify','--hash',$script:RefName) -AllowFailure
    if ($result.ExitCode -ne 0) { throw "Could not read existing remediation ref: $($result.Stderr)" }
    if ($result.Stdout -cnotmatch '^[0-9a-f]{40}$') { throw 'Remediation ref is malformed.' }
    $result.Stdout
}

function Read-Journal {
    $rowKeys = @('schema_version','seq','phase','wave','unit','attempt_id','old_tip','new_tip','microplan_sha256','lease_sha256','recovery_generation','recovery_claim_sha256','utc','prev_row_sha256','row_sha256')
    $rows = [Collections.Generic.List[object]]::new()
    $rowItems = @(Get-ChildItem -LiteralPath $script:RowsDirectory -Force)
    if (@($rowItems | Where-Object { $_.PSIsContainer -or ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 }).Count -ne 0) { throw 'Journal rows contains an unexpected directory/reparse leaf.' }
    $files = @($rowItems | Sort-Object Name)
    [int64]$expectedSeq = 1
    $previous = $script:ZeroSha256
    $attempts = @{}
    foreach ($file in $files) {
        if ($file.Name -cnotmatch '^(?<seq>[0-9]{20})-(?<phase>intent|committed|recovered-committed|aborted-no-ref-change)\.json$') { throw "Unexpected journal leaf: $($file.Name)" }
        $fileSequence = [int64]$Matches.seq
        $filePhase = switch ($Matches.phase) {
            'intent' { 'Intent' }
            'committed' { 'Committed' }
            'recovered-committed' { 'RecoveredCommitted' }
            'aborted-no-ref-change' { 'AbortedNoRefChange' }
        }
        $record = Read-CanonicalJsonFile -Path $file.FullName -ExpectedKeys $rowKeys
        $value = $record.Value
        foreach ($numericField in @('schema_version','seq','recovery_generation')) { Assert-JsonInt64Token $value[$numericField] "journal row $numericField" }
        if ([int64]$value['seq'] -ne $expectedSeq -or $fileSequence -ne $expectedSeq) { throw 'Journal sequence gap or filename mismatch.' }
        if ([string]$value['phase'] -cne $filePhase) { throw 'Journal phase/filename mismatch.' }
        if ([string]$value['wave'] -cne $Wave) { throw 'Journal wave mismatch.' }
        if ([int64]$value['schema_version'] -ne 1) { throw 'Journal schema version mismatch.' }
        if ([string]$value['attempt_id'] -cnotmatch '^[0-9a-f]{32}$') { throw 'Journal attempt ID is not canonical Guid N.' }
        Assert-CanonicalUtcTimestamp $value['utc'] 'journal row utc'
        if ([string]$value['prev_row_sha256'] -cne $previous) { throw 'Broken journal previous-row hash.' }
        $preimage = [ordered]@{}
        foreach ($key in $rowKeys[0..($rowKeys.Count - 2)]) { $preimage[$key] = $value[$key] }
        $actualHash = Get-Sha256Text 'dynamo-integration-row-v1' (ConvertTo-CanonicalBytes $preimage)
        if ($actualHash -cne [string]$value['row_sha256']) { throw 'Journal row hash mismatch.' }
        foreach ($field in @('microplan_sha256','lease_sha256','recovery_claim_sha256','prev_row_sha256','row_sha256')) { Assert-LowerHex ([string]$value[$field]) 64 "row $field" }
        foreach ($field in @('old_tip','new_tip')) { Assert-LowerHex ([string]$value[$field]) 40 "row $field" }
        Assert-PersistedOperationTuple -UnitName ([string]$value['unit']) -Old ([string]$value['old_tip']) -New ([string]$value['new_tip']) -Label 'Journal row'
        if ([int64]$value['recovery_generation'] -eq 0) {
            if ([string]$value['recovery_claim_sha256'] -cne $script:ZeroSha256) { throw 'Normal row must use the zero recovery claim hash.' }
        } elseif ([int64]$value['recovery_generation'] -gt 0) {
            if ([string]$value['recovery_claim_sha256'] -ceq $script:ZeroSha256) { throw 'Recovery row must bind a nonzero claim hash.' }
        } else { throw 'Negative recovery generation is forbidden.' }
        if ([string]$value['phase'] -eq 'Committed' -and ([int64]$value['recovery_generation'] -ne 0 -or [string]$value['recovery_claim_sha256'] -cne $script:ZeroSha256)) {
            throw 'Committed is exclusively a generation-zero normal terminal.'
        }
        if ([string]$value['phase'] -in @('RecoveredCommitted','AbortedNoRefChange') -and [int64]$value['recovery_generation'] -le 0) {
            throw 'Recovery terminal must bind a positive recovery generation.'
        }
        $attempt = [string]$value['attempt_id']
        if (-not $attempts.ContainsKey($attempt)) { $attempts[$attempt] = [Collections.Generic.List[object]]::new() }
        $attempts[$attempt].Add($record)
        $rows.Add($record)
        $previous = [string]$value['row_sha256']
        $expectedSeq++
    }
    foreach ($attempt in $attempts.Keys) {
        $group = @($attempts[$attempt])
        $intents = @($group | Where-Object { $_.Value['phase'] -eq 'Intent' })
        $terminals = @($group | Where-Object { $_.Value['phase'] -ne 'Intent' })
        if ($intents.Count -gt 1 -or $terminals.Count -gt 1 -or ($terminals.Count -eq 1 -and $intents.Count -ne 1)) { throw "Duplicate or contradictory rows for attempt $attempt" }
        if ($terminals.Count -eq 1 -and [int64]$intents[0].Value['seq'] -ge [int64]$terminals[0].Value['seq']) { throw "Terminal precedes Intent for attempt $attempt" }
        if ($group.Count -eq 2) {
            foreach ($field in @('wave','unit','attempt_id','old_tip','new_tip','microplan_sha256','lease_sha256')) {
                if ([string]$group[0].Value[$field] -cne [string]$group[1].Value[$field]) { throw "Attempt tuple changed at $field" }
            }
        }
    }
    $pendingAttempts = @($attempts.Keys | Where-Object {
        $candidateRows = @($attempts[$_])
        $candidateRows.Count -eq 1 -and [string]$candidateRows[0].Value['phase'] -eq 'Intent'
    })
    if ($pendingAttempts.Count -gt 1) { throw 'Multiple pending journal attempts are forbidden.' }
    if ($pendingAttempts.Count -eq 1 -and [string]$rows[-1].Value['attempt_id'] -cne [string]$pendingAttempts[0]) { throw 'A pending attempt must be the journal tail.' }
    [pscustomobject]@{
        Rows = $rows.ToArray()
        Attempts = $attempts
        TailSha256 = $previous
        NextSeq = $expectedSeq
    }
}

function Assert-GlobalJournalRefContinuity([object] $Journal) {
    $rows = @($Journal.Rows)
    $observedTip = Get-RefTip
    $observedResult = if ($null -eq $observedTip) { $script:ZeroOid } else { [string]$observedTip }
    if ($rows.Count -eq 0) {
        if ($null -ne $observedTip) { throw 'An empty integration journal requires the authoritative ref to be absent.' }
        return
    }

    $expectedOldTip = $script:ZeroOid
    for ($index = 0; $index -lt $rows.Count; $index += 2) {
        $intent = $rows[$index]
        if ([string]$intent.Value['phase'] -cne 'Intent') { throw 'Global journal operation ordering requires an Intent at every operation boundary.' }
        if ($index -eq 0 -and
            ([string]$intent.Value['old_tip'] -cne $script:ZeroOid -or [string]$intent.Value['new_tip'] -cne $script:Binding.ExecutionBaseline)) {
            throw 'The first integration journal operation must anchor zero OID to the immutable execution baseline.'
        }
        if ([string]$intent.Value['old_tip'] -cne $expectedOldTip) { throw 'Integration journal old_tip does not continue from the prior terminal result.' }

        if ($index + 1 -ge $rows.Count) {
            if ($observedResult -cne [string]$intent.Value['old_tip'] -and $observedResult -cne [string]$intent.Value['new_tip']) {
                throw 'Pending Intent does not bracket the authoritative ref tip.'
            }
            return
        }

        $terminal = $rows[$index + 1]
        if ([string]$terminal.Value['phase'] -eq 'Intent' -or
            [string]$terminal.Value['attempt_id'] -cne [string]$intent.Value['attempt_id']) {
            throw 'Every non-tail Intent must be followed immediately by its terminal row.'
        }
        $expectedOldTip = if ([string]$terminal.Value['phase'] -in @('Committed','RecoveredCommitted')) {
            [string]$terminal.Value['new_tip']
        } else {
            [string]$terminal.Value['old_tip']
        }
    }

    if ($observedResult -cne $expectedOldTip) { throw 'Completed journal tail result contradicts the authoritative direct ref.' }
}

function New-RowValue([Collections.IDictionary] $Lease, [string] $Phase, [int64] $Generation, [string] $ClaimSha256, [object] $Journal) {
    $row = [ordered]@{
        schema_version = 1
        seq = [int64]$Journal.NextSeq
        phase = $Phase
        wave = [string]$Lease['wave']
        unit = [string]$Lease['unit']
        attempt_id = [string]$Lease['attempt_id']
        old_tip = [string]$Lease['old_tip']
        new_tip = [string]$Lease['new_tip']
        microplan_sha256 = [string]$Lease['microplan_sha256']
        lease_sha256 = [string]$Lease['lease_sha256']
        recovery_generation = $Generation
        recovery_claim_sha256 = $ClaimSha256
        utc = Get-UtcNowCanonical
        prev_row_sha256 = [string]$Journal.TailSha256
    }
    $row['row_sha256'] = Get-Sha256Text 'dynamo-integration-row-v1' (ConvertTo-CanonicalBytes $row)
    $row
}

function Add-JournalRow([Collections.IDictionary] $Row, [string] $FailpointAfterTemp, [string] $FailpointAfterRename) {
    $bytes = ConvertTo-CanonicalBytes $Row
    $seqText = ([int64]$Row['seq']).ToString('D20', [Globalization.CultureInfo]::InvariantCulture)
    $attempt = [string]$Row['attempt_id']
    $phase = [string]$Row['phase']
    $phaseSlug = switch ($phase) {
        'Intent' { 'intent' }
        'Committed' { 'committed' }
        'RecoveredCommitted' { 'recovered-committed' }
        'AbortedNoRefChange' { 'aborted-no-ref-change' }
        default { throw "Invalid row phase: $phase" }
    }
    $generationText = ([int64]$Row['recovery_generation']).ToString('D10', [Globalization.CultureInfo]::InvariantCulture)
    $nonce = [Guid]::NewGuid().ToString('N').ToLowerInvariant()
    $temp = Join-Path $script:TempDirectory "$attempt.g$generationText.$seqText.$phaseSlug.$nonce.tmp"
    $final = Join-Path $script:RowsDirectory "$seqText-$phaseSlug.json"
    Write-CreateNewDurable $temp $bytes
    Invoke-Failpoint $FailpointAfterTemp
    Move-NoReplaceDurable $temp $final
    Assert-SafeExistingPath -Path $script:RowsDirectory -LeafType Directory | Out-Null
    $readback = Read-CanonicalJsonFile -Path $final
    if ($readback.Sha256 -cne (Get-Sha256Bytes $bytes)) { throw 'Final row readback mismatch.' }
    Invoke-Failpoint $FailpointAfterRename
    Read-Journal
}

function Get-StateFingerprint {
    $parts = [Collections.Generic.List[string]]::new()
    $tip = Get-RefTip
    $parts.Add("ref=" + ($(if ($null -eq $tip) { '<missing>' } else { $tip })))
    $directories = @(
        $script:WaveRoot,
        (Split-Path -Parent $script:RowsDirectory),
        $script:RowsDirectory,
        $script:TempDirectory,
        $script:OrphanDirectory,
        $script:ClosedLeaseDirectory,
        (Split-Path -Parent $script:ClaimArchiveDirectory),
        $script:ClaimArchiveDirectory,
        $script:ClosedClaimDirectory
    ) | Select-Object -Unique
    foreach ($directory in $directories) {
        $directoryRecord = Assert-SafeExistingPath -Path $directory -LeafType Directory
        $parts.Add("dir=" + $directoryRecord.Path.Substring($script:WaveRoot.Length) + "|" + $directoryRecord.Identity + "|" + $directoryRecord.Owner + "|" + $directoryRecord.AclSha256)
    }
    foreach ($directory in @($script:RowsDirectory,$script:TempDirectory,$script:OrphanDirectory,$script:ClosedLeaseDirectory,$script:ClaimArchiveDirectory,$script:ClosedClaimDirectory)) {
        foreach ($file in @(Get-ChildItem -LiteralPath $directory -File -Force | Sort-Object Name)) {
            $record = Assert-SafeExistingPath -Path $file.FullName -LeafType File
            $bytes = Read-StableBytes $file.FullName
            $readback = Assert-SafeExistingPath -Path $file.FullName -LeafType File
            if ($record.Identity -cne $readback.Identity -or $record.Owner -cne $readback.Owner -or $record.AclSha256 -cne $readback.AclSha256) { throw "State leaf changed during fingerprint: $($file.FullName)" }
            $parts.Add($file.FullName.Substring($script:WaveRoot.Length) + '=' + (Get-Sha256Bytes $bytes) + "|" + $record.Identity + "|" + $record.Owner + "|" + $record.AclSha256)
        }
    }
    if (Test-Path -LiteralPath $script:ActiveClaimPath -PathType Leaf) {
        $activeClaimRecord = Assert-SafeExistingPath -Path $script:ActiveClaimPath -LeafType File
        $activeClaimBytes = Read-StableBytes $script:ActiveClaimPath
        $activeClaimReadback = Assert-SafeExistingPath -Path $script:ActiveClaimPath -LeafType File
        if ($activeClaimRecord.Identity -cne $activeClaimReadback.Identity -or $activeClaimRecord.Owner -cne $activeClaimReadback.Owner -or $activeClaimRecord.AclSha256 -cne $activeClaimReadback.AclSha256) { throw 'Active claim changed during fingerprint.' }
        $parts.Add($script:ActiveClaimPath.Substring($script:WaveRoot.Length) + '=' + (Get-Sha256Bytes $activeClaimBytes) + "|" + $activeClaimRecord.Identity + "|" + $activeClaimRecord.Owner + "|" + $activeClaimRecord.AclSha256)
    }
    Get-Sha256Text 'dynamo-integration-state-snapshot-v1' ($script:Utf8.GetBytes([string]::Join("`n", $parts)))
}

function Get-AttemptRows([object] $Journal, [string] $AttemptId) {
    if (-not $Journal.Attempts.ContainsKey($AttemptId)) { return @() }
    @($Journal.Attempts[$AttemptId])
}

function Get-ClosedLeasePath([string] $AttemptId, [string] $LeaseSha256) { Join-Path $script:ClosedLeaseDirectory "closed-ref-update.$AttemptId.$LeaseSha256.lock" }
function Get-ClosedClaimPath([string] $AttemptId, [int64] $Generation, [string] $ClaimSha256) {
    $generationText = $Generation.ToString('D10', [Globalization.CultureInfo]::InvariantCulture)
    Join-Path $script:ClosedClaimDirectory "closed-recovery.$AttemptId.g$generationText.$ClaimSha256.claim"
}
function Get-ArchivedClaimPath([string] $AttemptId, [int64] $Generation, [string] $ClaimSha256) {
    $generationText = $Generation.ToString('D10', [Globalization.CultureInfo]::InvariantCulture)
    Join-Path $script:ClaimArchiveDirectory "recovery-claim.$AttemptId.g$generationText.$ClaimSha256.claim"
}

function Find-ClosedClaim([string] $AttemptId) {
    $matches = @(Get-ChildItem -LiteralPath $script:ClosedClaimDirectory -File -Force | Where-Object { $_.Name -match "^closed-recovery\.$AttemptId\.g[0-9]{10}\.[0-9a-f]{64}\.claim$" })
    if ($matches.Count -gt 1) { throw "Multiple closed recovery claims for attempt $AttemptId" }
    if ($matches.Count -eq 1) { return $matches[0].FullName }
    $null
}

function Get-ArchivedClaims([string] $AttemptId) {
    $files = @(Get-ChildItem -LiteralPath $script:ClaimArchiveDirectory -File -Force |
        Where-Object { $_.Name -match "^recovery-claim\.$AttemptId\.g[0-9]{10}\.[0-9a-f]{64}\.claim$" } |
        Sort-Object Name)
    $records = [Collections.Generic.List[object]]::new()
    [int64]$expectedGeneration = 1
    $expectedPrior = $script:ZeroSha256
    foreach ($file in $files) {
        $record = Read-RecoveryClaim $file.FullName
        if ([string]$record.Value['attempt_id'] -cne $AttemptId -or [int64]$record.Value['generation'] -ne $expectedGeneration -or
            [string]$record.Value['prior_claim_sha256'] -cne $expectedPrior -or
            $file.Name -cne ("recovery-claim.{0}.g{1:D10}.{2}.claim" -f $AttemptId, $expectedGeneration, [string]$record.Value['claim_sha256'])) {
            throw 'Archived recovery-claim generation chain is non-contiguous or mismatched.'
        }
        Assert-OwnerDead $record.Value['owner']
        $records.Add($record)
        $expectedPrior = [string]$record.Value['claim_sha256']
        $expectedGeneration++
    }
    $records.ToArray()
}

function Get-IncompleteArchiveAttempts {
    $attempts = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($file in Get-ChildItem -LiteralPath $script:ClaimArchiveDirectory -File -Force) {
        if ($file.Name -cnotmatch '^recovery-claim\.([0-9a-f]{32})\.g[0-9]{10}\.[0-9a-f]{64}\.claim$') {
            throw "Unexpected recovery archive leaf: $($file.Name)"
        }
        $null = $attempts.Add($Matches[1])
    }
    @($attempts | Where-Object { $null -eq (Find-ClosedClaim $_) } | Sort-Object)
}

function Find-ArchiveOnlyAttempt {
    $attempts = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($file in Get-ChildItem -LiteralPath $script:ClaimArchiveDirectory -File -Force) {
        if ($file.Name -cnotmatch '^recovery-claim\.([0-9a-f]{32})\.g[0-9]{10}\.[0-9a-f]{64}\.claim$') { throw "Unexpected recovery archive leaf: $($file.Name)" }
        $null = $attempts.Add($Matches[1])
    }
    $candidates = @($attempts | Where-Object {
        $attemptId = $_
        $null -eq (Find-ClosedClaim $attemptId) -and
        @(Get-ChildItem -LiteralPath $script:ClosedLeaseDirectory -File -Force | Where-Object { $_.Name -match "^closed-ref-update\.$attemptId\.[0-9a-f]{64}\.lock$" }).Count -eq 1
    })
    if ($candidates.Count -ne 1) { throw 'Archive-only recovery state does not identify exactly one incomplete attempt.' }
    $candidates[0]
}

function Assert-LeaseRowsMatch([Collections.IDictionary] $Lease, [object[]] $Rows) {
    foreach ($row in $Rows) {
        foreach ($field in @('wave','unit','attempt_id','old_tip','new_tip','microplan_sha256','lease_sha256')) {
            if ([string]$row.Value[$field] -cne [string]$Lease[$field]) { throw "Lease/row mismatch at $field" }
        }
    }
}

function Assert-LeaseJournalAnchor([Collections.IDictionary] $Lease, [object] $Journal, [object[]] $Rows) {
    if ($Rows.Count -eq 0) {
        if ([int64]$Lease['expected_seq'] -ne [int64]$Journal.NextSeq -or [string]$Lease['expected_tail_sha256'] -cne [string]$Journal.TailSha256) {
            throw 'Rowless lease does not bind the current journal sequence/tail.'
        }
        return
    }
    $intent = @($Rows | Where-Object { $_.Value['phase'] -eq 'Intent' })
    if ($intent.Count -ne 1 -or [int64]$Lease['expected_seq'] -ne [int64]$intent[0].Value['seq'] -or
        [string]$Lease['expected_tail_sha256'] -cne [string]$intent[0].Value['prev_row_sha256']) {
        throw 'Lease does not bind its first durable Intent sequence/tail.'
    }
    if ([string]$Rows[-1].Value['row_sha256'] -cne [string]$Journal.TailSha256) { throw 'Rows after the active attempt are forbidden.' }
}

function Assert-JournalStateRelations([object] $Journal) {
    Assert-GlobalJournalRefContinuity $Journal
    foreach ($attempt in $Journal.Attempts.Keys) {
        $rows = @($Journal.Attempts[$attempt])
        $intent = @($rows | Where-Object { $_.Value['phase'] -eq 'Intent' })
        $terminal = @($rows | Where-Object { $_.Value['phase'] -ne 'Intent' })
        $leaseHash = if ($intent.Count -eq 1) { [string]$intent[0].Value['lease_sha256'] } elseif ($terminal.Count -eq 1) { [string]$terminal[0].Value['lease_sha256'] } else { throw "Attempt $attempt has no exact lease-bearing row." }
        $lease = Resolve-ExactLeaseRecord $attempt $leaseHash
        Assert-LeaseRowsMatch $lease.Value $rows
        if ($intent.Count -ne 1 -or [int64]$lease.Value['expected_seq'] -ne [int64]$intent[0].Value['seq'] -or
            [string]$lease.Value['expected_tail_sha256'] -cne [string]$intent[0].Value['prev_row_sha256']) { throw "Lease/journal anchor mismatch for attempt $attempt" }
        foreach ($row in $rows) {
            $null = Resolve-ExactRecoveryClaimForRow $row $lease.Value
        }
        # A generation-positive terminal is bound only by its exact row tuple above.
        # Generation-zero Committed rows may additionally be bound from a later recovery-head snapshot (shape b/c).
        if ($terminal.Count -eq 1 -and [int64]$terminal[0].Value['recovery_generation'] -eq 0) {
            $closedClaimPath = Find-ClosedClaim $attempt
            $claimPath = $closedClaimPath
            if ($null -eq $claimPath -and (Test-Path -LiteralPath $script:ActiveClaimPath -PathType Leaf)) {
                $activeCandidate = Read-RecoveryClaim $script:ActiveClaimPath
                if ([string]$activeCandidate.Value['attempt_id'] -ceq $attempt) { $claimPath = $script:ActiveClaimPath }
            }
            if ($null -eq $claimPath) {
                $archivedCandidates = @(Get-ArchivedClaims $attempt)
                if ($archivedCandidates.Count -gt 0) { $claimPath = $archivedCandidates[-1].Path }
            }
            if ($null -ne $claimPath) {
                $claim = Read-RecoveryClaim $claimPath
                if ([string]$claim.Value['lease_sha256'] -cne [string]$lease.Value['lease_sha256']) { throw "Claim/lease mismatch for attempt $attempt" }
                $claimBinds = [string]$claim.Value['snapshot_tail_sha256'] -ceq [string]$terminal[0].Value['row_sha256']
                if (-not $claimBinds) { throw "Claim/terminal mismatch for attempt $attempt" }
            }
        }
    }
}

function Assert-TerminalRef([Collections.IDictionary] $Terminal) {
    $tip = Get-RefTip
    if ($null -eq $tip) { $tip = $script:ZeroOid }
    $expected = if ([string]$Terminal['phase'] -in @('Committed','RecoveredCommitted')) { [string]$Terminal['new_tip'] } else { [string]$Terminal['old_tip'] }
    if ($tip -cne $expected) { throw 'Terminal row contradicts authoritative ref.' }
}

function Read-RecoveryClaim([string] $Path) {
    $keys = @('schema_version','execution_baseline','plan_set_sha256','wave','ref_name','attempt_id','generation','lease_sha256','snapshot_tail_sha256','snapshot_ref_tip','prior_claim_sha256','owner','created_at','claim_sha256')
    $record = Read-CanonicalJsonFile -Path $Path -ExpectedKeys $keys
    $preimage = [ordered]@{}
    foreach ($key in $keys[0..($keys.Count - 2)]) { $preimage[$key] = $record.Value[$key] }
    $actual = Get-Sha256Text 'dynamo-recovery-claim-v1' (ConvertTo-CanonicalBytes $preimage)
    if ($actual -cne [string]$record.Value['claim_sha256']) { throw 'Recovery claim hash mismatch.' }
    Assert-JsonInt64Token $record.Value['schema_version'] 'recovery claim schema_version'
    Assert-JsonInt64Token $record.Value['generation'] 'recovery claim generation'
    if ([int64]$record.Value['schema_version'] -ne 1) { throw 'Recovery claim schema version mismatch.' }
    if ([string]$record.Value['wave'] -cne $Wave) { throw 'Recovery claim wave mismatch.' }
    if ([string]$record.Value['execution_baseline'] -cne $script:Binding.ExecutionBaseline -or
        [string]$record.Value['plan_set_sha256'] -cne $script:Binding.PlanSetSha256 -or
        [string]$record.Value['ref_name'] -cne $script:RefName) { throw 'Recovery claim fixed binding/ref mismatch.' }
    if ([string]$record.Value['attempt_id'] -cnotmatch '^[0-9a-f]{32}$') { throw 'Claim attempt ID is not canonical Guid N.' }
    if ([int64]$record.Value['generation'] -lt 1) { throw 'Claim generation must be positive.' }
    foreach ($field in @('lease_sha256','snapshot_tail_sha256','prior_claim_sha256','claim_sha256')) { Assert-LowerHex ([string]$record.Value[$field]) 64 "claim $field" }
    Assert-LowerHex ([string]$record.Value['snapshot_ref_tip']) 40 'claim snapshot_ref_tip'
    Assert-OwnerSchema $record.Value['owner'] ([string]$record.Value['attempt_id'])
    if ([string]$record.Value['owner']['git_common_dir_identity_sha256'] -cne $script:Binding.CommonDirectoryRecord.IdentitySha256) { throw 'Claim owner common-directory identity does not match the immutable binding.' }
    Assert-CanonicalUtcTimestamp $record.Value['created_at'] 'recovery claim created_at'
    $record
}

function Resolve-ExactLeaseRecord([string] $AttemptId, [string] $LeaseSha256) {
    if ($AttemptId -cnotmatch '^[0-9a-f]{32}$') { throw 'Exact lease lookup attempt ID is invalid.' }
    Assert-LowerHex $LeaseSha256 64 'exact lease lookup hash'
    $candidates = [Collections.Generic.List[object]]::new()
    $closedPath = Get-ClosedLeasePath $AttemptId $LeaseSha256
    if (Test-Path -LiteralPath $closedPath -PathType Leaf) {
        $closed = Read-Lease $closedPath
        if ([string]$closed.Value['attempt_id'] -cne $AttemptId -or [string]$closed.Value['lease_sha256'] -cne $LeaseSha256) { throw 'Exact closed lease filename/content mismatch.' }
        $candidates.Add($closed)
    }
    if (Test-Path -LiteralPath $script:ActiveLeasePath -PathType Leaf) {
        $active = Read-Lease $script:ActiveLeasePath
        if ([string]$active.Value['attempt_id'] -ceq $AttemptId -and [string]$active.Value['lease_sha256'] -ceq $LeaseSha256) {
            $candidates.Add($active)
        }
    }
    if ($candidates.Count -ne 1) { throw "Lease tuple $AttemptId/$LeaseSha256 does not resolve to exactly one active/closed record." }
    $candidates[0]
}

function Resolve-ExactRecoveryClaim([string] $AttemptId, [int64] $Generation, [string] $ClaimSha256, [string] $LeaseSha256) {
    if ($AttemptId -cnotmatch '^[0-9a-f]{32}$') { throw 'Exact claim lookup attempt ID is invalid.' }
    if ($Generation -lt 1) { throw 'Exact claim lookup generation must be positive.' }
    Assert-LowerHex $ClaimSha256 64 'exact claim lookup hash'
    if ($ClaimSha256 -ceq $script:ZeroSha256) { throw 'Exact claim lookup hash must be nonzero.' }
    Assert-LowerHex $LeaseSha256 64 'exact claim lease hash'
    $candidates = [Collections.Generic.List[object]]::new()
    foreach ($path in @(
        (Get-ArchivedClaimPath $AttemptId $Generation $ClaimSha256),
        (Get-ClosedClaimPath $AttemptId $Generation $ClaimSha256)
    )) {
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            $candidate = Read-RecoveryClaim $path
            if ([string]$candidate.Value['attempt_id'] -cne $AttemptId -or
                [int64]$candidate.Value['generation'] -ne $Generation -or
                [string]$candidate.Value['claim_sha256'] -cne $ClaimSha256) {
                throw 'Exact claim filename/content mismatch.'
            }
            $candidates.Add($candidate)
        }
    }
    if (Test-Path -LiteralPath $script:ActiveClaimPath -PathType Leaf) {
        $active = Read-RecoveryClaim $script:ActiveClaimPath
        if ([string]$active.Value['attempt_id'] -ceq $AttemptId -and
            [int64]$active.Value['generation'] -eq $Generation -and
            [string]$active.Value['claim_sha256'] -ceq $ClaimSha256) {
            $candidates.Add($active)
        }
    }
    if ($candidates.Count -ne 1) { throw "Claim tuple $AttemptId/g$Generation/$ClaimSha256 does not resolve to exactly one active/archive/closed record." }
    $claim = $candidates[0]
    if ([string]$claim.Value['lease_sha256'] -cne $LeaseSha256) { throw 'Exact claim/lease tuple mismatch.' }
    $claim
}

function Resolve-ExactRecoveryClaimForRow([object] $RowRecord, [Collections.IDictionary] $Lease) {
    $generation = [int64]$RowRecord.Value['recovery_generation']
    $claimSha256 = [string]$RowRecord.Value['recovery_claim_sha256']
    if ($generation -eq 0) {
        if ($claimSha256 -cne $script:ZeroSha256) { throw 'Generation-zero row has a nonzero recovery claim.' }
        return $null
    }
    Resolve-ExactRecoveryClaim ([string]$RowRecord.Value['attempt_id']) $generation $claimSha256 ([string]$Lease['lease_sha256'])
}

function New-RecoveryClaim([Collections.IDictionary] $Lease, [int64] $Generation, [string] $PriorClaim, [string] $Tail, [string] $SnapshotRefTip) {
    $owner = New-Owner ([string]$Lease['attempt_id'])
    $claim = [ordered]@{
        schema_version = 1
        execution_baseline = $script:Binding.ExecutionBaseline
        plan_set_sha256 = $script:Binding.PlanSetSha256
        wave = $Wave
        ref_name = $script:RefName
        attempt_id = [string]$Lease['attempt_id']
        generation = $Generation
        lease_sha256 = [string]$Lease['lease_sha256']
        snapshot_tail_sha256 = $Tail
        snapshot_ref_tip = $SnapshotRefTip
        prior_claim_sha256 = $PriorClaim
        owner = $owner
        created_at = Get-UtcNowCanonical
    }
    $claim['claim_sha256'] = Get-Sha256Text 'dynamo-recovery-claim-v1' (ConvertTo-CanonicalBytes $claim)
    $claim
}

function Get-TempFingerprint {
    [string]::Join("`n", @(
        Get-ChildItem -LiteralPath $script:TempDirectory -File -Force |
            Sort-Object Name |
            ForEach-Object {
                $record = Assert-SafeExistingPath -Path $_.FullName -LeafType File
                $bytes = Read-StableBytes $_.FullName
                $readback = Assert-SafeExistingPath -Path $_.FullName -LeafType File
                if ($record.Identity -cne $readback.Identity -or $record.Owner -cne $readback.Owner -or $record.AclSha256 -cne $readback.AclSha256) { throw "Temp leaf changed during fingerprint: $($_.FullName)" }
                "$($_.Name)=$(Get-Sha256Bytes $bytes)|$($record.Identity)|$($record.Owner)|$($record.AclSha256)"
            }
    ))
}

function Archive-OrphanTemps {
    param(
        [Parameter(Mandatory)][Collections.IDictionary] $Lease,
        [switch] $ValidateOnly,
        [object[]] $ExpectedRecords
    )
    $attempt = [string]$Lease['attempt_id']
    $attemptTemps = @(Get-ChildItem -LiteralPath $script:TempDirectory -File -Force | Where-Object { $_.Name.StartsWith($attempt + '.', [StringComparison]::Ordinal) } | Sort-Object Name)
    if ($attemptTemps.Count -gt 1) { throw 'More than one partial row for the active attempt is ambiguous.' }
    $hasExpectedRecords = $PSBoundParameters.ContainsKey('ExpectedRecords')
    $expectedByPath = @{}
    if ($hasExpectedRecords) {
        foreach ($expected in @($ExpectedRecords)) {
            if ($expectedByPath.ContainsKey([string]$expected.Path)) { throw 'Duplicate expected temp record path.' }
            $expectedByPath[[string]$expected.Path] = $expected
        }
        if ($expectedByPath.Count -ne $attemptTemps.Count) { throw 'Temp record set changed across recovery claim acquisition.' }
    }
    $validatedRecords = [Collections.Generic.List[object]]::new()
    foreach ($temp in $attemptTemps) {
        if ($temp.Name -cnotmatch '^([0-9a-f]{32})\.g([0-9]{10})\.([0-9]{20})\.(intent|committed|recovered-committed|aborted-no-ref-change)\.([0-9a-f]{32})\.tmp$') { throw "Malformed attempt temp leaf: $($temp.Name)" }
        $tempAttempt = $Matches[1]
        $tempGeneration = $Matches[2]
        $tempSequence = $Matches[3]
        $tempPhase = $Matches[4]
        $tempNonce = $Matches[5]
        $rowKeys = @('schema_version','seq','phase','wave','unit','attempt_id','old_tip','new_tip','microplan_sha256','lease_sha256','recovery_generation','recovery_claim_sha256','utc','prev_row_sha256','row_sha256')
        $record = Read-CanonicalJsonFile -Path $temp.FullName -ExpectedKeys $rowKeys
        foreach ($numericField in @('schema_version','seq','recovery_generation')) { Assert-JsonInt64Token $record.Value[$numericField] "temp row $numericField" }
        Assert-CanonicalUtcTimestamp $record.Value['utc'] 'temp row utc'
        if ($hasExpectedRecords) {
            if (-not $expectedByPath.ContainsKey($record.Path)) { throw 'Temp record path changed across recovery claim acquisition.' }
            Assert-CanonicalRecordUnchanged $expectedByPath[$record.Path] $record 'Prospective row temp'
        }
        $expectedPhase = switch ($tempPhase) { 'intent' {'Intent'} 'committed' {'Committed'} 'recovered-committed' {'RecoveredCommitted'} 'aborted-no-ref-change' {'AbortedNoRefChange'} }
        $journal = Read-Journal
        if ($tempAttempt -cne $attempt -or [string]$record.Value['attempt_id'] -cne $attempt -or [string]$record.Value['lease_sha256'] -cne [string]$Lease['lease_sha256'] -or
            [int64]$record.Value['recovery_generation'] -ne [int64]$tempGeneration -or [int64]$record.Value['seq'] -ne [int64]$tempSequence -or
            [string]$record.Value['phase'] -cne $expectedPhase -or [int64]$record.Value['seq'] -ne [int64]$journal.NextSeq -or
            [string]$record.Value['prev_row_sha256'] -cne [string]$journal.TailSha256) { throw 'Temp row is not the exact next prospective row for the active attempt.' }
        Assert-LeaseRowsMatch $Lease @($record)
        $preimage = [ordered]@{}
        foreach ($key in $rowKeys[0..($rowKeys.Count - 2)]) { $preimage[$key] = $record.Value[$key] }
        if ((Get-Sha256Text 'dynamo-integration-row-v1' (ConvertTo-CanonicalBytes $preimage)) -cne [string]$record.Value['row_sha256']) { throw 'Temp prospective row hash mismatch.' }
        if ([int64]$record.Value['recovery_generation'] -eq 0) {
            if ([string]$record.Value['recovery_claim_sha256'] -cne $script:ZeroSha256) { throw 'Normal temp row has a recovery claim.' }
        } else {
            $null = Resolve-ExactRecoveryClaimForRow $record $Lease
        }
        $hash = Get-Sha256Bytes $record.Bytes
        $destination = Join-Path $script:OrphanDirectory "$attempt.g$tempGeneration.$tempSequence.$tempPhase.$tempNonce.$hash.orphan"
        if ($ValidateOnly) {
            $validatedRecords.Add($record)
        } else {
            $expectedRecord = if ($hasExpectedRecords) { $expectedByPath[$record.Path] } else { $record }
            $null = Move-VerifiedCanonicalRecord -Source $temp.FullName -Destination $destination -Expected $expectedRecord -Reader {
                param([string] $CandidatePath)
                Read-CanonicalJsonFile -Path $CandidatePath -ExpectedKeys $rowKeys
            } -Label 'Prospective row orphan archival'
        }
    }
    if ($ValidateOnly) { $validatedRecords.ToArray() }
}

function Assert-DetachedRowRecord {
    param(
        [Parameter(Mandatory)][object] $Record,
        [Parameter(Mandatory)][string] $FilenameAttempt,
        [Parameter(Mandatory)][int64] $FilenameGeneration,
        [Parameter(Mandatory)][int64] $FilenameSequence,
        [Parameter(Mandatory)][string] $FilenamePhaseSlug,
        [Parameter(Mandatory)][object] $Journal
    )
    $value = $Record.Value
    foreach ($numericField in @('schema_version','seq','recovery_generation')) { Assert-JsonInt64Token $value[$numericField] "detached row $numericField" }
    $expectedPhase = switch ($FilenamePhaseSlug) {
        'intent' { 'Intent' }
        'committed' { 'Committed' }
        'recovered-committed' { 'RecoveredCommitted' }
        'aborted-no-ref-change' { 'AbortedNoRefChange' }
        default { throw 'Detached row filename phase is invalid.' }
    }
    if ([int64]$value['schema_version'] -ne 1 -or
        [string]$value['wave'] -cne $Wave -or
        [string]$value['attempt_id'] -cne $FilenameAttempt -or
        [int64]$value['recovery_generation'] -ne $FilenameGeneration -or
        [int64]$value['seq'] -ne $FilenameSequence -or
        [string]$value['phase'] -cne $expectedPhase) {
        throw 'Detached row filename/schema tuple mismatch.'
    }
    if ($FilenameAttempt -cnotmatch '^[0-9a-f]{32}$' -or $FilenameSequence -lt 1) { throw 'Detached row attempt/sequence is invalid.' }
    Assert-CanonicalUtcTimestamp $value['utc'] 'detached row utc'
    foreach ($field in @('microplan_sha256','lease_sha256','recovery_claim_sha256','prev_row_sha256','row_sha256')) { Assert-LowerHex ([string]$value[$field]) 64 "detached row $field" }
    foreach ($field in @('old_tip','new_tip')) { Assert-LowerHex ([string]$value[$field]) 40 "detached row $field" }
    Assert-PersistedOperationTuple -UnitName ([string]$value['unit']) -Old ([string]$value['old_tip']) -New ([string]$value['new_tip']) -Label 'Detached journal row'
    $rowKeys = @('schema_version','seq','phase','wave','unit','attempt_id','old_tip','new_tip','microplan_sha256','lease_sha256','recovery_generation','recovery_claim_sha256','utc','prev_row_sha256','row_sha256')
    $preimage = [ordered]@{}
    foreach ($key in $rowKeys[0..($rowKeys.Count - 2)]) { $preimage[$key] = $value[$key] }
    if ((Get-Sha256Text 'dynamo-integration-row-v1' (ConvertTo-CanonicalBytes $preimage)) -cne [string]$value['row_sha256']) { throw 'Detached row hash mismatch.' }
    if ($FilenameGeneration -eq 0) {
        if ([string]$value['recovery_claim_sha256'] -cne $script:ZeroSha256) { throw 'Generation-zero detached row has a recovery claim.' }
    } elseif ($FilenameGeneration -gt 0) {
        if ([string]$value['recovery_claim_sha256'] -ceq $script:ZeroSha256) { throw 'Recovery detached row has a zero claim hash.' }
    } else { throw 'Detached row recovery generation is negative.' }
    if ($expectedPhase -eq 'Committed' -and $FilenameGeneration -ne 0) { throw 'Detached Committed row must be generation zero.' }
    if ($expectedPhase -in @('RecoveredCommitted','AbortedNoRefChange') -and $FilenameGeneration -le 0) { throw 'Detached recovery terminal must have a positive generation.' }

    $lease = Resolve-ExactLeaseRecord $FilenameAttempt ([string]$value['lease_sha256'])
    Assert-LeaseRowsMatch $lease.Value @($Record)
    $null = Resolve-ExactRecoveryClaimForRow $Record $lease.Value

    if ($FilenameSequence -gt [int64]$Journal.NextSeq) { throw 'Detached row sequence is beyond the next journal position.' }
    $expectedPrevious = if ($FilenameSequence -eq 1) {
        $script:ZeroSha256
    } elseif ($FilenameSequence -eq [int64]$Journal.NextSeq) {
        [string]$Journal.TailSha256
    } else {
        [string]$Journal.Rows[$FilenameSequence - 2].Value['row_sha256']
    }
    if ([string]$value['prev_row_sha256'] -cne $expectedPrevious) { throw 'Detached row does not bind its historical prospective journal position.' }
}

function Assert-PersistedLeaseBackReference([object] $LeaseRecord, [object] $Journal, [switch] $RequireTerminal) {
    $lease = $LeaseRecord.Value
    $attempt = [string]$lease['attempt_id']
    $rows = @(Get-AttemptRows $Journal $attempt)
    if ($rows.Count -eq 0) {
        if ($RequireTerminal -or $LeaseRecord.Path -cne $script:ActiveLeasePath) { throw "Persisted lease $attempt has no journal attempt." }
        Assert-LeaseJournalAnchor $lease $Journal @()
        return
    }
    Assert-LeaseRowsMatch $lease $rows
    $intents = @($rows | Where-Object { $_.Value['phase'] -eq 'Intent' })
    $terminals = @($rows | Where-Object { $_.Value['phase'] -ne 'Intent' })
    if ($intents.Count -ne 1 -or [int64]$lease['expected_seq'] -ne [int64]$intents[0].Value['seq'] -or
        [string]$lease['expected_tail_sha256'] -cne [string]$intents[0].Value['prev_row_sha256']) { throw "Persisted lease $attempt does not bind its journal anchor." }
    if ($RequireTerminal -and $terminals.Count -ne 1) { throw "Closed persisted lease $attempt has no exact terminal." }
    if ($LeaseRecord.Path -ceq $script:ActiveLeasePath -and [string]$rows[-1].Value['row_sha256'] -cne [string]$Journal.TailSha256) {
        throw "Active persisted lease $attempt is not the journal tail attempt."
    }
}

function Assert-PersistedClaimBackReference([object] $ClaimRecord, [object] $Journal, [switch] $RequireTerminal) {
    $claim = $ClaimRecord.Value
    $attempt = [string]$claim['attempt_id']
    $leaseRecord = Resolve-ExactLeaseRecord $attempt ([string]$claim['lease_sha256'])
    Assert-PersistedLeaseBackReference $leaseRecord $Journal
    $lease = $leaseRecord.Value
    if ([string]$claim['snapshot_ref_tip'] -cnotin @([string]$lease['old_tip'],[string]$lease['new_tip'])) { throw "Persisted claim $attempt has a snapshot ref outside its lease tuple." }
    $rows = @(Get-AttemptRows $Journal $attempt)
    $tailAnchors = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $null = $tailAnchors.Add([string]$lease['expected_tail_sha256'])
    foreach ($row in $rows) { $null = $tailAnchors.Add([string]$row.Value['row_sha256']) }
    if (-not $tailAnchors.Contains([string]$claim['snapshot_tail_sha256'])) { throw "Persisted claim $attempt does not bind a lease/journal tail anchor." }
    if ($RequireTerminal) {
        $terminals = @($rows | Where-Object { $_.Value['phase'] -ne 'Intent' })
        if ($terminals.Count -ne 1) { throw "Closed persisted claim $attempt has no exact terminal." }
        $terminalBinds = [int64]$terminals[0].Value['recovery_generation'] -eq [int64]$claim['generation'] -and
            [string]$terminals[0].Value['recovery_claim_sha256'] -ceq [string]$claim['claim_sha256']
        $claimBinds = [string]$claim['snapshot_tail_sha256'] -ceq [string]$terminals[0].Value['row_sha256']
        if (-not $terminalBinds -and -not $claimBinds) { throw "Closed persisted claim $attempt has no exact terminal/claim binding." }
    }
}

function Assert-StateLeaves {
    $waveNames = @(Get-ChildItem -LiteralPath $script:WaveRoot -Force | ForEach-Object Name | Sort-Object)
    $expectedWaveNames = @(@('journal','leases','recovery') + $(if (Test-Path -LiteralPath $script:ActiveLeasePath -PathType Leaf) { @('active-ref-update.lock') } else { @() })) | Sort-Object
    if ([string]::Join("`n",$waveNames) -cne [string]::Join("`n",$expectedWaveNames)) { throw 'Wave root contains an unexpected or missing entry.' }
    $journalRoot = Split-Path -Parent $script:RowsDirectory
    if ([string]::Join("`n",@(Get-ChildItem -LiteralPath $journalRoot -Force | ForEach-Object Name | Sort-Object)) -cne "orphans`nrows`ntmp") { throw 'Journal directory layout is not exact.' }
    $recoveryRoot = Split-Path -Parent $script:ClaimArchiveDirectory
    $recoveryNames = @(Get-ChildItem -LiteralPath $recoveryRoot -Force | ForEach-Object Name | Sort-Object)
    $expectedRecoveryNames = @(@('archives','closed') + $(if (Test-Path -LiteralPath $script:ActiveClaimPath -PathType Leaf) { @('active-recovery.claim') } else { @() })) | Sort-Object
    if ([string]::Join("`n",$recoveryNames) -cne [string]::Join("`n",$expectedRecoveryNames)) { throw 'Recovery directory layout is not exact.' }
    foreach ($directory in @($script:TempDirectory,$script:OrphanDirectory,$script:ClosedLeaseDirectory,$script:ClaimArchiveDirectory,$script:ClosedClaimDirectory)) {
        $unexpected = @(Get-ChildItem -LiteralPath $directory -Force | Where-Object { $_.PSIsContainer -or ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 })
        if ($unexpected.Count -ne 0) { throw "Unexpected directory/reparse entry under fixed leaf directory: $directory" }
    }
    $stateJournal = Read-Journal
    $closedLeaseAttempts = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($file in Get-ChildItem -LiteralPath $script:ClosedLeaseDirectory -File -Force) {
        if ($file.Name -cnotmatch '^closed-ref-update\.([0-9a-f]{32})\.([0-9a-f]{64})\.lock$') { throw "Unexpected closed lease leaf: $($file.Name)" }
        $filenameAttempt = $Matches[1]
        $filenameHash = $Matches[2]
        $lease = Read-Lease $file.FullName
        if ([string]$lease.Value['attempt_id'] -cne $filenameAttempt -or [string]$lease.Value['lease_sha256'] -cne $filenameHash) { throw 'Closed lease filename/content mismatch.' }
        if (-not $closedLeaseAttempts.Add($filenameAttempt)) { throw "Multiple closed leases for attempt $filenameAttempt" }
        Assert-PersistedLeaseBackReference $lease $stateJournal -RequireTerminal
    }
    $closedClaimAttempts = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($file in Get-ChildItem -LiteralPath $script:ClosedClaimDirectory -File -Force) {
        if ($file.Name -cnotmatch '^closed-recovery\.([0-9a-f]{32})\.g([0-9]{10})\.([0-9a-f]{64})\.claim$') { throw "Unexpected closed claim leaf: $($file.Name)" }
        $filenameAttempt = $Matches[1]
        $filenameGeneration = [int64]$Matches[2]
        $filenameHash = $Matches[3]
        $claim = Read-RecoveryClaim $file.FullName
        if ([string]$claim.Value['attempt_id'] -cne $filenameAttempt -or [int64]$claim.Value['generation'] -ne $filenameGeneration -or [string]$claim.Value['claim_sha256'] -cne $filenameHash) {
            throw 'Closed claim filename/content mismatch.'
        }
        if (-not $closedClaimAttempts.Add($filenameAttempt)) { throw "Multiple closed claims for attempt $filenameAttempt" }
        $claimArchives = @(Get-ArchivedClaims $filenameAttempt)
        $expectedPrior = if ($claimArchives.Count -eq 0) { $script:ZeroSha256 } else { [string]$claimArchives[-1].Value['claim_sha256'] }
        if ([int64]$claim.Value['generation'] -ne ($claimArchives.Count + 1) -or [string]$claim.Value['prior_claim_sha256'] -cne $expectedPrior) {
            throw 'Closed recovery claim does not extend its exact archive chain.'
        }
        Assert-PersistedClaimBackReference $claim $stateJournal -RequireTerminal
    }
    $archiveAttempts = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($file in Get-ChildItem -LiteralPath $script:ClaimArchiveDirectory -File -Force) {
        if ($file.Name -cnotmatch '^recovery-claim\.([0-9a-f]{32})\.g[0-9]{10}\.[0-9a-f]{64}\.claim$') { throw "Unexpected recovery archive leaf: $($file.Name)" }
        $null = $archiveAttempts.Add($Matches[1])
    }
    foreach ($attempt in $archiveAttempts) {
        foreach ($archivedClaim in @(Get-ArchivedClaims $attempt)) { Assert-PersistedClaimBackReference $archivedClaim $stateJournal }
    }
    if (Test-Path -LiteralPath $script:ActiveLeasePath -PathType Leaf) {
        $activeLease = Read-Lease $script:ActiveLeasePath
        Assert-PersistedLeaseBackReference $activeLease $stateJournal
    }
    if (Test-Path -LiteralPath $script:ActiveClaimPath -PathType Leaf) {
        $activeClaim = Read-RecoveryClaim $script:ActiveClaimPath
        $activeArchives = @(Get-ArchivedClaims ([string]$activeClaim.Value['attempt_id']))
        $expectedPrior = if ($activeArchives.Count -eq 0) { $script:ZeroSha256 } else { [string]$activeArchives[-1].Value['claim_sha256'] }
        if ([int64]$activeClaim.Value['generation'] -ne ($activeArchives.Count + 1) -or [string]$activeClaim.Value['prior_claim_sha256'] -cne $expectedPrior) {
            throw 'Active recovery claim does not extend its exact archive chain.'
        }
        Assert-PersistedClaimBackReference $activeClaim $stateJournal
    }
    $detachedRowKeys = @('schema_version','seq','phase','wave','unit','attempt_id','old_tip','new_tip','microplan_sha256','lease_sha256','recovery_generation','recovery_claim_sha256','utc','prev_row_sha256','row_sha256')
    $detachedJournal = $stateJournal
    foreach ($file in Get-ChildItem -LiteralPath $script:OrphanDirectory -File -Force) {
        if ($file.Name -cnotmatch '^([0-9a-f]{32})\.g([0-9]{10})\.([0-9]{20})\.(intent|committed|recovered-committed|aborted-no-ref-change)\.([0-9a-f]{32})\.([0-9a-f]{64})\.orphan$') {
            throw "Unexpected orphan row leaf: $($file.Name)"
        }
        $filenameAttempt = $Matches[1]
        $filenameGeneration = [int64]$Matches[2]
        $filenameSequence = [int64]$Matches[3]
        $filenamePhase = $Matches[4]
        $rawHash = $Matches[6]
        $orphan = Read-CanonicalJsonFile -Path $file.FullName -ExpectedKeys $detachedRowKeys
        if ($orphan.Sha256 -cne $rawHash) { throw 'Orphan row raw hash mismatch.' }
        Assert-DetachedRowRecord -Record $orphan -FilenameAttempt $filenameAttempt -FilenameGeneration $filenameGeneration -FilenameSequence $filenameSequence -FilenamePhaseSlug $filenamePhase -Journal $detachedJournal
    }
    foreach ($file in Get-ChildItem -LiteralPath $script:TempDirectory -File -Force) {
        if ($file.Name -cnotmatch '^([0-9a-f]{32})\.g([0-9]{10})\.([0-9]{20})\.(intent|committed|recovered-committed|aborted-no-ref-change)\.[0-9a-f]{32}\.tmp$') { throw "Unexpected temp row leaf: $($file.Name)" }
        $filenameAttempt = $Matches[1]
        $filenameGeneration = [int64]$Matches[2]
        $filenameSequence = [int64]$Matches[3]
        $filenamePhase = $Matches[4]
        $temp = Read-CanonicalJsonFile -Path $file.FullName -ExpectedKeys $detachedRowKeys
        Assert-DetachedRowRecord -Record $temp -FilenameAttempt $filenameAttempt -FilenameGeneration $filenameGeneration -FilenameSequence $filenameSequence -FilenamePhaseSlug $filenamePhase -Journal $detachedJournal
    }
}

function Test-CompletedState([object] $Journal) {
    if (@(Get-ChildItem -LiteralPath $script:TempDirectory -Force).Count -ne 0) { return $false }
    if (Test-Path -LiteralPath $script:ActiveLeasePath -PathType Leaf) { return $false }
    if (Test-Path -LiteralPath $script:ActiveClaimPath -PathType Leaf) { return $false }
    if (@(Get-IncompleteArchiveAttempts).Count -ne 0) { return $false }
    if ($Journal.Rows.Count -eq 0) { return $false }
    $terminalRecord = $Journal.Rows[-1]
    if ([string]$terminalRecord.Value['phase'] -eq 'Intent') { return $false }
    $attempt = [string]$terminalRecord.Value['attempt_id']
    $closedLeasePath = Get-ClosedLeasePath $attempt ([string]$terminalRecord.Value['lease_sha256'])
    if (-not (Test-Path -LiteralPath $closedLeasePath -PathType Leaf)) { return $false }
    $lease = Read-Lease $closedLeasePath
    $completedRows = @(Get-AttemptRows $Journal $attempt)
    Assert-LeaseRowsMatch $lease.Value $completedRows
    Assert-LeaseJournalAnchor $lease.Value $Journal $completedRows
    Assert-TerminalRef $terminalRecord.Value
    $closedClaimPath = Find-ClosedClaim $attempt
    $phase = [string]$terminalRecord.Value['phase']
    if ($phase -eq 'Committed' -and [int64]$terminalRecord.Value['recovery_generation'] -eq 0 -and $null -eq $closedClaimPath) {
        return $true
    }
    if ($null -eq $closedClaimPath -or -not (Test-Path -LiteralPath $closedClaimPath -PathType Leaf)) { return $false }
    $claim = Read-RecoveryClaim $closedClaimPath
    if ([string]$claim.Value['attempt_id'] -cne $attempt -or
        [string]$claim.Value['lease_sha256'] -cne [string]$lease.Value['lease_sha256']) { throw 'RecoveredComplete claim binding mismatch.' }
    $terminalBindsClaim = [string]$terminalRecord.Value['recovery_claim_sha256'] -ceq [string]$claim.Value['claim_sha256']
    $claimBindsTerminal = [string]$claim.Value['snapshot_tail_sha256'] -ceq [string]$terminalRecord.Value['row_sha256']
    if (-not $terminalBindsClaim -and -not $claimBindsTerminal) { throw 'RecoveredComplete lacks a terminal/claim-side binding.' }
    $true
}

function Close-ActiveLease([object] $LeaseRecord) {
    $lease = $LeaseRecord.Value
    $destination = Get-ClosedLeasePath ([string]$lease['attempt_id']) ([string]$lease['lease_sha256'])
    $null = Move-VerifiedCanonicalRecord -Source $script:ActiveLeasePath -Destination $destination -Expected $LeaseRecord -Reader {
        param([string] $CandidatePath)
        Read-Lease $CandidatePath
    } -Label 'Active lease closure'
    Invoke-Failpoint 'AfterLeaseRename'
}

function Invoke-NormalMode {
    $preJournal = Read-Journal
    Assert-JournalStateRelations $preJournal
    if ((Test-Path -LiteralPath $script:ActiveLeasePath) -or (Test-Path -LiteralPath $script:ActiveClaimPath)) { throw 'An active lease or recovery claim blocks normal admission.' }
    if (@(Get-IncompleteArchiveAttempts).Count -ne 0) { throw 'An incomplete archived recovery chain blocks normal admission.' }
    foreach ($attempt in $preJournal.Attempts.Keys) {
        $rows = @($preJournal.Attempts[$attempt])
        if ($rows.Count -eq 1 -and [string]$rows[0].Value['phase'] -eq 'Intent') { throw 'A pending Intent blocks normal admission.' }
    }
    if (@(Get-ChildItem -LiteralPath $script:TempDirectory -File -Force).Count -ne 0) { throw 'A retained partial row blocks normal admission.' }
    $preFingerprint = Get-StateFingerprint
    Invoke-TestBarrier 'AfterNormalSnapshot'
    if ((Get-StateFingerprint) -cne $preFingerprint) { throw 'Integration state identity/content/ACL changed after the normal admission snapshot.' }
    $tip = Get-RefTip
    if ($Mode -eq 'Initialize') {
        if ($NewTip -cne $script:Binding.ExecutionBaseline) { throw 'Initialize NewTip must equal the immutable execution baseline.' }
        if ($null -ne $tip) { throw 'Initialize requires the authoritative ref to be absent.' }
        $effectiveOld = $script:ZeroOid
    } else {
        if ($OldTip -ceq $NewTip) { throw 'Advance requires distinct old and new tips.' }
        if ($tip -cne $OldTip) { throw 'Advance OldTip is not the authoritative ref tip.' }
        $effectiveOld = $OldTip
    }
    foreach ($oid in @($NewTip) + $(if ($Mode -eq 'Advance') { @($OldTip) } else { @() })) {
        $exists = Invoke-Git -Arguments @('cat-file','-e',"$oid^{commit}") -AllowFailure
        if ($exists.ExitCode -ne 0) { throw "Tip is not an existing commit: $oid" }
    }
    $attempt = [Guid]::NewGuid().ToString('N').ToLowerInvariant()
    if ($preJournal.Attempts.ContainsKey($attempt) -or
        @(Get-ChildItem -LiteralPath $script:ClosedLeaseDirectory -File -Force | Where-Object { $_.Name.StartsWith("closed-ref-update.$attempt.",[StringComparison]::Ordinal) }).Count -ne 0 -or
        $null -ne (Find-ClosedClaim $attempt) -or @(Get-ArchivedClaims $attempt).Count -ne 0) { throw 'Generated attempt ID collides with immutable state.' }
    $owner = New-Owner $attempt
    $leaseValue = New-LeaseValue $attempt $Wave $Unit $effectiveOld $NewTip $MicroplanSha256 $owner $preJournal.NextSeq $preJournal.TailSha256
    Invoke-TestBarrier 'BeforeLeaseCreate'
    Write-CreateNewDurable $script:ActiveLeasePath (ConvertTo-CanonicalBytes $leaseValue)
    Invoke-Failpoint 'AfterLeaseCreate'
    $lease = Read-Lease $script:ActiveLeasePath
    Invoke-TestBarrier 'AfterActiveLeaseRead'
    $afterFingerprint = Get-StateFingerprint
    # Account for exactly the newly created active lease and prove all prior state is unchanged.
    $leaseBytes = [IO.File]::ReadAllBytes($script:ActiveLeasePath)
    $currentJournal = Read-Journal
    if ($currentJournal.TailSha256 -cne $preJournal.TailSha256 -or (Get-RefTip) -cne $tip) { throw 'State changed during normal admission.' }
    Assert-LeaseJournalAnchor $lease.Value $currentJournal @()
    foreach ($directory in @($script:RowsDirectory,$script:TempDirectory,$script:OrphanDirectory,$script:ClosedLeaseDirectory,$script:ClaimArchiveDirectory,$script:ClosedClaimDirectory)) {
        # Native path/identity readback closes directory-swap races before the first row mutation.
        Assert-SafeExistingPath -Path $directory -LeafType Directory | Out-Null
    }
    if ($afterFingerprint -cne $preFingerprint -or $leaseBytes.Length -eq 0) { throw 'Preflight state changed during active-lease admission.' }
    $leaseBeforeMutation = Read-Lease $script:ActiveLeasePath
    Assert-CanonicalRecordUnchanged $lease $leaseBeforeMutation 'Active lease before normal mutation'

    $journal = Read-Journal
    $intent = New-RowValue $lease.Value 'Intent' 0 $script:ZeroSha256 $journal
    $journal = Add-JournalRow $intent 'AfterIntentTempFsync' 'AfterIntentRowRename'
    $cas = Invoke-Git -Arguments @('update-ref','--no-deref',$script:RefName,$NewTip,$effectiveOld) -AllowFailure
    if ($cas.ExitCode -ne 0) { throw "Authoritative ref CAS failed; explicit recovery is required: $($cas.Stderr)" }
    Invoke-Failpoint 'AfterRefCas'
    if ((Get-RefTip) -cne $NewTip) { throw 'Authoritative ref readback mismatch after CAS.' }
    $terminal = New-RowValue $lease.Value 'Committed' 0 $script:ZeroSha256 $journal
    $journal = Add-JournalRow $terminal 'AfterTerminalTempFsync' 'AfterTerminalRowRename'
    if ((Get-RefTip) -cne $NewTip) { throw 'Authoritative ref changed before lease closure.' }
    Close-ActiveLease $lease
    [ordered]@{ status = 'NormalComplete'; wave = $Wave; unit = $Unit; attempt_id = $attempt; old_tip = $effectiveOld; new_tip = $NewTip; terminal_row_sha256 = $journal.TailSha256 }
}

function Get-RecoveryShapeSnapshot([object] $LeaseRecord, [object] $Journal) {
    $lease = $LeaseRecord.Value
    $attempt = [string]$lease['attempt_id']
    $resolvedLease = Resolve-ExactLeaseRecord $attempt ([string]$lease['lease_sha256'])
    Assert-CanonicalRecordUnchanged $LeaseRecord $resolvedLease 'Recovery lease snapshot'
    $activeLease = $resolvedLease.Path -ceq $script:ActiveLeasePath
    $attemptRows = @(Get-AttemptRows $Journal $attempt)
    Assert-LeaseRowsMatch $lease $attemptRows
    Assert-LeaseJournalAnchor $lease $Journal $attemptRows
    $intents = @($attemptRows | Where-Object { $_.Value['phase'] -eq 'Intent' })
    $terminals = @($attemptRows | Where-Object { $_.Value['phase'] -ne 'Intent' })
    $observedTip = Get-RefTip
    $refPresent = $null -ne $observedTip
    $tip = if ($refPresent) { $observedTip } else { $script:ZeroOid }
    $shape = $null
    $terminalPhase = $null

    if ($activeLease) {
        $closedLeasePath = Get-ClosedLeasePath $attempt ([string]$lease['lease_sha256'])
        if (Test-Path -LiteralPath $closedLeasePath) { throw 'Active and exact closed lease cannot coexist.' }
        if ($intents.Count -eq 0 -and $terminals.Count -eq 0) {
            if ($tip -cne [string]$lease['old_tip']) { throw 'Shape 0 requires the authoritative ref to equal old_tip.' }
            $shape = '0'
            $terminalPhase = 'AbortedNoRefChange'
        } elseif ($intents.Count -eq 1 -and $terminals.Count -eq 0) {
            if ($tip -ceq [string]$lease['old_tip']) {
                $terminalPhase = 'AbortedNoRefChange'
            } elseif ($tip -ceq [string]$lease['new_tip']) {
                $terminalPhase = 'RecoveredCommitted'
            } else {
                throw 'Shape a authoritative ref is missing or has a third value.'
            }
            $shape = 'a'
        } elseif ($intents.Count -eq 1 -and $terminals.Count -eq 1) {
            $expectedTip = if ([string]$terminals[0].Value['phase'] -in @('Committed','RecoveredCommitted')) { [string]$lease['new_tip'] } else { [string]$lease['old_tip'] }
            if ($tip -cne $expectedTip) { throw 'Shape b terminal contradicts the authoritative ref.' }
            $shape = 'b'
            $terminalPhase = [string]$terminals[0].Value['phase']
        } else { throw 'State is not one of recovery shapes 0, a, or b.' }
    } else {
        if ($intents.Count -ne 1 -or $terminals.Count -ne 1) { throw 'Shape c requires one Intent and one terminal.' }
        $expectedTip = if ([string]$terminals[0].Value['phase'] -in @('Committed','RecoveredCommitted')) { [string]$lease['new_tip'] } else { [string]$lease['old_tip'] }
        if ($tip -cne $expectedTip) { throw 'Shape c terminal contradicts the authoritative ref.' }
        $shape = 'c'
        $terminalPhase = [string]$terminals[0].Value['phase']
    }

    $rowTokens = @($Journal.Rows | ForEach-Object {
        "$($_.Path)|$($_.Sha256)|$($_.Identity)|$($_.Owner)|$($_.AclSha256)"
    })
    $attemptRowTokens = @($attemptRows | ForEach-Object {
        "$($_.Path)|$($_.Sha256)|$($_.Identity)|$($_.Owner)|$($_.AclSha256)"
    })
    $directoryTokens = @(
        $script:WaveRoot,
        (Split-Path -Parent $script:RowsDirectory),
        $script:RowsDirectory,
        $script:TempDirectory,
        $script:OrphanDirectory,
        $script:ClosedLeaseDirectory,
        (Split-Path -Parent $script:ClaimArchiveDirectory),
        $script:ClaimArchiveDirectory,
        $script:ClosedClaimDirectory
    ) | Select-Object -Unique | ForEach-Object {
        $directoryRecord = Assert-SafeExistingPath -Path $_ -LeafType Directory
        "$($directoryRecord.Path)|$($directoryRecord.Identity)|$($directoryRecord.Owner)|$($directoryRecord.AclSha256)"
    }
    $activeClaimRecord = if (Test-Path -LiteralPath $script:ActiveClaimPath -PathType Leaf) { Read-RecoveryClaim $script:ActiveClaimPath } else { $null }
    [pscustomobject]@{
        Shape = $shape
        TerminalPhase = $terminalPhase
        RefPresent = $refPresent
        Tip = $tip
        NextSeq = [int64]$Journal.NextSeq
        TailSha256 = [string]$Journal.TailSha256
        RowsFingerprint = Get-Sha256Text 'dynamo-recovery-row-snapshot-v1' ($script:Utf8.GetBytes([string]::Join("`n", $rowTokens)))
        AttemptRowsFingerprint = Get-Sha256Text 'dynamo-recovery-attempt-row-snapshot-v1' ($script:Utf8.GetBytes([string]::Join("`n", $attemptRowTokens)))
        DirectoryFingerprint = Get-Sha256Text 'dynamo-recovery-directory-snapshot-v1' ($script:Utf8.GetBytes([string]::Join("`n", $directoryTokens)))
        LeasePath = $resolvedLease.Path
        LeaseSha256 = $resolvedLease.Sha256
        LeaseIdentity = $resolvedLease.Identity
        LeaseOwner = $resolvedLease.Owner
        LeaseAclSha256 = $resolvedLease.AclSha256
        ActiveClaimPresent = $null -ne $activeClaimRecord
        ActiveClaimRecord = $activeClaimRecord
        Journal = $Journal
        AttemptRows = $attemptRows
        Intents = $intents
        Terminals = $terminals
        LeaseRecord = $resolvedLease
    }
}

function Assert-RecoveryShapeSnapshotUnchanged {
    param(
        [Parameter(Mandatory)][object] $Expected,
        [Parameter(Mandatory)][object] $LeaseRecord,
        [Parameter(Mandatory)][string] $Boundary,
        [AllowNull()][object] $ExpectedActiveClaim
    )
    $journal = Read-Journal
    Assert-JournalStateRelations $journal
    $actual = Get-RecoveryShapeSnapshot $LeaseRecord $journal
    foreach ($property in @('Shape','TerminalPhase','RefPresent','Tip','NextSeq','TailSha256','RowsFingerprint','AttemptRowsFingerprint','DirectoryFingerprint','LeasePath','LeaseSha256','LeaseIdentity','LeaseOwner','LeaseAclSha256')) {
        if ([string]$actual.$property -cne [string]$Expected.$property) { throw "Recovery shape snapshot changed at $Boundary ($property)." }
    }
    $claimExpectationWasSupplied = $PSBoundParameters.ContainsKey('ExpectedActiveClaim')
    $expectedClaim = if ($claimExpectationWasSupplied) { $ExpectedActiveClaim } else { $Expected.ActiveClaimRecord }
    if ($null -eq $expectedClaim) {
        if ($actual.ActiveClaimPresent) { throw "Recovery active-claim snapshot unexpectedly appeared at $Boundary." }
    } else {
        if (-not $actual.ActiveClaimPresent) { throw "Recovery active-claim snapshot disappeared at $Boundary." }
        Assert-CanonicalRecordUnchanged $expectedClaim $actual.ActiveClaimRecord "Recovery active claim at $Boundary"
    }
    $actual
}

function Invoke-RecoverMode {
    $journal = Read-Journal
    Assert-JournalStateRelations $journal
    if (Test-CompletedState $journal) {
        return [ordered]@{ status = 'AlreadyComplete'; wave = $Wave; terminal_row_sha256 = $journal.TailSha256 }
    }
    $activeLeaseExists = Test-Path -LiteralPath $script:ActiveLeasePath -PathType Leaf
    $activeClaimExists = Test-Path -LiteralPath $script:ActiveClaimPath -PathType Leaf

    if ($activeLeaseExists) {
        $leaseRecord = Read-Lease $script:ActiveLeasePath
    } elseif ($activeClaimExists) {
        $claimForAttempt = Read-RecoveryClaim $script:ActiveClaimPath
        $closedPath = Get-ClosedLeasePath ([string]$claimForAttempt.Value['attempt_id']) ([string]$claimForAttempt.Value['lease_sha256'])
        if (-not (Test-Path -LiteralPath $closedPath -PathType Leaf)) { throw 'Active claim without exact active/closed lease.' }
        $leaseRecord = Read-Lease $closedPath
    } else {
        $archiveOnlyAttempt = Find-ArchiveOnlyAttempt
        $closedLeaseMatches = @(Get-ChildItem -LiteralPath $script:ClosedLeaseDirectory -File -Force | Where-Object { $_.Name -match "^closed-ref-update\.$archiveOnlyAttempt\.[0-9a-f]{64}\.lock$" })
        $leaseRecord = Read-Lease $closedLeaseMatches[0].FullName
    }
    $lease = $leaseRecord.Value
    $attempt = [string]$lease['attempt_id']
    if ($null -ne (Find-ClosedClaim $attempt)) { throw 'An incomplete attempt cannot coexist with an attempt-specific closed recovery claim.' }
    $attemptRows = @(Get-AttemptRows $journal $attempt)
    Assert-LeaseRowsMatch $lease $attemptRows
    Assert-LeaseJournalAnchor $lease $journal $attemptRows

    # Classify the entire recoverable shape before any archive/claim/orphan mutation.
    # Malformed, symbolic, missing, or third-value refs therefore fail read-only.
    $shapeSnapshot = Get-RecoveryShapeSnapshot $leaseRecord $journal
    $allTemps = @(Get-ChildItem -LiteralPath $script:TempDirectory -File -Force)
    if (@($allTemps | Where-Object { -not $_.Name.StartsWith($attempt + '.', [StringComparison]::Ordinal) }).Count -ne 0) {
        throw 'A partial row for another attempt blocks recovery.'
    }
    $tempRecords = @(Archive-OrphanTemps -Lease $lease -ValidateOnly)
    $tempFingerprint = Get-TempFingerprint

    if ($activeClaimExists) {
        $oldClaim = Read-RecoveryClaim $script:ActiveClaimPath
        if ([string]$oldClaim.Value['attempt_id'] -cne $attempt -or [string]$oldClaim.Value['lease_sha256'] -cne [string]$lease['lease_sha256']) { throw 'Claim/lease mismatch.' }
        Assert-OwnerDead $oldClaim.Value['owner']
        $existingArchiveChain = @(Get-ArchivedClaims $attempt)
        $expectedGeneration = $existingArchiveChain.Count + 1
        $expectedPrior = if ($existingArchiveChain.Count -eq 0) { $script:ZeroSha256 } else { [string]$existingArchiveChain[-1].Value['claim_sha256'] }
        if ([int64]$oldClaim.Value['generation'] -ne $expectedGeneration -or [string]$oldClaim.Value['prior_claim_sha256'] -cne $expectedPrior) { throw 'Active recovery claim does not extend the archived generation chain.' }
        Invoke-TestBarrier 'BeforeClaimTakeover'
        $generation = [int64]$oldClaim.Value['generation'] + 1
        $priorClaim = [string]$oldClaim.Value['claim_sha256']
        $archive = Get-ArchivedClaimPath $attempt ([int64]$oldClaim.Value['generation']) $priorClaim
        $null = Move-VerifiedCanonicalRecord -Source $script:ActiveClaimPath -Destination $archive -Expected $oldClaim -Reader {
            param([string] $CandidatePath)
            Read-RecoveryClaim $CandidatePath
        } -Label 'Stale recovery claim archival'
        Invoke-Failpoint 'AfterClaimArchive'
    } else {
        $archives = @(Get-ArchivedClaims $attempt)
        if ($archives.Count -gt 0) {
            $last = $archives[-1]
            Assert-OwnerDead $last.Value['owner']
            $generation = $archives.Count + 1
            $priorClaim = [string]$last.Value['claim_sha256']
        } else {
            Assert-OwnerDead $lease['owner']
            $generation = 1
            $priorClaim = $script:ZeroSha256
        }
    }

    # Archiving a stale claim must not alter the classified journal/ref/lease snapshot.
    $null = Assert-RecoveryShapeSnapshotUnchanged -Expected $shapeSnapshot -LeaseRecord $leaseRecord -Boundary 'before new claim creation' -ExpectedActiveClaim $null
    if ((Get-TempFingerprint) -cne $tempFingerprint) { throw 'Partial-row evidence changed before recovery claim creation.' }
    $null = @(Archive-OrphanTemps -Lease $lease -ValidateOnly -ExpectedRecords $tempRecords)
    $claimValue = New-RecoveryClaim $lease $generation $priorClaim $shapeSnapshot.TailSha256 $shapeSnapshot.Tip
    Invoke-TestBarrier 'BeforeClaimCreate'
    Write-CreateNewDurable $script:ActiveClaimPath (ConvertTo-CanonicalBytes $claimValue)
    Invoke-Failpoint 'AfterClaimCreate'
    if ($generation -gt 1) { Invoke-Failpoint 'AfterClaimTakeover' }
    $claimRecord = Read-RecoveryClaim $script:ActiveClaimPath
    Invoke-TestBarrier 'AfterActiveClaimRead'
    $claim = $claimRecord.Value

    if ([string]$claim['lease_sha256'] -cne [string]$lease['lease_sha256'] -or
        [string]$claim['attempt_id'] -cne $attempt -or
        [int64]$claim['generation'] -ne $generation -or
        [string]$claim['prior_claim_sha256'] -cne $priorClaim -or
        [string]$claim['snapshot_tail_sha256'] -cne [string]$shapeSnapshot.TailSha256 -or
        [string]$claim['snapshot_ref_tip'] -cne [string]$shapeSnapshot.Tip) {
        throw 'Recovery claim does not exactly encode the reviewed snapshot.'
    }
    $postClaimSnapshot = Assert-RecoveryShapeSnapshotUnchanged -Expected $shapeSnapshot -LeaseRecord $leaseRecord -Boundary 'after claim acquisition' -ExpectedActiveClaim $claimRecord
    if ((Get-TempFingerprint) -cne $tempFingerprint) { throw 'Partial-row evidence changed during recovery claim acquisition.' }
    $claimBeforeMutation = Read-RecoveryClaim $script:ActiveClaimPath
    Assert-CanonicalRecordUnchanged $claimRecord $claimBeforeMutation 'Active recovery claim before recovery mutation'
    $null = Archive-OrphanTemps -Lease $lease -ExpectedRecords $tempRecords
    if (@(Get-ChildItem -LiteralPath $script:TempDirectory -File -Force).Count -ne 0) { throw 'Reviewed partial-row evidence did not move entirely to immutable orphan storage.' }
    $postOrphanSnapshot = Assert-RecoveryShapeSnapshotUnchanged -Expected $shapeSnapshot -LeaseRecord $leaseRecord -Boundary 'after orphan archival' -ExpectedActiveClaim $claimRecord
    $journal = $postOrphanSnapshot.Journal
    $intents = @($postOrphanSnapshot.Intents)
    $terminals = @($postOrphanSnapshot.Terminals)

    if ($postOrphanSnapshot.Shape -in @('0','a','b')) {
        if ($postOrphanSnapshot.Shape -eq '0') {
            $intent = New-RowValue $lease 'Intent' $generation ([string]$claim['claim_sha256']) $journal
            $journal = Add-JournalRow $intent 'AfterIntentTempFsync' 'AfterIntentRowRename'
            $terminal = New-RowValue $lease 'AbortedNoRefChange' $generation ([string]$claim['claim_sha256']) $journal
            $journal = Add-JournalRow $terminal 'AfterTerminalTempFsync' 'AfterRecoveryTerminalRowRename'
        } elseif ($postOrphanSnapshot.Shape -eq 'a') {
            $terminal = New-RowValue $lease ([string]$postOrphanSnapshot.TerminalPhase) $generation ([string]$claim['claim_sha256']) $journal
            $journal = Add-JournalRow $terminal 'AfterTerminalTempFsync' 'AfterRecoveryTerminalRowRename'
        } else {
            if ([string]$claim['snapshot_tail_sha256'] -cne [string]$terminals[0].Value['row_sha256']) { throw 'Claim terminal binding mismatch.' }
        }
        $leaseForClosure = Resolve-ExactLeaseRecord $attempt ([string]$lease['lease_sha256'])
        Assert-CanonicalRecordUnchanged $leaseRecord $leaseForClosure 'Recovery lease before closure'
        if ($leaseForClosure.Path -cne $script:ActiveLeasePath) { throw 'Recovery shape requires the exact active lease before closure.' }
        Close-ActiveLease $leaseForClosure
    } else {
        if ([string]$claim['snapshot_tail_sha256'] -cne [string]$terminals[0].Value['row_sha256']) { throw 'Shape c claim terminal binding mismatch.' }
        $closedLease = Resolve-ExactLeaseRecord $attempt ([string]$lease['lease_sha256'])
        Assert-CanonicalRecordUnchanged $leaseRecord $closedLease 'Shape c closed lease'
        if ($closedLease.Path -ceq $script:ActiveLeasePath) { throw 'Shape c unexpectedly regained an active lease.' }
    }

    $journal = Read-Journal
    Assert-JournalStateRelations $journal
    $finalRows = @(Get-AttemptRows $journal $attempt)
    $finalTerminal = @($finalRows | Where-Object { $_.Value['phase'] -ne 'Intent' })
    if ($finalTerminal.Count -ne 1) { throw 'Recovery did not converge to one terminal.' }
    Assert-TerminalRef $finalTerminal[0].Value
    # A claim created before a recovery terminal binds it from the terminal side; shape b/c binds it from the claim side.
    $terminalBindsClaim = [string]$finalTerminal[0].Value['recovery_claim_sha256'] -ceq [string]$claim['claim_sha256']
    $claimBindsTerminal = [string]$claim['snapshot_tail_sha256'] -ceq [string]$finalTerminal[0].Value['row_sha256']
    if (-not $terminalBindsClaim -and -not $claimBindsTerminal) { throw 'Final terminal/claim binding mismatch.' }
    $closedClaimPath = Get-ClosedClaimPath $attempt $generation ([string]$claim['claim_sha256'])
    $null = Move-VerifiedCanonicalRecord -Source $script:ActiveClaimPath -Destination $closedClaimPath -Expected $claimRecord -Reader {
        param([string] $CandidatePath)
        Read-RecoveryClaim $CandidatePath
    } -Label 'Recovery claim closure'
    Invoke-Failpoint 'AfterClaimRename'
    [ordered]@{ status = 'RecoveredComplete'; wave = $Wave; unit = [string]$lease['unit']; attempt_id = $attempt; recovery_generation = $generation; terminal_row_sha256 = [string]$finalTerminal[0].Value['row_sha256'] }
}

# The repository probe starts at the committed helper's directory and never honors caller path selectors.
$script:RepositoryProbeRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$canonicalHelperPath = Join-Path $script:RepositoryProbeRoot 'scripts/remediation/modules/canonical-json.ps1'
. $canonicalHelperPath
$canonicalHelperBefore = Get-PathRecord $canonicalHelperPath
$scriptPathRecord = Get-PathRecord $PSCommandPath
$pathSecurityHelperPath = Join-Path $script:RepositoryProbeRoot 'scripts/remediation/modules/path-security.ps1'
$pathSecurityHelperBefore = Get-PathRecord $pathSecurityHelperPath
. $pathSecurityHelperPath
$pathSecurityHelperAfter = Assert-SafeExistingPath -Path $pathSecurityHelperPath -LeafType File
if ($pathSecurityHelperBefore.Identity -cne $pathSecurityHelperAfter.Identity -or $pathSecurityHelperBefore.Owner -cne $pathSecurityHelperAfter.Owner -or $pathSecurityHelperBefore.AclSha256 -cne $pathSecurityHelperAfter.AclSha256) {
    throw 'Path-security helper path identity, owner, or ACL changed during load.'
}
$bindingSchemaHelperPath = Join-Path $script:RepositoryProbeRoot 'scripts/remediation/modules/plan-set-binding-schema.ps1'
$bindingSchemaHelperBefore = Get-PathRecord $bindingSchemaHelperPath
. $bindingSchemaHelperPath
$bindingSchemaHelperAfter = Assert-SafeExistingPath -Path $bindingSchemaHelperPath -LeafType File
if ($bindingSchemaHelperBefore.Identity -cne $bindingSchemaHelperAfter.Identity -or $bindingSchemaHelperBefore.Owner -cne $bindingSchemaHelperAfter.Owner -or $bindingSchemaHelperBefore.AclSha256 -cne $bindingSchemaHelperAfter.AclSha256) {
    throw 'Binding-schema helper path identity, owner, or ACL changed during load.'
}
$expectedScriptPath = [IO.Path]::GetFullPath((Join-Path $script:RepositoryProbeRoot 'scripts/remediation/update-integration-ref.ps1'))
if ($scriptPathRecord.Path -cne $expectedScriptPath) { throw 'Helper must run from its canonical fixed repository path.' }
$repoResult = Invoke-Git -Arguments @('rev-parse','--show-toplevel')
$repositoryRoot = [IO.Path]::GetFullPath($repoResult.Stdout)
if ($repositoryRoot -cne $script:RepositoryProbeRoot) { throw 'Git repository root does not match the helper-derived root.' }
$commonResult = Invoke-Git -Arguments @('rev-parse','--path-format=absolute','--git-common-dir')
$commonDirectory = [IO.Path]::GetFullPath($commonResult.Stdout)
$script:CommonDirectory = $commonDirectory
Assert-SafeExistingPath -Path $repositoryRoot -LeafType Directory | Out-Null
Assert-SafeExistingPath -Path $commonDirectory -LeafType Directory | Out-Null
$canonicalHelperAfter = Assert-SafeExistingPath -Path $canonicalHelperPath -LeafType File
if ($canonicalHelperBefore.Identity -cne $canonicalHelperAfter.Identity -or $canonicalHelperBefore.Owner -cne $canonicalHelperAfter.Owner -or $canonicalHelperBefore.AclSha256 -cne $canonicalHelperAfter.AclSha256) {
    throw 'Canonical helper path identity, owner, or ACL changed during load.'
}
if ($Mode -eq 'Initialize' -and $Wave -cne 'wave0') {
    if ([Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_TEST_MODE', 'Process') -cne '1') {
        throw 'Production Initialize is restricted to the wave0 integration ref.'
    }
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar
    if (-not $commonDirectory.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Non-wave0 Initialize is accepted only for an explicit OS-temp remediation fixture.'
    }
}
$script:Binding = Assert-BindingAndManifest $commonDirectory
Assert-FailpointConfiguration

if ($Mode -ne 'Recover') {
    if ($Mode -eq 'Initialize' -and $NewTip -cne $script:Binding.ExecutionBaseline) { throw 'Initialize NewTip must equal the immutable execution baseline.' }
    if ($Mode -eq 'Advance' -and $OldTip -ceq $NewTip) { throw 'Advance requires distinct old and new tips.' }
    foreach ($oid in @($NewTip) + $(if ($Mode -eq 'Advance') { @($OldTip) } else { @() })) {
        if ((Invoke-Git -Arguments @('cat-file','-e',"$oid^{commit}") -AllowFailure).ExitCode -ne 0) { throw "Tip is not an existing commit: $oid" }
    }
    $admissionTip = Get-RefTip
    if ($Mode -eq 'Initialize' -and $null -ne $admissionTip) { throw 'Initialize requires the authoritative ref to be absent.' }
    if ($Mode -eq 'Advance' -and $admissionTip -cne $OldTip) { throw 'Advance OldTip is not the authoritative ref tip.' }
}

$stateComponents = @('dynamo-remediation','integration-state-v1',$script:Binding.ExecutionBaseline,$script:Binding.PlanSetSha256,'waves',$Wave)
$stateCandidate = Join-Path $commonDirectory ([string]::Join([IO.Path]::DirectorySeparatorChar, $stateComponents))
if ($Mode -eq 'Advance' -and -not (Test-Path -LiteralPath $stateCandidate)) { throw 'Advance requires an existing initialized wave state.' }
$requireExistingLayout = $Mode -eq 'Recover' -or (Test-Path -LiteralPath $stateCandidate)
if ($requireExistingLayout) {
    $stateRoot = $commonDirectory
    foreach ($component in $stateComponents) {
        $stateRoot = Join-Path $stateRoot $component
        Assert-SafeExistingPath -Path $stateRoot -LeafType Directory | Out-Null
        Assert-RestrictiveDirectory $stateRoot
    }
} else {
    $stateRoot = Ensure-SafeDirectoryChain $commonDirectory $stateComponents
}
$script:WaveRoot = $stateRoot
$ensureOrRead = {
    param([string]$Root, [string[]]$Components)
    if ($requireExistingLayout) {
        $path = $Root
        foreach ($component in $Components) {
            $path = Join-Path $path $component
            Assert-SafeExistingPath -Path $path -LeafType Directory | Out-Null
            Assert-RestrictiveDirectory $path
        }
        return $path
    }
    Ensure-SafeDirectoryChain $Root $Components
}
$script:RowsDirectory = & $ensureOrRead $stateRoot @('journal','rows')
$script:TempDirectory = & $ensureOrRead $stateRoot @('journal','tmp')
$script:OrphanDirectory = & $ensureOrRead $stateRoot @('journal','orphans')
$leaseRoot = & $ensureOrRead $stateRoot @('leases')
$script:ClosedLeaseDirectory = $leaseRoot
$recoveryRoot = & $ensureOrRead $stateRoot @('recovery')
$script:ClaimArchiveDirectory = & $ensureOrRead $recoveryRoot @('archives')
$script:ClosedClaimDirectory = & $ensureOrRead $recoveryRoot @('closed')
$script:ActiveLeasePath = Join-Path $stateRoot 'active-ref-update.lock'
$script:ActiveClaimPath = Join-Path $recoveryRoot 'active-recovery.claim'

Assert-StateLeaves
$result = if ($Mode -eq 'Recover') { Invoke-RecoverMode } else { Invoke-NormalMode }
$output = ConvertTo-CanonicalBytes $result
[Console]::Out.Write($script:Utf8.GetString($output))
