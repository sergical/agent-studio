#[derive(Debug, serde::Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum DocumentSaveError {
    Cancelled,
    Failed { message: String },
}

impl From<String> for DocumentSaveError {
    fn from(message: String) -> Self {
        Self::Failed { message }
    }
}
impl From<&str> for DocumentSaveError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
impl std::fmt::Display for DocumentSaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Save cancelled"),
            Self::Failed { message } => f.write_str(message),
        }
    }
}
impl std::error::Error for DocumentSaveError {}

impl From<skill_studio_core::skill_service::PreparedContentError> for DocumentSaveError {
    fn from(error: skill_studio_core::skill_service::PreparedContentError) -> Self {
        if error.is_cancelled() {
            Self::Cancelled
        } else {
            Self::Failed {
                message: error.to_string(),
            }
        }
    }
}

impl From<skill_studio_core::skill_service::WritePreparationError> for DocumentSaveError {
    fn from(error: skill_studio_core::skill_service::WritePreparationError) -> Self {
        use skill_studio_core::skill_service::{
            CoordinationFailure, ScanError, WritePreparationError,
        };
        match error {
            WritePreparationError::Scan(ScanError::Coordination(
                CoordinationFailure::Cancelled,
            )) => Self::Cancelled,
            other => other.to_string().into(),
        }
    }
}

use std::sync::{Arc, Mutex};

use skill_studio_core::skill_service::CancellationToken;
use tauri::{Emitter, Manager};

#[derive(Default, Clone)]
pub struct DocumentOperationState(Arc<Mutex<DocumentOperationInner>>);

#[derive(Default)]
struct DocumentOperationInner {
    active: Option<(String, CancellationToken)>,
    exit_code: Option<i32>,
}

pub(crate) struct DocumentOperation {
    state: DocumentOperationState,
    id: String,
    pub cancellation: CancellationToken,
    app: Option<tauri::AppHandle>,
}

impl DocumentOperationState {
    pub(crate) fn begin(&self, id: String) -> Result<DocumentOperation, String> {
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
        {
            return Err("Invalid document operation ID".into());
        }
        let mut state = self
            .0
            .lock()
            .map_err(|_| "Document operation state is unavailable")?;
        if state.exit_code.is_some() {
            return Err("The application is shutting down".into());
        }
        if state.active.is_some() {
            return Err("A document operation is still running or cleaning up".into());
        }
        let cancellation = CancellationToken::default();
        state.active = Some((id.clone(), cancellation.clone()));
        Ok(DocumentOperation {
            state: self.clone(),
            id,
            cancellation,
            app: None,
        })
    }

    pub(crate) fn request_exit(&self, code: i32) -> Result<bool, String> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| "Document operation state is unavailable")?;
        state.exit_code.get_or_insert(code);
        if let Some((_, cancellation)) = &state.active {
            cancellation.cancel();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn finish(&self, id: &str) -> Option<i32> {
        let mut state = self.0.lock().ok()?;
        if state
            .active
            .as_ref()
            .is_some_and(|(active_id, _)| active_id == id)
        {
            state.active = None;
            state.exit_code
        } else {
            None
        }
    }

    fn cancel(&self, id: &str) -> Result<bool, String> {
        let state = self
            .0
            .lock()
            .map_err(|_| "Document operation state is unavailable")?;
        if let Some((_, cancellation)) = state
            .active
            .as_ref()
            .filter(|(active_id, _)| active_id == id)
        {
            cancellation.cancel();
            return Ok(true);
        }
        Ok(false)
    }
}

impl DocumentOperation {
    pub(crate) fn start(app: &tauri::AppHandle, id: Option<String>) -> Result<Self, String> {
        let mut operation = app
            .state::<DocumentOperationState>()
            .begin(id.unwrap_or_else(super::event_store::allocate_id))?;
        operation.app = Some(app.clone());
        app.emit("skills://document-operation-started", &operation.id)
            .map_err(|error| error.to_string())?;
        Ok(operation)
    }
}

impl Drop for DocumentOperation {
    fn drop(&mut self) {
        if let Some(code) = self.state.finish(&self.id) {
            if let Some(app) = &self.app {
                app.exit(code);
            }
        }
    }
}

pub(crate) fn check_document_cancellation(cancellation: &CancellationToken) -> Result<(), String> {
    if cancellation.is_cancelled() {
        Err("Document operation cancelled; any recorded intent may need recovery".into())
    } else {
        Ok(())
    }
}

#[tauri::command]
pub fn cancel_document_operation(
    operation_id: String,
    state: tauri::State<DocumentOperationState>,
) -> Result<bool, String> {
    state.cancel(&operation_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_error_wire_contract_preserves_failure_details() {
        assert_eq!(
            serde_json::to_value(DocumentSaveError::Cancelled).unwrap(),
            serde_json::json!({"code": "cancelled"})
        );
        assert_eq!(
            serde_json::to_value(DocumentSaveError::from("Recovery remains unresolved")).unwrap(),
            serde_json::json!({"code": "failed", "message": "Recovery remains unresolved"})
        );
    }

    #[test]
    fn exit_cancels_work_and_waits_for_its_final_release() {
        let state = DocumentOperationState::default();
        let operation = state.begin("running".into()).unwrap();
        assert!(state.request_exit(42).unwrap());
        assert!(operation.cancellation.is_cancelled());
        assert!(state.begin("new".into()).is_err());
        assert!(state.request_exit(7).unwrap());
        assert_eq!(state.finish("unrelated"), None);
        assert_eq!(state.finish("running"), Some(42));
        assert!(!state.request_exit(7).unwrap());
        assert!(state.begin("after-cleanup".into()).is_err());
        drop(operation);
    }

    #[test]
    fn cancellation_keeps_admission_until_owned_work_is_dropped() {
        let state = DocumentOperationState::default();
        let operation = state.begin("first".into()).unwrap();
        assert!(!state.cancel("other").unwrap());
        assert!(!operation.cancellation.is_cancelled());
        assert!(state.cancel("first").unwrap());
        assert!(operation.cancellation.is_cancelled());
        assert!(state.begin("second".into()).is_err());
        drop(operation);
        let next = state.begin("second".into()).unwrap();
        assert!(!state.cancel("first").unwrap());
        assert!(!next.cancellation.is_cancelled());
    }
}
