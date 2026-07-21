import { defineConfig } from "vitest/config";

// Vitest 4 replaced `environmentMatchGlobs` with `projects`. We split
// by file extension: schema/reducer tests (.test.ts) run in node, and
// component tests (.test.tsx) run in jsdom. The shared setup file is
// safe in both — it guards DOM access with `typeof window !== 'undefined'`.
export default defineConfig({
  test: {
    projects: [
      {
        test: {
          name: "unit",
          environment: "node",
          include: ["src/**/*.test.ts"],
          setupFiles: ["./src/__tests__/setup.ts"],
          css: false,
        },
      },
      {
        test: {
          name: "dom",
          environment: "jsdom",
          include: ["src/**/*.test.tsx"],
          setupFiles: ["./src/__tests__/setup.ts"],
          css: false,
        },
      },
    ],
  },
});
