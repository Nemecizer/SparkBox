# Third-party notices

SparkBox combines original application code with dependencies and bundled upstream material. The project license, once selected, covers only original SparkBox code.

- resources/playbooks/LICENSE and LICENSE-3rd-party preserve NVIDIA DGX Spark Playbooks licensing and upstream notices. The bundled source documents retain their provenance and content hashes. SparkBox is not an NVIDIA product or endorsement.
- tunnel/netstack/LICENSE preserves the vendored network-stack notice.
- package-lock.json, src-tauri/Cargo.lock and tunnel/go.mod/go.sum pin frontend, Rust and Go dependencies.
- third-party/ contains a generated dependency inventory and copied license/notice files from the resolved packages. Its README records scope and any missing notices. Keep this directory alongside redistributed source and binaries.

The application downloads neither model weights nor container images as part of its build. Models, engines and third-party services have their own terms when a user elects to use them. No such content or credentials are bundled into this export.
