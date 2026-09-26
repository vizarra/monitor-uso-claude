//! Arranque de Tauri, icono de la bandeja, ventana de detalle y comandos IPC.

mod alerts;
mod icon;
mod providers;
mod scheduler;
mod secrets;
mod settings;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, RunEvent, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as _};
use tauri_plugin_notification::NotificationExt as _;
use tauri_plugin_updater::{Update, UpdaterExt as _};
use tokio::sync::Notify;

use alerts::Limit;
use icon::IconValue;
use providers::api::ApiProvider;
use providers::subscription::SubscriptionProvider;
use providers::tokens::TokensProvider;
use providers::{now_ms, MetricValue, Provider, ProviderId, SectionState, Shared};
use scheduler::{IntervalHandle, Scheduler, Snapshot};
use settings::{PersistedState, Settings, Store};

const TRAY_ID: &str = "main";
const WINDOW_LABEL: &str = "main";
const WIDGET_LABEL: &str = "widget";
/// Lado del mini-widget en píxeles lógicos.
const WIDGET_SIZE: f64 = 64.0;
/// Evento que recibe la ventana cada vez que hay datos nuevos.
const SNAPSHOT_EVENT: &str = "snapshot-updated";
/// Evento que recibe la ventana cuando cambian los ajustes.
const SETTINGS_EVENT: &str = "settings-updated";
/// Si la ventana se acaba de ocultar al perder el foco (porque se hizo clic
/// en el icono), ese mismo clic no debe volver a abrirla.
const REOPEN_GUARD: Duration = Duration::from_millis(300);
/// Espera antes de cerrar la ventana al perder el foco, para ignorar los
/// cambios de foco internos de WebView2.
const BLUR_GRACE: Duration = Duration::from_millis(150);

struct AppState {
    snapshot: Mutex<Snapshot>,
    refresh: Arc<Notify>,
    shared: Arc<Shared>,
    last_hidden: Mutex<Option<Instant>>,
    /// Posición de la ventana de detalle al cerrarla, para reabrirla ahí
    /// cuando no se abre desde el icono (menú "Abrir" o widget).
    last_main_position: Mutex<Option<PhysicalPosition<i32>>>,
    settings: Mutex<Settings>,
    store: Store,
    interval: IntervalHandle,
    /// Estado que se guarda en disco (último reinicio, alertas avisadas,
    /// posición del widget). Se escribe solo cuando cambia.
    persisted: Mutex<PersistedState>,
    /// Tamaño del icono en píxeles, calculado al arrancar.
    icon_size: AtomicU32,
    /// Actualización encontrada con "Buscar actualizaciones", a la espera
    /// de que el usuario pulse "Instalar".
    pending_update: Mutex<Option<Update>>,
}

/// Bloquea un mutex aunque otro hilo haya entrado en pánico con él tomado:
/// el dato sigue siendo válido para leerlo o sobrescribirlo.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[tauri::command]
fn get_snapshot(state: tauri::State<'_, AppState>) -> Snapshot {
    lock(&state.snapshot).clone()
}

#[tauri::command]
fn refresh_now(state: tauri::State<'_, AppState>) {
    request_refresh(&state);
}

#[tauri::command]
fn get_settings(state: tauri::State<'_, AppState>) -> Settings {
    lock(&state.settings).clone()
}

/// Guarda y aplica los ajustes al momento: intervalo, icono y ventana.
///
/// Es `async` a propósito: en Windows, crear una ventana (el widget) desde
/// un comando síncrono bloquea el hilo principal y cuelga la app.
#[tauri::command]
async fn save_settings(app: AppHandle, settings: Settings) -> Result<Settings, String> {
    let state = app.state::<AppState>();
    let settings = settings.normalized();
    let previous = lock(&state.settings).clone();
    if settings.launch_at_login != previous.launch_at_login {
        apply_autostart(&app, settings.launch_at_login)?;
    }
    state
        .store
        .save_settings(&settings)
        .map_err(|_| "no se pudieron guardar los ajustes".to_string())?;
    state.interval.set(settings.interval());
    *lock(&state.settings) = settings.clone();
    let snapshot = lock(&state.snapshot).clone();
    update_tray(&app, &snapshot);
    sync_widget(&app, settings.show_widget);
    let _ = app.emit(SETTINGS_EVENT, &settings);
    Ok(settings)
}

