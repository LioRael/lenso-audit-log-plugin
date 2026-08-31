use std::{fmt, str::FromStr};

use lenso_postgres_kit::{
    OwnedPostgres, PostgresKitError, SchemaOperator, SetupOutcome, UpgradeOutcome,
};
use sha2::{Digest, Sha256};
use sqlx::{
    Connection, PgConnection, PgPool, Postgres, Row, Transaction,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use thiserror::Error;

use crate::schema::{
    AUDIT_LOG_MIGRATION_NAME, AUDIT_LOG_MIGRATION_SQL, AUDIT_LOG_MIGRATION_VERSION,
    AUDIT_LOG_SCHEMA, schema_plan,
};

const MANAGED_LEDGER: &str = "_lenso_schema_migrations";
const LEGACY_LEDGER_SCHEMA: &str = "platform";
const LEGACY_LEDGER_TABLE: &str = "schema_migrations";
const LEGACY_MIGRATION_NAME: &str = "audit-log/0001_create_audit_log_schema";
pub(crate) const DATABASE_MAINTENANCE_LOCK_SQL: &str =
    "SELECT pg_advisory_xact_lock(hashtextextended(current_database() || ':lenso-maintenance', 0))";
const UNSAFE_PUBLICATION_EXPOSURE_SQL: &str = "SELECT EXISTS ( \
       SELECT 1 FROM pg_publication AS publications \
       WHERE publications.puballtables \
       UNION ALL \
       SELECT 1 FROM pg_publication_namespace AS entries \
       JOIN pg_namespace AS namespaces ON namespaces.oid = entries.pnnspid \
       WHERE namespaces.nspname = $1 \
     )";

/// Explicit schema administration for the `PostgreSQL` Audit Log Plugin.
#[derive(Clone, Copy, Debug, Default)]
pub struct AuditLogOperator;

impl AuditLogOperator {
    /// Creates the fixed `audit_log` schema when it is absent.
    pub async fn setup(database_url: &str) -> Result<SetupOutcome, AuditLogOperatorError> {
        Ok(SchemaOperator::connect(database_url, schema_plan()?)
            .await?
            .setup()
            .await?)
    }

    /// Applies pending Audit Log migrations explicitly.
    pub async fn upgrade(database_url: &str) -> Result<UpgradeOutcome, AuditLogOperatorError> {
        Ok(SchemaOperator::connect(database_url, schema_plan()?)
            .await?
            .upgrade()
            .await?)
    }

    /// Adopts an exact legacy 0.1.5 Audit Log schema without rewriting its rows.
    ///
    /// This is an explicit operator-only transition for a database maintenance window. Every DDL
    /// actor in that window must participate in the Lenso database maintenance advisory-lock
    /// protocol; `PostgreSQL` has no SQL-level `LOCK SCHEMA` command that can exclude an
    /// uncoordinated database owner. The workflow refuses missing, partial, modified, extra,
    /// foreign-owned, over-granted, or already-managed schemas. Runtime preparation never calls
    /// it.
    pub async fn adopt_legacy(
        database_url: &str,
    ) -> Result<LegacyAdoptionOutcome, AuditLogOperatorError> {
        let options = PgConnectOptions::from_str(database_url)
            .map_err(|source| database("parse legacy adoption connection options", source))?
            .options([("application_name", "lenso-audit-log-legacy-adoption")]);
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|source| database("connect legacy adoption operator", source))?;
        let mut connection = pool
            .acquire()
            .await
            .map_err(|source| database("acquire legacy adoption connection", source))?;
        let mut transaction = connection
            .begin()
            .await
            .map_err(|source| database("begin legacy adoption transaction", source))?;

        acquire_database_maintenance_lock(&mut transaction).await?;
        acquire_schema_lock(&mut transaction).await?;
        verify_schema_owner_and_acl(
            &mut transaction,
            AUDIT_LOG_SCHEMA,
            LegacyAdoptionRefusal::SchemaShapeDiverged,
        )
        .await?;
        refuse_unsafe_publication_exposure(
            &mut transaction,
            AUDIT_LOG_SCHEMA,
            LegacyAdoptionRefusal::SchemaShapeDiverged,
        )
        .await?;
        refuse_unsafe_publication_exposure(
            &mut transaction,
            LEGACY_LEDGER_SCHEMA,
            LegacyAdoptionRefusal::LegacyLedgerDiverged,
        )
        .await?;
        refuse_managed_schema(&mut transaction).await?;
        verify_and_lock_legacy_ledger(&mut transaction).await?;
        verify_legacy_history(&mut transaction).await?;
        verify_and_lock_audit_schema(&mut transaction).await?;
        // Recheck after both persistent tables are locked. The schema advisory lock serializes
        // lenso-postgres-kit, the database maintenance lock serializes participating DDL actors,
        // and the relation locks serialize writers and DDL on the two persistent tables.
        refuse_unsafe_publication_exposure(
            &mut transaction,
            AUDIT_LOG_SCHEMA,
            LegacyAdoptionRefusal::SchemaShapeDiverged,
        )
        .await?;
        refuse_unsafe_publication_exposure(
            &mut transaction,
            LEGACY_LEDGER_SCHEMA,
            LegacyAdoptionRefusal::LegacyLedgerDiverged,
        )
        .await?;
        refuse_managed_schema(&mut transaction).await?;
        create_managed_ledger(&mut transaction).await?;
        verify_table_owner_and_acl(&mut transaction, AUDIT_LOG_SCHEMA, MANAGED_LEDGER).await?;
        record_current_migration(&mut transaction).await?;

        transaction
            .commit()
            .await
            .map_err(|source| database("commit legacy adoption", source))?;
        Ok(LegacyAdoptionOutcome::Adopted {
            version: AUDIT_LOG_MIGRATION_VERSION,
        })
    }
}

/// Successful explicit adoption of a legacy Audit Log schema.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdoptionOutcome {
    Adopted { version: u64 },
}

/// Fail-closed reason why a legacy schema was not adopted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdoptionRefusal {
    MissingOwnedSchema,
    OwnershipMismatch,
    UnexpectedPrivileges,
    AlreadyManaged,
    LegacyLedgerMissing,
    LegacyLedgerDiverged,
    SchemaShapeDiverged,
}

