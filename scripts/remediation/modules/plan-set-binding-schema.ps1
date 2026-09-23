Set-StrictMode -Version Latest

function Get-PlanSetBindingKeys([int64] $SchemaVersion) {
    if ($SchemaVersion -eq 1) {
        return @('schema_version','audit_baseline','execution_baseline','plan_set_sha256','manifest_native_path','manifest_sha256','manifest_bytes','git_common_dir_native_path','git_common_dir_identity_sha256','git_common_dir_owner','git_common_dir_acl_sha256','publisher_sha256','integration_helper_sha256','publisher_contract_test_sha256','integration_contract_test_sha256','bundle_prepared_row_sha256','binding_sha256')
    }
    if ($SchemaVersion -eq 2) {
        return @('schema_version','audit_baseline','execution_baseline','plan_set_sha256','manifest_native_path','manifest_sha256','manifest_bytes','git_common_dir_native_path','git_common_dir_identity_sha256','git_common_dir_owner','git_common_dir_acl_sha256','control_schema_path','control_schema_sha256','control_schema_version','control_hashes','bundle_prepared_row_sha256','binding_sha256')
    }
    throw "Unsupported binding schema version: $SchemaVersion"
}
