# Monitor de uso de Claude

> [!IMPORTANT]
> **Proyecto personal y no oficial.** Esta app la he creado yo por mi cuenta y la comparto gratis, sin ánimo de lucro. No está hecha, respaldada ni revisada por Anthropic, la empresa que desarrolla Claude. "Claude" y "Anthropic" son marcas de Anthropic y aquí solo se usan para indicar con qué servicio funciona la app.

> Usa una fuente no documentada (ver [Fuentes de datos](#fuentes-de-datos)) que puede dejar de funcionar en cualquier momento. El icono es propio: no usa logos ni recursos gráficos de Anthropic o Claude.

App de escritorio que vive en la bandeja del sistema y muestra, con un anillo de color, cuánto llevas gastado de Claude:

- **Plan Pro/Max:** porcentaje usado de la sesión de 5 h y del límite semanal, con la hora de reinicio de cada uno. Estos límites son compartidos entre claude.ai y Claude Code.
- **Claude Code:** tokens consumidos en la sesión actual (entrada, salida y caché).
- **API de pago por uso** (opcional): tokens y coste del mes, con una Admin key de organización.

## Descargar

| Sistema | Descarga |
| --- | --- |
| **Windows 10/11** | [**Instalador (.exe)**](https://github.com/vizarra/monitor-uso-claude/releases/latest/download/monitor-uso-claude_windows_x64-setup.exe) · alternativa: [.msi](https://github.com/vizarra/monitor-uso-claude/releases/latest/download/monitor-uso-claude_windows_x64.msi) |
| **macOS** (Apple Silicon: M1, M2…) · *experimental* | [**Imagen de disco (.dmg)**](https://github.com/vizarra/monitor-uso-claude/releases/latest/download/monitor-uso-claude_darwin_aarch64.dmg) |
| **Linux:** Ubuntu, Debian, Mint… · *experimental* | [**Paquete .deb**](https://github.com/vizarra/monitor-uso-claude/releases/latest/download/monitor-uso-claude_linux_amd64.deb) |
| **Linux:** Fedora, openSUSE… · *experimental* | [**Paquete .rpm**](https://github.com/vizarra/monitor-uso-claude/releases/latest/download/monitor-uso-claude_linux_x86_64.rpm) |
| **Linux:** otras distribuciones · *experimental* | [**AppImage**](https://github.com/vizarra/monitor-uso-claude/releases/latest/download/monitor-uso-claude_linux_amd64.AppImage) (sin instalar: dale permiso de ejecución y ábrela) |

Los enlaces descargan siempre la última versión. Todas las versiones están en [Releases](https://github.com/vizarra/monitor-uso-claude/releases). No hay versión para Mac con procesador Intel.

Los instaladores no están firmados, así que la primera vez el sistema mostrará un aviso: en [Instalación](#instalación) se explica cómo continuar en cada sistema.

## Cómo se usa

- **El anillo** refleja el uso de la sesión de 5 h: verde por debajo del 50 %, naranja hasta el 80 % y rojo a partir del 80 %. Gris con una raya significa "sin datos" (aún no hay lectura o la consulta falló). Con la sonda en pausa (ventana de 5 h cerrada y sin actividad), el anillo aparece vacío: 0 % "sin actividad".
- **Pasa el ratón** por encima para ver un resumen, o **haz clic** para abrir la ventana con el desglose. La ventana se cierra al hacer clic fuera.
- **⟳ Recargar datos** (en la ventana, o con clic derecho en el icono): consulta al momento, aunque la sonda esté en pausa.
- **Clic derecho:** Abrir, Recargar datos y Salir.
- **En Windows 11**, la ventana tiene aspecto de cristal: deja ver desenfocado lo que hay detrás, con un tono claro u oscuro según el tema de Windows. En Windows 10, Linux y macOS se ve con fondo sólido.
- **⚙ Ajustes:** intervalo de consulta (de 1 a 60 min), secciones visibles, número dentro del icono, alertas, arranque con el sistema, mini-widget (con su tamaño y su número), Admin key y "Acerca de" (versión, enlace al repo y actualizaciones).

### Actualizaciones

En **Ajustes → Acerca de → Buscar actualizaciones**, la app consulta la última release en GitHub. Si hay una versión nueva, **Instalar y reiniciar** la descarga, comprueba su firma, la instala y vuelve a abrir la app. Nunca lo comprueba por su cuenta: solo al pulsar el botón.

- Funciona a partir de la versión 0.1.3: si tienes una anterior, instala la nueva a mano una vez.
- En Linux solo está garantizado con la **AppImage**; con el `.deb` o el `.rpm`, actualiza descargando el paquete nuevo.

### Alertas

Por defecto se avisa con una notificación del sistema al llegar al **80 %** y al **95 %** de la sesión y de la semana, una sola vez por ventana. Se cambian en Ajustes → Alertas (hasta tres valores; vacío para no avisar).

### Mini-widget

Windows 11 solo muestra la bandeja en la barra de la **pantalla principal**. Si trabajas en otra pantalla, activa en Ajustes → Sistema el **mini-widget flotante**: un anillo pequeño, siempre visible, que puedes arrastrar a cualquier pantalla. Con un clic abre la ventana de detalle.

El anillo refleja el uso de la sesión con los mismos colores que el icono, y en el centro muestra el porcentaje (se puede quitar con "Mostrar el porcentaje en el widget"). Hay cuatro tamaños: mini, pequeño, mediano (por defecto) y grande.

## Instalación

Descarga el instalador de tu sistema desde la tabla de [Descargar](#descargar).

Los instaladores **no están firmados**, así que el sistema avisará la primera vez:

### Windows 10/11

1. Ejecuta el `.msi` o el `-setup.exe`.
2. Si aparece **"Windows protegió tu PC"** (SmartScreen), pulsa **Más información → Ejecutar de todos modos**.
3. **Fija el icono en la barra de tareas.** Windows 11 esconde los iconos nuevos en el desplegable (^). Lo más fiable es **arrastrar el anillo** desde el desplegable y soltarlo justo entre la ^ y los iconos de red o sonido; en otro sitio aparece el símbolo de prohibido. La alternativa (clic derecho en la barra de tareas → **Configuración de la barra de tareas** → **Otros iconos de la bandeja del sistema** → activar **Monitor de uso de Claude**) no siempre surte efecto en Windows 11. La propia app muestra un aviso con estas instrucciones.

Requiere WebView2, que ya viene con Windows 10/11 actualizados.

### macOS

1. Abre el `.dmg` y arrastra la app a Aplicaciones.
2. La primera vez, macOS la bloqueará por no estar firmada. Ve a **Ajustes del Sistema → Privacidad y seguridad** y pulsa **Abrir igualmente**.
3. La primera consulta puede pedir permiso para leer del **llavero** el elemento "Claude Code-credentials": pulsa **Permitir**.

La app vive en la barra de menús, sin icono en el Dock.

### Linux

Hay paquete `.deb`, `.rpm` y `.AppImage`. En **Ubuntu 24.04** no hace falta nada más: ya trae todo lo necesario para la bandeja. En otras distribuciones, la bandeja depende de **`libayatana-appindicator`**:

```bash
# Debian y derivadas (Ubuntu ya lo trae)
sudo apt install libayatana-appindicator3-1
```

- **GNOME sin los ajustes de Ubuntu** (Debian, Fedora…) no muestra iconos de bandeja de serie: instala la extensión **[AppIndicator and KStatusNotifierItem Support](https://extensions.gnome.org/extension/615/appindicator-support/)**. Ubuntu la trae activada.
- En Linux el icono de la bandeja no recibe clics ni muestra el resumen al pasar el ratón (AppIndicator no lo permite): la ventana se abre con **clic derecho → Abrir**.
- La Admin key se guarda en el llavero del escritorio (Secret Service: GNOME Keyring o KWallet).
- Con **Wayland** (lo normal en Ubuntu) las apps no pueden colocar sus ventanas: la ventana de detalle y el mini-widget se abren arriba a la izquierda y el widget no recuerda su posición. Mientras la ventana está abierta, la app aparece también en el dock.

## Requisitos

- **Para la sesión y la semana:** tener **Claude Code** instalado y con la sesión de claude.ai iniciada (Pro o Max). La app reutiliza sus credenciales en modo solo lectura.
- **Para los tokens de Claude Code:** haber usado Claude Code en este equipo.
- **Para la API:** una **Admin key** (`sk-ant-admin…`). Solo existe en cuentas de **organización** de Claude Console; con una cuenta individual, la sección de API queda desactivada.

## Fuentes de datos

| Sección | De dónde sale | Oficial |
|---|---|---|
| Plan Pro/Max | Cabeceras `anthropic-ratelimit-unified-*` de una petición mínima (`max_tokens: 1`) a `api.anthropic.com` con el token OAuth de Claude Code | No |
| Claude Code | Archivos JSONL de `~/.claude/projects/` (o `$CLAUDE_CONFIG_DIR/projects`) | No (formato interno) |
| API | Admin API de uso y costes (`/v1/organizations/usage_report/messages` y `/cost_report`) | Sí |

Detalles técnicos en [`docs/fuentes-datos.md`](docs/fuentes-datos.md).

**La sonda cuenta como uso**, así que la app **solo la hace cuando tu ventana de 5 h ya está abierta** o cuando pulsas ⟳ Recargar datos. Nunca abre una ventana de 5 h por su cuenta: si la ventana se cerró y no usas Claude Code, la sesión aparece al 0 % "sin actividad" hasta que vuelvas a usarlo. El uso solo desde claude.ai en el navegador no se detecta hasta pulsar ⟳.

## Privacidad

- Solo se conecta a **`api.anthropic.com`** y, cuando pulsas "Buscar actualizaciones", a **`github.com`**. Sin telemetría.
- **Token OAuth de Claude Code:** solo lectura. Nunca se guarda, se muestra ni se registra; solo viaja en la cabecera de la petición.
- **Admin key:** solo en el llavero del sistema (Credential Manager, Keychain o Secret Service). Ninguna pantalla la vuelve a mostrar.
- **Conversaciones:** de los JSONL solo se leen los campos de uso. El contenido de los mensajes nunca se guarda ni se muestra.
- **Nunca escribe en `~/.claude`.** Sus ajustes van en la carpeta de configuración del sistema, en la subcarpeta `monitor-uso-claude` (`%APPDATA%` en Windows, `~/Library/Application Support` en macOS y `~/.config` en Linux).

## Consumo

En reposo, con la ventana cerrada, la app es un solo proceso: unos **20 MB de RAM** (4,5 MB de memoria privada) y **0 % de CPU** entre consultas, medido en Windows. La ventana de detalle se crea al abrirla y se destruye al cerrarla. El mini-widget, si está activo, mantiene un webview abierto y sube el consumo.

## Estado por plataforma

| Sistema | Estado | Detalles |
| --- | --- | --- |
| **Windows 10/11** | ✅ Probado | Uso diario. |
| **Ubuntu 24.04** | ✅ Probado | En máquina virtual (GNOME, Wayland): bandeja, ventana, llavero y arranque con el sistema. Pendiente: sesión y semana con datos reales, y alertas. |
| **Ubuntu 22.04** | 🧪 En pruebas | Compila en CI; pendiente de confirmar que arranca (versión de `glibc`). |
| **macOS (Apple Silicon)** | 🧪 En pruebas | Compila en CI; pendiente de verificar la lectura de credenciales del llavero. |
| **Otras distribuciones Linux** | ❔ Sin probar | Deberían funcionar con `libayatana-appindicator` instalado. |
| **macOS (Intel)** | ❌ No disponible | No se genera versión para este procesador. |

Si lo pruebas en un sistema marcado como "en pruebas", me ayuda mucho que abras un [issue](https://github.com/vizarra/monitor-uso-claude/issues) contando si funciona o qué falla.

## Desarrollo

Requisitos: Rust estable, Node 22 y las [dependencias de Tauri 2](https://tauri.app/start/prerequisites/) de tu sistema.

```bash
npm ci
npm run tauri dev                                          # desarrollo
cargo test --manifest-path src-tauri/Cargo.toml            # tests
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo fmt --manifest-path src-tauri/Cargo.toml --check
npx tsc --noEmit                                           # tipos del frontend
npm run tauri build                                        # instaladores locales
```

Para publicar una versión: sube una etiqueta `vX.Y.Z`. El workflow `release.yml` genera los instaladores en un borrador de release, que se publica tras probarlo.

## Licencia

[MIT](LICENSE). Herramienta personal no oficial, sin relación con Anthropic.
