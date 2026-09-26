//! Proveedor de suscripción: % usado de la sesión de 5 h y del límite semanal.
//!
//! Reutiliza (solo lectura) el token OAuth de Claude Code y hace una petición
//! mínima (`max_tokens: 1`) para leer las cabeceras de límites. No está
//! documentado por Anthropic: es la fuente más frágil. Detalles en
//! `docs/fuentes-datos.md`.
//!
//! El token nunca se registra, se muestra ni sale de este módulo: solo viaja
//! en la cabecera `Authorization` hacia `api.anthropic.com`.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::header::{HeaderMap, CONTENT_TYPE};
use reqwest::{redirect, Client, StatusCode};
use serde::Deserialize;

use super::claude_dir;
use super::{
    now_ms, FetchFuture, Metric, MetricValue, Provider, ProviderError, ProviderId, Reading, Shared,
};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
/// El modelo más barato: la sonda solo necesita las cabeceras.
const PROBE_MODEL: &str = "claude-haiku-4-5-20251001";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

// --- Credenciales -----------------------------------------------------------

/// Token OAuth de Claude Code. Su `Debug` no muestra el valor y no
/// implementa `Display`, para que no pueda acabar en un log por descuido.
struct OAuthToken(String);

impl fmt::Debug for OAuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OAuthToken(<oculto>)")
    }
}

#[derive(Debug)]
struct Credentials {
    token: OAuthToken,
    expires_at_ms: Option<u64>,
}

#[derive(Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    oauth: Option<OAuthEntry>,
}

#[derive(Deserialize)]
struct OAuthEntry {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<f64>,
}

/// Interpreta el JSON de credenciales. Los errores nunca incluyen el
/// contenido, porque el JSON contiene el token.
fn parse_credentials(json: &str) -> Result<Credentials, ProviderError> {
    let file: CredentialsFile = serde_json::from_str(json).map_err(|_| {
        ProviderError("las credenciales de Claude Code no tienen el formato esperado".into())
    })?;
    let oauth = file.oauth.ok_or_else(|| {
        ProviderError("Claude Code no tiene una sesión de claude.ai iniciada".into())
    })?;
    let token = oauth
        .access_token
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            ProviderError("Claude Code no tiene una sesión de claude.ai iniciada".into())
        })?;
    let expires_at_ms = oauth
        .expires_at
        .filter(|e| e.is_finite() && *e > 0.0)
        // Es un número de milisegundos Unix: cabe de sobra en u64.
        .map(|e| e as u64);
    Ok(Credentials {
        token: OAuthToken(token),
        expires_at_ms,
    })
}

/// En macOS, Claude Code guarda las credenciales en el llavero.
#[cfg(target_os = "macos")]
fn read_keychain() -> Option<String> {
    let out = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

/// Lee las credenciales de Claude Code (solo lectura). Bloqueante.
fn read_credentials() -> Result<Credentials, ProviderError> {
    #[cfg(target_os = "macos")]
    if let Some(json) = read_keychain() {
        return parse_credentials(&json);
    }
    let path = claude_dir::config_dir()
        .map(|d| d.join(".credentials.json"))
        .ok_or_else(|| ProviderError("no se encuentra la carpeta de Claude Code".into()))?;
    let json = std::fs::read_to_string(path).map_err(|_| {
        ProviderError(
            "no se encuentran las credenciales de Claude Code (¿has iniciado sesión?)".into(),
        )
    })?;
    parse_credentials(&json)
}

// --- Cabeceras de límites ---------------------------------------------------

/// Uso de una ventana de límite.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Window {
    /// Porcentaje usado, de 0 a 100.
    percent: f64,
    resets_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Limits {
    session: Option<Window>,
    weekly: Option<Window>,
}

/// Convierte las cabeceras `anthropic-ratelimit-unified-<ventana>-*` en
/// porcentaje y hora de reinicio. `get` devuelve el valor de una cabecera.
fn parse_window<'a>(get: &impl Fn(&str) -> Option<&'a str>, name: &str) -> Option<Window> {
    let utilization: f64 = get(&format!("anthropic-ratelimit-unified-{name}-utilization"))?
        .trim()
        .parse()
        .ok()?;
    if !utilization.is_finite() || utilization < 0.0 {
        return None;
    }
    let resets_at_ms = get(&format!("anthropic-ratelimit-unified-{name}-reset"))
        .and_then(|v| v.trim().parse::<u64>().ok())
        .and_then(|secs| secs.checked_mul(1000));
    Some(Window {
        // La cabecera es una fracción (0,42 = 42 %); por encima de 1 se muestra 100 %.
        percent: (utilization * 100.0).min(100.0),
        resets_at_ms,
    })
}

