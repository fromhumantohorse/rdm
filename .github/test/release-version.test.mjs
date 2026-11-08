import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

test("release validation rejects a mismatched tag or missing notes before publication", (t) => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "rdm-release-test-"));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  const script = fileURLToPath(new URL("../check-release-version.mjs", import.meta.url));
  const check = (tag, type = "tag") => spawnSync(process.execPath, [script], {
    cwd, encoding: "utf8", windowsHide: true,
    env: { ...process.env, GITHUB_REF_TYPE: type, GITHUB_REF_NAME: tag },
  });
  writeFileSync(path.join(cwd, "Cargo.toml"), '[workspace.package]\nversion = "1.2.3"\nedition = "2024"\n\n[workspace.dependencies]\n');
  assert.equal(check("main", "branch").status, 0);
  assert.match(check("v9.9.9").stderr, /does not match/);
  assert.match(check("v1.2.3").stderr, /notes missing/);
  mkdirSync(path.join(cwd, "docs/releases"), { recursive: true });
  writeFileSync(path.join(cwd, "docs/releases/v1.2.3.md"), "Release notes");
  assert.equal(check("v1.2.3").status, 0);
  // A dependency's version must not be mistaken for the workspace's version.
  writeFileSync(path.join(cwd, "Cargo.toml"), '[workspace.package]\nedition = "2024"\n\n[dependencies.unrelated]\nversion = "1.2.3"\n');
  assert.match(check("v1.2.3").stderr, /does not match/);
});
