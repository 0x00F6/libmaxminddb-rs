import { defineConfig } from "vite";
export default defineConfig({
  base: "/libmaxminddb-rs/",
  build: { outDir: "dist", chunkSizeWarningLimit: 750 },
});
