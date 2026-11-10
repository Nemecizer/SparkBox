# SparkBox contributor instructions

Read README.md, docs/BUILDING.md, docs/RELEASING.md and SECURITY.md before making changes.

- Default to offline unit tests, fake nodes and browser Demo. Do not launch a native app, operate a cluster, unlock Admin, run inference or publish a release without the owner's explicit task authorization.
- Preserve Admin/Touch ID, Look only, SSH identity checks, controller leases, instance ownership, protected-update exact plans and the debug write fence. Do not bypass gates to get a test to pass.
- Keep secrets in Keychain and protected stdin paths. Never commit credentials, app state, private keys or real network profiles.
- New installations must have no example network endpoints. Inventory::example is explicit fixture data; it must never become a missing/corrupt saved inventory fallback.
- Update applicable Help and tips for visible changes. Follow docs/help/README.md and docs/help/STYLE.md. Use existing semantic colors and American spelling.
- Run targeted checks while iterating and relevant full checks at completion. scripts/check.sh covers TypeScript, UI, Rust debug/release safeguards, Python and Go. Real Linux/systemd/GPU/RDMA acceptance is separate. Existing Clippy warnings must not increase.
- Use a clean committed tree for scripts/build-candidate.sh. It creates a separate candidate and never launches it. For public distribution use scripts/public-release.py, which requires Developer ID Application; Apple Development is insufficient.
- Never publish, push, create a remote or submit for notarization as an incidental side effect. The notarization command requires an explicit --submit option and stored Keychain profile.

This public tree contains no private handoff history. Preserve historical fixture assertions; recorded test values do not establish support for a user's hardware or permission to mutate it.
