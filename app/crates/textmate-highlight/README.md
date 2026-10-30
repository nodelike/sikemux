# textmate-highlight

Shiki's TextMate grammars, loaded lazily from one compressed bundle and highlighted
line by line on [irosashi](https://crates.io/crates/irosashi), whose tokens match
vscode-textmate's.

- `Library` reads a bundle: an index of grammars with the grammars each one needs,
  and every grammar compressed on its own. A language's grammars are unpacked the
  first time it is highlighted.
- `Styles` turns a token's scopes into a style the way a VS Code theme does: the
  deepest scope with a matching rule wins, and the longest selector among its rules.
- `Document` keeps each line's tokenizer state, so an edit retokenizes from the
  changed line until the state matches what it was.

Scope matching follows vscode-textmate's theme rules (MIT, Copyright (c) Microsoft
Corporation).
