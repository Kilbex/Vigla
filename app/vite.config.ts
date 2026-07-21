import { defineConfig, loadEnv } from "vite";
import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// https://vitejs.dev/config/
export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "");
  const host = process.env.TAURI_DEV_HOST;
  // Browser-only builds replace the entire Tauri IPC boundary with
  // deterministic in-process mocks. The web demo therefore cannot reach a
  // vendor CLI, filesystem command, or local Vigla runtime.
  const browserHarness =
    env.VITE_VIGLA_E2E === "1" || env.VITE_VIGLA_WEB_DEMO === "1";
  const browserAliases: Record<string, string> = browserHarness
    ? {
        "@tauri-apps/api/core": fileURLToPath(
          new URL("../tests/e2e/mocks/tauri-core.ts", import.meta.url),
        ),
        "@tauri-apps/api/event": fileURLToPath(
          new URL("../tests/e2e/mocks/tauri-event.ts", import.meta.url),
        ),
        "@tauri-apps/api/webviewWindow": fileURLToPath(
          new URL("../tests/e2e/mocks/tauri-webview-window.ts", import.meta.url),
        ),
      }
    : {};

  return {
    base: env.VITE_VIGLA_BASE || undefined,
    plugins: [react(), tailwindcss()],

    resolve: {
      alias: browserAliases,
    },

  // Vite options tailored for Tauri development.
    clearScreen: false,
    server: {
      port: 1420,
      strictPort: true,
      host: host || false,
      hmr: host
        ? { protocol: "ws", host, port: 1421 }
        : undefined,
      watch: {
        // Don't reload on Tauri host changes; cargo handles them.
        ignored: ["**/src-tauri/**"],
      },
    },

    // Env prefixes Tauri injects.
    envPrefix: ["VITE_", "TAURI_ENV_*"],

    build: {
      target:
        process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
      // Pin production minification to an explicit dependency. Vite 8's default
      // OXC path has been observed to be killed by macOS on this bundle, while
      // Terser handles the current Safari 13 Tauri WebView target.
      minify: process.env.TAURI_ENV_DEBUG ? false : "terser",
      sourcemap: !!process.env.TAURI_ENV_DEBUG,
      rolldownOptions: {
        output: {
          // Keep the always-on React Flow canvas and React runtime in stable,
          // cacheable chunks. This keeps the app entry below Vite's 500 kB
          // execution-time warning without splitting order-sensitive app code.
          codeSplitting: {
            groups: [
              {
                name: "react-runtime",
                test: /node_modules[\\/](react|react-dom|scheduler)[\\/]/,
                priority: 30,
              },
              {
                name: "operations-room",
                test: /node_modules[\\/]@xyflow[\\/]/,
                priority: 20,
              },
              {
                name: "state-runtime",
                test: /node_modules[\\/]zustand[\\/]/,
                priority: 10,
              },
            ],
          },
        },
      },
    },
  };
});
