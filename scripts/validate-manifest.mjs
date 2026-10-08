import fs from 'node:fs';

const manifest = JSON.parse(fs.readFileSync('manifest/apps.json', 'utf8'));
const expected = ['photocraft', 'vectorcraft', 'designcraft', 'filmcraft', 'effectcraft', 'lightcraft', 'printcraft'];
if (manifest.schemaVersion !== 1) throw new Error('schemaVersion must be 1');
if (manifest.apps.length !== expected.length) throw new Error('manifest must contain exactly seven apps');
for (const id of expected) {
  const app = manifest.apps.find(value => value.id === id);
  if (!app) throw new Error(`missing ${id}`);
  const expectedRepository = id === 'printcraft' ? 'storytold/pdfcraft' : `storytold/${id}`;
  if (app.repository !== expectedRepository) throw new Error(`unexpected repository for ${id}`);
  for (const platform of ['windows-x64', 'macos-arm64', 'linux-x64', 'linux-arm64']) {
    const pattern = app.assetPatterns?.[platform];
    if (!pattern?.startsWith('^') || !pattern.endsWith('$')) throw new Error(`${id} ${platform} pattern must be anchored`);
    new RegExp(pattern.replace('{id}', id).replace('{version}', '0\\.0\\.0'), 'i');
  }
}
console.log('Manifest is valid.');
