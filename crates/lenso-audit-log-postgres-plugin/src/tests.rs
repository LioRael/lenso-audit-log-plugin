use std::{cell::RefCell, collections::BTreeMap, rc::Rc, time::Duration};

use lenso_app_plan::{
    AppComposition, CapabilityBinding, CapabilityEndpointPlan, CapabilityRequirementPlan,
    PluginInstancePlan, ResolvedAppPlan,
};
use lenso_capability_audit_log as audit;
use lenso_capability_audit_log::{
    APPEND_EVENT_OPERATION, AppendEventError, AppendEventRequest, AppendEventRequestActor,
    AppendEventRequestOutcome, AppendEventRequestSeverity, AuditLogClient, CAPABILITY_ID,
    DESCRIPTOR_VERSION, GET_EVENT_OPERATION, GetEventError, GetEventRequest, GetEventResponseEvent,
    LEGACY_METADATA_PORTABLE_JSON_KEY, LEGACY_METADATA_VALUE_KEY, LIST_EVENTS_OPERATION,
    ListEventsError, ListEventsRequest, recover_legacy_metadata,
};
use lenso_capability_secrets::{
    CAPABILITY_ID as SECRETS_CAPABILITY_ID, DESCRIPTOR_VERSION as SECRETS_DESCRIPTOR_VERSION,
    RESOLVE_OPERATION, ResolveError, ResolveRequest, ResolveResponse, Secrets, SecretsEndpoint,
    SecretsProvider,
};
use lenso_kernel::{
    ActivateContext, InvocationContext, Kernel, NativeRequestEndpoint, NativeRequestFuture,
    PluginFuture, PluginLifecycle, RuntimeFailure, ShutdownOutcome,
};
use lenso_native_adapter::{
    NativePluginFactory, NativePluginFactoryContext, NativePluginInstance, NativePluginRegistry,
};
use lenso_postgres_kit::{OwnedPostgres, PostgresKitError};
use lenso_runner::TokioDriver;
use serde_json::{Value, json};

use super::{
    AuditLogConfig, AuditLogOperator, AuditLogOperatorError, LegacyAdoptionOutcome,
    LegacyAdoptionRefusal, PACKAGE_ID, PLUGIN_DESCRIPTOR_JSON, PostgresAuditLogPlugin,
    PreparedAuditLog,
    model::{NewAuditEvent, ProjectionError, StoredEvent},
    operator::{DATABASE_MAINTENANCE_LOCK_SQL, prepare_managed},
    repository::{get_event, list_events, project_stored_metadata},
    schema::{AUDIT_LOG_MIGRATION_SQL, schema_plan},
    storage::{AuditStore, FixtureAuditStore},
};

const CONSUMER_PACKAGE_ID: &str = "test.audit-log-consumer";
const FIXTURE_PROVIDER_PACKAGE_ID: &str = "test.audit-log-provider";
const SECRETS_PACKAGE_ID: &str = "test.audit-log-secrets";
const EMPTY_PACKAGE_ID: &str = "test.without-audit-log";
const DATABASE_SECRET_REFERENCE: &str = "audit/database-url";
static POSTGRES_ACCEPTANCE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Debug)]
enum ConsumerAction {
    RoundTrip,
    ReadBoth,
    Append,
}

#[derive(Debug)]
struct AppendProof {
    id: String,
    metadata: BTreeMap<String, Value>,
    source_instance: String,
}

#[derive(Debug)]
struct ListProof {
    ids: Vec<String>,
}

#[derive(Debug)]
struct GetProof {
    id: String,
}

#[derive(Debug)]
enum Observed {
    RoundTrip {
        append: Result<AppendProof, audit::AuditLogAppendEventInvocationError>,
        list: Option<Result<ListProof, audit::AuditLogListEventsInvocationError>>,
        get: Option<Result<GetProof, audit::AuditLogGetEventInvocationError>>,
    },
    ReadBoth {
        list: Result<(), audit::AuditLogListEventsInvocationError>,
        get: Result<(), audit::AuditLogGetEventInvocationError>,
    },
    Append(Result<(), audit::AuditLogAppendEventInvocationError>),
}

#[derive(Clone, Debug)]
struct FixtureProviderFactory {
    store: AuditStore,
    writers: Vec<String>,
    readers: Vec<String>,
}

impl NativePluginFactory for FixtureProviderFactory {
    fn package_id(&self) -> &'static str {
        FIXTURE_PROVIDER_PACKAGE_ID
    }

    fn instantiate(
        &self,
        _context: NativePluginFactoryContext<'_>,
    ) -> Result<NativePluginInstance, RuntimeFailure> {
        let config = AuditLogConfig::new(
            DATABASE_SECRET_REFERENCE,
            self.writers.clone(),
            self.readers.clone(),
        )
        .map_err(|error| RuntimeFailure::Internal {
            detail: format!("invalid fixture configuration: {error}"),
        })?;
        let plugin = PostgresAuditLogPlugin {
            config,
            secrets: lenso::prelude::Port::default(),
            state: Rc::new(RefCell::new(Some(PreparedAuditLog {
                store: self.store.clone(),
            }))),
        };
        let endpoint =
            Rc::new(audit::AuditLogEndpoint::new(plugin)) as Rc<dyn NativeRequestEndpoint>;
        Ok(NativePluginInstance::new(vec![endpoint]))
    }
}

