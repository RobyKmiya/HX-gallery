#!/usr/bin/env node
/**
 * 一键发布：抬版本 → 同步三处版本号 → 类型检查 → 关旧实例 → 打包 → 安装 → 验证。
 *
 * 用法：
 *   node scripts/release.mjs            # 补丁位 +1（1.0.0 → 1.0.1）
 *   node scripts/release.mjs --minor    # 次版本 +1（1.0.1 → 1.1.0）
 *   node scripts/release.mjs --major    # 主版本 +1（1.1.0 → 2.0.0）
 *   node scripts/release.mjs --set 1.2.3
 *   node scripts/release.mjs --dry-run  # 只打印要做什么
 *
 * 背景（务必读）：`npm run build` 只编前端 dist，不会重编 Rust；
 * 桌面程序必须走 `tauri build` 才会产出新 exe。这个脚本强制走完整链路，
 * 并在打包前结束正在运行的实例（否则 Windows 会锁住 exe，链接阶段直接失败）。
 */
import { execFileSync, execSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';

const ROOT = path.resolve(import.meta.dirname, '..');
const TAURI_DIR = path.join(ROOT, 'src-tauri');
const PKG = path.join(ROOT, 'package.json');
const CARGO = path.join(TAURI_DIR, 'Cargo.toml');
const CONF = path.join(TAURI_DIR, 'tauri.conf.json');
const INSTALL_DIR = path.join(os.homedir(), 'AppData', 'Local', '画匣');
const INSTALLED_EXE = path.join(INSTALL_DIR, 'archive-gallery.exe');

const argv = process.argv.slice(2);
const dryRun = argv.includes('--dry-run');
const bumpKind = argv.includes('--major')
  ? 'major'
  : argv.includes('--minor')
    ? 'minor'
    : 'patch';
const setIndex = argv.indexOf('--set');
const explicitVersion = setIndex >= 0 ? argv[setIndex + 1] : null;

const log = (msg) => console.log(`\n▸ ${msg}`);
const ok = (msg) => console.log(`  ✓ ${msg}`);
const warn = (msg) => console.log(`  ! ${msg}`);

// 打包的是工作区现状：有未提交改动时警告（2026-11-19 教训——
// 退出后自动发布曾把未验收的半成品改动编进安装包）。只警告不阻止，
// 因为从应用面板触发的自动发布没法在这里交互确认。
function warnIfDirtyTree() {
  try {
    const status = execSync('git status --porcelain', { cwd: ROOT, encoding: 'utf8' }).trim();
    if (status) {
      warn('工作区有未提交改动，它们会被打包进这个版本：');
      for (const line of status.split('\n').slice(0, 10)) console.log(`    ${line}`);
      if (status.split('\n').length > 10) console.log('    …');
    }
  } catch {
    // 不在 git 仓库或 git 不可用：跳过检查
  }
}

// ── 关窗更新悬浮进度窗的数据源 ────────────────────────────
// release.rs 生成的 release-progress.ps1（PowerShell WinForms，置顶、可拖动、
// 可最小化成小条）每 0.5 秒读这个文件画进度条。除阶段名外还写各阶段的
// 「预计时长」（按本机历史构建校准，2026-11-20 实测全程约 21 分钟，大头是
// cargo 编译），HUD 用它做加权进度和「预计剩约」——7 个阶段等分会让进度条
// 在 cargo 编译那一步停十几分钟，看起来像卡死。只影响显示，写不进去绝不
// 影响发布本身：HUD 只是个观察窗。
const RELEASE_DIR = path.join(
  os.homedir(),
  'AppData',
  'Roaming',
  'com.archivegallery.desktop',
  'release',
);
const PROGRESS_FILE = path.join(RELEASE_DIR, 'progress.json');
const STAGE_TOTAL = 7;
// 每阶段预计秒数，与下面 7 个 step() 调用一一对应。快了 ETA 偏乐观、
// 慢了 HUD 会标注「本步比平时慢」——只影响进度观感，不改变发布行为。
const STAGE_EXPECTED_SEC = [2, 30, 240, 3, 1080, 15, 10];
const TOTAL_EXPECTED_SEC = STAGE_EXPECTED_SEC.reduce((sum, sec) => sum + sec, 0);
let stageIndex = 0;
let doneExpectedSec = 0;
let startedAt = Date.now();

function writeProgress(extra = {}) {
  try {
    fs.mkdirSync(RELEASE_DIR, { recursive: true });
    fs.writeFileSync(
      PROGRESS_FILE,
      JSON.stringify({
        startedAt,
        stageIndex,
        stageTotal: STAGE_TOTAL,
        at: Date.now(),
        ...extra,
      }),
      'utf8',
    );
  } catch {
    // 进度窗读不到就退化为「只看到时长在走」，发布流程不受影响
  }
}

function step(title, expectedSec, fn) {
  if (!dryRun) {
    writeProgress({
      stage: title,
      version: next,
      stageStartedAt: Date.now(),
      stageExpectedSec: expectedSec,
      doneExpectedSec,
      totalExpectedSec: TOTAL_EXPECTED_SEC,
    });
  }
  stageIndex += 1;
  log(title);
  if (dryRun) {
    console.log('  (dry-run，跳过执行)');
    return;
  }
  fn();
  doneExpectedSec += expectedSec;
}

// ── 版本号 ────────────────────────────────────────────────

function bump(version, kind) {
  const [major, minor, patch] = version.split('.').map(Number);
  if (kind === 'major') return `${major + 1}.0.0`;
  if (kind === 'minor') return `${major}.${minor + 1}.0`;
  return `${major}.${minor}.${patch + 1}`;
}

function readVersion() {
  return JSON.parse(fs.readFileSync(PKG, 'utf8')).version;
}

/** 三处版本号必须一致，否则 Tauri 会按 conf 里的版本打包，而 Cargo 里是另一个。 */
function syncVersion(next) {
  const pkg = JSON.parse(fs.readFileSync(PKG, 'utf8'));
  const before = pkg.version;
  pkg.version = next;
  fs.writeFileSync(PKG, `${JSON.stringify(pkg, null, 2)}\n`, 'utf8');

  const cargo = fs.readFileSync(CARGO, 'utf8');
  const cargoNext = cargo.replace(
    /^(version\s*=\s*)"[^"]+"/m,
    `$1"${next}"`,
  );
  if (cargoNext === cargo) throw new Error('Cargo.toml 里没找到 version 字段');
  fs.writeFileSync(CARGO, cargoNext, 'utf8');

  const conf = JSON.parse(fs.readFileSync(CONF, 'utf8'));
  conf.version = next;
  fs.writeFileSync(CONF, `${JSON.stringify(conf, null, 2)}\n`, 'utf8');

  return { before, after: next };
}

