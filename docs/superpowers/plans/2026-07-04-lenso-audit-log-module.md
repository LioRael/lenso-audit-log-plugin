# Lenso Audit Log Module Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the independent first-party `lenso-module-audit-log` crate and prove it through an optional `lenso-organization-module` integration.

**Architecture:** The audit module owns generic append-only audit events and exposes linked Rust writer/query APIs plus a schema-admin surface. `lenso-organization-module` depends on it only behind an optional feature and passes generic scope/resource data into the writer; `audit-log` never imports organization internals. Remote-module audit ingestion remains design-only in this implementation plan.

**Tech Stack:** Rust 2024, SQLx Postgres, Lenso `platform-core`, `platform-module`, schema-admin contracts, `platform-testing`, Tokio tests.

---

## File Structure

### New Repository Files

- Create: `Cargo.toml`
  - Workspace root for `crates/audit-log`.
- Create: `crates/audit-log/Cargo.toml`
  - Publishable crate metadata and dependencies.
- Create: `crates/audit-log/migrations/0001_create_audit_log_schema.sql`
  - Creates `audit_log.events` and indexes.
- Create: `crates/audit-log/src/lib.rs`
  - Public module exports.
- Create: `crates/audit-log/src/migrations.rs`
  - Migration slice consumed by hosts/tests.
- Create: `crates/audit-log/src/models.rs`
  - Strong audit event input/output/filter types.
- Create: `crates/audit-log/src/repositories.rs`
  - Postgres writer/query implementation.
- Create: `crates/audit-log/src/public.rs`
  - Ergonomic linked-module API.
- Create: `crates/audit-log/src/admin.rs`
  - Schema-admin data source over audit events.
- Create: `crates/audit-log/src/module.rs`
  - Manifest, capabilities, linked module constructor.
- Create: `crates/audit-log/tests/audit_log.rs`
  - Core integration coverage.

### Sibling Repository Files

- Modify: `/Users/leosouthey/Projects/framework/lenso-organization-module/Cargo.toml`
  - Add workspace dependency for `audit-log`.
- Modify: `/Users/leosouthey/Projects/framework/lenso-organization-module/crates/organization/Cargo.toml`
  - Add optional `audit-log` dependency and feature.
- Create: `/Users/leosouthey/Projects/framework/lenso-organization-module/crates/organization/src/audit.rs`
  - Optional organization-to-audit adapter helpers.
- Modify: `/Users/leosouthey/Projects/framework/lenso-organization-module/crates/organization/src/lib.rs`
  - Export `audit` only when the feature is enabled.
- Modify: `/Users/leosouthey/Projects/framework/lenso-organization-module/crates/organization/src/repositories.rs`
  - Add feature-gated audited methods that write business changes and audit rows in one transaction.
- Modify: `/Users/leosouthey/Projects/framework/lenso-organization-module/crates/organization/src/routes.rs`
  - Use audited route helpers for create organization, create invitation, and accept invitation when the feature is enabled.
- Modify: `/Users/leosouthey/Projects/framework/lenso-organization-module/crates/organization/tests/organization.rs`
  - Add feature-gated audit integration test.

---

## Task 1: Rust Workspace Skeleton

**Files:**
- Create: `Cargo.toml`
- Create: `crates/audit-log/Cargo.toml`
- Create: `crates/audit-log/src/lib.rs`
- Modify: `README.md`

- [ ] **Step 1: Create the workspace root**

Replace `Cargo.toml` with:

```toml
[workspace]
resolver = "2"
members = ["crates/audit-log"]

[workspace.package]
edition = "2024"
license = "MIT"
rust-version = "1.94"

[workspace.dependencies]
async-trait = "0.1"
chrono = { version = "0.4", features = ["serde"] }
platform-core = { package = "lenso-platform-core", version = "0.1.4" }
platform-module = { package = "lenso-platform-module", version = "0.1.2" }
platform-testing = { package = "lenso-platform-testing", version = "0.1.1" }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
sqlx = { version = "0.9", default-features = false, features = ["runtime-tokio", "tls-rustls-ring-webpki", "postgres", "chrono", "json"] }
tokio = { version = "1.52", features = ["macros", "rt-multi-thread"] }
utoipa = { version = "5.5", features = ["chrono"] }

[workspace.lints.rust]
unsafe_code = "forbid"
missing_debug_implementations = "warn"

[workspace.lints.clippy]
all = "warn"
pedantic = "warn"
module_name_repetitions = "allow"
missing_errors_doc = "allow"
missing_panics_doc = "allow"
must_use_candidate = "allow"
```

- [ ] **Step 2: Create the crate manifest**

Create `crates/audit-log/Cargo.toml`:

```toml
[package]
name = "lenso-module-audit-log"
version = "0.1.0"
edition.workspace = true
license.workspace = true
description = "First-party audit log module for the Lenso backend framework."
repository = "https://github.com/LioRael/lenso-audit-log-plugin"
homepage = "https://github.com/LioRael/lenso-audit-log-plugin"
readme = "../../README.md"
categories = ["web-programming", "development-tools"]
keywords = ["backend", "framework", "audit", "log"]
rust-version.workspace = true

[lib]
name = "audit_log"
path = "src/lib.rs"

[dependencies]
async-trait.workspace = true
chrono.workspace = true
platform-core.workspace = true
platform-module.workspace = true
serde.workspace = true
serde_json.workspace = true
sqlx.workspace = true
utoipa.workspace = true

[dev-dependencies]
platform-testing.workspace = true
tokio.workspace = true

[lints]
workspace = true
```

- [ ] **Step 3: Create minimal crate exports**

Create `crates/audit-log/src/lib.rs`:

```rust
pub mod admin;
pub mod migrations;
pub mod models;
pub mod module;
pub mod public;
pub mod repositories;
```

- [ ] **Step 4: Update README development commands**

Add this section to `README.md` after the design link:

```markdown
## Development

```sh
cargo fmt --all --check
cargo test --locked -p lenso-module-audit-log
```
```

- [ ] **Step 5: Run the skeleton check**

Run:

```sh
cargo check -p lenso-module-audit-log
```

Expected: FAIL because `admin`, `migrations`, `models`, `module`, `public`, and `repositories` are declared but not created.

- [ ] **Step 6: Commit the skeleton**

```sh
git add Cargo.toml crates/audit-log/Cargo.toml crates/audit-log/src/lib.rs README.md
git commit -m "chore: scaffold audit log crate"
```

---

## Task 2: Core Types, Migration, And Repository

**Files:**
- Create: `crates/audit-log/migrations/0001_create_audit_log_schema.sql`
- Create: `crates/audit-log/src/migrations.rs`
- Create: `crates/audit-log/src/models.rs`
- Create: `crates/audit-log/src/repositories.rs`
- Create: `crates/audit-log/src/admin.rs`
- Create: `crates/audit-log/src/module.rs`
- Create: `crates/audit-log/src/public.rs`
- Create: `crates/audit-log/tests/audit_log.rs`

- [ ] **Step 1: Create temporary module files for the first failing test pass**

Create `crates/audit-log/src/admin.rs`:

```rust
#[derive(Debug, Default)]
pub struct AuditLogAdminData;
```

Create `crates/audit-log/src/module.rs`:

```rust
pub const MODULE_NAME: &str = "audit-log";
pub const AUDIT_EVENTS_READ: &str = "audit_log.events.read";
```

Create `crates/audit-log/src/public.rs`:

```rust
pub use crate::models::{
    AuditActor, AuditEvent, AuditEventFilter, AuditEventInput, AuditOutcome, AuditRequestContext,
    AuditResource, AuditScope, AuditSeverity,
};
pub use crate::repositories::PostgresAuditLogRepository;
```

