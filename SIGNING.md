# Linggen Memory Artifact Signing Setup

This guide shows how to set up code signing for Linggen updates using `minisign`.

## Why Sign Updates?

Signing ensures that users only install authentic updates from you, preventing malicious actors from distributing tampered versions.

## Setup (One-Time)

### 1. Install Minisign

```bash
# macOS
brew install minisign
```

### 2. Generate Signing Keys

```bash
minisign -G
```

This creates:

- **Private key**: `minisign.key` (keep this secret!)
- **Public key**: `minisign.pub`

**Important**: Back up your private key securely. If lost, you cannot sign updates and users will need to reinstall.

### 3. Secure Your Private Key

We recommend keeping your key in `~/.linggen/linggen.key`.

```bash
mkdir -p ~/.linggen
mv minisign.key ~/.linggen/linggen.key
chmod 600 ~/.linggen/linggen.key
```

## Usage

### Manual Releases

When running `./scripts/release.sh`, the script will automatically sign artifacts if it finds a key.

**Option A: Environment variable (recommended for CI)**

```bash
export SIGNING_KEY_PASSWORD="your-password"  # if key is password-protected
./scripts/release.sh v0.7.0
```

**Option B: Key file (easier for local releases)**

The script auto-detects `~/.linggen/linggen.key`.

## Verification

After signing, the script:

1. Creates `<artifact>.sig` (signature file)
2. Uploads it to GitHub releases
3. Includes signature in `manifest.json`

The CLI verifies these signatures using the public key compiled into the binary.

## Security Best Practices

✅ **Do:**

- Keep private key secure (never commit to git)
- Use password protection for extra security
- Back up your key securely
- Use environment variables in CI/CD

❌ **Don't:**

- Share your private key
- Commit keys to version control
- Leave keys in plain text on shared systems
