// Stub MCP handler - implementación completa próximamente
// El SDK rmcp 0.8 tiene restricciones que requieren refactorización mayor

#[derive(Clone)]
pub struct McpHandler;

impl McpHandler {
    pub fn new(_db: std::sync::Arc<crate::db::Database>, _port: u16) -> Self {
        McpHandler
    }
}