- [ ] **Step 2: Write failing integration tests**

Create `crates/audit-log/tests/audit_log.rs`:

```rust
use audit_log::migrations::AUDIT_LOG_MIGRATIONS;
use audit_log::models::{
    AuditActor, AuditEventFilter, AuditEventInput, AuditOutcome, AuditRequestContext,
    AuditResource, AuditScope, AuditSeverity,
};
use audit_log::repositories::PostgresAuditLogRepository;
use chrono::Utc;
use platform_core::{PLATFORM_MIGRATIONS, apply_migrations};
use platform_testing::TestDatabase;
use serde_json::json;

async fn migrated_database() -> Option<TestDatabase> {
    let db = TestDatabase::create("audit_log").await?;
    apply_migrations(&db.pool, PLATFORM_MIGRATIONS)
        .await
        .expect("platform migrations");
    apply_migrations(&db.pool, AUDIT_LOG_MIGRATIONS)
        .await
        .expect("audit log migrations");
    Some(db)
}

fn sample_input() -> AuditEventInput {
    AuditEventInput {
        event_name: "organization.member_role_changed".to_owned(),
        module_name: "organization".to_owned(),
        action: "member_role_changed".to_owned(),
        outcome: AuditOutcome::Success,
        severity: AuditSeverity::Info,
        actor: AuditActor {
            kind: "user".to_owned(),
            id: Some("usr_owner".to_owned()),
            display: Some("Owner".to_owned()),
        },
        scope: Some(AuditScope {
            module: Some("organization".to_owned()),
            scope_type: "organization".to_owned(),
            id: "org_1".to_owned(),
            display: Some("Acme".to_owned()),
        }),
        resource: Some(AuditResource {
            resource_type: "organization_member".to_owned(),
            id: "member_1".to_owned(),
            display: Some("Avery".to_owned()),
        }),
        request: Some(AuditRequestContext {
            correlation_id: Some("corr_1".to_owned()),
            causation_id: Some("httpreq_1".to_owned()),
            request_id: Some("req_1".to_owned()),
            story_id: Some("corr_1".to_owned()),
        }),
        reason: Some("role changed from member to admin".to_owned()),
        metadata: json!({
            "old_role": "member",
            "new_role": "admin"
        }),
        occurred_at: Utc::now(),
    }
}

#[tokio::test]
async fn record_event_inserts_append_only_row() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let repository = PostgresAuditLogRepository::new(db.pool.clone());

    let event = repository
        .record_event(sample_input())
        .await
        .expect("audit event recorded");

    assert!(event.id.starts_with("audit_evt_"));
    assert_eq!(event.event_name, "organization.member_role_changed");
    assert_eq!(event.module_name, "organization");
    assert_eq!(event.outcome, AuditOutcome::Success);
    assert_eq!(event.scope_type.as_deref(), Some("organization"));
    assert_eq!(event.scope_id.as_deref(), Some("org_1"));
    assert_eq!(event.resource_type.as_deref(), Some("organization_member"));
    assert_eq!(event.resource_id.as_deref(), Some("member_1"));
    assert_eq!(event.correlation_id.as_deref(), Some("corr_1"));
    assert_eq!(event.metadata["old_role"], "member");

    db.cleanup().await;
}

#[tokio::test]
async fn list_events_filters_by_scope_resource_actor_and_outcome() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let repository = PostgresAuditLogRepository::new(db.pool.clone());
    repository
        .record_event(sample_input())
        .await
        .expect("audit event recorded");

    let rows = repository
        .list_events(AuditEventFilter {
            actor_id: Some("usr_owner".to_owned()),
            scope_type: Some("organization".to_owned()),
            scope_id: Some("org_1".to_owned()),
            resource_type: Some("organization_member".to_owned()),
            resource_id: Some("member_1".to_owned()),
            module_name: Some("organization".to_owned()),
            outcome: Some(AuditOutcome::Success),
            limit: 10,
            ..AuditEventFilter::default()
        })
        .await
        .expect("audit events listed");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].event_name, "organization.member_role_changed");

    let denied = repository
        .list_events(AuditEventFilter {
            outcome: Some(AuditOutcome::Denied),
            limit: 10,
            ..AuditEventFilter::default()
        })
        .await
        .expect("denied audit events listed");
    assert!(denied.is_empty());

    db.cleanup().await;
}

#[tokio::test]
async fn record_event_in_transaction_rolls_back_with_transaction() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let repository = PostgresAuditLogRepository::new(db.pool.clone());
    let mut tx = db.pool.begin().await.expect("begin transaction");

    repository
        .record_event_in_tx(&mut tx, sample_input())
        .await
        .expect("audit event in transaction");
    tx.rollback().await.expect("rollback transaction");

    let rows = repository
        .list_events(AuditEventFilter {
            limit: 10,
            ..AuditEventFilter::default()
        })
        .await
        .expect("audit events listed");
    assert!(rows.is_empty());

    db.cleanup().await;
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run:

```sh
cargo test --locked -p lenso-module-audit-log --test audit_log
```

Expected: FAIL with missing `AUDIT_LOG_MIGRATIONS`, model types, and repository methods.

- [ ] **Step 4: Add the migration**

Create `crates/audit-log/migrations/0001_create_audit_log_schema.sql`:

```sql
create schema if not exists audit_log;

create table if not exists audit_log.events (
    id text primary key,
    event_name text not null,
    module_name text not null,
    action text not null,
    outcome text not null,
    severity text not null,
    actor_kind text not null,
    actor_id text,
    actor_display text,
    scope_module text,
    scope_type text,
    scope_id text,
    scope_display text,
    resource_type text,
    resource_id text,
    resource_display text,
    correlation_id text,
    causation_id text,
    request_id text,
    story_id text,
    reason text,
    metadata jsonb not null default '{}'::jsonb,
    occurred_at timestamptz not null,
    created_at timestamptz not null default now()
);

create index if not exists audit_log_events_occurred_at_idx
    on audit_log.events (occurred_at desc, id desc);

create index if not exists audit_log_events_module_idx
    on audit_log.events (module_name, occurred_at desc, id desc);

create index if not exists audit_log_events_actor_idx
    on audit_log.events (actor_id, occurred_at desc, id desc)
    where actor_id is not null;

create index if not exists audit_log_events_scope_idx
    on audit_log.events (scope_type, scope_id, occurred_at desc, id desc)
    where scope_type is not null and scope_id is not null;

create index if not exists audit_log_events_resource_idx
    on audit_log.events (resource_type, resource_id, occurred_at desc, id desc)
    where resource_type is not null and resource_id is not null;

create index if not exists audit_log_events_correlation_idx
    on audit_log.events (correlation_id)
    where correlation_id is not null;
```

Create `crates/audit-log/src/migrations.rs`:

```rust
use platform_core::Migration;

pub const AUDIT_LOG_MIGRATIONS: &[Migration] = &[Migration {
    name: "audit-log/0001_create_audit_log_schema",
    sql: include_str!("../migrations/0001_create_audit_log_schema.sql"),
}];
```

- [ ] **Step 5: Add model types**

Create `crates/audit-log/src/models.rs`:

```rust
use chrono::{DateTime, Utc};
use platform_core::{ActorContext, RequestContext};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Success,
    Failure,
    Denied,
}

impl AuditOutcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Denied => "denied",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditSeverity {
    Info,
    Warning,
    Critical,
}