impl fmt::Display for LegacyAdoptionRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingOwnedSchema => "the legacy audit_log schema is missing",
            Self::OwnershipMismatch => "the operator does not own the legacy schema objects",
            Self::UnexpectedPrivileges => "the legacy schema has unexpected access grants",
            Self::AlreadyManaged => "the audit_log schema already has a Plugin ledger",
            Self::LegacyLedgerMissing => "the platform legacy migration ledger is missing",
            Self::LegacyLedgerDiverged => "the platform legacy migration evidence diverged",
            Self::SchemaShapeDiverged => "the legacy audit_log catalog shape diverged",
        })
    }
}

#[derive(Debug, Error)]
pub enum AuditLogOperatorError {
    #[error(transparent)]
    Plan(#[from] lenso_postgres_kit::PlanError),
    #[error(transparent)]
    Postgres(#[from] PostgresKitError),
    #[error(
        "managed Audit Log schema is exposed by a schema-wide or database-wide PostgreSQL publication"
    )]
    ManagedSchemaMismatch,
    #[error("legacy Audit Log adoption refused: {0}")]
    AdoptionRefused(LegacyAdoptionRefusal),
    #[error("PostgreSQL legacy adoption operation `{operation}` failed")]
    Database {
        operation: &'static str,
        #[source]
        source: sqlx::Error,
    },
}

fn database(operation: &'static str, source: sqlx::Error) -> AuditLogOperatorError {
    AuditLogOperatorError::Database { operation, source }
}

fn refuse(reason: LegacyAdoptionRefusal) -> AuditLogOperatorError {
    AuditLogOperatorError::AdoptionRefused(reason)
}

pub(crate) async fn prepare_managed(
    database_url: &str,
) -> Result<OwnedPostgres, AuditLogOperatorError> {
    let postgres = OwnedPostgres::prepare(database_url, schema_plan()?).await?;
    let verification = verify_managed_publication_safety(postgres.pool()).await;
    if let Err(error) = verification {
        postgres.pool().close().await;
        return Err(error);
    }
    Ok(postgres)
}

async fn verify_managed_publication_safety(pool: &PgPool) -> Result<(), AuditLogOperatorError> {
    let exposed: bool = sqlx::query_scalar(UNSAFE_PUBLICATION_EXPOSURE_SQL)
        .bind(AUDIT_LOG_SCHEMA)
        .fetch_one(pool)
        .await
        .map_err(|source| database("inspect managed publication exposure", source))?;
    if exposed {
        return Err(AuditLogOperatorError::ManagedSchemaMismatch);
    }
    Ok(())
}

async fn refuse_unsafe_publication_exposure(
    transaction: &mut Transaction<'_, Postgres>,
    schema: &str,
    refusal: LegacyAdoptionRefusal,
) -> Result<(), AuditLogOperatorError> {
    let exposed: bool = sqlx::query_scalar(UNSAFE_PUBLICATION_EXPOSURE_SQL)
        .bind(schema)
        .fetch_one(&mut **transaction)
        .await
        .map_err(|source| database("inspect legacy publication exposure", source))?;
    if exposed {
        return Err(refuse(refusal));
    }
    Ok(())
}

async fn acquire_schema_lock(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), AuditLogOperatorError> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock(
           hashtextextended(current_database() || ':' || $1, 0)
         )",
    )
    .bind(AUDIT_LOG_SCHEMA)
    .execute(&mut **transaction)
    .await
    .map_err(|source| database("lock owned schema", source))?;
    Ok(())
}

async fn acquire_database_maintenance_lock(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), AuditLogOperatorError> {
    sqlx::query(DATABASE_MAINTENANCE_LOCK_SQL)
        .execute(&mut **transaction)
        .await
        .map_err(|source| database("lock database maintenance window", source))?;
    Ok(())
}

async fn verify_schema_owner_and_acl(
    transaction: &mut Transaction<'_, Postgres>,
    schema: &str,
    metadata_refusal: LegacyAdoptionRefusal,
) -> Result<(), AuditLogOperatorError> {
    let row = sqlx::query(
        "SELECT roles.rolname::text AS owner,
                namespaces.nspacl IS NULL AS default_acl,
                NOT EXISTS (
                  SELECT 1 FROM pg_default_acl AS defaults
                  WHERE defaults.defaclrole = namespaces.nspowner
                    AND (defaults.defaclnamespace = 0
                      OR defaults.defaclnamespace = namespaces.oid)
                ) AS no_default_privileges,
                obj_description(namespaces.oid, 'pg_namespace') IS NULL AS no_comment,
                NOT EXISTS (
                  SELECT 1 FROM pg_seclabel AS labels
                  WHERE labels.classoid = 'pg_namespace'::regclass
                    AND labels.objoid = namespaces.oid
                ) AS no_security_label
         FROM pg_namespace AS namespaces
         JOIN pg_roles AS roles ON roles.oid = namespaces.nspowner
         WHERE namespaces.nspname = $1",
    )
    .bind(schema)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|source| database("inspect legacy schema owner and ACL", source))?;
    let Some(row) = row else {
        return Err(refuse(LegacyAdoptionRefusal::MissingOwnedSchema));
    };
    let current_role: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&mut **transaction)
        .await
        .map_err(|source| database("inspect adoption operator role", source))?;
    if row
        .try_get::<String, _>("owner")
        .map_err(|source| database("decode legacy schema owner", source))?
        != current_role
    {
        return Err(refuse(LegacyAdoptionRefusal::OwnershipMismatch));
    }
    let default_acl: bool = row
        .try_get("default_acl")
        .map_err(|source| database("decode legacy schema ACL", source))?;
    let no_default_privileges: bool = row
        .try_get("no_default_privileges")
        .map_err(|source| database("decode legacy default privileges", source))?;
    if !default_acl || !no_default_privileges {
        return Err(refuse(LegacyAdoptionRefusal::UnexpectedPrivileges));
    }
    let no_comment: bool = row
        .try_get("no_comment")
        .map_err(|source| database("decode legacy schema comment", source))?;
    let no_security_label: bool = row
        .try_get("no_security_label")
        .map_err(|source| database("decode legacy schema security label", source))?;
    if !no_comment || !no_security_label {
        return Err(refuse(metadata_refusal));
    }
    Ok(())
}

async fn refuse_managed_schema(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), AuditLogOperatorError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (
           SELECT 1
           FROM pg_class AS relations
           JOIN pg_namespace AS namespaces ON namespaces.oid = relations.relnamespace
           WHERE namespaces.nspname = $1 AND relations.relname = $2
         )",
    )
    .bind(AUDIT_LOG_SCHEMA)
    .bind(MANAGED_LEDGER)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|source| database("inspect Plugin migration ledger", source))?;
    if exists {
        return Err(refuse(LegacyAdoptionRefusal::AlreadyManaged));
    }
    Ok(())
}