fn parse_limits<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Limits {
    Limits {
        session: parse_window(&get, "5h"),
        weekly: parse_window(&get, "7d"),
    }
}

fn limits_from_headers(headers: &HeaderMap) -> Limits {
    parse_limits(|name| headers.get(name).and_then(|v| v.to_str().ok()))
}

// --- Cuándo sondear ---------------------------------------------------------

/// Decide si toca hacer la sonda. Cualquier petición cuenta como uso, así que
/// sondear con la ventana cerrada abriría una nueva de 5 h. Solo se sondea
/// cuando es seguro que hay una ventana abierta, o si el usuario pulsa
/// "Recargar datos".
///
/// `checked_since` es el arranque de la app o la última sonda, lo que sea
/// más reciente: actividad de Claude Code posterior significa que el usuario
/// acaba de hacer una petición y, por tanto, hay una ventana abierta.
fn should_probe(
    now: u64,
    session_reset: Option<u64>,
    last_activity: Option<u64>,
    checked_since: u64,
    requested: bool,
) -> bool {
    if requested {
        return true;
    }
    match session_reset {
        // Ventana abierta: la sonda no cambia nada.
        Some(reset) if now < reset => true,
        // Ventana cerrada: solo si se ha usado Claude Code desde el cierre.
        Some(reset) => last_activity.is_some_and(|a| a > reset),
        // Reinicio desconocido (arranque, o la respuesta no lo trajo): que
        // hubiera actividad hace poco no garantiza que la ventana siga
        // abierta, así que solo cuenta la actividad desde la última comprobación.
        None => last_activity.is_some_and(|a| a > checked_since),
    }
}

// --- Conversión a métricas --------------------------------------------------

fn reading(limits: &Limits, session_paused: bool, now: u64) -> Reading {
    let mut metrics = Vec::new();
    if session_paused {
        metrics.push(Metric {
            key: "session",
            label: "Sesión 5 h (sin actividad)",
            value: MetricValue::Percent(0.0),
            resets_at_ms: None,
        });
    } else if let Some(w) = limits.session {
        metrics.push(Metric {
            key: "session",
            label: "Sesión 5 h",
            value: MetricValue::Percent(w.percent),
            resets_at_ms: w.resets_at_ms,
        });
    }
    // El dato semanal sigue valiendo hasta su reinicio aunque no se sondee.
    if let Some(w) = limits
        .weekly
        .filter(|w| w.resets_at_ms.is_none_or(|r| r > now))
    {
        metrics.push(Metric {
            key: "weekly",
            label: "Semana",
            value: MetricValue::Percent(w.percent),
            resets_at_ms: w.resets_at_ms,
        });
    }
    Reading { metrics }
}

// --- Sonda ------------------------------------------------------------------

async fn probe(client: &Client, token: &OAuthToken) -> Result<Limits, ProviderError> {
    let body = format!(
        r#"{{"model":"{PROBE_MODEL}","max_tokens":1,"messages":[{{"role":"user","content":"."}}]}}"#
    );
    let response = client
        .post(API_URL)
        // `bearer_auth` marca la cabecera como sensible.
        .bearer_auth(&token.0)
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", "oauth-2025-04-20")
        .header(CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| {
            ProviderError(if e.is_timeout() {
                "tiempo de espera agotado al consultar los límites".into()
            } else {
                "sin conexión con api.anthropic.com".into()
            })
        })?;

    match response.status() {
        // Con 429 (límite alcanzado) las cabeceras siguen llegando.
        StatusCode::OK | StatusCode::TOO_MANY_REQUESTS => {}
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            return Err(ProviderError(
                "la sesión de Claude Code no es válida: abre Claude Code para renovarla".into(),
            ))
        }
        other => {
            return Err(ProviderError(format!(
                "la API respondió con el código {}",
                other.as_u16()
            )))
        }
    }

    let limits = limits_from_headers(response.headers());
    if limits.session.is_none() && limits.weekly.is_none() {
        return Err(ProviderError(
            "la respuesta no trae los límites del plan (puede que haya cambiado)".into(),
        ));
    }
    Ok(limits)
}

// --- Proveedor --------------------------------------------------------------

pub struct SubscriptionProvider {
    client: Option<Client>,
    shared: Arc<Shared>,
    /// Últimos límites conocidos, para mostrar la semana mientras la sonda está en pausa.
    last: Mutex<Option<Limits>>,
    /// Arranque de la app o última sonda (ms Unix), lo más reciente.
    checked_since: AtomicU64,
}

