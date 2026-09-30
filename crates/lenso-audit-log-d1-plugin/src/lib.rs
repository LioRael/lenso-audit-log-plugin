//! D1-backed Audit implementation. Configuration supplies policy, the Host supplies private storage.
pub mod host_facilities;
use host_facilities::StoreHandle;
use lenso::prelude::*;
use lenso_audit_log_core::{AuditPolicy, AuditService};
use lenso_capability_audit_log::{
    self as audit, AppendEventError, AppendEventRequest, AppendEventResponse, GetEventError,
    GetEventRequest, GetEventResponse, ListEventsError, ListEventsRequest, ListEventsResponse,
};

fn validate(config: &AuditPolicy) -> Result<(), RuntimeFailure> {
    config.validate()
}
#[lenso::plugin(lifecycle, configuration_schema="configuration.schema.json", validate=validate)]
#[derive(Clone, Debug)]
struct D1AuditLogPlugin {
    #[config]
    config: AuditPolicy,
    #[facility(id = "store")]
    store: StoreHandle,
}
impl Lifecycle for D1AuditLogPlugin {
    async fn prepare(&self, _: PrepareContext) -> Result<(), RuntimeFailure> {
        self.store.readiness().await
    }
}
impl D1AuditLogPlugin {
    fn service(&self) -> AuditService<StoreHandle> {
        AuditService {
            config: self.config.clone(),
            store: self.store.clone(),
        }
    }
}
#[lenso::provides(audit::AuditLog)]
impl D1AuditLogPlugin {
    async fn append_event(
        &self,
        context: Ctx,
        request: AppendEventRequest,
    ) -> PluginResult<AppendEventResponse, AppendEventError> {
        self.service().append_event(context, request).await
    }
    async fn get_event(
        &self,
        context: Ctx,
        request: GetEventRequest,
    ) -> PluginResult<GetEventResponse, GetEventError> {
        self.service().get_event(context, request).await
    }
    async fn list_events(
        &self,
        context: Ctx,
        request: ListEventsRequest,
    ) -> PluginResult<ListEventsResponse, ListEventsError> {
        self.service().list_events(context, request).await
    }
}
