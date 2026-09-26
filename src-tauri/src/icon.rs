//! Generación dinámica del icono de la bandeja.
//!
//! Diseño: un disco oscuro rodeado por un anillo de progreso cuyo color
//! depende del umbral. Por defecto, sin número y con el anillo grueso (el
//! porcentaje exacto está en el tooltip y en la ventana); con la opción
//! "Mostrar el porcentaje en el icono", el anillo es más fino y el número va
//! en blanco sobre el disco, con una fuente de píxeles de 3×5 alineada a la
//! rejilla para que se lea nítida a 16 px. Sin datos, el anillo queda gris
//! con una raya en el centro. El disco propio garantiza el contraste en temas claros y
//! oscuros, así que no se usa el modo plantilla de macOS (que quitaría los
//! colores).

use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform};

/// Nivel de uso, que decide el color del anillo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Por debajo del 50 %.
    Low,
    /// Del 50 % al 79 %.
    Medium,
    /// Del 80 % en adelante.
    High,
}

/// Qué muestra el icono.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IconValue {
    Percent(f64),
    NoData,
}

/// Píxeles RGBA sin premultiplicar, listos para `tauri::image::Image`.
pub struct IconImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

const GREEN: (u8, u8, u8) = (34, 197, 94);
const ORANGE: (u8, u8, u8) = (245, 158, 11);
const RED: (u8, u8, u8) = (239, 68, 68);
const DISC: (u8, u8, u8) = (31, 35, 40);
/// Raya central del estado "sin datos".
const DASH: (u8, u8, u8) = (220, 220, 220);
const TEXT: (u8, u8, u8) = (255, 255, 255);
/// Pista del anillo: gris semitransparente, visible sobre fondo claro y oscuro.
const TRACK: (u8, u8, u8, u8) = (128, 128, 128, 140);

/// Margen para el redondeo hacia arriba: 0,56 × 100 da 56,000…01 en coma
/// flotante y no debe mostrarse como 57.
const CEIL_EPSILON: f64 = 1e-9;

/// Porcentaje que se muestra: redondeado hacia arriba (como claude.ai) y
/// limitado a 0–100. Así nunca se muestra menos uso del que hay.
pub fn displayed_percent(percent: f64) -> u8 {
    if percent.is_nan() {
        return 0;
    }
    // Tras el `clamp` el valor cabe en u8, así que la conversión no trunca.
    (percent.clamp(0.0, 100.0) - CEIL_EPSILON).ceil().max(0.0) as u8
}

/// Nivel según el porcentaje mostrado, para que el color siempre coincida
/// con el número del tooltip (79,6 % se ve como "80" y ya es rojo).
pub fn level_for(displayed: u8) -> Level {
    match displayed {
        0..=49 => Level::Low,
        50..=79 => Level::Medium,
        _ => Level::High,
    }
}

fn level_color(level: Level) -> (u8, u8, u8) {
    match level {
        Level::Low => GREEN,
        Level::Medium => ORANGE,
        Level::High => RED,
    }
}

/// Tamaño del icono en píxeles físicos según la escala de la pantalla.
/// Cada plataforma tiene su tamaño lógico de bandeja.
pub fn tray_icon_size(scale_factor: f64) -> u32 {
    #[cfg(target_os = "macos")]
    const LOGICAL: f64 = 18.0;
    #[cfg(target_os = "linux")]
    const LOGICAL: f64 = 22.0;
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    const LOGICAL: f64 = 16.0;

    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    // Limitado a 16–64, así que la conversión a u32 es segura.
    (LOGICAL * scale).round().clamp(16.0, 64.0) as u32
}

/// Glifos de 3×5: cada fila son 3 bits, el más significativo a la izquierda.
fn glyph(ch: char) -> [u8; 5] {
    match ch {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b001, 0b001, 0b001],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        _ => [0b010, 0b010, 0b010, 0b000, 0b010], // '!'
    }
}

/// Número del centro. Solo caben dos cifras: al 100 % se muestra "!".
pub fn label(percent: f64) -> String {
    match displayed_percent(percent) {
        100 => "!".to_string(),
        n => n.to_string(),
    }
}

fn paint(r: u8, g: u8, b: u8, a: u8) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(r, g, b, a));
    paint.anti_alias = true;
    paint
}

