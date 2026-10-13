#!/bin/sh
# Installs the extension for the current user (VS Code reads unpacked
# extensions from ~/.vscode/extensions; reload the window afterwards).
set -e
cd "$(dirname "$0")"
version=$(sed -n 's/.*"version": "\(.*\)".*/\1/p' package.json | head -1)
dest="$HOME/.vscode/extensions/plumb-lang.plumb-$version"
rm -rf "$dest"
mkdir -p "$dest/syntaxes"
cp package.json extension.js language-configuration.json README.md "$dest/"
cp syntaxes/plumb.tmLanguage.json "$dest/syntaxes/"
echo "installed in $dest; reload VS Code (Developer: Reload Window)"
