#!/usr/bin/env node
'use strict';

// Rokomari Vibe Kanban launcher.
// Sets the central API base and delegates to the pinned `vibe-kanban-team` client.
// Developers run: npx @rokomari/vibe-kanban   (no env, no config)

const { spawn } = require('child_process');
const fs = require('fs');
const path = require('path');

// The vibe-kanban client uses modern Node globals (e.g. CustomEvent, added in
// Node 19). Fail early with a clear message instead of a cryptic runtime error.
const NODE_MAJOR = parseInt(process.versions.node.split('.')[0], 10);
if (NODE_MAJOR < 20) {
  console.error(
    `[rok-vibe-kanban] Node ${process.versions.node} is too old — requires Node >= 20 ` +
      `(20 LTS or newer; the client is built against Node 24).\n` +
      `  Install a newer Node, e.g. with nvm:  nvm install 22 && nvm use 22`
  );
  process.exit(1);
}

// === Rokomari deployment config ============================================
// Central remote server. Override with VK_SHARED_API_BASE only for testing.
const CENTRAL_API_BASE = 'https://vk.rokomari.io';

// Fallback client version, used only if explicitly enabled.
const FALLBACK_VERSION = process.env.ROK_VK_VERSION || '0.1.44-20260617110518';

// Fixed local UI port. Without this the server uses port 0 (random each run).
// Override with ROK_VK_PORT (or the client's own BACKEND_PORT/PORT).
const FIXED_PORT = process.env.ROK_VK_PORT || '8154';
// ===========================================================================

const env = { ...process.env };
// Don't clobber an explicit override — lets us point at a staging server.
if (!env.VK_SHARED_API_BASE) {
  env.VK_SHARED_API_BASE = CENTRAL_API_BASE;
}
// Pin the local server to a stable port unless one is already specified.
if (!env.BACKEND_PORT && !env.PORT) {
  env.BACKEND_PORT = FIXED_PORT;
}

const args = process.argv.slice(2);

function findOnPath(cmd) {
  const pathValue = process.env.PATH || '';
  for (const dir of pathValue.split(path.delimiter)) {
    if (!dir) continue;
    const candidate = path.join(dir, cmd);
    try {
      fs.accessSync(candidate, fs.constants.X_OK);
      return candidate;
    } catch (_e) {
      // keep scanning PATH
    }
  }
  return null;
}

function resolveBin(cmd) {
  if (process.platform === 'win32') {
    return findOnPath(`${cmd}.cmd`) || findOnPath(`${cmd}.exe`) || findOnPath(cmd);
  }
  return findOnPath(cmd);
}

function run(cmd, cmdArgs) {
  const child = spawn(cmd, cmdArgs, { stdio: 'inherit', env });
  child.on('exit', (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
    } else {
      process.exit(code == null ? 1 : code);
    }
  });
  child.on('error', (err) => {
    console.error(`[rok-vibe-kanban] failed to launch: ${err.message}`);
    process.exit(1);
  });
}

function resolvePinnedClientBin() {
  // Installer can provide an absolute package root to avoid module-resolution
  // edge cases in systemd environments.
  const explicitRoot = process.env.ROK_VK_TEAM_ROOT;
  if (explicitRoot) {
    const pkgJsonPath = path.join(explicitRoot, 'package.json');
    if (!fs.existsSync(pkgJsonPath)) {
      throw new Error(`ROK_VK_TEAM_ROOT has no package.json: ${explicitRoot}`);
    }
    const pkg = JSON.parse(fs.readFileSync(pkgJsonPath, 'utf8'));
    const binField = pkg.bin;
    const binRel =
      typeof binField === 'string'
        ? binField
        : binField && typeof binField === 'object'
          ? binField[Object.keys(binField)[0]]
          : null;
    if (!binRel) {
      throw new Error('vibe-kanban-team package.json has no bin field');
    }
    return {
      version: pkg.version || 'unknown',
      binAbs: path.join(explicitRoot, binRel),
    };
  }

  // `require.resolve('.../package.json')` can fail when package `exports` does
  // not expose package.json. Resolve the module entry and derive package root.
  const entryPath = require.resolve('vibe-kanban-team');
  const marker = `${path.sep}node_modules${path.sep}vibe-kanban-team${path.sep}`;
  const markerIdx = entryPath.lastIndexOf(marker);
  if (markerIdx === -1) {
    throw new Error(`could not derive vibe-kanban-team root from ${entryPath}`);
  }

  const pkgRoot = entryPath.slice(0, markerIdx + marker.length - 1);
  const pkgJsonPath = path.join(pkgRoot, 'package.json');
  const pkg = JSON.parse(fs.readFileSync(pkgJsonPath, 'utf8'));
  const binField = pkg.bin;
  const binRel =
    typeof binField === 'string'
      ? binField
      : binField && typeof binField === 'object'
        ? binField[Object.keys(binField)[0]]
        : null;
  if (!binRel) {
    throw new Error('vibe-kanban-team package.json has no bin field');
  }
  return {
    version: pkg.version || 'unknown',
    binAbs: path.join(pkgRoot, binRel),
  };
}

// Prefer the pinned dependency (single install, deterministic version).
try {
  const pinned = resolvePinnedClientBin();

  console.error(
    `[rok-vibe-kanban] launching vibe-kanban-team@${pinned.version} -> ${env.VK_SHARED_API_BASE} ` +
      `(local UI on http://127.0.0.1:${env.BACKEND_PORT || env.PORT})`
  );
  run(process.execPath, [pinned.binAbs, ...args]);
} catch (err) {
  if (env.ROK_VK_ALLOW_NPX_FALLBACK !== '1') {
    console.error(
      `[rok-vibe-kanban] pinned client not resolvable; npx fallback disabled.\n` +
        `  Re-run install.sh to install/patch vibe-kanban-team under @rokomari/vibe-kanban.\n` +
        `  Root cause: ${err && err.message ? err.message : String(err)}`
    );
    process.exit(1);
  }

  const npxPath = resolveBin('npx');
  const npmPath = resolveBin('npm');
  const pkgSpec = `vibe-kanban-team@${FALLBACK_VERSION}`;

  if (npxPath) {
    console.error(
      `[rok-vibe-kanban] pinned client not resolvable; falling back to npx ${pkgSpec}`
    );
    run(npxPath, ['-y', pkgSpec, ...args]);
  } else if (npmPath) {
    console.error(
      `[rok-vibe-kanban] pinned client not resolvable; npx not found, using npm exec ${pkgSpec}`
    );
    run(npmPath, ['exec', '--yes', pkgSpec, '--', ...args]);
  } else {
    console.error(
      `[rok-vibe-kanban] pinned client not resolvable and neither npx nor npm is on PATH.\n` +
        `  Install Node.js with npm, then ensure npm is on PATH.`
    );
    process.exit(1);
  }
}
