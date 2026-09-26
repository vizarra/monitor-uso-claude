import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// Tipos espejo de los de `src-tauri/src/providers/mod.rs` y `settings.rs`.
type ProviderId = "subscription" | "tokens" | "api";

type MetricValue =
  | { kind: "percent"; value: number }
  | { kind: "tokens"; value: number }
  | { kind: "usdCents"; value: number };

interface Metric {
  key: string;
  label: string;
  value: MetricValue;
  resetsAtMs: number | null;
}

type SectionState =
  | { status: "pending" }
  | { status: "ok"; reading: { metrics: Metric[] }; updatedAtMs: number }
  | { status: "noData"; reason: string; updatedAtMs: number };

type Snapshot = Partial<Record<ProviderId, SectionState>>;

interface Settings {
  intervalSecs: number;
  showSubscription: boolean;
  showTokens: boolean;
  showApi: boolean;
  showPercentInIcon: boolean;
  alertThresholds: number[];
  launchAtLogin: boolean;
  showWidget: boolean;
  showPercentInWidget: boolean;
  widgetSize: WidgetSize;
  pinHintDismissed: boolean;
}

type WidgetSize = "mini" | "small" | "medium" | "large";

function parseWidgetSize(value: string | null): WidgetSize {
  return value === "mini" || value === "small" || value === "large" ? value : "medium";
}

const TITLES: Record<ProviderId, string> = {
  subscription: "Plan Pro/Max",
  tokens: "Claude Code",
  api: "API",
};
const ORDER: ProviderId[] = ["subscription", "tokens", "api"];

const numberFmt = new Intl.NumberFormat("es-ES");
const usdFmt = new Intl.NumberFormat("es-ES", { style: "currency", currency: "USD" });
const timeFmt = new Intl.DateTimeFormat("es-ES", { hour: "2-digit", minute: "2-digit" });
const dateTimeFmt = new Intl.DateTimeFormat("es-ES", {
  weekday: "short",
  hour: "2-digit",
  minute: "2-digit",
});

let lastSnapshot: Snapshot = {};
let settings: Settings | null = null;

function $<T extends HTMLElement>(selector: string): T {
  const node = document.querySelector<T>(selector);
  if (!node) throw new Error(`Falta el elemento ${selector}`);
  return node;
}

function formatValue(v: MetricValue): string {
  switch (v.kind) {
    case "percent":
      return `${Math.round(v.value)} %`;
    case "tokens":
      return numberFmt.format(v.value);
    case "usdCents":
      return usdFmt.format(v.value / 100);
  }
}

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function renderSection(id: ProviderId, state: SectionState): HTMLElement {
  const section = el("section");
  section.append(el("h2", undefined, TITLES[id]));

  if (state.status === "pending") {
    section.append(el("p", "muted", "Cargando…"));
    return section;
  }
  if (state.status === "noData") {
    section.append(el("p", "muted", "Sin datos"));
    section.append(el("p", "reason", state.reason));
  } else {
    const list = el("dl");
    for (const m of state.reading.metrics) {
      list.append(el("dt", undefined, m.label));
      const dd = el("dd", undefined, formatValue(m.value));
      if (m.resetsAtMs !== null) {
        dd.append(el("span", "muted", ` · reinicio ${dateTimeFmt.format(m.resetsAtMs)}`));
      }
      list.append(dd);
    }
    section.append(list);
  }
  section.append(el("p", "updated", `Actualizado a las ${timeFmt.format(state.updatedAtMs)}`));
  return section;
}

function isVisible(id: ProviderId): boolean {
  if (!settings) return true;
  switch (id) {
    case "subscription":
      return settings.showSubscription;
    case "tokens":
      return settings.showTokens;
    case "api":
      return settings.showApi;
  }
}

function render(): void {
  const sections = ORDER.flatMap((id) => {
    const state = lastSnapshot[id];
    return state && isVisible(id) ? [renderSection(id, state)] : [];
  });
  $<HTMLDivElement>("#sections").replaceChildren(...sections);
}

// --- Ajustes ---------------------------------------------------------------

function showMessage(text: string, isError = false): void {
  const node = $<HTMLParagraphElement>("#settings-message");
  node.textContent = text;
  node.classList.toggle("error", isError);
}

// Grupos de botones de opción en lugar de <select>: en Linux, el desplegable
// de WebKitGTK es una ventana aparte que quita el foco y cierra la ventana.
function checkRadio(name: string, value: string): void {
  for (const input of document.querySelectorAll<HTMLInputElement>(`input[name="${name}"]`)) {
    input.checked = input.value === value;
  }
}