impl AuditSeverity {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditActor {
    pub kind: String,
    pub id: Option<String>,
    pub display: Option<String>,
}

impl AuditActor {
    #[must_use]
    pub fn from_request(request: &RequestContext) -> Self {
        match &request.actor {
            ActorContext::Anonymous => Self {
                kind: "anonymous".to_owned(),
                id: None,
                display: None,
            },
            ActorContext::User { user_id, .. } => Self {
                kind: "user".to_owned(),
                id: Some(user_id.clone()),
                display: None,
            },
            ActorContext::Service { service_id, .. } => Self {
                kind: "service".to_owned(),
                id: Some(service_id.clone()),
                display: None,
            },
            ActorContext::System => Self {
                kind: "system".to_owned(),
                id: None,
                display: None,
            },
        }
    }

    #[must_use]
    pub fn system() -> Self {
        Self {
            kind: "system".to_owned(),
            id: None,
            display: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditScope {
    pub module: Option<String>,
    pub scope_type: String,
    pub id: String,
    pub display: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditResource {
    pub resource_type: String,
    pub id: String,
    pub display: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRequestContext {
    pub correlation_id: Option<String>,
    pub causation_id: Option<String>,
    pub request_id: Option<String>,
    pub story_id: Option<String>,
}

impl AuditRequestContext {
    #[must_use]
    pub fn from_request(request: &RequestContext) -> Self {
        Self {
            correlation_id: Some(request.correlation_id.0.clone()),
            causation_id: request.causation_id.clone(),
            request_id: Some(request.request_id.0.clone()),
            story_id: Some(request.correlation_id.0.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEventInput {
    pub event_name: String,
    pub module_name: String,
    pub action: String,
    pub outcome: AuditOutcome,
    pub severity: AuditSeverity,
    pub actor: AuditActor,
    pub scope: Option<AuditScope>,
    pub resource: Option<AuditResource>,
    pub request: Option<AuditRequestContext>,
    pub reason: Option<String>,
    pub metadata: Value,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: String,
    pub event_name: String,
    pub module_name: String,
    pub action: String,
    pub outcome: AuditOutcome,
    pub severity: AuditSeverity,
    pub actor_kind: String,
    pub actor_id: Option<String>,
    pub actor_display: Option<String>,
    pub scope_module: Option<String>,
    pub scope_type: Option<String>,
    pub scope_id: Option<String>,
    pub scope_display: Option<String>,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub resource_display: Option<String>,
    pub correlation_id: Option<String>,
    pub causation_id: Option<String>,
    pub request_id: Option<String>,
    pub story_id: Option<String>,
    pub reason: Option<String>,
    pub metadata: Value,
    pub occurred_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditEventFilter {
    pub event_name: Option<String>,
    pub module_name: Option<String>,
    pub outcome: Option<AuditOutcome>,
    pub severity: Option<AuditSeverity>,
    pub actor_kind: Option<String>,
    pub actor_id: Option<String>,
    pub scope_module: Option<String>,
    pub scope_type: Option<String>,
    pub scope_id: Option<String>,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub correlation_id: Option<String>,
    pub occurred_after: Option<DateTime<Utc>>,
    pub occurred_before: Option<DateTime<Utc>>,
    pub limit: i64,
}

#[must_use]
pub fn redact_metadata(value: Value) -> Value {
    match value {
        Value::Object(entries) => Value::Object(redact_object(entries)),
        Value::Array(items) => Value::Array(items.into_iter().map(redact_metadata).collect()),
        other => other,
    }
}

fn redact_object(entries: Map<String, Value>) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| {
            if is_secret_key(&key) {
                (key, Value::String("[redacted]".to_owned()))
            } else {
                (key, redact_metadata(value))
            }
        })
        .collect()
}

fn is_secret_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.contains("password")
        || lower.contains("token")
        || lower.contains("secret")
        || lower.contains("private_key")
}
```

- [ ] **Step 6: Add repository implementation**

Create `crates/audit-log/src/repositories.rs`:

```rust
use crate::models::{
    AuditEvent, AuditEventFilter, AuditEventInput, AuditOutcome, AuditSeverity, redact_metadata,
};
use platform_core::{AppError, AppResult, DbPool, DbTransaction, ErrorCode};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct PostgresAuditLogRepository {
    pool: DbPool,
}

impl PostgresAuditLogRepository {
    #[must_use]
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    pub async fn record_event(&self, input: AuditEventInput) -> AppResult<AuditEvent> {
        let id = new_id();
        insert_event(&self.pool, &id, input).await
    }

    pub async fn record_event_in_tx(
        &self,
        tx: &mut DbTransaction<'_>,
        input: AuditEventInput,
    ) -> AppResult<AuditEvent> {
        let id = new_id();
        insert_event_in_tx(tx, &id, input).await
    }

    pub async fn list_events(&self, filter: AuditEventFilter) -> AppResult<Vec<AuditEvent>> {
        let limit = normalized_limit(filter.limit);
        sqlx::query_as::<_, AuditEventRow>(
            r#"
            select id, event_name, module_name, action, outcome, severity,
                   actor_kind, actor_id, actor_display,
                   scope_module, scope_type, scope_id, scope_display,
                   resource_type, resource_id, resource_display,
                   correlation_id, causation_id, request_id, story_id,
                   reason, metadata, occurred_at, created_at
            from audit_log.events
            where ($1::text is null or event_name = $1)
              and ($2::text is null or module_name = $2)
              and ($3::text is null or outcome = $3)
              and ($4::text is null or severity = $4)
              and ($5::text is null or actor_kind = $5)
              and ($6::text is null or actor_id = $6)
              and ($7::text is null or scope_module = $7)
              and ($8::text is null or scope_type = $8)
              and ($9::text is null or scope_id = $9)
              and ($10::text is null or resource_type = $10)
              and ($11::text is null or resource_id = $11)
              and ($12::text is null or correlation_id = $12)
              and ($13::timestamptz is null or occurred_at >= $13)
              and ($14::timestamptz is null or occurred_at <= $14)
            order by occurred_at desc, id desc
            limit $15
            "#,
        )
        .bind(filter.event_name)
        .bind(filter.module_name)
        .bind(filter.outcome.map(AuditOutcome::as_str))
        .bind(filter.severity.map(AuditSeverity::as_str))
        .bind(filter.actor_kind)
        .bind(filter.actor_id)
        .bind(filter.scope_module)
        .bind(filter.scope_type)
        .bind(filter.scope_id)
        .bind(filter.resource_type)
        .bind(filter.resource_id)
        .bind(filter.correlation_id)
        .bind(filter.occurred_after)
        .bind(filter.occurred_before)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map(|rows| rows.into_iter().map(AuditEventRow::into_event).collect())
        .map_err(map_sql_error)
    }
}

async fn insert_event(
    pool: &DbPool,
    id: &str,
    input: AuditEventInput,
) -> AppResult<AuditEvent> {
    insert_query(id, input)
        .fetch_one(pool)
        .await
        .map(AuditEventRow::into_event)
        .map_err(map_sql_error)
}

async fn insert_event_in_tx(
    tx: &mut DbTransaction<'_>,
    id: &str,
    input: AuditEventInput,
) -> AppResult<AuditEvent> {
    insert_query(id, input)
        .fetch_one(&mut **tx)
        .await
        .map(AuditEventRow::into_event)
        .map_err(map_sql_error)
}

fn insert_query(
    id: &str,
    input: AuditEventInput,
) -> sqlx::query::QueryAs<'_, sqlx::Postgres, AuditEventRow, sqlx::postgres::PgArguments> {
    let metadata = redact_metadata(input.metadata);
    let scope = input.scope;
    let resource = input.resource;
    let request = input.request;
    sqlx::query_as::<_, AuditEventRow>(
        r#"
        insert into audit_log.events (
            id, event_name, module_name, action, outcome, severity,
            actor_kind, actor_id, actor_display,
            scope_module, scope_type, scope_id, scope_display,
            resource_type, resource_id, resource_display,
            correlation_id, causation_id, request_id, story_id,
            reason, metadata, occurred_at
        )
        values (
            $1, $2, $3, $4, $5, $6,
            $7, $8, $9,
            $10, $11, $12, $13,
            $14, $15, $16,
            $17, $18, $19, $20,
            $21, $22, $23
        )
        returning id, event_name, module_name, action, outcome, severity,
                  actor_kind, actor_id, actor_display,
                  scope_module, scope_type, scope_id, scope_display,
                  resource_type, resource_id, resource_display,
                  correlation_id, causation_id, request_id, story_id,
                  reason, metadata, occurred_at, created_at
        "#,
    )
    .bind(id.to_owned())
    .bind(input.event_name)
    .bind(input.module_name)
    .bind(input.action)
    .bind(input.outcome.as_str())
    .bind(input.severity.as_str())
    .bind(input.actor.kind)
    .bind(input.actor.id)
    .bind(input.actor.display)
    .bind(scope.as_ref().and_then(|value| value.module.clone()))
    .bind(scope.as_ref().map(|value| value.scope_type.clone()))
    .bind(scope.as_ref().map(|value| value.id.clone()))
    .bind(scope.as_ref().and_then(|value| value.display.clone()))
    .bind(resource.as_ref().map(|value| value.resource_type.clone()))
    .bind(resource.as_ref().map(|value| value.id.clone()))
    .bind(resource.as_ref().and_then(|value| value.display.clone()))
    .bind(request.as_ref().and_then(|value| value.correlation_id.clone()))
    .bind(request.as_ref().and_then(|value| value.causation_id.clone()))
    .bind(request.as_ref().and_then(|value| value.request_id.clone()))
    .bind(request.as_ref().and_then(|value| value.story_id.clone()))
    .bind(input.reason)
    .bind(metadata)
    .bind(input.occurred_at)
}

#[derive(Debug, sqlx::FromRow)]
struct AuditEventRow {
    id: String,
    event_name: String,
    module_name: String,
    action: String,
    outcome: String,
    severity: String,
    actor_kind: String,
    actor_id: Option<String>,
    actor_display: Option<String>,
    scope_module: Option<String>,
    scope_type: Option<String>,
    scope_id: Option<String>,
    scope_display: Option<String>,
    resource_type: Option<String>,
    resource_id: Option<String>,
    resource_display: Option<String>,
    correlation_id: Option<String>,
    causation_id: Option<String>,
    request_id: Option<String>,
    story_id: Option<String>,
    reason: Option<String>,
    metadata: Value,
    occurred_at: chrono::DateTime<chrono::Utc>,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl AuditEventRow {
    fn into_event(self) -> AuditEvent {
        AuditEvent {
            id: self.id,
            event_name: self.event_name,
            module_name: self.module_name,
            action: self.action,
            outcome: parse_outcome(&self.outcome),
            severity: parse_severity(&self.severity),
            actor_kind: self.actor_kind,
            actor_id: self.actor_id,
            actor_display: self.actor_display,
            scope_module: self.scope_module,
            scope_type: self.scope_type,
            scope_id: self.scope_id,
            scope_display: self.scope_display,
            resource_type: self.resource_type,
            resource_id: self.resource_id,
            resource_display: self.resource_display,
            correlation_id: self.correlation_id,
            causation_id: self.causation_id,
            request_id: self.request_id,
            story_id: self.story_id,
            reason: self.reason,
            metadata: self.metadata,
            occurred_at: self.occurred_at,
            created_at: self.created_at,
        }
    }
}

fn parse_outcome(value: &str) -> AuditOutcome {
    match value {
        "failure" => AuditOutcome::Failure,
        "denied" => AuditOutcome::Denied,
        _ => AuditOutcome::Success,
    }
}

fn parse_severity(value: &str) -> AuditSeverity {
    match value {
        "warning" => AuditSeverity::Warning,
        "critical" => AuditSeverity::Critical,
        _ => AuditSeverity::Info,
    }
}

fn normalized_limit(limit: i64) -> i64 {
    limit.clamp(1, 200)
}

fn new_id() -> String {
    format!("audit_evt_{}", uuid::Uuid::now_v7())
}

fn map_sql_error(source: sqlx::Error) -> AppError {
    AppError::new(ErrorCode::Internal, "audit log query failed").with_source(source)
}
```

- [ ] **Step 7: Add `uuid` dependency required by repository IDs**

Modify root `Cargo.toml`:

```toml
uuid = { version = "1.19", features = ["v7"] }
```

Add it under `[workspace.dependencies]`.

Modify `crates/audit-log/Cargo.toml`:

```toml
uuid.workspace = true
```

Add it under `[dependencies]`.

- [ ] **Step 8: Run core tests**

Run:

```sh
cargo fmt --all
cargo test --locked -p lenso-module-audit-log --test audit_log
```

Expected: PASS for all three tests. If SQLx reports `uuid` missing, verify Step 7 is applied to both manifests.

- [ ] **Step 9: Commit core repository**

```sh
git add Cargo.toml crates/audit-log
git commit -m "feat: add audit log event store"
```

---

## Task 3: Public API, Admin Surface, And Module Manifest

**Files:**
- Modify: `crates/audit-log/src/public.rs`
- Modify: `crates/audit-log/src/admin.rs`
- Modify: `crates/audit-log/src/module.rs`
- Modify: `crates/audit-log/tests/audit_log.rs`

- [ ] **Step 1: Add failing manifest and admin tests**

Append to `crates/audit-log/tests/audit_log.rs`:

```rust
use audit_log::admin::AuditLogAdminData;
use audit_log::module::{AUDIT_EVENTS_READ, MODULE_NAME};
use platform_module::{AdminDataSource, AdminListQuery};

#[tokio::test]
async fn module_manifest_declares_read_capability_and_schema_admin_surface() {
    let manifest = audit_log::module::manifest();

    assert_eq!(manifest.name, MODULE_NAME);
    assert_eq!(manifest.capabilities, vec![AUDIT_EVENTS_READ.to_owned()]);
    let admin = manifest.admin.expect("audit log admin surface");
    let schema = match admin {
        platform_module::AdminSurface::Schema(schema) => schema,
        other => panic!("expected schema admin surface, got {other:?}"),
    };
    assert_eq!(schema.entities.len(), 1);
    assert_eq!(schema.entities[0].name, "events");
    assert_eq!(schema.entities[0].read_capability, AUDIT_EVENTS_READ);
}

#[tokio::test]
async fn admin_data_lists_and_gets_audit_events() {
    let Some(db) = migrated_database().await else {
        return;
    };
    let repository = PostgresAuditLogRepository::new(db.pool.clone());
    let event = repository
        .record_event(sample_input())
        .await
        .expect("audit event recorded");

    let admin = AuditLogAdminData::new(repository);
    let page = admin
        .list("events", &AdminListQuery::new(10, None))
        .await
        .expect("events listed");
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.records[0]["id"], event.id);
    assert_eq!(page.records[0]["scope_type"], "organization");

    let fetched = admin
        .get("events", &event.id)
        .await
        .expect("event fetched")
        .expect("event exists");
    assert_eq!(fetched["event_name"], "organization.member_role_changed");

    db.cleanup().await;
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```sh
cargo test --locked -p lenso-module-audit-log --test audit_log
```

Expected: FAIL because `manifest`, admin schema, admin data list/get, and public helpers are not implemented.

- [ ] **Step 3: Implement public helper API**

Replace `crates/audit-log/src/public.rs` with:

```rust
use crate::models::{
    AuditActor, AuditEvent, AuditEventFilter, AuditEventInput, AuditOutcome, AuditRequestContext,
    AuditResource, AuditScope, AuditSeverity,
};
use crate::repositories::PostgresAuditLogRepository;
use chrono::Utc;
use platform_core::{AppContext, AppResult, DbTransaction, RequestContext};
use serde_json::Value;

pub use crate::models::redact_metadata;

pub async fn record_event(ctx: &AppContext, input: AuditEventInput) -> AppResult<AuditEvent> {
    PostgresAuditLogRepository::new(ctx.db.clone())
        .record_event(input)
        .await
}

pub async fn record_event_in_tx(
    repository: &PostgresAuditLogRepository,
    tx: &mut DbTransaction<'_>,
    input: AuditEventInput,
) -> AppResult<AuditEvent> {
    repository.record_event_in_tx(tx, input).await
}

#[must_use]
pub fn success_input(
    request_ctx: &RequestContext,
    module_name: &str,
    action: &str,
    scope: Option<AuditScope>,
    resource: Option<AuditResource>,
    reason: Option<String>,
    metadata: Value,
) -> AuditEventInput {
    AuditEventInput {
        event_name: format!("{module_name}.{action}"),
        module_name: module_name.to_owned(),
        action: action.to_owned(),
        outcome: AuditOutcome::Success,
        severity: AuditSeverity::Info,
        actor: AuditActor::from_request(request_ctx),
        scope,
        resource,
        request: Some(AuditRequestContext::from_request(request_ctx)),
        reason,
        metadata,
        occurred_at: Utc::now(),
    }
}

pub use crate::models::{
    AuditActor as Actor, AuditEventFilter as EventFilter, AuditEventInput as EventInput,
    AuditOutcome as Outcome, AuditRequestContext as Request, AuditResource as Resource,
    AuditScope as Scope, AuditSeverity as Severity,
};
```

- [ ] **Step 4: Implement admin data source**

Replace `crates/audit-log/src/admin.rs` with:

```rust
use crate::models::{AuditEvent, AuditEventFilter};
use crate::repositories::PostgresAuditLogRepository;
use platform_core::{AppError, AppResult, ErrorCode};
use platform_module::{AdminDataSource, AdminListQuery, AdminPage};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct AuditLogAdminData {
    repository: PostgresAuditLogRepository,
}

impl AuditLogAdminData {
    #[must_use]
    pub fn new(repository: PostgresAuditLogRepository) -> Self {
        Self { repository }
    }
}

#[async_trait::async_trait]
impl AdminDataSource for AuditLogAdminData {
    async fn list(&self, entity: &str, query: &AdminListQuery) -> AppResult<AdminPage> {
        ensure_events_entity(entity)?;
        let rows = self
            .repository
            .list_events(AuditEventFilter {
                limit: query.limit.saturating_add(1),
                ..AuditEventFilter::default()
            })
            .await?;
        let has_more = rows.len() as i64 > query.limit.max(0);
        let take = rows.len().min(query.limit.max(0) as usize);
        let records = rows
            .into_iter()
            .take(take)
            .map(event_to_value)
            .collect::<Vec<_>>();
        let next_cursor = if has_more {
            records
                .last()
                .and_then(|record| record.get("id"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        } else {
            None
        };
        Ok(AdminPage {
            records,
            next_cursor,
        })
    }

    async fn get(&self, entity: &str, id: &str) -> AppResult<Option<Value>> {
        ensure_events_entity(entity)?;
        let rows = self
            .repository
            .list_events(AuditEventFilter {
                limit: 1,
                ..AuditEventFilter::default()
            })
            .await?;
        Ok(rows
            .into_iter()
            .find(|event| event.id == id)
            .map(event_to_value))
    }
}

fn ensure_events_entity(entity: &str) -> AppResult<()> {
    if entity == "events" {
        return Ok(());
    }
    Err(AppError::new(
        ErrorCode::NotFound,
        format!("unknown audit log entity: {entity}"),
    ))
}

fn event_to_value(event: AuditEvent) -> Value {
    json!({
        "id": event.id,
        "event_name": event.event_name,
        "module_name": event.module_name,
        "action": event.action,
        "outcome": event.outcome,
        "severity": event.severity,
        "actor_kind": event.actor_kind,
        "actor_id": event.actor_id,
        "actor_display": event.actor_display,
        "scope_module": event.scope_module,
        "scope_type": event.scope_type,
        "scope_id": event.scope_id,
        "scope_display": event.scope_display,
        "resource_type": event.resource_type,
        "resource_id": event.resource_id,
        "resource_display": event.resource_display,
        "correlation_id": event.correlation_id,
        "causation_id": event.causation_id,
        "request_id": event.request_id,
        "story_id": event.story_id,
        "reason": event.reason,
        "metadata": event.metadata,
        "occurred_at": event.occurred_at,
        "created_at": event.created_at,
    })
}
```

- [ ] **Step 5: Fix admin `get` to query by id directly**

Add this method to `PostgresAuditLogRepository` in `crates/audit-log/src/repositories.rs`:

```rust
pub async fn get_event(&self, id: &str) -> AppResult<Option<AuditEvent>> {
    sqlx::query_as::<_, AuditEventRow>(
        r#"
        select id, event_name, module_name, action, outcome, severity,
               actor_kind, actor_id, actor_display,
               scope_module, scope_type, scope_id, scope_display,
               resource_type, resource_id, resource_display,
               correlation_id, causation_id, request_id, story_id,
               reason, metadata, occurred_at, created_at
        from audit_log.events
        where id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&self.pool)
    .await
    .map(|row| row.map(AuditEventRow::into_event))
    .map_err(map_sql_error)
}
```

Then replace the `get` body in `crates/audit-log/src/admin.rs` with:

```rust
ensure_events_entity(entity)?;
self.repository.get_event(id).await.map(|row| row.map(event_to_value))
```

- [ ] **Step 6: Implement module manifest and linked module**

Replace `crates/audit-log/src/module.rs` with:

```rust
use crate::admin::AuditLogAdminData;
use crate::migrations::AUDIT_LOG_MIGRATIONS;
use crate::repositories::PostgresAuditLogRepository;
use platform_core::AppContext;
use platform_module::{
    AdminSchema, EntitySchema, FieldSchema, FieldType, HostLinkedModule, LinkedBinding, Module,
    ModuleManifest,
};
use std::sync::Arc;

pub const MODULE_NAME: &str = "audit-log";
pub const AUDIT_EVENTS_READ: &str = "audit_log.events.read";

#[must_use]
pub fn capabilities() -> Vec<String> {
    vec![AUDIT_EVENTS_READ.to_owned()]
}

#[must_use]
pub fn event_schema() -> AdminSchema {
    AdminSchema {
        entities: vec![EntitySchema {
            name: "events".to_owned(),
            label: "Audit Events".to_owned(),
            read_capability: AUDIT_EVENTS_READ.to_owned(),
            fields: vec![
                field("id", "ID", FieldType::String, false),
                field("event_name", "Event", FieldType::String, false),
                field("module_name", "Module", FieldType::String, false),
                field("action", "Action", FieldType::String, false),
                field("outcome", "Outcome", FieldType::String, false),
                field("severity", "Severity", FieldType::String, false),
                field("actor_kind", "Actor Kind", FieldType::String, false),
                field("actor_id", "Actor", FieldType::String, true),
                field("scope_type", "Scope Type", FieldType::String, true),
                field("scope_id", "Scope", FieldType::String, true),
                field("resource_type", "Resource Type", FieldType::String, true),
                field("resource_id", "Resource", FieldType::String, true),
                field("correlation_id", "Correlation", FieldType::String, true),
                field("reason", "Reason", FieldType::String, true),
                field("metadata", "Metadata", FieldType::Json, false),
                field("occurred_at", "Occurred", FieldType::Timestamp, false),
                field("created_at", "Created", FieldType::Timestamp, false),
            ],
        }],
    }
}

#[must_use]
pub fn manifest() -> ModuleManifest {
    ModuleManifest::builder(MODULE_NAME)
        .capabilities(capabilities())
        .admin(event_schema())
        .build()
}

#[must_use]
pub fn binding() -> LinkedBinding {
    LinkedBinding::builder().build()
}

#[must_use]
pub fn module(ctx: &AppContext) -> Module {
    let repository = PostgresAuditLogRepository::new(ctx.db.clone());
    Module::linked(manifest(), binding())
        .with_admin_data(Arc::new(AuditLogAdminData::new(repository)))
}

#[must_use]
pub fn linked_module() -> HostLinkedModule {
    HostLinkedModule::linked(MODULE_NAME, manifest, module, AUDIT_LOG_MIGRATIONS)
}

fn field(name: &str, label: &str, field_type: FieldType, nullable: bool) -> FieldSchema {
    FieldSchema {
        name: name.to_owned(),
        label: label.to_owned(),
        field_type,
        nullable,
    }
}
```

- [ ] **Step 7: Run tests and fix compile issues**

Run:

```sh
cargo fmt --all
cargo test --locked -p lenso-module-audit-log --test audit_log
```

Expected: PASS. The `linked_module` helper should match the sibling `organization` pattern and attach migrations through `HostLinkedModule::linked`.

- [ ] **Step 8: Commit public API and manifest**

```sh
git add crates/audit-log
git commit -m "feat: expose audit log module surface"
```

---

## Task 4: Metadata Redaction And Request Helpers

**Files:**
- Modify: `crates/audit-log/src/models.rs`
- Modify: `crates/audit-log/tests/audit_log.rs`

- [ ] **Step 1: Add redaction and request-context tests**

Append to `crates/audit-log/tests/audit_log.rs`:

```rust
use audit_log::models::{redact_metadata, AuditActor};
use platform_core::{ActorContext, CorrelationId, RequestContext, RequestId};

#[test]
fn metadata_redaction_replaces_secret_like_values() {
    let redacted = redact_metadata(json!({
        "token": "tok_live",
        "nested": {
            "password": "secret",
            "safe": "kept"
        },
        "items": [
            { "private_key": "pem" }
        ]
    }));

    assert_eq!(redacted["token"], "[redacted]");
    assert_eq!(redacted["nested"]["password"], "[redacted]");
    assert_eq!(redacted["nested"]["safe"], "kept");
    assert_eq!(redacted["items"][0]["private_key"], "[redacted]");
}

#[test]
fn actor_and_request_helpers_snapshot_lenso_request_context() {
    let mut request = RequestContext::new(
        RequestId::new("req_123"),
        CorrelationId::new("corr_123"),
    );
    request.actor = ActorContext::User {
        user_id: "usr_123".to_owned(),
        scopes: vec!["console.admin".to_owned()],
    };
    request.causation_id = Some("httpreq_123".to_owned());

    let actor = AuditActor::from_request(&request);
    let audit_request = AuditRequestContext::from_request(&request);

    assert_eq!(actor.kind, "user");
    assert_eq!(actor.id.as_deref(), Some("usr_123"));
    assert_eq!(audit_request.request_id.as_deref(), Some("req_123"));
    assert_eq!(audit_request.correlation_id.as_deref(), Some("corr_123"));
    assert_eq!(audit_request.story_id.as_deref(), Some("corr_123"));
    assert_eq!(audit_request.causation_id.as_deref(), Some("httpreq_123"));
}
```

- [ ] **Step 2: Run the targeted tests**

Run:

```sh
cargo test --locked -p lenso-module-audit-log --test audit_log metadata_redaction_replaces_secret_like_values actor_and_request_helpers_snapshot_lenso_request_context
```

Expected: PASS if Task 2 model helpers were implemented as specified.

- [ ] **Step 3: Commit helper coverage**

```sh
git add crates/audit-log/src/models.rs crates/audit-log/tests/audit_log.rs
git commit -m "test: cover audit helper redaction"
```

---

## Task 5: Organization Optional Audit Feature

**Files:**
- Modify: `/Users/leosouthey/Projects/framework/lenso-organization-module/Cargo.toml`
- Modify: `/Users/leosouthey/Projects/framework/lenso-organization-module/crates/organization/Cargo.toml`
- Create: `/Users/leosouthey/Projects/framework/lenso-organization-module/crates/organization/src/audit.rs`
- Modify: `/Users/leosouthey/Projects/framework/lenso-organization-module/crates/organization/src/lib.rs`
- Modify: `/Users/leosouthey/Projects/framework/lenso-organization-module/crates/organization/src/routes.rs`
- Modify: `/Users/leosouthey/Projects/framework/lenso-organization-module/crates/organization/tests/organization.rs`

- [ ] **Step 1: Add optional dependency wiring**

In `/Users/leosouthey/Projects/framework/lenso-organization-module/Cargo.toml`, add this under `[workspace.dependencies]`:

```toml
audit-log = { package = "lenso-module-audit-log", path = "../lenso-audit-log-module/crates/audit-log" }
```

In `/Users/leosouthey/Projects/framework/lenso-organization-module/crates/organization/Cargo.toml`, add this under `[features]`:

```toml
audit-log = ["dep:audit-log"]
default = []
```

Add this under `[dependencies]`:

```toml
audit-log = { workspace = true, optional = true }
```

- [ ] **Step 2: Create organization audit adapter**

Create `/Users/leosouthey/Projects/framework/lenso-organization-module/crates/organization/src/audit.rs`:

```rust
use crate::models::{CreatedInvitation, Membership, Organization};
use crate::module::MODULE_NAME;
use audit_log::models::{
    AuditActor, AuditEventInput, AuditOutcome, AuditRequestContext, AuditResource, AuditScope,
    AuditSeverity,
};
use chrono::{DateTime, Utc};
use platform_core::RequestContext;
use serde_json::{Value, json};

pub fn organization_created(
    request_ctx: &RequestContext,
    organization: &Organization,
    now: DateTime<Utc>,
) -> AuditEventInput {
    event(
        request_ctx,
        "organization.created",
        "created",
        organization_scope(organization),
        Some(AuditResource {
            resource_type: "organization".to_owned(),
            id: organization.id.clone(),
            display: Some(organization.name.clone()),
        }),
        Some("organization created".to_owned()),
        json!({ "slug": organization.slug }),
        now,
    )
}

pub fn invitation_created(
    request_ctx: &RequestContext,
    created: &CreatedInvitation,
    now: DateTime<Utc>,
) -> AuditEventInput {
    event(
        request_ctx,
        "organization.invitation_created",
        "invitation_created",
        Some(AuditScope {
            module: Some(MODULE_NAME.to_owned()),
            scope_type: "organization".to_owned(),
            id: created.invitation.organization_id.clone(),
            display: None,
        }),
        Some(AuditResource {
            resource_type: "organization_invitation".to_owned(),
            id: created.invitation.id.clone(),
            display: Some(created.invitation.email.clone()),
        }),
        Some("organization invitation created".to_owned()),
        json!({
            "email": created.invitation.email,
            "role_id": created.invitation.role_id,
            "expires_at": created.invitation.expires_at,
        }),
        now,
    )
}

pub fn invitation_accepted(
    request_ctx: &RequestContext,
    membership: &Membership,
    now: DateTime<Utc>,
) -> AuditEventInput {
    event(
        request_ctx,
        "organization.invitation_accepted",
        "invitation_accepted",
        Some(AuditScope {
            module: Some(MODULE_NAME.to_owned()),
            scope_type: "organization".to_owned(),
            id: membership.organization_id.clone(),
            display: None,
        }),
        Some(AuditResource {
            resource_type: "organization_member".to_owned(),
            id: membership.id.clone(),
            display: Some(membership.auth_user_id.0.clone()),
        }),
        Some("organization invitation accepted".to_owned()),
        json!({
            "auth_user_id": membership.auth_user_id.0,
            "role_id": membership.role_id,
            "role_name": membership.role_name,
        }),
        now,
    )
}

fn organization_scope(organization: &Organization) -> Option<AuditScope> {
    Some(AuditScope {
        module: Some(MODULE_NAME.to_owned()),
        scope_type: "organization".to_owned(),
        id: organization.id.clone(),
        display: Some(organization.name.clone()),
    })
}

fn event(
    request_ctx: &RequestContext,
    event_name: &str,
    action: &str,
    scope: Option<AuditScope>,
    resource: Option<AuditResource>,
    reason: Option<String>,
    metadata: Value,
    now: DateTime<Utc>,
) -> AuditEventInput {
    AuditEventInput {
        event_name: event_name.to_owned(),
        module_name: MODULE_NAME.to_owned(),
        action: action.to_owned(),
        outcome: AuditOutcome::Success,
        severity: AuditSeverity::Info,
        actor: AuditActor::from_request(request_ctx),
        scope,
        resource,
        request: Some(AuditRequestContext::from_request(request_ctx)),
        reason,
        metadata,
        occurred_at: now,
    }
}
```

- [ ] **Step 3: Export the feature-gated audit adapter**

In `/Users/leosouthey/Projects/framework/lenso-organization-module/crates/organization/src/lib.rs`, add:

```rust
#[cfg(feature = "audit-log")]
pub mod audit;
```

- [ ] **Step 4: Add feature-gated audited repository methods**

In `/Users/leosouthey/Projects/framework/lenso-organization-module/crates/organization/src/repositories.rs`, add these imports:

```rust
#[cfg(feature = "audit-log")]
use audit_log::repositories::PostgresAuditLogRepository;
#[cfg(feature = "audit-log")]
use platform_core::RequestContext;
```

Add these feature-gated methods inside `impl PostgresOrganizationRepository`:

```rust
#[cfg(feature = "audit-log")]
pub async fn create_organization_with_owner_audited(
    &self,
    request_ctx: &RequestContext,
    name: &str,
    slug: &str,
    owner_auth_user_id: &AuthUserId,
    now: DateTime<Utc>,
) -> AppResult<Organization> {
    let name = required_trimmed(name, "name")?;
    let slug = required_trimmed(slug, "slug")?;
    let organization_id = new_id("org");
    let owner_role_id = new_id("org_role");
    let admin_role_id = new_id("org_role");
    let member_role_id = new_id("org_role");
    let membership_id = new_id("org_member");
    let mut tx = self.pool.begin().await.map_err(map_sql_error)?;

    let organization = sqlx::query_as::<_, OrganizationRow>(
        r#"
        insert into organization.organizations (id, name, slug, created_at, updated_at, archived_at)
        values ($1, $2, $3, $4, $4, null)
        returning id, name, slug, created_at, updated_at, archived_at
        "#,
    )
    .bind(&organization_id)
    .bind(name)
    .bind(slug)
    .bind(now)
    .fetch_one(&mut *tx)
    .await
    .map(organization_from_row)
    .map_err(map_sql_error)?;

    insert_role(&mut tx, &owner_role_id, &organization_id, "owner", OWNER_PERMISSIONS, Some("owner"), now).await?;
    insert_role(&mut tx, &admin_role_id, &organization_id, "admin", ADMIN_PERMISSIONS, Some("admin"), now).await?;
    insert_role(&mut tx, &member_role_id, &organization_id, "member", MEMBER_PERMISSIONS, Some("member"), now).await?;

    sqlx::query(
        r#"
        insert into organization.memberships (id, organization_id, auth_user_id, role_id, created_at, updated_at, removed_at)
        values ($1, $2, $3, $4, $5, $5, null)
        "#,
    )
    .bind(&membership_id)
    .bind(&organization_id)
    .bind(&owner_auth_user_id.0)
    .bind(&owner_role_id)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(map_sql_error)?;

    PostgresAuditLogRepository::new(self.pool.clone())
        .record_event_in_tx(
            &mut tx,
            crate::audit::organization_created(request_ctx, &organization, now),
        )
        .await?;

    tx.commit().await.map_err(map_sql_error)?;
    Ok(organization)
}
```

Add the invitation methods as route-level proof events:

```rust
#[cfg(feature = "audit-log")]
pub async fn create_invitation_audited(
    &self,
    request_ctx: &RequestContext,
    organization_id: &str,
    email: &str,
    role_id: &str,
    expires_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> AppResult<CreatedInvitation> {
    let created = self
        .create_invitation(organization_id, email, role_id, expires_at, now)
        .await?;
    PostgresAuditLogRepository::new(self.pool.clone())
        .record_event(crate::audit::invitation_created(request_ctx, &created, now))
        .await?;
    Ok(created)
}

#[cfg(feature = "audit-log")]
pub async fn accept_invitation_audited(
    &self,
    request_ctx: &RequestContext,
    token: &str,
    auth_user_id: &AuthUserId,
    now: DateTime<Utc>,
) -> AppResult<Membership> {
    let membership = self.accept_invitation(token, auth_user_id, now).await?;
    PostgresAuditLogRepository::new(self.pool.clone())
        .record_event(crate::audit::invitation_accepted(request_ctx, &membership, now))
        .await?;
    Ok(membership)
}
```

The organization creation method is fully transactional because it already creates several rows inside one transaction. The invitation methods use the existing repository methods in this first proof slice; they are useful module-collaboration evidence but not hard-fail atomic audit sites.

- [ ] **Step 5: Wire routes to audited methods when the feature is enabled**

In `create_organization`, replace the existing organization creation block with:

```rust
#[cfg(feature = "audit-log")]
let organization = PostgresOrganizationRepository::new(ctx.db.clone())
    .create_organization_with_owner_audited(
        &request_ctx,
        &input.name,
        &input.slug,
        &AuthUserId(actor.user_id),
        ctx.clock.now(),
    )
    .await
    .map_err(|error| ApiErrorResponse::with_context(error, &request_ctx))?;

#[cfg(not(feature = "audit-log"))]
let organization = PostgresOrganizationRepository::new(ctx.db.clone())
    .create_organization_with_owner(
        &input.name,
        &input.slug,
        &AuthUserId(actor.user_id),
        ctx.clock.now(),
    )
    .await
    .map_err(|error| ApiErrorResponse::with_context(error, &request_ctx))?;
```

In `create_invitation`, replace the repository invitation creation call with:

```rust
#[cfg(feature = "audit-log")]
let created = repository
    .create_invitation_audited(
        &request_ctx,
        &organization_id,
        &input.email,
        &input.role_id,
        input.expires_at,
        ctx.clock.now(),
    )
    .await
    .map_err(|error| ApiErrorResponse::with_context(error, &request_ctx))?;

#[cfg(not(feature = "audit-log"))]
let created = repository
    .create_invitation(
        &organization_id,
        &input.email,
        &input.role_id,
        input.expires_at,
        ctx.clock.now(),
    )
    .await
    .map_err(|error| ApiErrorResponse::with_context(error, &request_ctx))?;
```

In `accept_invitation`, replace the existing repository call with:

```rust
let repository = PostgresOrganizationRepository::new(ctx.db.clone());

#[cfg(feature = "audit-log")]
let membership = repository
    .accept_invitation_audited(
        &request_ctx,
        &token,
        &AuthUserId(actor.user_id),
        ctx.clock.now(),
    )
    .await
    .map_err(|error| ApiErrorResponse::with_context(error, &request_ctx))?;

#[cfg(not(feature = "audit-log"))]
let membership = repository
    .accept_invitation(&token, &AuthUserId(actor.user_id), ctx.clock.now())
    .await
    .map_err(|error| ApiErrorResponse::with_context(error, &request_ctx))?;
```

- [ ] **Step 6: Add feature-gated route audit test**

Append to `/Users/leosouthey/Projects/framework/lenso-organization-module/crates/organization/tests/organization.rs`:

```rust
#[cfg(feature = "audit-log")]
#[tokio::test]
async fn http_routes_write_audit_events_when_audit_feature_is_enabled() {
    use audit_log::migrations::AUDIT_LOG_MIGRATIONS;
    use audit_log::models::AuditEventFilter;
    use audit_log::repositories::PostgresAuditLogRepository;

    let Some(db) = migrated_database().await else {
        return;
    };
    apply_migrations(&db.pool, AUDIT_LOG_MIGRATIONS)
        .await
        .expect("audit log migrations");
    seed_user(&db.pool, "usr_owner").await;
    seed_user(&db.pool, "usr_member").await;

    let app = test_app(&db);
    let created = request_json(
        app.clone(),
        "POST",
        "/v1/organizations",
        Some("Bearer dev-user:usr_owner"),
        Some(json!({ "name": "Audited Org", "slug": "audited-org" })),
    )
    .await;
    assert_eq!(created.0, StatusCode::OK);
    let organization_id = created.1["id"].as_str().expect("organization id");

    let repo = PostgresOrganizationRepository::new(db.pool.clone());
    let member_role = repo
        .member_role_for_organization(organization_id)
        .await
        .expect("member role");
    let invited = request_json(
        app.clone(),
        "POST",
        &format!("/v1/organizations/{organization_id}/invitations"),
        Some("Bearer dev-user:usr_owner:organization.invitations.manage"),
        Some(json!({
            "email": "audited-member@example.com",
            "role_id": member_role.id,
            "expires_at": (Utc::now() + Duration::days(1)).to_rfc3339(),
        })),
    )
    .await;
    assert_eq!(invited.0, StatusCode::OK);
    let token = invited.1["token"].as_str().expect("token").to_owned();

    let accepted = request_json(
        app,
        "POST",
        &format!("/v1/organization-invitations/{token}/accept"),
        Some("Bearer dev-user:usr_member"),
        None,
    )
    .await;
    assert_eq!(accepted.0, StatusCode::OK);

    let audit_rows = PostgresAuditLogRepository::new(db.pool.clone())
        .list_events(AuditEventFilter {
            scope_type: Some("organization".to_owned()),
            scope_id: Some(organization_id.to_owned()),
            module_name: Some("organization".to_owned()),
            limit: 10,
            ..AuditEventFilter::default()
        })
        .await
        .expect("audit events");
    let event_names = audit_rows
        .iter()
        .map(|event| event.event_name.as_str())
        .collect::<Vec<_>>();
    assert!(event_names.contains(&"organization.created"));
    assert!(event_names.contains(&"organization.invitation_created"));
    assert!(event_names.contains(&"organization.invitation_accepted"));
    assert!(audit_rows.iter().all(|event| event.correlation_id.is_some()));

    db.cleanup().await;
}
```

- [ ] **Step 7: Run organization checks without the feature**

Run:

```sh
cd /Users/leosouthey/Projects/framework/lenso-organization-module
cargo test --locked -p lenso-module-organization
```

Expected: PASS. This proves the optional dependency does not affect default builds.

- [ ] **Step 8: Run organization checks with the feature**

Run:

```sh
cd /Users/leosouthey/Projects/framework/lenso-organization-module
cargo test --locked -p lenso-module-organization --features audit-log http_routes_write_audit_events_when_audit_feature_is_enabled
```

Expected: PASS. This proves the route-level integration writes audit events.

- [ ] **Step 9: Commit organization integration**

```sh
cd /Users/leosouthey/Projects/framework/lenso-organization-module
git add Cargo.toml crates/organization/Cargo.toml crates/organization/src/audit.rs crates/organization/src/lib.rs crates/organization/src/repositories.rs crates/organization/src/routes.rs crates/organization/tests/organization.rs
git commit -m "feat: add optional audit log integration"
```

---

## Task 6: Final Verification And Documentation

**Files:**
- Modify: `README.md`
- Modify: `/Users/leosouthey/Projects/framework/lenso-organization-module/README.md`

- [ ] **Step 1: Document audit-log usage**

Add this section to `/Users/leosouthey/Projects/framework/lenso-audit-log-module/README.md`:

```markdown
## What It Provides

- Append-only audit event storage in `audit_log.events`.
- Generic actor, scope, resource, outcome, severity, reason, metadata, and Runtime Story correlation fields.
- Linked Rust writer APIs for first-party modules.
- Schema-admin read surface through `audit_log.events.read`.

## Install In A Lenso Host

```rust
use audit_log::module as audit_log_module;
use lenso::host::prelude::*;

pub fn host_composition() -> HostComposition {
    HostBuilder::new()
        .linked_module(audit_log_module::linked_module())
        .build()
}
```

`audit-log` is independent from `organization`. Organization events use generic `scope_type` and `scope_id` fields, so other modules can record workspace, project, account, tenant, or custom scoped activity without depending on organization tables.
```

- [ ] **Step 2: Document optional organization feature**

Add this section to `/Users/leosouthey/Projects/framework/lenso-organization-module/README.md`:

```markdown
## Optional Audit Log Integration

`organization` can write audit events when built with the `audit-log` feature and installed beside `lenso-module-audit-log`.

```toml
lenso-module-organization = { version = "0.1.0", features = ["audit-log"] }
lenso-module-audit-log = "0.1.0"
```

The integration records organization-created, invitation-created, and invitation-accepted events from the HTTP route path. Events use generic audit scopes:

```text
scope_module = organization
scope_type = organization
scope_id = <organization id>
```

The audit module does not depend on organization internals.
```

- [ ] **Step 3: Run audit-log repository verification**

Run:

```sh
cd /Users/leosouthey/Projects/framework/lenso-audit-log-module
cargo fmt --all --check
cargo test --locked -p lenso-module-audit-log
```

Expected: PASS.

- [ ] **Step 4: Run organization repository verification**

Run:

```sh
cd /Users/leosouthey/Projects/framework/lenso-organization-module
cargo fmt --all --check
cargo test --locked -p lenso-module-organization
cargo test --locked -p lenso-module-organization --features audit-log
```

Expected: PASS.

- [ ] **Step 5: Commit documentation**

```sh
cd /Users/leosouthey/Projects/framework/lenso-audit-log-module
git add README.md
git commit -m "docs: document audit log module usage"

cd /Users/leosouthey/Projects/framework/lenso-organization-module
git add README.md
git commit -m "docs: document audit log integration"
```

---

## Out Of Scope For This Plan

- Remote module `audit_events` envelopes.
- Dedicated `@lenso/audit-log-console`.
- Runtime config platform adapter.
- Admin action platform adapter.
- Module install/uninstall audit events.
- Hash-chain, retention, export, webhook, and SIEM features.

These remain in the design spec as follow-up work after the linked module and organization proof are working.
