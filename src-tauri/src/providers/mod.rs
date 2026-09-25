//! Trait `Provider` y tipos comunes a todas las fuentes de datos.
//!
//! Cada fuente es independiente: si una falla, su sección pasa a "sin datos"
//! y las demás siguen funcionando.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

pub mod api;
pub mod claude_dir;
pub mod subscription;
pub mod time;
pub mod tokens;

/// Duración de la ventana de sesión del plan: 5 h.
pub const SESSION_WINDOW_MS: u64 = 5 * 60 * 60 * 1000;

/// Estado que comparten los proveedores y la app. Usa atómicos (valores que
/// varios hilos leen y escriben sin cerrojo) porque son datos sueltos.
#[derive(Default)]
pub struct Shared {
    /// Reinicio de la ventana de 5 h según la última sonda (ms Unix; 0 = desconocido).
    session_reset_ms: AtomicU64,
    /// El usuario ha pulsado "Actualizar": la próxima sonda se hace aunque esté en pausa.
    probe_requested: AtomicBool,
}

impl Shared {
    pub fn session_reset_ms(&self) -> Option<u64> {
        match self.session_reset_ms.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(ms),
        }
    }

    pub fn set_session_reset_ms(&self, reset: Option<u64>) {
        self.session_reset_ms
            .store(reset.unwrap_or(0), Ordering::Relaxed);
    }

    pub fn request_probe(&self) {
        self.probe_requested.store(true, Ordering::Relaxed);
    }

    /// Devuelve si había una petición manual y la consume.
    pub fn take_probe_request(&self) -> bool {
        self.probe_requested.swap(false, Ordering::Relaxed)
    }
}

/// Identificador de cada fuente. También es la clave de su sección en la ventana.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderId {
    Subscription,
    Tokens,
    Api,
}

/// Valor de una métrica, con su unidad.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum MetricValue {
    /// Porcentaje de 0 a 100.
    Percent(f64),
    /// Recuento de tokens.
    Tokens(u64),
    /// Importe en centavos de dólar.
    UsdCents(f64),
}

/// Una línea del desglose (p. ej. "Sesión 5 h: 42 %").
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metric {
    /// Clave estable para el código (p. ej. `session`).
    pub key: &'static str,
    /// Texto visible en la ventana.
    pub label: &'static str,
    pub value: MetricValue,
    /// Momento de reinicio, en milisegundos Unix, si la métrica lo tiene.
    pub resets_at_ms: Option<u64>,
}

/// Lo que devuelve un proveedor cuando consigue datos.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub metrics: Vec<Metric>,
}

impl Reading {
    /// Busca una métrica por su clave.
    pub fn metric(&self, key: &str) -> Option<&Metric> {
        self.metrics.iter().find(|m| m.key == key)
    }
}

/// Error de un proveedor. El mensaje se muestra al usuario, así que nunca
/// debe incluir tokens, claves ni contenido de conversaciones.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderError(pub String);

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProviderError {}

/// Estado de una sección tal y como lo ve la ventana.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum SectionState {
    /// Aún no se ha consultado nunca.
    Pending,
    /// Última consulta correcta.
    #[serde(rename_all = "camelCase")]
    Ok {
        reading: Reading,
        updated_at_ms: u64,
    },
    /// La última consulta falló: la ventana muestra "sin datos" y el motivo.
    #[serde(rename_all = "camelCase")]
    NoData { reason: String, updated_at_ms: u64 },
}

impl SectionState {
    /// Convierte el resultado de una consulta en el estado de su sección.
    pub fn from_result(result: Result<Reading, ProviderError>, now_ms: u64) -> Self {
        match result {
            Ok(reading) => Self::Ok {
                reading,
                updated_at_ms: now_ms,
            },
            Err(e) => Self::NoData {
                reason: e.0,
                updated_at_ms: now_ms,
            },
        }
    }
}

/// Futuro que devuelve `fetch`. Va en una `Box` para que el trait pueda
/// usarse como `dyn Provider` (los `async fn` en traits aún no lo permiten).
pub type FetchFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Reading, ProviderError>> + Send + 'a>>;

/// Una fuente de datos que el scheduler consulta periódicamente.
pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn fetch(&self) -> FetchFuture<'_>;
}

/// Hora actual en milisegundos Unix. Si el reloj del sistema está antes de
/// 1970 (no debería pasar), devuelve 0 en lugar de fallar.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_becomes_no_data_with_reason() {
        let state = SectionState::from_result(Err(ProviderError("sin red".into())), 5);
        assert_eq!(
            state,
            SectionState::NoData {
                reason: "sin red".into(),
                updated_at_ms: 5
            }
        );
    }

    #[test]
    fn state_serializes_with_status_tag() {
        let reading = Reading {
            metrics: vec![Metric {
                key: "session",
                label: "Sesión 5 h",
                value: MetricValue::Percent(42.0),
                resets_at_ms: None,
            }],
        };
        let json =
            serde_json::to_value(SectionState::from_result(Ok(reading), 7)).expect("serializa");
        assert_eq!(json["status"], "ok");
        assert_eq!(json["updatedAtMs"], 7);
        assert_eq!(json["reading"]["metrics"][0]["value"]["kind"], "percent");
        assert_eq!(json["reading"]["metrics"][0]["value"]["value"], 42.0);
    }
}