#[derive(Clone, Debug)]
struct ConsumerFactory {
    action: ConsumerAction,
    observed: Rc<RefCell<Option<Observed>>>,
}

impl NativePluginFactory for ConsumerFactory {
    fn package_id(&self) -> &'static str {
        CONSUMER_PACKAGE_ID
    }

    fn instantiate(
        &self,
        _context: NativePluginFactoryContext<'_>,
    ) -> Result<NativePluginInstance, RuntimeFailure> {
        Ok(NativePluginInstance::with_lifecycle(
            Vec::new(),
            ConsumerLifecycle {
                action: self.action.clone(),
                observed: self.observed.clone(),
            },
        ))
    }
}

#[derive(Clone, Debug)]
struct ConsumerLifecycle {
    action: ConsumerAction,
    observed: Rc<RefCell<Option<Observed>>>,
}

impl PluginLifecycle for ConsumerLifecycle {
    fn activate(&self, context: ActivateContext) -> PluginFuture {
        let client = AuditLogClient::from_dependencies(context.dependencies());
        let action = self.action.clone();
        let observed = self.observed.clone();
        Box::pin(async move {
            let client = client?;
            let outcome = match action {
                ConsumerAction::RoundTrip => {
                    let append =
                        client
                            .append_event(append_request())
                            .await
                            .map(|response| AppendProof {
                                id: response.event.id,
                                metadata: response.event.metadata,
                                source_instance: response.event.source_instance,
                            });
                    let (list, get) = match &append {
                        Ok(response) => {
                            let id = response.id.clone();
                            (
                                Some(client.list_events(list_request()).await.map(|response| {
                                    ListProof {
                                        ids: response
                                            .events
                                            .into_iter()
                                            .map(|event| event.id)
                                            .collect(),
                                    }
                                })),
                                Some(client.get_event(GetEventRequest { id }).await.map(
                                    |response| GetProof {
                                        id: response.event.id,
                                    },
                                )),
                            )
                        }
                        Err(_) => (None, None),
                    };
                    Observed::RoundTrip { append, list, get }
                }
                ConsumerAction::ReadBoth => Observed::ReadBoth {
                    list: client.list_events(list_request()).await.map(|_| ()),
                    get: client
                        .get_event(GetEventRequest {
                            id: "audit_evt_missing".to_owned(),
                        })
                        .await
                        .map(|_| ()),
                },
                ConsumerAction::Append => {
                    Observed::Append(client.append_event(append_request()).await.map(|_| ()))
                }
            };
            observed.replace(Some(outcome));
            Ok(())
        })
    }
}

#[derive(Clone)]
struct StaticSecretsFactory {
    values: BTreeMap<String, String>,
}

impl std::fmt::Debug for StaticSecretsFactory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StaticSecretsFactory")
            .field("references", &self.values.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl NativePluginFactory for StaticSecretsFactory {
    fn package_id(&self) -> &'static str {
        SECRETS_PACKAGE_ID
    }

    fn instantiate(
        &self,
        _context: NativePluginFactoryContext<'_>,
    ) -> Result<NativePluginInstance, RuntimeFailure> {
        let endpoint = Rc::new(SecretsEndpoint::new(StaticSecretsProvider {
            values: self.values.clone(),
        })) as Rc<dyn NativeRequestEndpoint>;
        Ok(NativePluginInstance::new(vec![endpoint]))
    }
}

#[derive(Clone)]
struct StaticSecretsProvider {
    values: BTreeMap<String, String>,
}

impl std::fmt::Debug for StaticSecretsProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StaticSecretsProvider")
            .field("references", &self.values.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl SecretsProvider for StaticSecretsProvider {
    fn resolve(
        &self,
        _context: InvocationContext,
        request: ResolveRequest,
    ) -> NativeRequestFuture<Secrets> {
        let result = self
            .values
            .get(&request.reference)
            .cloned()
            .map(|value| ResolveResponse { value })
            .ok_or(ResolveError::UnknownReference);
        Box::pin(async move { Ok(result) })
    }
}

#[derive(Debug)]
struct EmptyFactory;

impl NativePluginFactory for EmptyFactory {
    fn package_id(&self) -> &'static str {
        EMPTY_PACKAGE_ID
    }

    fn instantiate(
        &self,
        _context: NativePluginFactoryContext<'_>,
    ) -> Result<NativePluginInstance, RuntimeFailure> {
        Ok(NativePluginInstance::default())
    }
}

#[test]
fn generated_descriptor_uses_plugin_identity_and_explicit_contracts() {
    let descriptor: serde_json::Value =
        serde_json::from_str(PLUGIN_DESCRIPTOR_JSON).expect("descriptor should be valid JSON");
    assert_eq!(PACKAGE_ID, "lenso.audit-log.postgres");
    assert_eq!(descriptor["plugin_id"], PACKAGE_ID);
    assert_eq!(descriptor["root_slot"], "audit");
    assert_eq!(
        descriptor["provided_capabilities"][0]["capability_id"],
        CAPABILITY_ID
    );
    assert_eq!(
        descriptor["required_capabilities"][0]["capability_id"],
        SECRETS_CAPABILITY_ID
    );
}