// ── 进程与路径 ────────────────────────────────────────────

/** 结束所有 archive-gallery 进程。打包前不关，Windows 会锁住 exe 让链接失败。 */
function killRunning() {
  let out = '';
  try {
    out = execSync(
      'tasklist /FI "IMAGENAME eq archive-gallery.exe" /NH',
      { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] },
    );
  } catch {
    return 0;
  }
  const pids = [...out.matchAll(/\barchive-gallery\.exe\s+(\d+)/gi)].map((m) =>
    Number(m[1]),
  );
  for (const pid of pids) {
    try {
      execSync(`taskkill /PID ${pid} /F`, { stdio: 'ignore' });
    } catch {
      // 已经退出就忽略
    }
  }
  return pids.length;
}

function run(cmd, args, cwd = ROOT) {
  execFileSync(cmd, args, { cwd, stdio: 'inherit', shell: false });
}

function runNpm(script) {
  // Windows 上 npm 是 .cmd，Node 24 起 spawnSync 直接执行 .cmd 会报 EINVAL，
  // 必须透过 shell 调用。
  if (process.platform === 'win32') {
    execSync(`npm run ${script}`, { cwd: ROOT, stdio: 'inherit' });
  } else {
    run('npm', ['run', script]);
  }
}

// ── 主流程 ────────────────────────────────────────────────

