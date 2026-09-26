# Fuentes de datos

Resultado de la investigación inicial (2026-09-25). Todos los ejemplos son sintéticos.

| Fuente | Oficial | Verificada en | Viabilidad |
| --- | --- | --- | --- |
| Suscripción (límites 5 h y 7 días) | No | Windows | Windows y Linux: archivo. macOS: llavero, sin verificar |
| Tokens de Claude Code | No (formato interno) | Windows | Las tres plataformas, misma ruta |
| API de uso y costes | Sí | Documentación | Independiente de la plataforma; exige cuenta de organización |

---

## 1. Suscripción: cabeceras de rate limit

### Credenciales

- **Windows y Linux:** archivo `~/.claude/.credentials.json`.
- **macOS:** llavero del sistema, entrada genérica con servicio `Claude Code-credentials` (el valor es el mismo JSON). No verificado: hay que probarlo en un Mac.

Estructura (solo nombres y tipos):

```
claudeAiOauth: object
  accessToken: string           ← el único campo que se usa; nunca se registra
  refreshToken: string          ← no se usa: la app no refresca el token
  expiresAt: number             (milisegundos Unix)
  refreshTokenExpiresAt: number (milisegundos Unix)
  scopes: string[]
  subscriptionType: string      (p. ej. "pro")
  rateLimitTier: string
```

Si `expiresAt` ya pasó, el proveedor devuelve "sin datos" con el aviso "abre Claude Code para renovar la sesión". La app no refresca el token: sería escribir credenciales.

### Petición de la sonda

```
POST https://api.anthropic.com/v1/messages
authorization: Bearer <OAUTH_TOKEN>
anthropic-version: 2023-06-01
anthropic-beta: oauth-2025-04-20
content-type: application/json

{"model": "claude-haiku-4-5-20251001", "max_tokens": 1, "messages": [{"role": "user", "content": "."}]}
```

Consume una cantidad mínima de la cuota; por eso se hace como mucho una vez por intervalo.

**Cuidado:** como cuenta como uso, una sonda con la ventana de 5 h cerrada abre una nueva. Por eso la app solo sondea si la ventana sigue abierta, si Claude Code ha escrito en sus JSONL después del cierre o si el usuario pulsa ⟳ "Recargar datos".

### Cabeceras de la respuesta (HTTP 200)

Valores de ejemplo sintéticos:

| Cabecera | Ejemplo | Significado |
| --- | --- | --- |
| `anthropic-ratelimit-unified-5h-utilization` | `0.42` | Fracción usada de la ventana de 5 h (0–1). Porcentaje = valor × 100 |
| `anthropic-ratelimit-unified-5h-reset` | `1790000000` | Reinicio de la ventana de 5 h (segundos Unix) |
| `anthropic-ratelimit-unified-5h-status` | `allowed` | Estado de la ventana de 5 h |
| `anthropic-ratelimit-unified-7d-utilization` | `0.61` | Fracción usada del límite semanal (0–1) |
| `anthropic-ratelimit-unified-7d-reset` | `1790500000` | Reinicio semanal (segundos Unix) |
| `anthropic-ratelimit-unified-7d-status` | `allowed` | Estado del límite semanal |
| `anthropic-ratelimit-unified-status` | `allowed` | Estado global |
| `anthropic-ratelimit-unified-reset` | `1790000000` | Reinicio del límite que manda ahora |
| `anthropic-ratelimit-unified-representative-claim` | `five_hour` | Qué límite es el más restrictivo ahora |
| `anthropic-ratelimit-unified-fallback-percentage` | `0.5` | Sin uso previsto |
| `anthropic-ratelimit-unified-overage-status` | `rejected` | Estado del uso extra |
| `anthropic-ratelimit-unified-overage-disabled-reason` | `org_level_disabled` | Motivo si el uso extra está desactivado |

Notas para el parser:

- Solo se observó `allowed` como estado. Otros valores posibles (p. ej. al agotar el límite) no se han visto: se tratan como texto y se muestran tal cual.
- Si falta `5h-utilization` o `7d-utilization`, esa métrica queda "sin datos"; la otra sigue mostrándose.
- Un valor mayor que 1 se muestra como 100 %.
- Con HTTP 401 el token no vale (caducado o revocado); con 429 las cabeceras pueden seguir presentes y deben leerse igual.

---

## 2. Tokens: JSONL de Claude Code

### Ubicación

- `~/.claude/projects/<proyecto>/<sesión>.jsonl`
- Subagentes: `~/.claude/projects/<proyecto>/<sesión>/subagents/<agente>.jsonl`. También consumen tokens y se incluyen.

La misma ruta en las tres plataformas (con `dirs::home_dir()`). Claude Code permite cambiar el directorio con la variable de entorno `CLAUDE_CONFIG_DIR`: si existe, se usa `$CLAUDE_CONFIG_DIR/projects`.

### Formato

