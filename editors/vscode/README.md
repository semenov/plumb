# Plumb for VS Code

Highlighting for `.plumb` files, and through `plumb lsp`: errors as you
type, completion after `.` (fields and methods of a value, functions of a
module, with their doc comments), hover with types and docs, and go to
definition (F12) in the project's files.

## Install

```sh
editors/vscode/install.sh      # copies it to ~/.vscode/extensions
```

then reload VS Code (Developer: Reload Window). The extension runs
`plumb`: from the `plumb.path` setting, the PATH, or `~/.plumb/bin/plumb`.
`Plumb: Restart the language server` picks up a rebuilt compiler.
