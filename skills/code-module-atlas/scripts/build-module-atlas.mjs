import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const here = dirname(fileURLToPath(import.meta.url));
const skillRoot = dirname(here);
const repoRoot = resolve(skillRoot, "..", "..");
const viewerRoot = join(skillRoot, "assets", "viewer");
const npmCommand = process.platform === "win32" ? "npm.cmd" : "npm";

function usage() {
  console.log(`Usage:
  node skills/code-module-atlas/scripts/build-module-atlas.mjs <repo-root> [--serve] [--port 5174]

Options:
  --config <path>       Explicit codedb-mcp TOML config. Defaults to <repo-root>/.codedb-mcp/codedb-mcp.toml
  --codedb-exe <path>   Explicit codebase-mcp executable. Defaults to sibling skills/codedb-mcp/assets/codebase-mcp.exe
  --output <path>       codedb_module_atlas output path relative to repo root. Defaults to .codedb-mcp/module-atlas-data.json
  --limit <n>           Maximum modules requested from codedb_module_atlas. Defaults to 2000
  --min-files <n>       Minimum files per module. Defaults to 2
  --serve               Start the Vite viewer after data generation.
  --port <n>            Port used with --serve. Defaults to 5174.
`);
}

function argValue(args, name, fallback) {
  const index = args.indexOf(name);
  if (index < 0) {
    return fallback;
  }
  return args[index + 1] ?? fallback;
}

function run(command, args, cwd) {
  const result = spawnSync(command, args, {
    cwd,
    stdio: "inherit",
    shell: process.platform === "win32" && command.endsWith(".cmd")
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`${command} exited with ${result.status}`);
  }
}

function main() {
  const args = process.argv.slice(2);
  if (args.includes("--help") || args.includes("-h")) {
    usage();
    return;
  }

  const targetArg = args.find((arg) => !arg.startsWith("--") && !["true", "false"].includes(arg));
  if (!targetArg) {
    usage();
    throw new Error("Missing repo root.");
  }

  const targetRoot = resolve(targetArg);
  const configPath = resolve(argValue(args, "--config", join(targetRoot, ".codedb-mcp", "codedb-mcp.toml")));
  const defaultExe = join(repoRoot, "skills", "codedb-mcp", "assets", "codebase-mcp.exe");
  const codedbExe = resolve(argValue(args, "--codedb-exe", defaultExe));
  const outputRel = argValue(args, "--output", ".codedb-mcp/module-atlas-data.json");
  const limit = Number(argValue(args, "--limit", "2000"));
  const minFiles = Number(argValue(args, "--min-files", "2"));
  const serve = args.includes("--serve");
  const port = argValue(args, "--port", "5174");

  if (!existsSync(targetRoot)) {
    throw new Error(`Repo root does not exist: ${targetRoot}`);
  }
  if (!existsSync(configPath)) {
    throw new Error(`Missing codedb-mcp config: ${configPath}`);
  }
  if (!existsSync(codedbExe)) {
    throw new Error(`Missing codedb executable: ${codedbExe}`);
  }

  const toolArgs = {
    output_path: outputRel,
    min_files: minFiles,
    limit,
    split_files: true
  };

  run(codedbExe, [
    "--config",
    configPath,
    "--root",
    targetRoot,
    "tool",
    "codedb_module_atlas",
    JSON.stringify(toolArgs)
  ], targetRoot);

  const atlasPath = resolve(targetRoot, outputRel);
  const pointsPath = resolve(dirname(atlasPath), "module-atlas-points.json");
  if (!existsSync(atlasPath)) {
    throw new Error(`codedb_module_atlas did not write ${atlasPath}`);
  }
  if (!existsSync(pointsPath)) {
    throw new Error(`codedb_module_atlas did not write ${pointsPath}`);
  }

  const publicDir = join(viewerRoot, "public");
  mkdirSync(publicDir, { recursive: true });
  copyFileSync(atlasPath, join(publicDir, "module-atlas-data.json"));
  copyFileSync(pointsPath, join(publicDir, "module-atlas-points.json"));

  if (!existsSync(join(viewerRoot, "node_modules"))) {
    run(npmCommand, ["install"], viewerRoot);
  }

  run(npmCommand, ["run", "build:meet-data"], viewerRoot);
  run(npmCommand, ["run", "prepare:meet"], viewerRoot);

  if (serve) {
    run(npmCommand, ["run", "dev", "--", "--port", String(port), "--strictPort"], viewerRoot);
  } else {
    console.log(`Viewer data is ready at ${viewerRoot}`);
    console.log(`Run: cd ${viewerRoot} && npm run dev -- --port ${port} --strictPort`);
  }
}

main();
