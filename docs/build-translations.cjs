// 生成 galleryctl tags translate 的输入：把 docs/tag-translation-map.json 与库内标签求交集。
// 用法：node docs/build-translations.cjs
const { execFileSync } = require('child_process');
const fs = require('fs');
const path = require('path');
const os = require('os');

const ROOT = path.resolve(__dirname, '..');
const LIBRARY = process.env.ARCHIVE_GALLERY_LIBRARY || 'F:/Hthings/BENZI';
const CLI = path.join(ROOT, 'src-tauri', 'target', 'debug', 'galleryctl.exe');

// 自己向 CLI 要标签，不依赖外部临时文件
function loadTags() {
  const raw = execFileSync(
    CLI,
    ['--library', LIBRARY, '--compact', 'tags', 'list', '--status', 'active', '--limit', '500'],
    { encoding: 'utf8', maxBuffer: 16 * 1024 * 1024 },
  );
  return JSON.parse(raw.replace(/^\uFEFF/, '')).data.tags;
}

const map = JSON.parse(fs.readFileSync(path.join(__dirname, 'tag-translation-map.json'), 'utf8'));

const flat = {};
for (const section of ['verified', 'inferred', 'booru', 'keep_as_is']) {
  for (const [k, v] of Object.entries(map[section] || {})) {
    if (k.startsWith('_')) continue;
    flat[k.toLowerCase()] = v;
  }
}

const tags = loadTags();
const translations = [];
const unmatched = [];
const noop = [];

for (const t of tags) {
  const zh = flat[String(t.name).toLowerCase()];
  if (zh === undefined) {
    unmatched.push(t.name);
    continue;
  }
  if (zh === t.name) {
    noop.push(`${t.name} (id ${t.id})`);
    continue;
  }
  translations.push({ id: t.id, translatedName: zh, source: 'ai' });
}

const out = path.join(__dirname, 'tag-translations.json');
fs.writeFileSync(out, JSON.stringify(translations, null, 1), 'utf8');

console.log('library        :', LIBRARY);
console.log('tags total     :', tags.length);
console.log('translatable   :', translations.length);
console.log('identical(noop):', noop.length, noop.join(' | '));
console.log('UNMATCHED      :', unmatched.length, unmatched.join(' | '));
console.log('wrote          :', out);
