#!/usr/bin/env node

import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const today = new Date().toISOString().slice(0, 10);
const args = process.argv.slice(2);
const version = args.find((arg) => !arg.startsWith('-'));
const dryRun = args.includes('--dry-run');
const noCommit = args.includes('--no-commit');
const noTag = args.includes('--no-tag');
const help = args.includes('--help') || args.includes('-h');
const date = getOptionValue('--date') || today;
const rpmRelease = getOptionValue('--rpm-release') || '1';
const tagName = version ? `v${version}` : '';

const files = {
  packageJson: 'package.json',
  packageLock: 'package-lock.json',
  cargoToml: 'src-tauri/Cargo.toml',
  cargoLock: 'src-tauri/Cargo.lock',
  tauriConfig: 'src-tauri/tauri.conf.json',
  metainfo: 'src-tauri/linux/dev.carelo.filemanager.metainfo.xml',
};

if (help || !version) {
  printHelp();
  process.exit(help ? 0 : 1);
}

if (!isValidVersion(version)) {
  fail(`Invalid version "${version}". Use a SemVer value such as 0.4.1.`);
}

if (!/^\d{4}-\d{2}-\d{2}$/.test(date)) {
  fail(`Invalid date "${date}". Use YYYY-MM-DD.`);
}

if (!/^\d+$/.test(rpmRelease)) {
  fail(`Invalid RPM release "${rpmRelease}". Use a positive integer.`);
}

if (noCommit && !noTag) {
  fail('Use --no-tag with --no-commit so the tag does not point at an unreleased commit.');
}

if (!dryRun) {
  assertCleanWorktree();
  if (!noTag) {
    assertTagDoesNotExist(tagName);
  }
}

const updates = [
  updatePackageJson(),
  updatePackageLock(),
  updateCargoToml(),
  updateCargoLock(),
  updateTauriConfig(),
  updateMetainfo(),
].filter(Boolean);

if (dryRun) {
  log(`Would update ${updates.length} files for ${tagName}:`);
  updates.forEach((file) => log(`- ${file}`));
  log(noCommit ? 'Would not create a commit.' : `Would commit: chore: release ${tagName}`);
  log(noTag ? 'Would not create a tag.' : `Would create tag: ${tagName}`);
  process.exit(0);
}

if (updates.length > 0 && !noCommit) {
  git(['add', ...updates]);

  if (hasStagedChanges()) {
    git(['commit', '-m', `chore: release ${tagName}`]);
  }
}

if (!noTag) {
  assertTagDoesNotExist(tagName);
  git(['tag', '-a', tagName, '-m', `Carelo ${tagName}`]);
}

const branch = gitOutput(['branch', '--show-current']) || 'HEAD';
const pushTarget = noTag ? branch : `${branch} ${tagName}`;
log(`Prepared ${tagName}. Push it with: git push origin ${pushTarget}`);

function getOptionValue(name) {
  const exact = args.indexOf(name);

  if (exact !== -1) {
    return args[exact + 1];
  }

  const prefix = `${name}=`;
  const arg = args.find((item) => item.startsWith(prefix));

  return arg ? arg.slice(prefix.length) : '';
}

function updatePackageJson() {
  const file = files.packageJson;
  const json = readJson(file);
  json.version = version;
  writeJson(file, json);
  return file;
}

function updatePackageLock() {
  const file = files.packageLock;
  const json = readJson(file);
  json.version = version;

  if (json.packages?.['']) {
    json.packages[''].version = version;
  }

  writeJson(file, json);
  return file;
}

function updateCargoToml() {
  const file = files.cargoToml;
  const next = readText(file).replace(
    /(^\[package\][\s\S]*?^version\s*=\s*")[^"]+(")/m,
    `$1${version}$2`,
  );
  writeText(file, next);
  return file;
}

function updateCargoLock() {
  const file = files.cargoLock;
  const next = readText(file).replace(
    /(\[\[package\]\]\nname = "carelo"\nversion = ")[^"]+(")/,
    `$1${version}$2`,
  );
  writeText(file, next);
  return file;
}

function updateTauriConfig() {
  const file = files.tauriConfig;
  const json = readJson(file);
  json.version = version;

  if (json.bundle?.linux?.rpm) {
    json.bundle.linux.rpm.release = rpmRelease;
  }

  writeJson(file, json);
  return file;
}

function updateMetainfo() {
  const file = files.metainfo;
  const entry = `    <release version="${version}" date="${date}" />`;
  const existingEntryPattern = new RegExp(
    `^\\s*<release version="${escapeRegExp(version)}" date="[^"]*" \\/>\\n?`,
    'm',
  );
  let xml = readText(file).replace(existingEntryPattern, '');

  if (xml.includes('<releases>')) {
    xml = xml.replace(/(<releases>\n)/, `$1${entry}\n`);
  } else {
    xml = xml.replace(
      /(\s*<content_rating[\s\S]*?\/>\n)/,
      `$1  <releases>\n${entry}\n  </releases>\n`,
    );
  }

  writeText(file, xml);
  return file;
}

function readJson(file) {
  return JSON.parse(readText(file));
}

function writeJson(file, value) {
  writeText(file, `${JSON.stringify(value, null, 2)}\n`);
}

function readText(file) {
  return readFileSync(resolve(root, file), 'utf8');
}

function writeText(file, value) {
  if (dryRun) {
    return;
  }

  writeFileSync(resolve(root, file), value);
}

function git(argsForGit) {
  execFileSync('git', argsForGit, { cwd: root, stdio: 'inherit' });
}

function gitOutput(argsForGit) {
  return execFileSync('git', argsForGit, { cwd: root, encoding: 'utf8' }).trim();
}

function hasStagedChanges() {
  try {
    execFileSync('git', ['diff', '--cached', '--quiet'], { cwd: root });
    return false;
  } catch (error) {
    if (error.status === 1) {
      return true;
    }

    throw error;
  }
}

function assertCleanWorktree() {
  const status = gitOutput(['status', '--porcelain']);

  if (status) {
    fail('Worktree is not clean. Commit or stash current changes before preparing a release.');
  }
}

function assertTagDoesNotExist(tag) {
  try {
    gitOutput(['rev-parse', '--verify', `refs/tags/${tag}`]);
    fail(`Tag ${tag} already exists.`);
  } catch (error) {
    if (error.status === 0) {
      throw error;
    }
  }
}

function isValidVersion(value) {
  return /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(value);
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function printHelp() {
  log(`Usage: npm run release:prepare -- <version> [options]

Updates release version files, commits the changes, and creates an annotated tag.

Options:
  --date YYYY-MM-DD       Release date for AppStream metadata. Defaults to today.
  --rpm-release N         RPM package release value. Defaults to 1.
  --dry-run               Show planned changes without writing files.
  --no-commit             Update files without creating a release commit.
  --no-tag                Update files without creating a git tag.

Example:
  npm run release:prepare -- 0.4.1`);
}

function log(message) {
  console.log(message);
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