fn apply_autostart(app: &AppHandle, enable: bool) -> Result<(), String> {
    let launcher = app.autolaunch();
    let result = if enable {
        launcher.enable()
    } else {
        launcher.disable()
    };
    result.map_err(|_| "no se pudo cambiar el arranque con el sistema".to_string())
}

/// El aviso para fijar el icono solo tiene sentido en Windows, que esconde
/// los iconos nuevos en el desplegable de la bandeja.
#[tauri::command]
fn pin_hint_visible(state: tauri::State<'_, AppState>) -> bool {
    cfg!(target_os = "windows") && !lock(&state.settings).pin_hint_dismissed
}

/// Abre la página de Windows donde se eligen los iconos visibles de la bandeja.
#[tauri::command]
fn open_taskbar_settings() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer.exe")
            .arg("ms-settings:taskbar")
            .spawn()
            .map(|_| ())
            .map_err(|_| "no se pudo abrir la configuración de la barra de tareas".to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("solo disponible en Windows".to_string())
    }
}

/// Dirección del repo público, enlazada desde "Acerca de".
const REPO_URL: &str = "https://github.com/vizarra/monitor-uso-claude";

/// Abre el repo en el navegador. La URL es fija: el frontend no puede elegirla.
#[tauri::command]
fn open_repo() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let program = "explorer.exe";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let program = "xdg-open";

    std::process::Command::new(program)
        .arg(REPO_URL)
        .spawn()
        .map(|_| ())
        .map_err(|_| "no se pudo abrir el navegador".to_string())
}

/// Busca una versión nueva en las releases de GitHub. Solo se llama al
/// pulsar el botón: la app nunca lo comprueba por su cuenta. Devuelve la
/// versión nueva o `None` si ya está al día.
#[tauri::command]
async fn check_update(app: AppHandle) -> Result<Option<String>, String> {
    let update = app
        .updater()
        .map_err(|_| "el actualizador no está disponible".to_string())?
        .check()
        .await
        .map_err(|_| "no se pudo comprobar si hay actualizaciones".to_string())?;
    let version = update.as_ref().map(|u| u.version.clone());
    *lock(&app.state::<AppState>().pending_update) = update;
    Ok(version)
}

/// Descarga e instala la versión encontrada por `check_update` (la firma se
/// verifica con la clave pública de tauri.conf.json) y reinicia la app. En
/// Windows el instalador cierra la app por su cuenta antes de reiniciar.
#[tauri::command]
async fn install_update(app: AppHandle) -> Result<(), String> {
    let update = lock(&app.state::<AppState>().pending_update)
        .take()
        .ok_or_else(|| "primero busca actualizaciones".to_string())?;
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|_| "no se pudo instalar la actualización".to_string())?;
    app.restart()
}

/// El mini-widget abre la ventana de detalle al hacer clic. Es `async` por
/// el mismo motivo que `save_settings`: puede tener que crear la ventana.
#[tauri::command]
async fn open_main_window(app: AppHandle) {
    show_window(&app, None);
}

/// Solo informa de si hay clave: ningún comando devuelve su valor.
#[tauri::command]
async fn has_admin_key() -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(|| secrets::load_admin_key().map(|k| k.is_some()))
        .await
        .map_err(|_| "error interno".to_string())?
        .map_err(|e| e.0)
}

#[tauri::command]
async fn set_admin_key(app: AppHandle, key: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || secrets::save_admin_key(&key))
        .await
        .map_err(|_| "error interno".to_string())?
        .map_err(|e| e.0)?;
    // Datos de la API cuanto antes (sin forzar la sonda de límites).
    app.state::<AppState>().refresh.notify_one();
    Ok(())
}

#[tauri::command]
async fn clear_admin_key(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(secrets::delete_admin_key)
        .await
        .map_err(|_| "error interno".to_string())?
        .map_err(|e| e.0)?;
    app.state::<AppState>().refresh.notify_one();
    Ok(())
}

