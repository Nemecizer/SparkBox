# Distribution privacy and first launch

The public source starts a new history. It excludes the maintainer's credentials, saved application data, real network profiles, personal acceptance logs and prior build folders. SSH test servers generate ephemeral private keys in memory. There are no bundled SSH private keys.

Demo pictures, Help and test fixtures contain synthetic device names, user names, IPv4/IPv6 addresses, MAC addresses and fingerprints. These are examples, not settings for a real network. Localhost, protocol constants and official service URLs remain necessary. Redaction tests deliberately contain fake token-shaped strings so that masking is tested realistically; they are not usable credentials. Third-party license notices retain their required public attribution.

The Developer ID signature exposes the publisher's certificate name and Apple team identifier. The application identifier is also public. Signing does not distribute the private signing key or the Keychain notarization profile.

## Fresh installation

With no existing SparkBox configuration, the native app starts in **Demo** and **Read-only**, with zero nodes and zero switches. It has no VPN profile or forwards, no discovered model endpoints and no automatic AI hardware lookup. The local API server does not start automatically and requires a key when enabled. Cloud provider entries contain public service URLs, not API keys.

Use **Connect my Sparks** to enter your own account and nodes. Compare each SSH fingerprint at the intended device before trusting it. Discovery and changes retain their normal consent and authentication requirements.

An update installed by an existing user keeps that user's settings in Application Support and credentials in Keychain. Those files are outside the app bundle. Preparing a distribution does not erase or reset the maintainer's installation. Use a separate macOS account for an actual first-launch acceptance test; do not delete your working configuration to simulate a new user.

## Recheck before publishing

Run `python3 scripts/audit-distribution.py` on a clean source checkout. The candidate build runs the same guard. It rejects common private state paths, embedded private-key blocks and symlinks. If you have known private identifiers, pass a JSON list stored outside the source with `--deny-file /path/to/private-list.json`. Findings identify files without printing sensitive values.

This guard is not a complete secret detector. Also scan the tracked source, reachable Git history, generated web assets, the final extracted app and archive contents. Review Help pictures and fixture data. Keep raw audit logs and private deny lists outside the publishing directory. Do not upload the entire workspace: publish only the reviewed repository history and the selected release files.

The application is not launched as part of packaging. Build, signature and notarization checks do not replace an independently authorized install test or validation on a user's cluster.
