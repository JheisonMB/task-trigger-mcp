# 🚀 Guía de Uso - task-trigger-mcp

## Inicio Rápido

### 1. Compilar el proyecto

```bash
cd /mnt/c/Users/PC/Documents/PersonalProjects/mcp/task-trigger-mcp
cargo build --release
```

### 2. Ejecutar el servidor

```bash
# Con logging (recomendado para desarrollo)
RUST_LOG=info cargo run

# O directamente con el binario compilado
./target/debug/task-trigger-mcp

# Configurar puerto personalizado
TASK_TRIGGER_PORT=8000 ./target/debug/task-trigger-mcp
```

### 3. Verificar que está funcionando

```bash
curl http://localhost:7755/health
# Respuesta: {"status":"ok","version":"0.1.0"}
```

---

## Ejemplos de Uso

### Crear una tarea programada

```bash
curl -X POST http://localhost:7755/mcp/call \
  -H "Content-Type: application/json" \
  -d '{
    "tool": "task_add",
    "params": {
      "id": "backup-db",
      "prompt": "echo Running daily backup",
      "schedule": "0 2 * * *",
      "cli": "opencode",
      "model": "gpt-4"
    }
  }'
```

### Crear tarea con schedule natural

```bash
curl -X POST http://localhost:7755/mcp/call \
  -H "Content-Type: application/json" \
  -d '{
    "tool": "task_add",
    "params": {
      "id": "check-logs",
      "prompt": "echo Checking application logs",
      "schedule": "every 30 minutes",
      "cli": "kiro"
    }
  }'
```

### Listar todas las tareas

```bash
curl -X POST http://localhost:7755/mcp/call \
  -H "Content-Type: application/json" \
  -d '{"tool": "task_list", "params": {}}'
```

### Crear un watcher para monitorear cambios

```bash
curl -X POST http://localhost:7755/mcp/call \
  -H "Content-Type: application/json" \
  -d '{
    "tool": "task_watch",
    "params": {
      "id": "watch-config",
      "path": "/etc/app/config.yml",
      "events": ["modify"],
      "prompt": "Configuration changed, reloading",
      "cli": "opencode"
    }
  }'
```

### Deshabilitar una tarea

```bash
curl -X POST http://localhost:7755/mcp/call \
  -H "Content-Type: application/json" \
  -d '{
    "tool": "task_disable",
    "params": {"id": "backup-db"}
  }'
```

### Habilitar una tarea

```bash
curl -X POST http://localhost:7755/mcp/call \
  -H "Content-Type: application/json" \
  -d '{
    "tool": "task_enable",
    "params": {"id": "backup-db"}
  }'
```

### Eliminar una tarea

```bash
curl -X POST http://localhost:7755/mcp/call \
  -H "Content-Type: application/json" \
  -d '{
    "tool": "task_remove",
    "params": {"id": "backup-db"}
  }'
```

### Obtener estado del daemon

```bash
curl -X POST http://localhost:7755/mcp/call \
  -H "Content-Type: application/json" \
  -d '{"tool": "task_status", "params": {}}'
```

### Ver watchers activos

```bash
curl -X POST http://localhost:7755/mcp/call \
  -H "Content-Type: application/json" \
  -d '{"tool": "task_watchers", "params": {}}'
```

---

## Formatos de Schedule Soportados

### Schedules Naturales (Convertidos a Cron)

- `every minute` → `* * * * *`
- `every 5 minutes` → `*/5 * * * *`
- `every hour` → `0 * * * *`
- `every 2 hours` → `0 */2 * * *`
- `every day` → `0 0 * * *`
- `daily at 2am` → `0 2 * * *`
- `every monday` → `0 0 * * 1`
- `every month` → `0 0 1 * *`

### Cron Directo

Formato estándar cron de 5 campos:
```
┌───────────── minuto (0 - 59)
│ ┌───────────── hora (0 - 23)
│ │ ┌───────────── día del mes (1 - 31)
│ │ │ ┌───────────── mes (1 - 12)
│ │ │ │ ┌───────────── día de semana (0 - 6, 0 = domingo)
│ │ │ │ │
│ │ │ │ │
* * * * *
```

Ejemplos:
- `0 0 * * *` - Diariamente a medianoche
- `0 */2 * * *` - Cada 2 horas
- `0 9 * * 1-5` - Lunes a viernes a las 9 AM
- `30 12 * * *` - Diariamente a las 12:30 PM

---

## Parámetros de Tareas

### task_add