/// "Recargar datos": fuerza la sonda aunque esté en pausa y adelanta el sondeo
/// (el scheduler respeta igualmente el mínimo de 60 s).
fn request_refresh(state: &AppState) {
    state.shared.request_probe();
    state.refresh.notify_one();
}

/// Porcentaje de la sesión de 5 h, si hay un dato válido.
fn session_percent(snapshot: &Snapshot) -> Option<f64> {
    let metric = match snapshot.get(&ProviderId::Subscription) {
        Some(SectionState::Ok { reading, .. }) => reading.metric("session")?,
        _ => return None,
    };
    match metric.value {
        MetricValue::Percent(p) if p.is_finite() => Some(p),
        _ => None,
    }
}

/// Valor de una métrica si su sección tiene datos.
fn metric_value(snapshot: &Snapshot, id: ProviderId, key: &str) -> Option<MetricValue> {
    match snapshot.get(&id) {
        Some(SectionState::Ok { reading, .. }) => reading.metric(key).map(|m| m.value.clone()),
        _ => None,
    }
}

/// Cantidad de tokens abreviada: "64,4 M", "812 mil" o "950".
fn short_tokens(n: u64) -> String {
    // La conversión a f64 solo pierde precisión por encima de 2^53 tokens.
    let v = n as f64;
    let text = if n >= 1_000_000 {
        format!("{:.1} M", v / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0} mil", v / 1_000.0)
    } else {
        n.to_string()
    };
    text.replace('.', ",")
}

/// Texto del tooltip del icono: resumen de sesión, semana y Claude Code. Los
/// porcentajes se redondean igual que el umbral de color del anillo.
fn tooltip_text(snapshot: &Snapshot) -> String {
    let mut lines = vec!["Uso de Claude".to_string()];
    lines.push(match session_percent(snapshot) {
        Some(p) => format!("Sesión 5 h: {} %", icon::displayed_percent(p)),
        None => "Sesión 5 h: sin datos".to_string(),
    });
    if let Some(MetricValue::Percent(p)) =
        metric_value(snapshot, ProviderId::Subscription, "weekly")
    {
        lines.push(format!("Semana: {} %", icon::displayed_percent(p)));
    }
    if let Some(MetricValue::Tokens(n)) = metric_value(snapshot, ProviderId::Tokens, "total") {
        lines.push(format!("Claude Code: {} tokens", short_tokens(n)));
    }
    lines.join("\n")
}

/// Dibuja el icono de la bandeja para el estado actual.
fn tray_image(snapshot: &Snapshot, size: u32, show_number: bool) -> Option<Image<'static>> {
    let value = session_percent(snapshot).map_or(IconValue::NoData, IconValue::Percent);
    let img = icon::render(value, size, show_number)?;
    Some(Image::new_owned(img.rgba, img.width, img.height))
}

/// Redibuja el icono y el tooltip de la bandeja.
fn update_tray(app: &AppHandle, snapshot: &Snapshot) {
    let state = app.state::<AppState>();
    let show_number = lock(&state.settings).show_percent_in_icon;
    let size = state.icon_size.load(Ordering::Relaxed);
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        if let Some(image) = tray_image(snapshot, size, show_number) {
            let _ = tray.set_icon(Some(image));
        }
        let _ = tray.set_tooltip(Some(tooltip_text(snapshot)));
    }
}

/// Guarda el reinicio de la ventana de 5 h si ha cambiado, para que al
/// volver a arrancar la app sepa si la ventana sigue abierta.
fn persist_session_reset(state: &AppState) {
    let reset = state.shared.session_reset_ms();
    let mut persisted = lock(&state.persisted);
    if reset.is_some() && reset != persisted.last_session_reset_ms {
        persisted.last_session_reset_ms = reset;
        let _ = state.store.save_state(&persisted);
    }
}

