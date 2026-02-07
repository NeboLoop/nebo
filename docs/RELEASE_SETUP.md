# Release Pipeline Setup Checklist

Steps to complete before the CI/CD release pipeline can run.

---

## 1. Create `TAP_GITHUB_TOKEN` (required)

Fine-grained Personal Access Token for pushing to distribution repos.

1. Go to https://github.com/settings/tokens?type=beta
2. Click **Generate new token**
3. Configure:
   - **Name:** `nebo-release-bot`
   - **Expiration:** 1 year
   - **Resource owner:** `nebolabs`
   - **Repository access:** Select repositories → `homebrew-tap` and `apt`
   - **Permissions → Repository permissions → Contents:** Read and write
4. Generate and copy the token
5. Add it as a secret on the nebo repo:
   ```bash
   gh secret set TAP_GITHUB_TOKEN
   # Paste the token when prompted
   ```

---

## 2. Enable GitHub Pages on `nebolabs/apt` (required)

The APT repository is served via GitHub Pages.

1. Go to https://github.com/nebolabs/apt/settings/pages
2. Under **Source**, select **Deploy from a branch**
3. Set branch to `main`, folder to `/ (root)`
4. Save

After the first release, the APT repo will be available at `https://nebolabs.github.io/apt/`.

---

## 3. Create `APT_GPG_PRIVATE_KEY` (optional, recommended)

GPG key for signing APT packages. Without this, packages are unsigned and users must add `[trusted=yes]` to their sources list.

```bash
# Generate key
gpg --full-generate-key
# Choose: RSA and RSA, 4096 bits, 0 (no expiry)
# Real name: Nebo
# Email: support@nebolabs.dev

# Set as repo secret
gpg --export-secret-keys --armor nebo | gh secret set APT_GPG_PRIVATE_KEY

# Export public key and add to the apt repo
gpg --armor --export nebo > /tmp/key.gpg
# Upload key.gpg to nebolabs/apt repo root
```

---

## 4. Test the pipeline

Once the above steps are done, trigger a release:

```bash
git tag v0.1.2
git push origin v0.1.2
```

Monitor at https://github.com/nebolabs/nebo/actions

The pipeline will: build all 5 platform binaries (desktop mode) → package .deb files → create GitHub Release → update Homebrew formula → update APT repository.
