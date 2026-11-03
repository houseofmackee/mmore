# Submitting mmore to homebrew-core

Once mmore is in homebrew-core, `brew install mmore` works without a tap.
`mmore.rb.in` is the formula for that: unlike the one in
[houseofmackee/homebrew-tap](https://github.com/houseofmackee/homebrew-tap), it
builds from source, as homebrew-core requires, and its test checks that Markdown
is rendered, plain text is passed through, and compressed files are read.

## When

homebrew-core only accepts projects that are notable enough:

- the repository is at least 30 days old (mmore's is from 2026-11-02, so from
  2026-12-02 on), and
- it has at least 75 stars, 30 forks, or 30 watchers when someone else submits
  it, or 225 stars when the author does.

## How

1. Release a version that has `--completions` and `--man` (0.5.0 or later); the
   formula installs both.
2. Print the formula for that version (this downloads the source archive to
   compute its checksum):

   ```sh
   scripts/homebrew_core_formula.sh 0.5.0 > mmore.rb
   ```

3. In a fork of [Homebrew/homebrew-core](https://github.com/Homebrew/homebrew-core),
   put it at `Formula/m/mmore.rb` and check it:

   ```sh
   HOMEBREW_NO_INSTALL_FROM_API=1 brew install --build-from-source mmore
   brew test mmore
   brew audit --strict --new --online mmore
   brew style mmore
   ```

4. Open a pull request titled `mmore 0.5.0 (new formula)`. Later releases are
   picked up by Homebrew's own version checks, so they need no pull requests.

Once it is accepted, `brew install mmore` installs the homebrew-core formula,
even with the tap added. The tap's formula can then be removed, and the release
workflow's step that updates it with it.
