import { defineConfig, mergeConfig } from "vitest/config";
import viteConfig from "./vite.config";

export default mergeConfig(viteConfig, defineConfig({
  test: {
    // Each DOM suite loads its own React/jsdom environment. Unbounded workers
    // compete with native compilation and can turn deterministic tests into timeouts.
    maxWorkers: 4,
  },
}));
