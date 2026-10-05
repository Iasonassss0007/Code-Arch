import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";

const [repoArg, outArg] = process.argv.slice(2);
if (!repoArg || !outArg) {
  console.error("usage: node eval/ts_ground_truth.mjs REPO OUT.json");
  process.exit(2);
}
const repo = fs.realpathSync(path.resolve(repoArg));
const ts = createRequire(path.join(repo, "package.json"))("typescript");

const CODE = /\.(ts|tsx|mts|cts|js|jsx|mjs|cjs)$/;
const files = execFileSync("git", ["ls-files"], { cwd: repo, encoding: "utf8", maxBuffer: 1 << 28 })
  .split("\n")
  .filter((f) => CODE.test(f) && !/\.d\.[cm]?ts$/.test(f));
const tracked = new Set(files);

const host = { ...ts.sys, onUnRecoverableConfigFileDiagnostic: () => {} };
const configs = new Map();
function optionsFor(abs) {
  const cfg = ts.findConfigFile(path.dirname(abs), ts.sys.fileExists, "tsconfig.json");
  const key = cfg && path.resolve(cfg).startsWith(repo) ? cfg : "";
  if (!configs.has(key)) {
    const parsed = key ? ts.getParsedCommandLineOfConfigFile(key, {}, host) : undefined;
    const base = parsed?.options ?? { moduleResolution: ts.ModuleResolutionKind.Bundler, module: ts.ModuleKind.ESNext };
    const options = { allowJs: true, ...base };
    configs.set(key, { options, cache: ts.createModuleResolutionCache(repo, (x) => x, options) });
  }
  return configs.get(key);
}

const rel = (abs) => path.relative(repo, abs).split(path.sep).join("/");
function toSource(r) {
  if (tracked.has(r)) return r;
  const stem = r.replace(/\.d\.[cm]?ts$/, "").replace(/\.[cm]?jsx?$/, "");
  const stems = [stem, stem.replace(/\/lib\/types\//, "/src/"), stem.replace(/\/lib\//, "/src/")];
  for (const s of stems) {
    for (const ext of [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", "/index.ts", "/index.tsx", "/index.js"]) {
      if (tracked.has(s + ext)) return s + ext;
    }
  }
  return null;
}

const edges = [];
const unresolved = [];
const nonSource = [];
let external = 0;
for (const f of files) {
  const abs = path.join(repo, f);
  const { options, cache } = optionsFor(abs);
  const info = ts.preProcessFile(fs.readFileSync(abs, "utf8"), true, true);
  for (const { fileName: spec } of info.importedFiles) {
    const res = ts.resolveModuleName(spec, abs, options, ts.sys, cache).resolvedModule;
    if (!res) {
      unresolved.push([f, spec]);
      continue;
    }
    const real = fs.existsSync(res.resolvedFileName) ? fs.realpathSync(res.resolvedFileName) : res.resolvedFileName;
    const r = rel(real);
    if (r.startsWith("..") || r.includes("node_modules/")) {
      external++;
      continue;
    }
    const src = toSource(r);
    if (src) edges.push([f, src, spec]);
    else nonSource.push([f, spec, r]);
  }
}

fs.writeFileSync(outArg, JSON.stringify({ typescript: ts.version, files, edges, unresolved, nonSource, external }));
console.log(`typescript ${ts.version}: ${files.length} files, ${edges.length} edges, ${unresolved.length} unresolved, ${nonSource.length} non-source, ${external} external`);
