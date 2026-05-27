import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(here);
const publicDir = join(root, "public");
const atlasPath = join(publicDir, "module-atlas-data.json");
const pointsPath = join(publicDir, "module-atlas-points.json");
const outPath = join(publicDir, "data", "dataset.json");
const MAX_OUT_EDGES_PER_FILE = 8;

function readJson(path) {
  const text = readFileSync(path, "utf8").replace(/^\uFEFF/, "");
  return JSON.parse(text);
}

function textOf(module) {
  return [
    module.label,
    ...(module.terms ?? []).map((term) => term.term),
    ...(module.pathRoots ?? []).map((root) => root.path),
    ...(module.centralFiles ?? []).map((file) => file.path),
    ...(module.keySymbols ?? []).map((symbol) => symbol.name)
  ]
    .join(" ")
    .toLowerCase();
}

function hasAny(text, words) {
  return words.some((word) => text.includes(word));
}

function classify(module) {
  const text = textOf(module);
  const categories = [];

  if (hasAny(text, ["hero", "skill", "battle", "alliance", "city", "map", "troop", "quest", "arena", "activity", "mail", "item", "kingdom", "rally", "dungeon"])) {
    categories.push("游戏");
  }
  if (hasAny(text, ["ui", "panel", "view", "button", "btn", "layout", "dialog", "window", "toggle", "icon"])) {
    categories.push("设计");
  }
  if (hasAny(text, ["server", "network", "service", "socket", "http", "rpc", "protocol", "async", "listener", "manager"])) {
    categories.push("运维");
  }
  if (hasAny(text, ["conf", "config", "table", "data", "excel", "json", "csv", "schema", "property", "bag"])) {
    categories.push("数据分析");
  }
  if (hasAny(text, ["ai", "behavior", "navmesh", "agent"])) {
    categories.push("AI");
  }
  if (hasAny(text, ["algorithm", "graph", "sort", "math", "pathfinding", "noise", "hash"])) {
    categories.push("算法");
  }
  if (hasAny(text, ["render", "shader", "timeline", "input", "audio", "animation", "unity", "packagecache", "plugin", "asset", "editor"])) {
    categories.push("技术");
  }
  if (hasAny(text, ["packagecache", "plugin", "3rd", "third", "swig", "fbx", "unity."])) {
    categories.push("软硬件");
  }

  const primary = categories[0] ?? "编程";
  const secondary = [...new Set(categories.filter((category) => category !== primary))].slice(0, 3);
  return { primary, secondary };
}

function summarize(module) {
  const terms = (module.terms ?? []).slice(0, 6).map((term) => term.term).join(", ");
  const roots = (module.pathRoots ?? [])
    .slice(0, 3)
    .map((root) => `${root.path} (${root.files})`)
    .join("；");
  const entries = (module.entryPoints ?? [])
    .slice(0, 3)
    .map((entry) => `${entry.name} @ ${entry.path}:${entry.line}`)
    .join("；");
  const metrics = `${module.fileCount} files, ${module.symbolCount} symbols, cohesion ${(module.cohesion ?? 0).toFixed(2)}, confidence ${(module.confidence ?? 0).toFixed(2)}`;
  return [metrics, terms ? `terms: ${terms}` : "", roots ? `roots: ${roots}` : "", entries ? `entry: ${entries}` : ""]
    .filter(Boolean)
    .join(" | ");
}

function dominantPath(module, project) {
  const rootPath = module.pathRoots?.[0]?.path || module.centralFiles?.[0]?.path;
  return rootPath ? `${project}/${rootPath}` : `${project}/module-${module.id}`;
}

function basename(path) {
  return path.split(/[\\/]/).pop() || path;
}