function checkedRadio(name: string): string | null {
  return document.querySelector<HTMLInputElement>(`input[name="${name}"]:checked`)?.value ?? null;
}

function fillSettingsForm(s: Settings): void {
  checkRadio("interval", String(s.intervalSecs));
  $<HTMLInputElement>("#show-subscription").checked = s.showSubscription;
  $<HTMLInputElement>("#show-tokens").checked = s.showTokens;
  $<HTMLInputElement>("#show-api").checked = s.showApi;
  $<HTMLInputElement>("#show-percent").checked = s.showPercentInIcon;
  $<HTMLInputElement>("#alert-thresholds").value = s.alertThresholds.join(", ");
  $<HTMLInputElement>("#launch-at-login").checked = s.launchAtLogin;
  $<HTMLInputElement>("#show-widget").checked = s.showWidget;
  $<HTMLInputElement>("#show-percent-widget").checked = s.showPercentInWidget;
  checkRadio("widget-size", s.widgetSize);
}

// Convierte "80, 95" en [80, 95]; el backend valida el rango y el orden.
function parseThresholds(text: string): number[] {
  return text
    .split(/[,;\s]+/)
    .map((t) => Number.parseInt(t, 10))
    .filter((n) => Number.isInteger(n));
}

function readSettingsForm(): Settings {
  return {
    // Si el intervalo guardado no está entre los botones, se conserva.
    intervalSecs: Number(checkedRadio("interval") ?? settings?.intervalSecs ?? 180),
    showSubscription: $<HTMLInputElement>("#show-subscription").checked,
    showTokens: $<HTMLInputElement>("#show-tokens").checked,
    showApi: $<HTMLInputElement>("#show-api").checked,
    showPercentInIcon: $<HTMLInputElement>("#show-percent").checked,
    alertThresholds: parseThresholds($<HTMLInputElement>("#alert-thresholds").value),
    launchAtLogin: $<HTMLInputElement>("#launch-at-login").checked,
    showWidget: $<HTMLInputElement>("#show-widget").checked,
    showPercentInWidget: $<HTMLInputElement>("#show-percent-widget").checked,
    widgetSize: parseWidgetSize(checkedRadio("widget-size")),
    pinHintDismissed: settings?.pinHintDismissed ?? false,
  };
}

async function saveSettings(): Promise<void> {
  try {
    settings = await invoke<Settings>("save_settings", { settings: readSettingsForm() });
    fillSettingsForm(settings);
    render();
    showMessage("Guardado");
  } catch (e) {
    showMessage(String(e), true);
  }
}

async function refreshKeyStatus(): Promise<void> {
  const status = $<HTMLParagraphElement>("#key-status");
  try {
    const hasKey = await invoke<boolean>("has_admin_key");
    status.textContent = hasKey ? "Admin key guardada." : "Sin Admin key: la sección de API está desactivada.";
    $<HTMLButtonElement>("#clear-key").hidden = !hasKey;
  } catch (e) {
    status.textContent = String(e);
  }
}

async function saveKey(): Promise<void> {
  const input = $<HTMLInputElement>("#admin-key");
  try {
    await invoke("set_admin_key", { key: input.value });
    showMessage("Clave guardada en el llavero");
    await refreshKeyStatus();
  } catch (e) {
    showMessage(String(e), true);
  } finally {
    // La clave no se queda en la página más tiempo del necesario.
    input.value = "";
  }
}

async function clearKey(): Promise<void> {
  try {
    await invoke("clear_admin_key");
    showMessage("Clave borrada del llavero");
    await refreshKeyStatus();
  } catch (e) {
    showMessage(String(e), true);
  }
}

async function refreshPinHint(): Promise<void> {
  $<HTMLDivElement>("#pin-hint").hidden = !(await invoke<boolean>("pin_hint_visible"));
}

async function dismissPinHint(): Promise<void> {
  if (!settings) return;
  try {
    settings = await invoke<Settings>("save_settings", {
      settings: { ...settings, pinHintDismissed: true },
    });
    await refreshPinHint();
  } catch (e) {
    showMessage(String(e), true);
  }
}

// Solo se consulta GitHub al pulsar el botón; nunca en segundo plano.
// El estado se muestra junto a los botones, en "Acerca de".
function showUpdateStatus(text: string, isError = false): void {
  const node = $<HTMLParagraphElement>("#update-status");
  node.textContent = text;
  node.classList.toggle("error", isError);
}

