Set-StrictMode -Version Latest

function Get-PublicationRowKeys {
    @('schema_version','seq','phase','execution_baseline','plan_set_sha256','attempt_id','generation','lease_sha256','evidence_root_identity_sha256','manifest_sha256','manifest_bytes','bundle_prepared_row_sha256','binding_sha256','utc','prev_row_sha256','row_sha256')
}

function Read-PublicationRow([string] $Path, [object] $Context) {
    $keys = Get-PublicationRowKeys
    $record = Read-CanonicalJsonFile -Path $Path -ExpectedKeys $keys
    $preimage = [ordered]@{}
    foreach ($key in $keys[0..($keys.Count - 2)]) { $preimage[$key] = $record.Value[$key] }
    if ((Get-DomainHash 'dynamo-publication-row-v1' (ConvertTo-CanonicalBytes $preimage)) -cne [string]$record.Value['row_sha256']) { throw 'Publication row hash mismatch.' }
    foreach ($field in @('schema_version','seq','generation','manifest_bytes')) { Assert-JsonInt64 $record.Value[$field] "Publication row $field" }
    if ([int64]$record.Value['schema_version'] -ne 1) { throw 'Publication row schema version mismatch.' }
    if ([string]$record.Value['execution_baseline'] -cne $Context.ExecutionBaseline -or [string]$record.Value['plan_set_sha256'] -cne $Context.PlanSetSha256) { throw 'Publication row plan identity mismatch.' }
    if ([string]$record.Value['attempt_id'] -cnotmatch '^[0-9a-f]{32}$' -or [int64]$record.Value['generation'] -lt 1) { throw 'Publication row attempt/generation is malformed.' }
    foreach ($field in @('lease_sha256','evidence_root_identity_sha256','manifest_sha256','bundle_prepared_row_sha256','binding_sha256','prev_row_sha256','row_sha256')) { Assert-LowerHex ([string]$record.Value[$field]) 64 "publication row $field" }
    if ([string]$record.Value['evidence_root_identity_sha256'] -cne $Context.EvidenceRoot.IdentitySha256 -or [int64]$record.Value['manifest_bytes'] -lt 1) { throw 'Publication row evidence/manifest identity mismatch.' }
    if ([string]$record.Value['utc'] -cnotmatch '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{7}Z$') { throw 'Publication row timestamp is noncanonical.' }
    $record
}

function Get-PublicationRows([object] $Context) {
    $items = @(Get-ChildItem -LiteralPath $Context.RowsDirectory -Force | Sort-Object Name)
    $allowed = @('00000000000000000001-bundle-prepared.json','00000000000000000002-binding-committed.json')
    foreach ($item in $items) { if ($item.PSIsContainer -or $item.Name -cnotin $allowed) { throw "Unexpected publication row: $($item.Name)" } }
    $prepared = $null
    $committed = $null
    $preparedPath = Join-Path $Context.RowsDirectory $allowed[0]
    $committedPath = Join-Path $Context.RowsDirectory $allowed[1]
    if (Test-Path -LiteralPath $preparedPath -PathType Leaf) {
        $prepared = Read-PublicationRow -Path $preparedPath -Context $Context
        if ([int64]$prepared.Value['seq'] -ne 1 -or [string]$prepared.Value['phase'] -cne 'BundlePrepared' -or
            [string]$prepared.Value['prev_row_sha256'] -cne $script:ZeroSha256 -or
            [string]$prepared.Value['bundle_prepared_row_sha256'] -cne $script:ZeroSha256 -or
            [string]$prepared.Value['binding_sha256'] -cne $script:ZeroSha256) { throw 'BundlePrepared invariants failed.' }
    }
    if (Test-Path -LiteralPath $committedPath -PathType Leaf) {
        if ($null -eq $prepared) { throw 'BindingCommitted exists without BundlePrepared.' }
        $committed = Read-PublicationRow -Path $committedPath -Context $Context
        if ([int64]$committed.Value['seq'] -ne 2 -or [string]$committed.Value['phase'] -cne 'BindingCommitted' -or
            [string]$committed.Value['prev_row_sha256'] -cne [string]$prepared.Value['row_sha256'] -or
            [string]$committed.Value['bundle_prepared_row_sha256'] -cne [string]$prepared.Value['row_sha256']) { throw 'BindingCommitted chain invariants failed.' }
        foreach ($field in @('execution_baseline','plan_set_sha256','attempt_id','evidence_root_identity_sha256','manifest_sha256','manifest_bytes')) {
            if ([string]$prepared.Value[$field] -cne [string]$committed.Value[$field]) { throw "Publication rows disagree on $field" }
        }
    }
    [pscustomobject]@{ Prepared = $prepared; Committed = $committed }
}

function Add-PublicationRow {
    param([Collections.IDictionary] $Value, [string] $Slug, [object] $Context)
    $valueBytes = ConvertTo-CanonicalBytes $Value
    $sequence = ([int64]$Value['seq']).ToString('D20', [Globalization.CultureInfo]::InvariantCulture)
    $nonce = [Guid]::NewGuid().ToString('N').ToLowerInvariant()
    $generation = ([int64]$Value['generation']).ToString('D10', [Globalization.CultureInfo]::InvariantCulture)
    $temp = Join-Path $Context.RowTempDirectory "$($Value['attempt_id']).g$generation.$sequence.$Slug.$nonce.tmp"
    $final = Join-Path $Context.RowsDirectory "$sequence-$Slug.json"
    Write-CreateNewDurable $temp $valueBytes
    Invoke-PublishFailpoint 'after-row-temp-write'
    Move-NoReplaceDurable $temp $final
    Read-PublicationRow -Path $final -Context $Context
}

function New-PublicationRow {
    param(
        [int64] $Sequence,
        [string] $Phase,
        [Collections.IDictionary] $Lease,
        [object] $Manifest,
        [string] $PreparedHash,
        [string] $BindingHash,
        [string] $PreviousHash,
        [object] $Context
    )
    $row = [ordered]@{
        schema_version = 1
        seq = $Sequence
        phase = $Phase
        execution_baseline = $Context.ExecutionBaseline
        plan_set_sha256 = $Context.PlanSetSha256
        attempt_id = [string]$Lease['attempt_id']
        generation = [int64]$Lease['generation']
        lease_sha256 = [string]$Lease['lease_sha256']
        evidence_root_identity_sha256 = $Context.EvidenceRoot.IdentitySha256
        manifest_sha256 = $Manifest.Sha256
        manifest_bytes = [int64]$Manifest.Bytes.LongLength
        bundle_prepared_row_sha256 = $PreparedHash
        binding_sha256 = $BindingHash
        utc = Get-UtcNowCanonical
        prev_row_sha256 = $PreviousHash
    }
    $row['row_sha256'] = Get-DomainHash 'dynamo-publication-row-v1' (ConvertTo-CanonicalBytes $row)
    $row
}
