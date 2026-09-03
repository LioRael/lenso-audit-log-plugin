//! Agent-facing read Tools over an explicitly bound Audit Log capability.

use lenso::prelude::*;
use lenso_capability_agent_tool_provider::{
    self as tool_contract, CatalogRequest, CatalogResponse, ContentType, ExecuteError,
    ExecuteRequest, ExecuteResponse, ExecutionFailedPayload, ToolDefinition, ToolExecutionClass,
};
use lenso_capability_audit_log::{self as audit_log, GetEventRequest, ListEventsRequest};
use lenso_kernel::RuntimeFailure;
use serde::{Serialize, de::DeserializeOwned};

pub const LIST_EVENTS_TOOL: &str = "audit_log_list_events";
pub const GET_EVENT_TOOL: &str = "audit_log_get_event";

#[lenso::plugin]
#[derive(Clone, Debug)]
struct AuditLogAgentToolsPlugin {
    audit_log: Port<audit_log::AuditLogClient>,
}

#[lenso::provides(tool_contract::ToolProvider)]
impl AuditLogAgentToolsPlugin {
    fn catalog(
        &self,
        _context: Ctx,
        _request: CatalogRequest,
    ) -> impl std::future::Future<Output = PluginResult<CatalogResponse, tool_contract::CatalogError>>
    {
        let _ = self;
        futures::future::ready(Ok(CatalogResponse {
            tools: tool_definitions(),
        }))
    }

    async fn execute(
        &self,
        context: Ctx,
        request: ExecuteRequest,
    ) -> PluginResult<ExecuteResponse, ExecuteError> {
        match request.name.as_str() {
            LIST_EVENTS_TOOL => {
                let arguments = decode::<ListEventsRequest>(&request)?;
                match self
                    .audit_log
                    .list_events_with_context(context, arguments)
                    .await
                {
                    Ok(response) => success(LIST_EVENTS_TOOL, &response),
                    Err(audit_log::AuditLogListEventsInvocationError::Domain(error)) => {
                        Err(PluginError::domain(map_list_events_error(&error)))
                    }
                    Err(audit_log::AuditLogListEventsInvocationError::Runtime(error)) => {
                        Err(PluginError::runtime(error))
                    }
                }
            }
            GET_EVENT_TOOL => {
                let arguments = decode::<GetEventRequest>(&request)?;
                match self
                    .audit_log
                    .get_event_with_context(context, arguments)
                    .await
                {
                    Ok(response) => success(GET_EVENT_TOOL, &response),
                    Err(audit_log::AuditLogGetEventInvocationError::Domain(error)) => {
                        Err(PluginError::domain(map_get_event_error(&error)))
                    }
                    Err(audit_log::AuditLogGetEventInvocationError::Runtime(error)) => {
                        Err(PluginError::runtime(error))
                    }
                }
            }
            _ => Err(PluginError::domain(ExecuteError::NotFound)),
        }
    }
}

fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        tool(
            LIST_EVENTS_TOOL,
            "List durable Audit Log evidence with bounded pagination and exact provenance, actor, scope, resource, outcome, severity, correlation, event, and time filters.",
            include_str!(
                "../../lenso-capability-audit-log/schemas/list-events-request.schema.json"
            ),
        ),
        tool(
            GET_EVENT_TOOL,
            "Get one durable Audit Log event by its exact event ID.",
            include_str!("../../lenso-capability-audit-log/schemas/get-event-request.schema.json"),
        ),
    ]
}

fn tool(name: &str, description: &str, schema: &str) -> ToolDefinition {
    let schema: serde_json::Value =
        serde_json::from_str(schema).expect("Audit Log Tool schema must be valid JSON");
    ToolDefinition {
        name: name.to_owned(),
        description: description.to_owned(),
        input_schema_json: schema
            .to_string()
            .try_into()
            .expect("Audit Log Tool schema must remain valid JSON"),
        execution: ToolExecutionClass::ParallelSafe,
    }
}

