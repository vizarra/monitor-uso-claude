import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

// Solo lo necesario de los tipos de `main.ts`.
type SectionState =
  | { status: "pending" }
  | {
      status: "ok";
      reading: { metrics: { key: string; value: { kind: string; value: number } }[] };
    }
  | { status: "noData" };

interface Snapshot {
  subscription?: SectionState;
}

interface Settings {
  showPercentInWidget: boolean;
}

const RADIUS = 26;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;
// Distancia (px) a partir de la cual un clic se convierte en arrastre.
const DRAG_THRESHOLD = 4;

let showNumber = false;
let lastSnapshot: Snapshot = {};

function sessionPercent(snapshot: Snapshot): number | null {
  const section = snapshot.subscription;
  if (section?.status !== "ok") return null;
  const metric = section.reading.metrics.find((m) => m.key === "session");
  if (!metric || metric.value.kind !== "percent" || !Number.isFinite(metric.value.value)) {
    return null;
  }
  return Math.min(100, Math.max(0, Math.round(metric.value.value)));
}

// Mismos umbrales que `level_for` en icon.rs.
function level(percent: number): "low" | "medium" | "high" {
  if (percent < 50) return "low";
  if (percent < 80) return "medium";
  return "high";
}

function render(): void {
  const percent = sessionPercent(lastSnapshot);
  const progress = document.querySelector<SVGCircleElement>("#progress");
  const dash = document.querySelector<SVGRectElement>("#dash");
  const label = document.querySelector<SVGTextElement>("#label");
  if (!progress || !dash || !label) return;

  dash.style.display = percent === null ? "" : "none";
  const filled = percent === null ? 0 : (percent / 100) * CIRCUMFERENCE;
  progress.style.strokeDasharray = `${filled} ${CIRCUMFERENCE}`;
  progress.setAttribute("class", `progress ${percent === null ? "" : level(percent)}`);
  label.textContent = showNumber && percent !== null ? (percent === 100 ? "!" : String(percent)) : "";
}

function setupPointer(): void {
  const widget = document.querySelector<HTMLDivElement>("#widget");
  if (!widget) return;
  let start: { x: number; y: number } | null = null;

  widget.addEventListener("mousedown", (e) => {
    if (e.button === 0) start = { x: e.screenX, y: e.screenY };
  });
  widget.addEventListener("mousemove", (e) => {
    if (!start) return;
    if (Math.hypot(e.screenX - start.x, e.screenY - start.y) > DRAG_THRESHOLD) {
      start = null;
      void getCurrentWindow().startDragging();
    }
  });
  widget.addEventListener("mouseup", () => {
    // Soltar sin haber arrastrado es un clic: abre la ventana de detalle.
    if (start) void invoke("open_main_window");
    start = null;
  });
}

window.addEventListener("DOMContentLoaded", async () => {
  setupPointer();
  await listen<Snapshot>("snapshot-updated", (event) => {
    lastSnapshot = event.payload;
    render();
  });
  await listen<Settings>("settings-updated", (event) => {
    showNumber = event.payload.showPercentInWidget;
    render();
  });
  showNumber = (await invoke<Settings>("get_settings")).showPercentInWidget;
  lastSnapshot = await invoke<Snapshot>("get_snapshot");
  render();
});