function buildFileEdges(points) {
  const pointById = new Map(points.map((point) => [point.id, point]));
  const edges = [];
  const seen = new Set();

  for (const point of points) {
    const targets = [...(point.depOutIds ?? [])]
      .filter((targetId) => targetId !== point.id && pointById.has(targetId))
      .sort((left, right) => {
        const leftPoint = pointById.get(left);
        const rightPoint = pointById.get(right);
        const leftDegree = (leftPoint?.depIn ?? 0) + (leftPoint?.depOut ?? 0);
        const rightDegree = (rightPoint?.depIn ?? 0) + (rightPoint?.depOut ?? 0);
        return rightDegree - leftDegree || left - right;
      })
      .slice(0, MAX_OUT_EDGES_PER_FILE);

    for (const targetId of targets) {
      const key = `${point.id}->${targetId}`;
      if (seen.has(key)) {
        continue;
      }
      seen.add(key);
      edges.push({ source: `file-${point.id}`, target: `file-${targetId}` });
    }
  }

  return edges.sort((left, right) => left.source.localeCompare(right.source) || left.target.localeCompare(right.target));
}

function summarizeFile(point, module) {
  const symbols = (point.symbols ?? []).slice(0, 8).join(", ");
  const metrics = `${point.languageLabel ?? point.language} file, ${point.lineCount} lines, ${point.symbols?.length ?? 0} symbols, ${point.depIn ?? 0} incoming deps, ${point.depOut ?? 0} outgoing deps`;
  return [
    metrics,
    `module: ${point.moduleLabel ?? module?.label ?? point.moduleId}`,
    symbols ? `symbols: ${symbols}` : ""
  ]
    .filter(Boolean)
    .join(" | ");
}

function main() {
  if (!existsSync(atlasPath)) {
    throw new Error(`Missing ${atlasPath}. Generate or copy module-atlas-data.json first.`);
  }
  if (!existsSync(pointsPath)) {
    throw new Error(`Missing ${pointsPath}. Generate or copy module-atlas-points.json first.`);
  }

  const atlas = readJson(atlasPath);
  const points = readJson(pointsPath);
  const project = atlas.metadata?.project ?? "codebase";
  const generatedAt = atlas.metadata?.generatedAt ?? new Date().toISOString();
  const modules = new Map((atlas.modules ?? []).map((module) => [module.id, module]));
  const edges = buildFileEdges(points);
  const incoming = new Map();
  const outgoing = new Map();

  for (const edge of edges) {
    outgoing.set(edge.source, (outgoing.get(edge.source) ?? 0) + 1);
    incoming.set(edge.target, (incoming.get(edge.target) ?? 0) + 1);
  }

  const nodes = points.map((point) => {
    const module = modules.get(point.moduleId);
    const id = `file-${point.id}`;
    return {
      id,
      url: `${project}/${point.path}`,
      title: basename(point.path),
      description: summarizeFile(point, module),
      iconUrl: "/favicon.svg",
      moduleId: point.moduleId,
      moduleLabel: point.moduleLabel ?? module?.label ?? String(point.moduleId),
      path: point.path,
      language: point.language,
      languageLabel: point.languageLabel ?? point.language,
      lineCount: point.lineCount,
      symbolCount: point.symbols?.length ?? 0,
      inDegree: incoming.get(id) ?? 0,
      outDegree: outgoing.get(id) ?? 0,
      crawledAt: generatedAt,
      depth: 3,
      category: classify(module ?? { label: point.moduleLabel, terms: [], pathRoots: [{ path: point.path }] })
    };
  });

  const dataset = {
    nodes,
    edges,
    meta: {
      seedUrls: [project],
      crawledAt: generatedAt,
      totalNodes: nodes.length,
      totalEdges: edges.length,
      maxDepth: 3,
      source: "codedb_module_atlas_files",
      project,
      totalFiles: atlas.metadata?.totalFiles ?? points.length,
      languages: atlas.metadata?.languages ?? [],
      visualEdgePolicy: `top ${MAX_OUT_EDGES_PER_FILE} outgoing dependencies per file by target degree`
    }
  };

  mkdirSync(dirname(outPath), { recursive: true });
  writeFileSync(outPath, `${JSON.stringify(dataset)}\n`, "utf8");
  console.log(`wrote ${outPath} (${nodes.length} files, ${edges.length} dependency edges)`);
}

main();
