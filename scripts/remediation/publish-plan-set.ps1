#requires -Version 7.4

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$script:AuditBaseline = '03ec755eb109975ecc8911f26cc75ee482f32a7a'
$script:ZeroSha256 = '0' * 64
$script:Utf8 = [Text.UTF8Encoding]::new($false, $true)
$script:ExactRepositoryState = $null
$script:PayloadPaths = @(
    'docs/superpowers/plans/2026-07-12-security-performance-dashboard-remediation-program.md'
    'docs/superpowers/plans/2026-07-12-security-remediation.md'
    'docs/superpowers/plans/2026-07-12-performance-remediation.md'
    'docs/superpowers/plans/2026-07-12-dashboard-ux-remediation.md'
    'docs/superpowers/plans/2026-07-13-wave0-bootstrap.md'
) | Sort-Object -CaseSensitive
$script:ControlSchemaPath = 'scripts/remediation/control-schema-v2.json'
$script:LegacyControlPaths = @(
    'scripts/remediation/publish-plan-set.ps1'
    'scripts/remediation/update-integration-ref.ps1'
    'tests/scripts/plan-set-publisher-contract.ps1'
    'tests/scripts/integration-ref-journal-contract.ps1'
) | Sort-Object -CaseSensitive
$script:ControlPaths = $script:LegacyControlPaths
$script:ControlSchema = $null
$script:PublisherFailpoints = @(
    'after-lease-create'
    'after-bundle-publish'
    'after-row-temp-write'
    'after-bundle-prepared'
    'after-binding-temp-write'
    'after-binding-create'
    'after-binding-committed'
    'after-lease-close'
)
$script:PublisherBarriers = @(
    'after-source-snapshot'
    'after-payload-copy'
    'before-publication-lease'
    'before-publication-complete'
)

