import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const port = Number(process.env.VITE_PORT) || 1420;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port,
    strictPort: true,
    host: true,
    hmr: { host: process.env.TAURI_DEV_HOST || "localhost" },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "esnext",
    cssTarget: ["chrome108", "safari15"],
    minify: !process.env.TAURI_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_DEBUG,
  },
});
