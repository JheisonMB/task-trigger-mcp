use anyhow::Result;

/// Executor para correr tareas headlessly
#[allow(dead_code)]
pub struct Executor;

impl Executor {
    #[allow(dead_code)]
    pub async fn execute_task(_task_id: &str, _prompt: &str) -> Result<()> {
        // TODO: Implementar ejecución de CLI
        Ok(())
    }
}