/// Comprueba los umbrales de alerta y muestra una notificación por cada uno
/// que se cruce por primera vez en su ventana.
fn process_alerts(app: &AppHandle, snapshot: &Snapshot) {
    let Some(SectionState::Ok { reading, .. }) = snapshot.get(&ProviderId::Subscription) else {
        return;
    };
    let state = app.state::<AppState>();
    let thresholds = lock(&state.settings).alert_thresholds.clone();
    let mut to_show = Vec::new();
    {
        let mut persisted = lock(&state.persisted);
        let before = persisted.fired_alerts.clone();
        for (limit, key) in [(Limit::Session, "session"), (Limit::Weekly, "weekly")] {
            let Some(metric) = reading.metric(key) else {
                continue;
            };
            if let MetricValue::Percent(p) = metric.value {
                if let Some(alert) = alerts::check(
                    &mut persisted.fired_alerts,
                    limit,
                    p,
                    metric.resets_at_ms,
                    &thresholds,
                ) {
                    to_show.push(alert);
                }
            }
        }
        if persisted.fired_alerts != before {
            let _ = state.store.save_state(&persisted);
        }
    }
    for alert in to_show {
        // Si el sistema no permite notificaciones, el aviso se pierde sin más.
        let _ = app
            .notification()
            .builder()
            .title(alert.title())
            .body(alert.body())
            .show();
    }
}

/// Crea o cierra el mini-widget según el ajuste. Cerrarlo (en vez de
/// ocultarlo) libera la memoria de su webview.
fn sync_widget(app: &AppHandle, show: bool) {
    match (show, app.get_webview_window(WIDGET_LABEL)) {
        (true, None) => {
            let _ = create_widget(app);
        }
        (false, Some(widget)) => {
            save_persisted(app);
            let _ = widget.destroy();
        }
        _ => {}
    }
}

fn create_widget(app: &AppHandle) -> tauri::Result<()> {
    let builder =
        WebviewWindowBuilder::new(app, WIDGET_LABEL, WebviewUrl::App("widget.html".into()))
            .title("Uso de Claude")
            .inner_size(WIDGET_SIZE, WIDGET_SIZE)
            .resizable(false)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .shadow(false)
            .focused(false)
            .visible(false);
    // En macOS la transparencia exige una API privada; allí el widget
    // simplemente tiene fondo.
    #[cfg(not(target_os = "macos"))]
    let builder = builder.transparent(true);
    let widget = builder.build()?;

    let saved = lock(&app.state::<AppState>().persisted).widget_position;
    let position = saved
        .filter(|&(x, y)| point_on_some_monitor(&widget, x, y))
        .or_else(|| default_widget_position(&widget));
    if let Some((x, y)) = position {
        widget.set_position(PhysicalPosition::new(x, y))?;
    }
    widget.show()
}

/// Comprueba que una posición guardada sigue dentro de alguna pantalla (por
/// si se desconectó el monitor donde estaba el widget).
fn point_on_some_monitor(window: &WebviewWindow, x: i32, y: i32) -> bool {
    window.available_monitors().is_ok_and(|monitors| {
        monitors.iter().any(|m| {
            let (p, s) = (m.position(), m.size());
            let right =
                p.x.saturating_add(i32::try_from(s.width).unwrap_or(i32::MAX));
            let bottom =
                p.y.saturating_add(i32::try_from(s.height).unwrap_or(i32::MAX));
            x >= p.x && x < right && y >= p.y && y < bottom
        })
    })
}

/// Esquina inferior derecha de la pantalla principal, dejando sitio a la barra.
fn default_widget_position(window: &WebviewWindow) -> Option<(i32, i32)> {
    let monitor = window.primary_monitor().ok().flatten()?;
    let margin = (120.0 * monitor.scale_factor()).round() as i32;
    let (p, s) = (monitor.position(), monitor.size());
    Some((
        p.x + i32::try_from(s.width).ok()? - margin,
        p.y + i32::try_from(s.height).ok()? - margin,
    ))
}

fn save_persisted(app: &AppHandle) {
    let state = app.state::<AppState>();
    let persisted = lock(&state.persisted);
    let _ = state.store.save_state(&persisted);
}

