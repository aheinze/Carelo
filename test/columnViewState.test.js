import test from 'node:test';
import assert from 'node:assert/strict';
import {
  columnRefreshPaths,
  reconcileRefreshedColumnTrail,
} from '../src/utils/columnViewState.js';
import { isSameOrChildLocalPath, parentLocalPath } from '../src/utils/localPaths.js';

function entry(path, kind = 'file') {
  return {
    path,
    name: path.split('/').at(-1),
    kind,
  };
}

test('normalizes legacy and batched column refresh requests', () => {
  assert.deepEqual(columnRefreshPaths({ path: '/root/a' }), ['/root/a']);
  assert.deepEqual(
    columnRefreshPaths({ paths: ['/root/a', '/root/b', '/root/a', ''] }),
    ['/root/a', '/root/b'],
  );
});

test('removes a deleted file and clears its focused selection', () => {
  const gone = entry('/root/a/gone.txt');
  const keep = entry('/root/a/keep.txt');
  const result = reconcileRefreshedColumnTrail({
    trail: [{
      path: '/root/a',
      entries: [gone, keep],
      selectedIndex: 0,
      selectionAnchorIndex: 0,
      selectedPaths: [],
    }],
    columnIndex: 0,
    entries: [keep],
    visibleEntries: [keep],
    focusedPath: gone.path,
    anchorPath: gone.path,
  });

  assert.deepEqual(result.trail[0].entries, [keep]);
  assert.equal(result.trail[0].selectedIndex, -1);
  assert.equal(result.focusedEntry, null);
});

test('prunes descendant columns when their opening folder was deleted', () => {
  const folder = entry('/root/a/folder', 'directory');
  const result = reconcileRefreshedColumnTrail({
    trail: [
      {
        path: '/root/a',
        entries: [folder],
        selectedIndex: 0,
        selectionAnchorIndex: 0,
        selectedPaths: [],
      },
      {
        path: folder.path,
        entries: [entry(`${folder.path}/inside.txt`)],
        selectedIndex: -1,
        selectionAnchorIndex: -1,
        selectedPaths: [],
      },
    ],
    columnIndex: 0,
    entries: [],
    visibleEntries: [],
    childPaths: [],
    focusedPath: folder.path,
    anchorPath: folder.path,
  });

  assert.equal(result.trail.length, 1);
  assert.equal(result.descendantsPruned, true);
});

test('keeps an open folder selected by path when a sibling disappears', () => {
  const sibling = entry('/root/a/first.txt');
  const folder = entry('/root/a/folder', 'directory');
  const result = reconcileRefreshedColumnTrail({
    trail: [
      {
        path: '/root/a',
        entries: [sibling, folder],
        selectedIndex: 1,
        selectionAnchorIndex: 1,
        selectedPaths: [],
      },
      {
        path: folder.path,
        entries: [],
        selectedIndex: -1,
        selectionAnchorIndex: -1,
        selectedPaths: [],
      },
    ],
    columnIndex: 0,
    entries: [folder],
    visibleEntries: [folder],
    childPaths: [folder.path],
    focusedPath: folder.path,
    anchorPath: folder.path,
  });

  assert.equal(result.trail[0].selectedIndex, 0);
  assert.equal(result.trail.length, 2);
  assert.equal(result.descendantsPruned, false);
});

test('finds local parent directories for Unix and Windows paths', () => {
  assert.equal(parentLocalPath('/home/user/file.txt'), '/home/user');
  assert.equal(parentLocalPath('~/folder/file.txt'), '~/folder');
  assert.equal(parentLocalPath('C:\\Users\\Artur\\file.txt'), 'C:\\Users\\Artur');
  assert.equal(parentLocalPath('C:\\file.txt'), 'C:\\');
  assert.equal(parentLocalPath('C:/Users/Artur/file.txt'), 'C:/Users/Artur');
  assert.equal(parentLocalPath('\\\\server\\share\\folder\\file.txt'), '\\\\server\\share\\folder');
  assert.equal(parentLocalPath('/tmp/folder/file\\name.txt'), '/tmp/folder');
});

test('detects descendant transfer targets across Unix and Windows separators', () => {
  assert.equal(isSameOrChildLocalPath('/source/child', '/source'), true);
  assert.equal(isSameOrChildLocalPath('/tmp', '/'), true);
  assert.equal(isSameOrChildLocalPath('/source-sibling', '/source'), false);
  assert.equal(
    isSameOrChildLocalPath('C:\\Users\\Artur\\Source\\Child', 'c:/users/artur/source'),
    true,
  );
  assert.equal(
    isSameOrChildLocalPath('C:\\Users\\Artur\\Source-2', 'C:\\Users\\Artur\\Source'),
    false,
  );
  assert.equal(
    isSameOrChildLocalPath('\\\\server\\share\\folder\\child', '\\\\SERVER\\SHARE\\folder'),
    true,
  );
  // A backslash remains a valid filename character for POSIX-style paths.
  assert.equal(isSameOrChildLocalPath('/tmp/folder/file\\name', '/tmp/folder/file'), false);
});
