# Homebrew tap for `x`

This directory holds the Homebrew formula for `x`. A tap is a separate Git
repository whose `Formula/` folder Homebrew reads from.

## Create the tap repository

1. Create a new repo, e.g. `xsys/homebrew-tap`.
2. Copy `Formula/x.rb` into it (keep the `Formula/` path).
3. Push.

## Use it on a machine

```sh
# Point Homebrew at the tap once:
brew tap xsys/tap https://github.com/xsys/homebrew-tap

# Build and install the latest main branch:
brew install --head x

# Or, once a tagged release exists, build the stable release:
brew install x
```

## Updating on a new release

The formula ships a `head` (main-branch) build so it works before any release
exists. Once you cut a tag such as `v0.2.0`:

1. Edit `Formula/x.rb`: uncomment the `url`/`sha256` lines and point `url` at
   `https://github.com/wangmingfa/x/archive/refs/tags/v0.2.0.tar.gz`.
2. Compute the checksum:

   ```sh
   curl -fsSL https://github.com/wangmingfa/x/archive/refs/tags/v0.2.0.tar.gz | sha256sum
   ```

3. Paste the checksum into `sha256`.
4. Bump the repo's `version` if the formula declares one and commit.

Homebrew will then prefer the stable `url` over `head` for plain `brew install x`.
