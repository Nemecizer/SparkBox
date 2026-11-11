# Prepare a public macOS release

Nothing in the build workflow creates a GitHub repository or uploads a release. Keep publication as a separate, explicit action after reviewing the prepared files.

## Complete source review

Choose a project license, review THIRD_PARTY_NOTICES.md and the generated third-party inventory, and verify a fresh build. Confirm README, version numbers and release notes accurately describe the prerelease. Keep .git history limited to this reviewed public tree. Exclude .env files, signing keys, tokens, VPN profiles, app state, build caches and private reports. The project license must not replace third-party notices.

Before building, follow [Distribution privacy and first launch](DISTRIBUTION-PRIVACY.md). The candidate script runs the publication guard, but a separate secret scan, history review and final archive inspection remain required.

## Developer ID signing

For distribution outside the Mac App Store, obtain a **Developer ID Application** certificate from your Apple Developer account and install it with its private key in your Mac's Keychain. **Apple Development** and **Apple Distribution** are different certificate types and do not satisfy this workflow.

List names only, without exporting keys:

```sh
security find-identity -v -p codesigning
```

Set the exact Developer ID Application identity shown by that command:

```sh
export SPARKBOX_SIGN_IDENTITY='Developer ID Application: YOUR NAME (TEAM ID)'
python3 scripts/public-release.py preflight
python3 scripts/public-release.py build public-v0.1.0
```

Preflight is read-only. Build requires a clean Git tree, a selected project license and the exact available certificate. It creates a separate app and ZIP; it never launches the app or submits it for notarization. It clears Tauri's automatic notarization environment variables for that child process so submission remains a separate step. Retain PROVENANCE.json, source commit and SHA-256.

## Prepare notarization authentication

Run this yourself in Terminal to store a named profile in Keychain. Follow the interactive prompts; do not paste passwords, API keys or private-key contents into source files or chat:

```sh
xcrun notarytool store-credentials sparkbox-notary
```

Use your Apple account's supported authentication method. The release script references only the profile name. It never exports the stored credentials.

## Submit, staple and verify

Submission sends the prepared binary to Apple. Run it only when you explicitly intend that external action:

```sh
python3 scripts/public-release.py notarize \
  ../SparkBox-builds/public-v0.1.0 \
  ../SparkBox-builds/public-v0.1.0-notarized \
  --profile sparkbox-notary --submit
```

The script checks the Developer ID signature, hardened runtime, trusted timestamp, lack of debug entitlements, provenance and archive checksum. It waits for Apple's response. Only **Accepted** allows it to copy the app into a new destination, staple and validate the ticket, verify Gatekeeper assessment, recreate the final ZIP and write fresh hashes. Rejected or incomplete submissions retain their response and stop. Existing candidates are never stapled or overwritten in place.

The final notarized ZIP, checksum and provenance must correspond to the exact source commit. Before public release, perform a separately authorized install test from a fresh download on a suitable Mac. No build or notarization result proves cluster functionality.

## Later GitHub publication

After approval, create the public repository in your account, push only this reviewed history, and create a version tag at the tested source commit. In **Releases → Draft a new release**, attach the final notarized ZIP, SHA256SUMS and release notes; mark the first release as a prerelease. GitHub supplies source archives for the tag. Never put a signing certificate private key or notarization password in the repository.

Primary references: [Apple Developer ID](https://developer.apple.com/developer-id/), [Apple notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow), [GitHub licensing](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/licensing-a-repository), [GitHub releases](https://docs.github.com/en/repositories/releasing-projects-on-github/managing-releases-in-a-repository).

## Build-path privacy

The public release builder remaps the local home directory in Rust compiler output to `/build/home` and the checkout directory to `/build/sparkbox`, including dependency paths, while preserving existing compiler flags. Go already uses `-trimpath`. Candidate validation refuses a bundle that still contains the current local home-directory prefix before notarization submission. Keep the independent private-identifier and secret scans: path remapping does not replace them.