impl SubscriptionProvider {
    pub fn new(shared: Arc<Shared>) -> Self {
        let client = Client::builder()
            .https_only(true)
            // Sin redirecciones: el token no debe viajar a ningún otro sitio.
            .redirect(redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("monitor-uso-claude/", env!("CARGO_PKG_VERSION")))
            .build()
            .ok();
        Self {
            client,
            shared,
            last: Mutex::new(None),
            checked_since: AtomicU64::new(now_ms()),
        }
    }

    fn last(&self) -> Option<Limits> {
        *self.last.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn set_last(&self, limits: Limits) {
        *self.last.lock().unwrap_or_else(|p| p.into_inner()) = Some(limits);
    }
}

impl Provider for SubscriptionProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Subscription
    }

    fn fetch(&self) -> FetchFuture<'_> {
        Box::pin(async move {
            let requested = self.shared.take_probe_request();
            let now = now_ms();
            let last_activity = tokio::task::spawn_blocking(claude_dir::last_activity_ms)
                .await
                .ok()
                .flatten();

            if !should_probe(
                now,
                self.shared.session_reset_ms(),
                last_activity,
                self.checked_since.load(Ordering::Relaxed),
                requested,
            ) {
                return match self.last() {
                    Some(last) => Ok(reading(&last, true, now)),
                    None => Err(ProviderError(
                        "en pausa: no hay actividad reciente de Claude Code. Pulsa ⟳ para consultar".into(),
                    )),
                };
            }

            let client = self
                .client
                .as_ref()
                .ok_or_else(|| ProviderError("no se pudo preparar la conexión HTTPS".into()))?;
            let credentials = tokio::task::spawn_blocking(read_credentials)
                .await
                .unwrap_or_else(|_| {
                    Err(ProviderError(
                        "error interno al leer las credenciales".into(),
                    ))
                })?;
            if credentials.expires_at_ms.is_some_and(|e| e <= now) {
                return Err(ProviderError(
                    "la sesión de Claude Code ha caducado: abre Claude Code para renovarla".into(),
                ));
            }

            self.checked_since.store(now, Ordering::Relaxed);
            let limits = probe(client, &credentials.token).await?;
            self.shared
                .set_session_reset_ms(limits.session.and_then(|w| w.resets_at_ms));
            self.set_last(limits);
            Ok(reading(&limits, false, now))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const HOUR: u64 = 60 * 60 * 1000;

    fn fixture_headers() -> HashMap<String, String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ratelimit_headers.txt");
        std::fs::read_to_string(path)
            .expect("fixture")
            .lines()
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect()
    }

    fn limits_from(map: &HashMap<String, String>) -> Limits {
        parse_limits(|name| map.get(name).map(String::as_str))
    }

    #[test]
    fn fixture_headers_to_percent_and_reset() {
        let limits = limits_from(&fixture_headers());
        let session = limits.session.expect("sesión");
        let weekly = limits.weekly.expect("semana");
        assert!((session.percent - 42.0).abs() < 1e-9);
        assert!((weekly.percent - 61.0).abs() < 1e-9);
        assert_eq!(session.resets_at_ms, Some(1_790_000_000_000));
        assert_eq!(weekly.resets_at_ms, Some(1_790_500_000_000));
    }

    #[test]
    fn missing_or_invalid_headers() {
        let mut map = fixture_headers();
        map.remove("anthropic-ratelimit-unified-7d-utilization");
        map.insert(
            "anthropic-ratelimit-unified-5h-reset".into(),
            "mañana".into(),
        );
        let limits = limits_from(&map);
        assert!(limits.weekly.is_none(), "sin utilización no hay semana");
        let session = limits.session.expect("sesión");
        assert_eq!(session.resets_at_ms, None, "reinicio ilegible");

        for bad in ["-0.1", "NaN", "abc", ""] {
            map.insert(
                "anthropic-ratelimit-unified-5h-utilization".into(),
                bad.into(),
            );
            assert!(limits_from(&map).session.is_none(), "{bad:?}");
        }
        assert_eq!(limits_from(&HashMap::new()), Limits::default());
    }

    #[test]
    fn utilization_edges() {
        let mut map = HashMap::new();
        for (raw, expected) in [("0", 0.0), ("1", 100.0), ("1.3", 100.0), (" 0.5 ", 50.0)] {
            map.insert(
                "anthropic-ratelimit-unified-5h-utilization".to_string(),
                raw.to_string(),
            );
            let p = limits_from(&map).session.expect("sesión").percent;
            assert!((p - expected).abs() < 1e-9, "{raw} → {p}");
        }
    }

    #[test]
    fn credentials_parsing() {
        let c = parse_credentials(
            r#"{"claudeAiOauth":{"accessToken":"<OAUTH_TOKEN>","refreshToken":"x","expiresAt":1790000000000,"scopes":[]}}"#,
        )
        .expect("credenciales");
        assert_eq!(c.token.0, "<OAUTH_TOKEN>");
        assert_eq!(c.expires_at_ms, Some(1_790_000_000_000));

        for bad in [
            "",
            "{}",
            r#"{"claudeAiOauth":{}}"#,
            r#"{"claudeAiOauth":{"accessToken":""}}"#,
            "no es json",
        ] {
            assert!(parse_credentials(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn errors_and_debug_never_show_the_token() {
        let c = parse_credentials(r#"{"claudeAiOauth":{"accessToken":"<OAUTH_TOKEN>"}}"#)
            .expect("credenciales");
        assert!(!format!("{c:?}").contains("<OAUTH_TOKEN>"));

        // Un tipo erróneo hace fallar a serde con un mensaje que podría citar el
        // valor; el error propio no debe incluirlo.
        let err = parse_credentials(
            r#"{"claudeAiOauth":{"accessToken":"<OAUTH_TOKEN>","expiresAt":"<OAUTH_TOKEN>"}}"#,
        )
        .expect_err("tipo erróneo");
        assert!(!err.0.contains("<OAUTH_TOKEN>"));
    }

    #[test]
    fn probe_decision() {
        let now = 100 * HOUR;
        let since = now - 10 * 60 * 1000; // arranque hace 10 min
                                          // Petición manual: siempre.
        assert!(should_probe(now, None, None, since, true));
        // Ventana abierta: siempre.
        assert!(should_probe(now, Some(now + HOUR), None, since, false));
        // Ventana cerrada sin actividad posterior: pausa.
        assert!(!should_probe(
            now,
            Some(now - HOUR),
            Some(now - 2 * HOUR),
            since,
            false
        ));
        assert!(!should_probe(now, Some(now - HOUR), None, since, false));
        // Ventana cerrada con actividad posterior: sonda.
        assert!(should_probe(
            now,
            Some(now - HOUR),
            Some(now - HOUR / 2),
            since,
            false
        ));
        // Reinicio desconocido: solo con actividad desde la última comprobación.
        assert!(should_probe(now, None, Some(now - 60_000), since, false));
        assert!(!should_probe(now, None, None, since, false));
    }

    #[test]
    fn unknown_reset_ignores_older_activity() {
        // Caso límite: ventana abierta a las 08:00 (cierra a las 13:00),
        // última actividad a las 12:30 y la app arranca a las 14:00. Aunque
        // la actividad fue hace menos de 5 h, sondear abriría otra ventana.
        let at = |h: u64, m: u64| (h * 60 + m) * 60 * 1000;
        assert!(!should_probe(
            at(14, 0),
            None,
            Some(at(12, 30)),
            at(14, 0),
            false
        ));
        // Si una respuesta no trae el reinicio, tampoco se repite la sonda sin
        // actividad nueva desde la anterior.
        assert!(!should_probe(
            at(16, 0),
            None,
            Some(at(14, 5)),
            at(15, 57),
            false
        ));
        assert!(should_probe(
            at(16, 0),
            None,
            Some(at(15, 58)),
            at(15, 57),
            false
        ));
    }

    #[test]
    fn paused_reading_keeps_valid_weekly() {
        let now = 100 * HOUR;
        let limits = Limits {
            session: Some(Window {
                percent: 80.0,
                resets_at_ms: Some(now - HOUR),
            }),
            weekly: Some(Window {
                percent: 61.0,
                resets_at_ms: Some(now + 24 * HOUR),
            }),
        };
        let r = reading(&limits, true, now);
        let session = r.metric("session").expect("sesión");
        assert_eq!(session.value, MetricValue::Percent(0.0));
        assert_eq!(session.resets_at_ms, None);
        assert_eq!(
            r.metric("weekly").expect("semana").value,
            MetricValue::Percent(61.0)
        );

        // Si la semana también se reinició, se deja de mostrar.
        let r = reading(&limits, true, now + 48 * HOUR);
        assert!(r.metric("weekly").is_none());
    }

    #[test]
    fn active_reading_uses_probe_values() {
        let now = 100 * HOUR;
        let limits = Limits {
            session: Some(Window {
                percent: 42.0,
                resets_at_ms: Some(now + HOUR),
            }),
            weekly: None,
        };
        let r = reading(&limits, false, now);
        let session = r.metric("session").expect("sesión");
        assert_eq!(session.value, MetricValue::Percent(42.0));
        assert_eq!(session.resets_at_ms, Some(now + HOUR));
        assert!(r.metric("weekly").is_none());
    }
}
