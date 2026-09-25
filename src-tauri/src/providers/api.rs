//! Proveedor de la API de pago por uso: tokens y coste del mes en curso, vía
//! la Admin API oficial (`usage_report/messages` y `cost_report`). Detalles
//! en `docs/fuentes-datos.md`.
//!
//! Solo funciona con una Admin key, que solo existe en cuentas de
//! organización. Sin clave, la sección aparece desactivada.

use std::time::Duration;

use reqwest::header::HeaderValue;
use reqwest::{redirect, Client, StatusCode};
use serde::Deserialize;

use super::time::month_start_rfc3339;
use super::{
    now_ms, FetchFuture, Metric, MetricValue, Provider, ProviderError, ProviderId, Reading,
};
use crate::secrets::{self, AdminKey};

const BASE_URL: &str = "https://api.anthropic.com/v1/organizations";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Con buckets diarios y `limit=31`, un mes cabe en una página; el tope evita
/// bucles si la API devolviera `has_more` sin fin.
const MAX_PAGES: usize = 5;

// --- Respuestas -------------------------------------------------------------

#[derive(Deserialize)]
struct Page<T> {
    data: Vec<Bucket<T>>,
    #[serde(default)]
    has_more: bool,
    next_page: Option<String>,
}

#[derive(Deserialize)]
struct Bucket<T> {
    results: Vec<T>,
}

#[derive(Deserialize)]
struct UsageRow {
    uncached_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation: Option<CacheCreation>,
}

#[derive(Deserialize)]
struct CacheCreation {
    ephemeral_5m_input_tokens: Option<u64>,
    ephemeral_1h_input_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct CostRow {
    /// Centavos como texto decimal: "123.45" son 1,2345 USD.
    amount: String,
    currency: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct UsageTotals {
    input: u64,
    output: u64,
    cache_creation: u64,
    cache_read: u64,
}

impl UsageTotals {
    fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_creation)
            .saturating_add(self.cache_read)
    }
}

fn add_usage(totals: &mut UsageTotals, page: &Page<UsageRow>) {
    for row in page.data.iter().flat_map(|b| &b.results) {
        totals.input = totals
            .input
            .saturating_add(row.uncached_input_tokens.unwrap_or(0));
        totals.output = totals.output.saturating_add(row.output_tokens.unwrap_or(0));
        totals.cache_read = totals
            .cache_read
            .saturating_add(row.cache_read_input_tokens.unwrap_or(0));
        if let Some(c) = &row.cache_creation {
            totals.cache_creation = totals
                .cache_creation
                .saturating_add(c.ephemeral_5m_input_tokens.unwrap_or(0))
                .saturating_add(c.ephemeral_1h_input_tokens.unwrap_or(0));
        }
    }
}

/// Suma los importes en centavos. Ignora los que no son USD o no se pueden leer.
fn add_cost(cents: &mut f64, page: &Page<CostRow>) {
    for row in page.data.iter().flat_map(|b| &b.results) {
        if row.currency.as_deref().is_some_and(|c| c != "USD") {
            continue;
        }
        if let Ok(v) = row.amount.trim().parse::<f64>() {
            if v.is_finite() {
                *cents += v;
            }
        }
    }
}

// --- Peticiones -------------------------------------------------------------

/// Codifica un valor para la query string (solo deja sin tocar los caracteres
/// no reservados de RFC 3986).
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn status_error(status: StatusCode) -> ProviderError {
    ProviderError(match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            "la Admin key no es válida o no tiene permisos".into()
        }
        StatusCode::TOO_MANY_REQUESTS => "la API ha limitado las peticiones; se reintentará".into(),
        other => format!("la API respondió con el código {}", other.as_u16()),
    })
}

/// Pide todas las páginas de un informe y las pasa a `add`.
async fn fetch_report<T: for<'de> Deserialize<'de>>(
    client: &Client,
    key: &AdminKey,
    path: &str,
    starting_at: &str,
    mut add: impl FnMut(&Page<T>),
) -> Result<(), ProviderError> {
    // Cabecera marcada como sensible: reqwest no la muestra en su `Debug`.
    let mut key_header = HeaderValue::from_str(key.expose())
        .map_err(|_| ProviderError("la Admin key guardada no es válida".into()))?;
    key_header.set_sensitive(true);

    let mut page_token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut url = format!(
            "{BASE_URL}/{path}?starting_at={}&bucket_width=1d&limit=31",
            encode(starting_at)
        );
        if let Some(token) = &page_token {
            url.push_str(&format!("&page={}", encode(token)));
        }
        let response = client
            .get(url)
            .header("x-api-key", key_header.clone())
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
            .map_err(|e| {
                ProviderError(if e.is_timeout() {
                    "tiempo de espera agotado al consultar la API".into()
                } else {
                    "sin conexión con api.anthropic.com".into()
                })
            })?;
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| ProviderError("respuesta de la API incompleta".into()))?;
        let page: Page<T> = serde_json::from_slice(&bytes)
            .map_err(|_| ProviderError("respuesta de la API con un formato inesperado".into()))?;
        add(&page);
        match page.next_page {
            Some(next) if page.has_more => page_token = Some(next),
            _ => return Ok(()),
        }
    }
    Ok(())
}

