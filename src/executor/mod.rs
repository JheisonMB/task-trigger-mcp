use anyhow::Result;

/// Executor para correr tareas headlessly
pub struct Executor;

impl Executor {
    pub async fn execute_task(task_id: &str, prompt: &str) -> Result<()> {
        // TODO: Implementar ejecución de CLI
        Ok(())
    }
}
