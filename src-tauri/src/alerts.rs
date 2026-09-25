//! Alertas al cruzar umbrales de uso del plan.
//!
//! Cada umbral avisa una sola vez por ventana: la ventana se identifica por
//! su hora de reinicio, así que al empezar una nueva se puede volver a
//! avisar. Lo ya avisado se guarda en `state.json` para no repetir avisos al
//! reiniciar la app.

use serde::{Deserialize, Serialize};

/// Límite al que se refiere una alerta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Limit {
    Session,
    Weekly,
}

/// Un umbral ya avisado en una ventana concreta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fired {
    pub limit: Limit,
    pub threshold: u8,
    /// Reinicio de la ventana en la que se avisó (0 si no se conocía).
    pub window_reset_ms: u64,
}

/// Aviso que hay que mostrar.
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub limit: Limit,
    pub threshold: u8,
    pub percent: f64,
}

impl Alert {
    pub fn title(&self) -> String {
        let what = match self.limit {
            Limit::Session => "sesión de 5 h",
            Limit::Weekly => "límite semanal",
        };
        format!("Claude: {} % de la {what}", self.threshold)
    }

    pub fn body(&self) -> String {
        format!(
            "Llevas un {:.0} % de uso. Se avisa una sola vez por ventana.",
            self.percent.clamp(0.0, 100.0)
        )
    }
}

/// Decide qué avisar y actualiza la lista de lo ya avisado.
///
/// Si se cruzan varios umbrales de golpe (p. ej. de 70 % a 96 %), solo se
/// avisa del más alto, pero todos quedan marcados. Lo avisado en ventanas
/// anteriores del mismo límite se descarta.
pub fn check(
    fired: &mut Vec<Fired>,
    limit: Limit,
    percent: f64,
    window_reset_ms: Option<u64>,
    thresholds: &[u8],
) -> Option<Alert> {
    if !percent.is_finite() {
        return None;
    }
    let window = window_reset_ms.unwrap_or(0);
    fired.retain(|f| f.limit != limit || f.window_reset_ms == window);

    let mut highest = None;
    for &t in thresholds {
        let already = fired.iter().any(|f| f.limit == limit && f.threshold == t);
        if percent >= f64::from(t) && !already {
            fired.push(Fired {
                limit,
                threshold: t,
                window_reset_ms: window,
            });
            highest = highest.max(Some(t));
        }
    }
    highest.map(|threshold| Alert {
        limit,
        threshold,
        percent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &[u8] = &[80, 95];

    #[test]
    fn fires_once_per_threshold_and_window() {
        let mut fired = Vec::new();
        assert!(check(&mut fired, Limit::Session, 50.0, Some(1), T).is_none());
        let a = check(&mut fired, Limit::Session, 81.0, Some(1), T).expect("80");
        assert_eq!(a.threshold, 80);
        // Sigue por encima: no se repite.
        assert!(check(&mut fired, Limit::Session, 85.0, Some(1), T).is_none());
        let a = check(&mut fired, Limit::Session, 95.0, Some(1), T).expect("95");
        assert_eq!(a.threshold, 95);
        assert!(check(&mut fired, Limit::Session, 99.0, Some(1), T).is_none());
    }

    #[test]
    fn new_window_can_fire_again() {
        let mut fired = Vec::new();
        assert!(check(&mut fired, Limit::Session, 90.0, Some(1), T).is_some());
        // Nueva ventana (otro reinicio): vuelve a avisar.
        assert!(check(&mut fired, Limit::Session, 90.0, Some(2), T).is_some());
        assert_eq!(fired.len(), 1, "lo de la ventana anterior se descarta");
    }

    #[test]
    fn jump_over_several_thresholds_reports_highest() {
        let mut fired = Vec::new();
        let a = check(&mut fired, Limit::Weekly, 97.0, Some(1), T).expect("aviso");
        assert_eq!(a.threshold, 95);
        assert_eq!(fired.len(), 2);
        assert!(check(&mut fired, Limit::Weekly, 97.0, Some(1), T).is_none());
    }

    #[test]
    fn limits_are_independent() {
        let mut fired = Vec::new();
        assert!(check(&mut fired, Limit::Session, 90.0, Some(1), T).is_some());
        assert!(check(&mut fired, Limit::Weekly, 90.0, Some(1), T).is_some());
        // Cambiar de ventana en la sesión no borra lo avisado en la semana.
        assert!(check(&mut fired, Limit::Session, 10.0, Some(2), T).is_none());
        assert!(check(&mut fired, Limit::Weekly, 90.0, Some(1), T).is_none());
    }

    #[test]
    fn no_thresholds_or_invalid_values() {
        let mut fired = Vec::new();
        assert!(check(&mut fired, Limit::Session, 99.0, Some(1), &[]).is_none());
        assert!(check(&mut fired, Limit::Session, f64::NAN, Some(1), T).is_none());
        assert!(fired.is_empty());
    }

    #[test]
    fn texts() {
        let a = Alert {
            limit: Limit::Session,
            threshold: 80,
            percent: 82.4,
        };
        assert_eq!(a.title(), "Claude: 80 % de la sesión de 5 h");
        assert!(a.body().contains("82 %"));
    }
}
