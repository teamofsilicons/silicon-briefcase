/** Isolated UI contract tests. Real authorization is tested in the Rust router suite. */
import { defineConfig } from "@playwright/test";

export const secret = "briefcase-organizations-local-ui-test-secret-0000000000000000";
export default defineConfig({
  testDir: "./e2e", testMatch: /(organizations|people)\.spec\.ts/, workers: 1,
  timeout: 45_000, expect: { timeout: 10_000 },
  use: { baseURL: "http://127.0.0.1:4190", viewport: { width: 1440, height: 1000 }, bypassCSP: true, contextOptions: { reducedMotion: "reduce" }, trace: "retain-on-failure" },
  webServer: [
    { command: "node e2e/fixtures/organizations-server.mjs", url: "http://127.0.0.1:4191/health", reuseExistingServer: false },
    { command: "pnpm exec next dev --port 4190", url: "http://127.0.0.1:4190/robots.txt", timeout: 120_000, reuseExistingServer: false, env: { APP_ID: "briefcase", APP_SECRET: "sa_app_local_ui_fixture_only", SESSION_SECRET: secret, PUBLIC_URL: "http://127.0.0.1:4190", APP_API_URL: "http://127.0.0.1:4191/api", ACCOUNTS_URL: "http://127.0.0.1:4191", NEXT_DIST_DIR: ".next-organizations" } },
  ],
});
