# Rust artifact publication

`publish_csv(destination, bytes, overwrite)` publishes already-generated UTF-8 BOM
CSV. `publish_bytes` provides the bounded byte primitive. This is a standalone
Rust port; the Fusion command adapter and operation-journal integration are pending.

Publication writes and syncs a fresh staging file on the destination filesystem.
An exclusive hardlink commits when replacement is disabled. An explicit replace
uses the platform's rename primitive. Neither path truncates an existing file.
The receipt includes the SHA-256 and size of the supplied bytes. Acknowledged
publication remains successful if subsequent staging cleanup fails.

Errors before the commit syscall have `publication: NotAttempted`. A commit
syscall error has `publication: Unknown`, requiring reconciliation rather than an
automatic retry. This distinction also covers lost acknowledgements on network
filesystems. File publication is an external effect and cannot be undone by a
Fusion model transaction.

Output is limited to 16 MiB and must be nonempty. CSV additionally requires UTF-8
and a `.csv` extension. Paths must be absolute and contain no parent traversal.
Existing directory/link/reparse-point destinations are refused. Windows device,
alternate-stream and ambiguous reserved names are refused before creating parents.
Missing parent directories may be created before publication.

Staging is private: Unix mode0700; Windows atomically assigned protected
current-user owner/DACL with opened-handle readback. Windows impersonation and
alternate process-token owners are unsupported. The published file retains its
private staging permissions; this API does not configure sharing permissions.
No caller configuration or existing ACL is repaired.

Scope excludes a hostile process replacing ancestor paths, SDK export-manager
calls, arbitrary caller-owned staging paths, sidecar bundles, and a power-loss
durability guarantee. Parent canonicalization and the current-user ACL do not
turn a path-based API into a capability filesystem. Unexpected staging children
are retained with a cleanup warning instead of recursively deleted.

Verification: **16 Linux tests and 18 Windows MSVC tests passed**, with matching
source hashes. Tests cover real filesystem replacement/exclusivity, eight
concurrent writers, hardlink alias preservation, bounds, precommit fault
injection, postcommit warnings, unexpected child preservation, Windows readonly
files, special paths, and private staging under an explicitly shared parent ACL.
The tests inject boundary failures; they do not claim an actual full-disk or
power-loss experiment. Raw logs and bindings are in `evidence/`.

Run from this directory: `cargo test --locked --offline`.

