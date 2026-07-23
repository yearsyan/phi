import { defineConfig } from "tsup";

export default defineConfig({
  entry: {
    index: "src/index.ts",
    cli: "src/cli.ts",
  },
  format: ["esm"],
  target: "node22",
  platform: "node",
  bundle: true,
  splitting: false,
  sourcemap: true,
  dts: true,
  clean: true,
  external: [
    "@earendil-works/pi-agent-core",
    "@earendil-works/pi-ai",
    "@earendil-works/pi-coding-agent",
    "typebox",
    "ws",
  ],
});