async fn verify_and_lock_legacy_ledger(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), AuditLogOperatorError> {
    let relation_kind: Option<String> = sqlx::query_scalar(
        "SELECT relations.relkind::text
         FROM pg_class AS relations
         JOIN pg_namespace AS namespaces ON namespaces.oid = relations.relnamespace
         WHERE namespaces.nspname = $1 AND relations.relname = $2",
    )
    .bind(LEGACY_LEDGER_SCHEMA)
    .bind(LEGACY_LEDGER_TABLE)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|source| database("inspect legacy migration ledger", source))?;
    if relation_kind.as_deref() != Some("r") {
        return Err(refuse(LegacyAdoptionRefusal::LegacyLedgerMissing));
    }
    verify_schema_owner_and_acl(
        &mut *transaction,
        LEGACY_LEDGER_SCHEMA,
        LegacyAdoptionRefusal::LegacyLedgerDiverged,
    )
    .await?;
    sqlx::query("LOCK TABLE platform.schema_migrations IN SHARE MODE")
        .execute(&mut **transaction)
        .await
        .map_err(|source| database("lock legacy migration ledger", source))?;

    let exact = load_table_signature(transaction, LEGACY_LEDGER_SCHEMA, LEGACY_LEDGER_TABLE)
        .await?
        == Some(TableSignature::ordinary())
        && load_columns(transaction, LEGACY_LEDGER_SCHEMA, LEGACY_LEDGER_TABLE).await?
            == expected_legacy_ledger_columns()
        && load_constraints(transaction, LEGACY_LEDGER_SCHEMA, LEGACY_LEDGER_TABLE).await?
            == vec![ConstraintSignature::new(
                "schema_migrations_pkey",
                "p",
                &[1],
                "PRIMARY KEY (name)",
            )]
        && load_indexes(transaction, LEGACY_LEDGER_SCHEMA, LEGACY_LEDGER_TABLE).await?
            == vec![IndexSignature::primary(
                "schema_migrations_pkey",
                LEGACY_LEDGER_SCHEMA,
                LEGACY_LEDGER_TABLE,
                &["name"],
            )]
        && table_extra_object_count(transaction, LEGACY_LEDGER_SCHEMA, LEGACY_LEDGER_TABLE).await?
            == 0
        && load_relation_types(transaction, LEGACY_LEDGER_SCHEMA, LEGACY_LEDGER_TABLE).await?
            == vec![
                TypeSignature::owned("_schema_migrations", "b"),
                TypeSignature::owned("schema_migrations", "c"),
            ];
    if !exact {
        return Err(refuse(LegacyAdoptionRefusal::LegacyLedgerDiverged));
    }
    verify_table_owner_and_acl(transaction, LEGACY_LEDGER_SCHEMA, LEGACY_LEDGER_TABLE).await?;
    Ok(())
}

async fn verify_legacy_history(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), AuditLogOperatorError> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT name
         FROM platform.schema_migrations
         WHERE name LIKE 'audit-log/%'
         ORDER BY name",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(|source| database("read legacy Audit Log migration evidence", source))?;
    if names != [LEGACY_MIGRATION_NAME] {
        return Err(refuse(LegacyAdoptionRefusal::LegacyLedgerDiverged));
    }
    Ok(())
}

async fn verify_and_lock_audit_schema(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), AuditLogOperatorError> {
    let inventory = load_relation_inventory(transaction, AUDIT_LOG_SCHEMA).await?;
    let expected_inventory = vec![
        ("audit_log_events_actor_idx".to_owned(), "i".to_owned()),
        (
            "audit_log_events_correlation_idx".to_owned(),
            "i".to_owned(),
        ),
        ("audit_log_events_module_idx".to_owned(), "i".to_owned()),
        (
            "audit_log_events_occurred_at_idx".to_owned(),
            "i".to_owned(),
        ),
        ("audit_log_events_resource_idx".to_owned(), "i".to_owned()),
        ("audit_log_events_scope_idx".to_owned(), "i".to_owned()),
        ("events".to_owned(), "r".to_owned()),
        ("events_pkey".to_owned(), "i".to_owned()),
    ];
    if inventory != expected_inventory {
        return Err(refuse(LegacyAdoptionRefusal::SchemaShapeDiverged));
    }

    sqlx::query("LOCK TABLE audit_log.events IN ACCESS EXCLUSIVE MODE")
        .execute(&mut **transaction)
        .await
        .map_err(|source| database("lock legacy Audit Log table", source))?;

    let expected_constraints = vec![
        ConstraintSignature::new(
            "events_outcome_check",
            "c",
            &[5],
            "CHECK (outcome = ANY (ARRAY['success'::text, 'failure'::text, 'denied'::text]))",
        ),
        ConstraintSignature::new("events_pkey", "p", &[1], "PRIMARY KEY (id)"),
        ConstraintSignature::new(
            "events_severity_check",
            "c",
            &[6],
            "CHECK (severity = ANY (ARRAY['info'::text, 'warning'::text, 'critical'::text]))",
        ),
    ];
    let exact = load_table_signature(transaction, AUDIT_LOG_SCHEMA, "events").await?
        == Some(TableSignature::ordinary())
        && load_columns(transaction, AUDIT_LOG_SCHEMA, "events").await? == expected_audit_columns()
        && load_constraints(transaction, AUDIT_LOG_SCHEMA, "events").await? == expected_constraints
        && load_indexes(transaction, AUDIT_LOG_SCHEMA, "events").await? == expected_audit_indexes()
        && table_extra_object_count(transaction, AUDIT_LOG_SCHEMA, "events").await? == 0
        && schema_extra_object_count(transaction, AUDIT_LOG_SCHEMA).await? == 0
        && load_schema_types(transaction, AUDIT_LOG_SCHEMA).await?
            == vec![
                TypeSignature::owned("_events", "b"),
                TypeSignature::owned("events", "c"),
            ];
    if !exact {
        return Err(refuse(LegacyAdoptionRefusal::SchemaShapeDiverged));
    }
    verify_table_owner_and_acl(transaction, AUDIT_LOG_SCHEMA, "events").await?;
    Ok(())
}

fn expected_legacy_ledger_columns() -> Vec<ColumnSignature> {
    vec![
        ColumnSignature::new(1, "name", "text", true, None),
        ColumnSignature::new(
            2,
            "applied_at",
            "timestamp with time zone",
            true,
            Some("now()"),
        ),
    ]
}