/// Arco desde las 12 en sentido horario que cubre `fraction` de la vuelta.
fn arc(cx: f32, cy: f32, r: f32, fraction: f32) -> Option<Path> {
    if fraction >= 1.0 {
        return PathBuilder::from_circle(cx, cy, r);
    }
    let sweep = fraction * std::f32::consts::TAU;
    // Unos 64 segmentos por vuelta: suave incluso a 64 px.
    let steps = ((64.0 * fraction).ceil() as usize).max(2);
    let mut pb = PathBuilder::new();
    pb.move_to(cx, cy - r);
    for i in 1..=steps {
        let t = sweep * i as f32 / steps as f32;
        pb.line_to(cx + r * t.sin(), cy - r * t.cos());
    }
    pb.finish()
}

/// Dibuja el número centrado, alineado a píxeles enteros.
fn draw_label(pixmap: &mut Pixmap, text: &str, u: f32, disc_radius: f32) {
    let s = pixmap.width() as f32;
    // Celda entera (nitidez) lo más grande posible sin que las esquinas de dos
    // cifras (7×5 celdas) toquen el anillo: su semidiagonal, √(7²+5²)/2 ≈ 4,3
    // celdas, debe caber en el radio del disco menos media unidad.
    let max_cell = ((disc_radius - 0.5 * u) / 4.3).floor();
    let cell = u.round().min(max_cell).max(1.0);
    let n = text.chars().count() as f32;
    let width = n * 3.0 * cell + (n - 1.0) * cell;
    let x0 = ((s - width) / 2.0).round();
    let y0 = ((s - 5.0 * cell) / 2.0).round();
    let (r, g, b) = TEXT;
    let mut text_paint = paint(r, g, b, 255);
    text_paint.anti_alias = false;
    for (i, ch) in text.chars().enumerate() {
        let gx = x0 + i as f32 * 4.0 * cell;
        for (row, bits) in glyph(ch).iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) != 0 {
                    if let Some(rect) =
                        Rect::from_xywh(gx + col as f32 * cell, y0 + row as f32 * cell, cell, cell)
                    {
                        pixmap.fill_rect(rect, &text_paint, Transform::identity(), None);
                    }
                }
            }
        }
    }
}

