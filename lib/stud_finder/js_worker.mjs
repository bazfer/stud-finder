// Session-local adapters for ESLint 9 and dependency-cruiser 18. Unsupported
// configurations use the original CLI backend. No executable config is cached.
import fs from 'node:fs';
import path from 'node:path';
import readline from 'node:readline';
import crypto from 'node:crypto';
import { createRequire } from 'node:module';
import { pathToFileURL, fileURLToPath } from 'node:url';

const root = process.cwd();
const require = createRequire(path.join(root, 'package.json'));
const hash = x => crypto.createHash('sha256').update(x).digest('hex');
const implementation = hash(fs.readFileSync(fileURLToPath(import.meta.url)));
const unsupported = { unsupported: true };
let tools, graph, lintFacts, eslint, dependencyChanged = false;
let dependencyEvent, attempted = false;
const watchers = [];
const directoryEntries = new Map();

// Directory stamps invalidate inventories, never source/configuration contents.
// Reuse sorted names across requests without repeatedly enumerating large trees.
function entries(directory) {
  const stat = fs.statSync(directory, { bigint: true });
  const stamp = [stat.ino, stat.mtimeNs, stat.ctimeNs].join(':');
  let cached = directoryEntries.get(directory);
  if (cached?.stamp !== stamp) {
    cached = { stamp, entries: fs.readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name)) };
    directoryEntries.set(directory, cached);
  }
  return cached.entries;
}

function watchDependencies(extraRoots = []) {
  const roots = new Set();
  for (const directory of ['node_modules', 'frontend/node_modules', ...extraRoots]) {
    try { roots.add(fs.realpathSync(directory)); } catch { /* optional */ }
  }
  let native;
  if (process.platform === 'darwin') {
    try { native = require('fsevents'); }
    catch { try { native = createRequire(path.join(root, 'frontend/package.json'))('fsevents'); } catch { /* optional */ } }
  }
  for (const directory of roots) {
    if (native) {
      const stop = native.watch(directory, (filename, flags) => {
        dependencyChanged = true; dependencyEvent = [filename, flags];
      });
      watchers.push({ close: stop });
    } else {
      const watcher = fs.watch(directory, { recursive: true }, (event, name) => {
        dependencyChanged = true; dependencyEvent = [directory, event, name];
      });
      watcher.on('error', error => { dependencyChanged = true; dependencyEvent = error.message; });
      watchers.push(watcher);
    }
  }
}

// A new process validates its installed dependency tree before accepting disk
// facts. During a session, dependency watches invalidate the loaded toolchain.
function installationIdentity(extraRoots = []) {
  const digest = crypto.createHash('sha256'), seen = new Set();
  function walk(file) {
    const physical = fs.realpathSync(file);
    if (seen.has(physical)) return;
    seen.add(physical);
    const stat = fs.statSync(physical, { bigint: true });
    digest.update(JSON.stringify([physical, String(stat.size), String(stat.mtimeNs), String(stat.ctimeNs)]));
    if (stat.isDirectory()) {
      for (const name of fs.readdirSync(physical).sort()) walk(path.join(physical, name));
    }
  }
  for (const directory of ['node_modules', 'frontend/node_modules', ...extraRoots]) {
    if (fs.existsSync(directory)) walk(directory);
  }
  return digest.digest('hex');
}

async function toolchain() {
  if (attempted && !tools) return null;
  attempted = true;
  if (tools) return dependencyChanged ? null : tools;
  try {
    const eslintPath = require.resolve('eslint/package.json');
    const eslintVersion = JSON.parse(fs.readFileSync(eslintPath)).version;
    const parser = require('@typescript-eslint/parser');
    const ts = require('typescript');
    const candidates = [path.join(root, 'node_modules/.bin/depcruise'), ...(process.env.PATH || '').split(path.delimiter).map(p => path.join(p, 'depcruise'))];
    const binary = fs.realpathSync(candidates.find(p => fs.existsSync(p)));
    const depRoot = path.dirname(path.dirname(binary));
    const depPackage = JSON.parse(fs.readFileSync(path.join(depRoot, 'package.json')));
    if (!eslintVersion.startsWith('9.') || !depPackage.version.startsWith('18.')) return null;
    const extraRoots = [path.dirname(depRoot), path.dirname(path.dirname(eslintPath))];
    watchDependencies(extraRoots);
    const installation = installationIdentity(extraRoots);
    const dep = await import(pathToFileURL(path.join(depRoot, depPackage.main)).href);
    const { default: extractTS } = await import(pathToFileURL(path.join(depRoot, 'src/config-utl/extract-ts-config.mjs')).href);
    tools = { ESLint: require('eslint').ESLint, parser, ts, cruise: dep.cruise, extractTS,
      identity: [implementation, installation, process.version, process.execArgv, process.env.NODE_OPTIONS, process.env.NODE_PATH, eslintVersion, parser.version, ts.version, depPackage.version].join(':') };
    return dependencyChanged ? null : tools;
  } catch { return null; }
}

