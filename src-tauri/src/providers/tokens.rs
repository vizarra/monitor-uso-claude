//! Proveedor de tokens: suma el uso de Claude Code a partir de sus JSONL.
//!
//! Formato documentado en `docs/fuentes-datos.md`. Solo se deserializan los
//! campos de uso; el contenido de los mensajes se salta sin guardarse nunca.
//! Nada se escribe bajo `~/.claude`.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Deserialize;

use super::claude_dir::{collect_jsonl, modified_ms, projects_dir};
use super::time::parse_rfc3339_ms;
use super::{
    now_ms, FetchFuture, Metric, MetricValue, Provider, ProviderError, ProviderId, Reading, Shared,
    SESSION_WINDOW_MS,
};

/// Inicio de la ventana en la que se suman los tokens: la ventana real de
/// 5 h del plan si la sonda la conoce y sigue abierta; si no, las últimas
/// 5 h móviles.
fn window_start(now: u64, session_reset: Option<u64>) -> (u64, bool) {
    match session_reset {
        Some(reset) if reset > now => (reset.saturating_sub(SESSION_WINDOW_MS), true),
        _ => (now.saturating_sub(SESSION_WINDOW_MS), false),
    }
}

/// Suma de tokens por tipo.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TokenTotals {
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
}

impl TokenTotals {
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_creation)
            .saturating_add(self.cache_read)
    }

    fn add(&mut self, other: &TokenTotals) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_creation = self.cache_creation.saturating_add(other.cache_creation);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
    }
}

// --- Parser de líneas -------------------------------------------------------

// Structs parciales: serde ignora cualquier campo no declarado, así que el
// texto de los mensajes (`message.content`) se salta sin copiarse.
#[derive(Deserialize)]
struct RawLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    message: Option<RawMessage>,
}

#[derive(Deserialize)]
struct RawMessage {
    id: Option<String>,
    usage: Option<RawUsage>,
}

#[derive(Deserialize)]
struct RawUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

/// Uso extraído de una línea válida.
#[derive(Debug, PartialEq)]
struct UsageLine {
    key: String,
    ts_ms: u64,
    usage: TokenTotals,
}

/// Interpreta una línea del JSONL. Devuelve `None` si no es una respuesta
/// del asistente con uso, o si está malformada.
fn parse_line(line: &str) -> Option<UsageLine> {
    // Filtro barato: la mayoría de líneas no tienen `usage` y no merece la
    // pena analizarlas.
    if !line.contains("\"usage\"") {
        return None;
    }
    let raw: RawLine = serde_json::from_str(line).ok()?;
    if raw.kind.as_deref() != Some("assistant") {
        return None;
    }
    let message = raw.message?;
    let usage = message.usage?;
    let ts_ms = parse_rfc3339_ms(raw.timestamp.as_deref()?)?;
    // Claude Code repite `usage` en cada bloque de una misma respuesta:
    // la clave agrupa esas líneas para contarlas una sola vez.
    let key = format!("{}|{}", message.id?, raw.request_id.unwrap_or_default());
    Some(UsageLine {
        key,
        ts_ms,
        usage: TokenTotals {
            input: usage.input_tokens.unwrap_or(0),
            output: usage.output_tokens.unwrap_or(0),
            cache_creation: usage.cache_creation_input_tokens.unwrap_or(0),
            cache_read: usage.cache_read_input_tokens.unwrap_or(0),
        },
    })
}

// --- Libro de respuestas deduplicadas ---------------------------------------

struct Entry {
    ts_ms: u64,
    usage: TokenTotals,
}

/// Respuestas únicas vistas hasta ahora, indexadas por `message.id|requestId`.
#[derive(Default)]
struct Ledger {
    entries: HashMap<String, Entry>,
}

impl Ledger {
    /// Añade una línea. Si la respuesta ya estaba, la última línea sustituye a
    /// la anterior (lleva el `output_tokens` definitivo).
    fn ingest(&mut self, line: &str) {
        if let Some(u) = parse_line(line) {
            self.entries.insert(
                u.key,
                Entry {
                    ts_ms: u.ts_ms,
                    usage: u.usage,
                },
            );
        }
    }

    fn totals_since(&self, since_ms: u64) -> TokenTotals {
        let mut totals = TokenTotals::default();
        for e in self.entries.values().filter(|e| e.ts_ms >= since_ms) {
            totals.add(&e.usage);
        }
        totals
    }

    /// Olvida lo que ya salió de la ventana, para que la memoria no crezca.
    fn prune_before(&mut self, since_ms: u64) {
        self.entries.retain(|_, e| e.ts_ms >= since_ms);
    }
}

// --- Lectura incremental de archivos ----------------------------------------

/// Recorre los JSONL recordando hasta qué byte se leyó cada uno, para leer
/// solo lo nuevo en cada sondeo.
struct Scanner {
    root: Option<PathBuf>,
    offsets: HashMap<PathBuf, u64>,
    ledger: Ledger,
}