fn decode<T: DeserializeOwned>(request: &ExecuteRequest) -> PluginResult<T, ExecuteError> {
    serde_json::from_str(request.arguments_json.as_str())
        .map_err(|_| PluginError::domain(ExecuteError::InvalidArguments))
}

fn success<T: Serialize>(
    tool_name: &str,
    response: &T,
) -> PluginResult<ExecuteResponse, ExecuteError> {
    let content = serde_json::to_string_pretty(response).map_err(|error| {
        PluginError::runtime(RuntimeFailure::PluginFailure {
            detail: format!("Audit Log Tool could not serialize its response: {error}"),
        })
    })?;
    Ok(ExecuteResponse {
        content_blocks: None,
        content,
        content_type: ContentType::Text,
        metadata_json: serde_json::json!({ "tool": tool_name })
            .to_string()
            .try_into()
            .expect("Audit Log Tool metadata must be valid JSON"),
    })
}

fn map_list_events_error(error: &audit_log::ListEventsError) -> ExecuteError {
    match error {
        audit_log::ListEventsError::Unauthorized => ExecuteError::PermissionDenied,
        audit_log::ListEventsError::InvalidQuery => ExecuteError::InvalidArguments,
        audit_log::ListEventsError::Unknown(_) => rejected("unknown_domain_error"),
    }
}

fn map_get_event_error(error: &audit_log::GetEventError) -> ExecuteError {
    match error {
        audit_log::GetEventError::Unauthorized => ExecuteError::PermissionDenied,
        audit_log::GetEventError::InvalidId => ExecuteError::InvalidArguments,
        audit_log::GetEventError::NotFound => ExecuteError::NotFound,
        audit_log::GetEventError::Unknown(_) => rejected("unknown_domain_error"),
    }
}

fn rejected(reason_code: &str) -> ExecuteError {
    ExecuteError::ExecutionFailed {
        payload: ExecutionFailedPayload {
            reason_code: reason_code.to_owned(),
            message: "Audit Log rejected the requested read operation.".to_owned(),
            details_json: serde_json::json!({ "domain_error": reason_code })
                .to_string()
                .try_into()
                .expect("Audit Log Tool error metadata must be valid JSON"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(name: &str, arguments: &str) -> ExecuteRequest {
        ExecuteRequest {
            name: name.to_owned(),
            arguments_json: arguments.try_into().unwrap(),
        }
    }

    #[test]
    fn descriptor_is_a_stateless_single_role_adapter() {
        let descriptor: serde_json::Value = serde_json::from_str(PLUGIN_DESCRIPTOR_JSON).unwrap();
        assert_eq!(descriptor["plugin_id"], "lenso.audit-log.agent-tools");
        assert_eq!(
            descriptor["provided_capabilities"][0]["capability_id"],
            "lenso.agent.tool-provider@2"
        );
        let required = descriptor["required_capabilities"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0]["capability_id"], "lenso.audit-log@1");
    }

    #[test]
    fn catalog_contains_only_parallel_safe_reads() {
        let tools = tool_definitions();
        assert_eq!(tools.len(), 2);
        assert!(
            tools
                .iter()
                .all(|tool| tool.execution == ToolExecutionClass::ParallelSafe)
        );
        assert!(tools.iter().all(|tool| !tool.name.contains("append")));
    }

    #[test]
    fn requests_and_domain_failures_preserve_contract_semantics() {
        let list = decode::<ListEventsRequest>(&request(
            LIST_EVENTS_TOOL,
            r#"{"event_name":"access.role_assigned","outcome":"success","limit":50}"#,
        ))
        .unwrap();
        assert_eq!(list.limit, 50);
        assert!(
            decode::<ListEventsRequest>(&request(
                LIST_EVENTS_TOOL,
                r#"{"outcome":"succeeded","limit":50}"#,
            ))
            .is_err()
        );
        assert_eq!(
            map_list_events_error(&audit_log::ListEventsError::Unauthorized),
            ExecuteError::PermissionDenied
        );
        assert_eq!(
            map_get_event_error(&audit_log::GetEventError::NotFound),
            ExecuteError::NotFound
        );
    }
}
