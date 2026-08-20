//! Tauri commands backing the first-launch setup screen.

use tauri::AppHandle;

use crate::errors::Result;
use crate::tools::{self, ToolsStatus};

/// Which tools are already present, and whether auto-install is possible here.
#[tauri::command]
pub async fn tools_status() -> ToolsStatus {
    tools::status()
}

/// Download any missing tools, emitting `tools://progress` as it runs.
#[tauri::command]
pub async fn tools_install(app: AppHandle) -> Result<ToolsStatus> {
    tools::install(&app).await
}
