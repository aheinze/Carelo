#!/usr/bin/env node

import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const tauriBin = resolve(
  projectRoot,
  'node_modules',
  '.bin',
  process.platform === 'win32' ? 'tauri.cmd' : 'tauri',
);
const command = existsSync(tauriBin) ? tauriBin : 'tauri';
const env = { ...process.env };

if (process.platform === 'linux') {
  env.WEBKIT_DISABLE_COMPOSITING_MODE ??= '1';
  env.WEBKIT_DISABLE_DMABUF_RENDERER ??= '1';
}

const child = spawn(command, process.argv.slice(2), {
  cwd: projectRoot,
  env,
  shell: process.platform === 'win32' && !existsSync(tauriBin),
  stdio: 'inherit',
});

child.on('error', (error) => {
  console.error(`Unable to start Tauri CLI: ${error.message}`);
  process.exit(1);
});

child.on('exit', (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }

  process.exit(code ?? 1);
});
