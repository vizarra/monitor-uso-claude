# Fixtures sintéticos

Datos inventados para los tests. No contienen conversaciones, tokens ni identificadores reales. El formato está descrito en `docs/fuentes-datos.md`.

| Archivo | Qué cubre | Resultado esperado |
| --- | --- | --- |
| `session_basic.jsonl` | Tipos de línea mezclados, duplicados de streaming (`msg_A`), campos opcionales, línea `isSidechain`, línea `assistant` sin `usage` | Tras deduplicar: entrada 31, salida 82, creación de caché 100, lectura de caché 1500 |
| `session_malformed.jsonl` | Línea no JSON, `timestamp` inválido y última línea cortada | Solo cuenta `msg_X`: entrada 7, salida 3 |
| `ratelimit_headers.txt` | Cabeceras de la sonda | Sesión 42 %, semana 61 %, reinicios 1790000000 y 1790500000 |
| `usage_report.json` | Respuesta de `usage_report/messages` con un día vacío | Entrada sin caché 2000, salida 750, creación de caché 400, lectura de caché 200 |
| `cost_report.json` | Respuesta de `cost_report` con un día vacío | 200,00 centavos = 2,00 USD |
