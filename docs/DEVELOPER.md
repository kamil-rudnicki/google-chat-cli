# Developer

```bash
git add .
git commit -m "Initial gchat CLI"

gh repo create <owner>/google-chat-cli --private --source=. --remote=origin --push
gh repo create <owner>/homebrew-gchat --public --clone=false
gh secret set HOMEBREW_TAP_TOKEN --repo <owner>/google-chat-cli

git tag v1.0.0
git push origin main v1.0.0
```

```bash
brew tap <owner>/gchat
brew install gchat
gchat --version
gchat chat list
```

```bash
./target/release/gchat auth list

echo 'export PATH="/Users/kamil/Developer/google-chat-cli/target/release:$PATH"' >> ~/.zshrc
source ~/.zshrc
```

## Install From Source

```bash
cargo install --path .
```

The binary is named `gchat`.

## First GitHub Release

1. Create the GitHub repo and push `main`.

   ```bash
   gh repo create <owner>/google-chat-cli --private --source=. --remote=origin --push
   ```

2. Create a Homebrew tap repo. Homebrew's `owner/gchat` tap name maps to a GitHub repo named `homebrew-gchat`.

   ```bash
   gh repo create <owner>/homebrew-gchat --public --clone=false
   ```

3. Create a fine-grained GitHub token that can write contents to `<owner>/homebrew-gchat`, then add it to the main repo as `HOMEBREW_TAP_TOKEN`.

   ```bash
   gh secret set HOMEBREW_TAP_TOKEN --repo <owner>/google-chat-cli
   ```

4. Tag the first version. The tag must match `Cargo.toml` without the leading `v`.

   ```bash
   git tag v1.0.0
   git push origin main v1.0.0
   ```

The release workflow builds archives, creates a GitHub Release, and updates `Formula/gchat.rb` in the tap repo when `HOMEBREW_TAP_TOKEN` is configured.

## Brew

For local formula testing before release:

```bash
brew install --HEAD ./Formula/gchat.rb
```