fn expected_audit_columns() -> Vec<ColumnSignature> {
    vec![
        ColumnSignature::new(1, "id", "text", true, None),
        ColumnSignature::new(2, "event_name", "text", true, None),
        ColumnSignature::new(3, "module_name", "text", true, None),
        ColumnSignature::new(4, "action", "text", true, None),
        ColumnSignature::new(5, "outcome", "text", true, None),
        ColumnSignature::new(6, "severity", "text", true, None),
        ColumnSignature::new(7, "actor_kind", "text", true, None),
        ColumnSignature::new(8, "actor_id", "text", false, None),
        ColumnSignature::new(9, "actor_display", "text", false, None),
        ColumnSignature::new(10, "scope_module", "text", false, None),
        ColumnSignature::new(11, "scope_type", "text", false, None),
        ColumnSignature::new(12, "scope_id", "text", false, None),
        ColumnSignature::new(13, "scope_display", "text", false, None),
        ColumnSignature::new(14, "resource_type", "text", false, None),
        ColumnSignature::new(15, "resource_id", "text", false, None),
        ColumnSignature::new(16, "resource_display", "text", false, None),
        ColumnSignature::new(17, "correlation_id", "text", false, None),
        ColumnSignature::new(18, "causation_id", "text", false, None),
        ColumnSignature::new(19, "request_id", "text", false, None),
        ColumnSignature::new(20, "story_id", "text", false, None),
        ColumnSignature::new(21, "reason", "text", false, None),
        ColumnSignature::new(22, "metadata", "jsonb", true, Some("'{}'::jsonb")),
        ColumnSignature::new(23, "occurred_at", "timestamp with time zone", true, None),
        ColumnSignature::new(
            24,
            "created_at",
            "timestamp with time zone",
            true,
            Some("now()"),
        ),
    ]
}

fn expected_audit_indexes() -> Vec<IndexSignature> {
    vec![
        IndexSignature::secondary(
            "audit_log_events_actor_idx",
            &["actor_id", "occurred_at DESC", "id DESC"],
            Some("actor_id IS NOT NULL"),
        ),
        IndexSignature::secondary(
            "audit_log_events_correlation_idx",
            &["correlation_id"],
            Some("correlation_id IS NOT NULL"),
        ),
        IndexSignature::secondary(
            "audit_log_events_module_idx",
            &["module_name", "occurred_at DESC", "id DESC"],
            None,
        ),
        IndexSignature::secondary(
            "audit_log_events_occurred_at_idx",
            &["occurred_at DESC", "id DESC"],
            None,
        ),
        IndexSignature::secondary(
            "audit_log_events_resource_idx",
            &[
                "resource_type",
                "resource_id",
                "occurred_at DESC",
                "id DESC",
            ],
            Some("resource_type IS NOT NULL AND resource_id IS NOT NULL"),
        ),
        IndexSignature::secondary(
            "audit_log_events_scope_idx",
            &["scope_type", "scope_id", "occurred_at DESC", "id DESC"],
            Some("scope_type IS NOT NULL AND scope_id IS NOT NULL"),
        ),
        IndexSignature::primary("events_pkey", AUDIT_LOG_SCHEMA, "events", &["id"]),
    ]
}

async fn create_managed_ledger(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), AuditLogOperatorError> {
    sqlx::query(
        "CREATE TABLE audit_log._lenso_schema_migrations (
           version bigint PRIMARY KEY CHECK (version > 0),
           name text NOT NULL,
           checksum text NOT NULL,
           applied_at timestamptz NOT NULL DEFAULT transaction_timestamp()
         )",
    )
    .execute(&mut **transaction)
    .await
    .map_err(|source| database("create Plugin migration ledger", source))?;
    Ok(())
}

async fn record_current_migration(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), AuditLogOperatorError> {
    let version = i64::try_from(AUDIT_LOG_MIGRATION_VERSION)
        .expect("validated Audit Log migration version fits PostgreSQL bigint");
    sqlx::query(
        "INSERT INTO audit_log._lenso_schema_migrations (version, name, checksum)
         VALUES ($1, $2, $3)",
    )
    .bind(version)
    .bind(AUDIT_LOG_MIGRATION_NAME)
    .bind(current_migration_checksum())
    .execute(&mut **transaction)
    .await
    .map_err(|source| database("record adopted migration", source))?;
    Ok(())
}

fn current_migration_checksum() -> String {
    let mut digest = Sha256::new();
    digest.update(AUDIT_LOG_MIGRATION_VERSION.to_be_bytes());
    digest.update([0]);
    digest.update(AUDIT_LOG_MIGRATION_NAME.as_bytes());
    digest.update([0]);
    digest.update(AUDIT_LOG_MIGRATION_SQL.as_bytes());
    format!("{:x}", digest.finalize())
}

// PostgreSQL exposes these independent catalog bits as booleans; preserving
// each bit is the purpose of this exact adoption fingerprint.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Eq, PartialEq)]
struct TableSignature {
    kind: String,
    persistence: String,
    access_method: String,
    replica_identity: String,
    row_security: bool,
    force_row_security: bool,
    is_partition: bool,
    options: Vec<String>,
    comment_is_absent: bool,
    security_label_count: i64,
}