/// Dibuja el icono. Devuelve `None` solo si `size` es 0.
pub fn render(value: IconValue, size: u32, show_number: bool) -> Option<IconImage> {
    let mut pixmap = Pixmap::new(size, size)?;
    let s = size as f32;
    let u = s / 16.0; // unidad: un píxel a 16 px
    let c = s / 2.0;

    // Geometría a 16 px. Sin número: anillo de 3 px entre los radios 5 y 8.
    // Con número: anillo de 2 px entre 6 y 8, para dejar sitio a las cifras.
    let (ring_radius, ring_width, disc_radius) = if show_number {
        (7.0 * u, 2.0 * u, 6.0 * u)
    } else {
        (6.5 * u, 3.0 * u, 5.0 * u)
    };

    // Disco de fondo y pista completa del anillo.
    if let Some(disc) = PathBuilder::from_circle(c, c, disc_radius) {
        let (r, g, b) = DISC;
        pixmap.fill_path(
            &disc,
            &paint(r, g, b, 255),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    let stroke = Stroke {
        width: ring_width,
        ..Stroke::default()
    };
    if let Some(ring) = PathBuilder::from_circle(c, c, ring_radius) {
        let (r, g, b, a) = TRACK;
        pixmap.stroke_path(
            &ring,
            &paint(r, g, b, a),
            &stroke,
            Transform::identity(),
            None,
        );
    }

    // Progreso.
    if let IconValue::Percent(p) = value {
        let shown = displayed_percent(p);
        if shown > 0 {
            let fraction = f32::from(shown) / 100.0;
            if let Some(path) = arc(c, c, ring_radius, fraction) {
                let (r, g, b) = level_color(level_for(shown));
                pixmap.stroke_path(
                    &path,
                    &paint(r, g, b, 255),
                    &stroke,
                    Transform::identity(),
                    None,
                );
            }
        }
    }

    // Sin datos: raya horizontal en el centro, alineada a píxeles enteros para
    // que se vea nítida (4×2 px a 16 px).
    if value == IconValue::NoData {
        let w = (4.0 * u).round().max(2.0);
        let h = (2.0 * u).round().max(1.0);
        if let Some(rect) = Rect::from_xywh(((s - w) / 2.0).round(), ((s - h) / 2.0).round(), w, h)
        {
            let (r, g, b) = DASH;
            let mut dash = paint(r, g, b, 255);
            dash.anti_alias = false;
            pixmap.fill_rect(rect, &dash, Transform::identity(), None);
        }
    }

    if let (true, IconValue::Percent(p)) = (show_number, value) {
        draw_label(&mut pixmap, &label(p), u, disc_radius);
    }

    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for px in pixmap.pixels() {
        let px = px.demultiply();
        rgba.extend_from_slice(&[px.red(), px.green(), px.blue(), px.alpha()]);
    }
    Some(IconImage {
        rgba,
        width: size,
        height: size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(img: &IconImage, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.width + x) * 4) as usize;
        [
            img.rgba[i],
            img.rgba[i + 1],
            img.rgba[i + 2],
            img.rgba[i + 3],
        ]
    }

    #[test]
    fn thresholds() {
        assert_eq!(level_for(0), Level::Low);
        assert_eq!(level_for(49), Level::Low);
        assert_eq!(level_for(50), Level::Medium);
        assert_eq!(level_for(79), Level::Medium);
        assert_eq!(level_for(80), Level::High);
        assert_eq!(level_for(100), Level::High);
    }

    #[test]
    fn color_follows_displayed_number() {
        // 79,2 se muestra como 80: debe ser rojo, no naranja.
        assert_eq!(displayed_percent(79.2), 80);
        assert_eq!(level_for(displayed_percent(79.2)), Level::High);
        assert_eq!(level_for(displayed_percent(49.0)), Level::Low);
        assert_eq!(level_for(displayed_percent(49.4)), Level::Medium);
    }

    #[test]
    fn displayed_percent_rounds_up_without_float_noise() {
        assert_eq!(displayed_percent(56.4), 57);
        assert_eq!(displayed_percent(0.564 * 100.0), 57);
        // 0,56 × 100 = 56,00000000000001 en coma flotante: sigue siendo 56.
        assert_eq!(displayed_percent(0.56 * 100.0), 56);
        assert_eq!(displayed_percent(0.57 * 100.0), 57);
        assert_eq!(displayed_percent(0.0), 0);
        assert_eq!(displayed_percent(0.1), 1);
        assert_eq!(displayed_percent(99.2), 100);
        assert_eq!(displayed_percent(100.0), 100);
    }

    #[test]
    fn displayed_percent_is_clamped() {
        assert_eq!(displayed_percent(-5.0), 0);
        assert_eq!(displayed_percent(130.0), 100);
        assert_eq!(displayed_percent(f64::NAN), 0);
        assert_eq!(displayed_percent(f64::INFINITY), 100);
    }

    #[test]
    fn icon_size_is_bounded() {
        assert!(tray_icon_size(1.0) >= 16);
        assert_eq!(tray_icon_size(10.0), 64);
        assert_eq!(tray_icon_size(f64::NAN), tray_icon_size(1.0));
        assert_eq!(tray_icon_size(0.0), tray_icon_size(1.0));
    }

    #[test]
    fn zero_size_gives_none() {
        assert!(render(IconValue::NoData, 0, false).is_none());
    }

    #[test]
    fn ring_uses_threshold_color_and_track() {
        // 90 %: rojo en el lado derecho (dentro del arco); a las 11 queda pista gris.
        let img = render(IconValue::Percent(90.0), 32, false).expect("icono");
        assert_eq!(img.rgba.len(), 32 * 32 * 4);
        let [r, g, b, a] = pixel(&img, 30, 16);
        assert!(
            r > 200 && g < 120 && b < 120 && a > 200,
            "rojo: {r},{g},{b},{a}"
        );

        // 25 %: el arco va de las 12 a las 3; a mitad (45°, dentro del anillo) es verde.
        let img = render(IconValue::Percent(25.0), 32, false).expect("icono");
        let [r, g, _, _] = pixel(&img, 25, 6);
        assert!(g > 150 && g > r, "verde a mitad del arco: {r},{g}");
        let [r, g, b, _] = pixel(&img, 1, 16);
        assert!(
            r.abs_diff(g) < 10 && g.abs_diff(b) < 10,
            "pista gris a la izquierda"
        );
    }

    #[test]
    fn center_is_plain_dark_disc_with_data() {
        // Sin número: con datos, todo el centro es disco oscuro.
        let img = render(IconValue::Percent(88.0), 16, false).expect("icono");
        for (x, y) in [(7, 7), (8, 8), (6, 8), (9, 7)] {
            let [r, g, b, a] = pixel(&img, x, y);
            assert!(r < 60 && g < 60 && b < 60 && a == 255, "disco en ({x},{y})");
        }
    }

    #[test]
    fn labels() {
        assert_eq!(label(7.2), "8");
        assert_eq!(label(42.0), "42");
        assert_eq!(label(98.6), "99");
        // Redondeado hacia arriba, 99,4 ya se muestra como lleno.
        assert_eq!(label(99.4), "!");
        assert_eq!(label(100.0), "!");
    }

    #[test]
    fn number_option_draws_white_digits() {
        // "8" a 16 px empieza en (7, 6): la celda central de la fila de en
        // medio (8, 8) es trazo y la de encima (8, 7) es el hueco del "8".
        let img = render(IconValue::Percent(8.0), 16, true).expect("icono");
        assert_eq!(pixel(&img, 8, 8), [255, 255, 255, 255]);
        let [r, g, b, _] = pixel(&img, 8, 7);
        assert!(r < 60 && g < 60 && b < 60, "disco oscuro");
        // Sin la opción, ese mismo píxel es disco.
        let img = render(IconValue::Percent(8.0), 16, false).expect("icono");
        let [r, g, b, _] = pixel(&img, 8, 8);
        assert!(r < 60 && g < 60 && b < 60, "sin número");
    }

    #[test]
    fn no_data_ignores_number_option() {
        let a = render(IconValue::NoData, 16, true).expect("icono");
        assert_eq!(
            pixel(&a, 7, 7),
            [220, 220, 220, 255],
            "raya también con número"
        );
    }

    #[test]
    fn no_data_shows_gray_ring_and_dash() {
        // A 16 px la raya ocupa x 6–9, y 7–8; encima (y=5) sigue el disco.
        let img = render(IconValue::NoData, 16, false).expect("icono");
        assert_eq!(pixel(&img, 7, 7), [220, 220, 220, 255]);
        assert_eq!(pixel(&img, 9, 8), [220, 220, 220, 255]);
        let [r, g, b, _] = pixel(&img, 8, 5);
        assert!(r < 60 && g < 60 && b < 60, "disco sobre la raya");
        // Ningún punto del anillo tiene color de umbral.
        let [r, g, b, _] = pixel(&img, 14, 8);
        assert!(r.abs_diff(g) < 10 && g.abs_diff(b) < 10, "pista gris");
    }

    /// Guarda PNG de muestra en `target/icon-preview/` para revisarlos a ojo:
    /// `cargo test --manifest-path src-tauri/Cargo.toml -- --ignored preview`
    #[test]
    #[ignore]
    fn preview() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/icon-preview");
        std::fs::create_dir_all(&dir).expect("directorio");
        let cases = [
            ("sin-datos", IconValue::NoData),
            ("07", IconValue::Percent(7.0)),
            ("42", IconValue::Percent(42.0)),
            ("65", IconValue::Percent(65.0)),
            ("88", IconValue::Percent(88.0)),
            ("100", IconValue::Percent(100.0)),
        ];
        // Hoja de contacto: una fila por tamaño y fondo (claro y oscuro), cada
        // icono ampliado ×8 sin suavizar, para ver los píxeles reales.
        const ZOOM: f32 = 8.0;
        let sizes = [16u32, 20, 24, 32];
        let cell = (44.0 * ZOOM) as u32;
        // Una hoja por variante: sin número (por defecto) y con número.
        for (file, show_number) in [("hoja.png", false), ("hoja-numero.png", true)] {
            let mut sheet = Pixmap::new(cell * cases.len() as u32, cell * sizes.len() as u32 * 2)
                .expect("hoja");
            for (row, size) in sizes.iter().enumerate() {
                for (bg_i, bg) in [(240, 240, 240), (32, 32, 32)].iter().enumerate() {
                    let y = (row * 2 + bg_i) as f32 * cell as f32;
                    let rect =
                        Rect::from_xywh(0.0, y, sheet.width() as f32, cell as f32).expect("rect");
                    sheet.fill_rect(
                        rect,
                        &paint(bg.0, bg.1, bg.2, 255),
                        Transform::identity(),
                        None,
                    );
                    for (col, (_, value)) in cases.iter().enumerate() {
                        let img = render(*value, *size, show_number).expect("icono");
                        let mut pm = Pixmap::new(*size, *size).expect("pixmap");
                        let (chunks, _) = img.rgba.as_chunks::<4>();
                        for (dst, &[r, g, b, a]) in pm.pixels_mut().iter_mut().zip(chunks) {
                            *dst = tiny_skia::ColorU8::from_rgba(r, g, b, a).premultiply();
                        }
                        let x = col as f32 * cell as f32;
                        sheet.draw_pixmap(
                            0,
                            0,
                            pm.as_ref(),
                            &tiny_skia::PixmapPaint {
                                quality: tiny_skia::FilterQuality::Nearest,
                                ..Default::default()
                            },
                            Transform::from_scale(ZOOM, ZOOM).post_translate(x + 8.0, y + 8.0),
                            None,
                        );
                    }
                }
            }
            sheet.save_png(dir.join(file)).expect("png");
        }
    }
}
