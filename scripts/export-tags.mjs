#!/usr/bin/env node
/**
 * 从真实库导出已翻译标签到 docs/tag-translations.json，作为库文件之外的可重建源。
 *
 * 用法：
 *   node scripts/export-tags.mjs                 # 依次取 ARCHIVE_GALLERY_LIBRARY / HUAXIA_LIBRARY / scripts/.local-library
 *   node scripts/export-tags.mjs --library <dir>
 *
 * 数据源：galleryctl tags list --status translated（分页游标取全量）。
 * 输出与 docs/tag-translations.json 现有格式完全同构：[{ id, translatedName, source }]。
 * `npm run release` 成功后自动跑一次（见 release.mjs 尾部），平时可手动执行。
 */
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const ROOT = path.resolve(import.meta.dirname, '..');
const DOCS_OUT = path.join(ROOT, 'docs', 'tag-translations.json');
/**
 * 库路径来源（按优先级）：--library 参数 → 环境变量 → scripts/.local-library。
 * 最后一个文件是**本机私有**的（已在 .gitignore 里），公开仓库里不带任何个人路径，
 * 但本地跑 release 时仍能自动找到库。
 */
function localLibraryFile() {
  try {
    const file = path.join(ROOT, 'scripts', '.local-library');
    if (fs.existsSync(file)) return fs.readFileSync(file, 'utf8').trim();
  } catch {
    // 读不到就当没配
  }
  return '';
}

const argv = process.argv.slice(2);
const libraryIndex = argv.indexOf('--library');
const library =
  libraryIndex >= 0
    ? argv[libraryIndex + 1]
    : process.env.ARCHIVE_GALLERY_LIBRARY ||
      process.env.HUAXIA_LIBRARY ||
      localLibraryFile();

if (!library) {
  console.error(
    '✗ 不知道库在哪：用 --library <目录>，或设置 ARCHIVE_GALLERY_LIBRARY，或把路径写进 scripts/.local-library',
  );
  process.exit(1);
}

function findGalleryctl() {
  const candidates = [
    process.env.GALLERYCTL,
    path.join(ROOT, 'src-tauri', 'target', 'release', 'galleryctl.exe'),
    path.join(ROOT, 'src-tauri', 'target', 'debug', 'galleryctl.exe'),
  ].filter(Boolean);
  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) return candidate;
  }
  throw new Error('找不到 galleryctl.exe，请先 npm run build:cli 或 npm run release');
}

function fetchAllTranslated(galleryctl) {
  const tags = [];
  let after = null;
  for (let page = 0; page < 50; page += 1) {
    const args = ['--library', library, 'tags', 'list', '--status', 'translated', '--limit', '200'];
    if (after !== null) args.push('--after', String(after));
    const raw = execFileSync(galleryctl, args, { encoding: 'utf8' });
    const payload = JSON.parse(raw);
    if (!payload.ok) throw new Error(payload.error || 'galleryctl 返回失败');
    tags.push(...(payload.data.tags ?? []));
    const next = payload.data.nextAfter;
    if (next === null || next === undefined) break;
    after = next;
  }
  return tags;
}

const galleryctl = findGalleryctl();
const tags = fetchAllTranslated(galleryctl);
const translated = tags
  .filter((tag) => typeof tag.translatedName === 'string' && tag.translatedName.length > 0)
  .map((tag) => ({ id: tag.id, translatedName: tag.translatedName, source: tag.source ?? 'ai' }));

if (translated.length === 0) {
  throw new Error('库里没有任何已翻译标签，拒绝用空文件覆盖 docs/tag-translations.json');
}

fs.writeFileSync(DOCS_OUT, `${JSON.stringify(translated, null, 1)}\n`, 'utf8');
console.log(`✓ 已导出 ${translated.length} 条译名 → ${path.relative(ROOT, DOCS_OUT)}（库：${library}）`);
