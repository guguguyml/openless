import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const appRoot = join(scriptDir, '..');

function readSource(relativePath) {
  return readFileSync(join(appRoot, relativePath), 'utf8');
}

test('sync section wires login logout manual sync actions', () => {
  const source = readSource('src/pages/settings/SyncSection.tsx');

  for (const symbol of [
    'syncRequestEmailCode',
    'syncVerifyEmailCode',
    'syncPull',
    'syncPushPending',
    'syncLogoutDevice',
    'syncClearCloudData',
  ]) {
    assert.ok(source.includes(symbol), `missing sync action: ${symbol}`);
  }

  for (const action of ['requestCode', 'verifyLogin', 'manualSync', 'logout', 'clearCloud']) {
    assert.ok(source.includes(`const ${action} = async`), `missing handler: ${action}`);
  }

  assert.ok(
    source.includes('await syncPull();') && source.includes('await syncPushPending();'),
    'login flow should pull before pushing pending changes',
  );
  assert.ok(
    source.includes("if (!window.confirm(t(\"settings.sync.clearConfirmFirst\"))) return;"),
    'clear flow should keep the first confirmation',
  );
  assert.ok(
    source.includes('const DEFAULT_SYNC_SERVER_URL = "https://sync.example.com";'),
    'sync section should keep the official default server placeholder',
  );
});

test('sync ipc mock layer keeps style packs history and offline queue state', () => {
  const source = readSource('src/lib/ipc.ts');

  for (const symbol of [
    'syncMockSettingsFromStylePacks',
    'listStylePacks',
    'saveStylePack',
    'setActiveStylePack',
    'syncPull',
    'syncPushPending',
    'syncLogoutDevice',
    'syncClearCloudData',
    'listHistory',
    'readAudioRecording',
    'listVocab',
    'listCorrectionRules',
    'listVocabPresets',
    'enqueueSyncChange',
    'replaceSyncQueue',
  ]) {
    assert.ok(source.includes(symbol), `missing ipc symbol: ${symbol}`);
  }

  assert.ok(source.includes('mockSyncState = {'), 'mock sync state should stay in-memory');
  assert.ok(source.includes('mockSyncAuthSession = null;'), 'logout should clear auth state');
  assert.ok(
    source.includes('cursor: mockSyncState.pendingChanges.length ? \'2\' : mockSyncState.cursor'),
    'push pending should preserve queue state when empty',
  );
});

test('settings page still mounts the sync section', () => {
  const source = readSource('src/pages/Settings.tsx');
  assert.ok(
    source.includes("section === 'sync' && <SyncSection />"),
    'settings page should render the sync section',
  );
});