/// Calcula dónde colocar la ventana junto al icono de la bandeja, en píxeles
/// físicos. La centra sobre el icono y la pone encima si el icono está en la
/// mitad inferior de la pantalla (barra abajo) o debajo si está arriba.
/// Nunca la deja salirse de la pantalla.
fn place_near_icon(
    icon_pos: (f64, f64),
    icon_size: (f64, f64),
    window: (f64, f64),
    screen_pos: (f64, f64),
    screen_size: (f64, f64),
) -> (f64, f64) {
    let (ix, iy) = icon_pos;
    let (iw, ih) = icon_size;
    let (ww, wh) = window;
    let (sx, sy) = screen_pos;
    let (sw, sh) = screen_size;

    let x = ix + iw / 2.0 - ww / 2.0;
    let y = if iy + ih / 2.0 > sy + sh / 2.0 {
        iy - wh
    } else {
        iy + ih
    };
    let clamp = |v: f64, min: f64, max: f64| v.min(max).max(min);
    (clamp(x, sx, sx + sw - ww), clamp(y, sy, sy + sh - wh))
}

fn position_window(window: &WebviewWindow, rect: tauri::Rect) -> tauri::Result<()> {
    let scale = window.scale_factor()?;
    let icon_pos: PhysicalPosition<f64> = rect.position.to_physical(scale);
    let icon_size: PhysicalSize<f64> = rect.size.to_physical(scale);
    let win = window.outer_size()?;
    let monitor = match window.monitor_from_point(icon_pos.x, icon_pos.y)? {
        Some(m) => Some(m),
        None => window.current_monitor()?,
    };
    let Some(monitor) = monitor else {
        return Ok(());
    };
    let (x, y) = place_near_icon(
        (icon_pos.x, icon_pos.y),
        (icon_size.width, icon_size.height),
        (f64::from(win.width), f64::from(win.height)),
        (
            f64::from(monitor.position().x),
            f64::from(monitor.position().y),
        ),
        (
            f64::from(monitor.size().width),
            f64::from(monitor.size().height),
        ),
    );
    window.set_position(PhysicalPosition::new(x, y))
}

/// Devuelve la ventana de detalle, creándola si no existe. No se crea al
/// arrancar (`"create": false` en tauri.conf.json) y se destruye al
/// cerrarla: así, en reposo, no quedan procesos de WebView2 en memoria.
fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        return Some(window);
    }
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == WINDOW_LABEL)?;
    WebviewWindowBuilder::from_config(app, config)
        .ok()?
        .build()
        .ok()
}

fn show_window(app: &AppHandle, rect: Option<tauri::Rect>) {
    let Some(window) = main_window(app) else {
        return;
    };
    let last_position = *lock(&app.state::<AppState>().last_main_position);
    match (rect, last_position) {
        // Si no se puede calcular la posición, se abre donde estuviera.
        (Some(rect), _) => {
            let _ = position_window(&window, rect);
        }
        (None, Some(pos)) => {
            let _ = window.set_position(pos);
        }
        (None, None) => {
            let _ = window.center();
        }
    }
    let _ = window.show();
    let _ = window.set_focus();
    // Al abrir la ventana se piden datos frescos. No fuerza la sonda si está
    // en pausa, y el scheduler mantiene el mínimo de 60 s entre sondeos.
    app.state::<AppState>().refresh.notify_one();
}

fn toggle_window(app: &AppHandle, rect: tauri::Rect) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        if window.is_visible().unwrap_or(false) {
            let _ = window.destroy();
            return;
        }
    }
    let state = app.state::<AppState>();
    let just_hidden = lock(&state.last_hidden).is_some_and(|t| t.elapsed() < REOPEN_GUARD);
    if !just_hidden {
        show_window(app, Some(rect));
    }
}

/// Cierra la ventana de detalle destruyéndola (no solo ocultándola), para
/// liberar la memoria de su webview.
fn hide_window(window: &tauri::Window) {
    if let Some(state) = window.try_state::<AppState>() {
        *lock(&state.last_hidden) = Some(Instant::now());
        if let Ok(pos) = window.outer_position() {
            *lock(&state.last_main_position) = Some(pos);
        }
    }
    let _ = window.destroy();
}