try {
const current = readVersion();
const next = explicitVersion ?? bump(current, bumpKind);

if (explicitVersion && !/^\d+\.\d+\.\d+$/.test(explicitVersion)) {
  console.error(`版本号格式应为 x.y.z，收到：${explicitVersion}`);
  process.exit(1);
}

console.log('画匣 · 一键发布');
console.log(`  当前版本 ${current} → ${next}${dryRun ? '（dry-run）' : ''}`);

warnIfDirtyTree();

step(`抬版本并同步三处（package.json / Cargo.toml / tauri.conf.json）`, 2, () => {
  syncVersion(next);
  ok(`三处已同步为 ${next}`);
});

step('前端类型检查', 30, () => {
  runNpm('check');
  ok('tsc 通过');
});

step('Rust 单元测试', 240, () => {
  run('cargo', ['test', '--manifest-path', path.join(TAURI_DIR, 'Cargo.toml'), '--lib']);
  ok('测试通过');
});

step('结束正在运行的实例', 3, () => {
  const count = killRunning();
  ok(count > 0 ? `已结束 ${count} 个进程` : '没有运行中的实例');
  // 给系统一点时间释放文件句柄
  execSync(
    'powershell -NoProfile -Command "Start-Sleep -Milliseconds 800"',
    { stdio: 'ignore' },
  );
});

step('打包（含 Rust release 编译）', 1080, () => {
  runNpm('build:bundle');
  ok('安装包已生成');
});

const setupPath = path.join(
  TAURI_DIR,
  'target',
  'release',
  'bundle',
  'nsis',
  `画匣_${next}_x64-setup.exe`,
);

step(`安装到 ${INSTALL_DIR}`, 15, () => {
  if (!fs.existsSync(setupPath)) {
    throw new Error(`找不到安装包：${setupPath}`);
  }
  run(setupPath, ['/S']);
  ok('安装完成');
});

step('验证', 10, () => {
  if (!fs.existsSync(INSTALLED_EXE)) {
    throw new Error(`安装后找不到 exe：${INSTALLED_EXE}`);
  }
  const stat = fs.statSync(INSTALLED_EXE);
  ok(`安装版 exe 时间戳 ${stat.mtime.toLocaleString()}`);

  // 版本号应写进 package.json 且与安装包名一致
  const installedVersion = readVersion();
  if (installedVersion !== next) {
    throw new Error(`版本不一致：${installedVersion} ≠ ${next}`);
  }
  ok(`版本号校验一致：${next}`);

  // 检查 exe 没被残留进程锁住
  try {
    const fd = fs.openSync(INSTALLED_EXE, 'r+');
    fs.closeSync(fd);
    ok('exe 未被锁定');
  } catch {
    warn('exe 仍被占用，可能还有进程没退干净');
  }
});

if (dryRun) {
  console.log('\n(dry-run 结束，未改动任何文件)');
} else {
  console.log(`\n完成。新版 ${next} 已安装，直接双击桌面「画匣」就是最新版。`);
  console.log(`安装包留档：${setupPath}`);

  // 把「抬版本」本身提交掉，让工作区回到干净状态。
  // 不提交的话 release.rs 的关窗判断会一直看到 dirty，
  // 形成「关窗 → 发布 → 抬版本 → 又脏 → 又发布」的自激循环（2026-11-20 连环闪黑窗）。
  // 只 add 这 4 个版本文件，绝不碰用户其他改动。
  try {
    const versionFiles = [
      'package.json',
      'src-tauri/Cargo.toml',
      'src-tauri/Cargo.lock',
      'src-tauri/tauri.conf.json',
    ];
    execSync(`git add ${versionFiles.join(' ')}`, { cwd: ROOT, stdio: 'ignore' });
    const staged = execSync('git diff --cached --name-only', { cwd: ROOT, encoding: 'utf8' }).trim();
    if (staged) {
      execSync(`git commit -m "chore: 发布 ${next}"`, { cwd: ROOT, stdio: 'inherit' });
      ok(`版本文件已提交（${staged.split('\n').length} 个），工作区不再因此变脏`);
    }
  } catch (error) {
    warn(`提交版本文件失败（不影响安装，但下次关窗可能再次触发发布）: ${String(error)}`);
  }

  // 发布成功后顺手导出真实库的标签译名快照（docs/tag-translations.json），
  // 防止译名只存在库文件单点丢失。失败只警告，不影响发布结果。
  try {
    execFileSync('node', [path.join(ROOT, 'scripts', 'export-tags.mjs')], { stdio: 'inherit' });
  } catch (error) {
    warn(`标签译名导出失败（不影响发布）: ${String(error)}`);
  }
}

if (!dryRun) writeProgress({ stage: '更新完成', result: 'success', version: next, stageIndex: STAGE_TOTAL });
} catch (error) {
  if (!dryRun) {
    writeProgress({
      stage: '更新失败',
      result: 'failed',
      error: String(error && error.message ? error.message : error),
    });
  }
  console.error(`
发布失败：${String(error && error.message ? error.message : error)}`);
  process.exit(1);
}