function readCache(directory, name, identity) {
  try {
    const value = JSON.parse(fs.readFileSync(path.join(directory, name)));
    return value.identity === identity ? value : null;
  } catch { return null; }
}
function saveCache(directory, name, value) {
  fs.mkdirSync(directory, { recursive: true });
  const destination = path.join(directory, name);
  const temporary = destination + '.' + process.pid + '.tmp';
  try { fs.writeFileSync(temporary, JSON.stringify(value)); fs.renameSync(temporary, destination); }
  finally { try { fs.unlinkSync(temporary); } catch { /* renamed */ } }
}
function fingerprints(files) {
  return Object.fromEntries(files.map(file => [file, hash(fs.readFileSync(file))]));
}

// Only this bounded declarative profile admits partial re-cruising. Graph-wide
// filters, custom rules, config extensions and executable configs fall back.
function profile(t, cacheDir, configPath) {
  const name = configPath || '.dependency-cruiser.json';
  if (!name.endsWith('.json')) return null;
  let config;
  try { config = JSON.parse(fs.readFileSync(name)); } catch { return null; }
  if (!config || typeof config !== 'object' || Array.isArray(config) || Object.keys(config).some(key => key !== 'options')) return null;
  if (Object.hasOwn(config, 'options') && (!config.options || typeof config.options !== 'object' || Array.isArray(config.options))) return null;
  const options = config.options || {};
  if (Object.keys(options).some(key => !['exclude', 'doNotFollow', 'tsConfig'].includes(key))) return null;
  for (const key of ['exclude', 'doNotFollow']) {
    const value = options[key];
    if (value && (typeof value !== 'object' || Object.keys(value).some(k => k !== 'path') || typeof value.path !== 'string')) return null;
  }
  let tsConfig = {}, tsInput = null;
  if (options.tsConfig) {
    if (Object.keys(options.tsConfig).some(key => key !== 'fileName') || typeof options.tsConfig.fileName !== 'string') return null;
    const filename = path.resolve(options.tsConfig.fileName);
    tsInput = [filename, hash(fs.readFileSync(filename))];
    const raw = t.ts.readConfigFile(filename, t.ts.sys.readFile);
    if (raw.error || raw.config.extends || raw.config.references) return null;
    const compiler = raw.config.compilerOptions || {};
    if (compiler.baseUrl) {
      const base = path.resolve(path.dirname(filename), compiler.baseUrl);
      if (base !== root && !base.startsWith(root + path.sep)) return null;
    }
    if (Object.values(compiler.paths || {}).flat().some(p => p.includes('..') || path.isAbsolute(p))) return null;
    tsConfig = t.extractTS(filename);
  }
  const inputs = [];
  const physicalCacheDir = fs.realpathSync(cacheDir);
  function walk(directory) {
    for (const entry of entries(directory)) {
      const file = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        if (file === physicalCacheDir) continue;
        if (!['.git', 'node_modules', '.claude', '.agents'].includes(entry.name)) walk(file);
      } else if (/\.(json|jsonc|yaml|yml)$|(^|\/)[^/]*lock[^/]*$/.test(file)) {
        // Config symlinks can escape the observed tree: use the full backend.
        if (entry.isSymbolicLink()) throw new Error('External configuration requires full analysis');
        inputs.push([file, hash(fs.readFileSync(file))]);
      }
    }
  }
  walk(root);
  return { options, tsConfig, signature: hash(JSON.stringify([config, tsInput, inputs])) };
}