#[tokio::test(flavor = "current_thread")]
async fn generated_client_round_trip_redacts_metadata_and_derives_source_identity() {
    let observed = run_fixture(
        vec!["consumer".to_owned()],
        vec!["consumer".to_owned()],
        ConsumerAction::RoundTrip,
        AuditStore::Fixture(FixtureAuditStore::default()),
    )
    .await;
    let Observed::RoundTrip {
        append: Ok(appended),
        list: Some(Ok(listed)),
        get: Some(Ok(got)),
    } = observed
    else {
        panic!("generated Client round trip failed: {observed:?}");
    };
    assert!(appended.id.starts_with("audit_evt_"));
    assert_eq!(appended.source_instance, "consumer");
    assert_eq!(listed.ids.len(), 1);
    assert_eq!(listed.ids[0], appended.id);
    assert_eq!(got.id, appended.id);
    assert_eq!(appended.metadata["api_token"], "[redacted]");
    assert_eq!(appended.metadata["nested"]["password"], "[redacted]");
    assert_eq!(appended.metadata["nested"]["safe"], "visible");
}

#[tokio::test(flavor = "current_thread")]
async fn writer_binding_does_not_grant_list_or_get_authority() {
    let observed = run_fixture(
        vec!["consumer".to_owned()],
        vec!["reader".to_owned()],
        ConsumerAction::ReadBoth,
        AuditStore::Fixture(FixtureAuditStore::default()),
    )
    .await;
    let Observed::ReadBoth { list, get } = observed else {
        panic!("unexpected consumer observation: {observed:?}");
    };
    assert!(matches!(
        list,
        Err(audit::AuditLogListEventsInvocationError::Domain(
            ListEventsError::Unauthorized
        ))
    ));
    assert!(matches!(
        get,
        Err(audit::AuditLogGetEventInvocationError::Domain(
            GetEventError::Unauthorized
        ))
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn reader_binding_does_not_grant_append_authority() {
    let observed = run_fixture(
        vec!["writer".to_owned()],
        vec!["consumer".to_owned()],
        ConsumerAction::Append,
        AuditStore::Fixture(FixtureAuditStore::default()),
    )
    .await;
    assert!(matches!(
        observed,
        Observed::Append(Err(audit::AuditLogAppendEventInvocationError::Domain(
            AppendEventError::Unauthorized
        )))
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn storage_failure_remains_a_runtime_failure() {
    let observed = run_fixture(
        vec!["consumer".to_owned()],
        vec!["reader".to_owned()],
        ConsumerAction::Append,
        AuditStore::Fixture(FixtureAuditStore::failing()),
    )
    .await;
    assert!(matches!(
        observed,
        Observed::Append(Err(
            audit::AuditLogAppendEventInvocationError::Runtime(
                RuntimeFailure::PluginFailure { detail }
            )
        )) if detail.contains("fixture Audit storage is unavailable")
    ));
}

#[test]
fn metadata_validation_is_bounded_portable_and_debug_redacted() {
    let request = append_request();
    let debug = format!("{request:?}");
    assert!(!debug.contains("api-token-value"));
    assert!(debug.contains("<redacted>"));

    let mut too_large = append_request();
    too_large
        .metadata
        .insert("blob".to_owned(), json!("x".repeat(65_536)));
    assert_eq!(
        NewAuditEvent::from_request(too_large, "consumer").unwrap_err(),
        AppendEventError::InvalidEvent
    );

    let mut non_portable = append_request();
    non_portable.metadata.insert(
        "unsafe_integer".to_owned(),
        json!(9_007_199_254_740_992_u64),
    );
    assert_eq!(
        NewAuditEvent::from_request(non_portable, "consumer").unwrap_err(),
        AppendEventError::InvalidEvent
    );

    let mut too_many_properties = append_request();
    too_many_properties.metadata = (0..1_025)
        .map(|index| (format!("key-{index}"), Value::Null))
        .collect();
    assert_eq!(
        NewAuditEvent::from_request(too_many_properties, "consumer").unwrap_err(),
        AppendEventError::InvalidEvent
    );

    let mut too_deep = Value::Null;
    for _ in 0..33 {
        too_deep = Value::Array(vec![too_deep]);
    }
    let mut recursive_overflow = append_request();
    recursive_overflow
        .metadata
        .insert("nested".to_owned(), too_deep);
    assert_eq!(
        NewAuditEvent::from_request(recursive_overflow, "consumer").unwrap_err(),
        AppendEventError::InvalidEvent
    );

    let mut exact_encoded_boundary = append_request();
    exact_encoded_boundary.metadata = BTreeMap::from([(
        "blob".to_owned(),
        json!("x".repeat(super::model::MAX_METADATA_BYTES - 11)),
    )]);
    assert!(NewAuditEvent::from_request(exact_encoded_boundary, "consumer").is_ok());

    let mut over_encoded_boundary = append_request();
    over_encoded_boundary.metadata = BTreeMap::from([(
        "blob".to_owned(),
        json!("x".repeat(super::model::MAX_METADATA_BYTES - 10)),
    )]);
    assert_eq!(
        NewAuditEvent::from_request(over_encoded_boundary, "consumer").unwrap_err(),
        AppendEventError::InvalidEvent
    );
}

#[test]
fn stored_event_projection_revalidates_every_legacy_wire_boundary() {
    let event = NewAuditEvent::from_request(append_request(), "consumer").unwrap();
    let mut stored = StoredEvent::fixture(event, chrono::Utc::now());
    stored.event_name = "x".repeat(257);
    assert!(matches!(
        stored.project::<GetEventResponseEvent>(),
        Err(ProjectionError::InvalidStoredValue {
            field: "event_name"
        })
    ));

    stored.event_name = "event".to_owned();
    stored.metadata = BTreeMap::from([("payload".to_owned(), json!("x".repeat(65_536)))]);
    assert!(matches!(
        stored.project::<GetEventResponseEvent>(),
        Err(ProjectionError::InvalidStoredValue { field: "metadata" })
    ));
}

#[test]
fn generated_append_request_validation_matches_unicode_and_nullable_string_bounds() {
    let mut unicode_at_limit = append_request();
    unicode_at_limit.event_name = "审".repeat(256);
    assert!(NewAuditEvent::from_request(unicode_at_limit, "consumer").is_ok());

    let mut unicode_over_limit = append_request();
    unicode_over_limit.event_name = "审".repeat(257);
    assert_eq!(
        NewAuditEvent::from_request(unicode_over_limit, "consumer").unwrap_err(),
        AppendEventError::InvalidEvent
    );

    let mut empty_nullable_string = append_request();
    empty_nullable_string.actor.id = Some(String::new());
    assert_eq!(
        NewAuditEvent::from_request(empty_nullable_string, "consumer").unwrap_err(),
        AppendEventError::InvalidEvent
    );
}

#[test]
fn legacy_metadata_projection_is_lossless_for_every_json_shape() {
    let object = json!({"visible": true});
    assert_eq!(
        project_stored_metadata(object.clone()),
        serde_json::from_value(object).unwrap()
    );
    for legacy_value in [json!(["one", 2]), json!("scalar"), json!(42), Value::Null] {
        let projected = project_stored_metadata(legacy_value.clone());
        assert_eq!(
            projected,
            BTreeMap::from([(LEGACY_METADATA_VALUE_KEY.to_owned(), legacy_value)])
        );
        assert!(
            lenso_contract_runtime::validate_portable_json_value(
                &serde_json::to_value(&projected).unwrap()
            )
            .is_ok()
        );
    }
}

#[test]
fn legacy_metadata_projection_preserves_unsafe_numbers_behind_a_portable_envelope() {
    for original in [
        json!({"positive": {"nested": [9_007_199_254_740_992_u64]}}),
        json!([{"negative": -9_007_199_254_740_992_i64}]),
    ] {
        let projected = project_stored_metadata(original.clone());
        assert_eq!(projected.len(), 1);
        assert!(projected.contains_key(LEGACY_METADATA_PORTABLE_JSON_KEY));
        assert_eq!(recover_legacy_metadata(&projected).unwrap(), Some(original));
        assert!(
            lenso_contract_runtime::validate_portable_json_value(
                &serde_json::to_value(&projected).unwrap()
            )
            .is_ok(),
            "the projected response must cross a portable lane"
        );
    }

    for safe_boundary in [
        json!({"number": 9_007_199_254_740_991_u64}),
        json!({"number": -9_007_199_254_740_991_i64}),
    ] {
        assert_eq!(
            project_stored_metadata(safe_boundary.clone()),
            serde_json::from_value(safe_boundary).unwrap()
        );
    }
}

#[test]
fn reserved_legacy_metadata_keys_cannot_collide_with_an_original_object() {
    for key in [LEGACY_METADATA_VALUE_KEY, LEGACY_METADATA_PORTABLE_JSON_KEY] {
        let original = json!({key: {"original": true}});
        let projected = project_stored_metadata(original.clone());
        assert_eq!(recover_legacy_metadata(&projected).unwrap(), Some(original));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn linked_factory_rejects_invalid_configuration_before_startup() {
    let plan = actual_plan(
        r#"{"database_url_secret":"audit/database-url","writer_instances":[],"reader_instances":["consumer"]}"#,
        false,
    );
    let local = tokio::task::LocalSet::new();
    let error = local
        .run_until(async {
            Kernel::start_native(
                plan,
                TokioDriver::new(),
                NativePluginRegistry::new()
                    .with_linked_factories()
                    .with_factory(StaticSecretsFactory {
                        values: BTreeMap::from([(
                            DATABASE_SECRET_REFERENCE.to_owned(),
                            "postgresql://unused".to_owned(),
                        )]),
                    }),
            )
            .await
            .unwrap_err()
        })
        .await;
    assert!(matches!(error, RuntimeFailure::InvalidResolvedPlan { .. }));
}

#[tokio::test(flavor = "current_thread")]
async fn removing_audit_plugin_selection_removes_behavior_without_kernel_changes() {
    let plan = AppComposition::new(
        vec![PluginInstancePlan::new("empty", EMPTY_PACKAGE_ID)],
        Vec::new(),
    )
    .resolve()
    .unwrap();
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let app = Kernel::start_native(
                plan,
                TokioDriver::new(),
                NativePluginRegistry::new()
                    .with_linked_factories()
                    .with_factory(EmptyFactory),
            )
            .await
            .expect("unselected Audit Log Plugin must be inert");
            assert_eq!(
                app.shutdown(Duration::from_secs(1)).await,
                ShutdownOutcome::Clean
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
#[cfg_attr(
    not(feature = "postgres-acceptance"),
    ignore = "requires LENSO_POSTGRES_TEST_URL and exclusive ownership of audit_log schema"
)]
async fn prepare_verification_then_operator_setup_and_linked_postgres_round_trip() {
    let _exclusive_database = POSTGRES_ACCEPTANCE_LOCK.lock().await;
    let database_url = postgres_test_url();
    let pool = sqlx::PgPool::connect(&database_url).await.unwrap();
    drop_audit_schema(&pool).await;

    let missing_config = AuditLogConfig::new(
        DATABASE_SECRET_REFERENCE,
        vec!["writer".to_owned()],
        vec!["reader".to_owned()],
    )
    .unwrap();
    let missing_plan = actual_plan(&serde_json::to_string(&missing_config).unwrap(), false);
    let local = tokio::task::LocalSet::new();
    let error = local
        .run_until(async {
            Kernel::start_native(
                missing_plan,
                TokioDriver::new(),
                NativePluginRegistry::new()
                    .with_linked_factories()
                    .with_factory(StaticSecretsFactory {
                        values: BTreeMap::from([(
                            DATABASE_SECRET_REFERENCE.to_owned(),
                            database_url.clone(),
                        )]),
                    }),
            )
            .await
            .unwrap_err()
        })
        .await;
    assert!(matches!(error, RuntimeFailure::PluginFailure { .. }));
    let exists: bool = sqlx::query_scalar(
        "select exists (select 1 from pg_namespace where nspname = 'audit_log')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!exists, "Plugin prepare must not run setup migrations");

    super::AuditLogOperator::setup(&database_url).await.unwrap();
    verify_managed_prepare_rejects_implicit_publication_exposure(&pool, &database_url).await;
    verify_unbounded_legacy_rows_fail_before_wire_projection(&pool, &database_url).await;

    let observed = Rc::new(RefCell::new(None));
    let config = AuditLogConfig::new(
        DATABASE_SECRET_REFERENCE,
        vec!["consumer".to_owned()],
        vec!["consumer".to_owned()],
    )
    .unwrap();
    let plan = actual_plan(&serde_json::to_string(&config).unwrap(), true);
    let local = tokio::task::LocalSet::new();
    let outcome = local
        .run_until(async {
            let app = Kernel::start_native(
                plan,
                TokioDriver::new(),
                NativePluginRegistry::new()
                    .with_linked_factories()
                    .with_factory(StaticSecretsFactory {
                        values: BTreeMap::from([(
                            DATABASE_SECRET_REFERENCE.to_owned(),
                            database_url.clone(),
                        )]),
                    })
                    .with_factory(ConsumerFactory {
                        action: ConsumerAction::RoundTrip,
                        observed: observed.clone(),
                    }),
            )
            .await
            .expect("operator-managed schema should prepare");
            let outcome = observed.borrow_mut().take().unwrap();
            assert_eq!(
                app.shutdown(Duration::from_secs(2)).await,
                ShutdownOutcome::Clean
            );
            outcome
        })
        .await;
    assert!(matches!(
        outcome,
        Observed::RoundTrip {
            append: Ok(_),
            list: Some(Ok(_)),
            get: Some(Ok(_)),
        }
    ));
    drop_audit_schema(&pool).await;
}

#[tokio::test(flavor = "current_thread")]
#[cfg_attr(
    not(feature = "postgres-acceptance"),
    ignore = "requires LENSO_POSTGRES_TEST_URL and exclusive ownership of audit_log and platform schemas"
)]
async fn operator_adopts_only_an_exact_legacy_schema_and_preserves_all_rows() {
    let _exclusive_database = POSTGRES_ACCEPTANCE_LOCK.lock().await;
    let database_url = postgres_test_url();
    let pool = sqlx::PgPool::connect(&database_url).await.unwrap();

    verify_successful_legacy_adoption(&pool, &database_url).await;
    expect_adoption_refused(&database_url, LegacyAdoptionRefusal::AlreadyManaged).await;

    for (tamper, refusal) in [
        (
            "update platform.schema_migrations set name = 'audit-log/tampered'",
            LegacyAdoptionRefusal::LegacyLedgerDiverged,
        ),
        (
            "drop index audit_log.audit_log_events_actor_idx",
            LegacyAdoptionRefusal::SchemaShapeDiverged,
        ),
        (
            "alter table audit_log.events alter column metadata set default '[]'::jsonb",
            LegacyAdoptionRefusal::SchemaShapeDiverged,
        ),
        (
            "alter table audit_log.events drop constraint events_outcome_check;
             alter table audit_log.events add constraint events_outcome_check
               check (outcome in ('SUCCESS', 'failure', 'denied'))",
            LegacyAdoptionRefusal::SchemaShapeDiverged,
        ),
        (
            "alter table audit_log.events
             alter column event_name type text collate \"C\"",
            LegacyAdoptionRefusal::SchemaShapeDiverged,
        ),
        (
            "comment on table audit_log.events is 'unexpected'",
            LegacyAdoptionRefusal::SchemaShapeDiverged,
        ),
        (
            "cluster audit_log.events using audit_log_events_occurred_at_idx",
            LegacyAdoptionRefusal::SchemaShapeDiverged,
        ),
        (
            "create table audit_log.unexpected (id text primary key)",
            LegacyAdoptionRefusal::SchemaShapeDiverged,
        ),
        (
            "grant select on audit_log.events to public",
            LegacyAdoptionRefusal::UnexpectedPrivileges,
        ),
        (
            "grant usage on type audit_log.events to public",
            LegacyAdoptionRefusal::SchemaShapeDiverged,
        ),
        (
            "alter default privileges in schema audit_log
             grant select on tables to public",
            LegacyAdoptionRefusal::UnexpectedPrivileges,
        ),
        (
            "create text search configuration audit_log.unexpected
             (copy = pg_catalog.english)",
            LegacyAdoptionRefusal::SchemaShapeDiverged,
        ),
        (
            "create publication audit_log_test_publication
             for tables in schema audit_log",
            LegacyAdoptionRefusal::SchemaShapeDiverged,
        ),
        (
            "create publication audit_log_test_publication
             for tables in schema platform",
            LegacyAdoptionRefusal::LegacyLedgerDiverged,
        ),
        (
            "create publication audit_log_test_publication for all tables",
            LegacyAdoptionRefusal::SchemaShapeDiverged,
        ),
    ] {
        expect_legacy_tamper_refused(&pool, &database_url, tamper, refusal).await;
    }
    verify_maintenance_lock_serializes_participating_schema_ddl(&pool, &database_url).await;
    reset_legacy_test_state(&pool).await;
}

async fn verify_managed_prepare_rejects_implicit_publication_exposure(
    pool: &sqlx::PgPool,
    database_url: &str,
) {
    for publication in [
        "create publication audit_log_test_publication for tables in schema audit_log",
        "create publication audit_log_test_publication for all tables",
    ] {
        sqlx::raw_sql(publication).execute(pool).await.unwrap();
        assert!(matches!(
            prepare_managed(database_url).await,
            Err(AuditLogOperatorError::ManagedSchemaMismatch)
        ));
        sqlx::raw_sql("drop publication audit_log_test_publication")
            .execute(pool)
            .await
            .unwrap();
    }

    prepare_managed(database_url)
        .await
        .expect("publication-free managed schema should prepare")
        .pool()
        .close()
        .await;
}

async fn verify_unbounded_legacy_rows_fail_before_wire_projection(
    pool: &sqlx::PgPool,
    database_url: &str,
) {
    sqlx::raw_sql(
        "insert into audit_log.events (
           id, event_name, module_name, action, outcome, severity, actor_kind, metadata, occurred_at
         ) values
           ('legacy_oversized_text', repeat('x', 257), 'legacy', 'read', 'success', 'info',
            'system', '{}'::jsonb, now()),
           ('legacy_oversized_metadata', 'legacy.metadata', 'legacy', 'read', 'success', 'info',
            'system', jsonb_build_object('payload', repeat('x', 65536)), now()),
           ('legacy_oversized_timestamp', 'legacy.timestamp', 'legacy', 'read', 'success', 'info',
            'system', '{}'::jsonb, timestamptz '10000-01-01 00:00:00+00')",
    )
    .execute(pool)
    .await
    .unwrap();

    let postgres = prepare_managed(database_url).await.unwrap();
    for (id, field) in [
        ("legacy_oversized_text", "event_name"),
        ("legacy_oversized_metadata", "metadata"),
        ("legacy_oversized_timestamp", "occurred_at"),
    ] {
        let stored = get_event(&postgres, id).await.unwrap().unwrap();
        assert!(matches!(
            stored.project::<GetEventResponseEvent>(),
            Err(ProjectionError::InvalidStoredValue { field: actual }) if actual == field
        ));
    }
    postgres.pool().close().await;
    sqlx::query("delete from audit_log.events where id like 'legacy_oversized_%'")
        .execute(pool)
        .await
        .unwrap();
}

async fn verify_maintenance_lock_serializes_participating_schema_ddl(
    pool: &sqlx::PgPool,
    database_url: &str,
) {
    reset_legacy_test_state(pool).await;
    install_legacy_state(pool).await;

    let mut maintenance = pool.begin().await.unwrap();
    sqlx::query(DATABASE_MAINTENANCE_LOCK_SQL)
        .execute(&mut *maintenance)
        .await
        .unwrap();

    let mut adoption = Box::pin(AuditLogOperator::adopt_legacy(database_url));
    assert!(
        tokio::time::timeout(Duration::from_secs(2), adoption.as_mut())
            .await
            .is_err(),
        "adoption must wait for an existing database maintenance lock"
    );

    sqlx::query("create table audit_log.concurrent_extra (id text primary key)")
        .execute(&mut *maintenance)
        .await
        .unwrap();
    maintenance.commit().await.unwrap();

    let error = adoption.await.unwrap_err();
    assert!(matches!(
        error,
        AuditLogOperatorError::AdoptionRefused(LegacyAdoptionRefusal::SchemaShapeDiverged)
    ));
    assert!(!managed_ledger_exists(pool).await);
}

async fn verify_successful_legacy_adoption(pool: &sqlx::PgPool, database_url: &str) {
    reset_legacy_test_state(pool).await;
    install_legacy_state(pool).await;
    insert_legacy_metadata_rows(pool).await;
    let before = load_raw_metadata(pool).await;

    assert!(matches!(
        OwnedPostgres::prepare(database_url, schema_plan().unwrap())
            .await
            .unwrap_err(),
        PostgresKitError::UnmanagedSchema { .. }
    ));
    assert!(
        !managed_ledger_exists(pool).await,
        "prepare must not silently adopt a legacy schema"
    );

    assert_eq!(
        AuditLogOperator::adopt_legacy(database_url).await.unwrap(),
        LegacyAdoptionOutcome::Adopted { version: 1 }
    );
    assert_eq!(load_raw_metadata(pool).await, before);

    let postgres = prepare_managed(database_url).await.unwrap();
    let filter = super::model::EventFilter::from_request(list_request()).unwrap();
    let listed = list_events(&postgres, &filter).await.unwrap();
    assert_eq!(listed.len(), 4);
    let listed = listed
        .into_iter()
        .map(|event| (event.id, event.metadata))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        listed["legacy_object"],
        BTreeMap::from([("visible".to_owned(), json!(true))])
    );
    for (id, value) in [
        ("legacy_array", json!(["one", 2])),
        ("legacy_scalar", json!("scalar")),
        ("legacy_null", Value::Null),
    ] {
        let expected = BTreeMap::from([(LEGACY_METADATA_VALUE_KEY.to_owned(), value)]);
        assert_eq!(listed[id], expected);
        assert_eq!(
            get_event(&postgres, id).await.unwrap().unwrap().metadata,
            expected
        );
    }
    assert_eq!(
        get_event(&postgres, "legacy_object")
            .await
            .unwrap()
            .unwrap()
            .metadata,
        BTreeMap::from([("visible".to_owned(), json!(true))])
    );
    postgres.pool().close().await;
}

async fn expect_legacy_tamper_refused(
    pool: &sqlx::PgPool,
    database_url: &str,
    tamper: &'static str,
    refusal: LegacyAdoptionRefusal,
) {
    reset_legacy_test_state(pool).await;
    install_legacy_state(pool).await;
    sqlx::raw_sql(tamper).execute(pool).await.unwrap();
    expect_adoption_refused(database_url, refusal).await;
}

async fn run_fixture(
    writers: Vec<String>,
    readers: Vec<String>,
    action: ConsumerAction,
    store: AuditStore,
) -> Observed {
    let observed = Rc::new(RefCell::new(None));
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let app = Kernel::start_native(
                fixture_plan(),
                TokioDriver::new(),
                NativePluginRegistry::new()
                    .with_factory(FixtureProviderFactory {
                        store,
                        writers,
                        readers,
                    })
                    .with_factory(ConsumerFactory {
                        action,
                        observed: observed.clone(),
                    }),
            )
            .await
            .expect("fixture composition should start");
            let outcome = observed
                .borrow_mut()
                .take()
                .expect("consumer should record its generated Client result");
            assert_eq!(
                app.shutdown(Duration::from_secs(1)).await,
                ShutdownOutcome::Clean
            );
            outcome
        })
        .await
}

fn fixture_plan() -> ResolvedAppPlan {
    let consumer = PluginInstancePlan::new("consumer", CONSUMER_PACKAGE_ID).with_requirement(
        CapabilityRequirementPlan::one(CAPABILITY_ID, DESCRIPTOR_VERSION),
    );
    let provider = PluginInstancePlan::new("audit", FIXTURE_PROVIDER_PACKAGE_ID).with_capability(
        CapabilityEndpointPlan::new(
            CAPABILITY_ID,
            DESCRIPTOR_VERSION,
            [
                APPEND_EVENT_OPERATION,
                GET_EVENT_OPERATION,
                LIST_EVENTS_OPERATION,
            ],
        ),
    );
    AppComposition::new(
        vec![consumer, provider],
        vec![CapabilityBinding::new(
            "consumer",
            CAPABILITY_ID,
            DESCRIPTOR_VERSION,
            "audit",
        )],
    )
    .resolve()
    .unwrap()
}

fn actual_plan(configuration: &str, include_consumer: bool) -> ResolvedAppPlan {
    let audit = PluginInstancePlan::new("audit", PACKAGE_ID)
        .with_configuration(configuration)
        .with_requirement(CapabilityRequirementPlan::one(
            SECRETS_CAPABILITY_ID,
            SECRETS_DESCRIPTOR_VERSION,
        ))
        .with_capability(CapabilityEndpointPlan::new(
            CAPABILITY_ID,
            DESCRIPTOR_VERSION,
            [
                APPEND_EVENT_OPERATION,
                GET_EVENT_OPERATION,
                LIST_EVENTS_OPERATION,
            ],
        ));
    let secrets = PluginInstancePlan::new("secrets", SECRETS_PACKAGE_ID).with_capability(
        CapabilityEndpointPlan::new(
            SECRETS_CAPABILITY_ID,
            SECRETS_DESCRIPTOR_VERSION,
            [RESOLVE_OPERATION],
        ),
    );
    let mut plugins = vec![audit, secrets];
    let mut bindings = vec![CapabilityBinding::new(
        "audit",
        SECRETS_CAPABILITY_ID,
        SECRETS_DESCRIPTOR_VERSION,
        "secrets",
    )];
    if include_consumer {
        plugins.push(
            PluginInstancePlan::new("consumer", CONSUMER_PACKAGE_ID).with_requirement(
                CapabilityRequirementPlan::one(CAPABILITY_ID, DESCRIPTOR_VERSION),
            ),
        );
        bindings.push(CapabilityBinding::new(
            "consumer",
            CAPABILITY_ID,
            DESCRIPTOR_VERSION,
            "audit",
        ));
    }
    AppComposition::new(plugins, bindings).resolve().unwrap()
}

fn append_request() -> AppendEventRequest {
    AppendEventRequest {
        action: "close".to_owned(),
        actor: AppendEventRequestActor {
            display: Some("Avery".to_owned()),
            id: Some("user-123".to_owned()),
            kind: "user".to_owned(),
        },
        event_name: "support.ticket.closed".to_owned(),
        metadata: BTreeMap::from([
            ("api_token".to_owned(), json!("api-token-value")),
            (
                "nested".to_owned(),
                json!({"password": "password-value", "safe": "visible"}),
            ),
        ]),
        occurred_at: "2026-08-30T00:00:00Z".to_owned(),
        outcome: AppendEventRequestOutcome::Success,
        reason: Some("resolved".to_owned()),
        request_context: None,
        resource: None,
        scope: None,
        severity: AppendEventRequestSeverity::Info,
    }
}

fn list_request() -> ListEventsRequest {
    ListEventsRequest {
        actor_id: None,
        actor_kind: None,
        correlation_id: None,
        cursor: None,
        event_name: None,
        limit: 50,
        source_instance: None,
        occurred_after: None,
        occurred_before: None,
        outcome: None,
        resource_id: None,
        resource_type: None,
        scope_id: None,
        scope_module: None,
        scope_type: None,
        severity: None,
    }
}

async fn reset_legacy_test_state(pool: &sqlx::PgPool) {
    sqlx::raw_sql(
        "drop publication if exists audit_log_test_publication;
         drop schema if exists audit_log cascade",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("drop schema if exists platform cascade")
        .execute(pool)
        .await
        .unwrap();
}

async fn install_legacy_state(pool: &sqlx::PgPool) {
    sqlx::raw_sql(
        "create schema platform;
         create table platform.schema_migrations (
           name text primary key,
           applied_at timestamptz not null default now()
         );
         insert into platform.schema_migrations (name)
         values ('audit-log/0001_create_audit_log_schema');",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::raw_sql(AUDIT_LOG_MIGRATION_SQL)
        .execute(pool)
        .await
        .unwrap();
}

async fn insert_legacy_metadata_rows(pool: &sqlx::PgPool) {
    sqlx::query(
        "insert into audit_log.events (
           id, event_name, module_name, action, outcome, severity, actor_kind, metadata, occurred_at
         ) values
           ('legacy_object', 'legacy.object', 'legacy', 'read', 'success', 'info', 'system', '{\"visible\":true}'::jsonb, '2026-08-30T00:00:04Z'),
           ('legacy_array', 'legacy.array', 'legacy', 'read', 'success', 'info', 'system', '[\"one\",2]'::jsonb, '2026-08-30T00:00:03Z'),
           ('legacy_scalar', 'legacy.scalar', 'legacy', 'read', 'success', 'info', 'system', '\"scalar\"'::jsonb, '2026-08-30T00:00:02Z'),
           ('legacy_null', 'legacy.null', 'legacy', 'read', 'success', 'info', 'system', 'null'::jsonb, '2026-08-30T00:00:01Z')",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn load_raw_metadata(pool: &sqlx::PgPool) -> Vec<(String, Value)> {
    sqlx::query_as("select id, metadata from audit_log.events order by id")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn managed_ledger_exists(pool: &sqlx::PgPool) -> bool {
    sqlx::query_scalar(
        "select exists (
           select 1
           from pg_class as relations
           join pg_namespace as namespaces on namespaces.oid = relations.relnamespace
           where namespaces.nspname = 'audit_log'
             and relations.relname = '_lenso_schema_migrations'
         )",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn expect_adoption_refused(database_url: &str, expected: LegacyAdoptionRefusal) {
    let error = AuditLogOperator::adopt_legacy(database_url)
        .await
        .unwrap_err();
    assert!(
        matches!(error, AuditLogOperatorError::AdoptionRefused(actual) if actual == expected),
        "expected {expected:?}, got {error:?}"
    );
    let pool = sqlx::PgPool::connect(database_url).await.unwrap();
    if expected != LegacyAdoptionRefusal::AlreadyManaged {
        assert!(
            !managed_ledger_exists(&pool).await,
            "a refused adoption must roll back the Plugin ledger"
        );
    }
    pool.close().await;
}

fn postgres_test_url() -> String {
    let database_url = std::env::var("LENSO_POSTGRES_TEST_URL")
        .expect("LENSO_POSTGRES_TEST_URL must be set for ignored acceptance tests");
    let parsed = url::Url::parse(&database_url)
        .expect("LENSO_POSTGRES_TEST_URL must be a valid PostgreSQL URL");
    assert!(
        matches!(parsed.scheme(), "postgres" | "postgresql"),
        "LENSO_POSTGRES_TEST_URL must use the postgres scheme"
    );
    let database_name = parsed
        .path_segments()
        .and_then(Iterator::last)
        .filter(|value| !value.is_empty())
        .expect("LENSO_POSTGRES_TEST_URL must name a database");
    assert!(
        database_name.to_ascii_lowercase().contains("test"),
        "refusing destructive acceptance test against a database without `test` in its name"
    );
    database_url
}

async fn drop_audit_schema(pool: &sqlx::PgPool) {
    sqlx::query("drop schema if exists audit_log cascade")
        .execute(pool)
        .await
        .unwrap();
}
