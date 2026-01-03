<img src="assets/logo.svg" width="128" alt="Patch the Okapi helps you find and fix lines.">

# Okapi: Find. Fix. Finished.

Sometimes you know how to identify lines which need editing, but crafting a replacement string is either hard or impossible. Enter **okapi**, a batch editor with a difference. Match lines from across thousands of files by regex, then edit them all in one temporary file with your $EDITOR. Then just save and close, and **okapi** does the rest.

This workflow is especially useful when you need to make careful edits across a large set of files where blind search-and-replace isn't enough.

---

## How it works

1. **Okapi** runs [**ripgrep**](https://github.com/BurntSushi/ripgrep) with a PCRE-compatible regex.
2. Each matching line is collected (up to a configurable limit).
3. All matching lines are opened together in your editor.
4. You edit the lines directly and save the file.
5. When the editor closes, Okapi applies your changes back to the source files.

Don't worry: if you changed a file in the meantime, okapi will print a warning and skip those edits. Each line is tracked with enough metadata to ensure it is written back to the correct file and line.

---

## Usage

### Finding lines

Find all lines in the current directory, recursively, containing "Anatome". Note that ripgrep is case sensitive by default.

```bash
okapi Anatome
```

Find all lines matching a regex. We use ripgrep in PCRE mode, so lookahead and lookbehind patterns are allowed.

```bash
okapi "(?<=Eastern )Leaves"
```

Find all lines which contain the pattern. The found pattern must start within the first 15 columns of text:

```bash
okapi "(Suffragette ?){2}" -c "..15"
```

Find all lines which contain the pattern, but _exclude_ lines that match a secondary pattern:

```bash
okapi "Hamilton" -e "Saskia|Alexander"
```

Use a case-insensitive search to find the pattern within the columns:

```bash
okapi "(TOOTIME){3}" -c "10..35" -i
```

Find matches only within Markdown files by passing a `--type` through to ripgrep:

```bash
okapi "Clearest Blue" -- --type md
```

### Editing lines

// TODO

## Requirements

* **ripgrep** (`rg`) must be installed and available in `PATH`
* An editor that can block until exit

---

## Notes and safety

* **Okapi** edits real files. Use version control.
* If the editor exits without saving (`:q!`), no changes are applied.
* Large match sets are intentionally capped. Presently, the number of matches is limited to 18,278, due to 3-character alphabetic aliases.

---

## License

See the project repository for license details.