fn reading(usage: &UsageTotals, cost_cents: f64) -> Reading {
    let tokens = |key, label, value| Metric {
        key,
        label,
        value: MetricValue::Tokens(value),
        resets_at_ms: None,
    };
    Reading {
        metrics: vec![
            Metric {
                key: "cost",
                label: "Coste del mes",
                value: MetricValue::UsdCents(cost_cents),
                resets_at_ms: None,
            },
            tokens("total", "Tokens del mes", usage.total()),
            tokens("input", "Entrada", usage.input),
            tokens("output", "Salida", usage.output),
            tokens(
                "cache",
                "Caché (escritura + lectura)",
                usage.cache_creation.saturating_add(usage.cache_read),
            ),
        ],
    }
}

// --- Proveedor --------------------------------------------------------------

pub struct ApiProvider {
    client: Option<Client>,
}

impl ApiProvider {
    pub fn new() -> Self {
        let client = Client::builder()
            .https_only(true)
            // Sin redirecciones: la clave no debe viajar a ningún otro sitio.
            .redirect(redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("monitor-uso-claude/", env!("CARGO_PKG_VERSION")))
            .build()
            .ok();
        Self { client }
    }
}

impl Provider for ApiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Api
    }

    fn fetch(&self) -> FetchFuture<'_> {
        Box::pin(async move {
            let key = tokio::task::spawn_blocking(secrets::load_admin_key)
                .await
                .map_err(|_| ProviderError("error interno al leer el llavero".into()))?
                .map_err(|e| ProviderError(e.0))?
                .ok_or_else(|| {
                    ProviderError(
                        "desactivada: añade una Admin key en Ajustes (solo cuentas de organización)"
                            .into(),
                    )
                })?;
            let client = self
                .client
                .as_ref()
                .ok_or_else(|| ProviderError("no se pudo preparar la conexión HTTPS".into()))?;

            let start = month_start_rfc3339(now_ms());
            let mut usage = UsageTotals::default();
            fetch_report(client, &key, "usage_report/messages", &start, |p| {
                add_usage(&mut usage, p)
            })
            .await?;
            let mut cost_cents = 0.0;
            fetch_report(client, &key, "cost_report", &start, |p| {
                add_cost(&mut cost_cents, p)
            })
            .await?;
            Ok(reading(&usage, cost_cents))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        std::fs::read_to_string(path).expect("fixture")
    }

    #[test]
    fn usage_fixture_totals() {
        let page: Page<UsageRow> =
            serde_json::from_str(&fixture("usage_report.json")).expect("json");
        let mut t = UsageTotals::default();
        add_usage(&mut t, &page);
        assert_eq!(
            t,
            UsageTotals {
                input: 2000,
                output: 750,
                cache_creation: 400,
                cache_read: 200,
            }
        );
        assert_eq!(t.total(), 3350);
        assert!(!page.has_more);
    }

    #[test]
    fn cost_fixture_totals() {
        let page: Page<CostRow> = serde_json::from_str(&fixture("cost_report.json")).expect("json");
        let mut cents = 0.0;
        add_cost(&mut cents, &page);
        assert!((cents - 200.0).abs() < 1e-9, "{cents}");
    }

    #[test]
    fn cost_skips_bad_amounts_and_other_currencies() {
        let json = r#"{"data":[{"results":[
            {"amount":"10.5","currency":"USD"},
            {"amount":"abc","currency":"USD"},
            {"amount":"99","currency":"EUR"},
            {"amount":"0.25"}
        ]}],"has_more":false,"next_page":null}"#;
        let page: Page<CostRow> = serde_json::from_str(json).expect("json");
        let mut cents = 0.0;
        add_cost(&mut cents, &page);
        assert!((cents - 10.75).abs() < 1e-9, "{cents}");
    }

    #[test]
    fn missing_fields_count_as_zero() {
        let json = r#"{"data":[{"results":[{"output_tokens":5}]}]}"#;
        let page: Page<UsageRow> = serde_json::from_str(json).expect("json");
        let mut t = UsageTotals::default();
        add_usage(&mut t, &page);
        assert_eq!(t.total(), 5);
    }

    #[test]
    fn query_encoding() {
        assert_eq!(encode("2026-09-01T00:00:00Z"), "2026-09-01T00%3A00%3A00Z");
        assert_eq!(encode("page_MjAy+/="), "page_MjAy%2B%2F%3D");
    }

    #[test]
    fn reading_has_cost_first() {
        let r = reading(&UsageTotals::default(), 123.45);
        assert_eq!(r.metrics[0].key, "cost");
        assert_eq!(r.metrics[0].value, MetricValue::UsdCents(123.45));
    }
}
