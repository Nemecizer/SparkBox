configHash = sha256(agent.json || 0x00 || rules.json || 0x00 || notify.json), lowercase hex, over the
exact bytes of the three files as installed (no parsing, no newline normalisation: rules.json here has
no trailing newline, notify.json has one, and both carry non-ASCII). expected.txt holds the hex and a
newline. The Mac (agent::config_hash) computes it when it pushes; the agent reports it in status,
beat.json and /healthz; they must agree for "settings current".