Una línea JSON por evento. Hay muchos tipos (`type`): `user`, `assistant`, `attachment`, `system`, `mode`, `file-history-snapshot`, etc. **Solo interesan las líneas `type == "assistant"` con `message.usage`.**

Campos que se leen (el resto, incluido `message.content`, se ignora y nunca se deserializa):

```
type: string                       "assistant"
timestamp: string                  RFC 3339 (UTC, con "Z")
requestId: string
sessionId: string
isSidechain: boolean
message.id: string
message.model: string
message.usage.input_tokens: number
message.usage.output_tokens: number
message.usage.cache_creation_input_tokens: number
message.usage.cache_read_input_tokens: number
```

Campos opcionales observados en `message.usage` que no se necesitan: `cache_creation.ephemeral_5m_input_tokens`, `cache_creation.ephemeral_1h_input_tokens`, `server_tool_use.*`, `output_tokens_details.thinking_tokens`, `service_tier`, `speed`, `inference_geo`, `iterations`.

### Duplicados

Claude Code escribe varias líneas `assistant` por respuesta (una por bloque de contenido) con el mismo `message.id` y `requestId`, y repite `usage`. En los datos reales: ~4 100 líneas con `usage` para ~2 300 respuestas únicas. En algunos duplicados `output_tokens` crece entre líneas (nunca disminuye) y el resto de campos es igual.

**Regla:** agrupar por `message.id` + `requestId` y quedarse con la última línea (la de mayor `output_tokens`). Sumar sin deduplicar casi duplicaría el total.

### Robustez

- En las pruebas no apareció ninguna línea malformada, pero la última línea puede estar a medio escribir: se ignora y se relee en el siguiente sondeo (la posición guardada debe quedarse al principio de la línea incompleta).
- Los campos de `usage` que falten cuentan como 0.

---

## 3. API de uso y costes (Admin API)

Documentación oficial: <https://platform.claude.com/docs/en/manage-claude/usage-cost-api>.

**Requisito:** una Admin API key (`sk-ant-admin01-...`), que solo existe en cuentas de **organización** de Claude Console. Las cuentas individuales no tienen Admin API.

Cabeceras comunes:

```
x-api-key: <ADMIN_KEY>
anthropic-version: 2023-06-01
```

Frescura: los datos aparecen en unos 5 min. Frecuencia máxima recomendada: una petición por minuto (encaja con el mínimo de 60 s).

### Uso: `GET /v1/organizations/usage_report/messages`

Parámetros relevantes: `starting_at` (RFC 3339, obligatorio), `ending_at`, `bucket_width` (`1m` | `1h` | `1d`, por defecto `1d`), `group_by[]` (`model`, `workspace_id`, `api_key_id`, …), `limit` (1d: máx. 31; 1h: máx. 168; 1m: máx. 1440), `page`.

```json
{
  "data": [
    {
      "starting_at": "2026-09-01T00:00:00Z",
      "ending_at": "2026-09-02T00:00:00Z",
      "results": [
        {
          "uncached_input_tokens": 1500,
          "cache_creation": { "ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 0 },
          "cache_read_input_tokens": 200,
          "output_tokens": 500,
          "server_tool_use": { "web_search_requests": 0 },
          "model": "claude-sonnet-5",
          "api_key_id": null, "workspace_id": null, "service_tier": null,
          "context_window": null, "inference_geo": null,
          "account_id": null, "service_account_id": null
        }
      ]
    }
  ],
  "has_more": false,
  "next_page": null
}
```

Los campos de agrupación valen `null` si no se agrupa por ellos. Los días sin uso aparecen con `results: []`.

### Coste: `GET /v1/organizations/cost_report`

Parámetros: `starting_at`, `ending_at`, `bucket_width` (solo `1d`), `group_by[]` (`description`, `workspace_id`), `limit` (1–31, por defecto 7), `page`.

```json
{
  "data": [
    {
      "starting_at": "2026-09-01T00:00:00Z",
      "ending_at": "2026-09-02T00:00:00Z",
      "results": [
        { "amount": "123.45", "currency": "USD", "cost_type": null, "description": null,
          "model": null, "token_type": null, "service_tier": null,
          "context_window": null, "inference_geo": null, "workspace_id": null }
      ]
    }
  ],
  "has_more": false,
  "next_page": null
}
```

**`amount` es un string decimal en centavos:** `"123.45"` son 1,2345 USD. Se parsea como decimal, no como entero. El coste de Priority Tier no aparece en este endpoint.

### Paginación

Si `has_more` es `true`, repetir la petición con `page=<next_page>`. Para "mes en curso" con `bucket_width=1d` y `limit=31` basta una página.

### Plan para la app

- Periodo: desde el día 1 del mes actual (UTC) hasta ahora.
- Uso: `usage_report/messages` con `bucket_width=1d`, sumando todos los buckets.
- Coste: `cost_report` sin agrupar, sumando `amount` de todos los buckets.