if (-not ('DynamoPlanPublisherNative' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
using System.Text;

public static class DynamoPlanPublisherNative
{
    const uint FILE_READ_ATTRIBUTES = 0x80;
    const uint FILE_SHARE_READ = 1, FILE_SHARE_WRITE = 2, FILE_SHARE_DELETE = 4;
    const uint OPEN_EXISTING = 3;
    const uint FILE_FLAG_BACKUP_SEMANTICS = 0x02000000;
    const uint FILE_FLAG_OPEN_REPARSE_POINT = 0x00200000;
    const uint MOVEFILE_WRITE_THROUGH = 0x8;

    [StructLayout(LayoutKind.Sequential)]
    struct FILE_ID_INFO {
        public ulong VolumeSerialNumber;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 16)] public byte[] FileId;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern SafeFileHandle CreateFileW(string name, uint access, uint share, IntPtr security,
        uint creation, uint flags, IntPtr template);

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
        if (!RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
            throw new PlatformNotSupportedException("Native identity requires Windows FileIdInfo in Wave 0.");
        using (var handle = CreateFileW(LongPath(path), FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, IntPtr.Zero, OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, IntPtr.Zero)) {
            if (handle.IsInvalid) { int error = Marshal.GetLastWin32Error(); throw new Win32Exception(error, "CreateFileW failed (" + error.ToString() + ")"); }
            FILE_ID_INFO info;
            if (!GetFileInformationByHandleEx(handle, 18, out info, (uint)Marshal.SizeOf(typeof(FILE_ID_INFO))))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "FileIdInfo failed");
            return info.VolumeSerialNumber.ToString("x16") + ":" +
                BitConverter.ToString(info.FileId).Replace("-", "").ToLowerInvariant();
        }
    }

    public static string GetFinalPath(string path)
    {
        if (!RuntimeInformation.IsOSPlatform(OSPlatform.Windows)) return Path.GetFullPath(path);
        using (var handle = CreateFileW(LongPath(path), FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, IntPtr.Zero, OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, IntPtr.Zero)) {
            if (handle.IsInvalid) { int error = Marshal.GetLastWin32Error(); throw new Win32Exception(error, "CreateFileW failed (" + error.ToString() + ")"); }
            var buffer = new StringBuilder(32768);
            uint length = GetFinalPathNameByHandleW(handle, buffer, (uint)buffer.Capacity, 0);
            if (length == 0 || length >= buffer.Capacity) throw new Win32Exception(Marshal.GetLastWin32Error());
            string value = buffer.ToString();
            if (value.StartsWith(@"\\?\UNC\", StringComparison.OrdinalIgnoreCase)) return @"\\" + value.Substring(8);
            if (value.StartsWith(@"\\?\", StringComparison.OrdinalIgnoreCase)) return value.Substring(4);
            return value;
        }
    }

    public static void MoveNoReplaceWriteThrough(string source, string destination)
    {
        if (File.Exists(destination) || Directory.Exists(destination))
            throw new IOException("Destination already exists: " + destination);
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows)) {
            if (!MoveFileExW(LongPath(source), LongPath(destination), MOVEFILE_WRITE_THROUGH))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "MoveFileExW failed");
        } else if (Directory.Exists(source)) {
            Directory.Move(source, destination);
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
    param([string] $Path, [string[]] $ExpectedKeys)
    $before = Assert-SafeExistingPath -Path $Path -LeafType File
    Assert-RestrictedAcl -Path $Path -Kind File
    $bytes = [IO.File]::ReadAllBytes($Path)
    $after = Assert-SafeExistingPath -Path $Path -LeafType File
    if ($before.IdentitySha256 -cne $after.IdentitySha256 -or $before.Owner -cne $after.Owner -or
        $before.AclSha256 -cne $after.AclSha256) {
        throw "Canonical JSON path identity/owner/ACL changed during read: $Path"
    }
    if ($bytes.Length -eq 0 -or $bytes[-1] -ne 10) { throw "Canonical JSON must end in one LF: $Path" }
    if ($bytes.Length -gt 1 -and $bytes[-2] -eq 10) { throw "Canonical JSON has extra LF: $Path" }
    if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xef -and $bytes[1] -eq 0xbb -and $bytes[2] -eq 0xbf) { throw "BOM rejected: $Path" }
    foreach ($byte in $bytes) { if ($byte -eq 13) { throw "CR rejected: $Path" } }
    $jsonText = $script:Utf8.GetString($bytes)
    $document = [Text.Json.JsonDocument]::Parse($jsonText, [Text.Json.JsonDocumentOptions]@{
        AllowTrailingCommas = $false
        CommentHandling = [Text.Json.JsonCommentHandling]::Disallow
    })
    try {
        if ($document.RootElement.ValueKind -ne [Text.Json.JsonValueKind]::Object) { throw "JSON root must be object: $Path" }
        $value = Convert-JsonElement $document.RootElement
    } finally { $document.Dispose() }
    if ($ExpectedKeys) {
        if ([string]::Join("`n", @($value.Keys)) -cne [string]::Join("`n", $ExpectedKeys)) {
            throw "Canonical JSON key order/schema mismatch: $Path"
        }
    }
    $roundTrip = ConvertTo-CanonicalBytes $value
    if (-not (Test-BytesEqual $bytes $roundTrip)) {
        throw "Noncanonical JSON bytes: $Path"
    }
    [pscustomobject]@{ Path = $Path; Bytes = $bytes; Value = $value; Sha256 = Get-Sha256Bytes $bytes; PathRecord = $after }
}

function Write-CreateNewDurable([string] $Path, [byte[]] $Bytes) {
    $stream = [IO.FileStream]::new($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None, 4096, [IO.FileOptions]::WriteThrough)
    try {
        $stream.Write($Bytes, 0, $Bytes.Length)
        $stream.Flush($true)
    } finally { $stream.Dispose() }
    $readback = [IO.File]::ReadAllBytes($Path)
    if (-not (Test-BytesEqual $Bytes $readback)) {
        throw "Durable readback mismatch: $Path"
    }
}

function Move-NoReplaceDurable([string] $Source, [string] $Destination, [switch] $Directory) {
    [DynamoPlanPublisherNative]::MoveNoReplaceWriteThrough($Source, $Destination)
    if (Test-Path -LiteralPath $Source) { throw "Rename source remains: $Source" }
    if ($Directory) {
        if (-not (Test-Path -LiteralPath $Destination -PathType Container)) { throw "Renamed directory is missing: $Destination" }
    } elseif (-not (Test-Path -LiteralPath $Destination -PathType Leaf)) { throw "Renamed file is missing: $Destination" }
}

function Invoke-Git {
    param([string[]] $Arguments, [switch] $AllowFailure)
    $psi = [Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = 'git'
    $psi.WorkingDirectory = $script:RepositoryRoot
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true
    foreach ($name in @('GIT_DIR','GIT_WORK_TREE','GIT_COMMON_DIR','GIT_INDEX_FILE','GIT_OBJECT_DIRECTORY','GIT_ALTERNATE_OBJECT_DIRECTORIES','GIT_NAMESPACE','GIT_CEILING_DIRECTORIES','GIT_DISCOVERY_ACROSS_FILESYSTEM','GIT_CONFIG','GIT_CONFIG_GLOBAL','GIT_CONFIG_SYSTEM','GIT_CONFIG_COUNT','GIT_REPLACE_REF_BASE')) {
        $null = $psi.Environment.Remove($name)
    }
    $psi.Environment['GIT_NO_REPLACE_OBJECTS'] = '1'
    $psi.Environment['GIT_OPTIONAL_LOCKS'] = '0'
    $psi.Environment['GIT_CONFIG_NOSYSTEM'] = '1'
    $psi.Environment['GIT_TERMINAL_PROMPT'] = '0'
    $psi.ArgumentList.Add('-c'); $psi.ArgumentList.Add('core.hooksPath=NUL')
    $psi.ArgumentList.Add('-C'); $psi.ArgumentList.Add($script:RepositoryRoot)
    foreach ($argument in $Arguments) { $psi.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($psi)
    $stdout = $process.StandardOutput.ReadToEnd()
    $stderr = $process.StandardError.ReadToEnd()
    $process.WaitForExit()
    if ($process.ExitCode -ne 0 -and -not $AllowFailure) { throw "git failed ($($process.ExitCode)): $stderr" }
    [pscustomobject]@{ ExitCode = $process.ExitCode; Stdout = $stdout.TrimEnd("`r", "`n"); Stderr = $stderr.TrimEnd("`r", "`n") }
}

function Invoke-GitBytes {
    param([string[]] $Arguments, [switch] $AllowFailure)
    $psi = [Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = 'git'
    $psi.WorkingDirectory = $script:RepositoryRoot
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true
    foreach ($name in @('GIT_DIR','GIT_WORK_TREE','GIT_COMMON_DIR','GIT_INDEX_FILE','GIT_OBJECT_DIRECTORY','GIT_ALTERNATE_OBJECT_DIRECTORIES','GIT_NAMESPACE','GIT_CEILING_DIRECTORIES','GIT_DISCOVERY_ACROSS_FILESYSTEM','GIT_CONFIG','GIT_CONFIG_GLOBAL','GIT_CONFIG_SYSTEM','GIT_CONFIG_COUNT','GIT_REPLACE_REF_BASE')) {
        $null = $psi.Environment.Remove($name)
    }
    $psi.Environment['GIT_NO_REPLACE_OBJECTS'] = '1'
    $psi.Environment['GIT_OPTIONAL_LOCKS'] = '0'
    $psi.Environment['GIT_CONFIG_NOSYSTEM'] = '1'
    $psi.Environment['GIT_TERMINAL_PROMPT'] = '0'
    $psi.ArgumentList.Add('-c'); $psi.ArgumentList.Add('core.hooksPath=NUL')
    $psi.ArgumentList.Add('-C'); $psi.ArgumentList.Add($script:RepositoryRoot)
    foreach ($argument in $Arguments) { $psi.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($psi)
    $memory = [IO.MemoryStream]::new()
    try {
        $copyTask = $process.StandardOutput.BaseStream.CopyToAsync($memory)
        $errorTask = $process.StandardError.ReadToEndAsync()
        $null = $copyTask.GetAwaiter().GetResult()
        $errorText = $errorTask.GetAwaiter().GetResult()
        $process.WaitForExit()
        if ($process.ExitCode -ne 0 -and -not $AllowFailure) { throw "git failed ($($process.ExitCode)): $errorText" }
        [pscustomobject]@{ ExitCode = $process.ExitCode; Bytes = $memory.ToArray(); Stderr = $errorText.TrimEnd("`r", "`n") }
    }
    finally {
        $memory.Dispose()
        $process.Dispose()
    }
}

function ConvertFrom-NulSeparatedUtf8([byte[]] $Bytes, [string] $Label) {
    if ($Bytes.Length -eq 0) { return @() }
    $records = [Collections.Generic.List[string]]::new()
    [int]$start = 0
    for ($index = 0; $index -lt $Bytes.Length; $index++) {
        if ($Bytes[$index] -ne 0) { continue }
        if ($index -eq $start) { throw "$Label contains an empty record." }
        $segment = [byte[]]::new($index - $start)
        [Array]::Copy($Bytes, $start, $segment, 0, $segment.Length)
        $records.Add($script:Utf8.GetString($segment))
        $start = $index + 1
    }
    if ($start -ne $Bytes.Length) { throw "$Label is not NUL terminated." }
    $records.ToArray()
}

function Get-RawCommitRecord([string] $Oid) {
    Assert-LowerHex $Oid 40 'raw commit OID'
    [byte[]]$bytes = (Invoke-GitBytes -Arguments @('cat-file','commit',$Oid)).Bytes
    [int]$headerEnd = -1
    for ($index = 0; $index -lt ($bytes.Length - 1); $index++) {
        if ($bytes[$index] -eq 10 -and $bytes[$index + 1] -eq 10) { $headerEnd = $index; break }
    }
    if ($headerEnd -lt 0) { throw "Raw commit has no header terminator: $Oid" }
    $headerBytes = [byte[]]::new($headerEnd)
    [Array]::Copy($bytes, 0, $headerBytes, 0, $headerEnd)
    foreach ($byte in $headerBytes) { if ($byte -eq 13) { throw "Raw commit header contains CR: $Oid" } }
    $lines = [Text.Encoding]::Latin1.GetString($headerBytes).Split([char]10)
    if ($lines.Count -lt 3) { throw "Raw commit has too few canonical headers: $Oid" }
    $treeMatch = [regex]::Match($lines[0], '^tree (?<oid>[0-9a-f]{40})$')
    if (-not $treeMatch.Success) { throw "Raw commit tree header is malformed: $Oid" }
    $tree = $treeMatch.Groups['oid'].Value
    $parents = [Collections.Generic.List[string]]::new()
    [int]$index = 1
    while ($index -lt $lines.Count) {
        $parentMatch = [regex]::Match($lines[$index], '^parent (?<oid>[0-9a-f]{40})$')
        if (-not $parentMatch.Success) { break }
        $parents.Add($parentMatch.Groups['oid'].Value)
        $index++
    }
    if ($index -lt $lines.Count -and $lines[$index] -cmatch '^(?:tree|parent)(?:[\x00-\x20]|$)') {
        throw "Raw commit tree/parent header is malformed or out of order: $Oid"
    }
    if ($index -ge $lines.Count -or -not [regex]::IsMatch($lines[$index], '^author .+ <[^<>]*> [0-9]+ [+-][0-9]{4}$')) {
        throw "Raw commit author header is malformed or out of order: $Oid"
    }
    $index++
    if ($index -ge $lines.Count -or -not [regex]::IsMatch($lines[$index], '^committer .+ <[^<>]*> [0-9]+ [+-][0-9]{4}$')) {
        throw "Raw commit committer header is malformed or out of order: $Oid"
    }
    $index++
    [bool]$hasOptionalHeader = $false
    while ($index -lt $lines.Count) {
        $line = $lines[$index]
        if ($line.StartsWith(' ', [StringComparison]::Ordinal)) {
            if (-not $hasOptionalHeader) { throw "Raw commit has an orphan header continuation: $Oid" }
            $index++
            continue
        }
        if ($line -cmatch '^(?:tree|parent|author|committer)(?:[\x00-\x20]|$)') {
            throw "Raw commit contains a repeated or out-of-order reserved header: $Oid"
        }
        if ($line -cnotmatch '^[A-Za-z0-9][A-Za-z0-9-]* .+$') { throw "Raw commit contains a malformed optional header: $Oid" }
        $hasOptionalHeader = $true
        $index++
    }
    [pscustomobject]@{ Oid = $Oid; Tree = $tree; Parents = $parents.ToArray() }
}

function Get-HeadTreeEntries([string] $TreeOid) {
    Assert-LowerHex $TreeOid 40 'raw tree OID'
    $records = @(ConvertFrom-NulSeparatedUtf8 -Bytes (Invoke-GitBytes -Arguments @('ls-tree','-r','-z','--full-tree',$TreeOid)).Bytes -Label 'HEAD tree listing')
    $entries = [Collections.Generic.List[object]]::new()
    foreach ($record in $records) {
        $match = [regex]::Match($record, '^(?<mode>[0-7]{6}) (?<type>[a-z]+) (?<oid>[0-9a-f]{40})\t(?<path>.+)$')
        if (-not $match.Success) { throw 'Malformed HEAD tree entry.' }
        $mode = $match.Groups['mode'].Value
        $type = $match.Groups['type'].Value
        $path = $match.Groups['path'].Value
        if ($type -cne 'blob' -or $mode -notin @('100644','100755')) { throw "Unsupported tracked entry type/mode: $path" }
        if ($path.Contains('\') -or [IO.Path]::IsPathFullyQualified($path) -or @($path.Split('/') | Where-Object { $_ -in @('','.','..') }).Count -ne 0) { throw "Unsafe tracked repository path: $path" }
        $entries.Add([pscustomobject]@{ Mode = $mode; Oid = $match.Groups['oid'].Value; Path = $path })
    }
    $entries.ToArray()
}

function Read-AsciiLine([IO.Stream] $Stream, [string] $Label) {
    $bytes = [Collections.Generic.List[byte]]::new()
    while ($true) {
        $value = $Stream.ReadByte()
        if ($value -lt 0) { throw "Unexpected EOF while reading $Label." }
        if ($value -eq 10) { break }
        if ($value -gt 127 -or $value -eq 13) { throw "Noncanonical ASCII line while reading $Label." }
        $bytes.Add([byte]$value)
    }
    [Text.Encoding]::ASCII.GetString($bytes.ToArray())
}

function Read-ExactBytes([IO.Stream] $Stream, [int] $Length, [string] $Label) {
    $bytes = [byte[]]::new($Length)
    [int]$offset = 0
    while ($offset -lt $Length) {
        $read = $Stream.Read($bytes, $offset, $Length - $offset)
        if ($read -le 0) { throw "Unexpected EOF while reading $Label." }
        $offset += $read
    }
    $bytes
}

function Get-GitBatchBlobs([object[]] $Entries) {
    $oids = @($Entries | ForEach-Object { $_.Oid } | Sort-Object -Unique -CaseSensitive)
    $result = [Collections.Generic.Dictionary[string,byte[]]]::new([StringComparer]::Ordinal)
    $psi = [Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = 'git'
    $psi.WorkingDirectory = $script:RepositoryRoot
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true
    foreach ($name in @('GIT_DIR','GIT_WORK_TREE','GIT_COMMON_DIR','GIT_INDEX_FILE','GIT_OBJECT_DIRECTORY','GIT_ALTERNATE_OBJECT_DIRECTORIES','GIT_NAMESPACE','GIT_CEILING_DIRECTORIES','GIT_DISCOVERY_ACROSS_FILESYSTEM','GIT_CONFIG','GIT_CONFIG_GLOBAL','GIT_CONFIG_SYSTEM','GIT_CONFIG_COUNT','GIT_REPLACE_REF_BASE')) { $null = $psi.Environment.Remove($name) }
    $psi.Environment['GIT_NO_REPLACE_OBJECTS'] = '1'
    $psi.Environment['GIT_OPTIONAL_LOCKS'] = '0'
    $psi.Environment['GIT_CONFIG_NOSYSTEM'] = '1'
    $psi.Environment['GIT_TERMINAL_PROMPT'] = '0'
    foreach ($argument in @('-c','core.hooksPath=NUL','-C',$script:RepositoryRoot,'cat-file','--batch')) { $psi.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($psi)
    try {
        $errorTask = $process.StandardError.ReadToEndAsync()
        $process.StandardInput.NewLine = "`n"
        foreach ($oid in $oids) {
            # Keep the batch protocol duplex-safe: submit exactly one request, then
            # drain its complete response before another request can fill stdin.
            $process.StandardInput.WriteLine($oid)
            $process.StandardInput.Flush()
            $header = Read-AsciiLine -Stream $process.StandardOutput.BaseStream -Label "blob header $oid"
            $match = [regex]::Match($header, '^([0-9a-f]{40}) blob ([0-9]+)$')
            if (-not $match.Success -or $match.Groups[1].Value -cne $oid) { throw "Unexpected cat-file batch header: $header" }
            [int64]$length64 = [int64]::Parse($match.Groups[2].Value, [Globalization.CultureInfo]::InvariantCulture)
            if ($length64 -gt [int]::MaxValue) { throw "Tracked blob is too large to verify safely: $oid" }
            [byte[]]$blob = Read-ExactBytes -Stream $process.StandardOutput.BaseStream -Length ([int]$length64) -Label "blob $oid"
            if ($process.StandardOutput.BaseStream.ReadByte() -ne 10) { throw "Missing cat-file batch separator: $oid" }
            $result.Add($oid, $blob)
        }
        $process.StandardInput.Close()
        $process.WaitForExit()
        $errorText = $errorTask.GetAwaiter().GetResult()
        if ($process.ExitCode -ne 0) { throw "git cat-file --batch failed ($($process.ExitCode)): $errorText" }
        if ($process.StandardOutput.BaseStream.ReadByte() -ne -1) { throw 'git cat-file --batch emitted unexpected trailing bytes.' }
        $result
    }
    finally { $process.Dispose() }
}

function Get-IndexTreeEntries {
    $records = @(ConvertFrom-NulSeparatedUtf8 -Bytes (Invoke-GitBytes -Arguments @('ls-files','--stage','-z','--')).Bytes -Label 'index listing')
    $entries = [Collections.Generic.List[string]]::new()
    foreach ($record in $records) {
        $match = [regex]::Match($record, '^(?<mode>[0-7]{6}) (?<oid>[0-9a-f]{40}) (?<stage>[0-3])\t(?<path>.+)$')
        if (-not $match.Success -or $match.Groups['stage'].Value -cne '0') { throw 'Index contains a malformed or non-stage-zero entry.' }
        $entries.Add("$($match.Groups['mode'].Value)|$($match.Groups['oid'].Value)|$($match.Groups['path'].Value)")
    }
    $entries.ToArray()
}

function Assert-IndexAndStatusMatchHead([object[]] $Entries) {
    $expected = @($Entries | ForEach-Object { "$($_.Mode)|$($_.Oid)|$($_.Path)" })
    $actual = @(Get-IndexTreeEntries)
    if ([string]::Join("`n", $actual) -cne [string]::Join("`n", $expected)) { throw 'Index entries differ from the raw HEAD tree.' }
    $flags = @(ConvertFrom-NulSeparatedUtf8 -Bytes (Invoke-GitBytes -Arguments @('ls-files','-v','-z','--')).Bytes -Label 'index flag listing')
    $expectedFlags = @($Entries | ForEach-Object { "H $($_.Path)" })
    if ([string]::Join("`n", $flags) -cne [string]::Join("`n", $expectedFlags)) { throw 'Index contains assume-unchanged, skip-worktree, or another concealed state.' }
    $untracked = (Invoke-GitBytes -Arguments @('ls-files','--others','--exclude-standard','-z','--')).Bytes
    if ($untracked.Length -ne 0) { throw 'Publisher requires no untracked nonignored worktree paths.' }
    $status = (Invoke-Git -Arguments @('status','--porcelain=v1','--untracked-files=all')).Stdout
    if (-not [string]::IsNullOrEmpty($status)) { throw 'Publisher requires a clean worktree and index.' }
}

function New-ExactRepositoryState([string] $TreeOid) {
    $entries = @(Get-HeadTreeEntries -TreeOid $TreeOid)
    $blobs = Get-GitBatchBlobs -Entries $entries
    $expected = [Collections.Generic.List[object]]::new()
    foreach ($entry in $entries) {
        $expected.Add([pscustomobject]@{ Mode = $entry.Mode; Oid = $entry.Oid; Path = $entry.Path; Bytes = $blobs[$entry.Oid] })
    }
    $state = [pscustomobject]@{ Tree = $TreeOid; Entries = $expected.ToArray() }
    Assert-ExactRepositoryState -Expected $state
    $state
}

function Assert-ExactRepositoryState([object] $Expected) {
    Assert-IndexAndStatusMatchHead -Entries $Expected.Entries
    $verifiedDirectories = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in $Expected.Entries) {
        $current = $script:RepositoryRoot
        $parts = $entry.Path.Split('/')
        for ($index = 0; $index -lt ($parts.Count - 1); $index++) {
            $current = Join-Path $current $parts[$index]
            if ($verifiedDirectories.Add($current)) {
                $directory = Get-Item -LiteralPath $current -Force
                if (-not $directory.PSIsContainer -or ($directory.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                    throw "Tracked path has an unsafe directory ancestor: $($entry.Path)"
                }
            }
        }
        $path = Join-Path $script:RepositoryRoot $entry.Path
        $item = Get-Item -LiteralPath $path -Force
        if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Tracked path is not a safe regular file: $($entry.Path)" }
        [byte[]]$actual = [IO.File]::ReadAllBytes($path)
        [byte[]]$crlf = ConvertTo-DeterministicCrlfBytes $entry.Bytes
        if (-not (Test-BytesEqual $actual $entry.Bytes) -and -not (Test-BytesEqual $actual $crlf)) {
            throw "Tracked worktree bytes differ from the raw HEAD blob: $($entry.Path)"
        }
    }
}

function Get-AclRecord([string] $Path) {
    if (-not $IsWindows) { throw 'Wave 0 ACL fingerprinting currently requires Windows.' }
    $item = Get-Item -LiteralPath $Path -Force
    $acl = [IO.FileSystemAclExtensions]::GetAccessControl($item)
    [pscustomobject]@{
        Owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
        Group = $acl.GetGroup([Security.Principal.SecurityIdentifier]).Value
        InheritanceProtected = [bool]$acl.AreAccessRulesProtected
        Descriptor = $acl.GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::Access)
    }
}

function Get-PathRecord([string] $Path) {
    $full = [IO.Path]::GetFullPath($Path)
    if (-not (Test-Path -LiteralPath $full)) { throw "Path does not exist: $full" }
    $item = Get-Item -LiteralPath $full -Force
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Reparse point rejected: $full" }
    try { $final = [DynamoPlanPublisherNative]::GetFinalPath($full) }
    catch { throw "Could not resolve native final path for $full`: $($_.Exception.Message)" }
    if ($final -cne $full) { throw "Lexical/final path mismatch: $full -> $final" }
    $identity = [DynamoPlanPublisherNative]::GetIdentity($full)
    $parts = $identity.Split(':')
    if ($parts.Count -ne 2) { throw "Malformed native identity: $full" }
    $acl = Get-AclRecord $full
    $identityValue = [ordered]@{
        platform = 'windows'
        volume_or_device = $parts[0]
        file_id_or_inode = $parts[1]
    }
    $aclValue = [ordered]@{
        platform = 'windows'
        owner = $acl.Owner
        group = $acl.Group
        inheritance_protected = $acl.InheritanceProtected
        descriptor = $acl.Descriptor
    }
    [pscustomobject]@{
        Path = $full
        Identity = $identity
        IdentitySha256 = Get-DomainHash 'dynamo-native-identity-v1' (ConvertTo-CanonicalBytes $identityValue)
        Owner = $acl.Owner
        AclSha256 = Get-DomainHash 'dynamo-acl-fingerprint-v1' (ConvertTo-CanonicalBytes $aclValue)
    }
}

function Assert-RestrictedAcl {
    param(
        [string] $Path,
        [ValidateSet('Directory','File')][string] $Kind
    )
    if (-not $IsWindows) { throw 'Wave 0 restrictive ACL validation currently requires Windows.' }
    $item = Get-Item -LiteralPath $Path -Force
    if ($Kind -eq 'Directory' -and -not $item.PSIsContainer) { throw "Expected restricted directory: $Path" }
    if ($Kind -eq 'File' -and $item.PSIsContainer) { throw "Expected restricted file: $Path" }
    $acl = [IO.FileSystemAclExtensions]::GetAccessControl($item)
    $current = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    $system = 'S-1-5-18'
    $owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
    if ($owner -cne $current) { throw "Restricted ACL owner mismatch: $Path" }
    if ($Kind -eq 'Directory' -and -not $acl.AreAccessRulesProtected) { throw "Restricted directory inherits ACLs: $Path" }
    $rules = @($acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]))
    if ($rules.Count -ne 2) { throw "Restricted ACL must contain exactly current-user and SYSTEM rules: $Path" }
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($rule in $rules) {
        $sid = $rule.IdentityReference.Value
        if ($sid -cne $current -and $sid -cne $system) { throw "Restricted ACL contains an unexpected principal: $Path" }
        if (-not $seen.Add($sid)) { throw "Restricted ACL contains a duplicate principal: $Path" }
        if ($rule.AccessControlType -ne [Security.AccessControl.AccessControlType]::Allow -or
            $rule.FileSystemRights -ne [Security.AccessControl.FileSystemRights]::FullControl -or
            $rule.PropagationFlags -ne [Security.AccessControl.PropagationFlags]::None) {
            throw "Restricted ACL rule is not exact full control: $Path"
        }
        if ($Kind -eq 'Directory') {
            $expectedInheritance = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
            if ($rule.IsInherited -or $rule.InheritanceFlags -ne $expectedInheritance) { throw "Restricted directory ACL rule shape mismatch: $Path" }
        } elseif ($rule.InheritanceFlags -ne [Security.AccessControl.InheritanceFlags]::None) {
            throw "Restricted file ACL rule shape mismatch: $Path"
        }
    }
    if (-not $seen.Contains($current) -or -not $seen.Contains($system)) { throw "Restricted ACL principal set mismatch: $Path" }
}

function Assert-SafeExistingPath {
    param([string] $Path, [ValidateSet('Any','File','Directory')][string] $LeafType = 'Any')
    $full = Resolve-ReparseFreeExistingPath -Path $Path
    $leaf = Get-Item -LiteralPath $full -Force
    if ($LeafType -eq 'File' -and $leaf.PSIsContainer) { throw "Expected regular file: $full" }
    if ($LeafType -eq 'Directory' -and -not $leaf.PSIsContainer) { throw "Expected directory: $full" }
    Get-PathRecord $full
}

function Set-RestrictiveDirectory([string] $Path) {
    $current = [Security.Principal.WindowsIdentity]::GetCurrent().User
    $system = [Security.Principal.SecurityIdentifier]::new('S-1-5-18')
    $security = [Security.AccessControl.DirectorySecurity]::new()
    $security.SetOwner($current)
    $security.SetAccessRuleProtection($true, $false)
    $inheritance = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
    foreach ($sid in @($current, $system)) {
        $rule = [Security.AccessControl.FileSystemAccessRule]::new($sid, [Security.AccessControl.FileSystemRights]::FullControl, $inheritance, [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Allow)
        $null = $security.AddAccessRule($rule)
    }
    [IO.FileSystemAclExtensions]::SetAccessControl([IO.DirectoryInfo]::new($Path), $security)
    Assert-RestrictedAcl -Path $Path -Kind Directory
}

function Ensure-SafeDirectoryChain([string] $TrustedRoot, [string[]] $Children) {
    $current = (Assert-SafeExistingPath -Path $TrustedRoot -LeafType Directory).Path
    foreach ($child in $Children) {
        if ($child -notmatch '^[A-Za-z0-9._-]+$') { throw "Unsafe directory component: $child" }
        $next = Join-Path $current $child
        if (-not (Test-Path -LiteralPath $next)) {
            try {
                $security = [Security.AccessControl.DirectorySecurity]::new()
                $currentSid = [Security.Principal.WindowsIdentity]::GetCurrent().User
                $system = [Security.Principal.SecurityIdentifier]::new('S-1-5-18')
                $security.SetOwner($currentSid)
                $security.SetAccessRuleProtection($true, $false)
                $inheritance = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
                foreach ($sid in @($currentSid, $system)) {
                    $rule = [Security.AccessControl.FileSystemAccessRule]::new($sid, [Security.AccessControl.FileSystemRights]::FullControl, $inheritance, [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Allow)
                    $null = $security.AddAccessRule($rule)
                }
                [IO.FileSystemAclExtensions]::CreateDirectory($security, $next) | Out-Null
            } catch [IO.IOException] {
                if (-not (Test-Path -LiteralPath $next -PathType Container)) { throw }
            }
        }
        $record = Assert-SafeExistingPath -Path $next -LeafType Directory
        Assert-RestrictedAcl -Path $next -Kind Directory
        $prefix = $current.TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar
        if (-not $record.Path.StartsWith($prefix, [StringComparison]::Ordinal)) { throw "Directory escaped trusted root: $next" }
        $current = $record.Path
    }
    $current
}

function Get-MachineIdentitySha256 {
    try {
        $machine = (Get-ItemPropertyValue -LiteralPath 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Cryptography' -Name MachineGuid -ErrorAction Stop).ToString().ToLowerInvariant()
    } catch { throw "Machine identity is unavailable: $($_.Exception.Message)" }
    Get-DomainHash 'dynamo-machine-identity-v1' $script:Utf8.GetBytes($machine)
}

function Get-ProcessStartIdentity([int] $ProcessId) {
    try {
        $process = [Diagnostics.Process]::GetProcessById($ProcessId)
        try { $raw = $process.StartTime.ToUniversalTime().ToFileTimeUtc().ToString([Globalization.CultureInfo]::InvariantCulture) }
        finally { $process.Dispose() }
        Get-DomainHash 'dynamo-process-birth-v1' $script:Utf8.GetBytes($raw)
    } catch [ArgumentException] { $null }
    catch { throw "Process birth identity is ambiguous for PID ${ProcessId}: $($_.Exception.Message)" }
}

function New-Owner([string] $AttemptId, [string] $CommonIdentitySha256) {
    [ordered]@{
        machine_identity_sha256 = Get-MachineIdentitySha256
        pid = [int64]$PID
        process_start_identity = Get-ProcessStartIdentity $PID
        attempt_id = $AttemptId
        git_common_dir_identity_sha256 = $CommonIdentitySha256
    }
}

function Assert-OwnerDead([Collections.IDictionary] $Owner, [string] $CommonIdentitySha256) {
    $keys = @('machine_identity_sha256','pid','process_start_identity','attempt_id','git_common_dir_identity_sha256')
    if ([string]::Join("`n", @($Owner.Keys)) -cne [string]::Join("`n", $keys)) { throw 'Publication owner schema mismatch.' }
    if ([string]$Owner['machine_identity_sha256'] -cne (Get-MachineIdentitySha256)) { throw 'Remote publication owner cannot be recovered.' }
    if ([string]$Owner['git_common_dir_identity_sha256'] -cne $CommonIdentitySha256) { throw 'Publication owner common-directory identity mismatch.' }
    $current = Get-ProcessStartIdentity ([int]$Owner['pid'])
    if ($null -eq $current) { return }
    if ($current -ceq [string]$Owner['process_start_identity']) { throw 'Active publication lease owner is still alive.' }
}

function Assert-LowerHex([string] $Value, [int] $Length, [string] $Label) {
    if ($Value -cnotmatch "^[0-9a-f]{$Length}$") { throw "Invalid $Label" }
}

function Assert-JsonInt64([AllowNull()][object] $Value, [string] $Label) {
    if ($Value -isnot [int64]) { throw "$Label must be an actual JSON integer." }
}

function Invoke-PublishFailpoint([string] $Name) {
    $requested = [Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_TEST_FAILPOINT', 'Process')
    if ($requested -eq $Name) {
        [Console]::Error.WriteLine("Injected publisher failpoint: $Name")
        [Environment]::Exit(97)
    }
}

function Assert-InitialEnvironment([string] $EvidenceRoot) {
    $failpoint = [Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_TEST_FAILPOINT', 'Process')
    $barrier = [Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_PUBLISH_BARRIER', 'Process')
    if ($failpoint -and $failpoint -notin $script:PublisherFailpoints) { throw "Unknown publisher failpoint: $failpoint" }
    if ($barrier -and $barrier -notin $script:PublisherBarriers) { throw "Unknown publisher barrier: $barrier" }
    if ($failpoint -or $barrier) {
        if ([Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_TEST_MODE', 'Process') -cne '1') {
            throw 'Publisher test controls require explicit test mode.'
        }
        $temp = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar
        $repo = [IO.Path]::GetFullPath($script:RepositoryRoot).TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar
        $root = [IO.Path]::GetFullPath($EvidenceRoot).TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar
        if (-not $repo.StartsWith($temp, [StringComparison]::OrdinalIgnoreCase) -or -not $root.StartsWith($temp, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Publisher test controls are accepted only in OS-temp fixtures.'
        }
    }
}

function Get-GitBlobBytes([string] $RelativePath) {
    $psi = [Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = 'git'
    $psi.WorkingDirectory = $script:RepositoryRoot
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true
    foreach ($name in @('GIT_DIR','GIT_WORK_TREE','GIT_COMMON_DIR','GIT_INDEX_FILE','GIT_OBJECT_DIRECTORY','GIT_ALTERNATE_OBJECT_DIRECTORIES','GIT_NAMESPACE','GIT_CEILING_DIRECTORIES','GIT_DISCOVERY_ACROSS_FILESYSTEM','GIT_CONFIG','GIT_CONFIG_GLOBAL','GIT_CONFIG_SYSTEM','GIT_CONFIG_COUNT','GIT_REPLACE_REF_BASE')) { $null = $psi.Environment.Remove($name) }
    $psi.Environment['GIT_CONFIG_NOSYSTEM'] = '1'
    $psi.Environment['GIT_NO_REPLACE_OBJECTS'] = '1'
    $psi.Environment['GIT_OPTIONAL_LOCKS'] = '0'
    $psi.Environment['GIT_TERMINAL_PROMPT'] = '0'
    foreach ($argument in @('-C',$script:RepositoryRoot,'cat-file','blob',"HEAD:$RelativePath")) { $psi.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($psi)
    $memory = [IO.MemoryStream]::new()
    try {
        $copyTask = $process.StandardOutput.BaseStream.CopyToAsync($memory)
        $errorTask = $process.StandardError.ReadToEndAsync()
        $null = $copyTask.GetAwaiter().GetResult()
        $errorText = $errorTask.GetAwaiter().GetResult()
        $process.WaitForExit()
        if ($process.ExitCode -ne 0) { throw "Could not read committed blob $RelativePath`: $errorText" }
        $memory.ToArray()
    } finally {
        $memory.Dispose()
        $process.Dispose()
    }
}

function Assert-CleanRepositoryState {
    if ($null -ne $script:ExactRepositoryState) {
        Assert-ExactRepositoryState -Expected $script:ExactRepositoryState
        return
    }
    $status = (Invoke-Git -Arguments @('status','--porcelain=v1','--untracked-files=all')).Stdout
    if (-not [string]::IsNullOrEmpty($status)) { throw 'Publisher requires a clean worktree and index throughout publication.' }
}

function ConvertTo-DeterministicCrlfBytes([byte[]] $Bytes) {
    $result = [Collections.Generic.List[byte]]::new($Bytes.Length + 256)
    [byte]$previous = 0
    foreach ($byte in $Bytes) {
        if ($byte -eq 10 -and $previous -ne 13) { $result.Add(13) }
        $result.Add($byte)
        $previous = $byte
    }
    $result.ToArray()
}

function Get-MatchedHeadBlobBytes([string] $RelativePath, [string] $NativePath) {
    [byte[]]$headBytes = Get-GitBlobBytes $RelativePath
    [byte[]]$workingBytes = [IO.File]::ReadAllBytes($NativePath)
    [byte[]]$crlfBytes = ConvertTo-DeterministicCrlfBytes $headBytes
    if (-not (Test-BytesEqual $workingBytes $headBytes) -and -not (Test-BytesEqual $workingBytes $crlfBytes)) {
        throw "Working bytes differ from the committed HEAD blob under the only accepted LF/CRLF encodings: $RelativePath"
    }
    Write-Output -NoEnumerate $headBytes
}

function Assert-WorkingPathMatchesHead([string] $RelativePath, [string] $NativePath) {
    Get-MatchedHeadBlobBytes -RelativePath $RelativePath -NativePath $NativePath | Out-Null
}

function Get-ExecutionAnchor {
    $head = (Invoke-Git -Arguments @('rev-parse','HEAD')).Stdout
    Assert-LowerHex $head 40 'execution baseline'
    $commit = Get-RawCommitRecord -Oid $head
    if ($commit.Parents.Count -ne 1 -or $commit.Parents[0] -cne $script:AuditBaseline) {
        throw 'Execution baseline must be the one bootstrap commit over the audit baseline.'
    }
    $auditCommit = Get-RawCommitRecord -Oid $script:AuditBaseline
    $changed = @(
        ConvertFrom-NulSeparatedUtf8 -Bytes (
            Invoke-GitBytes -Arguments @('diff-tree','--no-commit-id','--name-only','-z','-r','--no-renames','--no-ext-diff','--no-textconv',$auditCommit.Tree,$commit.Tree,'--')
        ).Bytes -Label 'bootstrap raw-tree diff' |
            Sort-Object -CaseSensitive
    )
    if ([string]::Join("`n", $changed) -cne [string]::Join("`n", $script:ControlPaths)) {
        throw 'Bootstrap commit does not change exactly the four control files.'
    }
    [ordered]@{
        head = $head
        tree = $commit.Tree
        parent = [string]$commit.Parents[0]
        changed = $changed
    }
}

function Assert-ExecutionAnchorEqual([Collections.IDictionary] $Expected, [Collections.IDictionary] $Actual) {
    if (-not (Test-BytesEqual (ConvertTo-CanonicalBytes $Expected) (ConvertTo-CanonicalBytes $Actual))) {
        throw 'Execution HEAD/parent/exact-diff anchor changed during publication.'
    }
}

function Get-FileSnapshot {
    param([string] $RelativePath, [switch] $FromHead)
    if ($RelativePath.Contains('\')) { throw "Repository path must use slashes: $RelativePath" }
    $path = Join-Path $script:RepositoryRoot $RelativePath
    $before = Assert-SafeExistingPath -Path $path -LeafType File
    [byte[]]$bytes = if ($FromHead) {
        Get-MatchedHeadBlobBytes -RelativePath $RelativePath -NativePath $path
    } else {
        [IO.File]::ReadAllBytes($path)
    }
    $after = Assert-SafeExistingPath -Path $path -LeafType File
    if ($before.IdentitySha256 -cne $after.IdentitySha256 -or $before.Owner -cne $after.Owner -or $before.AclSha256 -cne $after.AclSha256) {
        throw "Source/control path identity, owner, or ACL changed during read: $RelativePath"
    }
    [pscustomobject]@{
        Row = [ordered]@{
            path = $RelativePath
            bytes = [int64]$bytes.LongLength
            sha256 = Get-Sha256Bytes $bytes
        }
        PathRecord = [ordered]@{
            path = $RelativePath
            identity_sha256 = $after.IdentitySha256
            owner = $after.Owner
            acl_sha256 = $after.AclSha256
        }
    }
}

function Get-SourceSnapshot {
    Assert-CleanRepositoryState
    $anchorBefore = Get-ExecutionAnchor
    $payloadSnapshots = @($script:PayloadPaths | ForEach-Object { Get-FileSnapshot $_ })
    $controlSnapshots = @($script:ControlPaths | ForEach-Object { Get-FileSnapshot -RelativePath $_ -FromHead })
    $anchorAfter = Get-ExecutionAnchor
    Assert-ExecutionAnchorEqual -Expected $anchorBefore -Actual $anchorAfter
    Assert-CleanRepositoryState
    $payloads = @($payloadSnapshots | ForEach-Object { $_.Row })
    $controls = @($controlSnapshots | ForEach-Object { $_.Row })
    [pscustomobject]@{
        Payloads = $payloads
        Controls = $controls
        PathRecords = @(@($payloadSnapshots) + @($controlSnapshots) | ForEach-Object { $_.PathRecord })
        ExecutionAnchor = $anchorAfter
        Fingerprint = Get-DomainHash 'dynamo-source-set-v1' (ConvertTo-CanonicalBytes ([ordered]@{
            payloads = $payloads
            controls = $controls
        }))
    }
}

function Initialize-ControlSchema {
    $path = Join-Path $script:RepositoryRoot $script:ControlSchemaPath
    $before = Assert-SafeExistingPath -Path $path -LeafType File
    [byte[]]$headBytes = Get-MatchedHeadBlobBytes -RelativePath $script:ControlSchemaPath -NativePath $path
    $after = Assert-SafeExistingPath -Path $path -LeafType File
    if ($before.IdentitySha256 -cne $after.IdentitySha256 -or $before.Owner -cne $after.Owner -or $before.AclSha256 -cne $after.AclSha256) { throw 'Control schema path identity, owner, or ACL changed during read.' }
    $document = [Text.Json.JsonDocument]::Parse($script:Utf8.GetString($headBytes), [Text.Json.JsonDocumentOptions]@{ AllowTrailingCommas = $false; CommentHandling = [Text.Json.JsonCommentHandling]::Disallow })
    try { $value = Convert-JsonElement $document.RootElement } finally { $document.Dispose() }
    if (-not (Test-BytesEqual $headBytes (ConvertTo-CanonicalBytes $value)) -or [string]::Join("`n", @($value.Keys)) -cne "schema_version`ncontrols") { throw 'Control schema must be canonical JSON with its exact v2 keys.' }
    Assert-JsonInt64 $value['schema_version'] 'Control schema schema_version'
    if ([int64]$value['schema_version'] -ne 2) { throw 'Unsupported control schema version.' }
    $controls = @($value['controls'])
    if ($controls.Count -lt 1 -or @($controls | Where-Object { $_ -isnot [string] -or $_ -notmatch '^[a-zA-Z0-9._/-]+$' -or $_ -match '(^|/)\.\.(/|$)' }).Count -ne 0) {
        throw 'Control schema contains an invalid repository-relative control path.'
    }
    $ordinal = @($controls | Sort-Object -CaseSensitive)
    if ([string]::Join("`n", $controls) -cne [string]::Join("`n", $ordinal) -or @($controls | Sort-Object -Unique -CaseSensitive).Count -ne $controls.Count) {
        throw 'Control schema paths must be ordinal-sorted and unique.'
    }
    if ($controls -notcontains $script:ControlSchemaPath) { throw 'Control schema must protect itself.' }
    $script:ControlPaths = $controls
    $script:ControlSchema = [pscustomobject]@{
        Path = $script:ControlSchemaPath
        Version = [int64]$value['schema_version']
        Sha256 = Get-Sha256Bytes $headBytes
        Controls = $controls
    }
}

function Assert-SnapshotEqual([object] $Expected, [object] $Actual) {
    $left = ConvertTo-CanonicalBytes ([ordered]@{ payloads = @($Expected.Payloads); controls = @($Expected.Controls) })
    $right = ConvertTo-CanonicalBytes ([ordered]@{ payloads = @($Actual.Payloads); controls = @($Actual.Controls) })
    if (-not (Test-BytesEqual $left $right)) {
        throw 'Reviewed source/control bytes changed during publication.'
    }
    if (-not (Test-BytesEqual (ConvertTo-CanonicalBytes @($Expected.PathRecords)) (ConvertTo-CanonicalBytes @($Actual.PathRecords)))) {
        throw 'Reviewed source/control path identity, owner, or ACL changed during publication.'
    }
    Assert-ExecutionAnchorEqual -Expected $Expected.ExecutionAnchor -Actual $Actual.ExecutionAnchor
}

function Get-GitIgnoreEvidence {
    $rows = [Collections.Generic.List[object]]::new()
    $lineCache = @{}
    foreach ($path in $script:PayloadPaths) {
        $result = Invoke-Git -Arguments @('check-ignore','-v','--no-index','--',$path)
        $text = $result.Stdout
        $tab = $text.IndexOf("`t", [StringComparison]::Ordinal)
        if ($tab -lt 1) { throw "Could not parse gitignore evidence for $path" }
        $origin = $text.Substring(0, $tab)
        $match = [regex]::Match($origin, '^(?<source>[^:]+):(?<line>[0-9]+):(?<pattern>.*)$')
        if (-not $match.Success) { throw "Could not parse gitignore origin for $path" }
        $source = $match.Groups['source'].Value.Replace('\','/')
        if ([IO.Path]::IsPathFullyQualified($source) -or $source.StartsWith('../', [StringComparison]::Ordinal)) { throw 'Only repository ignore files are accepted.' }
        $line = [int64]::Parse($match.Groups['line'].Value, [Globalization.CultureInfo]::InvariantCulture)
        $pattern = $match.Groups['pattern'].Value
        $ignorePath = Join-Path $script:RepositoryRoot $source
        Assert-SafeExistingPath -Path $ignorePath -LeafType File | Out-Null
        Assert-WorkingPathMatchesHead -RelativePath $source -NativePath $ignorePath
        if (-not $lineCache.ContainsKey($source)) { $lineCache[$source] = [IO.File]::ReadAllLines($ignorePath) }
        $lines = $lineCache[$source]
        if ($line -lt 1 -or $line -gt $lines.Length -or $lines[$line - 1] -cne $pattern) { throw "Gitignore evidence does not match committed source for $path" }
        $headIgnore = Invoke-Git -Arguments @('cat-file','-e',"HEAD:$source") -AllowFailure
        if ($headIgnore.ExitCode -ne 0) { throw "Ignore source is not committed at HEAD: $source" }
        $rows.Add([ordered]@{
            path = $path
            rule_source = $source
            rule_line = $line
            pattern = $pattern
        })
    }
    $rows.ToArray()
}

function Assert-GitIgnoreEvidenceEqual([object[]] $Expected, [object[]] $Actual) {
    if (-not (Test-BytesEqual (ConvertTo-CanonicalBytes @($Expected)) (ConvertTo-CanonicalBytes @($Actual)))) {
        throw 'Gitignore evidence changed during publication.'
    }
}

function Test-ContainedPath([string] $Candidate, [string] $Container) {
    $candidateFull = [IO.Path]::GetFullPath($Candidate).TrimEnd('\','/')
    $containerFull = [IO.Path]::GetFullPath($Container).TrimEnd('\','/')
    $candidateFull -ceq $containerFull -or $candidateFull.StartsWith($containerFull + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)
}

function Assert-PublisherPreflight([string] $EvidenceRoot) {
    if (-not [IO.Path]::IsPathFullyQualified($EvidenceRoot)) { throw 'Evidence root must be absolute.' }
    $rootRecord = Assert-SafeExistingPath -Path $EvidenceRoot -LeafType Directory

    $scriptRecord = Assert-SafeExistingPath -Path $PSCommandPath -LeafType File
    $expectedScript = [IO.Path]::GetFullPath((Join-Path $script:RepositoryRoot 'scripts/remediation/publish-plan-set.ps1'))
    if ($scriptRecord.Path -cne $expectedScript) { throw 'Publisher is not running from its canonical repository path.' }
    $top = [IO.Path]::GetFullPath((Invoke-Git -Arguments @('rev-parse','--show-toplevel')).Stdout)
    if ($top -cne $script:RepositoryRoot) { throw 'Publisher-derived and Git repository roots differ.' }

    Assert-CleanRepositoryState
    $anchor = Get-ExecutionAnchor
    $head = [string]$anchor['head']
    $script:ExactRepositoryState = New-ExactRepositoryState -TreeOid ([string]$anchor['tree'])
    Assert-WorkingPathMatchesHead -RelativePath 'scripts/remediation/publish-plan-set.ps1' -NativePath $PSCommandPath

    $worktreeOutput = (Invoke-Git -Arguments @('worktree','list','--porcelain')).Stdout
    $worktrees = [Collections.Generic.List[string]]::new()
    foreach ($line in ($worktreeOutput -split "`n")) {
        if ($line.StartsWith('worktree ', [StringComparison]::Ordinal)) {
            $path = [IO.Path]::GetFullPath($line.Substring(9))
            if (-not (Test-Path -LiteralPath $path -PathType Container)) { throw "Registered worktree is missing/prunable: $path" }
            Assert-SafeExistingPath -Path $path -LeafType Directory | Out-Null
            $worktrees.Add($path)
        }
    }
    if ($worktrees.Count -eq 0) { throw 'Git reported no registered worktree.' }
    foreach ($worktree in $worktrees) {
        if (Test-ContainedPath -Candidate $rootRecord.Path -Container $worktree) { throw 'Evidence root may not be inside or equal to a worktree.' }
    }

    $reviewedDirectory = Join-Path $script:RepositoryRoot 'docs/superpowers/plans'
    $actualReviewed = @(
        Get-ChildItem -LiteralPath $reviewedDirectory -File -Force |
            Where-Object { $_.Name -like '2026-07-12-*.md' -or $_.Name -eq '2026-07-13-wave0-bootstrap.md' } |
            ForEach-Object { "docs/superpowers/plans/$($_.Name)" } |
            Sort-Object -CaseSensitive
    )
    if ([string]::Join("`n", $actualReviewed) -cne [string]::Join("`n", $script:PayloadPaths)) { throw 'Reviewed plan source set is missing or contains an unreviewed extra.' }

    [pscustomobject]@{
        EvidenceRoot = $rootRecord
        ExecutionBaseline = $head
        ExecutionAnchor = $anchor
        Worktrees = $worktrees.ToArray()
    }
}

function Invoke-PublishBarrier([string] $Name, [string] $EvidenceRoot) {
    if ([Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_PUBLISH_BARRIER', 'Process') -cne $Name) { return }
    $directory = Join-Path $EvidenceRoot '.publisher-test-barriers'
    [IO.Directory]::CreateDirectory($directory) | Out-Null
    $entered = Join-Path $directory "$Name.entered"
    $release = Join-Path $directory "$Name.release"
    [IO.File]::WriteAllText($entered, 'entered', $script:Utf8)
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (-not (Test-Path -LiteralPath $release -PathType Leaf)) {
        if ([DateTime]::UtcNow -ge $deadline) { throw "Publisher barrier timed out: $Name" }
        Start-Sleep -Milliseconds 25
    }
}

function New-PublicationLease {
    param(
        [string] $AttemptId,
        [int64] $Generation,
        [string] $PriorLeaseSha256,
        [string] $ExpectedTailSha256,
        [object] $Context
    )
    $lease = [ordered]@{
        schema_version = 1
        execution_baseline = $Context.ExecutionBaseline
        plan_set_sha256 = $Context.PlanSetSha256
        attempt_id = $AttemptId
        generation = $Generation
        evidence_root_native_path = $Context.EvidenceRoot.Path
        evidence_root_identity_sha256 = $Context.EvidenceRoot.IdentitySha256
        owner = New-Owner -AttemptId $AttemptId -CommonIdentitySha256 $Context.CommonDirectory.IdentitySha256
        source_set_sha256 = $Context.SourceSnapshot.Fingerprint
        canonical_core_sha256 = Get-Sha256Bytes $Context.CoreBytes
        expected_tail_sha256 = $ExpectedTailSha256
        prior_lease_sha256 = $PriorLeaseSha256
        created_at = Get-UtcNowCanonical
    }
    $lease['lease_sha256'] = Get-DomainHash 'dynamo-publication-lease-v1' (ConvertTo-CanonicalBytes $lease)
    $lease
}

function Read-PublicationLease([string] $Path, [object] $Context) {
    $keys = @('schema_version','execution_baseline','plan_set_sha256','attempt_id','generation','evidence_root_native_path','evidence_root_identity_sha256','owner','source_set_sha256','canonical_core_sha256','expected_tail_sha256','prior_lease_sha256','created_at','lease_sha256')
    $record = Read-CanonicalJsonFile -Path $Path -ExpectedKeys $keys
    $value = $record.Value
    $preimage = [ordered]@{}
    foreach ($key in $keys[0..($keys.Count - 2)]) { $preimage[$key] = $value[$key] }
    if ((Get-DomainHash 'dynamo-publication-lease-v1' (ConvertTo-CanonicalBytes $preimage)) -cne [string]$value['lease_sha256']) { throw 'Publication lease hash mismatch.' }
    Assert-JsonInt64 $value['schema_version'] 'Publication lease schema_version'
    Assert-JsonInt64 $value['generation'] 'Publication lease generation'
    if ([int64]$value['schema_version'] -ne 1) { throw 'Publication lease schema version mismatch.' }
    if ([string]$value['execution_baseline'] -cne $Context.ExecutionBaseline -or [string]$value['plan_set_sha256'] -cne $Context.PlanSetSha256) { throw 'Publication lease baseline/plan-set mismatch.' }
    if ([string]$value['evidence_root_native_path'] -cne $Context.EvidenceRoot.Path -or [string]$value['evidence_root_identity_sha256'] -cne $Context.EvidenceRoot.IdentitySha256) { throw 'Publication lease evidence-root mismatch.' }
    if ([string]$value['attempt_id'] -cnotmatch '^[0-9a-f]{32}$' -or [int64]$value['generation'] -lt 1) { throw 'Publication lease attempt/generation is malformed.' }
    foreach ($field in @('source_set_sha256','canonical_core_sha256','expected_tail_sha256','prior_lease_sha256','lease_sha256')) { Assert-LowerHex ([string]$value[$field]) 64 "publication lease $field" }
    if ([string]$value['source_set_sha256'] -cne $Context.SourceSnapshot.Fingerprint -or [string]$value['canonical_core_sha256'] -cne (Get-Sha256Bytes $Context.CoreBytes)) { throw 'Publication lease source/core snapshot mismatch.' }
    if ([string]$value['created_at'] -cnotmatch '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{7}Z$') { throw 'Publication lease timestamp is noncanonical.' }
    $owner = $value['owner']
    $ownerKeys = @('machine_identity_sha256','pid','process_start_identity','attempt_id','git_common_dir_identity_sha256')
    if ($owner -isnot [Collections.IDictionary] -or [string]::Join("`n", @($owner.Keys)) -cne [string]::Join("`n", $ownerKeys)) { throw 'Publication lease owner schema mismatch.' }
    Assert-JsonInt64 $owner['pid'] 'Publication lease owner pid'
    foreach ($field in @('machine_identity_sha256','process_start_identity','git_common_dir_identity_sha256')) { Assert-LowerHex ([string]$owner[$field]) 64 "publication owner $field" }
    if ([int64]$owner['pid'] -lt 1 -or [int64]$owner['pid'] -gt [int]::MaxValue -or [string]$owner['attempt_id'] -cne [string]$value['attempt_id'] -or
        [string]$owner['git_common_dir_identity_sha256'] -cne $Context.CommonDirectory.IdentitySha256) { throw 'Publication lease owner identity mismatch.' }
    [pscustomobject]@{ Value = $value; Bytes = $record.Bytes; Sha256 = $record.Sha256; Path = $Path }
}

function Read-Binding([object] $Context, [object] $Manifest, [object] $Prepared, [string] $Path = $Context.BindingPath) {
    $probe = Read-CanonicalJsonFile -Path $Path
    Assert-JsonInt64 $probe.Value['schema_version'] 'Binding schema_version'
    [int64]$schemaVersion = $probe.Value['schema_version']
    $record = Read-CanonicalJsonFile -Path $Path -ExpectedKeys (Get-PlanSetBindingKeys $schemaVersion)
    $keys = Get-PlanSetBindingKeys $schemaVersion
    $preimage = [ordered]@{}
    foreach ($key in $keys[0..($keys.Count - 2)]) { $preimage[$key] = $record.Value[$key] }
    $hash = Get-DomainHash 'dynamo-plan-set-binding-v1' (ConvertTo-CanonicalBytes $preimage)
    if ($hash -cne [string]$record.Value['binding_sha256']) { throw 'Binding hash mismatch.' }
    Assert-JsonInt64 $record.Value['manifest_bytes'] 'Binding manifest_bytes'
    if ($schemaVersion -notin @(1,2)) { throw 'Binding schema version mismatch.' }
    $expectedControls = @{}
    foreach ($row in $Context.SourceSnapshot.Controls) { $expectedControls[$row.path] = $row.sha256 }
    $expected = [ordered]@{
        audit_baseline = $script:AuditBaseline
        execution_baseline = $Context.ExecutionBaseline
        plan_set_sha256 = $Context.PlanSetSha256
        manifest_native_path = $Manifest.Path
        manifest_sha256 = $Manifest.Sha256
        manifest_bytes = [int64]$Manifest.Bytes.LongLength
        git_common_dir_native_path = $Context.CommonDirectory.Path
        git_common_dir_identity_sha256 = $Context.CommonDirectory.IdentitySha256
        git_common_dir_owner = $Context.CommonDirectory.Owner
        git_common_dir_acl_sha256 = $Context.CommonDirectory.AclSha256
        bundle_prepared_row_sha256 = [string]$Prepared.Value['row_sha256']
    }
    if ($schemaVersion -eq 1) {
        $expected['publisher_sha256'] = $expectedControls['scripts/remediation/publish-plan-set.ps1']
        $expected['integration_helper_sha256'] = $expectedControls['scripts/remediation/update-integration-ref.ps1']
        $expected['publisher_contract_test_sha256'] = $expectedControls['tests/scripts/plan-set-publisher-contract.ps1']
        $expected['integration_contract_test_sha256'] = $expectedControls['tests/scripts/integration-ref-journal-contract.ps1']
    } else {
        if ($null -eq $Context.ControlSchema) { throw 'Binding v2 requires the loaded control schema.' }
        $expected['control_schema_path'] = $Context.ControlSchema.Path
        $expected['control_schema_sha256'] = $Context.ControlSchema.Sha256
        $expected['control_schema_version'] = [int64]$Context.ControlSchema.Version
        $expected['control_hashes'] = @($Context.SourceSnapshot.Controls | ForEach-Object { [ordered]@{ path = $_.path; sha256 = $_.sha256 } })
    }
    foreach ($key in $expected.Keys) {
        if ($key -ceq 'control_hashes') { continue }
        if ([string]$record.Value[$key] -cne [string]$expected[$key]) { throw "Binding mismatch at $key" }
    }
    if ($schemaVersion -eq 2 -and -not (Test-BytesEqual (ConvertTo-CanonicalBytes @($record.Value['control_hashes'])) (ConvertTo-CanonicalBytes @($expected['control_hashes'])))) {
        throw 'Binding mismatch at control_hashes.'
    }
    [pscustomobject]@{ Value = $record.Value; Bytes = $record.Bytes; Sha256 = $hash; Path = $Path; PathRecord = $record.PathRecord; SchemaVersion = $schemaVersion }
}

function Write-Binding([object] $Context, [object] $Manifest, [object] $Prepared) {
    $controls = @{}
    foreach ($row in $Context.SourceSnapshot.Controls) { $controls[$row.path] = $row.sha256 }
    $binding = [ordered]@{
        schema_version = 2
        audit_baseline = $script:AuditBaseline
        execution_baseline = $Context.ExecutionBaseline
        plan_set_sha256 = $Context.PlanSetSha256
        manifest_native_path = $Manifest.Path
        manifest_sha256 = $Manifest.Sha256
        manifest_bytes = [int64]$Manifest.Bytes.LongLength
        git_common_dir_native_path = $Context.CommonDirectory.Path
        git_common_dir_identity_sha256 = $Context.CommonDirectory.IdentitySha256
        git_common_dir_owner = $Context.CommonDirectory.Owner
        git_common_dir_acl_sha256 = $Context.CommonDirectory.AclSha256
        control_schema_path = $Context.ControlSchema.Path
        control_schema_sha256 = $Context.ControlSchema.Sha256
        control_schema_version = [int64]$Context.ControlSchema.Version
        control_hashes = @($Context.SourceSnapshot.Controls | ForEach-Object { [ordered]@{ path = $_.path; sha256 = $_.sha256 } })
        bundle_prepared_row_sha256 = [string]$Prepared.Value['row_sha256']
    }
    $binding['binding_sha256'] = Get-DomainHash 'dynamo-plan-set-binding-v1' (ConvertTo-CanonicalBytes $binding)
    $nonce = [Guid]::NewGuid().ToString('N').ToLowerInvariant()
    $temp = Join-Path $Context.BindingTempDirectory "$nonce.binding.tmp"
    Write-CreateNewDurable $temp (ConvertTo-CanonicalBytes $binding)
    Invoke-PublishFailpoint 'after-binding-temp-write'
    Move-NoReplaceDurable $temp $Context.BindingPath
    Read-Binding -Context $Context -Manifest $Manifest -Prepared $Prepared
}

function Assert-StalePublicationRow {
    param(
        [object] $Record,
        [object] $Context,
        [object] $Manifest,
        [object] $Rows,
        [object[]] $ArchivedLeases
    )
    $value = $Record.Value
    $expectedAttempt = if ($null -ne $Context.ActiveLease) { [string]$Context.ActiveLease.Value['attempt_id'] } elseif ($null -ne $Rows.Prepared) { [string]$Rows.Prepared.Value['attempt_id'] } else { $null }
    if (-not $expectedAttempt -or [string]$value['attempt_id'] -cne $expectedAttempt -or
        [string]$value['manifest_sha256'] -cne $Manifest.Sha256 -or [int64]$value['manifest_bytes'] -ne [int64]$Manifest.Bytes.LongLength) {
        throw 'Stale publication row does not bind the active recovery attempt/manifest.'
    }
    $leaseMatches = @($ArchivedLeases | Where-Object {
        [string]$_.Value['attempt_id'] -ceq [string]$value['attempt_id'] -and
        [int64]$_.Value['generation'] -eq [int64]$value['generation'] -and
        [string]$_.Value['lease_sha256'] -ceq [string]$value['lease_sha256']
    })
    if ($leaseMatches.Count -ne 1) { throw 'Stale publication row is not bound to exactly one archived dead-owner lease.' }
    if ([string]$value['phase'] -ceq 'BundlePrepared') {
        if ([int64]$value['seq'] -ne 1 -or [string]$value['prev_row_sha256'] -cne $script:ZeroSha256 -or
            [string]$value['bundle_prepared_row_sha256'] -cne $script:ZeroSha256 -or [string]$value['binding_sha256'] -cne $script:ZeroSha256) {
            throw 'Stale BundlePrepared temp invariants failed.'
        }
        return
    }
    if ([string]$value['phase'] -ceq 'BindingCommitted') {
        if ($null -eq $Rows.Prepared -or -not (Test-Path -LiteralPath $Context.BindingPath -PathType Leaf)) { throw 'Stale BindingCommitted temp lacks its fixed prepared row/binding.' }
        $binding = Read-Binding -Context $Context -Manifest $Manifest -Prepared $Rows.Prepared
        if ([int64]$value['seq'] -ne 2 -or [string]$value['prev_row_sha256'] -cne [string]$Rows.Prepared.Value['row_sha256'] -or
            [string]$value['bundle_prepared_row_sha256'] -cne [string]$Rows.Prepared.Value['row_sha256'] -or
            [string]$value['binding_sha256'] -cne $binding.Sha256) { throw 'Stale BindingCommitted temp invariants failed.' }
        return
    }
    throw 'Stale publication row phase is not recoverable.'
}

function Move-VerifiedOrphan([string] $Source, [string] $Destination) {
    $before = [IO.File]::ReadAllBytes($Source)
    Move-NoReplaceDurable $Source $Destination
    $after = [IO.File]::ReadAllBytes($Destination)
    if (-not (Test-BytesEqual $before $after)) { throw 'Publication orphan bytes changed during archive rename.' }
}

function Recover-PublicationTemps([object] $Context, [object] $Manifest, [object] $Rows) {
    $attempt = [string]$Context.ActiveLease.Value['attempt_id']
    $archives = @(Get-ArchivedPublicationLeases -Context $Context -AttemptId $attempt)
    Assert-ArchivedLeaseSemantics -Archives $archives -Prepared $Rows.Prepared -Context $Context

    foreach ($item in @(Get-ChildItem -LiteralPath $Context.RowTempDirectory -Force | Sort-Object Name)) {
        if ($item.PSIsContainer) { throw "Unexpected publication row temp directory: $($item.Name)" }
        $match = [regex]::Match($item.Name, '^(?<attempt>[0-9a-f]{32})\.g(?<generation>[0-9]{10})\.(?<sequence>[0-9]{20})\.(?<slug>bundle-prepared|binding-committed)\.(?<nonce>[0-9a-f]{32})\.tmp$')
        if (-not $match.Success) { throw "Malformed publication row temp: $($item.Name)" }
        $record = Read-PublicationRow -Path $item.FullName -Context $Context
        $expectedSlug = if ([string]$record.Value['phase'] -ceq 'BundlePrepared') { 'bundle-prepared' } elseif ([string]$record.Value['phase'] -ceq 'BindingCommitted') { 'binding-committed' } else { '' }
        if ($match.Groups['attempt'].Value -cne [string]$record.Value['attempt_id'] -or
            [int64]::Parse($match.Groups['generation'].Value, [Globalization.CultureInfo]::InvariantCulture) -ne [int64]$record.Value['generation'] -or
            [int64]::Parse($match.Groups['sequence'].Value, [Globalization.CultureInfo]::InvariantCulture) -ne [int64]$record.Value['seq'] -or
            $match.Groups['slug'].Value -cne $expectedSlug) { throw 'Publication row temp filename/content mismatch.' }
        Assert-StalePublicationRow -Record $record -Context $Context -Manifest $Manifest -Rows $Rows -ArchivedLeases $archives
        $rawHash = Get-Sha256Bytes $record.Bytes
        $orphan = Join-Path $Context.RowOrphanDirectory "$($item.BaseName).$rawHash.orphan"
        Move-VerifiedOrphan -Source $item.FullName -Destination $orphan
    }

    foreach ($item in @(Get-ChildItem -LiteralPath $Context.BindingTempDirectory -Force | Sort-Object Name)) {
        if ($item.PSIsContainer -or $item.Name -cnotmatch '^[0-9a-f]{32}\.binding\.tmp$') { throw "Malformed binding temp: $($item.Name)" }
        if ($null -eq $Rows.Prepared) { throw 'Binding temp exists without BundlePrepared.' }
        $record = Read-Binding -Context $Context -Manifest $Manifest -Prepared $Rows.Prepared -Path $item.FullName
        $rawHash = Get-Sha256Bytes $record.Bytes
        $orphan = Join-Path $Context.BindingOrphanDirectory "$($item.BaseName).$rawHash.orphan"
        Move-VerifiedOrphan -Source $item.FullName -Destination $orphan
    }
}

function Assert-PublicationOrphans([object] $Context, [object] $Manifest, [object] $Rows, [object] $Binding) {
    $attempt = [string]$Rows.Committed.Value['attempt_id']
    $archives = @(Get-ArchivedPublicationLeases -Context $Context -AttemptId $attempt)
    foreach ($item in @(Get-ChildItem -LiteralPath $Context.RowOrphanDirectory -Force | Sort-Object Name)) {
        if ($item.PSIsContainer) { throw "Unexpected publication row orphan directory: $($item.Name)" }
        $match = [regex]::Match($item.Name, '^(?<prefix>[0-9a-f]{32}\.g[0-9]{10}\.[0-9]{20}\.(?:bundle-prepared|binding-committed)\.[0-9a-f]{32})\.(?<hash>[0-9a-f]{64})\.orphan$')
        if (-not $match.Success) { throw "Malformed publication row orphan: $($item.Name)" }
        $record = Read-PublicationRow -Path $item.FullName -Context $Context
        if ((Get-Sha256Bytes $record.Bytes) -cne $match.Groups['hash'].Value) { throw 'Publication row orphan raw hash mismatch.' }
        Assert-StalePublicationRow -Record $record -Context $Context -Manifest $Manifest -Rows $Rows -ArchivedLeases $archives
    }
    foreach ($item in @(Get-ChildItem -LiteralPath $Context.BindingOrphanDirectory -Force | Sort-Object Name)) {
        if ($item.PSIsContainer) { throw "Unexpected binding orphan directory: $($item.Name)" }
        $match = [regex]::Match($item.Name, '^[0-9a-f]{32}\.binding\.(?<hash>[0-9a-f]{64})\.orphan$')
        if (-not $match.Success) { throw "Malformed binding orphan: $($item.Name)" }
        $record = Read-Binding -Context $Context -Manifest $Manifest -Prepared $Rows.Prepared -Path $item.FullName
        if ((Get-Sha256Bytes $record.Bytes) -cne $match.Groups['hash'].Value -or -not (Test-BytesEqual $record.Bytes $Binding.Bytes)) { throw 'Binding orphan does not equal the fixed binding.' }
    }
}

function Assert-StagedPayloadTree([object] $Context, [string] $StagingPath) {
    $stagingFull = [IO.Path]::GetFullPath($StagingPath)
    $evidencePrefix = $Context.EvidenceRoot.Path.TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar
    if (-not $stagingFull.StartsWith($evidencePrefix, [StringComparison]::Ordinal)) { throw 'Staging path escaped the bound evidence root.' }
    $current = $Context.EvidenceRoot.Path
    foreach ($component in [IO.Path]::GetRelativePath($Context.EvidenceRoot.Path, $stagingFull).Split(@([IO.Path]::DirectorySeparatorChar,[IO.Path]::AltDirectorySeparatorChar), [StringSplitOptions]::RemoveEmptyEntries)) {
        $current = Join-Path $current $component
        Assert-SafeExistingPath -Path $current -LeafType Directory | Out-Null
        Assert-RestrictedAcl -Path $current -Kind Directory
    }
    $expectedDirectories = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $expectedFiles = [Collections.Generic.Dictionary[string,object]]::new([StringComparer]::Ordinal)
    $null = $expectedDirectories.Add('.')
    foreach ($row in $Context.SourceSnapshot.Payloads) {
        $expectedFiles.Add([string]$row.path, $row)
        $parts = ([string]$row.path).Split('/')
        $prefix = [Collections.Generic.List[string]]::new()
        for ($index = 0; $index -lt ($parts.Count - 1); $index++) {
            $prefix.Add($parts[$index])
            $null = $expectedDirectories.Add([string]::Join('/', $prefix))
        }
    }
    $seenFiles = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $pending = [Collections.Generic.Queue[string]]::new()
    $pending.Enqueue($stagingFull)
    while ($pending.Count -gt 0) {
        $directory = $pending.Dequeue()
        $relativeDirectory = if ($directory -ceq $stagingFull) { '.' } else { [IO.Path]::GetRelativePath($stagingFull, $directory).Replace('\','/') }
        if (-not $expectedDirectories.Contains($relativeDirectory)) { throw "Unexpected directory in staged payload tree: $relativeDirectory" }
        Assert-SafeExistingPath -Path $directory -LeafType Directory | Out-Null
        Assert-RestrictedAcl -Path $directory -Kind Directory
        foreach ($item in @(Get-ChildItem -LiteralPath $directory -Force | Sort-Object Name -CaseSensitive)) {
            if ($item.PSIsContainer) { $pending.Enqueue($item.FullName); continue }
            $relativeFile = [IO.Path]::GetRelativePath($stagingFull, $item.FullName).Replace('\','/')
            if (-not $expectedFiles.ContainsKey($relativeFile) -or -not $seenFiles.Add($relativeFile)) { throw "Unexpected file in staged payload tree: $relativeFile" }
            Assert-SafeExistingPath -Path $item.FullName -LeafType File | Out-Null
            Assert-RestrictedAcl -Path $item.FullName -Kind File
            $bytes = [IO.File]::ReadAllBytes($item.FullName)
            $expected = $expectedFiles[$relativeFile]
            if ($bytes.LongLength -ne [int64]$expected.bytes -or (Get-Sha256Bytes $bytes) -cne [string]$expected.sha256) { throw "Staged payload mismatch: $relativeFile" }
        }
    }
    if ($seenFiles.Count -ne $expectedFiles.Count) { throw 'Staged payload file inventory is incomplete.' }
}

function Assert-ManifestAndBundle([object] $Context, [string] $BundlePath = $Context.FinalBundle) {
    $bundleFull = [IO.Path]::GetFullPath($BundlePath)
    $evidencePrefix = $Context.EvidenceRoot.Path.TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar
    if (-not $bundleFull.StartsWith($evidencePrefix, [StringComparison]::Ordinal)) { throw 'Bundle path escaped the bound evidence root.' }
    $current = $Context.EvidenceRoot.Path
    foreach ($component in [IO.Path]::GetRelativePath($Context.EvidenceRoot.Path, $bundleFull).Split(@([IO.Path]::DirectorySeparatorChar,[IO.Path]::AltDirectorySeparatorChar), [StringSplitOptions]::RemoveEmptyEntries)) {
        $current = Join-Path $current $component
        Assert-SafeExistingPath -Path $current -LeafType Directory | Out-Null
        Assert-RestrictedAcl -Path $current -Kind Directory
    }
    $manifestPath = Join-Path $BundlePath 'plan-set-manifest-v1.json'
    $keys = @('schema_version','plan_set_sha256','audit_baseline','execution_baseline','git_common_dir_identity_sha256','payloads','controls','gitignore_evidence','published_at')
    $manifest = Read-CanonicalJsonFile -Path $manifestPath -ExpectedKeys $keys
    $value = $manifest.Value
    Assert-JsonInt64 $value['schema_version'] 'Manifest schema_version'
    foreach ($row in @($value['payloads']) + @($value['controls'])) {
        if ($row -isnot [Collections.IDictionary]) { throw 'Manifest source row must be an object.' }
        Assert-JsonInt64 $row['bytes'] 'Manifest source-row bytes'
    }
    foreach ($row in @($value['gitignore_evidence'])) {
        if ($row -isnot [Collections.IDictionary]) { throw 'Manifest gitignore row must be an object.' }
        Assert-JsonInt64 $row['rule_line'] 'Manifest gitignore rule_line'
    }
    if ([int64]$value['schema_version'] -ne 1 -or [string]$value['plan_set_sha256'] -cne $Context.PlanSetSha256 -or
        [string]$value['audit_baseline'] -cne $script:AuditBaseline -or [string]$value['execution_baseline'] -cne $Context.ExecutionBaseline -or
        [string]$value['git_common_dir_identity_sha256'] -cne $Context.CommonDirectory.IdentitySha256) { throw 'Manifest fixed identity mismatch.' }
    if ([string]$value['published_at'] -cnotmatch '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{7}Z$') { throw 'Manifest timestamp is noncanonical.' }

    $manifestCore = [ordered]@{
        schema_version = [int64]$value['schema_version']
        audit_baseline = [string]$value['audit_baseline']
        execution_baseline = [string]$value['execution_baseline']
        git_common_dir_identity_sha256 = [string]$value['git_common_dir_identity_sha256']
        payloads = @($value['payloads'])
        controls = @($value['controls'])
    }
    if ((Get-DomainHash 'dynamo-plan-set-v1' (ConvertTo-CanonicalBytes $manifestCore)) -cne $Context.PlanSetSha256) { throw 'Manifest deterministic plan-set hash mismatch.' }
    $expectedPayloadBytes = ConvertTo-CanonicalBytes @($Context.SourceSnapshot.Payloads)
    $actualPayloadBytes = ConvertTo-CanonicalBytes @($value['payloads'])
    $expectedControlBytes = ConvertTo-CanonicalBytes @($Context.SourceSnapshot.Controls)
    $actualControlBytes = ConvertTo-CanonicalBytes @($value['controls'])
    if (-not (Test-BytesEqual $expectedPayloadBytes $actualPayloadBytes) -or
        -not (Test-BytesEqual $expectedControlBytes $actualControlBytes)) { throw 'Manifest source/control rows differ from the reviewed snapshot.' }
    $expectedIgnore = ConvertTo-CanonicalBytes @($Context.GitIgnoreEvidence)
    $actualIgnore = ConvertTo-CanonicalBytes @($value['gitignore_evidence'])
    if (-not (Test-BytesEqual $expectedIgnore $actualIgnore)) { throw 'Manifest gitignore evidence mismatch.' }

    $expectedDirectories = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $null = $expectedDirectories.Add('.')
    $expectedFiles = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $null = $expectedFiles.Add('plan-set-manifest-v1.json')
    foreach ($row in $Context.SourceSnapshot.Payloads) {
        $null = $expectedFiles.Add([string]$row.path)
        $parts = ([string]$row.path).Split('/')
        $prefix = [Collections.Generic.List[string]]::new()
        for ($index = 0; $index -lt ($parts.Count - 1); $index++) {
            $prefix.Add($parts[$index])
            $null = $expectedDirectories.Add([string]::Join('/', $prefix))
        }
    }
    $actualFiles = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $pending = [Collections.Generic.Queue[string]]::new()
    $pending.Enqueue($bundleFull)
    while ($pending.Count -gt 0) {
        $directory = $pending.Dequeue()
        $relativeDirectory = if ($directory -ceq $bundleFull) { '.' } else { [IO.Path]::GetRelativePath($bundleFull, $directory).Replace('\','/') }
        if (-not $expectedDirectories.Contains($relativeDirectory)) { throw "Unexpected directory in published bundle: $relativeDirectory" }
        Assert-SafeExistingPath -Path $directory -LeafType Directory | Out-Null
        Assert-RestrictedAcl -Path $directory -Kind Directory
        foreach ($item in @(Get-ChildItem -LiteralPath $directory -Force | Sort-Object Name -CaseSensitive)) {
            if ($item.PSIsContainer) { $pending.Enqueue($item.FullName); continue }
            Assert-SafeExistingPath -Path $item.FullName -LeafType File | Out-Null
            Assert-RestrictedAcl -Path $item.FullName -Kind File
            $relativeFile = [IO.Path]::GetRelativePath($bundleFull, $item.FullName).Replace('\','/')
            if (-not $actualFiles.Add($relativeFile) -or -not $expectedFiles.Contains($relativeFile)) { throw "Unexpected file in published bundle: $relativeFile" }
        }
    }
    if ($actualFiles.Count -ne $expectedFiles.Count -or @($expectedFiles | Where-Object { -not $actualFiles.Contains($_) }).Count -ne 0) {
        throw 'Published bundle file inventory is not exact.'
    }
    foreach ($row in $Context.SourceSnapshot.Payloads) {
        $leaf = Join-Path $BundlePath $row.path
        Assert-SafeExistingPath -Path $leaf -LeafType File | Out-Null
        Assert-RestrictedAcl -Path $leaf -Kind File
        $bytes = [IO.File]::ReadAllBytes($leaf)
        if ($bytes.LongLength -ne [int64]$row.bytes -or (Get-Sha256Bytes $bytes) -cne [string]$row.sha256) { throw "Published payload mismatch: $($row.path)" }
    }
    [pscustomobject]@{ Path = $manifestPath; Value = $value; Bytes = $manifest.Bytes; Sha256 = $manifest.Sha256 }
}

function Publish-OrReadBundle([object] $Context) {
    if (Test-Path -LiteralPath $Context.FinalBundle) {
        if (-not [bool]$Context.ActiveLease.RecoveredExistingState) { throw 'A pre-existing final bundle has no recoverable publication provenance.' }
        return Assert-ManifestAndBundle $Context
    }
    $externalPlanRoot = Ensure-SafeDirectoryChain $Context.EvidenceRoot.Path @('Dynamo','plan-set',$Context.ExecutionBaseline)
    $stagingName = ".staging-$($Context.ActiveLease.Value['attempt_id'])-g$(([int64]$Context.ActiveLease.Value['generation']).ToString('D10'))-$([Guid]::NewGuid().ToString('N').ToLowerInvariant())"
    $staging = Join-Path $externalPlanRoot $stagingName
    [IO.Directory]::CreateDirectory($staging) | Out-Null
    Set-RestrictiveDirectory $staging
    Assert-SafeExistingPath -Path $staging -LeafType Directory | Out-Null
    foreach ($row in $Context.SourceSnapshot.Payloads) {
        $source = Join-Path $script:RepositoryRoot $row.path
        $parts = ([string]$row.path).Split('/')
        $parent = if ($parts.Count -gt 1) { Ensure-SafeDirectoryChain -TrustedRoot $staging -Children $parts[0..($parts.Count - 2)] } else { $staging }
        $destination = Join-Path $parent $parts[-1]
        $sourceBytes = [IO.File]::ReadAllBytes($source)
        if ($sourceBytes.LongLength -ne [int64]$row.bytes -or (Get-Sha256Bytes $sourceBytes) -cne [string]$row.sha256) { throw "Payload changed before staging write: $($row.path)" }
        Write-CreateNewDurable $destination $sourceBytes
    }
    Assert-StagedPayloadTree -Context $Context -StagingPath $staging
    $payloadBarrierStateSnapshot = Get-PublicationAdmissionSnapshot $Context
    Invoke-PublishBarrier -Name 'after-payload-copy' -EvidenceRoot $Context.EvidenceRoot.Path
    Assert-PublicationMutationBoundaryUnchanged -Context $Context -ExpectedExecutionAnchor $Context.SourceSnapshot.ExecutionAnchor -ExpectedSourceSnapshot $Context.SourceSnapshot -ExpectedGitIgnoreEvidence $Context.GitIgnoreEvidence -ExpectedStateSnapshot $payloadBarrierStateSnapshot
    Assert-StagedPayloadTree -Context $Context -StagingPath $staging
    $manifestValue = [ordered]@{
        schema_version = 1
        plan_set_sha256 = $Context.PlanSetSha256
        audit_baseline = $script:AuditBaseline
        execution_baseline = $Context.ExecutionBaseline
        git_common_dir_identity_sha256 = $Context.CommonDirectory.IdentitySha256
        payloads = @($Context.SourceSnapshot.Payloads)
        controls = @($Context.SourceSnapshot.Controls)
        gitignore_evidence = @($Context.GitIgnoreEvidence)
        published_at = Get-UtcNowCanonical
    }
    Write-CreateNewDurable (Join-Path $staging 'plan-set-manifest-v1.json') (ConvertTo-CanonicalBytes $manifestValue)
    Assert-ManifestAndBundle -Context $Context -BundlePath $staging | Out-Null
    Move-NoReplaceDurable -Source $staging -Destination $Context.FinalBundle -Directory
    Assert-ManifestAndBundle $Context
}

function Get-PublicationArchiveInventory([object] $Context) {
    $entries = [Collections.Generic.List[object]]::new()
    $attempts = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($item in @(Get-ChildItem -LiteralPath $Context.LeaseArchiveDirectory -Force | Sort-Object Name)) {
        if ($item.PSIsContainer) { throw "Unexpected publication lease archive directory: $($item.Name)" }
        $match = [regex]::Match($item.Name, '^publication-lease\.(?<attempt>[0-9a-f]{32})\.g(?<generation>[0-9]{10})\.(?<hash>[0-9a-f]{64})\.lock$')
        if (-not $match.Success) { throw "Unexpected publication lease archive: $($item.Name)" }
        $attempt = $match.Groups['attempt'].Value
        $null = $attempts.Add($attempt)
        $entries.Add([pscustomobject]@{
            File = $item
            AttemptId = $attempt
            Generation = [int64]::Parse($match.Groups['generation'].Value, [Globalization.CultureInfo]::InvariantCulture)
            LeaseSha256 = $match.Groups['hash'].Value
        })
    }
    [pscustomobject]@{ Entries = $entries.ToArray(); Attempts = @($attempts | Sort-Object -CaseSensitive) }
}

function Get-ArchivedPublicationLeases([object] $Context, [string] $AttemptId) {
    $inventory = Get-PublicationArchiveInventory $Context
    if (@($inventory.Attempts | Where-Object { $_ -cne $AttemptId }).Count -ne 0) { throw 'Publication archive contains another attempt.' }
    $matches = @($inventory.Entries | Where-Object { $_.AttemptId -ceq $AttemptId } | Sort-Object Generation)
    $records = [Collections.Generic.List[object]]::new()
    [int64]$expectedGeneration = 1
    $prior = $script:ZeroSha256
    foreach ($match in $matches) {
        $record = Read-PublicationLease -Path $match.File.FullName -Context $Context
        if ([int64]$record.Value['generation'] -ne $expectedGeneration -or [string]$record.Value['prior_lease_sha256'] -cne $prior) { throw 'Archived publication lease chain is not contiguous.' }
        $expectedName = "publication-lease.$AttemptId.g$($expectedGeneration.ToString('D10')).$($record.Value['lease_sha256']).lock"
        if ($match.File.Name -cne $expectedName -or [string]$record.Value['attempt_id'] -cne $AttemptId) { throw 'Archived publication lease filename/content mismatch.' }
        $prior = [string]$record.Value['lease_sha256']
        $null = $records.Add($record)
        $expectedGeneration++
    }
    $records.ToArray()
}

function Assert-ArchivedLeaseSemantics([object[]] $Archives, [object] $Prepared, [object] $Context) {
    if ($null -eq $Archives -or $Archives.Count -eq 0) { return }
    foreach ($archive in $Archives) {
        if ($null -eq $archive -or $null -eq $archive.Value) { throw 'Archived publication lease record is missing.' }
        $expectedTail = if ($null -ne $Prepared -and [int64]$archive.Value['generation'] -gt [int64]$Prepared.Value['generation']) {
            [string]$Prepared.Value['row_sha256']
        } else { $script:ZeroSha256 }
        if ([string]$archive.Value['expected_tail_sha256'] -cne $expectedTail) { throw 'Archived publication lease expected-tail mismatch.' }
        Assert-OwnerDead -Owner $archive.Value['owner'] -CommonIdentitySha256 $Context.CommonDirectory.IdentitySha256
    }
}

function Assert-PreparedLeasePresent([object] $Prepared, [object[]] $Leases) {
    if ($null -eq $Prepared) { return }
    $matches = @($Leases | Where-Object {
        [int64]$_.Value['generation'] -eq [int64]$Prepared.Value['generation'] -and
        [string]$_.Value['lease_sha256'] -ceq [string]$Prepared.Value['lease_sha256'] -and
        [string]$_.Value['attempt_id'] -ceq [string]$Prepared.Value['attempt_id']
    })
    if ($matches.Count -ne 1) { throw 'BundlePrepared does not bind exactly one publication lease generation.' }
}

function Acquire-PublicationLease([object] $Context, [object] $Rows) {
    $attempt = if ($null -ne $Rows.Prepared) { [string]$Rows.Prepared.Value['attempt_id'] } else { $null }
    [int64]$generation = 1
    $prior = $script:ZeroSha256
    $recoveredExistingState = $null -ne $Rows.Prepared
    $unexpectedClosed = @(Get-ChildItem -LiteralPath $Context.LeasesDirectory -Force | Where-Object { -not $_.PSIsContainer -or $_.Name -cne 'archives' })
    if ($unexpectedClosed.Count -ne 0) { throw 'Nonterminal publication state contains a closed or unexpected lease entry.' }
    $inventory = Get-PublicationArchiveInventory $Context
    if ($attempt) {
        if (@($inventory.Attempts | Where-Object { $_ -cne $attempt }).Count -ne 0) { throw 'Publication archive attempt differs from prepared row.' }
    } elseif ($inventory.Attempts.Count -gt 1) {
        throw 'Publication archive contains multiple attempts.'
    } elseif ($inventory.Attempts.Count -eq 1) {
        $attempt = [string]$inventory.Attempts[0]
        $recoveredExistingState = $true
    }

    if (Test-Path -LiteralPath $Context.ActiveLeasePath) {
        if (-not (Test-Path -LiteralPath $Context.ActiveLeasePath -PathType Leaf)) { throw 'Active publication lease is not a file.' }
        $active = Read-PublicationLease -Path $Context.ActiveLeasePath -Context $Context
        if ($attempt -and [string]$active.Value['attempt_id'] -cne $attempt) { throw 'Active publication lease attempt differs from prepared row.' }
        $attempt = [string]$active.Value['attempt_id']
        $archives = @(Get-ArchivedPublicationLeases -Context $Context -AttemptId $attempt)
        Assert-ArchivedLeaseSemantics -Archives $archives -Prepared $Rows.Prepared -Context $Context
        $expectedGeneration = [int64]$archives.Count + 1
        $expectedPrior = if ($archives.Count -eq 0) { $script:ZeroSha256 } else { [string]$archives[-1].Value['lease_sha256'] }
        $expectedTail = if ($null -ne $Rows.Prepared -and $expectedGeneration -gt [int64]$Rows.Prepared.Value['generation']) { [string]$Rows.Prepared.Value['row_sha256'] } else { $script:ZeroSha256 }
        if ([int64]$active.Value['generation'] -ne $expectedGeneration -or [string]$active.Value['prior_lease_sha256'] -cne $expectedPrior -or
            [string]$active.Value['expected_tail_sha256'] -cne $expectedTail) { throw 'Active publication lease does not extend the exact archive chain.' }
        Assert-PreparedLeasePresent -Prepared $Rows.Prepared -Leases (@($archives) + @($active))
        Assert-OwnerDead -Owner $active.Value['owner'] -CommonIdentitySha256 $Context.CommonDirectory.IdentitySha256
        $archiveName = "publication-lease.$attempt.g$(([int64]$active.Value['generation']).ToString('D10')).$($active.Value['lease_sha256']).lock"
        $archivePath = Join-Path $Context.LeaseArchiveDirectory $archiveName
        $before = [IO.File]::ReadAllBytes($Context.ActiveLeasePath)
        Move-NoReplaceDurable $Context.ActiveLeasePath $archivePath
        $archived = Read-PublicationLease -Path $archivePath -Context $Context
        if (-not (Test-BytesEqual $before $archived.Bytes)) { throw 'Archived publication lease bytes changed.' }
        $null = Get-ArchivedPublicationLeases -Context $Context -AttemptId $attempt
        $generation = [int64]$active.Value['generation'] + 1
        $prior = [string]$active.Value['lease_sha256']
        $recoveredExistingState = $true
    } elseif ($attempt) {
        $archives = @(Get-ArchivedPublicationLeases -Context $Context -AttemptId $attempt)
        Assert-ArchivedLeaseSemantics -Archives $archives -Prepared $Rows.Prepared -Context $Context
        if ($archives.Count -gt 0) {
            $last = $archives[-1]
            $generation = [int64]$last.Value['generation'] + 1
            $prior = [string]$last.Value['lease_sha256']
            $recoveredExistingState = $true
        } elseif ($null -ne $Rows.Prepared) {
            throw 'BundlePrepared exists without its active or archived publication lease.'
        }
        Assert-PreparedLeasePresent -Prepared $Rows.Prepared -Leases $archives
    }
    if (-not $attempt) { $attempt = [Guid]::NewGuid().ToString('N').ToLowerInvariant() }
    $expectedTail = if ($null -ne $Rows.Prepared) { [string]$Rows.Prepared.Value['row_sha256'] } else { $script:ZeroSha256 }
    $lease = New-PublicationLease -AttemptId $attempt -Generation $generation -PriorLeaseSha256 $prior -ExpectedTailSha256 $expectedTail -Context $Context
    Write-CreateNewDurable $Context.ActiveLeasePath (ConvertTo-CanonicalBytes $lease)
    Invoke-PublishFailpoint 'after-lease-create'
    $record = Read-PublicationLease -Path $Context.ActiveLeasePath -Context $Context
    $record | Add-Member -NotePropertyName RecoveredExistingState -NotePropertyValue ([bool]$recoveredExistingState)
    $record
}

function Close-PublicationLease([object] $Context, [object] $Lease) {
    $attempt = [string]$Lease.Value['attempt_id']
    $generation = ([int64]$Lease.Value['generation']).ToString('D10')
    $destination = Join-Path $Context.LeasesDirectory "closed-publication.$attempt.g$generation.$($Lease.Value['lease_sha256']).lock"
    $before = [IO.File]::ReadAllBytes($Context.ActiveLeasePath)
    Move-NoReplaceDurable $Context.ActiveLeasePath $destination
    $after = [IO.File]::ReadAllBytes($destination)
    if (-not (Test-BytesEqual $before $after)) { throw 'Closed publication lease bytes changed.' }
    Invoke-PublishFailpoint 'after-lease-close'
    $destination
}

function Assert-TerminalPublicationLeaseChain {
    param(
        [object] $Context,
        [object] $Prepared,
        [object] $Committed,
        [object] $TerminalLease
    )
    $attempt = [string]$Committed.Value['attempt_id']
    $archives = @(Get-ArchivedPublicationLeases -Context $Context -AttemptId $attempt)
    Assert-ArchivedLeaseSemantics -Archives $archives -Prepared $Prepared -Context $Context
    $expectedGeneration = [int64]$archives.Count + 1
    $expectedPrior = if ($archives.Count -eq 0) { $script:ZeroSha256 } else { [string]$archives[-1].Value['lease_sha256'] }
    $expectedTail = if ($expectedGeneration -gt [int64]$Prepared.Value['generation']) { [string]$Prepared.Value['row_sha256'] } else { $script:ZeroSha256 }
    $terminal = $TerminalLease.Value
    if ([string]$terminal['attempt_id'] -cne $attempt -or [int64]$terminal['generation'] -ne $expectedGeneration -or
        [string]$terminal['prior_lease_sha256'] -cne $expectedPrior -or [string]$terminal['expected_tail_sha256'] -cne $expectedTail) {
        throw 'Terminal publication lease does not extend the exact archive chain.'
    }
    if ([string]$Committed.Value['lease_sha256'] -cne [string]$terminal['lease_sha256'] -or
        [int64]$Committed.Value['generation'] -ne [int64]$terminal['generation'] -or
        [string]$Committed.Value['attempt_id'] -cne [string]$terminal['attempt_id'] -or
        [string]$Committed.Value['evidence_root_identity_sha256'] -cne [string]$terminal['evidence_root_identity_sha256']) {
        throw 'BindingCommitted does not bind the terminal publication lease exactly.'
    }
    Assert-PreparedLeasePresent -Prepared $Prepared -Leases (@($archives) + @($TerminalLease))
}

function Assert-PublicationComplete([object] $Context, [object] $Manifest, [object] $Rows, [object] $Binding) {
    if ($null -eq $Rows.Prepared -or $null -eq $Rows.Committed) { throw 'Publication is not terminal.' }
    if ([string]$Rows.Committed.Value['binding_sha256'] -cne $Binding.Sha256) { throw 'BindingCommitted does not bind the fixed binding.' }
    foreach ($row in @($Rows.Prepared, $Rows.Committed)) {
        if ([string]$row.Value['manifest_sha256'] -cne $Manifest.Sha256 -or [int64]$row.Value['manifest_bytes'] -ne [int64]$Manifest.Bytes.LongLength -or
            [string]$row.Value['evidence_root_identity_sha256'] -cne $Context.EvidenceRoot.IdentitySha256) {
            throw 'Publication row does not bind the fixed manifest/evidence root.'
        }
    }
    if (Test-Path -LiteralPath $Context.ActiveLeasePath) { throw 'PublicationComplete still has an active lease.' }
    $attempt = [string]$Rows.Committed.Value['attempt_id']
    $generation = ([int64]$Rows.Committed.Value['generation']).ToString('D10')
    $leaseHash = [string]$Rows.Committed.Value['lease_sha256']
    $closedName = "closed-publication.$attempt.g$generation.$leaseHash.lock"
    $leaseItems = @(Get-ChildItem -LiteralPath $Context.LeasesDirectory -Force)
    $unexpectedLeaseItems = @($leaseItems | Where-Object { ($_.PSIsContainer -and $_.Name -cne 'archives') -or (-not $_.PSIsContainer -and $_.Name -cne $closedName) })
    if ($unexpectedLeaseItems.Count -ne 0) { throw 'PublicationComplete lease set contains an unexpected entry.' }
    $closedFiles = @($leaseItems | Where-Object { -not $_.PSIsContainer })
    if ($closedFiles.Count -ne 1 -or $closedFiles[0].Name -cne $closedName) { throw 'PublicationComplete closed lease set is not exact.' }
    $closed = Read-PublicationLease -Path $closedFiles[0].FullName -Context $Context
    if ([string]$closed.Value['lease_sha256'] -cne $leaseHash) { throw 'Closed publication lease hash mismatch.' }
    Assert-TerminalPublicationLeaseChain -Context $Context -Prepared $Rows.Prepared -Committed $Rows.Committed -TerminalLease $closed
    foreach ($directory in @($Context.RowTempDirectory,$Context.BindingTempDirectory)) {
        if (@(Get-ChildItem -LiteralPath $directory -Force).Count -ne 0) { throw "PublicationComplete has unexpected residue: $directory" }
    }
    Assert-PublicationOrphans -Context $Context -Manifest $Manifest -Rows $Rows -Binding $Binding
    $completionStateSnapshot = Get-PublicationAdmissionSnapshot $Context
    Invoke-PublishBarrier -Name 'before-publication-complete' -EvidenceRoot $Context.EvidenceRoot.Path
    Assert-PublicationMutationBoundaryUnchanged -Context $Context -ExpectedExecutionAnchor $Context.SourceSnapshot.ExecutionAnchor -ExpectedSourceSnapshot $Context.SourceSnapshot -ExpectedGitIgnoreEvidence $Context.GitIgnoreEvidence -ExpectedStateSnapshot $completionStateSnapshot

    # Never emit a handoff from cached pre-barrier records. Reread and rebind the
    # complete terminal graph after the final mutation boundary.
    $freshManifest = Assert-ManifestAndBundle $Context
    $freshRows = Get-PublicationRows $Context
    if ($null -eq $freshRows.Prepared -or $null -eq $freshRows.Committed) { throw 'Fresh PublicationComplete reread is not terminal.' }
    $freshBinding = Read-Binding -Context $Context -Manifest $freshManifest -Prepared $freshRows.Prepared
    if ([string]$freshRows.Committed.Value['binding_sha256'] -cne $freshBinding.Sha256) { throw 'Fresh BindingCommitted does not bind the fixed binding.' }
    foreach ($row in @($freshRows.Prepared,$freshRows.Committed)) {
        if ([string]$row.Value['manifest_sha256'] -cne $freshManifest.Sha256 -or [int64]$row.Value['manifest_bytes'] -ne [int64]$freshManifest.Bytes.LongLength) {
            throw 'Fresh publication row does not bind the fixed manifest.'
        }
    }
    $freshAttempt = [string]$freshRows.Committed.Value['attempt_id']
    $freshGeneration = ([int64]$freshRows.Committed.Value['generation']).ToString('D10')
    $freshLeaseHash = [string]$freshRows.Committed.Value['lease_sha256']
    $freshClosedName = "closed-publication.$freshAttempt.g$freshGeneration.$freshLeaseHash.lock"
    $freshLeaseItems = @(Get-ChildItem -LiteralPath $Context.LeasesDirectory -Force)
    if (@($freshLeaseItems | Where-Object { ($_.PSIsContainer -and $_.Name -cne 'archives') -or (-not $_.PSIsContainer -and $_.Name -cne $freshClosedName) }).Count -ne 0) {
        throw 'Fresh PublicationComplete lease inventory is not exact.'
    }
    $freshClosedFiles = @($freshLeaseItems | Where-Object { -not $_.PSIsContainer })
    if ($freshClosedFiles.Count -ne 1) { throw 'Fresh PublicationComplete closed lease is missing.' }
    $freshClosed = Read-PublicationLease -Path $freshClosedFiles[0].FullName -Context $Context
    Assert-TerminalPublicationLeaseChain -Context $Context -Prepared $freshRows.Prepared -Committed $freshRows.Committed -TerminalLease $freshClosed
    foreach ($directory in @($Context.RowTempDirectory,$Context.BindingTempDirectory)) {
        if (@(Get-ChildItem -LiteralPath $directory -Force).Count -ne 0) { throw "Fresh PublicationComplete has unexpected residue: $directory" }
    }
    Assert-PublicationOrphans -Context $Context -Manifest $freshManifest -Rows $freshRows -Binding $freshBinding
    [ordered]@{
        execution_baseline = $Context.ExecutionBaseline
        plan_set_sha256 = $Context.PlanSetSha256
        manifest_sha256 = $freshManifest.Sha256
        binding_sha256 = $freshBinding.Sha256
        publication_tail_sha256 = [string]$freshRows.Committed.Value['row_sha256']
        git_common_dir_identity_sha256 = $Context.CommonDirectory.IdentitySha256
    }
}

function Assert-ExactExistingDirectoryMembers {
    param(
        [string] $Path,
        [string[]] $Directories,
        [string[]] $ExactFiles = @(),
        [string[]] $FilePatterns = @()
    )
    Assert-SafeExistingPath -Path $Path -LeafType Directory | Out-Null
    Assert-RestrictedAcl -Path $Path -Kind Directory
    $items = @(Get-ChildItem -LiteralPath $Path -Force | Sort-Object Name)
    $actualDirectories = @($items | Where-Object PSIsContainer | ForEach-Object Name)
    if ([string]::Join("`n", $actualDirectories) -cne [string]::Join("`n", @($Directories | Sort-Object -CaseSensitive))) {
        throw "Existing publication directory inventory is not exact: $Path"
    }
    foreach ($item in @($items | Where-Object { -not $_.PSIsContainer })) {
        $allowed = $item.Name -cin $ExactFiles
        foreach ($pattern in $FilePatterns) { if ($item.Name -cmatch $pattern) { $allowed = $true; break } }
        if (-not $allowed) { throw "Unexpected existing publication artifact: $($item.FullName)" }
        Assert-SafeExistingPath -Path $item.FullName -LeafType File | Out-Null
        Assert-RestrictedAcl -Path $item.FullName -Kind File
    }
}

function Assert-ExistingPublicationRowArtifacts {
    param(
        [object] $Context,
        [object] $Manifest,
        [object] $Rows,
        [object[]] $TempLeaseChain,
        [object[]] $OrphanLeaseChain,
        [string] $AttemptId
    )
    $savedActive = $Context.ActiveLease
    if ($null -eq $Context.ActiveLease -and $AttemptId) {
        $Context.ActiveLease = [pscustomobject]@{ Value = [ordered]@{ attempt_id = $AttemptId } }
    }
    try {
        $rowTemps = @(Get-ChildItem -LiteralPath $Context.RowTempDirectory -File -Force | Sort-Object Name)
        if ($rowTemps.Count -gt 1) { throw 'Existing publication state contains multiple row temps.' }
        foreach ($item in $rowTemps) {
            $match = [regex]::Match($item.Name, '^(?<attempt>[0-9a-f]{32})\.g(?<generation>[0-9]{10})\.(?<sequence>[0-9]{20})\.(?<slug>bundle-prepared|binding-committed)\.(?<nonce>[0-9a-f]{32})\.tmp$')
            if (-not $match.Success) { throw "Malformed publication row temp: $($item.Name)" }
            $record = Read-PublicationRow -Path $item.FullName -Context $Context
            $expectedSlug = if ([string]$record.Value['phase'] -ceq 'BundlePrepared') { 'bundle-prepared' } elseif ([string]$record.Value['phase'] -ceq 'BindingCommitted') { 'binding-committed' } else { '' }
            if ($match.Groups['attempt'].Value -cne [string]$record.Value['attempt_id'] -or
                [int64]::Parse($match.Groups['generation'].Value, [Globalization.CultureInfo]::InvariantCulture) -ne [int64]$record.Value['generation'] -or
                [int64]::Parse($match.Groups['sequence'].Value, [Globalization.CultureInfo]::InvariantCulture) -ne [int64]$record.Value['seq'] -or
                $match.Groups['slug'].Value -cne $expectedSlug) { throw 'Publication row temp filename/content mismatch.' }
            if (($null -ne $Rows.Prepared -and [string]$record.Value['phase'] -ceq 'BundlePrepared') -or
                ($null -eq $Rows.Prepared -and [string]$record.Value['phase'] -ceq 'BindingCommitted')) {
                throw 'Publication row temp duplicates or skips the durable row sequence.'
            }
            Assert-StalePublicationRow -Record $record -Context $Context -Manifest $Manifest -Rows $Rows -ArchivedLeases $TempLeaseChain
            $orphanDestination = Join-Path $Context.RowOrphanDirectory "$($item.BaseName).$(Get-Sha256Bytes $record.Bytes).orphan"
            if (Test-Path -LiteralPath $orphanDestination) { throw 'Publication row temp collides with its deterministic orphan destination.' }
        }
        foreach ($item in @(Get-ChildItem -LiteralPath $Context.RowOrphanDirectory -File -Force | Sort-Object Name)) {
            $match = [regex]::Match($item.Name, '^(?<attempt>[0-9a-f]{32})\.g(?<generation>[0-9]{10})\.(?<sequence>[0-9]{20})\.(?<slug>bundle-prepared|binding-committed)\.(?<nonce>[0-9a-f]{32})\.(?<hash>[0-9a-f]{64})\.orphan$')
            if (-not $match.Success) { throw "Malformed publication row orphan: $($item.Name)" }
            $record = Read-PublicationRow -Path $item.FullName -Context $Context
            $expectedSlug = if ([string]$record.Value['phase'] -ceq 'BundlePrepared') { 'bundle-prepared' } elseif ([string]$record.Value['phase'] -ceq 'BindingCommitted') { 'binding-committed' } else { '' }
            if ($match.Groups['attempt'].Value -cne [string]$record.Value['attempt_id'] -or
                [int64]::Parse($match.Groups['generation'].Value, [Globalization.CultureInfo]::InvariantCulture) -ne [int64]$record.Value['generation'] -or
                [int64]::Parse($match.Groups['sequence'].Value, [Globalization.CultureInfo]::InvariantCulture) -ne [int64]$record.Value['seq'] -or
                $match.Groups['slug'].Value -cne $expectedSlug -or (Get-Sha256Bytes $record.Bytes) -cne $match.Groups['hash'].Value) {
                throw 'Publication row orphan filename/content mismatch.'
            }
            Assert-StalePublicationRow -Record $record -Context $Context -Manifest $Manifest -Rows $Rows -ArchivedLeases $OrphanLeaseChain
        }
    }
    finally { $Context.ActiveLease = $savedActive }
}

function Assert-ExistingProtectedEvidenceAncestors([object] $Context) {
    $current = $Context.EvidenceRoot.Path
    foreach ($component in @('Dynamo','plan-set',$Context.ExecutionBaseline)) {
        $next = Join-Path $current $component
        if (-not (Test-Path -LiteralPath $next)) { return }
        Assert-SafeExistingPath -Path $next -LeafType Directory | Out-Null
        Assert-RestrictedAcl -Path $next -Kind Directory
        $current = $next
    }
}

function Assert-ExistingProtectedTreeSafety([string] $Root) {
    if (-not (Test-Path -LiteralPath $Root)) { return }
    $pending = [Collections.Generic.Queue[string]]::new()
    $pending.Enqueue([IO.Path]::GetFullPath($Root))
    while ($pending.Count -gt 0) {
        $current = $pending.Dequeue()
        $item = Get-Item -LiteralPath $current -Force
        if ($item.PSIsContainer) {
            Assert-SafeExistingPath -Path $current -LeafType Directory | Out-Null
            Assert-RestrictedAcl -Path $current -Kind Directory
            foreach ($child in @(Get-ChildItem -LiteralPath $current -Force | Sort-Object Name -CaseSensitive)) { $pending.Enqueue($child.FullName) }
        }
        else {
            Assert-SafeExistingPath -Path $current -LeafType File | Out-Null
            Assert-RestrictedAcl -Path $current -Kind File
        }
    }
}

function Assert-ExistingPublicationStateReadOnly([object] $Context) {
    Assert-ExistingProtectedTreeSafety $Context.ControlRoot
    Assert-ExistingProtectedTreeSafety (Join-Path $Context.EvidenceRoot.Path 'Dynamo')
    Assert-ExistingProtectedEvidenceAncestors $Context
    $bindingExists = Test-Path -LiteralPath $Context.BindingPath
    $bundleExists = Test-Path -LiteralPath $Context.FinalBundle
    $publicationExists = Test-Path -LiteralPath $Context.PublicationRoot
    if (-not $publicationExists) {
        if ($bindingExists -or $bundleExists) { throw 'Binding/final bundle exists without its publication state.' }
        return
    }

    $publicationRelative = [IO.Path]::GetRelativePath($Context.CommonDirectory.Path, $Context.PublicationRoot)
    if ($publicationRelative.StartsWith('..', [StringComparison]::Ordinal) -or [IO.Path]::IsPathFullyQualified($publicationRelative)) { throw 'Publication state escaped the Git common directory.' }
    $protectedCurrent = $Context.CommonDirectory.Path
    foreach ($component in $publicationRelative.Split(@([IO.Path]::DirectorySeparatorChar,[IO.Path]::AltDirectorySeparatorChar), [StringSplitOptions]::RemoveEmptyEntries)) {
        $protectedCurrent = Join-Path $protectedCurrent $component
        Assert-SafeExistingPath -Path $protectedCurrent -LeafType Directory | Out-Null
        Assert-RestrictedAcl -Path $protectedCurrent -Kind Directory
    }

    # Directory creation is itself crashable. A protected, artifact-free subset
    # of the canonical skeleton is equivalent to empty state and can converge.
    $allowedEmptyDirectories = @(
        '.', 'binding-orphans', 'binding-tmp', 'journal', 'journal/orphans',
        'journal/rows', 'journal/tmp', 'leases', 'leases/archives'
    )
    $pending = [Collections.Generic.Queue[string]]::new()
    $pending.Enqueue($Context.PublicationRoot)
    [bool]$hasArtifact = $false
    while ($pending.Count -gt 0) {
        $directory = $pending.Dequeue()
        $relative = if ($directory -ceq $Context.PublicationRoot) { '.' } else { [IO.Path]::GetRelativePath($Context.PublicationRoot, $directory).Replace('\','/') }
        if ($relative -cnotin $allowedEmptyDirectories) { throw "Unknown directory in existing publication skeleton: $relative" }
        Assert-SafeExistingPath -Path $directory -LeafType Directory | Out-Null
        Assert-RestrictedAcl -Path $directory -Kind Directory
        foreach ($child in @(Get-ChildItem -LiteralPath $directory -Force)) {
            if ($child.PSIsContainer) { $pending.Enqueue($child.FullName) }
            else { $hasArtifact = $true }
        }
    }
    if (-not $hasArtifact) {
        if ($bindingExists -or $bundleExists) { throw 'Binding/final bundle exists with only an empty publication skeleton.' }
        return
    }

    Assert-ExactExistingDirectoryMembers -Path $Context.PublicationRoot -Directories @('binding-orphans','binding-tmp','journal','leases') -ExactFiles @('active-publication.lock')
    Assert-ExactExistingDirectoryMembers -Path $Context.LeasesDirectory -Directories @('archives') -FilePatterns @('^closed-publication\.[0-9a-f]{32}\.g[0-9]{10}\.[0-9a-f]{64}\.lock$')
    Assert-ExactExistingDirectoryMembers -Path $Context.LeaseArchiveDirectory -Directories @() -FilePatterns @('^publication-lease\.[0-9a-f]{32}\.g[0-9]{10}\.[0-9a-f]{64}\.lock$')
    $journalRoot = Split-Path -Parent $Context.RowsDirectory
    Assert-ExactExistingDirectoryMembers -Path $journalRoot -Directories @('orphans','rows','tmp')
    Assert-ExactExistingDirectoryMembers -Path $Context.RowsDirectory -Directories @() -ExactFiles @('00000000000000000001-bundle-prepared.json','00000000000000000002-binding-committed.json')
    Assert-ExactExistingDirectoryMembers -Path $Context.RowTempDirectory -Directories @() -FilePatterns @('^[0-9a-f]{32}\.g[0-9]{10}\.[0-9]{20}\.(?:bundle-prepared|binding-committed)\.[0-9a-f]{32}\.tmp$')
    Assert-ExactExistingDirectoryMembers -Path $Context.RowOrphanDirectory -Directories @() -FilePatterns @('^[0-9a-f]{32}\.g[0-9]{10}\.[0-9]{20}\.(?:bundle-prepared|binding-committed)\.[0-9a-f]{32}\.[0-9a-f]{64}\.orphan$')
    Assert-ExactExistingDirectoryMembers -Path $Context.BindingTempDirectory -Directories @() -FilePatterns @('^[0-9a-f]{32}\.binding\.tmp$')
    Assert-ExactExistingDirectoryMembers -Path $Context.BindingOrphanDirectory -Directories @() -FilePatterns @('^[0-9a-f]{32}\.binding\.[0-9a-f]{64}\.orphan$')

    $rows = Get-PublicationRows $Context
    if ($null -ne $rows.Prepared -and -not $bundleExists) { throw 'BundlePrepared exists without the final bundle.' }
    if ($null -ne $rows.Committed -and $null -eq $rows.Prepared) { throw 'BindingCommitted exists without BundlePrepared.' }
    $manifest = if ($bundleExists) { Assert-ManifestAndBundle $Context } else { $null }
    $binding = $null
    if ($bindingExists) {
        if ($null -eq $rows.Prepared -or $null -eq $manifest) { throw 'Binding exists without BundlePrepared and its final bundle.' }
        $binding = Read-Binding -Context $Context -Manifest $manifest -Prepared $rows.Prepared
    }
    if ($null -ne $rows.Committed -and $null -eq $binding) { throw 'BindingCommitted exists without its fixed binding.' }

    $active = if (Test-Path -LiteralPath $Context.ActiveLeasePath -PathType Leaf) { Read-PublicationLease -Path $Context.ActiveLeasePath -Context $Context } else { $null }
    $inventory = Get-PublicationArchiveInventory $Context
    if ($inventory.Attempts.Count -gt 1) { throw 'Publication archive contains multiple attempts.' }
    $attemptCandidates = [Collections.Generic.List[string]]::new()
    if ($null -ne $rows.Prepared) { $null = $attemptCandidates.Add([string]$rows.Prepared.Value['attempt_id']) }
    if ($null -ne $active) { $null = $attemptCandidates.Add([string]$active.Value['attempt_id']) }
    foreach ($attempt in $inventory.Attempts) { $null = $attemptCandidates.Add([string]$attempt) }
    $attempts = @($attemptCandidates | Sort-Object -Unique -CaseSensitive)
    if ($attempts.Count -gt 1) { throw 'Existing publication artifacts disagree on attempt identity.' }
    $attemptId = if ($attempts.Count -eq 1) { [string]$attempts[0] } else { $null }
    $archives = @()
    if ($attemptId) { $archives = @(Get-ArchivedPublicationLeases -Context $Context -AttemptId $attemptId) }
    Assert-ArchivedLeaseSemantics -Archives $archives -Prepared $rows.Prepared -Context $Context

    if ($null -ne $active) {
        $expectedGeneration = [int64]$archives.Count + 1
        $expectedPrior = if ($archives.Count -eq 0) { $script:ZeroSha256 } else { [string]$archives[-1].Value['lease_sha256'] }
        $expectedTail = if ($null -ne $rows.Prepared -and $expectedGeneration -gt [int64]$rows.Prepared.Value['generation']) { [string]$rows.Prepared.Value['row_sha256'] } else { $script:ZeroSha256 }
        if ([int64]$active.Value['generation'] -ne $expectedGeneration -or [string]$active.Value['prior_lease_sha256'] -cne $expectedPrior -or [string]$active.Value['expected_tail_sha256'] -cne $expectedTail) {
            throw 'Active publication lease does not extend the exact archive chain.'
        }
        Assert-OwnerDead -Owner $active.Value['owner'] -CommonIdentitySha256 $Context.CommonDirectory.IdentitySha256
    }

    $closedItems = @(Get-ChildItem -LiteralPath $Context.LeasesDirectory -File -Force)
    if ($null -eq $rows.Committed) {
        if ($closedItems.Count -ne 0) { throw 'Preterminal publication state contains a closed lease.' }
        if ($null -eq $active -and $archives.Count -eq 0) { throw 'Preterminal publication state has no active/archive lease provenance.' }
        Assert-PreparedLeasePresent -Prepared $rows.Prepared -Leases (@($archives) + @($active | Where-Object { $null -ne $_ }))
    }
    else {
        if ($null -eq $manifest -or $null -eq $binding) { throw 'Terminal publication state lacks manifest/binding provenance.' }
        if (@(Get-ChildItem -LiteralPath $Context.RowTempDirectory -Force).Count -ne 0 -or @(Get-ChildItem -LiteralPath $Context.BindingTempDirectory -Force).Count -ne 0) {
            throw 'Terminal publication state contains an uncommitted temp.'
        }
        if (($null -ne $active) -eq ($closedItems.Count -eq 1)) { throw 'Terminal publication state must have exactly one active-or-closed lease.' }
        if ($closedItems.Count -gt 1) { throw 'Terminal publication state contains extra closed leases.' }
        $terminalLease = if ($null -ne $active) { $active } else { Read-PublicationLease -Path $closedItems[0].FullName -Context $Context }
        Assert-TerminalPublicationLeaseChain -Context $Context -Prepared $rows.Prepared -Committed $rows.Committed -TerminalLease $terminalLease
        if ([string]$rows.Committed.Value['binding_sha256'] -cne $binding.Sha256) { throw 'BindingCommitted does not bind the fixed binding.' }
    }

    $rowArtifactCount = @(Get-ChildItem -LiteralPath $Context.RowTempDirectory -File -Force).Count + @(Get-ChildItem -LiteralPath $Context.RowOrphanDirectory -File -Force).Count
    $bindingTemps = @(Get-ChildItem -LiteralPath $Context.BindingTempDirectory -File -Force)
    $bindingOrphans = @(Get-ChildItem -LiteralPath $Context.BindingOrphanDirectory -File -Force)
    if (($rowArtifactCount -gt 0 -or $bindingTemps.Count -gt 0 -or $bindingOrphans.Count -gt 0) -and $null -eq $manifest) {
        throw 'Publication temp/orphan exists without the final bundle.'
    }
    if ($bindingTemps.Count -gt 1 -or ($bindingTemps.Count -gt 0 -and $bindingExists)) { throw 'Existing binding temp inventory is not a recoverable crash shape.' }
    if (($bindingTemps.Count -gt 0 -or $bindingOrphans.Count -gt 0) -and $null -eq $rows.Prepared) { throw 'Binding temp/orphan exists without BundlePrepared.' }
    if ($null -ne $manifest -and $attemptId) {
        Assert-ExistingPublicationRowArtifacts -Context $Context -Manifest $manifest -Rows $rows -TempLeaseChain (@($archives) + @($active | Where-Object { $null -ne $_ })) -OrphanLeaseChain $archives -AttemptId $attemptId
    }
    foreach ($item in $bindingTemps) {
        $record = Read-Binding -Context $Context -Manifest $manifest -Prepared $rows.Prepared -Path $item.FullName
        $orphanDestination = Join-Path $Context.BindingOrphanDirectory "$($item.BaseName).$(Get-Sha256Bytes $record.Bytes).orphan"
        if (Test-Path -LiteralPath $orphanDestination) { throw 'Binding temp collides with its deterministic orphan destination.' }
    }
    foreach ($item in $bindingOrphans) {
        $match = [regex]::Match($item.Name, '^[0-9a-f]{32}\.binding\.(?<hash>[0-9a-f]{64})\.orphan$')
        $record = Read-Binding -Context $Context -Manifest $manifest -Prepared $rows.Prepared -Path $item.FullName
        if (-not $match.Success -or (Get-Sha256Bytes $record.Bytes) -cne $match.Groups['hash'].Value -or ($null -ne $binding -and -not (Test-BytesEqual $record.Bytes $binding.Bytes))) {
            throw 'Binding orphan inventory is not canonical.'
        }
    }
    if ($bundleExists -and $null -eq $active -and $archives.Count -eq 0 -and $null -eq $rows.Committed) {
        throw 'A pre-existing final bundle has no active/archive publication provenance.'
    }
}

function Get-PublicationAdmissionPathSnapshot([string] $Path) {
    $full = [IO.Path]::GetFullPath($Path)
    if (-not (Test-Path -LiteralPath $full)) { return @('<absent>') }
    $rows = [Collections.Generic.List[string]]::new()
    $pending = [Collections.Generic.Queue[string]]::new()
    $pending.Enqueue($full)
    while ($pending.Count -gt 0) {
        $current = $pending.Dequeue()
        $item = Get-Item -LiteralPath $current -Force
        $relative = if ($current -ceq $full) { '.' } else { [IO.Path]::GetRelativePath($full, $current).Replace('\','/') }
        $record = Assert-SafeExistingPath -Path $current -LeafType $(if ($item.PSIsContainer) { 'Directory' } else { 'File' })
        if ($item.PSIsContainer) {
            $rows.Add("D|$relative|$([int64]$item.Attributes)|$($record.IdentitySha256)|$($record.Owner)|$($record.AclSha256)")
            foreach ($child in @(Get-ChildItem -LiteralPath $current -Force | Sort-Object Name -CaseSensitive)) { $pending.Enqueue($child.FullName) }
        }
        else {
            $bytes = [IO.File]::ReadAllBytes($current)
            $rows.Add("F|$relative|$([int64]$item.Attributes)|$($record.IdentitySha256)|$($record.Owner)|$($record.AclSha256)|$($bytes.LongLength)|$(Get-Sha256Bytes $bytes)")
        }
    }
    @($rows | Sort-Object -CaseSensitive)
}

function Get-PublicationAdmissionSnapshot([object] $Context) {
    [string]::Join("`n", @(
        @(Get-PublicationAdmissionPathSnapshot $Context.ControlRoot | ForEach-Object { "CONTROL|$_" })
        @(Get-PublicationAdmissionPathSnapshot $Context.PublicationRoot | ForEach-Object { "STATE|$_" })
        @(Get-PublicationAdmissionPathSnapshot $Context.BindingPath | ForEach-Object { "BINDING|$_" })
        @(Get-PublicationAdmissionPathSnapshot $Context.FinalBundle | ForEach-Object { "BUNDLE|$_" })
        @(Get-PublicationAdmissionPathSnapshot (Join-Path $Context.EvidenceRoot.Path 'Dynamo') | ForEach-Object { "EVIDENCE|$_" })
    ))
}

function Assert-PublicationAdmissionUnchanged {
    param(
        [object] $Context,
        [object] $ExpectedExecutionAnchor,
        [object] $ExpectedSourceSnapshot,
        [object[]] $ExpectedGitIgnoreEvidence,
        [string] $ExpectedStateSnapshot
    )
    Assert-PublicationMutationBoundaryUnchanged -Context $Context -ExpectedExecutionAnchor $ExpectedExecutionAnchor -ExpectedSourceSnapshot $ExpectedSourceSnapshot -ExpectedGitIgnoreEvidence $ExpectedGitIgnoreEvidence -ExpectedStateSnapshot $ExpectedStateSnapshot
    Assert-ExistingPublicationStateReadOnly $Context
}

function Assert-PublicationMutationBoundaryUnchanged {
    param(
        [object] $Context,
        [object] $ExpectedExecutionAnchor,
        [object] $ExpectedSourceSnapshot,
        [object[]] $ExpectedGitIgnoreEvidence,
        [string] $ExpectedStateSnapshot
    )
    Assert-ExecutionAnchorEqual -Expected $ExpectedExecutionAnchor -Actual (Get-ExecutionAnchor)
    Assert-SnapshotEqual -Expected $ExpectedSourceSnapshot -Actual (Get-SourceSnapshot)
    Assert-GitIgnoreEvidenceEqual -Expected $ExpectedGitIgnoreEvidence -Actual (Get-GitIgnoreEvidence)
    $evidence = Get-PathRecord $Context.EvidenceRoot.Path
    $common = Get-PathRecord $Context.CommonDirectory.Path
    if ($evidence.IdentitySha256 -cne $Context.EvidenceRoot.IdentitySha256 -or $evidence.Owner -cne $Context.EvidenceRoot.Owner -or $evidence.AclSha256 -cne $Context.EvidenceRoot.AclSha256 -or
        $common.IdentitySha256 -cne $Context.CommonDirectory.IdentitySha256 -or $common.Owner -cne $Context.CommonDirectory.Owner -or $common.AclSha256 -cne $Context.CommonDirectory.AclSha256) {
        throw 'Publication admission path identity/ACL changed while waiting.'
    }
    $control = Get-PathRecord $Context.ControlRoot
    if ($null -eq $Context.ControlRootRecord -or $control.IdentitySha256 -cne $Context.ControlRootRecord.IdentitySha256 -or
        $control.Owner -cne $Context.ControlRootRecord.Owner -or $control.AclSha256 -cne $Context.ControlRootRecord.AclSha256) {
        throw 'Publication control-root identity/ACL changed while waiting.'
    }
    Assert-RestrictedAcl -Path $Context.ControlRoot -Kind Directory
    if ((Get-PublicationAdmissionSnapshot $Context) -cne $ExpectedStateSnapshot) { throw 'Publication state changed while waiting for admission.' }
}

# Resolve the repository only from the committed publisher location.
$script:RepositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$canonicalHelperPath = Join-Path $script:RepositoryRoot 'scripts/remediation/modules/canonical-json.ps1'
. $canonicalHelperPath
$canonicalHelperBefore = Get-PathRecord $canonicalHelperPath
$canonicalHelperAfter = Get-PathRecord $canonicalHelperPath
if ($canonicalHelperBefore.IdentitySha256 -cne $canonicalHelperAfter.IdentitySha256 -or $canonicalHelperBefore.Owner -cne $canonicalHelperAfter.Owner -or $canonicalHelperBefore.AclSha256 -cne $canonicalHelperAfter.AclSha256) {
    throw 'Canonical helper path identity, owner, or ACL changed during load.'
}
$pathSecurityHelperPath = Join-Path $script:RepositoryRoot 'scripts/remediation/modules/path-security.ps1'
$pathSecurityHelperBefore = Get-PathRecord $pathSecurityHelperPath
. $pathSecurityHelperPath
$pathSecurityHelperAfter = Assert-SafeExistingPath -Path $pathSecurityHelperPath -LeafType File
if ($pathSecurityHelperBefore.IdentitySha256 -cne $pathSecurityHelperAfter.IdentitySha256 -or $pathSecurityHelperBefore.Owner -cne $pathSecurityHelperAfter.Owner -or $pathSecurityHelperBefore.AclSha256 -cne $pathSecurityHelperAfter.AclSha256) {
    throw 'Path-security helper path identity, owner, or ACL changed during load.'
}
$bindingSchemaHelperPath = Join-Path $script:RepositoryRoot 'scripts/remediation/modules/plan-set-binding-schema.ps1'
$bindingSchemaHelperBefore = Get-PathRecord $bindingSchemaHelperPath
. $bindingSchemaHelperPath
$bindingSchemaHelperAfter = Assert-SafeExistingPath -Path $bindingSchemaHelperPath -LeafType File
if ($bindingSchemaHelperBefore.IdentitySha256 -cne $bindingSchemaHelperAfter.IdentitySha256 -or $bindingSchemaHelperBefore.Owner -cne $bindingSchemaHelperAfter.Owner -or $bindingSchemaHelperBefore.AclSha256 -cne $bindingSchemaHelperAfter.AclSha256) {
    throw 'Binding-schema helper path identity, owner, or ACL changed during load.'
}
$publicationJournalHelperPath = Join-Path $script:RepositoryRoot 'scripts/remediation/modules/publisher-publication-journal.ps1'
$publicationJournalHelperBefore = Get-PathRecord $publicationJournalHelperPath
. $publicationJournalHelperPath
$publicationJournalHelperAfter = Assert-SafeExistingPath -Path $publicationJournalHelperPath -LeafType File
if ($publicationJournalHelperBefore.IdentitySha256 -cne $publicationJournalHelperAfter.IdentitySha256 -or $publicationJournalHelperBefore.Owner -cne $publicationJournalHelperAfter.Owner -or $publicationJournalHelperBefore.AclSha256 -cne $publicationJournalHelperAfter.AclSha256) {
    throw 'Publication-journal helper path identity, owner, or ACL changed during load.'
}
Initialize-ControlSchema
$evidenceRootValue = [Environment]::GetEnvironmentVariable('DYNAMO_REMEDIATION_EVIDENCE_ROOT', 'Process')
if ([string]::IsNullOrWhiteSpace($evidenceRootValue)) { throw 'DYNAMO_REMEDIATION_EVIDENCE_ROOT is required.' }
Assert-InitialEnvironment -EvidenceRoot $evidenceRootValue
$preflight = Assert-PublisherPreflight -EvidenceRoot $evidenceRootValue
$sourceSnapshot = Get-SourceSnapshot
Assert-ExecutionAnchorEqual -Expected $preflight.ExecutionAnchor -Actual $sourceSnapshot.ExecutionAnchor
$gitignoreEvidence = Get-GitIgnoreEvidence
Invoke-PublishBarrier -Name 'after-source-snapshot' -EvidenceRoot $preflight.EvidenceRoot.Path
$postSourceEvidence = Get-PathRecord $preflight.EvidenceRoot.Path
if ($postSourceEvidence.IdentitySha256 -cne $preflight.EvidenceRoot.IdentitySha256 -or $postSourceEvidence.Owner -cne $preflight.EvidenceRoot.Owner -or $postSourceEvidence.AclSha256 -cne $preflight.EvidenceRoot.AclSha256) {
    throw 'Evidence-root identity/ACL changed after the source snapshot.'
}
Assert-SnapshotEqual -Expected $sourceSnapshot -Actual (Get-SourceSnapshot)
Assert-GitIgnoreEvidenceEqual -Expected $gitignoreEvidence -Actual (Get-GitIgnoreEvidence)

$commonPath = [IO.Path]::GetFullPath((Invoke-Git -Arguments @('rev-parse','--path-format=absolute','--git-common-dir')).Stdout)
$commonRecord = Assert-SafeExistingPath -Path $commonPath -LeafType Directory
$core = [ordered]@{
    schema_version = 1
    audit_baseline = $script:AuditBaseline
    execution_baseline = $preflight.ExecutionBaseline
    git_common_dir_identity_sha256 = $commonRecord.IdentitySha256
    payloads = @($sourceSnapshot.Payloads)
    controls = @($sourceSnapshot.Controls)
}
$coreBytes = ConvertTo-CanonicalBytes $core
$planSetSha256 = Get-DomainHash 'dynamo-plan-set-v1' $coreBytes

$controlRootCandidate = Join-Path $commonRecord.Path 'dynamo-remediation'
$publicationRootCandidate = Join-Path $controlRootCandidate "publication-state-v1/$($preflight.ExecutionBaseline)/$planSetSha256"
$finalBundle = Join-Path $preflight.EvidenceRoot.Path "Dynamo/plan-set/$($preflight.ExecutionBaseline)/$planSetSha256"
$context = [pscustomobject]@{
    EvidenceRoot = $preflight.EvidenceRoot
    CommonDirectory = $commonRecord
    ExecutionBaseline = $preflight.ExecutionBaseline
    PlanSetSha256 = $planSetSha256
    CoreBytes = $coreBytes
    SourceSnapshot = $sourceSnapshot
    ControlSchema = $script:ControlSchema
    GitIgnoreEvidence = $gitignoreEvidence
    ControlRoot = $controlRootCandidate
    ControlRootRecord = $null
    PublicationRoot = $publicationRootCandidate
    LeasesDirectory = Join-Path $publicationRootCandidate 'leases'
    LeaseArchiveDirectory = Join-Path $publicationRootCandidate 'leases/archives'
    RowsDirectory = Join-Path $publicationRootCandidate 'journal/rows'
    RowTempDirectory = Join-Path $publicationRootCandidate 'journal/tmp'
    RowOrphanDirectory = Join-Path $publicationRootCandidate 'journal/orphans'
    BindingTempDirectory = Join-Path $publicationRootCandidate 'binding-tmp'
    BindingOrphanDirectory = Join-Path $publicationRootCandidate 'binding-orphans'
    ActiveLeasePath = Join-Path $publicationRootCandidate 'active-publication.lock'
    BindingPath = Join-Path $controlRootCandidate 'plan-set-binding-v1.json'
    FinalBundle = $finalBundle
    ActiveLease = $null
}

# Admission is read-only: impossible or corrupt partial state must fail before
# directory creation, lease archival, bundle publication, or journal mutation.
Assert-ExistingPublicationStateReadOnly $context

$controlRoot = Ensure-SafeDirectoryChain $commonRecord.Path @('dynamo-remediation')
$publicationRoot = Ensure-SafeDirectoryChain $controlRoot @('publication-state-v1',$preflight.ExecutionBaseline,$planSetSha256)
$leasesDirectory = Ensure-SafeDirectoryChain $publicationRoot @('leases')
$leaseArchiveDirectory = Ensure-SafeDirectoryChain $leasesDirectory @('archives')
$journalRoot = Ensure-SafeDirectoryChain $publicationRoot @('journal')
$rowsDirectory = Ensure-SafeDirectoryChain $journalRoot @('rows')
$rowTempDirectory = Ensure-SafeDirectoryChain $journalRoot @('tmp')
$rowOrphanDirectory = Ensure-SafeDirectoryChain $journalRoot @('orphans')
$bindingTempDirectory = Ensure-SafeDirectoryChain $publicationRoot @('binding-tmp')
$bindingOrphanDirectory = Ensure-SafeDirectoryChain $publicationRoot @('binding-orphans')
$context.ControlRootRecord = Get-PathRecord $controlRoot

$rows = Get-PublicationRows $context
if ($null -ne $rows.Committed) {
    $manifest = Assert-ManifestAndBundle $context
    if (-not (Test-Path -LiteralPath $context.BindingPath -PathType Leaf)) { throw 'BindingCommitted exists without the fixed binding.' }
    $binding = Read-Binding -Context $context -Manifest $manifest -Prepared $rows.Prepared
    if (Test-Path -LiteralPath $context.ActiveLeasePath -PathType Leaf) {
        $terminalLease = Read-PublicationLease -Path $context.ActiveLeasePath -Context $context
        Assert-TerminalPublicationLeaseChain -Context $context -Prepared $rows.Prepared -Committed $rows.Committed -TerminalLease $terminalLease
        Assert-OwnerDead -Owner $terminalLease.Value['owner'] -CommonIdentitySha256 $context.CommonDirectory.IdentitySha256
        Close-PublicationLease -Context $context -Lease $terminalLease | Out-Null
    }
    $handoff = Assert-PublicationComplete -Context $context -Manifest $manifest -Rows (Get-PublicationRows $context) -Binding $binding
    [Console]::Out.WriteLine(($handoff | ConvertTo-Json -Compress -Depth 10))
    exit 0
}

Assert-ExistingPublicationStateReadOnly $context
$admissionStateSnapshot = Get-PublicationAdmissionSnapshot $context
Invoke-PublishBarrier -Name 'before-publication-lease' -EvidenceRoot $context.EvidenceRoot.Path
Assert-PublicationAdmissionUnchanged -Context $context -ExpectedExecutionAnchor $preflight.ExecutionAnchor -ExpectedSourceSnapshot $sourceSnapshot -ExpectedGitIgnoreEvidence $gitignoreEvidence -ExpectedStateSnapshot $admissionStateSnapshot
$activeLease = Acquire-PublicationLease -Context $context -Rows $rows
$context.ActiveLease = $activeLease
$manifest = Publish-OrReadBundle $context
Invoke-PublishFailpoint 'after-bundle-publish'
Recover-PublicationTemps -Context $context -Manifest $manifest -Rows $rows

$rows = Get-PublicationRows $context
if ($null -eq $rows.Prepared) {
    $preparedValue = New-PublicationRow -Sequence 1 -Phase 'BundlePrepared' -Lease $activeLease.Value -Manifest $manifest -PreparedHash $script:ZeroSha256 -BindingHash $script:ZeroSha256 -PreviousHash $script:ZeroSha256 -Context $context
    $prepared = Add-PublicationRow -Value $preparedValue -Slug 'bundle-prepared' -Context $context
    $rows = [pscustomobject]@{ Prepared = $prepared; Committed = $null }
}
Invoke-PublishFailpoint 'after-bundle-prepared'

if (Test-Path -LiteralPath $context.BindingPath -PathType Leaf) {
    $binding = Read-Binding -Context $context -Manifest $manifest -Prepared $rows.Prepared
} else {
    $binding = Write-Binding -Context $context -Manifest $manifest -Prepared $rows.Prepared
}
Invoke-PublishFailpoint 'after-binding-create'

$rows = Get-PublicationRows $context
if ($null -eq $rows.Committed) {
    $committedValue = New-PublicationRow -Sequence 2 -Phase 'BindingCommitted' -Lease $activeLease.Value -Manifest $manifest -PreparedHash ([string]$rows.Prepared.Value['row_sha256']) -BindingHash $binding.Sha256 -PreviousHash ([string]$rows.Prepared.Value['row_sha256']) -Context $context
    $committed = Add-PublicationRow -Value $committedValue -Slug 'binding-committed' -Context $context
    $rows = [pscustomobject]@{ Prepared = $rows.Prepared; Committed = $committed }
}
Invoke-PublishFailpoint 'after-binding-committed'

Assert-TerminalPublicationLeaseChain -Context $context -Prepared $rows.Prepared -Committed $rows.Committed -TerminalLease $activeLease
Close-PublicationLease -Context $context -Lease $activeLease | Out-Null
$handoff = Assert-PublicationComplete -Context $context -Manifest $manifest -Rows (Get-PublicationRows $context) -Binding $binding
[Console]::Out.WriteLine(($handoff | ConvertTo-Json -Compress -Depth 10))
