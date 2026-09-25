//! Fechas sin dependencias externas: el formato que usan Claude Code y la API
//! es fijo (RFC 3339 en UTC), así que basta con unas pocas funciones propias.

/// Días desde 1970-01-01 para una fecha del calendario gregoriano
/// (algoritmo `days_from_civil` de Howard Hinnant).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12; // marzo = 0
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Convierte un timestamp RFC 3339 (`2026-09-20T10:00:05.123Z` o con
/// desfase `+02:00`) a milisegundos Unix. `None` si no es válido.
pub fn parse_rfc3339_ms(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    let num = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = s.get(range)?;
        if part.bytes().all(|c| c.is_ascii_digit()) {
            part.parse().ok()
        } else {
            None
        }
    };
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't') {
        return None;
    }
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    // Fracción de segundo opcional: se usan los tres primeros dígitos.
    let mut i = 19;
    let mut millis = 0;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        let mut end = start;
        while b.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        if end == start {
            return None;
        }
        let digits = &s[start..end.min(start + 3)];
        millis = digits.parse::<i64>().ok()? * 10_i64.pow(3 - digits.len() as u32);
        i = end;
    }

    let offset_min = match b.get(i) {
        Some(b'Z' | b'z') if i + 1 == b.len() => 0,
        Some(sign @ (b'+' | b'-')) if i + 6 == b.len() && b[i + 3] == b':' => {
            let minutes = num(i + 1..i + 3)? * 60 + num(i + 4..i + 6)?;
            if *sign == b'+' {
                minutes
            } else {
                -minutes
            }
        }
        _ => return None,
    };

    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_min * 60;
    u64::try_from(secs * 1_000 + millis).ok()
}

/// Fecha del calendario (año, mes, día) para un número de días desde
/// 1970-01-01 (algoritmo `civil_from_days` de Howard Hinnant).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Inicio (00:00 UTC del día 1) del mes que contiene `now_ms`, como texto
/// RFC 3339, listo para los parámetros de la API.
pub fn month_start_rfc3339(now_ms: u64) -> String {
    let days = i64::try_from(now_ms / 86_400_000).unwrap_or(0);
    let (year, month, _) = civil_from_days(days);
    format!("{year:04}-{month:02}-01T00:00:00Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_parsing() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_ms("2000-01-01T00:00:00Z"),
            Some(946_684_800_000)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-09-20T10:00:05.123Z"),
            // Valor de referencia: Date.parse de JavaScript.
            Some(1_789_898_405_123)
        );
        // Fracción de un dígito y de más de tres.
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:01.5Z"), Some(1_500));
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:01.123456Z"), Some(1_123));
        // Desfase horario.
        assert_eq!(parse_rfc3339_ms("1970-01-01T02:00:00+02:00"), Some(0));
        assert_eq!(
            parse_rfc3339_ms("1970-01-01T00:00:00-01:30"),
            Some(5_400_000)
        );
        // Año bisiesto: 29 de febrero de 2024.
        assert_eq!(
            parse_rfc3339_ms("2024-02-29T00:00:00Z"),
            Some(1_709_164_800_000)
        );
    }

    #[test]
    fn rfc3339_rejects_invalid() {
        for bad in [
            "",
            "no-es-una-fecha",
            "2026-13-01T00:00:00Z",
            "2026-09-20T25:00:00Z",
            "2026-09-20 10:00:00Z",
            "2026-09-20T10:00:00",
            "2026-09-20T10:00:00.Z",
            "2026-09-20T10:00:00Zx",
            "1969-12-31T23:59:59Z",
        ] {
            assert_eq!(parse_rfc3339_ms(bad), None, "{bad}");
        }
    }

    #[test]
    fn civil_roundtrip() {
        for days in [-1, 0, 59, 60, 365, 10_957, 19_782, 20_716, 30_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{days}");
        }
    }

    #[test]
    fn month_start() {
        // 2026-09-25T21:00:00Z
        let now = parse_rfc3339_ms("2026-09-25T21:00:00Z").expect("fecha");
        assert_eq!(month_start_rfc3339(now), "2026-09-01T00:00:00Z");
        // Último milisegundo de febrero en año bisiesto.
        let now = parse_rfc3339_ms("2024-03-01T00:00:00Z").expect("fecha") - 1;
        assert_eq!(month_start_rfc3339(now), "2024-02-01T00:00:00Z");
        assert_eq!(month_start_rfc3339(0), "1970-01-01T00:00:00Z");
    }
}