impl Scanner {
    fn new(root: Option<PathBuf>) -> Self {
        Self {
            root,
            offsets: HashMap::new(),
            ledger: Ledger::default(),
        }
    }

    /// Lee lo nuevo y devuelve los totales de la ventana que empieza en `since_ms`.
    fn scan(&mut self, since_ms: u64) -> Result<TokenTotals, ProviderError> {
        let root = self
            .root
            .clone()
            .filter(|r| r.is_dir())
            .ok_or_else(|| ProviderError("no se encuentra la carpeta de Claude Code".into()))?;

        let files = collect_jsonl(&root);
        for path in &files {
            // Un archivo ilegible (bloqueado, borrado a medias…) se salta
            // y se reintenta en el siguiente sondeo.
            let _ = self.scan_file(path, since_ms);
        }
        self.offsets.retain(|p, _| files.contains(p));
        self.ledger.prune_before(since_ms);
        Ok(self.ledger.totals_since(since_ms))
    }

    fn scan_file(&mut self, path: &Path, since_ms: u64) -> io::Result<()> {
        let meta = fs::metadata(path)?;
        let len = meta.len();
        let offset = match self.offsets.get(path).copied() {
            // Primera vez: si no se ha tocado desde antes de la ventana, no
            // puede aportar nada; se marca como leído sin abrirlo.
            None if modified_ms(&meta).is_some_and(|m| m < since_ms) => {
                self.offsets.insert(path.to_path_buf(), len);
                return Ok(());
            }
            None => 0,
            Some(o) if o == len => return Ok(()),
            // El archivo ha encogido (reescrito): se vuelve a leer entero.
            // Las claves evitan contar dos veces lo que ya estaba.
            Some(o) if o > len => 0,
            Some(o) => o,
        };

        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut buf = Vec::new();
        file.take(len - offset).read_to_end(&mut buf)?;

        // Solo se procesan líneas completas; una línea a medio escribir se
        // queda para el siguiente sondeo.
        let Some(last_newline) = buf.iter().rposition(|&b| b == b'\n') else {
            return Ok(());
        };
        for line in buf[..last_newline].split(|&b| b == b'\n') {
            if let Ok(text) = std::str::from_utf8(line) {
                self.ledger.ingest(text);
            }
        }
        self.offsets
            .insert(path.to_path_buf(), offset + last_newline as u64 + 1);
        Ok(())
    }
}

fn reading(t: &TokenTotals, aligned: bool) -> Reading {
    let metric = |key, label, value| Metric {
        key,
        label,
        value: MetricValue::Tokens(value),
        resets_at_ms: None,
    };
    Reading {
        metrics: vec![
            metric(
                "total",
                if aligned {
                    "Total (sesión actual)"
                } else {
                    "Total (últimas 5 h)"
                },
                t.total(),
            ),
            metric("input", "Entrada", t.input),
            metric("output", "Salida", t.output),
            metric("cacheWrite", "Escritura en caché", t.cache_creation),
            metric("cacheRead", "Lectura de caché", t.cache_read),
        ],
    }
}

// --- Proveedor --------------------------------------------------------------

pub struct TokensProvider {
    scanner: Arc<Mutex<Scanner>>,
    shared: Arc<Shared>,
}

impl TokensProvider {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self {
            scanner: Arc::new(Mutex::new(Scanner::new(projects_dir()))),
            shared,
        }
    }
}

