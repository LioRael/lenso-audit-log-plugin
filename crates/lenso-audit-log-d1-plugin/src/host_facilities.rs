//! Private finite storage operations; no SQL or database binding reaches a Capability.
use lenso::prelude::*;
use lenso_audit_log_core::{
    EventStore, StoreError,
    model::{EventFilter, NewAuditEvent, StoredEvent},
};
use lenso_capability_audit_log::AppendEventError;
#[derive(Clone)]
pub struct StoreHandle {
    #[cfg(target_arch = "wasm32")]
    inner: wasm_bindgen::JsValue,
}
impl std::fmt::Debug for StoreHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditD1Store").finish_non_exhaustive()
    }
}
fn unavailable() -> RuntimeFailure {
    RuntimeFailure::PluginFailure {
        detail: "D1 Audit storage unavailable (a submitted append may have committed)".into(),
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub fn store(_: &serde_json::Value) -> Result<StoreHandle, RuntimeFailure> {
    Err(RuntimeFailure::InvalidResolvedPlan {
        detail: "D1 Audit requires the Workers D1 profile".into(),
    })
}
#[cfg(target_arch = "wasm32")]
pub fn store(value: &wasm_bindgen::JsValue) -> Result<StoreHandle, RuntimeFailure> {
    if !value.is_object() {
        return Err(unavailable());
    }
    Ok(StoreHandle {
        inner: value.clone(),
    })
}
impl StoreHandle {
    #[cfg(target_arch = "wasm32")]
    pub async fn readiness(&self) -> Result<(), RuntimeFailure> {
        #[cfg(target_arch = "wasm32")]
        {
            let _: bool = self
                .call("readiness", &())
                .await
                .map_err(|_| unavailable())?;
            Ok(())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Err(unavailable())
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn readiness(&self) -> impl std::future::Future<Output = Result<(), RuntimeFailure>> {
        std::future::ready(Err(unavailable()))
    }

    #[cfg(target_arch = "wasm32")]
    async fn call<I: serde::Serialize, O: serde::de::DeserializeOwned>(
        &self,
        name: &str,
        input: &I,
    ) -> Result<O, StoreError> {
        use wasm_bindgen::JsCast;
        let method = js_sys::Reflect::get(&self.inner, &wasm_bindgen::JsValue::from_str(name))
            .map_err(|_| StoreError::Unavailable)?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| StoreError::Unavailable)?;
        let arg =
            serde::Serialize::serialize(input, &serde_wasm_bindgen::Serializer::json_compatible())
                .map_err(|_| StoreError::Unavailable)?;
        let result = method
            .call1(&self.inner, &arg)
            .map_err(|_| StoreError::Unavailable)?;
        let result = wasm_bindgen_futures::JsFuture::from(js_sys::Promise::resolve(&result))
            .await
            .map_err(|_| StoreError::Unavailable)?;
        serde_wasm_bindgen::from_value(result).map_err(|_| StoreError::Unavailable)
    }
}

#[cfg(target_arch = "wasm32")]
impl EventStore for StoreHandle {
    fn fresh_id(&self) -> Result<String, PluginError<AppendEventError>> {
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::JsCast;
            let f = js_sys::Reflect::get(&self.inner, &wasm_bindgen::JsValue::from_str("fresh_id"))
                .ok()
                .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
                .ok_or_else(|| PluginError::runtime(unavailable()))?;
            let id = f
                .call0(&self.inner)
                .ok()
                .and_then(|v| v.as_string())
                .filter(|s| s.starts_with("audit_evt_") && s.len() <= 128)
                .ok_or_else(|| PluginError::runtime(unavailable()))?;
            Ok(id)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Err(PluginError::runtime(unavailable()))
        }
    }
    async fn append_event(&self, event: NewAuditEvent) -> Result<StoredEvent, StoreError> {
        #[cfg(target_arch = "wasm32")]
        {
            let input = serde_json::json!({"occurred_key":key(event.occurred_at),"event":&event});
            let stored: StoredEvent = self.call("append", &input).await?;
            if StoredEvent::from_event(event, stored.created_at) == stored {
                Ok(stored)
            } else {
                Err(StoreError::IdempotencyConflict)
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = event;
            Err(StoreError::Unavailable)
        }
    }
    async fn get_event(&self, id: &str) -> Result<Option<StoredEvent>, StoreError> {
        #[cfg(target_arch = "wasm32")]
        {
            self.call("get", &id).await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = id;
            Err(StoreError::Unavailable)
        }
    }
    async fn list_events(&self, filter: &EventFilter) -> Result<Vec<StoredEvent>, StoreError> {
        #[cfg(target_arch = "wasm32")]
        {
            let mut input = serde_json::to_value(filter).map_err(|_| StoreError::Unavailable)?;
            input["occurred_after"] = filter.occurred_after.map(key).into();
            input["occurred_before"] = filter.occurred_before.map(key).into();
            if let Some(cursor) = &filter.cursor {
                input["cursor"]["occurred_at"] = key(cursor.occurred_at).into();
            }
            self.call("list", &input).await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = filter;
            Err(StoreError::Unavailable)
        }
    }
}
#[cfg(target_arch = "wasm32")]
fn key(date: chrono::DateTime<chrono::Utc>) -> String {
    date.format("%Y-%m-%dT%H:%M:%S%.9fZ").to_string()
}

#[cfg(not(target_arch = "wasm32"))]
impl EventStore for StoreHandle {
    fn fresh_id(&self) -> Result<String, PluginError<AppendEventError>> {
        Err(PluginError::runtime(unavailable()))
    }
    fn append_event(
        &self,
        _event: NewAuditEvent,
    ) -> impl std::future::Future<Output = Result<StoredEvent, StoreError>> {
        std::future::ready(Err(StoreError::Unavailable))
    }
    fn get_event(
        &self,
        _id: &str,
    ) -> impl std::future::Future<Output = Result<Option<StoredEvent>, StoreError>> {
        std::future::ready(Err(StoreError::Unavailable))
    }
    fn list_events(
        &self,
        _filter: &EventFilter,
    ) -> impl std::future::Future<Output = Result<Vec<StoredEvent>, StoreError>> {
        std::future::ready(Err(StoreError::Unavailable))
    }
}
