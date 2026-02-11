<img src="assets/logo.svg" width="128" alt="Patch the Okapi helps you find and fix lines.">

# Okapi: Find it. Fix it. Finished.

Sometimes you know how to identify lines which need editing, but crafting a replacement string is either hard or
impossible. Enter **okapi**, a batch editor with a difference. Match lines from across thousands of files by regex, then
edit them all in one temporary file with your `$EDITOR`. Then just save and close, and **okapi** does the rest.

---

## How it works

1. **Okapi** runs [**ripgrep**](https://github.com/BurntSushi/ripgrep) with the provided pattern.
2. Each matching line is collected (up to a configurable limit).
3. All matching lines are opened together in your editor.
4. You edit the lines directly and save the file.
5. When the editor closes, Okapi applies your changes back to the source files.

Don't worry: if you changed a file in the meantime, okapi will print a warning and skip those edits. Each line is
tracked with enough metadata to ensure it is written back to the correct file and line.

[![asciicast](https://asciinema.org/a/Jzpw63nXDnMr0pF7.svg)](https://asciinema.org/a/Jzpw63nXDnMr0pF7)

---

## Usage

### Finding lines

Find all lines in the current directory, recursively, containing "Anatome". Note that `ripgrep` is case sensitive by
default.

```bash
okapi Anatome
```

Find all lines matching a regex.

```bash
okapi "Eastern L[eavs]+\b"
```

Find all lines which contain the pattern. The found pattern must start within the first 15 columns of text:

```bash
okapi "(Suffragette ?){2}" --columns "..15"
```

Find all lines which contain the pattern, but _exclude_ lines that match a secondary pattern:

```bash
okapi "(Saskia)? Hamilton" --exclude "Alexander"
```

Use a case-insensitive search to find the pattern within the range of character columns. The start index must fall
within the column range. The `--ignore-case` flag also affects any exclude patterns:

```bash
okapi "(tootime){3}" -c "10..35" --ignore-case
```

Any arguments that **okapi** doesn't handle are passed through to `ripgrep`. Here, the command finds matches only within
Markdown files by passing [a
`--type` argument](https://iepathos.github.io/ripgrep/manual-filtering-types/?h=type#basic-type-selection-t-type):

```bash
okapi "Clearest Blue" -- --type md
```

Here's a search using PCRE for
a [lookbehind](https://iepathos.github.io/ripgrep/advanced-patterns/lookaround/#lookaround-assertions):

```bash
okapi "(?<=The)\sGym" -- -P
```

### Editing lines

Edit the text just as you would any other text file. However, Okapi is line-based, so be sure not to add any linebreaks.

Once you're done, just save and quit. The files will be modified to match the lines in the temporary buffer.

## Requirements

* **ripgrep** (`rg`) must be installed and available in `PATH`
* You must have an editor which can block until exit

---

## Notes and safety

* **Okapi** edits real files. Use version control.
* Any changes saved to the virtual buffer will be persisted on a clean exit.
* If the editor exits without saving, or if no lines were changed, then the original files are untouched.
* If lines in the buffer have been changed but the editor exits with a nonzero exit status (e.g. `:cq!`), then you will
  be prompted to either persist the changes or save the abandoned buffer.
* Large match sets are intentionally capped at 1,000, which can be adjusted with `-m`. Presently, the number of matches
  is limited to 18,278, due to 3-character alphabetic aliases.

---

## License

See the project repository for license details.
