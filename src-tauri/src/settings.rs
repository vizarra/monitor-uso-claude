//! Ajustes de la app (sin secretos) y estado que conviene recordar entre
//! arranques. Se guardan como JSON en la carpeta de configuración del SO
//! (`%APPDATA%`, `~/Library/Application Support`, `~/.config`), en una
//! subcarpeta propia; nunca en `~/.claude`.
//!
//! Un archivo ausente o dañado nunca impide arrancar: se usan los valores
//! por defecto.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::alerts::Fired;
use crate::scheduler::{clamp_interval, DEFAULT_INTERVAL};

const APP_DIR: &str = "monitor-uso-claude";
const SETTINGS_FILE: &str = "settings.json";
const STATE_FILE: &str = "state.json";
/// Intervalo máximo: con más de una hora los datos dejan de ser útiles.
const MAX_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Número máximo de umbrales de alerta.
const MAX_THRESHOLDS: usize = 3;

/// Ajustes que el usuario cambia desde la ventana.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Segundos entre sondeos (60 como mínimo).
    pub interval_secs: u64,
    pub show_subscription: bool,
    pub show_tokens: bool,
    pub show_api: bool,
    /// Muestra el porcentaje en el centro del icono de la bandeja.
    pub show_percent_in_icon: bool,
    /// Porcentajes (1–100) a los que se avisa con una notificación; vacío = sin alertas.
    pub alert_thresholds: Vec<u8>,
    /// Arrancar la app al iniciar sesión en el sistema.
    pub launch_at_login: bool,
    /// Mini-widget flotante con el anillo, para pantallas sin bandeja.
    pub show_widget: bool,
    /// El usuario ha cerrado el aviso de cómo fijar el icono (Windows).
    pub pin_hint_dismissed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            interval_secs: DEFAULT_INTERVAL.as_secs(),
            show_subscription: true,
            show_tokens: true,
            show_api: true,
            show_percent_in_icon: false,
            alert_thresholds: vec![80, 95],
            launch_at_login: false,
            show_widget: false,
            pin_hint_dismissed: false,
        }
    }
}

impl Settings {
    /// Corrige valores fuera de rango (p. ej. editados a mano en el JSON).
    pub fn normalized(mut self) -> Self {
        self.interval_secs = self.interval().as_secs();
        self.alert_thresholds.retain(|t| (1..=100).contains(t));
        self.alert_thresholds.sort_unstable();
        self.alert_thresholds.dedup();
        self.alert_thresholds.truncate(MAX_THRESHOLDS);
        self
    }

    pub fn interval(&self) -> Duration {
        clamp_interval(Duration::from_secs(self.interval_secs)).min(MAX_INTERVAL)
    }
}

/// Estado que no es un ajuste pero conviene conservar al reiniciar la app.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PersistedState {
    /// Último reinicio conocido de la ventana de 5 h (ms Unix). Permite saber
    /// al arrancar si la ventana sigue abierta sin tener que sondear.
    pub last_session_reset_ms: Option<u64>,
    /// Alertas ya mostradas, para no repetirlas al reiniciar la app.
    pub fired_alerts: Vec<Fired>,
    /// Última posición del mini-widget (píxeles físicos).
    pub widget_position: Option<(i32, i32)>,
}

/// Lectura y escritura de los archivos de la app.
pub struct Store {
    dir: Option<PathBuf>,
}

impl Store {
    pub fn new() -> Self {
        Self {
            dir: dirs::config_dir().map(|d| d.join(APP_DIR)),
        }
    }

    #[cfg(test)]
    fn at(dir: PathBuf) -> Self {
        Self { dir: Some(dir) }
    }

    pub fn load_settings(&self) -> Settings {
        self.load::<Settings>(SETTINGS_FILE)
            .unwrap_or_default()
            .normalized()
    }

    pub fn save_settings(&self, settings: &Settings) -> io::Result<()> {
        self.save(SETTINGS_FILE, settings)
    }

    pub fn load_state(&self) -> PersistedState {
        self.load(STATE_FILE).unwrap_or_default()
    }

    pub fn save_state(&self, state: &PersistedState) -> io::Result<()> {
        self.save(STATE_FILE, state)
    }

    fn load<T: for<'de> Deserialize<'de>>(&self, name: &str) -> Option<T> {
        let text = fs::read_to_string(self.dir.as_ref()?.join(name)).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn save<T: Serialize>(&self, name: &str, value: &T) -> io::Result<()> {
        let dir = self.dir.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "sin carpeta de configuración")
        })?;
        let json = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
        write_atomic(dir, name, &json)
    }
}

/// Escribe en un archivo temporal y lo renombra, para que un cierre a mitad
/// de escritura nunca deje un JSON cortado.
fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{name}.tmp"));
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, dir.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "monitor-uso-claude-settings-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn defaults_when_missing() {
        let tmp = TempDir::new("missing");
        let store = Store::at(tmp.0.clone());
        assert_eq!(store.load_settings(), Settings::default());
        assert_eq!(store.load_state(), PersistedState::default());
        assert_eq!(Settings::default().interval_secs, 180);
        assert!(!Settings::default().show_percent_in_icon);
    }

    #[test]
    fn roundtrip() {
        let tmp = TempDir::new("roundtrip");
        let store = Store::at(tmp.0.clone());
        let settings = Settings {
            interval_secs: 300,
            show_api: false,
            show_percent_in_icon: true,
            ..Settings::default()
        };
        store.save_settings(&settings).expect("guardar");
        assert_eq!(store.load_settings(), settings);

        let state = PersistedState {
            last_session_reset_ms: Some(1_790_000_000_000),
            widget_position: Some((-100, 200)),
            ..PersistedState::default()
        };
        store.save_state(&state).expect("guardar");
        assert_eq!(store.load_state(), state);
        assert!(!tmp.0.join("settings.json.tmp").exists());
    }

    #[test]
    fn corrupt_or_partial_files() {
        let tmp = TempDir::new("corrupt");
        fs::create_dir_all(&tmp.0).expect("dir");
        let store = Store::at(tmp.0.clone());

        fs::write(tmp.0.join(SETTINGS_FILE), "{ esto no es json").expect("escribir");
        assert_eq!(store.load_settings(), Settings::default());

        // Campos que faltan toman su valor por defecto; los desconocidos se ignoran.
        fs::write(
            tmp.0.join(SETTINGS_FILE),
            r#"{"showTokens": false, "campoFuturo": 1}"#,
        )
        .expect("escribir");
        let s = store.load_settings();
        assert!(!s.show_tokens);
        assert_eq!(s.interval_secs, 180);
    }

    #[test]
    fn thresholds_are_normalized() {
        let norm = |t: Vec<u8>| {
            Settings {
                alert_thresholds: t,
                ..Settings::default()
            }
            .normalized()
            .alert_thresholds
        };
        assert_eq!(Settings::default().alert_thresholds, vec![80, 95]);
        assert_eq!(norm(vec![95, 80, 80, 0, 101]), vec![80, 95]);
        assert_eq!(norm(vec![10, 20, 30, 40]), vec![10, 20, 30]);
        assert_eq!(norm(vec![]), Vec::<u8>::new());
    }

    #[test]
    fn interval_is_clamped() {
        let clamp = |secs| {
            Settings {
                interval_secs: secs,
                ..Settings::default()
            }
            .normalized()
            .interval_secs
        };
        assert_eq!(clamp(0), 60);
        assert_eq!(clamp(59), 60);
        assert_eq!(clamp(60), 60);
        assert_eq!(clamp(600), 600);
        assert_eq!(clamp(100_000), 3600);
    }
}
