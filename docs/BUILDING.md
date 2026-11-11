# Build SparkBox from source

## Prerequisites

Use an Apple Silicon Mac with Xcode and its command-line developer tools, Node/npm, Rust/Cargo, Go and Python 3.12 or newer. Ensure those tools are on PATH. The maintainer's observed toolchain is Node 26.13.0, npm 11.20.0, Rust/Cargo 1.99.1, Go 1.27.3 and Python 3.15.2; these are recorded versions, not a promise that every older combination works. The Go module declares its required toolchain in tunnel/go.mod. The system /usr/bin/python3 may be too old.

Offline TLS fixtures also need an OpenSSL installation that generates X.509 v3 certificates; the verified toolchain used Homebrew OpenSSL 4.1.0. The system LibreSSL 3.3.6 generated a v1 certificate for that fixture and was rejected correctly. On a Homebrew Apple Silicon setup, use `PATH=/opt/homebrew/bin:$PATH` for every check, including direct test-binary runners, and confirm `python3 --version` and `openssl version` first. This selects existing tools; it does not change the system installations.

Locked dependency manifests are package-lock.json, src-tauri/Cargo.lock and tunnel/go.mod/go.sum. Initial dependency installation needs internet access to the package registries. No cluster or paid AI service is needed to compile.

## Compile without launching

From the source directory:

```sh
npm ci
npm run build
npm run build:app -- --no-bundle
```

The last command builds the Go VPN helper, production web assets and native release executable. It runs release Help checks. It does not launch SparkBox. For local app bundling use `npm run build:app`; this writes inside this checkout's src-tauri/target. Use a separate checkout when preserving an existing build.

To create a separate locally signed candidate from a clean Git commit:

```sh
scripts/build-candidate.sh local-candidate
```

That script needs an available Apple code-signing identity. It preserves existing candidate folders and produces an app, ZIP and provenance in ../SparkBox-builds/local-candidate. Local Apple Development signing does not produce a notarized public release. Follow RELEASING.md for distribution.

## Verify offline

```sh
npm run check
npm run build
VITE_HELP_RELEASE=1 npx vitest run src/help/help.test.ts
(cd src-tauri && cargo clippy --lib)
python3 -m unittest discover -s scripts -p '*_test.py'
python3 scripts/audit-distribution.py
```

The default native test worker count is one because fake-node tests spawn local processes. Tests need permission to create temporary files, spawn processes and bind loopback sockets. Platform-specific Linux/systemd cases may skip on macOS; report those limits. Never substitute a real node for a fake-node fixture.

For an independent fresh-copy proof, clone this local repository into an empty folder, run npm ci there, and use a new CARGO_TARGET_DIR. Package registries and toolchain caches may be reused; node_modules, target artifacts and app state must not be copied from the maintainer checkout. See the preparation evidence supplied alongside this source archive for the actual results.

The native debug identity is separate from release and retains its write fence. A native debug launch is a separate action; the offline browser Demo is the default UI inspection method.