```javascript
{
  "id": "string",              // Identificador único (alfanumérico + - _)
  "prompt": "string",          // Descripción o comando a ejecutar
  "schedule": "string",        // Schedule en formato natural o cron
  "cli": "opencode" | "kiro",  // CLI a usar
  "model": "string",           // (Opcional) Modelo LLM (ej: gpt-4)
  "duration_minutes": number,  // (Opcional) Minutos hasta que expire la tarea
  "working_dir": "string"      // (Opcional) Directorio de trabajo
}
```

### task_watch

```javascript
{
  "id": "string",                      // Identificador único
  "path": "string",                    // Ruta del archivo/directorio a monitorear
  "events": ["modify", "create", ...], // Eventos a escuchar
  "prompt": "string",                  // Prompt cuando se dispara
  "cli": "opencode" | "kiro",          // CLI a usar
  "model": "string",                   // (Opcional) Modelo LLM
  "debounce_seconds": number,          // (Opcional) Esperar N segundos antes de disparar
  "recursive": boolean                 // (Opcional) Incluir subdirectorios
}
```

---

## Variables Disponibles en Prompts

Los siguientes placeholders pueden usarse en el campo `prompt`:

- `{{TIMESTAMP}}` - Timestamp actual en RFC3339
- `{{TASK_ID}}` - ID de la tarea
- `{{LOG_PATH}}` - Ruta del archivo de log
- `{{FILE_PATH}}` - Ruta del archivo que disparó el watcher (solo watchers)
- `{{EVENT_TYPE}}` - Tipo de evento (create/modify/delete/move)

Ejemplo:
```
"prompt": "Task {{TASK_ID}} triggered at {{TIMESTAMP}} - Check logs at {{LOG_PATH}}"
```

---

## Directorio de Datos

El servidor almacena todos los datos en:
```
~/.task-trigger/
├── tasks.db        # Base de datos SQLite
└── logs/           # Logs de ejecución
    ├── task-1.log
    ├── task-2.log
    └── ...
```

---

## Códigos de Respuesta HTTP

| Status | Significado |
|--------|------------|
| 200 | Éxito |
| 400 | Error en los parámetros o ejecución de herramienta |
| 404 | Endpoint no encontrado |
| 500 | Error interno del servidor |

---

## Troubleshooting

### El servidor no inicia

```bash
# Verificar logs
RUST_LOG=debug ./target/debug/task-trigger-mcp

# Limpiar base de datos corrupta
rm ~/.task-trigger/tasks.db
```

### Puerto 7755 en uso

```bash
# Usar puerto diferente
TASK_TRIGGER_PORT=8000 ./target/debug/task-trigger-mcp

# O encontrar y matar el proceso
lsof -i :7755
kill -9 <PID>
```

### Tareas no persisten

```bash
# Verificar que el directorio existe
ls -la ~/.task-trigger/

# Verificar permisos
chmod 755 ~/.task-trigger/
chmod 644 ~/.task-trigger/tasks.db
```

---

## APIs Disponibles

| Método | Endpoint | Descripción |
|--------|----------|------------|
| GET | `/health` | Health check del servidor |
| GET | `/mcp/tools` | Listar herramientas disponibles |
| POST | `/mcp/call` | Ejecutar una herramienta |

---

## Herramientas Disponibles

1. **task_add** - Registrar nueva tarea
2. **task_watch** - Monitorear archivo/directorio
3. **task_list** - Listar todas las tareas
4. **task_watchers** - Listar todos los watchers
5. **task_remove** - Eliminar una tarea
6. **task_unwatch** - Dejar de monitorear
7. **task_enable** - Habilitar tarea
8. **task_disable** - Deshabilitar tarea
9. **task_status** - Estado del daemon
10. **task_logs** - Obtener logs de una tarea

---

## Notas Importantes

1. ✅ Toda la data se persiste en SQLite
2. ✅ Las tareas sobreviven reinicios del servidor
3. ✅ Los IDs deben ser únicos y alfanuméricos
4. ✅ Los schedules se validan antes de guardar
5. ✅ El servidor es stateless (data en BD)
6. ✅ Múltiples instancias pueden correr en puertos diferentes

---

## Próximas Features (Post-MVP)

- [ ] Integración con crontab/launchd
- [ ] Ejecución real de tareas
- [ ] Captura de salida/errores
- [ ] Historial de ejecuciones
- [ ] Notificaciones en caso de fallo
- [ ] API de progreso/logs en vivo
- [ ] Interfaz web de administración

