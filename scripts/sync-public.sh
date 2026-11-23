#!/usr/bin/env bash
# 从 master 重建公开分支 gh-public（自动排除内部文档、构建产物、安装包）。
# 用法：改完代码、提交到 master 之后跑一次，然后 git push origin gh-public:main
set -e
cd "$(dirname "$0")/.."

VERSION=$(node -p "require('./package.json').version")
git checkout -q master

# 有未提交改动就别动分支，避免把半成品带进公开分支
if [ -n "$(git status --porcelain)" ]; then
  echo "✗ 工作区有未提交改动，先提交再同步"; exit 1
fi

git branch -D gh-public >/dev/null 2>&1 || true
git checkout -q --orphan gh-public
git rm -r --cached . -q

# 白名单：只收应用本体和使用者文档
git add README.md LICENSE .gitignore package.json package-lock.json \
        tsconfig.json tsconfig.node.json vite.config.ts index.html \
        public src scripts \
        "docs/使用说明.md" "docs/使用说明.html" \
        src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/build.rs \
        src-tauri/tauri.conf.json src-tauri/src src-tauri/icons src-tauri/capabilities

git commit -q -m "画匣 发布快照 v$VERSION" -m "由 scripts/sync-public.sh 从 master 生成；不含内部文档、本地路径、构建产物。"
COUNT=$(git ls-tree -r --name-only HEAD | wc -l | tr -d ' ')
git checkout -f -q master

echo "✓ gh-public 已重建：v$VERSION，$COUNT 个文件"
echo "  推送：git push origin gh-public:main"
