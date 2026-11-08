// Reject a mismatched release tag before spending time building or signing its packages.
import { readFileSync, existsSync } from "node:fs";

if (process.env.GITHUB_REF_TYPE === "tag") {
  const cargo = readFileSync("Cargo.toml", "utf8");
  const workspace = cargo.match(/^\[workspace\.package\]\s*\n([\s\S]*?)(?=^\[|$(?![\s\S]))/m)?.[1];
  const version = workspace?.match(/^version\s*=\s*"(\d+\.\d+\.\d+)"\s*$/m)?.[1];
  const tag = process.env.GITHUB_REF_NAME;
  if (!version || tag !== `v${version}`) {
    throw new Error(`Release tag ${tag} does not match Cargo.toml version ${version ?? "(missing)"}`);
  }
  if (!existsSync(`docs/releases/${tag}.md`)) throw new Error(`Release notes missing for ${tag}`);
  console.log(`Release tag, workspace version and release notes match: ${tag}`);
}