impl Provider for TokensProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Tokens
    }

    fn fetch(&self) -> FetchFuture<'_> {
        let scanner = Arc::clone(&self.scanner);
        let session_reset = self.shared.session_reset_ms();
        Box::pin(async move {
            // La lectura de disco es bloqueante: va a un hilo aparte para no
            // frenar el resto de la app.
            tokio::task::spawn_blocking(move || {
                let (since_ms, aligned) = window_start(now_ms(), session_reset);
                let mut scanner = scanner.lock().unwrap_or_else(|p| p.into_inner());
                scanner.scan(since_ms).map(|t| reading(&t, aligned))
            })
            .await
            .unwrap_or_else(|_| Err(ProviderError("error interno al leer los tokens".into())))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        fs::read_to_string(path).expect("fixture")
    }

    fn ingest_all(text: &str) -> Ledger {
        let mut ledger = Ledger::default();
        for line in text.lines() {
            ledger.ingest(line);
        }
        ledger
    }

    /// Carpeta temporal propia de cada test, que se borra al terminar.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("monitor-uso-claude-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("temp");
            Self(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn line(id: &str, ts: &str, input: u64, output: u64) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","requestId":"req_{id}","message":{{"id":"msg_{id}","content":[],"usage":{{"input_tokens":{input},"output_tokens":{output}}}}}}}"#
        )
    }

    #[test]
    fn window_aligns_with_open_plan_session() {
        let hour = 60 * 60 * 1000;
        let now = 100 * hour;
        // Ventana del plan abierta: empieza 5 h antes de su reinicio.
        assert_eq!(
            window_start(now, Some(now + 2 * hour)),
            (now - 3 * hour, true)
        );
        // Cerrada o desconocida: últimas 5 h móviles.
        assert_eq!(window_start(now, Some(now - hour)), (now - 5 * hour, false));
        assert_eq!(window_start(now, None), (now - 5 * hour, false));
    }

    #[test]
    fn basic_fixture_dedupes_streaming_lines() {
        let ledger = ingest_all(&fixture("session_basic.jsonl"));
        assert_eq!(ledger.entries.len(), 3);
        let t = ledger.totals_since(0);
        assert_eq!(
            t,
            TokenTotals {
                input: 31,
                output: 82,
                cache_creation: 100,
                cache_read: 1500,
            }
        );
        assert_eq!(t.total(), 1713);
    }

    #[test]
    fn malformed_fixture_only_counts_valid_lines() {
        let ledger = ingest_all(&fixture("session_malformed.jsonl"));
        let t = ledger.totals_since(0);
        assert_eq!((t.input, t.output), (7, 3));
    }

    #[test]
    fn window_excludes_old_messages() {
        let ledger = ingest_all(&fixture("session_basic.jsonl"));
        // msg_A es de las 10:00:06; msg_B y msg_C, de las 10:05 y 10:06.
        let since = parse_rfc3339_ms("2026-09-20T10:05:00Z").expect("fecha");
        let t = ledger.totals_since(since);
        assert_eq!((t.input, t.output), (21, 32));

        let mut ledger = ledger;
        ledger.prune_before(since);
        assert_eq!(ledger.entries.len(), 2);
    }

    #[test]
    fn non_assistant_and_foreign_lines_ignored() {
        assert!(parse_line(r#"{"type":"user","message":{"usage":{}}}"#).is_none());
        assert!(parse_line(r#"{"type":"assistant","usage":{}}"#).is_none());
        assert!(parse_line("[1,2,3]").is_none());
        assert!(
            parse_line(r#"{"type":"assistant","message":{"usage":{"input_tokens":"x"}}}"#)
                .is_none()
        );
    }

    #[test]
    fn missing_folder_gives_no_data() {
        let mut scanner = Scanner::new(Some(PathBuf::from("/no/existe/esta/carpeta")));
        assert!(scanner.scan(0).is_err());
        let mut scanner = Scanner::new(None);
        assert!(scanner.scan(0).is_err());
    }

    #[test]
    fn incremental_reading_and_partial_lines() {
        let tmp = TempDir::new("incremental");
        let sub = tmp.0.join("proyecto").join("sesion").join("subagents");
        fs::create_dir_all(&sub).expect("subagents");
        let main_file = tmp.0.join("proyecto").join("sesion.jsonl");
        let agent_file = sub.join("agente.jsonl");
        let ts = "2026-09-20T10:00:00Z";

        fs::write(&main_file, format!("{}\n", line("A", ts, 10, 1))).expect("escribir");
        fs::write(&agent_file, format!("{}\n", line("B", ts, 5, 1))).expect("escribir");

        let mut scanner = Scanner::new(Some(tmp.0.clone()));
        assert_eq!(scanner.scan(0).expect("scan").input, 15);

        // Se añade una línea completa y otra a medio escribir.
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&main_file)
            .expect("abrir");
        let partial = line("D", ts, 1000, 1);
        let (first_half, second_half) = partial.split_at(partial.len() / 2);
        write!(f, "{}\n{first_half}", line("C", ts, 100, 1)).expect("añadir");
        assert_eq!(scanner.scan(0).expect("scan").input, 115);

        // Al completarse la línea, se cuenta una sola vez.
        writeln!(f, "{second_half}").expect("completar");
        assert_eq!(scanner.scan(0).expect("scan").input, 1115);
        assert_eq!(scanner.scan(0).expect("scan").input, 1115);
    }

    #[test]
    fn rewritten_file_is_reread_without_double_counting() {
        let tmp = TempDir::new("rewrite");
        let file = tmp.0.join("s.jsonl");
        let ts = "2026-09-20T10:00:00Z";
        fs::write(
            &file,
            format!("{}\n{}\n", line("A", ts, 10, 1), line("B", ts, 20, 1)),
        )
        .expect("escribir");
        let mut scanner = Scanner::new(Some(tmp.0.clone()));
        assert_eq!(scanner.scan(0).expect("scan").input, 30);

        // Más corto que antes: se relee desde el principio.
        fs::write(&file, format!("{}\n", line("A", ts, 10, 1))).expect("reescribir");
        assert_eq!(scanner.scan(0).expect("scan").input, 30);
    }
}
