import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// The console is a static bundle served by the Rust API from web/dist. In development,
// `vite` serves it on 5173 and proxies API and report requests to the running service.
export default defineConfig({
  plugins: [react()],
  build: { outDir: "dist", emptyOutDir: true, sourcemap: false },
  server: {
    host: "127.0.0.1",
    port: 5173,
    proxy: {
      "/api": "http://127.0.0.1:8787",
      "/reports": "http://127.0.0.1:8787",
      "/artifacts": "http://127.0.0.1:8787",
    },
  },
});
