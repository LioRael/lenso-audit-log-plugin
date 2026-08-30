use lenso_postgres_kit::{Migration, PlanError, SchemaPlan};

pub const AUDIT_LOG_SCHEMA: &str = "audit_log";
pub(crate) const AUDIT_LOG_MIGRATION_VERSION: u64 = 1;
pub(crate) const AUDIT_LOG_MIGRATION_NAME: &str = "create-audit-log-schema";
pub(crate) const AUDIT_LOG_MIGRATION_SQL: &str =
    include_str!("../migrations/001_create_audit_log_schema.sql");

const MIGRATIONS: &[Migration] = &[Migration::new(
    AUDIT_LOG_MIGRATION_VERSION,
    AUDIT_LOG_MIGRATION_NAME,
    AUDIT_LOG_MIGRATION_SQL,
)];

pub(crate) fn schema_plan() -> Result<SchemaPlan, PlanError> {
    SchemaPlan::new(AUDIT_LOG_SCHEMA, MIGRATIONS)
}