async function dependencies(request, t) {
  const p = profile(t, request.cacheDir, request.configPath);
  if (!p) return unsupported;
  const identity = [t.identity, root, p.signature].join(':');
  graph ||= readCache(request.cacheDir, 'js-graph.json', identity);
  if (graph && (!graph.sources || typeof graph.sources !== 'object' || !Array.isArray(graph.modules) ||
    !graph.modules.every(m => typeof m.source === 'string' && Array.isArray(m.dependencies) && m.dependencies.every(d => typeof d.resolved === 'string')))) graph = null;
  const sources = fingerprints(request.files);
  const samePopulation = graph && JSON.stringify(Object.keys(graph.sources)) === JSON.stringify(request.files);
  const reusable = graph?.identity === identity && samePopulation && Array.isArray(graph.modules);
  const dirty = reusable ? request.files.filter(file => sources[file] !== graph.sources[file]) : request.files;
  if (reusable && !dirty.length) return { modules: graph.modules };
  const result = await t.cruise(reusable ? dirty : ['.'], {
    ...p.options, outputType: 'json', baseDir: root, ...(reusable ? { maxDepth: 1 } : {})
  }, {}, { tsConfig: p.tsConfig });
  if (result.exitCode !== 0) throw new Error('dependency-cruiser failed');
  const output = typeof result.output === 'string' ? JSON.parse(result.output) : result.output;
  const wanted = new Set(request.files);
  const compact = output.modules.filter(m => wanted.has(m.source)).map(m => ({
    source: m.source, dependencies: (m.dependencies || []).filter(d => wanted.has(d.resolved)).map(d => ({ resolved: d.resolved }))
  }));
  let modules = compact;
  if (reusable) {
    const changed = new Set(dirty), replacements = new Map(compact.filter(m => changed.has(m.source)).map(m => [m.source, m]));
    modules = graph.modules.filter(m => !changed.has(m.source) || replacements.has(m.source)).map(m => replacements.get(m.source) || m);
    const present = new Set(modules.map(m => m.source));
    modules.push(...compact.filter(m => changed.has(m.source) && !present.has(m.source)));
  }
  graph = { identity, sources, modules };
  saveCache(request.cacheDir, 'js-graph.json', graph);
  return { modules };
}

async function complexity(request, t) {
  const identity = [t.identity, root].join(':');
  lintFacts ||= readCache(request.cacheDir, 'js-complexity-worker.json', identity);
  if (lintFacts?.identity !== identity || !lintFacts.entries || typeof lintFacts.entries !== 'object') lintFacts = { identity, entries: {} };
  if (!eslint) {
    eslint = new t.ESLint({ cwd: root, overrideConfigFile: true, overrideConfig: [{
    files: ['**/*.{js,jsx,ts,tsx}'], languageOptions: { ecmaVersion: 2022, sourceType: 'module', parser: t.parser },
    rules: { complexity: ['error', 0] }
    }] });
    await eslint.lintText('export const warmup = 0;', { filePath: path.join(root, '__stud_finder_warmup__.ts') });
  }
  const sources = fingerprints(request.files);
  const missing = request.files.filter(file => lintFacts.entries[file]?.hash !== sources[file] || !Number.isSafeInteger(lintFacts.entries[file]?.count) || lintFacts.entries[file].count < 0);
  if (missing.length) {
    const results = await eslint.lintFiles(missing);
    const counts = Object.fromEntries(missing.map(file => [file, 0]));
    for (const entry of results) {
      const file = path.relative(root, entry.filePath);
      const values = entry.messages.map(m => m.message.match(/complexity of (\d+)/)?.[1]).filter(Boolean).map(Number);
      if (Object.hasOwn(counts, file)) counts[file] = values.length ? Math.max(...values) : 0;
    }
    for (const file of missing) lintFacts.entries[file] = { hash: sources[file], count: counts[file] };
    for (const file of Object.keys(lintFacts.entries)) if (!Object.hasOwn(sources, file)) delete lintFacts.entries[file];
    saveCache(request.cacheDir, 'js-complexity-worker.json', lintFacts);
  }
  return { counts: Object.fromEntries(request.files.map(file => [file, lintFacts.entries[file].count])) };
}

for await (const line of readline.createInterface({ input: process.stdin, crlfDelay: Infinity })) {
  try {
    const request = JSON.parse(line);
    const t = await toolchain();
    let result = !t ? { ...unsupported, reason: dependencyEvent || 'unsupported toolchain', restart: dependencyChanged } : request.operation === 'dependencies' ? await dependencies(request, t) : await complexity(request, t);
    if (dependencyChanged) result = { ...unsupported, reason: dependencyEvent, restart: true };
    process.stdout.write(JSON.stringify({ result }) + '\n');
  } catch (error) { process.stdout.write(JSON.stringify({ error: error.message }) + '\n'); }
}
for (const watcher of watchers) watcher.close();
