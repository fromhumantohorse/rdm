import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const repository = fileURLToPath(new URL("../../", import.meta.url));
const windows = process.platform === "win32";
const gitSh = path.join(process.env.ProgramFiles ?? "C:/Program Files", "Git/bin/sh.exe");
const shell = windows ? gitSh : "sh";
const run = (program, args, cwd) => spawnSync(program, args, { cwd, encoding: "utf8", windowsHide: true });

function fixture(t) {
  const directory = mkdtempSync(path.join(os.tmpdir(), "rdm-packaging-test-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const root = path.join(directory, "repo");
  mkdirSync(path.join(root, "extension"), { recursive: true });
  for (const folder of [".git", ".github", "crates", "docs", "packaging"]) {
    mkdirSync(path.join(root, folder));
    writeFileSync(path.join(root, folder, "keep.txt"), "preserved");
  }
  writeFileSync(path.join(root, "keep.txt"), "preserved");
  writeFileSync(path.join(root, "extension", "keep.txt"), "preserved");
  return { directory, root };
}

test("shell packager rejects checkout ancestors and source output paths", { skip: windows && !existsSync(gitSh) }, (t) => {
  const { directory, root } = fixture(t);
  const sources = ["extension", ".git", ".github", "crates", "docs", "packaging"];
  for (const output of [root, directory, ...sources.map((s) => path.join(root, s)), path.join(root, "extension", "nested")]) {
    const result = run(shell, ["-c", 'root=$(cd "$1" && pwd -P); out=$2; . "$3"; prepare_output', "guard", root, output,
      path.join(repository, "packaging/output-path.sh")], root);
    assert.notEqual(result.status, 0, output);
    assert.match(result.stderr, /Refusing output/);
    assert.equal(readFileSync(path.join(root, "keep.txt"), "utf8"), "preserved");
    assert.equal(readFileSync(path.join(root, "extension", "keep.txt"), "utf8"), "preserved");
    for (const source of sources) assert.equal(readFileSync(path.join(root, source, "keep.txt"), "utf8"), "preserved");
  }
});

test("shell packager resolves a relative output before changing directory", { skip: windows && !existsSync(gitSh) }, (t) => {
  const { root } = fixture(t);
  const result = run(shell, ["-c", 'root=$(cd "$1" && pwd -P); out=target/build; . "$2"; prepare_output; case "$out" in /*) ;; *) exit 2;; esac; test -d "$out"',
    "guard", root, path.join(repository, "packaging/output-path.sh")], root);
  assert.equal(result.status, 0, result.stderr);
  assert.ok(existsSync(path.join(root, "target/build")));
});

test("shell packager protects source folders when the checkout is reached through a link", { skip: windows && !existsSync(gitSh) }, (t) => {
  const { root, directory } = fixture(t);
  const linked = path.join(directory, "linked-checkout");
  symlinkSync(root, linked, windows ? "junction" : "dir");
  const result = run(shell, ["-c", 'root=$1; out=$2; . "$3"; prepare_output', "guard", linked,
    path.join(linked, "crates"), path.join(repository, "packaging/output-path.sh")], root);
  assert.notEqual(result.status, 0, result.stderr);
  assert.match(result.stderr, /Refusing output/);
  assert.equal(readFileSync(path.join(root, "crates/keep.txt"), "utf8"), "preserved");
});

test("PowerShell packager guards paths and resolves relative output", { skip: !windows }, (t) => {
  const { root, directory } = fixture(t);
  const runner = path.join(directory, "check.ps1");
  writeFileSync(runner, `param($Helper, $Root, $Output)
$ErrorActionPreference = 'Stop'
. $Helper
Initialize-ExtensionOutput -Root $Root -Output $Output
`);
  const check = (output) => run("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", runner,
    path.join(repository, "packaging/output-path.ps1"), root, output], root);
  const sources = ["extension", ".git", ".github", "crates", "docs", "packaging"];
  for (const output of [root, directory, ...sources.map((s) => path.join(root, s)), path.join(root, "extension", "nested")]) {
    const result = check(output);
    assert.notEqual(result.status, 0, output);
    assert.match(result.stderr, /Refusing output/);
    assert.equal(readFileSync(path.join(root, "keep.txt"), "utf8"), "preserved");
    for (const source of sources) assert.equal(readFileSync(path.join(root, source, "keep.txt"), "utf8"), "preserved");
  }
  const built = check("target/build");
  assert.equal(built.status, 0, built.stderr);
  assert.ok(path.isAbsolute(built.stdout.trim()));
  assert.ok(existsSync(path.join(root, "target/build")));
});
