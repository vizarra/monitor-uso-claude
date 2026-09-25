import { defineConfig } from "vite";

// Configuración recomendada por Tauri: puerto fijo y sin limpiar la consola,
// para no ocultar los errores de Rust.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "es2022",
    outDir: "dist",
    // Dos páginas: la ventana de detalle y el mini-widget.
    rollupOptions: {
      input: {
        main: "index.html",
        widget: "widget.html",
      },
    },
  },
});
