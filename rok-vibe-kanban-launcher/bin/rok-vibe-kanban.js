#!/usr/bin/env node
'use strict';

// Rokomari Vibe Kanban launcher.
// Sets the central API base and delegates to the pinned `vibe-kanban-team` client.
// Developers run: npx @rokomari/vibe-kanban   (no env, no config)

const { spawn } = require('child_process');
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

// Fallback client version, used only if the pinned dependency cannot be
// resolved. Keep this in lockstep with the `vibe-kanban-team` version in
// package.json and with the deployed remote image tag.
const FALLBACK_VERSION = process.env.ROK_VK_VERSION || '0.1.44-20260617110518';

// Fixed local UI port. Without this the server uses port 0 (random each run).
// Override with ROK_VK_PORT (or the client's own BACKEND_PORT/PORT).
const FIXED_PORT = process.env.ROK_VK_PORT || '8154';
const NON_RETRYABLE_AUTH_EXIT_CODE = 42;
const AUTH_FAILURE_PATTERN = /Download failed:\s*HTTP\s*401\b.*manifest\.json/i;
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

function run(cmd, cmdArgs) {
  const child = spawn(cmd, cmdArgs, {
    stdio: ['inherit', 'pipe', 'pipe'],
    env,
  });
  let sawManifestAuthFailure = false;
  let outputTail = '';

  function forwardAndScan(stream, target) {
    stream.on('data', (chunk) => {
      target.write(chunk);
      const text = chunk.toString();
      outputTail = (outputTail + text).slice(-8192);
      if (AUTH_FAILURE_PATTERN.test(text) || AUTH_FAILURE_PATTERN.test(outputTail)) {
        sawManifestAuthFailure = true;
      }
    });
  }

  if (child.stdout) {
    forwardAndScan(child.stdout, process.stdout);
  }
  if (child.stderr) {
    forwardAndScan(child.stderr, process.stderr);
  }

  child.on('exit', (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
    } else {
      const normalizedCode = code == null ? 1 : code;
      if (normalizedCode !== 0 && sawManifestAuthFailure) {
        console.error(
          `[rok-vibe-kanban] non-retryable startup failure: unauthorized binary manifest download ` +
            `(HTTP 401). Ask the platform team to grant access or publish a public binary bundle.`
        );
        process.exit(NON_RETRYABLE_AUTH_EXIT_CODE);
      }
      process.exit(normalizedCode);
    }
  });
  child.on('error', (err) => {
    console.error(`[rok-vibe-kanban] failed to launch: ${err.message}`);
    process.exit(1);
  });
}

// Prefer the pinned dependency (single install, deterministic version).
// Fall back to npx if it isn't resolvable for some reason.
try {
  const pkgJsonPath = require.resolve('vibe-kanban-team/package.json');
  const pkg = require(pkgJsonPath);
  const binField = pkg.bin;
  const binRel =
    typeof binField === 'string' ? binField : binField[Object.keys(binField)[0]];
  const binAbs = path.join(path.dirname(pkgJsonPath), binRel);

  console.error(
    `[rok-vibe-kanban] launching vibe-kanban-team@${pkg.version} -> ${env.VK_SHARED_API_BASE} ` +
      `(local UI on http://127.0.0.1:${env.BACKEND_PORT || env.PORT})`
  );
  run(process.execPath, [binAbs, ...args]);
} catch (_e) {
  console.error(
    `[rok-vibe-kanban] pinned client not resolvable; falling back to npx vibe-kanban-team@${FALLBACK_VERSION}`
  );
  const npxCmd = process.platform === 'win32' ? 'npx.cmd' : 'npx';
  run(npxCmd, ['-y', `vibe-kanban-team@${FALLBACK_VERSION}`, ...args]);
}
