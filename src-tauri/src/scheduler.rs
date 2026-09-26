//! Sondeo periódico de los proveedores.
//!
//! Cada proveedor se consulta en su propia tarea y con tiempo límite, de modo
//! que un error, un cuelgue o un pánico solo dejan su sección en "sin datos".

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::{sleep_until, timeout, Instant};

use crate::providers::{now_ms, Provider, ProviderError, ProviderId, SectionState};

/// Intervalo mínimo entre sondeos, también para "Recargar datos" a mano.
pub const MIN_INTERVAL: Duration = Duration::from_secs(60);
/// Intervalo por defecto.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(180);
/// Tiempo máximo que se espera a un proveedor antes de darlo por fallido.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Estado de todas las secciones, indexado por proveedor.
pub type Snapshot = BTreeMap<ProviderId, SectionState>;

/// Aplica el mínimo de 60 s a un intervalo configurado por el usuario.
pub fn clamp_interval(interval: Duration) -> Duration {
    interval.max(MIN_INTERVAL)
}

/// Intervalo compartido, que los ajustes pueden cambiar mientras el
/// scheduler funciona. Siempre respeta el mínimo de 60 s.
#[derive(Clone)]
pub struct IntervalHandle(Arc<AtomicU64>);

impl IntervalHandle {
    fn new(interval: Duration) -> Self {
        Self(Arc::new(AtomicU64::new(clamp_interval(interval).as_secs())))
    }

    pub fn set(&self, interval: Duration) {
        self.0
            .store(clamp_interval(interval).as_secs(), Ordering::Relaxed);
    }

    pub fn get(&self) -> Duration {
        Duration::from_secs(self.0.load(Ordering::Relaxed))
    }
}

pub struct Scheduler {
    providers: Vec<Arc<dyn Provider>>,
    interval: IntervalHandle,
    refresh: Arc<Notify>,
}

impl Scheduler {
    pub fn new(providers: Vec<Arc<dyn Provider>>, interval: Duration) -> Self {
        Self {
            providers,
            interval: IntervalHandle::new(interval),
            refresh: Arc::new(Notify::new()),
        }
    }

    /// Permite cambiar el intervalo desde los ajustes.
    pub fn interval_handle(&self) -> IntervalHandle {
        self.interval.clone()
    }

    /// Permite pedir un sondeo inmediato desde fuera (menú o ventana).
    pub fn refresh_handle(&self) -> Arc<Notify> {
        Arc::clone(&self.refresh)
    }

    /// Estado inicial: todas las secciones pendientes.
    pub fn initial_snapshot(&self) -> Snapshot {
        self.providers
            .iter()
            .map(|p| (p.id(), SectionState::Pending))
            .collect()
    }

    /// Bucle principal: sondea, avisa con `on_update` y espera al siguiente
    /// turno. Una petición manual adelanta el sondeo, pero nunca a menos de
    /// `MIN_INTERVAL` del anterior.
    pub async fn run<F>(self, on_update: F)
    where
        F: Fn(Snapshot) + Send + 'static,
    {
        loop {
            let last = Instant::now();
            on_update(poll_all(&self.providers).await);

            tokio::select! {
                _ = sleep_until(last + self.interval.get()) => {}
                _ = self.refresh.notified() => sleep_until(last + MIN_INTERVAL).await,
            }
        }
    }
}