async function checkUpdate(): Promise<void> {
  const check = $<HTMLButtonElement>("#check-update");
  const install = $<HTMLButtonElement>("#install-update");
  check.disabled = true;
  install.hidden = true;
  showUpdateStatus("Buscando actualizaciones…");
  try {
    const version = await invoke<string | null>("check_update");
    if (version === null) {
      showUpdateStatus("Ya tienes la última versión.");
    } else {
      showUpdateStatus(`Hay una versión nueva: ${version}.`);
      install.hidden = false;
    }
  } catch (e) {
    showUpdateStatus(String(e), true);
  } finally {
    check.disabled = false;
  }
}

async function installUpdate(): Promise<void> {
  const check = $<HTMLButtonElement>("#check-update");
  const install = $<HTMLButtonElement>("#install-update");
  check.disabled = true;
  install.disabled = true;
  showUpdateStatus("Descargando e instalando… la app se reiniciará.");
  try {
    await invoke("install_update");
  } catch (e) {
    showUpdateStatus(String(e), true);
    install.hidden = true;
  } finally {
    check.disabled = false;
    install.disabled = false;
  }
}

function toggleSettings(): void {
  const form = $<HTMLFormElement>("#settings");
  const opening = form.hidden;
  form.hidden = !opening;
  $<HTMLDivElement>("#sections").hidden = opening;
  $<HTMLDivElement>("#pin-hint").hidden = true;
  if (!opening) void refreshPinHint();
  $<HTMLHeadingElement>("#title").textContent = opening ? "Ajustes" : "Uso de Claude";
  $<HTMLButtonElement>("#toggle-settings").textContent = opening ? "←" : "⚙";
  $<HTMLButtonElement>("#toggle-settings").title = opening ? "Volver" : "Ajustes";
  showMessage("");
  if (opening) void refreshKeyStatus();
}

// El cristal (Acrylic detrás y fondos translúcidos encima) solo se muestra
// en Windows 11. En Linux y macOS la ventana no recibe el efecto, y en
// Windows 10 Acrylic va a tirones, así que ahí se mantiene el fondo sólido.
// Windows 11 se reconoce por `platformVersion` 13 o superior (método que
// documenta Microsoft).
interface UserAgentData {
  platform: string;
  getHighEntropyValues(hints: string[]): Promise<{ platformVersion?: string }>;
}

async function isWindows11(): Promise<boolean> {
  const uad = (navigator as Navigator & { userAgentData?: UserAgentData }).userAgentData;
  if (uad?.platform !== "Windows") return false;
  try {
    const { platformVersion } = await uad.getHighEntropyValues(["platformVersion"]);
    return Number.parseInt(platformVersion ?? "0", 10) >= 13;
  } catch {
    return false;
  }
}

window.addEventListener("DOMContentLoaded", async () => {
  if (await isWindows11()) document.documentElement.classList.add("glass");
  $<HTMLButtonElement>("#refresh").addEventListener("click", () => {
    void invoke("refresh_now");
  });
  $<HTMLButtonElement>("#toggle-settings").addEventListener("click", toggleSettings);
  $<HTMLFormElement>("#settings").addEventListener("change", (event) => {
    // El campo de la clave se guarda solo con su botón.
    if ((event.target as HTMLElement).id !== "admin-key") void saveSettings();
  });
  $<HTMLFormElement>("#settings").addEventListener("submit", (event) => event.preventDefault());
  $<HTMLButtonElement>("#save-key").addEventListener("click", () => void saveKey());
  $<HTMLButtonElement>("#clear-key").addEventListener("click", () => void clearKey());
  $<HTMLButtonElement>("#open-taskbar").addEventListener("click", () => {
    invoke("open_taskbar_settings").catch((e) => showMessage(String(e), true));
  });
  $<HTMLButtonElement>("#dismiss-pin-hint").addEventListener("click", () => void dismissPinHint());
  $<HTMLButtonElement>("#open-repo").addEventListener("click", () => {
    invoke("open_repo").catch((e) => showMessage(String(e), true));
  });
  $<HTMLButtonElement>("#check-update").addEventListener("click", () => void checkUpdate());
  $<HTMLButtonElement>("#install-update").addEventListener("click", () => void installUpdate());
  getVersion()
    .then((version) => ($<HTMLSpanElement>("#app-version").textContent = version))
    .catch(() => undefined);

  await listen<Snapshot>("snapshot-updated", (event) => {
    lastSnapshot = event.payload;
    render();
  });
  await listen<Settings>("settings-updated", (event) => {
    settings = event.payload;
    fillSettingsForm(settings);
    render();
  });

  settings = await invoke<Settings>("get_settings");
  fillSettingsForm(settings);
  lastSnapshot = await invoke<Snapshot>("get_snapshot");
  render();
  await refreshPinHint();
});
