import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// The Tauri app's `devUrl` points at this port; a different free port would load nothing.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 5173, strictPort: true, host: "127.0.0.1" },
  build: { target: "safari16", outDir: "dist", emptyOutDir: true },
  test: { environment: "jsdom", globals: true },
});