impl TableSignature {
    fn ordinary() -> Self {
        Self {
            kind: "r".to_owned(),
            persistence: "p".to_owned(),
            access_method: "heap".to_owned(),
            replica_identity: "d".to_owned(),
            row_security: false,
            force_row_security: false,
            is_partition: false,
            options: Vec::new(),
            comment_is_absent: true,
            security_label_count: 0,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct TypeSignature {
    name: String,
    kind: String,
    owner_matches: bool,
    acl_is_default: bool,
    comment_is_absent: bool,
    security_label_count: i64,
}

impl TypeSignature {
    fn owned(name: &str, kind: &str) -> Self {
        Self {
            name: name.to_owned(),
            kind: kind.to_owned(),
            owner_matches: true,
            acl_is_default: true,
            comment_is_absent: true,
            security_label_count: 0,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct ColumnSignature {
    position: i16,
    name: String,
    data_type: String,
    not_null: bool,
    default_expression: Option<String>,
    identity: String,
    generated: String,
    collation: Option<String>,
    acl_is_default: bool,
    comment_is_absent: bool,
}

impl ColumnSignature {
    fn new(
        position: i16,
        name: &str,
        data_type: &str,
        not_null: bool,
        default_expression: Option<&str>,
    ) -> Self {
        Self {
            position,
            name: name.to_owned(),
            data_type: data_type.to_owned(),
            not_null,
            default_expression: default_expression.map(normalize_sql),
            identity: String::new(),
            generated: String::new(),
            collation: (data_type == "text").then(|| "default".to_owned()),
            acl_is_default: true,
            comment_is_absent: true,
        }
    }
}

// Constraint catalog flags are independent and must not be collapsed.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Eq, PartialEq)]
struct ConstraintSignature {
    name: String,
    kind: String,
    columns: Vec<i16>,
    definition: String,
    validated: bool,
    deferrable: bool,
    initially_deferred: bool,
    comment_is_absent: bool,
    security_label_count: i64,
}

impl ConstraintSignature {
    fn new(name: &str, kind: &str, columns: &[i16], definition: &str) -> Self {
        Self {
            name: name.to_owned(),
            kind: kind.to_owned(),
            columns: columns.to_vec(),
            definition: normalize_sql(definition),
            validated: true,
            deferrable: false,
            initially_deferred: false,
            comment_is_absent: true,
            security_label_count: 0,
        }
    }
}

// Index catalog flags are independent and all participate in exact adoption.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Eq, PartialEq)]
struct IndexSignature {
    name: String,
    unique: bool,
    primary: bool,
    exclusion: bool,
    immediate: bool,
    clustered: bool,
    replica_identity: bool,
    check_xmin: bool,
    nulls_not_distinct: bool,
    valid: bool,
    ready: bool,
    live: bool,
    access_method: String,
    attribute_count: i16,
    key_attribute_count: i16,
    definition: String,
    keys: Vec<String>,
    predicate: Option<String>,
    comment_is_absent: bool,
    security_label_count: i64,
}

impl IndexSignature {
    fn primary(name: &str, schema: &str, table: &str, keys: &[&str]) -> Self {
        Self::new(name, schema, table, true, true, keys, None)
    }

    fn secondary(name: &str, keys: &[&str], predicate: Option<&str>) -> Self {
        Self::new(
            name,
            AUDIT_LOG_SCHEMA,
            "events",
            false,
            false,
            keys,
            predicate,
        )
    }

    fn new(
        name: &str,
        schema: &str,
        table: &str,
        unique: bool,
        primary: bool,
        keys: &[&str],
        predicate: Option<&str>,
    ) -> Self {
        let unique_keyword = if unique { "UNIQUE " } else { "" };
        let predicate_clause =
            predicate.map_or_else(String::new, |predicate| format!(" WHERE ({predicate})"));
        let definition = normalize_sql(&format!(
            "CREATE {unique_keyword}INDEX {name} ON {schema}.{table} USING btree ({}){predicate_clause}",
            keys.join(", ")
        ));
        Self {
            name: name.to_owned(),
            unique,
            primary,
            exclusion: false,
            immediate: true,
            clustered: false,
            replica_identity: false,
            check_xmin: false,
            nulls_not_distinct: false,
            valid: true,
            ready: true,
            live: true,
            access_method: "btree".to_owned(),
            attribute_count: i16::try_from(keys.len()).expect("Audit index key count fits i16"),
            key_attribute_count: i16::try_from(keys.len()).expect("Audit index key count fits i16"),
            definition,
            keys: keys
                .iter()
                .map(|key| key.strip_suffix(" DESC").unwrap_or(key))
                .map(normalize_sql)
                .collect(),
            predicate: predicate.map(normalize_sql),
            comment_is_absent: true,
            security_label_count: 0,
        }
    }
}

async fn load_table_signature(
    connection: &mut PgConnection,
    schema: &str,
    table: &str,
) -> Result<Option<TableSignature>, AuditLogOperatorError> {
    let row = sqlx::query(
        "SELECT relations.relkind::text AS kind,
                relations.relpersistence::text AS persistence,
                access_methods.amname::text AS access_method,
                relations.relreplident::text AS replica_identity,
                relations.relrowsecurity AS row_security,
                relations.relforcerowsecurity AS force_row_security,
                relations.relispartition AS is_partition,
                COALESCE(relations.reloptions, ARRAY[]::text[]) AS options,
                obj_description(relations.oid, 'pg_class') IS NULL AS comment_is_absent,
                (SELECT count(*) FROM pg_seclabel AS labels
                 WHERE labels.classoid = 'pg_class'::regclass
                   AND labels.objoid = relations.oid) AS security_label_count
         FROM pg_class AS relations
         JOIN pg_namespace AS namespaces ON namespaces.oid = relations.relnamespace
         JOIN pg_am AS access_methods ON access_methods.oid = relations.relam
         WHERE namespaces.nspname = $1 AND relations.relname = $2",
    )
    .bind(schema)
    .bind(table)
    .fetch_optional(connection)
    .await
    .map_err(|source| database("inspect table properties", source))?;
    row.map(|row| {
        Ok(TableSignature {
            kind: row.try_get("kind")?,
            persistence: row.try_get("persistence")?,
            access_method: row.try_get("access_method")?,
            replica_identity: row.try_get("replica_identity")?,
            row_security: row.try_get("row_security")?,
            force_row_security: row.try_get("force_row_security")?,
            is_partition: row.try_get("is_partition")?,
            options: row.try_get("options")?,
            comment_is_absent: row.try_get("comment_is_absent")?,
            security_label_count: row.try_get("security_label_count")?,
        })
    })
    .transpose()
    .map_err(|source| database("decode table properties", source))
}

async fn verify_table_owner_and_acl(
    connection: &mut PgConnection,
    schema: &str,
    table: &str,
) -> Result<(), AuditLogOperatorError> {
    let row = sqlx::query(
        "SELECT pg_get_userbyid(relations.relowner) = current_user AS owner_matches,
                relations.relacl IS NULL
                AND NOT EXISTS (
                  SELECT 1 FROM pg_attribute AS attributes
                  WHERE attributes.attrelid = relations.oid
                    AND attributes.attnum > 0
                    AND NOT attributes.attisdropped
                    AND attributes.attacl IS NOT NULL
                ) AS default_acl
         FROM pg_class AS relations
         JOIN pg_namespace AS namespaces ON namespaces.oid = relations.relnamespace
         WHERE namespaces.nspname = $1 AND relations.relname = $2",
    )
    .bind(schema)
    .bind(table)
    .fetch_optional(connection)
    .await
    .map_err(|source| database("inspect table and column ownership grants", source))?;
    let Some(row) = row else {
        return Err(refuse(LegacyAdoptionRefusal::SchemaShapeDiverged));
    };
    if !row
        .try_get::<bool, _>("owner_matches")
        .map_err(|source| database("decode table owner", source))?
    {
        return Err(refuse(LegacyAdoptionRefusal::OwnershipMismatch));
    }
    if !row
        .try_get::<bool, _>("default_acl")
        .map_err(|source| database("decode table and column ACL", source))?
    {
        return Err(refuse(LegacyAdoptionRefusal::UnexpectedPrivileges));
    }
    Ok(())
}

async fn load_columns(
    connection: &mut PgConnection,
    schema: &str,
    table: &str,
) -> Result<Vec<ColumnSignature>, AuditLogOperatorError> {
    let rows = sqlx::query(
        "SELECT attributes.attnum AS position,
                attributes.attname::text AS name,
                format_type(attributes.atttypid, attributes.atttypmod)::text AS data_type,
                attributes.attnotnull AS not_null,
                pg_get_expr(defaults.adbin, defaults.adrelid, true)::text AS default_expression,
                attributes.attidentity::text AS identity,
                attributes.attgenerated::text AS generated,
                collations.collname::text AS collation,
                attributes.attacl IS NULL AS acl_is_default,
                col_description(relations.oid, attributes.attnum) IS NULL AS comment_is_absent
         FROM pg_attribute AS attributes
         JOIN pg_class AS relations ON relations.oid = attributes.attrelid
         JOIN pg_namespace AS namespaces ON namespaces.oid = relations.relnamespace
         LEFT JOIN pg_attrdef AS defaults
           ON defaults.adrelid = attributes.attrelid AND defaults.adnum = attributes.attnum
         LEFT JOIN pg_collation AS collations ON collations.oid = attributes.attcollation
         WHERE namespaces.nspname = $1
           AND relations.relname = $2
           AND attributes.attnum > 0
           AND NOT attributes.attisdropped
         ORDER BY attributes.attnum",
    )
    .bind(schema)
    .bind(table)
    .fetch_all(connection)
    .await
    .map_err(|source| database("inspect table columns", source))?;
    rows.into_iter()
        .map(|row| {
            let default_expression = row
                .try_get::<Option<String>, _>("default_expression")?
                .map(|value| normalize_sql(&value));
            Ok(ColumnSignature {
                position: row.try_get("position")?,
                name: row.try_get("name")?,
                data_type: row.try_get("data_type")?,
                not_null: row.try_get("not_null")?,
                default_expression,
                identity: row.try_get("identity")?,
                generated: row.try_get("generated")?,
                collation: row.try_get("collation")?,
                acl_is_default: row.try_get("acl_is_default")?,
                comment_is_absent: row.try_get("comment_is_absent")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(|source| database("decode table columns", source))
}

async fn load_constraints(
    connection: &mut PgConnection,
    schema: &str,
    table: &str,
) -> Result<Vec<ConstraintSignature>, AuditLogOperatorError> {
    let rows = sqlx::query(
        "SELECT constraints.conname::text AS name,
                constraints.contype::text AS kind,
                COALESCE(constraints.conkey, ARRAY[]::smallint[]) AS columns,
                pg_get_constraintdef(constraints.oid, true)::text AS definition,
                constraints.convalidated AS validated,
                constraints.condeferrable AS deferrable,
                constraints.condeferred AS initially_deferred
                , obj_description(constraints.oid, 'pg_constraint') IS NULL AS comment_is_absent,
                (SELECT count(*) FROM pg_seclabel AS labels
                 WHERE labels.classoid = 'pg_constraint'::regclass
                   AND labels.objoid = constraints.oid) AS security_label_count
         FROM pg_constraint AS constraints
         JOIN pg_class AS relations ON relations.oid = constraints.conrelid
         JOIN pg_namespace AS namespaces ON namespaces.oid = relations.relnamespace
         WHERE namespaces.nspname = $1
           AND relations.relname = $2
           AND constraints.contype <> 'n'
         ORDER BY constraints.conname",
    )
    .bind(schema)
    .bind(table)
    .fetch_all(connection)
    .await
    .map_err(|source| database("inspect table constraints", source))?;
    rows.into_iter()
        .map(|row| {
            Ok(ConstraintSignature {
                name: row.try_get("name")?,
                kind: row.try_get("kind")?,
                columns: row.try_get("columns")?,
                definition: normalize_sql(&row.try_get::<String, _>("definition")?),
                validated: row.try_get("validated")?,
                deferrable: row.try_get("deferrable")?,
                initially_deferred: row.try_get("initially_deferred")?,
                comment_is_absent: row.try_get("comment_is_absent")?,
                security_label_count: row.try_get("security_label_count")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(|source| database("decode table constraints", source))
}

async fn load_indexes(
    connection: &mut PgConnection,
    schema: &str,
    table: &str,
) -> Result<Vec<IndexSignature>, AuditLogOperatorError> {
    let rows = sqlx::query(
        "SELECT index_relations.relname::text AS name,
                indexes.indisunique AS unique,
                indexes.indisprimary AS primary,
                indexes.indisexclusion AS exclusion,
                indexes.indimmediate AS immediate,
                indexes.indisclustered AS clustered,
                indexes.indisreplident AS replica_identity,
                indexes.indcheckxmin AS check_xmin,
                indexes.indnullsnotdistinct AS nulls_not_distinct,
                indexes.indisvalid AS valid,
                indexes.indisready AS ready,
                indexes.indislive AS live,
                access_methods.amname::text AS access_method,
                indexes.indnatts AS attribute_count,
                indexes.indnkeyatts AS key_attribute_count,
                pg_get_indexdef(index_relations.oid)::text AS definition,
                ARRAY(
                  SELECT pg_get_indexdef(indexes.indexrelid, key_number, true)::text
                  FROM generate_series(1, indexes.indnkeyatts::integer) AS key_number
                  ORDER BY key_number
                ) AS keys,
                pg_get_expr(indexes.indpred, indexes.indrelid, true)::text AS predicate,
                obj_description(index_relations.oid, 'pg_class') IS NULL AS comment_is_absent,
                (SELECT count(*) FROM pg_seclabel AS labels
                 WHERE labels.classoid = 'pg_class'::regclass
                   AND labels.objoid = index_relations.oid) AS security_label_count
         FROM pg_index AS indexes
         JOIN pg_class AS relations ON relations.oid = indexes.indrelid
         JOIN pg_namespace AS namespaces ON namespaces.oid = relations.relnamespace
         JOIN pg_class AS index_relations ON index_relations.oid = indexes.indexrelid
         JOIN pg_am AS access_methods ON access_methods.oid = index_relations.relam
         WHERE namespaces.nspname = $1 AND relations.relname = $2
         ORDER BY index_relations.relname",
    )
    .bind(schema)
    .bind(table)
    .fetch_all(connection)
    .await
    .map_err(|source| database("inspect table indexes", source))?;
    rows.into_iter()
        .map(|row| {
            let keys = row
                .try_get::<Vec<String>, _>("keys")?
                .into_iter()
                .map(|value| normalize_sql(&value))
                .collect();
            let predicate = row
                .try_get::<Option<String>, _>("predicate")?
                .map(|value| normalize_sql(&value));
            Ok(IndexSignature {
                name: row.try_get("name")?,
                unique: row.try_get("unique")?,
                primary: row.try_get("primary")?,
                exclusion: row.try_get("exclusion")?,
                immediate: row.try_get("immediate")?,
                clustered: row.try_get("clustered")?,
                replica_identity: row.try_get("replica_identity")?,
                check_xmin: row.try_get("check_xmin")?,
                nulls_not_distinct: row.try_get("nulls_not_distinct")?,
                valid: row.try_get("valid")?,
                ready: row.try_get("ready")?,
                live: row.try_get("live")?,
                access_method: row.try_get("access_method")?,
                attribute_count: row.try_get("attribute_count")?,
                key_attribute_count: row.try_get("key_attribute_count")?,
                definition: normalize_sql(&row.try_get::<String, _>("definition")?),
                keys,
                predicate,
                comment_is_absent: row.try_get("comment_is_absent")?,
                security_label_count: row.try_get("security_label_count")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(|source| database("decode table indexes", source))
}

async fn load_relation_inventory(
    connection: &mut PgConnection,
    schema: &str,
) -> Result<Vec<(String, String)>, AuditLogOperatorError> {
    sqlx::query_as(
        "SELECT relations.relname::text, relations.relkind::text
         FROM pg_class AS relations
         JOIN pg_namespace AS namespaces ON namespaces.oid = relations.relnamespace
         WHERE namespaces.nspname = $1
         ORDER BY relations.relname",
    )
    .bind(schema)
    .fetch_all(connection)
    .await
    .map_err(|source| database("inspect owned schema relations", source))
}

async fn load_schema_types(
    connection: &mut PgConnection,
    schema: &str,
) -> Result<Vec<TypeSignature>, AuditLogOperatorError> {
    let rows = sqlx::query(
        "SELECT types.typname::text AS name,
                types.typtype::text AS kind,
                pg_get_userbyid(types.typowner) = current_user AS owner_matches,
                types.typacl IS NULL AS acl_is_default,
                obj_description(types.oid, 'pg_type') IS NULL AS comment_is_absent,
                (SELECT count(*) FROM pg_seclabel AS labels
                 WHERE labels.classoid = 'pg_type'::regclass
                   AND labels.objoid = types.oid) AS security_label_count
         FROM pg_type AS types
         JOIN pg_namespace AS namespaces ON namespaces.oid = types.typnamespace
         WHERE namespaces.nspname = $1
         ORDER BY types.typname",
    )
    .bind(schema)
    .fetch_all(connection)
    .await
    .map_err(|source| database("inspect owned schema types", source))?;
    decode_type_signatures(rows)
}

async fn load_relation_types(
    connection: &mut PgConnection,
    schema: &str,
    table: &str,
) -> Result<Vec<TypeSignature>, AuditLogOperatorError> {
    let rows = sqlx::query(
        "SELECT types.typname::text AS name,
                types.typtype::text AS kind,
                pg_get_userbyid(types.typowner) = current_user AS owner_matches,
                types.typacl IS NULL AS acl_is_default,
                obj_description(types.oid, 'pg_type') IS NULL AS comment_is_absent,
                (SELECT count(*) FROM pg_seclabel AS labels
                 WHERE labels.classoid = 'pg_type'::regclass
                   AND labels.objoid = types.oid) AS security_label_count
         FROM pg_type AS types
         JOIN pg_namespace AS namespaces ON namespaces.oid = types.typnamespace
         JOIN pg_class AS relations ON relations.relnamespace = namespaces.oid
         WHERE namespaces.nspname = $1
           AND relations.relname = $2
           AND (types.typrelid = relations.oid
             OR types.typelem = (
               SELECT row_type.oid
               FROM pg_type AS row_type
               WHERE row_type.typrelid = relations.oid
             ))
         ORDER BY types.typname",
    )
    .bind(schema)
    .bind(table)
    .fetch_all(connection)
    .await
    .map_err(|source| database("inspect relation-owned types", source))?;
    decode_type_signatures(rows)
}

fn decode_type_signatures(
    rows: Vec<sqlx::postgres::PgRow>,
) -> Result<Vec<TypeSignature>, AuditLogOperatorError> {
    rows.into_iter()
        .map(|row| {
            Ok(TypeSignature {
                name: row.try_get("name")?,
                kind: row.try_get("kind")?,
                owner_matches: row.try_get("owner_matches")?,
                acl_is_default: row.try_get("acl_is_default")?,
                comment_is_absent: row.try_get("comment_is_absent")?,
                security_label_count: row.try_get("security_label_count")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(|source| database("decode schema-owned types", source))
}

async fn table_extra_object_count(
    connection: &mut PgConnection,
    schema: &str,
    table: &str,
) -> Result<i64, AuditLogOperatorError> {
    sqlx::query_scalar(
        "SELECT
           (SELECT count(*) FROM pg_trigger AS triggers
              WHERE triggers.tgrelid = relations.oid AND NOT triggers.tgisinternal)
         + (SELECT count(*) FROM pg_policy AS policies
              WHERE policies.polrelid = relations.oid)
         + (SELECT count(*) FROM pg_rewrite AS rules
              WHERE rules.ev_class = relations.oid AND rules.rulename <> '_RETURN')
         + (SELECT count(*) FROM pg_inherits AS inheritance
              WHERE inheritance.inhrelid = relations.oid OR inheritance.inhparent = relations.oid)
         + (SELECT count(*) FROM pg_publication_rel AS publications
              WHERE publications.prrelid = relations.oid)
         FROM pg_class AS relations
         JOIN pg_namespace AS namespaces ON namespaces.oid = relations.relnamespace
         WHERE namespaces.nspname = $1 AND relations.relname = $2",
    )
    .bind(schema)
    .bind(table)
    .fetch_optional(connection)
    .await
    .map_err(|source| database("inspect table-owned extra objects", source))?
    .ok_or_else(|| refuse(LegacyAdoptionRefusal::SchemaShapeDiverged))
}

async fn schema_extra_object_count(
    connection: &mut PgConnection,
    schema: &str,
) -> Result<i64, AuditLogOperatorError> {
    // PostgreSQL 18 exposes schema ownership through the namespace columns counted below plus
    // pg_class.relnamespace, pg_constraint.connamespace, pg_default_acl.defaclnamespace,
    // pg_publication_namespace.pnnspid, and pg_type.typnamespace. Relation inventory, exact
    // per-table constraints, default-ACL proof, publication proof, and exact type inventory cover
    // those five catalogs respectively. The orphan-constraint term below rejects any constraint
    // that does not belong to one of the already-inventoried relations or types.
    sqlx::query_scalar(
        "SELECT
           (SELECT count(*) FROM pg_proc WHERE pronamespace = namespaces.oid)
         + (SELECT count(*) FROM pg_collation WHERE collnamespace = namespaces.oid)
         + (SELECT count(*) FROM pg_conversion WHERE connamespace = namespaces.oid)
         + (SELECT count(*) FROM pg_operator WHERE oprnamespace = namespaces.oid)
         + (SELECT count(*) FROM pg_opclass WHERE opcnamespace = namespaces.oid)
         + (SELECT count(*) FROM pg_opfamily WHERE opfnamespace = namespaces.oid)
         + (SELECT count(*) FROM pg_statistic_ext WHERE stxnamespace = namespaces.oid)
         + (SELECT count(*) FROM pg_constraint AS constraints
              WHERE constraints.connamespace = namespaces.oid
                AND NOT EXISTS (
                  SELECT 1 FROM pg_class AS relations
                  WHERE relations.oid = constraints.conrelid
                    AND relations.relnamespace = namespaces.oid
                )
                AND NOT EXISTS (
                  SELECT 1 FROM pg_type AS types
                  WHERE types.oid = constraints.contypid
                    AND types.typnamespace = namespaces.oid
                ))
         + (SELECT count(*) FROM pg_extension WHERE extnamespace = namespaces.oid)
         + (SELECT count(*) FROM pg_ts_config WHERE cfgnamespace = namespaces.oid)
         + (SELECT count(*) FROM pg_ts_dict WHERE dictnamespace = namespaces.oid)
         + (SELECT count(*) FROM pg_ts_parser WHERE prsnamespace = namespaces.oid)
         + (SELECT count(*) FROM pg_ts_template WHERE tmplnamespace = namespaces.oid)
         + (SELECT count(*) FROM pg_type AS types
              WHERE types.typnamespace = namespaces.oid
                AND obj_description(types.oid, 'pg_type') IS NOT NULL)
         + (SELECT count(*) FROM pg_seclabel AS labels
              WHERE (labels.classoid = 'pg_namespace'::regclass
                       AND labels.objoid = namespaces.oid)
                 OR (labels.classoid = 'pg_class'::regclass
                       AND labels.objoid IN (
                         SELECT relations.oid FROM pg_class AS relations
                         WHERE relations.relnamespace = namespaces.oid
                       ))
                 OR (labels.classoid = 'pg_constraint'::regclass
                       AND labels.objoid IN (
                         SELECT constraints.oid
                         FROM pg_constraint AS constraints
                         JOIN pg_class AS relations ON relations.oid = constraints.conrelid
                         WHERE relations.relnamespace = namespaces.oid
                       ))
                 OR (labels.classoid = 'pg_type'::regclass
                       AND labels.objoid IN (
                         SELECT types.oid FROM pg_type AS types
                         WHERE types.typnamespace = namespaces.oid
                       )))
         FROM pg_namespace AS namespaces
         WHERE namespaces.nspname = $1",
    )
    .bind(schema)
    .fetch_optional(connection)
    .await
    .map_err(|source| database("inspect schema-owned extra objects", source))?
    .ok_or_else(|| refuse(LegacyAdoptionRefusal::MissingOwnedSchema))
}

fn normalize_sql(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    let mut in_quoted_literal = false;

    while let Some(character) = characters.next() {
        if in_quoted_literal {
            normalized.push(character);
            if character == '\'' {
                if characters.peek() == Some(&'\'') {
                    normalized.push(characters.next().expect("peeked escaped quote"));
                } else {
                    in_quoted_literal = false;
                }
            }
        } else if character == '\'' {
            in_quoted_literal = true;
            normalized.push(character);
        } else if !character.is_ascii_whitespace() && !matches!(character, '(' | ')' | '"') {
            normalized.push(character);
        }
    }

    normalized
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn migration_checksum_matches_the_kit_algorithm_fixture() {
        let mut digest = Sha256::new();
        digest.update(AUDIT_LOG_MIGRATION_VERSION.to_be_bytes());
        digest.update([0]);
        digest.update(AUDIT_LOG_MIGRATION_NAME.as_bytes());
        digest.update([0]);
        digest.update(AUDIT_LOG_MIGRATION_SQL.as_bytes());
        assert_eq!(
            current_migration_checksum(),
            format!("{:x}", digest.finalize())
        );
    }

    #[test]
    fn migration_bytes_remain_stable_for_legacy_adoption() {
        assert_eq!(
            format!("{:x}", Sha256::digest(AUDIT_LOG_MIGRATION_SQL.as_bytes())),
            "f6f6b84e1d184eaaac746e68c9cfa65ff864d5439c8e50668a381a23cb984199"
        );
    }

    #[test]
    fn catalog_expression_normalization_preserves_meaningful_tokens() {
        assert_eq!(
            normalize_sql("CHECK ((outcome = ANY (ARRAY['success'::text])))"),
            "CHECKoutcome=ANYARRAY['success'::text]"
        );
        assert_ne!(
            normalize_sql("CHECK (outcome = 'success')"),
            normalize_sql("CHECK (outcome = 'SUCCESS')")
        );
        assert_ne!(
            normalize_sql("CHECK (outcome = 'success')"),
            normalize_sql("CHECK (outcome = 'suc cess')")
        );
        assert_eq!(
            normalize_sql("CHECK (message = 'can''t fail')"),
            "CHECKmessage='can''t fail'"
        );
    }

    #[test]
    fn security_label_metadata_breaks_the_exact_table_fingerprint() {
        let expected = TableSignature::ordinary();
        let mut labeled = TableSignature::ordinary();
        labeled.security_label_count = 1;
        assert_ne!(labeled, expected);
    }
}