/// Consulta todos los proveedores en paralelo y devuelve el estado de cada uno.
pub async fn poll_all(providers: &[Arc<dyn Provider>]) -> Snapshot {
    let tasks: Vec<_> = providers
        .iter()
        .map(|p| {
            let provider = Arc::clone(p);
            let id = provider.id();
            let handle = tokio::spawn(async move {
                match timeout(FETCH_TIMEOUT, provider.fetch()).await {
                    Ok(result) => result,
                    Err(_) => Err(ProviderError("tiempo de espera agotado".into())),
                }
            });
            (id, handle)
        })
        .collect();

    let mut snapshot = Snapshot::new();
    for (id, handle) in tasks {
        // Si la tarea entró en pánico, `handle.await` devuelve Err y la app sigue.
        let result = handle
            .await
            .unwrap_or_else(|_| Err(ProviderError("error interno del proveedor".into())));
        snapshot.insert(id, SectionState::from_result(result, now_ms()));
    }
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{FetchFuture, Reading};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    struct Fixed(ProviderId, Result<Reading, ProviderError>);
    impl Provider for Fixed {
        fn id(&self) -> ProviderId {
            self.0
        }
        fn fetch(&self) -> FetchFuture<'_> {
            let r = self.1.clone();
            Box::pin(async move { r })
        }
    }

    struct Panics;
    impl Provider for Panics {
        fn id(&self) -> ProviderId {
            ProviderId::Api
        }
        fn fetch(&self) -> FetchFuture<'_> {
            Box::pin(async { panic!("fallo simulado") })
        }
    }

    struct Hangs;
    impl Provider for Hangs {
        fn id(&self) -> ProviderId {
            ProviderId::Tokens
        }
        fn fetch(&self) -> FetchFuture<'_> {
            Box::pin(std::future::pending())
        }
    }

    struct Counting(Arc<AtomicUsize>);
    impl Provider for Counting {
        fn id(&self) -> ProviderId {
            ProviderId::Subscription
        }
        fn fetch(&self) -> FetchFuture<'_> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(Reading { metrics: vec![] }) })
        }
    }

    fn empty() -> Reading {
        Reading { metrics: vec![] }
    }

    #[test]
    fn interval_never_below_minimum() {
        assert_eq!(clamp_interval(Duration::from_secs(5)), MIN_INTERVAL);
        assert_eq!(clamp_interval(Duration::ZERO), MIN_INTERVAL);
        assert_eq!(clamp_interval(Duration::from_secs(60)), MIN_INTERVAL);
        assert_eq!(
            clamp_interval(Duration::from_secs(300)),
            Duration::from_secs(300)
        );
    }

    #[test]
    fn scheduler_clamps_configured_interval() {
        let s = Scheduler::new(vec![], Duration::from_secs(1));
        assert_eq!(s.interval.get(), MIN_INTERVAL);
        // También al cambiarlo en caliente.
        let handle = s.interval_handle();
        handle.set(Duration::from_secs(10));
        assert_eq!(s.interval.get(), MIN_INTERVAL);
        handle.set(Duration::from_secs(600));
        assert_eq!(s.interval.get(), Duration::from_secs(600));
    }

    #[tokio::test(start_paused = true)]
    async fn failures_are_isolated_per_provider() {
        let providers: Vec<Arc<dyn Provider>> = vec![
            Arc::new(Fixed(ProviderId::Subscription, Ok(empty()))),
            Arc::new(Panics),
            Arc::new(Hangs),
        ];
        let snap = poll_all(&providers).await;

        assert!(matches!(
            snap[&ProviderId::Subscription],
            SectionState::Ok { .. }
        ));
        assert!(matches!(
            snap[&ProviderId::Api],
            SectionState::NoData { .. }
        ));
        assert!(matches!(
            snap[&ProviderId::Tokens],
            SectionState::NoData { .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn manual_refresh_respects_minimum_spacing() {
        let calls = Arc::new(AtomicUsize::new(0));
        let scheduler = Scheduler::new(
            vec![Arc::new(Counting(Arc::clone(&calls)))],
            DEFAULT_INTERVAL,
        );
        let refresh = scheduler.refresh_handle();
        let updates = Arc::new(Mutex::new(0usize));
        let updates_cb = Arc::clone(&updates);
        tokio::spawn(scheduler.run(move |_| {
            if let Ok(mut n) = updates_cb.lock() {
                *n += 1;
            }
        }));

        // Primer sondeo al arrancar.
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Petición manual a los 10 s: se aplaza hasta cumplir los 60 s.
        tokio::time::sleep(Duration::from_secs(9)).await;
        refresh.notify_one();
        tokio::time::sleep(Duration::from_secs(40)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::sleep(Duration::from_secs(11)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // Sin peticiones manuales, el siguiente llega al cumplirse el intervalo.
        tokio::time::sleep(DEFAULT_INTERVAL).await;
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(*updates.lock().expect("mutex"), 3);
    }
}
