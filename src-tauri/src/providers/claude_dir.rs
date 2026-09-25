//! Localización de los archivos de Claude Code, compartida por los proveedores.
//! Solo lectura: nada de este módulo escribe bajo `~/.claude`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Profundidad máxima de carpetas: `projects/<proyecto>/<sesión>/subagents/`.
const MAX_DEPTH: usize = 4;

/// Carpeta de configuración de Claude Code. Respeta `CLAUDE_CONFIG_DIR`.
pub fn config_dir() -> Option<PathBuf> {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => Some(dirs::home_dir()?.join(".claude")),
    }
}

/// Carpeta con los JSONL de las sesiones.
pub fn projects_dir() -> Option<PathBuf> {
    Some(config_dir()?.join("projects"))
}

/// Busca los `.jsonl` bajo `dir`, incluidos los de subagentes.
pub fn collect_jsonl(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(dir, 0, &mut out);
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() && depth < MAX_DEPTH {
            walk(&path, depth + 1, out);
        } else if kind.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
            out.push(path);
        }
    }
}

/// Fecha de modificación de un archivo en milisegundos Unix.
pub fn modified_ms(meta: &fs::Metadata) -> Option<u64> {
    let d = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    u64::try_from(d.as_millis()).ok()
}

/// Última vez que Claude Code escribió en algún JSONL: indica actividad.
pub fn last_activity_ms() -> Option<u64> {
    collect_jsonl(&projects_dir()?)
        .iter()
        .filter_map(|p| modified_ms(&fs::metadata(p).ok()?))
        .max()
}