pub fn run() {
    let store = Store::new();
    let settings = store.load_settings();
    let shared = Arc::new(Shared::default());

    // Si la ventana de 5 h que se conocía al cerrar sigue abierta, se usa
    // al arrancar: así la sonda puede empezar sin esperar actividad.
    let persisted = store.load_state();
    if persisted
        .last_session_reset_ms
        .is_some_and(|r| r > now_ms())
    {
        shared.set_session_reset_ms(persisted.last_session_reset_ms);
    }

    let providers: Vec<Arc<dyn Provider>> = vec![
        Arc::new(SubscriptionProvider::new(Arc::clone(&shared))),
        Arc::new(TokensProvider::new(Arc::clone(&shared))),
        Arc::new(ApiProvider::new()),
    ];
    let scheduler = Scheduler::new(providers, settings.interval());
    let state = AppState {
        snapshot: Mutex::new(scheduler.initial_snapshot()),
        refresh: scheduler.refresh_handle(),
        shared,
        last_hidden: Mutex::new(None),
        last_main_position: Mutex::new(None),
        interval: scheduler.interval_handle(),
        settings: Mutex::new(settings),
        store,
        persisted: Mutex::new(persisted),
        icon_size: AtomicU32::new(icon::tray_icon_size(1.0)),
        pending_update: Mutex::new(None),
    };

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            refresh_now,
            get_settings,
            save_settings,
            has_admin_key,
            set_admin_key,
            clear_admin_key,
            pin_hint_visible,
            open_taskbar_settings,
            open_repo,
            check_update,
            install_update,
            open_main_window
        ])
        .on_window_event(|window, event| match (window.label(), event) {
            (WINDOW_LABEL, WindowEvent::Focused(false)) => {
                // Al crearse, WebView2 pasa el foco a su ventana interna y
                // llega un "perdió el foco" seguido al instante de "lo tiene".
                // Se espera un momento y solo se cierra si sigue sin foco.
                let window = window.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(BLUR_GRACE).await;
                    if !window.is_focused().unwrap_or(false) {
                        hide_window(&window);
                    }
                });
            }
            (WINDOW_LABEL, WindowEvent::CloseRequested { api, .. }) => {
                // Cerrar la ventana solo la oculta: la app sigue en la bandeja.
                api.prevent_close();
                hide_window(window);
            }
            // El widget se quita desde Ajustes; así no se pierde con Alt+F4.
            (WIDGET_LABEL, WindowEvent::CloseRequested { api, .. }) => api.prevent_close(),
            // La posición del widget se recuerda en memoria y se guarda al salir.
            (WIDGET_LABEL, WindowEvent::Moved(pos)) => {
                if let Some(state) = window.try_state::<AppState>() {
                    lock(&state.persisted).widget_position = Some((pos.x, pos.y));
                }
            }
            _ => {}
        })
        .setup(move |app| {
            // En macOS, sin icono en el Dock: la app vive solo en la barra de menús.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // "Abrir" hace falta en Linux, donde la bandeja no recibe clics.
            let open = MenuItem::with_id(app, "open", "Abrir", true, None::<&str>)?;
            let refresh = MenuItem::with_id(app, "refresh", "Recargar datos", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Salir", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &refresh, &quit])?;

            // El icono se dibuja a la resolución de la pantalla principal
            // para que los números queden nítidos.
            let scale = app
                .primary_monitor()
                .ok()
                .flatten()
                .map_or(1.0, |m| m.scale_factor());
            let icon_size = icon::tray_icon_size(scale);
            let state = app.state::<AppState>();
            state.icon_size.store(icon_size, Ordering::Relaxed);
            let show_number = lock(&state.settings).show_percent_in_icon;

            let initial = lock(&state.snapshot).clone();
            let settings = lock(&state.settings).clone();

            // El ajuste manda sobre el registro del sistema (p. ej. si el
            // usuario borró la entrada a mano o reinstaló la app).
            let handle = app.handle();
            if handle.autolaunch().is_enabled().ok() != Some(settings.launch_at_login) {
                let _ = apply_autostart(handle, settings.launch_at_login);
            }
            sync_widget(handle, settings.show_widget);

            let icon = tray_image(&initial, icon_size, show_number)
                .or_else(|| app.default_window_icon().cloned())
                .expect("la app debe tener un icono para la bandeja");

            TrayIconBuilder::with_id(TRAY_ID)
                .icon(icon)
                .tooltip(tooltip_text(&initial))
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_window(app, None),
                    "refresh" => request_refresh(&app.state::<AppState>()),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        toggle_window(tray.app_handle(), rect);
                    }
                })
                .build(app)?;

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(scheduler.run(move |snapshot| {
                update_tray(&handle, &snapshot);
                let _ = handle.emit(SNAPSHOT_EVENT, &snapshot);
                process_alerts(&handle, &snapshot);
                let state = handle.state::<AppState>();
                persist_session_reset(&state);
                *lock(&state.snapshot) = snapshot;
            }));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("no se pudo iniciar la aplicación");

    app.run(|app, event| match event {
        // Destruir la ventana de detalle deja la app sin ventanas, y Tauri
        // entonces intentaría salir. Solo se sale con "Salir" (que llega
        // con un código de salida).
        RunEvent::ExitRequested {
            code: None, api, ..
        } => api.prevent_exit(),
        // Al salir se guarda la posición del widget (y el resto del estado).
        RunEvent::Exit => save_persisted(app),
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use providers::{Metric, Reading};

    #[test]
    fn tooltip_shows_session_percent() {
        let mut snap = Snapshot::new();
        snap.insert(
            ProviderId::Subscription,
            SectionState::Ok {
                reading: Reading {
                    metrics: vec![Metric {
                        key: "session",
                        label: "Sesión 5 h",
                        value: MetricValue::Percent(41.6),
                        resets_at_ms: None,
                    }],
                },
                updated_at_ms: 0,
            },
        );
        assert_eq!(tooltip_text(&snap), "Uso de Claude\nSesión 5 h: 42 %");
    }

    #[test]
    fn tooltip_full_summary() {
        let mut snap = Snapshot::new();
        let metric = |key, value| Metric {
            key,
            label: "",
            value,
            resets_at_ms: None,
        };
        snap.insert(
            ProviderId::Subscription,
            SectionState::Ok {
                reading: Reading {
                    metrics: vec![
                        metric("session", MetricValue::Percent(88.0)),
                        metric("weekly", MetricValue::Percent(64.4)),
                    ],
                },
                updated_at_ms: 0,
            },
        );
        snap.insert(
            ProviderId::Tokens,
            SectionState::Ok {
                reading: Reading {
                    metrics: vec![metric("total", MetricValue::Tokens(64_413_085))],
                },
                updated_at_ms: 0,
            },
        );
        assert_eq!(
            tooltip_text(&snap),
            "Uso de Claude\nSesión 5 h: 88 %\nSemana: 64 %\nClaude Code: 64,4 M tokens"
        );
        // Windows corta los tooltips de más de 128 caracteres.
        assert!(tooltip_text(&snap).chars().count() < 128);
    }

    #[test]
    fn short_token_counts() {
        assert_eq!(short_tokens(950), "950");
        assert_eq!(short_tokens(812_494), "812 mil");
        assert_eq!(short_tokens(64_413_085), "64,4 M");
        assert_eq!(short_tokens(0), "0");
    }

    #[test]
    fn tooltip_without_data() {
        let mut snap = Snapshot::new();
        snap.insert(ProviderId::Subscription, SectionState::Pending);
        assert_eq!(tooltip_text(&snap), "Uso de Claude\nSesión 5 h: sin datos");
    }

    #[test]
    fn window_above_icon_when_taskbar_at_bottom() {
        // Pantalla 1920x1080, icono abajo a la derecha.
        let (x, y) = place_near_icon(
            (1800.0, 1050.0),
            (24.0, 24.0),
            (320.0, 420.0),
            (0.0, 0.0),
            (1920.0, 1080.0),
        );
        assert_eq!(y, 1050.0 - 420.0);
        // No se sale por la derecha.
        assert_eq!(x, 1920.0 - 320.0);
    }

    #[test]
    fn window_below_icon_when_menu_bar_at_top() {
        let (x, y) = place_near_icon(
            (1000.0, 0.0),
            (22.0, 22.0),
            (320.0, 420.0),
            (0.0, 0.0),
            (1920.0, 1080.0),
        );
        assert_eq!(y, 22.0);
        assert_eq!(x, 1011.0 - 160.0);
    }
}
